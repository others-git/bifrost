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

mod atv;
mod bravia;

use crate::models::media::{
    MediaCapabilities, MediaCommand, MediaDevice, MediaDeviceKind, MediaEvent, MediaState,
    NowPlaying, TransportCmd,
};
use crate::models::remote::{RemoteCommandInfo, RemoteDevice, RemoteKey, RemoteState};
use crate::providers::{
    CredentialField, FieldKind, MediaProvider, MediaProviderFactory, RemoteProvider,
    RemoteProviderFactory,
    discovery::{DeviceDiscovery, HttpSweepDiscovery, SsdpDiscovery, UnionDiscovery},
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
    /// The TV's network address (its configured host), if known.
    pub ip: Option<String>,
}

/// A state push from the TV itself — today sourced from the Android TV Remote
/// v2 session (see `atv::client::AtvEvent`), but vendor-agnostic at this seam.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TvPush {
    /// Foreground app package (e.g. `com.netflix.ninja`).
    CurrentApp(String),
    /// Absolute device volume.
    Volume { level: u32, max: u32, muted: bool },
    /// Screen on/off.
    Screen(bool),
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
    /// Type literal text into the TV's focused field (on-screen keyboard /
    /// search box). Default: unsupported.
    async fn send_text(&self, _text: &str) -> Result<()> {
        anyhow::bail!("this TV does not support text input")
    }
    /// The TV's full native command catalogue (beyond the canonical keys), if it
    /// exposes one. Default: none.
    async fn commands(&self) -> Result<Vec<RemoteCommandInfo>> {
        Ok(Vec::new())
    }
    /// The TV's installed-app catalog (title + launch URI), if enumerable.
    /// Default: none.
    async fn apps(&self) -> Result<Vec<crate::models::remote::InstalledApp>> {
        Ok(Vec::new())
    }
    /// Subscribe to the TV's own state pushes (foreground app / volume /
    /// screen), when the vendor has a push channel (Bravia: the paired Android
    /// TV Remote session). The receiver survives reconnects — the vendor's
    /// link owns the socket. Default: no push channel.
    async fn push_stream(&self) -> Result<tokio::sync::mpsc::Receiver<TvPush>> {
        anyhow::bail!("this TV has no push channel")
    }
    /// Invoke a native command by its token (from [`Self::commands`]). Default:
    /// unsupported.
    async fn send_command(&self, _token: &str) -> Result<()> {
        anyhow::bail!("this TV exposes no native command catalogue")
    }
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
    /// Android TV Remote v2 client certificate (PEM), set once the remote is
    /// paired. Together with [`atv_key`](Self::atv_key) it's the credential for
    /// sending remote keys to Android/Google TV Bravias.
    #[serde(default)]
    atv_cert: Option<String>,
    /// Private key (PEM) paired with [`atv_cert`](Self::atv_cert).
    #[serde(default)]
    atv_key: Option<String>,
}

