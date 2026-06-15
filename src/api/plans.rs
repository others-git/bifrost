//! Floor plans: 2D tile grids with edge walls and mounted lights.
//!
//! Walls are segments on tile boundaries ('h' = top edge of tile (x,y),
//! 'v' = left edge). Lights attach to one of five mount points per tile:
//! centre or an edge. Several lights may share a mount (cluster).

use crate::AppState;
use crate::api::auth::Session;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, put},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

const MAX_DIM: i64 = 128;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_plans).post(create_plan))
        .route("/{id}", get(get_plan).delete(remove_plan))
        .route("/{id}/layout", put(set_layout))
        .route("/{id}/lights", put(set_lights))
        .route("/{id}/audio", put(set_audio_placements))
        .route("/{id}/rooms", put(set_rooms))
        .route("/{id}/size", put(set_size))
}

// ── Wire types ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct PlanSummary {
    id: String,
    name: String,
    width: i64,
    height: i64,
    lights: i64,
    created_at: String,
}

#[derive(Serialize, Deserialize)]
struct Wall {
    x: i64,
    y: i64,
    dir: WallDir,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
enum WallDir {
    H,
    V,
}

impl WallDir {
    fn as_str(self) -> &'static str {
        match self {
            Self::H => "h",
            Self::V => "v",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Mount {
    C,
    N,
    S,
    E,
    W,
}

impl Mount {
    fn as_str(self) -> &'static str {
        match self {
            Self::C => "c",
            Self::N => "n",
            Self::S => "s",
            Self::E => "e",
            Self::W => "w",
        }
    }

    fn from_db(s: &str) -> Self {
        match s {
            "n" => Self::N,
            "s" => Self::S,
            "e" => Self::E,
            "w" => Self::W,
            _ => Self::C,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Placement {
    light_id: String,
    x: i64,
    y: i64,
    mount: Mount,
    /// Vertices an LED strip passes through after its start tile — straight
    /// runs have one, cornered runs more. None for a point light.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    points: Option<Vec<[i64; 2]>>,
}

/// An audio device placed on the plan. Point-only — no LED-strip `points`.
#[derive(Serialize, Deserialize)]
struct AudioPlacement {
    audio_device_id: String,
    x: i64,
    y: i64,
    mount: Mount,
}

#[derive(Serialize, Deserialize)]
struct Room {
    id: String,
    name: String,
    /// The Bifrost Room this region is bound to. Read-only for clients;
    /// assigned server-side (a new Room is created for new plan rooms).
    #[serde(default)]
    room_id: Option<String>,
    tiles: Vec<[i64; 2]>,
}

#[derive(Serialize)]
struct PlanDetail {
    id: String,
    name: String,
    width: i64,
    height: i64,
    /// Floor tiles as [x, y] pairs (sparse).
    tiles: Vec<[i64; 2]>,
    walls: Vec<Wall>,
    lights: Vec<Placement>,
    audio: Vec<AudioPlacement>,
    rooms: Vec<Room>,
}

// ── Handlers ────────────────────────────────────────────────────────────────

async fn list_plans(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    match sqlx::query(
        "SELECT p.id, p.name, p.width, p.height, p.created_at, COUNT(l.light_id) AS lights
         FROM floor_plans p LEFT JOIN plan_lights l ON l.plan_id = p.id
         GROUP BY p.id ORDER BY p.created_at",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|r| PlanSummary {
                    id: r.get("id"),
                    name: r.get("name"),
                    width: r.get("width"),
                    height: r.get("height"),
                    lights: r.get("lights"),
                    created_at: r.get("created_at"),
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => {
            tracing::error!("db error listing plans: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
struct CreatePlanRequest {
    name: String,
    width: i64,
    height: i64,
}

async fn create_plan(
    State(state): State<Arc<AppState>>,
    _: Session,
    Json(req): Json<CreatePlanRequest>,
) -> impl IntoResponse {
    if req.name.trim().is_empty() {
        return (StatusCode::UNPROCESSABLE_ENTITY, "plan name is required").into_response();
    }
    if !(1..=MAX_DIM).contains(&req.width) || !(1..=MAX_DIM).contains(&req.height) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("width and height must be between 1 and {MAX_DIM}"),
        )
            .into_response();
    }

    let id = Uuid::new_v4().to_string();
    match sqlx::query("INSERT INTO floor_plans (id, name, width, height) VALUES (?, ?, ?, ?)")
        .bind(&id)
        .bind(req.name.trim())
        .bind(req.width)
        .bind(req.height)
        .execute(&state.db)
        .await
    {
        Ok(_) => (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response(),
        Err(e) => {
            tracing::error!("db error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn get_plan(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let plan = match sqlx::query("SELECT id, name, width, height FROM floor_plans WHERE id = ?")
        .bind(&id)
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

    let tiles = sqlx::query("SELECT x, y FROM plan_tiles WHERE plan_id = ?")
        .bind(&id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| [r.get::<i64, _>("x"), r.get::<i64, _>("y")])
        .collect();

    let walls = sqlx::query("SELECT x, y, dir FROM plan_walls WHERE plan_id = ?")
        .bind(&id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| Wall {
            x: r.get("x"),
            y: r.get("y"),
            dir: if r.get::<String, _>("dir") == "h" {
                WallDir::H
            } else {
                WallDir::V
            },
        })
        .collect();

    let lights =
        sqlx::query("SELECT light_id, x, y, mount, points FROM plan_lights WHERE plan_id = ?")
            .bind(&id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| Placement {
                light_id: r.get("light_id"),
                x: r.get("x"),
                y: r.get("y"),
                mount: Mount::from_db(&r.get::<String, _>("mount")),
                points: r
                    .get::<Option<String>, _>("points")
                    .and_then(|j| serde_json::from_str(&j).ok()),
            })
            .collect();

    let audio = sqlx::query(
        "SELECT audio_device_id, x, y, mount FROM plan_audio_devices WHERE plan_id = ?",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| AudioPlacement {
        audio_device_id: r.get("audio_device_id"),
        x: r.get("x"),
        y: r.get("y"),
        mount: Mount::from_db(&r.get::<String, _>("mount")),
    })
    .collect();

    let rooms = load_rooms(&state, &id).await;

    Json(PlanDetail {
        id: plan.get("id"),
        name: plan.get("name"),
        width: plan.get("width"),
        height: plan.get("height"),
        tiles,
        walls,
        lights,
        audio,
        rooms,
    })
    .into_response()
}

async fn load_rooms(state: &AppState, plan_id: &str) -> Vec<Room> {
    let rows = sqlx::query("SELECT id, name, room_id FROM plan_rooms WHERE plan_id = ?")
        .bind(plan_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let mut rooms = Vec::with_capacity(rows.len());
    for r in rows {
        let plan_room_id: String = r.get("id");
        let tiles = sqlx::query("SELECT x, y FROM plan_room_tiles WHERE room_id = ?")
            .bind(&plan_room_id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|t| [t.get::<i64, _>("x"), t.get::<i64, _>("y")])
            .collect();
        rooms.push(Room {
            id: plan_room_id,
            name: r.get("name"),
            room_id: r.get("room_id"),
            tiles,
        });
    }
    rooms
}

async fn remove_plan(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let _ = sqlx::query("DELETE FROM floor_plans WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct SetLayoutRequest {
    tiles: Vec<[i64; 2]>,
    walls: Vec<Wall>,
}

/// Replace the plan's full layout (tiles + walls) — one editor save.
async fn set_layout(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetLayoutRequest>,
) -> impl IntoResponse {
    let Some((width, height)) = plan_dims(&state, &id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    if let Some((x, y)) = first_out_of_bounds(&req, width, height) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("coordinate ({x}, {y}) is outside the {width}x{height} grid"),
        )
            .into_response();
    }

    // One transaction for the whole replace: the deletes + every tile/wall
    // insert commit together with a single fsync, instead of one fsync per row.
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!("set_layout: begin failed: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let _ = sqlx::query("DELETE FROM plan_tiles WHERE plan_id = ?")
        .bind(&id)
        .execute(&mut *tx)
        .await;
    let _ = sqlx::query("DELETE FROM plan_walls WHERE plan_id = ?")
        .bind(&id)
        .execute(&mut *tx)
        .await;

    for [x, y] in &req.tiles {
        let _ = sqlx::query("INSERT OR IGNORE INTO plan_tiles (plan_id, x, y) VALUES (?, ?, ?)")
            .bind(&id)
            .bind(x)
            .bind(y)
            .execute(&mut *tx)
            .await;
    }
    for w in &req.walls {
        let _ = sqlx::query(
            "INSERT OR IGNORE INTO plan_walls (plan_id, x, y, dir) VALUES (?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(w.x)
        .bind(w.y)
        .bind(w.dir.as_str())
        .execute(&mut *tx)
        .await;
    }

    if let Err(e) = tx.commit().await {
        tracing::error!("set_layout: commit failed: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

fn first_out_of_bounds(req: &SetLayoutRequest, width: i64, height: i64) -> Option<(i64, i64)> {
    for &[x, y] in &req.tiles {
        if !(0..width).contains(&x) || !(0..height).contains(&y) {
            return Some((x, y));
        }
    }
    for w in &req.walls {
        // Wall coordinates may reach the far boundary (x == width for 'v',
        // y == height for 'h').
        let x_max = if w.dir == WallDir::V {
            width
        } else {
            width - 1
        };
        let y_max = if w.dir == WallDir::H {
            height
        } else {
            height - 1
        };
        if !(0..=x_max).contains(&w.x) || !(0..=y_max).contains(&w.y) {
            return Some((w.x, w.y));
        }
    }
    None
}

#[derive(Deserialize)]
struct SetSizeRequest {
    width: i64,
    height: i64,
}

/// Resize the plan grid. Content that falls outside the new bounds (tiles,
/// walls, light placements, room-region tiles) is pruned.
async fn set_size(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetSizeRequest>,
) -> impl IntoResponse {
    if !(1..=MAX_DIM).contains(&req.width) || !(1..=MAX_DIM).contains(&req.height) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("width and height must be between 1 and {MAX_DIM}"),
        )
            .into_response();
    }
    if plan_dims(&state, &id).await.is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let _ = sqlx::query("UPDATE floor_plans SET width = ?, height = ? WHERE id = ?")
        .bind(req.width)
        .bind(req.height)
        .bind(&id)
        .execute(&state.db)
        .await;

    let _ = sqlx::query("DELETE FROM plan_tiles WHERE plan_id = ? AND (x >= ? OR y >= ?)")
        .bind(&id)
        .bind(req.width)
        .bind(req.height)
        .execute(&state.db)
        .await;
    // Wall coordinates may reach the far boundary (x == width for 'v',
    // y == height for 'h') — same rule as layout validation.
    let _ = sqlx::query(
        "DELETE FROM plan_walls WHERE plan_id = ? AND (
            (dir = 'h' AND (x >= ? OR y > ?)) OR
            (dir = 'v' AND (x > ? OR y >= ?))
         )",
    )
    .bind(&id)
    .bind(req.width)
    .bind(req.height)
    .bind(req.width)
    .bind(req.height)
    .execute(&state.db)
    .await;
    // Any vertex outside removes the whole placement (strips included).
    let _ = sqlx::query(
        "DELETE FROM plan_lights WHERE plan_id = ?1 AND (
            x >= ?2 OR y >= ?3 OR (
                points IS NOT NULL AND EXISTS (
                    SELECT 1 FROM json_each(plan_lights.points)
                    WHERE json_extract(json_each.value, '$[0]') >= ?2
                       OR json_extract(json_each.value, '$[1]') >= ?3
                )
            )
         )",
    )
    .bind(&id)
    .bind(req.width)
    .bind(req.height)
    .execute(&state.db)
    .await;
    let _ = sqlx::query("DELETE FROM plan_audio_devices WHERE plan_id = ? AND (x >= ? OR y >= ?)")
        .bind(&id)
        .bind(req.width)
        .bind(req.height)
        .execute(&state.db)
        .await;
    let _ = sqlx::query(
        "DELETE FROM plan_room_tiles WHERE room_id IN (SELECT id FROM plan_rooms WHERE plan_id = ?)
         AND (x >= ? OR y >= ?)",
    )
    .bind(&id)
    .bind(req.width)
    .bind(req.height)
    .execute(&state.db)
    .await;

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct SetLightsRequest {
    placements: Vec<Placement>,
}

/// Replace all light placements on the plan.
async fn set_lights(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetLightsRequest>,
) -> impl IntoResponse {
    let Some((width, height)) = plan_dims(&state, &id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    for p in &req.placements {
        let mut vertices = vec![[p.x, p.y]];
        if let Some(points) = &p.points {
            vertices.extend(points.iter().copied());
        }
        for [x, y] in vertices {
            if !(0..width).contains(&x) || !(0..height).contains(&y) {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("placement ({x}, {y}) is outside the {width}x{height} grid"),
                )
                    .into_response();
            }
        }
        let known = sqlx::query("SELECT 1 FROM lights WHERE id = ?")
            .bind(&p.light_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .is_some();
        if !known {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("unknown light '{}'", p.light_id),
            )
                .into_response();
        }
    }

    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!("set_lights: begin failed: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let _ = sqlx::query("DELETE FROM plan_lights WHERE plan_id = ?")
        .bind(&id)
        .execute(&mut *tx)
        .await;

    for p in &req.placements {
        let points_json = p
            .points
            .as_ref()
            .filter(|pts| !pts.is_empty())
            .map(|pts| serde_json::to_string(pts).unwrap_or_else(|_| "[]".into()));
        let _ = sqlx::query(
            "INSERT OR REPLACE INTO plan_lights (plan_id, light_id, x, y, mount, points) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&p.light_id)
        .bind(p.x)
        .bind(p.y)
        .bind(p.mount.as_str())
        .bind(points_json)
        .execute(&mut *tx)
        .await;
    }

    if let Err(e) = tx.commit().await {
        tracing::error!("set_lights: commit failed: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // Add-on-save: newly placed lights join their region's Room.
    add_placed_lights_to_rooms(&state, &id).await;

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct SetAudioRequest {
    placements: Vec<AudioPlacement>,
}

/// Replace all audio-device placements on the plan (point placements only).
async fn set_audio_placements(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetAudioRequest>,
) -> impl IntoResponse {
    let Some((width, height)) = plan_dims(&state, &id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    for p in &req.placements {
        if !(0..width).contains(&p.x) || !(0..height).contains(&p.y) {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!(
                    "placement ({}, {}) is outside the {width}x{height} grid",
                    p.x, p.y
                ),
            )
                .into_response();
        }
        let known = sqlx::query("SELECT 1 FROM audio_devices WHERE id = ?")
            .bind(&p.audio_device_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .is_some();
        if !known {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("unknown audio device '{}'", p.audio_device_id),
            )
                .into_response();
        }
    }

    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!("set_audio_placements: begin failed: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let _ = sqlx::query("DELETE FROM plan_audio_devices WHERE plan_id = ?")
        .bind(&id)
        .execute(&mut *tx)
        .await;

    for p in &req.placements {
        let _ = sqlx::query(
            "INSERT OR REPLACE INTO plan_audio_devices (plan_id, audio_device_id, x, y, mount) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&p.audio_device_id)
        .bind(p.x)
        .bind(p.y)
        .bind(p.mount.as_str())
        .execute(&mut *tx)
        .await;
    }

    if let Err(e) = tx.commit().await {
        tracing::error!("set_audio_placements: commit failed: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct SetRoomsRequest {
    rooms: Vec<Room>,
}

/// Replace the plan's room regions. Regions bind to Bifrost Rooms:
/// - a new region creates a Room (and binds it)
/// - renaming a region renames its Room (and clears `inherited_name` — the
///   user took naming ownership, so provider rename-follow stops)
/// - removing a region does NOT delete the Room (rooms are first-class;
///   delete them in Settings)
///
/// Lights placed inside a region are ADDED to its Room on save (never removed).
async fn set_rooms(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetRoomsRequest>,
) -> impl IntoResponse {
    let Some((width, height)) = plan_dims(&state, &id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    for room in &req.rooms {
        if room.name.trim().is_empty() {
            return (StatusCode::UNPROCESSABLE_ENTITY, "room name is required").into_response();
        }
        for &[x, y] in &room.tiles {
            if !(0..width).contains(&x) || !(0..height).contains(&y) {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("room tile ({x}, {y}) is outside the {width}x{height} grid"),
                )
                    .into_response();
            }
        }
    }

    // Keep Room bindings for plan rooms that survive this save.
    let existing = load_rooms(&state, &id).await;
    let existing_bindings: std::collections::HashMap<String, (String, Option<String>)> = existing
        .into_iter()
        .map(|r| (r.id.clone(), (r.name, r.room_id)))
        .collect();

    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!("set_rooms: begin failed: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let _ = sqlx::query("DELETE FROM plan_rooms WHERE plan_id = ?")
        .bind(&id)
        .execute(&mut *tx)
        .await;

    for room in &req.rooms {
        let plan_room_id = if room.id.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            room.id.clone()
        };

        let room_id = match existing_bindings.get(&plan_room_id) {
            Some((old_name, Some(rid))) => {
                // Region renamed → rename the Room and stop rename-follow.
                if old_name != room.name.trim() {
                    let _ = sqlx::query(
                        "UPDATE rooms SET name = ?, inherited_name = NULL WHERE id = ?",
                    )
                    .bind(room.name.trim())
                    .bind(rid)
                    .execute(&mut *tx)
                    .await;
                }
                Some(rid.clone())
            }
            _ => {
                // New region (or binding lost) → bind to the same-named Room
                // if one exists (case-insensitive), else create one. Painting
                // an "office" region must not duplicate the synced "Office".
                let existing_room =
                    sqlx::query("SELECT id FROM rooms WHERE name = ? COLLATE NOCASE")
                        .bind(room.name.trim())
                        .fetch_optional(&mut *tx)
                        .await
                        .ok()
                        .flatten()
                        .map(|r| r.get::<String, _>("id"));
                match existing_room {
                    Some(rid) => Some(rid),
                    None => {
                        let rid = Uuid::new_v4().to_string();
                        let _ = sqlx::query("INSERT INTO rooms (id, name) VALUES (?, ?)")
                            .bind(&rid)
                            .bind(room.name.trim())
                            .execute(&mut *tx)
                            .await;
                        Some(rid)
                    }
                }
            }
        };

        let _ =
            sqlx::query("INSERT INTO plan_rooms (id, plan_id, name, room_id) VALUES (?, ?, ?, ?)")
                .bind(&plan_room_id)
                .bind(&id)
                .bind(room.name.trim())
                .bind(&room_id)
                .execute(&mut *tx)
                .await;
        for &[x, y] in &room.tiles {
            let _ = sqlx::query(
                "INSERT OR IGNORE INTO plan_room_tiles (room_id, x, y) VALUES (?, ?, ?)",
            )
            .bind(&plan_room_id)
            .bind(x)
            .bind(y)
            .execute(&mut *tx)
            .await;
        }
    }

    if let Err(e) = tx.commit().await {
        tracing::error!("set_rooms: commit failed: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    add_placed_lights_to_rooms(&state, &id).await;

    StatusCode::NO_CONTENT.into_response()
}

/// Add-on-save: every light placed inside a bound region becomes a DIRECT
/// member of that region's Room, additively — saving never removes members
/// (manage membership in Settings).
pub(crate) async fn add_placed_lights_to_rooms(state: &AppState, plan_id: &str) {
    let rooms = load_rooms(state, plan_id).await;
    let placements = sqlx::query("SELECT light_id, x, y FROM plan_lights WHERE plan_id = ?")
        .bind(plan_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    for room in rooms {
        let Some(room_id) = &room.room_id else {
            continue;
        };
        let tile_set: std::collections::HashSet<(i64, i64)> =
            room.tiles.iter().map(|t| (t[0], t[1])).collect();

        for p in &placements {
            if !tile_set.contains(&(p.get::<i64, _>("x"), p.get::<i64, _>("y"))) {
                continue;
            }
            let light_id: String = p.get("light_id");

            // Skip lights already effective members via a provider-group link.
            let via_link = sqlx::query(
                "SELECT 1 FROM room_links rl
                 JOIN provider_group_lights pgl ON pgl.provider_group_id = rl.provider_group_id
                 WHERE rl.room_id = ? AND pgl.light_id = ?",
            )
            .bind(room_id)
            .bind(&light_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .is_some();
            if via_link {
                continue;
            }

            let _ =
                sqlx::query("INSERT OR IGNORE INTO room_lights (room_id, light_id) VALUES (?, ?)")
                    .bind(room_id)
                    .bind(&light_id)
                    .execute(&state.db)
                    .await;
        }
    }
}

async fn plan_dims(state: &AppState, id: &str) -> Option<(i64, i64)> {
    sqlx::query("SELECT width, height FROM floor_plans WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .map(|r| (r.get("width"), r.get("height")))
}
