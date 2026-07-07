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
//! - **Media (media_player):** `HaMediaFactory` (`media_player.*` →
//!   `MediaDevice`: power/volume/mute/source/transport, grouping) is registered
//!   too, so HA TVs and speakers surface on the Media page and through the media
//!   API/MCP. Reads on demand (no background manager). *Not yet:* launching
//!   named content on a player (`media_player.play_media` / HA Assist) — that's
//!   the next media increment.
//!
//! ## Primary-entity filter (one-shot WebSocket)
//! HA's model is one **device** → many **entities**; integrations expose a
//! device's settings as extra `switch.*` etc. (e.g. a Sonos speaker's
//! crossfade/loudness toggles). We surface only **primary** controls — entities
//! with no `entity_category`, not disabled/hidden. That signal lives only in the
//! **entity registry**, not `/api/states`, so discovery does a cached **one-shot
//! WebSocket** call (`config/entity_registry/list`) and filters on it. A failed
//! fetch degrades to unfiltered (the old behaviour). See `HA-API.md`.
//!
//! ## Transport: REST poll now, WebSocket push later
//! State is REST-polled (`GET /api/states`, services, `/api/template`) under the
//! existing **poll** model. The next increment upgrades the WebSocket use from
//! the one-shot registry fetch to a persistent `subscribe_events` push channel
//! (mirroring Onkyo's `MediaConnectionMode::Push`) for instant state — see
//! `references/ha_websocket_api.md`.
//!
//! ## Known limitations / next
//! - **De-dup:** a device exposed *both* natively (e.g. Hue) *and* via HA shows
//!   up twice — left to Rooms membership to de-dupe for now.
//! - **Device grouping:** the registry also gives each entity's `device_id` — the
//!   basis for grouping a device's entities (the deferred device-registry import).

use crate::models::generic::{
    Control, GENERIC_HA_EXCLUDED_DOMAINS, GenericDevice, control_write_to_ha, controls_from_ha,
};
use crate::models::media::{
    MediaCapabilities, MediaCommand, MediaDevice, MediaDeviceKind, MediaEvent, MediaState,
    NowPlaying, PlayState, TransportCmd,
};
use crate::models::power::{PowerDevice, PowerKind, PowerState};
use crate::models::remote::{RemoteDevice, RemoteKey, RemoteState};
use crate::models::sensor::{SensorDevice, SensorKind, SensorReading, SensorState};
use crate::models::{Color, Light, LightCapabilities, LightState, Provider};
use crate::providers::{
    CredentialField, FieldKind, GenericProvider, GenericProviderFactory, LightProvider,
    MediaProvider, MediaProviderFactory, PowerProvider, PowerProviderFactory, ProviderFactory,
    ProviderGroup, RemoteProvider, RemoteProviderFactory, SensorProvider, SensorProviderFactory,
};
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use uuid::Uuid;

const LIGHT_PREFIX: &str = "light.";
const MEDIA_PREFIX: &str = "media_player.";
const REMOTE_PREFIX: &str = "remote.";
/// HA entity domains that map onto Bifrost's strictly-on/off `PowerDevice`.
/// `homeassistant.turn_on`/`turn_off` works uniformly across all of them.
const POWER_PREFIXES: &[&str] = &["switch.", "fan.", "input_boolean."];
/// HA entity domains that can carry a Bifrost `SensorDevice`. Unlike power, the
/// `sensor.` domain is a flood, so [`sensor_kind`] is an **allowlist** by
/// `device_class` — only environmental/presence classes are surfaced.
const SENSOR_PREFIXES: &[&str] = &["binary_sensor.", "sensor."];

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
    /// Raw long-lived token — REST uses the header on `client`; the WebSocket
    /// auth message needs the raw value.
    token: String,
    /// Cached entity registry (entity_id → metadata), refreshed over WebSocket.
    /// Lets discovery surface only *primary* device controls, not a device's
    /// `config`/`diagnostic` sub-entities. `None` until first fetched.
    registry_cache: RegistryCache,
    /// Cached device registry (HA `device_id` → normalized hardware id). Joined
    /// with the entity registry's `device_id` to give each entity a `hw_id` for
    /// cross-provider de-dup. `None` until first fetched.
    device_cache: DeviceCache,
}

/// Time-stamped entity registry snapshot behind a lock (see `registry_cache`).
type RegistryCache = Mutex<Option<(Instant, Arc<HashMap<String, EntityMeta>>)>>;
/// Time-stamped device registry snapshot: HA device_id → hardware id.
type DeviceCache = Mutex<Option<(Instant, Arc<HashMap<String, String>>)>>;

/// Entity-registry metadata that decides whether an entity is a primary,
/// user-facing device control or an auxiliary one (a device's config switch,
/// diagnostics, or a disabled/hidden entity). Only in the registry, not
/// `/api/states`. See `HA-API.md`.
#[derive(Debug, Clone, Default, Deserialize)]
struct EntityMeta {
    #[serde(default)]
    entity_category: Option<String>,
    #[serde(default)]
    disabled_by: Option<String>,
    #[serde(default)]
    hidden_by: Option<String>,
    /// The HA *device* this entity belongs to — the join key into the device
    /// registry for a hardware id (de-dup). Absent for entity-only integrations.
    #[serde(default)]
    device_id: Option<String>,
}

impl EntityMeta {
    fn is_primary(&self) -> bool {
        self.entity_category.is_none() && self.disabled_by.is_none() && self.hidden_by.is_none()
    }
}

/// Derive a normalized hardware id from one HA device-registry entry, for
/// cross-provider de-dup. Prefers an explicit `("mac", …)` **connection** (the
/// authoritative source); failing that, falls back to a MAC-shaped value in
/// `identifiers` — some integrations (e.g. Onkyo) key the device by its MAC
/// string there rather than as a connection. `mac_hw_id` only accepts a real
/// MAC-48/EUI-64 shape, so a non-hardware identifier is ignored. `None` when the
/// device exposes no usable hardware id.
fn ha_device_hw_id(d: &Value) -> Option<String> {
    let pairs = |key: &str| {
        d.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|c| c.as_array())
    };
    // Authoritative: a ("mac", "...") connection.
    let from_connection = pairs("connections")
        .filter(|c| c.first().and_then(Value::as_str) == Some("mac"))
        .find_map(|c| c.get(1).and_then(Value::as_str))
        .and_then(crate::providers::mac_hw_id);
    if from_connection.is_some() {
        return from_connection;
    }
    // Fallback: a MAC-shaped identifier value, e.g. ["onkyo", "0009b0e82343"].
    pairs("identifiers")
        .find_map(|c| c.get(1).and_then(Value::as_str))
        .and_then(crate::providers::mac_hw_id)
}

