//! Govee cloud API v2.
//!
//! Base URL: `https://openapi.api.govee.com/router/api/v1`
//! Authentication: `Govee-API-Key: <key>` header. Obtain a key from the Govee developer portal.
//!
//! Rate limit: ~10 req/s; 10,000 req/day per API key.

use crate::models::{Color, Light, LightCapabilities, LightState, Provider};
use crate::providers::LightProvider;
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::{Client, header};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

const BASE_URL: &str = "https://openapi.api.govee.com/router/api/v1";

pub struct GoveeProvider {
    client: Client,
    /// Base URL for the API; overridden in tests to point at a wiremock server.
    base_url: String,
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

/// Clear the process-wide SKU + scene caches. Tests only — the caches are keyed by
/// `base_url`, and wiremock reuses ephemeral ports across tests, so a stale entry
/// from a prior (dropped) mock server could otherwise satisfy a fresh lookup.
#[cfg(test)]
async fn clear_sku_cache() {
    sku_cache().write().await.clear();
    scene_cache().write().await.clear();
}

impl GoveeProvider {
    pub fn new(api_key: impl AsRef<str>) -> Result<Self> {
        Self::new_with_base(api_key, BASE_URL)
    }

    fn new_with_base(api_key: impl AsRef<str>, base_url: impl Into<String>) -> Result<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "Govee-API-Key",
            header::HeaderValue::from_str(api_key.as_ref())?,
        );
        // Bounded so a cloud outage fails the poll fast instead of hanging it.
        let client = Client::builder()
            .default_headers(headers)
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self {
            client,
            base_url: base_url.into(),
        })
    }

    /// Fetch the account's device list (shared by discovery and SKU lookup).
    async fn fetch_devices(&self) -> Result<Vec<GoveeDevice>> {
        let resp: GoveeResponse<GoveeDeviceList> =
            send_retrying(self.client.get(format!("{}/user/devices", self.base_url)))
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
        let resp: GoveeResponse<GoveeSceneData> = send_retrying(
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

    /// Test constructor: points at a local HTTP mock server instead of the Govee cloud.
    #[cfg(test)]
    pub fn new_for_test(base_url: impl Into<String>, api_key: impl AsRef<str>) -> Result<Self> {
        Self::new_with_base(api_key, base_url)
    }
}

/// Send a request, retrying on rate-limit (429) and transient/server errors.
///
/// Govee's cloud is aggressively rate-limited (~10 req/s; per-device limits too),
/// so a burst on app launch or a sync/prune sweep would otherwise fail outright
/// with 429 — the reported "flaky on launch / laggy controls". Back off and
/// retry a few times, honouring `Retry-After` when the server sends it.
async fn send_retrying(req: reqwest::RequestBuilder) -> reqwest::Result<reqwest::Response> {
    const MAX_RETRIES: u32 = 3;
    let mut attempt = 0u32;
    loop {
        // JSON / empty bodies are always cloneable; unwrap is safe here.
        let this = req
            .try_clone()
            .expect("Govee request body must be cloneable");
        match this.send().await {
            Ok(resp) => {
                let retryable = resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
                    || resp.status().is_server_error();
                if retryable && attempt < MAX_RETRIES {
                    let wait = retry_after(&resp).unwrap_or_else(|| backoff(attempt));
                    attempt += 1;
                    tokio::time::sleep(wait).await;
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

/// Exponential backoff: ~250ms, 500ms, 1s.
fn backoff(attempt: u32) -> std::time::Duration {
    std::time::Duration::from_millis(250u64 << attempt.min(3))
}

/// The server's `Retry-After` (whole seconds), if present and parseable.
fn retry_after(resp: &reqwest::Response) -> Option<std::time::Duration> {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(std::time::Duration::from_secs)
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
                // Convert Kelvin to mirek (1_000_000 / K). 0 means "not in
                // color-temperature mode" — checked_div skips it.
                if let Some(m) = v.as_u64().and_then(|k| 1_000_000u64.checked_div(k)) {
                    state.color_temp_mirek = Some(m as u16);
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
impl LightProvider for GoveeProvider {
    fn name(&self) -> &str {
        "govee"
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
        // Govee requires one command per capability.
        let mut commands: Vec<Value> = vec![json!({
            "type": "devices.capabilities.on_off",
            "instance": "powerSwitch",
            "value": if state.on { 1 } else { 0 }
        })];

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
            let kelvin = 1_000_000u32 / mirek.max(1) as u32;
            commands.push(json!({
                "type": "devices.capabilities.color_setting",
                "instance": "colorTemperatureK",
                "value": kelvin
            }));
        }

        // A dynamic scene ("effect") is applied by echoing back its opaque value
        // under the dynamic_scene capability. The frontend sends `effect` only on
        // an actual scene pick, so this doesn't ride along with colour tweaks.
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

        let sku = self.sku_for(provider_id).await?;

        for cmd in commands {
            let body = json!({
                "requestId": Uuid::new_v4().to_string(),
                "payload": {
                    "sku": sku,
                    "device": provider_id,
                    "capability": cmd
                }
            });

            let resp: GoveeResponse<Value> = send_retrying(
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
        }

        Ok(())
    }

    async fn get_state(&self, provider_id: &str) -> Result<LightState> {
        let sku = self.sku_for(provider_id).await?;
        let body = json!({
            "requestId": Uuid::new_v4().to_string(),
            "payload": { "sku": sku, "device": provider_id }
        });

        let resp: GoveeResponse<GoveeStateData> = send_retrying(
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

// ── Factory ─────────────────────────────────────────────────────────────────

use crate::providers::{CredentialField, FieldKind, ProviderFactory};

pub struct GoveeProviderFactory;

impl ProviderFactory for GoveeProviderFactory {
    fn provider_type(&self) -> &'static str {
        "govee"
    }

    fn display_name(&self) -> &'static str {
        "Govee (Cloud)"
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn crate::providers::LightProvider>> {
        let creds: serde_json::Value = serde_json::from_str(credentials_json)?;
        let api_key = creds["api_key"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("govee credentials missing api_key"))?
            .to_string();
        Ok(Box::new(GoveeProvider::new(&api_key)?))
    }

    fn credentials_schema(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "api_key",
            label: "API Key",
            kind: FieldKind::Password,
            required: true,
            hint: Some("Govee Home app → Profile → About Us → Apply for API Key"),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mock_provider(server: &MockServer) -> GoveeProvider {
        GoveeProvider::new_for_test(server.uri(), "test-govee-key").unwrap()
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
}
