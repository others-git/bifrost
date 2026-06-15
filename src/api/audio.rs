//! Audio device API: list devices, read live state, send commands.
//!
//! Mirrors the lights API split: service functions own the behaviour and are
//! shared by the session-authenticated routes here and the Bearer-key routes
//! in `v1`. Reads hit the device live (LAN round trips are cheap) and refresh
//! the cached `last_state`; an unreachable device falls back to the cache with
//! `reachable: false` instead of erroring the whole request.

use crate::AppState;
use crate::api::auth::require_session;
use crate::models::audio::{AudioCapabilities, AudioCommand, AudioFavorite, AudioState};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/devices", get(list_devices_handler))
        .route("/devices/{id}", get(get_device_handler))
        .route("/devices/{id}/state", put(set_device_handler))
        .route("/devices/{id}/favorites", get(list_favorites_handler))
        .route("/devices/{id}/favorites/play", post(play_favorite_handler))
        .route("/devices/{id}/group", post(group_handler))
        .route("/devices/{id}/ungroup", post(ungroup_handler))
        .route("/devices/{id}/enabled", put(set_enabled_handler))
        .route("/devices/{id}/glyph", put(set_glyph_handler))
        .route("/devices/{id}/shadow", put(set_shadow_handler))
        .route("/devices/{id}/room", put(set_room_handler))
        .route("/devices/{id}/receiver", put(set_receiver_handler))
        .route("/devices/{id}/companion", put(set_companion_handler))
}

async fn set_receiver_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetReceiverRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    set_receiver_status(set_audio_receiver(&state, &id, req.receiver_id, req.receiver_source).await)
}

async fn set_companion_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetCompanionRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    set_companion_status(set_audio_companion(&state, &id, req.primary_id).await)
}

async fn set_enabled_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetEnabledRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    crate::api::set_device_enabled(&state, "audio_devices", &id, req.enabled)
        .await
        .into_response()
}

async fn set_glyph_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetGlyphRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    crate::api::set_device_glyph(&state, "audio_devices", &id, req.glyph)
        .await
        .into_response()
}

async fn set_shadow_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetShadowRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    crate::api::dedup::set_device_shadow(&state, "audio_devices", &id, req.shadowed_by)
        .await
        .into_response()
}

async fn set_room_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetRoomRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    crate::api::rooms::set_device_room(
        &state,
        "audio_devices",
        "room_audio_devices",
        "audio_device_id",
        &id,
        req.room_id,
    )
    .await
    .into_response()
}

/// Body for "group this speaker with a coordinator" — the Bifrost device id of
/// the speaker that should coordinate the synced playback group.
#[derive(Deserialize)]
pub(crate) struct GroupRequest {
    pub coordinator_id: String,
}

/// Body for "play a favorite" — the id is carried here rather than in the path
/// because provider-native ids (e.g. Sonos `FV:2/12`) contain slashes.
#[derive(Deserialize)]
pub(crate) struct PlayFavoriteRequest {
    pub favorite_id: String,
}

// ── Wire shape ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AudioDeviceRow {
    pub id: String,
    pub provider_id: String,
    /// Provider-native id (e.g. `main`) — matches `audio_state` push events.
    pub device_id: String,
    pub name: String,
    pub kind: String,
    pub capabilities: AudioCapabilities,
    pub state: AudioState,
    pub last_seen: Option<String>,
    /// Disabled devices keep their room membership but receive no commands and
    /// are hidden from room control.
    pub enabled: bool,
    /// Optional glyph override (name); `None` = derive from `kind`.
    pub glyph: Option<String>,
    /// Normalized hardware identity for cross-provider de-dup; `None` if unknown.
    pub hw_id: Option<String>,
    /// When set, a duplicate of (shadowed by) this device id — hidden from
    /// control and collapsed in the inventory.
    pub shadowed_by: Option<String>,
    /// `true` if the shadow was set automatically by hw_id matching.
    pub shadow_auto: bool,
    /// The room this device is directly assigned to (Devices-page assignment),
    /// or `None`. Room *links* (synced provider groups) aren't reflected here.
    pub room_id: Option<String>,
    /// M22 receiver binding: the audio device id whose volume/mute this source
    /// routes to (the receiver is the volume authority). `None` = unbound.
    pub receiver_id: Option<String>,
    /// The receiver input to select when this source becomes active; `None` =
    /// leave the receiver's input alone.
    pub receiver_source: Option<String>,
    /// M26 composite: the PRIMARY audio device id this entity merges into, if it
    /// is a companion (a complementary view of the same physical device). `None`
    /// = standalone. A companion is hidden from control; its state/controls merge
    /// into the primary (unlike `shadowed_by`, which discards them).
    pub companion_of: Option<String>,
}

