//! Philips Hue API v2 (local CLIP v2 REST + SSE event stream).
//!
//! Authentication: press the bridge link button, then POST to `https://<bridge>/api` with
//! `{"devicetype": "bifrost#server"}`. The response contains the `username` which is used
//! as the `hue-application-key` header on all subsequent requests.
//!
//! Base URL: `https://<bridge-ip>/clip/v2/resource`
//! The bridge uses a self-signed TLS cert; accept it or pin the bridge cert.

pub mod pairing;

use crate::models::{Color, HueGamut, Light, LightCapabilities, LightState, Provider};
use crate::providers::LightProvider;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::{Client, header};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Per-request deadline for plain REST calls. NOT applied to the SSE stream,
/// which is long-lived by design (its liveness is the connection manager's
/// health-check job).
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Minimum spacing between *write* commands dispatched to a single bridge. Hue's
/// CLIP v2 caps light commands at ~10/s per bridge and returns **429** past that,
/// which a fan-out (e.g. Restore Home touching every bulb) trivially trips when
/// it fires all the `set_state` calls at once. 120ms ≈ 8/s leaves headroom.
const MIN_WRITE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(120);

/// Reserve the next dispatch slot for `bridge_base` and return how long to wait
/// before sending so writes to one bridge stay paced under Hue's rate limit.
///
/// The gate is keyed by bridge base URL (independent bridges aren't cross-
/// throttled) and lives process-wide because every control call builds a fresh
/// `HueProvider`, so a per-instance limiter would share no state across the
/// lights of a fan-out. The brief async lock is held only for the slot
/// arithmetic — never across the HTTP request — so a slow/unreachable bulb
/// paces the queue without blocking it.
async fn reserve_write_slot(bridge_base: &str) -> std::time::Duration {
    use std::collections::HashMap;
    use std::sync::{Mutex as StdMutex, OnceLock};
    use std::time::Instant;
    use tokio::sync::Mutex as AsyncMutex;

    static GATES: OnceLock<StdMutex<HashMap<String, std::sync::Arc<AsyncMutex<Instant>>>>> =
        OnceLock::new();
    let gate = {
        let map = GATES.get_or_init(|| StdMutex::new(HashMap::new()));
        let mut guard = map.lock().unwrap();
        guard
            .entry(bridge_base.to_string())
            .or_insert_with(|| {
                std::sync::Arc::new(AsyncMutex::new(Instant::now() - MIN_WRITE_INTERVAL))
            })
            .clone()
    };

    let mut next = gate.lock().await;
    let now = Instant::now();
    // Earliest this caller may dispatch: the later of "now" and the reserved slot.
    let start = (*next).max(now);
    *next = start + MIN_WRITE_INTERVAL;
    start.saturating_duration_since(now)
}

pub struct HueProvider {
    client: Client,
    /// Base URL for the bridge, e.g. `https://192.168.1.10`.
    /// All resource paths are appended to this.
    bridge_base: String,
}

impl HueProvider {
    pub fn new(bridge_ip: impl Into<String>, app_key: impl Into<String>) -> Result<Self> {
        let ip = bridge_ip.into();
        // Accept a full base URL too (used by tests and unusual setups).
        let bridge_base = if ip.starts_with("http://") || ip.starts_with("https://") {
            ip
        } else {
            format!("https://{ip}")
        };
        Self::new_with_base(bridge_base, app_key.into(), true)
    }

    /// Internal constructor that accepts a full base URL and a flag for TLS cert acceptance.
    /// Used by tests via `new_for_test`.
    fn new_with_base(
        bridge_base: impl Into<String>,
        app_key: impl Into<String>,
        accept_invalid_certs: bool,
    ) -> Result<Self> {
        let app_key = app_key.into();
        // Shared, pooled client keyed by app-key + TLS mode (the only things that
        // vary between bridges' clients; the base URL lives on the struct). Keeps
        // the bridge connection warm across per-request rebuilds rather than
        // re-handshaking every control. See [`crate::providers::cached_client`].
        let client = crate::providers::cached_client(
            &format!("hue:{accept_invalid_certs}:{app_key}"),
            || {
                let mut headers = header::HeaderMap::new();
                headers.insert(
                    "hue-application-key",
                    header::HeaderValue::from_str(&app_key)?,
                );
                // connect_timeout only bounds connection establishment, so it is safe
                // for the long-lived SSE stream; without it a powered-off bridge (or a
                // host that rebooted before the network came up) leaves connect
                // attempts hanging in TCP retries for minutes per attempt.
                Ok(Client::builder()
                    .default_headers(headers)
                    .danger_accept_invalid_certs(accept_invalid_certs)
                    .connect_timeout(std::time::Duration::from_secs(10))
                    .build()?)
            },
        )?;
        Ok(Self {
            client,
            bridge_base: bridge_base.into(),
        })
    }

