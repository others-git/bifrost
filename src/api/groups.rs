//! Light groups: named sets of lights controlled together.

use crate::AppState;
use crate::api::auth::require_session;
use crate::api::lights::build_provider;
use crate::models::LightState;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_groups).post(create_group))
        .route("/{id}", delete(remove_group))
        .route("/{id}/lights", put(set_members))
        .route("/{id}/state", put(set_group_state))
        .route("/{id}/scenes", get(list_scenes).post(create_scene))
        .route("/{id}/scenes/{scene_id}", delete(remove_scene))
        .route("/{id}/scenes/{scene_id}/apply", post(apply_scene))
}

#[derive(Serialize)]
struct GroupRow {
    id: String,
    name: String,
    light_ids: Vec<String>,
}

async fn list_groups(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let groups = match sqlx::query("SELECT id, name FROM groups ORDER BY created_at")
        .fetch_all(&state.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("db error listing groups: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let mut out = Vec::with_capacity(groups.len());
    for g in groups {
        let id: String = g.get("id");
        let members = sqlx::query("SELECT light_id FROM group_lights WHERE group_id = ?")
            .bind(&id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();
        out.push(GroupRow {
            id,
            name: g.get("name"),
            light_ids: members.into_iter().map(|m| m.get("light_id")).collect(),
        });
    }

    Json(out).into_response()
}

#[derive(Deserialize)]
struct CreateGroupRequest {
    name: String,
    #[serde(default)]
    light_ids: Vec<String>,
}

async fn create_group(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateGroupRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if req.name.trim().is_empty() {
        return (StatusCode::UNPROCESSABLE_ENTITY, "group name is required").into_response();
    }

    let id = Uuid::new_v4().to_string();
    if let Err(e) = sqlx::query("INSERT INTO groups (id, name) VALUES (?, ?)")
        .bind(&id)
        .bind(req.name.trim())
        .execute(&state.db)
        .await
    {
        tracing::error!("db error: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    for light_id in &req.light_ids {
        let _ =
            sqlx::query("INSERT OR IGNORE INTO group_lights (group_id, light_id) VALUES (?, ?)")
                .bind(&id)
                .bind(light_id)
                .execute(&state.db)
                .await;
    }

    (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response()
}

async fn remove_group(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let _ = sqlx::query("DELETE FROM groups WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct SetMembersRequest {
    light_ids: Vec<String>,
}

/// Replace the group's membership with the given light IDs.
async fn set_members(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<SetMembersRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let exists = sqlx::query("SELECT 1 FROM groups WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await;
    match exists {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    let _ = sqlx::query("DELETE FROM group_lights WHERE group_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;
    for light_id in &req.light_ids {
        let _ =
            sqlx::query("INSERT OR IGNORE INTO group_lights (group_id, light_id) VALUES (?, ?)")
                .bind(&id)
                .bind(light_id)
                .execute(&state.db)
                .await;
    }

    StatusCode::NO_CONTENT.into_response()
}

/// Apply one state to every light in the group, in parallel.
async fn set_group_state(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(new_state): Json<LightState>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let members = match sqlx::query(
        "SELECT l.id AS light_id, l.device_id, p.provider_type, p.credentials
         FROM group_lights g
         JOIN lights l ON l.id = g.light_id
         JOIN providers p ON p.id = l.provider_id
         WHERE g.group_id = ? AND p.enabled = 1",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("db error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if members.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let state_json = serde_json::to_string(&new_state).unwrap_or_default();
    let mut jobs = Vec::new();
    for row in members {
        let light_id: String = row.get("light_id");
        let device_id: String = row.get("device_id");
        let provider_type: String = row.get("provider_type");
        let credentials_enc: String = row.get("credentials");

        let provider = match build_provider(&state, &provider_type, &credentials_enc) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("group state: provider build failed: {e:#}");
                continue;
            }
        };

        let db = state.db.clone();
        let target = new_state.clone();
        let target_json = state_json.clone();
        jobs.push(async move {
            match provider.set_state(&device_id, &target).await {
                Ok(()) => {
                    let _ = sqlx::query(
                        "UPDATE lights SET last_state = ?, last_seen = datetime('now') WHERE id = ?",
                    )
                    .bind(&target_json)
                    .bind(&light_id)
                    .execute(&db)
                    .await;
                    true
                }
                Err(e) => {
                    tracing::error!("group state: set_state failed for {device_id}: {e:#}");
                    false
                }
            }
        });
    }

    let results = futures_util::future::join_all(jobs).await;
    let applied = results.iter().filter(|ok| **ok).count();
    let failed = results.len() - applied;

    Json(serde_json::json!({ "applied": applied, "failed": failed })).into_response()
}

// ── Group scenes (Hue-like palette scenes) ──────────────────────────────────

#[derive(Serialize, Deserialize)]
struct GroupScene {
    id: String,
    group_id: String,
    name: String,
    /// 1..100; None leaves brightness unchanged.
    brightness: Option<f32>,
    /// Hex colours ("#rrggbb") distributed round-robin across the lights.
    palette: Vec<String>,
}

/// Parse "#rrggbb" (case-insensitive). None for anything else.
fn parse_hex_color(s: &str) -> Option<(u8, u8, u8)> {
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

async fn list_scenes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    match sqlx::query(
        "SELECT id, group_id, name, brightness, palette FROM group_scenes
         WHERE group_id = ? ORDER BY created_at",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|r| GroupScene {
                    id: r.get("id"),
                    group_id: r.get("group_id"),
                    name: r.get("name"),
                    brightness: r.get("brightness"),
                    palette: serde_json::from_str(&r.get::<String, _>("palette"))
                        .unwrap_or_default(),
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => {
            tracing::error!("db error listing group scenes: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
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
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if req.name.trim().is_empty() {
        return (StatusCode::UNPROCESSABLE_ENTITY, "scene name is required").into_response();
    }
    if let Some(b) = req.brightness
        && !(1.0..=100.0).contains(&b)
    {
        return (StatusCode::UNPROCESSABLE_ENTITY, "brightness must be 1-100").into_response();
    }
    for c in &req.palette {
        if parse_hex_color(c).is_none() {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("'{c}' is not a #rrggbb colour"),
            )
                .into_response();
        }
    }

    let exists = sqlx::query("SELECT 1 FROM groups WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await;
    match exists {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    let scene_id = Uuid::new_v4().to_string();
    let palette_json = serde_json::to_string(&req.palette).unwrap_or_else(|_| "[]".into());
    match sqlx::query(
        "INSERT INTO group_scenes (id, group_id, name, brightness, palette) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&scene_id)
    .bind(&id)
    .bind(req.name.trim())
    .bind(req.brightness)
    .bind(&palette_json)
    .execute(&state.db)
    .await
    {
        Ok(_) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "id": scene_id })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("db error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn remove_scene(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((group_id, scene_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let _ = sqlx::query("DELETE FROM group_scenes WHERE id = ? AND group_id = ?")
        .bind(&scene_id)
        .bind(&group_id)
        .execute(&state.db)
        .await;

    StatusCode::NO_CONTENT.into_response()
}

/// Apply the scene: turn members on, distribute the palette round-robin
/// (light 1 → colour 1, light 2 → colour 2, …, wrapping), set brightness.
/// Lights are ordered by name so the distribution is stable.
async fn apply_scene(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((group_id, scene_id)): Path<(String, String)>,
) -> impl IntoResponse {
    use crate::models::Color;

    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let scene = match sqlx::query(
        "SELECT brightness, palette FROM group_scenes WHERE id = ? AND group_id = ?",
    )
    .bind(&scene_id)
    .bind(&group_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let brightness: Option<f32> = scene.get("brightness");
    let palette: Vec<String> =
        serde_json::from_str(&scene.get::<String, _>("palette")).unwrap_or_default();
    let colors: Vec<Color> = palette
        .iter()
        .filter_map(|s| parse_hex_color(s))
        .map(|(r, g, b)| Color::from_rgb(r, g, b))
        .collect();

    let members = match sqlx::query(
        "SELECT l.id AS light_id, l.device_id, p.provider_type, p.credentials
         FROM group_lights g
         JOIN lights l ON l.id = g.light_id
         JOIN providers p ON p.id = l.provider_id
         WHERE g.group_id = ? AND p.enabled = 1
         ORDER BY l.name",
    )
    .bind(&group_id)
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("db error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if members.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let mut jobs = Vec::new();
    for (i, row) in members.into_iter().enumerate() {
        let light_id: String = row.get("light_id");
        let device_id: String = row.get("device_id");
        let provider_type: String = row.get("provider_type");
        let credentials_enc: String = row.get("credentials");

        let provider = match build_provider(&state, &provider_type, &credentials_enc) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("scene apply: provider build failed: {e:#}");
                continue;
            }
        };

        let target = LightState {
            on: true,
            brightness,
            color: if colors.is_empty() {
                None
            } else {
                Some(colors[i % colors.len()].clone())
            },
            color_temp_mirek: None,
        };
        let target_json = serde_json::to_string(&target).unwrap_or_default();

        let db = state.db.clone();
        jobs.push(async move {
            match provider.set_state(&device_id, &target).await {
                Ok(()) => {
                    let _ = sqlx::query(
                        "UPDATE lights SET last_state = ?, last_seen = datetime('now') WHERE id = ?",
                    )
                    .bind(&target_json)
                    .bind(&light_id)
                    .execute(&db)
                    .await;
                    true
                }
                Err(e) => {
                    tracing::error!("scene apply: set_state failed for {device_id}: {e:#}");
                    false
                }
            }
        });
    }

    let results = futures_util::future::join_all(jobs).await;
    let applied = results.iter().filter(|ok| **ok).count();
    let failed = results.len() - applied;

    Json(serde_json::json!({ "applied": applied, "failed": failed })).into_response()
}

#[cfg(test)]
mod tests {
    use super::parse_hex_color;

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
}
