//! Floor plans: 2D tile grids with edge walls and mounted lights.
//!
//! Walls are segments on tile boundaries ('h' = top edge of tile (x,y),
//! 'v' = left edge). Lights attach to one of five mount points per tile:
//! centre or an edge. Several lights may share a mount (cluster).

use crate::AppState;
use crate::api::auth::require_session;
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
use uuid::Uuid;

const MAX_DIM: i64 = 128;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_plans).post(create_plan))
        .route("/{id}", get(get_plan).delete(remove_plan))
        .route("/{id}/layout", put(set_layout))
        .route("/{id}/lights", put(set_lights))
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
}

#[derive(Serialize, Deserialize)]
struct Placement {
    light_id: String,
    x: i64,
    y: i64,
    mount: Mount,
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
}

// ── Handlers ────────────────────────────────────────────────────────────────

async fn list_plans(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

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
    headers: HeaderMap,
    Json(req): Json<CreatePlanRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
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
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

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

    let lights = sqlx::query("SELECT light_id, x, y, mount FROM plan_lights WHERE plan_id = ?")
        .bind(&id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| Placement {
            light_id: r.get("light_id"),
            x: r.get("x"),
            y: r.get("y"),
            mount: match r.get::<String, _>("mount").as_str() {
                "n" => Mount::N,
                "s" => Mount::S,
                "e" => Mount::E,
                "w" => Mount::W,
                _ => Mount::C,
            },
        })
        .collect();

    Json(PlanDetail {
        id: plan.get("id"),
        name: plan.get("name"),
        width: plan.get("width"),
        height: plan.get("height"),
        tiles,
        walls,
        lights,
    })
    .into_response()
}

async fn remove_plan(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

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
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<SetLayoutRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

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

    let _ = sqlx::query("DELETE FROM plan_tiles WHERE plan_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;
    let _ = sqlx::query("DELETE FROM plan_walls WHERE plan_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;

    for [x, y] in &req.tiles {
        let _ = sqlx::query("INSERT OR IGNORE INTO plan_tiles (plan_id, x, y) VALUES (?, ?, ?)")
            .bind(&id)
            .bind(x)
            .bind(y)
            .execute(&state.db)
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
        .execute(&state.db)
        .await;
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
struct SetLightsRequest {
    placements: Vec<Placement>,
}

/// Replace all light placements on the plan.
async fn set_lights(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<SetLightsRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

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

    let _ = sqlx::query("DELETE FROM plan_lights WHERE plan_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;

    for p in &req.placements {
        let _ = sqlx::query(
            "INSERT OR REPLACE INTO plan_lights (plan_id, light_id, x, y, mount) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&p.light_id)
        .bind(p.x)
        .bind(p.y)
        .bind(p.mount.as_str())
        .execute(&state.db)
        .await;
    }

    StatusCode::NO_CONTENT.into_response()
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
