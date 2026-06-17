//! Govee LAN API — local UDP control, no cloud / API key.
//!
//! Govee devices with "LAN Control" enabled (Govee Home app → device settings)
//! join multicast group `239.255.255.250` and:
//!   - listen for a `scan` request on UDP **4001** (discovery),
//!   - reply to the client on UDP **4002**,
//!   - accept control commands (`turn`/`brightness`/`colorwc`/`devStatus`) on
//!     UDP **4003**, replying again on 4002.
//!
//! We bind one socket to `<bind_addr>:4002`, multicast a scan to find devices,
//! and address each by its LAN IP thereafter. Because this is local UDP with no
//! daily quota, we poll more often than the cloud provider.
//!
//! Deployment note: in Docker this needs host networking (`--network host`) for
//! multicast to reach the LAN.
//!
//! Protocol message shapes are injectable (target/ports/timeout) so tests drive
//! the provider against a loopback mock device instead of real hardware.

use crate::models::{Color, Light, LightCapabilities, LightState, Provider};
use crate::providers::{LightProvider, ProviderGroup};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, OnceCell};
use uuid::Uuid;

const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const SCAN_PORT: u16 = 4001;
const RECV_PORT: u16 = 4002;
const CONTROL_PORT: u16 = 4003;
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1500);

pub struct GoveeLanProvider {
    /// Local interface address the UDP socket binds to (0.0.0.0 = all).
    bind_addr: IpAddr,
    /// Local port we receive scan/devStatus replies on (4002 in production).
    listen_port: u16,
    /// Where scan requests are sent (the multicast group:4001 in production).
    discovery_target: SocketAddr,
    /// Per-device control port (4003 in production).
    control_port: u16,
    /// How long discovery collects replies / a state query waits.
    timeout: Duration,
    socket: OnceCell<UdpSocket>,
    /// Serialises request/response exchanges so concurrent polls don't steal
    /// each other's replies off the shared socket.
    exchange: Mutex<()>,
}

impl GoveeLanProvider {
    /// Production provider bound to `bind_addr`, using the standard Govee ports.
    pub fn new(bind_addr: IpAddr) -> Self {
        Self {
            bind_addr,
            listen_port: RECV_PORT,
            discovery_target: SocketAddr::new(IpAddr::V4(MULTICAST_ADDR), SCAN_PORT),
            control_port: CONTROL_PORT,
            timeout: DEFAULT_TIMEOUT,
            socket: OnceCell::new(),
            exchange: Mutex::new(()),
        }
    }

    /// Test constructor: point discovery + control at a loopback mock device and
    /// use ephemeral ports so the socket binds cleanly under test.
    #[cfg(test)]
    fn new_for_test(mock_addr: SocketAddr, timeout: Duration) -> Self {
        Self {
            bind_addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
            listen_port: 0, // OS-assigned
            discovery_target: mock_addr,
            control_port: mock_addr.port(),
            timeout,
            socket: OnceCell::new(),
            exchange: Mutex::new(()),
        }
    }

    async fn socket(&self) -> Result<&UdpSocket> {
        self.socket
            .get_or_try_init(|| async {
                UdpSocket::bind((self.bind_addr, self.listen_port))
                    .await
                    .with_context(|| {
                        format!(
                            "binding Govee LAN socket to {}:{} (port 4002 must be free)",
                            self.bind_addr, self.listen_port
                        )
                    })
            })
            .await
    }

    /// Send the control commands a `LightState` implies to one device.
    async fn send_commands(&self, ip: &str, state: &LightState) -> Result<()> {
        let target = SocketAddr::new(
            ip.parse::<IpAddr>()
                .with_context(|| format!("invalid Govee LAN device address '{ip}'"))?,
            self.control_port,
        );

        // turn → brightness → color/ct, mirroring the cloud provider's order.
        let mut packets: Vec<Vec<u8>> =
            vec![command("turn", json!({ "value": u8::from(state.on) }))];

        if let Some(b) = state.brightness {
            packets.push(command(
                "brightness",
                json!({ "value": (b.round() as i64).clamp(1, 100) }),
            ));
        }
        if let Some(color) = &state.color {
            let (r, g, b) = color.to_rgb();
            packets.push(command(
                "colorwc",
                json!({ "color": { "r": r, "g": g, "b": b }, "colorTemInKelvin": 0 }),
            ));
        }
        if let Some(mirek) = state.color_temp_mirek {
            let kelvin = 1_000_000u32 / u32::from(mirek.max(1));
            packets.push(command(
                "colorwc",
                json!({ "color": { "r": 0, "g": 0, "b": 0 }, "colorTemInKelvin": kelvin }),
            ));
        }

        let sock = self.socket().await?;
        let _guard = self.exchange.lock().await;
        for pkt in packets {
            sock.send_to(&pkt, target)
                .await
                .context("sending Govee LAN command")?;
        }
        Ok(())
    }
}

