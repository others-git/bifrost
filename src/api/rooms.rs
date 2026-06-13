//! Rooms: the user-owned grouping abstraction.
//!
//! A room aggregates **links** to provider-group mirrors (synced from the
//! provider — Hue rooms/zones) plus **direct lights** (for providers without
//! a native grouping concept). Effective membership is the union of both.
//!
//! Control prefers the provider's native group call (Hue `grouped_light`,
//! one request for the whole room) when a provider's members all come from a
//! single linked group; otherwise it fans out per light in parallel.

use crate::AppState;
use crate::api::audio::apply_audio_command;
use crate::api::auth::require_session;
use crate::api::lights::build_provider;
use crate::models::LightState;
use crate::models::audio::AudioCommand;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_rooms).post(create_room))
        .route("/{id}", delete(remove_room))
        .route("/{id}/merge", post(merge_rooms))
        .route("/{id}/audio", put(set_room_audio_devices))
        .route("/{id}/audio/state", put(set_room_audio_state))
        .route("/{id}/enabled", put(set_room_enabled))
        .route("/{id}/lights", put(set_direct_lights))
        .route("/{id}/power", put(set_room_power_devices))
        .route("/{id}/links", put(set_links))
        .route("/{id}/state", put(set_room_state))
        .route("/{id}/scenes/{scene_id}/apply", post(apply_scene))
}

/// Read-only list of provider-group mirrors, for the link-editing UI.
pub fn provider_groups_router() -> Router<Arc<AppState>> {
    Router::new().route("/", get(list_provider_groups))
}

// ── Wire types ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct LinkInfo {
    provider_group_id: String,
    name: String,
    provider_id: String,
    /// "light" or "audio" — which domain this linked provider room/zone is.
    domain: String,
}

#[derive(Serialize)]
struct RoomInfo {
    id: String,
    name: String,
    /// Union of linked provider-group members and direct lights.
    light_ids: Vec<String>,
    direct_light_ids: Vec<String>,
    links: Vec<LinkInfo>,
    /// Audio devices this room controls (volume/mute fans out to all), each
    /// with its per-room volume offset.
    audio_devices: Vec<RoomAudioMember>,
    /// Power devices (switches/plugs/fans) the room contains.
    power_device_ids: Vec<String>,
    /// Disabled rooms are hidden from the Dashboard/Floor Plan and the public
    /// API, but still listed in Settings so they can be re-enabled.
    enabled: bool,
}

#[derive(Serialize)]
struct ProviderGroupInfo {
    id: String,
    provider_id: String,
    provider_group_id: String,
    name: String,
    /// The group's primary domain label (from the provider type). An area can
    /// still carry members across domains — see the `*_ids` lists below.
    domain: String,
    /// Member lights.
    light_ids: Vec<String>,
    /// Member audio devices.
    audio_device_ids: Vec<String>,
    /// Member power devices (switches/plugs/fans).
    power_device_ids: Vec<String>,
}

/// "audio" if the provider type is a registered audio provider, else "light".
fn domain_label(state: &AppState, provider_type: &str) -> &'static str {
    if state.registry.is_known_audio(provider_type) {
        "audio"
    } else {
        "light"
    }
}

// ── Membership resolution ────────────────────────────────────────────────────

pub(crate) struct MemberRow {
    pub(crate) light_id: String,
    pub(crate) device_id: String,
    pub(crate) provider_type: String,
    pub(crate) credentials: String,
}

/// All effective members of a room (links ∪ direct), with provider info,
/// deduplicated, ordered by light name for stable palette distribution.
pub(crate) async fn effective_members(state: &AppState, room_id: &str) -> Vec<MemberRow> {
    sqlx::query(
        "SELECT DISTINCT l.id AS light_id, l.device_id, l.provider_id,
                p.provider_type, p.credentials, l.name
         FROM lights l
         JOIN providers p ON p.id = l.provider_id
         WHERE p.enabled = 1 AND l.id IN (
             SELECT light_id FROM room_lights WHERE room_id = ?1
             UNION
             SELECT pgl.light_id
             FROM room_links rl
             JOIN provider_group_lights pgl ON pgl.provider_group_id = rl.provider_group_id
             WHERE rl.room_id = ?1
         )
         ORDER BY l.name",
    )
    .bind(room_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| MemberRow {
        light_id: r.get("light_id"),
        device_id: r.get("device_id"),
        provider_type: r.get("provider_type"),
        credentials: r.get("credentials"),
    })
    .collect()
}