fn row_to_device(r: sqlx::sqlite::SqliteRow) -> AudioDeviceRow {
    AudioDeviceRow {
        id: r.get("id"),
        provider_id: r.get("provider_id"),
        device_id: r.get("device_id"),
        name: r.get("name"),
        kind: r.get("kind"),
        capabilities: serde_json::from_str(&r.get::<String, _>("capabilities")).unwrap_or_default(),
        state: r
            .get::<Option<String>, _>("last_state")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        last_seen: r.get("last_seen"),
        enabled: r.get::<i64, _>("enabled") != 0,
        glyph: r.get("glyph"),
        hw_id: r.get("hw_id"),
        shadowed_by: r.get("shadowed_by"),
        shadow_auto: r.get::<i64, _>("shadow_auto") != 0,
        room_id: r.get("room_id"),
        receiver_id: r.get("receiver_id"),
        receiver_source: r.get("receiver_source"),
        companion_of: r.get("companion_of"),
    }
}

/// M26: overlay a companion's complementary state onto its primary — fill
/// now-playing, source/source-list, and **surface the companion's receiver
/// binding** where the primary lacks them, and union the offered capabilities.
/// The receiver volume overlay (run afterwards) then shows the receiver's volume
/// on the merged binding. Nothing is hidden — the union lives on the primary.
fn merge_companion_into(primary: &mut AudioDeviceRow, companion: &AudioDeviceRow) {
    if primary.state.now_playing.is_none() {
        primary
            .state
            .now_playing
            .clone_from(&companion.state.now_playing);
    }
    if primary.state.source.is_none() {
        primary.state.source.clone_from(&companion.state.source);
    }
    for s in &companion.state.source_list {
        if !primary.state.source_list.contains(s) {
            primary.state.source_list.push(s.clone());
        }
    }
    // A receiver binding on the companion takes volume-control precedence.
    if primary.receiver_id.is_none() {
        primary.receiver_id.clone_from(&companion.receiver_id);
        primary
            .receiver_source
            .clone_from(&companion.receiver_source);
    }
    primary.capabilities.transport |= companion.capabilities.transport;
    primary.capabilities.sources |= companion.capabilities.sources;
    primary.capabilities.favorites |= companion.capabilities.favorites;
    primary.capabilities.now_playing |= companion.capabilities.now_playing;
}

/// The companion rows (M26) merged into `primary_id`, if any.
async fn load_companions(state: &AppState, primary_id: &str) -> Vec<AudioDeviceRow> {
    sqlx::query(
        "SELECT id, provider_id, device_id, name, kind, capabilities, last_state, last_seen, enabled, glyph, hw_id, shadowed_by, shadow_auto, receiver_id, receiver_source, companion_of,
                (SELECT room_id FROM room_audio_devices WHERE audio_device_id = audio_devices.id LIMIT 1) AS room_id
         FROM audio_devices WHERE companion_of = ?",
    )
    .bind(primary_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| tracing::error!("db error loading companions: {e}"))
    .unwrap_or_default()
    .into_iter()
    .map(row_to_device)
    .collect()
}

// ── Services (shared with /api/v1) ───────────────────────────────────────────

pub(crate) fn build_audio_provider(
    state: &AppState,
    provider_type: &str,
    credentials_enc: &str,
) -> anyhow::Result<Box<dyn crate::providers::AudioProvider>> {
    let creds_json = state.decrypt_credentials(credentials_enc)?;
    state.registry.build_audio(provider_type, &creds_json)
}

