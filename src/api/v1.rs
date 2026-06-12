//! Public API (`/api/v1`) for third-party apps.
//!
//! Authenticated with a Bearer API key (`Authorization: Bearer <key>`) rather
//! than the session cookie the UI uses. There is no RBAC — a valid key has full
//! access. The surface mirrors what the UI can do for **lights** and **rooms**
//! (including scenes); the floor plan and provider internals are not exposed.
//!
//! Handlers are thin: they authenticate, then delegate to the same service
//! functions the session API uses, so behaviour can't drift between the two.

use crate::AppState;
use crate::api::apikeys::require_api_key;
use crate::api::lights::{apply_light_state, get_light_by_id, list_all_lights, set_light_status};
use crate::api::rooms::{
    NewScene, SceneError, apply_room_scene, apply_uniform_state, create_room_scene,
    delete_room_scene, effective_member_ids, effective_members, list_room_scenes, room_exists,
};
use crate::models::LightState;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/lights", get(list_lights))
        .route("/lights/{id}", get(get_light))
        .route("/lights/{id}/state", put(set_light_state))
        .route("/rooms", get(list_rooms))
        .route("/rooms/{id}/state", put(set_room_state))
        .route("/rooms/{id}/scenes", get(list_scenes).post(create_scene))
        .route(
            "/rooms/{id}/scenes/{scene_id}",
            axum::routing::delete(remove_scene),
        )
        .route("/rooms/{id}/scenes/{scene_id}/apply", post(apply_scene))
}

/// Shared 401 guard. Returns `Err(401)` when the Bearer key is missing/invalid.
async fn auth(state: &Arc<AppState>, headers: &HeaderMap) -> Result<(), StatusCode> {
    match require_api_key(state, headers).await {
        Some(_) => Ok(()),
        None => Err(StatusCode::UNAUTHORIZED),
    }
}

// ── Lights ───────────────────────────────────────────────────────────────────

async fn list_lights(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
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
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
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
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    set_light_status(apply_light_state(&state, &id, &new_state).await).into_response()
}

// ── Rooms ────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct V1Room {
    id: String,
    name: String,
    /// Effective members (linked provider-group lights ∪ direct lights).
    light_ids: Vec<String>,
}

async fn list_rooms(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }

    let rows = sqlx::query("SELECT id, name FROM rooms ORDER BY created_at")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let id: String = row.get("id");
        let light_ids = effective_member_ids(&state, &id).await;
        out.push(V1Room {
            name: row.get("name"),
            light_ids,
            id,
        });
    }
    Json(out).into_response()
}

async fn set_room_state(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(new_state): Json<LightState>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    let members = effective_members(&state, &id).await;
    if members.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let (applied, failed) = apply_uniform_state(&state, &id, &new_state, members).await;
    Json(serde_json::json!({ "applied": applied, "failed": failed })).into_response()
}

// ── Room scenes ──────────────────────────────────────────────────────────────

async fn list_scenes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    // Distinguish a missing room (404) from a room with no scenes (empty list).
    if !room_exists(&state, &id).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    match list_room_scenes(&state, &id).await {
        Ok(scenes) => Json(scenes).into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
struct CreateSceneRequest {
    name: String,
    #[serde(default)]
    brightness: Option<f32>,
    #[serde(default)]
    palette: Vec<String>,
}

async fn create_scene(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<CreateSceneRequest>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    let input = NewScene {
        name: req.name,
        brightness: req.brightness,
        palette: req.palette,
    };
    match create_room_scene(&state, &id, input).await {
        Ok(scene_id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "id": scene_id })),
        )
            .into_response(),
        Err(SceneError::Validation(m)) => (StatusCode::UNPROCESSABLE_ENTITY, m).into_response(),
        Err(SceneError::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(SceneError::Db) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn remove_scene(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((room_id, scene_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    delete_room_scene(&state, &room_id, &scene_id).await;
    StatusCode::NO_CONTENT.into_response()
}

async fn apply_scene(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((room_id, scene_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    match apply_room_scene(&state, &room_id, &scene_id).await {
        Some((applied, failed)) => {
            Json(serde_json::json!({ "applied": applied, "failed": failed })).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
