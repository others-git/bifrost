//! Audio device API: list devices, read live state, send commands.
//!
//! Mirrors the lights API split: service functions own the behaviour and are
//! shared by the session-authenticated routes here and the Bearer-key routes
//! in `v1`. Reads hit the device live (LAN round trips are cheap) and refresh
//! the cached `last_state`; an unreachable device falls back to the cache with
//! `reachable: false` instead of erroring the whole request.

use crate::AppState;
use crate::api::auth::require_session;
use crate::models::audio::{AudioCapabilities, AudioCommand, AudioState};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, put},
};
use serde::Serialize;
use sqlx::Row;
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/devices", get(list_devices_handler))
        .route("/devices/{id}", get(get_device_handler))
        .route("/devices/{id}/state", put(set_device_handler))
}

// ── Wire shape ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub(crate) struct AudioDeviceRow {
    pub id: String,
    pub provider_id: String,
    /// Provider-native id (e.g. `main`) — matches `audio_state` push events.
    pub device_id: String,
    pub name: String,
    pub kind: String,
    pub capabilities: AudioCapabilities,
    pub state: AudioState,
    pub last_seen: Option<String>,
}

fn row_to_device(r: sqlx::sqlite::SqliteRow) -> AudioDeviceRow {
    AudioDeviceRow {
        id: r.get("id"),
        provider_id: r.get("provider_id"),
        device_id: r.get("device_id"),
        name: r.get("name"),
        kind: r.get("kind"),
        capabilities: serde_json::from_str(&r.get::<String, _>("capabilities")).unwrap_or_default(),
        state: r
            .get::<Option<String>, _>("last_state")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        last_seen: r.get("last_seen"),
    }
}

// ── Services (shared with /api/v1) ───────────────────────────────────────────

pub(crate) fn build_audio_provider(
    state: &AppState,
    provider_type: &str,
    credentials_enc: &str,
) -> anyhow::Result<Box<dyn crate::providers::AudioProvider>> {
    let creds_json = state.decrypt_credentials(credentials_enc)?;
    state.registry.build_audio(provider_type, &creds_json)
}

pub(crate) async fn list_all_devices(state: &AppState) -> Result<Vec<AudioDeviceRow>, ()> {
    sqlx::query(
        "SELECT id, provider_id, device_id, name, kind, capabilities, last_state, last_seen
         FROM audio_devices ORDER BY name",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| tracing::error!("db error listing audio devices: {e}"))
    .map(|rows| rows.into_iter().map(row_to_device).collect())
}

/// Fetch one device with a live state read. Falls back to the cached state
/// (marked unreachable) when the device doesn't answer; `Ok(None)` = unknown id.
pub(crate) async fn get_device_live(
    state: &AppState,
    id: &str,
) -> Result<Option<AudioDeviceRow>, ()> {
    let row = sqlx::query(
        "SELECT a.id, a.provider_id, a.device_id, a.name, a.kind, a.capabilities,
                a.last_state, a.last_seen, p.provider_type, p.credentials
         FROM audio_devices a JOIN providers p ON a.provider_id = p.id
         WHERE a.id = ? AND p.enabled = 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| tracing::error!("db error fetching audio device: {e}"))?;

    let Some(row) = row else { return Ok(None) };
    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials: String = row.get("credentials");
    let mut device = row_to_device(row);

    match build_audio_provider(state, &provider_type, &credentials) {
        Ok(provider) => match provider.get_state(&device_id).await {
            Ok(fresh) => {
                let state_json = serde_json::to_string(&fresh).unwrap_or_default();
                let _ = sqlx::query(
                    "UPDATE audio_devices SET last_state = ?, last_seen = datetime('now') WHERE id = ?",
                )
                .bind(&state_json)
                .bind(&device.id)
                .execute(&state.db)
                .await;
                device.state = fresh;
            }
            Err(e) => {
                tracing::debug!("audio device {id} unreachable: {e:#}");
                device.state.reachable = Some(false);
            }
        },
        Err(e) => {
            tracing::error!("failed to build audio provider: {e:#}");
            device.state.reachable = Some(false);
        }
    }
    Ok(Some(device))
}

pub(crate) enum SetAudioOutcome {
    Ok,
    NotFound,
    BadCommand(String),
    ProviderError,
    Db,
}

pub(crate) async fn apply_audio_command(
    state: &AppState,
    id: &str,
    cmd: &AudioCommand,
) -> SetAudioOutcome {
    let row = sqlx::query(
        "SELECT a.device_id, p.provider_type, p.credentials
         FROM audio_devices a JOIN providers p ON a.provider_id = p.id
         WHERE a.id = ? AND p.enabled = 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await;

    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return SetAudioOutcome::NotFound,
        Err(e) => {
            tracing::error!("db error: {e}");
            return SetAudioOutcome::Db;
        }
    };

    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials: String = row.get("credentials");

    let provider = match build_audio_provider(state, &provider_type, &credentials) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to build audio provider: {e:#}");
            return SetAudioOutcome::Db;
        }
    };

    match provider.set_state(&device_id, cmd).await {
        Ok(()) => SetAudioOutcome::Ok,
        // Validation failures (unknown source/zone) are the caller's mistake,
        // not a gateway fault; connection problems stay 502s.
        Err(e) if e.to_string().contains("unknown") || e.to_string().contains("not supported") => {
            SetAudioOutcome::BadCommand(e.to_string())
        }
        Err(e) => {
            tracing::error!("audio set_state error: {e:#}");
            SetAudioOutcome::ProviderError
        }
    }
}

