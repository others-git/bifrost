pub mod discovery;
pub mod govee;
pub mod govee_lan;
pub mod ha;
pub mod hue;
pub mod lifx;
pub mod onkyo;
pub mod shelly;
pub mod smarttv;
pub mod sonos;
pub mod tasmota;
pub mod wled;

use discovery::DeviceDiscovery;

use crate::models::generic::{Control, GenericDevice};
use crate::models::media::{MediaCommand, MediaDevice, MediaEvent, MediaFavorite, MediaState};
use crate::models::power::{PowerDevice, PowerState};
use crate::models::remote::{RemoteCommandInfo, RemoteDevice, RemoteKey, RemoteState};
use crate::models::{Light, LightState};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::Serialize;
use std::collections::HashMap;

/// Normalize a raw hardware identity (a MAC-48 or EUI-64, as a provider reports
/// it — `AA:BB:CC:DD:EE:FF`, `00:17:88:01:0b:...`, etc.) into the stable de-dup
/// key Bifrost stores in `*.hw_id`: lowercased hex, separators stripped, with a
/// `mac:` prefix. Returns `None` for anything that isn't a plausible MAC/EUI-64
/// so two devices can never match on garbage. Every provider that populates
/// `hw_id`, and the de-dup reconciler, route through this so the keys are
/// directly comparable.
pub fn mac_hw_id(raw: &str) -> Option<String> {
    let hex: String = raw
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .flat_map(char::to_lowercase)
        .collect();
    // 12 hex = MAC-48 (Govee, Sonos, most Wi-Fi gear); 16 = EUI-64 (Zigbee/Hue).
    (hex.len() == 12 || hex.len() == 16).then(|| format!("mac:{hex}"))
}

/// A process-wide cache of `reqwest::Client`s keyed by an opaque config signature.
///
/// HTTP providers are **rebuilt per request** (`build_provider`/`build_media_provider`
/// decrypt creds and construct a fresh provider on every control). A new `Client`
/// each time opens a new TCP+TLS connection per command — no keep-alive, so every
/// control pays a handshake. A `Client` owns its connection pool and is cheap to
/// clone (`Arc` inside), so caching one per distinct config keeps the pool warm
/// across requests, cutting the handshake off all but the first call.
///
/// `key` must capture everything that distinguishes the client — auth headers
/// (token/app-key), TLS mode, timeouts — so two providers that need different
/// clients never share one. Prefix it per provider (`"hue:"`, `"ha:"`, …) to keep
/// namespaces from colliding. `build` runs only on a cache miss.
pub fn cached_client(
    key: &str,
    build: impl FnOnce() -> Result<reqwest::Client>,
) -> Result<reqwest::Client> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, reqwest::Client>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut map = cache.lock().expect("HTTP client cache poisoned");
    if let Some(client) = map.get(key) {
        return Ok(client.clone());
    }
    let client = build()?;
    map.insert(key.to_string(), client.clone());
    Ok(client)
}

// ── Core provider trait ─────────────────────────────────────────────────────

/// A light group defined inside the provider's own ecosystem (e.g. a Hue
/// room or zone), mirrored locally and linkable from Bifrost Rooms.
#[derive(Debug, Clone)]
pub struct ProviderGroup {
    /// The group's id in the provider's namespace (e.g. Hue room UUID).
    pub provider_group_id: String,
    pub name: String,
    /// Device IDs of member lights, in the provider's namespace
    /// (matches `Light::provider_id` / the `lights.device_id` column).
    pub member_device_ids: Vec<String>,
    /// Native group-control handle (Hue grouped_light rid); None if the
    /// provider has no single-call group control.
    pub grouped_ref: Option<String>,
}

/// Runtime interface every provider must implement.
#[async_trait]
pub trait LightProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn discover(&self) -> Result<Vec<Light>>;
    async fn set_state(&self, device_id: &str, state: &LightState) -> Result<()>;
    async fn get_state(&self, device_id: &str) -> Result<LightState>;

    /// Groups (rooms/zones) defined in the provider's own ecosystem.
    /// Default: none — only providers with a native grouping concept override.
    async fn discover_groups(&self) -> Result<Vec<ProviderGroup>> {
        Ok(vec![])
    }

    /// Apply a state to a whole provider group in one native call, using the
    /// `grouped_ref` from `discover_groups`. Returns Ok(false) when the
    /// provider has no such mechanism (callers then fan out per light).
    async fn set_group_state(&self, _grouped_ref: &str, _state: &LightState) -> Result<bool> {
        Ok(false)
    }

    /// Set individual colour **segments** (e.g. a Govee strip's sections). Only
    /// providers that advertise `LightCapabilities::segments` implement this; the
    /// default errors so a UI that offers it on the wrong device fails loudly.
    /// Write-only — there's no segment readback.
    async fn set_segments(
        &self,
        _device_id: &str,
        _segments: &[crate::models::SegmentColor],
    ) -> Result<()> {
        Err(anyhow!(
            "{} does not support per-segment control",
            self.name()
        ))
    }

    /// Developer-mode diagnostics — raw upstream data, capabilities we *see* vs
    /// the ones we actually *support*, etc. Surfaced via `/api/dev` only when dev
    /// mode is on; never used in a normal deploy. Default: none. Implement to
    /// expose provider internals (e.g. Govee's full per-device capability list,
    /// flagging the ones we don't model yet) for development + contributors.
    async fn debug_info(&self) -> Option<serde_json::Value> {
        None
    }
}

