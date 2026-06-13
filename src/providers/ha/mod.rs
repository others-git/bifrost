//! Home Assistant integration — a **"high-class" provider**: one adapter that
//! surfaces *any* of HA's ~1000 integrations as Bifrost devices, plus HA Areas
//! as `ProviderGroup`s that wrap into Bifrost Rooms via the shared Sync flow.
//! Where the other providers each speak one device protocol, HA inherits the
//! whole HA platform through a single credential — while native Hue stays
//! direct for reliability. See `references/ha_*.md` for the API specs.
//!
//! ## What's wired today
//! - **Lights:** `light.*` entities → `Light` (the `LightProvider` impl), so any
//!   light HA controls (Zigbee, Z-Wave, Wi-Fi, …) becomes a Bifrost light.
//! - **Areas → Rooms:** `discover_groups` maps HA Areas to `ProviderGroup`s, so
//!   the provider's **Sync** button mirrors HA's room structure into Bifrost
//!   Rooms, exactly like Hue rooms/zones.
//! - Registered as a **light-domain** factory in `default_registry()`.
//!
//! ## On hold (implemented, not registered)
//! - **Audio:** the `AudioProvider` impl + `HaAudioFactory` (`media_player.*` →
//!   `AudioDevice`, grouping, transport) are kept and tested, but **not**
//!   registered while audio work is paused. Registering `HaAudioFactory`
//!   alongside the light factory is the only step to bring HA audio online — at
//!   which point the registry's light-XOR-audio assumption for one
//!   `provider_type` (the `is_known` / `is_known_audio` branch in the Sync and
//!   startup paths) needs auditing so one HA row can own both `lights` and
//!   `audio_devices`.
//!
//! ## Transport: REST poll now, WebSocket push later
//! Talks to the REST API (`GET /api/states`, `POST /api/services/*`,
//! `POST /api/template`) under the existing **poll** model — no new
//! connection-manager machinery, and `/api/template` covers Area→entity mapping
//! without the WebSocket registry. The next increment is a WS `subscribe_events`
//! push manager (mirroring Onkyo's `AudioConnectionMode::Push`) for instant
//! state — see `references/ha_websocket_api.md`.
//!
//! ## Known limitation
//! - **De-dup:** a device exposed *both* natively (e.g. Hue) *and* via HA shows
//!   up twice. For now, leave it to Rooms membership to de-dupe; a provider-level
//!   guard can come later.

use crate::models::audio::{
    AudioCapabilities, AudioCommand, AudioDevice, AudioDeviceKind, AudioState, NowPlaying,
    PlayState, TransportCmd,
};
use crate::models::power::{PowerDevice, PowerKind, PowerState};
use crate::models::{Color, Light, LightCapabilities, LightState, Provider};
use crate::providers::{
    AudioProvider, AudioProviderFactory, CredentialField, FieldKind, LightProvider, PowerProvider,
    PowerProviderFactory, ProviderFactory, ProviderGroup,
};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

const LIGHT_PREFIX: &str = "light.";
const MEDIA_PREFIX: &str = "media_player.";
/// HA entity domains that map onto Bifrost's strictly-on/off `PowerDevice`.
/// `homeassistant.turn_on`/`turn_off` works uniformly across all of them.
const POWER_PREFIXES: &[&str] = &["switch.", "fan.", "input_boolean."];

// `MediaPlayerEntityFeature` bits we care about (see ha_media_player_entity.md).
const FEAT_PREVIOUS_TRACK: u64 = 16;
const FEAT_NEXT_TRACK: u64 = 32;
const FEAT_SELECT_SOURCE: u64 = 2048;
const FEAT_PLAY: u64 = 16384;
const FEAT_PAUSE: u64 = 1;
const FEAT_GROUPING: u64 = 524288;

pub struct HaProvider {
    client: Client,
    /// Normalised, e.g. `http://homeassistant.local:8123` (no trailing slash).
    base_url: String,
}

#[derive(Debug, Deserialize)]
struct HaConfig {
    /// Base URL of the HA instance, e.g. `http://homeassistant.local:8123`.
    base_url: String,
    /// Long-lived access token (Profile → Long-Lived Access Tokens).
    token: String,
}

