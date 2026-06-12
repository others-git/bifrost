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
    /// device id → SKU, fetched lazily. Control and state requests REQUIRE
    /// the device's SKU in the payload; without it the API answers 400.
    sku_cache: tokio::sync::OnceCell<std::collections::HashMap<String, String>>,
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
            sku_cache: tokio::sync::OnceCell::new(),
        })
    }

    /// Fetch the account's device list (shared by discovery and SKU lookup).
    async fn fetch_devices(&self) -> Result<Vec<GoveeDevice>> {
        let resp: GoveeResponse<GoveeDeviceList> = self
            .client
            .get(format!("{}/user/devices", self.base_url))
            .send()
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

    /// The SKU for a device — required by control/state payloads.
    /// Cached per provider instance (one devices call, then free).
    async fn sku_for(&self, device_id: &str) -> Result<String> {
        let map = self
            .sku_cache
            .get_or_try_init(|| async {
                let devices = self.fetch_devices().await?;
                Ok::<_, anyhow::Error>(
                    devices
                        .into_iter()
                        .map(|d| (d.device, d.sku))
                        .collect::<std::collections::HashMap<_, _>>(),
                )
            })
            .await?;
        map.get(device_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown Govee device '{device_id}'"))
    }

    /// Test constructor: points at a local HTTP mock server instead of the Govee cloud.
    #[cfg(test)]
    pub fn new_for_test(base_url: impl Into<String>, api_key: impl AsRef<str>) -> Result<Self> {
        Self::new_with_base(api_key, base_url)
    }
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
        provider_id: d.device,
        provider: Provider::Govee,
        name: d.device_name,
        state: state.unwrap_or_default(),
        capabilities: LightCapabilities {
            dimmable: has_dim,
            color_rgb: has_color,
            color_temperature: has_color_temp,
            hue_gamut: None,
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
        Ok(self
            .fetch_devices()
            .await?
            .into_iter()
            .map(|d| govee_device_to_light(d, None))
            .collect())
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

            let resp: GoveeResponse<Value> = self
                .client
                .post(format!("{}/device/control", self.base_url))
                .json(&body)
                .send()
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

        let resp: GoveeResponse<GoveeStateData> = self
            .client
            .post(format!("{}/device/state", self.base_url))
            .json(&body)
            .send()
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
}
