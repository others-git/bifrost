//! Power device API: list strictly-on/off devices (switches, plugs, fans,
//! boolean helpers), read live state, and toggle them.
//!
//! Mirrors the lights and audio APIs: service functions own the behaviour and
//! are shared by the session-authenticated routes here and the Bearer-key
//! routes in `v1`. A power device's only state is `on`, so there is no command
//! vocabulary — writes are a single bool. Reads hit the device live and refresh
//! the cached `last_state`; an unreachable device falls back to the cache with
//! `reachable: false` instead of erroring the whole request.

use crate::AppState;
use crate::api::auth::require_session;
use crate::models::power::{PowerKind, PowerState};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, put},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/devices", get(list_devices_handler))
        .route("/devices/{id}", get(get_device_handler))
        .route("/devices/{id}/state", put(set_device_handler))
        .route("/devices/{id}/enabled", put(set_enabled_handler))
        .route("/devices/{id}/glyph", put(set_glyph_handler))
        .route("/devices/{id}/shadow", put(set_shadow_handler))
        .route("/devices/{id}/room", put(set_room_handler))
}

/// Body for a power write — the whole command is a single bool.
#[derive(Debug, Deserialize)]
pub(crate) struct PowerCommand {
    pub on: bool,
}

// ── Wire shape ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub(crate) struct PowerDeviceRow {
    pub id: String,
    pub provider_id: String,
    /// Provider-native id (e.g. an HA `entity_id`).
    pub device_id: String,
    pub name: String,
    pub kind: PowerKind,
    pub state: PowerState,
    pub last_seen: Option<String>,
    /// Disabled devices keep their room membership but receive no commands and
    /// are hidden from room control.
    pub enabled: bool,
    /// Optional glyph override (name); `None` = derive from `kind`.
    pub glyph: Option<String>,
    /// Normalized hardware identity for cross-provider de-dup; `None` if unknown.
    pub hw_id: Option<String>,
    /// When set, a duplicate of (shadowed by) this device id — hidden from
    /// control and collapsed in the inventory.
    pub shadowed_by: Option<String>,
    /// `true` if the shadow was set automatically by hw_id matching.
    pub shadow_auto: bool,
    /// The room this device is directly assigned to (Devices-page assignment),
    /// or `None`. Room *links* (synced provider groups) aren't reflected here.
    pub room_id: Option<String>,
}

fn kind_str(kind: PowerKind) -> &'static str {
    match kind {
        PowerKind::Switch => "switch",
        PowerKind::Outlet => "outlet",
        PowerKind::Fan => "fan",
        PowerKind::Toggle => "toggle",
        PowerKind::Generic => "generic",
    }
}

fn parse_kind(s: &str) -> PowerKind {
    match s {
        "switch" => PowerKind::Switch,
        "outlet" => PowerKind::Outlet,
        "fan" => PowerKind::Fan,
        "toggle" => PowerKind::Toggle,
        _ => PowerKind::Generic,
    }
}

fn row_to_device(r: sqlx::sqlite::SqliteRow) -> PowerDeviceRow {
    PowerDeviceRow {
        id: r.get("id"),
        provider_id: r.get("provider_id"),
        device_id: r.get("device_id"),
        name: r.get("name"),
        kind: parse_kind(&r.get::<String, _>("kind")),
        state: r
            .get::<Option<String>, _>("last_state")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        last_seen: r.get("last_seen"),
        enabled: r.get::<i64, _>("enabled") != 0,
        glyph: r.get("glyph"),
        hw_id: r.get("hw_id"),
        shadowed_by: r.get("shadowed_by"),
        shadow_auto: r.get::<i64, _>("shadow_auto") != 0,
        room_id: r.get("room_id"),
    }
}

// ── Services (shared with /api/v1) ───────────────────────────────────────────

/// Decrypt credentials and construct a power provider via the registry.
pub(crate) fn build_power_provider(
    state: &AppState,
    provider_type: &str,
    credentials_enc: &str,
) -> anyhow::Result<Box<dyn crate::providers::PowerProvider>> {
    let creds_json = state.decrypt_credentials(credentials_enc)?;
    state.registry.build_power(provider_type, &creds_json)
}