/// A `{"msg":{"cmd":..,"data":..}}` request, serialised to bytes.
fn command(cmd: &str, data: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&json!({ "msg": { "cmd": cmd, "data": data } })).unwrap_or_default()
}

// ── Wire types ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Envelope {
    msg: Msg,
}

#[derive(Deserialize)]
struct Msg {
    cmd: String,
    #[serde(default)]
    data: serde_json::Value,
}

#[derive(Deserialize)]
struct ScanData {
    ip: String,
    #[serde(default)]
    sku: String,
}

#[derive(Deserialize)]
struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

#[derive(Deserialize)]
struct DevStatusData {
    #[serde(rename = "onOff")]
    on_off: u8,
    brightness: Option<f32>,
    color: Option<Rgb>,
    #[serde(rename = "colorTemInKelvin")]
    color_temp_kelvin: Option<u32>,
}

fn devstatus_to_state(d: DevStatusData) -> LightState {
    let mut state = LightState {
        on: d.on_off == 1,
        brightness: d.brightness,
        reachable: Some(true),
        ..Default::default()
    };
    // A non-zero Kelvin means the device is in colour-temperature mode (its rgb
    // reads as 0,0,0); otherwise the rgb is the active colour.
    match d.color_temp_kelvin {
        Some(k) if k > 0 => state.color_temp_mirek = Some((1_000_000 / k) as u16),
        _ => {
            if let Some(c) = d.color {
                state.color = Some(Color::from_rgb(c.r, c.g, c.b));
            }
        }
    }
    state
}

fn scan_to_light(ip: &str, sku: &str) -> Light {
    let name = if sku.is_empty() {
        format!("Govee @ {ip}")
    } else {
        format!("Govee {sku} @ {ip}")
    };
    Light {
        id: Uuid::new_v4(),
        provider_id: ip.to_string(),
        provider: Provider::Govee,
        name,
        state: LightState::default(),
        // Govee LAN devices are RGBWW strips/bulbs: dimmable, colour, and CT.
        capabilities: LightCapabilities {
            dimmable: true,
            color_rgb: true,
            color_temperature: true,
            hue_gamut: None,
            effects: Vec::new(),
        },
        last_seen: chrono::Utc::now(),
        hw_id: None, // LAN scan exposes only IP; no MAC yet.
    }
}

// ── Provider impl ───────────────────────────────────────────────────────────

#[async_trait]
impl LightProvider for GoveeLanProvider {
    fn name(&self) -> &str {
        "govee-lan"
    }

