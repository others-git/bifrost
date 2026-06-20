//! Onkyo / Integra eISCP integration (TCP port 60128).
//!
//! eISCP wraps short ASCII "ISCP" messages (`!1PWR01`) in a 16-byte binary
//! header. The receiver answers queries (`!1MVLQSTN`) and *also* pushes
//! unsolicited state echoes on the same socket whenever anything changes —
//! responses and pushes are indistinguishable, so reads are "collect codes
//! until satisfied or timeout".
//!
//! A receiver honors only **one** eISCP control connection at a time, so Bifrost
//! opens exactly one socket per receiver and multiplexes everything over it: a
//! shared per-host [`OnkyoLink`] actor owns the socket, broadcasts every decoded
//! message (the push stream) and writes command batches. Reads/writes and the
//! push subscription all go through that one link — opening a second connection
//! would make the receiver drop the first, kicking the push channel.
//!
//! Key command groups used:
//! - `PWR` power, `MVL` volume (hex), `AMT` mute, `SLI` input selector
//! - `NTC` network transport (PLAY/PAUSE/STOP/TRUP/TRDN), `NSV` service select
//! - `NTI`/`NAT`/`NAL` track metadata, `NST` play status (`prs` triplet)

use crate::models::media::{
    MediaCapabilities, MediaCommand, MediaDevice, MediaDeviceKind, MediaState, NowPlaying,
    PlayState, TransportCmd,
};
use crate::providers::discovery::{DeviceDiscovery, DiscoveredDevice, ScanOptions, udp_probe};
use crate::providers::{CredentialField, FieldKind, MediaProvider, MediaProviderFactory};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

pub const DEFAULT_PORT: u16 = 60128;
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(2500);

/// The full-state query battery sent on every (re)connect and by readers, so a
/// fresh subscriber and the cache re-sync to the receiver's current state.
const STATE_QUERIES: &[&str] = &[
    "PWRQSTN", "MVLQSTN", "AMTQSTN", "SLIQSTN", "NSTQSTN", "NTIQSTN", "NATQSTN", "NALQSTN",
    "ZPWQSTN", "ZVLQSTN", "ZMTQSTN", "SLZQSTN",
];

// ── eISCP packet codec (pure functions) ─────────────────────────────────────

/// Wrap an ISCP message (e.g. `PWRQSTN`) in an eISCP packet: 16-byte header +
/// `!1<msg>\r`. The `1` is the unit-type destination (the receiver).
pub fn encode_packet(msg: &str) -> Vec<u8> {
    encode_packet_unit('1', msg)
}

/// Like [`encode_packet`] but with an explicit unit-type char. Discovery uses
/// `x` ("any device") to broadcast `!xECNQSTN`.
fn encode_packet_unit(unit: char, msg: &str) -> Vec<u8> {
    let payload = format!("!{unit}{msg}\r");
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
        let n = buf.len().min(4);
        if !buf.is_empty() && buf[0..n] != b"ISCP"[..n] {
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

/// Resolve a requested source to wire commands for one zone. Friendly input
/// names and raw hex go straight to the zone's selector; streaming service
/// names select NET first, then the service (`NSV<code>0`).
fn source_commands(zone: &ZoneCodes, requested: &str) -> Result<Vec<String>> {
    let sel = zone.selector;
    let want = requested.trim().to_ascii_lowercase();
    if let Some((code, _)) = SOURCES.iter().find(|(_, n)| *n == want) {
        return Ok(vec![format!("{sel}{code}")]);
    }
    if let Some((code, _)) = NET_SERVICES.iter().find(|(_, n)| *n == want) {
        return Ok(vec![format!("{sel}2B"), format!("NSV{code}0")]);
    }
    // Raw selector hex passthrough (e.g. "2B" or "0x2B").
    let raw = want.trim_start_matches("0x").to_ascii_uppercase();
    if raw.len() == 2 && raw.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(vec![format!("{sel}{raw}")]);
    }
    Err(anyhow!("unknown media source '{requested}'"))
}

/// Per-zone eISCP command codes. Zone 2 mirrors the main zone with its own
/// command family (`ZPW`/`ZVL`/`ZMT`/`SLZ`, transport via `NTZ`).
struct ZoneCodes {
    power: &'static str,
    volume: &'static str,
    mute: &'static str,
    selector: &'static str,
    transport: &'static str,
}

const MAIN: ZoneCodes = ZoneCodes {
    power: "PWR",
    volume: "MVL",
    mute: "AMT",
    selector: "SLI",
    transport: "NTC",
};
const ZONE2: ZoneCodes = ZoneCodes {
    power: "ZPW",
    volume: "ZVL",
    mute: "ZMT",
    selector: "SLZ",
    transport: "NTZ",
};

fn zone_codes(device_id: &str) -> Option<&'static ZoneCodes> {
    match device_id {
        "main" => Some(&MAIN),
        "zone2" => Some(&ZONE2),
        _ => None,
    }
}