/// Just the effective member light ids — the shape the room listings (session,
/// `/api/v1`, MCP) need. A dedicated lean query so the per-room listing path
/// doesn't fetch and decrypt-bearing columns (`provider_type`, `credentials`)
/// only to discard them; mirrors `effective_members`' filter/union/order.
pub(crate) async fn effective_member_ids(state: &AppState, room_id: &str) -> Vec<String> {
    sqlx::query(
        "SELECT DISTINCT l.id AS light_id, l.name
         FROM lights l
         JOIN providers p ON p.id = l.provider_id
         WHERE p.enabled = 1 AND l.id IN (
             SELECT light_id FROM room_lights WHERE room_id = ?1
             UNION
             SELECT pgl.light_id
             FROM room_links rl
             JOIN provider_group_lights pgl ON pgl.provider_group_id = rl.provider_group_id
             WHERE rl.room_id = ?1
         )
         ORDER BY l.name",
    )
    .bind(room_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| r.get("light_id"))
    .collect()
}

/// The room's effective power-device members (switches/plugs/fans), enabled
/// providers only: explicit membership (`room_power_devices`) ∪ devices from
/// linked provider-groups (a synced HA Area). Shared by the session and public
/// room listings.
pub(crate) async fn effective_power_member_ids(state: &AppState, room_id: &str) -> Vec<String> {
    sqlx::query(
        "SELECT pd.id AS power_device_id
         FROM power_devices pd
         JOIN providers p ON p.id = pd.provider_id
         WHERE p.enabled = 1
           AND pd.id IN (
               SELECT power_device_id FROM room_power_devices WHERE room_id = ?1
               UNION
               SELECT pgp.power_device_id
               FROM room_links rl
               JOIN provider_group_power_devices pgp
                 ON pgp.provider_group_id = rl.provider_group_id
               WHERE rl.room_id = ?1
           )
         ORDER BY pd.name",
    )
    .bind(room_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| r.get("power_device_id"))
    .collect()
}

/// One audio device's membership in a room, with its per-room volume offset.
#[derive(Serialize)]
pub(crate) struct RoomAudioMember {
    pub(crate) audio_device_id: String,
    /// Signed %, added to the room volume then clamped 0–100 per device.
    pub(crate) volume_offset: i64,
}

/// A room's effective audio devices — the audio analog of `effective_members`
/// (lights): explicit membership (`room_audio_devices`) ∪ devices from linked
/// audio provider-groups (`room_links` → `provider_group_audio_devices`), with
/// the per-room `volume_offset` (0 when there's no explicit row). Shared by the
/// session and v1 room listings so they can't drift.
pub(crate) async fn effective_audio_members(
    state: &AppState,
    room_id: &str,
) -> Vec<RoomAudioMember> {
    sqlx::query(
        "SELECT d.id AS audio_device_id, COALESCE(rad.volume_offset, 0) AS volume_offset
         FROM audio_devices d
         LEFT JOIN room_audio_devices rad
           ON rad.room_id = ?1 AND rad.audio_device_id = d.id
         WHERE d.id IN (
             SELECT audio_device_id FROM room_audio_devices WHERE room_id = ?1
             UNION
             SELECT pga.audio_device_id
             FROM room_links rl
             JOIN provider_group_audio_devices pga ON pga.provider_group_id = rl.provider_group_id
             WHERE rl.room_id = ?1
         )
         ORDER BY d.name",
    )
    .bind(room_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| RoomAudioMember {
        audio_device_id: r.get("audio_device_id"),
        volume_offset: r.get("volume_offset"),
    })
    .collect()
}

