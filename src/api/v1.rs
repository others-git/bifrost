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
    GroupRequest, PlayFavoriteRequest, apply_audio_command, favorites_response, get_device_live,
    group_devices, group_response, list_all_devices, list_device_favorites, play_device_favorite,
    play_favorite_response, set_audio_companion, set_audio_receiver, set_audio_status,
    set_companion_status, set_receiver_status, ungroup_device,
};
use crate::api::lights::{apply_light_state, get_light_by_id, list_all_lights, set_light_status};
use crate::api::power::{
    PowerCommand, apply_power_state, get_power_device_live, list_all_power_devices,
    set_power_status,
};
use crate::api::remote::{apply_remote_command, list_remotes, read_remote_state, remote_status};
use crate::api::rooms::{apply_room_state, effective_members, list_public_rooms};
use crate::api::scenes::{
    SceneCaptureError, apply_scene_entries, capture_scene, delete_scene as delete_scene_svc,
    list_all_scenes,
};
use crate::models::LightState;
use crate::models::remote::RemoteCommand;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post, put},
};
use serde::Deserialize;
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/lights", get(list_lights))
        .route("/lights/{id}", get(get_light))
        .route("/lights/{id}/state", put(set_light_state))
        .route("/rooms", get(list_rooms))
        .route("/rooms/{id}/state", put(set_room_state))
        .route("/scenes", get(list_scenes).post(create_scene))
        .route("/scenes/{id}/activate", post(activate_scene))
        .route("/scenes/{id}", axum::routing::delete(remove_scene))
        .route("/audio/devices", get(list_audio))
        .route("/audio/devices/{id}", get(get_audio))
        .route("/audio/devices/{id}/state", put(set_audio))
        .route("/audio/devices/{id}/favorites", get(list_audio_favorites))
        .route(
            "/audio/devices/{id}/favorites/play",
            post(play_audio_favorite),
        )
        .route("/audio/devices/{id}/group", post(group_audio))
        .route("/audio/devices/{id}/ungroup", post(ungroup_audio))
        .route("/audio/devices/{id}/receiver", put(set_receiver_audio))
        .route("/audio/devices/{id}/companion", put(set_companion_audio))
        .route("/power/devices", get(list_power))
        .route("/power/devices/{id}", get(get_power))
        .route("/power/devices/{id}/state", put(set_power))
        .route("/remote/devices", get(list_remote))
        .route("/remote/devices/{id}", get(get_remote))
        .route("/remote/devices/{id}/command", post(remote_command))
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

async fn list_rooms(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    Json(list_public_rooms(&state).await).into_response()
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
    let (applied, failed) = apply_room_state(&state, &id, &new_state, members).await;
    if applied == 0 && failed == 0 {
        return StatusCode::NOT_FOUND.into_response();
    }
    Json(serde_json::json!({ "applied": applied, "failed": failed })).into_response()
}

// ── Scenes (full-state snapshots: whole-home or room-scoped) ─────────────────

async fn list_scenes(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    match list_all_scenes(&state).await {
        Ok(scenes) => Json(scenes).into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
struct CreateSceneRequest {
    name: String,
    /// Omit for a whole-home snapshot; a room id scopes it to that room (Room Scene).
    #[serde(default)]
    room_id: Option<String>,
}

async fn create_scene(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateSceneRequest>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
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

async fn remove_scene(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    delete_scene_svc(&state, &id).await;
    StatusCode::NO_CONTENT.into_response()
}

async fn activate_scene(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    match apply_scene_entries(&state, &id, None).await {
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

async fn group_audio(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<GroupRequest>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    group_response(group_devices(&state, &id, &req.coordinator_id).await)
}

async fn ungroup_audio(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    group_response(ungroup_device(&state, &id).await)
}

async fn set_receiver_audio(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetReceiverRequest>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    set_receiver_status(set_audio_receiver(&state, &id, req.receiver_id, req.receiver_source).await)
}

async fn set_companion_audio(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetCompanionRequest>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    set_companion_status(set_audio_companion(&state, &id, req.primary_id).await)
}

// ── Power devices ──────────────────────────────────────────────────────────────

async fn list_power(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    match list_all_power_devices(&state).await {
        Ok(devices) => Json(devices).into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn get_power(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    match get_power_device_live(&state, &id).await {
        Ok(Some(device)) => Json(device).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn set_power(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(cmd): Json<PowerCommand>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    set_power_status(apply_power_state(&state, &id, cmd.on).await).into_response()
}

async fn list_remote(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    Json(list_remotes(&state).await).into_response()
}

async fn get_remote(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    match read_remote_state(&state, &id).await {
        Some(s) => Json(s).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn remote_command(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(cmd): Json<RemoteCommand>,
) -> impl IntoResponse {
    if let Err(s) = auth(&state, &headers).await {
        return s.into_response();
    }
    remote_status(apply_remote_command(&state, &id, &cmd).await).into_response()
}
