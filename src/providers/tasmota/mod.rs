//! Tasmota HTTP REST integration.
//!
//! Tasmota is open-source firmware for ESP-based IoT devices (bulbs, strips, plugs).
//! <https://tasmota.github.io/docs/>
//!
//! All commands go through a single endpoint:
//! - `GET /cm?cmnd=<command>` — query or control
//!
//! Key commands used:
//! - `Status 0`   — device name + capabilities snapshot
//! - `State`      — current on/off, brightness (0–100), color (hex RRGGBB[WW])
//! - `Backlog …`  — execute multiple `;`-separated commands atomically

use crate::models::{Color, Light, LightCapabilities, LightState, Provider};
use crate::providers::discovery::{DeviceDiscovery, HttpSweepDiscovery};
use crate::providers::{CredentialField, FieldKind, LightProvider, ProviderFactory};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use uuid::Uuid;

pub struct TasmotaProvider {
    client: Client,
    base_url: String,
}

impl TasmotaProvider {
    fn new_with_base(base_url: impl Into<String>) -> Result<Self> {
        // Bounded so a powered-off device fails the poll fast instead of hanging it.
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        Ok(Self {
            client,
            base_url: base_url.into(),
        })
    }

    pub fn new(device_ip: impl AsRef<str>) -> Result<Self> {
        let ip = device_ip.as_ref();
        let base_url = if ip.starts_with("http://") || ip.starts_with("https://") {
            ip.to_string()
        } else {
            format!("http://{ip}")
        };
        Self::new_with_base(base_url)
    }

    pub fn from_credentials(creds_json: &str) -> Result<Self> {
        let creds: serde_json::Value = serde_json::from_str(creds_json)?;
        let ip = creds["device_ip"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("tasmota credentials missing device_ip"))?;
        Self::new(ip)
    }

    #[cfg(test)]
    pub fn new_for_test(base_url: impl Into<String>) -> Result<Self> {
        Self::new_with_base(base_url)
    }

    async fn cm(&self, cmnd: &str) -> Result<reqwest::Response> {
        self.client
            .get(format!("{}/cm", self.base_url))
            .query(&[("cmnd", cmnd)])
            .send()
            .await
            .with_context(|| format!("Tasmota command '{cmnd}' failed"))
    }
}

// ── Wire types ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TasmotaStatus0 {
    #[serde(rename = "Status")]
    status: TasmotaStatusInner,
}

#[derive(Debug, Deserialize)]
struct TasmotaStatusInner {
    #[serde(rename = "DeviceName")]
    device_name: String,
    #[serde(rename = "FriendlyName")]
    friendly_name: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct TasmotaState {
    #[serde(rename = "POWER", alias = "Power")]
    power: Option<String>,
    /// Brightness 0–100 (Tasmota native range — no conversion needed).
    #[serde(rename = "Dimmer")]
    dimmer: Option<u32>,
    /// Hex color: RRGGBB or RRGGBBWW; only the first 6 chars (RGB) are used.
    #[serde(rename = "Color")]
    color: Option<String>,
}

// ── Conversion helpers ──────────────────────────────────────────────────────

fn parse_tasmota_state(s: &TasmotaState) -> LightState {
    let on = s
        .power
        .as_deref()
        .map(|p| p.eq_ignore_ascii_case("on") || p == "1")
        .unwrap_or(false);

    let brightness = s.dimmer.map(|d| d as f32);

    let color = s.color.as_deref().and_then(parse_hex_color);

    LightState {
        on,
        brightness,
        color,
        color_temp_mirek: None,
        reachable: None,
        effect: None,
        transport: None,
    }
}

/// Parse the first 6 hex chars of a Tasmota color string as RGB.
fn parse_hex_color(hex: &str) -> Option<Color> {
    if hex.len() < 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::from_rgb(r, g, b))
}

// ── Provider impl ───────────────────────────────────────────────────────────

#[async_trait]
impl LightProvider for TasmotaProvider {
    fn name(&self) -> &str {
        "tasmota"
    }

    async fn discover(&self) -> Result<Vec<Light>> {
        let status: TasmotaStatus0 = self
            .cm("Status 0")
            .await?
            .error_for_status()?
            .json()
            .await
            .context("Tasmota Status 0 JSON parse failed")?;

        let state_raw: TasmotaState = self
            .cm("State")
            .await?
            .error_for_status()?
            .json()
            .await
            .context("Tasmota State JSON parse failed")?;

        let name = status
            .status
            .friendly_name
            .into_iter()
            .next()
            .filter(|n| !n.is_empty())
            .unwrap_or(status.status.device_name);

        let light = Light {
            id: Uuid::new_v4(),
            provider_id: "main".into(),
            provider: Provider::Tasmota,
            name,
            state: parse_tasmota_state(&state_raw),
            capabilities: LightCapabilities {
                dimmable: state_raw.dimmer.is_some(),
                color_rgb: state_raw.color.is_some(),
                color_temperature: false,
                hue_gamut: None,
                effects: Vec::new(),
            },
            last_seen: Utc::now(),
            hw_id: None,
        };

        Ok(vec![light])
    }

    async fn get_state(&self, _device_id: &str) -> Result<LightState> {
        let state_raw: TasmotaState = self
            .cm("State")
            .await?
            .error_for_status()?
            .json()
            .await
            .context("Tasmota State JSON parse failed")?;

        Ok(parse_tasmota_state(&state_raw))
    }

