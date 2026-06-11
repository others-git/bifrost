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
        let client = Client::builder().default_headers(headers).build()?;
        Ok(Self {
            client,
            base_url: base_url.into(),
        })
    }

    /// Test constructor: points at a local HTTP mock server instead of the Govee cloud.
    #[cfg(test)]
    pub fn new_for_test(base_url: impl Into<String>, api_key: impl AsRef<str>) -> Result<Self> {
        Self::new_with_base(api_key, base_url)
    }
}

// ── Wire types ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GoveeResponse<T> {
    code: i32,
    message: String,
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct GoveeDeviceList {
    devices: Vec<GoveeDevice>,
}

#[derive(Debug, Deserialize)]
struct GoveeDevice {
    device: String, // MAC-style device id
    #[serde(rename = "deviceName")]
    device_name: String,
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

fn parse_govee_state(caps: Vec<GoveeStateCapability>) -> LightState {
    let mut state = LightState::default();
    for cap in caps {
        match cap.instance.as_str() {
            "powerSwitch" => {
                state.on = cap.state.as_u64().unwrap_or(0) == 1;
            }
            "brightness" => {
                if let Some(v) = cap.state.as_u64() {
                    state.brightness = Some(v as f32);
                }
            }
            "colorRgb" => {
                if let Some(v) = cap.state.as_u64() {
                    let r = ((v >> 16) & 0xFF) as u8;
                    let g = ((v >> 8) & 0xFF) as u8;
                    let b = (v & 0xFF) as u8;
                    state.color = Some(Color::from_rgb(r, g, b));
                }
            }
            "colorTemperatureK" => {
                if let Some(k) = cap.state.as_u64() {
                    // Convert Kelvin to mirek (1_000_000 / K)
                    state.color_temp_mirek = Some((1_000_000 / k.max(1)) as u16);
                }
            }
            _ => {}
        }
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
        let resp: GoveeResponse<GoveeDeviceList> = self
            .client
            .get(format!("{}/user/devices", self.base_url))
            .send()
            .await
            .context("Govee discover request failed")?
            .error_for_status()?
            .json()
            .await?;

        if resp.code != 200 {
            bail!("Govee API error {}: {}", resp.code, resp.message);
        }

        Ok(resp
            .data
            .map(|d| d.devices)
            .unwrap_or_default()
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

        for cmd in commands {
            let body = json!({
                "requestId": Uuid::new_v4().to_string(),
                "payload": {
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
        let body = json!({
            "requestId": Uuid::new_v4().to_string(),
            "payload": { "device": provider_id }
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
            .data
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

    #[tokio::test]
    async fn set_state_on_off_sends_power_switch_command() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/device/control"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 200, "message": "success", "data": {}
            })))
            .mount(&server)
            .await;

        let state = LightState {
            on: true,
            ..Default::default()
        };
        mock_provider(&server)
            .await
            .set_state("AA:BB:CC:DD:EE:FF", &state)
            .await
            .unwrap();

        let received = server.received_requests().await.unwrap();
        // One request for power switch.
        assert_eq!(received.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["payload"]["capability"]["instance"], "powerSwitch");
        assert_eq!(body["payload"]["capability"]["value"], 1);
    }

    #[tokio::test]
    async fn set_state_with_brightness_sends_two_commands() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/device/control"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 200, "message": "success", "data": {}
            })))
            .mount(&server)
            .await;

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

        let received = server.received_requests().await.unwrap();
        // Two requests: power + brightness.
        assert_eq!(received.len(), 2);
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
