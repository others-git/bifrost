//! BifrostSmartTv — a vendor-agnostic smart-TV integration framework.
//!
//! A smart TV is *both* a media device (power, volume, source/app, transport)
//! and a remote (D-pad, nav/media keys, app launch), so one physical TV surfaces
//! through **both** [`MediaProvider`] and [`RemoteProvider`] from a single
//! provider row (`smarttv`) — the same multi-domain pattern Home Assistant uses.
//! All the generic glue lives here; the brand-specific protocol lives behind the
//! [`SmartTvVendor`] seam.
//!
//! ## Adding a vendor
//!
//! 1. Create `src/providers/smarttv/<vendor>.rs`.
//! 2. Implement [`SmartTvVendor`] for it (and a pairing/auth flow if the TV needs
//!    one — see [`bravia::pairing`]).
//! 3. Add a match arm to [`build_vendor`] keyed on the stored `brand`.
//! 4. If it answers a distinct discovery signature, add an [`SsdpDiscovery`] for
//!    it that pre-fills `brand`.
//!
//! Nothing else in the framework changes — the two trait impls, the Bifrost
//! device shapes, and the command routing are all vendor-neutral.

mod bravia;

use crate::models::media::{
    MediaCapabilities, MediaCommand, MediaDevice, MediaDeviceKind, MediaState, NowPlaying,
    TransportCmd,
};
use crate::models::remote::{RemoteDevice, RemoteKey, RemoteState};
use crate::providers::{
    CredentialField, FieldKind, MediaProvider, MediaProviderFactory, RemoteProvider,
    RemoteProviderFactory,
    discovery::{DeviceDiscovery, SsdpDiscovery},
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use uuid::Uuid;

// ── The vendor seam ──────────────────────────────────────────────────────────

/// Identity of a TV, resolved from the device (model/nickname + hardware id).
pub(crate) struct TvIdentity {
    pub name: String,
    /// Normalized `mac:…` id — shared by this TV's media + remote rows so they
    /// auto-pair, and used for cross-provider de-dup against an HA copy.
    pub hw_id: Option<String>,
}

/// A vendor-neutral snapshot of a TV's live state. The framework maps it into a
/// [`MediaState`] (the media domain) and a [`RemoteState`] (the remote domain).
pub(crate) struct TvSnapshot {
    pub reachable: bool,
    pub power: bool,
    pub volume: u8,
    pub mute: bool,
    pub source: Option<String>,
    pub sources: Vec<String>,
    pub current_app: Option<String>,
    pub now_playing: Option<NowPlaying>,
}

/// One smart-TV brand's protocol — the *only* thing a new vendor implements.
/// Constructed cheaply (host + auth, no network); all I/O is in the async methods.
#[async_trait]
pub(crate) trait SmartTvVendor: Send + Sync {
    /// Brand label for the UI / logs (e.g. `"Sony Bravia"`).
    fn brand(&self) -> &'static str;
    /// Resolve the TV's name + hardware id (one network round-trip).
    async fn identity(&self) -> Result<TvIdentity>;
    /// Read the TV's full live state.
    async fn snapshot(&self) -> Result<TvSnapshot>;
    async fn set_power(&self, on: bool) -> Result<()>;
    async fn set_volume(&self, percent: u8) -> Result<()>;
    async fn set_mute(&self, mute: bool) -> Result<()>;
    /// Select an input/source (or launch an app) by name as reported in
    /// `snapshot().sources`.
    async fn set_source(&self, source: &str) -> Result<()>;
    /// Press one canonical remote key.
    async fn send_key(&self, key: RemoteKey) -> Result<()>;
    /// Launch an app by its vendor-native id (package / uri / deep link).
    async fn launch_app(&self, app: &str) -> Result<()>;
}

// ── Credentials + vendor dispatch ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SmartTvCreds {
    /// The TV's IP / host.
    host: String,
    /// Which vendor adapter to use; defaults to the only one today (Bravia).
    /// Discovery stamps this so the user never picks it.
    #[serde(default)]
    brand: Option<String>,
    /// Vendor auth token (Bravia: the PIN-pairing cookie). Absent before pairing.
    #[serde(default)]
    auth: Option<String>,
}