    async fn discover(&self) -> Result<Vec<Light>> {
        let sock = self.socket().await?;
        let _guard = self.exchange.lock().await;

        sock.send_to(
            &command("scan", json!({ "account_topic": "reserve" })),
            self.discovery_target,
        )
        .await
        .context("sending Govee LAN scan")?;

        // Collect replies until the timeout window closes, deduping by IP.
        let deadline = Instant::now() + self.timeout;
        let mut found: HashMap<String, Light> = HashMap::new();
        let mut buf = [0u8; 2048];
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, sock.recv_from(&mut buf)).await {
                Ok(Ok((n, _src))) => {
                    if let Ok(env) = serde_json::from_slice::<Envelope>(&buf[..n])
                        && env.msg.cmd == "scan"
                        && let Ok(d) = serde_json::from_value::<ScanData>(env.msg.data)
                    {
                        found
                            .entry(d.ip.clone())
                            .or_insert_with(|| scan_to_light(&d.ip, &d.sku));
                    }
                }
                Ok(Err(e)) => return Err(e).context("receiving Govee LAN scan reply"),
                Err(_) => break, // window elapsed
            }
        }
        Ok(found.into_values().collect())
    }

    async fn set_state(&self, device_id: &str, state: &LightState) -> Result<()> {
        self.send_commands(device_id, state).await
    }

    async fn get_state(&self, device_id: &str) -> Result<LightState> {
        let target = SocketAddr::new(
            device_id
                .parse::<IpAddr>()
                .with_context(|| format!("invalid Govee LAN device address '{device_id}'"))?,
            self.control_port,
        );
        let sock = self.socket().await?;
        let _guard = self.exchange.lock().await;

        sock.send_to(&command("devStatus", json!({})), target)
            .await
            .context("sending Govee LAN devStatus")?;

        let deadline = Instant::now() + self.timeout;
        let mut buf = [0u8; 2048];
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, sock.recv_from(&mut buf)).await {
                Ok(Ok((n, src))) => {
                    // Only accept a devStatus from the device we asked.
                    if src.ip().to_string() != device_id {
                        continue;
                    }
                    if let Ok(env) = serde_json::from_slice::<Envelope>(&buf[..n])
                        && env.msg.cmd == "devStatus"
                        && let Ok(d) = serde_json::from_value::<DevStatusData>(env.msg.data)
                    {
                        return Ok(devstatus_to_state(d));
                    }
                }
                Ok(Err(e)) => return Err(e).context("receiving Govee LAN devStatus reply"),
                Err(_) => break,
            }
        }
        // No reply in the window → the device is unreachable on the LAN.
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

// ── Factory ─────────────────────────────────────────────────────────────────

use crate::providers::{ConnectionMode, CredentialField, FieldKind, ProviderFactory};

pub struct GoveeLanProviderFactory;

