use crate::AppState;
use crate::api::auth::Session;
use crate::models::{LightState, SegmentColor};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_lights))
        .route("/{id}", get(get_light).put(set_light_state))
        .route("/{id}/enabled", axum::routing::put(set_light_enabled))
        .route("/{id}/glyph", axum::routing::put(set_light_glyph))
        .route("/{id}/name", axum::routing::put(set_light_name))
        .route("/{id}/shadow", axum::routing::put(set_light_shadow))
        .route("/{id}/room", axum::routing::put(set_light_room))
        .route("/{id}/segments", axum::routing::put(set_light_segments))
}

/// Body for a per-segment colour write: `{ "segments": [{"segment":0,"rgb":16711680}, …] }`.
#[derive(Deserialize)]
pub(crate) struct SegmentsRequest {
    pub segments: Vec<SegmentColor>,
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
/// A light's cached `last_state` (default if it has none yet).
pub(crate) async fn current_light_state(db: &sqlx::SqlitePool, light_id: &str) -> LightState {
    sqlx::query_scalar::<_, Option<String>>("SELECT last_state FROM lights WHERE id = ?")
        .bind(light_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Merge an incoming write onto the light's current state. `on` / `brightness` /
/// `reachable` are **independent** dimensions; colour, colour-temperature, and a
/// dynamic effect are **mutually exclusive** modes — setting any one clears the
/// other two, so the result always names one honest mode (this is what lets a
/// scene snapshot capture/reapply an effect without a stale effect leaking onto a
/// now-plain-colour light).
///
/// Crucially, a write that touches **none** of the three mode fields (e.g. a
/// brightness-only or on/off change) **preserves the running mode**. Sending this
/// merged state to the provider — rather than the bare incoming patch — is what
/// keeps adjusting brightness from cancelling a running effect: a provider like HA
/// turns the effect off on a `light.turn_on` that omits `effect`, so we re-assert
/// it. Generic across every provider and surface (session / v1 / MCP / voice /
/// room cascade).
pub(crate) fn merge_light_state(mut merged: LightState, new: &LightState) -> LightState {
    merged.on = new.on;
    if new.brightness.is_some() {
        merged.brightness = new.brightness;
    }
    if new.reachable.is_some() {
        merged.reachable = new.reachable;
    }
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
    merged
}

/// Write a light's full resolved state to the cache.
pub(crate) async fn write_light_state(db: &sqlx::SqlitePool, light_id: &str, state: &LightState) {
    let Ok(json) = serde_json::to_string(state) else {
        return;
    };
    let _ =
        sqlx::query("UPDATE lights SET last_state = ?, last_seen = datetime('now') WHERE id = ?")
            .bind(json)
            .bind(light_id)
            .execute(db)
            .await;
}

pub(crate) async fn persist_light_state(db: &sqlx::SqlitePool, light_id: &str, new: &LightState) {
    let merged = merge_light_state(current_light_state(db, light_id).await, new);
    write_light_state(db, light_id, &merged).await;
}

/// Resolve a controllable light to its provider device id + a freshly built
/// provider, or an [`SetLightOutcome`] failure. A disabled/shadowed light (or a
/// disabled provider) is not controllable — a shadowed duplicate defers to its
/// native canonical.
async fn controllable_provider(
    state: &AppState,
    id: &str,
) -> Result<(String, Box<dyn crate::providers::LightProvider>), SetLightOutcome> {
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
        Ok(None) => return Err(SetLightOutcome::NotFound),
        Err(e) => {
            tracing::error!("db error: {e}");
            return Err(SetLightOutcome::Db);
        }
    };

    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials_enc: String = row.get("credentials");
    let provider = build_provider(state, &provider_type, &credentials_enc).map_err(|e| {
        tracing::error!("failed to build provider: {e}");
        SetLightOutcome::Db
    })?;
    Ok((device_id, provider))
}

/// Send `new_state` to the light's provider and cache it as `last_state`.
pub(crate) async fn apply_light_state(
    state: &AppState,
    id: &str,
    new_state: &LightState,
) -> SetLightOutcome {
    let (device_id, provider) = match controllable_provider(state, id).await {
        Ok(v) => v,
        Err(outcome) => {
            tracing::debug!(light = %id, "set_state: light not controllable");
            return outcome;
        }
    };

    // Merge onto the current state before driving the provider, so it receives the
    // full honest mode — in particular a brightness-/on-only change re-asserts a
    // running effect instead of dropping it (see `merge_light_state`).
    let merged = merge_light_state(current_light_state(&state.db, id).await, new_state);
    tracing::debug!(light = %id, device = %device_id, state = ?merged, "set_state → provider");
    match provider.set_state(&device_id, &merged).await {
        Ok(()) => {
            write_light_state(&state.db, id, &merged).await;
            tracing::debug!(light = %id, "set_state ok");
            SetLightOutcome::Ok
        }
        Err(e) => {
            tracing::error!(light = %id, "provider set_state error: {e:#}");
            SetLightOutcome::ProviderError
        }
    }
}

/// Send per-segment colours to the light's provider (e.g. a Govee strip). Not
/// persisted: providers expose segment *control* but not *readback*, so there's
/// no segment state to cache or snapshot.
pub(crate) async fn apply_light_segments(
    state: &AppState,
    id: &str,
    segments: &[crate::models::SegmentColor],
) -> SetLightOutcome {
    let (device_id, provider) = match controllable_provider(state, id).await {
        Ok(v) => v,
        Err(outcome) => {
            tracing::debug!(light = %id, "set_segments: light not controllable");
            return outcome;
        }
    };

    tracing::debug!(light = %id, device = %device_id, segments = segments.len(), "set_segments → provider");
    match provider.set_segments(&device_id, segments).await {
        Ok(()) => SetLightOutcome::Ok,
        Err(e) => {
            tracing::error!(light = %id, "provider set_segments error: {e:#}");
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

async fn set_light_segments(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SegmentsRequest>,
) -> impl IntoResponse {
    set_light_status(apply_light_segments(&state, &id, &req.segments).await).into_response()
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

async fn set_light_name(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetNameRequest>,
) -> impl IntoResponse {
    crate::api::set_device_name(&state, "lights", &id, crate::api::clean_name(req.name))
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

#[cfg(test)]
mod tests {
    use super::merge_light_state;
    use crate::models::{Color, LightState};

    fn effecting() -> LightState {
        LightState {
            on: true,
            brightness: Some(80.0),
            effect: Some("Flames".into()),
            ..Default::default()
        }
    }

    #[test]
    fn brightness_only_write_preserves_the_running_effect() {
        // The Nanoleaf/HA bug: adjusting brightness must not cancel a running
        // effect. A write that names no mode keeps the current one.
        let new = LightState {
            on: true,
            brightness: Some(20.0),
            ..Default::default()
        };
        let merged = merge_light_state(effecting(), &new);
        assert_eq!(merged.brightness, Some(20.0));
        assert_eq!(merged.effect.as_deref(), Some("Flames"));
        assert!(merged.color.is_none());
    }

    #[test]
    fn on_off_write_preserves_the_running_effect() {
        let new = LightState {
            on: false,
            ..Default::default()
        };
        let merged = merge_light_state(effecting(), &new);
        assert!(!merged.on);
        assert_eq!(merged.effect.as_deref(), Some("Flames"));
    }

    #[test]
    fn setting_a_colour_clears_the_effect_and_temp() {
        let mut base = effecting();
        base.color_temp_mirek = None;
        let new = LightState {
            on: true,
            color: Some(Color::from_rgb(255, 0, 0)),
            ..Default::default()
        };
        let merged = merge_light_state(base, &new);
        assert!(merged.color.is_some());
        assert!(merged.effect.is_none());
        assert!(merged.color_temp_mirek.is_none());
    }

    #[test]
    fn setting_temp_clears_colour_and_effect() {
        let base = LightState {
            on: true,
            color: Some(Color::from_rgb(0, 255, 0)),
            ..Default::default()
        };
        let new = LightState {
            on: true,
            color_temp_mirek: Some(250),
            ..Default::default()
        };
        let merged = merge_light_state(base, &new);
        assert_eq!(merged.color_temp_mirek, Some(250));
        assert!(merged.color.is_none());
        assert!(merged.effect.is_none());
    }

    #[test]
    fn a_clear_effect_token_clears_the_effect() {
        let new = LightState {
            on: true,
            effect: Some("no_effect".into()),
            ..Default::default()
        };
        let merged = merge_light_state(effecting(), &new);
        assert!(merged.effect.is_none());
    }
}