impl HaProvider {
    fn new_with(base_url: &str, token: &str) -> Result<Self> {
        let base_url = normalise_base_url(base_url);

        let mut headers = HeaderMap::new();
        let mut auth = HeaderValue::from_str(&format!("Bearer {token}"))
            .context("invalid Home Assistant token (not a valid header value)")?;
        auth.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth);

        let client = Client::builder()
            .default_headers(headers)
            // Bounded so an unreachable HA fails a poll fast instead of hanging.
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(15))
            .build()?;

        Ok(Self { client, base_url })
    }

    pub fn from_credentials(creds_json: &str) -> Result<Self> {
        let cfg: HaConfig = serde_json::from_str(creds_json)
            .context("Home Assistant credentials must include base_url and token")?;
        Self::new_with(&cfg.base_url, &cfg.token)
    }

    #[cfg(test)]
    pub fn new_for_test(base_url: impl AsRef<str>) -> Result<Self> {
        Self::new_with(base_url.as_ref(), "test-token")
    }

    // ── REST helpers ────────────────────────────────────────────────────────

    /// All entity states (`GET /api/states`).
    async fn get_states(&self) -> Result<Vec<HaEntity>> {
        let entities = self
            .client
            .get(format!("{}/api/states", self.base_url))
            .send()
            .await
            .context("HA /api/states request failed")?
            .error_for_status()?
            .json::<Vec<HaEntity>>()
            .await?;
        Ok(entities)
    }

    /// A single entity state (`GET /api/states/{entity_id}`).
    async fn get_entity(&self, entity_id: &str) -> Result<HaEntity> {
        let entity = self
            .client
            .get(format!("{}/api/states/{entity_id}", self.base_url))
            .send()
            .await
            .context("HA get_state request failed")?
            .error_for_status()?
            .json::<HaEntity>()
            .await?;
        Ok(entity)
    }

    /// Call a service (`POST /api/services/{domain}/{service}`), targeting
    /// `entity_id` and merging any `extra` service data.
    async fn call_service(
        &self,
        domain: &str,
        service: &str,
        entity_id: &str,
        extra: Value,
    ) -> Result<()> {
        let mut body = json!({ "entity_id": entity_id });
        if let Value::Object(extra) = extra {
            for (k, v) in extra {
                body[k] = v;
            }
        }
        self.client
            .post(format!("{}/api/services/{domain}/{service}", self.base_url))
            .json(&body)
            .send()
            .await
            .with_context(|| format!("HA {domain}.{service} call failed"))?
            .error_for_status()?;
        Ok(())
    }

    /// Render a Jinja template (`POST /api/template`) and return the plaintext.
    async fn render_template(&self, template: &str) -> Result<String> {
        let text = self
            .client
            .post(format!("{}/api/template", self.base_url))
            .json(&json!({ "template": template }))
            .send()
            .await
            .context("HA /api/template request failed")?
            .error_for_status()?
            .text()
            .await?;
        Ok(text)
    }

    /// Areas containing entities whose id starts with any of `prefixes`, as
    /// `ProviderGroup`s. Shared by the light, audio, and power `discover_groups`
    /// impls (each passes the entity domains it owns).
    async fn discover_groups_for(&self, prefixes: &[&str]) -> Result<Vec<ProviderGroup>> {
        // Render the area→entities map as JSON in one round-trip; `/api/states`
        // doesn't carry area_id, but templates expose the registry.
        let template = "\
            {%- set ns = namespace(rows=[]) -%}\
            {%- for a in areas() -%}\
            {%- set ns.rows = ns.rows + [{'area_id': a, 'name': area_name(a), 'entities': area_entities(a)}] -%}\
            {%- endfor -%}\
            {{ ns.rows | to_json }}";
        let rendered = self.render_template(template).await?;
        let areas: Vec<HaArea> =
            serde_json::from_str(&rendered).context("HA area template returned non-JSON")?;

        Ok(areas
            .into_iter()
            .filter_map(|a| {
                let members: Vec<String> = a
                    .entities
                    .into_iter()
                    .filter(|e| prefixes.iter().any(|p| e.starts_with(p)))
                    .collect();
                if members.is_empty() {
                    return None; // areas with no devices of this domain are noise
                }
                Some(ProviderGroup {
                    provider_group_id: a.area_id,
                    name: a.name,
                    member_device_ids: members,
                    grouped_ref: None, // HA has no single-call "area" control handle
                })
            })
            .collect())
    }
}