/// Build the vendor adapter named by the stored `brand` (default: Bravia, the
/// only vendor today). This is the single dispatch point new vendors extend.
fn build_vendor(creds: &SmartTvCreds) -> Result<Box<dyn SmartTvVendor>> {
    match creds.brand.as_deref() {
        Some("bravia") | None => Ok(Box::new(bravia::BraviaVendor::new(
            &creds.host,
            creds.auth.clone(),
        )?)),
        Some(other) => Err(anyhow!("unknown smart-TV brand '{other}'")),
    }
}

fn parse_creds(credentials_json: &str) -> Result<SmartTvCreds> {
    serde_json::from_str(credentials_json).map_err(|e| anyhow!("invalid smart-TV credentials: {e}"))
}

// ── Pairing (brand-agnostic, for the add-provider flow) ──────────────────────

/// Outcome of a pairing step. A TV that needs no pairing returns `Paired`
/// immediately; one that uses a PIN (Bravia) returns `PinDisplayed` first.
pub enum SmartTvPairOutcome {
    /// The TV is now showing a PIN; call [`pair_complete`] with it.
    PinDisplayed,
    /// Paired — `auth` is the credential to store alongside `host`.
    Paired { auth: String },
}

/// Begin pairing with the TV at `host` (vendor auto-selected — Bravia today). The
/// TV pops a PIN unless it's already authorised.
pub async fn pair_begin(host: &str) -> Result<SmartTvPairOutcome> {
    match bravia::pairing::begin(host).await? {
        bravia::pairing::PairOutcome::PinDisplayed => Ok(SmartTvPairOutcome::PinDisplayed),
        bravia::pairing::PairOutcome::Paired(auth) => Ok(SmartTvPairOutcome::Paired { auth }),
    }
}

/// Finish pairing with the on-screen PIN; returns the `auth` credential to store.
pub async fn pair_complete(host: &str, pin: &str) -> Result<String> {
    bravia::pairing::complete(host, pin).await
}

/// The credential fields the add-provider form collects. `host` is pre-filled by
/// discovery; `auth` is produced by the vendor's pairing flow (PIN for Bravia).
const SMARTTV_SCHEMA: &[CredentialField] = &[
    CredentialField {
        name: "host",
        label: "TV IP address",
        kind: FieldKind::IpAddress,
        required: true,
        hint: Some("Found automatically by the network scan."),
    },
    CredentialField {
        name: "auth",
        label: "Pairing token",
        kind: FieldKind::Password,
        required: false,
        hint: Some("Obtained by pairing — enter the PIN shown on the TV."),
    },
];

/// SSDP search target Sony's ScalarWeb (Bravia) service answers.
const SONY_SCALARWEB_ST: &str = "urn:schemas-sony-com:service:ScalarWebAPI:1";

fn smarttv_discoverer() -> Box<dyn DeviceDiscovery> {
    // Sony TVs answer the ScalarWeb ST and carry "sony" in the reply; the IP from
    // the LOCATION header pre-fills `host`. Only Bravia today, so no brand stamp
    // is needed (the dispatch defaults to it).
    Box::new(SsdpDiscovery::new(
        SONY_SCALARWEB_ST,
        "sony",
        "Sony Bravia (Smart TV)",
        "host",
    ))
}

// ── The framework adapter: one TV, two domains ───────────────────────────────

/// A connected smart TV: holds the resolved vendor and implements both the media
/// and remote provider traits over it. Each factory builds its own instance
/// (cheap — host + auth + an HTTP client).
struct SmartTv {
    vendor: Box<dyn SmartTvVendor>,
    /// Stable provider-native id for the single device this provider serves.
    device_id: String,
}

impl SmartTv {
    fn from_creds(credentials_json: &str) -> Result<Self> {
        let creds = parse_creds(credentials_json)?;
        Ok(Self {
            device_id: creds.host.clone(),
            vendor: build_vendor(&creds)?,
        })
    }
}

