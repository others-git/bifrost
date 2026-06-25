//! Colour palettes — reusable colour sets imported from a provider's stored
//! scenes (today: Hue scenes via `LightProvider::discover_palettes`).
//!
//! A palette is light-agnostic: unlike a [scene](crate::api::scenes) (a per-light
//! `LightState` snapshot), it's just an ordered list of colours. Applying one to
//! a room **distributes** its colours across that room's lights — light `i` takes
//! `colours[i % n]` — so a Hue scene authored for one room is reusable in any
//! room. All three surfaces would share these service fns; today it's session-only
//! (like Boards), but the logic lives here so `v1`/MCP can call it unchanged.

use crate::AppState;
use crate::api::auth::Session;
use crate::api::lights::build_provider;
use crate::models::LightState;
use crate::models::palette::{Palette, PaletteColor};
use crate::providers::LightProvider;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// Max simultaneous light writes when distributing a palette — same Hue-bridge
/// rate ceiling the scene fan-out respects.
const PALETTE_FANOUT_CONCURRENCY: usize = 6;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_handler))
        .route("/import", post(import_handler))
        .route("/{id}", axum::routing::delete(delete_handler))
        .route("/{id}/apply", post(apply_handler))
}

// ── Service layer ────────────────────────────────────────────────────────────

/// Every stored palette, newest first.
pub(crate) async fn list_palettes(state: &AppState) -> Vec<Palette> {
    let rows =
        sqlx::query("SELECT id, name, source, colors FROM palettes ORDER BY created_at DESC")
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();
    rows.into_iter()
        .map(|r| Palette {
            id: r.get("id"),
            name: r.get("name"),
            source: r.get("source"),
            colors: serde_json::from_str(&r.get::<String, _>("colors")).unwrap_or_default(),
        })
        .collect()
}

/// Pull every enabled provider's stored scenes in as palettes, upserting by
/// `(source, source_id)` so a re-import refreshes in place. Returns how many
/// palettes were imported/updated.
pub(crate) async fn import_palettes(state: &AppState) -> usize {
    let providers =
        sqlx::query("SELECT provider_type, credentials FROM providers WHERE enabled = 1")
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

    let mut imported = 0;
    for row in providers {
        let provider_type: String = row.get("provider_type");
        let credentials: String = row.get("credentials");
        // Only light providers expose palettes; building a non-light type fails,
        // which we skip. The default `discover_palettes` is empty, so a light
        // provider without scenes simply contributes nothing.
        let Ok(provider) = build_provider(state, &provider_type, &credentials) else {
            continue;
        };
        let palettes = match provider.discover_palettes().await {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(provider = %provider_type, "discover_palettes failed: {e:#}");
                continue;
            }
        };
        for p in palettes {
            let colors_json = serde_json::to_string(&p.colors).unwrap_or_else(|_| "[]".into());
            let res = sqlx::query(
                "INSERT INTO palettes (id, name, source, source_id, colors) VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT(source, source_id)
                 DO UPDATE SET name = excluded.name, colors = excluded.colors",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(&p.name)
            .bind(&provider_type)
            .bind(&p.provider_id)
            .bind(&colors_json)
            .execute(&state.db)
            .await;
            if res.is_ok() {
                imported += 1;
            }
        }
    }
    tracing::debug!(imported, "palette import complete");
    imported
}

/// Delete a palette. `true` if a row was removed.
pub(crate) async fn delete_palette(state: &AppState, id: &str) -> bool {
    sqlx::query("DELETE FROM palettes WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await
        .map(|r| r.rows_affected() > 0)
        .unwrap_or(false)
}

/// Distribute a palette's colours across `room_id`'s effective member lights
/// (light `i` ← `colours[i % len]`) and apply them. `None` if the palette doesn't
/// exist; otherwise `(applied, failed)` light counts.
pub(crate) async fn apply_palette(
    state: &AppState,
    palette_id: &str,
    room_id: &str,
) -> Option<(usize, usize)> {
    let colors_json: String = sqlx::query("SELECT colors FROM palettes WHERE id = ?")
        .bind(palette_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()?
        .get("colors");
    let colors: Vec<PaletteColor> = serde_json::from_str(&colors_json).unwrap_or_default();
    if colors.is_empty() {
        return Some((0, 0));
    }

    let members = crate::api::rooms::effective_member_ids(state, room_id).await;
    let targets: Vec<(String, LightState)> = members
        .into_iter()
        .enumerate()
        .map(|(i, light_id)| (light_id, colors[i % colors.len()].to_light_state()))
        .collect();

    Some(apply_light_states(state, targets).await)
}

/// Fan out per-light `set_state` writes, sharing one provider per credential set
/// and bounding concurrency. Returns `(applied, failed)`. Mirrors the scene
/// apply engine for the palette-distribution path.
async fn apply_light_states(
    state: &AppState,
    targets: Vec<(String, LightState)>,
) -> (usize, usize) {
    if targets.is_empty() {
        return (0, 0);
    }
    let ids: Vec<String> = targets.iter().map(|(id, _)| id.clone()).collect();
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let q = format!(
        "SELECT l.id, l.device_id, p.provider_type, p.credentials \
         FROM lights l JOIN providers p ON p.id = l.provider_id \
         WHERE p.enabled = 1 AND l.id IN ({placeholders})"
    );
    let mut query = sqlx::query(&q);
    for id in &ids {
        query = query.bind(id);
    }
    let rows = query.fetch_all(&state.db).await.unwrap_or_default();
    let info: HashMap<String, (String, String, String)> = rows
        .into_iter()
        .map(|r| {
            (
                r.get::<String, _>("id"),
                (
                    r.get::<String, _>("device_id"),
                    r.get::<String, _>("provider_type"),
                    r.get::<String, _>("credentials"),
                ),
            )
        })
        .collect();

    let mut providers: HashMap<String, Option<Arc<dyn LightProvider>>> = HashMap::new();
    let mut jobs = Vec::new();
    for (light_id, light_state) in targets {
        let Some((device_id, provider_type, credentials)) = info.get(&light_id).cloned() else {
            continue;
        };
        let provider = providers
            .entry(credentials.clone())
            .or_insert_with(
                || match build_provider(state, &provider_type, &credentials) {
                    Ok(p) => Some(Arc::from(p)),
                    Err(e) => {
                        tracing::error!("palette apply: provider build failed: {e:#}");
                        None
                    }
                },
            )
            .clone();
        let Some(provider) = provider else { continue };
        jobs.push(async move { provider.set_state(&device_id, &light_state).await.is_ok() });
    }

    use futures_util::stream::StreamExt;
    let results: Vec<bool> = futures_util::stream::iter(jobs)
        .buffer_unordered(PALETTE_FANOUT_CONCURRENCY)
        .collect()
        .await;
    let applied = results.iter().filter(|ok| **ok).count();
    (applied, results.len() - applied)
}

// ── HTTP handlers (session) ──────────────────────────────────────────────────

async fn list_handler(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    Json(list_palettes(&state).await)
}

#[derive(Serialize)]
struct ImportResponse {
    imported: usize,
}

async fn import_handler(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    let imported = import_palettes(&state).await;
    Json(ImportResponse { imported })
}

async fn delete_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if delete_palette(&state, &id).await {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

#[derive(Deserialize)]
struct ApplyRequest {
    room_id: String,
}

#[derive(Serialize)]
struct ApplyResponse {
    applied: usize,
    failed: usize,
}

async fn apply_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<ApplyRequest>,
) -> impl IntoResponse {
    match apply_palette(&state, &id, &req.room_id).await {
        Some((applied, failed)) => Json(ApplyResponse { applied, failed }).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
