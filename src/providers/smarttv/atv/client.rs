//! TLS transport for Android TV Remote v2: the one-time pairing handshake
//! (port 6467) and the remote key channel (port 6466).
//!
//! Both channels are length-delimited protobuf over TLS, authenticated by our
//! self-signed client cert ([`super::crypto::Identity`]). The TV's own cert is
//! self-signed too, so server verification is replaced with a trust-on-first-use
//! verifier — the pairing-secret exchange (not the PKI) is what proves identity.

use super::crypto::{self, Identity};
use super::messages::{self, PairingIn, RemoteIn};
use super::wire::{self, FrameReader};
use anyhow::{Result, anyhow, bail};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

const PAIR_PORT: u16 = 6467;
pub(crate) const REMOTE_PORT: u16 = 6466;
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// rustls `ServerCertVerifier` that trusts any certificate (TOFU). Signature
/// checks still run via the ring provider so the handshake stays well-formed.
#[derive(Debug)]
struct TofuVerifier(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Open a TLS connection to `host:port` presenting our client cert.
///
/// Protocol version is firmware roulette: Sony Bravias reject a TLS 1.3 +
/// RSA-client-cert handshake (`IllegalParameter`) and want 1.2, while Google
/// TV dongles reject 1.2 outright (`handshake_failure` alert) and require
/// 1.3. Try 1.2 first (the known-good path for the installed base), then
/// retry the handshake with 1.3 — one extra round-trip only on 1.3-only
/// devices, and only on (re)connect.
async fn connect(host: &str, port: u16, identity: &Identity) -> Result<TlsStream<TcpStream>> {
    let mut last_err = anyhow!("no TLS attempt made");
    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
        match connect_with(host, port, identity, version).await {
            Ok(s) => return Ok(s),
            Err(e) => {
                tracing::debug!(target: "bifrost::smarttv", host, port, ?version, "ATV: TLS attempt failed: {e:#}");
                last_err = e;
            }
        }
    }
    Err(last_err)
}

async fn connect_with(
    host: &str,
    port: u16,
    identity: &Identity,
    version: &'static rustls::SupportedProtocolVersion,
) -> Result<TlsStream<TcpStream>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cert = rustls::pki_types::CertificateDer::from(identity.cert_der()?);
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(identity.key_der()?.into());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[version])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(TofuVerifier(provider)))
        .with_client_auth_cert(vec![cert], key)?;

    tracing::debug!(target: "bifrost::smarttv", host, port, "ATV: TCP connecting");
    let tcp = tokio::time::timeout(IO_TIMEOUT, TcpStream::connect((host, port)))
        .await
        .map_err(|_| anyhow!("timed out connecting to {host}:{port}"))??;
    // The verifier ignores the name, but rustls still requires one.
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string()).unwrap_or(
        rustls::pki_types::ServerName::IpAddress(std::net::Ipv4Addr::LOCALHOST.into()),
    );
    let stream = TlsConnector::from(Arc::new(config))
        .connect(server_name, tcp)
        .await
        .map_err(|e| anyhow!("TLS handshake with {host}:{port} failed: {e}"))?;
    tracing::debug!(target: "bifrost::smarttv", host, port, "ATV: TLS established");
    Ok(stream)
}

/// The DER of the peer's leaf certificate from a completed handshake.
fn peer_cert(stream: &TlsStream<TcpStream>) -> Result<Vec<u8>> {
    let (_, conn) = stream.get_ref();
    conn.peer_certificates()
        .and_then(|c| c.first())
        .map(|c| c.as_ref().to_vec())
        .ok_or_else(|| anyhow!("TV presented no certificate"))
}

/// Write one length-delimited frame.
async fn write_msg(stream: &mut TlsStream<TcpStream>, msg: &[u8]) -> Result<()> {
    stream.write_all(&wire::frame(msg)).await?;
    stream.flush().await?;
    Ok(())
}

/// Read the next complete frame, refilling from the socket as needed.
async fn read_msg(stream: &mut TlsStream<TcpStream>, reader: &mut FrameReader) -> Result<Vec<u8>> {
    loop {
        if let Some(frame) = reader.next_frame() {
            return Ok(frame);
        }
        let mut buf = [0u8; 4096];
        let n = tokio::time::timeout(IO_TIMEOUT, stream.read(&mut buf))
            .await
            .map_err(|_| anyhow!("timed out reading from TV"))??;
        if n == 0 {
            bail!("TV closed the connection");
        }
        reader.push(&buf[..n]);
    }
}

/// A live pairing handshake paused at the point where the TV is showing its
/// 6-digit code, holding the open connection so [`finish`](Self::finish) can
/// send the secret on the *same* socket.
pub struct PairingSession {
    stream: TlsStream<TcpStream>,
    reader: FrameReader,
    client_cert_der: Vec<u8>,
    server_cert_der: Vec<u8>,
}

