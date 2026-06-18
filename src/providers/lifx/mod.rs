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

pub mod lan;

/// The LIFX cloud transport (`api.lifx.com`). One of the two transports the
/// unified [`LifxProvider`] owns; reached for any bulb not locally reachable, and
/// for effects/groups (the LAN transport handles plain colour/brightness/power).
pub struct LifxCloud {
    client: Client,
    /// Base URL for the API; overridden in tests to point at a wiremock server.
    base_url: String,
}

impl LifxCloud {
    pub fn new(token: impl AsRef<str>) -> Result<Self> {
        Self::new_with_base(token, BASE_URL)
    }

    fn new_with_base(token: impl AsRef<str>, base_url: impl Into<String>) -> Result<Self> {
        // Shared, pooled client keyed by token (the base URL lives on the struct),
        // so per-request rebuilds reuse one warm connection to the LIFX cloud
        // instead of re-handshaking each control. See [`crate::providers::cached_client`].
        let token = token.as_ref();
        let client = crate::providers::cached_client(&format!("lifx:{token}"), || {
            let mut headers = header::HeaderMap::new();
            headers.insert(
                header::AUTHORIZATION,
                header::HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
            // Bounded so a cloud outage fails the poll fast instead of hanging it.
            Ok(Client::builder()
                .default_headers(headers)
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(30))
                .build()?)
        })?;
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

    /// POST a firmware effect to a LIFX selector — `breathe`/`pulse`/`move`/
    /// `morph`/`flame`, or `off` to clear. A separate endpoint from `/state`.
    async fn apply_effect(&self, selector: &str, effect: &str, state: &LightState) -> Result<()> {
        let mut req = self.client.post(format!(
            "{}/lights/{selector}/effects/{effect}",
            self.base_url
        ));
        if let Some(body) = lifx_effect_body(effect, state) {
            req = req.json(&body);
        }
        let resp = req.send().await.context("LIFX effect request failed")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("LIFX effect error {status}: {text}");
        }
        Ok(())
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
    /// Linear-strip bulbs (Z / Beam) — support the `move` firmware effect.
    #[serde(default)]
    has_multizone: bool,
    /// 2D matrix bulbs (Tile / Candle / Path) — support `morph` + `flame`.
    #[serde(default)]
    has_matrix: bool,
}

/// The firmware effects a LIFX device supports, by capability. Every colour bulb
/// can `breathe`/`pulse`; `move` needs a multizone strip; `morph`/`flame` need a
/// matrix. `off` clears any running effect. Names are LIFX-native (the cloud
/// `/effects/<name>` endpoint), with `off` as the clear value.
fn lifx_effects(caps: Option<&LifxCapabilities>, has_color: bool) -> Vec<String> {
    if !has_color {
        return Vec::new(); // white-only bulbs have no colour effects
    }
    let mut fx = vec![
        "off".to_string(),
        "breathe".to_string(),
        "pulse".to_string(),
    ];
    if caps.map(|c| c.has_multizone).unwrap_or(false) {
        fx.push("move".to_string());
    }
    if caps.map(|c| c.has_matrix).unwrap_or(false) {
        fx.push("morph".to_string());
        fx.push("flame".to_string());
    }
    fx
}

/// Build the JSON body for a LIFX `/effects/<name>` POST. `None` = the `off`
/// endpoint (no body). Uses sensible defaults; `breathe`/`pulse` breathe toward
/// the light's current colour (or white). See the LIFX HTTP effects docs.
fn lifx_effect_body(effect: &str, state: &LightState) -> Option<serde_json::Value> {
    let target = state.color.as_ref().map(|c| {
        let (r, g, b) = c.to_rgb();
        let (hue, sat) = rgb_to_hs(r, g, b);
        format!("hue:{hue:.1} saturation:{sat:.4}")
    });
    match effect {
        "off" => None,
        "breathe" | "pulse" => Some(json!({
            "color": target.unwrap_or_else(|| "white".to_string()),
            "period": 2.0,
            "cycles": 5,
            "persist": false,
            "power_on": true,
        })),
        "move" => {
            Some(json!({ "direction": "forward", "period": 2.0, "cycles": 5, "power_on": true }))
        }
        "morph" => Some(json!({
            "period": 5.0,
            "palette": ["red", "orange", "yellow", "green", "blue", "purple"],
            "power_on": true,
        })),
        "flame" => Some(json!({ "period": 5.0, "power_on": true })),
        _ => Some(json!({ "power_on": true })),
    }
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
    let caps_ref = l.product.as_ref().and_then(|p| p.capabilities.as_ref());
    let (has_color, has_temp) = caps_ref
        .map(|c| (c.has_color, c.has_variable_color_temp))
        // The cloud API doesn't always include product metadata; assume a full
        // colour bulb (the common case) rather than hiding capabilities.
        .unwrap_or((true, true));
    let effects = lifx_effects(caps_ref, has_color);

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
            effects,
        },
        last_seen: Utc::now(),
    }
}