// ── Wire types ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct HaEntity {
    entity_id: String,
    state: String,
    #[serde(default)]
    attributes: Value,
}

#[derive(Debug, Deserialize)]
struct HaArea {
    area_id: String,
    name: String,
    #[serde(default)]
    entities: Vec<String>,
}

// ── Attribute accessors ───────────────────────────────────────────────────────

fn attr_f64(attrs: &Value, key: &str) -> Option<f64> {
    attrs.get(key).and_then(Value::as_f64)
}
fn attr_u64(attrs: &Value, key: &str) -> Option<u64> {
    attrs.get(key).and_then(Value::as_u64)
}
fn attr_bool(attrs: &Value, key: &str) -> Option<bool> {
    attrs.get(key).and_then(Value::as_bool)
}
fn attr_str(attrs: &Value, key: &str) -> Option<String> {
    attrs.get(key).and_then(Value::as_str).map(str::to_owned)
}
fn attr_str_vec(attrs: &Value, key: &str) -> Vec<String> {
    attrs
        .get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn friendly_name(entity_id: &str, attrs: &Value) -> String {
    attr_str(attrs, "friendly_name").unwrap_or_else(|| entity_id.to_string())
}

// ── Light mapping ─────────────────────────────────────────────────────────────

fn parse_light_state(e: &HaEntity) -> LightState {
    let attrs = &e.attributes;
    let on = e.state == "on";

    let brightness = attr_f64(attrs, "brightness").map(|b| (b / 255.0 * 100.0) as f32);

    let color = attrs
        .get("rgb_color")
        .and_then(Value::as_array)
        .filter(|a| a.len() >= 3)
        .and_then(|a| {
            let r = a[0].as_u64()? as u8;
            let g = a[1].as_u64()? as u8;
            let b = a[2].as_u64()? as u8;
            Some(Color::from_rgb(r, g, b))
        });

    // Newer HA exposes color_temp_kelvin; older exposes color_temp in mirek.
    let color_temp_mirek = attr_u64(attrs, "color_temp_kelvin")
        .filter(|k| *k > 0)
        .map(|k| (1_000_000 / k) as u16)
        .or_else(|| attr_u64(attrs, "color_temp").map(|m| m as u16));

    LightState {
        on,
        brightness,
        color,
        color_temp_mirek,
        reachable: Some(e.state != "unavailable"),
    }
}

fn light_capabilities(attrs: &Value) -> LightCapabilities {
    let modes = attr_str_vec(attrs, "supported_color_modes");
    let has = |m: &str| modes.iter().any(|x| x.as_str() == m);
    LightCapabilities {
        dimmable: modes.iter().any(|m| m.as_str() != "onoff"),
        color_rgb: ["hs", "rgb", "rgbw", "rgbww", "xy"].iter().any(|&m| has(m)),
        color_temperature: has("color_temp"),
        hue_gamut: None, // not exposed through HA; native Hue keeps the gamut
    }
}

fn entity_to_light(e: HaEntity) -> Light {
    let state = parse_light_state(&e);
    let capabilities = light_capabilities(&e.attributes);
    Light {
        id: Uuid::new_v4(),
        provider_id: e.entity_id.clone(), // HA entity_id is the stable handle
        provider: Provider::Ha,
        name: friendly_name(&e.entity_id, &e.attributes),
        state,
        capabilities,
        last_seen: Utc::now(),
    }
}

// ── Audio mapping ─────────────────────────────────────────────────────────────

fn parse_audio_state(e: &HaEntity) -> AudioState {
    let attrs = &e.attributes;
    let reachable = e.state != "unavailable";
    let power = !matches!(e.state.as_str(), "off" | "unavailable" | "standby");

    let volume = attr_f64(attrs, "volume_level")
        .map(|v| (v * 100.0).round().clamp(0.0, 100.0) as u8)
        .unwrap_or(0);

    let play_state = match e.state.as_str() {
        "playing" | "buffering" => Some(PlayState::Playing),
        "paused" => Some(PlayState::Paused),
        "idle" | "off" | "standby" => Some(PlayState::Stopped),
        _ => None,
    };

    let title = attr_str(attrs, "media_title");
    let artist = attr_str(attrs, "media_artist");
    let album = attr_str(attrs, "media_album_name");
    let now_playing = if title.is_some() || artist.is_some() || album.is_some() {
        Some(NowPlaying {
            title,
            artist,
            album,
            play_state,
        })
    } else {
        None
    };

    AudioState {
        power,
        volume,
        mute: attr_bool(attrs, "is_volume_muted").unwrap_or(false),
        source: attr_str(attrs, "source"),
        now_playing,
        reachable: Some(reachable),
    }
}

fn audio_capabilities(attrs: &Value) -> AudioCapabilities {
    let feat = attr_u64(attrs, "supported_features").unwrap_or(0);
    let has = |bit: u64| feat & bit != 0;
    AudioCapabilities {
        sources: has(FEAT_SELECT_SOURCE),
        transport: has(FEAT_PLAY)
            || has(FEAT_PAUSE)
            || has(FEAT_NEXT_TRACK)
            || has(FEAT_PREVIOUS_TRACK),
        now_playing: attrs.get("media_title").is_some(),
        // browse_media exists but is richer than Bifrost "favorites"; map later.
        favorites: false,
        grouping: has(FEAT_GROUPING),
    }
}

fn audio_kind(attrs: &Value) -> AudioDeviceKind {
    match attr_str(attrs, "device_class").as_deref() {
        Some("receiver") | Some("tv") => AudioDeviceKind::Receiver,
        _ => AudioDeviceKind::Speaker,
    }
}

fn entity_to_audio(e: HaEntity) -> AudioDevice {
    let state = parse_audio_state(&e);
    let capabilities = audio_capabilities(&e.attributes);
    let kind = audio_kind(&e.attributes);
    AudioDevice {
        id: Uuid::new_v4(),
        provider_id: e.entity_id.clone(),
        name: friendly_name(&e.entity_id, &e.attributes),
        kind,
        capabilities,
        state,
    }
}

/// Map a Bifrost transport command to the HA `media_player` service name.
fn transport_service(cmd: TransportCmd) -> &'static str {
    match cmd {
        TransportCmd::Play => "media_play",
        TransportCmd::Pause => "media_pause",
        TransportCmd::Stop => "media_stop",
        TransportCmd::Next => "media_next_track",
        TransportCmd::Previous => "media_previous_track",
        TransportCmd::Toggle => "media_play_pause",
    }
}

