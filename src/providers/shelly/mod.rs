//! Shelly Gen1 HTTP REST integration.
//!
//! Shelly is a range of smart home devices (dimmers, plugs, bulbs) by Allterco.
//! <https://shelly-api-docs.shelly.cloud/gen1/>
//!
//! All Shelly Gen1 light devices expose:
//! - `GET /settings`  — device name and configuration
//! - `GET /light/0`   — current state (ison, brightness 0–100)
//! - `GET /light/0?turn=on&brightness=50` — change state
//!
//! Brightness in the Shelly API is already 0–100, matching our model directly.
//! Auth: optional HTTP basic auth via an `auth` credential field.

use crate::models::{Light, LightCapabilities, LightState, Provider};
use crate::providers::discovery::{DeviceDiscovery, HttpSweepDiscovery};
use crate::providers::{CredentialField, FieldKind, LightProvider, ProviderFactory};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use uuid::Uuid;

pub struct ShellyProvider {
    client: Client,
    base_url: String,
}

impl ShellyProvider {
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
            .ok_or_else(|| anyhow::anyhow!("shelly credentials missing device_ip"))?;
        Self::new(ip)
    }

    #[cfg(test)]
    pub fn new_for_test(base_url: impl Into<String>) -> Result<Self> {
        Self::new_with_base(base_url)
    }
}

// ── Wire types ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ShellySettings {
    /// User-assigned name; may be empty on factory-default devices.
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ShellyLight {
    ison: bool,
    /// Brightness 0–100 (Shelly native range — no conversion needed).
    brightness: Option<u32>,
}

// ── Provider impl ───────────────────────────────────────────────────────────

#[async_trait]
impl LightProvider for ShellyProvider {
    fn name(&self) -> &str {
        "shelly"
    }

    async fn discover(&self) -> Result<Vec<Light>> {
        let settings: ShellySettings = self
            .client
            .get(format!("{}/settings", self.base_url))
            .send()
            .await
            .context("Shelly settings request failed")?
            .error_for_status()?
            .json()
            .await
            .context("Shelly settings JSON parse failed")?;

        let light_state: ShellyLight = self
            .client
            .get(format!("{}/light/0", self.base_url))
            .send()
            .await
            .context("Shelly light/0 request failed")?
            .error_for_status()?
            .json()
            .await
            .context("Shelly light/0 JSON parse failed")?;

        let name = settings
            .name
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Shelly".into());

        let light = Light {
            id: Uuid::new_v4(),
            provider_id: "main".into(),
            provider: Provider::Shelly,
            name,
            state: shelly_to_light_state(&light_state),
            capabilities: LightCapabilities {
                dimmable: light_state.brightness.is_some(),
                color_rgb: false,
                color_temperature: false,
                hue_gamut: None,
            },
            last_seen: Utc::now(),
        };

        Ok(vec![light])
    }

    async fn get_state(&self, _device_id: &str) -> Result<LightState> {
        let light: ShellyLight = self
            .client
            .get(format!("{}/light/0", self.base_url))
            .send()
            .await
            .context("Shelly light/0 request failed")?
            .error_for_status()?
            .json()
            .await
            .context("Shelly light/0 JSON parse failed")?;

        Ok(shelly_to_light_state(&light))
    }

    async fn set_state(&self, _device_id: &str, state: &LightState) -> Result<()> {
        let turn = if state.on { "on" } else { "off" };
        let mut params: Vec<(&str, String)> = vec![("turn", turn.into())];

        if let Some(brightness) = state.brightness {
            params.push(("brightness", (brightness.round() as u32).to_string()));
        }

        self.client
            .get(format!("{}/light/0", self.base_url))
            .query(&params)
            .send()
            .await
            .context("Shelly set_state request failed")?
            .error_for_status()?;

        Ok(())
    }
}

fn shelly_to_light_state(l: &ShellyLight) -> LightState {
    LightState {
        on: l.ison,
        brightness: l.brightness.map(|b| b as f32),
        color: None,
        color_temp_mirek: None,
        reachable: None,
    }
}

// ── Factory ─────────────────────────────────────────────────────────────────

pub struct ShellyProviderFactory;

