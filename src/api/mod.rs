pub mod ai_endpoints;
pub mod apikeys;
pub mod auth;
pub mod dedup;
pub mod dev;
pub mod enrollment;
pub mod events;
pub mod kiosk_update;
pub mod kiosks;
pub mod lights;
pub mod mcp;
pub mod media;
pub mod plans;
pub mod power;
pub mod providers;
pub mod remote;
pub mod rooms;
pub mod scenes;
pub mod settings;
pub mod setup;
pub mod v1;
pub mod voice;

use crate::AppState;
use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;

/// Body for enabling/disabling a device.
#[derive(Deserialize)]
pub(crate) struct SetEnabledRequest {
    pub enabled: bool,
}

/// Body for overriding a device's glyph (`null` clears it → use the type default).
#[derive(Deserialize)]
pub(crate) struct SetGlyphRequest {
    pub glyph: Option<String>,
}

/// Body for manually linking a device as a duplicate of another (`null` clears
/// the link → device becomes visible again).
#[derive(Deserialize)]
pub(crate) struct SetShadowRequest {
    pub shadowed_by: Option<String>,
}

/// Body for assigning a device to a room from the device side (`null` removes it
/// from its room).
#[derive(Deserialize)]
pub(crate) struct SetRoomRequest {
    pub room_id: Option<String>,
}

/// Body for binding a source media device to a receiver (M22). `receiver_id`
/// `null` clears the binding; `receiver_source` is the receiver input to select
/// when the source becomes active (`null` = leave the input alone).
#[derive(Deserialize)]
pub(crate) struct SetReceiverRequest {
    pub receiver_id: Option<String>,
    #[serde(default)]
    pub receiver_source: Option<String>,
}

/// M26 composite: merge this media entity into `primary_id` as its companion, or
/// unmerge with `null`.
#[derive(Deserialize)]
pub(crate) struct SetCompanionRequest {
    pub primary_id: Option<String>,
}

/// Set (or clear) a device row's glyph override. `table` is a fixed per-domain
/// identifier, so the formatted SQL is injection-free.
pub(crate) async fn set_device_glyph(
    state: &AppState,
    table: &str,
    id: &str,
    glyph: Option<String>,
) -> StatusCode {
    match sqlx::query(&format!("UPDATE {table} SET glyph = ? WHERE id = ?"))
        .bind(glyph)
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT,
        Ok(_) => StatusCode::NOT_FOUND,
        Err(e) => {
            tracing::error!("db error setting {table} glyph: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// Flip a device row's `enabled` flag. A disabled device is still tracked and
/// keeps its room membership, but the control/membership queries skip it.
/// `table` is a fixed identifier per domain (`lights` / `media_devices` /
/// `power_devices`), so the formatted SQL is injection-free.
pub(crate) async fn set_device_enabled(
    state: &AppState,
    table: &str,
    id: &str,
    enabled: bool,
) -> StatusCode {
    match sqlx::query(&format!("UPDATE {table} SET enabled = ? WHERE id = ?"))
        .bind(i64::from(enabled))
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT,
        Ok(_) => StatusCode::NOT_FOUND,
        Err(e) => {
            tracing::error!("db error setting {table} enabled: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .nest("/ai-endpoints", ai_endpoints::router())
        .nest("/api-keys", apikeys::router())
        .nest("/media", media::router())
        // Deprecated alias: the media domain was renamed from "audio"; kept so
        // existing clients on /api/audio/* keep working for a release.
        .nest("/audio", media::router())
        .nest("/auth", auth::router())
        .nest("/dev", dev::router())
        .nest("/enrollment", enrollment::router())
        .nest("/events", events::router())
        .nest("/kiosks", kiosks::router())
        .nest("/lights", lights::router())
        .nest("/plans", plans::router())
        .nest("/power", power::router())
        .nest("/provider-groups", rooms::provider_groups_router())
        .nest("/providers", providers::router())
        .nest("/remote", remote::router())
        .nest("/rooms", rooms::router())
        .nest("/scenes", scenes::router())
        .nest("/settings", settings::router())
        .nest("/setup", setup::router())
        .nest("/v1", v1::router())
        .nest("/voice", voice::router())
        .route("/health", get(health))
        .route("/instance", get(instance))
}

#[derive(Serialize)]
struct InstanceResponse {
    /// Random per-process id; changes on every server restart/redeploy.
    instance_id: String,
    version: &'static str,
}

/// Cheap, unauthenticated liveness/build nonce for clients to poll. Unlike
/// [`health`] it touches no DB or connection locks, so it's safe to hit often
/// (the kiosk polls it to self-reload after a redeploy).
async fn instance(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(InstanceResponse {
        instance_id: state.instance_id.clone(),
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Serialize)]
struct HealthResponse {
    ok: bool,
    version: &'static str,
    uptime_secs: u64,
    providers: Vec<ProviderHealth>,
}

#[derive(Serialize)]
struct ProviderHealth {
    id: String,
    name: String,
    state: String,
}

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rows = sqlx::query(
        "SELECT id, name, provider_type FROM providers WHERE enabled = 1 ORDER BY created_at",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let connections = state.connections.lock().await;
    let mut providers = Vec::with_capacity(rows.len());

    for row in &rows {
        let id: String = row.get("id");
        let name: String = row.get("name");
        let provider_type: String = row.get("provider_type");

        // Light providers run an SSE/polling manager. On-demand media providers
        // (Sonos) have none — they're read live per request, so report "ready".
        let conn_state = if let Some(lock) = connections.get_state_lock(&id) {
            lock.read().await.label().to_string()
        } else if state.registry.is_known_media(&provider_type) {
            "ready".to_string()
        } else {
            "not_started".to_string()
        };

        providers.push(ProviderHealth {
            id,
            name,
            state: conn_state,
        });
    }

    Json(HealthResponse {
        ok: true,
        version: env!("CARGO_PKG_VERSION"),
        uptime_secs: state.started_at.elapsed().as_secs(),
        providers,
    })
}
