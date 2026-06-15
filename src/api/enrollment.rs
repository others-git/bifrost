//! Device enrollment — pair a headless device (the wall-tablet voice satellite)
//! without typing a key.
//!
//! Flow: an authenticated dashboard session mints a short-lived, single-use
//! token (`POST /api/enrollment`) and renders it as a pairing **QR**. The device
//! scans the QR and redeems the token (`POST /api/enrollment/redeem`) for a real
//! `bfr_` API key — the same key type the public API and voice seam accept, so
//! it shows up in Settings and can be revoked like any other. The redeem route
//! is unauthenticated *by design* (the device has no credential yet) but gated
//! on a valid, unexpired, unused token, which only an authed session can mint.

use crate::AppState;
use crate::api::apikeys::mint_api_key;
use crate::api::auth::require_session;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;

/// How long a pairing token stays valid: long enough to walk to the tablet and
/// scan, short enough that a leaked QR is quickly useless.
const TOKEN_TTL_SECS: i64 = 300;

/// Mint a high-entropy opaque token (192 bits, hex). Carried in the QR; it's
/// already unguessable and short-lived, so it's stored as plaintext.
fn generate_token() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", post(create_token))
        .route("/redeem", post(redeem_token))
}

#[derive(Serialize)]
struct EnrollmentToken {
    token: String,
    expires_at: String,
    expires_in_secs: i64,
}

/// `POST /api/enrollment` (session) — mint a pairing token. Opportunistically
/// prunes expired/used tokens so the table stays tiny.
async fn create_token(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let _ = sqlx::query(
        "DELETE FROM enrollment_tokens WHERE expires_at < datetime('now') OR used_at IS NOT NULL",
    )
    .execute(&state.db)
    .await;

    let token = generate_token();
    let row = sqlx::query(
        "INSERT INTO enrollment_tokens (token, expires_at)
         VALUES (?, datetime('now', ?))
         RETURNING expires_at",
    )
    .bind(&token)
    .bind(format!("+{TOKEN_TTL_SECS} seconds"))
    .fetch_one(&state.db)
    .await;

    match row {
        Ok(r) => Json(EnrollmentToken {
            token,
            expires_at: r.get("expires_at"),
            expires_in_secs: TOKEN_TTL_SECS,
        })
        .into_response(),
        Err(e) => {
            tracing::error!("db error minting enrollment token: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
struct RedeemRequest {
    token: String,
    /// Names the minted key so it's identifiable in Settings (e.g. the tablet's
    /// label). Defaults to "Paired device" when absent.
    #[serde(default)]
    device_name: Option<String>,
}

/// `POST /api/enrollment/redeem` (no session — the token *is* the proof). Claims
/// a pending token and mints an API key named after the device. The claim is a
/// single atomic UPDATE so a token can never be redeemed twice (race-safe).
async fn redeem_token(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RedeemRequest>,
) -> impl IntoResponse {
    let claimed = sqlx::query(
        "UPDATE enrollment_tokens SET used_at = datetime('now')
         WHERE token = ? AND used_at IS NULL AND expires_at > datetime('now')
         RETURNING token",
    )
    .bind(req.token.trim())
    .fetch_optional(&state.db)
    .await;

    match claimed {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                "invalid, expired, or already-used enrollment token",
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!("db error redeeming enrollment token: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    let name = req
        .device_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Paired device");

    match mint_api_key(&state, name).await {
        Ok(m) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "key": m.key, "prefix": m.prefix, "name": name })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("db error minting key on enrollment: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_unique_and_long() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 48, "24 bytes → 48 hex chars");
        assert_ne!(a, b, "two tokens must differ");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