// ── Power mapping ──────────────────────────────────────────────────────────────

/// Classify a power entity into a glyph-bearing `PowerKind` from its domain
/// (the `entity_id` prefix) and, for switches, HA's `device_class`.
fn power_kind(entity_id: &str, attrs: &Value) -> PowerKind {
    if entity_id.starts_with("fan.") {
        PowerKind::Fan
    } else if entity_id.starts_with("input_boolean.") {
        PowerKind::Toggle
    } else if entity_id.starts_with("switch.") {
        match attr_str(attrs, "device_class").as_deref() {
            Some("outlet") => PowerKind::Outlet,
            _ => PowerKind::Switch,
        }
    } else {
        PowerKind::Generic
    }
}

fn parse_power_state(e: &HaEntity) -> PowerState {
    PowerState {
        on: e.state == "on",
        reachable: Some(e.state != "unavailable"),
    }
}

fn entity_to_power(e: HaEntity) -> PowerDevice {
    let state = parse_power_state(&e);
    let kind = power_kind(&e.entity_id, &e.attributes);
    PowerDevice {
        id: Uuid::new_v4(),
        kind,
        name: friendly_name(&e.entity_id, &e.attributes),
        provider_id: e.entity_id.clone(),
        state,
    }
}

// ── LightProvider impl ────────────────────────────────────────────────────────

#[async_trait]
impl LightProvider for HaProvider {
    fn name(&self) -> &str {
        "homeassistant"
    }

    async fn discover(&self) -> Result<Vec<Light>> {
        Ok(self
            .get_states()
            .await?
            .into_iter()
            .filter(|e| e.entity_id.starts_with(LIGHT_PREFIX))
            .map(entity_to_light)
            .collect())
    }

