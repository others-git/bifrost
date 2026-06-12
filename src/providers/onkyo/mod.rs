//! Onkyo / Integra eISCP integration (TCP port 60128).
//!
//! eISCP wraps short ASCII "ISCP" messages (`!1PWR01`) in a 16-byte binary
//! header. The receiver answers queries (`!1MVLQSTN`) and *also* pushes
//! unsolicited state echoes on the same socket whenever anything changes —
//! responses and pushes are indistinguishable, so reads are "collect codes
//! until satisfied or timeout".
//!
//! This provider opens a short-lived connection per operation (the receiver
//! accepts several concurrent eISCP clients). A persistent push subscription
//! for live UI updates is a planned follow-up.
//!
//! Key command groups used:
//! - `PWR` power, `MVL` volume (hex), `AMT` mute, `SLI` input selector
//! - `NTC` network transport (PLAY/PAUSE/STOP/TRUP/TRDN), `NSV` service select
//! - `NTI`/`NAT`/`NAL` track metadata, `NST` play status (`prs` triplet)

use crate::models::audio::{
    AudioCapabilities, AudioCommand, AudioDevice, AudioDeviceKind, AudioState, NowPlaying,
    PlayState, TransportCmd,
};
use crate::providers::{AudioProvider, AudioProviderFactory, CredentialField, FieldKind};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use std::collections::HashMap;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

pub const DEFAULT_PORT: u16 = 60128;
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(2500);

// ── eISCP packet codec (pure functions) ─────────────────────────────────────

/// Wrap an ISCP message (e.g. `PWRQSTN`) in an eISCP packet: 16-byte header +
/// `!1<msg>\r`.
pub fn encode_packet(msg: &str) -> Vec<u8> {
    let payload = format!("!1{msg}\r");
    let mut out = Vec::with_capacity(16 + payload.len());
    out.extend_from_slice(b"ISCP");
    out.extend_from_slice(&16u32.to_be_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.push(0x01); // version
    out.extend_from_slice(&[0, 0, 0]); // reserved
    out.extend_from_slice(payload.as_bytes());
    out
}

/// Decode the first complete packet in `buf`. Returns the inner message with
/// the `!1` prefix and `\x1a`/`\r`/`\n` terminators stripped (e.g. `PWR01`),
/// plus the total bytes consumed. `None` when the buffer holds no complete
/// packet yet.
pub fn decode_packet(buf: &[u8]) -> Option<(String, usize)> {
    if buf.len() < 16 || &buf[0..4] != b"ISCP" {
        // Resync: skip garbage until a plausible magic. (Receivers are well
        // behaved; this guards against a mid-stream connect.)
        if !buf.is_empty() && &buf[0..buf.len().min(4)] != &b"ISCP"[..buf.len().min(4)] {
            return Some((String::new(), 1));
        }
        return None;
    }
    let header_size = u32::from_be_bytes(buf[4..8].try_into().unwrap()) as usize;
    let data_size = u32::from_be_bytes(buf[8..12].try_into().unwrap()) as usize;
    let total = header_size + data_size;
    if header_size < 16 || total > 64 * 1024 {
        return Some((String::new(), 1)); // corrupt header — resync byte-wise
    }
    if buf.len() < total {
        return None;
    }
    let raw = &buf[header_size..total];
    let text = String::from_utf8_lossy(raw);
    let msg = text
        .trim_end_matches(['\x1a', '\r', '\n'])
        .trim_start_matches("!1")
        .to_string();
    Some((msg, total))
}

// ── Source code mapping ─────────────────────────────────────────────────────

/// SLI input selector codes ↔ friendly names (the common subset; unknown codes
/// pass through as raw hex).
const SOURCES: &[(&str, &str)] = &[
    ("00", "vcr"),
    ("01", "cbl"),
    ("02", "game"),
    ("03", "aux"),
    ("05", "pc"),
    ("10", "bd"),
    ("11", "strm-box"),
    ("12", "tv"),
    ("23", "cd"),
    ("24", "fm"),
    ("25", "am"),
    ("29", "usb-front"),
    ("2A", "usb-rear"),
    ("2B", "net"),
    ("2E", "bluetooth"),
];

/// NSV streaming service codes reachable while on the NET input.
const NET_SERVICES: &[(&str, &str)] = &[
    ("00", "music-server"),
    ("01", "favorites"),
    ("0A", "spotify"),
    ("0E", "tunein"),
    ("12", "deezer"),
    ("18", "airplay"),
    ("19", "tidal"),
    ("F2", "internet-radio"),
];

fn source_name(code: &str) -> String {
    SOURCES
        .iter()
        .find(|(c, _)| c.eq_ignore_ascii_case(code))
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| code.to_string())
}

