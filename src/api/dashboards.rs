//! Dashboards ("Boards") API — CRUD over user-composed widget boards. The backend
//! is a **generic layout store**: it persists each board's name + widget layout (a
//! JSON array) and never interprets a widget's `type`/`config`, so the frontend can
//! add widget types freely. Session-gated UI configuration.

use crate::AppState;
use crate::api::auth::Session;
use crate::models::dashboard::{Dashboard, Widget, clean_aspect, parse_layout};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, put},
};
use serde::Deserialize;
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

/// Uploaded background media cap — generous enough for a short video loop,
/// small enough that a board can't quietly become a media library.
const BG_MEDIA_MAX_BYTES: usize = 25 * 1024 * 1024;

/// Accepted background media types: stills, gif, and short video loops (a muted
/// looping video is *cheaper* to render than a large gif on the wall tablets).
const BG_MEDIA_MIMES: [&str; 6] = [
    "image/jpeg",
    "image/png",
    "image/webp",
    "image/gif",
    "video/mp4",
    "video/webm",
];

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_handler).post(create_handler))
        .route("/reorder", put(reorder_handler))
        .route(
            "/{id}",
            get(get_handler).put(update_handler).delete(delete_handler),
        )
        .route(
            "/{id}/background/media",
            get(get_bg_media)
                .put(put_bg_media)
                .delete(delete_bg_media)
                // The default axum body cap (2MB) is far below a video loop.
                .layer(DefaultBodyLimit::max(BG_MEDIA_MAX_BYTES)),
        )
}

fn row_to_dashboard(r: &sqlx::sqlite::SqliteRow) -> Dashboard {
    Dashboard {
        id: r.get("id"),
        name: r.get("name"),
        position: r.get("position"),
        aspect: r.get("aspect"),
        background: r
            .get::<Option<String>, _>("background")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Null),
        widgets: parse_layout(&r.get::<String, _>("layout")),
    }
}

pub(crate) async fn list_dashboards(state: &AppState) -> Vec<Dashboard> {
    sqlx::query(
        "SELECT id, name, position, aspect, background, layout FROM dashboards ORDER BY position, created_at",
    )
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
    /// Aspect ratio for the board canvas (e.g. `"16:9"`); defaults to 16:9.
    #[serde(default)]
    aspect: Option<String>,
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
    let aspect = clean_aspect(req.aspect.as_deref());
    let id = Uuid::new_v4().to_string();
    // Append at the end of the picker order.
    let position: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(position), -1) + 1 FROM dashboards")
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
    match sqlx::query(
        "INSERT INTO dashboards (id, name, position, aspect, layout) VALUES (?, ?, ?, ?, '[]')",
    )
    .bind(&id)
    .bind(name)
    .bind(position)
    .bind(&aspect)
    .execute(&state.db)
    .await
    {
        Ok(_) => {
            crate::api::notify_inventory(&state, "dashboards");
            (
                StatusCode::CREATED,
                Json(Dashboard {
                    id,
                    name: name.to_string(),
                    position,
                    aspect,
                    background: serde_json::Value::Null,
                    widgets: Vec::new(),
                }),
            )
                .into_response()
        }
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
    match sqlx::query(
        "SELECT id, name, position, aspect, background, layout FROM dashboards WHERE id = ?",
    )
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
    /// New aspect ratio (omit to leave it unchanged).
    #[serde(default)]
    aspect: Option<String>,
    /// Full replacement of the widget layout (omit to leave it unchanged).
    #[serde(default)]
    widgets: Option<Vec<Widget>>,
    /// Background spec (opaque JSON). Double-optional: omitted = unchanged,
    /// `null` = clear, an object = replace. The custom deserializer is what
    /// keeps an explicit `null` from vanishing into the *outer* Option.
    #[serde(default, deserialize_with = "explicit_null")]
    background: Option<Option<serde_json::Value>>,
}

/// Field-present deserializer: wraps the raw value in `Some`, so a JSON `null`
/// arrives as `Some(None)` (clear) instead of `None` (unchanged).
fn explicit_null<'de, D>(d: D) -> Result<Option<Option<serde_json::Value>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    Ok(Some(Option::<serde_json::Value>::deserialize(d)?))
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
    if let Some(aspect) = req.aspect.as_deref() {
        let _ = sqlx::query("UPDATE dashboards SET aspect = ? WHERE id = ?")
            .bind(clean_aspect(Some(aspect)))
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
    if let Some(background) = &req.background {
        let json = background
            .as_ref()
            .filter(|v| !v.is_null())
            .map(|v| v.to_string());
        let _ = sqlx::query("UPDATE dashboards SET background = ? WHERE id = ?")
            .bind(json)
            .bind(&id)
            .execute(&state.db)
            .await;
    }
    // Announce the change so every open Boards view — a wall kiosk above all —
    // re-reads the board live instead of showing a stale layout until reload.
    crate::api::notify_inventory(&state, "dashboards");
    StatusCode::NO_CONTENT.into_response()
}

// ── Background media (uploaded image / short video loop) ─────────────────────

/// `PUT /api/dashboards/{id}/background/media` (session) — store the board's
/// uploaded background. Raw bytes, typed by the `Content-Type` header (allowlist
/// [`BG_MEDIA_MIMES`]); replaces any previous upload. The background *spec*
/// (scrim, cache-buster) is saved separately via the board update.
async fn put_bg_media(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    if !BG_MEDIA_MIMES.contains(&mime.as_str()) {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("background media must be one of {BG_MEDIA_MIMES:?}"),
        )
            .into_response();
    }
    if body.is_empty() {
        return (StatusCode::UNPROCESSABLE_ENTITY, "empty upload").into_response();
    }
    match sqlx::query("UPDATE dashboards SET bg_media = ?, bg_mime = ? WHERE id = ?")
        .bind(body.as_ref())
        .bind(&mime)
        .bind(&id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            crate::api::notify_inventory(&state, "dashboards");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error storing board background media: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `GET /api/dashboards/{id}/background/media` (session) — serve the upload.
/// Immutable-cached: the frontend cache-busts with a `?v=` stamp on replace.
async fn get_bg_media(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let row = sqlx::query("SELECT bg_media, bg_mime FROM dashboards WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    let media = row.as_ref().and_then(|r| {
        let bytes: Option<Vec<u8>> = r.get("bg_media");
        let mime: Option<String> = r.get("bg_mime");
        Some((bytes?, mime?))
    });
    match media {
        Some((bytes, mime)) => (
            [
                (header::CONTENT_TYPE, mime),
                (
                    header::CACHE_CONTROL,
                    "private, max-age=31536000, immutable".to_string(),
                ),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `DELETE /api/dashboards/{id}/background/media` (session) — drop the upload
/// (idempotent; the spec is cleared separately via the board update).
async fn delete_bg_media(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let _ = sqlx::query("UPDATE dashboards SET bg_media = NULL, bg_mime = NULL WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;
    crate::api::notify_inventory(&state, "dashboards");
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
    crate::api::notify_inventory(&state, "dashboards");
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
    crate::api::notify_inventory(&state, "dashboards");
    StatusCode::NO_CONTENT.into_response()
}