    async fn get_state(&self, device_id: &str) -> Result<LightState> {
        Ok(parse_light_state(&self.get_entity(device_id).await?))
    }

    async fn set_state(&self, device_id: &str, state: &LightState) -> Result<()> {
        if !state.on {
            return self
                .call_service("light", "turn_off", device_id, json!({}))
                .await;
        }
        let mut data = json!({});
        if let Some(b) = state.brightness {
            data["brightness_pct"] = json!(b.round().clamp(0.0, 100.0) as u8);
        }
        // Prefer explicit color; else colour temperature if present.
        if let Some(color) = &state.color {
            let (r, g, b) = color.to_rgb();
            data["rgb_color"] = json!([r, g, b]);
        } else if let Some(mirek) = state.color_temp_mirek {
            data["color_temp_kelvin"] = json!(1_000_000u32 / mirek.max(1) as u32);
        }
        self.call_service("light", "turn_on", device_id, data).await
    }

    async fn discover_groups(&self) -> Result<Vec<ProviderGroup>> {
        self.discover_groups_for(&[LIGHT_PREFIX]).await
    }
}

// ── AudioProvider impl ────────────────────────────────────────────────────────

#[async_trait]
impl AudioProvider for HaProvider {
    fn name(&self) -> &str {
        "homeassistant"
    }

    async fn discover(&self) -> Result<Vec<AudioDevice>> {
        Ok(self
            .get_states()
            .await?
            .into_iter()
            .filter(|e| e.entity_id.starts_with(MEDIA_PREFIX))
            .map(entity_to_audio)
            .collect())
    }

    async fn get_state(&self, device_id: &str) -> Result<AudioState> {
        Ok(parse_audio_state(&self.get_entity(device_id).await?))
    }

    async fn set_state(&self, device_id: &str, cmd: &AudioCommand) -> Result<()> {
        // Power first, so "power on + volume" works from standby (matches Onkyo).
        if let Some(power) = cmd.power {
            let svc = if power { "turn_on" } else { "turn_off" };
            self.call_service("media_player", svc, device_id, json!({}))
                .await?;
        }
        if let Some(v) = cmd.volume {
            self.call_service(
                "media_player",
                "volume_set",
                device_id,
                json!({ "volume_level": (v.min(100) as f64) / 100.0 }),
            )
            .await?;
        }
        if let Some(mute) = cmd.mute {
            self.call_service(
                "media_player",
                "volume_mute",
                device_id,
                json!({ "is_volume_muted": mute }),
            )
            .await?;
        }
        if let Some(source) = &cmd.source {
            self.call_service(
                "media_player",
                "select_source",
                device_id,
                json!({ "source": source }),
            )
            .await?;
        }
        if let Some(t) = cmd.transport {
            self.call_service("media_player", transport_service(t), device_id, json!({}))
                .await?;
        }
        Ok(())
    }

    async fn group(&self, device_id: &str, coordinator_id: &str) -> Result<()> {
        if device_id == coordinator_id {
            return Err(anyhow!("a speaker cannot be grouped with itself"));
        }
        // HA `join` is invoked on the coordinator; listed members join its group.
        self.call_service(
            "media_player",
            "join",
            coordinator_id,
            json!({ "group_members": [device_id] }),
        )
        .await
    }

    async fn ungroup(&self, device_id: &str) -> Result<()> {
        self.call_service("media_player", "unjoin", device_id, json!({}))
            .await
    }

    async fn discover_groups(&self) -> Result<Vec<ProviderGroup>> {
        self.discover_groups_for(&[MEDIA_PREFIX]).await
    }
}

// ── PowerProvider impl ─────────────────────────────────────────────────────────

#[async_trait]
impl PowerProvider for HaProvider {
    fn name(&self) -> &str {
        "homeassistant"
    }

    async fn discover(&self) -> Result<Vec<PowerDevice>> {
        Ok(self
            .get_states()
            .await?
            .into_iter()
            .filter(|e| POWER_PREFIXES.iter().any(|p| e.entity_id.starts_with(p)))
            .map(entity_to_power)
            .collect())
    }

