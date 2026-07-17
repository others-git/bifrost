//! Smart-TV host relocation — self-healing after a DHCP change.
//!
//! A power outage routinely hands a TV a fresh DHCP lease; the provider's
//! stored credentials still pin the old IP, so the TV silently drops off every
//! surface until someone re-configures it. This slow watch (~30s) probes each
//! enabled smart-TV provider's stored host with a cheap TCP connect; when one
//! is unreachable it re-runs the type's own discovery legs and rebinds the
//! credentials to a candidate that **proves it is the same device**:
//!
//! - a discovery candidate carrying the provider's exact hardware id (the mDNS
//!   `bt` MAC an Android/Google TV advertises), or
//! - for a Bravia, a live ScalarWeb identity read from the candidate host whose
//!   normalized MAC matches ([`smarttv::bravia_identity_matches`]).
//!
//! An unverifiable candidate is never adopted — rebinding to a neighbour's TV
//! would be strictly worse than staying lost. On a match the credentials are
//! rewritten (host only) and the provider manager restarts, so the push channel
//! and demand pollers reattach to the new address immediately. A TV that is
//! genuinely off/unplugged just keeps the watch ticking; the scan burst is
//! bounded (~2s) and only runs while something is actually lost.

use crate::AppState;
use crate::providers::discovery::{DiscoveredDevice, ScanOptions};
use sqlx::Row;
use std::sync::Arc;
use std::time::Duration;

/// Cadence of the watch — slow enough to be invisible, fast enough that a TV
/// coming back from an outage is usable again within a minute.
const RELOCATE_TICK: Duration = Duration::from_secs(30);

/// Budget for the TCP reachability probe of the stored host.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Background loop: spawned once at startup (`lib.rs`).
pub async fn smarttv_relocate_loop(state: Arc<AppState>) {
    let mut ticker = tokio::time::interval(RELOCATE_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let lost = lost_tvs(&state).await;
        if lost.is_empty() {
            continue;
        }
        // One scan serves every lost provider this pass. The discoverer is the
        // same union of legs the add-provider "Scan network" runs.
        let Some(discoverer) = state.registry.discoverer("smarttv") else {
            return; // type has no discoverer — nothing this loop can ever do
        };
        let extra_subnets = crate::api::settings::expanded_subnets(&state).await;
        let timeout = if extra_subnets.is_empty() {
            Duration::from_secs(2)
        } else {
            Duration::from_secs(6)
        };
        let candidates = discoverer
            .scan(&ScanOptions {
                timeout,
                extra_subnets,
            })
            .await
            .unwrap_or_default();
        relocate_with_candidates(&state, lost, &candidates).await;
    }
}

/// A smart-TV provider whose stored host stopped answering.
pub struct LostTv {
    pub provider_id: String,
    pub provider_name: String,
    /// Decrypted credential object (host, brand, auth, …).
    pub creds: serde_json::Map<String, serde_json::Value>,
    pub host: String,
    /// Vendor adapter key (`bravia` when unset, matching `build_vendor`).
    pub brand: String,
    /// The hardware id a candidate must prove (`mac:…`).
    pub expected_hw: String,
}