pub(crate) async fn list_all_devices(state: &AppState) -> Result<Vec<AudioDeviceRow>, ()> {
    let mut devices: Vec<AudioDeviceRow> = sqlx::query(
        "SELECT id, provider_id, device_id, name, kind, capabilities, last_state, last_seen, enabled, glyph, hw_id, shadowed_by, shadow_auto, receiver_id, receiver_source, companion_of,
                (SELECT room_id FROM room_audio_devices WHERE audio_device_id = audio_devices.id LIMIT 1) AS room_id
         FROM audio_devices ORDER BY name",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| tracing::error!("db error listing audio devices: {e}"))?
    .into_iter()
    .map(row_to_device)
    .collect();

    // M26: merge each companion's complementary state into its primary, before
    // the receiver overlay (so a companion's receiver binding shows the receiver's
    // volume on the merged card). Companions stay in the list (marked
    // `companion_of`); control surfaces hide them, the inventory collapses them.
    let companions: Vec<AudioDeviceRow> = devices
        .iter()
        .filter(|d| d.companion_of.is_some())
        .cloned()
        .collect();
    for c in &companions {
        if let Some(primary) = devices
            .iter_mut()
            .find(|p| c.companion_of.as_deref() == Some(p.id.as_str()))
        {
            merge_companion_into(primary, c);
        }
    }

    // A bound source shows its receiver's volume/mute (the receiver owns volume),
    // mirroring `get_device_live`. The receiver is in this same list, so overlay
    // from it — no extra query.
    let vol_mute: std::collections::HashMap<String, (u8, bool)> = devices
        .iter()
        .map(|d| (d.id.clone(), (d.state.volume, d.state.mute)))
        .collect();
    for d in &mut devices {
        if let Some(rid) = &d.receiver_id
            && let Some((volume, mute)) = vol_mute.get(rid)
        {
            d.state.volume = *volume;
            d.state.mute = *mute;
        }
    }
    Ok(devices)
}

/// Fetch one device with a live state read. Falls back to the cached state
/// (marked unreachable) when the device doesn't answer; `Ok(None)` = unknown id.
pub(crate) async fn get_device_live(
    state: &AppState,
    id: &str,
) -> Result<Option<AudioDeviceRow>, ()> {
    let row = sqlx::query(
        "SELECT a.id, a.provider_id, a.device_id, a.name, a.kind, a.capabilities,
                a.last_state, a.last_seen, a.enabled, a.glyph, a.hw_id, a.shadowed_by, a.shadow_auto,
                a.receiver_id, a.receiver_source, a.companion_of,
                (SELECT room_id FROM room_audio_devices WHERE audio_device_id = a.id LIMIT 1) AS room_id,
                p.provider_type, p.credentials
         FROM audio_devices a JOIN providers p ON a.provider_id = p.id
         WHERE a.id = ? AND p.enabled = 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| tracing::error!("db error fetching audio device: {e}"))?;

    let Some(row) = row else { return Ok(None) };
    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials: String = row.get("credentials");
    let mut device = row_to_device(row);

    match build_audio_provider(state, &provider_type, &credentials) {
        Ok(provider) => match provider.get_state(&device_id).await {
            Ok(fresh) => {
                let state_json = serde_json::to_string(&fresh).unwrap_or_default();
                let _ = sqlx::query(
                    "UPDATE audio_devices SET last_state = ?, last_seen = datetime('now') WHERE id = ?",
                )
                .bind(&state_json)
                .bind(&device.id)
                .execute(&state.db)
                .await;
                device.state = fresh;
            }
            Err(e) => {
                tracing::debug!("audio device {id} unreachable: {e:#}");
                device.state.reachable = Some(false);
            }
        },
        Err(e) => {
            tracing::error!("failed to build audio provider: {e:#}");
            device.state.reachable = Some(false);
        }
    }

    // M26: overlay companions' complementary state (now-playing, sources, and
    // their receiver binding) onto this primary — before the receiver overlay,
    // so a companion's binding shows the receiver's volume here too.
    for companion in load_companions(state, &device.id).await {
        merge_companion_into(&mut device, &companion);
    }

    // For a bound source the receiver owns volume/mute, so show the receiver's
    // values — what the source's own volume slider actually controls. Use the
    // receiver's *cached* state, not a fresh read: push-mode receivers (Onkyo)
    // allow only one eISCP connection, which the push manager holds, so a
    // competing per-request read returns a partial response and would clobber a
    // good cached volume with 0. The push manager keeps last_state current.
    if let Some(rid) = &device.receiver_id
        && let Ok(Some(r)) = sqlx::query(
            "SELECT last_state FROM audio_devices WHERE id = ? AND enabled = 1 AND shadowed_by IS NULL",
        )
        .bind(rid)
        .fetch_optional(&state.db)
        .await
        && let Some(rstate) = r
            .get::<Option<String>, _>("last_state")
            .and_then(|s| serde_json::from_str::<AudioState>(&s).ok())
    {
        device.state.volume = rstate.volume;
        device.state.mute = rstate.mute;
    }
    Ok(Some(device))
}

pub(crate) enum SetAudioOutcome {
    Ok,
    NotFound,
    BadCommand(String),
    ProviderError,
    Db,
}

pub(crate) enum SetReceiverOutcome {
    Ok,
    NotFound,
    BadRequest(String),
    Db,
}

/// Bind (or, with `receiver_id = None`, unbind) a source audio device to a
/// receiver. Stored on the source — many sources may share one receiver. Rejects
/// a missing source/receiver and self-binding; chaining (binding to a device
/// that is itself bound) is rejected so volume can't route in a loop.
pub(crate) async fn set_audio_receiver(
    state: &AppState,
    id: &str,
    receiver_id: Option<String>,
    receiver_source: Option<String>,
) -> SetReceiverOutcome {
    if let Some(rid) = &receiver_id {
        if rid == id {
            return SetReceiverOutcome::BadRequest("a device cannot be its own receiver".into());
        }
        let receiver = sqlx::query("SELECT receiver_id FROM audio_devices WHERE id = ?")
            .bind(rid)
            .fetch_optional(&state.db)
            .await;
        match receiver {
            Ok(Some(r)) => {
                if r.get::<Option<String>, _>("receiver_id").is_some() {
                    return SetReceiverOutcome::BadRequest(
                        "that device is itself bound to a receiver; pick a standalone receiver"
                            .into(),
                    );
                }
            }
            Ok(None) => return SetReceiverOutcome::BadRequest("unknown receiver device".into()),
            Err(e) => {
                tracing::error!("db error validating receiver: {e}");
                return SetReceiverOutcome::Db;
            }
        }
    }
    // Clearing the binding clears the input too; setting it stores both.
    let stored_source = receiver_id.as_ref().and(receiver_source);
    match sqlx::query("UPDATE audio_devices SET receiver_id = ?, receiver_source = ? WHERE id = ?")
        .bind(&receiver_id)
        .bind(&stored_source)
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => SetReceiverOutcome::Ok,
        Ok(_) => SetReceiverOutcome::NotFound,
        Err(e) => {
            tracing::error!("db error setting receiver binding: {e}");
            SetReceiverOutcome::Db
        }
    }
}