/// A room as exposed to third parties (the public `/api/v1` API and the MCP
/// surface): enabled rooms only, with effective light and audio membership.
/// Shared so the two surfaces can't drift.
#[derive(Serialize)]
pub(crate) struct PublicRoom {
    pub id: String,
    pub name: String,
    /// Effective members (linked provider-group lights ∪ direct lights).
    pub light_ids: Vec<String>,
    /// Audio devices the room controls — drive each via /audio/devices/{id}/state.
    pub audio_device_ids: Vec<String>,
    /// Power devices the room contains — drive each via /power/devices/{id}/state.
    pub power_device_ids: Vec<String>,
}

pub(crate) async fn list_public_rooms(state: &AppState) -> Vec<PublicRoom> {
    // Disabled rooms are hidden from the public surfaces too.
    let rows = sqlx::query("SELECT id, name FROM rooms WHERE enabled = 1 ORDER BY created_at")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let id: String = row.get("id");
        let light_ids = effective_member_ids(state, &id).await;
        let audio_device_ids = effective_audio_members(state, &id)
            .await
            .into_iter()
            .map(|m| m.audio_device_id)
            .collect();
        let power_device_ids = effective_power_member_ids(state, &id).await;
        out.push(PublicRoom {
            name: row.get("name"),
            light_ids,
            audio_device_ids,
            power_device_ids,
            id,
        });
    }
    out
}

// ── Handlers ────────────────────────────────────────────────────────────────