    /// Build from decrypted credentials JSON `{"bridge_ip":"...","app_key":"..."}`.
    pub fn from_credentials(creds_json: &str) -> Result<Self> {
        let v: serde_json::Value = serde_json::from_str(creds_json)?;
        let bridge_ip = v["bridge_ip"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("hue credentials missing bridge_ip"))?;
        let app_key = v["app_key"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("hue credentials missing app_key"))?;
        Self::new(bridge_ip, app_key)
    }

    /// Test constructor: takes a plain-HTTP mock server base URL (e.g. `http://127.0.0.1:PORT`).
    #[cfg(test)]
    pub fn new_for_test(base_url: impl Into<String>, app_key: impl Into<String>) -> Result<Self> {
        Self::new_with_base(base_url, app_key, false)
    }

    fn resource_url(&self, path: &str) -> String {
        format!("{}/clip/v2/resource{}", self.bridge_base, path)
    }
}

// ── Wire types ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct HueListResponse<T> {
    data: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct HueLightResource {
    id: String,
    metadata: HueMetadata,
    on: HueOn,
    #[serde(default)]
    dimming: Option<HueDimming>,
    #[serde(default)]
    color: Option<HueColor>,
    #[serde(default)]
    color_temperature: Option<HueColorTemperature>,
    #[serde(default)]
    color_gamut_type: Option<String>,
    #[serde(default)]
    effects: Option<HueEffects>,
}