impl PairingSession {
    /// Drive pairing up to the code prompt: request → option → configuration.
    /// On success the TV is now displaying the code.
    pub async fn begin(host: &str, identity: &Identity, client_name: &str) -> Result<Self> {
        tracing::debug!(target: "bifrost::smarttv", host, "ATV pairing: begin");
        let mut stream = connect(host, PAIR_PORT, identity).await?;
        let server_cert_der = peer_cert(&stream)?;
        let client_cert_der = identity.cert_der()?;
        let mut reader = FrameReader::default();

        write_msg(&mut stream, &messages::pairing_request(client_name)).await?;
        expect(
            &read_msg(&mut stream, &mut reader).await?,
            PairingIn::RequestAck,
        )?;
        tracing::debug!(target: "bifrost::smarttv", host, "ATV pairing: request acked");

        write_msg(&mut stream, &messages::pairing_option()).await?;
        expect(
            &read_msg(&mut stream, &mut reader).await?,
            PairingIn::Options,
        )?;

        write_msg(&mut stream, &messages::pairing_configuration()).await?;
        expect(
            &read_msg(&mut stream, &mut reader).await?,
            PairingIn::ConfigurationAck,
        )?;
        tracing::debug!(target: "bifrost::smarttv", host, "ATV pairing: configured — TV is showing the code");

        Ok(PairingSession {
            stream,
            reader,
            client_cert_der,
            server_cert_der,
        })
    }

    /// Send the secret derived from `code` and await the TV's acknowledgement.
    /// Consumes the session (the connection closes afterwards).
    pub async fn finish(mut self, code: &str) -> Result<()> {
        let secret = crypto::pairing_secret(&self.client_cert_der, &self.server_cert_der, code)?;
        tracing::debug!(target: "bifrost::smarttv", "ATV pairing: secret computed, sending");
        write_msg(&mut self.stream, &messages::pairing_secret(&secret)).await?;
        expect(
            &read_msg(&mut self.stream, &mut self.reader).await?,
            PairingIn::SecretAck,
        )?;
        tracing::info!(target: "bifrost::smarttv", "ATV pairing: complete (cert trusted)");
        Ok(())
    }
}

/// Map a parsed pairing reply onto the step we expected, turning a non-OK status
/// into a descriptive error.
fn expect(body: &[u8], want: PairingIn) -> Result<()> {
    match messages::parse_pairing(body) {
        got if got == want => Ok(()),
        PairingIn::Other(402) => bail!("TV rejected the pairing code (bad secret)"),
        PairingIn::Other(status) => bail!("TV pairing error (status {status})"),
        other => bail!("unexpected pairing reply: {other:?} (wanted {want:?})"),
    }
}

/// A state push from the TV on the remote channel — the Android TV Remote v2
/// session isn't just an input pipe: the TV volunteers its foreground app,
/// absolute volume, and screen state on it. These are the only push source for
/// "which app is on screen" (ScalarWeb has no foreground getter and its
/// now-playing API errors whenever an app owns the screen).
#[derive(Debug, Clone, PartialEq)]
pub enum AtvEvent {
    /// Foreground app package (e.g. `com.netflix.ninja`).
    CurrentApp(String),
    /// Absolute device volume.
    Volume { level: u32, max: u32, muted: bool },
    /// Screen on/off.
    Started(bool),
}

/// A persistent remote-channel connection to one TV. Android TV Remote injects
/// only register on a **live, kept-open** session — the TV drops a key whose
/// connection tears down in the same instant — so, like the Onkyo link, one
/// background task owns the socket: it completes the configure/set-active
/// handshake, answers keepalive pings, writes key injects sent on `tx`, and
/// broadcasts the TV's state pushes on `events`. It reconnects with backoff
/// and is shared per host — key sends and push subscribers use one session.
/// The TV's last-pushed state, accumulated on the link — the snapshot source
/// for vendors with no query protocol (a Google TV dongle has no ScalarWeb;
/// the ATV channel is push-only and front-loads state on connect).
#[derive(Debug, Clone, Default)]
pub struct AtvStateCache {
    pub screen_on: Option<bool>,
    pub volume_level: Option<u32>,
    pub volume_max: Option<u32>,
    pub muted: Option<bool>,
    pub current_app: Option<String>,
}

struct RemoteLink {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    events: broadcast::Sender<AtvEvent>,
    state: Arc<Mutex<AtvStateCache>>,
    /// Fingerprint of the identity this link's actor holds — a re-pair mints a
    /// new cert, and the running actor must not keep presenting the old one.
    identity_fp: u64,
    /// Set when a fresher link replaces this one; the actor exits its retry
    /// loop instead of spinning forever with retired credentials.
    retired: Arc<std::sync::atomic::AtomicBool>,
    /// Set once the TV rejects our pairing cert (`CertificateUnknown`): the
    /// session can never connect, so an enqueued command would be silently
    /// dropped. Send paths check this and fail loudly ("re-pair the remote")
    /// instead of reporting a phantom success. Cleared by a healthy session.
    auth_failed: Arc<std::sync::atomic::AtomicBool>,
}

fn remote_links() -> &'static Mutex<HashMap<String, Arc<RemoteLink>>> {
    static M: OnceLock<Mutex<HashMap<String, Arc<RemoteLink>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

fn identity_fp(identity: &Identity) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    identity.cert_pem.hash(&mut h);
    identity.key_pem.hash(&mut h);
    h.finish()
}

/// Get (or lazily start) the shared link for `host`. A caller presenting a
/// DIFFERENT identity than the running link (the remote was re-paired, so the
/// provider rebuilt with fresh `atv_cert`/`atv_key`) retires the old link and
/// starts a new one — a re-pair takes effect live, no restart needed.
fn link_for(host: &str, identity: &Identity) -> Arc<RemoteLink> {
    let fp = identity_fp(identity);
    let mut map = remote_links().lock().expect("remote link map poisoned");
    if let Some(link) = map.get(host) {
        if link.identity_fp == fp {
            return link.clone();
        }
        tracing::info!(target: "bifrost::smarttv", host, "ATV remote: credentials changed — restarting the link");
        link.retired
            .store(true, std::sync::atomic::Ordering::Relaxed);
        map.remove(host);
    }
    let (tx, rx) = mpsc::unbounded_channel();
    let (events, _) = broadcast::channel(64);
    let retired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let auth_failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let state = Arc::new(Mutex::new(AtvStateCache::default()));
    tokio::spawn(remote_link_actor(
        host.to_string(),
        identity.clone(),
        rx,
        events.clone(),
        Arc::clone(&retired),
        Arc::clone(&auth_failed),
        Arc::clone(&state),
    ));
    let link = Arc::new(RemoteLink {
        tx,
        events,
        state,
        identity_fp: fp,
        retired,
        auth_failed,
    });
    map.insert(host.to_string(), link.clone());
    link
}

