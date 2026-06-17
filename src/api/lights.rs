use crate::AppState;
use crate::api::auth::Session;
use crate::models::LightState;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::Serialize;
use sqlx::Row;
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_lights))
        .route("/{id}", get(get_light).put(set_light_state))
        .route("/{id}/enabled", axum::routing::put(set_light_enabled))
        .route("/{id}/glyph", axum::routing::put(set_light_glyph))
        .route("/{id}/shadow", axum::routing::put(set_light_shadow))
        .route("/{id}/room", axum::routing::put(set_light_room))
}

#[derive(Serialize)]
pub(crate) struct LightRow {
    id: String,
    provider_id: String,
    device_id: String,
    name: String,
    capabilities: serde_json::Value,
    last_state: Option<serde_json::Value>,
    last_seen: Option<String>,
    /// A disabled device is still tracked and keeps its room membership, but
    /// receives no commands and is hidden from room control.
    enabled: bool,
    /// Optional glyph override (name); `None` = the default light glyph.
    glyph: Option<String>,
    /// Normalized hardware identity used for cross-provider de-dup; `None` if
    /// the provider doesn't expose one.
    hw_id: Option<String>,
    /// When set, this device is a duplicate of (shadowed by) the device with
    /// this id — hidden from control and collapsed in the inventory.
    shadowed_by: Option<String>,
    /// `true` when the shadow was set automatically by hw_id matching (native
    /// wins); `false` for a manual user link.
    shadow_auto: bool,
    /// The room this device is directly assigned to (Devices-page assignment),
    /// or `None`. Room *links* (synced provider groups) aren't reflected here.
    room_id: Option<String>,
    /// The room this device belongs to **via a synced provider-group link**, when
    /// it has no direct assignment. Lets the Devices page show the effective room
    /// (e.g. a Hue bulb in the linked "Living room") instead of "No room".
    inherited_room_id: Option<String>,
}

fn row_to_light(r: sqlx::sqlite::SqliteRow) -> LightRow {
    LightRow {
        id: r.get("id"),
        provider_id: r.get("provider_id"),
        device_id: r.get("device_id"),
        name: r.get("name"),
        capabilities: r
            .get::<Option<String>, _>("capabilities")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        last_state: r
            .get::<Option<String>, _>("last_state")
            .and_then(|s| serde_json::from_str(&s).ok()),
        last_seen: r.get("last_seen"),
        enabled: r.get::<i64, _>("enabled") != 0,
        glyph: r.get("glyph"),
        hw_id: r.get("hw_id"),
        shadowed_by: r.get("shadowed_by"),
        shadow_auto: r.get::<i64, _>("shadow_auto") != 0,
        room_id: r.get("room_id"),
        // try_get: queries that don't compute it (live get-one, internal reads)
        // simply leave it None.
        inherited_room_id: r.try_get("inherited_room_id").ok().flatten(),
    }
}

// ── Light services (reused by the session UI API and the public /v1 API) ─────