/// Find enabled smart-TV providers whose stored host fails a TCP probe and
/// which carry a hardware id to verify a replacement against. (Without an
/// id there is nothing safe to match on — those are skipped with a debug log.)
/// Public with [`relocate_with_candidates`] as the relocator's test seam — the
/// background loop is these two plus a real network scan.
pub async fn lost_tvs(state: &Arc<AppState>) -> Vec<LostTv> {
    let rows = sqlx::query(
        "SELECT id, name, credentials FROM providers
         WHERE provider_type = 'smarttv' AND enabled = 1",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut lost = Vec::new();
    for row in rows {
        let id: String = row.get("id");
        let name: String = row.get("name");
        let Some(creds) = state
            .decrypt_credentials(&row.get::<String, _>("credentials"))
            .ok()
            .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
            .and_then(|v| v.as_object().cloned())
        else {
            continue;
        };
        let Some(host) = creds
            .get("host")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|h| !h.trim().is_empty())
        else {
            continue;
        };
        let brand = creds
            .get("brand")
            .and_then(|v| v.as_str())
            .unwrap_or("bravia")
            .to_string();
        let port = if brand == "androidtv" { 6466 } else { 80 };
        if tcp_reachable(&host, port).await {
            continue;
        }
        // The identity to demand of any replacement: the creds' own hw_id
        // (stamped by discovery) or the one on the provider's device rows.
        let expected_hw = match creds
            .get("hw_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        {
            Some(h) => Some(h),
            None => sqlx::query_scalar::<_, String>(
                "SELECT hw_id FROM media_devices
                 WHERE provider_id = ? AND hw_id IS NOT NULL LIMIT 1",
            )
            .bind(&id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten(),
        };
        let Some(expected_hw) = expected_hw else {
            tracing::debug!(
                target: "bifrost::smarttv",
                provider = %id, %host,
                "TV unreachable but no stored hardware id — cannot relocate safely"
            );
            continue;
        };
        tracing::debug!(
            target: "bifrost::smarttv",
            provider = %id, %host, %brand,
            "stored TV host unreachable — scanning for a new address"
        );
        lost.push(LostTv {
            provider_id: id,
            provider_name: name,
            creds,
            host,
            brand,
            expected_hw,
        });
    }
    lost
}

/// `host[:port]` answers a TCP connect within [`PROBE_TIMEOUT`].
async fn tcp_reachable(host: &str, default_port: u16) -> bool {
    let addr = if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:{default_port}")
    };
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, tokio::net::TcpStream::connect(&addr)).await,
        Ok(Ok(_))
    )
}

/// Try to rebind each lost TV onto a scan candidate that proves the same
/// hardware identity. Public as the relocator's test seam — the loop above is
/// this plus a real network scan.
pub async fn relocate_with_candidates(
    state: &Arc<AppState>,
    lost: Vec<LostTv>,
    candidates: &[DiscoveredDevice],
) {
    for tv in lost {
        let mut new_host: Option<String> = None;
        for cand in candidates {
            if cand.host == tv.host {
                continue; // the dead address itself
            }
            // Cheapest proof first: the candidate already carries the hardware
            // id (mDNS `bt` MAC) — leg- and brand-agnostic.
            let cand_hw = cand.credentials.get("hw_id").and_then(|v| v.as_str());
            if cand_hw == Some(tv.expected_hw.as_str()) {
                new_host = Some(cand.host.clone());
                break;
            }
            // Bravia: ask the candidate for its live identity (the stored auth
            // cookie rides along — a different TV rejects or mismatches).
            if tv.brand != "androidtv" && cand_hw.is_none() {
                let auth = tv
                    .creds
                    .get("auth")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                if crate::providers::smarttv::bravia_identity_matches(
                    &cand.host,
                    auth,
                    &tv.expected_hw,
                )
                .await
                {
                    new_host = Some(cand.host.clone());
                    break;
                }
            }
        }
        let Some(new_host) = new_host else { continue };

        let mut creds = tv.creds.clone();
        creds.insert("host".into(), new_host.clone().into());
        let creds_json = serde_json::Value::Object(creds).to_string();
        let Ok(encrypted) = state.encrypt_credentials(&creds_json) else {
            tracing::error!(target: "bifrost::smarttv", provider = %tv.provider_id, "relocate: encryption failed");
            continue;
        };
        if let Err(e) = sqlx::query(
            "UPDATE providers SET credentials = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(&encrypted)
        .bind(&tv.provider_id)
        .execute(&state.db)
        .await
        {
            tracing::error!(target: "bifrost::smarttv", provider = %tv.provider_id, "relocate: db update failed: {e}");
            continue;
        }
        tracing::info!(
            target: "bifrost::smarttv",
            provider = %tv.provider_id, name = %tv.provider_name,
            old_host = %tv.host, %new_host,
            "TV relocated to a new address — restarting provider"
        );
        // Restart the manager so the push channel + pollers reattach NOW.
        {
            let mut connections = state.connections.lock().await;
            connections.stop(&tv.provider_id);
            crate::start_manager_for(
                &mut connections,
                state,
                &tv.provider_id,
                "smarttv",
                &creds_json,
            );
        }
    }
}
