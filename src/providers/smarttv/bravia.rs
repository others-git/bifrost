//! Sony **Bravia** smart-TV vendor — the first [`SmartTvVendor`] implementation.
//!
//! Bravia exposes the **ScalarWeb** JSON API (`POST http://<tv>/sony/<service>`
//! with `{"method","id","version","params"}`) for state + power + audio + apps,
//! and **IRCC** (a SOAP `X_SendIRCC` call) for remote key codes. Control is
//! authorised by a cookie obtained through **PIN pairing** ([`pairing`]): the
//! first `actRegister` makes the TV display a PIN, and a second `actRegister`
//! carrying that PIN (HTTP Basic) returns the `auth` cookie we store and replay.
//!
//! This whole file is the Bravia adapter; another brand is a sibling file
//! implementing the same trait. Nothing Sony-specific leaks into the framework.

use super::{SmartTvVendor, TvIdentity, TvSnapshot};
use crate::models::media::{NowPlaying, PlayState};
use crate::models::remote::{RemoteCommandInfo, RemoteKey};
use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{Value, json};
use std::time::Duration;

/// `http://<host>` (or the host verbatim if it already carries a scheme).
fn base_url(host: &str) -> String {
    crate::providers::base_url(host, "http", None)
}

pub(crate) struct BraviaVendor {
    base: String,
    /// The bare host/address (no scheme) — surfaced as the device IP.
    ip: String,
    /// The PIN-pairing `auth` cookie value, replayed as `Cookie: auth=…`.
    auth: Option<String>,
    /// Android TV Remote v2 identity (self-signed client cert), once the remote
    /// is paired. When present, key presses go over ATV Remote instead of IRCC —
    /// required for Android/Google TV Bravias, which no longer expose `/sony/IRCC`.
    atv: Option<super::atv::crypto::Identity>,
    client: Client,
}

/// Strip the scheme to get the bare host/address.
fn bare_host(base: &str) -> String {
    base.trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_string()
}