async fn list_rooms(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let rooms = match sqlx::query("SELECT id, name, enabled FROM rooms ORDER BY created_at")
        .fetch_all(&state.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("db error listing rooms: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let mut out = Vec::with_capacity(rooms.len());
    for room in rooms {
        let id: String = room.get("id");

        let direct: Vec<String> = sqlx::query("SELECT light_id FROM room_lights WHERE room_id = ?")
            .bind(&id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| r.get("light_id"))
            .collect();

        let links: Vec<LinkInfo> = sqlx::query(
            "SELECT pg.id, pg.name, pg.provider_id, p.provider_type
             FROM room_links rl
             JOIN provider_groups pg ON pg.id = rl.provider_group_id
             JOIN providers p ON p.id = pg.provider_id
             WHERE rl.room_id = ?",
        )
        .bind(&id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            let provider_type: String = r.get("provider_type");
            LinkInfo {
                provider_group_id: r.get("id"),
                name: r.get("name"),
                provider_id: r.get("provider_id"),
                domain: domain_label(&state, &provider_type).to_string(),
            }
        })
        .collect();

        let audio_devices = effective_audio_members(&state, &id).await;
        let power_device_ids = effective_power_member_ids(&state, &id).await;

        out.push(RoomInfo {
            light_ids: effective_member_ids(&state, &id).await,
            direct_light_ids: direct,
            links,
            audio_devices,
            power_device_ids,
            enabled: room.get::<i64, _>("enabled") != 0,
            id,
            name: room.get("name"),
        });
    }

    Json(out).into_response()
}

#[derive(Deserialize)]
struct SetRoomEnabledRequest {
    enabled: bool,
}

/// Enable or disable a room. Disabled rooms are hidden from the Dashboard, Floor
/// Plan, and the public API, but kept (and re-enableable) in Settings.
async fn set_room_enabled(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<SetRoomEnabledRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !room_exists(&state, &id).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    let _ = sqlx::query("UPDATE rooms SET enabled = ? WHERE id = ?")
        .bind(if req.enabled { 1 } else { 0 })
        .bind(&id)
        .execute(&state.db)
        .await;
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct SetRoomAudioRequest {
    /// Replaces the room's explicit audio membership. Synced audio-group
    /// members stay live via room_links; include a device here to set its
    /// offset (or to add it manually).
    #[serde(default)]
    devices: Vec<RoomAudioInput>,
}

#[derive(Deserialize)]
struct RoomAudioInput {
    audio_device_id: String,
    #[serde(default)]
    volume_offset: i64,
}

/// Set the room's explicit audio devices + per-device volume offsets (replace-
/// all, like `set_links`). Effective membership also includes synced audio-
/// group devices via room_links.
async fn set_room_audio_devices(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<SetRoomAudioRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !room_exists(&state, &id).await {
        return StatusCode::NOT_FOUND.into_response();
    }

    for d in &req.devices {
        let known = sqlx::query("SELECT 1 FROM audio_devices WHERE id = ?")
            .bind(&d.audio_device_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .is_some();
        if !known {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("unknown audio device '{}'", d.audio_device_id),
            )
                .into_response();
        }
    }

    let _ = sqlx::query("DELETE FROM room_audio_devices WHERE room_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;
    for d in &req.devices {
        let _ = sqlx::query(
            "INSERT OR REPLACE INTO room_audio_devices (room_id, audio_device_id, volume_offset)
             VALUES (?, ?, ?)",
        )
        .bind(&id)
        .bind(&d.audio_device_id)
        .bind(d.volume_offset)
        .execute(&state.db)
        .await;
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct RoomAudioStateRequest {
    #[serde(default)]
    volume: Option<u8>,
    #[serde(default)]
    mute: Option<bool>,
}

/// Fan a volume/mute command out to every audio device in the room, applying
/// each device's per-room offset to the volume (clamped 0–100).
async fn set_room_audio_state(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<RoomAudioStateRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let members = effective_audio_members(&state, &id).await;
    if members.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }

    // Fan out to every audio member concurrently — a room's speakers are
    // distinct devices (Sonos units on their own IPs), so a room volume change
    // should hit them in parallel rather than serially round-tripping each.
    let jobs = members.iter().filter_map(|m| {
        let volume = req
            .volume
            .map(|v| (v as i64 + m.volume_offset).clamp(0, 100) as u8);
        let cmd = AudioCommand {
            volume,
            mute: req.mute,
            ..Default::default()
        };
        if cmd.is_empty() {
            return None;
        }
        let state = &state;
        Some(async move {
            matches!(
                apply_audio_command(state, &m.audio_device_id, &cmd).await,
                crate::api::audio::SetAudioOutcome::Ok
            )
        })
    });
    let results = futures_util::future::join_all(jobs).await;
    let applied = results.iter().filter(|ok| **ok).count();
    let failed = results.len() - applied;
    Json(serde_json::json!({ "applied": applied, "failed": failed })).into_response()
}

/// Read a provider group's member device ids from one `provider_group_*` table.
/// Table/column are fixed identifiers, so the formatted SQL is injection-free.
async fn group_member_ids(state: &AppState, group_id: &str, table: &str, col: &str) -> Vec<String> {
    sqlx::query(&format!(
        "SELECT {col} AS m FROM {table} WHERE provider_group_id = ?"
    ))
    .bind(group_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|row| row.get::<String, _>("m"))
    .collect()
}

async fn list_provider_groups(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let rows = sqlx::query(
        "SELECT pg.id, pg.provider_id, pg.provider_group_id, pg.name, p.provider_type
         FROM provider_groups pg JOIN providers p ON p.id = pg.provider_id
         ORDER BY pg.name",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let id: String = r.get("id");
        let provider_type: String = r.get("provider_type");
        let domain = domain_label(&state, &provider_type);
        // Query every member table — an area (HA) can mix domains, so the label
        // alone can't tell us which members it has.
        let light_ids = group_member_ids(&state, &id, "provider_group_lights", "light_id").await;
        let audio_device_ids = group_member_ids(
            &state,
            &id,
            "provider_group_audio_devices",
            "audio_device_id",
        )
        .await;
        let power_device_ids = group_member_ids(
            &state,
            &id,
            "provider_group_power_devices",
            "power_device_id",
        )
        .await;
        out.push(ProviderGroupInfo {
            provider_id: r.get("provider_id"),
            provider_group_id: r.get("provider_group_id"),
            name: r.get("name"),
            domain: domain.to_string(),
            light_ids,
            audio_device_ids,
            power_device_ids,
            id,
        });
    }

    Json(out).into_response()
}

#[derive(Deserialize)]
struct CreateRoomRequest {
    name: String,
    #[serde(default)]
    light_ids: Vec<String>,
}

async fn create_room(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateRoomRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if req.name.trim().is_empty() {
        return (StatusCode::UNPROCESSABLE_ENTITY, "room name is required").into_response();
    }

    // Case-insensitive duplicate guard: "office" next to "Office" is how
    // unmergeable near-duplicates were born.
    let duplicate = sqlx::query("SELECT 1 FROM rooms WHERE name = ? COLLATE NOCASE")
        .bind(req.name.trim())
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .is_some();
    if duplicate {
        return (StatusCode::CONFLICT, "a room with this name already exists").into_response();
    }

    let id = Uuid::new_v4().to_string();
    if let Err(e) = sqlx::query("INSERT INTO rooms (id, name) VALUES (?, ?)")
        .bind(&id)
        .bind(req.name.trim())
        .execute(&state.db)
        .await
    {
        tracing::error!("db error: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    for light_id in &req.light_ids {
        let _ = sqlx::query("INSERT OR IGNORE INTO room_lights (room_id, light_id) VALUES (?, ?)")
            .bind(&id)
            .bind(light_id)
            .execute(&state.db)
            .await;
    }

    (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response()
}

async fn remove_room(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let _ = sqlx::query("DELETE FROM rooms WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct MergeRequest {
    /// The room to absorb. Its links, direct lights, and plan-region bindings
    /// move to `{id}` (the target), then it is deleted.
    source_room_id: String,
}

/// Merge `source_room_id` into the target room. The target keeps its own
/// name; everything the source owned is re-pointed (memberships dedupe via
/// INSERT OR IGNORE).
async fn merge_rooms(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<MergeRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if req.source_room_id == id {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "cannot merge a room into itself",
        )
            .into_response();
    }
    if !room_exists(&state, &id).await || !room_exists(&state, &req.source_room_id).await {
        return StatusCode::NOT_FOUND.into_response();
    }

    // Links and direct lights: copy with dedupe, then let the source's
    // rows die with the room (ON DELETE CASCADE).
    let _ = sqlx::query(
        "INSERT OR IGNORE INTO room_links (room_id, provider_group_id)
         SELECT ?, provider_group_id FROM room_links WHERE room_id = ?",
    )
    .bind(&id)
    .bind(&req.source_room_id)
    .execute(&state.db)
    .await;
    let _ = sqlx::query(
        "INSERT OR IGNORE INTO room_lights (room_id, light_id)
         SELECT ?, light_id FROM room_lights WHERE room_id = ?",
    )
    .bind(&id)
    .bind(&req.source_room_id)
    .execute(&state.db)
    .await;
    // Audio + power membership move the same way (keep the target's offset for
    // any audio device already shared, via OR IGNORE on the PK).
    let _ = sqlx::query(
        "INSERT OR IGNORE INTO room_audio_devices (room_id, audio_device_id, volume_offset)
         SELECT ?, audio_device_id, volume_offset FROM room_audio_devices WHERE room_id = ?",
    )
    .bind(&id)
    .bind(&req.source_room_id)
    .execute(&state.db)
    .await;
    let _ = sqlx::query(
        "INSERT OR IGNORE INTO room_power_devices (room_id, power_device_id)
         SELECT ?, power_device_id FROM room_power_devices WHERE room_id = ?",
    )
    .bind(&id)
    .bind(&req.source_room_id)
    .execute(&state.db)
    .await;

    // Plan-region bindings move wholesale. (Scenes are global, not room-bound.)
    let _ = sqlx::query("UPDATE plan_rooms SET room_id = ? WHERE room_id = ?")
        .bind(&id)
        .bind(&req.source_room_id)
        .execute(&state.db)
        .await;

    let _ = sqlx::query("DELETE FROM rooms WHERE id = ?")
        .bind(&req.source_room_id)
        .execute(&state.db)
        .await;

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct SetLightsRequest {
    light_ids: Vec<String>,
}

/// Replace the room's DIRECT lights (linked members are unaffected).
async fn set_direct_lights(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<SetLightsRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !room_exists(&state, &id).await {
        return StatusCode::NOT_FOUND.into_response();
    }

    let _ = sqlx::query("DELETE FROM room_lights WHERE room_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;
    for light_id in &req.light_ids {
        let _ = sqlx::query("INSERT OR IGNORE INTO room_lights (room_id, light_id) VALUES (?, ?)")
            .bind(&id)
            .bind(light_id)
            .execute(&state.db)
            .await;
    }

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct SetRoomPowerRequest {
    power_device_ids: Vec<String>,
}

/// Replace the room's power-device membership.
async fn set_room_power_devices(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<SetRoomPowerRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !room_exists(&state, &id).await {
        return StatusCode::NOT_FOUND.into_response();
    }

    for pid in &req.power_device_ids {
        let known = sqlx::query("SELECT 1 FROM power_devices WHERE id = ?")
            .bind(pid)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .is_some();
        if !known {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("unknown power device '{pid}'"),
            )
                .into_response();
        }
    }

    let _ = sqlx::query("DELETE FROM room_power_devices WHERE room_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;
    for pid in &req.power_device_ids {
        let _ = sqlx::query(
            "INSERT OR IGNORE INTO room_power_devices (room_id, power_device_id) VALUES (?, ?)",
        )
        .bind(&id)
        .bind(pid)
        .execute(&state.db)
        .await;
    }

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct SetLinksRequest {
    provider_group_ids: Vec<String>,
}

/// Replace the room's provider-group links.
async fn set_links(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<SetLinksRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !room_exists(&state, &id).await {
        return StatusCode::NOT_FOUND.into_response();
    }

    let _ = sqlx::query("DELETE FROM room_links WHERE room_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;
    for pg_id in &req.provider_group_ids {
        let _ = sqlx::query(
            "INSERT OR IGNORE INTO room_links (room_id, provider_group_id) VALUES (?, ?)",
        )
        .bind(&id)
        .bind(pg_id)
        .execute(&state.db)
        .await;
    }

    StatusCode::NO_CONTENT.into_response()
}

pub(crate) async fn room_exists(state: &AppState, id: &str) -> bool {
    sqlx::query("SELECT 1 FROM rooms WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .is_some()
}

// ── Room state (native group control where possible) ────────────────────────

/// A provider's portion of the room that can be driven with one native call.
struct NativeChunk {
    provider_type: String,
    credentials: String,
    grouped_ref: String,
    light_ids: Vec<String>,
}

/// For each provider: if the room's members from that provider all come from
/// exactly one linked group with a native handle (and no direct lights of
/// that provider), control natively. Returns the chunks plus the light IDs
/// they cover.
async fn native_chunks(state: &AppState, room_id: &str) -> Vec<NativeChunk> {
    let links = sqlx::query(
        "SELECT pg.id, pg.provider_id, pg.grouped_ref, p.provider_type, p.credentials
         FROM room_links rl
         JOIN provider_groups pg ON pg.id = rl.provider_group_id
         JOIN providers p ON p.id = pg.provider_id
         WHERE rl.room_id = ? AND p.enabled = 1",
    )
    .bind(room_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    // provider_id → links of that provider
    let mut by_provider: HashMap<String, Vec<&sqlx::sqlite::SqliteRow>> = HashMap::new();
    for l in &links {
        by_provider
            .entry(l.get::<String, _>("provider_id"))
            .or_default()
            .push(l);
    }

    let mut chunks = Vec::new();
    for (provider_id, provider_links) in by_provider {
        if provider_links.len() != 1 {
            continue; // multiple linked groups → fan out, simplest correct path
        }
        let link = provider_links[0];
        let Some(grouped_ref) = link.get::<Option<String>, _>("grouped_ref") else {
            continue;
        };

        // Any direct lights from this provider? Then the native call would
        // miss them — fan out instead.
        let direct_count: i64 = sqlx::query(
            "SELECT COUNT(*) AS n FROM room_lights rl
             JOIN lights l ON l.id = rl.light_id
             WHERE rl.room_id = ? AND l.provider_id = ?",
        )
        .bind(room_id)
        .bind(&provider_id)
        .fetch_one(&state.db)
        .await
        .map(|r| r.get("n"))
        .unwrap_or(0);
        if direct_count > 0 {
            continue;
        }

        let light_ids: Vec<String> =
            sqlx::query("SELECT light_id FROM provider_group_lights WHERE provider_group_id = ?")
                .bind(link.get::<String, _>("id"))
                .fetch_all(&state.db)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|r| r.get("light_id"))
                .collect();

        chunks.push(NativeChunk {
            provider_type: link.get("provider_type"),
            credentials: link.get("credentials"),
            grouped_ref,
            light_ids,
        });
    }
    chunks
}

/// Drive every effective member of a room to one **uniform** state, preferring
/// native group calls (one request per linked provider group) and fanning out
/// per-light only for members no group call covered. Returns (applied, failed).
///
/// `members` is passed in so callers that already resolved membership (e.g. to
/// check emptiness) don't query twice.
pub(crate) async fn apply_uniform_state(
    state: &AppState,
    room_id: &str,
    new_state: &LightState,
    members: Vec<MemberRow>,
) -> (usize, usize) {
    let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut applied = 0usize;
    let mut failed = 0usize;
    let state_json = serde_json::to_string(new_state).unwrap_or_default();

    // Native group calls first — one Hue grouped_light PUT replaces N per-light PUTs.
    for chunk in native_chunks(state, room_id).await {
        let provider = match build_provider(state, &chunk.provider_type, &chunk.credentials) {
            Ok(p) => p,
            Err(_) => continue,
        };
        match provider
            .set_group_state(&chunk.grouped_ref, new_state)
            .await
        {
            Ok(true) => {
                for light_id in &chunk.light_ids {
                    covered.insert(light_id.clone());
                    let _ = sqlx::query(
                        "UPDATE lights SET last_state = ?, last_seen = datetime('now') WHERE id = ?",
                    )
                    .bind(&state_json)
                    .bind(light_id)
                    .execute(&state.db)
                    .await;
                }
                applied += chunk.light_ids.len();
            }
            Ok(false) => {} // provider has no native control — fan out below
            Err(e) => {
                tracing::warn!("native group control failed, falling back per-light: {e:#}");
            }
        }
    }

    // Per-light fan-out for everything not covered natively. Build one provider
    // per distinct credential set so same-provider lights share a single HTTP
    // client (and its keep-alive connection pool) across the concurrent fan-out,
    // rather than rebuilding a client — and reopening a connection — per light.
    let mut providers: std::collections::HashMap<
        String,
        Option<Arc<dyn crate::providers::LightProvider>>,
    > = std::collections::HashMap::new();
    let mut jobs = Vec::new();
    for m in members {
        if covered.contains(&m.light_id) {
            continue;
        }
        let provider = providers
            .entry(m.credentials.clone())
            .or_insert_with(
                || match build_provider(state, &m.provider_type, &m.credentials) {
                    Ok(p) => Some(Arc::from(p)),
                    Err(e) => {
                        tracing::error!("room state: provider build failed: {e:#}");
                        None
                    }
                },
            )
            .clone();
        let Some(provider) = provider else {
            failed += 1;
            continue;
        };
        let db = state.db.clone();
        let target = new_state.clone();
        let target_json = state_json.clone();
        jobs.push(async move {
            match provider.set_state(&m.device_id, &target).await {
                Ok(()) => {
                    let _ = sqlx::query(
                        "UPDATE lights SET last_state = ?, last_seen = datetime('now') WHERE id = ?",
                    )
                    .bind(&target_json)
                    .bind(&m.light_id)
                    .execute(&db)
                    .await;
                    true
                }
                Err(e) => {
                    tracing::error!("room state: set_state failed for {}: {e:#}", m.device_id);
                    false
                }
            }
        });
    }
    let results = futures_util::future::join_all(jobs).await;
    applied += results.iter().filter(|ok| **ok).count();
    failed += results.iter().filter(|ok| !**ok).count();

    (applied, failed)
}

async fn set_room_state(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(new_state): Json<LightState>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let members = effective_members(&state, &id).await;
    if members.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let (applied, failed) = apply_uniform_state(&state, &id, &new_state, members).await;
    Json(serde_json::json!({ "applied": applied, "failed": failed })).into_response()
}

// ── Scene apply (global palette scenes, applied to a room) ───────────────────
//
// Scene definitions live in `crate::api::palette_scenes` (global, not bound to a
// room). Applying one to a room is the room's concern, so the route lives here.

async fn apply_scene(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((room_id, scene_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match crate::api::palette_scenes::apply_scene_to_room(&state, &scene_id, &room_id).await {
        Some((applied, failed)) => {
            Json(serde_json::json!({ "applied": applied, "failed": failed })).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