    async fn get_state(&self, device_id: &str) -> Result<PowerState> {
        Ok(parse_power_state(&self.get_entity(device_id).await?))
    }

    async fn set_state(&self, device_id: &str, on: bool) -> Result<()> {
        // The domain-agnostic `homeassistant.turn_on`/`turn_off` services route
        // to the entity's own domain, so one call path covers switches, fans,
        // and boolean helpers alike.
        let service = if on { "turn_on" } else { "turn_off" };
        self.call_service("homeassistant", service, device_id, json!({}))
            .await
    }

    async fn discover_groups(&self) -> Result<Vec<ProviderGroup>> {
        self.discover_groups_for(POWER_PREFIXES).await
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn normalise_base_url(raw: &str) -> String {
    let with_scheme = if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    with_scheme.trim_end_matches('/').to_string()
}

// ── Factories ─────────────────────────────────────────────────────────────────

const HA_CREDENTIALS: &[CredentialField] = &[
    CredentialField {
        name: "base_url",
        label: "Home Assistant URL",
        kind: FieldKind::Url,
        required: true,
        hint: Some("e.g. http://homeassistant.local:8123"),
    },
    CredentialField {
        name: "token",
        label: "Long-Lived Access Token",
        kind: FieldKind::Password,
        required: true,
        hint: Some("Profile → Security → Long-Lived Access Tokens → Create Token"),
    },
];

/// Light side of the HA adapter. Registered with `register(...)`.
pub struct HaLightFactory;

impl ProviderFactory for HaLightFactory {
    fn provider_type(&self) -> &'static str {
        "ha"
    }
    fn display_name(&self) -> &'static str {
        "Home Assistant"
    }
    fn build(&self, credentials_json: &str) -> Result<Box<dyn LightProvider>> {
        Ok(Box::new(HaProvider::from_credentials(credentials_json)?))
    }
    fn credentials_schema(&self) -> &'static [CredentialField] {
        HA_CREDENTIALS
    }
    /// HA is a local instance with no cloud rate limit, so poll faster than the
    /// cloud-conservative default for a more responsive feel (until WS push lands).
    fn connection_mode(&self) -> crate::providers::ConnectionMode {
        crate::providers::ConnectionMode::Poll { interval_secs: 30 }
    }
    /// HA isn't a single-device-domain provider — it's a platform adapter that
    /// can surface many device kinds — so it's filed under "Integrations" in the
    /// add-provider UI rather than under "Lights".
    fn domain(&self) -> crate::providers::ProviderDomain {
        crate::providers::ProviderDomain::Integration
    }
}

/// Audio side of the same HA adapter. Registered with `register_audio(...)`.
/// NOTE: shares the `"ha"` type key with `HaLightFactory` — see the
/// registry-exclusivity decision in the module docs.
pub struct HaAudioFactory;

impl AudioProviderFactory for HaAudioFactory {
    fn provider_type(&self) -> &'static str {
        "ha"
    }
    fn display_name(&self) -> &'static str {
        "Home Assistant"
    }
    fn build(&self, credentials_json: &str) -> Result<Box<dyn AudioProvider>> {
        Ok(Box::new(HaProvider::from_credentials(credentials_json)?))
    }
    fn credentials_schema(&self) -> &'static [CredentialField] {
        HA_CREDENTIALS
    }
}

/// Power side of the HA adapter (`switch.*` / `fan.*` / `input_boolean.*`).
/// Registered with `register_power(...)` **alongside** `HaLightFactory` — the
/// same `"ha"` provider row serves both domains. This is wired in
/// `default_registry()` (unlike `HaAudioFactory`, which stays on hold).
pub struct HaPowerFactory;