    async fn set_state(&self, _device_id: &str, state: &LightState) -> Result<()> {
        let power = if state.on { "ON" } else { "OFF" };
        let mut parts: Vec<String> = vec![format!("Power {power}")];

        if let Some(brightness) = state.brightness {
            parts.push(format!("Dimmer {}", brightness.round() as u32));
        }

        if let Some(color) = &state.color {
            let (r, g, b) = color.to_rgb();
            parts.push(format!("Color {:02X}{:02X}{:02X}", r, g, b));
        }

        let cmnd = format!("Backlog {}", parts.join("; "));
        self.cm(&cmnd).await?.error_for_status()?;

        Ok(())
    }
}

// ── Factory ─────────────────────────────────────────────────────────────────

pub struct TasmotaProviderFactory;

impl ProviderFactory for TasmotaProviderFactory {
    fn provider_type(&self) -> &'static str {
        "tasmota"
    }

    fn display_name(&self) -> &'static str {
        "Tasmota"
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn LightProvider>> {
        Ok(Box::new(TasmotaProvider::from_credentials(
            credentials_json,
        )?))
    }

    fn discoverer(&self) -> Option<Box<dyn DeviceDiscovery>> {
        // `Status 0` returns a status blob; `StatusFWR` is a Tasmota-specific key.
        Some(Box::new(HttpSweepDiscovery::new(
            "/cm?cmnd=Status%200",
            "Tasmota",
            "device_ip",
            |body| body.contains("StatusFWR"),
        )))
    }

    fn credentials_schema(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "device_ip",
            label: "Device IP Address",
            kind: FieldKind::IpAddress,
            required: true,
            hint: Some("IP address of the Tasmota device on your local network"),
        }]
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn status0_response() -> serde_json::Value {
        serde_json::json!({
            "Status": {
                "Module": 0,
                "DeviceName": "Tasmota",
                "FriendlyName": ["Kitchen Strip"],
                "Topic": "tasmota_A1B2C3"
            }
        })
    }

    fn state_response() -> serde_json::Value {
        serde_json::json!({
            "POWER": "ON",
            "Dimmer": 75,
            "Color": "FF8000"
        })
    }

    async fn mount_discover_mocks(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/cm"))
            .and(query_param("cmnd", "Status 0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(status0_response()))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/cm"))
            .and(query_param("cmnd", "State"))
            .respond_with(ResponseTemplate::new(200).set_body_json(state_response()))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn discover_returns_light_with_friendly_name() {
        let server = MockServer::start().await;
        mount_discover_mocks(&server).await;

        let lights = TasmotaProvider::new_for_test(server.uri())
            .unwrap()
            .discover()
            .await
            .unwrap();

        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].name, "Kitchen Strip");
        assert_eq!(lights[0].provider_id, "main");
    }

    #[tokio::test]
    async fn discover_detects_dimmable_and_color_capabilities() {
        let server = MockServer::start().await;
        mount_discover_mocks(&server).await;

        let lights = TasmotaProvider::new_for_test(server.uri())
            .unwrap()
            .discover()
            .await
            .unwrap();

        assert!(lights[0].capabilities.dimmable);
        assert!(lights[0].capabilities.color_rgb);
        assert!(!lights[0].capabilities.color_temperature);
    }

    #[tokio::test]
    async fn get_state_parses_on_brightness_and_color() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/cm"))
            .and(query_param("cmnd", "State"))
            .respond_with(ResponseTemplate::new(200).set_body_json(state_response()))
            .mount(&server)
            .await;

        let state = TasmotaProvider::new_for_test(server.uri())
            .unwrap()
            .get_state("main")
            .await
            .unwrap();

        assert!(state.on);
        assert_eq!(state.brightness, Some(75.0));
        assert!(state.color.is_some());
    }

    #[tokio::test]
    async fn get_state_parses_power_off() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/cm"))
            .and(query_param("cmnd", "State"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "POWER": "OFF",
                "Dimmer": 0
            })))
            .mount(&server)
            .await;

        let state = TasmotaProvider::new_for_test(server.uri())
            .unwrap()
            .get_state("main")
            .await
            .unwrap();

        assert!(!state.on);
    }

    #[tokio::test]
    async fn set_state_sends_backlog_with_power_and_dimmer() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/cm"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        let state = LightState {
            on: true,
            brightness: Some(50.0),
            ..Default::default()
        };
        TasmotaProvider::new_for_test(server.uri())
            .unwrap()
            .set_state("main", &state)
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let cmnd = requests[0]
            .url
            .query_pairs()
            .find(|(k, _)| k == "cmnd")
            .map(|(_, v)| v.to_string())
            .unwrap_or_default();
        assert!(cmnd.contains("Power ON"), "missing Power ON in: {cmnd}");
        assert!(cmnd.contains("Dimmer 50"), "missing Dimmer 50 in: {cmnd}");
    }

    #[tokio::test]
    async fn set_state_includes_color_hex_in_backlog() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/cm"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        let state = LightState {
            on: true,
            brightness: Some(100.0),
            color: Some(Color::from_rgb(255, 0, 0)),
            color_temp_mirek: None,
            reachable: None,
            effect: None,
            transport: None,
        };
        TasmotaProvider::new_for_test(server.uri())
            .unwrap()
            .set_state("main", &state)
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        let cmnd = requests[0]
            .url
            .query_pairs()
            .find(|(k, _)| k == "cmnd")
            .map(|(_, v)| v.to_string())
            .unwrap_or_default();
        assert!(cmnd.contains("Color"), "missing Color in: {cmnd}");
    }

    #[tokio::test]
    async fn factory_build_uses_device_ip_from_credentials() {
        assert!(
            TasmotaProviderFactory
                .build(r#"{"device_ip": "192.168.1.200"}"#)
                .is_ok()
        );
    }

    #[tokio::test]
    async fn factory_build_fails_on_missing_device_ip() {
        let err = TasmotaProviderFactory
            .build("{}")
            .err()
            .expect("expected error");
        assert!(err.to_string().contains("device_ip"));
    }
}
