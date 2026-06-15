//! Global palette scenes: Hue-style presets of brightness + a color palette.
//!
//! A scene is standalone (not bound to a room). You define it once and apply it
//! to any room, which distributes the palette across that room's lights (a
//! single colour, or brightness-only, drives the whole room uniformly via the
//! native group call). A room's current colors can also be captured as a new
//! scene. Individual lights stay independently controllable — a scene just
//! drives a room to a saved look.

use crate::AppState;
use crate::api::auth::Session;
use crate::api::lights::build_provider;
use crate::api::rooms::{
    apply_uniform_state, effective_member_ids, effective_members, room_exists,
};
use crate::models::{Color, LightState};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_handler).post(create_handler))
        .route("/{id}", delete(remove_handler))
        .route("/from-room/{room_id}", post(create_from_room_handler))
}

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub(crate) struct PaletteScene {
    pub id: String,
    pub name: String,
    pub brightness: Option<f32>,
    pub palette: Vec<String>,
}

/// Validated input for creating a scene (shared by the UI and public APIs).
pub(crate) struct NewScene {
    pub name: String,
    pub brightness: Option<f32>,
    pub palette: Vec<String>,
}

/// Why a scene mutation failed, mapped to a status by each caller.
pub(crate) enum SceneError {
    Validation(String),
    NotFound,
    Db,
}

/// Parse "#rrggbb" (case-insensitive). None for anything else.
pub(crate) fn parse_hex_color(s: &str) -> Option<(u8, u8, u8)> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some((
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ))
}

fn rgb_to_hex(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

// ── Services (reused by the session UI API and the public /v1 API) ───────────

pub(crate) async fn list_scenes(state: &AppState) -> Result<Vec<PaletteScene>, ()> {
    sqlx::query("SELECT id, name, brightness, palette FROM palette_scenes ORDER BY created_at")
        .fetch_all(&state.db)
        .await
        .map_err(|e| tracing::error!("db error listing scenes: {e}"))
        .map(|rows| {
            rows.into_iter()
                .map(|r| PaletteScene {
                    id: r.get("id"),
                    name: r.get("name"),
                    brightness: r.get("brightness"),
                    palette: serde_json::from_str(&r.get::<String, _>("palette"))
                        .unwrap_or_default(),
                })
                .collect()
        })
}

fn validate(req: &NewScene) -> Result<(), SceneError> {
    if req.name.trim().is_empty() {
        return Err(SceneError::Validation("scene name is required".into()));
    }
    if let Some(b) = req.brightness
        && !(1.0..=100.0).contains(&b)
    {
        return Err(SceneError::Validation("brightness must be 1-100".into()));
    }
    for c in &req.palette {
        if parse_hex_color(c).is_none() {
            return Err(SceneError::Validation(format!(
                "'{c}' is not a #rrggbb color"
            )));
        }
    }
    Ok(())
}

async fn insert_scene(
    state: &AppState,
    name: &str,
    brightness: Option<f32>,
    palette: &[String],
) -> Result<String, SceneError> {
    let scene_id = Uuid::new_v4().to_string();
    let palette_json = serde_json::to_string(palette).unwrap_or_else(|_| "[]".into());
    sqlx::query("INSERT INTO palette_scenes (id, name, brightness, palette) VALUES (?, ?, ?, ?)")
        .bind(&scene_id)
        .bind(name)
        .bind(brightness)
        .bind(&palette_json)
        .execute(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("db error creating scene: {e}");
            SceneError::Db
        })?;
    Ok(scene_id)
}

pub(crate) async fn create_scene(state: &AppState, req: NewScene) -> Result<String, SceneError> {
    validate(&req)?;
    insert_scene(state, req.name.trim(), req.brightness, &req.palette).await
}

pub(crate) async fn delete_scene(state: &AppState, scene_id: &str) {
    let _ = sqlx::query("DELETE FROM palette_scenes WHERE id = ?")
        .bind(scene_id)
        .execute(&state.db)
        .await;
}

