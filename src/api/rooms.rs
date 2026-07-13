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
use crate::api::auth::Session;
use crate::api::lights::build_provider;
use crate::api::media::{SetMediaOutcome, apply_media_command};
use crate::api::power::{SetPowerOutcome, apply_power_state};
use crate::models::LightState;
use crate::models::media::MediaCommand;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
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
        .route("/{id}/media", put(set_room_media_devices))
        .route("/{id}/media/state", put(set_room_media_state))
        .route("/{id}/enabled", put(set_room_enabled))
        .route("/{id}/lights", put(set_direct_lights))
        .route("/{id}/power", put(set_room_power_devices))
        .route("/{id}/links", put(set_links))
        .route("/{id}/controls", put(set_room_controls))
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
    /// "light" or "media" — which domain this linked provider room/zone is.
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
    /// Media devices this room controls (volume/mute fans out to all), each
    /// with its per-room volume offset.
    media_devices: Vec<RoomMediaMember>,
    /// Power devices (switches/plugs/fans) the room contains.
    power_device_ids: Vec<String>,
    /// User-configured quick-control buttons rendered on the room's Control card.
    controls: Vec<RoomControl>,
    /// Disabled rooms are hidden from the Dashboard/Floor Plan and the public
    /// API, but still listed in Settings so they can be re-enabled.
    enabled: bool,
}

/// A configured quick-control button (see migration 0034). `kind` decides the
/// action; `targets` names the devices it acts on (empty for `scene`, which uses
/// `scene_id`).
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct RoomControl {
    #[serde(default)]
    id: String,
    /// power | volume | brightness | scene
    kind: String,
    /// A Glyph registry name (frontend `components/glyphs`).
    glyph: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    targets: Vec<ControlTarget>,
    #[serde(default)]
    scene_id: Option<String>,
}

/// One device a control acts on. `domain` ∈ light | media | power.
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct ControlTarget {
    domain: String,
    id: String,
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
    /// Member media devices.
    media_device_ids: Vec<String>,
    /// Member power devices (switches/plugs/fans).
    power_device_ids: Vec<String>,
}