#[async_trait]
impl LightProvider for LifxCloud {
    fn name(&self) -> &str {
        "lifx-cloud"
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
        let selector = format!("id:{provider_id}");
        // A firmware effect lives on its own `/effects/<name>` endpoint, not on
        // `/state`. When the change carries one, drive it there; the frontend only
        // sends `effect` on an actual effect pick (it doesn't re-send the last
        // effect on a colour/brightness change), so a transient breathe/pulse
        // isn't re-triggered by an unrelated tweak.
        if let Some(effect) = state.effect.as_deref().filter(|e| !e.is_empty()) {
            return self.apply_effect(&selector, effect, state).await;
        }
        // One PUT carries the whole state — LIFX applies power + brightness +
        // colour atomically (unlike Govee's one-request-per-capability).
        self.put_state(&selector, &state_to_body(state)).await
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

// ── Unified provider (LAN-preferred, cloud fallback) ─────────────────────────

/// LIFX with **both transports**: a per-bulb LAN-preferred, cloud-fallback light
/// provider, mirroring the unified Govee provider. Plain colour/brightness/power
/// goes over the local network whenever the bulb answered a scan (faster, no quota,
/// works offline); everything else — an unscanned bulb, a failed LAN send, or an
/// **effect** (LAN effects aren't implemented) — uses the cloud. Groups stay
/// cloud-only (the LIFX cloud's one-call group selector). Either transport may be
/// absent: cloud-only (no LAN interface) and LAN-only (no token) both work.
pub struct LifxProvider {
    cloud: Option<LifxCloud>,
    lan: Option<lan::LifxLanProvider>,
}

impl LifxProvider {
    pub fn new(cloud: Option<LifxCloud>, lan: Option<lan::LifxLanProvider>) -> Self {
        Self { cloud, lan }
    }

    /// Cloud control, or a clear "unreachable" error when there's no token.
    async fn cloud_set(&self, id: &str, state: &LightState) -> Result<()> {
        match &self.cloud {
            Some(c) => c.set_state(id, state).await,
            None => {
                bail!("LIFX bulb {id} is not reachable on the LAN and no cloud token is configured")
            }
        }
    }
}

#[async_trait]
impl LightProvider for LifxProvider {
    fn name(&self) -> &str {
        "lifx"
    }

    async fn discover(&self) -> Result<Vec<Light>> {
        // The LAN scan tags which bulbs are locally reachable; the cloud (when
        // present) is the source of truth for names/groups/effects.
        let scanned: std::collections::HashSet<String> = match &self.lan {
            Some(lan) => lan
                .scan()
                .await
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(mac, _)| crate::providers::mac_hw_id(&mac))
                .collect(),
            None => std::collections::HashSet::new(),
        };
        if let Some(cloud) = &self.cloud {
            match cloud.discover().await {
                Ok(mut lights) => {
                    for l in &mut lights {
                        let on_lan = crate::providers::mac_hw_id(&l.provider_id)
                            .is_some_and(|k| scanned.contains(&k));
                        l.state.transport = Some(if on_lan { "lan" } else { "cloud" }.to_string());
                    }
                    let on_lan = lights
                        .iter()
                        .filter(|l| l.state.transport.as_deref() == Some("lan"))
                        .count();
                    tracing::debug!(
                        target: "bifrost::lifx",
                        lan_enabled = self.lan.is_some(),
                        lan_scanned = scanned.len(),
                        total = lights.len(),
                        on_lan,
                        on_cloud = lights.len() - on_lan,
                        "LIFX discover: {} bulb(s) — {on_lan} via LAN, {} via cloud",
                        lights.len(),
                        lights.len() - on_lan,
                    );
                    return Ok(lights);
                }
                Err(e) => {
                    // Resilient: with the LAN up, a cloud blip falls back to LAN.
                    if let Some(lan) = &self.lan {
                        tracing::warn!("LIFX cloud discovery failed, using LAN only: {e:#}");
                        return lan.discover().await;
                    }
                    return Err(e);
                }
            }
        }
        // LAN-only: the LAN transport is the source of truth.
        match &self.lan {
            Some(lan) => lan.discover().await,
            None => Ok(vec![]),
        }
    }

    async fn set_state(&self, id: &str, state: &LightState) -> Result<()> {
        // Effects only exist on the cloud `/effects` endpoint.
        if state.effect.as_deref().is_some_and(|e| !e.is_empty()) {
            tracing::debug!(target: "bifrost::lifx", bulb = %id, "set_state: effect → cloud (LAN effects not implemented)");
            return self.cloud_set(id, state).await;
        }
        // LAN-preferred: a `set_state` that can't resolve the bulb on the LAN
        // returns an error, which we treat as "fall back to the cloud".
        if let Some(lan) = &self.lan {
            match lan.set_state(id, state).await {
                Ok(()) => {
                    tracing::debug!(target: "bifrost::lifx", bulb = %id, "set_state: → LAN");
                    return Ok(());
                }
                Err(e) => {
                    tracing::debug!(target: "bifrost::lifx", bulb = %id, "set_state: not reachable on LAN → cloud ({e:#})")
                }
            }
        }
        tracing::debug!(target: "bifrost::lifx", bulb = %id, lan_enabled = self.lan.is_some(), "set_state: → cloud");
        self.cloud_set(id, state).await
    }

    async fn get_state(&self, id: &str) -> Result<LightState> {
        if let Some(lan) = &self.lan
            && let Ok(state) = lan.get_state(id).await
            && state.reachable != Some(false)
        {
            tracing::debug!(target: "bifrost::lifx", bulb = %id, "get_state: via LAN");
            return Ok(state); // already stamped transport = "lan"
        }
        if let Some(c) = &self.cloud {
            let mut state = c.get_state(id).await?;
            state.transport = Some("cloud".to_string());
            tracing::debug!(target: "bifrost::lifx", bulb = %id, "get_state: via cloud");
            return Ok(state);
        }
        Ok(LightState {
            on: false,
            reachable: Some(false),
            ..Default::default()
        })
    }

    async fn discover_groups(&self) -> Result<Vec<ProviderGroup>> {
        // Native group control is a cloud feature; the LAN transport has none.
        match &self.cloud {
            Some(c) => c.discover_groups().await,
            None => Ok(vec![]),
        }
    }

    async fn set_group_state(&self, grouped_ref: &str, state: &LightState) -> Result<bool> {
        match &self.cloud {
            Some(c) => c.set_group_state(grouped_ref, state).await,
            None => Ok(false), // no cloud → caller fans out per light (LAN)
        }
    }
}

// ── Factory ──────────────────────────────────────────────────────────────────

use crate::providers::{CredentialField, FieldKind, ProviderFactory};
use std::net::IpAddr;

pub struct LifxProviderFactory;

impl ProviderFactory for LifxProviderFactory {
    fn provider_type(&self) -> &'static str {
        "lifx"
    }

