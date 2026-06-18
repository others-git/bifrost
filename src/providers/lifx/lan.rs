//! LIFX LAN protocol — local binary UDP control, no cloud / token.
//!
//! LIFX bulbs speak a documented binary protocol over UDP **56700**. Each message
//! is a 36-byte header (little-endian) + a typed payload:
//!   - **Frame** (8B): `size` (u16), a packed `u16` of protocol(1024)+addressable+
//!     tagged+origin, then `source` (u32).
//!   - **Frame address** (16B): `target` (u64 — the 6-byte MAC, low bytes first),
//!     6 reserved, a flags byte (`res_required`/`ack_required`), then `sequence`.
//!   - **Protocol header** (12B): 8 reserved, `type` (u16), 2 reserved.
//!
//! We discover with a broadcast `GetService` (bulbs reply `StateService` carrying
//! their UDP port), read with `Get`→`State` (HSBK + power + label), and write with
//! `SetColor` + `SetPower`. Devices are keyed by **MAC** — the same id the LIFX
//! cloud uses (`d073d5xxxxxx`), so the two transports unify without a migration.
//!
//! Like the Govee LAN provider, the broadcast target and timeout are injectable so
//! tests drive the codec against a loopback mock bulb instead of real hardware.
//!
//! Deployment note: in Docker this needs host networking for broadcast to reach
//! the LAN.

use crate::models::{Color, Light, LightCapabilities, LightState, Provider};
use crate::providers::{LightProvider, ProviderGroup};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, OnceCell};
use uuid::Uuid;

const LIFX_PORT: u16 = 56700;
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1500);
/// Our `source` id, echoed by bulbs in replies (any nonzero value; "BIFR").
const SOURCE: u32 = 0x4249_4652;

// Message type numbers (LIFX protocol).
const MSG_GET_SERVICE: u16 = 2;
const MSG_STATE_SERVICE: u16 = 3;
const MSG_GET: u16 = 101;
const MSG_SET_COLOR: u16 = 102;
const MSG_STATE: u16 = 107;
const MSG_SET_POWER: u16 = 117;

const HEADER_LEN: usize = 36;

/// Process-wide `MAC → device address` map. Global (MACs are unique) and
/// process-lived because the provider is rebuilt per control request, so an
/// instance map would never survive to the next call — control would re-scan
/// (1.5s) every time. Only scanned bulbs appear, so membership is the per-device
/// LAN-eligibility gate. Mirrors the Govee LAN provider's `lan_ip_cache`.
fn addr_cache() -> &'static tokio::sync::RwLock<HashMap<String, SocketAddr>> {
    static CACHE: std::sync::OnceLock<tokio::sync::RwLock<HashMap<String, SocketAddr>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| tokio::sync::RwLock::new(HashMap::new()))
}

#[cfg(test)]
pub(crate) async fn clear_addr_cache() {
    addr_cache().write().await.clear();
}

/// A LIFX HSBK colour: hue/saturation/brightness as 0–65535, plus kelvin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hsbk {
    hue: u16,
    saturation: u16,
    brightness: u16,
    kelvin: u16,
}

impl Hsbk {
    fn to_bytes(self) -> [u8; 8] {
        let mut b = [0u8; 8];
        b[0..2].copy_from_slice(&self.hue.to_le_bytes());
        b[2..4].copy_from_slice(&self.saturation.to_le_bytes());
        b[4..6].copy_from_slice(&self.brightness.to_le_bytes());
        b[6..8].copy_from_slice(&self.kelvin.to_le_bytes());
        b
    }
}