/// Capture a room's currently-lit lights as a new global scene: the palette is
/// the distinct colors those lights are showing, and brightness is their
/// average. `NotFound` if the room is unknown; `Validation` if nothing is lit.
pub(crate) async fn create_scene_from_room(
    state: &AppState,
    room_id: &str,
    name: &str,
) -> Result<String, SceneError> {
    if name.trim().is_empty() {
        return Err(SceneError::Validation("scene name is required".into()));
    }
    if !room_exists(state, room_id).await {
        return Err(SceneError::NotFound);
    }

    let member_ids = effective_member_ids(state, room_id).await;
    let mut palette: Vec<String> = Vec::new();
    let mut brightness_sum = 0.0f32;
    let mut lit = 0u32;

    for light_id in member_ids {
        let row = sqlx::query("SELECT last_state FROM lights WHERE id = ?")
            .bind(&light_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
        let Some(row) = row else { continue };
        let Some(json) = row.get::<Option<String>, _>("last_state") else {
            continue;
        };
        let Ok(st) = serde_json::from_str::<LightState>(&json) else {
            continue;
        };
        if !st.on {
            continue;
        }
        lit += 1;
        brightness_sum += st.brightness.unwrap_or(100.0);
        if let Some(c) = &st.color {
            let (r, g, b) = c.to_rgb();
            let hex = rgb_to_hex(r, g, b);
            if !palette.contains(&hex) {
                palette.push(hex);
            }
        }
    }

    if lit == 0 {
        return Err(SceneError::Validation(
            "no lights in this room are on".into(),
        ));
    }

    let brightness = Some((brightness_sum / lit as f32).clamp(1.0, 100.0));
    insert_scene(state, name.trim(), brightness, &palette).await
}

/// Apply a global scene to a room: members on, palette distributed round-robin
/// in stable order, brightness set. Single-color/empty palettes collapse to a
/// native group call. `None` when the scene is unknown or the room has no
/// members (→ 404).
pub(crate) async fn apply_scene_to_room(
    state: &AppState,
    scene_id: &str,
    room_id: &str,
) -> Option<(usize, usize)> {
    let scene = sqlx::query("SELECT brightness, palette FROM palette_scenes WHERE id = ?")
        .bind(scene_id)
        .fetch_optional(&state.db)
        .await
        .ok()??;
    let brightness: Option<f32> = scene.get("brightness");
    let palette: Vec<String> =
        serde_json::from_str(&scene.get::<String, _>("palette")).unwrap_or_default();
    let colors: Vec<Color> = palette
        .iter()
        .filter_map(|s| parse_hex_color(s))
        .map(|(r, g, b)| Color::from_rgb(r, g, b))
        .collect();

    let members = effective_members(state, room_id).await;
    if members.is_empty() {
        return None;
    }

    // An empty (brightness-only) or single-color palette drives every member to
    // the same state, so the native group call applies — one Hue grouped_light
    // PUT instead of one per light. Multi-color palettes fan out.
    if colors.len() <= 1 {
        let target = LightState {
            on: true,
            brightness,
            color: colors.first().cloned(),
            color_temp_mirek: None,
            reachable: None,
        };
        return Some(apply_uniform_state(state, room_id, &target, members).await);
    }

    let mut jobs = Vec::new();
    for (i, m) in members.into_iter().enumerate() {
        let provider = match build_provider(state, &m.provider_type, &m.credentials) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("scene apply: provider build failed: {e:#}");
                continue;
            }
        };

        let target = LightState {
            on: true,
            brightness,
            color: Some(colors[i % colors.len()].clone()),
            color_temp_mirek: None,
            reachable: None,
        };
        let target_json = serde_json::to_string(&target).unwrap_or_default();

        let db = state.db.clone();
        jobs.push(async move {
            match provider.set_state(&m.device_id, &target).await {
                Ok(()) => {
                    let _ = sqlx::query(
                        "UPDATE lights SET last_state = ?, last_seen = datetime('now') WHERE id = ?",
                    )
                    .bind(&target_json)
                    .bind(&m.light_id)
                    .execute(&db)
                    .await;
                    true
                }
                Err(e) => {
                    tracing::error!("scene apply: set_state failed for {}: {e:#}", m.device_id);
                    false
                }
            }
        });
    }

    let results = futures_util::future::join_all(jobs).await;
    let applied = results.iter().filter(|ok| **ok).count();
    let failed = results.len() - applied;
    Some((applied, failed))
}

// ── Handlers (session-authenticated; thin wrappers over the services) ────────

async fn list_handler(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    match list_scenes(&state).await {
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

async fn create_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Json(req): Json<CreateSceneRequest>,
) -> impl IntoResponse {
    let input = NewScene {
        name: req.name,
        brightness: req.brightness,
        palette: req.palette,
    };
    match create_scene(&state, input).await {
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

async fn remove_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    delete_scene(&state, &id).await;
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct FromRoomRequest {
    name: String,
}

async fn create_from_room_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(room_id): Path<String>,
    Json(req): Json<FromRoomRequest>,
) -> impl IntoResponse {
    match create_scene_from_room(&state, &room_id, &req.name).await {
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

#[cfg(test)]
mod tests {
    use super::{parse_hex_color, rgb_to_hex};

    #[test]
    fn parses_valid_hex_colors() {
        assert_eq!(parse_hex_color("#ff8800"), Some((255, 136, 0)));
        assert_eq!(parse_hex_color("#FFFFFF"), Some((255, 255, 255)));
        assert_eq!(parse_hex_color("#000000"), Some((0, 0, 0)));
    }

    #[test]
    fn rejects_malformed_hex_colors() {
        assert_eq!(parse_hex_color("ff8800"), None); // missing #
        assert_eq!(parse_hex_color("#fff"), None); // short form unsupported
        assert_eq!(parse_hex_color("#gg0000"), None); // bad digits
        assert_eq!(parse_hex_color("#ff88001"), None); // wrong length
    }

    #[test]
    fn rgb_to_hex_roundtrips_through_parse() {
        let hex = rgb_to_hex(255, 136, 0);
        assert_eq!(hex, "#ff8800");
        assert_eq!(parse_hex_color(&hex), Some((255, 136, 0)));
    }
}
