//! LIFX cloud HTTP API.
//!
//! Base URL: `https://api.lifx.com/v1`
//! Authentication: `Authorization: Bearer <token>` — generate a token at
//! <https://cloud.lifx.com/settings>.
//! Rate limit: 120 requests/minute per token.
//!
//! LIFX models colour as HSBK (hue 0–360, saturation 0–1, brightness 0–1,
//! kelvin). We translate to/from Bifrost's CIE-xy `Color` + mirek: a saturated
//! bulb maps to an RGB colour, an unsaturated one to a white temperature.

use crate::models::{Color, Light, LightCapabilities, LightState, Provider};
use crate::providers::LightProvider;
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::{Client, header};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

const BASE_URL: &str = "https://api.lifx.com/v1";

pub struct LifxProvider {
    client: Client,
    /// Base URL for the API; overridden in tests to point at a wiremock server.
    base_url: String,
}

impl LifxProvider {
    pub fn new(token: impl AsRef<str>) -> Result<Self> {
        Self::new_with_base(token, BASE_URL)
    }

    fn new_with_base(token: impl AsRef<str>, base_url: impl Into<String>) -> Result<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {}", token.as_ref()))?,
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

    /// Fetch lights matching a selector (`all`, `id:<id>`, …).
    async fn fetch_lights(&self, selector: &str) -> Result<Vec<LifxLight>> {
        let resp = self
            .client
            .get(format!("{}/lights/{selector}", self.base_url))
            .send()
            .await
            .context("LIFX lights request failed")?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    /// Test constructor: points at a local HTTP mock server instead of the LIFX cloud.
    #[cfg(test)]
    pub fn new_for_test(base_url: impl Into<String>, token: impl AsRef<str>) -> Result<Self> {
        Self::new_with_base(token, base_url)
    }
}

// ── Wire types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct LifxLight {
    id: String,
    label: String,
    #[serde(default)]
    connected: bool,
    /// "on" | "off"
    power: String,
    /// 0.0–1.0
    #[serde(default)]
    brightness: f32,
    color: LifxColor,
    #[serde(default)]
    product: Option<LifxProduct>,
}

#[derive(Debug, Deserialize)]
struct LifxColor {
    #[serde(default)]
    hue: f32,
    #[serde(default)]
    saturation: f32,
    #[serde(default)]
    kelvin: u32,
}

#[derive(Debug, Deserialize)]
struct LifxProduct {
    #[serde(default)]
    capabilities: Option<LifxCapabilities>,
}

#[derive(Debug, Deserialize)]
struct LifxCapabilities {
    #[serde(default)]
    has_color: bool,
    #[serde(default)]
    has_variable_color_temp: bool,
}

/// HSV (h 0–360, s/v 0–1) → sRGB, for mapping a LIFX colour to Bifrost's `Color`.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let to = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (to(r1), to(g1), to(b1))
}

fn lifx_to_light(l: LifxLight) -> Light {
    let (has_color, has_temp) = l
        .product
        .as_ref()
        .and_then(|p| p.capabilities.as_ref())
        .map(|c| (c.has_color, c.has_variable_color_temp))
        // The cloud API doesn't always include product metadata; assume a full
        // colour bulb (the common case) rather than hiding capabilities.
        .unwrap_or((true, true));

    let mut state = LightState {
        on: l.power == "on",
        brightness: Some((l.brightness * 100.0).round().clamp(0.0, 100.0)),
        reachable: Some(l.connected),
        ..Default::default()
    };
    // A saturated bulb is in colour mode; an unsaturated one shows a white
    // temperature (colour & temp are mutually exclusive — see `persist_light_state`).
    if l.color.saturation > 0.01 {
        let (r, g, b) = hsv_to_rgb(l.color.hue, l.color.saturation, 1.0);
        state.color = Some(Color::from_rgb(r, g, b));
    } else if l.color.kelvin > 0 {
        state.color_temp_mirek =
            Some((1_000_000u32 / l.color.kelvin.max(1)).clamp(1, 65535) as u16);
    }
    // The cloud reports the last-known power for a disconnected bulb; treat it off.
    if !l.connected {
        state.on = false;
    }

    Light {
        id: Uuid::new_v4(),
        // LIFX's `id` is the bulb's serial (its MAC) — our cross-provider de-dup key.
        hw_id: crate::providers::mac_hw_id(&l.id),
        provider_id: l.id,
        provider: Provider::Lifx,
        name: l.label,
        state,
        capabilities: LightCapabilities {
            dimmable: true,
            color_rgb: has_color,
            color_temperature: has_temp,
            hue_gamut: None,
        },
        last_seen: Utc::now(),
    }
}

