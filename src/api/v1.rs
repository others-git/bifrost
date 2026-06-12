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
use crate::api::audio::{
    PlayFavoriteRequest, apply_audio_command, favorites_response, get_device_live,
    list_all_devices, list_device_favorites, play_device_favorite, play_favorite_response,
    set_audio_status,
};
use crate::api::lights::{apply_light_state, get_light_by_id, list_all_lights, set_light_status};
use crate::api::palette_scenes::{
    NewScene, SceneError, apply_scene_to_room, create_scene as create_palette_scene,
    create_scene_from_room, delete_scene as delete_palette_scene,
    list_scenes as list_palette_scenes,
};
use crate::api::rooms::{
    apply_uniform_state, effective_audio_members, effective_member_ids, effective_members,
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
        .route("/rooms/{id}/scenes/{scene_id}/apply", post(apply_scene))
        .route("/scenes", get(list_scenes).post(create_scene))
        .route("/scenes/from-room/{room_id}", post(create_scene_from))
        .route("/scenes/{id}", axum::routing::delete(remove_scene))
        .route("/audio/devices", get(list_audio))
        .route("/audio/devices/{id}", get(get_audio))
        .route("/audio/devices/{id}/state", put(set_audio))
        .route("/audio/devices/{id}/favorites", get(list_audio_favorites))
        .route(
            "/audio/devices/{id}/favorites/play",
            post(play_audio_favorite),
        )
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
    /// Audio devices the room controls — drive each via /audio/devices/{id}/state.
    audio_device_ids: Vec<String>,
}

async fn list_rooms(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }

    // Disabled rooms are hidden from the public API too.
    let rows = sqlx::query("SELECT id, name FROM rooms WHERE enabled = 1 ORDER BY created_at")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let id: String = row.get("id");
        let light_ids = effective_member_ids(&state, &id).await;
        let audio_device_ids = effective_audio_members(&state, &id)
            .await
            .into_iter()
            .map(|m| m.audio_device_id)
            .collect();
        out.push(V1Room {
            name: row.get("name"),
            light_ids,
            audio_device_ids,
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

// ── Scenes (global palette presets, applied to rooms) ────────────────────────

async fn list_scenes(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    match list_palette_scenes(&state).await {
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
    scene_create_response(create_palette_scene(&state, input).await)
}

#[derive(Deserialize)]
struct FromRoomRequest {
    name: String,
}

async fn create_scene_from(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(room_id): Path<String>,
    Json(req): Json<FromRoomRequest>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    scene_create_response(create_scene_from_room(&state, &room_id, &req.name).await)
}

fn scene_create_response(result: Result<String, SceneError>) -> axum::response::Response {
    match result {
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
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    delete_palette_scene(&state, &id).await;
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
    match apply_scene_to_room(&state, &scene_id, &room_id).await {
        Some((applied, failed)) => {
            Json(serde_json::json!({ "applied": applied, "failed": failed })).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

// ── Audio devices ─────────────────────────────────────────────────────────────

async fn list_audio(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    match list_all_devices(&state).await {
        Ok(devices) => Json(devices).into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn get_audio(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    match get_device_live(&state, &id).await {
        Ok(Some(device)) => Json(device).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn set_audio(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(cmd): Json<crate::models::audio::AudioCommand>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    set_audio_status(apply_audio_command(&state, &id, &cmd).await)
}

async fn list_audio_favorites(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    favorites_response(list_device_favorites(&state, &id).await)
}

async fn play_audio_favorite(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<PlayFavoriteRequest>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    play_favorite_response(play_device_favorite(&state, &id, &req.favorite_id).await)
}