/// Resolve a requested source to wire commands. Friendly input names and raw
/// SLI hex go straight to `SLI`; streaming service names select NET first,
/// then the service (`NSV<code>0`).
fn source_commands(requested: &str) -> Result<Vec<String>> {
    let want = requested.trim().to_ascii_lowercase();
    if let Some((code, _)) = SOURCES.iter().find(|(_, n)| *n == want) {
        return Ok(vec![format!("SLI{code}")]);
    }
    if let Some((code, _)) = NET_SERVICES.iter().find(|(_, n)| *n == want) {
        return Ok(vec!["SLI2B".to_string(), format!("NSV{code}0")]);
    }
    // Raw SLI hex passthrough (e.g. "2B" or "0x2B").
    let raw = want.trim_start_matches("0x").to_ascii_uppercase();
    if raw.len() == 2 && raw.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(vec![format!("SLI{raw}")]);
    }
    Err(anyhow!("unknown audio source '{requested}'"))
}

fn transport_command(cmd: TransportCmd) -> &'static str {
    match cmd {
        TransportCmd::Play => "NTCPLAY",
        TransportCmd::Pause => "NTCPAUSE",
        TransportCmd::Stop => "NTCSTOP",
        TransportCmd::Next => "NTCTRUP",
        TransportCmd::Previous => "NTCTRDN",
        TransportCmd::Toggle => "NTCP/P",
    }
}

/// Parse the `NST` play-status triplet (`prs`): play state, repeat, shuffle.
fn parse_play_state(data: &str) -> Option<PlayState> {
    match data.chars().next()? {
        'P' | 'F' | 'R' => Some(PlayState::Playing),
        'p' => Some(PlayState::Paused),
        'S' | 'E' => Some(PlayState::Stopped),
        _ => None,
    }
}

/// Clean a metadata reply: `N/A` and empty strings mean "nothing playing".
fn meta(value: Option<&String>) -> Option<String> {
    let v = value?.trim();
    if v.is_empty() || v == "N/A" {
        return None;
    }
    Some(v.to_string())
}

// ── Provider ────────────────────────────────────────────────────────────────

pub struct OnkyoProvider {
    host: String,
    port: u16,
    timeout: Duration,
}

impl OnkyoProvider {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn from_credentials(creds_json: &str) -> Result<Self> {
        let creds: serde_json::Value = serde_json::from_str(creds_json)?;
        let host = creds["host"]
            .as_str()
            .filter(|h| !h.trim().is_empty())
            .ok_or_else(|| anyhow!("onkyo credentials missing host"))?
            .trim()
            .to_string();
        let port = creds["port"].as_u64().map(|p| p as u16).unwrap_or_else(|| {
            creds["port"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_PORT)
        });
        Ok(Self::new(host, port))
    }

    #[cfg(test)]
    pub fn new_for_test(host: impl Into<String>, port: u16) -> Self {
        Self {
            timeout: Duration::from_millis(500),
            ..Self::new(host, port)
        }
    }

    async fn connect(&self) -> Result<TcpStream> {
        let addr = format!("{}:{}", self.host, self.port);
        tokio::time::timeout(self.timeout, TcpStream::connect(&addr))
            .await
            .with_context(|| format!("Onkyo connect to {addr} timed out"))?
            .with_context(|| format!("Onkyo connect to {addr} failed"))
    }