/// Encode one LIFX message: header + payload. `target` is the device MAC (`None`
/// for a tagged broadcast). `res_required` asks the bulb to send a state reply.
fn build_message(
    target: Option<[u8; 6]>,
    tagged: bool,
    res_required: bool,
    msg_type: u16,
    payload: &[u8],
    sequence: u8,
) -> Vec<u8> {
    let size = (HEADER_LEN + payload.len()) as u16;
    let mut b = Vec::with_capacity(size as usize);
    // ── Frame ──
    b.extend_from_slice(&size.to_le_bytes());
    // protocol = 1024, addressable bit 12, tagged bit 13, origin (bits 14-15) = 0.
    let mut flags: u16 = 1024 | (1 << 12);
    if tagged {
        flags |= 1 << 13;
    }
    b.extend_from_slice(&flags.to_le_bytes());
    b.extend_from_slice(&SOURCE.to_le_bytes());
    // ── Frame address ──
    let mut tgt = [0u8; 8];
    if let Some(mac) = target {
        tgt[..6].copy_from_slice(&mac);
    }
    b.extend_from_slice(&tgt);
    b.extend_from_slice(&[0u8; 6]); // reserved
    b.push(if res_required { 1 } else { 0 }); // res_required bit 0 (ack bit 1)
    b.push(sequence);
    // ── Protocol header ──
    b.extend_from_slice(&[0u8; 8]); // reserved
    b.extend_from_slice(&msg_type.to_le_bytes());
    b.extend_from_slice(&[0u8; 2]); // reserved
    b.extend_from_slice(payload);
    b
}

/// Decode a message into `(mac, type, payload)`, or `None` if it's too short.
fn decode(buf: &[u8]) -> Option<([u8; 6], u16, &[u8])> {
    if buf.len() < HEADER_LEN {
        return None;
    }
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&buf[8..14]);
    let msg_type = u16::from_le_bytes([buf[32], buf[33]]);
    Some((mac, msg_type, &buf[HEADER_LEN..]))
}

/// `[0xd0,0x73,…]` → `"d073d5000001"` (the cloud-matching device id).
fn mac_hex(mac: &[u8; 6]) -> String {
    mac.iter().map(|b| format!("{b:02x}")).collect()
}