fn tv_media_state(s: &TvSnapshot) -> MediaState {
    MediaState {
        power: s.power,
        volume: s.volume,
        mute: s.mute,
        source: s.source.clone(),
        source_list: s.sources.clone(),
        now_playing: s.now_playing.clone(),
        reachable: Some(s.reachable),
        group_coordinator: None,
    }
}

const TV_CAPS: MediaCapabilities = MediaCapabilities {
    sources: true,
    transport: true,
    now_playing: true,
    favorites: false,
    grouping: false,
};

#[async_trait]
impl MediaProvider for SmartTv {
    fn name(&self) -> &str {
        self.vendor.brand()
    }

    async fn discover(&self) -> Result<Vec<MediaDevice>> {
        let id = self.vendor.identity().await?;
        tracing::debug!(target: "bifrost::smarttv", brand = self.vendor.brand(), name = %id.name, hw_id = ?id.hw_id, "discovered TV (media domain)");
        Ok(vec![MediaDevice {
            id: Uuid::new_v4(),
            provider_id: self.device_id.clone(),
            name: id.name,
            kind: MediaDeviceKind::Tv,
            capabilities: TV_CAPS,
            state: MediaState::default(),
            hw_id: id.hw_id,
        }])
    }

    async fn get_state(&self, _device_id: &str) -> Result<MediaState> {
        Ok(tv_media_state(&self.vendor.snapshot().await?))
    }

    async fn set_state(&self, _device_id: &str, cmd: &MediaCommand) -> Result<()> {
        tracing::debug!(target: "bifrost::smarttv", brand = self.vendor.brand(), ?cmd, "set_state");
        // Power first, so "power on + source" works from standby.
        if let Some(on) = cmd.power {
            self.vendor.set_power(on).await?;
        }
        if let Some(v) = cmd.volume {
            self.vendor.set_volume(v).await?;
        }
        if let Some(m) = cmd.mute {
            self.vendor.set_mute(m).await?;
        }
        if let Some(src) = &cmd.source {
            self.vendor.set_source(src).await?;
        }
        if let Some(t) = cmd.transport {
            // A TV has no transport API; drive it through the equivalent remote
            // key (Next/Previous map straight across; the rest fold to play/pause).
            let key = match t {
                TransportCmd::Next => RemoteKey::Next,
                TransportCmd::Previous => RemoteKey::Previous,
                TransportCmd::Play
                | TransportCmd::Pause
                | TransportCmd::Toggle
                | TransportCmd::Stop => RemoteKey::PlayPause,
            };
            self.vendor.send_key(key).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl RemoteProvider for SmartTv {
    fn name(&self) -> &str {
        self.vendor.brand()
    }

    async fn discover(&self) -> Result<Vec<RemoteDevice>> {
        let id = self.vendor.identity().await?;
        tracing::debug!(target: "bifrost::smarttv", brand = self.vendor.brand(), name = %id.name, hw_id = ?id.hw_id, "discovered TV (remote domain)");
        Ok(vec![RemoteDevice {
            id: Uuid::new_v4(),
            provider_id: self.device_id.clone(),
            name: id.name,
            state: RemoteState::default(),
            hw_id: id.hw_id,
        }])
    }

    async fn get_state(&self, _device_id: &str) -> Result<RemoteState> {
        let s = self.vendor.snapshot().await?;
        Ok(RemoteState {
            on: s.power,
            current_app: s.current_app,
            reachable: Some(s.reachable),
        })
    }

    async fn send_key(
        &self,
        _device_id: &str,
        key: RemoteKey,
        _hold_secs: Option<f32>,
    ) -> Result<()> {
        tracing::debug!(target: "bifrost::smarttv", brand = self.vendor.brand(), ?key, "remote key");
        self.vendor.send_key(key).await
    }

    async fn launch_app(&self, _device_id: &str, activity: &str) -> Result<()> {
        tracing::debug!(target: "bifrost::smarttv", brand = self.vendor.brand(), activity, "launch app");
        self.vendor.launch_app(activity).await
    }

    async fn set_power(&self, _device_id: &str, on: bool) -> Result<()> {
        self.vendor.set_power(on).await
    }
}

// ── Factories (one provider type, registered across two domains) ─────────────

pub struct SmartTvMediaFactory;

impl MediaProviderFactory for SmartTvMediaFactory {
    fn provider_type(&self) -> &'static str {
        "smarttv"
    }
    fn display_name(&self) -> &'static str {
        "Smart TV"
    }
    fn build(&self, credentials_json: &str) -> Result<Box<dyn MediaProvider>> {
        Ok(Box::new(SmartTv::from_creds(credentials_json)?))
    }
    fn credentials_schema(&self) -> &'static [CredentialField] {
        SMARTTV_SCHEMA
    }
    fn discoverer(&self) -> Option<Box<dyn DeviceDiscovery>> {
        Some(smarttv_discoverer())
    }
}