    fn display_name(&self) -> &'static str {
        "LIFX"
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn LightProvider>> {
        let creds: serde_json::Value = serde_json::from_str(credentials_json)?;
        // Cloud when a token is supplied. LAN is **on by default** (LIFX LAN is on
        // by default on the bulbs); `bind_addr` only picks the interface, defaulting
        // to 0.0.0.0. A host that can't reach the LAN finds nothing on a scan and
        // falls back to the cloud, so defaulting it on is safe.
        let cloud = match creds["token"].as_str().filter(|t| !t.is_empty()) {
            Some(t) => Some(LifxCloud::new(t)?),
            None => None,
        };
        let bind = creds["bind_addr"]
            .as_str()
            .filter(|b| !b.is_empty())
            .unwrap_or("0.0.0.0");
        let addr: IpAddr = bind
            .parse()
            .with_context(|| format!("invalid LIFX LAN bind address '{bind}'"))?;
        let lan = Some(lan::LifxLanProvider::new(addr));
        Ok(Box::new(LifxProvider::new(cloud, lan)))
    }

    fn credentials_schema(&self) -> &'static [CredentialField] {
        &[
            CredentialField {
                name: "token",
                label: "Token",
                kind: FieldKind::Password,
                required: false,
                hint: Some(
                    "Personal access token from cloud.lifx.com/settings. Optional if you only use LAN control.",
                ),
            },
            CredentialField {
                name: "bind_addr",
                label: "LAN interface (advanced)",
                kind: FieldKind::IpAddress,
                required: false,
                hint: Some(
                    "Local control is on by default (preferred over the cloud whenever a bulb is reachable; LIFX LAN is on by default on the bulbs). Leave blank for all interfaces (0.0.0.0); set a specific IP only if multi-homed.",
                ),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mock_provider(server: &MockServer) -> LifxCloud {
        LifxCloud::new_for_test(server.uri(), "tok").unwrap()
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

    #[test]
    fn effects_derive_from_capabilities() {
        // White-only bulbs advertise no colour effects.
        assert!(lifx_effects(None, false).is_empty());
        // A plain colour bulb gets the universal off/breathe/pulse.
        let basic = lifx_effects(None, true);
        assert_eq!(basic, vec!["off", "breathe", "pulse"]);
        // A multizone strip adds `move`.
        let strip = LifxCapabilities {
            has_color: true,
            has_variable_color_temp: true,
            has_multizone: true,
            has_matrix: false,
        };
        assert!(lifx_effects(Some(&strip), true).contains(&"move".to_string()));
        // A matrix bulb adds `morph` + `flame`.
        let tile = LifxCapabilities {
            has_color: true,
            has_variable_color_temp: true,
            has_multizone: false,
            has_matrix: true,
        };
        let fx = lifx_effects(Some(&tile), true);
        assert!(fx.contains(&"morph".to_string()) && fx.contains(&"flame".to_string()));
    }

    #[tokio::test]
    async fn discover_advertises_effects_for_a_colour_bulb() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/lights/all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([light_json(
                "d073d5000001",
                "Kitchen",
                true,
                1.0,
                3500
            )])))
            .mount(&server)
            .await;
        let lights = mock_provider(&server).await.discover().await.unwrap();
        assert_eq!(
            lights[0].capabilities.effects,
            vec!["off", "breathe", "pulse"]
        );
    }

