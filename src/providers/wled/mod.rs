//! WLED HTTP REST integration.
//!
//! WLED is open-source firmware for addressable LEDs with a built-in web server.
//! <https://kno.wled.ge/>
//!
//! Base URL: `http://<device_ip>` (LAN only — no cloud dependency, no auth required).
//!
//! Key endpoints used:
//! - `GET  /json/info`  — device name
//! - `GET  /json/state` — on/off, brightness (0–255), segment colors ([R,G,B])
//! - `POST /json/state` — update state

use crate::models::{Color, Light, LightCapabilities, LightState, Provider};
use crate::providers::discovery::{DeviceDiscovery, HttpSweepDiscovery};
use crate::providers::{CredentialField, FieldKind, LightProvider, ProviderFactory};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use uuid::Uuid;

pub struct WledProvider {
    client: Client,
    /// Base URL for the device, e.g. `http://192.168.1.100`.
    base_url: String,
}

impl WledProvider {
    fn new_with_base(base_url: impl Into<String>) -> Result<Self> {
        // One pooled client shared across all WLED devices (no per-device config;
        // it pools connections per-host internally), so per-request rebuilds reuse
        // warm connections. See [`crate::providers::cached_client`].
        let client = crate::providers::cached_client("wled", || {
            // Bounded so a powered-off device fails the poll fast instead of hanging it.
            Ok(Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(15))
                .build()?)
        })?;
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
            .ok_or_else(|| anyhow::anyhow!("wled credentials missing device_ip"))?;
        Self::new(ip)
    }

    #[cfg(test)]
    pub fn new_for_test(base_url: impl Into<String>) -> Result<Self> {
        Self::new_with_base(base_url)
    }
}

// ── Wire types ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct WledInfo {
    name: String,
}

#[derive(Debug, Deserialize)]
struct WledState {
    on: bool,
    bri: u8,
    seg: Option<Vec<WledSegment>>,
}

#[derive(Debug, Deserialize)]
struct WledSegment {
    /// Color slots: col[0] = primary, col[1] = secondary, col[2] = tertiary.
    col: Option<Vec<Vec<u8>>>,
}

// ── Conversion helpers ──────────────────────────────────────────────────────

fn parse_wled_state(s: &WledState) -> LightState {
    // WLED brightness is 0–255; our model uses 0–100.
    let brightness = s.bri as f32 / 255.0 * 100.0;

    let color = s
        .seg
        .as_deref()
        .and_then(|segs| segs.first())
        .and_then(|seg| seg.col.as_deref())
        .and_then(|cols| cols.first())
        .filter(|rgb| rgb.len() >= 3)
        .map(|rgb| Color::from_rgb(rgb[0], rgb[1], rgb[2]));

    LightState {
        on: s.on,
        brightness: Some(brightness),
        color,
        color_temp_mirek: None,
        reachable: None,
        effect: None,
        transport: None,
    }
}

// ── Provider impl ───────────────────────────────────────────────────────────

#[async_trait]
impl LightProvider for WledProvider {
    fn name(&self) -> &str {
        "wled"
    }

    async fn discover(&self) -> Result<Vec<Light>> {
        let info: WledInfo = self
            .client
            .get(format!("{}/json/info", self.base_url))
            .send()
            .await
            .context("WLED info request failed")?
            .error_for_status()?
            .json()
            .await?;

        let state_raw: WledState = self
            .client
            .get(format!("{}/json/state", self.base_url))
            .send()
            .await
            .context("WLED state request failed")?
            .error_for_status()?
            .json()
            .await?;

        let light = Light {
            id: Uuid::new_v4(),
            // One physical device per provider entry; stable identifier is "main".
            provider_id: "main".into(),
            provider: Provider::Wled,
            name: info.name,
            state: parse_wled_state(&state_raw),
            capabilities: LightCapabilities {
                dimmable: true,
                color_rgb: true,
                color_temperature: false,
                hue_gamut: None,
                effects: Vec::new(),
                segments: None,
            },
            last_seen: Utc::now(),
            hw_id: None,
        };

        Ok(vec![light])
    }

    async fn get_state(&self, _device_id: &str) -> Result<LightState> {
        let state_raw: WledState = self
            .client
            .get(format!("{}/json/state", self.base_url))
            .send()
            .await
            .context("WLED get_state request failed")?
            .error_for_status()?
            .json()
            .await?;

        Ok(parse_wled_state(&state_raw))
    }

