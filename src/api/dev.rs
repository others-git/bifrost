//! Developer mode — contributor/dev-only surfaces gated behind the `dev_mode`
//! config flag (Settings → Developer). Off in a normal deploy: **every route
//! here 404s when dev mode is off**, so the surface doesn't exist in production.
//!
//! Auth mirrors the voice seam — a browser **session** (the dashboard UI) OR a
//! `bfr_` **Bearer key** (scripts / the assistant) — so dev diagnostics are
//! reachable both from the UI and directly over the API.
//!
//! Today it exposes **per-provider debug** (`LightProvider::debug_info`: raw
//! upstream capabilities, the ones we don't model yet — e.g. Govee segments /
//! music mode). It's the home for future dev tooling (kiosk dev helpers, raw API
//! peeks, fixture dumps); a live deploy never sees any of it.

use crate::AppState;
use crate::api::apikeys::require_api_key;
use crate::api::auth::require_session;
use crate::api::lights::build_provider;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
};
use serde_json::{Value, json};
use sqlx::Row;
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/info", get(info_handler))
        .route("/providers", get(providers_handler))
        .route("/providers/{id}/debug", get(provider_debug_handler))
        .route(
            "/devices/{provider_id}/{device_id}/raw",
            get(device_raw_handler),
        )
        .route("/media/{id}/routing", get(media_routing_handler))
        .route("/events", get(events_handler))
        .route("/streams", get(streams_handler))
        .route("/kiosks", get(kiosks_handler))
        .route("/events/clear", axum::routing::post(events_clear_handler))
}

/// Composite **precedence** diagnostic: for the media device `id`, which
/// underlying member wins each control (power / volume / transport / source /
/// favorites / remote) and why. Surfaces exactly what the read/write routing
/// does, so a confusing composite can be debugged at a glance.
async fn media_routing_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    if let Err(code) = guard(&state, &headers).await {
        return code.into_response();
    }
    Json(crate::api::media::composite_routing(&state, &id).await).into_response()
}

/// Whether developer mode is on (`config.dev_mode`).
pub(crate) async fn is_dev_mode(state: &AppState) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT dev_mode FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .unwrap_or(0)
        != 0
}

/// Session OR Bearer key — the same dual auth the voice seam uses.
async fn authed(state: &Arc<AppState>, headers: &HeaderMap) -> bool {
    require_session(state, headers).await.is_some()
        || require_api_key(state, headers).await.is_some()
}

/// Gate every dev route: **404** when dev mode is off (the surface is invisible
/// in production), **401** when on but unauthenticated.
async fn guard(state: &Arc<AppState>, headers: &HeaderMap) -> Result<(), StatusCode> {
    if !is_dev_mode(state).await {
        return Err(StatusCode::NOT_FOUND);
    }
    if !authed(state, headers).await {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

/// Query for the event journal: `after` = last seen seq (0 = from the start),
/// `target` = a target prefix (`bifrost::automation`), `limit` caps the batch.
#[derive(serde::Deserialize)]
struct EventsQuery {
    #[serde(default)]
    after: u64,
    target: Option<String>,
    /// Minimum severity to include ("warn", "error") — the panel's errors filter.
    level: Option<String>,
    limit: Option<usize>,
}

/// The in-app event log: everything Bifrost traced at debug+ under `bifrost::*`
/// (see `crate::journal`), regardless of the console `RUST_LOG` filter. The
/// panel polls with `after=<last_seq>` and appends.
async fn events_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<EventsQuery>,
) -> impl IntoResponse {
    if let Err(code) = guard(&state, &headers).await {
        return code.into_response();
    }
    let (entries, last_seq) = crate::journal::Journal::global().entries_after(
        q.after,
        q.target.as_deref(),
        q.level.as_deref(),
        q.limit.unwrap_or(200).min(500),
    );
    // `areas` is the live set of targets in the buffer — the panel's filter
    // options are derived from it rather than hardcoded, so a new tracing
    // target shows up on its own.
    Json(json!({
        "entries": entries,
        "last_seq": last_seq,
        "areas": crate::journal::Journal::global().areas(),
    }))
    .into_response()
}

/// The live `/api/events` subscriber list: who is connected, for how long, and
/// how much has actually gone out to each.
///
/// This is the server-side half of diagnosing "a surface went stale and only a
/// reload fixes it". The three shapes it distinguishes, none of which were
/// observable before:
/// - the client **isn't here** — its stream died and it never re-established;
/// - it's here, `beats_sent` climbing, `events_sent` flat — the hub isn't
///   emitting, so look at the fan-in, not the browser;
/// - it's here and being sent events it evidently isn't rendering — the bug is
///   in the client, and no amount of server-side reconnect logic will fix it.
///
/// `lagged` above zero means that subscriber has *missed* events outright.
async fn streams_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(code) = guard(&state, &headers).await {
        return code.into_response();
    }
    Json(json!({ "streams": state.streams.snapshot() })).into_response()
}

