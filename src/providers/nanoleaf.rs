//! Nanoleaf Open API integration (LAN, no cloud).
//!
//! Base URL: `http://<controller-ip>:16021`, all calls under `/api/v1`.
//!
//! Pairing: the controller must be put in pairing mode (hold the power button
//! ~5–7s until the LED flashes), then `POST /api/v1/new` returns
//! `{"auth_token": "…"}`. Every other call is `/api/v1/{auth_token}/…`. The token
//! is persisted as the `auth_token` credential (see [`pair`] and the
//! `POST /api/providers/nanoleaf/pair` endpoint, modelled on Hue's link button).
//!
//! Colour model: Nanoleaf exposes HSV (`hue` 0–360, `sat` 0–100, plus a separate
//! `brightness` 0–100) and a `ct` colour temperature in **kelvin** (1200–6500),
//! with a `colorMode` (`hs` / `ct` / `effect`) naming which is live. We translate
//! to/from Bifrost's CIE-xy [`Color`] + mirek, exactly one mode at a time. A
//! running effect (from `effects/effectsList`) is Bifrost's third light mode —
//! brightness stays valid while an effect plays (its own `/state` endpoint), so a
//! brightness change never clobbers the effect.

use crate::models::{
    Color, Light, LightCapabilities, LightState, Provider, is_clear_effect, kelvin_to_mirek,
    mirek_to_kelvin,
};
use crate::providers::discovery::{
    DeviceDiscovery, MdnsDiscovery, TcpPortSweepDiscovery, UnionDiscovery,
};
use crate::providers::{
    CredentialField, Credentials, FieldKind, LanBinding, LightProvider, ProviderFactory, base_url,
    cached_client, cred_str, hsv_to_rgb, rgb_to_hs,
};
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

/// The port the Nanoleaf Open API always listens on.
const NANOLEAF_PORT: u16 = 16021;
/// Nanoleaf's colour-temperature range, in kelvin (the `ct` field's real bounds;
/// some firmware reports 0–100, hence a fixed clamp rather than trusting min/max).
const CT_MIN_K: u32 = 1200;
const CT_MAX_K: u32 = 6500;

// ── Pairing ──────────────────────────────────────────────────────────────────

const PAIR_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, PartialEq)]
pub enum PairOutcome {
    /// Pairing succeeded; store this as the `auth_token` credential.
    Paired { auth_token: String },
    /// The controller isn't in pairing mode (it answered 401/403). The user must
    /// hold the power button until the LED flashes, then retry.
    NotInPairingMode,
}

#[derive(Debug, Deserialize)]
struct NewTokenReply {
    auth_token: String,
}

/// One pairing handshake against `base` (e.g. `http://192.168.1.20:16021`):
/// `POST /api/v1/new`. When the controller is in pairing mode it returns the
/// token; otherwise it answers 401/403, surfaced as [`PairOutcome::NotInPairingMode`]
/// so the caller can tell the user to press the button (mirrors Hue's link
/// button). Any other failure is an error (unreachable controller, bad reply).
pub async fn pair(base: &str) -> Result<PairOutcome> {
    let client = Client::builder().timeout(PAIR_TIMEOUT).build()?;
    let resp = client
        .post(format!("{base}/api/v1/new"))
        .send()
        .await
        .context("could not reach the Nanoleaf controller")?;
    // 401/403 = not in pairing mode (the only "press the button" signal).
    if matches!(resp.status().as_u16(), 401 | 403) {
        return Ok(PairOutcome::NotInPairingMode);
    }
    let reply: NewTokenReply = resp
        .error_for_status()?
        .json()
        .await
        .context("unexpected reply from the Nanoleaf controller")?;
    Ok(PairOutcome::Paired {
        auth_token: reply.auth_token,
    })
}

// ── Provider ─────────────────────────────────────────────────────────────────

/// A single Nanoleaf controller (one panel set) reached over the LAN Open API.
/// One physical controller per provider row, like WLED — its lone light uses the
/// stable device id `"main"`.
pub struct NanoleafProvider {
    client: Client,
    /// e.g. `http://192.168.1.20:16021`.
    base_url: String,
    /// Original host string, for actionable error messages.
    host: String,
    /// `None` until paired; every control needs it.
    auth_token: Option<String>,
}