    /// Send `commands`, then read packets until every code in `wanted` has
    /// been seen (replies and unsolicited echoes both count) or the timeout
    /// elapses. Returns code → latest data.
    async fn exchange(
        &self,
        commands: &[String],
        wanted: &[&str],
    ) -> Result<HashMap<String, String>> {
        let mut stream = self.connect().await?;
        for cmd in commands {
            stream.write_all(&encode_packet(cmd)).await?;
        }

        let mut collected: HashMap<String, String> = HashMap::new();
        if wanted.is_empty() {
            // Fire-and-forget writes still deserve a moment on the wire before
            // the socket closes — receivers drop unread input on RST.
            let _ = tokio::time::timeout(Duration::from_millis(50), stream.read(&mut [0u8; 256]))
                .await;
            return Ok(collected);
        }

        let mut buf: Vec<u8> = Vec::with_capacity(1024);
        let deadline = tokio::time::Instant::now() + self.timeout;
        let mut chunk = [0u8; 1024];

        'outer: while collected.len() < wanted.len() {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let n = match tokio::time::timeout(remaining, stream.read(&mut chunk)).await {
                Ok(Ok(0)) | Err(_) => break, // closed or timed out
                Ok(Ok(n)) => n,
                Ok(Err(e)) => return Err(e.into()),
            };
            buf.extend_from_slice(&chunk[..n]);

            while let Some((msg, consumed)) = decode_packet(&buf) {
                buf.drain(..consumed);
                if msg.len() < 3 {
                    continue;
                }
                let (code, data) = msg.split_at(3);
                if wanted.contains(&code) {
                    collected.insert(code.to_string(), data.to_string());
                }
                if collected.len() >= wanted.len() {
                    break 'outer;
                }
            }
        }
        Ok(collected)
    }
}

#[async_trait]
impl AudioProvider for OnkyoProvider {
    fn name(&self) -> &str {
        "onkyo"
    }

    async fn discover(&self) -> Result<Vec<AudioDevice>> {
        // Reachability probe + initial state in one round trip.
        let state = self.get_state("main").await?;
        Ok(vec![AudioDevice {
            id: Uuid::new_v4(),
            provider_id: "main".to_string(),
            name: format!("Onkyo receiver ({})", self.host),
            kind: AudioDeviceKind::Receiver,
            capabilities: AudioCapabilities {
                sources: true,
                transport: true,
                now_playing: true,
            },
            state,
        }])
    }

    async fn get_state(&self, device_id: &str) -> Result<AudioState> {
        if device_id != "main" {
            return Err(anyhow!("unknown Onkyo zone '{device_id}'"));
        }

        let base = self
            .exchange(
                &[
                    "PWRQSTN".into(),
                    "MVLQSTN".into(),
                    "AMTQSTN".into(),
                    "SLIQSTN".into(),
                ],
                &["PWR", "MVL", "AMT", "SLI"],
            )
            .await?;

        let power = base.get("PWR").map(|d| d == "01").unwrap_or(false);
        let volume = base
            .get("MVL")
            .and_then(|d| u8::from_str_radix(d, 16).ok())
            .unwrap_or(0)
            .min(100);
        let mute = base.get("AMT").map(|d| d == "01").unwrap_or(false);
        let source_code = base.get("SLI").cloned();
        let source = source_code.as_deref().map(source_name);

        // Track metadata only makes sense on the NET input while powered on.
        let now_playing = if power && source.as_deref() == Some("net") {
            let nets = self
                .exchange(
                    &[
                        "NSTQSTN".into(),
                        "NTIQSTN".into(),
                        "NATQSTN".into(),
                        "NALQSTN".into(),
                    ],
                    &["NST", "NTI", "NAT", "NAL"],
                )
                .await
                .unwrap_or_default();
            let np = NowPlaying {
                title: meta(nets.get("NTI")),
                artist: meta(nets.get("NAT")),
                album: meta(nets.get("NAL")),
                play_state: nets.get("NST").and_then(|d| parse_play_state(d)),
            };
            (np.title.is_some() || np.play_state.is_some()).then_some(np)
        } else {
            None
        };

        Ok(AudioState {
            power,
            volume,
            mute,
            source,
            now_playing,
            reachable: Some(true),
        })
    }

    async fn set_state(&self, device_id: &str, cmd: &AudioCommand) -> Result<()> {
        if device_id != "main" {
            return Err(anyhow!("unknown Onkyo zone '{device_id}'"));
        }
        if cmd.is_empty() {
            return Ok(());
        }

        // Order matters: power first (so volume/source apply out of standby),
        // source before transport (so PLAY hits the right service).
        let mut commands: Vec<String> = Vec::new();
        if let Some(on) = cmd.power {
            commands.push(if on { "PWR01" } else { "PWR00" }.to_string());
        }
        if let Some(source) = &cmd.source {
            commands.extend(source_commands(source)?);
        }
        if let Some(volume) = cmd.volume {
            commands.push(format!("MVL{:02X}", volume.min(100)));
        }
        if let Some(mute) = cmd.mute {
            commands.push(if mute { "AMT01" } else { "AMT00" }.to_string());
        }
        if let Some(transport) = cmd.transport {
            commands.push(transport_command(transport).to_string());
        }

        self.exchange(&commands, &[]).await?;
        Ok(())
    }
}