pub(crate) async fn list_all_lights(state: &AppState) -> Result<Vec<LightRow>, ()> {
    sqlx::query(
        "SELECT id, provider_id, device_id, name, capabilities, last_state, last_seen, enabled, glyph, hw_id, shadowed_by, shadow_auto,
                (SELECT room_id FROM room_lights WHERE light_id = lights.id LIMIT 1) AS room_id,
                (SELECT rl.room_id FROM room_links rl
                   JOIN provider_group_lights pgl ON pgl.provider_group_id = rl.provider_group_id
                   WHERE pgl.light_id = lights.id LIMIT 1) AS inherited_room_id
         FROM lights ORDER BY name",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| tracing::error!("db error listing lights: {e}"))
    .map(|rows| rows.into_iter().map(row_to_light).collect())
}

pub(crate) async fn get_light_by_id(state: &AppState, id: &str) -> Result<Option<LightRow>, ()> {
    sqlx::query(
        "SELECT id, provider_id, device_id, name, capabilities, last_state, last_seen, enabled, glyph, hw_id, shadowed_by, shadow_auto,
                (SELECT room_id FROM room_lights WHERE light_id = lights.id LIMIT 1) AS room_id,
                (SELECT rl.room_id FROM room_links rl
                   JOIN provider_group_lights pgl ON pgl.provider_group_id = rl.provider_group_id
                   WHERE pgl.light_id = lights.id LIMIT 1) AS inherited_room_id
         FROM lights WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| tracing::error!("db error fetching light: {e}"))
    .map(|opt| opt.map(row_to_light))
}

/// Outcome of driving a single light's state, mapped to a status by each caller.
pub(crate) enum SetLightOutcome {
    Ok,
    NotFound,
    ProviderError,
    Db,
}

/// Persist a light's state after a successful command by **merging** the
/// attributes present in `new` into the cached `last_state`.
///
/// Commands are often *partial*: a pure on/off carries no lighting attributes
/// (and must keep the colour/brightness the device holds across a power cycle);
/// a room cascade that moves only the brightness slider carries no colour, and a
/// colour-temperature ("white") change carries no `color`. None of these may
/// clobber the dimensions they didn't touch, so we merge field-by-field rather
/// than overwriting the row.
///
/// Colour and colour temperature are **mutually exclusive** — a light is in
/// exactly one mode — so setting `color` clears any cached `color_temp_mirek`
/// and vice-versa, letting the UI tell which mode the light is in from
/// `last_state` alone.
pub(crate) async fn persist_light_state(db: &sqlx::SqlitePool, light_id: &str, new: &LightState) {
    let current =
        sqlx::query_scalar::<_, Option<String>>("SELECT last_state FROM lights WHERE id = ?")
            .bind(light_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten()
            .flatten();
    let mut merged: LightState = current
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    merged.on = new.on;
    if new.brightness.is_some() {
        merged.brightness = new.brightness;
    }
    if new.reachable.is_some() {
        merged.reachable = new.reachable;
    }
    // Colour, colour-temperature, and a dynamic effect are mutually exclusive
    // modes — a light is in exactly one. Setting any one clears the other two so
    // `last_state` always names a single honest mode; this is what lets a Home
    // Scene snapshot capture and reapply an effect without a stale effect leaking
    // onto a light that has since gone back to a plain colour.
    if new.color.is_some() {
        merged.color = new.color.clone();
        merged.color_temp_mirek = None;
        merged.effect = None;
    } else if new.color_temp_mirek.is_some() {
        merged.color_temp_mirek = new.color_temp_mirek;
        merged.color = None;
        merged.effect = None;
    } else if let Some(effect) = new.effect.as_deref() {
        if crate::models::is_clear_effect(effect) {
            merged.effect = None;
        } else {
            merged.effect = Some(effect.to_string());
            merged.color = None;
            merged.color_temp_mirek = None;
        }
    }

    let Ok(json) = serde_json::to_string(&merged) else {
        return;
    };
    let _ =
        sqlx::query("UPDATE lights SET last_state = ?, last_seen = datetime('now') WHERE id = ?")
            .bind(json)
            .bind(light_id)
            .execute(db)
            .await;
}

/// Send `new_state` to the light's provider and cache it as `last_state`.
pub(crate) async fn apply_light_state(
    state: &AppState,
    id: &str,
    new_state: &LightState,
) -> SetLightOutcome {
    // A disabled or shadowed light (or disabled provider) receives no commands —
    // a shadowed duplicate defers to its native canonical.
    let row = sqlx::query(
        "SELECT l.device_id, p.provider_type, p.credentials
         FROM lights l JOIN providers p ON l.provider_id = p.id
         WHERE l.id = ? AND p.enabled = 1 AND l.enabled = 1 AND l.shadowed_by IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await;

    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return SetLightOutcome::NotFound,
        Err(e) => {
            tracing::error!("db error: {e}");
            return SetLightOutcome::Db;
        }
    };

    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials_enc: String = row.get("credentials");

    let provider = match build_provider(state, &provider_type, &credentials_enc) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to build provider: {e}");
            return SetLightOutcome::Db;
        }
    };

    match provider.set_state(&device_id, new_state).await {
        Ok(()) => {
            persist_light_state(&state.db, id, new_state).await;
            SetLightOutcome::Ok
        }
        Err(e) => {
            tracing::error!("provider set_state error: {e:#}");
            SetLightOutcome::ProviderError
        }
    }
}

/// Map a [`SetLightOutcome`] to the HTTP status both APIs return.
pub(crate) fn set_light_status(outcome: SetLightOutcome) -> StatusCode {
    match outcome {
        SetLightOutcome::Ok => StatusCode::NO_CONTENT,
        SetLightOutcome::NotFound => StatusCode::NOT_FOUND,
        SetLightOutcome::ProviderError => StatusCode::BAD_GATEWAY,
        SetLightOutcome::Db => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

// ── Handlers (session-authenticated; thin wrappers over the services) ────────

async fn list_lights(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    match list_all_lights(&state).await {
        Ok(lights) => Json(lights).into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn get_light(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match get_light_by_id(&state, &id).await {
        Ok(Some(light)) => Json(light).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn set_light_state(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(new_state): Json<LightState>,
) -> impl IntoResponse {
    set_light_status(apply_light_state(&state, &id, &new_state).await).into_response()
}

async fn set_light_enabled(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetEnabledRequest>,
) -> impl IntoResponse {
    crate::api::set_device_enabled(&state, "lights", &id, req.enabled)
        .await
        .into_response()
}

async fn set_light_glyph(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetGlyphRequest>,
) -> impl IntoResponse {
    crate::api::set_device_glyph(&state, "lights", &id, req.glyph)
        .await
        .into_response()
}

async fn set_light_shadow(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetShadowRequest>,
) -> impl IntoResponse {
    crate::api::dedup::set_device_shadow(&state, "lights", &id, req.shadowed_by)
        .await
        .into_response()
}

async fn set_light_room(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetRoomRequest>,
) -> impl IntoResponse {
    crate::api::rooms::set_device_room(
        &state,
        "lights",
        "room_lights",
        "light_id",
        &id,
        req.room_id,
    )
    .await
    .into_response()
}

/// Decrypt credentials and construct a provider via the registry.
pub(crate) fn build_provider(
    state: &AppState,
    provider_type: &str,
    credentials_enc: &str,
) -> anyhow::Result<Box<dyn crate::providers::LightProvider>> {
    let creds_json = state.decrypt_credentials(credentials_enc)?;
    state.registry.build(provider_type, &creds_json)
}
