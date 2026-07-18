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

/// Gap between probe retransmissions inside the listen window.
const RESEND_GAP: Duration = Duration::from_millis(400);

/// Send `payload` to `target` (a broadcast or multicast address), then collect
/// every reply datagram until `timeout` elapses. The probe is **retransmitted
/// up to twice** (~400ms apart) within the window: discovery datagrams are
/// unacknowledged UDP, and a Wi-Fi device dozing in power-save routinely misses
/// the first multicast frame — the UPnP spec itself tells control points to
/// send M-SEARCH more than once. Every consumer dedupes replies by host, so a
/// device answering each retransmission is harmless. Binds an ephemeral local
/// port on all interfaces; replies arrive there as unicast, so no
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
    // The first send is fail-fast (an unreachable network should surface);
    // retransmissions are best-effort.
    socket
        .send_to(payload, target)
        .await
        .with_context(|| format!("sending discovery probe to {target}"))?;

    let mut replies = Vec::new();
    let mut buf = [0u8; 2048];
    let deadline = tokio::time::Instant::now() + timeout;
    let mut resends_left: u8 = 2;
    let mut next_send = tokio::time::Instant::now() + RESEND_GAP;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        // Wake at whichever comes first: the retransmit slot or the deadline.
        let wake = if resends_left > 0 {
            next_send.min(deadline)
        } else {
            deadline
        };
        match tokio::time::timeout(wake - now, socket.recv_from(&mut buf)).await {
            Ok(Ok((n, from))) => replies.push((from, buf[..n].to_vec())),
            Ok(Err(_)) => break, // socket error — stop collecting
            Err(_) => {
                // This wait segment elapsed: retransmit if that's what we woke
                // for, otherwise the whole window is done.
                if resends_left > 0 && tokio::time::Instant::now() >= next_send {
                    let _ = socket.send_to(payload, target).await;
                    resends_left -= 1;
                    next_send = tokio::time::Instant::now() + RESEND_GAP;
                } else {
                    break;
                }
            }
        }
    }
    tracing::debug!(
        target: "bifrost::discover",
        %target,
        replies = replies.len(),
        resends = 2 - resends_left,
        "udp probe complete",
    );
    Ok(replies)
}

/// Credentials object pre-shaped for one host field, e.g. `{"device_ip": ip}`.
pub(crate) fn host_credentials(field: &str, host: &str) -> serde_json::Value {
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
        let raw = replies.len();
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
        tracing::debug!(
            target: "bifrost::discover",
            st = self.st,
            signature = self.signature,
            raw,
            matched = out.len(),
            "ssdp scan",
        );
        Ok(out)
    }
}

// ── HTTP signature sweep ──────────────────────────────────────────────────────

/// The primary local IPv4, found by asking the kernel which source address it
/// would use to reach the internet (no packets are sent). `None` when offline
/// or only loopback is available.
pub(crate) fn local_ipv4() -> Option<Ipv4Addr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_unspecified() => Some(v4),
        _ => None,
    }
}

/// Every `http://host` in the local /24 except our own address.
pub(crate) fn subnet_bases(ip: Ipv4Addr) -> Vec<String> {
    let o = ip.octets();
    (1u8..=254)
        .filter(|&h| h != o[3])
        .map(|h| format!("http://{}.{}.{}.{}", o[0], o[1], o[2], h))
        .collect()
}

/// Every `http://host` in the /24 with the given base address (`.1`–`.254`).
pub(crate) fn extra_subnet_bases(base: Ipv4Addr) -> Vec<String> {
    let o = base.octets();
    (1u8..=254)
        .map(|h| format!("http://{}.{}.{}.{}", o[0], o[1], o[2], h))
        .collect()
}