// ── Factory ─────────────────────────────────────────────────────────────────

pub struct OnkyoProviderFactory;

impl AudioProviderFactory for OnkyoProviderFactory {
    fn provider_type(&self) -> &'static str {
        "onkyo"
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn AudioProvider>> {
        Ok(Box::new(OnkyoProvider::from_credentials(credentials_json)?))
    }

    fn credentials_schema(&self) -> &'static [CredentialField] {
        &[
            CredentialField {
                name: "host",
                label: "Receiver IP address",
                kind: FieldKind::IpAddress,
                required: true,
                hint: Some(
                    "Enable Network Standby on the receiver so it can be powered on remotely",
                ),
            },
            CredentialField {
                name: "port",
                label: "Port",
                kind: FieldKind::Text,
                required: false,
                hint: Some("eISCP port, default 60128"),
            },
        ]
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    // ── Codec ────────────────────────────────────────────────────────────────

    #[test]
    fn encode_packet_produces_exact_eiscp_bytes() {
        let pkt = encode_packet("PWRQSTN");
        assert_eq!(&pkt[0..4], b"ISCP");
        assert_eq!(u32::from_be_bytes(pkt[4..8].try_into().unwrap()), 16);
        // payload = "!1PWRQSTN\r" = 10 bytes
        assert_eq!(u32::from_be_bytes(pkt[8..12].try_into().unwrap()), 10);
        assert_eq!(pkt[12], 0x01);
        assert_eq!(&pkt[13..16], &[0, 0, 0]);
        assert_eq!(&pkt[16..], b"!1PWRQSTN\r");
    }

    #[test]
    fn decode_packet_strips_prefix_and_receiver_terminators() {
        // Receivers terminate replies with \x1a\r\n rather than plain \r.
        let mut pkt = Vec::new();
        let payload = b"!1MVL1E\x1a\r\n";
        pkt.extend_from_slice(b"ISCP");
        pkt.extend_from_slice(&16u32.to_be_bytes());
        pkt.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        pkt.push(0x01);
        pkt.extend_from_slice(&[0, 0, 0]);
        pkt.extend_from_slice(payload);

        let (msg, consumed) = decode_packet(&pkt).expect("complete packet");
        assert_eq!(msg, "MVL1E");
        assert_eq!(consumed, pkt.len());
    }

    #[test]
    fn decode_packet_handles_partial_then_concatenated_buffers() {
        let a = encode_packet("PWR01");
        let b = encode_packet("AMT00");

        // Partial: not decodable yet.
        assert!(decode_packet(&a[..10]).is_none());

        // Two packets back to back decode in sequence.
        let mut joined = a.clone();
        joined.extend_from_slice(&b);
        let (m1, c1) = decode_packet(&joined).unwrap();
        assert_eq!(m1, "PWR01");
        let (m2, c2) = decode_packet(&joined[c1..]).unwrap();
        assert_eq!(m2, "AMT00");
        assert_eq!(c1 + c2, joined.len());
    }

    #[test]
    fn encode_decode_roundtrip() {
        let pkt = encode_packet("NTIBohemian Rhapsody");
        let (msg, consumed) = decode_packet(&pkt).unwrap();
        assert_eq!(msg, "NTIBohemian Rhapsody");
        assert_eq!(consumed, pkt.len());
    }

    // ── Mapping helpers ─────────────────────────────────────────────────────

    #[test]
    fn source_names_map_codes_in_both_directions() {
        assert_eq!(source_name("2B"), "net");
        assert_eq!(source_name("12"), "tv");
        assert_eq!(source_name("7F"), "7F", "unknown codes pass through");

        assert_eq!(source_commands("tv").unwrap(), vec!["SLI12"]);
        assert_eq!(source_commands("NET").unwrap(), vec!["SLI2B"]);
        assert_eq!(source_commands("0x10").unwrap(), vec!["SLI10"]);
        assert!(source_commands("kazoo").is_err());
    }