/// `"d073d5000001"` → `[0xd0,0x73,…]`; `None` if it isn't 12 hex chars.
fn mac_bytes(hex: &str) -> Option<[u8; 6]> {
    let hex: String = hex.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 12 {
        return None;
    }
    let mut out = [0u8; 6];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// A `SetColor` payload: reserved, HSBK, duration (ms).
fn set_color_payload(hsbk: Hsbk) -> Vec<u8> {
    let mut p = Vec::with_capacity(13);
    p.push(0); // reserved
    p.extend_from_slice(&hsbk.to_bytes());
    p.extend_from_slice(&0u32.to_le_bytes()); // duration 0
    p
}

/// A `SetPower` payload: level (0 or 65535), duration (ms).
fn set_power_payload(on: bool) -> Vec<u8> {
    let mut p = Vec::with_capacity(6);
    p.extend_from_slice(&(if on { 65535u16 } else { 0 }).to_le_bytes());
    p.extend_from_slice(&0u32.to_le_bytes());
    p
}

/// Map a Bifrost `LightState` to the HSBK a `SetColor` carries. Colour wins over
/// temperature (the two are mutually exclusive); brightness scales 0–100 → 0–65535.
fn state_to_hsbk(state: &LightState) -> Hsbk {
    let brightness = ((state.brightness.unwrap_or(100.0) / 100.0).clamp(0.0, 1.0) * 65535.0) as u16;
    if let Some(color) = &state.color {
        let (r, g, b) = color.to_rgb();
        let (hue, sat) = super::rgb_to_hs(r, g, b); // hue 0–360, sat 0–1
        Hsbk {
            hue: (hue / 360.0 * 65535.0) as u16,
            saturation: (sat.clamp(0.0, 1.0) * 65535.0) as u16,
            brightness,
            kelvin: 3500,
        }
    } else if let Some(mirek) = state.color_temp_mirek {
        let kelvin = (1_000_000u32 / u32::from(mirek.max(1))).clamp(1500, 9000) as u16;
        Hsbk {
            hue: 0,
            saturation: 0,
            brightness,
            kelvin,
        }
    } else {
        Hsbk {
            hue: 0,
            saturation: 0,
            brightness,
            kelvin: 3500,
        }
    }
}

/// Parse a `State` (107) payload into `(LightState, label)`. A saturated colour
/// maps to an RGB colour; an unsaturated one to a white temperature.
fn parse_state(p: &[u8]) -> Option<(LightState, String)> {
    if p.len() < 44 {
        return None;
    }
    let hue = u16::from_le_bytes([p[0], p[1]]);
    let saturation = u16::from_le_bytes([p[2], p[3]]);
    let brightness = u16::from_le_bytes([p[4], p[5]]);
    let kelvin = u16::from_le_bytes([p[6], p[7]]);
    let power = u16::from_le_bytes([p[10], p[11]]);
    let label = String::from_utf8_lossy(&p[12..44])
        .trim_end_matches('\0')
        .trim()
        .to_string();

    let mut state = LightState {
        on: power > 0,
        brightness: Some((f32::from(brightness) / 65535.0 * 100.0).round()),
        reachable: Some(true),
        ..Default::default()
    };
    if saturation > 0 {
        let (r, g, b) = super::hsv_to_rgb(
            f32::from(hue) / 65535.0 * 360.0,
            f32::from(saturation) / 65535.0,
            1.0,
        );
        state.color = Some(Color::from_rgb(r, g, b));
    } else {
        state.color_temp_mirek = Some((1_000_000u32 / u32::from(kelvin.max(1))) as u16);
    }
    Some((state, label))
}

fn lan_light(mac: &str, label: &str) -> Light {
    let name = if label.is_empty() {
        format!("LIFX {mac}")
    } else {
        label.to_string()
    };
    Light {
        id: Uuid::new_v4(),
        hw_id: crate::providers::mac_hw_id(mac),
        provider_id: mac.to_string(),
        provider: Provider::Lifx,
        name,
        // Over LAN we can't cheaply read product caps; LIFX colour bulbs are RGB +
        // tunable white + dimmable. (Effects come from the cloud transport.)
        capabilities: LightCapabilities {
            dimmable: true,
            color_rgb: true,
            color_temperature: true,
            hue_gamut: None,
            effects: Vec::new(),
            segments: None,
        },
        state: LightState {
            transport: Some("lan".to_string()),
            ..Default::default()
        },
        last_seen: chrono::Utc::now(),
    }
}

/// LIFX LAN transport. Owns one UDP socket; addresses each bulb by MAC, resolving
/// the MAC→address from a broadcast scan (cached, refreshed on a miss).
pub struct LifxLanProvider {
    bind_addr: IpAddr,
    /// Where `GetService` is broadcast (255.255.255.255:56700 in production).
    broadcast: SocketAddr,
    timeout: Duration,
    socket: OnceCell<UdpSocket>,
    /// Serialises request/response exchanges over the shared socket.
    exchange: Mutex<()>,
    sequence: AtomicU8,
}

impl LifxLanProvider {
    /// Production provider bound to `bind_addr`, broadcasting on the LIFX port.
    pub fn new(bind_addr: IpAddr) -> Self {
        Self {
            bind_addr,
            broadcast: SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), LIFX_PORT),
            timeout: DEFAULT_TIMEOUT,
            socket: OnceCell::new(),
            exchange: Mutex::new(()),
            sequence: AtomicU8::new(0),
        }
    }

    /// Test constructor: point discovery + control at a loopback mock bulb.
    #[cfg(test)]
    pub(crate) fn new_for_test(mock_addr: SocketAddr, timeout: Duration) -> Self {
        Self {
            bind_addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
            broadcast: mock_addr,
            timeout,
            socket: OnceCell::new(),
            exchange: Mutex::new(()),
            sequence: AtomicU8::new(0),
        }
    }

    fn next_seq(&self) -> u8 {
        self.sequence.fetch_add(1, Ordering::Relaxed)
    }

    async fn socket(&self) -> Result<&UdpSocket> {
        self.socket
            .get_or_try_init(|| async {
                let sock = UdpSocket::bind((self.bind_addr, 0))
                    .await
                    .with_context(|| format!("binding LIFX LAN socket to {}:0", self.bind_addr))?;
                // Required to send to the 255.255.255.255 broadcast address.
                sock.set_broadcast(true)
                    .context("enabling broadcast on the LIFX LAN socket")?;
                Ok::<_, anyhow::Error>(sock)
            })
            .await
    }

    /// Broadcast `GetService` and collect each bulb's MAC + address, refreshing the
    /// cache. This is the LAN-capability probe (only responders are LAN-eligible).
    pub async fn scan(&self) -> Result<Vec<(String, SocketAddr)>> {
        let sock = self.socket().await?;
        let _guard = self.exchange.lock().await;
        let msg = build_message(None, true, true, MSG_GET_SERVICE, &[], self.next_seq());
        sock.send_to(&msg, self.broadcast)
            .await
            .context("sending LIFX GetService")?;

        let deadline = Instant::now() + self.timeout;
        let mut found: HashMap<String, SocketAddr> = HashMap::new();
        let mut buf = [0u8; 1024];
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, sock.recv_from(&mut buf)).await {
                Ok(Ok((n, src))) => {
                    if let Some((mac, ty, payload)) = decode(&buf[..n])
                        && ty == MSG_STATE_SERVICE
                        && payload.len() >= 5
                        && payload[0] == 1
                    // service 1 = UDP
                    {
                        let port =
                            u32::from_le_bytes([payload[1], payload[2], payload[3], payload[4]]);
                        let addr = SocketAddr::new(src.ip(), port as u16);
                        found.entry(mac_hex(&mac)).or_insert(addr);
                    }
                }
                Ok(Err(e)) => return Err(e).context("receiving LIFX StateService"),
                Err(_) => break, // window elapsed
            }
        }
        if !found.is_empty() {
            let mut cache = addr_cache().write().await;
            for (mac, addr) in &found {
                cache.insert(mac.clone(), *addr);
            }
        }
        tracing::debug!(
            target: "bifrost::lifx",
            broadcast = %self.broadcast,
            replies = found.len(),
            bulbs = ?found.iter().map(|(m, a)| format!("{m}@{a}")).collect::<Vec<_>>(),
            "LIFX LAN scan: {} bulb(s) answered",
            found.len(),
        );
        Ok(found.into_iter().collect())
    }

    /// Resolve a MAC's current address, re-scanning on a cache miss.
    async fn addr_for(&self, mac: &str) -> Option<SocketAddr> {
        if let Some(addr) = addr_cache().read().await.get(mac).copied() {
            return Some(addr);
        }
        self.scan().await.ok()?;
        addr_cache().read().await.get(mac).copied()
    }

    /// Send `Get` and await the matching `State`, returning `(state, label)`.
    async fn query(&self, mac: &str, addr: SocketAddr) -> Result<Option<(LightState, String)>> {
        let target = mac_bytes(mac).context("invalid LIFX MAC")?;
        let sock = self.socket().await?;
        let _guard = self.exchange.lock().await;
        let msg = build_message(Some(target), false, true, MSG_GET, &[], self.next_seq());
        sock.send_to(&msg, addr).await.context("sending LIFX Get")?;

        let deadline = Instant::now() + self.timeout;
        let mut buf = [0u8; 1024];
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, sock.recv_from(&mut buf)).await {
                Ok(Ok((n, _src))) => {
                    if let Some((_mac, ty, payload)) = decode(&buf[..n])
                        && ty == MSG_STATE
                        && let Some(parsed) = parse_state(payload)
                    {
                        return Ok(Some(parsed));
                    }
                }
                Ok(Err(e)) => return Err(e).context("receiving LIFX State"),
                Err(_) => break,
            }
        }
        Ok(None)
    }
}

