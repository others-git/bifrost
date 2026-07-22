//! Host relocation — self-healing after a DHCP change, for every provider that
//! reaches its device at a **stored LAN address**.
//!
//! A power outage routinely hands devices fresh DHCP leases; a provider's stored
//! credentials still pin the old IP, so the device silently drops off every
//! surface until someone re-configures it by hand. This slow watch (~30s) probes
//! each enabled address-pinned provider's stored host with a cheap TCP connect;
//! when one is unreachable it re-runs that type's own discovery legs and rebinds
//! the credentials to a candidate that **proves it is the same device**:
//!
//! - a discovery candidate carrying one of the provider's known hardware ids
//!   (the mDNS `bt` MAC an Android TV advertises, the MAC in an Onkyo `ECN`
//!   reply, a Kasa plug's `get_sysinfo`), or
//! - the provider's own live check ([`LanBinding::is_same_device`]) — a Bravia's
//!   ScalarWeb MAC, a Sonos player's `UDN`, or, for token-bound gear with no
//!   readable MAC (Hue, Nanoleaf), a credential that only the real device
//!   accepts.
//!
//! An unverifiable candidate is never adopted — rebinding to a neighbour's
//! device would be strictly worse than staying lost. On a match the credentials
//! are rewritten (host field only) and the provider manager restarts, so push
//! channels and pollers reattach to the new address immediately.
//!
//! Which providers participate is declared by the provider itself
//! ([`crate::providers::LanBinding`]), not listed here — cloud providers and
//! MAC-addressed LAN providers that re-resolve on every scan have no stale
//! address to heal.
//!
//! A device that is genuinely off/unplugged would otherwise make this scan every
//! tick forever, so a provider that fails to relocate **backs off** (doubling,
//! capped at ~16 min); it retries immediately once its stored host answers again.

use crate::AppState;
use crate::providers::discovery::{DiscoveredDevice, ScanOptions};
use crate::providers::{Credentials, LanBinding, cred_str};
use futures_util::StreamExt;
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Cadence of the watch — slow enough to be invisible, fast enough that a device
/// coming back from an outage is usable again within a minute.
const RELOCATE_TICK: Duration = Duration::from_secs(30);

/// Budget for the TCP reachability probe of the stored host.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// How many stored hosts to probe at once. The sweep is idle waiting on TCP, so
/// this is about not letting the tick's wall time grow with the device count.
const PROBE_CONCURRENCY: usize = 16;

/// Hard cap on one provider's live identity check. A binding may use its own
/// HTTP client with its own (much longer) timeout; the watch's responsiveness
/// must not depend on every provider having picked a sane one.
const VERIFY_TIMEOUT: Duration = Duration::from_secs(5);

/// Ticks to skip after the first failed relocation, doubling per consecutive
/// failure up to [`MAX_BACKOFF_TICKS`]. An unplugged device is the common case
/// and must not cost a network scan every 30s forever.
const MIN_BACKOFF_TICKS: u32 = 2;
const MAX_BACKOFF_TICKS: u32 = 32; // ~16 minutes at a 30s tick