/// CLIP v2 dynamic effects on a light: `status` is the running effect
/// ("no_effect" when idle); `status_values` enumerates what this light supports.
#[derive(Debug, Deserialize)]
struct HueEffects {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    status_values: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct HueMetadata {
    name: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct HueOn {
    on: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct HueDimming {
    /// 0.0–100.0
    brightness: f32,
}

#[derive(Debug, Deserialize, Serialize)]
struct HueColor {
    xy: HueXy,
    /// CLIP v2 nests the gamut type inside the color object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gamut_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct HueXy {
    x: f32,
    y: f32,
}

#[derive(Debug, Deserialize, Serialize)]
struct HueColorTemperature {
    /// Mirek (153–500). None if light is in RGB mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    mirek: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct HueResourceRef {
    rid: String,
    rtype: String,
}

#[derive(Debug, Deserialize)]
struct HueGroupResource {
    id: String,
    metadata: HueMetadata,
    #[serde(default)]
    children: Vec<HueResourceRef>,
    /// Rooms/zones carry a grouped_light service — the native control handle.
    #[serde(default)]
    services: Vec<HueResourceRef>,
}

impl HueGroupResource {
    fn grouped_light_rid(&self) -> Option<String> {
        self.services
            .iter()
            .find(|s| s.rtype == "grouped_light")
            .map(|s| s.rid.clone())
    }
}

#[derive(Debug, Deserialize)]
struct HueDeviceResource {
    id: String,
    #[serde(default)]
    services: Vec<HueResourceRef>,
}

/// A device's Zigbee radio service — carries its MAC, the basis for the
/// cross-provider `hw_id`.
#[derive(Debug, Deserialize)]
struct HueZigbeeResource {
    id: String,
    #[serde(default)]
    mac_address: Option<String>,
}

#[derive(Debug, Serialize, Default)]
struct HuePutLight {
    #[serde(skip_serializing_if = "Option::is_none")]
    on: Option<HueOn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dimming: Option<HueDimming>,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<HueColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    color_temperature: Option<HueColorTemperature>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effects: Option<HueEffectPut>,
}

#[derive(Debug, Serialize)]
struct HueEffectPut {
    effect: String,
}

// ── Conversion helpers ──────────────────────────────────────────────────────

fn gamut_from_str(s: &str) -> Option<HueGamut> {
    match s {
        "A" => Some(HueGamut::A),
        "B" => Some(HueGamut::B),
        "C" => Some(HueGamut::C),
        _ => None,
    }
}

fn hue_resource_to_light(r: HueLightResource, hw_id: Option<String>) -> Light {
    // CLIP v2 puts gamut_type inside `color`; the top-level field is kept as a
    // fallback for older firmware.
    let gamut = r
        .color
        .as_ref()
        .and_then(|c| c.gamut_type.as_deref())
        .or(r.color_gamut_type.as_deref())
        .and_then(gamut_from_str);
    // A light that reports a color object at all is full-RGB capable.
    let has_color = r.color.is_some();
    // Dynamic effects: the running one → state, the supported set → capability.
    let effect = r.effects.as_ref().and_then(|e| e.status.clone());
    let effects = r.effects.map(|e| e.status_values).unwrap_or_default();
    let color = r.color.map(|c| {
        let brightness = r
            .dimming
            .as_ref()
            .map(|d| d.brightness / 100.0)
            .unwrap_or(1.0);
        Color {
            x: c.xy.x,
            y: c.xy.y,
            brightness,
        }
    });

    Light {
        id: Uuid::new_v4(),
        provider_id: r.id,
        provider: Provider::Hue,
        name: r.metadata.name,
        state: LightState {
            on: r.on.on,
            brightness: r.dimming.map(|d| d.brightness),
            color,
            color_temp_mirek: r.color_temperature.and_then(|ct| ct.mirek),
            reachable: None,
            effect,
            transport: None,
            ip: None, // Zigbee bulbs have no IP of their own (behind the bridge)
        },
        capabilities: LightCapabilities {
            dimmable: true,
            color_rgb: has_color,
            color_temperature: true,
            hue_gamut: gamut,
            effects,
            // Hue gradient lightstrips expose segments via a separate capability;
            // not modelled yet (tracked as a follow-up). Plain bulbs have none.
            segments: None,
        },
        last_seen: Utc::now(),
        hw_id,
    }
}

// ── Provider impl ───────────────────────────────────────────────────────────

#[async_trait]
impl LightProvider for HueProvider {
    fn name(&self) -> &str {
        "hue"
    }

    async fn discover(&self) -> Result<Vec<Light>> {
        let url = self.resource_url("/light");
        let resp: HueListResponse<HueLightResource> = self
            .client
            .get(&url)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .context("Hue discover request failed")?
            .error_for_status()?
            .json()
            .await?;

        let hw = self.light_hw_ids().await;
        Ok(resp
            .data
            .into_iter()
            .map(|r| {
                let hw_id = hw.get(&r.id).cloned();
                hue_resource_to_light(r, hw_id)
            })
            .collect())
    }

    async fn set_state(&self, provider_id: &str, state: &LightState) -> Result<()> {
        // Pace writes to the bridge so a fan-out doesn't trip Hue's 429 limit.
        let wait = reserve_write_slot(&self.bridge_base).await;
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }

        let url = self.resource_url(&format!("/light/{provider_id}"));
        let body = HuePutLight {
            on: Some(HueOn { on: state.on }),
            dimming: state.brightness.map(|b| HueDimming { brightness: b }),
            color: state.color.as_ref().map(|c| HueColor {
                xy: HueXy { x: c.x, y: c.y },
                gamut_type: None,
            }),
            color_temperature: state
                .color_temp_mirek
                .map(|m| HueColorTemperature { mirek: Some(m) }),
            effects: state
                .effect
                .as_ref()
                .map(|e| HueEffectPut { effect: e.clone() }),
        };

        self.client
            .put(&url)
            .json(&body)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .context("Hue set_state request failed")?
            .error_for_status()?;

        Ok(())
    }

    async fn get_state(&self, provider_id: &str) -> Result<LightState> {
        let url = self.resource_url(&format!("/light/{provider_id}"));
        let resp: HueListResponse<HueLightResource> = self
            .client
            .get(&url)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let resource = resp.data.into_iter().next().context("light not found")?;
        Ok(hue_resource_to_light(resource, None).state)
    }

    /// Hue rooms + zones. Room children are *devices* — their light service
    /// IDs come from `/resource/device`. Zone children are lights directly.
    async fn discover_groups(&self) -> Result<Vec<crate::providers::ProviderGroup>> {
        use crate::providers::ProviderGroup;
        use std::collections::HashMap;

        let rooms: HueListResponse<HueGroupResource> = self
            .client
            .get(self.resource_url("/room"))
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .context("Hue room request failed")?
            .error_for_status()?
            .json()
            .await?;

        let devices: HueListResponse<HueDeviceResource> = self
            .client
            .get(self.resource_url("/device"))
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .context("Hue device request failed")?
            .error_for_status()?
            .json()
            .await?;

        let zones: HueListResponse<HueGroupResource> = self
            .client
            .get(self.resource_url("/zone"))
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .context("Hue zone request failed")?
            .error_for_status()?
            .json()
            .await?;

        // device id → light resource ids
        let device_lights: HashMap<String, Vec<String>> = devices
            .data
            .into_iter()
            .map(|d| {
                let lights = d
                    .services
                    .into_iter()
                    .filter(|s| s.rtype == "light")
                    .map(|s| s.rid)
                    .collect();
                (d.id, lights)
            })
            .collect();

        let mut groups = Vec::new();

        for room in rooms.data {
            let members: Vec<String> = room
                .children
                .iter()
                .filter(|c| c.rtype == "device")
                .filter_map(|c| device_lights.get(&c.rid))
                .flatten()
                .cloned()
                .collect();
            if !members.is_empty() {
                groups.push(ProviderGroup {
                    provider_group_id: room.id.clone(),
                    name: room.metadata.name.clone(),
                    member_device_ids: members,
                    grouped_ref: room.grouped_light_rid(),
                });
            }
        }

        for zone in zones.data {
            let members: Vec<String> = zone
                .children
                .iter()
                .filter(|c| c.rtype == "light")
                .map(|c| c.rid.clone())
                .collect();
            if !members.is_empty() {
                groups.push(ProviderGroup {
                    provider_group_id: zone.id.clone(),
                    name: zone.metadata.name.clone(),
                    member_device_ids: members,
                    grouped_ref: zone.grouped_light_rid(),
                });
            }
        }

        Ok(groups)
    }

    /// Native group control: one PUT to the room/zone's grouped_light —
    /// exactly how the Hue app switches a whole room.
    async fn set_group_state(&self, grouped_ref: &str, state: &LightState) -> Result<bool> {
        let url = format!(
            "{}/clip/v2/resource/grouped_light/{grouped_ref}",
            self.bridge_base
        );
        let body = HuePutLight {
            on: Some(HueOn { on: state.on }),
            dimming: state.brightness.map(|b| HueDimming { brightness: b }),
            color: state.color.as_ref().map(|c| HueColor {
                xy: HueXy { x: c.x, y: c.y },
                gamut_type: None,
            }),
            color_temperature: state
                .color_temp_mirek
                .map(|m| HueColorTemperature { mirek: Some(m) }),
            // Effects aren't a grouped_light attribute — applied per-light only.
            effects: None,
        };

        self.client
            .put(&url)
            .json(&body)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .context("Hue grouped_light request failed")?
            .error_for_status()?;

        Ok(true)
    }
}

impl HueProvider {
    pub async fn ping(&self) -> Result<()> {
        let url = self.resource_url("/device");
        self.client.get(&url).send().await?.error_for_status()?;
        Ok(())
    }