/// Build the vendor adapter named by the stored `brand` (default: Bravia, the
/// only vendor today). This is the single dispatch point new vendors extend.
fn build_vendor(creds: &SmartTvCreds) -> Result<Box<dyn SmartTvVendor>> {
    // A paired ATV Remote identity (cert+key) unlocks native key control.
    let atv = match (&creds.atv_cert, &creds.atv_key) {
        (Some(cert_pem), Some(key_pem)) => Some(atv::crypto::Identity {
            cert_pem: cert_pem.clone(),
            key_pem: key_pem.clone(),
        }),
        _ => None,
    };
    match creds.brand.as_deref() {
        Some("bravia") | None => Ok(Box::new(bravia::BraviaVendor::new(
            &creds.host,
            creds.auth.clone(),
            atv,
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
    /// The TV allows control without pairing (Authentication "None") — add the
    /// provider with no token.
    NotRequired,
}

/// Begin pairing with the TV at `host` (vendor auto-selected — Bravia today). The
/// TV pops a PIN unless it's already authorised.
pub async fn pair_begin(host: &str) -> Result<SmartTvPairOutcome> {
    match bravia::pairing::begin(host).await? {
        bravia::pairing::PairOutcome::PinDisplayed => Ok(SmartTvPairOutcome::PinDisplayed),
        bravia::pairing::PairOutcome::Paired(auth) => Ok(SmartTvPairOutcome::Paired { auth }),
        bravia::pairing::PairOutcome::NotRequired => Ok(SmartTvPairOutcome::NotRequired),
    }
}

/// Finish pairing with the on-screen PIN; returns the `auth` credential to store.
pub async fn pair_complete(host: &str, pin: &str) -> Result<String> {
    bravia::pairing::complete(host, pin).await
}

// ── ATV Remote v2 pairing (for remote keys on Android/Google TVs) ────────────
//
// This is a *separate* credential from the ScalarWeb PIN cookie above: it's the
// self-signed client cert the Android TV Remote protocol authenticates with.
// Pairing is interactive — the TV shows a code only after the configuration
// step — so the live TLS session is parked here (keyed by host) between
// [`atv_pair_begin`] and [`atv_pair_complete`].

type AtvSession = (atv::client::PairingSession, atv::crypto::Identity);

fn atv_pairings() -> &'static std::sync::Mutex<std::collections::HashMap<String, AtvSession>> {
    static M: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, AtvSession>>> =
        std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// The name this instance pairs under. MUST be distinct per Bifrost deployment:
/// Android TV keeps ONE trusted certificate per client name, so two instances
/// pairing as the same name silently evict each other's pairing (pairing the
/// production deploy invalidated the dev instance's remote until re-paired).
/// The machine hostname distinguishes them and reads well in the TV's own
/// paired-devices list.
fn pairing_client_name() -> String {
    let host = std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty());
    match host {
        Some(h) => format!("Bifrost ({h})"),
        None => "Bifrost".to_string(),
    }
}

/// Begin ATV Remote pairing: generate a fresh identity, drive the handshake to
/// the point where the TV displays its 6-digit code, and park the live session.
/// Call [`atv_pair_complete`] with the code to finish.
pub async fn atv_pair_begin(host: &str) -> Result<()> {
    let identity = atv::crypto::Identity::generate()?;
    let session =
        atv::client::PairingSession::begin(host, &identity, &pairing_client_name()).await?;
    atv_pairings()
        .lock()
        .expect("atv pairing map poisoned")
        .insert(host.to_string(), (session, identity));
    Ok(())
}

/// Finish ATV Remote pairing with the on-screen `code`; returns the
/// `(cert_pem, key_pem)` to persist as the `atv_cert`/`atv_key` credentials.
pub async fn atv_pair_complete(host: &str, code: &str) -> Result<(String, String)> {
    let (session, identity) = atv_pairings()
        .lock()
        .expect("atv pairing map poisoned")
        .remove(host)
        .ok_or_else(|| anyhow!("no pairing in progress for this TV — start pairing again"))?;
    session.finish(code).await?;
    Ok((identity.cert_pem, identity.key_pem))
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

/// The ScalarWeb probe for the HTTP sweep: `getInterfaceInformation` needs no
/// PSK/pairing on a Bravia (it's part of the pre-auth surface), so it's callable
/// on a TV that's never been paired — exactly the add-provider situation.
pub(crate) const BRAVIA_SWEEP_BODY: &str =
    r#"{"method":"getInterfaceInformation","id":1,"params":[],"version":"1.0"}"#;

/// `true` when a `/sony/system` response identifies the box as a **TV** —
/// `productCategory` is `"tv"` on a Bravia, and something else on other Sony
/// ScalarWeb gear (soundbars, Blu-ray). Whitespace-stripped before matching so
/// a pretty-printing firmware can't dodge it.
pub(crate) fn bravia_sweep_match(body: &str) -> bool {
    let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains(r#""productCategory":"tv""#)
}

fn smarttv_discoverer() -> Box<dyn DeviceDiscovery> {
    // Two legs, deduped by host. Only Bravia today, so no brand stamp is needed
    // (the dispatch defaults to it).
    // 1. SSDP: Sony TVs answer the ScalarWeb ST and carry "sony" in the reply;
    //    the IP comes from the LOCATION header. Fast, but multicast — a Wi-Fi TV
    //    dozing in power-save (or any container/VLAN setup that can't multicast)
    //    can miss it.
    // 2. HTTP sweep: POST the unauthenticated ScalarWeb `getInterfaceInformation`
    //    to every host in the local /24 (+ Expanded-LAN subnets) and keep the
    //    ones that answer `productCategory: tv`. Authoritative — it's the same
    //    endpoint the provider drives after pairing — and it works where SSDP
    //    physically can't.
    Box::new(UnionDiscovery::new(vec![
        Box::new(SsdpDiscovery::new(
            SONY_SCALARWEB_ST,
            "sony",
            "Sony Bravia (Smart TV)",
            "host",
        )),
        Box::new(
            HttpSweepDiscovery::new(
                "/sony/system",
                "Sony Bravia (Smart TV)",
                "host",
                bravia_sweep_match,
            )
            .post(BRAVIA_SWEEP_BODY),
        ),
    ]))
}

// ── The framework adapter: one TV, two domains ───────────────────────────────

/// A connected smart TV: holds the resolved vendor and implements both the media
/// and remote provider traits over it. Each factory builds its own instance
/// (cheap — host + auth + an HTTP client).
struct SmartTv {
    // Shared with the push-fold task `event_stream` spawns.
    vendor: std::sync::Arc<dyn SmartTvVendor>,
    /// Stable provider-native id for the single device this provider serves.
    device_id: String,
}

impl SmartTv {
    fn from_creds(credentials_json: &str) -> Result<Self> {
        let creds = parse_creds(credentials_json)?;
        Ok(Self {
            device_id: creds.host.clone(),
            vendor: std::sync::Arc::from(build_vendor(&creds)?),
        })
    }
}

/// Foreground packages that aren't a user app on screen — the launcher, the
/// screensaver, system chrome. Now-playing clears rather than naming them.
fn is_system_surface(package: &str) -> bool {
    let p = package.to_ascii_lowercase();
    [
        "launcher",
        "dream",
        "screensaver",
        "backdrop",
        "systemui",
        "inputmethod",
    ]
    .iter()
    .any(|kw| p.contains(kw))
}

/// Fold one TV push into the running media state. Pure — the shared logic the
/// push stream applies, unit-tested without a TV. Any push proves the TV spoke
/// to us, so reachability rides along.
fn apply_tv_push(
    state: &mut MediaState,
    push: &TvPush,
    apps: &[crate::models::remote::InstalledApp],
) {
    state.reachable = Some(true);
    match push {
        TvPush::CurrentApp(pkg) => {
            state.now_playing = if is_system_surface(pkg) {
                None
            } else {
                let name = apps
                    .iter()
                    .find(|a| &a.package == pkg)
                    .map(|a| a.name.clone())
                    .unwrap_or_else(|| crate::models::remote::app_display_name(pkg));
                Some(NowPlaying {
                    title: Some(name),
                    artist: None,
                    album: None,
                    play_state: None,
                    artwork_url: None,
                })
            };
        }
        TvPush::Volume { level, max, muted } => {
            if *max > 0 {
                state.volume = ((level * 100).div_ceil(*max) as u8).min(100);
            }
            state.mute = *muted;
        }
        TvPush::Screen(on) => {
            state.power = *on;
            if !on {
                state.now_playing = None;
            }
        }
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
        ip: s.ip.clone(),
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

    /// Live pushes from the TV itself (foreground app / volume / screen) when
    /// the vendor has a push channel — the only source of "which app is on
    /// screen" (ScalarWeb has no foreground getter, and its now-playing API
    /// errors whenever an app owns the screen). Seeds from a scalar snapshot,
    /// resolves app packages against the TV's installed catalog, and emits
    /// changed-only full states on the same pipeline the demand poller feeds.
    async fn event_stream(&self) -> Result<tokio::sync::mpsc::Receiver<MediaEvent>> {
        let mut push = self.vendor.push_stream().await?;
        let (tx, out) = tokio::sync::mpsc::channel::<MediaEvent>(64);
        let vendor = std::sync::Arc::clone(&self.vendor);
        let device_id = self.device_id.clone();
        tokio::spawn(async move {
            // Seed from a live snapshot so the first push patches real state,
            // not defaults; the catalog names foreground packages.
            let mut state = match vendor.snapshot().await {
                Ok(s) => tv_media_state(&s),
                Err(_) => MediaState {
                    reachable: None,
                    ..Default::default()
                },
            };
            let apps = vendor.apps().await.unwrap_or_default();
            let mut last: Option<MediaState> = None;
            while let Some(p) = push.recv().await {
                apply_tv_push(&mut state, &p, &apps);
                if last.as_ref() != Some(&state) {
                    last = Some(state.clone());
                    if tx
                        .send(MediaEvent {
                            device_id: device_id.clone(),
                            state: state.clone(),
                        })
                        .await
                        .is_err()
                    {
                        return; // consumer gone
                    }
                }
            }
        });
        Ok(out)
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
            ip: s.ip,
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

    async fn list_apps(
        &self,
        _device_id: &str,
    ) -> Result<Vec<crate::models::remote::InstalledApp>> {
        self.vendor.apps().await
    }

    async fn launch_app(&self, _device_id: &str, activity: &str) -> Result<()> {
        tracing::debug!(target: "bifrost::smarttv", brand = self.vendor.brand(), activity, "launch app");
        self.vendor.launch_app(activity).await
    }

    async fn send_text(&self, _device_id: &str, text: &str) -> Result<()> {
        tracing::debug!(target: "bifrost::smarttv", brand = self.vendor.brand(), len = text.len(), "send text");
        self.vendor.send_text(text).await
    }

    async fn list_commands(&self, _device_id: &str) -> Result<Vec<RemoteCommandInfo>> {
        self.vendor.commands().await
    }

    async fn send_native(&self, _device_id: &str, token: &str) -> Result<()> {
        tracing::debug!(target: "bifrost::smarttv", brand = self.vendor.brand(), token, "native command");
        self.vendor.send_command(token).await
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
    fn pairing_client_name_is_instance_distinct() {
        // Whatever the hostname source, the name is non-empty, branded, and —
        // when a hostname exists — carries it, so two deployments never pair
        // under the same identity slot on the TV.
        let name = pairing_client_name();
        assert!(name.starts_with("Bifrost"));
        if let Some(h) = std::env::var("HOSTNAME")
            .ok()
            .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty())
        {
            assert_eq!(name, format!("Bifrost ({h})"));
        }
    }

    fn catalog() -> Vec<crate::models::remote::InstalledApp> {
        vec![crate::models::remote::InstalledApp {
            package: "com.netflix.ninja".into(),
            name: "Netflix".into(),
            activity: Some("com.netflix.ninja-Main".into()),
        }]
    }

    #[test]
    fn push_fold_names_the_foreground_app_from_the_catalog() {
        let mut st = MediaState::default();
        apply_tv_push(
            &mut st,
            &TvPush::CurrentApp("com.netflix.ninja".into()),
            &catalog(),
        );
        assert_eq!(
            st.now_playing.as_ref().and_then(|n| n.title.as_deref()),
            Some("Netflix")
        );
        assert_eq!(
            st.reachable,
            Some(true),
            "a push proves the TV is reachable"
        );

        // Unknown package falls back to the shared brand/prettify naming.
        apply_tv_push(
            &mut st,
            &TvPush::CurrentApp("com.hulu.livingroomplus".into()),
            &catalog(),
        );
        assert_eq!(
            st.now_playing.as_ref().and_then(|n| n.title.as_deref()),
            Some("Hulu")
        );

        // The launcher/screensaver isn't "playing" anything.
        apply_tv_push(
            &mut st,
            &TvPush::CurrentApp("com.google.android.tvlauncher".into()),
            &catalog(),
        );
        assert!(
            st.now_playing.is_none(),
            "system surfaces clear now-playing"
        );
    }

    #[test]
    fn push_fold_scales_volume_and_tracks_screen_state() {
        let mut st = MediaState::default();
        apply_tv_push(
            &mut st,
            &TvPush::Volume {
                level: 7,
                max: 25,
                muted: false,
            },
            &[],
        );
        assert_eq!(st.volume, 28, "7/25 scales to percent");
        assert!(!st.mute);
        // A zero max (proto3 omission / bogus push) must not divide by zero or
        // clobber the known volume.
        apply_tv_push(
            &mut st,
            &TvPush::Volume {
                level: 0,
                max: 0,
                muted: true,
            },
            &[],
        );
        assert_eq!(st.volume, 28);
        assert!(st.mute);

        apply_tv_push(
            &mut st,
            &TvPush::CurrentApp("com.netflix.ninja".into()),
            &[],
        );
        assert!(st.now_playing.is_some());
        apply_tv_push(&mut st, &TvPush::Screen(false), &[]);
        assert!(!st.power, "screen off is power off");
        assert!(st.now_playing.is_none(), "a sleeping TV plays nothing");
        apply_tv_push(&mut st, &TvPush::Screen(true), &[]);
        assert!(st.power);
    }

    #[test]
    fn bravia_sweep_match_accepts_a_tv_and_rejects_other_sony_gear() {
        // Compact (real Bravia shape) and pretty-printed both match.
        assert!(bravia_sweep_match(
            r#"{"result":[{"interfaceVersion":"5.0.1","modelName":"XR-55A80J","productCategory":"tv","productName":"BRAVIA"}],"id":1}"#
        ));
        assert!(bravia_sweep_match(
            "{\"result\": [{ \"productCategory\" : \"tv\" }], \"id\": 1}"
        ));
        // A Sony soundbar answers ScalarWeb too, but is not a TV.
        assert!(!bravia_sweep_match(
            r#"{"result":[{"productCategory":"homeTheaterSystem","productName":"HT-A7000"}],"id":1}"#
        ));
        // An unauthenticated-error or non-ScalarWeb response never matches.
        assert!(!bravia_sweep_match(r#"{"error":[403,"Forbidden"],"id":1}"#));
        assert!(!bravia_sweep_match("<html>not a bravia</html>"));
    }

    #[tokio::test]
    async fn smarttv_http_sweep_finds_a_bravia_by_scalarweb_probe() {
        use crate::providers::discovery::ScanOptions;
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // A mock Bravia: the sweep's exact POST (getInterfaceInformation, no
        // auth) gets the exact compact reply a real TV sends.
        let tv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sony/system"))
            .and(body_string_contains("getInterfaceInformation"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"result":[{"interfaceVersion":"5.0.1","modelName":"XR-55A80J","productCategory":"tv","productName":"BRAVIA"}],"id":1}"#,
            ))
            .mount(&tv)
            .await;

        // The same sweep leg `smarttv_discoverer` unions in, with test bases.
        let found = HttpSweepDiscovery::new(
            "/sony/system",
            "Sony Bravia (Smart TV)",
            "host",
            bravia_sweep_match,
        )
        .post(BRAVIA_SWEEP_BODY)
        .with_bases(vec![tv.uri()])
        .scan(&ScanOptions::new(std::time::Duration::from_secs(1)))
        .await
        .unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label.as_deref(), Some("Sony Bravia (Smart TV)"));
        // Credentials pre-shape the add-provider form's host field.
        assert!(found[0].credentials.get("host").is_some());
    }

    #[test]
    fn build_vendor_defaults_to_bravia() {
        let v = build_vendor(&SmartTvCreds {
            host: "192.168.1.40".into(),
            brand: None,
            auth: Some("cookie".into()),
            atv_cert: None,
            atv_key: None,
        })
        .unwrap();
        assert_eq!(v.brand(), "Sony Bravia");
    }

    #[test]
    fn build_vendor_accepts_a_paired_atv_identity() {
        let id = atv::crypto::Identity::generate().unwrap();
        let v = build_vendor(&SmartTvCreds {
            host: "192.168.1.40".into(),
            brand: Some("bravia".into()),
            auth: None,
            atv_cert: Some(id.cert_pem),
            atv_key: Some(id.key_pem),
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
            atv_cert: None,
            atv_key: None,
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
