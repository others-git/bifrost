//! TP-Link Kasa smart plugs — the legacy LAN protocol.
//!
//! Kasa shipped two protocol generations. **Legacy** (pre-~2021 firmware,
//! still what most already-deployed HS10x/KP1xx plugs run unless they've been
//! force-updated): raw TCP on port **9999**, JSON commands, zero
//! authentication. The "encryption" is a rudimentary XOR autokey stream
//! cipher with a hardcoded starting key (`171`) — obfuscation, not real
//! crypto, and every open-source Kasa client (python-kasa, the old HA
//! `pyHS100`, various node.js clients) implements the identical few lines.
//! Wire framing: TCP messages are a 4-byte big-endian length prefix followed
//! by the XORed payload; UDP discovery replies are the XORed payload with
//! **no** length prefix (verified against real hardware, not just docs).
//!
//! **KLAP** (2021+ firmware): a real handshake requiring a hash derived from
//! the TP-Link cloud account (`md5(md5(user), md5(pass))`) once a device has
//! ever been linked to the Kasa app — a materially different trust ask than
//! every other LAN provider in this codebase (none touch a third-party cloud
//! account). **Not implemented.** A KLAP-only device won't answer this
//! provider's plaintext query and won't be found by its discovery legs — the
//! honest failure mode (silence), not a crash. Flagging this explicitly per
//! the capability-parity rule: a KLAP plug would need real handshake/session
//! work, not a schema tweak.
//!
//! Commands used (see e.g. <https://github.com/whitslack/kasa> for the wider
//! catalogue):
//! - `{"system":{"get_sysinfo":{}}}` — name, model, mac, relay state.
//! - `{"system":{"set_relay_state":{"state":0|1}}}` — the only write.
//!
//! One provider row = one physical plug (`provider_id` is always `"main"`),
//! matching the WLED/Tasmota/Shelly convention. Multi-outlet power strips
//! (HS300/KP400, whose sysinfo carries a `children` array instead of a
//! top-level `relay_state`) aren't modeled — flagged, not silently dropped:
//! `discover` returns a single `PowerDevice` for the strip's overall relay
//! reporting only if the top-level field is present, and simply won't surface
//! individual outlets. Extend `Sysinfo`/`discover` with a `children` branch
//! when a real multi-outlet device needs supporting.

use crate::models::power::{PowerDevice, PowerKind, PowerState};
use crate::providers::discovery::{
    DeviceDiscovery, DiscoveredDevice, ScanOptions, UnionDiscovery, extra_subnet_bases,
    host_credentials, local_ipv4, subnet_bases, udp_probe,
};
use crate::providers::{
    CredentialField, Credentials, FieldKind, LanBinding, PowerProvider, PowerProviderFactory,
    mac_hw_id,
};
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use futures_util::stream::{self, StreamExt};
use serde::Deserialize;
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

const KASA_PORT: u16 = 9999;
const XOR_SEED: u8 = 171;
/// A garbled/malicious length prefix must not drive an unbounded allocation —
/// real sysinfo replies are well under 1KB.
const MAX_REPLY_LEN: usize = 64 * 1024;

// ── Wire codec ───────────────────────────────────────────────────────────────

/// Kasa's "encryption": an XOR autokey stream cipher — each plaintext byte is
/// XORed with a running key that becomes the just-produced ciphertext byte.
fn xor_encode(data: &[u8]) -> Vec<u8> {
    let mut key = XOR_SEED;
    data.iter()
        .map(|&b| {
            let c = b ^ key;
            key = c;
            c
        })
        .collect()
}

