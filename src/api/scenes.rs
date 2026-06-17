//! Scenes: named snapshots of light + power-device state, applied all at once.
//! A scene is scoped by `room_id`: a null `room_id` is a whole-home snapshot (a
//! *Home Scene*), a set `room_id` scopes it to that room's effective members (a
//! *Room Scene*). Both store each light's full `LightState` (colour, temperature,
//! or effect) together with each power device's on/off, and share one capture
//! (`capture_scene`) and apply (`apply_scene_entries`) engine. One home scene can
//! be the **default** — the one-tap "Restore Home" preset
//! (`POST /restore-default`), handy after a power outage resets bulbs/switches.

use crate::AppState;
use crate::api::auth::Session;
use crate::api::lights::build_provider;
use crate::api::power::{SetPowerOutcome, apply_power_state};
use crate::models::LightState;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_scenes).post(create_scene))
        .route("/restore-default", post(restore_default))
        .route("/{id}", delete(remove_scene))
        .route("/{id}/activate", post(activate_scene))
        .route("/{id}/default", put(set_default))
}

#[derive(Serialize)]
pub(crate) struct SceneRow {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub lights: i64,
    pub power: i64,
    /// The single preset the "Restore Home" button applies (home scenes only).
    pub is_default: bool,
    /// `None` = a whole-home scene; `Some(room)` = a room-scoped scene.
    pub room_id: Option<String>,
    /// Display name of the scoped room, for the UI grouping (None for home).
    pub room_name: Option<String>,
}

/// List every scene (home + room-scoped), newest grouping first. Shared by the
/// session, `/api/v1`, and MCP surfaces.
pub(crate) async fn list_all_scenes(state: &AppState) -> Result<Vec<SceneRow>, ()> {
    sqlx::query(
        "SELECT s.id, s.name, s.created_at, s.is_default, s.room_id, r.name AS room_name,
                (SELECT COUNT(*) FROM scene_entries e WHERE e.scene_id = s.id) AS lights,
                (SELECT COUNT(*) FROM scene_power_entries pe WHERE pe.scene_id = s.id) AS power
         FROM scenes s LEFT JOIN rooms r ON r.id = s.room_id
         ORDER BY s.room_id IS NOT NULL, r.name, s.created_at",
    )
    .fetch_all(&state.db)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|r| SceneRow {
                id: r.get("id"),
                name: r.get("name"),
                created_at: r.get("created_at"),
                lights: r.get("lights"),
                power: r.get("power"),
                is_default: r.get::<i64, _>("is_default") != 0,
                room_id: r.get("room_id"),
                room_name: r.get("room_name"),
            })
            .collect()
    })
    .map_err(|e| tracing::error!("db error listing scenes: {e}"))
}