impl BraviaVendor {
    pub(crate) fn new(
        host: &str,
        auth: Option<String>,
        atv: Option<super::atv::crypto::Identity>,
    ) -> Result<Self> {
        if host.trim().is_empty() {
            bail!("bravia: empty host");
        }
        let client = Client::builder().timeout(Duration::from_secs(5)).build()?;
        let base = base_url(host);
        Ok(Self {
            ip: bare_host(&base),
            base,
            auth,
            atv,
            client,
        })
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(base: &str, auth: Option<String>) -> Self {
        let base = base.trim_end_matches('/').to_string();
        Self {
            ip: bare_host(&base),
            base,
            auth,
            atv: None,
            client: Client::builder().build().unwrap(),
        }
    }

    /// One ScalarWeb call; returns the `result` value (usually an array). Errors
    /// on a transport failure or an `error` body.
    async fn scalar(
        &self,
        service: &str,
        method: &str,
        version: &str,
        params: Value,
    ) -> Result<Value> {
        let body = json!({ "method": method, "id": 1, "version": version, "params": params });
        tracing::debug!(target: "bifrost::smarttv", base = %self.base, service, method, params = %body["params"], "bravia scalar →");
        let mut req = self
            .client
            .post(format!("{}/sony/{service}", self.base))
            .json(&body);
        if let Some(c) = &self.auth {
            req = req.header(reqwest::header::COOKIE, format!("auth={c}"));
        }
        let resp = req.send().await.map_err(|e| {
            tracing::debug!(target: "bifrost::smarttv", method, "bravia transport error: {e}");
            e
        })?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .await
            .map_err(|e| anyhow!("bravia {method}: bad response ({e})"))?;
        if let Some(err) = v.get("error") {
            tracing::debug!(target: "bifrost::smarttv", method, %status, ?err, "bravia error response");
            bail!("bravia {method}: {err}");
        }
        if !status.is_success() {
            tracing::debug!(target: "bifrost::smarttv", method, %status, "bravia non-success status");
            bail!("bravia {method}: status {status}");
        }
        let result = v.get("result").cloned().unwrap_or(Value::Null);
        tracing::debug!(target: "bifrost::smarttv", method, "bravia scalar ✓");
        Ok(result)
    }

    /// Send one IRCC key code via the SOAP `X_SendIRCC` action.
    async fn ircc(&self, code: &str) -> Result<()> {
        let body = format!(
            r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:X_SendIRCC xmlns:u="urn:schemas-sony-com:service:IRCC:1"><IRCCCode>{code}</IRCCCode></u:X_SendIRCC></s:Body></s:Envelope>"#
        );
        let mut req = self
            .client
            .post(format!("{}/sony/IRCC", self.base))
            .header(reqwest::header::CONTENT_TYPE, "text/xml; charset=UTF-8")
            .header(
                "SOAPACTION",
                "\"urn:schemas-sony-com:service:IRCC:1#X_SendIRCC\"",
            )
            .body(body);
        if let Some(c) = &self.auth {
            req = req.header(reqwest::header::COOKIE, format!("auth={c}"));
        }
        tracing::debug!(target: "bifrost::smarttv", base = %self.base, code, "bravia IRCC →");
        let resp = req.send().await?;
        if !resp.status().is_success() {
            tracing::debug!(target: "bifrost::smarttv", code, status = %resp.status(), "bravia IRCC failed");
            bail!("bravia IRCC {code}: status {}", resp.status());
        }
        Ok(())
    }

    async fn power_status(&self) -> Result<bool> {
        let r = self
            .scalar("system", "getPowerStatus", "1.0", json!([]))
            .await?;
        let status = r
            .get(0)
            .and_then(|x| x.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("");
        Ok(status == "active")
    }

    async fn volume_info(&self) -> Result<(u8, bool)> {
        let r = self
            .scalar("audio", "getVolumeInformation", "1.0", json!([]))
            .await?;
        let targets = r
            .get(0)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        // Prefer the "speaker" target; fall back to the first reported one.
        let t = targets
            .iter()
            .find(|t| t.get("target").and_then(Value::as_str) == Some("speaker"))
            .or_else(|| targets.first());
        let volume = t
            .and_then(|t| t.get("volume"))
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .clamp(0, 100) as u8;
        let mute = t
            .and_then(|t| t.get("mute"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok((volume, mute))
    }

    /// External inputs (HDMI, etc.) as `(friendly title, uri)` pairs.
    async fn external_inputs(&self) -> Result<Vec<(String, String)>> {
        let r = self
            .scalar(
                "avContent",
                "getCurrentExternalInputsStatus",
                "1.0",
                json!([]),
            )
            .await?;
        let list = r
            .get(0)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(list
            .iter()
            .filter_map(|i| {
                let uri = i.get("uri").and_then(Value::as_str)?;
                let title = i
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(uri);
                Some((title.to_string(), uri.to_string()))
            })
            .collect())
    }

    /// Current source + now-playing title; `(None, None)` when nothing is playing
    /// (the TV returns an error in that state, which the caller tolerates).
    async fn playing_content(&self) -> Result<(Option<String>, Option<NowPlaying>)> {
        let r = self
            .scalar("avContent", "getPlayingContentInfo", "1.0", json!([]))
            .await?;
        let info = r.get(0);
        let source = info
            .and_then(|i| i.get("source"))
            .and_then(Value::as_str)
            .map(String::from);
        let title = info
            .and_then(|i| i.get("title").or_else(|| i.get("programTitle")))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from);
        let now = title.map(|t| NowPlaying {
            title: Some(t),
            artist: None,
            album: None,
            play_state: Some(PlayState::Playing),
        });
        Ok((source, now))
    }
}

#[async_trait]
impl SmartTvVendor for BraviaVendor {
    fn brand(&self) -> &'static str {
        "Sony Bravia"
    }

    async fn identity(&self) -> Result<TvIdentity> {
        let r = self
            .scalar("system", "getSystemInformation", "1.0", json!([]))
            .await?;
        let info = r.get(0);
        let name = info
            .and_then(|i| i.get("name"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| info.and_then(|i| i.get("model")).and_then(Value::as_str))
            .unwrap_or("Sony Bravia")
            .to_string();
        let hw_id = info
            .and_then(|i| i.get("macAddr"))
            .and_then(Value::as_str)
            .and_then(crate::providers::mac_hw_id);
        Ok(TvIdentity { name, hw_id })
    }

    async fn snapshot(&self) -> Result<TvSnapshot> {
        // Power is the liveness probe: a failure means unreachable; standby means
        // reachable-but-off (and the other queries would error, so skip them).
        let power = match self.power_status().await {
            Ok(p) => p,
            Err(_) => {
                return Ok(TvSnapshot {
                    reachable: false,
                    power: false,
                    volume: 0,
                    mute: false,
                    source: None,
                    sources: Vec::new(),
                    current_app: None,
                    now_playing: None,
                    ip: Some(self.ip.clone()),
                });
            }
        };
        if !power {
            return Ok(TvSnapshot {
                reachable: true,
                power: false,
                volume: 0,
                mute: false,
                source: None,
                sources: Vec::new(),
                current_app: None,
                now_playing: None,
                ip: Some(self.ip.clone()),
            });
        }
        let (volume, mute) = self.volume_info().await.unwrap_or((0, false));
        let (source, now_playing) = self.playing_content().await.unwrap_or((None, None));
        // External inputs as the selectable source list (best-effort).
        let sources = self
            .external_inputs()
            .await
            .map(|v| v.into_iter().map(|(t, _)| t).collect())
            .unwrap_or_default();
        Ok(TvSnapshot {
            reachable: true,
            power: true,
            volume,
            mute,
            source,
            sources,
            current_app: None,
            now_playing,
            ip: Some(self.ip.clone()),
        })
    }

    async fn set_power(&self, on: bool) -> Result<()> {
        self.scalar("system", "setPowerStatus", "1.0", json!([{ "status": on }]))
            .await
            .map(|_| ())
    }

    async fn set_volume(&self, percent: u8) -> Result<()> {
        // Bravia takes the volume as a string.
        self.scalar(
            "audio",
            "setAudioVolume",
            "1.0",
            json!([{ "target": "speaker", "volume": percent.min(100).to_string() }]),
        )
        .await
        .map(|_| ())
    }

    async fn set_mute(&self, mute: bool) -> Result<()> {
        self.scalar("audio", "setAudioMute", "1.0", json!([{ "status": mute }]))
            .await
            .map(|_| ())
    }

    async fn set_source(&self, source: &str) -> Result<()> {
        // Resolve a friendly input title (what `snapshot().sources` reports) to its
        // uri; a value that already looks like a uri passes through unchanged.
        let uri = if source.contains(':') {
            source.to_string()
        } else {
            self.external_inputs()
                .await
                .ok()
                .and_then(|inputs| {
                    inputs
                        .into_iter()
                        .find(|(t, _)| t.eq_ignore_ascii_case(source))
                        .map(|(_, u)| u)
                })
                .unwrap_or_else(|| source.to_string())
        };
        self.scalar(
            "avContent",
            "setPlayContent",
            "1.0",
            json!([{ "uri": uri }]),
        )
        .await
        .map(|_| ())
    }

    async fn send_key(&self, key: RemoteKey) -> Result<()> {
        // Android/Google TV Bravias dropped the `/sony/IRCC` SOAP endpoint; once
        // the ATV Remote is paired, route keys there. IRCC remains the path for
        // older (pre-Android) Bravias that never paired an ATV identity.
        if let Some(id) = &self.atv {
            return super::atv::client::send_key(&self.ip, id, super::atv::android_keycode(key))
                .await;
        }
        self.ircc(ircc_code(key)).await
    }

    async fn launch_app(&self, app: &str) -> Result<()> {
        self.scalar("appControl", "setActiveApp", "1.0", json!([{ "uri": app }]))
            .await
            .map(|_| ())
    }

    async fn send_text(&self, text: &str) -> Result<()> {
        // Android/Google TV Bravias drop `appControl.setTextForm` (it returns
        // ScalarWeb error 12, "no such method"), so once the ATV Remote is paired
        // type via key-event injection over that channel. ScalarWeb setTextForm
        // stays the path for older pre-Android Bravias that still support it.
        if let Some(id) = &self.atv {
            return super::atv::client::send_text(&self.ip, id, text).await;
        }
        self.scalar("appControl", "setTextForm", "1.0", json!([text]))
            .await
            .map(|_| ())
    }

    async fn commands(&self) -> Result<Vec<RemoteCommandInfo>> {
        // result = [<meta>, [{ "name": "Power", "value": "AAAA…" }, …]] — the TV's
        // full IRCC catalogue. The token we replay is the IRCC code itself.
        let r = self
            .scalar("system", "getRemoteControllerInfo", "1.0", json!([]))
            .await?;
        let list = r
            .get(1)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(list
            .iter()
            .filter_map(|c| {
                let name = c.get("name").and_then(Value::as_str)?;
                let value = c.get("value").and_then(Value::as_str)?;
                Some(RemoteCommandInfo {
                    name: name.to_string(),
                    token: value.to_string(),
                    ..Default::default()
                })
            })
            .collect())
    }

    async fn send_command(&self, token: &str) -> Result<()> {
        self.ircc(token).await
    }
}

/// Sony's published IRCC code for each canonical key. `Power` is the TV-power
/// *toggle*; deliberate on/off goes through ScalarWeb `setPowerStatus` instead.
fn ircc_code(key: RemoteKey) -> &'static str {
    match key {
        RemoteKey::Up => "AAAAAQAAAAEAAAB0Aw==",
        RemoteKey::Down => "AAAAAQAAAAEAAAB1Aw==",
        RemoteKey::Left => "AAAAAQAAAAEAAAA0Aw==",
        RemoteKey::Right => "AAAAAQAAAAEAAAAzAw==",
        RemoteKey::Select => "AAAAAQAAAAEAAABlAw==",
        RemoteKey::Back => "AAAAAgAAAJcAAAAjAw==",
        RemoteKey::Home => "AAAAAQAAAAEAAABgAw==",
        RemoteKey::Menu => "AAAAAgAAAJcAAAA2Aw==",
        RemoteKey::VolumeUp => "AAAAAQAAAAEAAAASAw==",
        RemoteKey::VolumeDown => "AAAAAQAAAAEAAAATAw==",
        RemoteKey::Mute => "AAAAAQAAAAEAAAAUAw==",
        RemoteKey::PlayPause => "AAAAAgAAAJcAAAAaAw==",
        RemoteKey::Next => "AAAAAgAAAJcAAAA9Aw==",
        RemoteKey::Previous => "AAAAAgAAAJcAAAA8Aw==",
        RemoteKey::Power => "AAAAAQAAAAEAAAAVAw==",
    }
}

/// Bravia PIN pairing — the two-step `actRegister` dance that yields the `auth`
/// cookie stored as the provider's credential.
pub(crate) mod pairing {
    use super::{Result, anyhow, bail, base_url, json};
    use reqwest::Client;
    use std::time::Duration;

    /// Stable client id so a re-pair reuses the TV's existing grant.
    const CLIENT_ID: &str = "Bifrost:bifrost-hub";

    pub(crate) enum PairOutcome {
        /// The TV is now showing a PIN; call [`complete`] with it.
        PinDisplayed,
        /// Already authorised (some firmwares) — the auth cookie.
        Paired(String),
    }

    fn act_register_body() -> serde_json::Value {
        json!({
            "method": "actRegister",
            "id": 8,
            "version": "1.0",
            "params": [
                { "clientid": CLIENT_ID, "nickname": "Bifrost", "level": "private" },
                [ { "value": "yes", "function": "WOL" } ]
            ]
        })
    }

    async fn act_register(host: &str, pin: Option<&str>) -> Result<reqwest::Response> {
        let client = Client::builder().timeout(Duration::from_secs(10)).build()?;
        let mut req = client
            .post(format!("{}/sony/accessControl", base_url(host)))
            .json(&act_register_body());
        if let Some(pin) = pin {
            req = req.basic_auth("", Some(pin)); // username empty, password = PIN
        }
        Ok(req.send().await?)
    }

    /// Pull the `auth` cookie value out of a `Set-Cookie` header.
    fn auth_cookie(resp: &reqwest::Response) -> Option<String> {
        for hv in resp.headers().get_all(reqwest::header::SET_COOKIE) {
            let Ok(s) = hv.to_str() else { continue };
            for part in s.split(';') {
                if let Some(v) = part.trim().strip_prefix("auth=")
                    && !v.is_empty()
                {
                    return Some(v.to_string());
                }
            }
        }
        None
    }

    /// Step 1: ask the TV to register us. It pops a PIN and answers `401`.
    pub(crate) async fn begin(host: &str) -> Result<PairOutcome> {
        tracing::debug!(target: "bifrost::smarttv", host, "bravia pairing: begin (actRegister)");
        let resp = act_register(host, None).await?;
        let status = resp.status();
        match status {
            reqwest::StatusCode::UNAUTHORIZED => {
                tracing::debug!(target: "bifrost::smarttv", host, "bravia pairing: TV displaying PIN (401)");
                Ok(PairOutcome::PinDisplayed)
            }
            s if s.is_success() => match auth_cookie(&resp) {
                Some(c) => {
                    tracing::debug!(target: "bifrost::smarttv", host, "bravia pairing: already authorised (cookie returned)");
                    Ok(PairOutcome::Paired(c))
                }
                None => Ok(PairOutcome::PinDisplayed),
            },
            s => {
                tracing::warn!(target: "bifrost::smarttv", host, %s, "bravia pairing: unexpected status");
                bail!("unexpected Bravia pairing status: {s}")
            }
        }
    }

    /// Step 2: resubmit with the on-screen PIN; returns the stored `auth` cookie.
    pub(crate) async fn complete(host: &str, pin: &str) -> Result<String> {
        tracing::debug!(target: "bifrost::smarttv", host, "bravia pairing: submitting PIN");
        let resp = act_register(host, Some(pin)).await?;
        let status = resp.status();
        if !status.is_success() {
            tracing::debug!(target: "bifrost::smarttv", host, %status, "bravia pairing: PIN rejected");
            bail!("Bravia rejected the PIN (status {status})");
        }
        let cookie = auth_cookie(&resp).ok_or_else(|| anyhow!("Bravia returned no auth cookie"))?;
        tracing::info!(target: "bifrost::smarttv", host, "bravia pairing: complete (auth cookie stored)");
        Ok(cookie)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ok_result(body: serde_json::Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(body)
    }

    #[tokio::test]
    async fn snapshot_reads_power_and_volume() {
        let tv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sony/system"))
            .and(body_string_contains("getPowerStatus"))
            .respond_with(ok_result(
                json!({ "result": [{ "status": "active" }], "id": 1 }),
            ))
            .mount(&tv)
            .await;
        Mock::given(method("POST"))
            .and(path("/sony/audio"))
            .and(body_string_contains("getVolumeInformation"))
            .respond_with(ok_result(
                json!({ "result": [[{ "target": "speaker", "volume": 30, "mute": false }]], "id": 1 }),
            ))
            .mount(&tv)
            .await;
        // No avContent mock: getPlayingContentInfo 404s → tolerated.

        let v = BraviaVendor::new_for_test(&tv.uri(), Some("cookie".into()));
        let s = v.snapshot().await.unwrap();
        assert!(s.reachable && s.power);
        assert_eq!(s.volume, 30);
        assert!(!s.mute);
        // The TV's host is surfaced as its IP.
        assert!(s.ip.as_deref().unwrap_or("").contains("127.0.0.1"));
    }

    #[tokio::test]
    async fn identity_reads_name_and_normalizes_mac() {
        let tv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sony/system"))
            .and(body_string_contains("getSystemInformation"))
            .respond_with(ok_result(json!({
                "result": [{ "name": "BRAVIA", "model": "XR-55", "macAddr": "AA:BB:CC:DD:EE:FF" }],
                "id": 1
            })))
            .mount(&tv)
            .await;

        let v = BraviaVendor::new_for_test(&tv.uri(), None);
        let id = v.identity().await.unwrap();
        assert_eq!(id.name, "BRAVIA");
        assert_eq!(id.hw_id.as_deref(), Some("mac:aabbccddeeff"));
    }

    #[tokio::test]
    async fn set_power_posts_scalar_command() {
        let tv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sony/system"))
            .and(body_string_contains("setPowerStatus"))
            .and(body_string_contains("\"status\":true"))
            .respond_with(ok_result(json!({ "result": [], "id": 1 })))
            .mount(&tv)
            .await;

        let v = BraviaVendor::new_for_test(&tv.uri(), Some("c".into()));
        v.set_power(true).await.unwrap();
    }

    #[tokio::test]
    async fn send_key_posts_ircc_code() {
        let tv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sony/IRCC"))
            .and(body_string_contains(ircc_code(RemoteKey::Up)))
            .respond_with(ResponseTemplate::new(200))
            .mount(&tv)
            .await;

        let v = BraviaVendor::new_for_test(&tv.uri(), Some("c".into()));
        v.send_key(RemoteKey::Up).await.unwrap();
    }

    #[tokio::test]
    async fn commands_lists_the_ircc_catalogue() {
        let tv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sony/system"))
            .and(body_string_contains("getRemoteControllerInfo"))
            .respond_with(ok_result(json!({
                "result": [
                    { "bundled": true },
                    [ { "name": "Num1", "value": "AAAAAQAAAAEAAAAAAw==" },
                      { "name": "Input", "value": "AAAAAQAAAAEAAAAlAw==" } ]
                ]
            })))
            .mount(&tv)
            .await;
        let v = BraviaVendor::new_for_test(&tv.uri(), Some("c".into()));
        let cmds = v.commands().await.unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].name, "Num1");
        assert!(cmds[1].token.starts_with("AAAA"));
    }

    #[tokio::test]
    async fn send_command_posts_the_native_ircc_token() {
        let tv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sony/IRCC"))
            .and(body_string_contains("MYTOKEN"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&tv)
            .await;
        let v = BraviaVendor::new_for_test(&tv.uri(), Some("c".into()));
        v.send_command("MYTOKEN").await.unwrap();
    }

    #[tokio::test]
    async fn send_text_posts_settextform() {
        let tv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sony/appControl"))
            .and(body_string_contains("setTextForm"))
            .and(body_string_contains("hello world"))
            .respond_with(ok_result(json!({ "result": [0] })))
            .mount(&tv)
            .await;
        let v = BraviaVendor::new_for_test(&tv.uri(), Some("c".into()));
        v.send_text("hello world").await.unwrap();
    }

    #[tokio::test]
    async fn set_source_resolves_friendly_title_to_uri() {
        let tv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sony/avContent"))
            .and(body_string_contains("getCurrentExternalInputsStatus"))
            .respond_with(ok_result(
                json!({ "result": [[{ "uri": "extInput:hdmi?port=1", "title": "HDMI 1" }]] }),
            ))
            .mount(&tv)
            .await;
        // setPlayContent must receive the *resolved* uri, not the friendly title.
        Mock::given(method("POST"))
            .and(path("/sony/avContent"))
            .and(body_string_contains("setPlayContent"))
            .and(body_string_contains("extInput:hdmi?port=1"))
            .respond_with(ok_result(json!({ "result": [] })))
            .mount(&tv)
            .await;

        let v = BraviaVendor::new_for_test(&tv.uri(), Some("c".into()));
        v.set_source("HDMI 1").await.unwrap();
    }

    #[tokio::test]
    async fn pairing_begin_reports_pin_displayed_on_401() {
        let tv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sony/accessControl"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&tv)
            .await;

        assert!(matches!(
            pairing::begin(&tv.uri()).await.unwrap(),
            pairing::PairOutcome::PinDisplayed
        ));
    }

    #[tokio::test]
    async fn pairing_complete_returns_auth_cookie() {
        let tv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sony/accessControl"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Set-Cookie", "auth=SECRET123; Path=/sony/; Max-Age=1209600")
                    .set_body_json(json!({ "result": [], "id": 8 })),
            )
            .mount(&tv)
            .await;

        let cookie = pairing::complete(&tv.uri(), "1234").await.unwrap();
        assert_eq!(cookie, "SECRET123");
    }
}