// ── Media provider trait ────────────────────────────────────────────────────

/// Runtime interface for media integrations (receivers, networked speakers).
/// The shape mirrors `LightProvider`: discovery returns full device snapshots,
/// reads return full state, writes are sparse commands.
#[async_trait]
pub trait MediaProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn discover(&self) -> Result<Vec<MediaDevice>>;
    async fn get_state(&self, device_id: &str) -> Result<MediaState>;
    async fn set_state(&self, device_id: &str, cmd: &MediaCommand) -> Result<()>;

    /// Open a persistent push channel: the device reports state changes as
    /// they happen (volume knob, track changes, …). The returned receiver
    /// closing means the connection dropped — the `MediaPushManager` owns
    /// reconnection, never the provider. Only implemented by providers whose
    /// factory advertises `MediaConnectionMode::Push`.
    async fn event_stream(&self) -> Result<tokio::sync::mpsc::Receiver<MediaEvent>> {
        Err(anyhow!("{} does not support push events", self.name()))
    }

    /// List the provider's saved favorites/presets that can be started on this
    /// device (e.g. Sonos Favorites). Default: none — providers without a
    /// favorites concept (or where the protocol can't enumerate them, like
    /// Onkyo eISCP) simply report an empty list.
    async fn list_favorites(&self, _device_id: &str) -> Result<Vec<MediaFavorite>> {
        Ok(Vec::new())
    }

    /// Start playing a favorite (by its provider-native id from
    /// `list_favorites`) on this device. Default: unsupported.
    async fn play_favorite(&self, _device_id: &str, _favorite_id: &str) -> Result<()> {
        Err(anyhow!("{} does not support favorites", self.name()))
    }

    /// **Cast** content to this device — a raw `(content_id, content_type)`
    /// passthrough (the casting seam; richer resolution like app deep-links /
    /// title search is future work). `content_type` is provider-native (HA's
    /// `media_content_type`: `music`/`url`/`app`/`channel`/…). Default:
    /// unsupported. Implemented by HA today (`media_player.play_media`).
    async fn play_media(
        &self,
        _device_id: &str,
        _content_id: &str,
        _content_type: &str,
    ) -> Result<()> {
        Err(anyhow!("{} does not support casting", self.name()))
    }

    /// Join `device_id` into the playback group coordinated by `coordinator_id`
    /// (both provider-native ids) so the two play in sync, controlled through
    /// the coordinator — the provider's own speaker grouping (e.g. Sonos),
    /// independent of Bifrost Rooms. Default: unsupported.
    async fn group(&self, _device_id: &str, _coordinator_id: &str) -> Result<()> {
        Err(anyhow!("{} does not support speaker grouping", self.name()))
    }

    /// Remove `device_id` from any playback group, returning it to standalone
    /// playback. Default: unsupported.
    async fn ungroup(&self, _device_id: &str) -> Result<()> {
        Err(anyhow!("{} does not support speaker grouping", self.name()))
    }

    /// The provider's own rooms/zones (e.g. each Sonos player's room), mirrored
    /// locally and wrapped by Bifrost Rooms — the media analog of
    /// `LightProvider::discover_groups`. `ProviderGroup::member_device_ids` hold
    /// media device ids (matching `MediaDevice::provider_id`). Default: none.
    async fn discover_groups(&self) -> Result<Vec<ProviderGroup>> {
        Ok(vec![])
    }
}