/// Map any zone-specific code to (device id, canonical main-zone code) so the
/// push reader can fold all zones with one `apply_message`.
fn canonical(code: &str) -> Option<(&'static str, &'static str)> {
    match code {
        "PWR" => Some(("main", "PWR")),
        "MVL" => Some(("main", "MVL")),
        "AMT" => Some(("main", "AMT")),
        "SLI" => Some(("main", "SLI")),
        "NTI" => Some(("main", "NTI")),
        "NAT" => Some(("main", "NAT")),
        "NAL" => Some(("main", "NAL")),
        "NST" => Some(("main", "NST")),
        "ZPW" => Some(("zone2", "PWR")),
        "ZVL" => Some(("zone2", "MVL")),
        "ZMT" => Some(("zone2", "AMT")),
        "SLZ" => Some(("zone2", "SLI")),
        _ => None,
    }
}

fn transport_command(zone: &ZoneCodes, cmd: TransportCmd) -> String {
    let op = match cmd {
        TransportCmd::Play => "PLAY",
        TransportCmd::Pause => "PAUSE",
        TransportCmd::Stop => "STOP",
        TransportCmd::Next => "TRUP",
        TransportCmd::Previous => "TRDN",
        TransportCmd::Toggle => "P/P",
    };
    format!("{}{op}", zone.transport)
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

/// Fold one eISCP message into an accumulated state. Returns true when the
/// message changed something a consumer cares about (drives push events).
fn apply_message(state: &mut MediaState, code: &str, data: &str) -> bool {
    let clean = |d: &str| {
        let t = d.trim();
        (!t.is_empty() && t != "N/A").then(|| t.to_string())
    };
    match code {
        "PWR" => {
            state.power = data == "01";
            if !state.power {
                state.now_playing = None;
            }
            true
        }
        "MVL" => match u8::from_str_radix(data, 16) {
            Ok(v) => {
                state.volume = v.min(100);
                true
            }
            Err(_) => false,
        },
        "AMT" => {
            state.mute = data == "01";
            true
        }
        "SLI" => {
            let name = source_name(data);
            if name != "net" {
                state.now_playing = None; // metadata belongs to the NET input
            }
            state.source = Some(name);
            true
        }
        "NTI" | "NAT" | "NAL" => {
            let np = state.now_playing.get_or_insert_with(Default::default);
            match code {
                "NTI" => np.title = clean(data),
                "NAT" => np.artist = clean(data),
                _ => np.album = clean(data),
            }
            true
        }
        "NST" => {
            let np = state.now_playing.get_or_insert_with(Default::default);
            np.play_state = parse_play_state(data);
            true
        }
        _ => false,
    }
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

    /// Best-effort receiver MAC (cross-provider `hw_id`) from the device-info
    /// (`NRI`) reply — a chunk of XML carrying a `macaddress`. `None` if the
    /// receiver doesn't answer NRI, so discovery never fails on its account.
    async fn nri_hw_id(&self) -> Option<String> {
        let reply = self.exchange(&["NRIQSTN".into()], &["NRI"]).await.ok()?;
        parse_nri_mac(reply.get("NRI")?)
    }

    /// Send `commands` over the shared link, then collect every code in `wanted`
    /// from the link's broadcast (replies and unsolicited echoes both count)
    /// until satisfied or the timeout elapses. Returns code → latest data.
    /// `wanted` empty = fire-and-forget write. Never opens its own socket — all
    /// I/O multiplexes over the one [`OnkyoLink`] connection.
    async fn exchange(
        &self,
        commands: &[String],
        wanted: &[&str],
    ) -> Result<HashMap<String, String>> {
        let link = link_for(&self.host, self.port);
        // Subscribe before writing so a fast reply can't slip past us.
        let mut rx = link.events.subscribe();
        if !commands.is_empty() {
            let mut batch = Vec::new();
            for cmd in commands {
                batch.extend_from_slice(&encode_packet(cmd));
            }
            link.writes
                .send(batch)
                .map_err(|_| anyhow!("Onkyo link closed"))?;
        }

        let mut collected: HashMap<String, String> = HashMap::new();
        if wanted.is_empty() {
            return Ok(collected);
        }
        let deadline = tokio::time::Instant::now() + self.timeout;
        while collected.len() < wanted.len() {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok((code, data))) => {
                    if wanted.contains(&code.as_str()) {
                        collected.insert(code, data);
                    }
                }
                // Dropped a few broadcasts under load — keep waiting for ours.
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                // Link actor gone, or our own deadline — stop with what we have.
                Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => break,
            }
        }
        Ok(collected)
    }
}

// ── Shared single-connection link ───────────────────────────────────────────

/// A decoded eISCP message split into `(code, data)`, e.g. `("MVL", "1F")`.
type RawMsg = (String, String);

/// The one socket Bifrost holds to a receiver. A background actor owns it,
/// reconnecting with backoff; it broadcasts every decoded message on `events`
/// and writes batches sent on `writes`. Shared process-wide per `host:port`, so
/// the push manager and every per-request read/write use the same connection
/// instead of fighting over the receiver's single eISCP slot.
struct OnkyoLink {
    writes: mpsc::UnboundedSender<Vec<u8>>,
    events: broadcast::Sender<RawMsg>,
}

