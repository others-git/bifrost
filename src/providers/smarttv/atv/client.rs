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
use tokio::sync::mpsc;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

const PAIR_PORT: u16 = 6467;
const REMOTE_PORT: u16 = 6466;
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
async fn connect(host: &str, port: u16, identity: &Identity) -> Result<TlsStream<TcpStream>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cert = rustls::pki_types::CertificateDer::from(identity.cert_der()?);
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(identity.key_der()?.into());
    // Pin TLS 1.2: Android TV Remote firmware rejects our TLS 1.3 + RSA-client-cert
    // handshake with an `IllegalParameter` alert (the reference clients all use
    // 1.2), and the TV negotiates ECDHE-RSA-AES128-GCM-SHA256 happily.
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS12])?
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

/// A persistent remote-channel connection to one TV. Android TV Remote injects
/// only register on a **live, kept-open** session — the TV drops a key whose
/// connection tears down in the same instant — so, like the Onkyo link, one
/// background task owns the socket: it completes the configure/set-active
/// handshake, answers keepalive pings, and writes key injects sent on `tx`. It
/// reconnects with backoff and is shared per host.
struct RemoteLink {
    tx: mpsc::UnboundedSender<u32>,
}

fn remote_links() -> &'static Mutex<HashMap<String, Arc<RemoteLink>>> {
    static M: OnceLock<Mutex<HashMap<String, Arc<RemoteLink>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Queue a key for the TV at `host`, lazily (re)starting its persistent remote
/// link. Fire-and-forget: returns once enqueued (the link owns delivery), so a
/// rapid key sequence pipelines over one open session instead of paying a full
/// TLS + handshake per press.
pub async fn send_key(host: &str, identity: &Identity, key_code: u32) -> Result<()> {
    let link = {
        let mut map = remote_links().lock().expect("remote link map poisoned");
        if let Some(link) = map.get(host) {
            link.clone()
        } else {
            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(remote_link_actor(host.to_string(), identity.clone(), rx));
            let link = Arc::new(RemoteLink { tx });
            map.insert(host.to_string(), link.clone());
            link
        }
    };
    tracing::debug!(target: "bifrost::smarttv", host, key_code, "ATV remote: queue key");
    link.tx
        .send(key_code)
        .map_err(|_| anyhow!("remote link for {host} is closed"))?;
    Ok(())
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
/// then back off and retry. Queued keys wait for the next live session.
async fn remote_link_actor(host: String, identity: Identity, mut rx: mpsc::UnboundedReceiver<u32>) {
    loop {
        if let Err(e) = run_session(&host, REMOTE_PORT, &identity, &mut rx).await {
            tracing::debug!(target: "bifrost::smarttv", host, "ATV remote session ended: {e:#}");
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// One remote-channel session: connect, complete the configure/set-active
/// handshake, then service keepalive pings and inject keys from `rx` until the
/// connection closes (returns `Ok` when the sender is gone, `Err` on I/O loss).
async fn run_session(
    host: &str,
    port: u16,
    identity: &Identity,
    rx: &mut mpsc::UnboundedReceiver<u32>,
) -> Result<()> {
    let mut stream = connect(host, port, identity).await?;
    let mut reader = FrameReader::default();

    // Handshake: the TV drives configure → set-active.
    let handshake = async {
        loop {
            match messages::parse_remote(&read_msg(&mut stream, &mut reader).await?) {
                RemoteIn::Configure => {
                    write_msg(&mut stream, &messages::remote_configure()).await?
                }
                RemoteIn::SetActive => {
                    write_msg(&mut stream, &messages::remote_set_active()).await?;
                    return Ok::<(), anyhow::Error>(());
                }
                RemoteIn::Ping(v) => {
                    write_msg(&mut stream, &messages::remote_ping_response(v)).await?
                }
                RemoteIn::Other => {}
            }
        }
    };
    tokio::time::timeout(IO_TIMEOUT, handshake)
        .await
        .map_err(|_| anyhow!("timed out negotiating the remote channel"))??;
    tracing::debug!(target: "bifrost::smarttv", host, "ATV remote: session active");

    // Live session: inject queued keys, answer pings, until the socket drops.
    loop {
        tokio::select! {
            code = rx.recv() => {
                let Some(code) = code else { return Ok(()) }; // all senders dropped
                write_msg(&mut stream, &messages::remote_key_inject(code)).await?;
                tracing::debug!(target: "bifrost::smarttv", host, key_code = code, "ATV remote: key injected");
            }
            frame = read_msg(&mut stream, &mut reader) => {
                if let RemoteIn::Ping(v) = messages::parse_remote(&frame?) {
                    write_msg(&mut stream, &messages::remote_ping_response(v)).await?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

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
        });

        // Queue a key, then run one session against the mock's port.
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(23).unwrap();
        let host = addr.ip().to_string();
        let client = tokio::spawn(async move {
            let _ = run_session(&host, addr.port(), &client_id, &mut rx).await;
        });

        tokio::time::timeout(Duration::from_secs(5), srv)
            .await
            .expect("mock TV handshake/ping/inject timed out")
            .unwrap();
        client.abort();
    }
}