pub struct SmartTvRemoteFactory;

impl RemoteProviderFactory for SmartTvRemoteFactory {
    fn provider_type(&self) -> &'static str {
        "smarttv"
    }
    fn display_name(&self) -> &'static str {
        "Smart TV"
    }
    fn build(&self, credentials_json: &str) -> Result<Box<dyn RemoteProvider>> {
        Ok(Box::new(SmartTv::from_creds(credentials_json)?))
    }
    fn credentials_schema(&self) -> &'static [CredentialField] {
        SMARTTV_SCHEMA
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_vendor_defaults_to_bravia() {
        let v = build_vendor(&SmartTvCreds {
            host: "192.168.1.40".into(),
            brand: None,
            auth: Some("cookie".into()),
        })
        .unwrap();
        assert_eq!(v.brand(), "Sony Bravia");
    }

    #[test]
    fn build_vendor_rejects_unknown_brand() {
        let result = build_vendor(&SmartTvCreds {
            host: "192.168.1.40".into(),
            brand: Some("nosuchbrand".into()),
            auth: None,
        });
        match result {
            Err(e) => assert!(e.to_string().contains("unknown smart-TV brand")),
            Ok(_) => panic!("expected an error for an unknown brand"),
        }
    }

    #[test]
    fn media_factory_build_rejects_bad_credentials() {
        assert!(SmartTvMediaFactory.build("not json").is_err());
    }

    #[test]
    fn remote_factory_build_succeeds_with_host() {
        assert!(
            SmartTvRemoteFactory
                .build(r#"{"host":"192.168.1.40"}"#)
                .is_ok()
        );
    }

    #[test]
    fn media_factory_advertises_discovery() {
        assert!(SmartTvMediaFactory.discoverer().is_some());
    }

    // The framework adapter end to end: a factory-built provider reads a Bravia
    // mock and maps the snapshot into MediaState / RemoteState.
    #[tokio::test]
    async fn factory_built_provider_maps_tv_state_across_both_domains() {
        use serde_json::json;
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let tv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sony/system"))
            .and(body_string_contains("getPowerStatus"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "result": [{ "status": "active" }] })),
            )
            .mount(&tv)
            .await;
        Mock::given(method("POST"))
            .and(path("/sony/audio"))
            .and(body_string_contains("getVolumeInformation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({ "result": [[{ "target": "speaker", "volume": 22, "mute": true }]] }),
            ))
            .mount(&tv)
            .await;

        let creds = format!(r#"{{"host":"{}"}}"#, tv.uri());
        let media = SmartTvMediaFactory.build(&creds).unwrap();
        let ms = media.get_state("tv").await.unwrap();
        assert!(ms.power && ms.reachable == Some(true));
        assert_eq!(ms.volume, 22);
        assert!(ms.mute);

        let remote = SmartTvRemoteFactory.build(&creds).unwrap();
        let rs = remote.get_state("tv").await.unwrap();
        assert!(rs.on && rs.reachable == Some(true));
    }
}
