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
use serde::Serialize;
use std::net::SocketAddr;
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

/// A provider's network auto-detect. One method: probe the LAN, return what
/// answered. Implementations stay thin — the I/O lives in [`udp_probe`].
#[async_trait]
pub trait DeviceDiscovery: Send + Sync {
    async fn scan(&self, timeout: Duration) -> Result<Vec<DiscoveredDevice>>;
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
    let bind = if target.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

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
}
