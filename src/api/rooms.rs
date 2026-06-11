//! Rooms: the user-owned grouping abstraction.
//!
//! A room aggregates **links** to provider-group mirrors (synced from the
//! provider — Hue rooms/zones) plus **direct lights** (for providers without
//! a native grouping concept). Effective membership is the union of both.
//!
//! Control prefers the provider's native group call (Hue `grouped_light`,
//! one request for the whole room) when a provider's members all come from a
//! single linked group; otherwise it fans out per light in parallel.

use crate::AppState;
use crate::api::auth::require_session;
use crate::api::lights::build_provider;
use crate::models::{Color, LightState};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_rooms).post(create_room))
        .route("/{id}", delete(remove_room))
        .route("/{id}/merge", post(merge_rooms))
        .route("/{id}/lights", put(set_direct_lights))
        .route("/{id}/links", put(set_links))
        .route("/{id}/state", put(set_room_state))
        .route("/{id}/scenes", get(list_scenes).post(create_scene))
        .route("/{id}/scenes/{scene_id}", delete(remove_scene))
        .route("/{id}/scenes/{scene_id}/apply", post(apply_scene))
}

/// Read-only list of provider-group mirrors, for the link-editing UI.
pub fn provider_groups_router() -> Router<Arc<AppState>> {
    Router::new().route("/", get(list_provider_groups))
}

// ── Wire types ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct LinkInfo {
    provider_group_id: String,
    name: String,
    provider_id: String,
}

#[derive(Serialize)]
struct RoomInfo {
    id: String,
    name: String,
    /// Union of linked provider-group members and direct lights.
    light_ids: Vec<String>,
    direct_light_ids: Vec<String>,
    links: Vec<LinkInfo>,
}

#[derive(Serialize)]
struct ProviderGroupInfo {
    id: String,
    provider_id: String,
    provider_group_id: String,
    name: String,
    light_ids: Vec<String>,
}

// ── Membership resolution ────────────────────────────────────────────────────

struct MemberRow {
    light_id: String,
    device_id: String,
    provider_type: String,
    credentials: String,
}

/// All effective members of a room (links ∪ direct), with provider info,
/// deduplicated, ordered by light name for stable palette distribution.
async fn effective_members(state: &AppState, room_id: &str) -> Vec<MemberRow> {
    sqlx::query(
        "SELECT DISTINCT l.id AS light_id, l.device_id, l.provider_id,
                p.provider_type, p.credentials, l.name
         FROM lights l
         JOIN providers p ON p.id = l.provider_id
         WHERE p.enabled = 1 AND l.id IN (
             SELECT light_id FROM room_lights WHERE room_id = ?1
             UNION
             SELECT pgl.light_id
             FROM room_links rl
             JOIN provider_group_lights pgl ON pgl.provider_group_id = rl.provider_group_id
             WHERE rl.room_id = ?1
         )
         ORDER BY l.name",
    )
    .bind(room_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| MemberRow {
        light_id: r.get("light_id"),
        device_id: r.get("device_id"),
        provider_type: r.get("provider_type"),
        credentials: r.get("credentials"),
    })
    .collect()
}

async fn effective_member_ids(state: &AppState, room_id: &str) -> Vec<String> {
    effective_members(state, room_id)
        .await
        .into_iter()
        .map(|m| m.light_id)
        .collect()
}

// ── Handlers ────────────────────────────────────────────────────────────────