static LINKS: OnceLock<std::sync::Mutex<HashMap<String, Arc<OnkyoLink>>>> = OnceLock::new();

/// Get (or lazily create) the shared link to `host:port`.
fn link_for(host: &str, port: u16) -> Arc<OnkyoLink> {
    let map = LINKS.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let key = format!("{host}:{port}");
    let mut guard = map.lock().unwrap();
    if let Some(link) = guard.get(&key) {
        return link.clone();
    }
    let (writes_tx, writes_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (events_tx, _) = broadcast::channel::<RawMsg>(256);
    tokio::spawn(onkyo_link_actor(
        host.to_string(),
        port,
        writes_rx,
        events_tx.clone(),
    ));
    let link = Arc::new(OnkyoLink {
        writes: writes_tx,
        events: events_tx,
    });
    guard.insert(key, link.clone());
    link
}

/// Liveness/resync heartbeat. If the receiver sends nothing for this long, the
/// actor re-queries the full state battery — a reply proves the link is alive
/// **and** re-syncs the cache against any push it missed. The eISCP socket can go
/// **half-open with no FIN** (the receiver drops us on standby, or a LAN blip),
/// which leaves `read` blocking forever and freezes the cached state; the
/// heartbeat is what detects that. Short in tests so the reconnect path is fast.
const HEARTBEAT: Duration = if cfg!(test) {
    Duration::from_millis(100)
} else {
    Duration::from_secs(30)
};
/// Reconnect after this many consecutive heartbeats go unanswered (a live but
/// quiet receiver answers the very first probe, resetting the count).
const MAX_SILENT_HEARTBEATS: u32 = 2;

/// Owns the single socket: connects (refreshing full state each time), then
/// loops writing queued batches and broadcasting decoded replies/echoes.
async fn onkyo_link_actor(
    host: String,
    port: u16,
    mut writes: mpsc::UnboundedReceiver<Vec<u8>>,
    events: broadcast::Sender<RawMsg>,
) {
    let addr = format!("{host}:{port}");
    let mut attempt: u32 = 0;
    loop {
        if let Ok(Ok(mut stream)) =
            tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await
        {
            attempt = 0;
            // Re-query full state on (re)connect so subscribers and the cache resync.
            let mut init = Vec::new();
            for q in STATE_QUERIES {
                init.extend_from_slice(&encode_packet(q));
            }
            if stream.write_all(&init).await.is_ok() {
                let mut buf: Vec<u8> = Vec::with_capacity(1024);
                let mut chunk = [0u8; 1024];
                let mut silent: u32 = 0;
                loop {
                    tokio::select! {
                        biased;
                        w = writes.recv() => match w {
                            Some(bytes) => {
                                if stream.write_all(&bytes).await.is_err() {
                                    break; // socket died → reconnect
                                }
                            }
                            None => return, // all senders dropped (link gone)
                        },
                        r = tokio::time::timeout(HEARTBEAT, stream.read(&mut chunk)) => match r {
                            Ok(Ok(0)) | Ok(Err(_)) => break, // closed/errored → reconnect
                            Ok(Ok(n)) => {
                                silent = 0;
                                buf.extend_from_slice(&chunk[..n]);
                                while let Some((msg, consumed)) = decode_packet(&buf) {
                                    buf.drain(..consumed);
                                    if msg.len() < 3 {
                                        continue;
                                    }
                                    let (code, data) = msg.split_at(3);
                                    let _ = events.send((code.to_string(), data.to_string()));
                                }
                            }
                            Err(_) => {
                                // Idle for HEARTBEAT: a live receiver is just quiet,
                                // so probe it (which also re-syncs the cache); a
                                // silently half-open socket never answers, so give
                                // up after a few unanswered probes and reconnect.
                                silent += 1;
                                if silent > MAX_SILENT_HEARTBEATS {
                                    break;
                                }
                                let mut probe = Vec::new();
                                for q in STATE_QUERIES {
                                    probe.extend_from_slice(&encode_packet(q));
                                }
                                if stream.write_all(&probe).await.is_err() {
                                    break;
                                }
                            }
                        },
                    }
                }
            }
        }
        // Backoff before reconnecting (capped); writes queued meanwhile flush on reconnect.
        let delay = Duration::from_millis(250u64.saturating_mul(1 << attempt.min(6)).min(20_000));
        attempt = attempt.saturating_add(1);
        tokio::time::sleep(delay).await;
    }
}

/// Pull a MAC out of an `NRI` device-info XML payload. Handles both the child
/// element (`<macaddress>0009B0…</macaddress>`) and attribute (`macaddress="…"`)
/// shapes, returning the normalized cross-provider `hw_id`.
fn parse_nri_mac(xml: &str) -> Option<String> {
    let idx = xml.find("macaddress")?;
    // Skip past the tag/attr name, the `>` or `="`, then take the hex run.
    let after = &xml[idx + "macaddress".len()..];
    let hex: String = after
        .chars()
        .skip_while(|c| !c.is_ascii_hexdigit())
        .take_while(|c| c.is_ascii_hexdigit())
        .collect();
    crate::providers::mac_hw_id(&hex)
}

#[async_trait]
impl MediaProvider for OnkyoProvider {
    fn name(&self) -> &str {
        "onkyo"
    }

    async fn discover(&self) -> Result<Vec<MediaDevice>> {
        // Main-zone probe doubles as the reachability check.
        let main_state = self.get_state("main").await?;
        // The receiver's MAC identifies the physical unit for de-dup; the main
        // zone carries it (zone 2 is the same box, kept None to avoid a self-cluster).
        let hw_id = self.nri_hw_id().await;
        let mut devices = vec![MediaDevice {
            id: Uuid::new_v4(),
            provider_id: "main".to_string(),
            name: format!("Onkyo receiver ({})", self.host),
            kind: MediaDeviceKind::Receiver,
            capabilities: MediaCapabilities {
                sources: true,
                transport: true,
                now_playing: true,
                // eISCP can select NET services (Spotify, etc.) but can't
                // enumerate saved presets, so no favorites list.
                favorites: false,
                grouping: false,
            },
            state: main_state,
            hw_id,
        }];

        // Zone 2 exists when ZPW answers with a real power state ("00"/"01");
        // receivers without it stay silent or reply N/A.
        let probe = self.exchange(&["ZPWQSTN".into()], &["ZPW"]).await?;
        if probe.get("ZPW").map(|d| d == "00" || d == "01") == Some(true) {
            let state = self.get_state("zone2").await.unwrap_or_default();
            devices.push(MediaDevice {
                id: Uuid::new_v4(),
                provider_id: "zone2".to_string(),
                name: format!("Onkyo zone 2 ({})", self.host),
                kind: MediaDeviceKind::Zone,
                capabilities: MediaCapabilities {
                    sources: true,
                    transport: true,
                    now_playing: false,
                    favorites: false,
                    grouping: false,
                },
                state,
                hw_id: None,
            });
        }
        Ok(devices)
    }

    async fn get_state(&self, device_id: &str) -> Result<MediaState> {
        let zone =
            zone_codes(device_id).ok_or_else(|| anyhow!("unknown Onkyo zone '{device_id}'"))?;

        let base = self
            .exchange(
                &[
                    format!("{}QSTN", zone.power),
                    format!("{}QSTN", zone.volume),
                    format!("{}QSTN", zone.mute),
                    format!("{}QSTN", zone.selector),
                ],
                &[zone.power, zone.volume, zone.mute, zone.selector],
            )
            .await?;

        // Onkyo allows ~one eISCP connection, which the push manager holds; a
        // competing per-request read can come back without the volume reply.
        // Don't fabricate `volume = 0` (it would clobber the push-maintained
        // cache and zero a bound source's slider) — fail so the caller falls
        // back to the cached state instead.
        let volume = base
            .get(zone.volume)
            .and_then(|d| u8::from_str_radix(d, 16).ok())
            .with_context(|| {
                format!(
                    "Onkyo {device_id}: incomplete state read (no {})",
                    zone.volume
                )
            })?
            .min(100);
        let power = base.get(zone.power).map(|d| d == "01").unwrap_or(false);
        let mute = base.get(zone.mute).map(|d| d == "01").unwrap_or(false);
        let source_code = base.get(zone.selector).cloned();
        let source = source_code.as_deref().map(source_name);

        // Track metadata only makes sense on the NET input while powered on,
        // and the NTI/NAT/NAL feed describes the main zone's NET session.
        let now_playing = if device_id == "main" && power && source.as_deref() == Some("net") {
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

        Ok(MediaState {
            power,
            volume,
            mute,
            source,
            source_list: Vec::new(),
            now_playing,
            reachable: Some(true),
            group_coordinator: None, // receiver zones aren't sync groups
            ip: Some(self.host.clone()),
        })
    }

    async fn set_state(&self, device_id: &str, cmd: &MediaCommand) -> Result<()> {
        let zone =
            zone_codes(device_id).ok_or_else(|| anyhow!("unknown Onkyo zone '{device_id}'"))?;
        if cmd.is_empty() {
            return Ok(());
        }

        // Order matters: power first (so volume/source apply out of standby),
        // source before transport (so PLAY hits the right service).
        let mut commands: Vec<String> = Vec::new();
        if let Some(on) = cmd.power {
            commands.push(format!("{}{}", zone.power, if on { "01" } else { "00" }));
        }
        if let Some(source) = &cmd.source {
            commands.extend(source_commands(zone, source)?);
        }
        if let Some(volume) = cmd.volume {
            commands.push(format!("{}{:02X}", zone.volume, volume.min(100)));
        }
        if let Some(mute) = cmd.mute {
            commands.push(format!("{}{}", zone.mute, if mute { "01" } else { "00" }));
        }
        if let Some(transport) = cmd.transport {
            commands.push(transport_command(zone, transport));
        }

        self.exchange(&commands, &[]).await?;
        Ok(())
    }

    /// Live push channel: subscribe to the shared link and fold its broadcast
    /// of decoded messages into accumulated per-zone state snapshots. The link
    /// owns connection/reconnection, so this returns immediately (lazy) and the
    /// channel stays open across receiver reconnects — the actor re-queries full
    /// state on each (re)connect, so the stream re-syncs without the manager
    /// having to reconnect (no more per-command push-channel bounce).
    async fn event_stream(
        &self,
    ) -> Result<tokio::sync::mpsc::Receiver<crate::models::media::MediaEvent>> {
        use crate::models::media::MediaEvent;

        let link = link_for(&self.host, self.port);
        let mut rx = link.events.subscribe();
        // Nudge a refresh so a just-subscribed reader sees current state even if
        // the actor connected before this subscription existed.
        let mut init = Vec::new();
        for q in STATE_QUERIES {
            init.extend_from_slice(&encode_packet(q));
        }
        let _ = link.writes.send(init);

        let (tx, out_rx) = tokio::sync::mpsc::channel::<MediaEvent>(64);
        tokio::spawn(async move {
            // One accumulator per zone; zone codes fold via their canonical form.
            let fresh = || MediaState {
                reachable: Some(true),
                ..Default::default()
            };
            let mut main_state = fresh();
            let mut zone2_state = fresh();
            // Last snapshot emitted per zone, so we forward only real changes: a
            // heartbeat re-query (or a duplicate push) of unchanged state stays
            // silent, while a push the link missed is corrected the instant the
            // heartbeat re-reads it.
            let mut last_main: Option<MediaState> = None;
            let mut last_zone2: Option<MediaState> = None;
            loop {
                let (code, data) = match rx.recv().await {
                    Ok(m) => m,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                };
                if data == "QSTN" || data == "N/A" {
                    // A query echo, or "command unsupported" (e.g. ZPW on a
                    // receiver with no zone 2) — not state.
                    continue;
                }
                let Some((device_id, canon)) = canonical(&code) else {
                    continue;
                };
                let (state, last) = if device_id == "zone2" {
                    (&mut zone2_state, &mut last_zone2)
                } else {
                    (&mut main_state, &mut last_main)
                };
                apply_message(state, canon, &data);
                if last.as_ref() != Some(&*state) {
                    let snapshot = state.clone();
                    *last = Some(snapshot.clone());
                    if tx
                        .send(MediaEvent {
                            device_id: device_id.to_string(),
                            state: snapshot,
                        })
                        .await
                        .is_err()
                    {
                        return; // consumer dropped → stop
                    }
                }
            }
        });
        Ok(out_rx)
    }
}

// ── Network discovery ─────────────────────────────────────────────────────────

/// eISCP auto-detect: broadcast `!xECNQSTN` to UDP 60128; receivers answer with
/// an `ECN<model>/<port>/<area>/<mac>` packet from their own address.
pub struct OnkyoDiscovery {
    /// Probe target — the global broadcast address in production, a loopback
    /// mock in tests.
    target: SocketAddr,
}

impl OnkyoDiscovery {
    pub fn broadcast() -> Self {
        Self {
            target: SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), DEFAULT_PORT),
        }
    }

    #[cfg(test)]
    fn to(target: SocketAddr) -> Self {
        Self { target }
    }
}

