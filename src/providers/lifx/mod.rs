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
use crate::providers::{LightProvider, ProviderGroup};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::{Client, header};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
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

    /// PUT a state body to a LIFX selector (`id:<id>`, `group_id:<id>`, …).
    /// Shared by per-light `set_state` and native group control.
    async fn put_state(&self, selector: &str, body: &serde_json::Value) -> Result<()> {
        let resp = self
            .client
            .put(format!("{}/lights/{selector}/state", self.base_url))
            .json(body)
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
    /// The LIFX group this bulb belongs to (mirrored as a Bifrost Room).
    #[serde(default)]
    group: Option<LifxGroup>,
}

#[derive(Debug, Deserialize)]
struct LifxGroup {
    id: String,
    name: String,
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

/// sRGB → (hue 0–360, saturation 0–1). The inverse of [`hsv_to_rgb`]'s hue/sat,
/// used to drive LIFX colour by hue+saturation only (see [`state_to_body`]).
fn rgb_to_hs(r: u8, g: u8, b: u8) -> (f32, f32) {
    let (rf, gf, bf) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let delta = max - min;
    let hue = if delta == 0.0 {
        0.0
    } else if max == rf {
        60.0 * ((gf - bf) / delta).rem_euclid(6.0)
    } else if max == gf {
        60.0 * ((bf - rf) / delta + 2.0)
    } else {
        60.0 * ((rf - gf) / delta + 4.0)
    };
    let sat = if max == 0.0 { 0.0 } else { delta / max };
    (hue, sat)
}

/// Build the LIFX state PUT body (power + brightness + colour) from a Bifrost
/// state. Colour and colour temperature are mutually exclusive — a `color` wins
/// over a `kelvin` temperature, matching the per-light cache merge.
///
/// Colour is sent as **hue + saturation only**, never `rgb:`. LIFX derives
/// brightness from the magnitude of an `rgb:` triple, so `rgb:255,0,0` forces
/// the bulb to full brightness; since Bifrost stores colour at full value
/// (brightness is a separate channel), a colour-only change would jump the bulb
/// to max. Hue+saturation leaves brightness untouched, so it changes only when
/// the explicit `brightness` field is present.
fn state_to_body(state: &LightState) -> serde_json::Value {
    let mut body = json!({ "power": if state.on { "on" } else { "off" } });
    if let Some(b) = state.brightness {
        body["brightness"] = json!((b / 100.0).clamp(0.0, 1.0));
    }
    if let Some(color) = &state.color {
        let (r, g, b) = color.to_rgb();
        let (hue, sat) = rgb_to_hs(r, g, b);
        body["color"] = json!(format!("hue:{hue:.1} saturation:{sat:.4}"));
    } else if let Some(mirek) = state.color_temp_mirek {
        let kelvin = (1_000_000u32 / mirek.max(1) as u32).clamp(1500, 9000);
        body["color"] = json!(format!("kelvin:{kelvin}"));
    }
    body
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
            effects: Vec::new(),
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
        self.put_state(&format!("id:{provider_id}"), &state_to_body(state))
            .await
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

    /// Mirror LIFX groups as Bifrost-linkable provider groups: cluster the
    /// account's bulbs by their `group` **id**, preserving first-seen order so
    /// the synced rooms come out stable. `grouped_ref` is the `group_id:<id>`
    /// selector, enabling single-call group control via `set_group_state`.
    ///
    /// LIFX caches the group *name* on each bulb independently, so a rename can
    /// leave a stale member reporting the old name (e.g. one bulb still says
    /// "Bathroom" after the group was renamed "Bedeoom"). We therefore take the
    /// **majority** name across the group's members rather than whichever bulb
    /// the API happened to list first — a single laggy bulb can't misname the
    /// room.
    async fn discover_groups(&self) -> Result<Vec<ProviderGroup>> {
        let lights = self.fetch_lights("all").await?;
        let mut order: Vec<String> = Vec::new();
        let mut members: HashMap<String, Vec<String>> = HashMap::new();
        // group id → (name → how many members report it)
        let mut name_votes: HashMap<String, HashMap<String, usize>> = HashMap::new();
        for l in lights {
            let Some(g) = l.group else { continue };
            if !members.contains_key(&g.id) {
                order.push(g.id.clone());
            }
            members.entry(g.id.clone()).or_default().push(l.id);
            *name_votes
                .entry(g.id.clone())
                .or_default()
                .entry(g.name)
                .or_default() += 1;
        }
        Ok(order
            .into_iter()
            .map(|id| {
                // Most-reported name wins; ties broken lexicographically so the
                // result is deterministic regardless of map iteration order.
                let name = name_votes
                    .remove(&id)
                    .unwrap_or_default()
                    .into_iter()
                    .max_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
                    .map(|(n, _)| n)
                    .unwrap_or_default();
                ProviderGroup {
                    member_device_ids: members.remove(&id).unwrap_or_default(),
                    grouped_ref: Some(format!("group_id:{id}")),
                    provider_group_id: id,
                    name,
                }
            })
            .collect())
    }

    /// Drive a whole LIFX group in one cloud call via its `group_id:<id>`
    /// selector (the `grouped_ref` from `discover_groups`).
    async fn set_group_state(&self, grouped_ref: &str, state: &LightState) -> Result<bool> {
        self.put_state(grouped_ref, &state_to_body(state)).await?;
        Ok(true)
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

    #[test]
    fn rgb_to_hs_primaries_and_gray() {
        let approx = |(h, s): (f32, f32), eh: f32, es: f32| {
            assert!((h - eh).abs() < 0.5, "hue {h} vs {eh}");
            assert!((s - es).abs() < 0.01, "sat {s} vs {es}");
        };
        approx(rgb_to_hs(255, 0, 0), 0.0, 1.0);
        approx(rgb_to_hs(0, 255, 0), 120.0, 1.0);
        approx(rgb_to_hs(0, 0, 255), 240.0, 1.0);
        // Brightness is carried separately, so a dim-but-saturated value still
        // reports full saturation (magnitude doesn't leak into colour).
        approx(rgb_to_hs(60, 0, 0), 0.0, 1.0);
        // Gray/white has no saturation.
        approx(rgb_to_hs(128, 128, 128), 0.0, 0.0);
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
        // Colour goes as hue+saturation (red → hue 0, sat 1), never `rgb:` —
        // LIFX would otherwise infer brightness from the RGB magnitude.
        assert_eq!(body["color"], "hue:0.0 saturation:1.0000");
    }

    #[tokio::test]
    async fn color_only_change_does_not_force_brightness() {
        // The whole-room colour bug: a colour-only change (no brightness) must
        // not carry a brightness field, and must use hue+saturation so LIFX
        // leaves the bulb's current brightness alone (rgb: would jump it to max).
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/lights/id:x/state"))
            .respond_with(ResponseTemplate::new(207))
            .mount(&server)
            .await;
        let state = LightState {
            on: true,
            color: Some(Color::from_rgb(0, 255, 0)), // pure green
            ..Default::default()
        };
        mock_provider(&server)
            .await
            .set_state("x", &state)
            .await
            .unwrap();
        let req = &server.received_requests().await.unwrap()[0];
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert!(body.get("brightness").is_none(), "must not send brightness");
        assert_eq!(body["color"], "hue:120.0 saturation:1.0000");
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

    fn light_json_in_group(id: &str, group_id: &str, group_name: &str) -> serde_json::Value {
        json!({
            "id": id,
            "label": id,
            "connected": true,
            "power": "on",
            "brightness": 0.5,
            "color": { "hue": 0.0, "saturation": 0.0, "kelvin": 3500 },
            "group": { "id": group_id, "name": group_name }
        })
    }

    #[tokio::test]
    async fn discover_groups_clusters_bulbs_by_lifx_group() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/lights/all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                light_json_in_group("d073d5000001", "grp-kitchen", "Kitchen"),
                light_json_in_group("d073d5000002", "grp-kitchen", "Kitchen"),
                light_json_in_group("d073d5000003", "grp-bedroom", "Bedroom"),
            ])))
            .mount(&server)
            .await;

        let groups = mock_provider(&server)
            .await
            .discover_groups()
            .await
            .unwrap();
        assert_eq!(groups.len(), 2);

        let kitchen = &groups[0];
        assert_eq!(kitchen.name, "Kitchen");
        assert_eq!(kitchen.provider_group_id, "grp-kitchen");
        assert_eq!(
            kitchen.member_device_ids,
            vec!["d073d5000001", "d073d5000002"]
        );
        // grouped_ref is the selector that drives the whole group in one call.
        assert_eq!(kitchen.grouped_ref.as_deref(), Some("group_id:grp-kitchen"));

        let bedroom = &groups[1];
        assert_eq!(bedroom.member_device_ids, vec!["d073d5000003"]);
    }

    #[tokio::test]
    async fn discover_groups_uses_majority_name_when_lifx_caches_stale_names() {
        // Real LIFX behaviour: all bulbs share one group id, but a renamed group
        // can leave a laggy bulb reporting the old name. The majority wins, even
        // when the stale bulb is listed first.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/lights/all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                light_json_in_group("d073d5000004", "grp-1", "Bathroom"), // stale, first
                light_json_in_group("d073d5000003", "grp-1", "Bedeoom"),
                light_json_in_group("d073d5000002", "grp-1", "Bedeoom"),
                light_json_in_group("d073d5000001", "grp-1", "Bedeoom"),
            ])))
            .mount(&server)
            .await;

        let groups = mock_provider(&server)
            .await
            .discover_groups()
            .await
            .unwrap();
        assert_eq!(groups.len(), 1, "one group id → one room");
        assert_eq!(
            groups[0].name, "Bedeoom",
            "majority name, not the stale first bulb"
        );
        assert_eq!(groups[0].member_device_ids.len(), 4, "all members kept");
        assert_eq!(groups[0].grouped_ref.as_deref(), Some("group_id:grp-1"));
    }

    #[tokio::test]
    async fn discover_groups_skips_ungrouped_bulbs() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/lights/all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([light_json(
                "d073d5000009",
                "Lonely",
                true,
                0.0,
                3000
            )])))
            .mount(&server)
            .await;
        assert!(
            mock_provider(&server)
                .await
                .discover_groups()
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn set_group_state_drives_group_id_selector_in_one_call() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/lights/group_id:grp-kitchen/state"))
            .and(header("authorization", "Bearer tok"))
            .respond_with(ResponseTemplate::new(207))
            .mount(&server)
            .await;

        let state = LightState {
            on: true,
            brightness: Some(60.0),
            ..Default::default()
        };
        let handled = mock_provider(&server)
            .await
            .set_group_state("group_id:grp-kitchen", &state)
            .await
            .unwrap();
        assert!(handled, "LIFX advertises native group control");

        let req = &server.received_requests().await.unwrap()[0];
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["power"], "on");
        assert!((body["brightness"].as_f64().unwrap() - 0.6).abs() < 1e-6);
    }
}
