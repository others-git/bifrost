//! Network auto-detection for providers.
//!
//! A provider opts into "scan the LAN for my devices" by having its factory
//! return a [`DeviceDiscovery`] object. The object is deliberately tiny: it
//! declares a probe (what to send, where) and how to recognise a match, while
//! the shared [`udp_probe`] engine owns the socket mechanics. Providers that
//! don't implement it simply inherit the `None` default and show no scan
//! button in the UI.
//!
//! Most discoverable LAN gear answers a single broadcast/multicast datagram
//! (SSDP for UPnP, eISCP for Onkyo, the Govee LAN scan), so one UDP primitive
//! covers them; HTTP-only devices that need mDNS can add their own object later
//! without touching this contract.

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use serde::Serialize;
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;

/// A device found by a network scan. `credentials` is pre-shaped to drop
/// straight into the add-provider form (e.g. `{"host": "192.168.1.40"}`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiscoveredDevice {
    /// IP (or host) where the device answered.
    pub host: String,
    /// Human label (model/name) when the reply carried one.
    pub label: Option<String>,
    /// Credential fields to pre-fill, matching the provider's schema.
    pub credentials: serde_json::Value,
}

/// Options passed to a scan. `timeout` bounds the whole probe; `extra_subnets`
/// are additional private /24 bases for the HTTP sweep (Expanded-LAN). Broadcast
/// discoverers (SSDP, eISCP) use only `timeout` — they can't cross a subnet.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub timeout: Duration,
    pub extra_subnets: Vec<Ipv4Addr>,
}

impl ScanOptions {
    /// A scan of just the local subnet (no Expanded-LAN).
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            extra_subnets: Vec::new(),
        }
    }
}

/// A provider's network auto-detect. One method: probe the LAN, return what
/// answered. Implementations stay thin — the I/O lives in [`udp_probe`].
#[async_trait]
pub trait DeviceDiscovery: Send + Sync {
    async fn scan(&self, opts: &ScanOptions) -> Result<Vec<DiscoveredDevice>>;
}

/// Send `payload` once to `target` (a broadcast or multicast address), then
/// collect every reply datagram until `timeout` elapses. Binds an ephemeral
/// local port on all interfaces; replies arrive there as unicast, so no
/// multicast-group membership is needed for the M-SEARCH / probe-and-listen
/// pattern these providers use.
pub async fn udp_probe(
    target: SocketAddr,
    payload: &[u8],
    timeout: Duration,
) -> Result<Vec<(SocketAddr, Vec<u8>)>> {
    let bind = if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind)
        .await
        .context("binding discovery socket")?;
    // Harmless for unicast/multicast sends; required for 255.255.255.255.
    let _ = socket.set_broadcast(true);
    socket
        .send_to(payload, target)
        .await
        .with_context(|| format!("sending discovery probe to {target}"))?;

    let mut replies = Vec::new();
    let mut buf = [0u8; 2048];
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, socket.recv_from(&mut buf)).await {
            Ok(Ok((n, from))) => replies.push((from, buf[..n].to_vec())),
            // Socket error or window elapsed — stop collecting.
            Ok(Err(_)) | Err(_) => break,
        }
    }
    Ok(replies)
}

/// Credentials object pre-shaped for one host field, e.g. `{"device_ip": ip}`.
fn host_credentials(field: &str, host: &str) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert(
        field.to_string(),
        serde_json::Value::String(host.to_string()),
    );
    serde_json::Value::Object(m)
}

// ── SSDP (UPnP) discovery ─────────────────────────────────────────────────────

const SSDP_TARGET: &str = "239.255.255.250:1900";
/// SSDP/eISCP answers arrive within MX (1s); cap the listen window so the UI
/// button returns promptly even when given a longer budget.
const MULTICAST_WINDOW: Duration = Duration::from_secs(2);

/// An SSDP `M-SEARCH` probe for search target `st`.
pub fn msearch(st: &str) -> Vec<u8> {
    format!(
        "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 1\r\nST: {st}\r\n\r\n"
    )
    .into_bytes()
}