    /// Map each light resource id → its Zigbee MAC (`hw_id`) for cross-provider
    /// de-dup. Best-effort: any failure yields an empty map so discovery still
    /// works (de-dup just doesn't fire). Joins `/resource/device` (a device's
    /// `light` services + its `zigbee_connectivity` service) with
    /// `/resource/zigbee_connectivity` (the MAC).
    async fn light_hw_ids(&self) -> std::collections::HashMap<String, String> {
        match self.fetch_light_hw_ids().await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("Hue hardware-id fetch failed ({e:#}); de-dup disabled this run");
                std::collections::HashMap::new()
            }
        }
    }

    async fn fetch_light_hw_ids(&self) -> Result<std::collections::HashMap<String, String>> {
        use std::collections::HashMap;

        let devices: HueListResponse<HueDeviceResource> = self
            .client
            .get(self.resource_url("/device"))
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .context("Hue device request failed")?
            .error_for_status()?
            .json()
            .await?;

        let zigbee: HueListResponse<HueZigbeeResource> = self
            .client
            .get(self.resource_url("/zigbee_connectivity"))
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .context("Hue zigbee_connectivity request failed")?
            .error_for_status()?
            .json()
            .await?;

        // zigbee service id → normalized MAC
        let zb_mac: HashMap<String, String> = zigbee
            .data
            .into_iter()
            .filter_map(|z| {
                let mac = z.mac_address?;
                crate::providers::mac_hw_id(&mac).map(|hw| (z.id, hw))
            })
            .collect();

        // For each device, attach its MAC to every light service it exposes.
        let mut out = HashMap::new();
        for d in devices.data {
            let Some(mac) = d
                .services
                .iter()
                .find(|s| s.rtype == "zigbee_connectivity")
                .and_then(|s| zb_mac.get(&s.rid))
            else {
                continue;
            };
            for s in &d.services {
                if s.rtype == "light" {
                    out.insert(s.rid.clone(), mac.clone());
                }
            }
        }
        Ok(out)
    }