pub(crate) async fn list_all_power_devices(state: &AppState) -> Result<Vec<PowerDeviceRow>, ()> {
    sqlx::query(
        "SELECT id, provider_id, device_id, name, kind, last_state, last_seen, enabled, glyph, hw_id, shadowed_by, shadow_auto,
                (SELECT room_id FROM room_power_devices WHERE power_device_id = power_devices.id LIMIT 1) AS room_id
         FROM power_devices ORDER BY name",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| tracing::error!("db error listing power devices: {e}"))
    .map(|rows| rows.into_iter().map(row_to_device).collect())
}

/// Fetch one device with a live state read. Falls back to the cached state
/// (marked unreachable) when the device doesn't answer; `Ok(None)` = unknown id.
pub(crate) async fn get_power_device_live(
    state: &AppState,
    id: &str,
) -> Result<Option<PowerDeviceRow>, ()> {
    let row = sqlx::query(
        "SELECT pd.id, pd.provider_id, pd.device_id, pd.name, pd.kind, pd.last_state, pd.last_seen,
                pd.enabled, pd.glyph, pd.hw_id, pd.shadowed_by, pd.shadow_auto,
                (SELECT room_id FROM room_power_devices WHERE power_device_id = pd.id LIMIT 1) AS room_id,
                p.provider_type, p.credentials
         FROM power_devices pd JOIN providers p ON pd.provider_id = p.id
         WHERE pd.id = ? AND p.enabled = 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| tracing::error!("db error fetching power device: {e}"))?;

    let Some(row) = row else { return Ok(None) };
    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials: String = row.get("credentials");
    let mut device = row_to_device(row);

    match build_power_provider(state, &provider_type, &credentials) {
        Ok(provider) => match provider.get_state(&device_id).await {
            Ok(fresh) => {
                persist_state(state, &device.id, &fresh).await;
                device.state = fresh;
            }
            Err(e) => {
                tracing::debug!("power device {id} unreachable: {e:#}");
                device.state.reachable = Some(false);
            }
        },
        Err(e) => {
            tracing::error!("failed to build power provider: {e:#}");
            device.state.reachable = Some(false);
        }
    }
    Ok(Some(device))
}

pub(crate) enum SetPowerOutcome {
    Ok,
    NotFound,
    ProviderError,
    Db,
}

pub(crate) async fn apply_power_state(state: &AppState, id: &str, on: bool) -> SetPowerOutcome {
    // A disabled device (or disabled provider) receives no commands.
    let row = sqlx::query(
        "SELECT pd.device_id, p.provider_type, p.credentials
         FROM power_devices pd JOIN providers p ON pd.provider_id = p.id
         WHERE pd.id = ? AND p.enabled = 1 AND pd.enabled = 1 AND pd.shadowed_by IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await;

    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return SetPowerOutcome::NotFound,
        Err(e) => {
            tracing::error!("db error: {e}");
            return SetPowerOutcome::Db;
        }
    };

    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials: String = row.get("credentials");

    let provider = match build_power_provider(state, &provider_type, &credentials) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to build power provider: {e:#}");
            return SetPowerOutcome::Db;
        }
    };

    match provider.set_state(&device_id, on).await {
        Ok(()) => {
            // Reflect the commanded state in the cache; the device reports
            // reachable since the call succeeded.
            persist_state(
                state,
                id,
                &PowerState {
                    on,
                    reachable: Some(true),
                },
            )
            .await;
            SetPowerOutcome::Ok
        }
        Err(e) => {
            tracing::error!("power set_state error for {device_id}: {e:#}");
            SetPowerOutcome::ProviderError
        }
    }
}

async fn persist_state(state: &AppState, id: &str, power: &PowerState) {
    if let Ok(json) = serde_json::to_string(power) {
        let _ = sqlx::query(
            "UPDATE power_devices SET last_state = ?, last_seen = datetime('now') WHERE id = ?",
        )
        .bind(&json)
        .bind(id)
        .execute(&state.db)
        .await;
    }
}

pub(crate) fn set_power_status(outcome: SetPowerOutcome) -> StatusCode {
    match outcome {
        SetPowerOutcome::Ok => StatusCode::NO_CONTENT,
        SetPowerOutcome::NotFound => StatusCode::NOT_FOUND,
        SetPowerOutcome::ProviderError => StatusCode::BAD_GATEWAY,
        SetPowerOutcome::Db => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Discover an integration's power devices and upsert them. Returns the count.
/// Called from the shared `/api/providers/{id}/discover` handler.
pub(crate) async fn discover_power_devices(
    state: &AppState,
    provider_row_id: &str,
    provider_type: &str,
    credentials_enc: &str,
) -> Result<usize, StatusCode> {
    let provider = build_power_provider(state, provider_type, credentials_enc).map_err(|e| {
        tracing::error!("failed to build power provider: {e:#}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let devices = provider.discover().await.map_err(|e| {
        tracing::error!("power discovery error: {e:#}");
        StatusCode::BAD_GATEWAY
    })?;

    for device in &devices {
        let state_json = serde_json::to_string(&device.state).unwrap_or_default();
        let _ = sqlx::query(
            "INSERT INTO power_devices (id, provider_id, device_id, name, kind, last_state, last_seen, hw_id)
             VALUES (?, ?, ?, ?, ?, ?, datetime('now'), ?)
             ON CONFLICT (provider_id, device_id)
             DO UPDATE SET name       = excluded.name,
                           kind       = excluded.kind,
                           last_state = excluded.last_state,
                           last_seen  = excluded.last_seen,
                           hw_id      = excluded.hw_id",
        )
        .bind(device.id.to_string())
        .bind(provider_row_id)
        .bind(&device.provider_id)
        .bind(&device.name)
        .bind(kind_str(device.kind))
        .bind(&state_json)
        .bind(&device.hw_id)
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
    match list_all_power_devices(&state).await {
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
    match get_power_device_live(&state, &id).await {
        Ok(Some(device)) => Json(device).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn set_device_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(cmd): Json<PowerCommand>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    set_power_status(apply_power_state(&state, &id, cmd.on).await).into_response()
}

async fn set_enabled_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetEnabledRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    crate::api::set_device_enabled(&state, "power_devices", &id, req.enabled)
        .await
        .into_response()
}

async fn set_glyph_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetGlyphRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    crate::api::set_device_glyph(&state, "power_devices", &id, req.glyph)
        .await
        .into_response()
}

async fn set_shadow_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetShadowRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    crate::api::dedup::set_device_shadow(&state, "power_devices", &id, req.shadowed_by)
        .await
        .into_response()
}

async fn set_room_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetRoomRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    crate::api::rooms::set_device_room(
        &state,
        "power_devices",
        "room_power_devices",
        "power_device_id",
        &id,
        req.room_id,
    )
    .await
    .into_response()
}