/// Runtime interface for **power devices** — strictly on/off endpoints
/// (switches, plugs, fans, boolean helpers). The shape mirrors `LightProvider`
/// and `MediaProvider`, but the write is just a bool: this domain exists
/// precisely so simple toggles don't carry a light's or a receiver's unused
/// state. Implemented today by the Home Assistant adapter, which surfaces HA's
/// `switch.*` / `fan.*` / `input_boolean.*` entities through it.
#[async_trait]
pub trait PowerProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn discover(&self) -> Result<Vec<PowerDevice>>;
    async fn get_state(&self, device_id: &str) -> Result<PowerState>;
    async fn set_state(&self, device_id: &str, on: bool) -> Result<()>;

    /// Provider-native groups (rooms/areas) containing power devices, mirrored
    /// and wrapped by Bifrost Rooms — the power analog of
    /// `LightProvider::discover_groups`. `member_device_ids` hold power device
    /// ids (matching `PowerDevice::provider_id`). Default: none.
    async fn discover_groups(&self) -> Result<Vec<ProviderGroup>> {
        Ok(vec![])
    }
}

/// How the runtime keeps an media provider's state fresh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaConnectionMode {
    /// Persistent connection with device-initiated updates (Onkyo eISCP).
    Push,
    /// No background channel; state is read on demand.
    OnDemand,
}

/// Factory for one media provider type. Registered in the same
/// `ProviderRegistry` as light factories — the registry stays the single
/// place provider types live; light and media just occupy separate maps.
pub trait MediaProviderFactory: Send + Sync {
    /// The stable string key stored in `providers.provider_type` (e.g. `"onkyo"`).
    fn provider_type(&self) -> &'static str;

    /// Human-facing name for the UI (e.g. `"Sonos"`). Defaults to the type key.
    fn display_name(&self) -> &'static str {
        self.provider_type()
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn MediaProvider>>;

    fn credentials_schema(&self) -> &'static [CredentialField];

    /// Defaults to on-demand reads; push providers override.
    fn connection_mode(&self) -> MediaConnectionMode {
        MediaConnectionMode::OnDemand
    }

    /// Network auto-detect for this provider type. Default: none — providers
    /// with a LAN discovery protocol (Sonos SSDP, Onkyo eISCP, …) override.
    fn discoverer(&self) -> Option<Box<dyn DeviceDiscovery>> {
        None
    }
}

/// Factory for one power provider type. A `provider_type` may register a power
/// factory **in addition to** a light/media one — that's how a single
/// integration (Home Assistant) serves several device domains from one provider
/// row. Power devices are polled like light pollers; there is no push variant
/// yet, so there is no separate connection-mode enum.
pub trait PowerProviderFactory: Send + Sync {
    /// The stable string key stored in `providers.provider_type` (e.g. `"ha"`).
    fn provider_type(&self) -> &'static str;

    /// Human-facing name for the UI. Defaults to the type key.
    fn display_name(&self) -> &'static str {
        self.provider_type()
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn PowerProvider>>;

    fn credentials_schema(&self) -> &'static [CredentialField];
}

/// A virtual remote control for a TV / streamer. Like `PowerProvider`, a
/// `provider_type` may register this **in addition to** its other domains (HA
/// serves lights + power + media + remote from one row). Providers map Bifrost's
/// canonical [`RemoteKey`] to their native command vocabulary.
#[async_trait]
pub trait RemoteProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn discover(&self) -> Result<Vec<RemoteDevice>>;
    async fn get_state(&self, device_id: &str) -> Result<RemoteState>;
    /// Press one canonical key, optionally as a long-press (`hold_secs`).
    async fn send_key(&self, device_id: &str, key: RemoteKey, hold_secs: Option<f32>)
    -> Result<()>;
    /// Type literal text into the focused field. Default: unsupported.
    async fn send_text(&self, _device_id: &str, _text: &str) -> Result<()> {
        anyhow::bail!("this remote does not support text input")
    }
    /// Launch an app by package id or deep-link URL.
    async fn launch_app(&self, device_id: &str, activity: &str) -> Result<()>;
    async fn set_power(&self, device_id: &str, on: bool) -> Result<()>;
    /// The device's **expanded** command catalogue — the keys beyond the canonical
    /// set it exposes (a Bravia's IRCC list, …). Default: none.
    async fn list_commands(&self, _device_id: &str) -> Result<Vec<RemoteCommandInfo>> {
        Ok(Vec::new())
    }
    /// Send a native command by its `token` (from [`Self::list_commands`]).
    /// Default: unsupported.
    async fn send_native(&self, _device_id: &str, _token: &str) -> Result<()> {
        anyhow::bail!("this remote has no expanded commands")
    }
}

/// Factory for one remote provider type (see [`PowerProviderFactory`] for the
/// multi-domain registration pattern).
pub trait RemoteProviderFactory: Send + Sync {
    fn provider_type(&self) -> &'static str;
    fn display_name(&self) -> &'static str {
        self.provider_type()
    }
    fn build(&self, credentials_json: &str) -> Result<Box<dyn RemoteProvider>>;
    fn credentials_schema(&self) -> &'static [CredentialField];
}