    pub async fn event_stream(
        &self,
    ) -> Result<
        impl futures_util::Stream<
            Item = Result<
                eventsource_stream::Event,
                eventsource_stream::EventStreamError<reqwest::Error>,
            >,
        >,
    > {
        use eventsource_stream::Eventsource;

        let url = format!("{}/eventstream/clip/v2", self.bridge_base);
        let resp = self
            .client
            .get(&url)
            .header("Accept", "text/event-stream")
            .send()
            .await
            .context("Hue event stream connect failed")?
            .error_for_status()?;

        Ok(resp.bytes_stream().eventsource())
    }
}

// ── Factory ─────────────────────────────────────────────────────────────────

use crate::providers::discovery::{DeviceDiscovery, SsdpDiscovery};
use crate::providers::{CredentialField, FieldKind, ProviderFactory};

pub struct HueProviderFactory;

impl ProviderFactory for HueProviderFactory {
    fn provider_type(&self) -> &'static str {
        "hue"
    }

    fn display_name(&self) -> &'static str {
        "Philips Hue"
    }

    fn discoverer(&self) -> Option<Box<dyn DeviceDiscovery>> {
        // Hue bridges answer SSDP and tag themselves "IpBridge" in the SERVER
        // header; the LOCATION carries the bridge IP for the bridge_ip field.
        Some(Box::new(SsdpDiscovery::new(
            "upnp:rootdevice",
            "ipbridge",
            "Hue bridge",
            "bridge_ip",
        )))
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn crate::providers::LightProvider>> {
        let creds: serde_json::Value = serde_json::from_str(credentials_json)?;
        let bridge_ip = creds["bridge_ip"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("hue credentials missing bridge_ip"))?
            .to_string();
        let app_key = creds["app_key"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("hue credentials missing app_key"))?
            .to_string();
        Ok(Box::new(HueProvider::new(bridge_ip, app_key)?))
    }

    fn connection_mode(&self) -> crate::providers::ConnectionMode {
        crate::providers::ConnectionMode::Sse
    }

    fn credentials_schema(&self) -> &'static [CredentialField] {
        &[
            CredentialField {
                name: "bridge_ip",
                label: "Bridge IP Address",
                kind: FieldKind::IpAddress,
                required: true,
                hint: Some("Find this in the Hue app → Settings → My Hue system → Bridge"),
            },
            CredentialField {
                name: "app_key",
                label: "Application Key",
                kind: FieldKind::Password,
                required: true,
                hint: Some(
                    "Press the link button, then POST to https://<bridge-ip>/api with {\"devicetype\":\"bifrost#server\"}",
                ),
            },
        ]
    }
}