/// Background loop: spawned once at startup (`lib.rs`).
pub async fn relocate_loop(state: Arc<AppState>) {
    let mut ticker = tokio::time::interval(RELOCATE_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut backoff = Backoff::default();
    loop {
        ticker.tick().await;
        backoff.tick();
        let all_lost = lost_devices(&state).await;
        // Forget providers that are healthy again, were deleted, or became
        // unprovable — otherwise every id that ever failed lingers for the life
        // of the process. Dropping an entry also means a device that comes back
        // and later fails again starts from the shortest wait.
        backoff.retain(all_lost.iter().map(|d| d.provider_id.as_str()));
        let lost: Vec<LostDevice> = all_lost
            .into_iter()
            .filter(|d| backoff.ready(&d.provider_id))
            .collect();
        if lost.is_empty() {
            continue;
        }
        let extra_subnets = crate::api::settings::expanded_subnets(&state).await;
        let timeout = if extra_subnets.is_empty() {
            Duration::from_secs(2)
        } else {
            Duration::from_secs(6)
        };

        // One scan per provider TYPE serves every lost provider of that type
        // this pass. The discoverer is the same union of legs the add-provider
        // "Scan network" button runs.
        let mut by_type: HashMap<String, Vec<LostDevice>> = HashMap::new();
        for d in lost {
            by_type.entry(d.provider_type.clone()).or_default().push(d);
        }
        for (provider_type, group) in by_type {
            let Some(discoverer) = state.registry.discoverer(&provider_type) else {
                continue; // no discoverer — nothing this loop can do for the type
            };
            let candidates = discoverer
                .scan(&ScanOptions {
                    timeout,
                    extra_subnets: extra_subnets.clone(),
                })
                .await
                .unwrap_or_default();
            let ids: Vec<String> = group.iter().map(|d| d.provider_id.clone()).collect();
            let healed = relocate_with_candidates(&state, group, &candidates).await;
            for id in ids {
                if healed.contains(&id) {
                    backoff.clear(&id);
                } else {
                    backoff.missed(&id);
                }
            }
        }
    }
}

/// Per-provider retry spacing, so a permanently-absent device doesn't keep
/// scanning the network every tick.
#[derive(Default)]
struct Backoff {
    /// provider id → (consecutive misses, ticks left before the next attempt).
    state: HashMap<String, (u32, u32)>,
}

impl Backoff {
    /// Advance every countdown by one tick.
    fn tick(&mut self) {
        for (_, wait) in self.state.values_mut() {
            *wait = wait.saturating_sub(1);
        }
    }

    fn ready(&self, provider_id: &str) -> bool {
        self.state
            .get(provider_id)
            .is_none_or(|(_, wait)| *wait == 0)
    }

    /// A relocation attempt found nothing — wait longer before the next one.
    fn missed(&mut self, provider_id: &str) {
        let entry = self.state.entry(provider_id.to_string()).or_insert((0, 0));
        entry.0 = entry.0.saturating_add(1);
        entry.1 = (MIN_BACKOFF_TICKS.saturating_mul(1 << entry.0.min(5))).min(MAX_BACKOFF_TICKS);
    }

    /// Relocated (or reachable again) — the next failure starts from scratch.
    fn clear(&mut self, provider_id: &str) {
        self.state.remove(provider_id);
    }

    /// Drop everything not in `live` — providers that recovered or went away.
    fn retain<'a>(&mut self, live: impl Iterator<Item = &'a str>) {
        let live: std::collections::HashSet<&str> = live.collect();
        self.state.retain(|id, _| live.contains(id.as_str()));
    }
}

/// A provider whose stored host stopped answering.
pub struct LostDevice {
    pub provider_id: String,
    pub provider_name: String,
    pub provider_type: String,
    /// Decrypted credential object.
    pub creds: Credentials,
    pub host: String,
    /// Hardware ids recorded for this provider's devices (plus the creds' own),
    /// any of which a candidate may prove itself with. Possibly empty.
    pub known_hw: Vec<String>,
    binding: Box<dyn LanBinding>,
}

/// Every device table carrying a `hw_id` a provider's devices could be known by.
const HW_TABLES: &[&str] = &[
    "lights",
    "media_devices",
    "power_devices",
    "sensor_devices",
    "remote_devices",
];

