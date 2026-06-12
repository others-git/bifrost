//! Client API keys for the public `/api/v1` surface.
//!
//! Keys are high-entropy random strings shown to the user exactly once. Only a
//! SHA-256 hash is stored, so the plaintext key cannot be recovered from the DB.
//! There is no RBAC: any valid key grants full access to lights and rooms.
//!
//! This module owns both the *management* routes (session-authenticated, used
//! by the Settings UI to mint/list/revoke keys) and [`require_api_key`], the
//! Bearer-token check the public API uses.

use crate::AppState;
use crate::api::auth::require_session;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

/// Human-readable prefix so keys are recognisable in logs and the UI.
const KEY_PREFIX: &str = "bfr_";
/// Characters of the key kept for display (`bfr_` + 8 hex chars).
const DISPLAY_PREFIX_LEN: usize = KEY_PREFIX.len() + 8;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_keys).post(create_key))
        .route("/{id}", axum::routing::delete(revoke_key))
}

// ── Key helpers ──────────────────────────────────────────────────────────────

/// Mint a new random key: `bfr_` followed by 64 hex chars (256 bits).
fn generate_key() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{KEY_PREFIX}{}", hex::encode(bytes))
}

/// Deterministic SHA-256 (hex) of a key, for storage and lookup. Keys are
/// high-entropy, so a fast hash is appropriate — there is nothing to brute force.
fn hash_key(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}

/// Read a `Authorization: Bearer <key>` header value.
fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let auth = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    auth.strip_prefix("Bearer ")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Authenticate a public-API request. Returns the matching key id and bumps its
/// `last_used` timestamp; `None` if the header is missing or the key is unknown.
pub async fn require_api_key(state: &Arc<AppState>, headers: &HeaderMap) -> Option<String> {
    let key = extract_bearer(headers)?;
    let hash = hash_key(&key);
    let row = sqlx::query(
        "UPDATE api_keys SET last_used = datetime('now') WHERE key_hash = ? RETURNING id",
    )
    .bind(&hash)
    .fetch_optional(&state.db)
    .await
    .ok()??;
    Some(row.get("id"))
}

// ── Management handlers (session-authenticated) ──────────────────────────────

#[derive(Serialize)]
struct ApiKeyInfo {
    id: String,
    name: String,
    prefix: String,
    created_at: String,
    last_used: Option<String>,
}

async fn list_keys(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    match sqlx::query(
        "SELECT id, name, prefix, created_at, last_used FROM api_keys ORDER BY created_at",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|r| ApiKeyInfo {
                    id: r.get("id"),
                    name: r.get("name"),
                    prefix: r.get("prefix"),
                    created_at: r.get("created_at"),
                    last_used: r.get("last_used"),
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => {
            tracing::error!("db error listing api keys: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
struct CreateKeyRequest {
    name: String,
}

async fn create_key(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateKeyRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if req.name.trim().is_empty() {
        return (StatusCode::UNPROCESSABLE_ENTITY, "key name is required").into_response();
    }

    let id = Uuid::new_v4().to_string();
    let key = generate_key();
    let key_hash = hash_key(&key);
    let prefix = &key[..DISPLAY_PREFIX_LEN];

    if let Err(e) =
        sqlx::query("INSERT INTO api_keys (id, name, key_hash, prefix) VALUES (?, ?, ?, ?)")
            .bind(&id)
            .bind(req.name.trim())
            .bind(&key_hash)
            .bind(prefix)
            .execute(&state.db)
            .await
    {
        tracing::error!("db error creating api key: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // The full key is returned exactly once — it cannot be recovered later.
    (
        StatusCode::CREATED,
        Json(
            serde_json::json!({ "id": id, "name": req.name.trim(), "key": key, "prefix": prefix }),
        ),
    )
        .into_response()
}

async fn revoke_key(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let _ = sqlx::query("DELETE FROM api_keys WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;

    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_keys_are_prefixed_and_unique() {
        let a = generate_key();
        let b = generate_key();
        assert!(a.starts_with(KEY_PREFIX));
        assert_eq!(a.len(), KEY_PREFIX.len() + 64);
        assert_ne!(a, b, "two generated keys must differ");
    }

    #[test]
    fn hash_is_deterministic_and_distinguishes_keys() {
        let key = generate_key();
        assert_eq!(hash_key(&key), hash_key(&key), "hash must be stable");
        assert_ne!(
            hash_key(&key),
            hash_key(&generate_key()),
            "different keys must hash differently"
        );
        // SHA-256 hex is 64 chars and never contains the plaintext key.
        let h = hash_key(&key);
        assert_eq!(h.len(), 64);
        assert!(!h.contains(&key));
    }

    #[test]
    fn extract_bearer_reads_token_and_rejects_garbage() {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, "Bearer bfr_abc123".parse().unwrap());
        assert_eq!(extract_bearer(&h), Some("bfr_abc123".to_string()));

        let mut empty = HeaderMap::new();
        empty.insert(header::AUTHORIZATION, "Bearer ".parse().unwrap());
        assert_eq!(extract_bearer(&empty), None);

        let mut basic = HeaderMap::new();
        basic.insert(header::AUTHORIZATION, "Basic abc".parse().unwrap());
        assert_eq!(extract_bearer(&basic), None);

        assert_eq!(extract_bearer(&HeaderMap::new()), None);
    }

    #[test]
    fn display_prefix_is_recognisable() {
        let key = generate_key();
        let prefix = &key[..DISPLAY_PREFIX_LEN];
        assert!(prefix.starts_with("bfr_"));
        assert_eq!(prefix.len(), DISPLAY_PREFIX_LEN);
    }
}