pub(crate) fn set_audio_status(outcome: SetAudioOutcome) -> axum::response::Response {
    match outcome {
        SetAudioOutcome::Ok => StatusCode::NO_CONTENT.into_response(),
        SetAudioOutcome::NotFound => StatusCode::NOT_FOUND.into_response(),
        SetAudioOutcome::BadCommand(m) => (StatusCode::UNPROCESSABLE_ENTITY, m).into_response(),
        SetAudioOutcome::ProviderError => StatusCode::BAD_GATEWAY.into_response(),
        SetAudioOutcome::Db => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Discover an audio provider's devices and upsert them. Returns the count.
/// Called from the shared `/api/providers/{id}/discover` handler.
pub(crate) async fn discover_audio_devices(
    state: &AppState,
    provider_row_id: &str,
    provider_type: &str,
    credentials_enc: &str,
) -> Result<usize, StatusCode> {
    let provider = build_audio_provider(state, provider_type, credentials_enc).map_err(|e| {
        tracing::error!("failed to build audio provider: {e:#}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let devices = provider.discover().await.map_err(|e| {
        tracing::error!("audio discovery error: {e:#}");
        StatusCode::BAD_GATEWAY
    })?;

    for device in &devices {
        let caps = serde_json::to_string(&device.capabilities).unwrap_or_default();
        let state_json = serde_json::to_string(&device.state).unwrap_or_default();
        let kind = match device.kind {
            crate::models::audio::AudioDeviceKind::Receiver => "receiver",
            crate::models::audio::AudioDeviceKind::Speaker => "speaker",
            crate::models::audio::AudioDeviceKind::Zone => "zone",
        };
        let _ = sqlx::query(
            "INSERT INTO audio_devices (id, provider_id, device_id, name, kind, capabilities, last_state, last_seen)
             VALUES (?, ?, ?, ?, ?, ?, ?, datetime('now'))
             ON CONFLICT (provider_id, device_id)
             DO UPDATE SET name         = excluded.name,
                           kind         = excluded.kind,
                           capabilities = excluded.capabilities,
                           last_state   = excluded.last_state,
                           last_seen    = excluded.last_seen",
        )
        .bind(device.id.to_string())
        .bind(provider_row_id)
        .bind(&device.provider_id)
        .bind(&device.name)
        .bind(kind)
        .bind(&caps)
        .bind(&state_json)
        .execute(&state.db)
        .await;
    }
    Ok(devices.len())
}

// ── Handlers (session-authenticated) ─────────────────────────────────────────

async fn list_devices_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match list_all_devices(&state).await {
        Ok(devices) => Json(devices).into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn get_device_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match get_device_live(&state, &id).await {
        Ok(Some(device)) => Json(device).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn set_device_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(cmd): Json<AudioCommand>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    set_audio_status(apply_audio_command(&state, &id, &cmd).await)
}