/// Hardware ids Bifrost has recorded for one provider, across every device
/// domain — a provider type can serve several (a Smart TV row owns a media and a
/// remote device; a Hue bridge owns lights and sensors).
async fn known_hw_ids(state: &Arc<AppState>, provider_id: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for table in HW_TABLES {
        let rows = sqlx::query_scalar::<_, String>(&format!(
            "SELECT DISTINCT hw_id FROM {table} WHERE provider_id = ? AND hw_id IS NOT NULL"
        ))
        .bind(provider_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
        for id in rows {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    ids
}

/// Find enabled address-pinned providers whose stored host fails a TCP probe.
/// Public with [`relocate_with_candidates`] as the relocator's test seam — the
/// background loop is these two plus a real network scan.
pub async fn lost_devices(state: &Arc<AppState>) -> Vec<LostDevice> {
    let rows =
        sqlx::query("SELECT id, name, provider_type, credentials FROM providers WHERE enabled = 1")
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

    // Everything addressable, before probing.
    let mut candidates = Vec::new();
    for row in rows {
        let provider_type: String = row.get("provider_type");
        // The provider itself declares whether it's address-pinned at all.
        let Some(binding) = state.registry.lan_binding(&provider_type) else {
            continue;
        };
        let Some(creds) = state
            .decrypt_credentials(&row.get::<String, _>("credentials"))
            .ok()
            .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
            .and_then(|v| v.as_object().cloned())
        else {
            continue;
        };
        let Some(host) = cred_str(&creds, binding.host_field()).map(str::to_string) else {
            continue;
        };
        candidates.push(LostDevice {
            provider_id: row.get("id"),
            provider_name: row.get("name"),
            provider_type,
            creds,
            host,
            known_hw: Vec::new(), // filled in below, only for the unreachable
            binding,
        });
    }

    // Probe concurrently. A house runs a dozen address-pinned providers, and a
    // serial sweep costs PROBE_TIMEOUT each — during an outage that alone would
    // outlast the whole tick and starve the scan it exists to trigger.
    let unreachable: Vec<LostDevice> = futures_util::stream::iter(candidates)
        .map(|dev| async move {
            let port = dev.binding.probe_port(&dev.creds);
            (!tcp_reachable(&dev.host, port).await).then_some(dev)
        })
        .buffer_unordered(PROBE_CONCURRENCY)
        .filter_map(|d| async move { d })
        .collect()
        .await;

    let mut lost = Vec::new();
    for mut dev in unreachable {
        dev.known_hw = known_hw_ids(state, &dev.provider_id).await;
        // Discovery may also have stamped the id straight onto the credentials.
        if let Some(hw) = cred_str(&dev.creds, "hw_id").map(str::to_string)
            && !dev.known_hw.contains(&hw)
        {
            dev.known_hw.push(hw);
        }
        // Nothing to prove a candidate against means no scan could ever succeed.
        // Skipping here is what stops an unprovable device (a TV known only by
        // the `host:<ip>` id derived from the address that just changed) from
        // sweeping the network on its behalf for the life of the process.
        if !dev.binding.can_verify(&dev.creds, &dev.known_hw) {
            tracing::debug!(
                target: "bifrost::relocate",
                provider = %dev.provider_id, provider_type = %dev.provider_type, host = %dev.host,
                "unreachable but nothing could prove a replacement — not scanning"
            );
            continue;
        }
        tracing::debug!(
            target: "bifrost::relocate",
            provider = %dev.provider_id, provider_type = %dev.provider_type, host = %dev.host,
            known_hw = dev.known_hw.len(),
            "stored host unreachable — scanning for a new address"
        );
        lost.push(dev);
    }
    lost
}

/// The `host:port` to probe. A credential may hold a bare IP, an explicit port,
/// a full base URL (which several providers accept), or an IPv6 literal — all of
/// which must resolve to a connectable address rather than a mangled string that
/// can never connect and reads as permanently lost.
fn probe_addr(host: &str, default_port: u16) -> String {
    let bare = crate::providers::host_only(host);
    let port = crate::providers::host_port(host).unwrap_or(default_port);
    if bare.contains(':') {
        format!("[{bare}]:{port}") // IPv6 literal needs brackets to connect
    } else {
        format!("{bare}:{port}")
    }
}

/// The stored host answers a TCP connect within [`PROBE_TIMEOUT`].
async fn tcp_reachable(host: &str, default_port: u16) -> bool {
    let addr = probe_addr(host, default_port);
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, tokio::net::TcpStream::connect(&addr)).await,
        Ok(Ok(_))
    )
}

/// Try to rebind each lost provider onto a scan candidate that proves the same
/// hardware identity; returns the ids that were relocated. Public as the
/// relocator's test seam — the loop above is this plus a real network scan.
pub async fn relocate_with_candidates(
    state: &Arc<AppState>,
    lost: Vec<LostDevice>,
    candidates: &[DiscoveredDevice],
) -> Vec<String> {
    // Addresses another provider row is already configured at. A candidate
    // sitting on one of those is a device Bifrost already manages, so adopting
    // it would point two rows at one device rather than heal anything — the
    // reachable failure mode for a household-scoped binding (any Sonos player
    // proves a Sonos row) where a sibling row's host legitimately matches.
    let owned = crate::api::providers::known_provider_hosts(state).await;

    let mut healed = Vec::new();
    for dev in lost {
        let mut new_host: Option<String> = None;
        let self_host = crate::providers::host_only(&dev.host);
        for cand in candidates {
            if cand.host == dev.host {
                continue; // the dead address itself
            }
            // Owned by a DIFFERENT row. Our own address is in the set too, so
            // it's compared out — a candidate answering at the address we're
            // already pinned to is ours by definition, not a collision.
            let cand_host = crate::providers::host_only(&cand.host);
            if cand_host != self_host && owned.contains(cand_host) {
                continue;
            }
            match cand.credentials.get("hw_id").and_then(|v| v.as_str()) {
                // The candidate named itself and it's one of ours — cheapest
                // possible proof, leg- and provider-agnostic.
                Some(hw) if dev.known_hw.iter().any(|k| k == hw) => {
                    new_host = Some(cand.host.clone());
                    break;
                }
                // It named itself as something else. No live check can overturn
                // that, so don't pay for one.
                Some(_) => continue,
                // Anonymous candidate: ask the provider. Bounded here rather
                // than trusting each provider's own client timeouts, so one slow
                // host can't stall the watch for every other lost device.
                None => {
                    let verify = dev
                        .binding
                        .is_same_device(&cand.host, &dev.creds, &dev.known_hw);
                    if tokio::time::timeout(VERIFY_TIMEOUT, verify)
                        .await
                        .unwrap_or(false)
                    {
                        new_host = Some(cand.host.clone());
                        break;
                    }
                }
            }
        }
        let Some(new_host) = new_host else { continue };

        let mut creds = dev.creds.clone();
        creds.insert(dev.binding.host_field().into(), new_host.clone().into());
        let creds_json = serde_json::Value::Object(creds).to_string();
        let Ok(encrypted) = state.encrypt_credentials(&creds_json) else {
            tracing::error!(target: "bifrost::relocate", provider = %dev.provider_id, "relocate: encryption failed");
            continue;
        };
        if let Err(e) = sqlx::query(
            "UPDATE providers SET credentials = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(&encrypted)
        .bind(&dev.provider_id)
        .execute(&state.db)
        .await
        {
            tracing::error!(target: "bifrost::relocate", provider = %dev.provider_id, "relocate: db update failed: {e}");
            continue;
        }
        tracing::info!(
            target: "bifrost::relocate",
            provider = %dev.provider_id, name = %dev.provider_name,
            provider_type = %dev.provider_type,
            old_host = %dev.host, %new_host,
            "device relocated to a new address — restarting provider"
        );
        // Restart the manager so push channels + pollers reattach NOW.
        {
            let mut connections = state.connections.lock().await;
            connections.stop(&dev.provider_id);
            crate::start_manager_for(
                &mut connections,
                state,
                &dev.provider_id,
                &dev.provider_type,
                &creds_json,
            );
        }
        healed.push(dev.provider_id);
    }
    healed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_spaces_retries_and_resets_on_success() {
        let mut b = Backoff::default();
        // Never-seen provider is always ready.
        assert!(b.ready("p1"));

        b.missed("p1");
        assert!(!b.ready("p1"), "a failed attempt waits before retrying");
        for _ in 0..MIN_BACKOFF_TICKS * 2 {
            b.tick();
        }
        assert!(b.ready("p1"));

        // Consecutive failures space out further, capped.
        let mut wait = 0;
        for _ in 0..10 {
            b.missed("p1");
            wait = b.state["p1"].1;
        }
        assert_eq!(wait, MAX_BACKOFF_TICKS, "backoff is capped");

        // A success clears the history entirely.
        b.clear("p1");
        assert!(b.ready("p1"));
    }

    #[test]
    fn backoff_forgets_providers_that_are_no_longer_lost() {
        let mut b = Backoff::default();
        b.missed("gone");
        b.missed("still-lost");
        // Only the still-lost provider is reported this pass; the other
        // recovered or was deleted and must not linger for the process lifetime.
        b.retain(["still-lost"].into_iter());
        assert!(b.ready("gone"), "a recovered provider starts clean");
        assert!(!b.ready("still-lost"));
    }

    #[test]
    fn probe_addr_survives_schemes_ports_paths_and_ipv6() {
        // A bare IP takes the binding's default port.
        assert_eq!(probe_addr("192.168.1.5", 443), "192.168.1.5:443");
        // An explicit port wins over the default.
        assert_eq!(probe_addr("192.168.1.5:1400", 80), "192.168.1.5:1400");
        // Several providers accept a full base URL; the scheme and path must not
        // leak into the connect string, or a healthy device reads as lost.
        assert_eq!(probe_addr("https://192.168.1.5/", 443), "192.168.1.5:443");
        assert_eq!(probe_addr("http://192.168.1.5/api", 80), "192.168.1.5:80");
        assert_eq!(
            probe_addr("http://192.168.1.5:8080/api", 80),
            "192.168.1.5:8080"
        );
        // IPv6 literals need brackets to connect at all.
        assert_eq!(probe_addr("fd00::1", 80), "[fd00::1]:80");
        assert_eq!(probe_addr("[fd00::1]:1400", 80), "[fd00::1]:1400");
    }

    #[tokio::test]
    async fn tcp_reachable_sees_a_listener_and_not_a_dead_port() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let _ = listener.accept().await;
            }
        });
        assert!(tcp_reachable("127.0.0.1", port).await);
        // A scheme-carrying credential probes the same bare host.
        assert!(tcp_reachable(&format!("http://127.0.0.1:{port}/"), 80).await);
        // Nothing listens on 1 (reserved).
        assert!(!tcp_reachable("127.0.0.1", 1).await);
    }
}
