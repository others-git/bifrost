//! Sensor device API: list read-only sensors (motion, occupancy, contact,
//! illuminance, temperature, humidity) and read their live state.
//!
//! The simplest device API: sensors have **no writes**, so there is no
//! set-state route — only reads, discovery, and the shared inventory operations
//! (enable, glyph, name, shadow, room assignment). Service functions own the
//! behaviour and are shared by the session routes here and the Bearer-key routes
//! in `v1`. Reads hit the device live and refresh the cached `last_state`; an
//! unreachable sensor falls back to the cache with `reachable: false` rather than
//! erroring the whole request. Live freshness normally arrives via the provider's
//! push channel (Hue SSE / HA WebSocket), not this poll.

use crate::AppState;
use crate::api::auth::Session;
use crate::models::sensor::{SensorKind, SensorState};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::Serialize;
use sqlx::Row;
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/devices", get(list_devices_handler))
        .route("/devices/{id}", get(get_device_handler))
        .merge(crate::api::inventory_router(
            "/devices",
            "sensor_devices",
            "room_sensor_devices",
            "sensor_device_id",
        ))
}

// ── Wire shape ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub(crate) struct SensorDeviceRow {
    pub id: String,
    pub provider_id: String,
    /// Provider-native id (e.g. an HA `entity_id`, a Hue resource id).
    pub device_id: String,
    pub name: String,
    pub kind: SensorKind,
    pub state: SensorState,
    /// Reading unit (°C, lx, %) when the provider reports one.
    pub unit: Option<String>,
    pub last_seen: Option<String>,
    /// Disabled sensors keep their room membership but are excluded from room
    /// occupancy aggregation and hidden from control surfaces.
    pub enabled: bool,
    /// Optional glyph override (name); `None` = derive from `kind`.
    pub glyph: Option<String>,
    /// Normalized hardware identity for cross-provider de-dup; `None` if unknown.
    pub hw_id: Option<String>,
    /// When set, a duplicate of (shadowed by) this device id — hidden from
    /// aggregation and collapsed in the inventory.
    pub shadowed_by: Option<String>,
    /// `true` if the shadow was set automatically by hw_id matching.
    pub shadow_auto: bool,
    /// The room this device is directly assigned to (Devices-page assignment),
    /// or `None`. Room *links* (synced provider groups) aren't reflected here.
    pub room_id: Option<String>,
    /// The room this device belongs to **via a synced provider-group link**, when
    /// it has no direct assignment.
    pub inherited_room_id: Option<String>,
}

pub(crate) fn kind_str(kind: SensorKind) -> &'static str {
    match kind {
        SensorKind::Motion => "motion",
        SensorKind::Occupancy => "occupancy",
        SensorKind::Contact => "contact",
        SensorKind::Illuminance => "illuminance",
        SensorKind::Temperature => "temperature",
        SensorKind::Humidity => "humidity",
        SensorKind::Generic => "generic",
    }
}

pub(crate) fn parse_kind(s: &str) -> SensorKind {
    match s {
        "motion" => SensorKind::Motion,
        "occupancy" => SensorKind::Occupancy,
        "contact" => SensorKind::Contact,
        "illuminance" => SensorKind::Illuminance,
        "temperature" => SensorKind::Temperature,
        "humidity" => SensorKind::Humidity,
        _ => SensorKind::Generic,
    }
}

fn row_to_device(r: sqlx::sqlite::SqliteRow) -> SensorDeviceRow {
    SensorDeviceRow {
        id: r.get("id"),
        provider_id: r.get("provider_id"),
        device_id: r.get("device_id"),
        name: r.get("name"),
        kind: parse_kind(&r.get::<String, _>("kind")),
        state: r
            .get::<Option<String>, _>("last_state")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        unit: r.get("unit"),
        last_seen: r.get("last_seen"),
        enabled: r.get::<i64, _>("enabled") != 0,
        glyph: r.get("glyph"),
        hw_id: r.get("hw_id"),
        shadowed_by: r.get("shadowed_by"),
        shadow_auto: r.get::<i64, _>("shadow_auto") != 0,
        room_id: r.get("room_id"),
        inherited_room_id: r.try_get("inherited_room_id").ok().flatten(),
    }
}

// ── Services (shared with /api/v1) ───────────────────────────────────────────

/// Decrypt credentials and construct a sensor provider via the registry.
pub(crate) fn build_sensor_provider(
    state: &AppState,
    provider_type: &str,
    credentials_enc: &str,
) -> anyhow::Result<Box<dyn crate::providers::SensorProvider>> {
    let creds_json = state.decrypt_credentials(credentials_enc)?;
    state.registry.build_sensor(provider_type, &creds_json)
}