#[async_trait]
impl LightProvider for LifxLanProvider {
    fn name(&self) -> &str {
        "lifx-lan"
    }

    async fn discover(&self) -> Result<Vec<Light>> {
        let mut lights = Vec::new();
        for (mac, addr) in self.scan().await? {
            // Best-effort label/state read; a non-answering bulb still lists.
            let label = match self.query(&mac, addr).await {
                Ok(Some((_, label))) => label,
                _ => String::new(),
            };
            lights.push(lan_light(&mac, &label));
        }
        Ok(lights)
    }

    async fn set_state(&self, device_id: &str, state: &LightState) -> Result<()> {
        let Some(addr) = self.addr_for(device_id).await else {
            bail!("LIFX device {device_id} not found on the LAN");
        };
        let target = mac_bytes(device_id).context("invalid LIFX MAC")?;
        let sock = self.socket().await?;
        let _guard = self.exchange.lock().await;
        // Colour first, then power, so turning on lands on the chosen colour.
        let color = build_message(
            Some(target),
            false,
            false,
            MSG_SET_COLOR,
            &set_color_payload(state_to_hsbk(state)),
            self.next_seq(),
        );
        sock.send_to(&color, addr)
            .await
            .context("sending LIFX SetColor")?;
        let power = build_message(
            Some(target),
            false,
            false,
            MSG_SET_POWER,
            &set_power_payload(state.on),
            self.next_seq(),
        );
        sock.send_to(&power, addr)
            .await
            .context("sending LIFX SetPower")?;
        Ok(())
    }