/// Subscribe to the TV's state pushes (foreground app / volume / screen),
/// lazily starting the persistent link. The receiver survives reconnects —
/// the link owns the socket lifecycle, the channel never closes.
pub fn subscribe(host: &str, identity: &Identity) -> broadcast::Receiver<AtvEvent> {
    link_for(host, identity).events.subscribe()
}

/// The TV's last-pushed state (lazily starting the link). The ATV channel
/// front-loads screen/volume pushes on connect, so this fills within a
/// moment of first use; fields stay `None` until the TV has spoken.
pub fn cached_state(host: &str, identity: &Identity) -> AtvStateCache {
    link_for(host, identity)
        .state
        .lock()
        .expect("atv state cache poisoned")
        .clone()
}

/// Queue a pre-built RemoteMessage body (key inject, app-link launch, …) for
/// delivery on the persistent session.
pub fn send_message(host: &str, identity: &Identity, body: Vec<u8>) -> Result<()> {
    let link = link_for(host, identity);
    // Don't enqueue into a session the TV rejects — the message would be
    // silently dropped and the caller told it succeeded. Fail with the fix.
    if link.auth_failed.load(std::sync::atomic::Ordering::Relaxed) {
        anyhow::bail!(
            "the TV rejected this remote's pairing — re-pair it (Settings → Smart TV → Pair remote)"
        );
    }
    link.tx
        .send(body)
        .map_err(|_| anyhow!("remote link for {host} is closed"))
}

/// Speak into the TV's own voice assistant: open a voice session, stream the
/// audio (8 kHz mono 16-bit LE PCM — [`crate::audio::wav_to_atv_voice_pcm`]) in
/// ≤20 KB `RemoteVoicePayload` chunks, then close it. The TV pops its Assistant
/// overlay and runs the utterance. Enqueued on the shared persistent session,
/// so it composes with keys and never opens a second connection. Paced ~roughly
/// real-time so the TV's audio buffer doesn't overrun.
pub async fn send_voice(host: &str, identity: &Identity, pcm_8k_mono_le: &[u8]) -> Result<()> {
    // A random session id per utterance (keeps overlapping requests distinct).
    let session_id = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(1)
        & 0x7fff_ffff) as i32;
    // NOTE: on-device probing (Chromecast w/ Google TV) shows the TV ACCEPTS
    // the voice session (acks RemoteVoicePayload) but does NOT surface the
    // Assistant overlay from network voice — no keycode (ASSIST/VOICE_ASSIST/
    // SEARCH) opened it either. Sending voice over ATV v2 to trigger the
    // Assistant is undocumented and unproven; this path is the protocol-correct
    // minimal (begin/payload/end) and works if a firmware/device does surface
    // it. See docs/composite-devices or the atv_say_probe for the investigation.
    send_message(host, identity, messages::remote_voice_begin(session_id))?;
    tracing::debug!(target: "bifrost::smarttv", host, session_id, bytes = pcm_8k_mono_le.len(), "ATV voice: begin");
    // 20 KB chunks = 10 000 samples = 1.25 s of 8 kHz audio; pace close to that
    // so we don't fire the whole clip in a burst.
    for chunk in pcm_8k_mono_le.chunks(20_000) {
        send_message(
            host,
            identity,
            messages::remote_voice_payload(session_id, chunk),
        )?;
        let secs = chunk.len() as f64 / 2.0 / crate::audio::ATV_VOICE_RATE as f64;
        tokio::time::sleep(Duration::from_secs_f64(secs * 0.85)).await;
    }
    send_message(host, identity, messages::remote_voice_end(session_id))?;
    tracing::debug!(target: "bifrost::smarttv", host, session_id, "ATV voice: end");
    Ok(())
}

/// Queue a key for the TV at `host`, lazily (re)starting its persistent remote
/// link. Fire-and-forget: returns once enqueued (the link owns delivery), so a
/// rapid key sequence pipelines over one open session instead of paying a full
/// TLS + handshake per press.
pub async fn send_key(host: &str, identity: &Identity, key_code: u32) -> Result<()> {
    tracing::debug!(target: "bifrost::smarttv", host, key_code, "ATV remote: queue key");
    send_message(host, identity, messages::remote_key_inject(key_code))
}

/// Type `text` into the TV's focused field by injecting one key event per
/// character over the persistent remote channel (Android TVs expose no ScalarWeb
/// text input). Unmappable characters are skipped. Requires a text field to be
/// focused on the TV — there's no field-focus signal to check, so this is
/// best-effort, mirroring how a hardware remote's keyboard behaves.
pub async fn send_text(host: &str, identity: &Identity, text: &str) -> Result<()> {
    let codes: Vec<u32> = text.chars().filter_map(super::char_keycode).collect();
    if codes.is_empty() {
        return Ok(());
    }
    tracing::debug!(target: "bifrost::smarttv", host, chars = codes.len(), "ATV remote: type text");
    for code in codes {
        send_key(host, identity, code).await?;
    }
    Ok(())
}

