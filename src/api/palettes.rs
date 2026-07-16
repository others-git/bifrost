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

/// Pull every enabled provider's stored scenes in as palettes: de-duplicated by
/// name (see [`dedupe_palettes`]), upserted by `(source, source_id)`, and —
/// when the source's discovery completed — pruned, so per-room scene copies and
/// scenes deleted on the provider disappear on the next import instead of
/// accumulating. Returns how many palettes were imported/updated.
pub(crate) async fn import_palettes(state: &AppState) -> usize {
    let providers =
        sqlx::query("SELECT provider_type, credentials FROM providers WHERE enabled = 1")
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

    // Group discovery by provider TYPE (= the `source` column) so a two-bridge
    // setup prunes across the whole type — but dedupe PER ROW (inside the loop):
    // per-room copies of one bridge's stock scene collapse, while two bridges'
    // genuinely distinct same-named scenes both survive.
    let mut by_source: HashMap<String, (Vec<crate::providers::ProviderPalette>, bool)> =
        HashMap::new();
    for row in providers {
        let provider_type: String = row.get("provider_type");
        let credentials: String = row.get("credentials");
        // Only light providers expose palettes; building a non-light type fails,
        // which we skip — but the type is still marked INCOMPLETE: a light row
        // whose credentials fail to build must not have its palettes pruned on
        // the strength of a healthy sibling's discovery. (For genuinely
        // non-light types the incomplete empty entry is a harmless no-op.)
        let Ok(provider) = build_provider(state, &provider_type, &credentials) else {
            by_source
                .entry(provider_type)
                .or_insert((Vec::new(), true))
                .1 = false;
            continue;
        };
        let entry = by_source
            .entry(provider_type.clone())
            .or_insert((Vec::new(), true));
        match provider.discover_palettes().await {
            Ok(p) => entry.0.extend(dedupe_palettes(p)),
            Err(e) => {
                tracing::debug!(provider = %provider_type, "discover_palettes failed: {e:#}");
                // Incomplete discovery: never prune this source on partial data
                // (an unreachable bridge must not wipe its imported palettes).
                entry.1 = false;
            }
        }
    }

    let mut imported = 0;
    for (source, (kept, complete)) in by_source {
        imported += sync_source_palettes(&state.db, &source, &kept, complete).await;
    }
    tracing::debug!(imported, "palette import complete");
    imported
}

/// Collapse ONE provider row's per-room copies of the same scene. A Hue bridge
/// stores one scene resource PER ROOM, so a stock scene ("Savanna sunset")
/// arrives once per room — same name, near-identical colours (per-light
/// gamut/brightness drift makes exact matching useless). A palette is
/// room-agnostic, so keep ONE per name: the richest copy (most colours),
/// tie-broken by lowest source id so the winner — and therefore the upsert
/// key — is stable across re-imports. Applied per bridge, never across
/// bridges: two bridges' same-named scenes are distinct palettes.
fn dedupe_palettes(
    mut palettes: Vec<crate::providers::ProviderPalette>,
) -> Vec<crate::providers::ProviderPalette> {
    palettes.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then(b.colors.len().cmp(&a.colors.len()))
            .then(a.provider_id.cmp(&b.provider_id))
    });
    let mut seen = std::collections::HashSet::new();
    palettes.retain(|p| seen.insert(p.name.clone()));
    palettes
}

