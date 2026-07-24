//! Content feeds API — "recently added" items from a feed source (Plex today),
//! serving the Boards feed widget. Gated like the dashboards it feeds, plus a
//! paired kiosk's `bfr_key` cookie ([`SessionOrKiosk`]) — a wall fixture whose
//! minted session lapsed must keep its posters (the kiosk speaks only to
//! Bifrost; posters exist only through the proxy here).
//! Nothing is persisted: reads are live against the source, behind a
//! short response cache so a wall of kiosks polling the same widget doesn't
//! hammer the server. Grouping (episodes → one show tile) happens HERE via the
//! shared [`crate::models::feed::rollup`], so every source inherits it.
//!
//! Posters are **proxied** (`/{id}/image`): the source's token lives in the
//! provider, never in the browser. The path is joined onto the provider's own
//! stored base URL only — an absolute or protocol-relative path is rejected,
//! so the proxy can't be steered at another host.

use crate::AppState;
use crate::api::auth::SessionOrKiosk;
use crate::models::feed::{FeedEntry, rollup};
use crate::providers::FeedProvider;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a `recent` response is served from cache. Recently-added changes
/// on the scale of hours; the widget polls every minute; several kiosks
/// showing one board must cost one upstream request per window, not N.
const RECENT_CACHE_TTL: Duration = Duration::from_secs(60);

/// The widget's tile count is capped, and the raw fetch over-fetches past it
/// so rollup has material to group (10 episodes of one show must not eat the
/// whole window and hide the movie added just before them).
const MAX_LIMIT: usize = 30;
const OVERFETCH_FACTOR: usize = 5;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sources", get(list_sources))
        .route("/{id}/libraries", get(list_libraries))
        .route("/{id}/recent", get(recent))
        .route("/{id}/image", get(image))
}

/// One configured feed-source provider row — the widget config's source picker.
#[derive(Serialize)]
struct FeedSource {
    id: String,
    name: String,
    provider_type: String,
    type_name: String,
}

async fn list_sources(State(state): State<Arc<AppState>>, _: SessionOrKiosk) -> impl IntoResponse {
    let rows = sqlx::query(
        "SELECT id, provider_type, name FROM providers WHERE enabled = 1 ORDER BY display_order, created_at",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| tracing::error!("db error listing feed sources: {e}"))
    .unwrap_or_default();
    let sources: Vec<FeedSource> = rows
        .iter()
        .filter_map(|r| {
            let ptype: String = r.get("provider_type");
            if !state.registry.is_known_feed(&ptype) {
                return None;
            }
            let type_name = state
                .registry
                .display_name(&ptype)
                .unwrap_or(ptype.as_str())
                .to_string();
            Some(FeedSource {
                id: r.get("id"),
                name: r.get("name"),
                provider_type: ptype,
                type_name,
            })
        })
        .collect();
    Json(sources).into_response()
}

/// Build the live feed provider behind a configured provider row, or the
/// status the handler should answer with (404 unknown/disabled/not-a-feed,
/// 500 undecryptable).
async fn build_feed_provider(
    state: &AppState,
    provider_id: &str,
) -> Result<Box<dyn FeedProvider>, StatusCode> {
    let row = sqlx::query(
        "SELECT provider_type, credentials FROM providers WHERE id = ? AND enabled = 1",
    )
    .bind(provider_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;
    let ptype: String = row.get("provider_type");
    if !state.registry.is_known_feed(&ptype) {
        return Err(StatusCode::NOT_FOUND);
    }
    let creds = state
        .decrypt_credentials(&row.get::<String, _>("credentials"))
        .map_err(|e| {
            tracing::error!("feed credentials decrypt failed: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    state.registry.build_feed(&ptype, &creds).map_err(|e| {
        tracing::error!("feed provider build failed: {e:#}");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

async fn list_libraries(
    State(state): State<Arc<AppState>>,
    _: SessionOrKiosk,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let provider = match build_feed_provider(&state, &id).await {
        Ok(p) => p,
        Err(s) => return s.into_response(),
    };
    match provider.libraries().await {
        Ok(libs) => Json(libs).into_response(),
        Err(e) => {
            tracing::warn!(target: "bifrost::feeds", provider = %id, "libraries read failed: {e:#}");
            (StatusCode::BAD_GATEWAY, e.to_string()).into_response()
        }
    }
}

#[derive(Deserialize)]
struct RecentQuery {
    library: String,
    #[serde(default)]
    limit: Option<usize>,
}

/// The recent-items response cache. Keyed per (provider, library, limit);
/// stale entries are dropped on each read, so the map can't grow past the set
/// of feeds actually on someone's board.
type RecentCache = std::sync::Mutex<HashMap<String, (Instant, Vec<FeedEntry>)>>;
static RECENT_CACHE: std::sync::OnceLock<RecentCache> = std::sync::OnceLock::new();

async fn recent(
    State(state): State<Arc<AppState>>,
    _: SessionOrKiosk,
    Path(id): Path<String>,
    Query(q): Query<RecentQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(6).clamp(1, MAX_LIMIT);
    let cache_key = format!("{id}:{}:{limit}", q.library);
    {
        let mut cache = RECENT_CACHE
            .get_or_init(Default::default)
            .lock()
            .expect("feed cache poisoned");
        cache.retain(|_, (at, _)| at.elapsed() < RECENT_CACHE_TTL);
        if let Some((_, entries)) = cache.get(&cache_key) {
            return Json(entries.clone()).into_response();
        }
    }

    let provider = match build_feed_provider(&state, &id).await {
        Ok(p) => p,
        Err(s) => return s.into_response(),
    };
    match provider.recent(&q.library, limit * OVERFETCH_FACTOR).await {
        Ok(items) => {
            let entries = rollup(items, limit);
            tracing::debug!(target: "bifrost::feeds", provider = %id, library = %q.library, tiles = entries.len(), "recent feed read");
            RECENT_CACHE
                .get_or_init(Default::default)
                .lock()
                .expect("feed cache poisoned")
                .insert(cache_key, (Instant::now(), entries.clone()));
            Json(entries).into_response()
        }
        Err(e) => {
            tracing::warn!(target: "bifrost::feeds", provider = %id, "recent read failed: {e:#}");
            (StatusCode::BAD_GATEWAY, e.to_string()).into_response()
        }
    }
}

#[derive(Deserialize)]
struct ImageQuery {
    path: String,
    #[serde(default)]
    w: Option<u32>,
    #[serde(default)]
    h: Option<u32>,
}

/// Poster proxy: fetch a provider-relative asset with the provider's own token
/// and re-serve the bytes. Immutable-cached — a Plex thumb path carries a
/// version stamp, so a given path's bytes never change.
async fn image(
    State(state): State<Arc<AppState>>,
    _: SessionOrKiosk,
    Path(id): Path<String>,
    Query(q): Query<ImageQuery>,
) -> impl IntoResponse {
    // First line of defence here; the provider re-checks before fetching.
    if !crate::providers::is_safe_asset_path(&q.path) {
        return (
            StatusCode::BAD_REQUEST,
            "asset path must be server-relative",
        )
            .into_response();
    }
    let provider = match build_feed_provider(&state, &id).await {
        Ok(p) => p,
        Err(s) => return s.into_response(),
    };
    match provider.image(&q.path, q.w, q.h).await {
        Ok((bytes, mime)) => (
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
        Err(e) => {
            tracing::warn!(target: "bifrost::feeds", provider = %id, "image proxy failed: {e:#}");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}