/// `GET /api/dev/kiosks` — every wall tablet's three layers on one line.
///
/// A kiosk fails in three places and, read separately, they are easy to confuse
/// for one another. This puts them side by side, per kiosk:
///
/// - **device** — `last_seen` / `online`, from the native app's 10s check-in.
///   Aging means the tablet is off the network (or the app died); nothing above
///   it can be believed.
/// - **page** — `web`, self-reported by the WebView. A stale `web.seen_at` with
///   a live check-in means the page itself is frozen or crashed while the device
///   is fine. A fresh report whose `since_beat_ms` keeps growing means the page
///   is running and its stream is being refused or dropped.
/// - **stream** — `stream`, this kiosk's live `/api/events` subscriber, matched
///   by kiosk **id** rather than name (two tablets of one model share a default
///   name). Absent while the page reports itself alive is the signature of a
///   stream the hub is turning away rather than one the client abandoned.
///
/// Beneath all three sits **policy** — what the app says about the device-owner
/// powers that keep a lock screen off the panel. Every layer above can read
/// perfectly healthy while the tablet shows "swipe to unlock", because a kiosk
/// behind the keyguard is still checking in, still rendering, still streaming;
/// it just isn't reachable by a hand. `keyguard_locked` true, or
/// `keyguard_disabled` false, is that fault. All-null = an app build older than
/// the one that reports this.
///
/// Read-only, and Bearer-reachable like the rest of `/api/dev`, because the
/// question this answers — *which* of my tablets is the dead one — is one you
/// ask from a shell, often while standing nowhere near either of them.
/// Read an optional boolean column as JSON (null when the app never reported it).
fn flag(r: &sqlx::sqlite::SqliteRow, col: &str) -> Value {
    match r.get::<Option<i64>, _>(col) {
        Some(v) => json!(v != 0),
        None => Value::Null,
    }
}

async fn kiosks_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(code) = guard(&state, &headers).await {
        return code.into_response();
    }
    let streams = state.streams.snapshot();
    let rows = sqlx::query(
        "SELECT id, name, app_version, last_seen, screen_on, viewport_w, viewport_h,
                web_seen_at, web_since_event_ms, web_since_beat_ms, web_reconnects,
                web_ready_state, web_page_age_ms,
                device_owner, lock_task, keyguard_disabled, keyguard_locked,
                CAST((julianday('now') - julianday(last_seen)) * 86400 AS INTEGER) AS last_seen_secs,
                CAST((julianday('now') - julianday(web_seen_at)) * 86400 AS INTEGER) AS web_seen_secs
         FROM kiosks ORDER BY name",
    )
    .fetch_all(&state.db)
    .await;
    let Ok(rows) = rows else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };

    let kiosks: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let id: String = r.get("id");
            let stream = streams
                .iter()
                .find(|s| s.kiosk_id.as_deref() == Some(id.as_str()));
            json!({
                "id": id,
                "name": r.get::<String, _>("name"),
                "app_version": r.get::<Option<String>, _>("app_version"),
                "screen_on": r.get::<Option<i64>, _>("screen_on").map(|v| v != 0),
                "viewport": match (r.get::<Option<i64>, _>("viewport_w"), r.get::<Option<i64>, _>("viewport_h")) {
                    (Some(w), Some(h)) => json!(format!("{w}x{h}")),
                    _ => Value::Null,
                },
                "device": {
                    "last_seen": r.get::<Option<String>, _>("last_seen"),
                    "last_seen_secs": r.get::<Option<i64>, _>("last_seen_secs"),
                },
                "web": {
                    "seen_at": r.get::<Option<String>, _>("web_seen_at"),
                    "seen_secs": r.get::<Option<i64>, _>("web_seen_secs"),
                    "since_event_ms": r.get::<Option<i64>, _>("web_since_event_ms"),
                    "since_beat_ms": r.get::<Option<i64>, _>("web_since_beat_ms"),
                    "reconnects": r.get::<Option<i64>, _>("web_reconnects"),
                    "ready_state": r.get::<Option<i64>, _>("web_ready_state"),
                    "page_age_ms": r.get::<Option<i64>, _>("web_page_age_ms"),
                },
                "policy": {
                    "device_owner": flag(&r, "device_owner"),
                    "lock_task": flag(&r, "lock_task"),
                    "keyguard_disabled": flag(&r, "keyguard_disabled"),
                    "keyguard_locked": flag(&r, "keyguard_locked"),
                },
                "stream": stream,
            })
        })
        .collect();
    Json(json!({ "kiosks": kiosks })).into_response()
}

async fn events_clear_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(code) = guard(&state, &headers).await {
        return code.into_response();
    }
    crate::journal::Journal::global().clear();
    StatusCode::NO_CONTENT.into_response()
}

async fn info_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(code) = guard(&state, &headers).await {
        return code.into_response();
    }
    let providers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM providers")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);
    Json(json!({
        "dev_mode": true,
        "version": env!("CARGO_PKG_VERSION"),
        "build_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "providers": providers,
    }))
    .into_response()
}