/// "media" if the provider type is a registered media provider, else "light".
fn domain_label(state: &AppState, provider_type: &str) -> &'static str {
    if state.registry.is_known_media(provider_type) {
        "media"
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

/// The shared FROM/WHERE/UNION/ORDER body for a room's effective light members —
/// direct (`room_lights`) ∪ linked provider-group lights, enabled + unshadowed,
/// ordered by name. `effective_members` and `effective_member_ids` select
/// different columns over this **same** predicate, so the membership rule can't
/// drift between them.
const ROOM_LIGHT_MEMBER_BODY: &str = "
    FROM lights l
    JOIN providers p ON p.id = l.provider_id
    WHERE p.enabled = 1 AND l.enabled = 1 AND l.shadowed_by IS NULL AND l.id IN (
        SELECT light_id FROM room_lights WHERE room_id = ?1
        UNION
        SELECT pgl.light_id
        FROM room_links rl
        JOIN provider_group_lights pgl ON pgl.provider_group_id = rl.provider_group_id
        WHERE rl.room_id = ?1
        -- A direct assignment (Devices page) is authoritative: once a light has a
        -- direct room, its provider-group inheritance is suppressed everywhere, so
        -- it appears only in the room it was assigned to (no cross-room duplicate).
        AND pgl.light_id NOT IN (SELECT light_id FROM room_lights)
    )
    ORDER BY l.name";

/// All effective members of a room (links ∪ direct), with provider info,
/// deduplicated, ordered by light name for stable palette distribution.
pub(crate) async fn effective_members(state: &AppState, room_id: &str) -> Vec<MemberRow> {
    sqlx::query(&format!(
        "SELECT DISTINCT l.id AS light_id, l.device_id, l.provider_id, \
                p.provider_type, p.credentials, l.name {ROOM_LIGHT_MEMBER_BODY}"
    ))
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
    sqlx::query(&format!(
        "SELECT DISTINCT l.id AS light_id, l.name {ROOM_LIGHT_MEMBER_BODY}"
    ))
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
         WHERE p.enabled = 1 AND pd.enabled = 1 AND pd.shadowed_by IS NULL
           AND pd.id IN (
               SELECT power_device_id FROM room_power_devices WHERE room_id = ?1
               UNION
               SELECT pgp.power_device_id
               FROM room_links rl
               JOIN provider_group_power_devices pgp
                 ON pgp.provider_group_id = rl.provider_group_id
               WHERE rl.room_id = ?1
               -- A direct assignment is authoritative: a directly-assigned device
               -- drops its provider-group inheritance everywhere (no duplicate).
               AND pgp.power_device_id NOT IN (SELECT power_device_id FROM room_power_devices)
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

/// Occupancy of a room from its **presence** sensors (motion / occupancy). The
/// provider-agnostic room property that presence-driven behaviour reads:
/// - `None` — the room has no presence sensors, so occupancy is unknown here.
/// - `Some(true)` — at least one presence member is currently detecting.
/// - `Some(false)` — it has presence members but none are detecting.
///
/// Non-presence sensors (contact/lux/temp/humidity) and disabled/shadowed members
/// are ignored, so this is safe to read from any surface (kiosk scheduler today).
pub(crate) async fn room_occupancy(state: &AppState, room_id: &str) -> Option<bool> {
    let members = room_presence_readings(state, room_id).await;
    (!members.is_empty()).then(|| members.iter().any(|(_, detecting)| *detecting))
}

/// Each of a room's enabled, non-shadowed **presence** members (motion /
/// occupancy) with whether its cached reading is currently detecting. The
/// member list behind [`room_occupancy`]; the automation engine also reads it
/// directly so it can overlay its own fresher in-memory readings (the DB cache
/// is written by a separate task and can trail a push event by a beat).
pub(crate) async fn room_presence_readings(state: &AppState, room_id: &str) -> Vec<(String, bool)> {
    use crate::models::sensor::SensorState;
    let rows = sqlx::query(
        "SELECT sd.id AS id, sd.kind AS kind, sd.last_state AS last_state
         FROM sensor_devices sd
         JOIN providers p ON p.id = sd.provider_id
         WHERE p.enabled = 1 AND sd.enabled = 1 AND sd.shadowed_by IS NULL
           AND sd.id IN (
               SELECT sensor_device_id FROM room_sensor_devices WHERE room_id = ?1
               UNION
               SELECT pgs.sensor_device_id
               FROM room_links rl
               JOIN provider_group_sensor_devices pgs
                 ON pgs.provider_group_id = rl.provider_group_id
               WHERE rl.room_id = ?1
               AND pgs.sensor_device_id NOT IN (SELECT sensor_device_id FROM room_sensor_devices)
           )",
    )
    .bind(room_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    rows.into_iter()
        .filter(|r| crate::api::sensors::parse_kind(&r.get::<String, _>("kind")).is_presence())
        .map(|r| {
            let st: SensorState = r
                .get::<Option<String>, _>("last_state")
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            (r.get("id"), st.is_detecting())
        })
        .collect()
}

/// Assign a device to (at most) one room from the *device* side — the knob the
/// Devices page uses, the counterpart to the room-centric membership setters.
/// Clears the device's existing direct membership in `member_table`, then adds
/// it to `room_id` (or leaves it unassigned when `None`). `member_table` /
/// `device_col` are fixed per-domain identifiers, so the formatted SQL is
/// injection-free. Uses `INSERT OR IGNORE`, so a bad device or room id (FK
/// violation) is skipped → `NOT_FOUND` rather than a 500. Only *direct*
/// membership is touched; room links (synced provider groups) are managed on the
/// Rooms page.
pub(crate) async fn set_device_room(
    state: &AppState,
    device_table: &str,
    member_table: &str,
    device_col: &str,
    device_id: &str,
    room_id: Option<String>,
) -> StatusCode {
    // Unknown device → 404 (matches the other device sub-resource setters).
    let exists = sqlx::query(&format!("SELECT 1 FROM {device_table} WHERE id = ?"))
        .bind(device_id)
        .fetch_optional(&state.db)
        .await;
    match exists {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND,
        Err(e) => {
            tracing::error!("db error checking {device_table} {device_id}: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    }

    // Validate the target room up front so a bad id is a clean 404, not an FK 500.
    if let Some(room_id) = &room_id {
        let room = sqlx::query("SELECT 1 FROM rooms WHERE id = ?")
            .bind(room_id)
            .fetch_optional(&state.db)
            .await;
        match room {
            Ok(Some(_)) => {}
            Ok(None) => return StatusCode::NOT_FOUND,
            Err(e) => {
                tracing::error!("db error checking room {room_id}: {e}");
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
        }
    }

    // One direct room per device: clear existing membership, then add the new one.
    if let Err(e) = sqlx::query(&format!(
        "DELETE FROM {member_table} WHERE {device_col} = ?"
    ))
    .bind(device_id)
    .execute(&state.db)
    .await
    {
        tracing::error!("db error clearing {member_table} for {device_id}: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    let Some(room_id) = room_id else {
        crate::api::notify_inventory(state, device_table);
        return StatusCode::NO_CONTENT; // unassigned
    };

    match sqlx::query(&format!(
        "INSERT OR IGNORE INTO {member_table} (room_id, {device_col}) VALUES (?, ?)"
    ))
    .bind(&room_id)
    .bind(device_id)
    .execute(&state.db)
    .await
    {
        Ok(_) => {
            crate::api::notify_inventory(state, device_table);
            StatusCode::NO_CONTENT
        }
        Err(e) => {
            tracing::error!("db error assigning {device_id} to room {room_id}: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// One media device's membership in a room, with its per-room volume offset.
#[derive(Serialize)]
pub(crate) struct RoomMediaMember {
    pub(crate) media_device_id: String,
    /// Signed %, added to the room volume then clamped 0–100 per device.
    pub(crate) volume_offset: i64,
}

/// A room's effective media devices — the media analog of `effective_members`
/// (lights): explicit membership (`room_media_devices`) ∪ devices from linked
/// media provider-groups (`room_links` → `provider_group_media_devices`), with
/// the per-room `volume_offset` (0 when there's no explicit row). Shared by the
/// session and v1 room listings so they can't drift.
pub(crate) async fn effective_media_members(
    state: &AppState,
    room_id: &str,
) -> Vec<RoomMediaMember> {
    // A composite must appear once — as its derived surface, not its hidden
    // companions. `group_surfaces` collapses each group the same way the device
    // list does, so this can't drift from the control surface.
    let surfaces = crate::api::media::group_surfaces(state).await;
    sqlx::query(
        "SELECT d.id AS media_device_id, d.group_id, COALESCE(rad.volume_offset, 0) AS volume_offset
         FROM media_devices d
         LEFT JOIN room_media_devices rad
           ON rad.room_id = ?1 AND rad.media_device_id = d.id
         WHERE d.enabled = 1 AND d.shadowed_by IS NULL AND d.id IN (
             SELECT media_device_id FROM room_media_devices WHERE room_id = ?1
             UNION
             SELECT pga.media_device_id
             FROM room_links rl
             JOIN provider_group_media_devices pga ON pga.provider_group_id = rl.provider_group_id
             WHERE rl.room_id = ?1
             -- A direct assignment (Devices page) is authoritative: a device with a
             -- direct room drops its provider-group inheritance everywhere, so a
             -- Sonos speaker moved to another room no longer dupes into its synced
             -- group's room.
             AND pga.media_device_id NOT IN (SELECT media_device_id FROM room_media_devices)
         )
         ORDER BY d.name",
    )
    .bind(room_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .filter(|r| {
        // Keep singletons and each group's surface; drop hidden companions.
        match r.get::<Option<String>, _>("group_id") {
            None => true,
            Some(g) => surfaces.get(&g).map(String::as_str) == Some(r.get::<String, _>("media_device_id").as_str()),
        }
    })
    .map(|r| RoomMediaMember {
        media_device_id: r.get("media_device_id"),
        volume_offset: r.get("volume_offset"),
    })
    .collect()
}

/// Of the given media device ids, the subset that is some *other* id's receiver
/// (M22) — i.e. a receiver whose volume is driven through a bound source also in
/// the set. Used to collapse a bound pair to a single volume target.
async fn receiver_targets_within(
    state: &AppState,
    ids: &[String],
) -> std::collections::HashSet<String> {
    if ids.is_empty() {
        return std::collections::HashSet::new();
    }
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT DISTINCT receiver_id FROM media_devices
         WHERE receiver_id IS NOT NULL AND id IN ({placeholders}) AND receiver_id IN ({placeholders})"
    );
    let mut q = sqlx::query(&sql);
    for id in ids {
        q = q.bind(id);
    }
    for id in ids {
        q = q.bind(id);
    }
    q.fetch_all(&state.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.get::<String, _>("receiver_id"))
        .collect()
}

/// A room as exposed to third parties (the public `/api/v1` API and the MCP
/// surface): enabled rooms only, with effective light and media membership.
/// Shared so the two surfaces can't drift.
#[derive(Serialize)]
pub(crate) struct PublicRoom {
    pub id: String,
    pub name: String,
    /// Effective members (linked provider-group lights ∪ direct lights).
    pub light_ids: Vec<String>,
    /// Media devices the room controls — drive each via /media/devices/{id}/state.
    pub media_device_ids: Vec<String>,
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
        let media_device_ids = effective_media_members(state, &id)
            .await
            .into_iter()
            .map(|m| m.media_device_id)
            .collect();
        let power_device_ids = effective_power_member_ids(state, &id).await;
        out.push(PublicRoom {
            name: row.get("name"),
            light_ids,
            media_device_ids,
            power_device_ids,
            id,
        });
    }
    out
}

// ── Handlers ────────────────────────────────────────────────────────────────

async fn list_rooms(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
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

        let media_devices = effective_media_members(&state, &id).await;
        let power_device_ids = effective_power_member_ids(&state, &id).await;
        let controls = list_room_controls(&state, &id).await;

        out.push(RoomInfo {
            light_ids: effective_member_ids(&state, &id).await,
            direct_light_ids: direct,
            links,
            media_devices,
            power_device_ids,
            controls,
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
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetRoomEnabledRequest>,
) -> impl IntoResponse {
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
struct SetRoomMediaRequest {
    /// Replaces the room's explicit media membership. Synced media-group
    /// members stay live via room_links; include a device here to set its
    /// offset (or to add it manually).
    #[serde(default)]
    devices: Vec<RoomMediaInput>,
}

#[derive(Deserialize)]
struct RoomMediaInput {
    media_device_id: String,
    #[serde(default)]
    volume_offset: i64,
}

/// Set the room's explicit media devices + per-device volume offsets (replace-
/// all, like `set_links`). Effective membership also includes synced media-
/// group devices via room_links.
async fn set_room_media_devices(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetRoomMediaRequest>,
) -> impl IntoResponse {
    if !room_exists(&state, &id).await {
        return StatusCode::NOT_FOUND.into_response();
    }

    for d in &req.devices {
        let known = sqlx::query("SELECT 1 FROM media_devices WHERE id = ?")
            .bind(&d.media_device_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .is_some();
        if !known {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("unknown media device '{}'", d.media_device_id),
            )
                .into_response();
        }
    }

    let _ = sqlx::query("DELETE FROM room_media_devices WHERE room_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;
    for d in &req.devices {
        // Guard: a disabled or shadowed device is not a valid room member —
        // skip it, so any room save also cleans out stale members (e.g. an HA
        // duplicate that was later disabled). Control already ignores them.
        let _ = sqlx::query(
            "INSERT OR REPLACE INTO room_media_devices (room_id, media_device_id, volume_offset)
             SELECT ?, id, ? FROM media_devices
              WHERE id = ? AND enabled = 1 AND shadowed_by IS NULL",
        )
        .bind(&id)
        .bind(d.volume_offset)
        .bind(&d.media_device_id)
        .execute(&state.db)
        .await;
    }
    StatusCode::NO_CONTENT.into_response()
}

/// The room's configured quick-control buttons, ordered for display. Targets
/// are decoded from the stored JSON; a malformed row degrades to empty targets
/// rather than failing the whole listing.
pub(crate) async fn list_room_controls(state: &AppState, room_id: &str) -> Vec<RoomControl> {
    sqlx::query(
        "SELECT id, kind, glyph, label, targets, scene_id
         FROM room_controls WHERE room_id = ? ORDER BY position, created_at",
    )
    .bind(room_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| {
        let targets_json: String = r.get("targets");
        RoomControl {
            id: r.get("id"),
            kind: r.get("kind"),
            glyph: r.get("glyph"),
            label: r.get("label"),
            targets: serde_json::from_str(&targets_json).unwrap_or_default(),
            scene_id: r.get("scene_id"),
        }
    })
    .collect()
}

#[derive(Deserialize)]
struct SetRoomControlsRequest {
    /// Replaces the room's controls wholesale (add/edit/remove/reorder), like
    /// `set_links`. Array order becomes each control's display `position`.
    #[serde(default)]
    controls: Vec<RoomControl>,
}

/// A control kind must name one of the four supported actions.
fn valid_control_kind(kind: &str) -> bool {
    matches!(kind, "power" | "volume" | "brightness" | "scene")
}

/// Confirm a target device id exists in the table for its domain.
async fn target_exists(state: &AppState, t: &ControlTarget) -> bool {
    let table = match t.domain.as_str() {
        "light" => "lights",
        "media" => "media_devices",
        "power" => "power_devices",
        _ => return false,
    };
    sqlx::query(&format!("SELECT 1 FROM {table} WHERE id = ?"))
        .bind(&t.id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .is_some()
}

/// Replace the room's configured quick-control buttons. Validates each control's
/// kind, glyph, and targets (a `scene` needs a known `scene_id`; the others need
/// at least one valid target device). Replace-all, mirroring `set_links`.
async fn set_room_controls(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetRoomControlsRequest>,
) -> impl IntoResponse {
    if !room_exists(&state, &id).await {
        return StatusCode::NOT_FOUND.into_response();
    }

    for c in &req.controls {
        if !valid_control_kind(&c.kind) {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("unknown control kind '{}'", c.kind),
            )
                .into_response();
        }
        if c.glyph.trim().is_empty() {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                "control glyph is required",
            )
                .into_response();
        }
        if c.kind == "scene" {
            let ok = match &c.scene_id {
                Some(sid) => sqlx::query("SELECT 1 FROM scenes WHERE id = ?")
                    .bind(sid)
                    .fetch_optional(&state.db)
                    .await
                    .ok()
                    .flatten()
                    .is_some(),
                None => false,
            };
            if !ok {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "scene control needs a known scene_id",
                )
                    .into_response();
            }
        } else {
            if c.targets.is_empty() {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("'{}' control needs at least one target device", c.kind),
                )
                    .into_response();
            }
            for t in &c.targets {
                if !target_exists(&state, t).await {
                    return (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        format!("unknown {} device '{}'", t.domain, t.id),
                    )
                        .into_response();
                }
            }
        }
    }

    let _ = sqlx::query("DELETE FROM room_controls WHERE room_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;
    for (pos, c) in req.controls.iter().enumerate() {
        let targets_json = serde_json::to_string(&c.targets).unwrap_or_else(|_| "[]".into());
        let _ = sqlx::query(
            "INSERT INTO room_controls (id, room_id, kind, glyph, label, targets, scene_id, position)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&id)
        .bind(&c.kind)
        .bind(c.glyph.trim())
        .bind(&c.label)
        .bind(&targets_json)
        .bind(&c.scene_id)
        .bind(pos as i64)
        .execute(&state.db)
        .await;
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct RoomMediaStateRequest {
    #[serde(default)]
    volume: Option<u8>,
    #[serde(default)]
    mute: Option<bool>,
}

/// Fan a volume/mute command out to every media device in the room, applying
/// each device's per-room offset to the volume (clamped 0–100).
async fn set_room_media_state(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<RoomMediaStateRequest>,
) -> impl IntoResponse {
    let members = effective_media_members(&state, &id).await;
    if members.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }

    // A receiver that is the volume-target of another member in this room is
    // driven *through* that bound source (M22 routing), so skip it here — else
    // the room volume would hit the receiver twice (once direct, once routed),
    // a last-write-wins race when their offsets differ.
    let member_ids: Vec<String> = members.iter().map(|m| m.media_device_id.clone()).collect();
    let bound_targets = receiver_targets_within(&state, &member_ids).await;

    // Fan out to every media member concurrently — a room's speakers are
    // distinct devices (Sonos units on their own IPs), so a room volume change
    // should hit them in parallel rather than serially round-tripping each.
    let jobs = members
        .iter()
        .filter(|m| !bound_targets.contains(&m.media_device_id))
        .filter_map(|m| {
            let volume = req
                .volume
                .map(|v| (v as i64 + m.volume_offset).clamp(0, 100) as u8);
            let cmd = MediaCommand {
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
                    apply_media_command(state, &m.media_device_id, &cmd).await,
                    crate::api::media::SetMediaOutcome::Ok
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

async fn list_provider_groups(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
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
        let media_device_ids = group_member_ids(
            &state,
            &id,
            "provider_group_media_devices",
            "media_device_id",
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
            media_device_ids,
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
    _: Session,
    Json(req): Json<CreateRoomRequest>,
) -> impl IntoResponse {
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
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
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
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<MergeRequest>,
) -> impl IntoResponse {
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
    // Media + power membership move the same way (keep the target's offset for
    // any media device already shared, via OR IGNORE on the PK).
    let _ = sqlx::query(
        "INSERT OR IGNORE INTO room_media_devices (room_id, media_device_id, volume_offset)
         SELECT ?, media_device_id, volume_offset FROM room_media_devices WHERE room_id = ?",
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
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetLightsRequest>,
) -> impl IntoResponse {
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
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetRoomPowerRequest>,
) -> impl IntoResponse {
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
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetLinksRequest>,
) -> impl IntoResponse {
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
    // Whether to try the provider's native group call first. A native grouped
    // call (Hue grouped_light) hits EVERY group member — right for a whole-room
    // command, wrong for a lit-only cascade that must not wake off lights.
    use_native: bool,
) -> (usize, usize) {
    let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut applied = 0usize;
    let mut failed = 0usize;
    // `persist_light_state` merges only the attributes present in `new_state`, so
    // a partial command (pure on/off, or a brightness-/colour-only cascade) keeps
    // every member light's untouched dimensions intact.

    // Native group calls first — one Hue grouped_light PUT replaces N per-light PUTs.
    for chunk in native_chunks(state, room_id).await {
        if !use_native {
            break;
        }
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
                    crate::api::lights::persist_light_state(&state.db, light_id, new_state).await;
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
        jobs.push(async move {
            // Merge onto this member's own current state so a room brightness-/on-
            // only cascade doesn't cancel a member's running effect (and each
            // member keeps its own colour/mode).
            let merged = crate::api::lights::merge_light_state(
                crate::api::lights::current_light_state(&db, &m.light_id).await,
                &target,
            );
            match provider.set_state(&m.device_id, &merged).await {
                Ok(()) => {
                    crate::api::lights::write_light_state(&db, &m.light_id, &merged).await;
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

/// Apply a room on/off (+ color/brightness for lights) to **all** member
/// domains: lights via [`apply_uniform_state`], then the room's `on` state
/// fanned out to media members (power only) and power-device members. This is
/// the shared room-control path for the session, `/v1`, and MCP `set_room`, so
/// "turn the room on/off" means the whole room (CLAUDE.md's room model), not
/// just its lights. (Palette-scene apply stays lights-only and keeps calling
/// `apply_uniform_state` directly — a color scene shouldn't toggle switches.)
/// The room-state entry point for the UI's patch-shaped body: an EXPLICIT `on`
/// is power intent and routes to [`apply_room_state`] (whole room, pure-power
/// fan-out rules); a patch with NO `on` is an attribute-only cascade — it casts
/// brightness/colour/temperature/effect onto the room's **lit lights only**.
/// Dimming or recolouring a room must never wake its off lamps: the room slider
/// says "adjust what's shining", not "power everything on at this level"
/// (that command exists — it's `{on: true, brightness: …}`, what scenes and the
/// automation editor's "turn on and…" emit).
pub(crate) async fn apply_room_patch(
    state: &AppState,
    room_id: &str,
    patch: &crate::models::LightStatePatch,
    members: Vec<MemberRow>,
) -> (usize, usize) {
    if let Some(on) = patch.on {
        let full = LightState {
            on,
            brightness: patch.brightness,
            color: patch.color.clone(),
            color_temp_mirek: patch.color_temp_mirek,
            effect: patch.effect.clone(),
            ..Default::default()
        };
        return apply_room_state(state, room_id, &full, members).await;
    }
    let has_attrs = patch.brightness.is_some()
        || patch.color.is_some()
        || patch.color_temp_mirek.is_some()
        || patch.effect.is_some();
    if !has_attrs {
        return (0, 0); // an empty patch asks for nothing
    }
    // Only members that are currently shining take the attribute change.
    let mut lit = Vec::new();
    for m in members {
        if crate::api::lights::current_light_state(&state.db, &m.light_id)
            .await
            .on
        {
            lit.push(m);
        }
    }
    if lit.is_empty() {
        tracing::debug!(room = %room_id, "attribute cascade: no lit members — nothing to do");
        return (0, 0);
    }
    let full = LightState {
        on: true, // the members are already on; absolute, so a no-op power-wise
        brightness: patch.brightness,
        color: patch.color.clone(),
        color_temp_mirek: patch.color_temp_mirek,
        effect: patch.effect.clone(),
        ..Default::default()
    };
    // Never the native group call here — it would hit the off members too. And
    // never the media/power fan-out: an attribute change has no power intent.
    let (applied, failed) = apply_uniform_state(state, room_id, &full, lit, false).await;
    tracing::debug!(room = %room_id, applied, failed, "attribute cascade applied to lit members only");
    (applied, failed)
}

pub(crate) async fn apply_room_state(
    state: &AppState,
    room_id: &str,
    new_state: &LightState,
    members: Vec<MemberRow>,
) -> (usize, usize) {
    let (applied, failed) = apply_uniform_state(state, room_id, new_state, members, true).await;
    // Only a *pure* power change (on/off with no light attributes) fans out to
    // the room's media + power members. A brightness/color/temp/effect change is a
    // lighting-attribute command — its implicit `on: true` must NOT power on the
    // room's speakers/switches (e.g. "make the room blue" shouldn't start Sonos,
    // and a room-wide effect pick shouldn't either).
    let pure_power = new_state.brightness.is_none()
        && new_state.color.is_none()
        && new_state.color_temp_mirek.is_none()
        && new_state.effect.is_none();
    if !pure_power {
        tracing::debug!(room = %room_id, on = new_state.on, applied, failed, fanned_out = false, "apply room state (lights only, no media/power fan-out)");
        return (applied, failed);
    }
    let (a, f) = apply_room_power(state, room_id, new_state.on).await;
    tracing::debug!(room = %room_id, on = new_state.on, applied = applied + a, failed = failed + f, fanned_out = true, "apply room state (power fanned out to media + power members)");
    (applied + a, failed + f)
}

/// Drive every media + power member of a room to `on`. Media members get a
/// power-only command (so a bound source still wakes its receiver, routing
/// through `apply_media_command`); power members go through `apply_power_state`.
/// A disabled/absent member (`NotFound`) is skipped silently — it's not a
/// failure, just out of scope; only real provider/DB errors count as failed.
async fn apply_room_power(state: &AppState, room_id: &str, on: bool) -> (usize, usize) {
    let mut applied = 0usize;
    let mut failed = 0usize;
    let cmd = MediaCommand {
        power: Some(on),
        ..Default::default()
    };
    for member in effective_media_members(state, room_id).await {
        match apply_media_command(state, &member.media_device_id, &cmd).await {
            SetMediaOutcome::Ok => applied += 1,
            SetMediaOutcome::NotFound => {}
            _ => failed += 1,
        }
    }
    for power_id in effective_power_member_ids(state, room_id).await {
        match apply_power_state(state, &power_id, on).await {
            SetPowerOutcome::Ok => applied += 1,
            SetPowerOutcome::NotFound => {}
            _ => failed += 1,
        }
    }
    (applied, failed)
}

async fn set_room_state(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(new_state): Json<crate::models::LightStatePatch>,
) -> impl IntoResponse {
    let members = effective_members(&state, &id).await;
    let (applied, failed) = apply_room_patch(&state, &id, &new_state, members).await;
    // A power command that reached nothing is an unknown/empty room (404). An
    // attribute cascade over a room with nothing lit is a successful no-op.
    if applied == 0 && failed == 0 && new_state.on.is_some() {
        return StatusCode::NOT_FOUND.into_response();
    }
    Json(serde_json::json!({ "applied": applied, "failed": failed })).into_response()
}

// ── Scene apply (activate a saved scene from a room control) ─────────────────
//
// Scenes (`crate::api::scenes`) are self-scoped snapshots — a Room Scene already
// carries its room's members, so applying one is just activating it. The route
// stays room-shaped (`/rooms/{room_id}/scenes/{scene_id}/apply`) because it backs
// a room's `scene` quick-control button; `room_id` is informational.

async fn apply_scene(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path((_room_id, scene_id)): Path<(String, String)>,
) -> impl IntoResponse {
    match crate::api::scenes::apply_scene_entries(&state, &scene_id, None).await {
        Some((applied, failed)) => {
            Json(serde_json::json!({ "applied": applied, "failed": failed })).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod occupancy_tests {
    use super::*;
    use crate::AppState;
    use crate::providers::default_registry;
    use sqlx::SqlitePool;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;
    use std::sync::Arc;

    async fn test_state() -> Arc<AppState> {
        let opts = SqliteConnectOptions::from_str(":memory:")
            .unwrap()
            .foreign_keys(true);
        let db = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("./migrations").run(&db).await.unwrap();
        Arc::new(AppState::new(
            db,
            "test-secret-key-32-bytes-exactly",
            default_registry(),
        ))
    }

    async fn seed_provider(state: &AppState) {
        sqlx::query("INSERT INTO providers (id, provider_type, name, credentials) VALUES ('p','ha','HA','x')")
            .execute(&state.db)
            .await
            .unwrap();
    }

    async fn seed_room(state: &AppState) {
        sqlx::query("INSERT INTO rooms (id, name) VALUES ('r','Living Room')")
            .execute(&state.db)
            .await
            .unwrap();
    }

    /// Insert a sensor and directly assign it to room `r`.
    async fn seed_sensor(state: &AppState, id: &str, kind: &str, last_state: &str) {
        sqlx::query(
            "INSERT INTO sensor_devices (id, provider_id, device_id, name, kind, last_state)
             VALUES (?, 'p', ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(id)
        .bind(id)
        .bind(kind)
        .bind(last_state)
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query("INSERT INTO room_sensor_devices (room_id, sensor_device_id) VALUES ('r', ?)")
            .bind(id)
            .execute(&state.db)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn none_when_room_has_no_presence_sensors() {
        let state = test_state().await;
        seed_provider(&state).await;
        seed_room(&state).await;
        // A non-presence sensor (illuminance) doesn't establish occupancy.
        seed_sensor(
            &state,
            "lux",
            "illuminance",
            r#"{"reading":{"number":500}}"#,
        )
        .await;
        assert_eq!(room_occupancy(&state, "r").await, None);
    }

    #[tokio::test]
    async fn true_when_any_presence_member_is_detecting() {
        let state = test_state().await;
        seed_provider(&state).await;
        seed_room(&state).await;
        seed_sensor(&state, "m1", "motion", r#"{"reading":{"bool":false}}"#).await;
        seed_sensor(&state, "m2", "occupancy", r#"{"reading":{"bool":true}}"#).await;
        assert_eq!(room_occupancy(&state, "r").await, Some(true));
    }

    #[tokio::test]
    async fn false_when_presence_members_all_clear() {
        let state = test_state().await;
        seed_provider(&state).await;
        seed_room(&state).await;
        seed_sensor(&state, "m1", "motion", r#"{"reading":{"bool":false}}"#).await;
        assert_eq!(room_occupancy(&state, "r").await, Some(false));
    }

    #[tokio::test]
    async fn disabled_presence_sensor_is_excluded() {
        let state = test_state().await;
        seed_provider(&state).await;
        seed_room(&state).await;
        seed_sensor(&state, "m1", "motion", r#"{"reading":{"bool":true}}"#).await;
        sqlx::query("UPDATE sensor_devices SET enabled = 0 WHERE id = 'm1'")
            .execute(&state.db)
            .await
            .unwrap();
        // The only presence member is disabled → room has no live presence input.
        assert_eq!(room_occupancy(&state, "r").await, None);
    }
}
