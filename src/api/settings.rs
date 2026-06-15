//! App-wide settings (the singleton `config` row), beyond first-run auth.
//!
//! Currently just the **Expanded-LAN scan list**: extra private /24 subnets that
//! network auto-detect should sweep in addition to the container's own subnet.
//! This is how auto-detect reaches devices on a different LAN when Bifrost runs
//! bridged (without host networking) — unicast TCP routes across subnets even
//! though broadcast/multicast don't. Only the HTTP-sweep providers (WLED,
//! Tasmota, Shelly) use it; SSDP/eISCP discovery is broadcast and can't cross a
//! subnet regardless.

use crate::AppState;
use crate::api::auth::Session;
use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::net::Ipv4Addr;
use std::sync::Arc;

/// Hard cap on configured subnets so a scan stays bounded (8 × 254 ≈ 2k hosts).
const MAX_SUBNETS: usize = 8;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/", get(get_settings).put(put_settings))
}

#[derive(Serialize, Deserialize)]
struct Settings {
    /// Extra private /24 subnets to scan, as `a.b.c.0/24` strings.
    expanded_lan_scan: Vec<String>,
}

/// Parse and validate one configured subnet, returning its /24 base address.
/// Accepts `192.168.1.0/24`, `192.168.1.0`, or `192.168.1`. Rejects anything
/// outside RFC1918 private space, enforcing "never cross a private boundary".
fn parse_private_subnet(raw: &str) -> Result<Ipv4Addr, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("empty subnet".into());
    }
    // Drop any /prefix; we always treat entries as /24.
    let addr_part = s.split('/').next().unwrap_or(s);
    let mut octets = [0u8; 4];
    let pieces: Vec<&str> = addr_part.split('.').collect();
    if pieces.len() < 3 || pieces.len() > 4 {
        return Err(format!("'{raw}' is not an IPv4 subnet"));
    }
    for (i, p) in pieces.iter().enumerate() {
        octets[i] = p
            .parse::<u8>()
            .map_err(|_| format!("'{raw}' has a bad octet"))?;
    }
    // /24 base: zero the host octet.
    octets[3] = 0;
    let base = Ipv4Addr::from(octets);
    if !base.is_private() {
        return Err(format!(
            "'{raw}' is not a private network (only 10/8, 172.16/12, 192.168/16 allowed)"
        ));
    }
    Ok(base)
}

/// The configured Expanded-LAN subnets as /24 base addresses, for the scanner.
/// Invalid stored entries are skipped (validation happens on write).
pub(crate) async fn expanded_subnets(state: &AppState) -> Vec<Ipv4Addr> {
    let raw: String = sqlx::query("SELECT scan_subnets FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .map(|r| r.get::<String, _>("scan_subnets"))
        .unwrap_or_default();
    raw.split(',')
        .filter(|s| !s.trim().is_empty())
        .filter_map(|s| parse_private_subnet(s).ok())
        .take(MAX_SUBNETS)
        .collect()
}

async fn get_settings(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    let raw: String = sqlx::query("SELECT scan_subnets FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .map(|r| r.get::<String, _>("scan_subnets"))
        .unwrap_or_default();
    let list = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    Json(Settings {
        expanded_lan_scan: list,
    })
    .into_response()
}

async fn put_settings(
    State(state): State<Arc<AppState>>,
    _: Session,
    Json(req): Json<Settings>,
) -> impl IntoResponse {
    if req.expanded_lan_scan.len() > MAX_SUBNETS {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("at most {MAX_SUBNETS} subnets allowed"),
        )
            .into_response();
    }

    // Validate + normalise every entry to a private /24 base.
    let mut normalised = Vec::new();
    for entry in &req.expanded_lan_scan {
        match parse_private_subnet(entry) {
            Ok(base) => normalised.push(format!("{base}/24")),
            Err(e) => return (StatusCode::UNPROCESSABLE_ENTITY, e).into_response(),
        }
    }
    normalised.dedup();
    let stored = normalised.join(",");

    let res = sqlx::query("UPDATE config SET scan_subnets = ? WHERE id = 1")
        .bind(&stored)
        .execute(&state.db)
        .await;
    match res {
        Ok(_) => Json(Settings {
            expanded_lan_scan: normalised,
        })
        .into_response(),
        Err(e) => {
            tracing::error!("db error saving settings: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_private_subnets_to_24_base() {
        assert_eq!(
            parse_private_subnet("192.168.1.0/24").unwrap(),
            Ipv4Addr::new(192, 168, 1, 0)
        );
        // Host octet is zeroed to the /24 base.
        assert_eq!(
            parse_private_subnet("10.0.5.37").unwrap(),
            Ipv4Addr::new(10, 0, 5, 0)
        );
        // Three-octet shorthand.
        assert_eq!(
            parse_private_subnet("172.16.4").unwrap(),
            Ipv4Addr::new(172, 16, 4, 0)
        );
    }

    #[test]
    fn rejects_public_and_malformed() {
        assert!(parse_private_subnet("8.8.8.0/24").is_err(), "public");
        assert!(
            parse_private_subnet("172.32.0.0").is_err(),
            "outside 172.16/12"
        );
        assert!(parse_private_subnet("not.an.ip").is_err());
        assert!(parse_private_subnet("").is_err());
        assert!(parse_private_subnet("999.1.1.1").is_err());
    }
}
