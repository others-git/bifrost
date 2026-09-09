//! Govee cloud API v2.
//!
//! Base URL: `https://openapi.api.govee.com/router/api/v1`
//! Authentication: `Govee-API-Key: <key>` header. Obtain a key from the Govee developer portal.
//!
//! Rate limit: ~10 req/s; 10,000 req/day per API key.

use crate::models::{Color, Light, LightCapabilities, LightState, Provider, SegmentColor};
use crate::providers::LightProvider;
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::{Client, header};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

const BASE_URL: &str = "https://openapi.api.govee.com/router/api/v1";

/// The Govee cloud transport (`openapi.api.govee.com`). One of the two transports
/// the unified [`GoveeProvider`] owns; reached for any device whose commands can't
/// (or shouldn't) go over the LAN — no LAN reply, a LAN send that failed, or an
/// effect/dynamic-scene (LAN has no scene catalogue).
pub struct GoveeCloud {
    client: Client,
    /// Base URL for the API; overridden in tests to point at a wiremock server.
    base_url: String,
    /// The request pacer this endpoint's traffic draws from. Process-wide, so
    /// every rebuilt provider shares one budget (see [`limiter_for`]).
    limiter: Arc<RateLimiter>,
}

/// Process-wide `base_url → (device id → SKU)` cache. Control and state
/// requests REQUIRE the device's SKU (the API answers 400 without it), but the
/// SKU isn't in a control payload, so it must be resolved from the device list.
///
/// The provider is **rebuilt on every control request** (`build_provider`), so a
/// per-instance cache never survived — each command first paid a full
/// `GET /user/devices` round-trip, the reported "laggy controls". A device's SKU
/// is immutable, so caching it for the life of the process is safe. Keying by
/// `base_url` lets production (constant `BASE_URL`) share one cache across every
/// rebuilt provider while tests (each a unique mock URI) stay isolated.
fn sku_cache() -> &'static tokio::sync::RwLock<
    std::collections::HashMap<String, std::collections::HashMap<String, String>>,
> {
    static CACHE: std::sync::OnceLock<
        tokio::sync::RwLock<
            std::collections::HashMap<String, std::collections::HashMap<String, String>>,
        >,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| tokio::sync::RwLock::new(std::collections::HashMap::new()))
}

/// One named dynamic scene ("effect") for a Govee device: the display `name`, the
/// capability `instance` it must be applied under (`lightScene` for the built-in
/// catalog, `diyScene` for the user's DIY scenes), and the opaque `value`
/// (`{id, paramId}`) echoed back verbatim on control.
#[derive(Debug, Clone)]
struct GoveeScene {
    name: String,
    instance: String,
    value: Value,
}

/// Process-wide `base_url → (device id → scenes)` cache. A device's dynamic
/// scenes live behind *separate* `/device/scenes` (+ `/device/diy-scenes`) calls,
/// and each scene's apply `value` is opaque, so we cache them per device — scenes
/// change rarely, and this keeps discovery from paying a scenes round-trip per
/// device on every poll.
type SceneList = Vec<GoveeScene>;
fn scene_cache() -> &'static tokio::sync::RwLock<
    std::collections::HashMap<String, std::collections::HashMap<String, SceneList>>,