/// Pull the host out of an SSDP reply's `LOCATION:` header
/// (`http://192.168.1.50:1400/xml/...` → `192.168.1.50`).
pub fn location_host(response: &str) -> Option<String> {
    for line in response.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("location:") {
            let after = rest.trim().strip_prefix("http://")?;
            let host = after.split([':', '/']).next()?;
            if !host.is_empty() {
                return Some(host.to_string());
            }
        }
    }
    None
}

/// Discovery via SSDP: multicast an `M-SEARCH`, keep replies that carry the
/// expected `signature` (a lowercase substring, `""` = accept any), and read
/// each device's IP from its `LOCATION` header. Shared by Sonos (ZonePlayer)
/// and Hue (IpBridge).
pub struct SsdpDiscovery {
    st: &'static str,
    signature: &'static str,
    label: &'static str,
    cred_field: &'static str,
    target: SocketAddr,
}

impl SsdpDiscovery {
    pub fn new(
        st: &'static str,
        signature: &'static str,
        label: &'static str,
        cred_field: &'static str,
    ) -> Self {
        Self {
            st,
            signature,
            label,
            cred_field,
            target: SSDP_TARGET.parse().unwrap(),
        }
    }

    #[cfg(test)]
    fn to(mut self, target: SocketAddr) -> Self {
        self.target = target;
        self
    }
}

#[async_trait]
impl DeviceDiscovery for SsdpDiscovery {
    async fn scan(&self, opts: &ScanOptions) -> Result<Vec<DiscoveredDevice>> {
        let replies = udp_probe(
            self.target,
            &msearch(self.st),
            opts.timeout.min(MULTICAST_WINDOW),
        )
        .await?;
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (_, bytes) in replies {
            let text = String::from_utf8_lossy(&bytes);
            if !self.signature.is_empty() && !text.to_ascii_lowercase().contains(self.signature) {
                continue;
            }
            let Some(host) = location_host(&text) else {
                continue;
            };
            if !seen.insert(host.clone()) {
                continue;
            }
            out.push(DiscoveredDevice {
                label: Some(self.label.to_string()),
                credentials: host_credentials(self.cred_field, &host),
                host,
            });
        }
        Ok(out)
    }
}

// ── HTTP signature sweep ──────────────────────────────────────────────────────

/// The primary local IPv4, found by asking the kernel which source address it
/// would use to reach the internet (no packets are sent). `None` when offline
/// or only loopback is available.
fn local_ipv4() -> Option<Ipv4Addr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_unspecified() => Some(v4),
        _ => None,
    }
}

/// Every `http://host` in the local /24 except our own address.
fn subnet_bases(ip: Ipv4Addr) -> Vec<String> {
    let o = ip.octets();
    (1u8..=254)
        .filter(|&h| h != o[3])
        .map(|h| format!("http://{}.{}.{}.{}", o[0], o[1], o[2], h))
        .collect()
}

/// Every `http://host` in the /24 with the given base address (`.1`–`.254`).
fn extra_subnet_bases(base: Ipv4Addr) -> Vec<String> {
    let o = base.octets();
    (1u8..=254)
        .map(|h| format!("http://{}.{}.{}.{}", o[0], o[1], o[2], h))
        .collect()
}

/// GET `{base}{path}` for every base in parallel (capped), returning the bases
/// whose response body satisfies `matches`.
async fn http_probe(
    bases: Vec<String>,
    path: &str,
    per_host_timeout: Duration,
    matches: fn(&str) -> bool,
) -> Vec<String> {
    let Ok(client) = reqwest::Client::builder().timeout(per_host_timeout).build() else {
        return Vec::new();
    };
    let path = path.to_string();
    stream::iter(bases)
        .map(|base| {
            let client = client.clone();
            let url = format!("{base}{path}");
            async move {
                let body = client.get(&url).send().await.ok()?.text().await.ok()?;
                matches(&body).then_some(base)
            }
        })
        .buffer_unordered(64)
        .filter_map(|x| async move { x })
        .collect()
        .await
}

