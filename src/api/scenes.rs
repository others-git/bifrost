//! Scenes: named snapshots of light states, applied all at once.

use crate::AppState;
use crate::api::auth::require_session;
use crate::api::lights::build_provider;
use crate::models::LightState;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_scenes).post(create_scene))
        .route("/{id}", delete(remove_scene))
        .route("/{id}/activate", post(activate_scene))
}

#[derive(Serialize)]
struct SceneRow {
    id: String,
    name: String,
    created_at: String,
    lights: i64,
}

async fn list_scenes(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    match sqlx::query(
        "SELECT s.id, s.name, s.created_at, COUNT(e.light_id) AS lights
         FROM scenes s LEFT JOIN scene_entries e ON e.scene_id = s.id
         GROUP BY s.id ORDER BY s.created_at",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|r| SceneRow {
                    id: r.get("id"),
                    name: r.get("name"),
                    created_at: r.get("created_at"),
                    lights: r.get("lights"),
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => {
            tracing::error!("db error listing scenes: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
struct CreateSceneRequest {
    name: String,
}

/// Snapshot the current `last_state` of every light into a new scene.
async fn create_scene(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateSceneRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if req.name.trim().is_empty() {
        return (StatusCode::UNPROCESSABLE_ENTITY, "scene name is required").into_response();
    }

    let lights = match sqlx::query("SELECT id, last_state FROM lights WHERE last_state IS NOT NULL")
        .fetch_all(&state.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("db error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let scene_id = Uuid::new_v4().to_string();
    if let Err(e) = sqlx::query("INSERT INTO scenes (id, name) VALUES (?, ?)")
        .bind(&scene_id)
        .bind(req.name.trim())
        .execute(&state.db)
        .await
    {
        tracing::error!("db error: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let mut captured = 0usize;
    for row in &lights {
        let light_id: String = row.get("id");
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

    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "id": scene_id, "lights": captured })),
    )
        .into_response()
}

async fn remove_scene(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let _ = sqlx::query("DELETE FROM scenes WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;

    StatusCode::NO_CONTENT.into_response()
}

/// Apply every entry in the scene, in parallel, via each light's provider.
async fn activate_scene(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let entries = match sqlx::query(
        "SELECT e.light_id, e.state, l.device_id, p.provider_type, p.credentials
         FROM scene_entries e
         JOIN lights l ON l.id = e.light_id
         JOIN providers p ON p.id = l.provider_id
         WHERE e.scene_id = ? AND p.enabled = 1",
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

    if entries.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let mut jobs = Vec::new();
    for row in entries {
        let light_id: String = row.get("light_id");
        let device_id: String = row.get("device_id");
        let provider_type: String = row.get("provider_type");
        let credentials_enc: String = row.get("credentials");
        let state_json: String = row.get("state");

        let light_state: LightState = match serde_json::from_str(&state_json) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let provider = match build_provider(&state, &provider_type, &credentials_enc) {
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
    let applied = results.iter().filter(|ok| **ok).count();
    let failed = results.len() - applied;

    Json(serde_json::json!({ "applied": applied, "failed": failed })).into_response()
}