    #[test]
    fn streaming_service_selects_net_input_then_service() {
        assert_eq!(
            source_commands("spotify").unwrap(),
            vec!["SLI2B", "NSV0A0"]
        );
        assert_eq!(source_commands("tunein").unwrap(), vec!["SLI2B", "NSV0E0"]);
    }

    #[test]
    fn play_state_parses_nst_triplet() {
        assert_eq!(parse_play_state("P--"), Some(PlayState::Playing));
        assert_eq!(parse_play_state("p--"), Some(PlayState::Paused));
        assert_eq!(parse_play_state("S--"), Some(PlayState::Stopped));
        assert_eq!(parse_play_state("E--"), Some(PlayState::Stopped));
        assert_eq!(parse_play_state(""), None);
    }

    // ── Mock receiver ────────────────────────────────────────────────────────

    type Scripted = HashMap<&'static str, String>;

    /// A loopback eISCP "receiver": answers `…QSTN` queries from a scripted
    /// state table (or `N/A`), echoes accepted commands like real hardware,
    /// and records every message it receives.
    async fn spawn_mock_receiver(scripted: Scripted) -> (u16, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let recorded: Arc<Mutex<Vec<String>>> = Arc::default();
        let rec = Arc::clone(&recorded);

        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let scripted = scripted.clone();
                let rec = Arc::clone(&rec);
                tokio::spawn(async move {
                    let mut buf: Vec<u8> = Vec::new();
                    let mut chunk = [0u8; 1024];
                    loop {
                        let Ok(n) = sock.read(&mut chunk).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        while let Some((msg, consumed)) = decode_packet(&buf) {
                            buf.drain(..consumed);
                            if msg.len() < 3 {
                                continue;
                            }
                            rec.lock().await.push(msg.clone());
                            let (code, data) = msg.split_at(3);
                            let reply = if data == "QSTN" {
                                format!(
                                    "{code}{}",
                                    scripted.get(code).cloned().unwrap_or("N/A".into())
                                )
                            } else {
                                msg.clone() // echo accepted commands, like real hardware
                            };
                            let _ = sock.write_all(&encode_packet(&reply)).await;
                        }
                    }
                });
            }
        });

        (port, recorded)
    }

    fn baseline_scripted() -> Scripted {
        HashMap::from([
            ("PWR", "01".to_string()),
            ("MVL", "1E".to_string()), // 0x1E = 30
            ("AMT", "00".to_string()),
            ("SLI", "12".to_string()), // tv
        ])
    }

    // ── Provider behaviour ──────────────────────────────────────────────────

    #[tokio::test]
    async fn get_state_reads_power_volume_mute_and_source() {
        let (port, _) = spawn_mock_receiver(baseline_scripted()).await;
        let p = OnkyoProvider::new_for_test("127.0.0.1", port);

        let s = p.get_state("main").await.unwrap();
        assert!(s.power);
        assert_eq!(s.volume, 30);
        assert!(!s.mute);
        assert_eq!(s.source.as_deref(), Some("tv"));
        assert!(s.now_playing.is_none(), "no metadata away from NET input");
        assert_eq!(s.reachable, Some(true));
    }

    #[tokio::test]
    async fn get_state_on_net_input_includes_now_playing() {
        let mut scripted = baseline_scripted();
        scripted.insert("SLI", "2B".to_string());
        scripted.insert("NST", "P--".to_string());
        scripted.insert("NTI", "Bohemian Rhapsody".to_string());
        scripted.insert("NAT", "Queen".to_string());
        scripted.insert("NAL", "A Night at the Opera".to_string());
        let (port, _) = spawn_mock_receiver(scripted).await;
        let p = OnkyoProvider::new_for_test("127.0.0.1", port);

        let s = p.get_state("main").await.unwrap();
        assert_eq!(s.source.as_deref(), Some("net"));
        let np = s.now_playing.expect("now playing on NET input");
        assert_eq!(np.title.as_deref(), Some("Bohemian Rhapsody"));
        assert_eq!(np.artist.as_deref(), Some("Queen"));
        assert_eq!(np.album.as_deref(), Some("A Night at the Opera"));
        assert_eq!(np.play_state, Some(PlayState::Playing));
    }

    #[tokio::test]
    async fn get_state_treats_na_metadata_as_absent() {
        let mut scripted = baseline_scripted();
        scripted.insert("SLI", "2B".to_string());
        scripted.insert("NST", "S--".to_string());
        // NTI/NAT/NAL unscripted → mock answers "N/A".
        let (port, _) = spawn_mock_receiver(scripted).await;
        let p = OnkyoProvider::new_for_test("127.0.0.1", port);

        let s = p.get_state("main").await.unwrap();
        let np = s.now_playing.expect("play_state alone still reports");
        assert!(np.title.is_none());
        assert_eq!(np.play_state, Some(PlayState::Stopped));
    }

    #[tokio::test]
    async fn set_state_sends_power_before_volume_and_encodes_hex() {
        let (port, recorded) = spawn_mock_receiver(baseline_scripted()).await;
        let p = OnkyoProvider::new_for_test("127.0.0.1", port);

        p.set_state(
            "main",
            &AudioCommand {
                power: Some(true),
                volume: Some(40),
                mute: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // Drain delay: give the mock a beat to record.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let cmds = recorded.lock().await.clone();
        let pwr = cmds.iter().position(|c| c == "PWR01").expect("PWR01 sent");
        let mvl = cmds.iter().position(|c| c == "MVL28").expect("MVL28 sent"); // 40 = 0x28
        assert!(pwr < mvl, "power must precede volume: {cmds:?}");
        assert!(cmds.contains(&"AMT00".to_string()));
    }

    #[tokio::test]
    async fn set_state_spotify_selects_net_then_service_then_plays() {
        let (port, recorded) = spawn_mock_receiver(baseline_scripted()).await;
        let p = OnkyoProvider::new_for_test("127.0.0.1", port);

        p.set_state(
            "main",
            &AudioCommand {
                source: Some("spotify".into()),
                transport: Some(TransportCmd::Play),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;
        let cmds = recorded.lock().await.clone();
        let sli = cmds.iter().position(|c| c == "SLI2B").expect("SLI2B");
        let nsv = cmds.iter().position(|c| c == "NSV0A0").expect("NSV0A0");
        let ntc = cmds.iter().position(|c| c == "NTCPLAY").expect("NTCPLAY");
        assert!(sli < nsv && nsv < ntc, "order: {cmds:?}");
    }

    #[tokio::test]
    async fn set_state_with_unknown_source_errors_without_connecting() {
        // Port 1 — nothing listens; an early validation error must not try.
        let p = OnkyoProvider::new_for_test("127.0.0.1", 1);
        let err = p
            .set_state(
                "main",
                &AudioCommand {
                    source: Some("kazoo".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("kazoo"));
    }

    #[tokio::test]
    async fn discover_returns_single_main_zone_receiver() {
        let (port, _) = spawn_mock_receiver(baseline_scripted()).await;
        let p = OnkyoProvider::new_for_test("127.0.0.1", port);

        let devices = p.discover().await.unwrap();
        assert_eq!(devices.len(), 1);
        let d = &devices[0];
        assert_eq!(d.provider_id, "main");
        assert_eq!(d.kind, AudioDeviceKind::Receiver);
        assert!(d.capabilities.sources && d.capabilities.transport);
        assert!(d.state.power);
    }

    #[tokio::test]
    async fn get_state_against_dead_port_errors() {
        let p = OnkyoProvider::new_for_test("127.0.0.1", 1);
        assert!(p.get_state("main").await.is_err());
    }

    #[tokio::test]
    async fn unknown_zone_is_rejected() {
        let p = OnkyoProvider::new_for_test("127.0.0.1", 1);
        assert!(p.get_state("zone9").await.is_err());
        assert!(
            p.set_state("zone9", &AudioCommand::default())
                .await
                .is_err()
        );
    }

    // ── Factory ─────────────────────────────────────────────────────────────

    #[test]
    fn factory_builds_from_host_and_optional_port() {
        let f = OnkyoProviderFactory;
        assert!(f.build(r#"{"host":"10.0.0.5"}"#).is_ok());
        assert!(f.build(r#"{"host":"10.0.0.5","port":60128}"#).is_ok());
        assert!(f.build(r#"{"host":"10.0.0.5","port":"60128"}"#).is_ok());
        assert!(f.build(r#"{}"#).is_err(), "host is required");
        assert!(f.build(r#"{"host":"  "}"#).is_err(), "blank host rejected");
    }
}