    #[tokio::test]
    async fn set_state_with_effect_posts_to_the_effect_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/lights/id:d073d5000001/effects/breathe"))
            .and(header("authorization", "Bearer tok"))
            .respond_with(ResponseTemplate::new(207))
            .mount(&server)
            .await;
        let state = LightState {
            on: true,
            effect: Some("breathe".to_string()),
            ..Default::default()
        };
        mock_provider(&server)
            .await
            .set_state("d073d5000001", &state)
            .await
            .unwrap();
        let req = &server.received_requests().await.unwrap()[0];
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        // No colour set → breathes toward white, powering the bulb on.
        assert_eq!(body["color"], "white");
        assert_eq!(body["power_on"], true);
    }

    #[tokio::test]
    async fn set_state_with_effect_and_color_breathes_that_colour() {
        // A breathe/pulse carries the light's current colour, so a red light pulses
        // red — not white. The room + single-light editors send the colour along
        // with the effect for exactly this reason (regression: room effects reset
        // the colour to white because the colour wasn't forwarded).
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/lights/id:d073d5000002/effects/pulse"))
            .respond_with(ResponseTemplate::new(207))
            .mount(&server)
            .await;
        let state = LightState {
            on: true,
            color: Some(Color::from_rgb(255, 0, 0)),
            effect: Some("pulse".to_string()),
            ..Default::default()
        };
        mock_provider(&server)
            .await
            .set_state("d073d5000002", &state)
            .await
            .unwrap();
        let req = &server.received_requests().await.unwrap()[0];
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        let color = body["color"].as_str().unwrap();
        assert!(color.starts_with("hue:0."), "red → hue 0: {color}");
        assert!(
            color.contains("saturation:1"),
            "red is fully saturated: {color}"
        );
    }

    #[tokio::test]
    async fn set_state_with_off_effect_hits_the_off_endpoint_with_no_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/lights/id:x/effects/off"))
            .respond_with(ResponseTemplate::new(207))
            .mount(&server)
            .await;
        let state = LightState {
            on: true,
            effect: Some("off".to_string()),
            ..Default::default()
        };
        mock_provider(&server)
            .await
            .set_state("x", &state)
            .await
            .unwrap();
        let req = &server.received_requests().await.unwrap()[0];
        assert!(req.body.is_empty(), "the off endpoint takes no body");
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
        // And the cloud transport talks to the mock successfully.
        let p = LifxCloud::new_for_test(server.uri(), "tok").unwrap();
        assert_eq!(p.discover().await.unwrap().len(), 0);
    }

    #[test]
    fn factory_defaults_lan_on() {
        let f = LifxProviderFactory;
        // LAN is on by default (0.0.0.0), so even an empty config builds (LAN-only).
        assert!(f.build("{}").is_ok(), "LAN-only by default");
        assert!(f.build(r#"{"token":"tok"}"#).is_ok(), "cloud + default LAN");
        assert!(
            f.build(r#"{"token":"tok","bind_addr":"192.168.1.5"}"#)
                .is_ok(),
            "cloud + explicit interface"
        );
        assert!(
            f.build(r#"{"bind_addr":"not-an-ip"}"#).is_err(),
            "malformed LAN address is rejected"
        );
        // Schema offers both fields, neither hard-required.
        let schema = f.credentials_schema();
        assert_eq!(schema.len(), 2);
        assert!(schema.iter().all(|c| !c.required));
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