impl ProviderFactory for GoveeLanProviderFactory {
    fn provider_type(&self) -> &'static str {
        "govee-lan"
    }

    fn display_name(&self) -> &'static str {
        "Govee (LAN)"
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn LightProvider>> {
        let creds: serde_json::Value = serde_json::from_str(credentials_json)?;
        let bind = creds["bind_addr"].as_str().unwrap_or("0.0.0.0");
        let bind_addr: IpAddr = bind
            .parse()
            .with_context(|| format!("invalid Govee LAN bind address '{bind}'"))?;
        Ok(Box::new(GoveeLanProvider::new(bind_addr)))
    }

    fn credentials_schema(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "bind_addr",
            label: "Network interface",
            kind: FieldKind::IpAddress,
            required: true,
            hint: Some(
                "Local IP of the interface on your Govee LAN — use 0.0.0.0 unless multi-homed. Enable LAN Control on each device in the Govee Home app.",
            ),
        }]
    }

    fn connection_mode(&self) -> ConnectionMode {
        // Local UDP has no daily quota, so refresh more eagerly than the cloud.
        ConnectionMode::Poll { interval_secs: 30 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex as AsyncMutex;

    /// A loopback stand-in for a Govee device: answers `scan` and `devStatus`,
    /// and records control commands it receives for assertions.
    struct MockDevice {
        addr: SocketAddr,
        received: Arc<AsyncMutex<Vec<serde_json::Value>>>,
    }

    async fn spawn_mock_device() -> MockDevice {
        let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = sock.local_addr().unwrap();
        let received = Arc::new(AsyncMutex::new(Vec::new()));
        let recv_clone = received.clone();

        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                let Ok((n, src)) = sock.recv_from(&mut buf).await else {
                    break;
                };
                let Ok(env) = serde_json::from_slice::<Envelope>(&buf[..n]) else {
                    continue;
                };
                match env.msg.cmd.as_str() {
                    "scan" => {
                        let reply = json!({"msg":{"cmd":"scan","data":{
                            "ip": "127.0.0.1", "sku": "H6159", "device": "AA:BB:CC:DD:EE:FF"
                        }}});
                        let _ = sock
                            .send_to(serde_json::to_vec(&reply).unwrap().as_slice(), src)
                            .await;
                    }
                    "devStatus" => {
                        let reply = json!({"msg":{"cmd":"devStatus","data":{
                            "onOff": 1, "brightness": 80,
                            "color": {"r": 255, "g": 0, "b": 0}, "colorTemInKelvin": 0
                        }}});
                        let _ = sock
                            .send_to(serde_json::to_vec(&reply).unwrap().as_slice(), src)
                            .await;
                    }
                    other => {
                        recv_clone
                            .lock()
                            .await
                            .push(json!({ "cmd": other, "data": env.msg.data }));
                    }
                }
            }
        });

        MockDevice { addr, received }
    }

    fn test_provider(mock: &MockDevice) -> GoveeLanProvider {
        GoveeLanProvider::new_for_test(mock.addr, Duration::from_millis(400))
    }

    #[tokio::test]
    async fn discover_returns_devices_from_scan_reply() {
        let mock = spawn_mock_device().await;
        let provider = test_provider(&mock);

        let lights = provider.discover().await.unwrap();
        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].provider_id, "127.0.0.1");
        assert!(lights[0].name.contains("H6159"));
        assert!(lights[0].capabilities.color_rgb);
        assert!(lights[0].capabilities.dimmable);
        assert!(lights[0].capabilities.color_temperature);
    }

    #[tokio::test]
    async fn get_state_parses_devstatus_reply() {
        let mock = spawn_mock_device().await;
        let provider = test_provider(&mock);

        let state = provider.get_state("127.0.0.1").await.unwrap();
        assert!(state.on);
        assert_eq!(state.brightness, Some(80.0));
        assert_eq!(state.reachable, Some(true));
        assert!(state.color.is_some(), "red rgb should map to a colour");
        assert!(state.color_temp_mirek.is_none());
    }

    #[tokio::test]
    async fn get_state_times_out_to_unreachable() {
        // Point at a closed loopback port: no device answers.
        let dead = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4); // unused
        let provider = GoveeLanProvider::new_for_test(dead, Duration::from_millis(150));

        let state = provider.get_state("127.0.0.1").await.unwrap();
        assert!(!state.on);
        assert_eq!(state.reachable, Some(false));
    }

    #[tokio::test]
    async fn set_state_sends_turn_brightness_and_color() {
        let mock = spawn_mock_device().await;
        let provider = test_provider(&mock);

        provider
            .set_state(
                "127.0.0.1",
                &LightState {
                    on: true,
                    brightness: Some(60.0),
                    color: Some(Color::from_rgb(0, 255, 0)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // Give the mock a moment to record the datagrams.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let got = mock.received.lock().await;
        let cmds: Vec<&str> = got.iter().map(|c| c["cmd"].as_str().unwrap()).collect();
        assert!(cmds.contains(&"turn"), "missing turn: {cmds:?}");
        assert!(cmds.contains(&"brightness"), "missing brightness: {cmds:?}");
        assert!(cmds.contains(&"colorwc"), "missing colorwc: {cmds:?}");

        let turn = got.iter().find(|c| c["cmd"] == "turn").unwrap();
        assert_eq!(turn["data"]["value"], 1);
        let bright = got.iter().find(|c| c["cmd"] == "brightness").unwrap();
        assert_eq!(bright["data"]["value"], 60);
    }

    #[tokio::test]
    async fn color_temp_state_sends_kelvin_colorwc() {
        let mock = spawn_mock_device().await;
        let provider = test_provider(&mock);

        provider
            .set_state(
                "127.0.0.1",
                &LightState {
                    on: true,
                    color_temp_mirek: Some(250), // 4000K
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;
        let got = mock.received.lock().await;
        let cw = got.iter().find(|c| c["cmd"] == "colorwc").unwrap();
        assert_eq!(cw["data"]["colorTemInKelvin"], 4000);
    }

    #[tokio::test]
    async fn factory_builds_from_bind_addr_credentials() {
        let factory = GoveeLanProviderFactory;
        assert!(factory.build(r#"{"bind_addr":"0.0.0.0"}"#).is_ok());
        // Missing bind_addr defaults to 0.0.0.0 rather than failing.
        assert!(factory.build("{}").is_ok());
        // A malformed address is rejected.
        assert!(factory.build(r#"{"bind_addr":"not-an-ip"}"#).is_err());
    }

    #[test]
    fn factory_uses_local_poll_mode() {
        let factory = GoveeLanProviderFactory;
        match factory.connection_mode() {
            ConnectionMode::Poll { interval_secs } => assert!(interval_secs <= 60),
            other => panic!("expected Poll, got {other:?}"),
        }
    }
}