/// Keep an entity iff it's a primary control. Entities absent from the registry
/// (or an empty registry from a failed fetch) default to kept — so a WebSocket
/// failure degrades to the old unfiltered behaviour rather than hiding devices.
fn keep_entity(registry: &HashMap<String, EntityMeta>, entity_id: &str) -> bool {
    registry
        .get(entity_id)
        .map(EntityMeta::is_primary)
        .unwrap_or(true)
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

        // Shared, pooled client keyed by token (the base URL lives on the struct),
        // so per-request rebuilds reuse one warm connection to HA instead of
        // re-handshaking each control. See [`crate::providers::cached_client`].
        let client = crate::providers::cached_client(&format!("ha:{token}"), || {
            let mut headers = HeaderMap::new();
            let mut auth = HeaderValue::from_str(&format!("Bearer {token}"))
                .context("invalid Home Assistant token (not a valid header value)")?;
            auth.set_sensitive(true);
            headers.insert(AUTHORIZATION, auth);
            Ok(Client::builder()
                .default_headers(headers)
                // Bounded so an unreachable HA fails a poll fast instead of hanging.
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(15))
                .build()?)
        })?;

        Ok(Self {
            client,
            base_url,
            token: token.to_string(),
            registry_cache: Mutex::new(None),
            device_cache: Mutex::new(None),
        })
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

    /// Call a service that **returns a response** (`?return_response=true`, e.g.
    /// `media_player.search_media`) and hand back the parsed `service_response`
    /// object. Like [`call_service`] but keeps the JSON body.
    async fn call_service_with_response(
        &self,
        domain: &str,
        service: &str,
        entity_id: &str,
        extra: Value,
    ) -> Result<Value> {
        let mut body = json!({ "entity_id": entity_id });
        if let Value::Object(extra) = extra {
            for (k, v) in extra {
                body[k] = v;
            }
        }
        let resp: Value = self
            .client
            .post(format!(
                "{}/api/services/{domain}/{service}?return_response=true",
                self.base_url
            ))
            .json(&body)
            .send()
            .await
            .with_context(|| format!("HA {domain}.{service} call failed"))?
            .error_for_status()?
            .json()
            .await
            .with_context(|| format!("HA {domain}.{service} response was not JSON"))?;
        Ok(resp.get("service_response").cloned().unwrap_or(resp))
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

    // ── Entity registry (WebSocket) ───────────────────────────────────────────

    fn ws_url(&self) -> String {
        let base = self
            .base_url
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        format!("{base}/api/websocket")
    }

    /// The entity registry, cached briefly. Used to filter discovery to primary
    /// device controls. A fetch failure caches an **empty** map (→ no filtering)
    /// so HA stays usable when the WebSocket can't be reached.
    async fn entity_registry(&self) -> Arc<HashMap<String, EntityMeta>> {
        const TTL: Duration = Duration::from_secs(60);
        if let Some((at, reg)) = self.registry_cache.lock().await.as_ref()
            && at.elapsed() < TTL
        {
            return Arc::clone(reg);
        }
        let reg = Arc::new(self.fetch_entity_registry().await.unwrap_or_else(|e| {
            tracing::warn!(
                "HA entity registry fetch failed ({e:#}); surfacing all entities unfiltered"
            );
            HashMap::new()
        }));
        *self.registry_cache.lock().await = Some((Instant::now(), Arc::clone(&reg)));
        reg
    }

    /// Open a WebSocket and complete the `auth_required → auth → auth_ok`
    /// handshake, returning the authed stream. Shared by every WS caller (the two
    /// registry fetches and the push subscription) so the connect+auth dance lives
    /// in one place.
    async fn ws_connect_authed(&self) -> Result<HaWs> {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message;

        let url = self.ws_url();
        let (mut ws, _) = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(url.as_str()),
        )
        .await
        .context("HA WebSocket connect timed out")?
        .with_context(|| format!("HA WebSocket connect to {url} failed"))?;

        // `auth_required` greeting → send token → expect `auth_ok`.
        let _ = ws_next_json(&mut ws).await?;
        ws.send(Message::text(
            json!({ "type": "auth", "access_token": self.token }).to_string(),
        ))
        .await?;
        let auth = ws_next_json(&mut ws).await?;
        if auth.get("type").and_then(Value::as_str) != Some("auth_ok") {
            bail!("HA WebSocket auth rejected: {auth}");
        }
        Ok(ws)
    }

    /// One-shot `config/<X>/list` WebSocket request: connect+auth, ask, wait for
    /// the `id:1` result, close, and hand back the `result` array. Callers map the
    /// entries (entity vs device registry differ only in that mapping).
    async fn ws_fetch_list(&self, list_type: &str) -> Result<Vec<Value>> {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message;

        let mut ws = self.ws_connect_authed().await?;
        ws.send(Message::text(
            json!({ "id": 1, "type": list_type }).to_string(),
        ))
        .await?;
        let result = loop {
            let v = ws_next_json(&mut ws).await?;
            if v.get("id").and_then(Value::as_i64) == Some(1)
                && v.get("type").and_then(Value::as_str) == Some("result")
            {
                break v;
            }
        };
        let _ = ws.close(None).await;
        result
            .get("result")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| anyhow!("HA {list_type} result has no array"))
    }

    /// One-shot WebSocket fetch of `config/entity_registry/list`. See `HA-API.md`.
    async fn fetch_entity_registry(&self) -> Result<HashMap<String, EntityMeta>> {
        let entries = self.ws_fetch_list("config/entity_registry/list").await?;
        let mut map = HashMap::with_capacity(entries.len());
        for e in entries {
            if let Some(id) = e
                .get("entity_id")
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                let meta: EntityMeta = serde_json::from_value(e).unwrap_or_default();
                map.insert(id, meta);
            }
        }
        Ok(map)
    }

    /// The device registry (HA `device_id` → hardware id), cached briefly like
    /// the entity registry. A fetch failure caches an **empty** map, so de-dup
    /// simply doesn't fire rather than breaking discovery.
    async fn device_registry(&self) -> Arc<HashMap<String, String>> {
        const TTL: Duration = Duration::from_secs(60);
        if let Some((at, reg)) = self.device_cache.lock().await.as_ref()
            && at.elapsed() < TTL
        {
            return Arc::clone(reg);
        }
        let reg = Arc::new(self.fetch_device_registry().await.unwrap_or_else(|e| {
            tracing::warn!("HA device registry fetch failed ({e:#}); de-dup disabled this run");
            HashMap::new()
        }));
        *self.device_cache.lock().await = Some((Instant::now(), Arc::clone(&reg)));
        reg
    }

    /// One-shot WebSocket fetch of `config/device_registry/list`, mapping each
    /// device's id to a normalized hardware id taken from its `connections`
    /// (the `("mac", …)` pair). Devices without a usable MAC are omitted.
    async fn fetch_device_registry(&self) -> Result<HashMap<String, String>> {
        let entries = self.ws_fetch_list("config/device_registry/list").await?;
        let mut map = HashMap::new();
        for d in entries {
            let Some(id) = d.get("id").and_then(Value::as_str).map(str::to_string) else {
                continue;
            };
            if let Some(hw) = ha_device_hw_id(&d) {
                map.insert(id, hw);
            }
        }
        Ok(map)
    }

    /// Each discoverable entity's hardware id (`entity_id` → `hw_id`), joining
    /// the entity registry (entity → device) with the device registry (device →
    /// MAC). Entities with no device or no MAC are simply absent (→ no de-dup).
    async fn entity_hw_ids(&self) -> HashMap<String, String> {
        let entities = self.entity_registry().await;
        let devices = self.device_registry().await;
        if devices.is_empty() {
            return HashMap::new();
        }
        entities
            .iter()
            .filter_map(|(entity_id, meta)| {
                let dev = meta.device_id.as_deref()?;
                let hw = devices.get(dev)?;
                Some((entity_id.clone(), hw.clone()))
            })
            .collect()
    }

    /// Areas containing entities whose id starts with any of `prefixes`, as
    /// `ProviderGroup`s. Shared by the light, media, and power `discover_groups`
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
        let reg = self.entity_registry().await;

        Ok(areas
            .into_iter()
            .filter_map(|a| {
                let members: Vec<String> = a
                    .entities
                    .into_iter()
                    .filter(|e| prefixes.iter().any(|p| e.starts_with(p)) && keep_entity(&reg, e))
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
    /// RFC 3339 timestamp of the entity's last state change.
    #[serde(default)]
    last_changed: Option<String>,
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
        .map(|k| crate::models::kelvin_to_mirek(k as u32))
        .or_else(|| attr_u64(attrs, "color_temp").map(|m| m as u16));

    // HA reports the active effect by name, or the literal "None" when idle.
    let effect = attr_str(attrs, "effect").filter(|e| e != "None" && !e.is_empty());

    LightState {
        on,
        brightness,
        color,
        color_temp_mirek,
        reachable: Some(e.state != "unavailable"),
        effect,
        transport: None,
        ip: None,
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
        // HA surfaces the entity's supported effects as `effect_list`; pass them
        // through verbatim (the UI humanizes "None" → "Off").
        effects: attr_str_vec(attrs, "effect_list"),
        // HA doesn't surface a per-segment colour capability uniformly; native
        // providers (Govee) own segment control.
        segments: None,
    }
}

fn entity_to_light(e: HaEntity, hw_id: Option<String>) -> Light {
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
        hw_id,
    }
}

// ── Media mapping ─────────────────────────────────────────────────────────────

/// `base_url` is the HA instance root — `entity_picture` artwork is a
/// HA-relative path (`/api/media_player_proxy/…`) that must be absolutized so
/// the browser can load it straight from HA.
fn parse_media_state(e: &HaEntity, base_url: &str) -> MediaState {
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
    let artwork_url = attr_str(attrs, "entity_picture").map(|p| {
        if p.starts_with("http://") || p.starts_with("https://") {
            p
        } else {
            format!(
                "{}{}{}",
                base_url,
                if p.starts_with('/') { "" } else { "/" },
                p
            )
        }
    });
    let now_playing = if title.is_some() || artist.is_some() || album.is_some() {
        Some(NowPlaying {
            title,
            artist,
            album,
            play_state,
            artwork_url,
        })
    } else {
        None
    };

    MediaState {
        power,
        volume,
        mute: attr_bool(attrs, "is_volume_muted").unwrap_or(false),
        source: attr_str(attrs, "source"),
        // On a smart TV, `source_list` is the installed apps (Hulu, Netflix, …);
        // on a receiver, its inputs. Switch with `select_source` (our `source`).
        source_list: attr_str_vec(attrs, "source_list"),
        now_playing,
        reachable: Some(reachable),
        group_coordinator: None, // HA media_player grouping not modelled yet
        ip: None,
    }
}

fn media_capabilities(attrs: &Value) -> MediaCapabilities {
    let feat = attr_u64(attrs, "supported_features").unwrap_or(0);
    let has = |bit: u64| feat & bit != 0;
    MediaCapabilities {
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

fn media_kind(attrs: &Value) -> MediaDeviceKind {
    match attr_str(attrs, "device_class").as_deref() {
        Some("tv") => return MediaDeviceKind::Tv,
        Some("receiver") => return MediaDeviceKind::Receiver,
        _ => {}
    }
    // Many smart-TV integrations (e.g. Sony BRAVIA) don't set `device_class`, so
    // they'd fall through to "Speaker". But a `media_player` that reports a
    // running app (`app_id`/`app_name`) is a TV / streamer, not a speaker — a
    // speaker never runs named apps. Use that as the fallback TV signal.
    if attr_str(attrs, "app_id").is_some() || attr_str(attrs, "app_name").is_some() {
        return MediaDeviceKind::Tv;
    }
    MediaDeviceKind::Speaker
}

fn entity_to_media(e: HaEntity, hw_id: Option<String>, base_url: &str) -> MediaDevice {
    let state = parse_media_state(&e, base_url);
    let capabilities = media_capabilities(&e.attributes);
    let kind = media_kind(&e.attributes);
    MediaDevice {
        id: Uuid::new_v4(),
        provider_id: e.entity_id.clone(),
        name: friendly_name(&e.entity_id, &e.attributes),
        kind,
        capabilities,
        state,
        hw_id,
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

/// Pick the first **playable** hit out of a `media_player.search_media` response
/// and return its `(media_content_id, media_content_type)` to cast. Tolerates the
/// two shapes HA emits — a bare `{ "result": [...] }` or per-entity
/// `{ "<entity_id>": { "result": [...] } }` — and skips items HA flags
/// `can_play: false`. `None` when nothing usable was returned.
fn first_search_hit(service_response: &Value) -> Option<(String, String)> {
    fn results(v: &Value) -> Option<&Vec<Value>> {
        v.get("result").and_then(Value::as_array)
    }
    let items = results(service_response).or_else(|| {
        service_response
            .as_object()?
            .values()
            .find_map(|v| results(v))
    })?;
    items
        .iter()
        .filter(|it| it.get("can_play").and_then(Value::as_bool) != Some(false))
        .find_map(|it| {
            let id = it.get("media_content_id").and_then(Value::as_str)?;
            let ty = it
                .get("media_content_type")
                .and_then(Value::as_str)
                .unwrap_or("video");
            Some((id.to_string(), ty.to_string()))
        })
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

fn entity_to_power(e: HaEntity, hw_id: Option<String>) -> PowerDevice {
    let state = parse_power_state(&e);
    let kind = power_kind(&e.entity_id, &e.attributes);
    PowerDevice {
        id: Uuid::new_v4(),
        kind,
        name: friendly_name(&e.entity_id, &e.attributes),
        provider_id: e.entity_id.clone(),
        state,
        hw_id,
    }
}

// ── Sensor mapping ─────────────────────────────────────────────────────────────

/// Classify a `binary_sensor`/`sensor` entity into a Bifrost [`SensorKind`], or
/// `None` to skip it. This is a deliberate **allowlist** by `device_class` — the
/// `sensor.` domain is HA's biggest flood (diagnostics, uptime, signal, …), so
/// only the environmental/presence classes Bifrost models are surfaced. Widen it
/// by adding an arm, not by loosening the filter.
fn sensor_kind(entity_id: &str, attrs: &Value) -> Option<SensorKind> {
    let dc = attr_str(attrs, "device_class");
    if entity_id.starts_with("binary_sensor.") {
        match dc.as_deref() {
            Some("motion") => Some(SensorKind::Motion),
            Some("occupancy" | "presence") => Some(SensorKind::Occupancy),
            Some("door" | "window" | "opening" | "garage_door") => Some(SensorKind::Contact),
            _ => None,
        }
    } else if entity_id.starts_with("sensor.") {
        match dc.as_deref() {
            Some("illuminance") => Some(SensorKind::Illuminance),
            Some("temperature") => Some(SensorKind::Temperature),
            Some("humidity") => Some(SensorKind::Humidity),
            _ => None,
        }
    } else {
        None
    }
}

/// Parse an HA entity's `state` into a [`SensorState`] for the given kind.
/// Boolean kinds read `on`/`off`; numeric kinds parse the state as a float.
/// `unavailable`/`unknown`/unparseable → no reading (reachability preserved).
fn parse_sensor_state(e: &HaEntity, kind: SensorKind) -> SensorState {
    let reachable = Some(e.state != "unavailable");
    if e.state == "unavailable" || e.state == "unknown" {
        return SensorState {
            reading: None,
            reachable,
            changed_at: None,
        };
    }
    let reading = match kind {
        SensorKind::Motion | SensorKind::Occupancy | SensorKind::Contact => {
            Some(SensorReading::Bool(e.state == "on"))
        }
        _ => e.state.parse::<f64>().ok().map(SensorReading::Number),
    };
    SensorState {
        reading,
        reachable,
        changed_at: e.last_changed.clone(),
    }
}

/// Map one modeled sensor entity to a [`SensorDevice`], or `None` to skip it.
fn entity_to_sensor(e: HaEntity, hw_id: Option<String>) -> Option<SensorDevice> {
    let kind = sensor_kind(&e.entity_id, &e.attributes)?;
    let unit = attr_str(&e.attributes, "unit_of_measurement");
    Some(SensorDevice {
        id: Uuid::new_v4(),
        kind,
        name: friendly_name(&e.entity_id, &e.attributes),
        provider_id: e.entity_id.clone(),
        state: parse_sensor_state(&e, kind),
        unit,
        hw_id,
    })
}

// ── LightProvider impl ────────────────────────────────────────────────────────

#[async_trait]
impl LightProvider for HaProvider {
    fn name(&self) -> &str {
        "homeassistant"
    }

    async fn discover(&self) -> Result<Vec<Light>> {
        let reg = self.entity_registry().await;
        let hw = self.entity_hw_ids().await;
        Ok(self
            .get_states()
            .await?
            .into_iter()
            .filter(|e| e.entity_id.starts_with(LIGHT_PREFIX) && keep_entity(&reg, &e.entity_id))
            .map(|e| {
                let hw_id = hw.get(&e.entity_id).cloned();
                entity_to_light(e, hw_id)
            })
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
        // An effect pick is its own dimension — `light.turn_on { effect }` selects
        // it (and clears colour/temp), so route it instead of a colour/temp body.
        if let Some(effect) = state.effect.as_deref().filter(|e| !e.is_empty()) {
            data["effect"] = json!(effect);
        } else if let Some(color) = &state.color {
            // Prefer explicit color; else colour temperature if present.
            let (r, g, b) = color.to_rgb();
            data["rgb_color"] = json!([r, g, b]);
        } else if let Some(mirek) = state.color_temp_mirek {
            data["color_temp_kelvin"] = json!(crate::models::mirek_to_kelvin(mirek));
        }
        self.call_service("light", "turn_on", device_id, data).await
    }

    async fn discover_groups(&self) -> Result<Vec<ProviderGroup>> {
        self.discover_groups_for(&[LIGHT_PREFIX]).await
    }
}

// ── MediaProvider impl ────────────────────────────────────────────────────────

#[async_trait]
impl MediaProvider for HaProvider {
    fn name(&self) -> &str {
        "homeassistant"
    }

    async fn discover(&self) -> Result<Vec<MediaDevice>> {
        let reg = self.entity_registry().await;
        let hw = self.entity_hw_ids().await;
        Ok(self
            .get_states()
            .await?
            .into_iter()
            .filter(|e| e.entity_id.starts_with(MEDIA_PREFIX) && keep_entity(&reg, &e.entity_id))
            .map(|e| {
                let hw_id = hw.get(&e.entity_id).cloned();
                entity_to_media(e, hw_id, &self.base_url)
            })
            .collect())
    }

    async fn get_state(&self, device_id: &str) -> Result<MediaState> {
        Ok(parse_media_state(
            &self.get_entity(device_id).await?,
            &self.base_url,
        ))
    }

    async fn set_state(&self, device_id: &str, cmd: &MediaCommand) -> Result<()> {
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

    /// Cast: raw passthrough to HA `media_player.play_media`. `content_type` maps
    /// to HA's `media_content_type` (`music`/`url`/`app`/`channel`/…).
    async fn play_media(
        &self,
        device_id: &str,
        content_id: &str,
        content_type: &str,
    ) -> Result<()> {
        self.call_service(
            "media_player",
            "play_media",
            device_id,
            json!({ "media_content_id": content_id, "media_content_type": content_type }),
        )
        .await
    }

    /// Resolve a human title via `media_player.search_media` and cast the top
    /// playable hit with `media_player.play_media`. `Ok(false)` when the search
    /// returns nothing (or the integration has no search), so the caller can fall
    /// back to just opening an app.
    async fn search_and_play(&self, device_id: &str, query: &str) -> Result<bool> {
        let resp = self
            .call_service_with_response(
                "media_player",
                "search_media",
                device_id,
                json!({ "search_query": query }),
            )
            .await?;
        let Some((content_id, content_type)) = first_search_hit(&resp) else {
            return Ok(false);
        };
        self.play_media(device_id, &content_id, &content_type)
            .await?;
        Ok(true)
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
        let reg = self.entity_registry().await;
        let hw = self.entity_hw_ids().await;
        Ok(self
            .get_states()
            .await?
            .into_iter()
            .filter(|e| {
                POWER_PREFIXES.iter().any(|p| e.entity_id.starts_with(p))
                    && keep_entity(&reg, &e.entity_id)
            })
            .map(|e| {
                let hw_id = hw.get(&e.entity_id).cloned();
                entity_to_power(e, hw_id)
            })
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

// ── SensorProvider impl ────────────────────────────────────────────────────────

#[async_trait]
impl SensorProvider for HaProvider {
    fn name(&self) -> &str {
        "homeassistant"
    }

    async fn discover(&self) -> Result<Vec<SensorDevice>> {
        let reg = self.entity_registry().await;
        let hw = self.entity_hw_ids().await;
        Ok(self
            .get_states()
            .await?
            .into_iter()
            .filter(|e| {
                SENSOR_PREFIXES.iter().any(|p| e.entity_id.starts_with(p))
                    && keep_entity(&reg, &e.entity_id)
            })
            .filter_map(|e| {
                let hw_id = hw.get(&e.entity_id).cloned();
                entity_to_sensor(e, hw_id)
            })
            .collect())
    }

    async fn get_state(&self, device_id: &str) -> Result<SensorState> {
        let entity = self.get_entity(device_id).await?;
        // Recompute the kind so a boolean vs numeric read is parsed correctly.
        let kind =
            sensor_kind(&entity.entity_id, &entity.attributes).unwrap_or(SensorKind::Generic);
        Ok(parse_sensor_state(&entity, kind))
    }

    async fn discover_groups(&self) -> Result<Vec<ProviderGroup>> {
        self.discover_groups_for(SENSOR_PREFIXES).await
    }
}

// ── WebSocket push (state_changed) ──────────────────────────────────────────

/// One pushed state change off the HA `state_changed` subscription, already
/// classified into the Bifrost device domain it belongs to. The push manager
/// fans these onto the per-domain event pipelines (light / media / power), so a
/// single HA WebSocket keeps **all three** domains live instead of 30 s polling.
#[derive(Debug, Clone)]
pub enum HaPushEvent {
    Light {
        device_id: String,
        state: LightState,
    },
    Media(MediaEvent),
    Power {
        device_id: String,
        state: PowerState,
    },
    Sensor {
        device_id: String,
        state: SensorState,
    },
}

/// Map one HA entity (the `new_state` of a `state_changed` event) to its domain
/// event, or `None` for entity domains Bifrost doesn't track.
fn classify_push(e: HaEntity, base_url: &str) -> Option<HaPushEvent> {
    let id = e.entity_id.clone();
    if id.starts_with(LIGHT_PREFIX) {
        Some(HaPushEvent::Light {
            device_id: id,
            state: parse_light_state(&e),
        })
    } else if id.starts_with(MEDIA_PREFIX) {
        Some(HaPushEvent::Media(MediaEvent {
            device_id: id,
            state: parse_media_state(&e, base_url),
        }))
    } else if POWER_PREFIXES.iter().any(|p| id.starts_with(p)) {
        Some(HaPushEvent::Power {
            device_id: id,
            state: parse_power_state(&e),
        })
    } else {
        // Only modeled sensor classes (see `sensor_kind`) push; the rest of the
        // binary_sensor/sensor flood is ignored, matching discovery.
        sensor_kind(&id, &e.attributes).map(|kind| HaPushEvent::Sensor {
            state: parse_sensor_state(&e, kind),
            device_id: id,
        })
    }
}

impl HaProvider {
    /// Forward a natural-language command to **HA Assist**
    /// (`POST /api/conversation/process`) and return its spoken response plus
    /// whether HA acted successfully (`response_type` is not `error`).
    ///
    /// This is the voice pipeline's long-tail fallback: Bifrost's native grammar
    /// handles what it can deterministically, and anything it can't — notably
    /// "play <named content> on the <TV>" — is delegated to HA, which resolves
    /// and acts on it (reusing HA's media resolution across all integrations).
    pub async fn converse(&self, text: &str) -> Result<(String, bool)> {
        let resp = self
            .client
            .post(format!("{}/api/conversation/process", self.base_url))
            .json(&json!({ "text": text }))
            .send()
            .await
            .context("HA conversation request failed")?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let speech = resp
            .pointer("/response/speech/plain/speech")
            .and_then(Value::as_str)
            .unwrap_or("Okay.")
            .to_string();
        let ok = resp
            .pointer("/response/response_type")
            .and_then(Value::as_str)
            != Some("error");
        Ok((speech, ok))
    }

    /// Open a persistent WebSocket, authenticate, and `subscribe_events` to
    /// `state_changed`, returning a stream of classified per-domain push events.
    ///
    /// Mirrors the media push pattern (Onkyo's `event_stream`): the handshake
    /// runs synchronously so a connect/auth failure is surfaced as `Err` (the
    /// manager backs off), then a spawned task pumps frames until the socket
    /// drops — at which point the sender closes and the manager reconnects. This
    /// method does **not** own reconnection; the push manager does.
    pub async fn push_events(&self) -> Result<tokio::sync::mpsc::Receiver<HaPushEvent>> {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let mut ws = self.ws_connect_authed().await?;

        // Subscribe to state changes; expect the `result`/`success` ack.
        ws.send(Message::text(
            json!({ "id": 1, "type": "subscribe_events", "event_type": "state_changed" })
                .to_string(),
        ))
        .await?;
        let ack = ws_next_json(&mut ws).await?;
        if ack.get("type").and_then(Value::as_str) == Some("result")
            && ack.get("success").and_then(Value::as_bool) != Some(true)
        {
            bail!("HA subscribe_events rejected: {ack}");
        }

        let (tx, rx) = tokio::sync::mpsc::channel::<HaPushEvent>(128);
        let base_url = self.base_url.clone();
        tokio::spawn(async move {
            loop {
                let v = match ws.next().await {
                    Some(Ok(Message::Text(t))) => match serde_json::from_str::<Value>(t.as_str()) {
                        Ok(v) => v,
                        Err(_) => continue,
                    },
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    // Close, error, non-text, or stream end → drop the sender so
                    // the manager sees the channel close and reconnects.
                    _ => break,
                };
                if v.get("type").and_then(Value::as_str) != Some("event") {
                    continue;
                }
                // `state_changed` carries `event.data.new_state` (null on removal).
                let new_state = v.pointer("/event/data/new_state");
                let Some(new_state) = new_state.filter(|s| !s.is_null()) else {
                    continue;
                };
                let Ok(entity) = serde_json::from_value::<HaEntity>(new_state.clone()) else {
                    continue;
                };
                if let Some(event) = classify_push(entity, &base_url)
                    && tx.send(event).await.is_err()
                {
                    return; // consumer dropped
                }
            }
        });
        Ok(rx)
    }
}

// ── Remote mapping ─────────────────────────────────────────────────────────────

/// Map a canonical Bifrost [`RemoteKey`] to the Android TV Remote keycode HA's
/// `remote.send_command` expects (see the androidtv_remote integration docs).
fn remote_key_command(key: RemoteKey) -> &'static str {
    match key {
        RemoteKey::Up => "DPAD_UP",
        RemoteKey::Down => "DPAD_DOWN",
        RemoteKey::Left => "DPAD_LEFT",
        RemoteKey::Right => "DPAD_RIGHT",
        RemoteKey::Select => "DPAD_CENTER",
        RemoteKey::Back => "BACK",
        RemoteKey::Home => "HOME",
        RemoteKey::Menu => "MENU",
        RemoteKey::VolumeUp => "VOLUME_UP",
        RemoteKey::VolumeDown => "VOLUME_DOWN",
        RemoteKey::Mute => "MUTE",
        RemoteKey::PlayPause => "MEDIA_PLAY_PAUSE",
        RemoteKey::Next => "MEDIA_NEXT",
        RemoteKey::Previous => "MEDIA_PREVIOUS",
        RemoteKey::Power => "POWER",
    }
}

fn parse_remote_state(e: &HaEntity) -> RemoteState {
    RemoteState {
        on: e.state == "on",
        current_app: attr_str(&e.attributes, "current_activity"),
        reachable: Some(e.state != "unavailable"),
        ip: None,
    }
}

fn entity_to_remote(e: HaEntity, hw_id: Option<String>) -> RemoteDevice {
    let state = parse_remote_state(&e);
    RemoteDevice {
        id: Uuid::new_v4(),
        name: friendly_name(&e.entity_id, &e.attributes),
        provider_id: e.entity_id.clone(),
        state,
        hw_id,
    }
}

// ── RemoteProvider impl ─────────────────────────────────────────────────────────

#[async_trait]
impl RemoteProvider for HaProvider {
    fn name(&self) -> &str {
        "homeassistant"
    }

    async fn discover(&self) -> Result<Vec<RemoteDevice>> {
        let reg = self.entity_registry().await;
        let hw = self.entity_hw_ids().await;
        Ok(self
            .get_states()
            .await?
            .into_iter()
            .filter(|e| e.entity_id.starts_with(REMOTE_PREFIX) && keep_entity(&reg, &e.entity_id))
            .map(|e| {
                let hw_id = hw.get(&e.entity_id).cloned();
                entity_to_remote(e, hw_id)
            })
            .collect())
    }

    async fn get_state(&self, device_id: &str) -> Result<RemoteState> {
        Ok(parse_remote_state(&self.get_entity(device_id).await?))
    }

    async fn send_key(
        &self,
        device_id: &str,
        key: RemoteKey,
        hold_secs: Option<f32>,
    ) -> Result<()> {
        let mut data = json!({ "command": remote_key_command(key) });
        if let Some(secs) = hold_secs {
            data["hold_secs"] = json!(secs);
        }
        self.call_service("remote", "send_command", device_id, data)
            .await
    }

    async fn send_text(&self, device_id: &str, text: &str) -> Result<()> {
        // The Android TV Remote integration types literal text via `text:<str>`.
        self.call_service(
            "remote",
            "send_command",
            device_id,
            json!({ "command": format!("text:{text}") }),
        )
        .await
    }

    async fn launch_app(&self, device_id: &str, activity: &str) -> Result<()> {
        // `remote.turn_on { activity }` accepts a Play Store package id or a
        // deep-link URL and brings that app to the foreground.
        self.call_service(
            "remote",
            "turn_on",
            device_id,
            json!({ "activity": activity }),
        )
        .await
    }

    async fn set_power(&self, device_id: &str, on: bool) -> Result<()> {
        let svc = if on { "turn_on" } else { "turn_off" };
        self.call_service("remote", svc, device_id, json!({})).await
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn normalise_base_url(raw: &str) -> String {
    crate::providers::base_url(raw, "http", None)
}

/// The HA WebSocket stream type returned by `ws_connect_authed`.
type HaWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Read the next text frame from a HA WebSocket as JSON, skipping ping/pong.
async fn ws_next_json<S>(ws: &mut S) -> Result<Value>
where
    S: futures_util::Stream<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Unpin,
{
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Message;
    while let Some(msg) = ws.next().await {
        match msg? {
            Message::Text(t) => return Ok(serde_json::from_str(t.as_str())?),
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => bail!("HA WebSocket closed during handshake"),
            _ => continue,
        }
    }
    bail!("HA WebSocket stream ended before a response")
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
    /// HA pushes state over its `subscribe_events` WebSocket, so the runtime
    /// keeps one persistent connection (`HaPushManager`) live across all of HA's
    /// device domains instead of polling each on an interval.
    fn connection_mode(&self) -> crate::providers::ConnectionMode {
        crate::providers::ConnectionMode::HaPush
    }
    /// HA isn't a single-device-domain provider — it's a platform adapter that
    /// can surface many device kinds — so it's filed under "Integrations" in the
    /// add-provider UI rather than under "Lights".
    fn domain(&self) -> crate::providers::ProviderDomain {
        crate::providers::ProviderDomain::Integration
    }
}

/// Media side of the same HA adapter. Registered with `register_media(...)`.
/// NOTE: shares the `"ha"` type key with `HaLightFactory` — see the
/// registry-exclusivity decision in the module docs.
pub struct HaMediaFactory;

impl MediaProviderFactory for HaMediaFactory {
    fn provider_type(&self) -> &'static str {
        "ha"
    }
    fn display_name(&self) -> &'static str {
        "Home Assistant"
    }
    fn build(&self, credentials_json: &str) -> Result<Box<dyn MediaProvider>> {
        Ok(Box::new(HaProvider::from_credentials(credentials_json)?))
    }
    fn credentials_schema(&self) -> &'static [CredentialField] {
        HA_CREDENTIALS
    }
}

/// Power side of the HA adapter (`switch.*` / `fan.*` / `input_boolean.*`).
/// Registered with `register_power(...)` **alongside** `HaLightFactory` — the
/// same `"ha"` provider row serves both domains. This is wired in
/// `default_registry()` (unlike `HaMediaFactory`, which stays on hold).
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

/// Sensor side of the HA adapter (`binary_sensor.*` / `sensor.*`, allowlisted to
/// motion/occupancy/contact/illuminance/temperature/humidity — see
/// [`sensor_kind`]). Registered with `register_sensor(...)` alongside the other
/// HA factories; readings stay live over the shared `HaPushManager` WebSocket.
pub struct HaSensorFactory;

impl SensorProviderFactory for HaSensorFactory {
    fn provider_type(&self) -> &'static str {
        "ha"
    }
    fn display_name(&self) -> &'static str {
        "Home Assistant"
    }
    fn build(&self, credentials_json: &str) -> Result<Box<dyn SensorProvider>> {
        Ok(Box::new(HaProvider::from_credentials(credentials_json)?))
    }
    fn credentials_schema(&self) -> &'static [CredentialField] {
        HA_CREDENTIALS
    }
}

/// Remote side of the HA adapter (`remote.*` — Android TV / streamer remotes).
/// Registered with `register_remote(...)` alongside the other HA factories.
pub struct HaRemoteFactory;

impl RemoteProviderFactory for HaRemoteFactory {
    fn provider_type(&self) -> &'static str {
        "ha"
    }
    fn display_name(&self) -> &'static str {
        "Home Assistant"
    }
    fn build(&self, credentials_json: &str) -> Result<Box<dyn RemoteProvider>> {
        Ok(Box::new(HaProvider::from_credentials(credentials_json)?))
    }
    fn credentials_schema(&self) -> &'static [CredentialField] {
        HA_CREDENTIALS
    }
}

// ── Generic passthrough (the controllable long tail) ─────────────────────────

#[async_trait]
impl GenericProvider for HaProvider {
    fn name(&self) -> &str {
        "Home Assistant"
    }

    async fn discover(&self) -> Result<Vec<GenericDevice>> {
        let reg = self.entity_registry().await;
        let hw = self.entity_hw_ids().await;
        Ok(self
            .get_states()
            .await?
            .into_iter()
            .filter(|e| {
                // Escape-hatch by default: surface every entity except the
                // natively-handled / read-only domains, and any non-primary
                // (hidden/disabled/diagnostic) entity.
                !GENERIC_HA_EXCLUDED_DOMAINS
                    .iter()
                    .any(|p| e.entity_id.starts_with(p))
                    && keep_entity(&reg, &e.entity_id)
            })
            .map(|e| {
                let kind = e.entity_id.split('.').next().unwrap_or("").to_string();
                GenericDevice {
                    provider_id: String::new(), // set by the API layer
                    name: friendly_name(&e.entity_id, &e.attributes),
                    controls: controls_from_ha(&kind, &e.state, &e.attributes),
                    kind,
                    hw_id: hw.get(&e.entity_id).cloned(),
                    device_id: e.entity_id,
                }
            })
            .collect())
    }

    async fn get_controls(&self, device_id: &str) -> Result<Vec<Control>> {
        let e = self.get_entity(device_id).await?;
        let domain = device_id.split('.').next().unwrap_or("");
        Ok(controls_from_ha(domain, &e.state, &e.attributes))
    }

    async fn set_control(&self, device_id: &str, key: &str, value: &Value) -> Result<()> {
        let domain = device_id.split('.').next().unwrap_or("");
        let (svc_domain, service, extra) = control_write_to_ha(domain, key, value)
            .ok_or_else(|| anyhow!("no service mapping for {domain} control '{key}'"))?;
        self.call_service(&svc_domain, &service, device_id, extra)
            .await
    }
}

/// Generic-passthrough side of the HA adapter — climate, cover, lock, `number`,
/// `select`, `button`, … surfaced as control primitives. Registered with
/// `register_generic(...)` alongside the other HA factories.
pub struct HaGenericFactory;

impl GenericProviderFactory for HaGenericFactory {
    fn provider_type(&self) -> &'static str {
        "ha"
    }
    fn build(&self, credentials_json: &str) -> Result<Box<dyn GenericProvider>> {
        Ok(Box::new(HaProvider::from_credentials(credentials_json)?))
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

    #[test]
    fn ha_device_hw_id_prefers_mac_connection() {
        let d = json!({
            "connections": [["mac", "00:09:B0:E8:23:43"]],
            "identifiers": [["onkyo", "somethingelse"]],
        });
        assert_eq!(ha_device_hw_id(&d).as_deref(), Some("mac:0009b0e82343"));
    }

    #[test]
    fn ha_device_hw_id_falls_back_to_mac_shaped_identifier() {
        // Onkyo's HA integration keys the device by its MAC string in identifiers,
        // not as a ("mac", …) connection — this is what made it miss de-dup.
        let d = json!({
            "connections": [],
            "identifiers": [["onkyo", "0009b0e82343"]],
        });
        assert_eq!(ha_device_hw_id(&d).as_deref(), Some("mac:0009b0e82343"));
    }

    #[test]
    fn ha_device_hw_id_ignores_non_hardware_identifiers() {
        // A non-MAC identifier (a UUID, an entity id) must not become a hw_id.
        let d = json!({
            "identifiers": [["hue", "0b2c3d4e-1111-2222-3333-444455556666"]],
        });
        assert_eq!(ha_device_hw_id(&d), None);
        assert_eq!(ha_device_hw_id(&json!({})), None);
    }

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
                "source_list": ["Spotify", "Hulu", "Netflix"],
                "media_title": "Test Track",
                "media_artist": "Tester",
                "entity_picture": "/api/media_player_proxy/media_player.office?token=abc",
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
    async fn discover_passes_through_effect_list_and_active_effect() {
        let server = MockServer::start().await;
        mount_states(
            &server,
            json!([{
                "entity_id": "light.strip",
                "state": "on",
                "attributes": {
                    "friendly_name": "Strip",
                    "supported_color_modes": ["rgb"],
                    "effect_list": ["None", "Rainbow", "Colorloop"],
                    "effect": "Rainbow"
                }
            }]),
        )
        .await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        let lights = LightProvider::discover(&p).await.unwrap();
        assert_eq!(
            lights[0].capabilities.effects,
            vec!["None", "Rainbow", "Colorloop"]
        );
        assert_eq!(lights[0].state.effect.as_deref(), Some("Rainbow"));
    }

    #[test]
    fn idle_effect_reported_as_none_is_cleared() {
        // HA reports "None" (the string) when no effect is running.
        let e: HaEntity = serde_json::from_value(json!({
            "entity_id": "light.x",
            "state": "on",
            "attributes": { "effect": "None" }
        }))
        .unwrap();
        assert_eq!(parse_light_state(&e).effect, None);
    }

    #[tokio::test]
    async fn set_light_state_with_effect_calls_turn_on_with_effect() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/services/light/turn_on"))
            .and(body_string_contains("effect"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;

        let state = LightState {
            on: true,
            effect: Some("Rainbow".to_string()),
            // A colour is also set, but the effect wins (mutually exclusive on HA).
            color: Some(Color::from_rgb(255, 0, 0)),
            ..Default::default()
        };
        let p = HaProvider::new_for_test(server.uri()).unwrap();
        LightProvider::set_state(&p, "light.strip", &state)
            .await
            .unwrap();

        let reqs = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(body["effect"], "Rainbow");
        assert!(body.get("rgb_color").is_none(), "effect supersedes colour");
    }

    #[tokio::test]
    async fn discover_media_maps_media_player_state() {
        let server = MockServer::start().await;
        mount_states(&server, json!([media_entity()])).await;

        let devices = MediaProvider::discover(&HaProvider::new_for_test(server.uri()).unwrap())
            .await
            .unwrap();

        assert_eq!(devices.len(), 1);
        let d = &devices[0];
        assert_eq!(d.provider_id, "media_player.office");
        assert_eq!(d.state.volume, 40);
        assert!(d.state.power);
        assert_eq!(d.state.source.as_deref(), Some("Spotify"));
        // The TV/receiver's selectable inputs/apps are surfaced for switching.
        assert_eq!(d.state.source_list, vec!["Spotify", "Hulu", "Netflix"]);
        assert!(d.capabilities.transport);
        assert!(d.capabilities.grouping);
        assert_eq!(
            d.state.now_playing.as_ref().unwrap().title.as_deref(),
            Some("Test Track")
        );
        // HA's `entity_picture` is instance-relative; it must come back absolute
        // (joined to the provider base URL) so the browser can load it directly.
        assert_eq!(
            d.state.now_playing.as_ref().unwrap().artwork_url.as_deref(),
            Some(
                format!(
                    "{}/api/media_player_proxy/media_player.office?token=abc",
                    server.uri()
                )
                .as_str()
            )
        );
    }

    #[test]
    fn media_kind_uses_app_signal_when_device_class_absent() {
        // `device_class` wins when present.
        assert_eq!(
            media_kind(&json!({ "device_class": "tv" })),
            MediaDeviceKind::Tv
        );
        assert_eq!(
            media_kind(&json!({ "device_class": "receiver" })),
            MediaDeviceKind::Receiver
        );
        // No `device_class`, but a running app → TV (e.g. Sony BRAVIA, which
        // doesn't set device_class but reports `app_name`/`app_id`).
        assert_eq!(
            media_kind(&json!({ "app_name": "YouTube" })),
            MediaDeviceKind::Tv
        );
        assert_eq!(
            media_kind(&json!({ "app_id": "com.netflix.ninja" })),
            MediaDeviceKind::Tv
        );
        // Plain media, no app → a speaker.
        assert_eq!(
            media_kind(&json!({ "media_title": "Some Song" })),
            MediaDeviceKind::Speaker
        );
    }

    #[tokio::test]
    async fn set_media_volume_calls_volume_set_with_fraction() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/services/media_player/volume_set"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;

        let cmd = MediaCommand {
            volume: Some(40),
            ..Default::default()
        };
        let p = HaProvider::new_for_test(server.uri()).unwrap();
        MediaProvider::set_state(&p, "media_player.office", &cmd)
            .await
            .unwrap();

        let reqs = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(body["entity_id"], "media_player.office");
        assert!((body["volume_level"].as_f64().unwrap() - 0.4).abs() < 1e-9);
    }

    #[tokio::test]
    async fn play_media_passes_content_through_to_play_media_service() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/services/media_player/play_media"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        MediaProvider::play_media(&p, "media_player.tv", "https://example/stream.m3u8", "url")
            .await
            .unwrap();

        let reqs = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(body["entity_id"], "media_player.tv");
        assert_eq!(body["media_content_id"], "https://example/stream.m3u8");
        assert_eq!(body["media_content_type"], "url");
    }

    #[tokio::test]
    async fn search_and_play_casts_the_top_playable_hit() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/services/media_player/search_media"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "service_response": {
                    "result": [
                        { "media_content_id": "skip", "media_content_type": "app", "can_play": false },
                        { "media_content_id": "show/bobs-burgers", "media_content_type": "tvshow", "can_play": true }
                    ]
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/services/media_player/play_media"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        let played = p
            .search_and_play("media_player.tv", "bobs burgers")
            .await
            .unwrap();
        assert!(played);

        let reqs = server.received_requests().await.unwrap();
        // First call carries the search query; the play call casts the playable hit.
        let search: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(search["search_query"], "bobs burgers");
        let play: Value = serde_json::from_slice(&reqs[1].body).unwrap();
        assert_eq!(play["media_content_id"], "show/bobs-burgers");
        assert_eq!(play["media_content_type"], "tvshow");
    }

    #[tokio::test]
    async fn search_and_play_is_false_when_nothing_matches() {
        let server = MockServer::start().await;
        // Only search is mounted; if play_media were called it would 404 → Err.
        Mock::given(method("POST"))
            .and(path("/api/services/media_player/search_media"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "service_response": { "result": [] } })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        let played = p.search_and_play("media_player.tv", "nonexistent").await;
        assert!(!played.unwrap());
    }

    #[test]
    fn first_search_hit_handles_both_shapes_and_skips_unplayable() {
        // Bare `{ result: [...] }`.
        let bare = json!({ "result": [
            { "media_content_id": "a", "media_content_type": "music", "can_play": true }
        ]});
        assert_eq!(
            first_search_hit(&bare),
            Some(("a".to_string(), "music".to_string()))
        );
        // Per-entity nesting, first item not playable → take the next.
        let nested = json!({ "media_player.tv": { "result": [
            { "media_content_id": "x", "can_play": false },
            { "media_content_id": "y" }
        ]}});
        // `media_content_type` defaults to "video" when absent.
        assert_eq!(
            first_search_hit(&nested),
            Some(("y".to_string(), "video".to_string()))
        );
        // Nothing playable.
        assert_eq!(first_search_hit(&json!({ "result": [] })), None);
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
    async fn discover_generic_maps_climate_and_excludes_native_domains() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/states"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "entity_id": "climate.bedroom", "state": "heat",
                  "attributes": { "temperature": 21, "min_temp": 7, "max_temp": 35,
                                  "hvac_modes": ["off", "heat"], "current_temperature": 19,
                                  "friendly_name": "Bedroom" } },
                { "entity_id": "switch.porch", "state": "on", "attributes": {} }
            ])))
            .mount(&server)
            .await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        let devices = GenericProvider::discover(&p).await.unwrap();
        // The switch belongs to the power domain, not generic.
        assert_eq!(devices.len(), 1);
        let tv = &devices[0];
        assert_eq!(tv.device_id, "climate.bedroom");
        assert_eq!(tv.kind, "climate");
        assert_eq!(tv.name, "Bedroom");
        assert!(
            tv.controls
                .iter()
                .any(|c| matches!(c, Control::Number { key, .. } if key == "temperature"))
        );
        assert!(
            tv.controls
                .iter()
                .any(|c| matches!(c, Control::Enum { key, .. } if key == "hvac_mode"))
        );
    }

    #[tokio::test]
    async fn discover_generic_surfaces_longtail_via_denylist() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/states"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                // A Litter-Robot: HA models it as a vacuum (START|STOP|STATE).
                { "entity_id": "vacuum.litter_robot_4_litter_box", "state": "docked",
                  "attributes": { "supported_features": 12296, "friendly_name": "Litter-Robot 4" } },
                // A domain with no specific mapping still surfaces (state readout).
                { "entity_id": "valve.water_main", "state": "open", "attributes": {} },
                // Read-only noise stays excluded.
                { "entity_id": "sensor.cpu_temp", "state": "55", "attributes": {} },
            ])))
            .mount(&server)
            .await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        let devices = GenericProvider::discover(&p).await.unwrap();
        let ids: Vec<&str> = devices.iter().map(|d| d.device_id.as_str()).collect();
        assert!(ids.contains(&"vacuum.litter_robot_4_litter_box"));
        assert!(ids.contains(&"valve.water_main"));
        assert!(!ids.iter().any(|id| id.starts_with("sensor.")));

        let robot = devices
            .iter()
            .find(|d| d.kind == "vacuum")
            .expect("vacuum surfaced");
        assert!(
            robot
                .controls
                .iter()
                .any(|c| matches!(c, Control::Button { key, .. } if key == "start"))
        );
    }

    #[tokio::test]
    async fn generic_set_control_calls_the_mapped_service() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/services/climate/set_temperature"))
            .and(body_string_contains("climate.bedroom"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        GenericProvider::set_control(&p, "climate.bedroom", "temperature", &json!(21))
            .await
            .unwrap();

        let reqs = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(body["temperature"].as_f64(), Some(21.0));
    }

    fn remote_entity() -> Value {
        json!({
            "entity_id": "remote.bedroom_tv",
            "state": "on",
            "attributes": {
                "friendly_name": "Bedroom TV",
                "current_activity": "com.netflix.ninja",
                "activity_list": [],
                "supported_features": 4
            }
        })
    }

    #[tokio::test]
    async fn discover_remote_maps_entities_with_current_app() {
        let server = MockServer::start().await;
        mount_states(&server, json!([remote_entity(), light_entity()])).await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        let remotes = RemoteProvider::discover(&p).await.unwrap();

        // Only the remote.* entity maps (the light is excluded).
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].provider_id, "remote.bedroom_tv");
        assert_eq!(remotes[0].name, "Bedroom TV");
        assert!(remotes[0].state.on);
        assert_eq!(
            remotes[0].state.current_app.as_deref(),
            Some("com.netflix.ninja")
        );
    }

    #[tokio::test]
    async fn send_key_calls_send_command_with_mapped_keycode() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/services/remote/send_command"))
            .and(body_string_contains("DPAD_CENTER"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        RemoteProvider::send_key(&p, "remote.bedroom_tv", RemoteKey::Select, None)
            .await
            .unwrap();

        let reqs = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(body["entity_id"], "remote.bedroom_tv");
        assert_eq!(body["command"], "DPAD_CENTER");
    }

    #[tokio::test]
    async fn launch_app_calls_turn_on_with_activity() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/services/remote/turn_on"))
            .and(body_string_contains("com.netflix.ninja"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        RemoteProvider::launch_app(&p, "remote.bedroom_tv", "com.netflix.ninja")
            .await
            .unwrap();

        let reqs = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(body["activity"], "com.netflix.ninja");
    }

    #[test]
    fn remote_key_mapping_covers_navigation_and_media() {
        assert_eq!(remote_key_command(RemoteKey::Up), "DPAD_UP");
        assert_eq!(remote_key_command(RemoteKey::Back), "BACK");
        assert_eq!(remote_key_command(RemoteKey::PlayPause), "MEDIA_PLAY_PAUSE");
        assert_eq!(remote_key_command(RemoteKey::Power), "POWER");
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
        assert!(
            HaRemoteFactory
                .build(r#"{"base_url":"http://ha.local:8123","token":"abc"}"#)
                .is_ok()
        );
        assert!(
            HaSensorFactory
                .build(r#"{"base_url":"http://ha.local:8123","token":"abc"}"#)
                .is_ok()
        );
        // `.err()` drops the Ok value (a `Box<dyn MediaProvider>`, which isn't
        // `Debug`) so the error can be unwrapped.
        let err = HaMediaFactory
            .build(r#"{"base_url":"http://ha.local:8123"}"#)
            .err()
            .expect("missing token should fail to build");
        assert!(err.to_string().contains("base_url") || err.to_string().contains("token"));
    }

    #[test]
    fn is_primary_only_for_uncategorised_active_entities() {
        assert!(
            EntityMeta::default().is_primary(),
            "a plain entity is primary"
        );
        let config = EntityMeta {
            entity_category: Some("config".into()),
            ..Default::default()
        };
        assert!(!config.is_primary(), "config sub-controls are not");
        let disabled = EntityMeta {
            disabled_by: Some("user".into()),
            ..Default::default()
        };
        assert!(!disabled.is_primary());
        let hidden = EntityMeta {
            hidden_by: Some("integration".into()),
            ..Default::default()
        };
        assert!(!hidden.is_primary());
    }

    /// A mock HA WebSocket: greets, accepts any auth, and answers
    /// `config/entity_registry/list` with the given entries.
    async fn spawn_mock_ha_ws(entries: Value) -> u16 {
        use futures_util::{SinkExt, StreamExt};
        use tokio::net::TcpListener;
        use tokio_tungstenite::tungstenite::Message;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            ws.send(Message::text(r#"{"type":"auth_required"}"#))
                .await
                .unwrap();
            let _ = ws.next().await; // auth message
            ws.send(Message::text(r#"{"type":"auth_ok"}"#))
                .await
                .unwrap();
            let _ = ws.next().await; // list request
            let result = json!({ "id": 1, "type": "result", "success": true, "result": entries });
            ws.send(Message::text(result.to_string())).await.unwrap();
        });
        port
    }

    #[tokio::test]
    async fn entity_registry_keeps_primary_drops_config_entities() {
        let port = spawn_mock_ha_ws(json!([
            { "entity_id": "switch.real_plug", "entity_category": null },
            { "entity_id": "switch.sonos_crossfade", "entity_category": "config" },
            { "entity_id": "switch.led_indicator", "entity_category": "config" },
            { "entity_id": "media_player.tv", "entity_category": null },
            { "entity_id": "switch.was_disabled", "disabled_by": "user" }
        ]))
        .await;

        let p = HaProvider::new_for_test(format!("http://127.0.0.1:{port}")).unwrap();
        let reg = p.entity_registry().await;

        assert!(keep_entity(&reg, "switch.real_plug"));
        assert!(keep_entity(&reg, "media_player.tv"), "the TV stays primary");
        assert!(
            !keep_entity(&reg, "switch.sonos_crossfade"),
            "config dropped"
        );
        assert!(!keep_entity(&reg, "switch.led_indicator"));
        assert!(!keep_entity(&reg, "switch.was_disabled"));
        // An entity absent from the registry defaults to kept.
        assert!(keep_entity(&reg, "switch.unknown"));
    }

    #[tokio::test]
    async fn entity_registry_unreachable_ws_keeps_everything() {
        // A just-freed port refuses the connection fast (vs. a blackholed one),
        // so the fetch fails quickly and degrades to no filtering.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let p = HaProvider::new_for_test(format!("http://127.0.0.1:{port}")).unwrap();
        let reg = p.entity_registry().await;
        assert!(reg.is_empty());
        assert!(
            keep_entity(&reg, "switch.anything"),
            "no registry → unfiltered"
        );
    }

    #[test]
    fn classify_push_routes_each_domain_and_ignores_others() {
        let light = HaEntity {
            entity_id: "light.kitchen".into(),
            state: "on".into(),
            attributes: json!({ "brightness": 255 }),
            last_changed: None,
        };
        assert!(matches!(
            classify_push(light, "http://ha.local:8123"),
            Some(HaPushEvent::Light { device_id, .. }) if device_id == "light.kitchen"
        ));

        let media = HaEntity {
            entity_id: "media_player.tv".into(),
            state: "playing".into(),
            attributes: json!({ "volume_level": 0.5 }),
            last_changed: None,
        };
        match classify_push(media, "http://ha.local:8123") {
            Some(HaPushEvent::Media(ev)) => {
                assert_eq!(ev.device_id, "media_player.tv");
                assert_eq!(ev.state.volume, 50);
            }
            other => panic!("expected media, got {other:?}"),
        }

        let fan = HaEntity {
            entity_id: "fan.bedroom".into(),
            state: "on".into(),
            attributes: json!({}),
            last_changed: None,
        };
        assert!(matches!(
            classify_push(fan, "http://ha.local:8123"),
            Some(HaPushEvent::Power { state, .. }) if state.on
        ));

        // A classless sensor isn't a modeled kind (see `sensor_kind`) — dropped.
        let sensor = HaEntity {
            entity_id: "sensor.temp".into(),
            state: "21".into(),
            attributes: json!({}),
            last_changed: None,
        };
        assert!(classify_push(sensor, "http://ha.local:8123").is_none());

        // But a device_class'd temperature sensor and a motion binary_sensor route
        // onto the sensor pipeline.
        let temp = HaEntity {
            entity_id: "sensor.hall_temp".into(),
            state: "21.5".into(),
            attributes: json!({ "device_class": "temperature", "unit_of_measurement": "°C" }),
            last_changed: None,
        };
        assert!(matches!(
            classify_push(temp, "http://ha.local:8123"),
            Some(HaPushEvent::Sensor { device_id, state })
                if device_id == "sensor.hall_temp" && state.reading == Some(SensorReading::Number(21.5))
        ));

        let motion = HaEntity {
            entity_id: "binary_sensor.hall_motion".into(),
            state: "on".into(),
            attributes: json!({ "device_class": "motion" }),
            last_changed: None,
        };
        assert!(matches!(
            classify_push(motion, "http://ha.local:8123"),
            Some(HaPushEvent::Sensor { state, .. }) if state.is_detecting()
        ));
    }

    fn sensor_entities() -> Value {
        json!([
            { "entity_id": "binary_sensor.hall_motion", "state": "on",
              "attributes": { "friendly_name": "Hall Motion", "device_class": "motion" } },
            { "entity_id": "binary_sensor.front_door", "state": "off",
              "attributes": { "friendly_name": "Front Door", "device_class": "door" } },
            { "entity_id": "sensor.hall_lux", "state": "480",
              "attributes": { "friendly_name": "Hall Lux", "device_class": "illuminance",
                              "unit_of_measurement": "lx" } },
            { "entity_id": "sensor.hall_temp", "state": "21.5",
              "attributes": { "friendly_name": "Hall Temp", "device_class": "temperature",
                              "unit_of_measurement": "°C" } },
            // The binary_sensor/sensor flood we don't model — must be dropped.
            { "entity_id": "binary_sensor.update_available", "state": "off",
              "attributes": { "device_class": "update" } },
            { "entity_id": "sensor.wifi_signal", "state": "-52",
              "attributes": { "device_class": "signal_strength", "unit_of_measurement": "dBm" } },
            { "entity_id": "sensor.no_class", "state": "hello", "attributes": {} },
            light_entity(),
        ])
    }

    #[tokio::test]
    async fn discover_sensor_allowlists_presence_and_environmental_classes() {
        let server = MockServer::start().await;
        mount_states(&server, sensor_entities()).await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        let devices = SensorProvider::discover(&p).await.unwrap();

        // Four modeled sensors map through; the flood + light are dropped.
        assert_eq!(devices.len(), 4, "got: {devices:?}");
        let by_id = |id: &str| {
            devices
                .iter()
                .find(|d| d.provider_id == id)
                .unwrap_or_else(|| panic!("missing {id}"))
        };
        let motion = by_id("binary_sensor.hall_motion");
        assert_eq!(motion.kind, SensorKind::Motion);
        assert!(motion.state.is_detecting());
        assert_eq!(by_id("binary_sensor.front_door").kind, SensorKind::Contact);
        let lux = by_id("sensor.hall_lux");
        assert_eq!(lux.kind, SensorKind::Illuminance);
        assert_eq!(lux.unit.as_deref(), Some("lx"));
        assert_eq!(lux.state.reading, Some(SensorReading::Number(480.0)));
        assert_eq!(by_id("sensor.hall_temp").kind, SensorKind::Temperature);
    }

    #[test]
    fn sensor_kind_is_a_device_class_allowlist() {
        let mk = |id: &str, dc: &str| sensor_kind(id, &json!({ "device_class": dc }));
        assert_eq!(mk("binary_sensor.x", "motion"), Some(SensorKind::Motion));
        assert_eq!(
            mk("binary_sensor.x", "occupancy"),
            Some(SensorKind::Occupancy)
        );
        assert_eq!(
            mk("binary_sensor.x", "presence"),
            Some(SensorKind::Occupancy)
        );
        assert_eq!(mk("binary_sensor.x", "window"), Some(SensorKind::Contact));
        assert_eq!(mk("sensor.x", "illuminance"), Some(SensorKind::Illuminance));
        assert_eq!(mk("sensor.x", "humidity"), Some(SensorKind::Humidity));
        // Not modeled → skipped.
        assert_eq!(mk("binary_sensor.x", "update"), None);
        assert_eq!(mk("sensor.x", "signal_strength"), None);
        assert_eq!(sensor_kind("sensor.x", &json!({})), None);
        assert_eq!(sensor_kind("light.x", &json!({})), None);
    }

    #[test]
    fn parse_sensor_state_handles_unavailable_and_bad_numbers() {
        let ent = |state: &str| HaEntity {
            entity_id: "sensor.x".into(),
            state: state.into(),
            attributes: json!({}),
            last_changed: None,
        };
        // Unavailable → no reading, unreachable.
        let s = parse_sensor_state(&ent("unavailable"), SensorKind::Temperature);
        assert_eq!(s.reading, None);
        assert_eq!(s.reachable, Some(false));
        // A non-numeric reading for a numeric kind → no reading, still reachable.
        let s = parse_sensor_state(&ent("hello"), SensorKind::Illuminance);
        assert_eq!(s.reading, None);
        assert_eq!(s.reachable, Some(true));
        // Boolean kind maps on/off.
        assert!(parse_sensor_state(&ent("on"), SensorKind::Motion).is_detecting());
        assert!(!parse_sensor_state(&ent("off"), SensorKind::Motion).is_detecting());
    }

    /// A mock HA WebSocket for the push path: greets, accepts auth, acks the
    /// `subscribe_events` request, then emits the given `state_changed` events.
    async fn spawn_mock_ha_push_ws(events: Vec<Value>) -> u16 {
        use futures_util::{SinkExt, StreamExt};
        use tokio::net::TcpListener;
        use tokio_tungstenite::tungstenite::Message;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            ws.send(Message::text(r#"{"type":"auth_required"}"#))
                .await
                .unwrap();
            let _ = ws.next().await; // auth
            ws.send(Message::text(r#"{"type":"auth_ok"}"#))
                .await
                .unwrap();
            let _ = ws.next().await; // subscribe_events
            ws.send(Message::text(
                json!({ "id": 1, "type": "result", "success": true }).to_string(),
            ))
            .await
            .unwrap();
            for new_state in events {
                let frame = json!({
                    "type": "event",
                    "event": { "event_type": "state_changed", "data": { "new_state": new_state } }
                });
                ws.send(Message::text(frame.to_string())).await.unwrap();
            }
            // Hold the socket so the consumer reads everything before it closes.
            tokio::time::sleep(Duration::from_millis(200)).await;
        });
        port
    }

    #[tokio::test]
    async fn push_events_streams_classified_state_changes() {
        let port = spawn_mock_ha_push_ws(vec![
            json!({ "entity_id": "light.kitchen", "state": "on",
                    "attributes": { "brightness": 255 } }),
            json!({ "entity_id": "switch.porch", "state": "on", "attributes": {} }),
            // Removed entity (new_state null) must be skipped, not panic.
            Value::Null,
        ])
        .await;

        let p = HaProvider::new_for_test(format!("http://127.0.0.1:{port}")).unwrap();
        let mut rx = p.push_events().await.unwrap();

        let first = rx.recv().await.expect("a light event");
        assert!(matches!(
            first,
            HaPushEvent::Light { device_id, state } if device_id == "light.kitchen" && state.on
        ));
        let second = rx.recv().await.expect("a power event");
        assert!(matches!(
            second,
            HaPushEvent::Power { device_id, state } if device_id == "switch.porch" && state.on
        ));
    }

    #[tokio::test]
    async fn converse_returns_assist_speech_and_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/conversation/process"))
            .and(body_string_contains("play Bob"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "response": {
                    "speech": { "plain": { "speech": "Playing Bob's Burgers on the TV." } },
                    "response_type": "action_done"
                },
                "conversation_id": null
            })))
            .expect(1)
            .mount(&server)
            .await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        let (speech, ok) = p.converse("play Bob's Burgers on the TV").await.unwrap();
        assert_eq!(speech, "Playing Bob's Burgers on the TV.");
        assert!(ok);
    }

    #[tokio::test]
    async fn converse_reports_error_response_type_as_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/conversation/process"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "response": {
                    "speech": { "plain": { "speech": "Sorry, I couldn't find that." } },
                    "response_type": "error"
                }
            })))
            .mount(&server)
            .await;

        let p = HaProvider::new_for_test(server.uri()).unwrap();
        let (speech, ok) = p.converse("play nonsense").await.unwrap();
        assert_eq!(speech, "Sorry, I couldn't find that.");
        assert!(!ok, "an error response_type is not a success");
    }

    #[tokio::test]
    async fn push_events_errors_when_auth_rejected() {
        use futures_util::{SinkExt, StreamExt};
        use tokio::net::TcpListener;
        use tokio_tungstenite::tungstenite::Message;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            ws.send(Message::text(r#"{"type":"auth_required"}"#))
                .await
                .unwrap();
            let _ = ws.next().await;
            ws.send(Message::text(r#"{"type":"auth_invalid"}"#))
                .await
                .unwrap();
        });

        let p = HaProvider::new_for_test(format!("http://127.0.0.1:{port}")).unwrap();
        let err = p.push_events().await.unwrap_err();
        assert!(err.to_string().contains("auth rejected"), "{err}");
    }
}