    async fn set_state(&self, _device_id: &str, state: &LightState) -> Result<()> {
        let bri = (state.brightness.unwrap_or(100.0) / 100.0 * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8;

        let mut body = serde_json::json!({ "on": state.on, "bri": bri });

        if let Some(color) = &state.color {
            let (r, g, b) = color.to_rgb();
            body["seg"] = serde_json::json!([{ "col": [[r, g, b]] }]);
        }

        self.client
            .post(format!("{}/json/state", self.base_url))
            .json(&body)
            .send()
            .await
            .context("WLED set_state request failed")?
            .error_for_status()?;

        Ok(())
    }
}

// ── Factory ─────────────────────────────────────────────────────────────────

pub struct WledProviderFactory;

impl ProviderFactory for WledProviderFactory {
    fn provider_type(&self) -> &'static str {
        "wled"
    }

    fn display_name(&self) -> &'static str {
        "WLED"
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn LightProvider>> {
        Ok(Box::new(WledProvider::from_credentials(credentials_json)?))
    }

    fn discoverer(&self) -> Option<Box<dyn DeviceDiscovery>> {
        // WLED's /json/info reports `"brand":"WLED"`.
        Some(Box::new(HttpSweepDiscovery::new(
            "/json/info",
            "WLED",
            "device_ip",
            |body| body.contains("WLED"),
        )))
    }

    fn credentials_schema(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "device_ip",
            label: "Device IP Address",
            kind: FieldKind::IpAddress,
            required: true,
            hint: Some("IP address of the WLED device on your local network (e.g. 192.168.1.100)"),
        }]
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn info_response() -> serde_json::Value {
        serde_json::json!({ "name": "Living Room Strip", "ver": "0.14.1" })
    }

    fn state_on_response() -> serde_json::Value {
        serde_json::json!({
            "on": true,
            "bri": 128,
            "seg": [{ "col": [[255, 128, 0], [0, 0, 0], [0, 0, 0]] }]
        })
    }

    async fn mount_discover_mocks(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/json/info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(info_response()))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/json/state"))
            .respond_with(ResponseTemplate::new(200).set_body_json(state_on_response()))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn discover_returns_light_with_device_name() {
        let server = MockServer::start().await;
        mount_discover_mocks(&server).await;

        let lights = WledProvider::new_for_test(server.uri())
            .unwrap()
            .discover()
            .await
            .unwrap();

        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].name, "Living Room Strip");
        assert_eq!(lights[0].provider_id, "main");
        assert!(lights[0].capabilities.dimmable);
        assert!(lights[0].capabilities.color_rgb);
        assert!(!lights[0].capabilities.color_temperature);
    }

    #[tokio::test]
    async fn discover_parses_initial_state() {
        let server = MockServer::start().await;
        mount_discover_mocks(&server).await;

        let lights = WledProvider::new_for_test(server.uri())
            .unwrap()
            .discover()
            .await
            .unwrap();

        let state = &lights[0].state;
        assert!(state.on);
        // bri 128 / 255 * 100 ≈ 50.2%
        let brightness = state.brightness.unwrap();
        assert!(
            (brightness - 50.2).abs() < 0.5,
            "expected ~50.2%, got {brightness}"
        );
        assert!(state.color.is_some());
    }

    #[tokio::test]
    async fn get_state_parses_off_state() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/json/state"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "on": false,
                "bri": 255,
                "seg": [{ "col": [[0, 0, 255], [0, 0, 0], [0, 0, 0]] }]
            })))
            .mount(&server)
            .await;

        let state = WledProvider::new_for_test(server.uri())
            .unwrap()
            .get_state("main")
            .await
            .unwrap();

        assert!(!state.on);
        let brightness = state.brightness.unwrap();
        assert!(
            (brightness - 100.0).abs() < 0.5,
            "expected 100%, got {brightness}"
        );
        assert!(state.color.is_some());
    }

    #[tokio::test]
    async fn set_state_sends_on_and_brightness() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/json/state"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"on": true, "bri": 128})),
            )
            .mount(&server)
            .await;

        let state = LightState {
            on: true,
            brightness: Some(50.0),
            ..Default::default()
        };
        WledProvider::new_for_test(server.uri())
            .unwrap()
            .set_state("main", &state)
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["on"], true);
        // 50.0 / 100.0 * 255.0 = 127.5 → rounds to 128
        assert_eq!(body["bri"], 128);
    }

    #[tokio::test]
    async fn set_state_includes_segment_color_when_present() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/json/state"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"on": true})))
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
        WledProvider::new_for_test(server.uri())
            .unwrap()
            .set_state("main", &state)
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert!(
            body["seg"].is_array(),
            "expected seg array in body, got: {body}"
        );
        let col = &body["seg"][0]["col"][0];
        assert!(col.is_array(), "expected col[0] array");
        assert_eq!(col.as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn factory_build_uses_device_ip_from_credentials() {
        let factory = WledProviderFactory;
        let result = factory.build(r#"{"device_ip": "192.168.1.50"}"#);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn factory_build_fails_on_missing_device_ip() {
        let factory = WledProviderFactory;
        let err = factory
            .build("{}")
            .err()
            .expect("expected error for missing device_ip");
        assert!(err.to_string().contains("device_ip"));
    }
}
