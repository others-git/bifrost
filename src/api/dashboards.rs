//! Dashboards ("Boards") API — CRUD over user-composed widget boards. The backend
//! is a **generic layout store**: it persists each board's name + widget layout (a
//! JSON array) and never interprets a widget's `type`/`config`, so the frontend can
//! add widget types freely. Session-gated UI configuration.

use crate::AppState;
use crate::api::auth::Session;
use crate::models::dashboard::{Dashboard, Widget, parse_layout};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, put},
};
use serde::Deserialize;
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_handler).post(create_handler))
        .route("/reorder", put(reorder_handler))
        .route(
            "/{id}",
            get(get_handler).put(update_handler).delete(delete_handler),
        )
}

fn row_to_dashboard(r: &sqlx::sqlite::SqliteRow) -> Dashboard {
    Dashboard {
        id: r.get("id"),
        name: r.get("name"),
        position: r.get("position"),
        widgets: parse_layout(&r.get::<String, _>("layout")),
    }
}

pub(crate) async fn list_dashboards(state: &AppState) -> Vec<Dashboard> {
    sqlx::query("SELECT id, name, position, layout FROM dashboards ORDER BY position, created_at")
        .fetch_all(&state.db)
        .await
        .map_err(|e| tracing::error!("db error listing dashboards: {e}"))
        .unwrap_or_default()
        .iter()
        .map(row_to_dashboard)
        .collect()
}

async fn list_handler(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    Json(list_dashboards(&state).await).into_response()
}

#[derive(Deserialize)]
struct CreateRequest {
    name: String,
}

async fn create_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Json(req): Json<CreateRequest>,
) -> impl IntoResponse {
    let name = req.name.trim();
    if name.is_empty() {
        return (StatusCode::UNPROCESSABLE_ENTITY, "board name is required").into_response();
    }
    let id = Uuid::new_v4().to_string();
    // Append at the end of the picker order.
    let position: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(position), -1) + 1 FROM dashboards")
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
    match sqlx::query("INSERT INTO dashboards (id, name, position, layout) VALUES (?, ?, ?, '[]')")
        .bind(&id)
        .bind(name)
        .bind(position)
        .execute(&state.db)
        .await
    {
        Ok(_) => (
            StatusCode::CREATED,
            Json(Dashboard {
                id,
                name: name.to_string(),
                position,
                widgets: Vec::new(),
            }),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("db error creating dashboard: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn get_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match sqlx::query("SELECT id, name, position, layout FROM dashboards WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some(row)) => Json(row_to_dashboard(&row)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error fetching dashboard: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
struct UpdateRequest {
    #[serde(default)]
    name: Option<String>,
    /// Full replacement of the widget layout (omit to leave it unchanged).
    #[serde(default)]
    widgets: Option<Vec<Widget>>,
}

async fn update_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<UpdateRequest>,
) -> impl IntoResponse {
    let exists = sqlx::query_scalar::<_, i64>("SELECT 1 FROM dashboards WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .is_some();
    if !exists {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Some(name) = req.name.as_deref() {
        let name = name.trim();
        if name.is_empty() {
            return (StatusCode::UNPROCESSABLE_ENTITY, "board name is required").into_response();
        }
        let _ = sqlx::query("UPDATE dashboards SET name = ? WHERE id = ?")
            .bind(name)
            .bind(&id)
            .execute(&state.db)
            .await;
    }
    if let Some(widgets) = &req.widgets {
        // Persist the whole layout atomically; the frontend sends the full set.
        let layout = serde_json::to_string(widgets).unwrap_or_else(|_| "[]".into());
        let _ = sqlx::query("UPDATE dashboards SET layout = ? WHERE id = ?")
            .bind(layout)
            .bind(&id)
            .execute(&state.db)
            .await;
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn delete_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let _ = sqlx::query("DELETE FROM dashboards WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct ReorderRequest {
    /// Board ids in their new order.
    ids: Vec<String>,
}

async fn reorder_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Json(req): Json<ReorderRequest>,
) -> impl IntoResponse {
    for (pos, id) in req.ids.iter().enumerate() {
        let _ = sqlx::query("UPDATE dashboards SET position = ? WHERE id = ?")
            .bind(pos as i64)
            .bind(id)
            .execute(&state.db)
            .await;
    }
    StatusCode::NO_CONTENT.into_response()
}