impl NanoleafProvider {
    pub fn new(host: impl AsRef<str>, auth_token: Option<String>) -> Result<Self> {
        let host = host.as_ref().to_string();
        let base = base_url(&host, "http", Some(NANOLEAF_PORT));
        // One pooled client for every controller: nothing here varies per host
        // (no auth headers — the token lives in the path — and the base URL is
        // on the struct), so a per-host key would only mint duplicate clients
        // that live for the process. Bounded so a powered-off controller fails
        // fast.
        let client = cached_client("nanoleaf", || {
            Ok(Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(15))
                .build()?)
        })?;
        Ok(Self {
            client,
            base_url: base,
            host,
            auth_token,
        })
    }

    pub fn from_credentials(creds_json: &str) -> Result<Self> {
        let creds: Value = serde_json::from_str(creds_json)?;
        let host = creds["host"]
            .as_str()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .ok_or_else(|| anyhow!("nanoleaf credentials missing host"))?;
        let token = creds["auth_token"]
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        Self::new(host, token)
    }

    /// The auth token, or a clear "hold the power button and pair" error.
    fn token(&self) -> Result<&str> {
        self.auth_token.as_deref().filter(|t| !t.is_empty()).ok_or_else(|| {
            anyhow!(
                "Nanoleaf at {} is not paired — hold the controller's power button for ~5-7s until the LED flashes, then pair (POST /api/providers/nanoleaf/pair).",
                self.host
            )
        })
    }

    async fn fetch_info(&self, token: &str) -> Result<NanoInfo> {
        Ok(self
            .client
            .get(format!("{}/api/v1/{token}/", self.base_url))
            .send()
            .await
            .context("Nanoleaf info request failed")?
            .error_for_status()?
            .json()
            .await?)
    }

    /// PUT a partial `state` body (`{"on":…, "brightness":…, "hue":…}`).
    async fn put_state(&self, token: &str, body: &Value) -> Result<()> {
        let resp = self
            .client
            .put(format!("{}/api/v1/{token}/state", self.base_url))
            .json(body)
            .send()
            .await
            .context("Nanoleaf set-state request failed")?;
        ensure_success(resp, "state").await
    }

    /// Select a named dynamic effect (`PUT /effects {"select": …}`).
    async fn select_effect(&self, token: &str, name: &str) -> Result<()> {
        let resp = self
            .client
            .put(format!("{}/api/v1/{token}/effects", self.base_url))
            .json(&json!({ "select": name }))
            .send()
            .await
            .context("Nanoleaf select-effect request failed")?;
        ensure_success(resp, "effects").await
    }

    #[cfg(test)]
    fn new_for_test(base: impl AsRef<str>, token: &str) -> Self {
        // `base` is a full `http://…` URL (a wiremock server); `base_url` keeps an
        // explicit scheme verbatim, so no port is appended.
        Self::new(base.as_ref(), Some(token.to_string())).unwrap()
    }
}

async fn ensure_success(resp: reqwest::Response, what: &str) -> Result<()> {
    if resp.status().is_success() {
        return Ok(());
    }
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    bail!("Nanoleaf {what} error {status}: {text}");
}

// ── Wire types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct NanoInfo {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "serialNo")]
    serial_no: String,
    state: NanoState,
    #[serde(default)]
    effects: NanoEffects,
}

#[derive(Debug, Deserialize)]
struct NanoState {
    on: NanoOn,
    #[serde(default)]
    brightness: NanoVal,
    #[serde(default)]
    hue: NanoVal,
    #[serde(default)]
    sat: NanoVal,
    #[serde(default)]
    ct: NanoVal,
    /// `hs` | `ct` | `effect` — which mode is live.
    #[serde(default, rename = "colorMode")]
    color_mode: String,
}

#[derive(Debug, Default, Deserialize)]
struct NanoOn {
    #[serde(default)]
    value: bool,
}

#[derive(Debug, Default, Deserialize)]
struct NanoVal {
    #[serde(default)]
    value: f32,
}

#[derive(Debug, Default, Deserialize)]
struct NanoEffects {
    /// The current effect name; a reserved built-in wrapped in asterisks
    /// (`*Static*`, `*Dynamic*`, `*Solid*`) means "no selectable user effect is
    /// playing" — the asterisk prefix is what marks them.
    #[serde(default)]
    select: String,
    #[serde(default, rename = "effectsList")]
    effects_list: Vec<String>,
}

// ── Conversion ───────────────────────────────────────────────────────────────