#[async_trait]
impl LightProvider for LifxProvider {
    fn name(&self) -> &str {
        "lifx"
    }

    async fn discover(&self) -> Result<Vec<Light>> {
        Ok(self
            .fetch_lights("all")
            .await?
            .into_iter()
            .map(lifx_to_light)
            .collect())
    }

    async fn set_state(&self, provider_id: &str, state: &LightState) -> Result<()> {
        // One PUT carries the whole state — LIFX applies power + brightness +
        // colour atomically (unlike Govee's one-request-per-capability).
        let mut body = json!({ "power": if state.on { "on" } else { "off" } });
        if let Some(b) = state.brightness {
            body["brightness"] = json!((b / 100.0).clamp(0.0, 1.0));
        }
        if let Some(color) = &state.color {
            let (r, g, b) = color.to_rgb();
            body["color"] = json!(format!("rgb:{r},{g},{b}"));
        } else if let Some(mirek) = state.color_temp_mirek {
            let kelvin = (1_000_000u32 / mirek.max(1) as u32).clamp(1500, 9000);
            body["color"] = json!(format!("kelvin:{kelvin}"));
        }

        let resp = self
            .client
            .put(format!("{}/lights/id:{provider_id}/state", self.base_url))
            .json(&body)
            .send()
            .await
            .context("LIFX set_state request failed")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("LIFX control error {status}: {text}");
        }
        Ok(())
    }

    async fn get_state(&self, provider_id: &str) -> Result<LightState> {
        let light = self
            .fetch_lights(&format!("id:{provider_id}"))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("LIFX light '{provider_id}' not found"))?;
        Ok(lifx_to_light(light).state)
    }
}

// ── Factory ──────────────────────────────────────────────────────────────────

use crate::providers::{CredentialField, FieldKind, ProviderFactory};

pub struct LifxProviderFactory;