/// The inverse: the running key becomes the just-consumed CIPHERTEXT byte
/// (not the recovered plaintext byte) — same autokey stream, decrypt direction.
fn xor_decode(data: &[u8]) -> Vec<u8> {
    let mut key = XOR_SEED;
    data.iter()
        .map(|&c| {
            let b = c ^ key;
            key = c;
            b
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct SysinfoEnvelope {
    system: SysinfoSystem,
}

#[derive(Debug, Deserialize)]
struct SysinfoSystem {
    get_sysinfo: Sysinfo,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct Sysinfo {
    alias: Option<String>,
    model: Option<String>,
    mac: Option<String>,
    /// Some regions/hardware revisions report the MAC under this key instead
    /// of `mac` — a documented quirk across independent Kasa clients.
    mic_mac: Option<String>,
    relay_state: Option<i64>,
}

impl Sysinfo {
    fn hw_id(&self) -> Option<String> {
        self.mac
            .as_deref()
            .or(self.mic_mac.as_deref())
            .and_then(mac_hw_id)
    }

    fn on(&self) -> bool {
        self.relay_state.unwrap_or(0) != 0
    }
}

fn sysinfo_query() -> serde_json::Value {
    serde_json::json!({"system": {"get_sysinfo": {}}})
}

fn set_relay_query(on: bool) -> serde_json::Value {
    serde_json::json!({"system": {"set_relay_state": {"state": i64::from(on)}}})
}

/// Send one command over a fresh TCP connection and return the decoded JSON
/// reply. One-shot (connect, write, read, close) — Kasa's legacy protocol has
/// no session to keep warm, and a plug is polled infrequently enough that
/// per-call connection setup is immaterial.
async fn tcp_query(
    addr: SocketAddr,
    cmd: &serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value> {
    tokio::time::timeout(timeout, async {
        let payload = xor_encode(&serde_json::to_vec(cmd)?);
        let mut frame = Vec::with_capacity(4 + payload.len());
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(&payload);

        let mut stream = TcpStream::connect(addr)
            .await
            .context("connecting to Kasa device")?;
        stream
            .write_all(&frame)
            .await
            .context("writing Kasa command")?;

        let mut len_buf = [0u8; 4];
        stream
            .read_exact(&mut len_buf)
            .await
            .context("reading Kasa response length")?;
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > MAX_REPLY_LEN {
            bail!("Kasa response claims an implausible length: {len}");
        }
        let mut body = vec![0u8; len];
        stream
            .read_exact(&mut body)
            .await
            .context("reading Kasa response body")?;

        serde_json::from_slice(&xor_decode(&body)).context("parsing Kasa response JSON")
    })
    .await
    .map_err(|_| anyhow!("Kasa device at {addr} timed out"))?
}

fn parse_sysinfo_reply(reply: serde_json::Value) -> Result<Sysinfo> {
    let env: SysinfoEnvelope =
        serde_json::from_value(reply).context("Kasa reply did not carry a sysinfo envelope")?;
    Ok(env.system.get_sysinfo)
}

// ── Provider ─────────────────────────────────────────────────────────────────

pub struct KasaProvider {
    addr: SocketAddr,
    timeout: Duration,
}

impl KasaProvider {
    fn new_with_addr(addr: SocketAddr) -> Self {
        Self {
            addr,
            timeout: Duration::from_secs(5),
        }
    }

    pub fn new(device_ip: &str) -> Result<Self> {
        let ip: Ipv4Addr = device_ip
            .parse()
            .with_context(|| format!("invalid Kasa device_ip: {device_ip}"))?;
        Ok(Self::new_with_addr(SocketAddr::from((ip, KASA_PORT))))
    }

    pub fn from_credentials(creds_json: &str) -> Result<Self> {
        let creds: serde_json::Value = serde_json::from_str(creds_json)?;
        let ip = creds["device_ip"]
            .as_str()
            .ok_or_else(|| anyhow!("kasa credentials missing device_ip"))?;
        Self::new(ip)
    }

    /// One `get_sysinfo` round-trip — the plug's whole readable state (name,
    /// model, MAC, relay). Every read path goes through here.
    async fn sysinfo(&self) -> Result<Sysinfo> {
        parse_sysinfo_reply(tcp_query(self.addr, &sysinfo_query(), self.timeout).await?)
    }

    #[cfg(test)]
    fn new_for_test(addr: SocketAddr) -> Self {
        let mut p = Self::new_with_addr(addr);
        p.timeout = Duration::from_millis(400);
        p
    }
}

#[async_trait]
impl PowerProvider for KasaProvider {
    fn name(&self) -> &str {
        "kasa"
    }

    async fn discover(&self) -> Result<Vec<PowerDevice>> {
        let info = self.sysinfo().await?;
        Ok(vec![PowerDevice {
            id: Uuid::new_v4(),
            // One physical plug per provider entry; stable identifier is "main"
            // (matches the WLED/Tasmota/Shelly one-device-per-row convention).
            provider_id: "main".into(),
            name: info
                .alias
                .clone()
                .unwrap_or_else(|| "Kasa Plug".to_string()),
            kind: PowerKind::Outlet,
            state: PowerState {
                on: info.on(),
                reachable: Some(true),
            },
            hw_id: info.hw_id(),
        }])
    }

    async fn get_state(&self, _device_id: &str) -> Result<PowerState> {
        let info = self.sysinfo().await?;
        Ok(PowerState {
            on: info.on(),
            reachable: Some(true),
        })
    }

    async fn set_state(&self, _device_id: &str, on: bool) -> Result<()> {
        let reply = tcp_query(self.addr, &set_relay_query(on), self.timeout).await?;
        let err_code = reply
            .pointer("/system/set_relay_state/err_code")
            .and_then(|v| v.as_i64());
        match err_code {
            Some(0) => Ok(()),
            other => bail!("Kasa set_relay_state failed (err_code {other:?})"),
        }
    }
}

// ── Discovery ────────────────────────────────────────────────────────────────

/// Build a [`DiscoveredDevice`] from a resolved sysinfo reply.
fn discovered_device(host: &str, info: &Sysinfo) -> DiscoveredDevice {
    DiscoveredDevice {
        host: host.to_string(),
        label: info
            .alias
            .clone()
            .or_else(|| info.model.clone())
            .or_else(|| Some("TP-Link Kasa".to_string())),
        credentials: host_credentials("device_ip", host),
    }
}

/// UDP broadcast leg — the native mechanism (the same thing the Kasa app
/// itself does): one broadcast query to `255.255.255.255:9999`, every legacy
/// device on the subnet replies directly to our probe socket. Fast, but
/// broadcast doesn't cross every virtualized network stack (confirmed: it's
/// blocked from this project's own WSL2 dev environment even though unicast
/// isn't) — [`KasaTcpSweepDiscovery`] is the fallback for exactly that case.
pub struct KasaBroadcastDiscovery {
    /// Injected in tests (a loopback responder, since a real broadcast can't
    /// be asserted on in CI); `None` = the real broadcast address.
    target: Option<SocketAddr>,
}

impl KasaBroadcastDiscovery {
    pub fn new() -> Self {
        Self { target: None }
    }

    #[cfg(test)]
    pub(crate) fn with_target(mut self, target: SocketAddr) -> Self {
        self.target = Some(target);
        self
    }
}

impl Default for KasaBroadcastDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DeviceDiscovery for KasaBroadcastDiscovery {
    async fn scan(&self, opts: &ScanOptions) -> Result<Vec<DiscoveredDevice>> {
        let payload = xor_encode(&serde_json::to_vec(&sysinfo_query())?);
        let target = self
            .target
            .unwrap_or(SocketAddr::from((Ipv4Addr::BROADCAST, KASA_PORT)));
        let replies = udp_probe(target, &payload, opts.timeout.min(Duration::from_secs(3))).await?;

        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (from, bytes) in replies {
            let host = from.ip().to_string();
            if !seen.insert(host.clone()) {
                continue; // a retransmitted probe can draw a second reply
            }
            let Ok(env) = serde_json::from_slice::<SysinfoEnvelope>(&xor_decode(&bytes)) else {
                continue; // not a Kasa reply (or a different device on 9999)
            };
            out.push(discovered_device(&host, &env.system.get_sysinfo));
        }
        Ok(out)
    }
}

/// TCP unicast sweep — the fallback when broadcast can't reach the device.
/// Connects to every host in the local /24 (+ Expanded-LAN subnets) on port
/// 9999 and sends the SAME `get_sysinfo` query as the broadcast leg; a match
/// is authoritative (a real decoded, parsed sysinfo reply), not just an open
/// port, so it can't be confused by some other service squatting on 9999.
pub struct KasaTcpSweepDiscovery {
    /// Injected in tests; `None` = derive the local /24 at runtime.
    hosts: Option<Vec<String>>,
    /// Real devices always speak 9999; overridable in tests, since a mock
    /// listener binds an ephemeral port (real hardware's fixed port can't be
    /// bound safely in a test — it may be occupied by an actual device, or by
    /// another test running concurrently).
    port: u16,
}

impl KasaTcpSweepDiscovery {
    pub fn new() -> Self {
        Self {
            hosts: None,
            port: KASA_PORT,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_hosts(mut self, hosts: Vec<String>) -> Self {
        self.hosts = Some(hosts);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }
}

impl Default for KasaTcpSweepDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DeviceDiscovery for KasaTcpSweepDiscovery {
    async fn scan(&self, opts: &ScanOptions) -> Result<Vec<DiscoveredDevice>> {
        let mut hosts: Vec<String> = match &self.hosts {
            Some(h) => h.clone(),
            None => local_ipv4()
                .map(subnet_bases)
                .unwrap_or_default()
                .into_iter()
                .map(|b| b.trim_start_matches("http://").to_string())
                .collect(),
        };
        for subnet in &opts.extra_subnets {
            for base in extra_subnet_bases(*subnet) {
                let h = base.trim_start_matches("http://").to_string();
                if !hosts.contains(&h) {
                    hosts.push(h);
                }
            }
        }
        if hosts.is_empty() {
            return Ok(Vec::new());
        }

        // Cap per-host wait so the sweep fits the budget — unused IPs hang to
        // this limit; live ones answer in single-digit milliseconds.
        let per_host = opts.timeout.min(Duration::from_millis(500));
        let probed = hosts.len();
        let port = self.port;
        let results: Vec<(String, Sysinfo)> = stream::iter(hosts)
            .map(|host| {
                let query = sysinfo_query();
                async move {
                    let addr: SocketAddr = format!("{host}:{port}").parse().ok()?;
                    let reply = tcp_query(addr, &query, per_host).await.ok()?;
                    let info = parse_sysinfo_reply(reply).ok()?;
                    Some((host, info))
                }
            })
            .buffer_unordered(64)
            .filter_map(|x| async move { x })
            .collect()
            .await;

        tracing::debug!(
            target: "bifrost::discover",
            probed,
            matched = results.len(),
            "kasa tcp sweep",
        );
        Ok(results
            .iter()
            .map(|(host, info)| discovered_device(host, info))
            .collect())
    }
}

// ── Factory ──────────────────────────────────────────────────────────────────

pub struct KasaPowerFactory;

/// A plug is reached at a stored IP, so a DHCP change strands it. The legacy
/// protocol is unauthenticated, so identity is the plug's own MAC: `get_sysinfo`
/// on the candidate must report the hardware id already recorded for this
/// provider's device row.
struct KasaLanBinding;

#[async_trait]
impl LanBinding for KasaLanBinding {
    fn host_field(&self) -> &'static str {
        "device_ip"
    }

    fn probe_port(&self, _creds: &Credentials) -> u16 {
        KASA_PORT
    }

    async fn is_same_device(&self, host: &str, _creds: &Credentials, known_hw: &[String]) -> bool {
        if known_hw.is_empty() {
            return false; // nothing to compare against
        }
        // Discovery yields a bare IP; an explicit `ip:port` is honoured too.
        let Some(addr) = host.parse::<SocketAddr>().ok().or_else(|| {
            host.parse::<Ipv4Addr>()
                .ok()
                .map(|ip| SocketAddr::from((ip, KASA_PORT)))
        }) else {
            return false;
        };
        match KasaProvider::new_with_addr(addr).sysinfo().await {
            Ok(info) => info.hw_id().is_some_and(|hw| known_hw.contains(&hw)),
            Err(_) => false,
        }
    }
}

impl PowerProviderFactory for KasaPowerFactory {
    fn provider_type(&self) -> &'static str {
        "kasa"
    }

    fn display_name(&self) -> &'static str {
        "TP-Link Kasa"
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn PowerProvider>> {
        Ok(Box::new(KasaProvider::from_credentials(credentials_json)?))
    }

    fn credentials_schema(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "device_ip",
            label: "Device IP Address",
            kind: FieldKind::IpAddress,
            required: true,
            hint: Some(
                "IP address of the Kasa plug on your local network. Legacy (unauthenticated) \
                 devices only — a plug that's been force-updated to TP-Link's newer KLAP \
                 protocol needs your TP-Link cloud credentials and isn't supported yet.",
            ),
        }]
    }

    fn discoverer(&self) -> Option<Box<dyn DeviceDiscovery>> {
        Some(Box::new(UnionDiscovery::new(vec![
            Box::new(KasaBroadcastDiscovery::new()),
            Box::new(KasaTcpSweepDiscovery::new()),
        ])))
    }

    fn lan_binding(&self) -> Option<Box<dyn LanBinding>> {
        Some(Box::new(KasaLanBinding))
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    // ── codec ──

    #[test]
    fn xor_roundtrips() {
        let plaintext = br#"{"system":{"get_sysinfo":{}}}"#;
        let encoded = xor_encode(plaintext);
        assert_ne!(encoded.as_slice(), plaintext, "must not be a no-op");
        assert_eq!(xor_decode(&encoded), plaintext);
    }

    #[test]
    fn xor_matches_the_known_first_byte() {
        // The very first ciphertext byte is always plaintext[0] ^ 171 — the
        // one fact independent of autokey chaining, so a wrong seed constant
        // is caught even if roundtrip alone would hide it.
        let encoded = xor_encode(b"{");
        assert_eq!(encoded[0], b'{' ^ 171);
    }

    #[test]
    fn xor_encode_is_not_a_simple_repeating_xor() {
        // Autokey (chained) vs a naive single-byte XOR: two identical
        // plaintext bytes must NOT encode identically once the key has moved.
        let encoded = xor_encode(b"aaaa");
        assert_ne!(encoded[0], encoded[1], "key must advance between bytes");
    }

    // ── LAN binding (host relocation) ──

    #[tokio::test]
    async fn lan_binding_proves_identity_by_the_plugs_own_mac() {
        use crate::providers::LanBinding as _;
        let plug = spawn_mock_plug("Raven Lights", true).await;
        let b = KasaLanBinding;
        assert_eq!(b.host_field(), "device_ip");
        assert_eq!(b.probe_port(&serde_json::Map::new()), KASA_PORT);

        let creds = serde_json::Map::new();
        let host = plug.addr.to_string();
        // The mock reports AA:BB:CC:DD:EE:FF.
        assert!(
            b.is_same_device(&host, &creds, &["mac:aabbccddeeff".to_string()])
                .await
        );
        // A different plug's id must never match.
        assert!(
            !b.is_same_device(&host, &creds, &["mac:112233445566".to_string()])
                .await
        );
        // Nothing recorded to compare against → refuse rather than guess.
        assert!(!b.is_same_device(&host, &creds, &[]).await);
        // An unreachable candidate is refused, not adopted on optimism.
        assert!(
            !b.is_same_device("127.0.0.1:1", &creds, &["mac:aabbccddeeff".to_string()])
                .await
        );
    }

    // ── mock TCP device ──

    struct MockPlug {
        addr: SocketAddr,
        relay_state: Arc<Mutex<i64>>,
    }

    /// A loopback TCP responder speaking the real wire protocol (length
    /// prefix + XOR framing) — mirrors LIFX's `spawn_mock_bulb` UDP pattern
    /// for a connection-oriented protocol. Answers `get_sysinfo` with `alias`
    /// and the current relay state, and applies `set_relay_state` for real so
    /// a follow-up `get_sysinfo` reflects it (proving the write took).
    async fn spawn_mock_plug(alias: &str, initial_on: bool) -> MockPlug {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let relay_state = Arc::new(Mutex::new(i64::from(initial_on)));
        let state = relay_state.clone();
        let alias = alias.to_string();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let state = state.clone();
                let alias = alias.clone();
                tokio::spawn(async move {
                    let mut len_buf = [0u8; 4];
                    if sock.read_exact(&mut len_buf).await.is_err() {
                        return;
                    }
                    let len = u32::from_be_bytes(len_buf) as usize;
                    let mut body = vec![0u8; len];
                    if sock.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    let decoded = xor_decode(&body);
                    let cmd: serde_json::Value = serde_json::from_slice(&decoded).unwrap();

                    let reply = if cmd.pointer("/system/get_sysinfo").is_some() {
                        let on = *state.lock().await;
                        serde_json::json!({"system": {"get_sysinfo": {
                            "alias": alias,
                            "model": "HS105(US)",
                            "mac": "AA:BB:CC:DD:EE:FF",
                            "relay_state": on,
                            "sw_ver": "1.5.6 Build 191114 Rel.104204",
                        }}})
                    } else if let Some(req) = cmd.pointer("/system/set_relay_state/state") {
                        *state.lock().await = req.as_i64().unwrap_or(0);
                        serde_json::json!({"system": {"set_relay_state": {"err_code": 0}}})
                    } else {
                        serde_json::json!({"system": {"err_code": -1}})
                    };

                    let payload = xor_encode(&serde_json::to_vec(&reply).unwrap());
                    let mut frame = (payload.len() as u32).to_be_bytes().to_vec();
                    frame.extend_from_slice(&payload);
                    let _ = sock.write_all(&frame).await;
                });
            }
        });
        MockPlug { addr, relay_state }
    }

    #[tokio::test]
    async fn discover_reports_name_kind_and_state() {
        let plug = spawn_mock_plug("raven lights", true).await;
        let devices = KasaProvider::new_for_test(plug.addr)
            .discover()
            .await
            .unwrap();

        assert_eq!(devices.len(), 1);
        let d = &devices[0];
        assert_eq!(d.provider_id, "main");
        assert_eq!(d.name, "raven lights");
        assert_eq!(d.kind, PowerKind::Outlet);
        assert!(d.state.on);
        assert_eq!(d.state.reachable, Some(true));
        assert_eq!(d.hw_id.as_deref(), Some("mac:aabbccddeeff"));
    }

    #[tokio::test]
    async fn get_state_reflects_off() {
        let plug = spawn_mock_plug("Couch String", false).await;
        let state = KasaProvider::new_for_test(plug.addr)
            .get_state("main")
            .await
            .unwrap();
        assert!(!state.on);
    }

    #[tokio::test]
    async fn set_state_actually_flips_the_relay() {
        let plug = spawn_mock_plug("Bedroom Shelf", false).await;
        let provider = KasaProvider::new_for_test(plug.addr);

        provider.set_state("main", true).await.unwrap();
        assert_eq!(*plug.relay_state.lock().await, 1);

        // The write is reflected on the NEXT read — not faked client-side.
        let state = provider.get_state("main").await.unwrap();
        assert!(state.on);
    }

    #[tokio::test]
    async fn get_state_errors_when_nothing_is_listening() {
        let dead: SocketAddr = "127.0.0.1:1".parse().unwrap(); // reserved, always refused
        let err = KasaProvider::new_for_test(dead)
            .get_state("main")
            .await
            .unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn factory_build_uses_device_ip_from_credentials() {
        let factory = KasaPowerFactory;
        assert!(factory.build(r#"{"device_ip":"192.168.1.46"}"#).is_ok());
    }

    #[tokio::test]
    async fn factory_build_fails_on_missing_device_ip() {
        let factory = KasaPowerFactory;
        let err = factory
            .build("{}")
            .err()
            .expect("expected error for missing device_ip");
        assert!(err.to_string().contains("device_ip"));
    }

    // ── discovery ──

    #[tokio::test]
    async fn broadcast_discovery_decodes_a_reply() {
        // A loopback UDP responder standing in for the broadcast target —
        // real broadcast reachability is an environment fact (confirmed
        // blocked from this project's WSL2 dev shell; a unit test can't
        // assert on it either way), so this exercises the actual thing the
        // leg owns: XOR-encoding the query, decoding the reply, and parsing
        // it into a DiscoveredDevice. UDP framing has NO length prefix
        // (unlike TCP) — verified against real hardware, and this test would
        // fail if that framing regressed.
        let responder = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = responder.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            let Ok((n, from)) = responder.recv_from(&mut buf).await else {
                return;
            };
            let decoded = xor_decode(&buf[..n]);
            let cmd: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
            assert!(cmd.pointer("/system/get_sysinfo").is_some());
            let reply = serde_json::json!({"system": {"get_sysinfo": {
                "alias": "Discovered Plug",
                "model": "HS103(US)",
                "mac": "11:22:33:44:55:66",
                "relay_state": 1,
            }}});
            let encoded = xor_encode(&serde_json::to_vec(&reply).unwrap());
            let _ = responder.send_to(&encoded, from).await; // NO length prefix on UDP
        });

        let devices = KasaBroadcastDiscovery::new()
            .with_target(addr)
            .scan(&ScanOptions::new(Duration::from_millis(500)))
            .await
            .unwrap();

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].label.as_deref(), Some("Discovered Plug"));
        assert_eq!(devices[0].host, addr.ip().to_string());
        assert_eq!(devices[0].credentials["device_ip"], addr.ip().to_string());
    }

    #[tokio::test]
    async fn tcp_sweep_skips_hosts_with_nothing_listening() {
        let plug = spawn_mock_plug("Only Real One", false).await;
        // The whole 127.0.0.0/8 is loopback on Linux — .2 is routable locally
        // but nothing is bound there on the mock's port, a genuinely dead host
        // distinct from the real mock's own loopback address.
        let dead_host = "127.0.0.2".to_string();
        let devices = KasaTcpSweepDiscovery::new()
            .with_hosts(vec![dead_host, plug.addr.ip().to_string()])
            .with_port(plug.addr.port())
            .scan(&ScanOptions::new(Duration::from_millis(300)))
            .await
            .unwrap();
        // Only the real plug answered; the dead host is silently dropped, not errored.
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].label.as_deref(), Some("Only Real One"));
    }
}