/// Own the persistent connection: (re)connect, run a session until it drops,
/// then back off (capped exponential — a rejected cert is permanent until the
/// user re-pairs, and a flat 3s retry paints the event log) and retry. Queued
/// keys wait for the next live session. Exits when the link is retired
/// (credentials changed) or every sender is gone.
async fn remote_link_actor(
    host: String,
    identity: Identity,
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
    events: broadcast::Sender<AtvEvent>,
    retired: Arc<std::sync::atomic::AtomicBool>,
    auth_failed: Arc<std::sync::atomic::AtomicBool>,
    state: Arc<Mutex<AtvStateCache>>,
) {
    use std::sync::atomic::Ordering;
    let mut delay = Duration::from_secs(3);
    let mut warned_auth = false;
    loop {
        let started = std::time::Instant::now();
        match run_session(&host, REMOTE_PORT, &identity, &mut rx, &events, &state).await {
            Ok(()) => return, // all senders dropped — nothing left to serve
            Err(e) => {
                let msg = format!("{e:#}");
                // A cert rejection means our pairing is no longer trusted —
                // retrying can't fix it; say so once, loudly, and flag the link
                // so send paths fail with an actionable error.
                if msg.contains("CertificateUnknown") {
                    auth_failed.store(true, Ordering::Relaxed);
                    if !warned_auth {
                        warned_auth = true;
                        tracing::warn!(
                            target: "bifrost::smarttv",
                            host,
                            "ATV remote: the TV no longer trusts our pairing — re-pair the remote (Settings → Smart TV → Pair remote)"
                        );
                    }
                }
                tracing::debug!(target: "bifrost::smarttv", host, "ATV remote session ended: {msg}");
            }
        }
        // A session that lived a while was healthy — restart eagerly, and clear
        // the auth-failed flag (a re-pair or a recovered TV is trusted again).
        if started.elapsed() > Duration::from_secs(60) {
            delay = Duration::from_secs(3);
            warned_auth = false;
            auth_failed.store(false, Ordering::Relaxed);
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(300));
        if retired.load(std::sync::atomic::Ordering::Relaxed) {
            tracing::debug!(target: "bifrost::smarttv", host, "ATV remote: link retired (superseded)");
            return;
        }
    }
}

/// Fold a parsed push into the link's state cache and forward it onto the
/// event channel (no subscribers = dropped; the cache keeps the latest).
fn emit(events: &broadcast::Sender<AtvEvent>, state: &Mutex<AtvStateCache>, msg: &RemoteIn) {
    let ev = match msg {
        RemoteIn::CurrentApp(pkg) => AtvEvent::CurrentApp(pkg.clone()),
        RemoteIn::Volume { level, max, muted } => AtvEvent::Volume {
            level: *level,
            max: *max,
            muted: *muted,
        },
        RemoteIn::Started(on) => AtvEvent::Started(*on),
        _ => return,
    };
    {
        let mut cache = state.lock().expect("atv state cache poisoned");
        match &ev {
            AtvEvent::CurrentApp(p) => cache.current_app = Some(p.clone()),
            AtvEvent::Volume { level, max, muted } => {
                cache.volume_level = Some(*level);
                cache.volume_max = Some(*max);
                cache.muted = Some(*muted);
            }
            AtvEvent::Started(on) => cache.screen_on = Some(*on),
        }
    }
    let _ = events.send(ev);
}