async fn list_scenes(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    match list_all_scenes(&state).await {
        Ok(scenes) => Json(scenes).into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Outcome of capturing a scene snapshot.
pub(crate) struct SceneCapture {
    pub id: String,
    pub lights: usize,
    pub power: usize,
}

/// Why a capture failed (mapped to HTTP by each caller).
pub(crate) enum SceneCaptureError {
    /// The name was blank.
    EmptyName,
    /// A room scope named an unknown room.
    RoomNotFound,
    /// The DB write failed.
    Db,
}

#[derive(Deserialize)]
struct CreateSceneRequest {
    name: String,
    /// Omit / null for a whole-home scene; a room id scopes the snapshot to that
    /// room's effective members (Room Scene).
    #[serde(default)]
    room_id: Option<String>,
}

/// Snapshot light + power state into a new scene, shared by every surface.
/// `room_id = None` captures the whole home; `Some(room)` captures only that
/// room's effective members (a Room Scene). Each light's full `LightState`
/// (colour **or** temperature **or** effect) and each power device's on/off bit
/// are stored, so the scene restores exactly what was showing — effects included.
pub(crate) async fn capture_scene(
    state: &AppState,
    name: &str,
    room_id: Option<&str>,
) -> Result<SceneCapture, SceneCaptureError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(SceneCaptureError::EmptyName);
    }

    // A room scene is scoped to that room's effective members; a home scene to
    // everything. We over-fetch all devices and filter by the membership set
    // (avoids a dynamic `IN (…)` and reuses the shared room helpers).
    let (light_scope, power_scope) = match room_id {
        Some(rid) => {
            if !crate::api::rooms::room_exists(state, rid).await {
                return Err(SceneCaptureError::RoomNotFound);
            }
            (
                Some(
                    crate::api::rooms::effective_member_ids(state, rid)
                        .await
                        .into_iter()
                        .collect::<std::collections::HashSet<_>>(),
                ),
                Some(
                    crate::api::rooms::effective_power_member_ids(state, rid)
                        .await
                        .into_iter()
                        .collect::<std::collections::HashSet<_>>(),
                ),
            )
        }
        None => (None, None),
    };
    let in_scope = |id: &str, scope: &Option<std::collections::HashSet<String>>| {
        scope.as_ref().is_none_or(|s| s.contains(id))
    };

    let lights = sqlx::query("SELECT id, last_state FROM lights WHERE last_state IS NOT NULL")
        .fetch_all(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("db error: {e}");
            SceneCaptureError::Db
        })?;
    // Enabled, non-shadowed power devices with a known state.
    let powers = sqlx::query(
        "SELECT id, last_state FROM power_devices
         WHERE last_state IS NOT NULL AND enabled = 1 AND shadowed_by IS NULL",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let scene_id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO scenes (id, name, room_id) VALUES (?, ?, ?)")
        .bind(&scene_id)
        .bind(name)
        .bind(room_id)
        .execute(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("db error: {e}");
            SceneCaptureError::Db
        })?;

    let mut captured = 0usize;
    for row in &lights {
        let light_id: String = row.get("id");
        if !in_scope(&light_id, &light_scope) {
            continue;
        }
        let last_state: String = row.get("last_state");
        if sqlx::query("INSERT INTO scene_entries (scene_id, light_id, state) VALUES (?, ?, ?)")
            .bind(&scene_id)
            .bind(&light_id)
            .bind(&last_state)
            .execute(&state.db)
            .await
            .is_ok()
        {
            captured += 1;
        }
    }

    let mut power_captured = 0usize;
    for row in &powers {
        let id: String = row.get("id");
        if !in_scope(&id, &power_scope) {
            continue;
        }
        let last_state: String = row.get("last_state");
        // Only the on/off bit matters for a power device.
        let on = serde_json::from_str::<serde_json::Value>(&last_state)
            .ok()
            .and_then(|v| v.get("on").and_then(|b| b.as_bool()));
        let Some(on) = on else { continue };
        if sqlx::query(
            "INSERT INTO scene_power_entries (scene_id, power_device_id, on_state) VALUES (?, ?, ?)",
        )
        .bind(&scene_id)
        .bind(&id)
        .bind(on as i64)
        .execute(&state.db)
        .await
        .is_ok()
        {
            power_captured += 1;
        }
    }

    Ok(SceneCapture {
        id: scene_id,
        lights: captured,
        power: power_captured,
    })
}