/// Hit `{base}{path}` for every base in parallel (capped), returning the bases
/// whose response body satisfies `matches`. `post_body = None` is a GET;
/// `Some(json)` POSTs that body (for probe endpoints that are RPC-shaped, like
/// Sony's ScalarWeb).
async fn http_probe(
    bases: Vec<String>,
    path: &str,
    post_body: Option<&'static str>,
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
                let req = match post_body {
                    None => client.get(&url),
                    Some(body) => client
                        .post(&url)
                        .header("content-type", "application/json")
                        .body(body),
                };
                let body = req.send().await.ok()?.text().await.ok()?;
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
    /// `None` = GET; `Some(json)` = POST that body (RPC-shaped probe endpoints).
    post_body: Option<&'static str>,
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
            post_body: None,
            bases: None,
        }
    }

    /// Probe by POSTing `body` (JSON) instead of a GET.
    pub fn post(mut self, body: &'static str) -> Self {
        self.post_body = Some(body);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_bases(mut self, bases: Vec<String>) -> Self {
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
        let probed = bases.len();
        let hosts = http_probe(bases, self.path, self.post_body, per_host, self.signature).await;
        tracing::debug!(
            target: "bifrost::discover",
            path = self.path,
            probed,
            matched = hosts.len(),
            "http sweep",
        );
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

// ── mDNS (one-shot) discovery ─────────────────────────────────────────────────

const MDNS_TARGET: &str = "224.0.0.251:5353";

/// One-shot mDNS service discovery: query a `_service._tcp.local` PTR with the
/// QU (unicast-response) bit set, so answers come straight back to our probe
/// socket — the same probe-and-listen pattern as SSDP, no group membership.
/// Android/Google TVs (dongles included) advertise `_androidtvremote2._tcp`
/// with the device's friendly name as the instance and its Bluetooth MAC in
/// TXT `bt=` — a hardware id, which makes de-dup against an HA copy exact.
pub struct MdnsDiscovery {
    /// e.g. `"_androidtvremote2._tcp.local"`.
    service: &'static str,
    /// `brand` stamped into the credentials (routes `build_vendor`).
    brand: &'static str,
}

impl MdnsDiscovery {
    pub fn new(service: &'static str, brand: &'static str) -> Self {
        Self { service, brand }
    }
}

/// Encode a DNS name as length-prefixed labels.
fn dns_name(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for label in name.split('.').filter(|l| !l.is_empty()) {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out
}

/// Read a (possibly compressed) DNS name at `pos`; returns the name and the
/// position just after it in the *uncompressed* stream.
fn read_dns_name(pkt: &[u8], mut pos: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let mut jumped_end = None;
    let mut hops = 0;
    loop {
        let len = *pkt.get(pos)? as usize;
        if len == 0 {
            pos += 1;
            break;
        }
        if len & 0xC0 == 0xC0 {
            // Compression pointer (2 bytes). Only the first jump sets the end.
            let ptr = ((len & 0x3F) << 8) | *pkt.get(pos + 1)? as usize;
            if jumped_end.is_none() {
                jumped_end = Some(pos + 2);
            }
            pos = ptr;
            hops += 1;
            if hops > 16 {
                return None; // pointer loop
            }
            continue;
        }
        labels.push(String::from_utf8_lossy(pkt.get(pos + 1..pos + 1 + len)?).into_owned());
        pos += 1 + len;
    }
    Some((labels.join("."), jumped_end.unwrap_or(pos)))
}

/// Pull the instance label and the TXT `bt=` MAC (if any) out of one mDNS
/// response to our service query. The responder's source IP is the host.
fn parse_mdns_response(pkt: &[u8], service: &str) -> Option<(Option<String>, Option<String>)> {
    if pkt.len() < 12 || pkt[2] & 0x80 == 0 {
        return None; // not a DNS response
    }
    let counts: Vec<u16> = (0..4)
        .map(|i| u16::from_be_bytes([pkt[4 + i * 2], pkt[5 + i * 2]]))
        .collect();
    let mut pos = 12;
    // Skip questions.
    for _ in 0..counts[0] {
        let (_, next) = read_dns_name(pkt, pos)?;
        pos = next + 4;
    }
    let mut instance: Option<String> = None;
    let mut bt: Option<String> = None;
    let service_l = service.to_ascii_lowercase();
    for _ in 0..(counts[1] + counts[2] + counts[3]) {
        let (name, next) = read_dns_name(pkt, pos)?;
        let rtype = u16::from_be_bytes([*pkt.get(next)?, *pkt.get(next + 1)?]);
        let rdlen = u16::from_be_bytes([*pkt.get(next + 8)?, *pkt.get(next + 9)?]) as usize;
        let rdata_at = next + 10;
        let rdata = pkt.get(rdata_at..rdata_at + rdlen)?;
        match rtype {
            12 if name.to_ascii_lowercase() == service_l => {
                // PTR → "<Instance>.<service>": the first label is the name.
                if let Some((target, _)) = read_dns_name(pkt, rdata_at) {
                    instance = target.split('.').next().map(str::to_string);
                }
            }
            16 => {
                // TXT: length-prefixed strings; find bt=<mac>.
                let mut t = 0;
                while t < rdata.len() {
                    let l = rdata[t] as usize;
                    let kv = rdata.get(t + 1..t + 1 + l)?;
                    if let Some(mac) = std::str::from_utf8(kv)
                        .ok()
                        .and_then(|s| s.strip_prefix("bt="))
                    {
                        bt = Some(mac.to_string());
                    }
                    t += 1 + l;
                }
            }
            _ => {}
        }
        pos = rdata_at + rdlen;
    }
    Some((instance, bt))
}

#[async_trait]
impl DeviceDiscovery for MdnsDiscovery {
    async fn scan(&self, opts: &ScanOptions) -> Result<Vec<DiscoveredDevice>> {
        // DNS query: id 0, no flags, one question: PTR IN + QU bit.
        let mut q = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        q.extend(dns_name(self.service));
        q.extend_from_slice(&12u16.to_be_bytes()); // PTR
        q.extend_from_slice(&0x8001u16.to_be_bytes()); // IN | QU (unicast reply)
        let replies = udp_probe(MDNS_TARGET.parse()?, &q, opts.timeout).await?;
        let mut out = Vec::new();
        for (from, pkt) in &replies {
            let Some((instance, bt)) = parse_mdns_response(pkt, self.service) else {
                continue;
            };
            let host = from.ip().to_string();
            let mut creds = serde_json::Map::new();
            creds.insert("host".into(), host.clone().into());
            // An empty brand = a single-vendor provider (no `build_vendor`
            // routing, e.g. Nanoleaf) — don't stamp a meaningless key.
            if !self.brand.is_empty() {
                creds.insert("brand".into(), self.brand.into());
            }
            if let Some(n) = &instance {
                creds.insert("name".into(), n.clone().into());
            }
            if let Some(mac) = bt.as_deref().and_then(crate::providers::mac_hw_id) {
                creds.insert("hw_id".into(), mac.into());
            }
            out.push(DiscoveredDevice {
                host,
                label: instance,
                credentials: serde_json::Value::Object(creds),
            });
        }
        tracing::debug!(
            target: "bifrost::discover",
            service = self.service,
            replies = replies.len(),
            matched = out.len(),
            "mDNS scan",
        );
        Ok(out)
    }
}

// ── TCP port sweep ────────────────────────────────────────────────────────────

/// Find devices by an open TCP port — the multicast-proof fallback (WSL2,
/// container bridges, mcast-dropping APs). An Android/Google TV is the only
/// thing that listens on 6466, so a successful connect IS the signature.
pub struct TcpPortSweepDiscovery {
    port: u16,
    label: &'static str,
    brand: &'static str,
    /// Injected in tests; `None` = derive the local /24 at runtime.
    hosts: Option<Vec<String>>,
}

impl TcpPortSweepDiscovery {
    pub fn new(port: u16, label: &'static str, brand: &'static str) -> Self {
        Self {
            port,
            label,
            brand,
            hosts: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_hosts(mut self, hosts: Vec<String>) -> Self {
        self.hosts = Some(hosts);
        self
    }
}

#[async_trait]
impl DeviceDiscovery for TcpPortSweepDiscovery {
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
        let per_host = opts.timeout.min(Duration::from_millis(500));
        let port = self.port;
        let probed = hosts.len();
        let results = futures_util::stream::iter(hosts.into_iter().map(|h| async move {
            let ok =
                tokio::time::timeout(per_host, tokio::net::TcpStream::connect((h.as_str(), port)))
                    .await
                    .map(|r| r.is_ok())
                    .unwrap_or(false);
            ok.then_some(h)
        }))
        .buffer_unordered(64)
        .filter_map(|h| async move { h })
        .collect::<Vec<_>>()
        .await;
        tracing::debug!(
            target: "bifrost::discover",
            port,
            probed,
            matched = results.len(),
            "tcp port sweep",
        );
        Ok(results
            .into_iter()
            .map(|host| {
                let mut creds = serde_json::Map::new();
                creds.insert("host".into(), host.clone().into());
                if !self.brand.is_empty() {
                    creds.insert("brand".into(), self.brand.into());
                }
                DiscoveredDevice {
                    label: Some(self.label.to_string()),
                    credentials: serde_json::Value::Object(creds),
                    host,
                }
            })
            .collect())
    }
}

// ── Union of discoverers ──────────────────────────────────────────────────────

/// Run several discoverers concurrently and merge their results, deduped by
/// host (first leg wins — order the legs by precision). One leg failing to
/// probe never hides another's finds: a device that broadcasts *and* answers an
/// HTTP signature is found even where multicast is broken (container bridge
/// networking, APs that drop multicast), because the sweep leg still works.
pub struct UnionDiscovery(Vec<Box<dyn DeviceDiscovery>>);

impl UnionDiscovery {
    pub fn new(legs: Vec<Box<dyn DeviceDiscovery>>) -> Self {
        Self(legs)
    }
}

#[async_trait]
impl DeviceDiscovery for UnionDiscovery {
    async fn scan(&self, opts: &ScanOptions) -> Result<Vec<DiscoveredDevice>> {
        let results = futures_util::future::join_all(self.0.iter().map(|leg| leg.scan(opts))).await;
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for r in results {
            match r {
                Ok(devices) => {
                    for d in devices {
                        if seen.insert(d.host.clone()) {
                            out.push(d);
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!(target: "bifrost::discover", "union leg could not probe: {e:#}");
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a plausible mDNS response for `_androidtvremote2._tcp.local`:
    /// PTR (with a compression pointer back to the service name) + TXT bt=…
    fn mdns_response() -> Vec<u8> {
        let mut p = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 1];
        let service_at = p.len() as u16; // name offset for compression
        p.extend(dns_name("_androidtvremote2._tcp.local"));
        p.extend_from_slice(&12u16.to_be_bytes()); // PTR
        p.extend_from_slice(&1u16.to_be_bytes()); // IN
        p.extend_from_slice(&120u32.to_be_bytes());
        // rdata: "Bedroom dongle" + pointer to the service name.
        let mut rdata = Vec::new();
        rdata.push(14);
        rdata.extend_from_slice(b"Bedroom dongle");
        rdata.extend_from_slice(&(0xC000u16 | service_at).to_be_bytes());
        p.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        let instance_at = p.len() as u16; // instance name lives inside rdata
        p.extend(rdata);
        // TXT for the instance (compressed name), bt=<mac>.
        p.extend_from_slice(&(0xC000u16 | instance_at).to_be_bytes());
        p.extend_from_slice(&16u16.to_be_bytes()); // TXT
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&120u32.to_be_bytes());
        let txt = b"bt=BC:DF:58:61:07:A7";
        p.extend_from_slice(&((txt.len() + 1) as u16).to_be_bytes());
        p.push(txt.len() as u8);
        p.extend_from_slice(txt);
        p
    }

    #[test]
    fn mdns_response_parses_instance_and_bt_mac() {
        let pkt = mdns_response();
        let (instance, bt) =
            parse_mdns_response(&pkt, "_androidtvremote2._tcp.local").expect("parses");
        assert_eq!(instance.as_deref(), Some("Bedroom dongle"));
        assert_eq!(bt.as_deref(), Some("BC:DF:58:61:07:A7"));
        // Not-a-response (QR bit clear) is rejected.
        let mut query = pkt.clone();
        query[2] = 0;
        assert!(parse_mdns_response(&query, "_androidtvremote2._tcp.local").is_none());
    }

    #[test]
    fn dns_name_roundtrips_through_the_reader() {
        let enc = dns_name("_androidtvremote2._tcp.local");
        let (name, next) = read_dns_name(&enc, 0).unwrap();
        assert_eq!(name, "_androidtvremote2._tcp.local");
        assert_eq!(next, enc.len());
        // A pointer loop is rejected rather than spinning.
        let looped = vec![0xC0, 0x00];
        assert!(read_dns_name(&looped, 0).is_none());
    }

    #[tokio::test]
    async fn tcp_port_sweep_finds_an_open_port_and_skips_closed() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let _ = listener.accept().await;
            }
        });
        let sweep = TcpPortSweepDiscovery::new(port, "Android TV", "androidtv")
            .with_hosts(vec!["127.0.0.1".into(), "127.6.6.6".into()]);
        let found = sweep
            .scan(&ScanOptions::new(Duration::from_millis(600)))
            .await
            .unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].host, "127.0.0.1");
        assert_eq!(found[0].credentials["brand"], "androidtv");
    }

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
    async fn udp_probe_retransmits_when_the_first_probe_is_lost() {
        // The responder swallows the first datagram (a dozing Wi-Fi device) and
        // only answers the retransmission.
        let responder = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = responder.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = [0u8; 256];
            let _ = responder.recv_from(&mut buf).await; // first probe: dropped
            if let Ok((_, from)) = responder.recv_from(&mut buf).await {
                let _ = responder.send_to(b"PONG", from).await;
            }
        });

        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let replies = udp_probe(target, b"PING", Duration::from_secs(2))
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
    async fn http_sweep_post_probes_with_the_body_and_matches_the_response() {
        // An RPC-shaped probe endpoint (ScalarWeb style): the sweep POSTs the
        // configured JSON body and matches on the response.
        let tv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sony/system"))
            .and(wiremock::matchers::body_string_contains(
                "getInterfaceInformation",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"result":[{"productCategory":"tv","productName":"BRAVIA"}],"id":1}"#,
            ))
            .mount(&tv)
            .await;

        let found = HttpSweepDiscovery::new("/sony/system", "Sony Bravia", "host", |b| {
            b.contains(r#""productCategory":"tv""#)
        })
        .post(r#"{"method":"getInterfaceInformation","id":1,"params":[],"version":"1.0"}"#)
        .with_bases(vec![tv.uri()])
        .scan(&ScanOptions::new(Duration::from_secs(1)))
        .await
        .unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label.as_deref(), Some("Sony Bravia"));
    }

    /// A stub discovery leg returning fixed hosts (or an error).
    struct StubLeg(Result<Vec<&'static str>, &'static str>);

    #[async_trait]
    impl DeviceDiscovery for StubLeg {
        async fn scan(&self, _opts: &ScanOptions) -> Result<Vec<DiscoveredDevice>> {
            match &self.0 {
                Ok(hosts) => Ok(hosts
                    .iter()
                    .map(|h| DiscoveredDevice {
                        host: h.to_string(),
                        label: None,
                        credentials: host_credentials("host", h),
                    })
                    .collect()),
                Err(e) => Err(anyhow::anyhow!(*e)),
            }
        }
    }

    #[tokio::test]
    async fn union_discovery_merges_legs_dedupes_hosts_and_survives_a_failed_leg() {
        let union = UnionDiscovery::new(vec![
            Box::new(StubLeg(Ok(vec!["192.168.1.22", "192.168.1.30"]))),
            Box::new(StubLeg(Err("multicast unavailable"))), // must not hide the others
            Box::new(StubLeg(Ok(vec!["192.168.1.22", "192.168.1.40"]))), // dup of leg 1
        ]);
        let found = union
            .scan(&ScanOptions::new(Duration::from_millis(100)))
            .await
            .unwrap();
        let hosts: Vec<&str> = found.iter().map(|d| d.host.as_str()).collect();
        assert_eq!(hosts, vec!["192.168.1.22", "192.168.1.30", "192.168.1.40"]);
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