pub(crate) fn set_receiver_status(outcome: SetReceiverOutcome) -> axum::response::Response {
    match outcome {
        SetReceiverOutcome::Ok => StatusCode::NO_CONTENT.into_response(),
        SetReceiverOutcome::NotFound => StatusCode::NOT_FOUND.into_response(),
        SetReceiverOutcome::BadRequest(m) => (StatusCode::UNPROCESSABLE_ENTITY, m).into_response(),
        SetReceiverOutcome::Db => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub(crate) enum SetCompanionOutcome {
    Ok,
    NotFound,
    BadRequest(String),
    Db,
}

/// M26: merge an audio entity into a **primary** as its companion (the link is
/// stored on the companion as `companion_of`), or unmerge with `primary_id =
/// None`. Unlike a shadow, the companion's capabilities are routed/overlaid onto
/// the primary, not discarded. Rejects self-merge, an unknown/companion/shadowed
/// primary, and merging a device that is itself a primary (no chains).
pub(crate) async fn set_audio_companion(
    state: &AppState,
    id: &str,
    primary_id: Option<String>,
) -> SetCompanionOutcome {
    if let Some(pid) = &primary_id {
        if pid == id {
            return SetCompanionOutcome::BadRequest("a device cannot be its own companion".into());
        }
        match sqlx::query("SELECT companion_of, shadowed_by FROM audio_devices WHERE id = ?")
            .bind(pid)
            .fetch_optional(&state.db)
            .await
        {
            Ok(Some(r)) => {
                if r.get::<Option<String>, _>("companion_of").is_some() {
                    return SetCompanionOutcome::BadRequest(
                        "that device is itself merged into another; pick a standalone primary"
                            .into(),
                    );
                }
                if r.get::<Option<String>, _>("shadowed_by").is_some() {
                    return SetCompanionOutcome::BadRequest(
                        "that device is a hidden duplicate".into(),
                    );
                }
            }
            Ok(None) => return SetCompanionOutcome::BadRequest("unknown primary device".into()),
            Err(e) => {
                tracing::error!("db error validating companion primary: {e}");
                return SetCompanionOutcome::Db;
            }
        }
        // The companion must not itself be a primary of other devices (no chains).
        match sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM audio_devices WHERE companion_of = ?",
        )
        .bind(id)
        .fetch_one(&state.db)
        .await
        {
            Ok(n) if n > 0 => {
                return SetCompanionOutcome::BadRequest(
                    "this device already has companions merged into it".into(),
                );
            }
            Ok(_) => {}
            Err(e) => {
                tracing::error!("db error checking companion chain: {e}");
                return SetCompanionOutcome::Db;
            }
        }
    }
    match sqlx::query("UPDATE audio_devices SET companion_of = ? WHERE id = ?")
        .bind(&primary_id)
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => SetCompanionOutcome::Ok,
        Ok(_) => SetCompanionOutcome::NotFound,
        Err(e) => {
            tracing::error!("db error setting companion link: {e}");
            SetCompanionOutcome::Db
        }
    }
}