/// A **generic "passthrough" device** — the long tail of source device types
/// Bifrost doesn't natively model (climate, cover, lock, `number`, `select`,
/// `button`, …). Each device is just a set of control primitives ([`Control`]),
/// so one mapping + one fuzzy UI covers dozens of HA domains. Registered in
/// addition to a provider's other domains (HA serves it alongside light/power/
/// media/remote). Devices are read live, not persisted.
#[async_trait]
pub trait GenericProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn discover(&self) -> Result<Vec<GenericDevice>>;
    /// Re-read one device's current control primitives.
    async fn get_controls(&self, device_id: &str) -> Result<Vec<Control>>;
    /// Apply a control write — `key` identifies the primitive, `value` is its new
    /// value (bool / number / string; ignored for a button).
    async fn set_control(
        &self,
        device_id: &str,
        key: &str,
        value: &serde_json::Value,
    ) -> Result<()>;
}

/// Factory for one generic provider type (see [`PowerProviderFactory`] for the
/// multi-domain registration pattern).
pub trait GenericProviderFactory: Send + Sync {
    fn provider_type(&self) -> &'static str;
    fn build(&self, credentials_json: &str) -> Result<Box<dyn GenericProvider>>;
}

// ── Credential schema (for the setup UI) ───────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct CredentialField {
    pub name: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    Text,
    Password,
    IpAddress,
    Url,
}

// ── Connection mode ─────────────────────────────────────────────────────────

/// How the runtime keeps a provider's light states fresh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectionMode {
    /// Push events via the Hue CLIP v2 SSE stream, managed by `HueConnectionManager`.
    Sse,
    /// Periodic state polling via `PollingManager`.
    Poll { interval_secs: u64 },
    /// Multi-domain push via Home Assistant's `subscribe_events` WebSocket,
    /// managed by `HaPushManager` — keeps lights, media, and power live from one
    /// connection (replacing the per-domain poll). Built directly from the
    /// concrete `HaProvider` (like `Sse` is from `HueProvider`).
    HaPush,
}

/// Default polling cadence. Conservative enough for cloud APIs with daily
/// rate limits (Govee: 10 000 req/day).
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 120;

// ── Factory trait ───────────────────────────────────────────────────────────

/// A factory knows how to construct one type of provider from its credentials.
/// Implement this — not `LightProvider` directly — when adding a new integration.
pub trait ProviderFactory: Send + Sync {
    /// The stable string key stored in the database (e.g. `"hue"`, `"govee"`).
    fn provider_type(&self) -> &'static str;

    /// Human-facing name for the UI (e.g. `"Philips Hue"`). Defaults to the type key.
    fn display_name(&self) -> &'static str {
        self.provider_type()
    }

    /// Build a live provider from already-decrypted credentials JSON.
    fn build(&self, credentials_json: &str) -> Result<Box<dyn LightProvider>>;

    /// Describe the credential fields the UI must collect before calling `add_provider`.
    fn credentials_schema(&self) -> &'static [CredentialField];

    /// How the runtime should keep this provider's state fresh. Defaults to polling;
    /// only providers with a push channel need to override.
    fn connection_mode(&self) -> ConnectionMode {
        ConnectionMode::Poll {
            interval_secs: DEFAULT_POLL_INTERVAL_SECS,
        }
    }

    /// How this type is grouped in the "Add provider" UI. Defaults to `Light`
    /// (this is a `ProviderFactory`); a platform adapter that surfaces many
    /// device kinds (e.g. Home Assistant) overrides to `Integration`.
    fn domain(&self) -> ProviderDomain {
        ProviderDomain::Light
    }

    /// Network auto-detect for this provider type. Default: none — providers
    /// with a LAN discovery protocol override.
    fn discoverer(&self) -> Option<Box<dyn DeviceDiscovery>> {
        None
    }
}

// ── Registry ────────────────────────────────────────────────────────────────