/// A Nanoleaf `hue`(0–360)/`sat`(0–100) pair → a Bifrost RGB colour. Brightness
/// is carried separately (`v = 1.0`), so it never leaks into the colour.
fn hs_to_color(hue: f32, sat: f32) -> Color {
    let (r, g, b) = hsv_to_rgb(hue, (sat / 100.0).clamp(0.0, 1.0), 1.0);
    Color::from_rgb(r, g, b)
}

/// The effects the UI may pick — `no_effect` (the clear token) first, then the
/// controller's `effectsList`. Empty stays empty (no effects advertised).
fn effect_options(list: &[String]) -> Vec<String> {
    if list.is_empty() {
        return Vec::new();
    }
    let mut fx = vec!["no_effect".to_string()];
    fx.extend(list.iter().cloned());
    fx
}

/// Map a controller's `state` + `effects` onto Bifrost's single-mode light state.
/// `colorMode` is authoritative: `effect` (a real, non-built-in effect name) →
/// effect mode; `ct` → colour temperature; anything else (`hs`) → colour.
fn parse_state(s: &NanoState, fx: &NanoEffects) -> LightState {
    let mut state = LightState {
        on: s.on.value,
        brightness: Some(s.brightness.value.clamp(0.0, 100.0)),
        reachable: Some(true),
        ..Default::default()
    };
    let effect_name = fx.select.trim();
    let real_effect = s.color_mode == "effect"
        && !effect_name.is_empty()
        && !effect_name.starts_with('*') // built-ins like *Solid* / *Dynamic*
        && !is_clear_effect(effect_name);
    if real_effect {
        state.effect = Some(effect_name.to_string());
    } else if s.color_mode == "ct" {
        state.color_temp_mirek = Some(kelvin_to_mirek(s.ct.value.round().max(1.0) as u32));
    } else {
        state.color = Some(hs_to_color(s.hue.value, s.sat.value));
    }
    state
}

fn info_to_light(info: NanoInfo) -> Light {
    let capabilities = LightCapabilities {
        dimmable: true,
        color_rgb: true,
        color_temperature: true,
        hue_gamut: None,
        effects: effect_options(&info.effects.effects_list),
        segments: None,
    };
    let state = parse_state(&info.state, &info.effects);
    Light {
        id: Uuid::new_v4(),
        // The Open API exposes no MAC (only a serial), so cross-provider de-dup
        // against an HA copy generally can't auto-fire (it's exact-MAC only) — the
        // HA duplicate is shadowed manually. `mac_hw_id` still yields a key on the
        // off chance a serial is MAC-shaped, and `None` otherwise (safe).
        hw_id: crate::providers::mac_hw_id(&info.serial_no),
        provider_id: "main".into(),
        provider: Provider::Nanoleaf,
        name: info.name,
        state,
        capabilities,
        last_seen: Utc::now(),
    }
}

// ── Provider impl ────────────────────────────────────────────────────────────

#[async_trait]
impl LightProvider for NanoleafProvider {
    fn name(&self) -> &str {
        "nanoleaf"
    }

    async fn discover(&self) -> Result<Vec<Light>> {
        let token = self.token()?;
        let info = self.fetch_info(token).await?;
        Ok(vec![info_to_light(info)])
    }

    async fn get_state(&self, _device_id: &str) -> Result<LightState> {
        let token = self.token()?;
        let info = self.fetch_info(token).await?;
        Ok(parse_state(&info.state, &info.effects))
    }