async fn create_scene(
    State(state): State<Arc<AppState>>,
    _: Session,
    Json(req): Json<CreateSceneRequest>,
) -> impl IntoResponse {
    match capture_scene(&state, &req.name, req.room_id.as_deref()).await {
        Ok(c) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "id": c.id, "lights": c.lights, "power": c.power })),
        )
            .into_response(),
        Err(SceneCaptureError::EmptyName) => {
            (StatusCode::UNPROCESSABLE_ENTITY, "scene name is required").into_response()
        }
        Err(SceneCaptureError::RoomNotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(SceneCaptureError::Db) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Delete a scene (its entries cascade). Shared by every surface.
pub(crate) async fn delete_scene(state: &AppState, id: &str) {
    let _ = sqlx::query("DELETE FROM scenes WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await;
}

async fn remove_scene(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    delete_scene(&state, &id).await;
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize, Default)]
struct ActivateRequest {
    /// When present, only entries for these lights are applied — used by the
    /// floor-plan room controller to scope a scene to one room.
    #[serde(default)]
    light_ids: Option<Vec<String>>,
}

/// Apply a scene's entries, lights in parallel via each provider. An optional
/// body `{light_ids: [...]}` restricts which **lights** apply (floor-plan room
/// scoping); a scoped apply skips power entries (a room-light scope must not
/// toggle every switch in the home). Returns `None` when the scene has no
/// entries at all (→ 404).
async fn activate_scene(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    body: Option<Json<ActivateRequest>>,
) -> impl IntoResponse {
    let filter = body.and_then(|Json(b)| b.light_ids);
    match apply_scene_entries(&state, &id, filter).await {
        Some((applied, failed)) => {
            Json(serde_json::json!({ "applied": applied, "failed": failed })).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Core apply shared by every surface (`activate_scene`, `restore_default`, v1,
/// MCP). `light_filter` scopes the lights (and, when present, suppresses power so
/// a room-light scope stays lights-only — a room scene activated *without* a
/// filter still applies its own captured power members). `(applied, failed)`
/// counts both domains; `None` = the scene has no entries at all (→ 404).
pub(crate) async fn apply_scene_entries(
    state: &AppState,
    scene_id: &str,
    light_filter: Option<Vec<String>>,
) -> Option<(usize, usize)> {
    let light_rows = sqlx::query(
        "SELECT e.light_id, e.state, l.device_id, p.provider_type, p.credentials
         FROM scene_entries e
         JOIN lights l ON l.id = e.light_id
         JOIN providers p ON p.id = l.provider_id
         WHERE e.scene_id = ? AND p.enabled = 1",
    )
    .bind(scene_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    // Power entries only on an unscoped (whole-home) apply.
    let power_rows = if light_filter.is_none() {
        sqlx::query("SELECT power_device_id, on_state FROM scene_power_entries WHERE scene_id = ?")
            .bind(scene_id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    // 404 only when the scene itself is empty; an exhaustive filter just applies
    // zero entries.
    if light_rows.is_empty() && power_rows.is_empty() {
        // Distinguish "no such scene / empty" from "filtered to nothing".
        let exists = sqlx::query("SELECT 1 FROM scene_entries WHERE scene_id = ? LIMIT 1")
            .bind(scene_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .is_some();
        if !exists && light_filter.is_none() {
            return None;
        }
    }

    let light_rows: Vec<_> = match &light_filter {
        Some(ids) => light_rows
            .into_iter()
            .filter(|r| ids.contains(&r.get::<String, _>("light_id")))
            .collect(),
        None => light_rows,
    };

    let mut jobs = Vec::new();
    for row in light_rows {
        let light_id: String = row.get("light_id");
        let device_id: String = row.get("device_id");
        let provider_type: String = row.get("provider_type");
        let credentials_enc: String = row.get("credentials");
        let state_json: String = row.get("state");

        let light_state: LightState = match serde_json::from_str(&state_json) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let provider = match build_provider(state, &provider_type, &credentials_enc) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("scene activate: provider build failed: {e:#}");
                continue;
            }
        };

        let db = state.db.clone();
        jobs.push(async move {
            match provider.set_state(&device_id, &light_state).await {
                Ok(()) => {
                    let _ = sqlx::query(
                        "UPDATE lights SET last_state = ?, last_seen = datetime('now') WHERE id = ?",
                    )
                    .bind(&state_json)
                    .bind(&light_id)
                    .execute(&db)
                    .await;
                    true
                }
                Err(e) => {
                    tracing::error!("scene activate: set_state failed for {device_id}: {e:#}");
                    false
                }
            }
        });
    }

    let results = futures_util::future::join_all(jobs).await;
    let mut applied = results.iter().filter(|ok| **ok).count();
    let mut failed = results.len() - applied;

    // Power devices, sequentially via the shared service fn.
    for row in power_rows {
        let pid: String = row.get("power_device_id");
        let on = row.get::<i64, _>("on_state") != 0;
        match apply_power_state(state, &pid, on).await {
            SetPowerOutcome::Ok => applied += 1,
            _ => failed += 1,
        }
    }

    Some((applied, failed))
}

#[derive(Deserialize)]
struct SetDefaultRequest {
    #[serde(default)]
    default: bool,
}

/// Mark (or unmark) a scene as the single "Restore Home" default.
async fn set_default(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetDefaultRequest>,
) -> impl IntoResponse {
    // Only a whole-home scene can be the single "Restore Home" default.
    let room_id: Option<Option<String>> =
        sqlx::query_scalar("SELECT room_id FROM scenes WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    match room_id {
        None => return StatusCode::NOT_FOUND.into_response(),
        Some(Some(_)) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                "only whole-home scenes can be the Restore Home default",
            )
                .into_response();
        }
        Some(None) => {}
    }
    // At most one default (partial-unique index); clear all first, then set.
    let _ = sqlx::query("UPDATE scenes SET is_default = 0")
        .execute(&state.db)
        .await;
    if req.default {
        let _ = sqlx::query("UPDATE scenes SET is_default = 1 WHERE id = ?")
            .bind(&id)
            .execute(&state.db)
            .await;
    }
    StatusCode::NO_CONTENT.into_response()
}

/// One-tap "Restore Home": apply the default scene (whole-home, lights + power).
/// 404 when no default is set.
async fn restore_default(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    let default_id: Option<String> =
        sqlx::query_scalar("SELECT id FROM scenes WHERE is_default = 1")
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let Some(id) = default_id else {
        return (StatusCode::NOT_FOUND, "no default home scene is set").into_response();
    };
    match apply_scene_entries(&state, &id, None).await {
        Some((applied, failed)) => {
            Json(serde_json::json!({ "applied": applied, "failed": failed })).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