pub(crate) fn set_companion_status(outcome: SetCompanionOutcome) -> axum::response::Response {
    match outcome {
        SetCompanionOutcome::Ok => StatusCode::NO_CONTENT.into_response(),
        SetCompanionOutcome::NotFound => StatusCode::NOT_FOUND.into_response(),
        SetCompanionOutcome::BadRequest(m) => (StatusCode::UNPROCESSABLE_ENTITY, m).into_response(),
        SetCompanionOutcome::Db => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Route a command for an audio device, honouring an M22 receiver binding: a
/// bound source sends `volume`/`mute` to its receiver (and switches the receiver
/// input on power-on) while keeping `power`/`source`/`transport` on itself.
/// Unbound devices apply the command directly. Shared by session, `/v1`, and MCP
/// so every surface routes identically.
/// One backing entity of a composite device (M26), for command routing.
struct Backing {
    id: String,
    capabilities: AudioCapabilities,
    /// This backing routes its volume/mute to a receiver (M22 binding).
    receiver_bound: bool,
    /// This backing is the one actively reporting playback.
    has_now_playing: bool,
}

/// Route an `AudioCommand` across a composite's backings (`backings[0]` is the
/// primary), per the M26 rules:
/// - **volume / mute → a receiver-bound backing** (so it reaches the receiver),
///   else the primary;
/// - **transport → the backing controlling playback** (now-playing, then any
///   with transport), else the primary;
/// - **source/app → the backing with selectable inputs**, else the primary;
/// - **power → the primary**.
///
/// Returns `(backing_id, sub-command)` for each non-empty target. Each part is
/// then applied via [`apply_with_receiver`], which does that backing's own
/// receiver split — so a receiver-bound backing's volume reaches its receiver.
fn route_across_backings(cmd: &AudioCommand, backings: &[Backing]) -> Vec<(String, AudioCommand)> {
    let primary_id = backings[0].id.clone();
    let pick = |pred: fn(&Backing) -> bool| -> String {
        backings
            .iter()
            .find(|b| pred(b))
            .map_or_else(|| primary_id.clone(), |b| b.id.clone())
    };
    let mut parts: std::collections::BTreeMap<String, AudioCommand> =
        std::collections::BTreeMap::new();
    if cmd.volume.is_some() || cmd.mute.is_some() {
        let e = parts.entry(pick(|b| b.receiver_bound)).or_default();
        e.volume = cmd.volume;
        e.mute = cmd.mute;
    }
    if cmd.transport.is_some() {
        let target = backings
            .iter()
            .find(|b| b.capabilities.transport && b.has_now_playing)
            .or_else(|| backings.iter().find(|b| b.capabilities.transport))
            .map_or_else(|| primary_id.clone(), |b| b.id.clone());
        parts.entry(target).or_default().transport = cmd.transport;
    }
    if cmd.source.is_some() {
        parts
            .entry(pick(|b| b.capabilities.sources))
            .or_default()
            .source = cmd.source.clone();
    }
    if cmd.power.is_some() {
        parts.entry(primary_id).or_default().power = cmd.power;
    }
    parts.into_iter().filter(|(_, c)| !c.is_empty()).collect()
}

/// The composite's backings (primary first, then companions), or just `[id]`
/// when `id` has no companions. Each carries the capability/binding facts the
/// router needs.
async fn load_composite_backings(state: &AppState, id: &str) -> Vec<Backing> {
    let rows = sqlx::query(
        "SELECT id, capabilities, receiver_id, last_state, (id = ?) AS is_primary
         FROM audio_devices
         WHERE id = ? OR companion_of = ?
         ORDER BY is_primary DESC, name",
    )
    .bind(id)
    .bind(id)
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| tracing::error!("db error loading composite backings: {e}"))
    .unwrap_or_default();
    rows.into_iter()
        .map(|r| {
            let state: Option<AudioState> = r
                .get::<Option<String>, _>("last_state")
                .and_then(|s| serde_json::from_str(&s).ok());
            Backing {
                id: r.get("id"),
                capabilities: serde_json::from_str(&r.get::<String, _>("capabilities"))
                    .unwrap_or_default(),
                receiver_bound: r.get::<Option<String>, _>("receiver_id").is_some(),
                has_now_playing: state.is_some_and(|s| s.now_playing.is_some()),
            }
        })
        .collect()
}

/// Apply a command to a device. If `id` is a composite **primary** (has
/// companions merged in), route each field to the backing that owns it (M26);
/// otherwise drive the single device directly (with its own receiver split).
pub(crate) async fn apply_audio_command(
    state: &AppState,
    id: &str,
    cmd: &AudioCommand,
) -> SetAudioOutcome {
    let backings = load_composite_backings(state, id).await;
    if backings.len() <= 1 {
        return apply_with_receiver(state, id, cmd).await;
    }
    for (backing_id, sub) in route_across_backings(cmd, &backings) {
        match apply_with_receiver(state, &backing_id, &sub).await {
            SetAudioOutcome::Ok => {}
            other => return other,
        }
    }
    SetAudioOutcome::Ok
}

/// Drive one device, routing its volume/mute to a bound receiver (M22) if any.
async fn apply_with_receiver(state: &AppState, id: &str, cmd: &AudioCommand) -> SetAudioOutcome {
    match load_receiver_binding(state, id).await {
        Err(()) => SetAudioOutcome::Db,
        Ok(None) => apply_to_device(state, id, cmd).await,
        Ok(Some((receiver_id, receiver_source))) => {
            let (source_cmd, receiver_cmd) = cmd.split_for_receiver(receiver_source.as_deref());
            // Source first (power/input), so the receiver wakes to an active source.
            if !source_cmd.is_empty() {
                match apply_to_device(state, id, &source_cmd).await {
                    SetAudioOutcome::Ok => {}
                    other => return other,
                }
            }
            if !receiver_cmd.is_empty() {
                return apply_to_device(state, &receiver_id, &receiver_cmd).await;
            }
            SetAudioOutcome::Ok
        }
    }
}

/// Return `(receiver_id, receiver_source)` when `id` is a controllable source
/// bound to a usable (enabled, non-shadowed) receiver; `Ok(None)` when unbound
/// or the receiver is gone/disabled (treat a dangling binding as unbound).
async fn load_receiver_binding(
    state: &AppState,
    id: &str,
) -> Result<Option<(String, Option<String>)>, ()> {
    let row = sqlx::query(
        "SELECT a.receiver_id, a.receiver_source
         FROM audio_devices a JOIN providers p ON a.provider_id = p.id
         WHERE a.id = ? AND p.enabled = 1 AND a.enabled = 1 AND a.shadowed_by IS NULL
           AND a.receiver_id IS NOT NULL
           AND EXISTS (
               SELECT 1 FROM audio_devices r JOIN providers rp ON r.provider_id = rp.id
               WHERE r.id = a.receiver_id AND r.enabled = 1 AND r.shadowed_by IS NULL AND rp.enabled = 1
           )",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| tracing::error!("db error loading receiver binding: {e}"))?;
    Ok(row.map(|r| (r.get("receiver_id"), r.get("receiver_source"))))
}

async fn apply_to_device(state: &AppState, id: &str, cmd: &AudioCommand) -> SetAudioOutcome {
    // A disabled device receives no commands (control lookups skip it).
    let row = sqlx::query(
        "SELECT a.device_id, p.provider_type, p.credentials
         FROM audio_devices a JOIN providers p ON a.provider_id = p.id
         WHERE a.id = ? AND p.enabled = 1 AND a.enabled = 1 AND a.shadowed_by IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await;

    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return SetAudioOutcome::NotFound,
        Err(e) => {
            tracing::error!("db error: {e}");
            return SetAudioOutcome::Db;
        }
    };

    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials: String = row.get("credentials");

    let provider = match build_audio_provider(state, &provider_type, &credentials) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to build audio provider: {e:#}");
            return SetAudioOutcome::Db;
        }
    };

    match provider.set_state(&device_id, cmd).await {
        Ok(()) => SetAudioOutcome::Ok,
        // Validation failures (unknown source/zone) are the caller's mistake,
        // not a gateway fault; connection problems stay 502s.
        Err(e) if e.to_string().contains("unknown") || e.to_string().contains("not supported") => {
            SetAudioOutcome::BadCommand(e.to_string())
        }
        Err(e) => {
            tracing::error!("audio set_state error: {e:#}");
            SetAudioOutcome::ProviderError
        }
    }
}

/// Look up an audio device's provider-native id and a live provider built from
/// its (decrypted) credentials. Shared by the favorites services.
enum ProviderLookup {
    Found(String, Box<dyn crate::providers::AudioProvider>),
    NotFound,
    Db,
}

async fn lookup_audio_provider(state: &AppState, id: &str) -> ProviderLookup {
    // A disabled device receives no commands (control lookups skip it).
    let row = sqlx::query(
        "SELECT a.device_id, p.provider_type, p.credentials
         FROM audio_devices a JOIN providers p ON a.provider_id = p.id
         WHERE a.id = ? AND p.enabled = 1 AND a.enabled = 1 AND a.shadowed_by IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await;
    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return ProviderLookup::NotFound,
        Err(e) => {
            tracing::error!("db error: {e}");
            return ProviderLookup::Db;
        }
    };
    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials: String = row.get("credentials");
    match build_audio_provider(state, &provider_type, &credentials) {
        Ok(p) => ProviderLookup::Found(device_id, p),
        Err(e) => {
            tracing::error!("failed to build audio provider: {e:#}");
            ProviderLookup::Db
        }
    }
}

// ── Favorites (list + play) ──────────────────────────────────────────────────

pub(crate) enum FavoritesOutcome {
    Ok(Vec<AudioFavorite>),
    NotFound,
    Unreachable,
    Db,
}

pub(crate) async fn list_device_favorites(state: &AppState, id: &str) -> FavoritesOutcome {
    let (device_id, provider) = match lookup_audio_provider(state, id).await {
        ProviderLookup::Found(d, p) => (d, p),
        ProviderLookup::NotFound => return FavoritesOutcome::NotFound,
        ProviderLookup::Db => return FavoritesOutcome::Db,
    };
    match provider.list_favorites(&device_id).await {
        Ok(favs) => FavoritesOutcome::Ok(favs),
        Err(e) => {
            tracing::debug!("audio favorites unavailable for {id}: {e:#}");
            FavoritesOutcome::Unreachable
        }
    }
}

pub(crate) fn favorites_response(outcome: FavoritesOutcome) -> axum::response::Response {
    match outcome {
        FavoritesOutcome::Ok(favs) => Json(favs).into_response(),
        FavoritesOutcome::NotFound => StatusCode::NOT_FOUND.into_response(),
        FavoritesOutcome::Unreachable => StatusCode::BAD_GATEWAY.into_response(),
        FavoritesOutcome::Db => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub(crate) enum PlayFavoriteOutcome {
    Ok,
    NotFound,
    BadFavorite(String),
    Unreachable,
    Db,
}

pub(crate) async fn play_device_favorite(
    state: &AppState,
    id: &str,
    favorite_id: &str,
) -> PlayFavoriteOutcome {
    let (device_id, provider) = match lookup_audio_provider(state, id).await {
        ProviderLookup::Found(d, p) => (d, p),
        ProviderLookup::NotFound => return PlayFavoriteOutcome::NotFound,
        ProviderLookup::Db => return PlayFavoriteOutcome::Db,
    };
    match provider.play_favorite(&device_id, favorite_id).await {
        Ok(()) => PlayFavoriteOutcome::Ok,
        // An unknown favorite id (or a provider without favorites) is the
        // caller's mistake, not a gateway fault.
        Err(e) if e.to_string().contains("unknown") || e.to_string().contains("not support") => {
            PlayFavoriteOutcome::BadFavorite(e.to_string())
        }
        Err(e) => {
            tracing::error!("audio play_favorite error: {e:#}");
            PlayFavoriteOutcome::Unreachable
        }
    }
}

pub(crate) fn play_favorite_response(outcome: PlayFavoriteOutcome) -> axum::response::Response {
    match outcome {
        PlayFavoriteOutcome::Ok => StatusCode::NO_CONTENT.into_response(),
        PlayFavoriteOutcome::NotFound => StatusCode::NOT_FOUND.into_response(),
        PlayFavoriteOutcome::BadFavorite(m) => {
            (StatusCode::UNPROCESSABLE_ENTITY, m).into_response()
        }
        PlayFavoriteOutcome::Unreachable => StatusCode::BAD_GATEWAY.into_response(),
        PlayFavoriteOutcome::Db => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── Speaker grouping (provider-native, e.g. Sonos) ───────────────────────────

/// The bits needed to build a provider and address a device within it.
struct AudioRow {
    device_id: String,
    /// `audio_devices.provider_id` — the providers-table row id (which provider
    /// instance owns the device), not the provider-native id.
    provider_row_id: String,
    provider_type: String,
    credentials: String,
}

async fn load_audio_row(state: &AppState, id: &str) -> Result<Option<AudioRow>, ()> {
    let row = sqlx::query(
        "SELECT a.device_id, a.provider_id AS provider_row_id, p.provider_type, p.credentials
         FROM audio_devices a JOIN providers p ON a.provider_id = p.id
         WHERE a.id = ? AND p.enabled = 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| tracing::error!("db error loading audio device: {e}"))?;
    Ok(row.map(|r| AudioRow {
        device_id: r.get("device_id"),
        provider_row_id: r.get("provider_row_id"),
        provider_type: r.get("provider_type"),
        credentials: r.get("credentials"),
    }))
}

pub(crate) enum GroupOutcome {
    Ok,
    NotFound,
    BadRequest(String),
    Unreachable,
    Db,
}

/// Map a provider grouping call's result. A "not supported" provider or a
/// rejected request (e.g. grouping a speaker with itself) is the caller's
/// mistake (422); a transport failure is a gateway error (502).
fn map_group_result(res: anyhow::Result<()>) -> GroupOutcome {
    match res {
        Ok(()) => GroupOutcome::Ok,
        Err(e) if e.to_string().contains("not support") || e.to_string().contains("itself") => {
            GroupOutcome::BadRequest(e.to_string())
        }
        Err(e) => {
            tracing::error!("audio grouping error: {e:#}");
            GroupOutcome::Unreachable
        }
    }
}

/// Join the speaker `id` into the synced playback group coordinated by
/// `coordinator_id`. Both must be devices of the same provider instance.
pub(crate) async fn group_devices(
    state: &AppState,
    id: &str,
    coordinator_id: &str,
) -> GroupOutcome {
    let member = match load_audio_row(state, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return GroupOutcome::NotFound,
        Err(()) => return GroupOutcome::Db,
    };
    let coordinator = match load_audio_row(state, coordinator_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return GroupOutcome::NotFound,
        Err(()) => return GroupOutcome::Db,
    };
    if member.provider_row_id != coordinator.provider_row_id {
        return GroupOutcome::BadRequest(
            "speakers must belong to the same provider to be grouped".into(),
        );
    }
    let provider = match build_audio_provider(state, &member.provider_type, &member.credentials) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to build audio provider: {e:#}");
            return GroupOutcome::Db;
        }
    };
    map_group_result(
        provider
            .group(&member.device_id, &coordinator.device_id)
            .await,
    )
}

/// Remove the speaker `id` from any playback group it's in.
pub(crate) async fn ungroup_device(state: &AppState, id: &str) -> GroupOutcome {
    let row = match load_audio_row(state, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return GroupOutcome::NotFound,
        Err(()) => return GroupOutcome::Db,
    };
    let provider = match build_audio_provider(state, &row.provider_type, &row.credentials) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to build audio provider: {e:#}");
            return GroupOutcome::Db;
        }
    };
    map_group_result(provider.ungroup(&row.device_id).await)
}

pub(crate) fn group_response(outcome: GroupOutcome) -> axum::response::Response {
    match outcome {
        GroupOutcome::Ok => StatusCode::NO_CONTENT.into_response(),
        GroupOutcome::NotFound => StatusCode::NOT_FOUND.into_response(),
        GroupOutcome::BadRequest(m) => (StatusCode::UNPROCESSABLE_ENTITY, m).into_response(),
        GroupOutcome::Unreachable => StatusCode::BAD_GATEWAY.into_response(),
        GroupOutcome::Db => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub(crate) fn set_audio_status(outcome: SetAudioOutcome) -> axum::response::Response {
    match outcome {
        SetAudioOutcome::Ok => StatusCode::NO_CONTENT.into_response(),
        SetAudioOutcome::NotFound => StatusCode::NOT_FOUND.into_response(),
        SetAudioOutcome::BadCommand(m) => (StatusCode::UNPROCESSABLE_ENTITY, m).into_response(),
        SetAudioOutcome::ProviderError => StatusCode::BAD_GATEWAY.into_response(),
        SetAudioOutcome::Db => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Discover an audio provider's devices and upsert them. Returns the count.
/// Called from the shared `/api/providers/{id}/discover` handler.
pub(crate) async fn discover_audio_devices(
    state: &AppState,
    provider_row_id: &str,
    provider_type: &str,
    credentials_enc: &str,
) -> Result<usize, StatusCode> {
    let provider = build_audio_provider(state, provider_type, credentials_enc).map_err(|e| {
        tracing::error!("failed to build audio provider: {e:#}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let devices = provider.discover().await.map_err(|e| {
        tracing::error!("audio discovery error: {e:#}");
        StatusCode::BAD_GATEWAY
    })?;

    for device in &devices {
        let caps = serde_json::to_string(&device.capabilities).unwrap_or_default();
        let state_json = serde_json::to_string(&device.state).unwrap_or_default();
        let kind = match device.kind {
            crate::models::audio::AudioDeviceKind::Receiver => "receiver",
            crate::models::audio::AudioDeviceKind::Speaker => "speaker",
            crate::models::audio::AudioDeviceKind::Tv => "tv",
            crate::models::audio::AudioDeviceKind::Zone => "zone",
        };
        let _ = sqlx::query(
            "INSERT INTO audio_devices (id, provider_id, device_id, name, kind, capabilities, last_state, last_seen, hw_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, datetime('now'), ?)
             ON CONFLICT (provider_id, device_id)
             DO UPDATE SET name         = excluded.name,
                           kind         = excluded.kind,
                           capabilities = excluded.capabilities,
                           last_state   = excluded.last_state,
                           last_seen    = excluded.last_seen,
                           hw_id        = excluded.hw_id",
        )
        .bind(device.id.to_string())
        .bind(provider_row_id)
        .bind(&device.provider_id)
        .bind(&device.name)
        .bind(kind)
        .bind(&caps)
        .bind(&state_json)
        .bind(&device.hw_id)
        .execute(&state.db)
        .await;
    }
    Ok(devices.len())
}

// ── Handlers (session-authenticated) ─────────────────────────────────────────

async fn list_devices_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match list_all_devices(&state).await {
        Ok(devices) => Json(devices).into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn get_device_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match get_device_live(&state, &id).await {
        Ok(Some(device)) => Json(device).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn set_device_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(cmd): Json<AudioCommand>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    set_audio_status(apply_audio_command(&state, &id, &cmd).await)
}

async fn list_favorites_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    favorites_response(list_device_favorites(&state, &id).await)
}

async fn play_favorite_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<PlayFavoriteRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    play_favorite_response(play_device_favorite(&state, &id, &req.favorite_id).await)
}

async fn group_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<GroupRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    group_response(group_devices(&state, &id, &req.coordinator_id).await)
}

async fn ungroup_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    group_response(ungroup_device(&state, &id).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::audio::TransportCmd;

    fn backing(
        id: &str,
        transport: bool,
        sources: bool,
        receiver_bound: bool,
        now_playing: bool,
    ) -> Backing {
        Backing {
            id: id.into(),
            capabilities: AudioCapabilities {
                transport,
                sources,
                ..Default::default()
            },
            receiver_bound,
            has_now_playing: now_playing,
        }
    }

    #[test]
    fn routes_volume_to_the_receiver_bound_backing_not_the_primary() {
        // BRAVIA shape: primary = the TV (no receiver), companion = the speaker
        // (receiver-bound + reporting playback). Volume must reach the receiver
        // (via the speaker), transport → the speaker, power → the TV. This is the
        // exact precedence: a receiver binding on ANY backing wins volume.
        let backings = vec![
            backing("tv", false, false, false, false),
            backing("speaker", true, false, true, true),
        ];
        let cmd = AudioCommand {
            power: Some(true),
            volume: Some(30),
            mute: Some(true),
            source: None,
            transport: Some(TransportCmd::Toggle),
        };
        let routed: std::collections::HashMap<String, AudioCommand> =
            route_across_backings(&cmd, &backings).into_iter().collect();

        assert_eq!(routed["speaker"].volume, Some(30));
        assert_eq!(routed["speaker"].mute, Some(true));
        assert_eq!(routed["speaker"].transport, Some(TransportCmd::Toggle));
        assert_eq!(routed["tv"].power, Some(true));
        assert_eq!(routed["tv"].volume, None);
    }

    #[test]
    fn routes_to_primary_when_no_backing_owns_the_field() {
        // No receiver anywhere → volume falls back to the primary; source goes to
        // the backing that has the `sources` capability.
        let backings = vec![
            backing("primary", false, true, false, false),
            backing("comp", false, false, false, false),
        ];
        let cmd = AudioCommand {
            volume: Some(50),
            source: Some("hdmi1".into()),
            ..Default::default()
        };
        let routed: std::collections::HashMap<String, AudioCommand> =
            route_across_backings(&cmd, &backings).into_iter().collect();

        assert_eq!(routed["primary"].volume, Some(50));
        assert_eq!(routed["primary"].source.as_deref(), Some("hdmi1"));
    }
}