impl ProviderFactory for LifxProviderFactory {
    fn provider_type(&self) -> &'static str {
        "lifx"
    }

    fn display_name(&self) -> &'static str {
        "LIFX (Cloud)"
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn LightProvider>> {
        let creds: serde_json::Value = serde_json::from_str(credentials_json)?;
        let token = creds["token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("lifx credentials missing token"))?
            .to_string();
        Ok(Box::new(LifxProvider::new(&token)?))
    }

    fn credentials_schema(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "token",
            label: "Token",
            kind: FieldKind::Password,
            required: true,
            hint: Some("Generate a personal access token at cloud.lifx.com/settings"),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mock_provider(server: &MockServer) -> LifxProvider {
        LifxProvider::new_for_test(server.uri(), "tok").unwrap()
    }

    fn light_json(id: &str, label: &str, on: bool, sat: f32, kelvin: u32) -> serde_json::Value {
        json!({
            "id": id,
            "label": label,
            "connected": true,
            "power": if on { "on" } else { "off" },
            "brightness": 0.5,
            "color": { "hue": 120.0, "saturation": sat, "kelvin": kelvin },
            "product": { "capabilities": { "has_color": true, "has_variable_color_temp": true } }
        })
    }

    #[test]
    fn hsv_to_rgb_primaries() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), (255, 0, 0));
        assert_eq!(hsv_to_rgb(120.0, 1.0, 1.0), (0, 255, 0));
        assert_eq!(hsv_to_rgb(240.0, 1.0, 1.0), (0, 0, 255));
    }

    #[tokio::test]
    async fn discover_parses_lights_and_maps_state() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/lights/all"))
            .and(header("authorization", "Bearer tok"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                light_json("d073d5000001", "Kitchen", true, 1.0, 3500),
                light_json("d073d5000002", "Hall", false, 0.0, 2700),
            ])))
            .mount(&server)
            .await;

        let lights = mock_provider(&server).await.discover().await.unwrap();
        assert_eq!(lights.len(), 2);

        let kitchen = &lights[0];
        assert_eq!(kitchen.name, "Kitchen");
        assert!(matches!(kitchen.provider, Provider::Lifx));
        assert!(kitchen.state.on);
        assert_eq!(kitchen.state.brightness, Some(50.0));
        // Saturated → colour mode (no temp), capabilities advertise both.
        assert!(kitchen.state.color.is_some());
        assert_eq!(kitchen.state.color_temp_mirek, None);
        assert!(kitchen.capabilities.color_rgb);
        // The serial is a MAC → hw_id for de-dup.
        assert_eq!(kitchen.hw_id.as_deref(), Some("mac:d073d5000001"));

        let hall = &lights[1];
        assert!(!hall.state.on);
        // Unsaturated → white temperature, not a colour.
        assert!(hall.state.color.is_none());
        assert_eq!(hall.state.color_temp_mirek, Some((1_000_000 / 2700) as u16));
    }

    #[tokio::test]
    async fn offline_light_reports_unreachable_and_off() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/lights/all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "id": "d073d5000003", "label": "Lamp", "connected": false,
                "power": "on", "brightness": 0.8,
                "color": { "hue": 0.0, "saturation": 0.0, "kelvin": 3000 }
            }])))
            .mount(&server)
            .await;
        let lights = mock_provider(&server).await.discover().await.unwrap();
        assert_eq!(lights[0].state.reachable, Some(false));
        assert!(!lights[0].state.on, "offline bulb must report off");
    }

    #[tokio::test]
    async fn set_state_sends_power_brightness_and_color_in_one_put() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/lights/id:d073d5000001/state"))
            .and(header("authorization", "Bearer tok"))
            .respond_with(ResponseTemplate::new(207).set_body_json(json!({
                "results": [{ "id": "d073d5000001", "status": "ok" }]
            })))
            .mount(&server)
            .await;

        let state = LightState {
            on: true,
            brightness: Some(40.0),
            color: Some(Color::from_rgb(255, 0, 0)),
            ..Default::default()
        };
        mock_provider(&server)
            .await
            .set_state("d073d5000001", &state)
            .await
            .unwrap();

        let req = &server.received_requests().await.unwrap()[0];
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["power"], "on");
        assert!((body["brightness"].as_f64().unwrap() - 0.4).abs() < 1e-6);
        assert_eq!(body["color"], "rgb:255,0,0");
    }

    #[tokio::test]
    async fn set_state_white_uses_kelvin_selector() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/lights/id:x/state"))
            .respond_with(ResponseTemplate::new(207))
            .mount(&server)
            .await;
        let state = LightState {
            on: true,
            color_temp_mirek: Some(370), // ≈2700K
            ..Default::default()
        };
        mock_provider(&server)
            .await
            .set_state("x", &state)
            .await
            .unwrap();
        let req = &server.received_requests().await.unwrap()[0];
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["color"], format!("kelvin:{}", 1_000_000 / 370));
    }

    #[tokio::test]
    async fn get_state_fetches_by_id_selector() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/lights/id:d073d5000001"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([light_json(
                "d073d5000001",
                "Kitchen",
                true,
                0.0,
                4000
            )])))
            .mount(&server)
            .await;
        let s = mock_provider(&server)
            .await
            .get_state("d073d5000001")
            .await
            .unwrap();
        assert!(s.on);
        assert_eq!(s.color_temp_mirek, Some((1_000_000 / 4000) as u16));
    }

    #[tokio::test]
    async fn api_error_propagates() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/lights/all"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({ "error": "bad token" })))
            .mount(&server)
            .await;
        assert!(mock_provider(&server).await.discover().await.is_err());
    }

    #[tokio::test]
    async fn factory_build_constructs_a_working_provider() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/lights/all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        // The factory builds from JSON credentials; point it at the mock by
        // constructing directly with the same token (build path covered too).
        let built = LifxProviderFactory
            .build(r#"{"token":"tok"}"#)
            .expect("factory build");
        assert_eq!(built.name(), "lifx");
        // And the test-constructed provider talks to the mock successfully.
        let p = LifxProvider::new_for_test(server.uri(), "tok").unwrap();
        assert_eq!(p.discover().await.unwrap().len(), 0);
    }

    #[test]
    fn factory_credentials_schema_requires_a_token() {
        let schema = LifxProviderFactory.credentials_schema();
        assert_eq!(schema.len(), 1);
        assert_eq!(schema[0].name, "token");
        assert!(schema[0].required);
    }

    #[test]
    fn factory_build_rejects_missing_token() {
        assert!(LifxProviderFactory.build(r#"{}"#).is_err());
    }
}