> {
    static CACHE: std::sync::OnceLock<
        tokio::sync::RwLock<
            std::collections::HashMap<String, std::collections::HashMap<String, SceneList>>,
        >,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| tokio::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Per-device cloud write gate: one device's command batch at a time, and the
/// newest desired state wins.
///
/// Two problems this solves, both of which read to the user as "the command
/// didn't take". **Serialisation:** Govee needs one request per capability, and
/// firing power/brightness/colour at a device simultaneously is how a burst
/// arrives out of order (or partly rejected) — a batch now holds the gate for
/// the device it targets. **Coalescing:** a brightness drag emits a write every
/// ~200ms, and each one costs several cloud requests, so a two-second drag
/// enqueues far more traffic than the account is allowed. Every writer drops
/// its desired state in `pending` before queueing; whoever gets the gate sends
/// whatever is latest at that moment, and writers that find the slot already
/// taken return without sending. The drag then costs a couple of round-trips
/// ending on the value the finger stopped at, instead of replaying every
/// intermediate frame into a 429.
///
/// Coalescing is only safe because the service layer merges a patch onto the
/// light's cached state before calling the provider (`apply_light_state`), so
/// the newest write is a strict superset of the ones it supersedes — never a
/// partial that would lose the dimension an earlier write moved.
#[derive(Default)]
struct DeviceGate {
    /// Held for one device's whole command batch.
    batch: tokio::sync::Mutex<()>,
    /// The latest desired full state / segment list, or `None` once sent.
    pending_state: tokio::sync::Mutex<Option<LightState>>,
    pending_segments: tokio::sync::Mutex<Option<Vec<SegmentColor>>>,
}

/// The process-wide gate for one `(base_url, device)`. Process-lived because the
/// provider is rebuilt per request, so a per-instance gate would serialise
/// nothing.
fn device_gate(base_url: &str, device_id: &str) -> Arc<DeviceGate> {
    static MAP: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Arc<DeviceGate>>>> =
        std::sync::OnceLock::new();
    let map = MAP.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut map = map.lock().expect("Govee device-gate map poisoned");
    map.entry(format!("{base_url}|{device_id}"))
        .or_default()
        .clone()
}

/// Clear the process-wide SKU + scene caches. Tests only — the caches are keyed by
/// `base_url`, and wiremock reuses ephemeral ports across tests, so a stale entry
/// from a prior (dropped) mock server could otherwise satisfy a fresh lookup.
#[cfg(test)]
async fn clear_sku_cache() {
    sku_cache().write().await.clear();
    scene_cache().write().await.clear();
}

impl GoveeCloud {
    pub fn new(api_key: impl AsRef<str>) -> Result<Self> {
        Self::new_with_base(api_key, BASE_URL)
    }

    fn new_with_base(api_key: impl AsRef<str>, base_url: impl Into<String>) -> Result<Self> {
        // Shared, pooled client keyed by API key (not base URL) so test wiremock
        // servers on ephemeral ports share correctly; the base URL lives on the
        // struct, not the client. See [`crate::providers::cached_client`].
        let api_key = api_key.as_ref();
        let client = crate::providers::cached_client(&format!("govee:{api_key}"), || {
            let mut headers = header::HeaderMap::new();
            headers.insert("Govee-API-Key", header::HeaderValue::from_str(api_key)?);
            // Bounded so a cloud outage fails the poll fast instead of hanging it.
            Ok(Client::builder()
                .default_headers(headers)
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(30))
                .build()?)
        })?;
        let base_url = base_url.into();
        Ok(Self {
            limiter: limiter_for(&format!("{base_url}|{api_key}")),
            client,
            base_url,
        })
    }

    /// Fetch the account's device list (shared by discovery and SKU lookup).
    async fn fetch_devices(&self) -> Result<Vec<GoveeDevice>> {
        let resp: GoveeResponse<GoveeDeviceList> = self
            .send(self.client.get(format!("{}/user/devices", self.base_url)))
            .await
            .context("Govee devices request failed")?
            .error_for_status()?
            .json()
            .await?;

        if resp.code != 200 {
            bail!("Govee API error {}: {}", resp.code, resp.message);
        }
        Ok(resp
            .body()
            .map(GoveeDeviceList::into_vec)
            .unwrap_or_default())
    }

    /// Dev-mode debug: the account's devices with their **full** capability list,
    /// flagging each capability as supported (we model it) or not. Surfaces the
    /// Govee features we don't build yet — segments (`segmentedColorRgb`), music
    /// mode, gradient, nightlight, snapshot, etc. — so we can see what a strip
    /// actually exposes before building.
    async fn debug_devices(&self) -> Result<Value> {
        const SUPPORTED: &[&str] = &[
            "powerSwitch",
            "brightness",
            "colorRgb",
            "colorTemperatureK",
            "lightScene",
            "diyScene",
            "online",
            "dynamic_scene",
            "segmentedColorRgb",
            "segmentedBrightness",
        ];
        let devices = self.fetch_devices().await?;
        let report: Vec<Value> = devices
            .iter()
            .map(|d| {
                let caps: Vec<Value> = d
                    .capabilities
                    .iter()
                    .map(|c| {
                        json!({
                            "instance": c.instance,
                            "type": c.cap_type,
                            "supported": SUPPORTED.contains(&c.instance.as_str()),
                        })
                    })
                    .collect();
                let unsupported: Vec<&str> = d
                    .capabilities
                    .iter()
                    .map(|c| c.instance.as_str())
                    .filter(|i| !SUPPORTED.contains(i))
                    .collect();
                json!({
                    "name": d.device_name,
                    "sku": d.sku,
                    "device": d.device,
                    "capabilities": caps,
                    "unsupported_capabilities": unsupported,
                })
            })
            .collect();
        Ok(json!({ "devices": report }))
    }

    /// The SKU for a device — required by control/state payloads. Served from
    /// the process-wide [`sku_cache`]; on a miss, one device-list fetch
    /// populates every device's SKU so subsequent commands are round-trip-free.
    async fn sku_for(&self, device_id: &str) -> Result<String> {
        if let Some(sku) = sku_cache()
            .read()
            .await
            .get(&self.base_url)
            .and_then(|m| m.get(device_id))
            .cloned()
        {
            return Ok(sku);
        }
        let devices = self.fetch_devices().await?;
        {
            let mut cache = sku_cache().write().await;
            let entry = cache.entry(self.base_url.clone()).or_default();
            for d in &devices {
                entry.insert(d.device.clone(), d.sku.clone());
            }
        }
        devices
            .into_iter()
            .find(|d| d.device == device_id)
            .map(|d| d.sku)
            .ok_or_else(|| anyhow::anyhow!("unknown Govee device '{device_id}'"))
    }

    /// POST one scene endpoint (`device/scenes` or `device/diy-scenes`) and
    /// flatten its options into [`GoveeScene`]s, each tagged with its instance.
    async fn fetch_scenes(&self, endpoint: &str, sku: &str, device_id: &str) -> Result<SceneList> {
        let body = json!({
            "requestId": Uuid::new_v4().to_string(),
            "payload": { "sku": sku, "device": device_id }
        });
        let resp: GoveeResponse<GoveeSceneData> = self
            .send(
                self.client
                    .post(format!("{}/{endpoint}", self.base_url))
                    .json(&body),
            )
            .await?
            .error_for_status()?
            .json()
            .await?;
        if resp.code != 200 {
            bail!("Govee scenes error {}: {}", resp.code, resp.message);
        }
        Ok(resp
            .body()
            .map(GoveeSceneData::into_scenes)
            .unwrap_or_default())
    }

    /// The device's full dynamic-scene catalog (built-in `lightScene` + the user's
    /// `diyScene`s), served from the process-wide [`scene_cache`]; on a miss, the
    /// two scene endpoints populate it. Scenes change rarely, so this is
    /// effectively a one-time cost.
    async fn scenes_for(&self, device_id: &str) -> Result<SceneList> {
        if let Some(scenes) = scene_cache()
            .read()
            .await
            .get(&self.base_url)
            .and_then(|m| m.get(device_id))
            .cloned()
        {
            return Ok(scenes);
        }
        let sku = self.sku_for(device_id).await?;
        // Built-in scenes are required; DIY scenes are best-effort (an account
        // without any, or a model that doesn't support DIY, must not fail this).
        let mut scenes = self.fetch_scenes("device/scenes", &sku, device_id).await?;
        if let Ok(diy) = self
            .fetch_scenes("device/diy-scenes", &sku, device_id)
            .await
        {
            scenes.extend(diy);
        }
        scene_cache()
            .write()
            .await
            .entry(self.base_url.clone())
            .or_default()
            .insert(device_id.to_string(), scenes.clone());
        Ok(scenes)
    }

    /// Send a request under the shared [`RateLimiter`], retrying on rate-limit
    /// (429) and transient/server errors.
    ///
    /// The limiter is the primary defence — it keeps the burst from being sent
    /// at all — and the retry is what covers the residue: another Bifrost
    /// instance, the Govee phone app, or a per-device limit we can't see from
    /// here. A 429 parks the whole bucket rather than only this call, so the
    /// other capability commands of the same gesture slow down with it instead
    /// of piling straight into the same wall.
    async fn send(&self, req: reqwest::RequestBuilder) -> reqwest::Result<reqwest::Response> {
        const MAX_RETRIES: u32 = 4;
        let mut attempt = 0u32;
        loop {
            // JSON / empty bodies are always cloneable; unwrap is safe here.
            let this = req
                .try_clone()
                .expect("Govee request body must be cloneable");
            self.limiter.acquire().await;
            match this.send().await {
                Ok(resp) => {
                    let limited = resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS;
                    let retryable = limited || resp.status().is_server_error();
                    if limited {
                        let wait = retry_wait(&resp).unwrap_or_else(|| backoff(attempt));
                        tracing::debug!(
                            target: "bifrost::govee",
                            attempt,
                            wait_ms = wait.as_millis() as u64,
                            "cloud rate-limited (429) — parking every caller",
                        );
                        // Parking makes the next `acquire` wait, so no extra sleep here.
                        self.limiter.park(wait).await;
                    }
                    if retryable && attempt < MAX_RETRIES {
                        if !limited {
                            tokio::time::sleep(backoff(attempt)).await;
                        }
                        attempt += 1;
                        continue;
                    }
                    return Ok(resp);
                }
                // Transient transport errors (timeout / connect) also get a retry.
                Err(e) if attempt < MAX_RETRIES && (e.is_timeout() || e.is_connect()) => {
                    attempt += 1;
                    tokio::time::sleep(backoff(attempt)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Send one capability command to a device (`/device/control`). The unit of
    /// Govee control — `set_state` and `set_segments` issue several of these in
    /// sequence. [`GoveeCloud::send`] handles pacing and 429s.
    async fn send_control(&self, sku: &str, device: &str, capability: Value) -> Result<()> {
        let body = json!({
            "requestId": Uuid::new_v4().to_string(),
            "payload": { "sku": sku, "device": device, "capability": capability }
        });
        let resp: GoveeResponse<Value> = self
            .send(
                self.client
                    .post(format!("{}/device/control", self.base_url))
                    .json(&body),
            )
            .await?
            .error_for_status()?
            .json()
            .await?;
        if resp.code != 200 {
            bail!("Govee control error {}: {}", resp.code, resp.message);
        }
        Ok(())
    }

    /// Test constructor: points at a local HTTP mock server instead of the Govee cloud.
    #[cfg(test)]
    pub fn new_for_test(base_url: impl Into<String>, api_key: impl AsRef<str>) -> Result<Self> {
        Self::new_with_base(api_key, base_url)
    }
}

// ── Cloud request pacing ────────────────────────────────────────────────────

/// Sustained cloud request rate, and the burst allowed above it.
///
/// Govee's cloud publishes ~10 req/s per account (plus a daily cap and tighter
/// per-device limits). Every capability is a separate request — "turn on, dim to
/// 40%, go blue" is three — so one room command, or one brightness drag, clears
/// 10/s without trying. Discovering that per-request and retrying doesn't help:
/// the retry re-sends into the same burst. Instead every cloud request draws
/// from ONE process-wide budget, which turns a burst into a *paced* sequence.
const RATE_PER_SEC: f64 = 5.0;
const RATE_BURST: f64 = 10.0;

/// A token bucket shared by every [`GoveeCloud`] talking to the same endpoint.
///
/// Two jobs. It paces the steady stream (a drag, a room fan-out, a poll sweep)
/// below the account limit, and — via [`RateLimiter::park`] — it lets a single
/// 429 slow down *every* in-flight caller instead of only the unlucky one that
/// received it. Callers queue on the mutex in arrival order (tokio's mutex is
/// FIFO), so pacing never reorders commands for a device.
struct RateLimiter {
    state: tokio::sync::Mutex<BucketState>,
}

struct BucketState {
    tokens: f64,
    last: Instant,
    /// Set when the server said "too many": nothing leaves until it passes.
    hold_until: Option<Instant>,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            state: tokio::sync::Mutex::new(BucketState {
                tokens: RATE_BURST,
                last: Instant::now(),
                hold_until: None,
            }),
        }
    }

    /// Wait until one request may be sent, then spend its token.
    async fn acquire(&self) {
        // The guard is deliberately held across the sleeps: it's what queues
        // waiting callers in order and keeps the refill arithmetic atomic.
        let mut s = self.state.lock().await;
        loop {
            let now = Instant::now();
            let elapsed = now.saturating_duration_since(s.last).as_secs_f64();
            s.tokens = (s.tokens + elapsed * RATE_PER_SEC).min(RATE_BURST);
            s.last = now;

            if let Some(until) = s.hold_until {
                if until > now {
                    tokio::time::sleep(until - now).await;
                    continue;
                }
                s.hold_until = None;
            }
            if s.tokens >= 1.0 {
                s.tokens -= 1.0;
                return;
            }
            let deficit = 1.0 - s.tokens;
            tokio::time::sleep(Duration::from_secs_f64(deficit / RATE_PER_SEC)).await;
        }
    }

    /// A 429 (or an explicit `Retry-After`) parks the whole bucket: the account
    /// is over its limit, so every other caller must back off too, not just this
    /// request's retry loop.
    async fn park(&self, wait: Duration) {
        let mut s = self.state.lock().await;
        let until = Instant::now() + wait;
        if s.hold_until.is_none_or(|t| t < until) {
            s.hold_until = Some(until);
        }
        s.tokens = 0.0;
        s.last = Instant::now();
    }
}

/// The process-wide limiter for one endpoint+key. Production uses the constant
/// [`BASE_URL`], so every rebuilt provider for an account shares one budget;
/// tests each get their own (their mock URI is unique).
fn limiter_for(key: &str) -> Arc<RateLimiter> {
    static MAP: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Arc<RateLimiter>>>> =
        std::sync::OnceLock::new();
    let map = MAP.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut map = map.lock().expect("Govee limiter map poisoned");
    map.entry(key.to_string())
        .or_insert_with(|| Arc::new(RateLimiter::new()))
        .clone()
}

/// Exponential backoff: ~250ms, 500ms, 1s, 2s.
fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(250u64 << attempt.min(3))
}

/// How long the server asked us to wait, if it said so.
///
/// `Retry-After` is the standard answer; Govee also stamps rate-limit state on
/// its responses, and the reset value has been seen both as a delta and as an
/// absolute unix timestamp — a value bigger than a year of seconds can only be
/// an epoch, so treat it as one. Clamped, so a bogus header can't wedge control.
fn retry_wait(resp: &reqwest::Response) -> Option<Duration> {
    retry_wait_from(resp.headers())
}

/// [`retry_wait`] over a bare header map, so it's testable without a live response.
fn retry_wait_from(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    const MAX_WAIT: i64 = 10;
    let read = |name: &str| -> Option<i64> {
        headers.get(name)?.to_str().ok()?.trim().parse::<i64>().ok()
    };
    let secs = read("retry-after")
        .or_else(|| reset_delta(read("x-ratelimit-reset")))
        .or_else(|| reset_delta(read("api-ratelimit-reset")))?;
    Some(Duration::from_secs(secs.clamp(0, MAX_WAIT) as u64))
}

/// A rate-limit `*-Reset` value as seconds-from-now, whichever form it arrived in.
fn reset_delta(value: Option<i64>) -> Option<i64> {
    const ONE_YEAR_SECS: i64 = 31_536_000;
    let v = value?;
    Some(if v > ONE_YEAR_SECS {
        v - Utc::now().timestamp()
    } else {
        v
    })
}

// ── Wire types ─────────────────────────────────────────────────────────────

/// The live API answers `{"requestId", "msg", "code", "data" | "payload"}`;
/// older documentation showed `"message"` and only `"data"`. Accept both.
#[derive(Debug, Deserialize)]
struct GoveeResponse<T> {
    code: i32,
    #[serde(alias = "msg", default)]
    message: String,
    #[serde(default = "Option::default")]
    data: Option<T>,
    #[serde(default = "Option::default")]
    payload: Option<T>,
}

impl<T> GoveeResponse<T> {
    fn body(self) -> Option<T> {
        self.data.or(self.payload)
    }
}

/// `/user/devices` returns `data` as a bare array on the live API;
/// older docs wrapped it as `{"devices": [...]}`. Accept both.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum GoveeDeviceList {
    Bare(Vec<GoveeDevice>),
    Wrapped { devices: Vec<GoveeDevice> },
}

impl GoveeDeviceList {
    fn into_vec(self) -> Vec<GoveeDevice> {
        match self {
            Self::Bare(v) => v,
            Self::Wrapped { devices } => devices,
        }
    }
}

#[derive(Debug, Deserialize)]
struct GoveeDevice {
    /// Model SKU — REQUIRED by control/state request payloads.
    sku: String,
    device: String, // MAC-style device id
    #[serde(rename = "deviceName")]
    device_name: String,
    #[serde(default)]
    capabilities: Vec<GoveeCapability>,
}

#[derive(Debug, Deserialize)]
struct GoveeCapability {
    instance: String,
    /// The capability category (`devices.capabilities.*`). Unused in control —
    /// captured for dev-mode debug (to see what a device exposes vs what we model).
    #[serde(rename = "type", default)]
    cap_type: Option<String>,
    /// Capability shape descriptor. We only read it for `segmentedColorRgb` (to
    /// learn the segment count); other capabilities' parameters parse to an empty
    /// struct and are ignored (serde drops unknown fields).
    #[serde(default)]
    parameters: Option<GoveeCapParameters>,
}

/// Minimal view of a capability's `parameters` — just the STRUCT `fields` we need
/// to size a segmented capability. Permissive: any `parameters` shape that lacks
/// `fields` (range caps, scene `options`, …) yields an empty list.
#[derive(Debug, Deserialize, Default)]
struct GoveeCapParameters {
    #[serde(default)]
    fields: Vec<GoveeCapField>,
}

#[derive(Debug, Deserialize)]
struct GoveeCapField {
    #[serde(rename = "fieldName", default)]
    field_name: String,
    /// For an Array field: the per-element value range (segment indices `0..=max`).
    #[serde(rename = "elementRange", default)]
    element_range: Option<GoveeRange>,
    /// For an Array field: the allowed length range.
    #[serde(default)]
    size: Option<GoveeRange>,
}

#[derive(Debug, Deserialize, Clone, Copy)]
struct GoveeRange {
    #[serde(default)]
    max: i64,
}

/// Segment count from a device's `segmentedColorRgb` capability, if it has one:
/// the `segment` array field's element range gives indices `0..=max`, so the count
/// is `max + 1`. Falls back to the array's max length. `None` = no segment control.
fn segment_count(caps: &[GoveeCapability]) -> Option<u16> {
    let seg = caps
        .iter()
        .find(|c| c.instance == "segmentedColorRgb")?
        .parameters
        .as_ref()?
        .fields
        .iter()
        .find(|f| f.field_name == "segment")?;
    let n = seg
        .element_range
        .map(|r| r.max + 1)
        .or_else(|| seg.size.map(|r| r.max))?;
    u16::try_from(n.clamp(0, u16::MAX as i64))
        .ok()
        .filter(|n| *n > 0)
}

#[derive(Debug, Deserialize)]
struct GoveeStateData {
    capabilities: Vec<GoveeStateCapability>,
}

#[derive(Debug, Deserialize)]
struct GoveeStateCapability {
    instance: String,
    state: Value,
}

/// `/device/scenes` answers with the device's `dynamic_scene` capabilities, each
/// carrying a list of named scene `options` whose `value` is the opaque token to
/// echo back on control.
#[derive(Debug, Deserialize)]
struct GoveeSceneData {
    #[serde(default)]
    capabilities: Vec<GoveeSceneCapability>,
}

#[derive(Debug, Deserialize)]
struct GoveeSceneCapability {
    instance: String,
    #[serde(default)]
    parameters: GoveeSceneParameters,
}

#[derive(Debug, Deserialize, Default)]
struct GoveeSceneParameters {
    #[serde(default)]
    options: Vec<GoveeSceneOption>,
}

#[derive(Debug, Deserialize)]
struct GoveeSceneOption {
    name: String,
    value: Value,
}

impl GoveeSceneData {
    /// Flatten the dynamic-scene capabilities into [`GoveeScene`]s, tagging each
    /// with the capability instance it must be applied under (`lightScene` for the
    /// built-in catalog, `diyScene` for DIY). Non-scene capabilities are ignored.
    fn into_scenes(self) -> SceneList {
        self.capabilities
            .into_iter()
            .filter(|c| c.instance == "lightScene" || c.instance == "diyScene")
            .flat_map(|c| {
                let instance = c.instance;
                c.parameters.options.into_iter().map(move |o| GoveeScene {
                    name: o.name,
                    instance: instance.clone(),
                    value: o.value,
                })
            })
            .collect()
    }
}

// ── Conversion helpers ──────────────────────────────────────────────────────

fn govee_device_to_light(d: GoveeDevice, state: Option<LightState>) -> Light {
    let has_color = d.capabilities.iter().any(|c| c.instance == "colorRgb");
    let has_color_temp = d
        .capabilities
        .iter()
        .any(|c| c.instance == "colorTemperatureK");
    let has_dim = d.capabilities.iter().any(|c| c.instance == "brightness");
    let segments = segment_count(&d.capabilities);

    Light {
        id: Uuid::new_v4(),
        // Govee's `device` id is the unit's MAC — our cross-provider de-dup key.
        hw_id: crate::providers::mac_hw_id(&d.device),
        provider_id: d.device,
        provider: Provider::Govee,
        name: d.device_name,
        state: state.unwrap_or_default(),
        capabilities: LightCapabilities {
            dimmable: has_dim,
            color_rgb: has_color,
            color_temperature: has_color_temp,
            hue_gamut: None,
            effects: Vec::new(),
            segments,
        },
        last_seen: Utc::now(),
    }
}

/// The live API wraps each capability state as `{"value": ...}`; older docs
/// showed the bare value. Accept both.
fn cap_value(state: &Value) -> &Value {
    state.get("value").unwrap_or(state)
}

fn parse_govee_state(caps: Vec<GoveeStateCapability>) -> LightState {
    let mut state = LightState::default();
    for cap in caps {
        let v = cap_value(&cap.state);
        match cap.instance.as_str() {
            "online" => {
                // false as a bool or the string "false" — the live API has
                // been seen returning both.
                let online = v.as_bool().or_else(|| v.as_str().map(|s| s == "true"));
                state.reachable = online;
            }
            "powerSwitch" => {
                state.on = v.as_u64().unwrap_or(0) == 1;
            }
            "brightness" => {
                if let Some(b) = v.as_u64() {
                    state.brightness = Some(b as f32);
                }
            }
            "colorRgb" => {
                if let Some(rgb) = v.as_u64() {
                    let r = ((rgb >> 16) & 0xFF) as u8;
                    let g = ((rgb >> 8) & 0xFF) as u8;
                    let b = (rgb & 0xFF) as u8;
                    state.color = Some(Color::from_rgb(r, g, b));
                }
            }
            "colorTemperatureK" => {
                // 0 Kelvin means "not in color-temperature mode" — skip it.
                if let Some(k) = v.as_u64().filter(|k| *k > 0) {
                    state.color_temp_mirek = Some(crate::models::kelvin_to_mirek(k as u32));
                }
            }
            _ => {}
        }
    }
    // The cloud API reports the *last known* power state for offline devices.
    // An unreachable light isn't emitting anything — report it as off.
    if state.reachable == Some(false) {
        state.on = false;
    }
    state
}

// ── Provider impl ───────────────────────────────────────────────────────────

#[async_trait]
impl LightProvider for GoveeCloud {
    fn name(&self) -> &str {
        "govee-cloud"
    }

    async fn discover(&self) -> Result<Vec<Light>> {
        let devices = self.fetch_devices().await?;
        let mut lights = Vec::with_capacity(devices.len());
        for d in devices {
            // Devices that advertise the dynamic-scene capability expose their
            // scene list (the effects) behind a separate, cached call.
            let has_scenes = d.capabilities.iter().any(|c| c.instance == "lightScene");
            let mut light = govee_device_to_light(d, None);
            if has_scenes {
                // Best-effort: a scenes fetch failure must not fail discovery.
                if let Ok(scenes) = self.scenes_for(&light.provider_id).await {
                    light.capabilities.effects = scenes.into_iter().map(|s| s.name).collect();
                }
            }
            lights.push(light);
        }
        Ok(lights)
    }

    async fn set_state(&self, provider_id: &str, state: &LightState) -> Result<()> {
        // Queue behind (and supersede) any other write for this device.
        let gate = device_gate(&self.base_url, provider_id);
        *gate.pending_state.lock().await = Some(state.clone());
        let _batch = gate.batch.lock().await;
        let Some(state) = gate.pending_state.lock().await.take() else {
            // A batch that started after we enqueued already sent our state (or
            // a newer one). Nothing left to do — reporting success is honest.
            tracing::debug!(target: "bifrost::govee", device = %provider_id, "set_state superseded by a newer write");
            return Ok(());
        };

        // Govee requires one command per capability.
        let mut commands: Vec<Value> = vec![json!({
            "type": "devices.capabilities.on_off",
            "instance": "powerSwitch",
            "value": if state.on { 1 } else { 0 }
        })];

        // Attributes only matter while the light is on, and each one is a
        // separate metered request — sending colour/brightness alongside an
        // "off" spends three times the quota to say one thing, and hands the
        // device a reason to light back up.
        if state.on {
            if let Some(brightness) = state.brightness {
                commands.push(json!({
                    "type": "devices.capabilities.range",
                    "instance": "brightness",
                    "value": brightness.round() as u32
                }));
            }

            if let Some(color) = &state.color {
                let (r, g, b) = color.to_rgb();
                let rgb_int = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
                commands.push(json!({
                    "type": "devices.capabilities.color_setting",
                    "instance": "colorRgb",
                    "value": rgb_int
                }));
            }

            if let Some(mirek) = state.color_temp_mirek {
                let kelvin = crate::models::mirek_to_kelvin(mirek);
                commands.push(json!({
                    "type": "devices.capabilities.color_setting",
                    "instance": "colorTemperatureK",
                    "value": kelvin
                }));
            }

            // A dynamic scene ("effect") is applied by echoing back its opaque
            // value under the dynamic_scene capability. The frontend sends
            // `effect` only on an actual scene pick, so this doesn't ride along
            // with colour tweaks.
            if let Some(effect) = state.effect.as_deref().filter(|e| !e.is_empty()) {
                let scene = self
                    .scenes_for(provider_id)
                    .await?
                    .into_iter()
                    .find(|s| s.name == effect)
                    .ok_or_else(|| anyhow::anyhow!("unknown Govee scene '{effect}'"))?;
                commands.push(json!({
                    "type": "devices.capabilities.dynamic_scene",
                    "instance": scene.instance,
                    "value": scene.value
                }));
            }
        }

        let sku = self.sku_for(provider_id).await?;

        // **Sequentially**, power first. Firing them concurrently was faster on
        // a quiet account but is what makes a busy one flaky: the device gets
        // simultaneous capability writes, and `try_join_all` cancels the
        // siblings the moment one fails — so a rate-limited colour command also
        // dropped the brightness command that was already on the wire, leaving
        // the light half-applied. In order, a failure stops the batch with the
        // most important command (power) already delivered.
        for cmd in commands {
            self.send_control(&sku, provider_id, cmd).await?;
        }

        Ok(())
    }

    async fn set_segments(&self, provider_id: &str, segments: &[SegmentColor]) -> Result<()> {
        if segments.is_empty() {
            return Ok(());
        }
        // Same gate as `set_state`: one batch per device, newest wins. A strip
        // editor drag emits a segment write every 150ms, each of which is
        // several requests.
        let gate = device_gate(&self.base_url, provider_id);
        *gate.pending_segments.lock().await = Some(segments.to_vec());
        let _batch = gate.batch.lock().await;
        let Some(segments) = gate.pending_segments.lock().await.take() else {
            tracing::debug!(target: "bifrost::govee", device = %provider_id, "set_segments superseded by a newer write");
            return Ok(());
        };
        let segments = &segments[..];
        let sku = self.sku_for(provider_id).await?;
        // `segmentedColorRgb` / `segmentedBrightness` each set one value across a
        // *list* of segments, so group by value: each distinct colour and each
        // distinct brightness is one command. (Govee indexes segments from 0.)
        let mut by_rgb: HashMap<u32, Vec<u16>> = HashMap::new();
        let mut by_brightness: HashMap<u8, Vec<u16>> = HashMap::new();
        for s in segments {
            if let Some(rgb) = s.rgb {
                by_rgb.entry(rgb).or_default().push(s.segment);
            }
            if let Some(b) = s.brightness {
                by_brightness.entry(b).or_default().push(s.segment);
            }
        }
        let mut capabilities: Vec<Value> = Vec::new();
        for (rgb, segs) in by_rgb {
            capabilities.push(json!({
                "type": "devices.capabilities.segment_color_setting",
                "instance": "segmentedColorRgb",
                "value": { "segment": segs, "rgb": rgb }
            }));
        }
        for (brightness, segs) in by_brightness {
            capabilities.push(json!({
                "type": "devices.capabilities.segment_color_setting",
                "instance": "segmentedBrightness",
                "value": { "segment": segs, "brightness": brightness }
            }));
        }
        // Sequential, for the same reason `set_state` is: a cancelled sibling
        // leaves the strip half-painted.
        for c in capabilities {
            self.send_control(&sku, provider_id, c).await?;
        }
        Ok(())
    }

    async fn get_state(&self, provider_id: &str) -> Result<LightState> {
        let sku = self.sku_for(provider_id).await?;
        let body = json!({
            "requestId": Uuid::new_v4().to_string(),
            "payload": { "sku": sku, "device": provider_id }
        });

        let resp: GoveeResponse<GoveeStateData> = self
            .send(
                self.client
                    .post(format!("{}/device/state", self.base_url))
                    .json(&body),
            )
            .await?
            .error_for_status()?
            .json()
            .await?;

        if resp.code != 200 {
            bail!("Govee state error {}: {}", resp.code, resp.message);
        }

        Ok(resp
            .body()
            .map(|d| parse_govee_state(d.capabilities))
            .unwrap_or_default())
    }
}

// ── Unified provider (LAN-preferred, cloud fallback) ─────────────────────────

use crate::providers::govee_lan::{GoveeLanProvider, LanScan};
use crate::providers::mac_hw_id;
use std::net::IpAddr;

/// How long a LAN address stays trusted after the device last proved it's there
/// (a scan reply or a successful `devStatus` read). Comfortably longer than the
/// poll interval that refreshes it, short enough that a device which moved or
/// went away stops swallowing commands within one cycle.
const LAN_TTL: Duration = Duration::from_secs(300);

/// Floor between LAN scans triggered by a cache miss. A scan is a ~1.5s
/// multicast window; without this floor a cloud-only device (LAN Control off)
/// paid one on **every** command and every poll, which is most of what "the
/// controls are laggy" was.
const LAN_RESCAN_INTERVAL: Duration = Duration::from_secs(30);

/// A device's last known LAN address and when it last proved it was there.
#[derive(Clone)]
struct LanEntry {
    ip: String,
    seen: Instant,
}

/// Process-wide `normalized-MAC → LAN address` map. Populated by
/// [`GoveeProvider`]'s discovery/scan; read by control + state to address a
/// device over the LAN. Global (MACs are unique) and process-lived because
/// `build_provider` rebuilds the provider per request, so a per-instance map
/// would never survive to the next control call.
///
/// Membership IS the per-device LAN-eligibility gate — but membership **expires**
/// ([`LAN_TTL`]). LAN control is fire-and-forget UDP: a `send_to` at a device
/// that has moved to a new DHCP lease, or been unplugged, succeeds at the
/// syscall and returns `Ok`, so a never-expiring entry meant every command for
/// that light vanished silently and the cloud fallback never ran. An entry is
/// only trusted while something recent proved the device answers there.
fn lan_ip_cache() -> &'static tokio::sync::RwLock<std::collections::HashMap<String, LanEntry>> {
    static CACHE: std::sync::OnceLock<
        tokio::sync::RwLock<std::collections::HashMap<String, LanEntry>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| tokio::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Serialises miss-driven rescans and remembers when the last one ran, so a
/// burst of misses (a room command over several cloud-only lights) costs one
/// scan window shared by all of them rather than one each.
fn lan_scan_gate() -> &'static tokio::sync::Mutex<Option<Instant>> {
    static GATE: std::sync::OnceLock<tokio::sync::Mutex<Option<Instant>>> =
        std::sync::OnceLock::new();
    GATE.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// The device's LAN address, if one is cached and still within [`LAN_TTL`].
async fn fresh_lan_ip(key: &str) -> Option<String> {
    lan_ip_cache()
        .read()
        .await
        .get(key)
        .filter(|e| e.seen.elapsed() < LAN_TTL)
        .map(|e| e.ip.clone())
}

/// The device just answered on the LAN — extend its lease.
async fn mark_lan_seen(key: &str) {
    if let Some(e) = lan_ip_cache().write().await.get_mut(key) {
        e.seen = Instant::now();
    }
}

/// The device didn't answer at its cached address — drop it so control goes
/// cloud immediately instead of shouting into a dead socket until the TTL lapses.
async fn forget_lan(key: &str) {
    lan_ip_cache().write().await.remove(key);
}

/// Test helper: backdate a cached LAN lease so expiry can be exercised without
/// waiting out [`LAN_TTL`].
#[cfg(test)]
async fn seed_lan_entry(key: &str, ip: &str, age: Duration) {
    lan_ip_cache().write().await.insert(
        key.to_string(),
        LanEntry {
            ip: ip.to_string(),
            seen: Instant::now() - age,
        },
    );
}

/// Test helper: when the last LAN scan ran, for asserting the rescan floor.
#[cfg(test)]
async fn lan_scan_stamp() -> Option<Instant> {
    *lan_scan_gate().lock().await
}

#[cfg(test)]
async fn clear_lan_ip_cache() {
    lan_ip_cache().write().await.clear();
    *lan_scan_gate().lock().await = None;
}

/// Govee with **both transports**: a per-device LAN-preferred, cloud-fallback
/// light provider. The LAN path (local UDP — no quota, low latency) is used for
/// any device that answered a LAN scan; everything else — an unscanned device, a
/// LAN send that failed, or an effect/dynamic-scene (LAN has no scene catalogue)
/// — goes over the cloud. Either transport may be absent: cloud-only (no LAN
/// interface configured) and LAN-only (no API key) both work.
pub struct GoveeProvider {
    cloud: Option<GoveeCloud>,
    /// `Arc` so a background refresh can outlive the request that started it —
    /// the provider itself is rebuilt and dropped per request.
    lan: Option<Arc<GoveeLanProvider>>,
}

impl GoveeProvider {
    pub fn new(cloud: Option<GoveeCloud>, lan: Option<GoveeLanProvider>) -> Self {
        Self {
            cloud,
            lan: lan.map(Arc::new),
        }
    }

    /// Fold a batch of LAN scan results into the process-wide MAC→IP cache.
    async fn cache_scans(scans: &[LanScan]) {
        if scans.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut cache = lan_ip_cache().write().await;
        for s in scans {
            if let Some(k) = mac_hw_id(&s.mac) {
                cache.insert(
                    k,
                    LanEntry {
                        ip: s.ip.clone(),
                        seen: now,
                    },
                );
            }
        }
    }

    /// Resolve a device's current LAN address from its MAC. `None` means the
    /// device isn't LAN-eligible right now → use the cloud.
    ///
    /// A miss (or an expired lease) kicks off a refresh, since a control can
    /// arrive before the first poll populated the cache — but **in the
    /// background**. A scan is a 1.5s multicast window and this sits directly on
    /// the control path: a device that will never answer one (LAN Control off)
    /// would otherwise put that 1.5s in front of every command it receives, to
    /// learn nothing. The command in hand goes over the cloud; the next one gets
    /// the LAN. The refresh is also floored at [`LAN_RESCAN_INTERVAL`] and
    /// coalesced, because one scan refreshes every device at once.
    async fn lan_ip_for(&self, mac: &str) -> Option<String> {
        let lan = self.lan.clone()?;
        let key = mac_hw_id(mac)?;
        if let Some(ip) = fresh_lan_ip(&key).await {
            return Some(ip);
        }
        Self::refresh_lan_addresses(lan);
        None
    }

    /// Spawn one LAN scan to refill the address cache, unless a scan already ran
    /// inside [`LAN_RESCAN_INTERVAL`] or is running right now (`try_lock` — a
    /// held gate IS a scan in flight, and a second one would learn the same thing).
    fn refresh_lan_addresses(lan: Arc<GoveeLanProvider>) {
        tokio::spawn(async move {
            let Ok(mut gate) = lan_scan_gate().try_lock() else {
                return;
            };
            if gate.is_some_and(|t| t.elapsed() < LAN_RESCAN_INTERVAL) {
                return;
            }
            let scans = lan.scan().await.unwrap_or_default();
            *gate = Some(Instant::now());
            Self::cache_scans(&scans).await;
            tracing::debug!(
                target: "bifrost::govee",
                scanned = scans.len(),
                "LAN address cache miss — re-scanned in the background ({} device(s) replied)",
                scans.len(),
            );
        });
    }

    /// Cloud control, or a clear "unreachable" error when there's no cloud key.
    async fn cloud_set(&self, mac: &str, state: &LightState) -> Result<()> {
        match &self.cloud {
            Some(c) => c.set_state(mac, state).await,
            None => bail!(
                "Govee device {mac} is not reachable on the LAN and no cloud API key is configured"
            ),
        }
    }
}

#[async_trait]
impl LightProvider for GoveeProvider {
    fn name(&self) -> &str {
        "govee"
    }

    async fn discover(&self) -> Result<Vec<Light>> {
        // LAN scan first: it both refills the MAC→IP cache and surfaces LAN-only
        // devices (no cloud row). A scan failure is non-fatal.
        let scans = match &self.lan {
            Some(lan) => lan.scan().await.unwrap_or_default(),
            None => Vec::new(),
        };
        if self.lan.is_some() {
            // This scan refreshed every device's lease, so it counts against the
            // miss-driven rescan floor — a control arriving right after a poll
            // shouldn't open a second scan window for what this one just answered.
            *lan_scan_gate().lock().await = Some(Instant::now());
        }
        Self::cache_scans(&scans).await;

        // Cloud devices carry the richer metadata (name, capabilities, effects).
        let cloud_lights = match &self.cloud {
            Some(c) => match c.discover().await {
                Ok(l) => l,
                // Stay resilient: with the LAN up, a cloud blip shouldn't wipe the
                // device list. With no LAN, propagate (cloud is all we have).
                Err(e) if self.lan.is_some() => {
                    tracing::warn!("Govee cloud discovery failed, using LAN only: {e:#}");
                    Vec::new()
                }
                Err(e) => return Err(e),
            },
            None => Vec::new(),
        };

        // Union: keep every cloud light; append LAN-only devices (a scanned MAC
        // with no cloud row), keyed by MAC so they're stable across IP changes.
        let cloud_macs: std::collections::HashSet<String> = cloud_lights
            .iter()
            .filter_map(|l| mac_hw_id(&l.provider_id))
            .collect();
        let scanned_macs: std::collections::HashSet<String> =
            scans.iter().filter_map(|s| mac_hw_id(&s.mac)).collect();
        // mac → LAN IP, so a cloud device that also answered the scan shows its IP.
        let scanned_ips: std::collections::HashMap<String, String> = scans
            .iter()
            .filter_map(|s| mac_hw_id(&s.mac).map(|k| (k, s.ip.to_string())))
            .collect();
        let mut lights = cloud_lights;
        // Stamp how each cloud device will be reached so the UI shows it up front
        // (control prefers LAN whenever the device answered a scan).
        for l in &mut lights {
            let key = mac_hw_id(&l.provider_id);
            let on_lan = key.as_ref().is_some_and(|k| scanned_macs.contains(k));
            l.state.transport = Some(if on_lan { "lan" } else { "cloud" }.to_string());
            l.state.ip = key.and_then(|k| scanned_ips.get(&k).cloned());
        }
        for s in scans {
            let known = mac_hw_id(&s.mac).is_some_and(|k| cloud_macs.contains(&k));
            if !known {
                lights.push(lan_only_light(&s));
            }
        }
        let on_lan = lights
            .iter()
            .filter(|l| l.state.transport.as_deref() == Some("lan"))
            .count();
        tracing::debug!(
            target: "bifrost::govee",
            lan_enabled = self.lan.is_some(),
            cloud_enabled = self.cloud.is_some(),
            lan_scanned = scanned_macs.len(),
            cloud_devices = cloud_macs.len(),
            total = lights.len(),
            on_lan,
            on_cloud = lights.len() - on_lan,
            "Govee discover: {} device(s) — {on_lan} via LAN, {} via cloud",
            lights.len(),
            lights.len() - on_lan,
        );
        Ok(lights)
    }

    async fn set_state(&self, mac: &str, state: &LightState) -> Result<()> {
        // A dynamic scene ("effect") only exists in the cloud catalogue.
        if state.effect.as_deref().is_some_and(|e| !e.is_empty()) {
            tracing::debug!(target: "bifrost::govee", %mac, "set_state: effect → cloud (LAN has no scene catalogue)");
            return self.cloud_set(mac, state).await;
        }
        // LAN-preferred: only when this device answered a scan recently enough
        // for the address to still be trusted (see [`LAN_TTL`]).
        if let Some(lan) = &self.lan
            && let Some(ip) = self.lan_ip_for(mac).await
        {
            tracing::debug!(target: "bifrost::govee", %mac, %ip, "set_state: → LAN (device answered a scan)");
            match lan.set_state(&ip, state).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    // The address is suspect now — drop it so the *next* command
                    // goes straight to the cloud instead of repeating this.
                    if let Some(k) = mac_hw_id(mac) {
                        forget_lan(&k).await;
                    }
                    tracing::warn!("Govee LAN control failed for {mac}, trying cloud: {e:#}")
                }
            }
        } else {
            tracing::debug!(target: "bifrost::govee", %mac, lan_enabled = self.lan.is_some(), "set_state: → cloud (device not reachable on LAN)");
        }
        self.cloud_set(mac, state).await
    }

    async fn set_segments(&self, mac: &str, segments: &[SegmentColor]) -> Result<()> {
        // Segment control is **cloud-only** — the Govee LAN API has no
        // per-segment command, and capability discovery (segment count) comes
        // from the cloud device list anyway.
        match &self.cloud {
            Some(c) => {
                tracing::debug!(target: "bifrost::govee", %mac, segments = segments.len(), "set_segments → cloud");
                c.set_segments(mac, segments).await
            }
            None => bail!(
                "Govee device {mac} segment control needs a cloud API key (the LAN API has no segment command)"
            ),
        }
    }

    async fn get_state(&self, mac: &str) -> Result<LightState> {
        // The LAN read is also the liveness probe that keeps (or revokes) this
        // device's LAN lease — control is fire-and-forget UDP and can't prove
        // anything itself, so this is where "is it still there?" gets answered.
        if let Some(lan) = &self.lan
            && let Some(ip) = self.lan_ip_for(mac).await
        {
            let key = mac_hw_id(mac);
            match lan.get_state(&ip).await {
                Ok(mut state) if state.reachable != Some(false) => {
                    if let Some(k) = &key {
                        mark_lan_seen(k).await;
                    }
                    state.transport = Some("lan".to_string());
                    tracing::debug!(target: "bifrost::govee", %mac, %ip, "get_state: via LAN");
                    return Ok(state);
                }
                _ => {
                    if let Some(k) = &key {
                        forget_lan(k).await;
                    }
                    tracing::debug!(target: "bifrost::govee", %mac, %ip, "get_state: LAN silent — dropping the address, falling back to cloud");
                }
            }
        }
        // LAN unavailable/unreachable → cloud if we have it.
        if let Some(c) = &self.cloud {
            let mut state = c.get_state(mac).await?;
            state.transport = Some("cloud".to_string());
            tracing::debug!(target: "bifrost::govee", %mac, "get_state: via cloud");
            return Ok(state);
        }
        Ok(LightState {
            on: false,
            reachable: Some(false),
            ..Default::default()
        })
    }

    async fn debug_info(&self) -> Option<Value> {
        let mut out = json!({
            "transport": { "cloud": self.cloud.is_some(), "lan": self.lan.is_some() },
            "lan_cached_devices": lan_ip_cache()
                .read()
                .await
                .values()
                .filter(|e| e.seen.elapsed() < LAN_TTL)
                .count(),
        });
        if let Some(cloud) = &self.cloud {
            match cloud.debug_devices().await {
                Ok(d) => out["cloud"] = d,
                Err(e) => out["cloud_error"] = json!(e.to_string()),
            }
        }
        Some(out)
    }
}

