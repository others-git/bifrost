//! Philips Hue API v2 (local CLIP v2 REST + SSE event stream).
//!
//! Authentication: press the bridge link button, then POST to `http://<bridge>/api` with
//! `{"devicetype": "bifrost#server"}`. The response contains the `username` which is used
//! as the `hue-application-key` header on all subsequent requests.
//!
//! Base URL: `https://<bridge-ip>/clip/v2/resource`
//! The bridge uses a self-signed TLS cert; accept it or pin the bridge cert.

use crate::models::{Color, HueGamut, Light, LightCapabilities, LightState, Provider};
use crate::providers::LightProvider;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::{Client, header};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub struct HueProvider {
    client: Client,
    /// Base URL for the bridge, e.g. `https://192.168.1.10`.
    /// All resource paths are appended to this.
    bridge_base: String,
}

impl HueProvider {
    pub fn new(bridge_ip: impl Into<String>, app_key: impl Into<String>) -> Result<Self> {
        let bridge_base = format!("https://{}", bridge_ip.into());
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
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "hue-application-key",
            header::HeaderValue::from_str(&app_key)?,
        );
        let client = Client::builder()
            .default_headers(headers)
            .danger_accept_invalid_certs(accept_invalid_certs)
            .build()?;
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

fn hue_resource_to_light(r: HueLightResource) -> Light {
    let gamut = r.color_gamut_type.as_deref().and_then(gamut_from_str);
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
        },
        capabilities: LightCapabilities {
            dimmable: true,
            color_rgb: gamut.is_some(),
            color_temperature: true,
            hue_gamut: gamut,
        },
        last_seen: Utc::now(),
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
            .send()
            .await
            .context("Hue discover request failed")?
            .error_for_status()?
            .json()
            .await?;

        Ok(resp.data.into_iter().map(hue_resource_to_light).collect())
    }

    async fn set_state(&self, provider_id: &str, state: &LightState) -> Result<()> {
        let url = self.resource_url(&format!("/light/{provider_id}"));
        let body = HuePutLight {
            on: Some(HueOn { on: state.on }),
            dimming: state.brightness.map(|b| HueDimming { brightness: b }),
            color: state.color.as_ref().map(|c| HueColor {
                xy: HueXy { x: c.x, y: c.y },
            }),
            color_temperature: state
                .color_temp_mirek
                .map(|m| HueColorTemperature { mirek: Some(m) }),
        };

        self.client
            .put(&url)
            .json(&body)
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
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let resource = resp.data.into_iter().next().context("light not found")?;
        Ok(hue_resource_to_light(resource).state)
    }
}

impl HueProvider {
    pub async fn ping(&self) -> Result<()> {
        let url = self.resource_url("/device");
        self.client.get(&url).send().await?.error_for_status()?;
        Ok(())
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

use crate::providers::{CredentialField, FieldKind, ProviderFactory};

pub struct HueProviderFactory;

impl ProviderFactory for HueProviderFactory {
    fn provider_type(&self) -> &'static str {
        "hue"
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
                    "Press the link button, then POST to http://<bridge-ip>/api with {\"devicetype\":\"bifrost#server\"}",
                ),
            },
        ]
    }
}

/// Extract a partial `LightState` from a Hue SSE event data item.
/// Only fields present in the event are populated.
pub fn parse_light_state_from_event(item: &serde_json::Value) -> crate::models::LightState {
    let mut state = crate::models::LightState::default();

    if let Some(on) = item
        .get("on")
        .and_then(|o| o.get("on"))
        .and_then(|v| v.as_bool())
    {
        state.on = on;
    }
    if let Some(b) = item
        .get("dimming")
        .and_then(|d| d.get("brightness"))
        .and_then(|v| v.as_f64())
    {
        state.brightness = Some(b as f32);
    }
    if let Some(xy) = item.get("color").and_then(|c| c.get("xy")) {
        let x = xy.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let y = xy.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let brightness = state.brightness.unwrap_or(1.0) / 100.0;
        state.color = Some(crate::models::Color { x, y, brightness });
    }
    if let Some(mirek) = item
        .get("color_temperature")
        .and_then(|ct| ct.get("mirek"))
        .and_then(|v| v.as_u64())
    {
        state.color_temp_mirek = Some(mirek as u16);
    }

    state
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

    #[test]
    fn parse_sse_event_extracts_on_off() {
        let item = serde_json::json!({"on": {"on": true}});
        let state = parse_light_state_from_event(&item);
        assert!(state.on);
    }

    #[test]
    fn parse_sse_event_extracts_color_temp() {
        let item = serde_json::json!({"color_temperature": {"mirek": 250}});
        let state = parse_light_state_from_event(&item);
        assert_eq!(state.color_temp_mirek, Some(250));
    }
}