pub(crate) async fn list_all_sensor_devices(state: &AppState) -> Result<Vec<SensorDeviceRow>, ()> {
    sqlx::query(
        "SELECT id, provider_id, device_id, name, kind, unit, last_state, last_seen, enabled, glyph, hw_id, shadowed_by, shadow_auto,
                (SELECT room_id FROM room_sensor_devices WHERE sensor_device_id = sensor_devices.id LIMIT 1) AS room_id,
                (SELECT rl.room_id FROM room_links rl
                   JOIN provider_group_sensor_devices pgs ON pgs.provider_group_id = rl.provider_group_id
                   WHERE pgs.sensor_device_id = sensor_devices.id LIMIT 1) AS inherited_room_id
         FROM sensor_devices ORDER BY name",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| tracing::error!("db error listing sensor devices: {e}"))
    .map(|rows| rows.into_iter().map(row_to_device).collect())
}

/// Fetch one sensor with a live state read. Falls back to the cached state
/// (marked unreachable) when the device doesn't answer; `Ok(None)` = unknown id.
pub(crate) async fn get_sensor_device_live(
    state: &AppState,
    id: &str,
) -> Result<Option<SensorDeviceRow>, ()> {
    let row = sqlx::query(
        "SELECT sd.id, sd.provider_id, sd.device_id, sd.name, sd.kind, sd.unit, sd.last_state, sd.last_seen,
                sd.enabled, sd.glyph, sd.hw_id, sd.shadowed_by, sd.shadow_auto,
                (SELECT room_id FROM room_sensor_devices WHERE sensor_device_id = sd.id LIMIT 1) AS room_id,
                (SELECT rl.room_id FROM room_links rl
                   JOIN provider_group_sensor_devices pgs ON pgs.provider_group_id = rl.provider_group_id
                   WHERE pgs.sensor_device_id = sd.id LIMIT 1) AS inherited_room_id,
                p.provider_type, p.credentials
         FROM sensor_devices sd JOIN providers p ON sd.provider_id = p.id
         WHERE sd.id = ? AND p.enabled = 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| tracing::error!("db error fetching sensor device: {e}"))?;

    let Some(row) = row else { return Ok(None) };
    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials: String = row.get("credentials");
    let mut device = row_to_device(row);

    match build_sensor_provider(state, &provider_type, &credentials) {
        Ok(provider) => match provider.get_state(&device_id).await {
            Ok(fresh) => {
                persist_state(state, &device.id, &fresh).await;
                device.state = fresh;
            }
            Err(e) => {
                tracing::debug!("sensor device {id} unreachable: {e:#}");
                device.state.reachable = Some(false);
            }
        },
        Err(e) => {
            tracing::error!("failed to build sensor provider: {e:#}");
            device.state.reachable = Some(false);
        }
    }
    Ok(Some(device))
}

/// Persist a fresh sensor reading into the cache. Shared by the live read and
/// the push pipelines (Hue SSE / HA WebSocket write directly here).
pub(crate) async fn persist_state(state: &AppState, id: &str, sensor: &SensorState) {
    if let Ok(json) = serde_json::to_string(sensor) {
        let _ = sqlx::query(
            "UPDATE sensor_devices SET last_state = ?, last_seen = datetime('now') WHERE id = ?",
        )
        .bind(&json)
        .bind(id)
        .execute(&state.db)
        .await;
    }
}

/// Discover an integration's sensor devices and upsert them. Returns the count.
/// Called from the shared `/api/providers/{id}/discover` handler.
pub(crate) async fn discover_sensor_devices(
    state: &AppState,
    provider_row_id: &str,
    provider_type: &str,
    credentials_enc: &str,
) -> Result<usize, StatusCode> {
    let provider = build_sensor_provider(state, provider_type, credentials_enc).map_err(|e| {
        tracing::error!("failed to build sensor provider: {e:#}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let devices = provider.discover().await.map_err(|e| {
        tracing::error!("sensor discovery error: {e:#}");
        StatusCode::BAD_GATEWAY
    })?;

    // Batch the upserts in one transaction (one WAL commit, not one per device).
    let mut tx = state.db.begin().await.map_err(|e| {
        tracing::error!("discover_sensor_devices: begin failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    for device in &devices {
        let state_json = serde_json::to_string(&device.state).unwrap_or_default();
        let _ = sqlx::query(
            "INSERT INTO sensor_devices (id, provider_id, device_id, name, provider_name, kind, unit, last_state, last_seen, hw_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now'), ?)
             ON CONFLICT (provider_id, device_id)
             DO UPDATE SET name       = CASE WHEN name = provider_name THEN excluded.name ELSE name END,
                           provider_name = excluded.provider_name,
                           kind       = excluded.kind,
                           unit       = excluded.unit,
                           last_state = excluded.last_state,
                           last_seen  = excluded.last_seen,
                           hw_id      = excluded.hw_id",
        )
        .bind(device.id.to_string())
        .bind(provider_row_id)
        .bind(&device.provider_id)
        .bind(&device.name)
        .bind(&device.name)
        .bind(kind_str(device.kind))
        .bind(&device.unit)
        .bind(&state_json)
        .bind(&device.hw_id)
        .execute(&mut *tx)
        .await;
    }
    tx.commit().await.map_err(|e| {
        tracing::error!("discover_sensor_devices: commit failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(devices.len())
}

// ── Handlers (session-authenticated) ─────────────────────────────────────────

async fn list_devices_handler(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    match list_all_sensor_devices(&state).await {
        Ok(devices) => Json(devices).into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn get_device_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match get_sensor_device_live(&state, &id).await {
        Ok(Some(device)) => Json(device).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