    async fn get_state(&self, device_id: &str) -> Result<LightState> {
        if let Some(addr) = self.addr_for(device_id).await
            && let Some((mut state, _label)) = self.query(device_id, addr).await?
        {
            state.transport = Some("lan".to_string());
            return Ok(state);
        }
        // No reply → unreachable on the LAN.
        Ok(LightState {
            on: false,
            reachable: Some(false),
            ..Default::default()
        })
    }

    async fn discover_groups(&self) -> Result<Vec<ProviderGroup>> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOCK_MAC: [u8; 6] = [0xd0, 0x73, 0xd5, 0x12, 0x34, 0x56];
    const MOCK_MAC_HEX: &str = "d073d5123456";

    /// Recorded `(message type, payload)` pairs the mock bulb received.
    type Recorded = std::sync::Arc<Mutex<Vec<(u16, Vec<u8>)>>>;

    /// A loopback stand-in for a LIFX bulb: answers GetService + Get, and records
    /// the SetColor / SetPower payloads it receives for assertions.
    struct MockBulb {
        addr: SocketAddr,
        received: Recorded,
    }

    async fn spawn_mock_bulb(label: &str, power_on: bool, saturation: u16) -> MockBulb {
        let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = sock.local_addr().unwrap();
        let received = std::sync::Arc::new(Mutex::new(Vec::new()));
        let rec = received.clone();
        let label = label.to_string();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            loop {
                let Ok((n, src)) = sock.recv_from(&mut buf).await else {
                    break;
                };
                let Some((_mac, ty, payload)) = decode(&buf[..n]) else {
                    continue;
                };
                match ty {
                    MSG_GET_SERVICE => {
                        let mut p = Vec::new();
                        p.push(1u8); // service = UDP
                        p.extend_from_slice(&(addr.port() as u32).to_le_bytes());
                        let reply =
                            build_message(Some(MOCK_MAC), false, false, MSG_STATE_SERVICE, &p, 0);
                        let _ = sock.send_to(&reply, src).await;
                    }
                    MSG_GET => {
                        let mut p = vec![0u8; 52];
                        // HSBK: a saturated red (hue 0) or white; brightness ~50%.
                        p[2..4].copy_from_slice(&saturation.to_le_bytes());
                        p[4..6].copy_from_slice(&32768u16.to_le_bytes()); // brightness
                        p[6..8].copy_from_slice(&3500u16.to_le_bytes()); // kelvin
                        p[10..12]
                            .copy_from_slice(&(if power_on { 65535u16 } else { 0 }).to_le_bytes());
                        let lb = label.as_bytes();
                        p[12..12 + lb.len().min(32)].copy_from_slice(&lb[..lb.len().min(32)]);
                        let reply = build_message(Some(MOCK_MAC), false, false, MSG_STATE, &p, 0);
                        let _ = sock.send_to(&reply, src).await;
                    }
                    other => {
                        rec.lock().await.push((other, payload.to_vec()));
                    }
                }
            }
        });
        MockBulb { addr, received }
    }

    fn provider(mock: &MockBulb) -> LifxLanProvider {
        LifxLanProvider::new_for_test(mock.addr, Duration::from_millis(400))
    }

    #[test]
    fn mac_round_trips_hex_and_bytes() {
        assert_eq!(mac_hex(&MOCK_MAC), MOCK_MAC_HEX);
        assert_eq!(mac_bytes(MOCK_MAC_HEX), Some(MOCK_MAC));
        assert_eq!(mac_bytes("d0:73:d5:12:34:56"), Some(MOCK_MAC));
        assert_eq!(mac_bytes("nope"), None);
    }

    #[test]
    fn message_header_is_well_formed() {
        let msg = build_message(Some(MOCK_MAC), true, true, MSG_GET_SERVICE, &[], 7);
        assert_eq!(msg.len(), HEADER_LEN);
        // size field
        assert_eq!(u16::from_le_bytes([msg[0], msg[1]]), HEADER_LEN as u16);
        // tagged + addressable + protocol packed into the frame u16
        let flags = u16::from_le_bytes([msg[2], msg[3]]);
        assert_eq!(flags & 0x0FFF, 1024, "protocol");
        assert_ne!(flags & (1 << 12), 0, "addressable");
        assert_ne!(flags & (1 << 13), 0, "tagged");
        // decode round-trips the target + type
        let (mac, ty, payload) = decode(&msg).unwrap();
        assert_eq!(mac, MOCK_MAC);
        assert_eq!(ty, MSG_GET_SERVICE);
        assert!(payload.is_empty());
    }

    #[tokio::test]
    async fn discover_finds_bulb_via_get_service_and_reads_its_label() {
        clear_addr_cache().await;
        let mock = spawn_mock_bulb("Desk", true, 65535).await;
        let lights = provider(&mock).discover().await.unwrap();
        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].provider_id, MOCK_MAC_HEX);
        assert_eq!(lights[0].name, "Desk");
        assert_eq!(lights[0].hw_id.as_deref(), Some("mac:d073d5123456"));
        assert_eq!(lights[0].state.transport.as_deref(), Some("lan"));
    }

    #[tokio::test]
    async fn get_state_parses_a_saturated_colour_as_reachable() {
        clear_addr_cache().await;
        let mock = spawn_mock_bulb("Desk", true, 65535).await;
        let state = provider(&mock).get_state(MOCK_MAC_HEX).await.unwrap();
        assert!(state.on);
        assert_eq!(state.reachable, Some(true));
        assert!(state.color.is_some(), "saturated → a colour");
        assert_eq!(state.transport.as_deref(), Some("lan"));
        assert_eq!(state.brightness, Some(50.0));
    }

    #[tokio::test]
    async fn get_state_parses_unsaturated_as_white_temperature() {
        clear_addr_cache().await;
        let mock = spawn_mock_bulb("Desk", true, 0).await;
        let state = provider(&mock).get_state(MOCK_MAC_HEX).await.unwrap();
        assert!(state.color.is_none());
        assert!(state.color_temp_mirek.is_some(), "unsaturated → white");
    }

    #[tokio::test]
    async fn get_state_unreachable_when_no_reply() {
        clear_addr_cache().await;
        // Point at a closed loopback port: nothing answers.
        let dead = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9);
        let provider = LifxLanProvider::new_for_test(dead, Duration::from_millis(150));
        let state = provider.get_state(MOCK_MAC_HEX).await.unwrap();
        assert!(!state.on);
        assert_eq!(state.reachable, Some(false));
    }

    #[tokio::test]
    async fn set_state_sends_setcolor_then_setpower() {
        clear_addr_cache().await;
        let mock = spawn_mock_bulb("Desk", true, 65535).await;
        let provider = provider(&mock);
        // Seed the addr cache so control resolves the MAC.
        provider.scan().await.unwrap();
        provider
            .set_state(
                MOCK_MAC_HEX,
                &LightState {
                    on: true,
                    color: Some(Color::from_rgb(0, 255, 0)),
                    brightness: Some(80.0),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let got = mock.received.lock().await;
        let types: Vec<u16> = got.iter().map(|(t, _)| *t).collect();
        assert!(
            types.contains(&MSG_SET_COLOR),
            "missing SetColor: {types:?}"
        );
        assert!(
            types.contains(&MSG_SET_POWER),
            "missing SetPower: {types:?}"
        );
        // SetPower level for an on-command is 65535.
        let (_, pwr) = got.iter().find(|(t, _)| *t == MSG_SET_POWER).unwrap();
        assert_eq!(u16::from_le_bytes([pwr[0], pwr[1]]), 65535);
        // SetColor's HSBK saturation (bytes 3..5 of payload: reserved + hue) — green
        // is fully saturated.
        let (_, col) = got.iter().find(|(t, _)| *t == MSG_SET_COLOR).unwrap();
        let sat = u16::from_le_bytes([col[3], col[4]]);
        assert!(sat > 60000, "green should be near-fully saturated: {sat}");
    }

    #[tokio::test]
    async fn set_state_unknown_device_errors() {
        clear_addr_cache().await;
        let mock = spawn_mock_bulb("Desk", true, 65535).await;
        let provider = provider(&mock);
        // A MAC the scan never returns is not on the LAN.
        let err = provider
            .set_state(
                "aabbccddeeff",
                &LightState {
                    on: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
    }
}