impl ProviderFactory for ShellyProviderFactory {
    fn provider_type(&self) -> &'static str {
        "shelly"
    }

    fn display_name(&self) -> &'static str {
        "Shelly"
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn LightProvider>> {
        Ok(Box::new(ShellyProvider::from_credentials(
            credentials_json,
        )?))
    }

    fn discoverer(&self) -> Option<Box<dyn DeviceDiscovery>> {
        // Shelly Gen1's /shelly identity endpoint returns its type, mac and fw.
        Some(Box::new(HttpSweepDiscovery::new(
            "/shelly",
            "Shelly",
            "device_ip",
            |body| body.contains("\"mac\"") && body.contains("\"fw\""),
        )))
    }

    fn credentials_schema(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "device_ip",
            label: "Device IP Address",
            kind: FieldKind::IpAddress,
            required: true,
            hint: Some("IP address of the Shelly device on your local network"),
        }]
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mount_discover_mocks(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/settings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "Hallway Dimmer"
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/light/0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ison": true,
                "brightness": 60
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn discover_returns_light_with_configured_name() {
        let server = MockServer::start().await;
        mount_discover_mocks(&server).await;

        let lights = ShellyProvider::new_for_test(server.uri())
            .unwrap()
            .discover()
            .await
            .unwrap();

        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].name, "Hallway Dimmer");
        assert_eq!(lights[0].provider_id, "main");
    }

    #[tokio::test]
    async fn discover_detects_dimmable_capability() {
        let server = MockServer::start().await;
        mount_discover_mocks(&server).await;

        let lights = ShellyProvider::new_for_test(server.uri())
            .unwrap()
            .discover()
            .await
            .unwrap();

        assert!(lights[0].capabilities.dimmable);
        assert!(!lights[0].capabilities.color_rgb);
    }

    #[tokio::test]
    async fn discover_falls_back_to_shelly_when_name_is_empty() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/settings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"name": ""})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/light/0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ison": false,
                "brightness": 0
            })))
            .mount(&server)
            .await;

        let lights = ShellyProvider::new_for_test(server.uri())
            .unwrap()
            .discover()
            .await
            .unwrap();

        assert_eq!(lights[0].name, "Shelly");
    }

    #[tokio::test]
    async fn get_state_returns_current_on_and_brightness() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/light/0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ison": true,
                "brightness": 80
            })))
            .mount(&server)
            .await;

        let state = ShellyProvider::new_for_test(server.uri())
            .unwrap()
            .get_state("main")
            .await
            .unwrap();

        assert!(state.on);
        assert_eq!(state.brightness, Some(80.0));
    }

    #[tokio::test]
    async fn set_state_sends_turn_and_brightness_as_query_params() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/light/0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ison": true, "brightness": 50
            })))
            .mount(&server)
            .await;

        let state = LightState {
            on: true,
            brightness: Some(50.0),
            ..Default::default()
        };
        ShellyProvider::new_for_test(server.uri())
            .unwrap()
            .set_state("main", &state)
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let pairs: std::collections::HashMap<_, _> = requests[0].url.query_pairs().collect();
        assert_eq!(pairs.get("turn").map(|v| v.as_ref()), Some("on"));
        assert_eq!(pairs.get("brightness").map(|v| v.as_ref()), Some("50"));
    }

    #[tokio::test]
    async fn set_state_sends_turn_off() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/light/0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ison": false, "brightness": 0
            })))
            .mount(&server)
            .await;

        let state = LightState {
            on: false,
            ..Default::default()
        };
        ShellyProvider::new_for_test(server.uri())
            .unwrap()
            .set_state("main", &state)
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        let pairs: std::collections::HashMap<_, _> = requests[0].url.query_pairs().collect();
        assert_eq!(pairs.get("turn").map(|v| v.as_ref()), Some("off"));
    }

    #[tokio::test]
    async fn factory_build_uses_device_ip_from_credentials() {
        assert!(
            ShellyProviderFactory
                .build(r#"{"device_ip": "192.168.1.30"}"#)
                .is_ok()
        );
    }

    #[tokio::test]
    async fn factory_build_fails_on_missing_device_ip() {
        let err = ShellyProviderFactory
            .build("{}")
            .err()
            .expect("expected error");
        assert!(err.to_string().contains("device_ip"));
    }
}
