use crate::AppState;
use crate::api::auth::require_session;
use crate::models::LightState;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
};
use serde::Serialize;
use sqlx::Row;
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_lights))
        .route("/{id}", get(get_light).put(set_light_state))
}

#[derive(Serialize)]
pub(crate) struct LightRow {
    id: String,
    provider_id: String,
    device_id: String,
    name: String,
    capabilities: serde_json::Value,
    last_state: Option<serde_json::Value>,
    last_seen: Option<String>,
}

fn row_to_light(r: sqlx::sqlite::SqliteRow) -> LightRow {
    LightRow {
        id: r.get("id"),
        provider_id: r.get("provider_id"),
        device_id: r.get("device_id"),
        name: r.get("name"),
        capabilities: r
            .get::<Option<String>, _>("capabilities")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        last_state: r
            .get::<Option<String>, _>("last_state")
            .and_then(|s| serde_json::from_str(&s).ok()),
        last_seen: r.get("last_seen"),
    }
}

// ── Light services (reused by the session UI API and the public /v1 API) ─────

pub(crate) async fn list_all_lights(state: &AppState) -> Result<Vec<LightRow>, ()> {
    sqlx::query(
        "SELECT id, provider_id, device_id, name, capabilities, last_state, last_seen
         FROM lights ORDER BY name",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| tracing::error!("db error listing lights: {e}"))
    .map(|rows| rows.into_iter().map(row_to_light).collect())
}

pub(crate) async fn get_light_by_id(state: &AppState, id: &str) -> Result<Option<LightRow>, ()> {
    sqlx::query(
        "SELECT id, provider_id, device_id, name, capabilities, last_state, last_seen
         FROM lights WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| tracing::error!("db error fetching light: {e}"))
    .map(|opt| opt.map(row_to_light))
}

/// Outcome of driving a single light's state, mapped to a status by each caller.
pub(crate) enum SetLightOutcome {
    Ok,
    NotFound,
    ProviderError,
    Db,
}

/// Send `new_state` to the light's provider and cache it as `last_state`.
pub(crate) async fn apply_light_state(
    state: &AppState,
    id: &str,
    new_state: &LightState,
) -> SetLightOutcome {
    let row = sqlx::query(
        "SELECT l.device_id, p.provider_type, p.credentials
         FROM lights l JOIN providers p ON l.provider_id = p.id
         WHERE l.id = ? AND p.enabled = 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await;

    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return SetLightOutcome::NotFound,
        Err(e) => {
            tracing::error!("db error: {e}");
            return SetLightOutcome::Db;
        }
    };

    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials_enc: String = row.get("credentials");

    let provider = match build_provider(state, &provider_type, &credentials_enc) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to build provider: {e}");
            return SetLightOutcome::Db;
        }
    };

    match provider.set_state(&device_id, new_state).await {
        Ok(()) => {
            if let Ok(state_json) = serde_json::to_string(new_state) {
                let _ = sqlx::query(
                    "UPDATE lights SET last_state = ?, last_seen = datetime('now') WHERE id = ?",
                )
                .bind(&state_json)
                .bind(id)
                .execute(&state.db)
                .await;
            }
            SetLightOutcome::Ok
        }
        Err(e) => {
            tracing::error!("provider set_state error: {e:#}");
            SetLightOutcome::ProviderError
        }
    }
}

/// Map a [`SetLightOutcome`] to the HTTP status both APIs return.
pub(crate) fn set_light_status(outcome: SetLightOutcome) -> StatusCode {
    match outcome {
        SetLightOutcome::Ok => StatusCode::NO_CONTENT,
        SetLightOutcome::NotFound => StatusCode::NOT_FOUND,
        SetLightOutcome::ProviderError => StatusCode::BAD_GATEWAY,
        SetLightOutcome::Db => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

// ── Handlers (session-authenticated; thin wrappers over the services) ────────

async fn list_lights(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match list_all_lights(&state).await {
        Ok(lights) => Json(lights).into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn get_light(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match get_light_by_id(&state, &id).await {
        Ok(Some(light)) => Json(light).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn set_light_state(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(new_state): Json<LightState>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    set_light_status(apply_light_state(&state, &id, &new_state).await).into_response()
}

/// Decrypt credentials and construct a provider via the registry.
pub(crate) fn build_provider(
    state: &AppState,
    provider_type: &str,
    credentials_enc: &str,
) -> anyhow::Result<Box<dyn crate::providers::LightProvider>> {
    let creds_json = state.decrypt_credentials(credentials_enc)?;
    state.registry.build(provider_type, &creds_json)
}