    async fn set_state(&self, _device_id: &str, state: &LightState) -> Result<()> {
        let token = self.token()?;

        // Turning off: power down only — never re-select a mode (selecting an
        // effect would wake the controller back on).
        if !state.on {
            return self
                .put_state(token, &json!({ "on": { "value": false } }))
                .await;
        }

        let brightness = state
            .brightness
            .map(|b| json!({ "value": b.round().clamp(0.0, 100.0) as u32 }));

        // Effect is its own endpoint AND Bifrost's third mode. Apply power +
        // brightness first (both stay valid while an effect plays — this is what
        // keeps a brightness change from clobbering the effect), then select it so
        // `colorMode` settles on `effect`.
        if let Some(effect) = state.effect.as_deref().filter(|e| !is_clear_effect(e)) {
            let mut body = json!({ "on": { "value": true } });
            if let Some(b) = brightness {
                body["brightness"] = b;
            }
            self.put_state(token, &body).await?;
            return self.select_effect(token, effect).await;
        }

        // Plain colour / colour-temperature / on / brightness: one PUT carries the
        // whole state (Nanoleaf accepts several keys at once). Colour and colour
        // temperature are mutually exclusive — colour wins, matching the cache merge.
        let mut body = json!({ "on": { "value": true } });
        if let Some(b) = brightness {
            body["brightness"] = b;
        }
        if let Some(color) = &state.color {
            let (r, g, b) = color.to_rgb();
            let (hue, sat) = rgb_to_hs(r, g, b);
            body["hue"] = json!({ "value": hue.round() as u32 });
            body["sat"] = json!({ "value": (sat * 100.0).round() as u32 });
        } else if let Some(mirek) = state.color_temp_mirek {
            let kelvin = mirek_to_kelvin(mirek).clamp(CT_MIN_K, CT_MAX_K);
            body["ct"] = json!({ "value": kelvin });
        }
        self.put_state(token, &body).await
    }
}

// ── Factory ──────────────────────────────────────────────────────────────────

pub struct NanoleafProviderFactory;

/// The controller is reached at a stored IP, so a DHCP change strands it.
///
/// Identity proof is the **auth token**: the Open API exposes no MAC (only a
/// serial), so Nanoleaf devices carry no `hw_id` at all — but a token is minted
/// by one controller and rejected by every other, so an authenticated read
/// answers the same question a MAC comparison would.
struct NanoleafLanBinding;

#[async_trait]
impl LanBinding for NanoleafLanBinding {
    fn host_field(&self) -> &'static str {
        "host"
    }

    fn probe_port(&self, _creds: &Credentials) -> u16 {
        NANOLEAF_PORT
    }

    fn can_verify(&self, creds: &Credentials, _known_hw: &[String]) -> bool {
        cred_str(creds, "auth_token").is_some()
    }

    async fn is_same_device(&self, host: &str, creds: &Credentials, _known_hw: &[String]) -> bool {
        let Some(token) = cred_str(creds, "auth_token") else {
            return false; // unpaired: nothing to prove identity with
        };
        match NanoleafProvider::new(host, Some(token.to_string())) {
            Ok(p) => p.fetch_info(token).await.is_ok(),
            Err(_) => false,
        }
    }
}