impl PowerProviderFactory for HaPowerFactory {
    fn provider_type(&self) -> &'static str {
        "ha"
    }
    fn display_name(&self) -> &'static str {
        "Home Assistant"
    }
    fn build(&self, credentials_json: &str) -> Result<Box<dyn PowerProvider>> {
        Ok(Box::new(HaProvider::from_credentials(credentials_json)?))
    }
    fn credentials_schema(&self) -> &'static [CredentialField] {
        HA_CREDENTIALS
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// These run as soon as `pub mod ha;` is added. They use wiremock like the other
// providers; no live HA needed.

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn light_entity() -> Value {
        json!({
            "entity_id": "light.kitchen",
            "state": "on",
            "attributes": {
                "friendly_name": "Kitchen",
                "brightness": 128,
                "rgb_color": [255, 128, 0],
                "supported_color_modes": ["color_temp", "rgb"]
            }
        })
    }

    fn media_entity() -> Value {
        json!({
            "entity_id": "media_player.office",
            "state": "playing",
            "attributes": {
                "friendly_name": "Office",
                "volume_level": 0.4,
                "is_volume_muted": false,
                "source": "Spotify",
                "media_title": "Test Track",
                "media_artist": "Tester",
                "device_class": "speaker",
                "supported_features": FEAT_PLAY | FEAT_GROUPING | FEAT_SELECT_SOURCE
            }
        })
    }

    /// A mix of power-domain entities (and one light, to prove it's excluded).
    fn power_entities() -> Value {
        json!([
            { "entity_id": "switch.porch", "state": "on",
              "attributes": { "friendly_name": "Porch" } },
            { "entity_id": "switch.desk_plug", "state": "off",
              "attributes": { "friendly_name": "Desk Plug", "device_class": "outlet" } },
            { "entity_id": "fan.bedroom", "state": "on",
              "attributes": { "friendly_name": "Bedroom Fan" } },
            { "entity_id": "input_boolean.guest_mode", "state": "off",
              "attributes": { "friendly_name": "Guest Mode" } },
            // Not a power device — must be ignored by the power discover.
            light_entity(),
        ])
    }

    async fn mount_states(server: &MockServer, entities: Value) {
        Mock::given(method("GET"))
            .and(path("/api/states"))
            .respond_with(ResponseTemplate::new(200).set_body_json(entities))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn discover_maps_light_entities_with_capabilities() {
        let server = MockServer::start().await;
        mount_states(&server, json!([light_entity()])).await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        let lights = LightProvider::discover(&p).await.unwrap();

        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].provider_id, "light.kitchen");
        assert_eq!(lights[0].name, "Kitchen");
        assert!(lights[0].state.on);
        assert!(lights[0].capabilities.dimmable);
        assert!(lights[0].capabilities.color_rgb);
        assert!(lights[0].capabilities.color_temperature);
        // 128/255*100 ≈ 50.2
        assert!((lights[0].state.brightness.unwrap() - 50.2).abs() < 0.6);
    }

    #[tokio::test]
    async fn set_light_state_calls_turn_on_with_brightness_pct() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/services/light/turn_on"))
            .and(body_string_contains("brightness_pct"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;

        let state = LightState {
            on: true,
            brightness: Some(50.0),
            ..Default::default()
        };
        let p = HaProvider::new_for_test(server.uri()).unwrap();
        LightProvider::set_state(&p, "light.kitchen", &state)
            .await
            .unwrap();

        let reqs = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(body["entity_id"], "light.kitchen");
        assert_eq!(body["brightness_pct"], 50);
    }

    #[tokio::test]
    async fn discover_audio_maps_media_player_state() {
        let server = MockServer::start().await;
        mount_states(&server, json!([media_entity()])).await;

        let devices = AudioProvider::discover(&HaProvider::new_for_test(server.uri()).unwrap())
            .await
            .unwrap();

        assert_eq!(devices.len(), 1);
        let d = &devices[0];
        assert_eq!(d.provider_id, "media_player.office");
        assert_eq!(d.state.volume, 40);
        assert!(d.state.power);
        assert_eq!(d.state.source.as_deref(), Some("Spotify"));
        assert!(d.capabilities.transport);
        assert!(d.capabilities.grouping);
        assert_eq!(
            d.state.now_playing.as_ref().unwrap().title.as_deref(),
            Some("Test Track")
        );
    }

    #[tokio::test]
    async fn set_audio_volume_calls_volume_set_with_fraction() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/services/media_player/volume_set"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;

        let cmd = AudioCommand {
            volume: Some(40),
            ..Default::default()
        };
        let p = HaProvider::new_for_test(server.uri()).unwrap();
        AudioProvider::set_state(&p, "media_player.office", &cmd)
            .await
            .unwrap();

        let reqs = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(body["entity_id"], "media_player.office");
        assert!((body["volume_level"].as_f64().unwrap() - 0.4).abs() < 1e-9);
    }

    #[tokio::test]
    async fn group_joins_on_the_coordinator_with_member_in_group_members() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/services/media_player/join"))
            .and(body_string_contains("media_player.kitchen"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        // kitchen joins the group coordinated by office
        p.group("media_player.kitchen", "media_player.office")
            .await
            .unwrap();

        let reqs = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(body["entity_id"], "media_player.office"); // call targets coordinator
        assert_eq!(body["group_members"][0], "media_player.kitchen");
    }

    #[tokio::test]
    async fn group_rejects_self() {
        let server = MockServer::start().await;
        let p = HaProvider::new_for_test(server.uri()).unwrap();
        let err = p
            .group("media_player.x", "media_player.x")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("itself"), "{err}");
    }

    #[tokio::test]
    async fn discover_groups_maps_areas_to_provider_groups() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/template"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"[{"area_id":"office","name":"Office","entities":["light.kitchen","media_player.office","sensor.x"]}]"#,
            ))
            .mount(&server)
            .await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        let groups = LightProvider::discover_groups(&p).await.unwrap();

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].provider_group_id, "office");
        assert_eq!(groups[0].name, "Office");
        // Only the light entity is a member of the *light* group.
        assert_eq!(groups[0].member_device_ids, vec!["light.kitchen"]);
    }

    #[tokio::test]
    async fn discover_power_maps_switch_fan_and_boolean_with_kinds() {
        let server = MockServer::start().await;
        mount_states(&server, power_entities()).await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        let devices = PowerProvider::discover(&p).await.unwrap();

        // The light entity is excluded; the four power entities map through.
        assert_eq!(devices.len(), 4);
        let by_id = |id: &str| {
            devices
                .iter()
                .find(|d| d.provider_id == id)
                .unwrap_or_else(|| panic!("missing {id}"))
        };
        assert_eq!(by_id("switch.porch").kind, PowerKind::Switch);
        assert!(by_id("switch.porch").state.on);
        // device_class "outlet" upgrades a switch to the plug glyph.
        assert_eq!(by_id("switch.desk_plug").kind, PowerKind::Outlet);
        assert!(!by_id("switch.desk_plug").state.on);
        assert_eq!(by_id("fan.bedroom").kind, PowerKind::Fan);
        assert_eq!(by_id("input_boolean.guest_mode").kind, PowerKind::Toggle);
        assert_eq!(by_id("fan.bedroom").name, "Bedroom Fan");
    }

    #[tokio::test]
    async fn set_power_uses_domain_agnostic_homeassistant_service() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/services/homeassistant/turn_off"))
            .and(body_string_contains("fan.bedroom"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        PowerProvider::set_state(&p, "fan.bedroom", false)
            .await
            .unwrap();

        let reqs = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(body["entity_id"], "fan.bedroom");
    }

    #[tokio::test]
    async fn discover_groups_power_maps_areas_with_power_members() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/template"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"[{"area_id":"garage","name":"Garage","entities":["switch.opener","fan.vent","light.x","sensor.y"]}]"#,
            ))
            .mount(&server)
            .await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        let groups = PowerProvider::discover_groups(&p).await.unwrap();

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "Garage");
        // Only the power-domain entities are members; light/sensor are excluded.
        assert_eq!(
            groups[0].member_device_ids,
            vec!["switch.opener", "fan.vent"]
        );
    }

    #[tokio::test]
    async fn factory_build_ok_and_missing_token_errors() {
        assert!(
            HaLightFactory
                .build(r#"{"base_url":"http://ha.local:8123","token":"abc"}"#)
                .is_ok()
        );
        assert!(
            HaPowerFactory
                .build(r#"{"base_url":"http://ha.local:8123","token":"abc"}"#)
                .is_ok()
        );
        // `.err()` drops the Ok value (a `Box<dyn AudioProvider>`, which isn't
        // `Debug`) so the error can be unwrapped.
        let err = HaAudioFactory
            .build(r#"{"base_url":"http://ha.local:8123"}"#)
            .err()
            .expect("missing token should fail to build");
        assert!(err.to_string().contains("base_url") || err.to_string().contains("token"));
    }
}