/// Extract a partial state patch from a Hue SSE event data item.
/// Hue events only carry the fields that changed — absent fields stay `None`
/// so the merge leaves existing state untouched.
pub fn parse_patch_from_event(item: &serde_json::Value) -> crate::models::LightStatePatch {
    let on = item
        .get("on")
        .and_then(|o| o.get("on"))
        .and_then(|v| v.as_bool());
    let brightness = item
        .get("dimming")
        .and_then(|d| d.get("brightness"))
        .and_then(|v| v.as_f64())
        .map(|b| b as f32);
    let color = item.get("color").and_then(|c| c.get("xy")).map(|xy| {
        let x = xy.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let y = xy.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        crate::models::Color {
            x,
            y,
            brightness: brightness.unwrap_or(100.0) / 100.0,
        }
    });
    let color_temp_mirek = item
        .get("color_temperature")
        .and_then(|ct| ct.get("mirek"))
        .and_then(|v| v.as_u64())
        .map(|m| m as u16);
    // A `status` change on the light's effects object pushes the running effect.
    let effect = item
        .get("effects")
        .and_then(|e| e.get("status"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    crate::models::LightStatePatch {
        on,
        brightness,
        color,
        color_temp_mirek,
        reachable: None,
        effect,
        transport: None,
        ip: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mock_provider(server: &MockServer) -> HueProvider {
        HueProvider::new_for_test(server.uri(), "test-app-key").unwrap()
    }

    #[tokio::test]
    async fn discover_parses_light_list() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/clip/v2/resource/light"))
            .and(header("hue-application-key", "test-app-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{
                    "id": "abc-123",
                    "metadata": {"name": "Bedroom"},
                    "on": {"on": true},
                    "dimming": {"brightness": 80.0},
                    "color_gamut_type": "C"
                }]
            })))
            .mount(&server)
            .await;

        let lights = mock_provider(&server).await.discover().await.unwrap();

        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].name, "Bedroom");
        assert_eq!(lights[0].provider_id, "abc-123");
        assert!(lights[0].state.on);
        assert_eq!(lights[0].state.brightness, Some(80.0));
        assert_eq!(lights[0].capabilities.hue_gamut, Some(HueGamut::C));
    }

    #[tokio::test]
    async fn discover_populates_hw_id_from_zigbee_mac() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/clip/v2/resource/light"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{
                    "id": "light-1",
                    "metadata": {"name": "Bedroom"},
                    "on": {"on": true}
                }]
            })))
            .mount(&server)
            .await;
        // The device joins its light service to its zigbee_connectivity service…
        Mock::given(method("GET"))
            .and(path("/clip/v2/resource/device"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{
                    "id": "dev-1",
                    "services": [
                        {"rid": "light-1", "rtype": "light"},
                        {"rid": "zb-1", "rtype": "zigbee_connectivity"}
                    ]
                }]
            })))
            .mount(&server)
            .await;
        // …which carries the MAC.
        Mock::given(method("GET"))
            .and(path("/clip/v2/resource/zigbee_connectivity"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "zb-1", "mac_address": "00:17:88:01:0b:12:34:56"}]
            })))
            .mount(&server)
            .await;

        let lights = mock_provider(&server).await.discover().await.unwrap();
        assert_eq!(lights[0].hw_id.as_deref(), Some("mac:001788010b123456"));
    }

    #[tokio::test]
    async fn discover_hw_id_is_none_when_zigbee_lookup_fails() {
        // Only /light is mocked; the device/zigbee fetches 404, so discovery
        // still succeeds and the light simply carries no hw_id.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/clip/v2/resource/light"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "light-1", "metadata": {"name": "X"}, "on": {"on": false}}]
            })))
            .mount(&server)
            .await;

        let lights = mock_provider(&server).await.discover().await.unwrap();
        assert_eq!(lights.len(), 1);
        assert!(lights[0].hw_id.is_none());
    }

    #[tokio::test]
    async fn discover_returns_empty_list_when_bridge_has_no_lights() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/clip/v2/resource/light"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
            .mount(&server)
            .await;

        let lights = mock_provider(&server).await.discover().await.unwrap();
        assert!(lights.is_empty());
    }

    #[tokio::test]
    async fn set_state_sends_correct_payload() {
        let server = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path("/clip/v2/resource/light/abc-123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
            .mount(&server)
            .await;

        let state = LightState {
            on: true,
            brightness: Some(50.0),
            color: None,
            color_temp_mirek: Some(370),
            reachable: None,
            effect: None,
            transport: None,
            ip: None,
        };
        mock_provider(&server)
            .await
            .set_state("abc-123", &state)
            .await
            .unwrap();

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["on"]["on"], true);
        assert_eq!(body["dimming"]["brightness"], 50.0);
        assert_eq!(body["color_temperature"]["mirek"], 370);
    }

    #[tokio::test]
    async fn discover_reads_effects_capability_and_active() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/clip/v2/resource/light"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{
                    "id": "fx-1",
                    "metadata": {"name": "Candle"},
                    "on": {"on": true},
                    "effects": { "status": "candle", "status_values": ["no_effect", "candle", "fire"] }
                }]
            })))
            .mount(&server)
            .await;
        let lights = mock_provider(&server).await.discover().await.unwrap();
        assert_eq!(lights[0].state.effect.as_deref(), Some("candle"));
        assert_eq!(
            lights[0].capabilities.effects,
            vec!["no_effect", "candle", "fire"]
        );
    }

    #[tokio::test]
    async fn set_state_sends_the_effect() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/clip/v2/resource/light/abc-123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
            .mount(&server)
            .await;
        let state = LightState {
            on: true,
            effect: Some("fire".to_string()),
            ..Default::default()
        };
        mock_provider(&server)
            .await
            .set_state("abc-123", &state)
            .await
            .unwrap();
        let received = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["effects"]["effect"], "fire");
    }

    #[tokio::test]
    async fn concurrent_set_state_are_paced_under_rate_limit() {
        // Five writes fired at once to one bridge must be spaced out (not all
        // dispatched simultaneously), so Hue's ~10/s limit isn't tripped.
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
            .mount(&server)
            .await;

        let state = LightState {
            on: true,
            brightness: None,
            color: None,
            color_temp_mirek: None,
            reachable: None,
            effect: None,
            transport: None,
            ip: None,
        };

        let start = std::time::Instant::now();
        let jobs = (0..5).map(|i| {
            let uri = server.uri();
            let st = state.clone();
            async move {
                HueProvider::new_for_test(uri, "test-app-key")
                    .unwrap()
                    .set_state(&format!("light-{i}"), &st)
                    .await
                    .unwrap();
            }
        });
        futures_util::future::join_all(jobs).await;

        // 5 writes paced at 120ms ⇒ the last starts ≥ 4*120ms after the first.
        assert!(
            start.elapsed() >= MIN_WRITE_INTERVAL * 4,
            "writes were not paced: {:?}",
            start.elapsed()
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 5);
    }

    #[tokio::test]
    async fn get_state_returns_current_light_state() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/clip/v2/resource/light/xyz-789"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{
                    "id": "xyz-789",
                    "metadata": {"name": "Kitchen"},
                    "on": {"on": false},
                    "dimming": {"brightness": 10.0}
                }]
            })))
            .mount(&server)
            .await;

        let state = mock_provider(&server)
            .await
            .get_state("xyz-789")
            .await
            .unwrap();
        assert!(!state.on);
        assert_eq!(state.brightness, Some(10.0));
    }

    #[tokio::test]
    async fn bridge_error_response_propagates_as_err() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/clip/v2/resource/light"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        assert!(mock_provider(&server).await.discover().await.is_err());
    }

    async fn mount_group_mocks(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/clip/v2/resource/room"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{
                    "id": "room-1",
                    "metadata": {"name": "Living Room"},
                    "children": [
                        {"rid": "dev-1", "rtype": "device"},
                        {"rid": "dev-2", "rtype": "device"}
                    ],
                    "services": [{"rid": "gl-room-1", "rtype": "grouped_light"}]
                }]
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/clip/v2/resource/device"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"id": "dev-1", "services": [
                        {"rid": "light-1", "rtype": "light"},
                        {"rid": "zb-1", "rtype": "zigbee_connectivity"}
                    ]},
                    {"id": "dev-2", "services": [{"rid": "light-2", "rtype": "light"}]}
                ]
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/clip/v2/resource/zone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{
                    "id": "zone-1",
                    "metadata": {"name": "Downstairs"},
                    "children": [{"rid": "light-1", "rtype": "light"}]
                }]
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn discover_groups_maps_room_devices_to_lights() {
        let server = MockServer::start().await;
        mount_group_mocks(&server).await;

        let groups = mock_provider(&server)
            .await
            .discover_groups()
            .await
            .unwrap();

        let room = groups.iter().find(|g| g.name == "Living Room").unwrap();
        assert_eq!(room.member_device_ids, vec!["light-1", "light-2"]);
        assert_eq!(room.provider_group_id, "room-1");
        assert_eq!(room.grouped_ref.as_deref(), Some("gl-room-1"));
    }

    #[tokio::test]
    async fn set_group_state_puts_grouped_light() {
        let server = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path("/clip/v2/resource/grouped_light/gl-room-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
            .mount(&server)
            .await;

        let state = LightState {
            on: true,
            brightness: Some(40.0),
            ..Default::default()
        };
        let native = mock_provider(&server)
            .await
            .set_group_state("gl-room-1", &state)
            .await
            .unwrap();
        assert!(native, "Hue must report native group control");

        let received = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["on"]["on"], true);
        assert_eq!(body["dimming"]["brightness"], 40.0);
    }

    #[tokio::test]
    async fn discover_groups_includes_zones_with_direct_light_children() {
        let server = MockServer::start().await;
        mount_group_mocks(&server).await;

        let groups = mock_provider(&server)
            .await
            .discover_groups()
            .await
            .unwrap();

        let zone = groups.iter().find(|g| g.name == "Downstairs").unwrap();
        assert_eq!(zone.member_device_ids, vec!["light-1"]);
    }

    #[tokio::test]
    async fn discover_groups_skips_empty_rooms() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/clip/v2/resource/room"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "r", "metadata": {"name": "Empty"}, "children": []}]
            })))
            .mount(&server)
            .await;
        for p in ["/clip/v2/resource/device", "/clip/v2/resource/zone"] {
            Mock::given(method("GET"))
                .and(path(p))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})),
                )
                .mount(&server)
                .await;
        }

        let groups = mock_provider(&server)
            .await
            .discover_groups()
            .await
            .unwrap();
        assert!(groups.is_empty());
    }

    #[test]
    fn parse_sse_event_extracts_on_off() {
        let item = serde_json::json!({"on": {"on": true}});
        let patch = parse_patch_from_event(&item);
        assert_eq!(patch.on, Some(true));
    }

    #[test]
    fn parse_sse_event_extracts_color_temp() {
        let item = serde_json::json!({"color_temperature": {"mirek": 250}});
        let patch = parse_patch_from_event(&item);
        assert_eq!(patch.color_temp_mirek, Some(250));
    }

    #[test]
    fn brightness_only_event_leaves_on_state_untouched() {
        // Regression: this used to produce a full state with on=false,
        // flipping lights "off" in the UI whenever brightness changed.
        let item = serde_json::json!({"dimming": {"brightness": 42.0}});
        let patch = parse_patch_from_event(&item);
        assert_eq!(patch.on, None);
        assert_eq!(patch.brightness, Some(42.0));

        let mut state = crate::models::LightState {
            on: true,
            brightness: Some(80.0),
            ..Default::default()
        };
        patch.apply_to(&mut state);
        assert!(state.on, "merge must not turn the light off");
        assert_eq!(state.brightness, Some(42.0));
    }

    #[tokio::test]
    async fn discover_reads_gamut_from_color_object() {
        // CLIP v2 nests gamut_type inside color; color presence => RGB-capable.
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/clip/v2/resource/light"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{
                    "id": "abc",
                    "metadata": {"name": "Color Bulb"},
                    "on": {"on": true},
                    "dimming": {"brightness": 50.0},
                    "color": {
                        "xy": {"x": 0.45, "y": 0.41},
                        "gamut_type": "C"
                    }
                }]
            })))
            .mount(&server)
            .await;

        let lights = mock_provider(&server).await.discover().await.unwrap();
        assert!(
            lights[0].capabilities.color_rgb,
            "color picker capability missing"
        );
        assert_eq!(lights[0].capabilities.hue_gamut, Some(HueGamut::C));
    }
}