impl ProviderFactory for NanoleafProviderFactory {
    fn provider_type(&self) -> &'static str {
        "nanoleaf"
    }

    fn display_name(&self) -> &'static str {
        "Nanoleaf"
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn LightProvider>> {
        Ok(Box::new(NanoleafProvider::from_credentials(
            credentials_json,
        )?))
    }

    /// mDNS `_nanoleafapi._tcp` (the controller advertises its instance
    /// name) unioned with a TCP :16021 sweep — the multicast-proof fallback
    /// for networks that drop mDNS (and the only leg that works from WSL2's
    /// NAT). First leg to answer wins per host.
    fn discoverer(&self) -> Option<Box<dyn DeviceDiscovery>> {
        Some(Box::new(UnionDiscovery::new(vec![
            Box::new(MdnsDiscovery::new("_nanoleafapi._tcp.local", "")),
            Box::new(TcpPortSweepDiscovery::new(16021, "Nanoleaf", "")),
        ])))
    }

    fn lan_binding(&self) -> Option<Box<dyn LanBinding>> {
        Some(Box::new(NanoleafLanBinding))
    }

    fn credentials_schema(&self) -> &'static [CredentialField] {
        &[
            CredentialField {
                name: "host",
                label: "IP Address",
                kind: FieldKind::IpAddress,
                required: true,
                hint: Some(
                    "IP of the Nanoleaf controller on your LAN — use \"Scan network\" above, or enter it manually.",
                ),
            },
            CredentialField {
                name: "auth_token",
                label: "Auth token",
                kind: FieldKind::Password,
                required: false,
                hint: Some(
                    "Leave blank to pair: hold the controller's power button ~5-7s until the LED flashes, then use Pair to generate a token.",
                ),
            },
        ]
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn lan_binding_proves_identity_with_the_auth_token() {
        use crate::providers::LanBinding as _;
        let b = NanoleafLanBinding;
        assert_eq!(b.host_field(), "host");
        assert_eq!(b.probe_port(&serde_json::Map::new()), NANOLEAF_PORT);

        // The Open API exposes no MAC, so the token is the identity: the
        // controller that issued it serves the read; any other rejects it.
        let ours = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tok/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(info_json("hs", "")))
            .mount(&ours)
            .await;
        let stranger = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&stranger)
            .await;

        let creds: Credentials = json!({"auth_token": "tok"}).as_object().unwrap().clone();
        assert!(b.is_same_device(&ours.uri(), &creds, &[]).await);
        assert!(!b.is_same_device(&stranger.uri(), &creds, &[]).await);
        // Unpaired → nothing to prove identity with.
        assert!(
            !b.is_same_device(&ours.uri(), &serde_json::Map::new(), &[])
                .await
        );
    }

    fn info_json(color_mode: &str, select: &str) -> Value {
        json!({
            "name": "Shapes Studio",
            "serialNo": "S19513C1234",
            "manufacturer": "Nanoleaf",
            "firmwareVersion": "7.1.2",
            "model": "NL52",
            "state": {
                "on": { "value": true },
                "brightness": { "value": 60, "max": 100, "min": 0 },
                "hue": { "value": 120, "max": 360, "min": 0 },
                "sat": { "value": 80, "max": 100, "min": 0 },
                "ct": { "value": 4000, "max": 6500, "min": 1200 },
                "colorMode": color_mode
            },
            "effects": {
                "select": select,
                "effectsList": ["Color Burst", "Northern Lights", "Windmill"]
            }
        })
    }

    // ── Pure conversion helpers ─────────────────────────────────────────────

    #[tokio::test]
    async fn factory_scan_finds_a_controller_by_its_open_port() {
        // supports_discovery gates the UI's "Scan network" button.
        let f = NanoleafProviderFactory;
        let disc = f.discoverer().expect("nanoleaf advertises a discoverer");
        // A live TCP listener stands in for the controller's :16021 — probe it
        // directly through the sweep leg (the union's mDNS leg finds nothing).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let sweep = TcpPortSweepDiscovery::new(addr.port(), "Nanoleaf", "")
            .with_hosts(vec!["127.0.0.1".into()]);
        let found = sweep
            .scan(&crate::providers::discovery::ScanOptions::new(
                std::time::Duration::from_secs(1),
            ))
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].host, "127.0.0.1");
        assert_eq!(found[0].label.as_deref(), Some("Nanoleaf"));
        // No vendor routing for a single-vendor provider: no stray brand key.
        assert!(found[0].credentials.get("brand").is_none());
        drop(disc);
    }

    #[test]
    fn hs_to_color_and_back_roundtrips() {
        // Nanoleaf reports hue/sat; parse → Color → drive hue/sat back out.
        for (hue, sat) in [(0.0, 100.0), (120.0, 80.0), (240.0, 100.0)] {
            let color = hs_to_color(hue, sat);
            let (r, g, b) = color.to_rgb();
            let (h2, s2) = rgb_to_hs(r, g, b);
            assert!((h2 - hue).abs() < 2.0, "hue {h2} vs {hue}");
            assert!(
                (s2 * 100.0 - sat).abs() < 2.0,
                "sat {} vs {sat}",
                s2 * 100.0
            );
        }
    }

    #[test]
    fn ct_maps_to_mirek_both_ways() {
        // 4000K → mirek and back within the clamp band.
        let mirek = kelvin_to_mirek(4000);
        assert_eq!(mirek, (1_000_000 / 4000) as u16);
        let kelvin = mirek_to_kelvin(mirek).clamp(CT_MIN_K, CT_MAX_K);
        assert!((kelvin as i32 - 4000).abs() < 50);
    }

    #[test]
    fn effect_options_prepends_clear_token() {
        assert_eq!(
            effect_options(&["A".into(), "B".into()]),
            vec!["no_effect", "A", "B"]
        );
        // No effects on the controller → advertise none (not just the clear token).
        assert!(effect_options(&[]).is_empty());
    }

    // ── Read mapping (colorMode drives the mode) ────────────────────────────

    #[tokio::test]
    async fn discover_parses_hs_mode_as_colour() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tok/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(info_json("hs", "*Solid*")))
            .mount(&server)
            .await;
        let lights = NanoleafProvider::new_for_test(server.uri(), "tok")
            .discover()
            .await
            .unwrap();
        assert_eq!(lights.len(), 1);
        let l = &lights[0];
        assert_eq!(l.name, "Shapes Studio");
        assert!(matches!(l.provider, Provider::Nanoleaf));
        assert_eq!(l.provider_id, "main");
        assert!(l.state.on);
        assert_eq!(l.state.brightness, Some(60.0));
        assert!(l.state.color.is_some(), "hs mode → colour");
        assert_eq!(l.state.color_temp_mirek, None);
        assert_eq!(l.state.effect, None);
        assert_eq!(l.state.reachable, Some(true));
        // Capabilities advertise colour + temp + effects (clear token first).
        assert!(l.capabilities.color_rgb && l.capabilities.color_temperature);
        assert_eq!(l.capabilities.effects.first().unwrap(), "no_effect");
        assert!(l.capabilities.effects.contains(&"Windmill".to_string()));
    }

    #[tokio::test]
    async fn discover_parses_ct_mode_as_temperature() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tok/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(info_json("ct", "*Solid*")))
            .mount(&server)
            .await;
        let s = NanoleafProvider::new_for_test(server.uri(), "tok")
            .get_state("main")
            .await
            .unwrap();
        assert_eq!(s.color_temp_mirek, Some((1_000_000 / 4000) as u16));
        assert!(s.color.is_none());
        assert_eq!(s.effect, None);
    }

    #[tokio::test]
    async fn discover_parses_effect_mode() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tok/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(info_json("effect", "Windmill")))
            .mount(&server)
            .await;
        let s = NanoleafProvider::new_for_test(server.uri(), "tok")
            .get_state("main")
            .await
            .unwrap();
        assert_eq!(s.effect.as_deref(), Some("Windmill"));
        assert!(s.color.is_none(), "an effect clears colour");
        assert_eq!(s.color_temp_mirek, None);
    }

    #[tokio::test]
    async fn builtin_solid_effect_is_not_a_user_effect() {
        // colorMode "effect" but select "*Solid*" is the built-in solid, not a
        // dynamic effect — treated as a plain colour from hue/sat.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tok/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(info_json("effect", "*Solid*")))
            .mount(&server)
            .await;
        let s = NanoleafProvider::new_for_test(server.uri(), "tok")
            .get_state("main")
            .await
            .unwrap();
        assert_eq!(s.effect, None);
        assert!(s.color.is_some());
    }

    // ── Write mapping ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn set_state_colour_writes_hue_and_sat() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/tok/state"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let state = LightState {
            on: true,
            brightness: Some(50.0),
            color: Some(Color::from_rgb(255, 0, 0)),
            ..Default::default()
        };
        NanoleafProvider::new_for_test(server.uri(), "tok")
            .set_state("main", &state)
            .await
            .unwrap();
        let req = &server.received_requests().await.unwrap()[0];
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["on"]["value"], true);
        assert_eq!(body["brightness"]["value"], 50);
        // Red → hue 0, sat 100. No ct in a colour write.
        assert_eq!(body["hue"]["value"], 0);
        assert_eq!(body["sat"]["value"], 100);
        assert!(body.get("ct").is_none());
    }

    #[tokio::test]
    async fn set_state_white_writes_ct_kelvin_clamped() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/tok/state"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let state = LightState {
            on: true,
            color_temp_mirek: Some(250), // 4000K
            ..Default::default()
        };
        NanoleafProvider::new_for_test(server.uri(), "tok")
            .set_state("main", &state)
            .await
            .unwrap();
        let req = &server.received_requests().await.unwrap()[0];
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["ct"]["value"], 4000);
        assert!(body.get("hue").is_none());
    }

    #[tokio::test]
    async fn set_state_effect_selects_and_keeps_brightness() {
        // The "brightness during effect" contract: an effect write sets power +
        // brightness on /state, THEN selects the effect on /effects — brightness
        // stays valid, the effect is not clobbered.
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/tok/state"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/tok/effects"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let state = LightState {
            on: true,
            brightness: Some(35.0),
            effect: Some("Windmill".into()),
            ..Default::default()
        };
        NanoleafProvider::new_for_test(server.uri(), "tok")
            .set_state("main", &state)
            .await
            .unwrap();
        let reqs = server.received_requests().await.unwrap();
        assert_eq!(reqs.len(), 2, "one /state + one /effects");
        let state_req = reqs
            .iter()
            .find(|r| r.url.path().ends_with("/state"))
            .unwrap();
        let sbody: Value = serde_json::from_slice(&state_req.body).unwrap();
        assert_eq!(sbody["on"]["value"], true);
        assert_eq!(sbody["brightness"]["value"], 35);
        // The effect body carries the selected name, and no colour/ct is sent
        // (which would knock the controller out of effect mode).
        assert!(sbody.get("hue").is_none() && sbody.get("ct").is_none());
        let fx_req = reqs
            .iter()
            .find(|r| r.url.path().ends_with("/effects"))
            .unwrap();
        let fbody: Value = serde_json::from_slice(&fx_req.body).unwrap();
        assert_eq!(fbody["select"], "Windmill");
    }

    #[tokio::test]
    async fn set_state_off_powers_down_without_touching_effect() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/tok/state"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        // Even with an effect merged in, on:false must not re-select it (that
        // would power the controller back on).
        let state = LightState {
            on: false,
            effect: Some("Windmill".into()),
            ..Default::default()
        };
        NanoleafProvider::new_for_test(server.uri(), "tok")
            .set_state("main", &state)
            .await
            .unwrap();
        let reqs = server.received_requests().await.unwrap();
        assert_eq!(reqs.len(), 1, "only the power-off /state PUT");
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(body["on"]["value"], false);
    }

    // ── Pairing ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn pair_returns_token_when_in_pairing_mode() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/new"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "auth_token": "TOKEN123" })),
            )
            .mount(&server)
            .await;
        assert_eq!(
            pair(&server.uri()).await.unwrap(),
            PairOutcome::Paired {
                auth_token: "TOKEN123".into()
            }
        );
    }

    #[tokio::test]
    async fn pair_reports_not_in_pairing_mode_on_403() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/new"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        assert_eq!(
            pair(&server.uri()).await.unwrap(),
            PairOutcome::NotInPairingMode
        );
    }

    #[tokio::test]
    async fn pair_errors_on_unreachable_controller() {
        // Port 9 (discard) — nothing listening.
        assert!(pair("http://127.0.0.1:9").await.is_err());
    }

    // ── Unpaired provider + factory ─────────────────────────────────────────

    #[tokio::test]
    async fn unpaired_provider_gives_actionable_error() {
        let p = NanoleafProvider::new("192.168.1.20", None).unwrap();
        let err = p.discover().await.unwrap_err().to_string();
        assert!(
            err.contains("power button"),
            "actionable pairing hint: {err}"
        );
        // And a control write fails the same way (never silently no-ops).
        assert!(p.set_state("main", &LightState::default()).await.is_err());
    }

    #[test]
    fn hw_id_from_typical_serial_is_none_but_mac_shaped_matches() {
        // A normal Nanoleaf serial isn't a MAC → no auto de-dup key (the HA copy
        // is shadowed manually, since the Open API exposes no MAC).
        let info: NanoInfo = serde_json::from_value(info_json("hs", "*Solid*")).unwrap();
        assert!(info_to_light(info).hw_id.is_none());
    }

    #[test]
    fn factory_build_requires_host() {
        let f = NanoleafProviderFactory;
        assert_eq!(f.provider_type(), "nanoleaf");
        assert_eq!(f.display_name(), "Nanoleaf");
        // Host required; token optional.
        assert!(f.build("{}").is_err(), "missing host is rejected");
        assert!(
            f.build(r#"{"host":"192.168.1.20"}"#).is_ok(),
            "host only (unpaired)"
        );
        assert!(
            f.build(r#"{"host":"192.168.1.20","auth_token":"t"}"#)
                .is_ok(),
            "host + token"
        );
        let schema = f.credentials_schema();
        assert_eq!(schema.len(), 2);
        assert!(schema.iter().any(|c| c.name == "host" && c.required));
        assert!(schema.iter().any(|c| c.name == "auth_token" && !c.required));
    }

    #[tokio::test]
    async fn factory_built_provider_talks_to_the_controller() {
        // The factory build path reaches a live controller (mock) end to end.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tok/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(info_json("hs", "*Solid*")))
            .mount(&server)
            .await;
        let creds = json!({ "host": server.uri(), "auth_token": "tok" }).to_string();
        let provider = NanoleafProviderFactory.build(&creds).unwrap();
        assert_eq!(provider.name(), "nanoleaf");
        let lights = provider.discover().await.unwrap();
        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].name, "Shapes Studio");
    }
}