#[async_trait]
impl DeviceDiscovery for OnkyoDiscovery {
    async fn scan(&self, opts: &ScanOptions) -> Result<Vec<DiscoveredDevice>> {
        // eISCP discovery is a broadcast; Expanded-LAN subnets don't apply.
        let probe = encode_packet_unit('x', "ECNQSTN");
        let replies = udp_probe(self.target, &probe, opts.timeout).await?;

        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for (from, bytes) in replies {
            let Some((msg, _)) = decode_packet(&bytes) else {
                continue;
            };
            let Some(rest) = msg.strip_prefix("ECN") else {
                continue;
            };
            let host = from.ip().to_string();
            if !seen.insert(host.clone()) {
                continue;
            }
            let model = rest
                .split('/')
                .next()
                .filter(|m| !m.is_empty())
                .map(|m| m.to_string());
            out.push(DiscoveredDevice {
                label: model.map(|m| format!("Onkyo {m}")),
                credentials: serde_json::json!({ "host": host }),
                host,
            });
        }
        Ok(out)
    }
}

// ── Factory ─────────────────────────────────────────────────────────────────

pub struct OnkyoProviderFactory;

impl MediaProviderFactory for OnkyoProviderFactory {
    fn provider_type(&self) -> &'static str {
        "onkyo"
    }

    fn display_name(&self) -> &'static str {
        "Onkyo / Integra"
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn MediaProvider>> {
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

    fn connection_mode(&self) -> crate::providers::MediaConnectionMode {
        // The receiver echoes every state change unsolicited on the open socket.
        crate::providers::MediaConnectionMode::Push
    }

    fn discoverer(&self) -> Option<Box<dyn DeviceDiscovery>> {
        Some(Box::new(OnkyoDiscovery::broadcast()))
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    #[test]
    fn parse_nri_mac_handles_element_and_attribute_shapes() {
        // Child-element form.
        assert_eq!(
            parse_nri_mac(
                "<device><model>NR686</model><macaddress>0009B012ABCD</macaddress></device>"
            ),
            Some("mac:0009b012abcd".to_string())
        );
        // Attribute form.
        assert_eq!(
            parse_nri_mac(r#"<device id="x" macaddress="0009B012ABCD"/>"#),
            Some("mac:0009b012abcd".to_string())
        );
        // No MAC present, or a junk reply (e.g. "N/A") → None.
        assert_eq!(parse_nri_mac("<device><model>NR686</model></device>"), None);
        assert_eq!(parse_nri_mac("N/A"), None);
    }

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

        assert_eq!(source_commands(&MAIN, "tv").unwrap(), vec!["SLI12"]);
        assert_eq!(source_commands(&MAIN, "NET").unwrap(), vec!["SLI2B"]);
        assert_eq!(source_commands(&MAIN, "0x10").unwrap(), vec!["SLI10"]);
        assert!(source_commands(&MAIN, "kazoo").is_err());
        // Zone 2 uses its own selector family.
        assert_eq!(source_commands(&ZONE2, "tv").unwrap(), vec!["SLZ12"]);
    }

    #[test]
    fn streaming_service_selects_net_input_then_service() {
        assert_eq!(
            source_commands(&MAIN, "spotify").unwrap(),
            vec!["SLI2B", "NSV0A0"]
        );
        assert_eq!(
            source_commands(&MAIN, "tunein").unwrap(),
            vec!["SLI2B", "NSV0E0"]
        );
    }

    #[test]
    fn canonical_maps_zone2_codes_to_main_equivalents() {
        assert_eq!(canonical("ZVL"), Some(("zone2", "MVL")));
        assert_eq!(canonical("ZPW"), Some(("zone2", "PWR")));
        assert_eq!(canonical("MVL"), Some(("main", "MVL")));
        assert_eq!(canonical("XYZ"), None);
    }

    #[test]
    fn play_state_parses_nst_triplet() {
        assert_eq!(parse_play_state("P--"), Some(PlayState::Playing));
        assert_eq!(parse_play_state("p--"), Some(PlayState::Paused));
        assert_eq!(parse_play_state("S--"), Some(PlayState::Stopped));
        assert_eq!(parse_play_state("E--"), Some(PlayState::Stopped));
        assert_eq!(parse_play_state(""), None);
    }

    #[test]
    fn apply_message_folds_codes_into_state() {
        let mut s = MediaState::default();
        assert!(apply_message(&mut s, "PWR", "01"));
        assert!(apply_message(&mut s, "MVL", "28"));
        assert!(apply_message(&mut s, "AMT", "00"));
        assert!(apply_message(&mut s, "SLI", "2B"));
        assert!(apply_message(&mut s, "NTI", "Paranoid Android"));
        assert!(apply_message(&mut s, "NST", "P--"));
        assert!(!apply_message(&mut s, "XYZ", "whatever"));

        assert!(s.power);
        assert_eq!(s.volume, 40);
        assert!(!s.mute);
        assert_eq!(s.source.as_deref(), Some("net"));
        let np = s.now_playing.as_ref().unwrap();
        assert_eq!(np.title.as_deref(), Some("Paranoid Android"));
        assert_eq!(np.play_state, Some(PlayState::Playing));

        // Switching away from NET clears the metadata; power-off too.
        assert!(apply_message(&mut s, "SLI", "12"));
        assert!(s.now_playing.is_none());
        assert!(!apply_message(&mut s, "MVL", "ZZ"), "bad hex ignored");
        assert!(apply_message(&mut s, "PWR", "00"));
        assert!(!s.power);
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
    async fn get_state_errors_when_volume_reply_is_missing() {
        // A read that doesn't return a usable MVL (here the receiver answers
        // "N/A", as it would under eISCP connection contention) must error —
        // not report volume 0, which would clobber the push-maintained cache
        // and zero a bound source's slider.
        let mut scripted = baseline_scripted();
        scripted.remove("MVL");
        let (port, _) = spawn_mock_receiver(scripted).await;
        let p = OnkyoProvider::new_for_test("127.0.0.1", port);

        assert!(p.get_state("main").await.is_err());
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
            &MediaCommand {
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
            &MediaCommand {
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
                &MediaCommand {
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
        // ZPW unscripted → mock answers N/A → no zone 2.
        let (port, _) = spawn_mock_receiver(baseline_scripted()).await;
        let p = OnkyoProvider::new_for_test("127.0.0.1", port);

        let devices = p.discover().await.unwrap();
        assert_eq!(devices.len(), 1);
        let d = &devices[0];
        assert_eq!(d.provider_id, "main");
        assert_eq!(d.kind, MediaDeviceKind::Receiver);
        assert!(d.capabilities.sources && d.capabilities.transport);
        assert!(d.state.power);
        // No NRI scripted → the receiver reports no MAC, so no hw_id.
        assert!(d.hw_id.is_none());
    }

    #[tokio::test]
    async fn discover_populates_hw_id_from_nri_macaddress() {
        let mut scripted = baseline_scripted();
        scripted.insert(
            "NRI",
            "<device><model>NR686</model><macaddress>0009B012ABCD</macaddress></device>".into(),
        );
        let (port, _) = spawn_mock_receiver(scripted).await;
        let p = OnkyoProvider::new_for_test("127.0.0.1", port);

        let devices = p.discover().await.unwrap();
        assert_eq!(devices[0].hw_id.as_deref(), Some("mac:0009b012abcd"));
    }

    fn with_zone2(mut scripted: Scripted) -> Scripted {
        scripted.insert("ZPW", "01".to_string());
        scripted.insert("ZVL", "14".to_string()); // 0x14 = 20
        scripted.insert("ZMT", "01".to_string());
        scripted.insert("SLZ", "23".to_string()); // cd
        scripted
    }

    #[tokio::test]
    async fn discover_includes_zone2_when_receiver_reports_it() {
        let (port, _) = spawn_mock_receiver(with_zone2(baseline_scripted())).await;
        let p = OnkyoProvider::new_for_test("127.0.0.1", port);

        let devices = p.discover().await.unwrap();
        assert_eq!(devices.len(), 2);
        let z2 = &devices[1];
        assert_eq!(z2.provider_id, "zone2");
        assert_eq!(z2.kind, MediaDeviceKind::Zone);
        assert!(
            !z2.capabilities.now_playing,
            "NET metadata is main-zone only"
        );
        assert!(z2.state.power);
        assert_eq!(z2.state.volume, 20);
        assert!(z2.state.mute);
        assert_eq!(z2.state.source.as_deref(), Some("cd"));
    }

    #[tokio::test]
    async fn zone2_set_state_uses_zone_command_family() {
        let (port, recorded) = spawn_mock_receiver(with_zone2(baseline_scripted())).await;
        let p = OnkyoProvider::new_for_test("127.0.0.1", port);

        p.set_state(
            "zone2",
            &MediaCommand {
                power: Some(true),
                volume: Some(25),
                mute: Some(false),
                source: Some("bd".into()),
                transport: Some(TransportCmd::Play),
            },
        )
        .await
        .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;
        let cmds = recorded.lock().await.clone();
        assert!(cmds.contains(&"ZPW01".to_string()), "{cmds:?}");
        assert!(cmds.contains(&"ZVL19".to_string()), "25 = 0x19: {cmds:?}");
        assert!(cmds.contains(&"ZMT00".to_string()), "{cmds:?}");
        assert!(cmds.contains(&"SLZ10".to_string()), "{cmds:?}");
        assert!(cmds.contains(&"NTZPLAY".to_string()), "{cmds:?}");
    }

    #[tokio::test]
    async fn event_stream_routes_zone2_codes_to_zone2_device() {
        let (port, _) = spawn_mock_receiver(with_zone2(baseline_scripted())).await;
        let p = OnkyoProvider::new_for_test("127.0.0.1", port);

        let mut rx = p.event_stream().await.unwrap();
        let mut last_zone2 = None;
        while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await {
            if ev.device_id == "zone2" {
                last_zone2 = Some(ev);
            }
        }
        let ev = last_zone2.expect("zone2 events from the initial QSTN battery");
        assert!(ev.state.power);
        assert_eq!(ev.state.volume, 20);
        assert_eq!(ev.state.source.as_deref(), Some("cd"));
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
            p.set_state("zone9", &MediaCommand::default())
                .await
                .is_err()
        );
    }

    // ── Push channel ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn event_stream_emits_initial_state_then_unsolicited_updates() {
        let (port, _) = spawn_mock_receiver(baseline_scripted()).await;
        let p = OnkyoProvider::new_for_test("127.0.0.1", port);

        let mut rx = p.event_stream().await.unwrap();

        // The initial QSTN battery folds in reply by reply — drain until the
        // channel goes quiet and inspect the final accumulated snapshot.
        let mut latest = None;
        while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await {
            latest = Some(ev);
        }
        let ev = latest.expect("initial events");
        assert_eq!(ev.device_id, "main");
        assert!(ev.state.power);
        assert_eq!(ev.state.volume, 30);
        assert_eq!(ev.state.source.as_deref(), Some("tv"));

        // The channel must stay open (idle ≠ closed); unsolicited-push
        // forwarding is covered by the MediaPushManager scripted test.
        assert!(
            tokio::time::timeout(Duration::from_millis(100), rx.recv())
                .await
                .is_err(),
            "channel should be idle but open"
        );
    }

    #[tokio::test]
    async fn event_stream_against_dead_port_yields_no_events() {
        // The link connects (and reconnects) in the background, so subscribing
        // always succeeds; a dead receiver simply produces no events.
        let p = OnkyoProvider::new_for_test("127.0.0.1", 1);
        let mut rx = p.event_stream().await.expect("subscribe is lazy");
        assert!(
            tokio::time::timeout(Duration::from_millis(150), rx.recv())
                .await
                .is_err(),
            "a dead receiver yields no events"
        );
    }

    #[tokio::test]
    async fn reads_and_push_share_one_connection() {
        // The core M22/Onkyo fix: a receiver honors ~one eISCP connection, so a
        // read while the push stream is live must reuse the same socket, never
        // open a second one (which would kick the push channel).
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let conns = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&conns);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let scripted = baseline_scripted();
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
                            let (code, data) = msg.split_at(3);
                            let reply = if data == "QSTN" {
                                format!(
                                    "{code}{}",
                                    scripted.get(code).cloned().unwrap_or("N/A".into())
                                )
                            } else {
                                msg.clone()
                            };
                            let _ = sock.write_all(&encode_packet(&reply)).await;
                        }
                    }
                });
            }
        });

        let p = OnkyoProvider::new_for_test("127.0.0.1", port);
        let _push = p.event_stream().await.unwrap(); // holds the shared link
        for _ in 0..3 {
            let _ = p.get_state("main").await; // reads multiplex over the same link
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            conns.load(Ordering::SeqCst),
            1,
            "all I/O must share one connection"
        );
    }

    #[tokio::test]
    async fn link_reconnects_when_the_receiver_goes_silent() {
        // A receiver whose eISCP socket goes half-open *without* a FIN (it drops
        // us on standby, or a LAN blip) must be detected by the heartbeat and
        // reconnected, re-syncing fresh state — instead of `read` blocking forever
        // and the cache freezing on the last value (the 0.27.x Onkyo "isn't
        // sync'd" bug). The first connection answers once then falls silent; the
        // reconnect reports a new volume the push channel must surface.
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let conns = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&conns);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let nth = counter.fetch_add(1, Ordering::SeqCst); // 0 = first connection
                tokio::spawn(async move {
                    let vol = if nth == 0 { "1E" } else { "28" }; // 30, then 40
                    let mut answered = false;
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
                            if nth == 0 && answered {
                                continue; // first connection falls silent after one reply
                            }
                            let (code, data) = msg.split_at(3);
                            if data != "QSTN" {
                                continue;
                            }
                            let v = match code {
                                "MVL" => vol,
                                "PWR" => "01",
                                "AMT" => "00",
                                "SLI" => "12",
                                _ => "N/A",
                            };
                            let _ = sock.write_all(&encode_packet(&format!("{code}{v}"))).await;
                            answered = true;
                        }
                    }
                });
            }
        });

        let p = OnkyoProvider::new_for_test("127.0.0.1", port);
        let mut rx = p.event_stream().await.unwrap();

        // The reconnect resync must surface the new volume (40 = 0x28).
        let mut saw_new_volume = false;
        for _ in 0..30 {
            match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
                Ok(Some(ev)) if ev.state.volume == 40 => {
                    saw_new_volume = true;
                    break;
                }
                Ok(Some(_)) => {}
                Ok(None) => break, // channel closed
                Err(_) => {}       // idle tick — keep waiting through the reconnect
            }
        }
        assert!(
            saw_new_volume,
            "the link must reconnect after silence and resync the new volume"
        );
        assert!(
            conns.load(Ordering::SeqCst) >= 2,
            "the dead link should have been reconnected"
        );
    }

    // ── Network discovery ────────────────────────────────────────────────────

    /// A loopback "receiver" that answers one discovery probe with an ECN packet.
    async fn spawn_discovery_responder(ecn: &'static str) -> SocketAddr {
        let sock = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = sock.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            if let Ok((_, from)) = sock.recv_from(&mut buf).await {
                let _ = sock.send_to(&encode_packet(ecn), from).await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn discovery_parses_ecn_reply_into_host_and_model() {
        let addr = spawn_discovery_responder("ECNTX-NR686/60128/DX/001122334455").await;
        let found = OnkyoDiscovery::to(addr)
            .scan(&ScanOptions::new(Duration::from_millis(300)))
            .await
            .unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].host, "127.0.0.1");
        assert_eq!(found[0].label.as_deref(), Some("Onkyo TX-NR686"));
        assert_eq!(
            found[0].credentials,
            serde_json::json!({ "host": "127.0.0.1" })
        );
    }

    #[tokio::test]
    async fn discovery_returns_empty_when_no_receiver_answers() {
        let found = OnkyoDiscovery::to(SocketAddr::from((Ipv4Addr::LOCALHOST, 1)))
            .scan(&ScanOptions::new(Duration::from_millis(150)))
            .await
            .unwrap();
        assert!(found.is_empty());
    }

    // ── Factory ─────────────────────────────────────────────────────────────

    #[test]
    fn factory_exposes_a_discoverer() {
        use crate::providers::MediaProviderFactory as _;
        assert!(OnkyoProviderFactory.discoverer().is_some());
    }

    #[test]
    fn factory_advertises_push_mode() {
        use crate::providers::{MediaConnectionMode, MediaProviderFactory as _};
        assert_eq!(
            OnkyoProviderFactory.connection_mode(),
            MediaConnectionMode::Push
        );
    }

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
