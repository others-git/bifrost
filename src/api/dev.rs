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
/// <entity_id>`; it's the introspection that drives generic "passthrough" mapping
/// and capability auditing. Non-HA providers don't have an analog yet.
async fn device_raw(state: &AppState, provider_id: &str, device_id: &str) -> Option<Value> {
    let row = sqlx::query("SELECT provider_type, credentials FROM providers WHERE id = ?")
        .bind(provider_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()?;
    let ptype: String = row.get("provider_type");
    if ptype != "ha" {
        return Some(json!({
            "provider_type": ptype,
            "device_id": device_id,
            "note": "raw upstream introspection is available for Home Assistant devices",
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