/// Central registry. Add new providers once here; the rest of the app needs no changes.
/// Light and media factories live in separate maps under the same roof — a
/// provider type string belongs to exactly one of the two domains.
pub struct ProviderRegistry {
    factories: HashMap<&'static str, Box<dyn ProviderFactory>>,
    media_factories: HashMap<&'static str, Box<dyn MediaProviderFactory>>,
    power_factories: HashMap<&'static str, Box<dyn PowerProviderFactory>>,
    remote_factories: HashMap<&'static str, Box<dyn RemoteProviderFactory>>,
    generic_factories: HashMap<&'static str, Box<dyn GenericProviderFactory>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
            media_factories: HashMap::new(),
            power_factories: HashMap::new(),
            remote_factories: HashMap::new(),
            generic_factories: HashMap::new(),
        }
    }

    pub fn register<F: ProviderFactory + 'static>(&mut self, factory: F) {
        self.factories
            .insert(factory.provider_type(), Box::new(factory));
    }

    pub fn register_media<F: MediaProviderFactory + 'static>(&mut self, factory: F) {
        self.media_factories
            .insert(factory.provider_type(), Box::new(factory));
    }

    /// Register the power domain for a provider type. A type registered here as
    /// well as in `register` (light) is served across both domains from one
    /// provider row — the multi-domain integration case (Home Assistant).
    pub fn register_power<F: PowerProviderFactory + 'static>(&mut self, factory: F) {
        self.power_factories
            .insert(factory.provider_type(), Box::new(factory));
    }

    /// Build a live power provider from a type string + decrypted credentials JSON.
    pub fn build_power(
        &self,
        provider_type: &str,
        credentials_json: &str,
    ) -> Result<Box<dyn PowerProvider>> {
        self.power_factories
            .get(provider_type)
            .ok_or_else(|| anyhow!("unknown power provider type: {provider_type}"))?
            .build(credentials_json)
    }

    /// Returns true if `provider_type` serves the power domain.
    pub fn is_known_power(&self, provider_type: &str) -> bool {
        self.power_factories.contains_key(provider_type)
    }

    /// Register the remote domain for a provider type (multi-domain, like
    /// `register_power`).
    pub fn register_remote<F: RemoteProviderFactory + 'static>(&mut self, factory: F) {
        self.remote_factories
            .insert(factory.provider_type(), Box::new(factory));
    }

    /// Build a live remote provider from a type string + decrypted credentials JSON.
    pub fn build_remote(
        &self,
        provider_type: &str,
        credentials_json: &str,
    ) -> Result<Box<dyn RemoteProvider>> {
        self.remote_factories
            .get(provider_type)
            .ok_or_else(|| anyhow!("unknown remote provider type: {provider_type}"))?
            .build(credentials_json)
    }

    pub fn register_generic<F: GenericProviderFactory + 'static>(&mut self, factory: F) {
        self.generic_factories
            .insert(factory.provider_type(), Box::new(factory));
    }
    pub fn build_generic(
        &self,
        provider_type: &str,
        credentials_json: &str,
    ) -> Result<Box<dyn GenericProvider>> {
        self.generic_factories
            .get(provider_type)
            .ok_or_else(|| anyhow!("unknown generic provider type: {provider_type}"))?
            .build(credentials_json)
    }
    /// Whether this provider type serves generic (passthrough) devices.
    pub fn has_generic(&self, provider_type: &str) -> bool {
        self.generic_factories.contains_key(provider_type)
    }

    /// Returns true if `provider_type` serves the remote domain.
    pub fn is_known_remote(&self, provider_type: &str) -> bool {
        self.remote_factories.contains_key(provider_type)
    }

    /// Build a live media provider from a type string + decrypted credentials JSON.
    pub fn build_media(
        &self,
        provider_type: &str,
        credentials_json: &str,
    ) -> Result<Box<dyn MediaProvider>> {
        self.media_factories
            .get(provider_type)
            .ok_or_else(|| anyhow!("unknown media provider type: {provider_type}"))?
            .build(credentials_json)
    }

    /// Returns true if `provider_type` is a registered media provider.
    pub fn is_known_media(&self, provider_type: &str) -> bool {
        self.media_factories.contains_key(provider_type)
    }

    /// Human-facing name for a provider type (light, media, or power), if
    /// registered. A multi-domain type (HA) resolves via its light factory.
    pub fn display_name(&self, provider_type: &str) -> Option<&'static str> {
        if let Some(f) = self.factories.get(provider_type) {
            return Some(f.display_name());
        }
        if let Some(f) = self.media_factories.get(provider_type) {
            return Some(f.display_name());
        }
        self.power_factories
            .get(provider_type)
            .map(|f| f.display_name())
    }

    /// The network auto-detect object for a provider type, if it has one.
    /// Looks in both the light and media factory maps.
    pub fn discoverer(&self, provider_type: &str) -> Option<Box<dyn DeviceDiscovery>> {
        if let Some(f) = self.factories.get(provider_type) {
            return f.discoverer();
        }
        self.media_factories
            .get(provider_type)
            .and_then(|f| f.discoverer())
    }

    /// How the runtime should keep this media provider type's state fresh.
    pub fn media_connection_mode(&self, provider_type: &str) -> Option<MediaConnectionMode> {
        self.media_factories
            .get(provider_type)
            .map(|f| f.connection_mode())
    }

    /// Build a live provider from a type string + decrypted credentials JSON.
    pub fn build(
        &self,
        provider_type: &str,
        credentials_json: &str,
    ) -> Result<Box<dyn LightProvider>> {
        self.factories
            .get(provider_type)
            .ok_or_else(|| anyhow!("unknown provider type: {provider_type}"))?
            .build(credentials_json)
    }

    /// Returns true if `provider_type` is registered.
    pub fn is_known(&self, provider_type: &str) -> bool {
        self.factories.contains_key(provider_type)
    }

    /// The credential schema for a single provider type (light or media), if
    /// registered. Lets the API tell which fields are secret when prefilling
    /// the edit form.
    pub fn schema(&self, provider_type: &str) -> Option<&'static [CredentialField]> {
        if let Some(f) = self.factories.get(provider_type) {
            return Some(f.credentials_schema());
        }
        if let Some(f) = self.media_factories.get(provider_type) {
            return Some(f.credentials_schema());
        }
        self.power_factories
            .get(provider_type)
            .map(|f| f.credentials_schema())
    }

    /// How the runtime should keep this provider type's state fresh.
    pub fn connection_mode(&self, provider_type: &str) -> Option<ConnectionMode> {
        self.factories
            .get(provider_type)
            .map(|f| f.connection_mode())
    }

    /// The UI category for a configured provider type — the same grouping shown
    /// in the add-provider menu. A light/integration factory's `domain()` wins
    /// (so a multi-domain type like HA reads as `Integration`, not `Media`);
    /// otherwise an media-only type is `Media`.
    pub fn ui_domain(&self, provider_type: &str) -> Option<ProviderDomain> {
        if let Some(f) = self.factories.get(provider_type) {
            return Some(f.domain());
        }
        if self.media_factories.contains_key(provider_type) {
            return Some(ProviderDomain::Media);
        }
        self.power_factories
            .get(provider_type)
            .map(|_| ProviderDomain::Light)
    }

    /// All registered provider types for the "Add provider" UI, **one entry per
    /// type**, sorted by type name. A multi-domain integration (HA) registers in
    /// several maps but must appear once — its light/integration factory is
    /// authoritative for the menu, so an media factory sharing that type key is
    /// skipped here (it still serves discovery/control behind the scenes).
    pub fn all_types(&self) -> Vec<ProviderTypeInfo> {
        let mut types: Vec<_> = self
            .factories
            .values()
            .map(|f| ProviderTypeInfo {
                provider_type: f.provider_type(),
                display_name: f.display_name(),
                kind: f.domain(),
                supports_discovery: f.discoverer().is_some(),
                schema: f.credentials_schema().to_vec(),
            })
            .chain(
                self.media_factories
                    .values()
                    .filter(|f| !self.factories.contains_key(f.provider_type()))
                    .map(|f| ProviderTypeInfo {
                        provider_type: f.provider_type(),
                        display_name: f.display_name(),
                        kind: ProviderDomain::Media,
                        supports_discovery: f.discoverer().is_some(),
                        schema: f.credentials_schema().to_vec(),
                    }),
            )
            .collect();
        types.sort_by_key(|t| t.provider_type);
        types
    }
}