/// Synthesize a `Light` for a device seen only on the LAN (no cloud row). Keyed
/// by MAC (stable) when the scan reported one, else the IP. Govee LAN devices are
/// RGBWW: dimmable + colour + tunable white; effects are a cloud-only feature.
fn lan_only_light(s: &LanScan) -> Light {
    let provider_id = if s.mac.is_empty() {
        s.ip.clone()
    } else {
        s.mac.clone()
    };
    let name = if s.sku.is_empty() {
        format!("Govee @ {}", s.ip)
    } else {
        format!("Govee {} @ {}", s.sku, s.ip)
    };
    Light {
        id: Uuid::new_v4(),
        hw_id: mac_hw_id(&s.mac),
        provider_id,
        provider: Provider::Govee,
        name,
        // Seen only on the LAN → that's how it's reached.
        state: LightState {
            transport: Some("lan".to_string()),
            ip: Some(s.ip.to_string()),
            ..Default::default()
        },
        capabilities: LightCapabilities {
            dimmable: true,
            color_rgb: true,
            color_temperature: true,
            hue_gamut: None,
            effects: Vec::new(),
            // A LAN-only device (no cloud row) reports no capability list, so we
            // can't know its segment count — segment control needs the cloud anyway.
            segments: None,
        },
        last_seen: Utc::now(),
    }
}