/// Upsert the kept palettes for one source, then (when discovery was complete)
/// prune that source's rows whose `source_id` is no longer backed by a kept
/// scene — the de-dup losers and provider-side deletions. User palettes
/// (`source_id IS NULL`) and other sources are never touched.
async fn sync_source_palettes(
    db: &sqlx::SqlitePool,
    source: &str,
    kept: &[crate::providers::ProviderPalette],
    prune: bool,
) -> usize {
    let mut imported = 0;
    for p in kept {
        let colors_json = serde_json::to_string(&p.colors).unwrap_or_else(|_| "[]".into());
        let res = sqlx::query(
            "INSERT INTO palettes (id, name, source, source_id, colors) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(source, source_id)
             DO UPDATE SET name = excluded.name, colors = excluded.colors",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&p.name)
        .bind(source)
        .bind(&p.provider_id)
        .bind(&colors_json)
        .execute(db)
        .await;
        if res.is_ok() {
            imported += 1;
        }
    }

    // Never prune down to nothing: an EMPTY discovery is indistinguishable from
    // a hiccup (a mid-reboot bridge answering with no scenes, or every scene
    // filtering out as colourless) — wiping the whole source on that signal
    // would destroy real palettes. A user who truly emptied their bridge can
    // delete the stragglers from the Scenes page.
    if prune && !kept.is_empty() {
        let placeholders = std::iter::repeat_n("?", kept.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "DELETE FROM palettes
             WHERE source = ? AND source_id IS NOT NULL AND source_id NOT IN ({placeholders})"
        );
        let mut q = sqlx::query(&sql).bind(source);
        for p in kept {
            q = q.bind(&p.provider_id);
        }
        match q.execute(db).await {
            Ok(r) if r.rows_affected() > 0 => {
                tracing::debug!(source, pruned = r.rows_affected(), "stale palettes pruned");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(source, "palette prune failed: {e}"),
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderPalette;

    fn palette(id: &str, name: &str, n_colors: usize) -> ProviderPalette {
        ProviderPalette {
            provider_id: id.to_string(),
            name: name.to_string(),
            colors: (0..n_colors)
                .map(|i| PaletteColor {
                    xy: Some([0.3 + i as f32 * 0.01, 0.3]),
                    mirek: None,
                    brightness: None,
                })
                .collect(),
        }
    }

    #[test]
    fn dedupe_keeps_one_per_name_richest_copy_stable_winner() {
        // Three per-room copies of a stock scene: the 2-colour "Nightlight"
        // beats the 1-colour one; among equals the lowest id wins so re-imports
        // land on the same upsert key.
        let kept = dedupe_palettes(vec![
            palette("c-rid", "Nightlight", 2),
            palette("a-rid", "Nightlight", 1),
            palette("b-rid", "Nightlight", 2),
            palette("z-rid", "Savanna sunset", 6),
        ]);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].name, "Nightlight");
        assert_eq!(kept[0].provider_id, "b-rid");
        assert_eq!(kept[0].colors.len(), 2);
        assert_eq!(kept[1].name, "Savanna sunset");
    }

    async fn test_db() -> sqlx::SqlitePool {
        use std::str::FromStr;
        let opts = sqlx::sqlite::SqliteConnectOptions::from_str(":memory:")
            .unwrap()
            .foreign_keys(true);
        let db = sqlx::SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("./migrations").run(&db).await.unwrap();
        db
    }

    #[tokio::test]
    async fn sync_prunes_dedupe_losers_and_provider_deletions_only() {
        let db = test_db().await;
        // Existing rows: two per-room copies of one scene, one deleted-on-the-
        // bridge scene, a user palette (NULL source_id), and another source.
        for (id, name, source, source_id) in [
            ("p1", "Nightlight", "hue", Some("keep-rid")),
            ("p2", "Nightlight", "hue", Some("loser-rid")),
            ("p3", "Old scene", "hue", Some("deleted-rid")),
            ("p4", "My custom", "user", None),
            ("p5", "Other", "wled", Some("other-rid")),
        ] {
            sqlx::query(
                "INSERT INTO palettes (id, name, source, source_id, colors) VALUES (?, ?, ?, ?, '[]')",
            )
            .bind(id)
            .bind(name)
            .bind(source)
            .bind(source_id)
            .execute(&db)
            .await
            .unwrap();
        }

        let kept = vec![palette("keep-rid", "Nightlight", 2)];
        let imported = sync_source_palettes(&db, "hue", &kept, true).await;
        assert_eq!(imported, 1);

        let remaining: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT source, source_id FROM palettes ORDER BY source, source_id")
                .fetch_all(&db)
                .await
                .unwrap();
        assert_eq!(
            remaining,
            vec![
                ("hue".into(), Some("keep-rid".into())),
                ("user".into(), None),
                ("wled".into(), Some("other-rid".into())),
            ],
            "dedupe loser + provider deletion pruned; user + other-source rows untouched"
        );
    }

    #[tokio::test]
    async fn incomplete_discovery_never_prunes() {
        let db = test_db().await;
        sqlx::query(
            "INSERT INTO palettes (id, name, source, source_id, colors) VALUES ('p1','Kept','hue','rid','[]')",
        )
        .execute(&db)
        .await
        .unwrap();
        // Bridge unreachable → complete=false → the empty kept list must NOT
        // wipe previously-imported palettes.
        sync_source_palettes(&db, "hue", &[], false).await;
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM palettes")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn empty_but_successful_discovery_never_prunes_either() {
        let db = test_db().await;
        sqlx::query(
            "INSERT INTO palettes (id, name, source, source_id, colors) VALUES ('p1','Kept','hue','rid','[]')",
        )
        .execute(&db)
        .await
        .unwrap();
        // A bridge answering Ok with zero (colour-bearing) scenes looks exactly
        // like a hiccup — prune-to-zero must never fire even when "complete".
        sync_source_palettes(&db, "hue", &[], true).await;
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM palettes")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }
}