/// Discovery for HTTP-only LAN devices that don't broadcast (WLED, Tasmota,
/// Shelly): sweep the local /24, hitting each provider's own signature endpoint
/// and matching the response body. The match is authoritative — it's the same
/// response the provider uses to talk to the device.
pub struct HttpSweepDiscovery {
    path: &'static str,
    label: &'static str,
    cred_field: &'static str,
    signature: fn(&str) -> bool,
    /// Injected in tests; `None` = derive the local /24 at runtime.
    bases: Option<Vec<String>>,
}

impl HttpSweepDiscovery {
    pub fn new(
        path: &'static str,
        label: &'static str,
        cred_field: &'static str,
        signature: fn(&str) -> bool,
    ) -> Self {
        Self {
            path,
            label,
            cred_field,
            signature,
            bases: None,
        }
    }

    #[cfg(test)]
    fn with_bases(mut self, bases: Vec<String>) -> Self {
        self.bases = Some(bases);
        self
    }
}

#[async_trait]
impl DeviceDiscovery for HttpSweepDiscovery {
    async fn scan(&self, opts: &ScanOptions) -> Result<Vec<DiscoveredDevice>> {
        let mut bases = match &self.bases {
            Some(b) => b.clone(),
            None => local_ipv4().map(subnet_bases).unwrap_or_default(),
        };
        // Expanded-LAN: also sweep each configured private /24 (deduped).
        for subnet in &opts.extra_subnets {
            for base in extra_subnet_bases(*subnet) {
                if !bases.contains(&base) {
                    bases.push(base);
                }
            }
        }
        if bases.is_empty() {
            return Ok(Vec::new());
        }
        // Cap per-host wait so a full sweep fits the budget (unused IPs hang to
        // this limit; live ones answer in milliseconds).
        let per_host = opts.timeout.min(Duration::from_millis(600));
        let hosts = http_probe(bases, self.path, per_host, self.signature).await;
        Ok(hosts
            .into_iter()
            .map(|base| {
                let host = base.trim_start_matches("http://").to_string();
                DiscoveredDevice {
                    label: Some(self.label.to_string()),
                    credentials: host_credentials(self.cred_field, &host),
                    host,
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn udp_probe_collects_replies_until_timeout() {
        // A loopback responder that echoes a marker to whoever probes it.
        let responder = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = responder.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = [0u8; 256];
            if let Ok((_, from)) = responder.recv_from(&mut buf).await {
                let _ = responder.send_to(b"PONG", from).await;
            }
        });

        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let replies = udp_probe(target, b"PING", Duration::from_millis(300))
            .await
            .unwrap();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].1, b"PONG");
    }

    #[tokio::test]
    async fn udp_probe_returns_empty_when_nothing_answers() {
        // Nothing is listening on this port; the probe just times out.
        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, 1));
        let replies = udp_probe(target, b"PING", Duration::from_millis(150))
            .await
            .unwrap();
        assert!(replies.is_empty());
    }

    #[test]
    fn location_host_extracts_ip_and_ignores_other_lines() {
        let reply = "HTTP/1.1 200 OK\r\nLOCATION: http://192.168.7.21:1400/xml/device_description.xml\r\nSERVER: Linux UPnP/1.0\r\n\r\n";
        assert_eq!(location_host(reply).as_deref(), Some("192.168.7.21"));
        assert_eq!(location_host("nothing here"), None);
    }

    #[tokio::test]
    async fn ssdp_discovery_matches_signature_and_reads_location() {
        // Loopback responder answers the M-SEARCH with an IpBridge-flavoured
        // reply whose LOCATION carries the device IP.
        let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = sock.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            if let Ok((_, from)) = sock.recv_from(&mut buf).await {
                let reply = "HTTP/1.1 200 OK\r\nLOCATION: http://192.168.7.9:80/description.xml\r\nSERVER: FreeRTOS/7.4.2 UPnP/1.0 IpBridge/1.50.0\r\n\r\n";
                let _ = sock.send_to(reply.as_bytes(), from).await;
            }
        });

        let found = SsdpDiscovery::new("upnp:rootdevice", "ipbridge", "Hue bridge", "bridge_ip")
            .to(addr)
            .scan(&ScanOptions::new(Duration::from_millis(300)))
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].host, "192.168.7.9");
        assert_eq!(found[0].label.as_deref(), Some("Hue bridge"));
        assert_eq!(
            found[0].credentials,
            serde_json::json!({ "bridge_ip": "192.168.7.9" })
        );
    }

    #[tokio::test]
    async fn ssdp_discovery_drops_replies_without_the_signature() {
        let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = sock.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            if let Ok((_, from)) = sock.recv_from(&mut buf).await {
                // A non-Hue UPnP device: valid LOCATION, wrong signature.
                let reply = "HTTP/1.1 200 OK\r\nLOCATION: http://192.168.7.5:80/x.xml\r\nSERVER: SomeOtherDevice/1.0\r\n\r\n";
                let _ = sock.send_to(reply.as_bytes(), from).await;
            }
        });

        let found = SsdpDiscovery::new("upnp:rootdevice", "ipbridge", "Hue bridge", "bridge_ip")
            .to(addr)
            .scan(&ScanOptions::new(Duration::from_millis(300)))
            .await
            .unwrap();
        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn http_sweep_returns_hosts_whose_body_matches_the_signature() {
        // One server looks like WLED, one doesn't; the sweep keeps only the match.
        let wled = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/json/info"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"ver":"0.14","brand":"WLED","leds":{}}"#),
            )
            .mount(&wled)
            .await;
        let other = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/json/info"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"hello":"world"}"#))
            .mount(&other)
            .await;

        let found =
            HttpSweepDiscovery::new("/json/info", "WLED", "device_ip", |b| b.contains("WLED"))
                .with_bases(vec![wled.uri(), other.uri()])
                .scan(&ScanOptions::new(Duration::from_secs(1)))
                .await
                .unwrap();

        assert_eq!(found.len(), 1);
        // wiremock.uri() is http://127.0.0.1:PORT → host keeps the port here.
        assert!(found[0].host.starts_with("127.0.0.1:"));
        assert_eq!(found[0].label.as_deref(), Some("WLED"));
        assert!(found[0].credentials.get("device_ip").is_some());
    }

    #[tokio::test]
    async fn http_sweep_with_no_local_network_returns_empty() {
        // No injected bases and (in CI) no routable IPv4 → empty, never an error.
        let found =
            HttpSweepDiscovery::new("/json/info", "WLED", "device_ip", |b| b.contains("WLED"))
                .with_bases(vec![])
                .scan(&ScanOptions::new(Duration::from_millis(200)))
                .await
                .unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn expanded_lan_generates_full_24_for_a_private_base() {
        // The Expanded-LAN address set for one configured /24.
        let bases = extra_subnet_bases(Ipv4Addr::new(192, 168, 7, 0));
        assert_eq!(bases.len(), 254);
        assert_eq!(bases[0], "http://192.168.7.1");
        assert_eq!(bases[253], "http://192.168.7.254");
    }

    #[tokio::test]
    async fn http_sweep_finds_a_device_via_an_extra_subnet_base() {
        // Prove the merge path: with no local subnet, an injected base plus the
        // signature match still surfaces the device. (Real extra /24s expand to
        // port-80 hosts; here we exercise the matching + mapping end to end.)
        let wled = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/json/info"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"brand":"WLED"}"#))
            .mount(&wled)
            .await;

        let found =
            HttpSweepDiscovery::new("/json/info", "WLED", "device_ip", |b| b.contains("WLED"))
                .with_bases(vec![wled.uri()])
                .scan(&ScanOptions {
                    timeout: Duration::from_millis(400),
                    extra_subnets: vec![Ipv4Addr::new(10, 0, 0, 0)],
                })
                .await
                .unwrap();

        // The injected WLED base matches; the 10.0.0.x hosts don't answer.
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label.as_deref(), Some("WLED"));
    }
}
