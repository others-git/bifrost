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
    routing::{delete, get, put},
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
