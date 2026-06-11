use crate::AppState;
use crate::api::auth::require_session;
use crate::api::lights::build_provider;
use crate::connection::ConnectionStatus;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_providers).post(add_provider))
        .route("/types", get(list_types))
        .route("/{id}", delete(remove_provider))
        .route("/{id}/status", get(provider_status))
        .route("/{id}/discover", post(discover))
}

// ── List available provider types (for the setup UI) ───────────────────────

async fn list_types(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(state.registry.all_types()).into_response()
}

// ── Configured provider instances ──────────────────────────────────────────

#[derive(Serialize)]
struct ProviderRow {
    id: String,
    provider_type: String,
    name: String,
    enabled: bool,
    created_at: String,
}

async fn list_providers(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    match sqlx::query(
        "SELECT id, provider_type, name, enabled, created_at FROM providers ORDER BY created_at",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|r: sqlx::sqlite::SqliteRow| ProviderRow {
                    id: r.get("id"),
                    provider_type: r.get("provider_type"),
                    name: r.get("name"),
                    enabled: r.get::<i64, _>("enabled") != 0,
                    created_at: r.get("created_at"),
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => {
            tracing::error!("db error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct AddProviderRequest {
    name: String,
    provider_type: String,
    /// Shape must match the schema returned by `GET /api/providers/types`.
    credentials: serde_json::Value,
}

async fn add_provider(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<AddProviderRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    if !state.registry.is_known(&req.provider_type) {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "unknown provider_type '{}'; see GET /api/providers/types",
                req.provider_type
            ),
        )
            .into_response();
    }

    let creds_json = req.credentials.to_string();

    // Smoke-test: try building the provider now so bad credentials fail fast.
    if let Err(e) = state.registry.build(&req.provider_type, &creds_json) {
        return (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response();
    }

    let encrypted = match state.encrypt_credentials(&creds_json) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("encryption error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let id = Uuid::new_v4().to_string();
    match sqlx::query(
        "INSERT INTO providers (id, provider_type, name, credentials) VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&req.provider_type)
    .bind(&req.name)
    .bind(&encrypted)
    .execute(&state.db)
    .await
    {
        Ok(_) => {
            // For Hue providers, start a connection manager immediately.
            if req.provider_type == "hue"
                && let Ok(provider) =
                    crate::providers::hue::HueProvider::from_credentials(&creds_json)
            {
                state
                    .connections
                    .lock()
                    .await
                    .start(id.clone(), provider, state.db.clone());
            }
            (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response()
        }
        Err(e) => {
            tracing::error!("db error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn remove_provider(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    state.connections.lock().await.stop(&id);

    let _ = sqlx::query("DELETE FROM providers WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;

    StatusCode::NO_CONTENT.into_response()
}

async fn provider_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let state_lock = state.connections.lock().await.get_state_lock(&id);

    match state_lock {
        None => Json(serde_json::json!({ "state": "not_managed" })).into_response(),
        Some(lock) => {
            let cs = lock.read().await;
            Json(ConnectionStatus::from_state(&cs)).into_response()
        }
    }
}

// ── Discovery ───────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct DiscoverResponse {
    discovered: usize,
}

async fn discover(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let row = sqlx::query(
        "SELECT provider_type, credentials FROM providers WHERE id = ? AND enabled = 1",
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await;

    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let provider_type: String = row.get("provider_type");
    let credentials_enc: String = row.get("credentials");

    let provider = match build_provider(&state, &provider_type, &credentials_enc) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to build provider: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let lights = match provider.discover().await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("discovery error: {e:#}");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };

    for light in &lights {
        let light_id = light.id.to_string();
        let caps = serde_json::to_string(&light.capabilities).unwrap_or_default();
        let state_json = serde_json::to_string(&light.state).unwrap_or_default();
        let _ = sqlx::query(
            "INSERT INTO lights (id, provider_id, device_id, name, capabilities, last_state, last_seen)
             VALUES (?, ?, ?, ?, ?, ?, datetime('now'))
             ON CONFLICT (provider_id, device_id)
             DO UPDATE SET name        = excluded.name,
                           capabilities = excluded.capabilities,
                           last_state  = excluded.last_state,
                           last_seen   = excluded.last_seen",
        )
        .bind(&light_id)
        .bind(&id)
        .bind(&light.provider_id)
        .bind(&light.name)
        .bind(&caps)
        .bind(&state_json)
        .execute(&state.db)
        .await;
    }

    Json(DiscoverResponse {
        discovered: lights.len(),
    })
    .into_response()
}