/// One remote-channel session: connect, complete the configure/set-active
/// handshake, then service keepalive pings and inject keys from `rx` until the
/// connection closes (returns `Ok` when the sender is gone, `Err` on I/O loss).
async fn run_session(
    host: &str,
    port: u16,
    identity: &Identity,
    rx: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    events: &broadcast::Sender<AtvEvent>,
    state: &Mutex<AtvStateCache>,
) -> Result<()> {
    let mut stream = connect(host, port, identity).await?;
    let mut reader = FrameReader::default();

    // Handshake: the TV drives configure → set-active. It may front-load state
    // pushes (volume, screen) before set-active — forward those too.
    let handshake = async {
        loop {
            let msg = messages::parse_remote(&read_msg(&mut stream, &mut reader).await?);
            match &msg {
                RemoteIn::Configure => {
                    write_msg(&mut stream, &messages::remote_configure()).await?
                }
                RemoteIn::SetActive => {
                    write_msg(&mut stream, &messages::remote_set_active()).await?;
                    return Ok::<(), anyhow::Error>(());
                }
                RemoteIn::Ping(v) => {
                    write_msg(&mut stream, &messages::remote_ping_response(*v)).await?
                }
                other => emit(events, state, other),
            }
        }
    };
    tokio::time::timeout(IO_TIMEOUT, handshake)
        .await
        .map_err(|_| anyhow!("timed out negotiating the remote channel"))??;
    tracing::debug!(target: "bifrost::smarttv", host, "ATV remote: session active");

    // Live session: inject queued keys, answer pings, broadcast state pushes,
    // until the socket drops.
    loop {
        tokio::select! {
            body = rx.recv() => {
                let Some(body) = body else { return Ok(()) }; // all senders dropped
                write_msg(&mut stream, &body).await?;
                tracing::debug!(target: "bifrost::smarttv", host, bytes = body.len(), "ATV remote: message sent");
            }
            frame = read_msg(&mut stream, &mut reader) => {
                let msg = messages::parse_remote(&frame?);
                if let RemoteIn::Ping(v) = &msg {
                    write_msg(&mut stream, &messages::remote_ping_response(*v)).await?;
                } else {
                    emit(events, state, &msg);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// FULL end-to-end assistant probe: synthesize real speech via a live TTS
    /// endpoint, transcode, and stream it to a real paired TV.
    ///   ATV_SAY_HOST=192.168.1.44 ATV_SAY_TEXT="what is the weather today" \\
    ///   ATV_SAY_TTS=http://localhost:9123/v1 ATV_VOICE_DB=data/bifrost.db \\
    ///   BIFROST_SECRET=... cargo test --lib atv_say_probe -- --ignored --nocapture
    /// WATCH THE TV — the Assistant should hear and answer the phrase.
    #[tokio::test]
    #[ignore]
    async fn atv_say_probe() {
        let Ok(host) = std::env::var("ATV_SAY_HOST") else {
            return;
        };
        let text = std::env::var("ATV_SAY_TEXT").unwrap_or("what is the weather today".into());
        let tts = std::env::var("ATV_SAY_TTS").unwrap_or("http://localhost:9123/v1".into());
        let db_path = std::env::var("ATV_VOICE_DB").unwrap_or("data/bifrost.db".into());
        let secret = std::env::var("BIFROST_SECRET").expect("BIFROST_SECRET");
        let db = sqlx::sqlite::SqlitePoolOptions::new()
            .connect(&format!("sqlite://{db_path}?mode=ro"))
            .await
            .unwrap();
        let rows: Vec<String> =
            sqlx::query_scalar("SELECT credentials FROM providers WHERE provider_type = 'smarttv'")
                .fetch_all(&db)
                .await
                .unwrap();
        let state = crate::AppState::new(db, &secret, crate::providers::default_registry());
        let creds: serde_json::Value = rows
            .iter()
            .filter_map(|e| state.decrypt_credentials(e).ok())
            .filter_map(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
            .find(|c| c["host"].as_str().is_some_and(|h| h.contains(&host)))
            .expect("no smarttv provider for that host");
        let identity = Identity {
            cert_pem: creds["atv_cert"].as_str().expect("paired atv_cert").into(),
            key_pem: creds["atv_key"].as_str().unwrap().into(),
        };

        // 1. Synthesize.
        println!("=== synthesizing: {text:?} ===");
        let wav = reqwest::Client::new()
            .post(format!("{tts}/audio/speech"))
            .json(&serde_json::json!({ "input": text, "response_format": "wav" }))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        println!(
            "=== TTS returned {} bytes; transcoding to 8kHz PCM ===",
            wav.len()
        );
        let pcm = crate::audio::wav_to_atv_voice_pcm(&wav).unwrap();
        println!(
            "=== {} PCM bytes ({:.1}s); streaming to the TV (WATCH IT) ===",
            pcm.len(),
            pcm.len() as f64 / 2.0 / 8000.0
        );

        // 2. Open a dedicated session so we can READ the TV's responses
        //    (send_voice uses the shared link, whose reads we don't see here).
        let assist_key: u32 = std::env::var("ATV_SAY_KEY")
            .ok()
            .and_then(|k| k.parse().ok())
            .unwrap_or(219); // KEYCODE_ASSIST; try 231 (VOICE_ASSIST) / 84 (SEARCH)
        let mut stream = connect(&host, REMOTE_PORT, &identity).await.unwrap();
        let mut reader = FrameReader::default();
        loop {
            let f = read_msg(&mut stream, &mut reader).await.unwrap();
            match messages::parse_remote(&f) {
                RemoteIn::Configure => write_msg(&mut stream, &messages::remote_configure())
                    .await
                    .unwrap(),
                RemoteIn::SetActive => {
                    write_msg(&mut stream, &messages::remote_set_active())
                        .await
                        .unwrap();
                    break;
                }
                RemoteIn::Ping(v) => write_msg(&mut stream, &messages::remote_ping_response(v))
                    .await
                    .unwrap(),
                _ => {}
            }
        }
        println!("=== session active; assist key = {assist_key} ===");
        write_msg(&mut stream, &messages::remote_key_inject(assist_key))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(900)).await;
        let sid = 1;
        write_msg(&mut stream, &messages::remote_voice_begin(sid))
            .await
            .unwrap();
        for chunk in pcm.chunks(20_000) {
            write_msg(&mut stream, &messages::remote_voice_payload(sid, chunk))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        write_msg(&mut stream, &messages::remote_voice_end(sid))
            .await
            .unwrap();
        println!("=== voice sent — reading TV responses for 8s (WATCH THE TV) ===");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(2), read_msg(&mut stream, &mut reader))
                .await
            {
                Ok(Ok(f)) => match messages::parse_remote(&f) {
                    RemoteIn::Ping(v) => write_msg(&mut stream, &messages::remote_ping_response(v))
                        .await
                        .unwrap(),
                    _ => dump("say-response", &f),
                },
                Ok(Err(e)) => {
                    println!("read error: {e:#}");
                    break;
                }
                Err(_) => {}
            }
        }
        println!("=== done ===");
    }

    /// Manual voice-channel probe against a REAL paired TV:
    ///   ATV_VOICE_HOST=192.168.1.44 ATV_VOICE_DB=data/bifrost.db \\
    ///   BIFROST_SECRET=... cargo test --lib atv_voice_probe -- --ignored --nocapture
    /// Opens the paired session, fires RemoteVoiceBegin, streams ~2s of 8kHz
    /// 16-bit PCM (a 440Hz tone), then RemoteVoiceEnd — WATCH THE TV: if the
    /// Assistant overlay appears, the voice channel is live. Dumps every frame
    /// the TV sends on the session (it may negotiate an audio config back).
    #[tokio::test]
    #[ignore]
    async fn atv_voice_probe() {
        let Ok(host) = std::env::var("ATV_VOICE_HOST") else {
            return;
        };
        let db_path = std::env::var("ATV_VOICE_DB").unwrap_or("data/bifrost.db".into());
        let secret = std::env::var("BIFROST_SECRET").expect("BIFROST_SECRET");
        let db = sqlx::sqlite::SqlitePoolOptions::new()
            .connect(&format!("sqlite://{db_path}?mode=ro"))
            .await
            .unwrap();
        let rows: Vec<String> =
            sqlx::query_scalar("SELECT credentials FROM providers WHERE provider_type = 'smarttv'")
                .fetch_all(&db)
                .await
                .unwrap();
        let state = crate::AppState::new(db, &secret, crate::providers::default_registry());
        let creds: serde_json::Value = rows
            .iter()
            .filter_map(|e| state.decrypt_credentials(e).ok())
            .filter_map(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
            .find(|c| c["host"].as_str().is_some_and(|h| h.contains(&host)))
            .expect("no smarttv provider for that host");
        let identity = Identity {
            cert_pem: creds["atv_cert"].as_str().expect("paired atv_cert").into(),
            key_pem: creds["atv_key"].as_str().unwrap().into(),
        };

        let mut stream = connect(&host, REMOTE_PORT, &identity).await.unwrap();
        let mut reader = FrameReader::default();
        // Handshake.
        loop {
            let f = read_msg(&mut stream, &mut reader).await.unwrap();
            match messages::parse_remote(&f) {
                RemoteIn::Configure => write_msg(&mut stream, &messages::remote_configure())
                    .await
                    .unwrap(),
                RemoteIn::SetActive => {
                    write_msg(&mut stream, &messages::remote_set_active())
                        .await
                        .unwrap();
                    break;
                }
                RemoteIn::Ping(v) => write_msg(&mut stream, &messages::remote_ping_response(v))
                    .await
                    .unwrap(),
                _ => {}
            }
        }
        println!("=== session active — sending RemoteVoiceBegin (WATCH THE TV) ===");

        let session_id = 1;
        write_msg(&mut stream, &messages::remote_voice_begin(session_id))
            .await
            .unwrap();

        // ~2s of 8kHz mono 16-bit PCM, 440Hz tone, in 20KB chunks.
        let sample_rate = 8000u32;
        let mut pcm: Vec<u8> = Vec::new();
        for n in 0..(sample_rate * 2) {
            let t = n as f32 / sample_rate as f32;
            let s = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 8000.0;
            pcm.extend_from_slice(&(s as i16).to_le_bytes());
        }
        for chunk in pcm.chunks(20_000) {
            write_msg(
                &mut stream,
                &messages::remote_voice_payload(session_id, chunk),
            )
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        write_msg(&mut stream, &messages::remote_voice_end(session_id))
            .await
            .unwrap();
        println!("=== voice sequence sent — listening 8s for the TV's response ===");

        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(2), read_msg(&mut stream, &mut reader))
                .await
            {
                Ok(Ok(f)) => match messages::parse_remote(&f) {
                    RemoteIn::Ping(v) => write_msg(&mut stream, &messages::remote_ping_response(v))
                        .await
                        .unwrap(),
                    _ => dump("voice-response", &f),
                },
                Ok(Err(e)) => {
                    println!("read error (TV may have closed the session): {e:#}");
                    break;
                }
                Err(_) => {}
            }
        }
        println!("=== voice probe done ===");
    }

    /// Manual validation of the pairing HANDSHAKE against a real device:
    ///   ATV_PAIR_HOST=192.168.1.44 cargo test --lib atv_pair_probe -- --ignored --nocapture
    /// Success = the TLS 1.3 + leaf-cert handshake was accepted and the TV is
    /// now showing a pairing code (which this probe abandons — it times out on
    /// screen). Exactly the layer both dongle-pairing failures lived in.
    #[tokio::test]
    #[ignore]
    async fn atv_pair_probe() {
        let Ok(host) = std::env::var("ATV_PAIR_HOST") else {
            return;
        };
        let identity = Identity::generate().unwrap();
        match PairingSession::begin(&host, &identity, "Bifrost (probe)").await {
            Ok(_session) => {
                println!(
                    "=== pairing handshake ACCEPTED — the TV should be showing a code (abandoning; it times out) ==="
                );
            }
            Err(e) => panic!("pairing handshake still rejected: {e:#}"),
        }
    }

    /// Throwaway diagnostic against a REAL TV — run manually:
    ///   ATV_PROBE_HOST=192.168.1.22 ATV_PROBE_DB=data/bifrost.db \
    ///   BIFROST_SECRET=... cargo test --lib atv_probe -- --ignored --nocapture
    /// Dumps every incoming RemoteMessage's field tags (+ nested fields and any
    /// string payloads) for ~45s, triggering app switches + a volume nudge so
    /// the TV pushes ime/volume/start messages. Used to pin down proto tags.
    #[tokio::test]
    #[ignore]
    async fn atv_probe() {
        let Ok(host) = std::env::var("ATV_PROBE_HOST") else {
            return;
        };
        let db_path = std::env::var("ATV_PROBE_DB").unwrap_or("data/bifrost.db".into());
        let secret = std::env::var("BIFROST_SECRET").expect("BIFROST_SECRET");
        let db = sqlx::sqlite::SqlitePoolOptions::new()
            .connect(&format!("sqlite://{db_path}?mode=ro"))
            .await
            .unwrap();
        let rows: Vec<String> =
            sqlx::query_scalar("SELECT credentials FROM providers WHERE provider_type = 'smarttv'")
                .fetch_all(&db)
                .await
                .unwrap();
        let state = crate::AppState::new(db, &secret, crate::providers::default_registry());
        // Several TVs may exist — pick the provider whose host matches.
        let creds: serde_json::Value = rows
            .iter()
            .filter_map(|enc| state.decrypt_credentials(enc).ok())
            .filter_map(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
            .find(|c| c["host"].as_str().is_some_and(|h| h.contains(&host)))
            .expect("no smarttv provider for that host");
        let identity = Identity {
            cert_pem: creds["atv_cert"].as_str().expect("paired atv_cert").into(),
            key_pem: creds["atv_key"].as_str().unwrap().into(),
        };

        let mut stream = connect(&host, REMOTE_PORT, &identity).await.unwrap();
        let mut reader = FrameReader::default();
        // handshake
        loop {
            let f = read_msg(&mut stream, &mut reader).await.unwrap();
            match messages::parse_remote(&f) {
                RemoteIn::Configure => write_msg(&mut stream, &messages::remote_configure())
                    .await
                    .unwrap(),
                RemoteIn::SetActive => {
                    write_msg(&mut stream, &messages::remote_set_active())
                        .await
                        .unwrap();
                    break;
                }
                RemoteIn::Ping(v) => write_msg(&mut stream, &messages::remote_ping_response(v))
                    .await
                    .unwrap(),
                _ => dump("pre-handshake", &f),
            }
        }
        println!("=== session active; listening 45s ===");

        // Trigger pushes: app switch (YouTube → Netflix), one volume up+down.
        let h = host.clone();
        tokio::spawn(async move {
            let c = reqwest::Client::new();
            let post = |method: &'static str, params: serde_json::Value| {
                let c = c.clone();
                let h = h.clone();
                async move {
                    let _ = c.post(format!("http://{h}/sony/appControl"))
                        .json(&serde_json::json!({"method": method, "id":1, "version":"1.0", "params": params}))
                        .send().await;
                }
            };
            tokio::time::sleep(Duration::from_secs(3)).await;
            post("setActiveApp", serde_json::json!([{ "uri": "com.google.android.youtube.tv-com.google.android.apps.youtube.tv.activity.ShellActivity" }])).await;
            tokio::time::sleep(Duration::from_secs(8)).await;
            post(
                "setActiveApp",
                serde_json::json!([{ "uri": "com.netflix.ninja-com.netflix.ninja.MainActivity" }]),
            )
            .await;
        });
        // Volume nudge over this same session at t≈20s.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        let mut nudged = false;
        while tokio::time::Instant::now() < deadline {
            if !nudged && tokio::time::Instant::now() > deadline - Duration::from_secs(25) {
                nudged = true;
                write_msg(&mut stream, &messages::remote_key_inject(24))
                    .await
                    .unwrap(); // VOL_UP
                tokio::time::sleep(Duration::from_millis(400)).await;
                write_msg(&mut stream, &messages::remote_key_inject(25))
                    .await
                    .unwrap(); // VOL_DOWN
            }
            match tokio::time::timeout(Duration::from_secs(2), read_msg(&mut stream, &mut reader))
                .await
            {
                Ok(Ok(f)) => {
                    if let RemoteIn::Ping(v) = messages::parse_remote(&f) {
                        write_msg(&mut stream, &messages::remote_ping_response(v))
                            .await
                            .unwrap();
                    } else {
                        dump("msg", &f);
                    }
                }
                Ok(Err(e)) => {
                    println!("read error: {e:#}");
                    break;
                }
                Err(_) => {}
            }
        }
        println!("=== probe done ===");
    }

    #[cfg(test)]
    fn dump(label: &str, body: &[u8]) {
        fn render(fields: &[wire::Field], depth: usize) -> String {
            fields
                .iter()
                .map(|f| match f {
                    wire::Field::Varint(tag, v) => format!("{tag}={v}"),
                    wire::Field::Bytes(tag, b) => {
                        let nested = wire::parse_fields(b);
                        if depth < 3 && !nested.is_empty() {
                            format!("{tag}:{{ {} }}", render(&nested, depth + 1))
                        } else {
                            format!("{tag}:{:?}", String::from_utf8_lossy(b))
                        }
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
        println!("{label}: {}", render(&wire::parse_fields(body), 0));
    }

    /// A Google TV dongle's remote service is TLS 1.3-only (it alerts on a
    /// 1.2 ClientHello); `connect` must fall back and still get through.
    #[tokio::test]
    async fn connect_falls_back_to_tls13_for_13_only_devices() {
        let server_id = Identity::generate().unwrap();
        let client_id = Identity::generate().unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let scfg = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(
                    server_id.cert_der().unwrap(),
                )],
                rustls::pki_types::PrivateKeyDer::Pkcs8(server_id.key_der().unwrap().into()),
            )
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(scfg));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // Accept both attempts: the 1.2 handshake fails server-side, the
            // 1.3 retry completes; hold it open briefly.
            loop {
                let Ok((tcp, _)) = listener.accept().await else {
                    return;
                };
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    if let Ok(tls) = acceptor.accept(tcp).await {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        drop(tls);
                    }
                });
            }
        });

        let stream = connect(&addr.ip().to_string(), addr.port(), &client_id)
            .await
            .expect("fallback handshake");
        let (_, conn) = stream.get_ref();
        assert_eq!(
            conn.protocol_version(),
            Some(rustls::ProtocolVersion::TLSv1_3)
        );
    }

    /// Re-pairing mints a new identity; the shared per-host link must be
    /// replaced (old one retired), not reused with retired credentials.
    #[tokio::test]
    async fn repairing_replaces_the_hosts_link() {
        let id1 = Identity::generate().unwrap();
        let id2 = Identity::generate().unwrap();
        let a = link_for("link-replacement-test-host", &id1);
        let b = link_for("link-replacement-test-host", &id1);
        assert!(Arc::ptr_eq(&a, &b), "same identity reuses the link");
        let c = link_for("link-replacement-test-host", &id2);
        assert!(!Arc::ptr_eq(&a, &c), "a new identity replaces the link");
        assert!(
            a.retired.load(std::sync::atomic::Ordering::Relaxed),
            "the superseded link is retired so its actor exits"
        );
        assert_eq!(c.identity_fp, identity_fp(&id2));
    }

    /// A RemoteMessage carrying just `field` (status-free; the remote channel has
    /// no status envelope) — used by the mock TV to drive the handshake.
    fn remote_msg(field: u32, body: &[u8]) -> Vec<u8> {
        let mut m = Vec::new();
        wire::put_bytes_field(&mut m, field, body);
        m
    }

    async fn srv_read(s: &mut (impl AsyncReadExt + Unpin), r: &mut FrameReader) -> Vec<u8> {
        loop {
            if let Some(f) = r.next_frame() {
                return f;
            }
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).await.unwrap();
            assert!(n > 0, "client closed early");
            r.push(&buf[..n]);
        }
    }

    /// Drives `run_session` end-to-end against a mock TLS "TV": exercises
    /// `connect` (client cert + TOFU verifier), the configure → set-active
    /// handshake, keepalive **ping → pong**, and a key inject pulled from the
    /// channel over the persistent session.
    #[tokio::test]
    async fn run_session_handshakes_answers_pings_and_injects() {
        let server_id = Identity::generate().unwrap();
        let client_id = Identity::generate().unwrap();

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let scfg = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(
                    server_id.cert_der().unwrap(),
                )],
                rustls::pki_types::PrivateKeyDer::Pkcs8(server_id.key_der().unwrap().into()),
            )
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(scfg));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let srv = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut tls = acceptor.accept(tcp).await.unwrap();
            let mut r = FrameReader::default();
            // TV → configure → expect configure reply.
            tls.write_all(&wire::frame(&remote_msg(1, b"")))
                .await
                .unwrap();
            assert_eq!(
                messages::parse_remote(&srv_read(&mut tls, &mut r).await),
                RemoteIn::Configure
            );
            // TV → set_active → expect set_active reply.
            tls.write_all(&wire::frame(&remote_msg(2, b"")))
                .await
                .unwrap();
            assert_eq!(
                messages::parse_remote(&srv_read(&mut tls, &mut r).await),
                RemoteIn::SetActive
            );
            // TV → ping(7). Expect to see, across the next frames, both the
            // pong (ping_response with val1=7) and the key inject (keycode 23).
            let mut ping = Vec::new();
            wire::put_varint_field(&mut ping, 1, 7);
            tls.write_all(&wire::frame(&remote_msg(8, &ping)))
                .await
                .unwrap();

            let (mut saw_pong, mut saw_inject) = (false, false);
            while !(saw_pong && saw_inject) {
                let f = wire::parse_fields(&srv_read(&mut tls, &mut r).await);
                if let Some(b) = wire::field_bytes(&f, 9) {
                    assert_eq!(wire::field_varint(&wire::parse_fields(b), 1), Some(7));
                    saw_pong = true;
                } else if let Some(b) = wire::field_bytes(&f, 10) {
                    assert_eq!(wire::field_varint(&wire::parse_fields(b), 1), Some(23));
                    saw_inject = true;
                }
            }
            // TV pushes a foreground-app change on the same live session.
            let mut info = Vec::new();
            wire::put_bytes_field(&mut info, 12, b"com.netflix.ninja");
            let mut ime = Vec::new();
            wire::put_bytes_field(&mut ime, 1, &info);
            tls.write_all(&wire::frame(&remote_msg(20, &ime)))
                .await
                .unwrap();
            tls.flush().await.unwrap();
            // Hold the socket open until the client has seen the push.
            tokio::time::sleep(Duration::from_millis(500)).await;
        });

        // Queue a key, then run one session against the mock's port.
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(messages::remote_key_inject(23)).unwrap();
        let (events_tx, mut events_rx) = broadcast::channel(8);
        let cache = Arc::new(Mutex::new(AtvStateCache::default()));
        let cache2 = Arc::clone(&cache);
        let host = addr.ip().to_string();
        let client = tokio::spawn(async move {
            let _ = run_session(&host, addr.port(), &client_id, &mut rx, &events_tx, &cache2).await;
        });

        // The push surfaces on the subscription channel.
        let ev = tokio::time::timeout(Duration::from_secs(5), events_rx.recv())
            .await
            .expect("app push timed out")
            .unwrap();
        assert_eq!(ev, AtvEvent::CurrentApp("com.netflix.ninja".into()));
        assert_eq!(
            cache.lock().unwrap().current_app.as_deref(),
            Some("com.netflix.ninja"),
            "the link's state cache folds pushes"
        );
        tokio::time::timeout(Duration::from_secs(5), srv)
            .await
            .expect("mock TV handshake/ping/inject timed out")
            .unwrap();
        client.abort();
    }
}