/// How a provider type is grouped in the "Add provider" UI. Most providers map
/// to a single device domain (`Light`/`Media`); an `Integration` is a
/// higher-level platform adapter (e.g. Home Assistant) that can surface many
/// device kinds at once, so it gets its own category rather than being filed
/// under one domain.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderDomain {
    Light,
    Media,
    Integration,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Serialize)]
pub struct ProviderTypeInfo {
    pub provider_type: &'static str,
    /// Human-facing name for the UI.
    pub display_name: &'static str,
    pub kind: ProviderDomain,
    /// Whether the UI should offer a "scan network" button for this type.
    pub supports_discovery: bool,
    pub schema: Vec<CredentialField>,
}

// ── Built-in provider registration ─────────────────────────────────────────

/// Returns a registry pre-loaded with every built-in provider.
/// Call this once in main; pass the result into AppState.
pub fn default_registry() -> ProviderRegistry {
    let mut r = ProviderRegistry::new();
    // First-class providers. WLED / Tasmota / Shelly / Govee-LAN are kept in the
    // tree but intentionally **not registered** (dropped from the add-provider
    // menu); re-add a `register(...)` line to bring one back. `wled` is still
    // registered in test fixtures as the generic mockable light provider.
    r.register(hue::HueProviderFactory);
    r.register(govee::GoveeProviderFactory);
    r.register(lifx::LifxProviderFactory);
    // Home Assistant serves multiple device domains from one provider row:
    // lights, power (switch/fan/plug), and media (media_player — TVs, speakers).
    // It appears once in the add-provider menu as an "Integration" (its light
    // factory's domain); the power/media factories serve discovery + control.
    r.register(ha::HaLightFactory);
    r.register_power(ha::HaPowerFactory);
    r.register_media(ha::HaMediaFactory);
    r.register_remote(ha::HaRemoteFactory);
    // The generic "passthrough" surface for HA's controllable long tail (climate,
    // cover, lock, number, select, button, …) — one mapping, many device types.
    r.register_generic(ha::HaGenericFactory);
    r.register_media(onkyo::OnkyoProviderFactory);
    r.register_media(sonos::SonosProviderFactory);
    // Smart TVs serve two domains from one provider row (media + remote), like HA:
    // a vendor-agnostic framework (`smarttv`) with per-brand adapters (Bravia
    // today). The IP is auto-discovered; the vendor is auto-selected.
    r.register_media(smarttv::SmartTvMediaFactory);
    r.register_remote(smarttv::SmartTvRemoteFactory);
    r
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::models::{Light, LightState};
    use anyhow::Result;

    #[test]
    fn mac_hw_id_normalizes_mac48_and_eui64() {
        // MAC-48 with colons → lowercased, separators stripped, prefixed.
        assert_eq!(
            mac_hw_id("AA:BB:CC:DD:EE:FF"),
            Some("mac:aabbccddeeff".to_string())
        );
        // Hyphen-separated, mixed case — same canonical key.
        assert_eq!(
            mac_hw_id("aa-bb-cc-dd-ee-ff"),
            Some("mac:aabbccddeeff".to_string())
        );
        // EUI-64 (Zigbee/Hue) is 16 hex digits and also accepted.
        assert_eq!(
            mac_hw_id("00:17:88:01:0b:12:34:56"),
            Some("mac:001788010b123456".to_string())
        );
    }

    #[test]
    fn mac_hw_id_rejects_non_mac() {
        assert_eq!(mac_hw_id(""), None);
        assert_eq!(mac_hw_id("not-a-mac"), None);
        assert_eq!(mac_hw_id("AA:BB:CC"), None); // too short
        assert_eq!(mac_hw_id("light.kitchen"), None); // an HA entity id, not hardware
    }

    // ── Minimal mock provider for registry tests ────────────────────────────

    struct MockProvider;

    #[async_trait::async_trait]
    impl LightProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }
        async fn discover(&self) -> Result<Vec<Light>> {
            Ok(vec![])
        }
        async fn set_state(&self, _id: &str, _s: &LightState) -> Result<()> {
            Ok(())
        }
        async fn get_state(&self, _id: &str) -> Result<LightState> {
            Ok(LightState::default())
        }
    }

    pub struct MockProviderFactory;

    impl ProviderFactory for MockProviderFactory {
        fn provider_type(&self) -> &'static str {
            "mock"
        }

        fn build(&self, _credentials_json: &str) -> Result<Box<dyn LightProvider>> {
            Ok(Box::new(MockProvider))
        }

        fn credentials_schema(&self) -> &'static [CredentialField] {
            &[CredentialField {
                name: "token",
                label: "Token",
                kind: FieldKind::Password,
                required: true,
                hint: None,
            }]
        }
    }

    // ── Registry tests ──────────────────────────────────────────────────────

    #[test]
    fn register_and_build_known_type() {
        let mut reg = ProviderRegistry::new();
        reg.register(MockProviderFactory);
        assert!(reg.build("mock", "{}").is_ok());
    }

    #[test]
    fn build_unknown_type_returns_error() {
        let reg = ProviderRegistry::new();
        let err = reg
            .build("nonexistent", "{}")
            .err()
            .expect("expected an error");
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn is_known_reflects_registered_types() {
        let mut reg = ProviderRegistry::new();
        assert!(!reg.is_known("mock"));
        reg.register(MockProviderFactory);
        assert!(reg.is_known("mock"));
    }

    #[test]
    fn all_types_sorted_alphabetically() {
        let reg = default_registry();
        let types = reg.all_types();
        let names: Vec<_> = types.iter().map(|t| t.provider_type).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }

    #[test]
    fn default_registry_contains_hue_and_govee() {
        let reg = default_registry();
        assert!(reg.is_known("hue"));
        assert!(reg.is_known("govee"));
    }

    #[test]
    fn default_registry_drops_secondary_light_providers() {
        // First-class only: WLED / Tasmota / Shelly / Govee-LAN are kept in the
        // tree but unregistered (re-add a `register(...)` line to restore).
        let reg = default_registry();
        for t in ["wled", "tasmota", "shelly", "govee-lan"] {
            assert!(!reg.is_known(t), "{t} should be unregistered");
        }
    }

    #[test]
    fn default_registry_serves_ha_across_all_device_domains() {
        let reg = default_registry();
        let creds = r#"{"base_url":"http://ha.local:8123","token":"t"}"#;
        // One "ha" provider row serves lights, power, and media (media_player).
        assert!(reg.is_known("ha"), "light domain");
        assert!(reg.is_known_power("ha"), "power domain");
        assert!(reg.is_known_media("ha"), "media (media_player) domain");
        assert!(reg.build("ha", creds).is_ok());
        assert!(reg.build_power("ha", creds).is_ok());
        assert!(reg.build_media("ha", creds).is_ok());
    }

    #[test]
    fn ha_appears_once_in_the_add_menu_despite_multiple_domains() {
        let reg = default_registry();
        let ha: Vec<_> = reg
            .all_types()
            .into_iter()
            .filter(|t| t.provider_type == "ha")
            .collect();
        assert_eq!(ha.len(), 1, "HA must not be listed once per domain");
        // …and it's categorised as an Integration, not Media.
        assert_eq!(ha[0].kind, ProviderDomain::Integration);
    }

    #[test]
    fn ha_uses_websocket_push_not_polling() {
        let reg = default_registry();
        assert_eq!(
            reg.connection_mode("ha"),
            Some(ConnectionMode::HaPush),
            "HA keeps one subscribe_events WebSocket live instead of polling"
        );
    }

    #[test]
    fn hue_uses_sse_mode_all_others_poll() {
        let reg = default_registry();
        assert_eq!(reg.connection_mode("hue"), Some(ConnectionMode::Sse));
        for t in ["govee"] {
            match reg.connection_mode(t) {
                Some(ConnectionMode::Poll { interval_secs }) => {
                    assert!(interval_secs > 0, "{t}: zero poll interval")
                }
                other => panic!("{t}: expected Poll mode, got {other:?}"),
            }
        }
    }

    #[test]
    fn connection_mode_for_unknown_type_is_none() {
        let reg = ProviderRegistry::new();
        assert!(reg.connection_mode("nonexistent").is_none());
    }

    #[test]
    fn default_registry_contains_onkyo_and_sonos_as_media() {
        let reg = default_registry();
        assert!(reg.is_known_media("onkyo"));
        assert!(reg.is_known_media("sonos"));
        // Media types are not light types and vice versa.
        assert!(!reg.is_known("onkyo"));
        assert!(!reg.is_known_media("hue"));
        // Builds without touching the network.
        assert!(reg.build_media("onkyo", r#"{"host":"10.0.0.9"}"#).is_ok());
        assert!(reg.build_media("sonos", r#"{"host":"10.0.0.9"}"#).is_ok());
    }

    #[test]
    fn build_media_unknown_type_returns_error() {
        let reg = ProviderRegistry::new();
        let err = reg
            .build_media("nonexistent", "{}")
            .err()
            .expect("expected an error");
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn all_types_tags_light_and_media_domains() {
        let reg = default_registry();
        let types = reg.all_types();
        let kind_of = |t: &str| {
            types
                .iter()
                .find(|i| i.provider_type == t)
                .map(|i| i.kind)
                .unwrap()
        };
        assert_eq!(kind_of("hue"), ProviderDomain::Light);
        assert_eq!(kind_of("onkyo"), ProviderDomain::Media);
        assert_eq!(kind_of("sonos"), ProviderDomain::Media);
        // HA is a platform adapter, surfaced under its own "Integration" group
        // rather than filed under Lights despite being a light-domain provider.
        assert_eq!(kind_of("ha"), ProviderDomain::Integration);
    }

    #[test]
    fn credentials_schema_describes_required_fields() {
        let reg = default_registry();
        let types = reg.all_types();
        for t in &types {
            assert!(
                !t.schema.is_empty(),
                "provider '{}' has no credential fields",
                t.provider_type
            );
            // Govee and LIFX are "at least one of N" providers — a cloud
            // credential (key/token) and/or a LAN interface, neither individually
            // required; their `build` enforces that one is present. Every other
            // provider has a hard field.
            if t.provider_type == "govee" || t.provider_type == "lifx" {
                continue;
            }
            assert!(
                t.schema.iter().any(|f| f.required),
                "provider '{}' has no required fields",
                t.provider_type
            );
        }
    }
}