// ── Factory ─────────────────────────────────────────────────────────────────

use crate::providers::{CredentialField, FieldKind, ProviderFactory};

pub struct GoveeProviderFactory;

impl ProviderFactory for GoveeProviderFactory {
    fn provider_type(&self) -> &'static str {
        "govee"
    }

    fn display_name(&self) -> &'static str {
        "Govee"
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn crate::providers::LightProvider>> {
        let creds: serde_json::Value = serde_json::from_str(credentials_json)?;
        // Cloud when an API key is supplied. LAN is **on by default** (preferred,
        // no quota) — `bind_addr` only picks the interface, defaulting to 0.0.0.0
        // (all NICs). A host that can't reach the LAN simply finds nothing on a
        // scan and falls back to the cloud, so defaulting it on is safe.
        let cloud = match creds["api_key"].as_str().filter(|k| !k.is_empty()) {
            Some(k) => Some(GoveeCloud::new(k)?),
            None => None,
        };
        let bind = creds["bind_addr"]
            .as_str()
            .filter(|b| !b.is_empty())
            .unwrap_or("0.0.0.0");
        let addr: IpAddr = bind
            .parse()
            .with_context(|| format!("invalid Govee LAN bind address '{bind}'"))?;
        let lan = Some(GoveeLanProvider::new(addr));
        Ok(Box::new(GoveeProvider::new(cloud, lan)))
    }

    fn credentials_schema(&self) -> &'static [CredentialField] {
        &[
            CredentialField {
                name: "api_key",
                label: "API Key",
                kind: FieldKind::Password,
                required: false,
                hint: Some(
                    "Govee Home app → Profile → About Us → Apply for API Key. Optional if you only use LAN control.",
                ),
            },
            CredentialField {
                name: "bind_addr",
                label: "LAN interface (advanced)",
                kind: FieldKind::IpAddress,
                required: false,
                hint: Some(
                    "Local LAN control is on by default (preferred over the cloud whenever a device is reachable) — just turn on 'LAN Control' for each device in the Govee Home app. Leave blank for all interfaces (0.0.0.0); set a specific IP only if multi-homed.",
                ),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mock_provider(server: &MockServer) -> GoveeCloud {
        GoveeCloud::new_for_test(server.uri(), "test-govee-key").unwrap()
    }

    fn device_list_response() -> serde_json::Value {
        serde_json::json!({
            "code": 200,
            "message": "success",
            "data": {
                "devices": [{
                    "sku": "H6159",
                    "device": "AA:BB:CC:DD:EE:FF",
                    "deviceName": "Strip Lights",
                    "capabilities": [
                        {"type": "devices.capabilities.on_off", "instance": "powerSwitch"},
                        {"type": "devices.capabilities.range", "instance": "brightness"},
                        {"type": "devices.capabilities.color_setting", "instance": "colorRgb"},
                        {"type": "devices.capabilities.color_setting", "instance": "colorTemperatureK"}
                    ]
                }]
            }
        })
    }

    #[tokio::test]
    async fn discover_parses_device_list() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .and(header("Govee-API-Key", "test-govee-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(device_list_response()))
            .mount(&server)
            .await;

        let lights = mock_provider(&server).await.discover().await.unwrap();

        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].name, "Strip Lights");
        assert_eq!(lights[0].provider_id, "AA:BB:CC:DD:EE:FF");
        assert!(lights[0].capabilities.color_rgb);
        assert!(lights[0].capabilities.color_temperature);
        assert!(lights[0].capabilities.dimmable);
    }

    #[tokio::test]
    async fn discover_returns_empty_list_when_no_devices() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 200, "message": "success", "data": {"devices": []}
            })))
            .mount(&server)
            .await;

        let lights = mock_provider(&server).await.discover().await.unwrap();
        assert!(lights.is_empty());
    }

    #[tokio::test]
    async fn debug_devices_flags_unsupported_capabilities() {
        let server = MockServer::start().await;
        // A strip exposing one capability we model (brightness) and one we don't
        // (musicMode — react-to-music; not modelled yet).
        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 200, "message": "success",
                "data": { "devices": [{
                    "sku": "H6159",
                    "device": "AA:BB:CC:DD:EE:FF",
                    "deviceName": "Strip Lights",
                    "capabilities": [
                        {"type": "devices.capabilities.range", "instance": "brightness"},
                        {"type": "devices.capabilities.music_setting", "instance": "musicMode"}
                    ]
                }]}
            })))
            .mount(&server)
            .await;

        let report = mock_provider(&server).await.debug_devices().await.unwrap();
        let dev = &report["devices"][0];
        assert_eq!(dev["name"], "Strip Lights");
        // The unmodelled capability is surfaced for the dev to see.
        assert_eq!(dev["unsupported_capabilities"][0], "musicMode");
        let caps = dev["capabilities"].as_array().unwrap();
        let brightness = caps.iter().find(|c| c["instance"] == "brightness").unwrap();
        assert_eq!(brightness["supported"], true);
        let music = caps.iter().find(|c| c["instance"] == "musicMode").unwrap();
        assert_eq!(music["supported"], false);
        assert_eq!(music["type"], "devices.capabilities.music_setting");
    }

    fn device_list_with_scenes() -> serde_json::Value {
        serde_json::json!({
            "code": 200, "message": "success",
            "data": { "devices": [{
                "sku": "H6159",
                "device": "AA:BB:CC:DD:EE:FF",
                "deviceName": "Strip Lights",
                "capabilities": [
                    {"type": "devices.capabilities.on_off", "instance": "powerSwitch"},
                    {"type": "devices.capabilities.color_setting", "instance": "colorRgb"},
                    {"type": "devices.capabilities.dynamic_scene", "instance": "lightScene"}
                ]
            }]}
        })
    }

    fn scenes_response() -> serde_json::Value {
        serde_json::json!({
            "code": 200, "message": "success",
            "payload": { "capabilities": [{
                "type": "devices.capabilities.dynamic_scene",
                "instance": "lightScene",
                "parameters": { "options": [
                    {"name": "Sunrise", "value": {"id": 1, "paramId": 10}},
                    {"name": "Aurora",  "value": {"id": 2, "paramId": 20}}
                ]}
            }]}
        })
    }

    #[tokio::test]
    async fn discover_advertises_dynamic_scenes_as_effects() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(device_list_with_scenes()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/device/scenes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(scenes_response()))
            .mount(&server)
            .await;

        let lights = mock_provider(&server).await.discover().await.unwrap();
        assert_eq!(lights[0].capabilities.effects, vec!["Sunrise", "Aurora"]);
    }

    #[tokio::test]
    async fn set_state_with_effect_applies_the_dynamic_scene() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(device_list_with_scenes()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/device/scenes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(scenes_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/device/control"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 200, "message": "success", "data": {}
            })))
            .mount(&server)
            .await;

        let state = LightState {
            on: true,
            effect: Some("Aurora".to_string()),
            ..Default::default()
        };
        mock_provider(&server)
            .await
            .set_state("AA:BB:CC:DD:EE:FF", &state)
            .await
            .unwrap();

        let bodies = control_bodies(&server).await;
        let scene = bodies
            .iter()
            .find(|b| b["payload"]["capability"]["instance"] == "lightScene")
            .expect("a dynamic_scene control was sent");
        // The opaque value is echoed back verbatim.
        assert_eq!(
            scene["payload"]["capability"]["value"],
            serde_json::json!({ "id": 2, "paramId": 20 })
        );
    }

    fn diy_scenes_response() -> serde_json::Value {
        serde_json::json!({
            "code": 200, "message": "success",
            "payload": { "capabilities": [{
                "type": "devices.capabilities.dynamic_scene",
                "instance": "diyScene",
                "parameters": { "options": [
                    {"name": "My Vibe", "value": 9001}
                ]}
            }]}
        })
    }

    #[tokio::test]
    async fn discover_and_apply_merge_diy_scenes_with_their_own_instance() {
        let server = MockServer::start().await;
        // A device id unique to this test: the scene cache is process-wide and
        // keyed by (base_url, device id), and wiremock reuses ports across tests,
        // so a distinct id avoids inheriting another scene test's cached catalog
        // without a global cache clear (which would race the fetch-count test).
        let device_id = "DD:DD:DD:DD:DD:DD";
        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 200, "message": "success",
                "data": { "devices": [{
                    "sku": "H6159",
                    "device": device_id,
                    "deviceName": "DIY Strip",
                    "capabilities": [
                        {"type": "devices.capabilities.on_off", "instance": "powerSwitch"},
                        {"type": "devices.capabilities.dynamic_scene", "instance": "lightScene"}
                    ]
                }]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/device/scenes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(scenes_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/device/diy-scenes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(diy_scenes_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/device/control"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 200, "message": "success", "data": {}
            })))
            .mount(&server)
            .await;

        let p = mock_provider(&server).await;
        // The DIY scene shows up alongside the built-ins.
        let lights = p.discover().await.unwrap();
        assert_eq!(
            lights[0].capabilities.effects,
            vec!["Sunrise", "Aurora", "My Vibe"]
        );

        // Applying it routes under the `diyScene` instance, not `lightScene`.
        let state = LightState {
            on: true,
            effect: Some("My Vibe".to_string()),
            ..Default::default()
        };
        p.set_state(device_id, &state).await.unwrap();
        let scene = control_bodies(&server)
            .await
            .into_iter()
            .find(|b| b["payload"]["capability"]["type"] == "devices.capabilities.dynamic_scene")
            .expect("a dynamic_scene control was sent");
        assert_eq!(scene["payload"]["capability"]["instance"], "diyScene");
        assert_eq!(scene["payload"]["capability"]["value"], 9001);
    }

    /// Mount the devices endpoint (needed by SKU resolution) + control endpoint.
    async fn mount_control_mocks(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(device_list_response()))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/device/control"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 200, "message": "success", "data": {}
            })))
            .mount(server)
            .await;
    }

    /// Bodies of the POSTs to /device/control (skipping the SKU-lookup GET).
    async fn control_bodies(server: &MockServer) -> Vec<serde_json::Value> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/device/control")
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn set_state_on_off_sends_power_switch_command() {
        let server = MockServer::start().await;
        mount_control_mocks(&server).await;

        let state = LightState {
            on: true,
            ..Default::default()
        };
        mock_provider(&server)
            .await
            .set_state("AA:BB:CC:DD:EE:FF", &state)
            .await
            .unwrap();

        let bodies = control_bodies(&server).await;
        // One request for power switch.
        assert_eq!(bodies.len(), 1);
        let body = &bodies[0];
        // The live API rejects control requests without the device SKU (400).
        assert_eq!(body["payload"]["sku"], "H6159");
        assert_eq!(body["payload"]["capability"]["instance"], "powerSwitch");
        assert_eq!(body["payload"]["capability"]["value"], 1);
    }

    #[tokio::test]
    async fn set_state_with_brightness_sends_two_commands() {
        let server = MockServer::start().await;
        mount_control_mocks(&server).await;

        let state = LightState {
            on: true,
            brightness: Some(75.0),
            ..Default::default()
        };
        mock_provider(&server)
            .await
            .set_state("AA:BB:CC:DD:EE:FF", &state)
            .await
            .unwrap();

        // Two control requests: power + brightness.
        let bodies = control_bodies(&server).await;
        assert_eq!(bodies.len(), 2);
    }

    #[tokio::test]
    async fn discover_reads_segment_count_from_capability() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 200, "message": "success",
                "data": { "devices": [{
                    "sku": "H6159", "device": "AA:BB:CC:DD:EE:FF", "deviceName": "Strip",
                    "capabilities": [
                        {"type": "devices.capabilities.on_off", "instance": "powerSwitch"},
                        {"type": "devices.capabilities.segment_color_setting", "instance": "segmentedColorRgb",
                         "parameters": {"dataType": "STRUCT", "fields": [
                            {"fieldName": "segment", "dataType": "Array", "elementRange": {"min": 0, "max": 14}, "size": {"min": 1, "max": 15}},
                            {"fieldName": "rgb", "dataType": "INTEGER", "range": {"min": 0, "max": 16777215}}
                         ]}}
                    ]
                }]}
            })))
            .mount(&server)
            .await;

        let lights = mock_provider(&server).await.discover().await.unwrap();
        // elementRange 0..=14 → 15 addressable segments.
        assert_eq!(lights[0].capabilities.segments, Some(15));
    }

    #[tokio::test]
    async fn set_segments_groups_segments_by_colour() {
        let server = MockServer::start().await;
        mount_control_mocks(&server).await;

        let segs = vec![
            SegmentColor {
                segment: 0,
                rgb: Some(0xFF0000),
                brightness: None,
            },
            SegmentColor {
                segment: 1,
                rgb: Some(0xFF0000),
                brightness: None,
            },
            SegmentColor {
                segment: 2,
                rgb: Some(0x00FF00),
                brightness: None,
            },
        ];
        mock_provider(&server)
            .await
            .set_segments("AA:BB:CC:DD:EE:FF", &segs)
            .await
            .unwrap();

        // One command per distinct colour (red over [0,1], green over [2]).
        let bodies = control_bodies(&server).await;
        assert_eq!(bodies.len(), 2);
        for b in &bodies {
            assert_eq!(
                b["payload"]["capability"]["instance"], "segmentedColorRgb",
                "wrong capability: {b}"
            );
        }
        let red = bodies
            .iter()
            .find(|b| b["payload"]["capability"]["value"]["rgb"] == 0xFF0000)
            .expect("red group missing");
        let mut red_segs: Vec<u64> = red["payload"]["capability"]["value"]["segment"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .collect();
        red_segs.sort_unstable();
        assert_eq!(red_segs, vec![0, 1]);
        let green = bodies
            .iter()
            .find(|b| b["payload"]["capability"]["value"]["rgb"] == 0x00FF00)
            .expect("green group missing");
        assert_eq!(green["payload"]["capability"]["value"]["segment"][0], 2);
    }

    #[tokio::test]
    async fn set_segments_sends_brightness_via_segmented_brightness() {
        let server = MockServer::start().await;
        mount_control_mocks(&server).await;

        // A segment carrying both colour and brightness → two commands.
        let segs = vec![SegmentColor {
            segment: 3,
            rgb: Some(0x0000FF),
            brightness: Some(40),
        }];
        mock_provider(&server)
            .await
            .set_segments("AA:BB:CC:DD:EE:FF", &segs)
            .await
            .unwrap();

        let bodies = control_bodies(&server).await;
        assert_eq!(bodies.len(), 2);
        let bri = bodies
            .iter()
            .find(|b| b["payload"]["capability"]["instance"] == "segmentedBrightness")
            .expect("brightness command missing");
        assert_eq!(bri["payload"]["capability"]["value"]["brightness"], 40);
        assert_eq!(bri["payload"]["capability"]["value"]["segment"][0], 3);
    }

    #[tokio::test]
    async fn repeated_commands_resolve_sku_with_a_single_device_list_fetch() {
        // Drop any cache entry inherited from a prior test whose dropped mock
        // server happened to be reassigned this test's ephemeral port.
        clear_sku_cache().await;
        // Two separate commands must hit /user/devices only once — the second is
        // served from the process-wide cache (the "laggy controls" fix).
        let server = MockServer::start().await;
        mount_control_mocks(&server).await;

        let on = LightState {
            on: true,
            ..Default::default()
        };
        let provider = mock_provider(&server).await;
        provider.set_state("AA:BB:CC:DD:EE:FF", &on).await.unwrap();
        provider.set_state("AA:BB:CC:DD:EE:FF", &on).await.unwrap();

        let device_list_fetches = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/user/devices")
            .count();
        assert_eq!(device_list_fetches, 1, "SKU lookup must be cached");
    }

    #[tokio::test]
    async fn discover_parses_live_api_shape() {
        // The live API: "msg" instead of "message", data as a bare array,
        // capabilities with type+instance+parameters.
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "requestId": "uuid-1",
                "msg": "success",
                "code": 200,
                "data": [{
                    "sku": "H6601",
                    "device": "11:22:33:44:55:66",
                    "deviceName": "Desk Strip",
                    "type": "devices.types.light",
                    "capabilities": [
                        {"type": "devices.capabilities.on_off", "instance": "powerSwitch", "parameters": {}},
                        {"type": "devices.capabilities.range", "instance": "brightness", "parameters": {}},
                        {"type": "devices.capabilities.color_setting", "instance": "colorRgb", "parameters": {}}
                    ]
                }]
            })))
            .mount(&server)
            .await;

        let lights = mock_provider(&server).await.discover().await.unwrap();
        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].name, "Desk Strip");
        assert!(lights[0].capabilities.dimmable);
        assert!(lights[0].capabilities.color_rgb);
    }

    #[tokio::test]
    async fn get_state_parses_live_api_payload_and_value_wrappers() {
        // The live API: state under "payload", each value wrapped as {"value": x}.
        let server = MockServer::start().await;

        // SKU resolution fetches the device list first.
        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "requestId": "uuid-0",
                "msg": "success",
                "code": 200,
                "data": [{"sku": "H6601", "device": "11:22:33:44:55:66", "deviceName": "Desk Strip"}]
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/device/state"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "requestId": "uuid-2",
                "msg": "success",
                "code": 200,
                "payload": {
                    "sku": "H6601",
                    "device": "11:22:33:44:55:66",
                    "capabilities": [
                        {"type": "devices.capabilities.online", "instance": "online", "state": {"value": true}},
                        {"type": "devices.capabilities.on_off", "instance": "powerSwitch", "state": {"value": 1}},
                        {"type": "devices.capabilities.range", "instance": "brightness", "state": {"value": 64}},
                        {"type": "devices.capabilities.color_setting", "instance": "colorTemperatureK", "state": {"value": 0}}
                    ]
                }
            })))
            .mount(&server)
            .await;

        let state = mock_provider(&server)
            .await
            .get_state("11:22:33:44:55:66")
            .await
            .unwrap();

        assert!(state.on);
        assert_eq!(state.brightness, Some(64.0));
        // colorTemperatureK of 0 means "not in CT mode" — must not become mirek.
        assert_eq!(state.color_temp_mirek, None);
    }

    #[tokio::test]
    async fn get_state_offline_device_reports_off_and_unreachable() {
        // Offline devices return their *last known* power state — the API
        // happily says powerSwitch=1 for a light that's unplugged. The
        // `online: false` capability must win.
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(device_list_response()))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/device/state"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "requestId": "uuid-3",
                "msg": "success",
                "code": 200,
                "payload": {
                    "sku": "H6159",
                    "device": "AA:BB:CC:DD:EE:FF",
                    "capabilities": [
                        {"type": "devices.capabilities.online", "instance": "online", "state": {"value": false}},
                        {"type": "devices.capabilities.on_off", "instance": "powerSwitch", "state": {"value": 1}},
                        {"type": "devices.capabilities.range", "instance": "brightness", "state": {"value": 80}}
                    ]
                }
            })))
            .mount(&server)
            .await;

        let state = mock_provider(&server)
            .await
            .get_state("AA:BB:CC:DD:EE:FF")
            .await
            .unwrap();

        assert_eq!(state.reachable, Some(false));
        assert!(!state.on, "offline light must not report as on");
    }

    #[tokio::test]
    async fn get_state_online_device_reports_reachable() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(device_list_response()))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/device/state"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "requestId": "uuid-4",
                "msg": "success",
                "code": 200,
                "payload": {
                    "sku": "H6159",
                    "device": "AA:BB:CC:DD:EE:FF",
                    "capabilities": [
                        {"type": "devices.capabilities.online", "instance": "online", "state": {"value": true}},
                        {"type": "devices.capabilities.on_off", "instance": "powerSwitch", "state": {"value": 1}}
                    ]
                }
            })))
            .mount(&server)
            .await;

        let state = mock_provider(&server)
            .await
            .get_state("AA:BB:CC:DD:EE:FF")
            .await
            .unwrap();

        assert_eq!(state.reachable, Some(true));
        assert!(state.on);
    }

    #[tokio::test]
    async fn api_error_code_propagates_as_err() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 401, "message": "Unauthorized", "data": null
            })))
            .mount(&server)
            .await;

        assert!(mock_provider(&server).await.discover().await.is_err());
    }

    #[tokio::test]
    async fn discover_retries_after_a_rate_limit() {
        // Govee answers 429 on the first hit (a launch/sync burst), then 200.
        // The provider should back off (Retry-After: 0 here) and succeed, not fail.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(device_list_response()))
            .mount(&server)
            .await;

        let lights = mock_provider(&server).await.discover().await.unwrap();
        assert!(
            !lights.is_empty(),
            "discover should recover after a 429 retry"
        );
    }

    #[tokio::test]
    async fn discover_gives_up_after_persistent_rate_limit() {
        // Always 429 — after the bounded retries, surface the error rather than
        // hanging or looping forever.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
            .mount(&server)
            .await;
        assert!(mock_provider(&server).await.discover().await.is_err());
    }

    // ── Unified provider (LAN-preferred, cloud fallback) ─────────────────────

    use crate::providers::govee_lan::test_support::{spawn_mock_device, test_provider};
    use std::net::{Ipv4Addr, SocketAddr};
    use std::time::Duration;

    const MAC: &str = "AA:BB:CC:DD:EE:FF"; // the shared id used by both mocks

    /// Serialize the tests that clear/populate the process-wide LAN IP cache —
    /// under a parallel run, another test repopulating the cache between this
    /// test's `clear_lan_ip_cache` and its `set_state` flips the device's LAN
    /// eligibility mid-test (and a UDP send to a dead port "succeeds", so the
    /// cloud fallback never triggers).
    async fn lan_cache_lock() -> tokio::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await
    }

    /// A LAN transport pointed at a dead port: no device answers a scan, so every
    /// device is LAN-ineligible and control falls through to the cloud.
    fn dead_lan() -> GoveeLanProvider {
        let dead = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4);
        GoveeLanProvider::new_for_test(dead, Duration::from_millis(150))
    }

    #[tokio::test]
    async fn set_state_falls_back_to_cloud_when_device_not_on_lan() {
        // The headline guarantee: a Govee light that isn't reachable on the LAN
        // (didn't answer a scan) is still controllable — its command goes cloud.
        // (No clear_sku_cache: it's a process-wide map shared with the SKU-cache
        // test; this test's unique mock URL + identical SKU make a clear needless.)
        let _serial = lan_cache_lock().await;
        clear_lan_ip_cache().await;
        let server = MockServer::start().await;
        mount_control_mocks(&server).await;

        let provider = GoveeProvider::new(Some(mock_provider(&server).await), Some(dead_lan()));
        provider
            .set_state(
                MAC,
                &LightState {
                    on: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // The cloud control endpoint carried the command.
        let bodies = control_bodies(&server).await;
        assert_eq!(bodies.len(), 1, "expected one cloud control request");
        assert_eq!(
            bodies[0]["payload"]["capability"]["instance"],
            "powerSwitch"
        );
        assert_eq!(bodies[0]["payload"]["device"], MAC);
    }

    #[tokio::test]
    async fn set_state_prefers_lan_when_device_is_on_lan() {
        // A scanned device is controlled over the LAN; the cloud is never touched.
        let _serial = lan_cache_lock().await;
        clear_lan_ip_cache().await;
        let server = MockServer::start().await;
        mount_control_mocks(&server).await;
        let mock = spawn_mock_device().await;

        let provider = GoveeProvider::new(
            Some(mock_provider(&server).await),
            Some(test_provider(&mock)),
        );
        // Prime the address cache the way production does — the polling
        // manager's discover scan, not the control path.
        provider.discover().await.unwrap();
        provider
            .set_state(
                MAC,
                &LightState {
                    on: true,
                    brightness: Some(60.0),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;
        let got = mock.received.lock().await;
        let cmds: Vec<&str> = got.iter().map(|c| c["cmd"].as_str().unwrap()).collect();
        assert!(
            cmds.contains(&"turn"),
            "LAN device should get a turn: {cmds:?}"
        );
        assert!(cmds.contains(&"brightness"), "LAN brightness: {cmds:?}");
        assert!(
            control_bodies(&server).await.is_empty(),
            "the cloud must not be used when the device is on the LAN"
        );
    }

    #[tokio::test]
    async fn effect_routes_to_cloud_and_errors_without_one() {
        // Dynamic scenes only exist in the cloud catalogue, so an effect on a
        // LAN-only provider (no cloud key) is a clear error, never sent to the LAN.
        let _serial = lan_cache_lock().await;
        clear_lan_ip_cache().await;
        let mock = spawn_mock_device().await;
        let provider = GoveeProvider::new(None, Some(test_provider(&mock)));

        let err = provider
            .set_state(
                MAC,
                &LightState {
                    on: true,
                    effect: Some("candle".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("cloud"),
            "effect without cloud should mention the missing cloud key: {err}"
        );
        // And nothing was sent over the LAN.
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(mock.received.lock().await.is_empty());
    }

    #[tokio::test]
    async fn discover_surfaces_lan_only_device_without_cloud() {
        let _serial = lan_cache_lock().await;
        clear_lan_ip_cache().await;
        let mock = spawn_mock_device().await;
        let provider = GoveeProvider::new(None, Some(test_provider(&mock)));

        let lights = provider.discover().await.unwrap();
        assert_eq!(lights.len(), 1);
        // LAN-only devices are keyed by MAC (stable across IP changes).
        assert_eq!(lights[0].provider_id, MAC);
        assert!(lights[0].hw_id.is_some(), "MAC should yield an hw_id");
        assert!(lights[0].name.contains("H6159"));
        assert!(lights[0].capabilities.color_rgb);
        assert!(lights[0].capabilities.color_temperature);
        // Seen only on the LAN → reported as LAN-connected.
        assert_eq!(lights[0].state.transport.as_deref(), Some("lan"));
    }

    #[tokio::test]
    async fn discover_dedupes_cloud_and_lan_by_mac() {
        // The same physical device on both transports is one light — the cloud
        // row (richer name/caps) wins, and the LAN address is cached for control.
        // (discover uses fetch_devices directly, not the SKU cache, so no clear.)
        let _serial = lan_cache_lock().await;
        clear_lan_ip_cache().await;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(device_list_response()))
            .mount(&server)
            .await;
        let mock = spawn_mock_device().await;

        let provider = GoveeProvider::new(
            Some(mock_provider(&server).await),
            Some(test_provider(&mock)),
        );
        let lights = provider.discover().await.unwrap();
        assert_eq!(
            lights.len(),
            1,
            "same MAC on both transports collapses to one"
        );
        assert_eq!(lights[0].name, "Strip Lights", "cloud metadata wins");
        // The device answered the LAN scan, so it's surfaced as LAN-connected even
        // though its metadata came from the cloud.
        assert_eq!(lights[0].state.transport.as_deref(), Some("lan"));
    }

    // ── Cloud pacing ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn rate_limiter_spaces_a_burst_past_the_allowed_burst() {
        // The burst is free; everything past it is paced at RATE_PER_SEC. This is
        // what keeps a room command (one request per capability per light) from
        // arriving as a single spike the account answers with 429s.
        let limiter = RateLimiter::new();
        for _ in 0..RATE_BURST as usize {
            limiter.acquire().await;
        }
        let start = Instant::now();
        limiter.acquire().await;
        limiter.acquire().await;
        let elapsed = start.elapsed();
        let expected = Duration::from_secs_f64(2.0 / RATE_PER_SEC);
        assert!(
            elapsed >= expected.mul_f64(0.8),
            "two requests past the burst should be paced, took {elapsed:?}",
        );
    }

    #[tokio::test]
    async fn a_park_holds_every_caller_not_just_the_rate_limited_one() {
        // A 429 means the ACCOUNT is over its limit, so the sibling capability
        // commands of the same gesture must slow down too — parking only the
        // caller that saw the 429 just walks the rest into the same wall.
        let limiter = RateLimiter::new();
        limiter.park(Duration::from_millis(250)).await;
        let start = Instant::now();
        limiter.acquire().await;
        assert!(
            start.elapsed() >= Duration::from_millis(200),
            "a parked bucket must delay a fresh caller",
        );
    }

    #[test]
    fn retry_wait_reads_retry_after_seconds() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("retry-after", "3".parse().unwrap());
        assert_eq!(retry_wait_from(&h), Some(Duration::from_secs(3)));
    }

    #[test]
    fn retry_wait_falls_back_to_a_rate_limit_reset_header() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("x-ratelimit-reset", "2".parse().unwrap());
        assert_eq!(retry_wait_from(&h), Some(Duration::from_secs(2)));
    }

    #[test]
    fn retry_wait_clamps_an_absurd_header() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("retry-after", "86400".parse().unwrap());
        assert_eq!(retry_wait_from(&h), Some(Duration::from_secs(10)));
    }

    #[test]
    fn retry_wait_is_none_without_a_hint() {
        assert_eq!(retry_wait_from(&reqwest::header::HeaderMap::new()), None);
    }

    #[test]
    fn reset_delta_reads_an_absolute_epoch_as_a_delta() {
        // Govee has been seen sending the reset as a unix timestamp rather than
        // a delta; taking it literally would park control for 55 years.
        let epoch = Utc::now().timestamp() + 4;
        let delta = reset_delta(Some(epoch)).unwrap();
        assert!((3..=5).contains(&delta), "expected ~4s, got {delta}");
        // A plausible delta is left alone.
        assert_eq!(reset_delta(Some(5)), Some(5));
    }

    // ── Cloud command shaping ───────────────────────────────────────────────

    #[tokio::test]
    async fn set_state_off_sends_only_the_power_command() {
        // Every capability is a separate metered request. An "off" that also
        // ships the cached brightness and colour spends three times the quota to
        // say one thing — and hands the device a reason to light back up.
        let server = MockServer::start().await;
        mount_control_mocks(&server).await;

        mock_provider(&server)
            .await
            .set_state(
                "AA:BB:CC:DD:EE:FF",
                &LightState {
                    on: false,
                    brightness: Some(70.0),
                    color: Some(Color::from_rgb(255, 0, 0)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let bodies = control_bodies(&server).await;
        assert_eq!(bodies.len(), 1, "off must be one command: {bodies:?}");
        assert_eq!(
            bodies[0]["payload"]["capability"]["instance"],
            "powerSwitch"
        );
        assert_eq!(bodies[0]["payload"]["capability"]["value"], 0);
    }

    #[tokio::test]
    async fn a_devices_commands_are_sent_in_order_power_first() {
        // Concurrency here was the flakiness: simultaneous capability writes to
        // one device, and a cancelled sibling on the first failure. Ordered means
        // a batch that dies partway still delivered the power intent.
        let server = MockServer::start().await;
        mount_control_mocks(&server).await;

        mock_provider(&server)
            .await
            .set_state(
                "AA:BB:CC:DD:EE:FF",
                &LightState {
                    on: true,
                    brightness: Some(40.0),
                    color: Some(Color::from_rgb(0, 0, 255)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let instances: Vec<String> = control_bodies(&server)
            .await
            .iter()
            .map(|b| {
                b["payload"]["capability"]["instance"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(instances, vec!["powerSwitch", "brightness", "colorRgb"]);
    }

    #[tokio::test]
    async fn concurrent_writes_to_one_device_coalesce_to_the_newest() {
        // A brightness drag emits a write every ~200ms and each costs several
        // cloud requests, so the queue outruns the account's budget and the tail
        // 429s. Superseded writes are dropped: the drag costs a couple of
        // round-trips and lands on the value the finger stopped at.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(device_list_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/device/control"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(
                        serde_json::json!({"code": 200, "message": "success", "data": {}}),
                    )
                    // Slow enough that the later writes queue behind the first.
                    .set_delay(Duration::from_millis(400)),
            )
            .mount(&server)
            .await;

        let provider = Arc::new(mock_provider(&server).await);
        let write = |b: f32| {
            let p = provider.clone();
            tokio::spawn(async move {
                p.set_state(
                    "AA:BB:CC:DD:EE:FF",
                    &LightState {
                        on: true,
                        brightness: Some(b),
                        ..Default::default()
                    },
                )
                .await
            })
        };

        let first = write(10.0);
        // Let the first write take the device gate before the others queue.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let superseded = write(20.0);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let newest = write(30.0);

        for t in [first, superseded, newest] {
            t.await.unwrap().unwrap();
        }

        let sent: Vec<i64> = control_bodies(&server)
            .await
            .iter()
            .filter(|b| b["payload"]["capability"]["instance"] == "brightness")
            .map(|b| b["payload"]["capability"]["value"].as_i64().unwrap())
            .collect();
        assert!(
            sent.contains(&10) && sent.contains(&30),
            "the in-flight and the final value must both land: {sent:?}",
        );
        assert!(
            !sent.contains(&20),
            "the superseded mid-drag value must be dropped: {sent:?}",
        );
    }

    // ── LAN lease lifecycle ─────────────────────────────────────────────────

    #[tokio::test]
    async fn an_expired_lan_lease_falls_back_to_cloud() {
        // LAN control is fire-and-forget UDP: a send at a device that moved to a
        // new DHCP lease or was unplugged still returns Ok, so a never-expiring
        // address meant every command for that light vanished silently.
        let _serial = lan_cache_lock().await;
        clear_lan_ip_cache().await;
        let server = MockServer::start().await;
        mount_control_mocks(&server).await;
        // A live LAN device exists, but the cached lease for it is stale...
        let mock = spawn_mock_device().await;
        seed_lan_entry(
            &mac_hw_id(MAC).unwrap(),
            "127.0.0.1",
            LAN_TTL + Duration::from_secs(1),
        )
        .await;

        // ...and an expired lease is not used, so control must reach the light
        // over the cloud rather than shouting at the stale address. (The refresh
        // it kicks off is in the background and can't rescue this command.)
        let provider = GoveeProvider::new(Some(mock_provider(&server).await), Some(dead_lan()));
        provider
            .set_state(
                MAC,
                &LightState {
                    on: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(
            control_bodies(&server).await.len(),
            1,
            "an expired LAN lease must not swallow the command",
        );
        assert!(
            mock.received.lock().await.is_empty(),
            "nothing should have been sent to the stale address",
        );
    }

    #[tokio::test]
    async fn a_lan_miss_never_blocks_the_command_on_a_scan() {
        // A scan is a 1.5s multicast window and this sits on the control path.
        // A device with LAN Control off answers none of them, so blocking would
        // put 1.5s in front of every command it receives to learn nothing. The
        // command in hand goes cloud; the scan catches up behind it.
        let _serial = lan_cache_lock().await;
        clear_lan_ip_cache().await;
        let mock = spawn_mock_device().await;
        let provider = GoveeProvider::new(None, Some(test_provider(&mock)));

        let start = Instant::now();
        assert!(provider.lan_ip_for(MAC).await.is_none());
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "resolving must not wait on a scan, took {:?}",
            start.elapsed(),
        );

        // The background refresh lands, so the NEXT command gets the LAN.
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            provider.lan_ip_for(MAC).await.as_deref(),
            Some("127.0.0.1"),
            "the background scan should have filled the address cache",
        );
    }

    #[tokio::test]
    async fn lan_rescans_are_floored_to_one_per_interval() {
        // One scan refreshes every device, so a burst of misses — a room command
        // over several cloud-only lights — must share one window rather than
        // each opening its own.
        let _serial = lan_cache_lock().await;
        clear_lan_ip_cache().await;
        let provider = GoveeProvider::new(None, Some(dead_lan()));

        assert!(provider.lan_ip_for(MAC).await.is_none());
        tokio::time::sleep(Duration::from_millis(400)).await;
        let first = lan_scan_stamp().await.expect("the first miss should scan");

        for _ in 0..5 {
            assert!(provider.lan_ip_for(MAC).await.is_none());
        }
        tokio::time::sleep(Duration::from_millis(400)).await;

        assert_eq!(
            lan_scan_stamp().await,
            Some(first),
            "further misses inside the interval must not open another scan",
        );
    }

    #[tokio::test]
    async fn get_state_drops_a_lan_address_that_stops_answering() {
        // The LAN read is the only thing that can prove a device is still at its
        // address — control can't. When it goes silent the lease is revoked so
        // the next command goes straight to the cloud.
        let _serial = lan_cache_lock().await;
        clear_lan_ip_cache().await;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(device_list_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/device/state"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 200, "message": "success",
                "payload": { "capabilities": [
                    {"type": "devices.capabilities.online", "instance": "online", "state": {"value": true}},
                    {"type": "devices.capabilities.on_off", "instance": "powerSwitch", "state": {"value": 1}}
                ]}
            })))
            .mount(&server)
            .await;

        let key = mac_hw_id(MAC).unwrap();
        seed_lan_entry(&key, "127.0.0.1", Duration::ZERO).await;

        let provider = GoveeProvider::new(Some(mock_provider(&server).await), Some(dead_lan()));
        let state = provider.get_state(MAC).await.unwrap();

        assert_eq!(state.transport.as_deref(), Some("cloud"));
        assert!(
            !lan_ip_cache().read().await.contains_key(&key),
            "a silent device's LAN address must be dropped",
        );
    }

    #[tokio::test]
    async fn discover_marks_cloud_only_device_as_cloud() {
        // LAN is configured but the device doesn't answer a scan → it's reached
        // (and reported) over the cloud.
        let _serial = lan_cache_lock().await;
        clear_lan_ip_cache().await;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/devices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(device_list_response()))
            .mount(&server)
            .await;
        let provider = GoveeProvider::new(Some(mock_provider(&server).await), Some(dead_lan()));
        let lights = provider.discover().await.unwrap();
        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].state.transport.as_deref(), Some("cloud"));
    }

    #[test]
    fn factory_defaults_lan_on() {
        let f = GoveeProviderFactory;
        // LAN is on by default (0.0.0.0), so even an empty config builds (LAN-only).
        assert!(f.build("{}").is_ok(), "LAN-only by default");
        assert!(f.build(r#"{"api_key":"k"}"#).is_ok(), "cloud + default LAN");
        assert!(
            f.build(r#"{"api_key":"k","bind_addr":"192.168.1.5"}"#)
                .is_ok(),
            "cloud + explicit interface"
        );
        assert!(
            f.build(r#"{"bind_addr":"not-an-ip"}"#).is_err(),
            "malformed LAN address is rejected"
        );
    }
}