async fn providers_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(code) = guard(&state, &headers).await {
        return code.into_response();
    }
    let rows = sqlx::query("SELECT id, name, provider_type, enabled FROM providers ORDER BY name")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    let list: Vec<Value> = rows
        .iter()
        .map(|r| {
            let ptype: String = r.get("provider_type");
            json!({
                "id": r.get::<String, _>("id"),
                "name": r.get::<String, _>("name"),
                "provider_type": ptype,
                "enabled": r.get::<i64, _>("enabled") != 0,
                // Only light providers expose `debug_info` so far.
                "has_debug": state.registry.is_known(&ptype),
            })
        })
        .collect();
    Json(json!({ "providers": list })).into_response()
}

async fn provider_debug_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(code) = guard(&state, &headers).await {
        return code.into_response();
    }
    match provider_debug(&state, &id).await {
        Some(v) => Json(v).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn device_raw_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((provider_id, device_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(code) = guard(&state, &headers).await {
        return code.into_response();
    }
    match device_raw(&state, &provider_id, &device_id).await {
        Some(v) => Json(v).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The **raw upstream representation** of one device — everything the source
/// exposes, including the parts Bifrost doesn't model (the `supported_features`
/// bitmask and every attribute). For Home Assistant that's `GET /api/states/
/// <entity_id>` (the introspection that drives generic "passthrough" mapping and
/// capability auditing); for Hue it's the CLIP v2 resource with this id, plus
/// its owner resource. Other providers don't have an analog yet.
async fn device_raw(state: &AppState, provider_id: &str, device_id: &str) -> Option<Value> {
    let row = sqlx::query("SELECT provider_type, credentials FROM providers WHERE id = ?")
        .bind(provider_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()?;
    let ptype: String = row.get("provider_type");
    tracing::debug!(target: "bifrost::dev", provider = %provider_id, device = %device_id, %ptype, "raw introspection requested");
    if ptype == "hue" {
        // The raw CLIP v2 resource (+ its owner) straight off the bridge —
        // `device_id` is the service's resource id for Hue rows.
        let creds = state
            .decrypt_credentials(&row.get::<String, _>("credentials"))
            .ok()?;
        let provider = crate::providers::hue::HueProvider::from_credentials(&creds).ok()?;
        return match provider.raw_resource(device_id).await {
            Ok(Some(raw)) => {
                tracing::debug!(target: "bifrost::dev", device = %device_id, "raw introspection: CLIP resource found");
                Some(json!({
                "provider_type": "hue",
                "resource_id": device_id,
                    "resource": raw.get("resource"),
                    "owner": raw.get("owner"),
                }))
            }
            Ok(None) => {
                tracing::debug!(target: "bifrost::dev", device = %device_id, "raw introspection: no CLIP resource with this id");
                Some(json!({
                    "provider_type": "hue",
                    "resource_id": device_id,
                    "note": "no CLIP v2 resource with this id on the bridge",
                }))
            }
            Err(e) => {
                tracing::warn!(target: "bifrost::dev", device = %device_id, "raw introspection failed: {e:#}");
                Some(json!({
                    "provider_type": "hue",
                    "resource_id": device_id,
                    "error": format!("{e:#}"),
                }))
            }
        };
    }
    if ptype != "ha" {
        return Some(json!({
            "provider_type": ptype,
            "device_id": device_id,
            "note": "raw upstream introspection is not implemented for this provider type",
        }));
    }
    let creds = state
        .decrypt_credentials(&row.get::<String, _>("credentials"))
        .ok()?;
    let v: Value = serde_json::from_str(&creds).ok()?;
    let base = v["base_url"].as_str()?.trim_end_matches('/');
    let token = v["token"].as_str()?;
    let entity: Value = reqwest::Client::new()
        .get(format!("{base}/api/states/{device_id}"))
        .bearer_auth(token)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let attrs = entity.get("attributes").cloned().unwrap_or(Value::Null);
    let domain = device_id.split('.').next().unwrap_or("");
    let state_str = entity.get("state").and_then(Value::as_str).unwrap_or("");
    Some(json!({
        "provider_type": "ha",
        "entity_id": device_id,
        "domain": domain,
        "state": entity.get("state"),
        "supported_features": attrs.get("supported_features"),
        "attributes": attrs,
        // How a generic "passthrough" device would model this entity — the #2
        // mapping, previewed here so it can be iterated against live entities.
        "generic_preview": crate::models::generic::controls_from_ha(domain, state_str, &attrs),
    }))
}

/// Build the provider and ask it for diagnostics (`debug_info`). Light providers
/// only for now (Govee is the rich one); other domains return a note.
async fn provider_debug(state: &AppState, id: &str) -> Option<Value> {
    let row = sqlx::query("SELECT provider_type, credentials FROM providers WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()?;
    let ptype: String = row.get("provider_type");
    let creds: String = row.get("credentials");
    let mut out = json!({ "id": id, "provider_type": ptype });
    if state.registry.is_known(&ptype) {
        match build_provider(state, &ptype, &creds) {
            Ok(p) => {
                out["debug"] = match p.debug_info().await {
                    Some(d) => d,
                    None => json!("provider exposes no debug info"),
                };
            }
            Err(e) => out["error"] = json!(format!("build failed: {e:#}")),
        }
    } else {
        out["debug"] =
            json!("debug not available for this provider domain yet (light providers only)");
    }
    Some(out)
}
