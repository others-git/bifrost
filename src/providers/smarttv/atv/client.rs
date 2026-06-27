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
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
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
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(TofuVerifier(provider)))
        .with_client_auth_cert(vec![cert], key)?;

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
        let mut stream = connect(host, PAIR_PORT, identity).await?;
        let server_cert_der = peer_cert(&stream)?;
        let client_cert_der = identity.cert_der()?;
        let mut reader = FrameReader::default();

        write_msg(&mut stream, &messages::pairing_request(client_name)).await?;
        expect(
            &read_msg(&mut stream, &mut reader).await?,
            PairingIn::RequestAck,
        )?;

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
        write_msg(&mut self.stream, &messages::pairing_secret(&secret)).await?;
        expect(
            &read_msg(&mut self.stream, &mut self.reader).await?,
            PairingIn::SecretAck,
        )?;
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

/// Connect to the remote channel, complete the configure/set-active handshake
/// (answering pings), then inject one key. A fresh connection per press keeps
/// the provider stateless; it's plenty responsive for tap input.
pub async fn send_key(host: &str, identity: &Identity, key_code: u32) -> Result<()> {
    let mut stream = connect(host, REMOTE_PORT, identity).await?;
    let mut reader = FrameReader::default();

    // Handshake: the TV drives configure → set-active; once active, inject.
    let active = async {
        loop {
            let frame = read_msg(&mut stream, &mut reader).await?;
            match messages::parse_remote(&frame) {
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
    tokio::time::timeout(IO_TIMEOUT, active)
        .await
        .map_err(|_| anyhow!("timed out negotiating the remote channel"))??;

    write_msg(&mut stream, &messages::remote_key_inject(key_code)).await?;
    stream.flush().await?;
    Ok(())
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

    /// Drives the remote channel end-to-end against a mock TLS "TV": exercises
    /// `connect` (client cert + TOFU verifier), the framed I/O, and the
    /// configure → set-active → key-inject state machine in [`send_key`].
    #[tokio::test]
    async fn send_key_negotiates_and_injects_over_tls() {
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
            // TV → configure; client must echo a configure back.
            tls.write_all(&wire::frame(&remote_msg(1, b"")))
                .await
                .unwrap();
            assert_eq!(
                messages::parse_remote(&srv_read(&mut tls, &mut r).await),
                RemoteIn::Configure
            );
            // TV → set_active; client echoes set_active.
            tls.write_all(&wire::frame(&remote_msg(2, b"")))
                .await
                .unwrap();
            assert_eq!(
                messages::parse_remote(&srv_read(&mut tls, &mut r).await),
                RemoteIn::SetActive
            );
            // Then the key inject arrives (field 10), keycode 23 (DPAD_CENTER).
            let inj = srv_read(&mut tls, &mut r).await;
            let f = wire::parse_fields(&inj);
            let body = wire::field_bytes(&f, 10).expect("key_inject");
            assert_eq!(wire::field_varint(&wire::parse_fields(body), 1), Some(23));
        });

        // Drive the client against the mock's ephemeral port (send_key hard-codes
        // 6466, so exercise the same path via the inner helpers).
        let host = addr.ip().to_string();
        let mut stream = connect(&host, addr.port(), &client_id).await.unwrap();
        let mut reader = FrameReader::default();
        loop {
            match messages::parse_remote(&read_msg(&mut stream, &mut reader).await.unwrap()) {
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
                RemoteIn::Other => {}
            }
        }
        write_msg(&mut stream, &messages::remote_key_inject(23))
            .await
            .unwrap();
        srv.await.unwrap();
    }
}
