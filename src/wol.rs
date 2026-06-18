//! Wake-on-LAN — send a "magic packet" so a power-on reaches a device that's in
//! deep/network standby (e.g. a TV) which the provider's own `turn_on` can't wake.
//!
//! Keyed on the device's normalized `hw_id` (`mac:<hex>` from
//! `providers::mac_hw_id`). Best-effort: a non-MAC id (EUI-64 / none) is a no-op,
//! and a send failure is logged by the caller, never fatal — WoL is a *nudge*
//! before the real `turn_on`, not a hard dependency. The target TV must have
//! "network standby" / "remote start" enabled for WoL to do anything.

use anyhow::{Context, Result};
use tokio::net::UdpSocket;

/// The standard WoL port. Many NICs accept the packet on any port (it's matched
/// at the L2 frame level), but 9 (discard) is the convention.
const WOL_PORT: u16 = 9;

/// Build the 102-byte magic packet for a 6-byte MAC: six `0xFF` bytes, then the
/// MAC repeated 16 times.
pub fn magic_packet(mac: [u8; 6]) -> [u8; 102] {
    let mut packet = [0xFFu8; 102];
    for chunk in packet[6..].chunks_exact_mut(6) {
        chunk.copy_from_slice(&mac);
    }
    packet
}

/// Parse a normalized `mac:<12hex>` id into 6 bytes. `None` for anything that
/// isn't a MAC-48 (EUI-64's 16 hex, a null id, garbage) — WoL only works on a
/// 6-byte MAC.
pub fn mac_from_hw_id(hw_id: &str) -> Option<[u8; 6]> {
    // Only `mac:<hex>` ids are wakeable. Strip the prefix *first* — otherwise the
    // `a`/`c` in "mac:" leak into a naive hex filter and corrupt the parse.
    let hex = hw_id.strip_prefix("mac:")?;
    if hex.len() != 12 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut mac = [0u8; 6];
    for (i, byte) in mac.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(mac)
}

/// Best-effort Wake-on-LAN for a device's `hw_id`: broadcast its magic packet.
/// `Ok(())` (no-op) when the id isn't a MAC-48; errors are surfaced for the
/// caller to log non-fatally.
pub async fn wake(hw_id: &str) -> Result<()> {
    let Some(mac) = mac_from_hw_id(hw_id) else {
        return Ok(()); // not a wakeable MAC — nothing to do
    };
    let sock = UdpSocket::bind(("0.0.0.0", 0))
        .await
        .context("binding WoL socket")?;
    sock.set_broadcast(true)
        .context("enabling broadcast for WoL")?;
    sock.send_to(&magic_packet(mac), ("255.255.255.255", WOL_PORT))
        .await
        .context("sending WoL magic packet")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_packet_is_ff_header_plus_16_macs() {
        let mac = [0xd0, 0x73, 0xd5, 0x12, 0x34, 0x56];
        let p = magic_packet(mac);
        assert_eq!(p.len(), 102);
        assert_eq!(&p[..6], &[0xFF; 6], "6-byte 0xFF header");
        for i in 0..16 {
            assert_eq!(&p[6 + i * 6..12 + i * 6], &mac, "MAC repeat {i}");
        }
    }

    #[test]
    fn mac_from_hw_id_only_accepts_mac48() {
        assert_eq!(
            mac_from_hw_id("mac:d073d5123456"),
            Some([0xd0, 0x73, 0xd5, 0x12, 0x34, 0x56]),
        );
        // EUI-64 (16 hex) is not a WoL MAC.
        assert_eq!(mac_from_hw_id("mac:00178801abcdef01"), None);
        assert_eq!(mac_from_hw_id("not-a-mac"), None);
        assert_eq!(mac_from_hw_id(""), None);
    }

    #[tokio::test]
    async fn wake_is_noop_for_non_mac() {
        // No socket is opened for a non-MAC id, so this can't fail.
        assert!(wake("eui:whatever").await.is_ok());
    }
}