async fn list_rooms(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let rooms = match sqlx::query("SELECT id, name FROM rooms ORDER BY created_at")
        .fetch_all(&state.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("db error listing rooms: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let mut out = Vec::with_capacity(rooms.len());
    for room in rooms {
        let id: String = room.get("id");

        let direct: Vec<String> = sqlx::query("SELECT light_id FROM room_lights WHERE room_id = ?")
            .bind(&id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| r.get("light_id"))
            .collect();

        let links: Vec<LinkInfo> = sqlx::query(
            "SELECT pg.id, pg.name, pg.provider_id
             FROM room_links rl JOIN provider_groups pg ON pg.id = rl.provider_group_id
             WHERE rl.room_id = ?",
        )
        .bind(&id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| LinkInfo {
            provider_group_id: r.get("id"),
            name: r.get("name"),
            provider_id: r.get("provider_id"),
        })
        .collect();

        out.push(RoomInfo {
            light_ids: effective_member_ids(&state, &id).await,
            direct_light_ids: direct,
            links,
            id,
            name: room.get("name"),
        });
    }

    Json(out).into_response()
}

async fn list_provider_groups(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let rows = sqlx::query(
        "SELECT id, provider_id, provider_group_id, name FROM provider_groups ORDER BY name",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let id: String = r.get("id");
        let light_ids =
            sqlx::query("SELECT light_id FROM provider_group_lights WHERE provider_group_id = ?")
                .bind(&id)
                .fetch_all(&state.db)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|l| l.get("light_id"))
                .collect();
        out.push(ProviderGroupInfo {
            provider_id: r.get("provider_id"),
            provider_group_id: r.get("provider_group_id"),
            name: r.get("name"),
            light_ids,
            id,
        });
    }

    Json(out).into_response()
}

#[derive(Deserialize)]
struct CreateRoomRequest {
    name: String,
    #[serde(default)]
    light_ids: Vec<String>,
}

async fn create_room(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateRoomRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if req.name.trim().is_empty() {
        return (StatusCode::UNPROCESSABLE_ENTITY, "room name is required").into_response();
    }

    // Case-insensitive duplicate guard: "office" next to "Office" is how
    // unmergeable near-duplicates were born.
    let duplicate = sqlx::query("SELECT 1 FROM rooms WHERE name = ? COLLATE NOCASE")
        .bind(req.name.trim())
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .is_some();
    if duplicate {
        return (
            StatusCode::CONFLICT,
            "a room with this name already exists",
        )
            .into_response();
    }

    let id = Uuid::new_v4().to_string();
    if let Err(e) = sqlx::query("INSERT INTO rooms (id, name) VALUES (?, ?)")
        .bind(&id)
        .bind(req.name.trim())
        .execute(&state.db)
        .await
    {
        tracing::error!("db error: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    for light_id in &req.light_ids {
        let _ = sqlx::query("INSERT OR IGNORE INTO room_lights (room_id, light_id) VALUES (?, ?)")
            .bind(&id)
            .bind(light_id)
            .execute(&state.db)
            .await;
    }

    (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response()
}

async fn remove_room(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let _ = sqlx::query("DELETE FROM rooms WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct MergeRequest {
    /// The room to absorb. Its links, direct lights, scenes, and plan-region
    /// bindings move to `{id}` (the target), then it is deleted.
    source_room_id: String,
}

/// Merge `source_room_id` into the target room. The target keeps its own
/// name; everything the source owned is re-pointed (memberships dedupe via
/// INSERT OR IGNORE).
async fn merge_rooms(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<MergeRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if req.source_room_id == id {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "cannot merge a room into itself",
        )
            .into_response();
    }
    if !room_exists(&state, &id).await || !room_exists(&state, &req.source_room_id).await {
        return StatusCode::NOT_FOUND.into_response();
    }

    // Links and direct lights: copy with dedupe, then let the source's
    // rows die with the room (ON DELETE CASCADE).
    let _ = sqlx::query(
        "INSERT OR IGNORE INTO room_links (room_id, provider_group_id)
         SELECT ?, provider_group_id FROM room_links WHERE room_id = ?",
    )
    .bind(&id)
    .bind(&req.source_room_id)
    .execute(&state.db)
    .await;
    let _ = sqlx::query(
        "INSERT OR IGNORE INTO room_lights (room_id, light_id)
         SELECT ?, light_id FROM room_lights WHERE room_id = ?",
    )
    .bind(&id)
    .bind(&req.source_room_id)
    .execute(&state.db)
    .await;

    // Scenes and plan-region bindings move wholesale.
    let _ = sqlx::query("UPDATE room_scenes SET room_id = ? WHERE room_id = ?")
        .bind(&id)
        .bind(&req.source_room_id)
        .execute(&state.db)
        .await;
    let _ = sqlx::query("UPDATE plan_rooms SET room_id = ? WHERE room_id = ?")
        .bind(&id)
        .bind(&req.source_room_id)
        .execute(&state.db)
        .await;

    let _ = sqlx::query("DELETE FROM rooms WHERE id = ?")
        .bind(&req.source_room_id)
        .execute(&state.db)
        .await;

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct SetLightsRequest {
    light_ids: Vec<String>,
}

/// Replace the room's DIRECT lights (linked members are unaffected).
async fn set_direct_lights(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<SetLightsRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !room_exists(&state, &id).await {
        return StatusCode::NOT_FOUND.into_response();
    }

    let _ = sqlx::query("DELETE FROM room_lights WHERE room_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;
    for light_id in &req.light_ids {
        let _ = sqlx::query("INSERT OR IGNORE INTO room_lights (room_id, light_id) VALUES (?, ?)")
            .bind(&id)
            .bind(light_id)
            .execute(&state.db)
            .await;
    }

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct SetLinksRequest {
    provider_group_ids: Vec<String>,
}

/// Replace the room's provider-group links.
async fn set_links(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<SetLinksRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !room_exists(&state, &id).await {
        return StatusCode::NOT_FOUND.into_response();
    }

    let _ = sqlx::query("DELETE FROM room_links WHERE room_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;
    for pg_id in &req.provider_group_ids {
        let _ = sqlx::query(
            "INSERT OR IGNORE INTO room_links (room_id, provider_group_id) VALUES (?, ?)",
        )
        .bind(&id)
        .bind(pg_id)
        .execute(&state.db)
        .await;
    }

    StatusCode::NO_CONTENT.into_response()
}

async fn room_exists(state: &AppState, id: &str) -> bool {
    sqlx::query("SELECT 1 FROM rooms WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .is_some()
}

// ── Room state (native group control where possible) ────────────────────────

/// A provider's portion of the room that can be driven with one native call.
struct NativeChunk {
    provider_type: String,
    credentials: String,
    grouped_ref: String,
    light_ids: Vec<String>,
}

/// For each provider: if the room's members from that provider all come from
/// exactly one linked group with a native handle (and no direct lights of
/// that provider), control natively. Returns the chunks plus the light IDs
/// they cover.
async fn native_chunks(state: &AppState, room_id: &str) -> Vec<NativeChunk> {
    let links = sqlx::query(
        "SELECT pg.id, pg.provider_id, pg.grouped_ref, p.provider_type, p.credentials
         FROM room_links rl
         JOIN provider_groups pg ON pg.id = rl.provider_group_id
         JOIN providers p ON p.id = pg.provider_id
         WHERE rl.room_id = ? AND p.enabled = 1",
    )
    .bind(room_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    // provider_id → links of that provider
    let mut by_provider: HashMap<String, Vec<&sqlx::sqlite::SqliteRow>> = HashMap::new();
    for l in &links {
        by_provider
            .entry(l.get::<String, _>("provider_id"))
            .or_default()
            .push(l);
    }

    let mut chunks = Vec::new();
    for (provider_id, provider_links) in by_provider {
        if provider_links.len() != 1 {
            continue; // multiple linked groups → fan out, simplest correct path
        }
        let link = provider_links[0];
        let Some(grouped_ref) = link.get::<Option<String>, _>("grouped_ref") else {
            continue;
        };

        // Any direct lights from this provider? Then the native call would
        // miss them — fan out instead.
        let direct_count: i64 = sqlx::query(
            "SELECT COUNT(*) AS n FROM room_lights rl
             JOIN lights l ON l.id = rl.light_id
             WHERE rl.room_id = ? AND l.provider_id = ?",
        )
        .bind(room_id)
        .bind(&provider_id)
        .fetch_one(&state.db)
        .await
        .map(|r| r.get("n"))
        .unwrap_or(0);
        if direct_count > 0 {
            continue;
        }

        let light_ids: Vec<String> =
            sqlx::query("SELECT light_id FROM provider_group_lights WHERE provider_group_id = ?")
                .bind(link.get::<String, _>("id"))
                .fetch_all(&state.db)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|r| r.get("light_id"))
                .collect();

        chunks.push(NativeChunk {
            provider_type: link.get("provider_type"),
            credentials: link.get("credentials"),
            grouped_ref,
            light_ids,
        });
    }
    chunks
}

async fn set_room_state(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(new_state): Json<LightState>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let members = effective_members(&state, &id).await;
    if members.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut applied = 0usize;
    let mut failed = 0usize;
    let state_json = serde_json::to_string(&new_state).unwrap_or_default();

    // Native group calls first.
    for chunk in native_chunks(&state, &id).await {
        let provider = match build_provider(&state, &chunk.provider_type, &chunk.credentials) {
            Ok(p) => p,
            Err(_) => continue,
        };
        match provider
            .set_group_state(&chunk.grouped_ref, &new_state)
            .await
        {
            Ok(true) => {
                for light_id in &chunk.light_ids {
                    covered.insert(light_id.clone());
                    let _ = sqlx::query(
                        "UPDATE lights SET last_state = ?, last_seen = datetime('now') WHERE id = ?",
                    )
                    .bind(&state_json)
                    .bind(light_id)
                    .execute(&state.db)
                    .await;
                }
                applied += chunk.light_ids.len();
            }
            Ok(false) => {} // provider has no native control — fan out below
            Err(e) => {
                tracing::warn!("native group control failed, falling back per-light: {e:#}");
            }
        }
    }

    // Per-light fan-out for everything not covered natively.
    let mut jobs = Vec::new();
    for m in members {
        if covered.contains(&m.light_id) {
            continue;
        }
        let provider = match build_provider(&state, &m.provider_type, &m.credentials) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("room state: provider build failed: {e:#}");
                failed += 1;
                continue;
            }
        };
        let db = state.db.clone();
        let target = new_state.clone();
        let target_json = state_json.clone();
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
                    tracing::error!("room state: set_state failed for {}: {e:#}", m.device_id);
                    false
                }
            }
        });
    }
    let results = futures_util::future::join_all(jobs).await;
    applied += results.iter().filter(|ok| **ok).count();
    failed += results.iter().filter(|ok| !**ok).count();

    Json(serde_json::json!({ "applied": applied, "failed": failed })).into_response()
}

// ── Room scenes (Hue-like palette scenes) ───────────────────────────────────

#[derive(Serialize)]
struct RoomScene {
    id: String,
    room_id: String,
    name: String,
    brightness: Option<f32>,
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
        "SELECT id, room_id, name, brightness, palette FROM room_scenes
         WHERE room_id = ? ORDER BY created_at",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|r| RoomScene {
                    id: r.get("id"),
                    room_id: r.get("room_id"),
                    name: r.get("name"),
                    brightness: r.get("brightness"),
                    palette: serde_json::from_str(&r.get::<String, _>("palette"))
                        .unwrap_or_default(),
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => {
            tracing::error!("db error listing room scenes: {e}");
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
                format!("'{c}' is not a #rrggbb color"),
            )
                .into_response();
        }
    }
    if !room_exists(&state, &id).await {
        return StatusCode::NOT_FOUND.into_response();
    }

    let scene_id = Uuid::new_v4().to_string();
    let palette_json = serde_json::to_string(&req.palette).unwrap_or_else(|_| "[]".into());
    match sqlx::query(
        "INSERT INTO room_scenes (id, room_id, name, brightness, palette) VALUES (?, ?, ?, ?, ?)",
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
    Path((room_id, scene_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let _ = sqlx::query("DELETE FROM room_scenes WHERE id = ? AND room_id = ?")
        .bind(&scene_id)
        .bind(&room_id)
        .execute(&state.db)
        .await;

    StatusCode::NO_CONTENT.into_response()
}

/// Apply the scene: members on, palette distributed round-robin in stable
/// (name) order, brightness set.
async fn apply_scene(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((room_id, scene_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let scene = match sqlx::query(
        "SELECT brightness, palette FROM room_scenes WHERE id = ? AND room_id = ?",
    )
    .bind(&scene_id)
    .bind(&room_id)
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

    let members = effective_members(&state, &room_id).await;
    if members.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let mut jobs = Vec::new();
    for (i, m) in members.into_iter().enumerate() {
        let provider = match build_provider(&state, &m.provider_type, &m.credentials) {
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
