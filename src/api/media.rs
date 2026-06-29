//! Media device API: list devices, read live state, send commands.
//!
//! Mirrors the lights API split: service functions own the behaviour and are
//! shared by the session-authenticated routes here and the Bearer-key routes
//! in `v1`. Reads hit the device live (LAN round trips are cheap) and refresh
//! the cached `last_state`; an unreachable device falls back to the cache with
//! `reachable: false` instead of erroring the whole request.

use crate::AppState;
use crate::api::auth::Session;
use crate::models::media::{MediaCapabilities, MediaCommand, MediaFavorite, MediaState};
use crate::models::remote::RemoteState;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
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
        .route("/devices/{id}/cast", post(cast_handler))
        .route("/devices/{id}/favorites", get(list_favorites_handler))
        .route("/devices/{id}/favorites/play", post(play_favorite_handler))
        .route("/devices/{id}/group", post(group_handler))
        .route("/devices/{id}/ungroup", post(ungroup_handler))
        .route("/devices/{id}/enabled", put(set_enabled_handler))
        .route("/devices/{id}/glyph", put(set_glyph_handler))
        .route("/devices/{id}/name", put(set_name_handler))
        .route("/devices/{id}/shadow", put(set_shadow_handler))
        .route("/devices/{id}/room", put(set_room_handler))
        .route("/devices/{id}/receiver", put(set_receiver_handler))
        .route("/devices/{id}/companion", put(set_companion_handler))
        .route("/play-on", post(play_on_handler))
}

/// `POST /api/media/play-on` — natural-language TV control ("play Bob's Burgers
/// on the bedroom TV"): resolves the named TV/remote and plays a title, launches
/// an app, or opens the last-used app. Delegates to the shared resolver.
async fn play_on_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Json(req): Json<crate::api::remote::PlayOnInput>,
) -> impl IntoResponse {
    crate::api::remote::play_on_response(&state, &req.device, &req.query)
        .await
        .into_response()
}

async fn set_receiver_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetReceiverRequest>,
) -> impl IntoResponse {
    set_receiver_status(set_media_receiver(&state, &id, req.receiver_id, req.receiver_source).await)
}

async fn set_companion_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetCompanionRequest>,
) -> impl IntoResponse {
    set_companion_status(set_media_companion(&state, &id, req.primary_id).await)
}

async fn set_enabled_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetEnabledRequest>,
) -> impl IntoResponse {
    crate::api::set_device_enabled(&state, "media_devices", &id, req.enabled)
        .await
        .into_response()
}

async fn set_glyph_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetGlyphRequest>,
) -> impl IntoResponse {
    crate::api::set_device_glyph(&state, "media_devices", &id, req.glyph)
        .await
        .into_response()
}

async fn set_name_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetNameRequest>,
) -> impl IntoResponse {
    crate::api::set_device_name(
        &state,
        "media_devices",
        &id,
        crate::api::clean_name(req.name),
    )
    .await
    .into_response()
}

async fn set_shadow_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetShadowRequest>,
) -> impl IntoResponse {
    crate::api::dedup::set_device_shadow(&state, "media_devices", &id, req.shadowed_by)
        .await
        .into_response()
}

async fn set_room_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<crate::api::SetRoomRequest>,
) -> impl IntoResponse {
    crate::api::rooms::set_device_room(
        &state,
        "media_devices",
        "room_media_devices",
        "media_device_id",
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
pub(crate) struct MediaDeviceRow {
    pub id: String,
    pub provider_id: String,
    /// Provider-native id (e.g. `main`) — matches `media_state` push events.
    pub device_id: String,
    pub name: String,
    pub kind: String,
    pub capabilities: MediaCapabilities,
    pub state: MediaState,
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
    /// The room this device belongs to **via a synced provider-group link**, when
    /// it has no direct assignment — so the Devices page shows the effective room
    /// rather than "No room".
    pub inherited_room_id: Option<String>,
    /// M22 receiver binding: the media device id whose volume/mute this source
    /// routes to (the receiver is the volume authority). `None` = unbound.
    pub receiver_id: Option<String>,
    /// The receiver input to select when this source becomes active; `None` =
    /// leave the receiver's input alone.
    pub receiver_source: Option<String>,
    /// **Derived** (not stored): the bound receiver's display name, resolved during
    /// read assembly wherever the receiver volume/mute overlay runs. Lets a client
    /// surface "Volume → <receiver>" straight from the device, instead of every
    /// surface re-looking-up `receiver_id` in the device list. `None` = unbound.
    pub receiver_name: Option<String>,
    /// Composite membership: the **logical-device group** this row belongs to.
    /// Rows sharing a `group_id` are one composite (a TV's several media views,
    /// its remote, …). `None` = standalone. Cross-domain — `remote_devices` carry
    /// the same column. Replaces the old directional `companion_of`/`paired_media_id`.
    pub group_id: Option<String>,
    /// **Derived** (not stored): the group's representative ("surface") id when
    /// this row is a *non-surface* member, else `None`. Computed from `group_id`
    /// via [`group_surfaces`] during read assembly. Kept under the historical name
    /// so the API/clients still see "this is a hidden companion of the surface".
    pub companion_of: Option<String>,
    /// M24 composite: the paired remote's device id, when this is a TV whose
    /// `media_player` shares hardware with an enabled `remote.*` entity (set by
    /// [`crate::api::remote::reconcile_remote_pairings`]). Lets a client render the
    /// unified TV control (keypad + apps) straight from the effective device,
    /// without a separate remote lookup. `None` = no paired remote.
    pub remote_id: Option<String>,
}

fn row_to_device(r: sqlx::sqlite::SqliteRow) -> MediaDeviceRow {
    MediaDeviceRow {
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
        inherited_room_id: r.try_get("inherited_room_id").ok().flatten(),
        receiver_id: r.get("receiver_id"),
        receiver_source: r.get("receiver_source"),
        receiver_name: None, // derived later from receiver_id during the overlay
        group_id: r.try_get("group_id").ok().flatten(),
        companion_of: None, // derived later from group_id + surface selection
        remote_id: r.try_get("remote_id").ok().flatten(),
    }
}

/// Rank a device kind for surface (group-representative) selection.
fn kind_rank(kind: &str) -> u8 {
    match kind {
        "tv" => 3,
        "receiver" => 2,
        "speaker" => 1,
        _ => 0,
    }
}

/// Map each non-singleton group to its **derived surface** — the member that
/// represents the composite. Highest authority (native over Integration) then
/// kind (a `tv` over a bare `speaker` view), ties broken on the smallest id so
/// the choice is stable. No surface is stored, so it can never drift from the
/// members.
pub(crate) async fn group_surfaces(state: &AppState) -> std::collections::HashMap<String, String> {
    let rows = sqlx::query(
        "SELECT m.id, m.group_id, m.kind, p.provider_type
         FROM media_devices m JOIN providers p ON m.provider_id = p.id
         WHERE m.group_id IS NOT NULL",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| tracing::error!("db error loading group surfaces: {e}"))
    .unwrap_or_default();
    // For each group, keep the best (authority, kind_rank, -id) member.
    let mut best: std::collections::HashMap<String, (u8, u8, String)> =
        std::collections::HashMap::new();
    for r in &rows {
        let group: String = r.get("group_id");
        let id: String = r.get("id");
        let kind: String = r.get("kind");
        let ptype: String = r.get("provider_type");
        let score = (backing_authority(&state.registry, &ptype), kind_rank(&kind));
        match best.get(&group) {
            Some((a, k, cur_id)) if (*a, *k) > score || ((*a, *k) == score && *cur_id <= id) => {}
            _ => {
                best.insert(group, (score.0, score.1, id));
            }
        }
    }
    best.into_iter().map(|(g, (_, _, id))| (g, id)).collect()
}

/// Set each row's derived `companion_of` (= its group's surface, when the row
/// isn't itself the surface). A singleton (no `group_id`) is always its own
/// surface → `companion_of = None`.
fn derive_companions(
    rows: &mut [MediaDeviceRow],
    surfaces: &std::collections::HashMap<String, String>,
) {
    for d in rows.iter_mut() {
        d.companion_of = d
            .group_id
            .as_deref()
            .and_then(|g| surfaces.get(g))
            .filter(|surface| surface.as_str() != d.id)
            .cloned();
    }
}

/// How much real "what's playing" a now-playing snapshot carries, for picking the
/// richest one across a composite's views: a member actually reporting a title
/// (or artist/album) with active playback outranks an empty/idle/stopped one,
/// which outranks `None`. Lets [`merge_companion_into`] surface the view that
/// knows what's on without an idle `media_player` masking it — irrespective of
/// which row is the primary (the M26 order-independence rule).
fn now_playing_score(np: &Option<crate::models::media::NowPlaying>) -> u8 {
    use crate::models::media::PlayState;
    match np {
        None => 0,
        Some(n) => {
            let has_content = n.title.is_some() || n.artist.is_some() || n.album.is_some();
            let active = matches!(n.play_state, Some(PlayState::Playing | PlayState::Paused));
            u8::from(has_content) * 2 + u8::from(active)
        }
    }
}

/// M26: overlay a companion's complementary state onto its primary — surface the
/// **richest** now-playing, fill source/source-list, surface the companion's
/// **receiver binding** where the primary lacks them, and union the offered
/// capabilities. The receiver volume overlay (run afterwards) then shows the
/// receiver's volume on the merged binding. Nothing is hidden — the union lives
/// on the primary.
fn merge_companion_into(primary: &mut MediaDeviceRow, companion: &MediaDeviceRow) {
    // A companion that's actually playing wins now-playing over an idle/empty
    // primary (one media_player view of a TV reads idle while a Cast view for the
    // same TV carries the title) — order-independent, never masked.
    if now_playing_score(&companion.state.now_playing)
        > now_playing_score(&primary.state.now_playing)
    {
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
    // A merged-in backing that carries the real volume/mute fills the primary when
    // it reports none — e.g. one media_player view of a TV reads volume 0 while
    // another merged-in view (a Cast/Google-TV entity, a soundbar) carries the
    // real volume. (The receiver overlay, run after, still wins for a bound source.)
    if primary.state.volume == 0 && companion.state.volume > 0 {
        primary.state.volume = companion.state.volume;
        primary.state.mute = companion.state.mute;
    }
    // Surface a companion's live sync-group membership too.
    if primary.state.group_coordinator.is_none() {
        primary
            .state
            .group_coordinator
            .clone_from(&companion.state.group_coordinator);
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
    primary.capabilities.grouping |= companion.capabilities.grouping;
}

/// The companion rows (M26) merged into `primary_id`, if any.
/// The other members of `id`'s composite group (everything sharing its
/// `group_id`, excluding `id` itself). Empty for a standalone device.
async fn load_companions(state: &AppState, id: &str) -> Vec<MediaDeviceRow> {
    sqlx::query(
        "SELECT id, provider_id, device_id, name, kind, capabilities, last_state, last_seen, enabled, glyph, hw_id, shadowed_by, shadow_auto, receiver_id, receiver_source, group_id,
                (SELECT room_id FROM room_media_devices WHERE media_device_id = media_devices.id LIMIT 1) AS room_id
         FROM media_devices
         WHERE group_id IS NOT NULL
           AND group_id = (SELECT group_id FROM media_devices WHERE id = ?)
           AND id != ?",
    )
    .bind(id)
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| tracing::error!("db error loading companions: {e}"))
    .unwrap_or_default()
    .into_iter()
    .map(row_to_device)
    .collect()
}

/// One member's contribution to a composite device's effective on/reachable
/// state — the primary `media_player`, the paired remote, a companion, or (later)
/// a native smart-TV surface (Bravia/Sony, …). Members are interchangeable here
/// on purpose: a new TV control path joins the composite by adding a
/// `PowerSignal`, with no special-casing in the resolver.
#[derive(Debug, Clone, Copy)]
struct PowerSignal {
    reachable: bool,
    on: bool,
}

impl PowerSignal {
    fn media(d: &MediaDeviceRow) -> Self {
        Self {
            reachable: d.state.reachable != Some(false),
            on: d.state.power,
        }
    }
    fn remote(rs: &RemoteState) -> Self {
        Self {
            reachable: rs.reachable != Some(false),
            on: rs.on,
        }
    }
}

/// Resolve a composite's effective `(reachable, on)` from its **media** views
/// (the primary + companion `media_player`s — all the same physical device) and
/// its paired **remotes**.
///
/// `on` is a **positive** signal: a device any reachable media view reports as
/// on/playing *is* on, so a leaner or stale sibling reading `off` can never mask
/// the view that knows it's playing. The media views are weighed symmetrically —
/// order-independent, the M26 rule — so the result never depends on which row is
/// the primary. (This is the exact Bravia case: the `media_player` Bifrost merged
/// as primary can go `unavailable`/`off` while a companion Cast entity for the
/// same TV stays live and playing.)
///
/// Remotes are the **standby-wake fallback**: when *no* media view is reachable
/// (a cold TV whose `media_player` reads `unavailable`), the paired remote's
/// `(reachable, on)` rescues the composite. A remote is not allowed to override a
/// reachable media view (a stale remote-on must not force a powered-off TV on).
/// With nothing reachable the primary is returned as-is — truly offline — and the
/// client still offers Wake-on-LAN, which reaches a down NIC a live read can't.
fn resolve_composite_power(
    primary: PowerSignal,
    media_members: &[PowerSignal],
    remotes: &[PowerSignal],
) -> PowerSignal {
    let reachable_media: Vec<&PowerSignal> = std::iter::once(&primary)
        .chain(media_members)
        .filter(|s| s.reachable)
        .collect();
    if !reachable_media.is_empty() {
        return PowerSignal {
            reachable: true,
            on: reachable_media.iter().any(|s| s.on),
        };
    }
    let reachable_remotes: Vec<&PowerSignal> = remotes.iter().filter(|s| s.reachable).collect();
    if !reachable_remotes.is_empty() {
        return PowerSignal {
            reachable: true,
            on: reachable_remotes.iter().any(|s| s.on),
        };
    }
    primary
}

/// Overlay the composite power resolution onto `device` from its member signals —
/// companion `media_player`s and the paired remotes. The resolved `(reachable,
/// on)` already folds in the primary itself, so it's written verbatim: a stale or
/// `off` primary is corrected by a fresher member, and vice-versa. No-op when the
/// device has no members (a plain, non-composite device).
fn apply_composite_power(
    device: &mut MediaDeviceRow,
    media_members: &[PowerSignal],
    remotes: &[PowerSignal],
) {
    if media_members.is_empty() && remotes.is_empty() {
        return;
    }
    let resolved = resolve_composite_power(PowerSignal::media(device), media_members, remotes);
    if device.state.power != resolved.on || device.state.reachable != Some(resolved.reachable) {
        tracing::debug!(
            target: "bifrost::composite",
            media = %device.id,
            on = resolved.on,
            reachable = resolved.reachable,
            media_members = media_members.len(),
            remotes = remotes.len(),
            "composite power resolved from member signals",
        );
    }
    device.state.reachable = Some(resolved.reachable);
    device.state.power = resolved.on;
}

/// One remote paired to a media **surface** (the primary if the remote is paired
/// to a companion), with what the composite needs to choose and resolve it: the
/// remote's id, its **backing authority** (a native vendor remote outranks an
/// integration `remote.*` for the same TV — [`backing_authority`]), and its
/// cached state.
struct PairedRemote {
    /// The composite group this remote belongs to.
    group_id: String,
    remote_id: String,
    priority: u8,
    state: Option<RemoteState>,
}

/// Every enabled remote paired to a media surface — optionally only `surface`'s
/// composite (the device or any of its companions). A composite can carry
/// **several** (a native vendor remote *and* an HA `remote.*` for the same TV), so
/// callers pick by need: the richest one for control ([`best_remote_per_surface`])
/// or *every* one's signal for the power/reachability OR. Returning all of them —
/// not an arbitrary `LIMIT 1` — is what keeps a composite from masking a member's
/// remote capabilities depending on which device was merged into which.
async fn load_paired_remotes(state: &AppState, group: Option<&str>) -> Vec<PairedRemote> {
    let mut sql = String::from(
        "SELECT r.group_id, r.id AS remote_id, r.last_state, p.provider_type
         FROM remote_devices r
         JOIN providers p ON r.provider_id = p.id
         WHERE r.enabled = 1 AND r.group_id IS NOT NULL",
    );
    if group.is_some() {
        sql.push_str(" AND r.group_id = ?");
    }
    let mut q = sqlx::query(&sql);
    if let Some(g) = group {
        q = q.bind(g);
    }
    q.fetch_all(&state.db)
        .await
        .map_err(|e| tracing::error!("db error loading paired remotes: {e}"))
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            let provider_type: String = r.get("provider_type");
            PairedRemote {
                group_id: r.get("group_id"),
                remote_id: r.get("remote_id"),
                priority: backing_authority(&state.registry, &provider_type),
                state: r
                    .get::<Option<String>, _>("last_state")
                    .and_then(|s| serde_json::from_str(&s).ok()),
            }
        })
        .collect()
}

/// The remote to surface for control, per composite **group**: the
/// **highest-priority** one wins (a native vendor remote over an HA `remote.*`
/// for the same TV), so the richer command catalogue (the full key set behind the
/// "Full remote") is never masked by a leaner integration copy. Ties break on the
/// smallest id, so the choice is deterministic. Keyed by `group_id`.
fn best_remote_per_group(paired: &[PairedRemote]) -> std::collections::HashMap<String, String> {
    let mut best: std::collections::HashMap<&str, &PairedRemote> = std::collections::HashMap::new();
    for p in paired {
        let take = match best.get(p.group_id.as_str()) {
            None => true,
            Some(cur) => {
                p.priority > cur.priority
                    || (p.priority == cur.priority && p.remote_id < cur.remote_id)
            }
        };
        if take {
            best.insert(&p.group_id, p);
        }
    }
    best.into_iter()
        .map(|(k, p)| (k.to_string(), p.remote_id.clone()))
        .collect()
}

// ── Services (shared with /api/v1) ───────────────────────────────────────────

pub(crate) fn build_media_provider(
    state: &AppState,
    provider_type: &str,
    credentials_enc: &str,
) -> anyhow::Result<Box<dyn crate::providers::MediaProvider>> {
    let creds_json = state.decrypt_credentials(credentials_enc)?;
    state.registry.build_media(provider_type, &creds_json)
}

pub(crate) async fn list_all_devices(state: &AppState) -> Result<Vec<MediaDeviceRow>, ()> {
    let mut devices: Vec<MediaDeviceRow> = sqlx::query(
        "SELECT id, provider_id, device_id, name, kind, capabilities, last_state, last_seen, enabled, glyph, hw_id, shadowed_by, shadow_auto, receiver_id, receiver_source, group_id,
                (SELECT room_id FROM room_media_devices WHERE media_device_id = media_devices.id LIMIT 1) AS room_id,
                (SELECT rl.room_id FROM room_links rl
                   JOIN provider_group_media_devices pga ON pga.provider_group_id = rl.provider_group_id
                   WHERE pga.media_device_id = media_devices.id LIMIT 1) AS inherited_room_id
         FROM media_devices ORDER BY name",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| tracing::error!("db error listing media devices: {e}"))?
    .into_iter()
    .map(row_to_device)
    .collect();

    // Resolve each group's derived surface, then mark non-surface members as
    // companions of it (`companion_of`). The surface is the only row shown as a
    // control surface; the rest are hidden and merged in.
    let surfaces = group_surfaces(state).await;
    derive_companions(&mut devices, &surfaces);

    // Merge each companion's complementary state into its surface, before the
    // receiver overlay (so a companion's receiver binding shows the receiver's
    // volume on the merged card). Companions stay in the list (marked
    // `companion_of`); control surfaces hide them, the inventory collapses them.
    let companions: Vec<MediaDeviceRow> = devices
        .iter()
        .filter(|d| d.companion_of.is_some())
        .cloned()
        .collect();
    for c in &companions {
        if let Some(surface) = devices
            .iter_mut()
            .find(|p| c.companion_of.as_deref() == Some(p.id.as_str()))
        {
            merge_companion_into(surface, c);
        }
    }

    // Composite remote + power/reachability. A composite can carry several paired
    // remotes (a native vendor remote *and* an HA copy of the same TV): the richest
    // one is surfaced for control (`remote_id`), while **every** remote's signal
    // feeds the power/reachability OR — so neither is masked by merge ordering.
    // When a surface's own media_player is unreachable (a standby TV reports
    // `unavailable`), the effective device still reads on/reachable if any member
    // (a companion or a paired remote) is.
    let paired = load_paired_remotes(state, None).await;
    let remote_ids = best_remote_per_group(&paired);
    for d in &mut devices {
        if d.companion_of.is_some() {
            continue; // a companion is hidden, not a surface
        }
        let group = d.group_id.clone();
        d.remote_id = group.as_deref().and_then(|g| remote_ids.get(g)).cloned();
        let media_members: Vec<PowerSignal> = companions
            .iter()
            .filter(|c| c.companion_of.as_deref() == Some(d.id.as_str()))
            .map(PowerSignal::media)
            .collect();
        let remotes: Vec<PowerSignal> = paired
            .iter()
            .filter(|p| Some(&p.group_id) == group.as_ref())
            .filter_map(|p| p.state.as_ref().map(PowerSignal::remote))
            .collect();
        apply_composite_power(d, &media_members, &remotes);
    }

    // A bound source shows its receiver's volume/mute (the receiver owns volume),
    // mirroring `get_device_live`. The receiver is in this same list, so overlay
    // from it — no extra query.
    let vol_mute: std::collections::HashMap<String, (u8, bool, String)> = devices
        .iter()
        .map(|d| (d.id.clone(), (d.state.volume, d.state.mute, d.name.clone())))
        .collect();
    for d in &mut devices {
        if let Some(rid) = &d.receiver_id
            && let Some((volume, mute, name)) = vol_mute.get(rid)
        {
            d.state.volume = *volume;
            d.state.mute = *mute;
            d.receiver_name = Some(name.clone());
        }
    }
    Ok(devices)
}

/// Fetch one device with a live state read. Falls back to the cached state
/// (marked unreachable) when the device doesn't answer; `Ok(None)` = unknown id.
pub(crate) async fn get_device_live(
    state: &AppState,
    id: &str,
) -> Result<Option<MediaDeviceRow>, ()> {
    let row = sqlx::query(
        "SELECT a.id, a.provider_id, a.device_id, a.name, a.kind, a.capabilities,
                a.last_state, a.last_seen, a.enabled, a.glyph, a.hw_id, a.shadowed_by, a.shadow_auto,
                a.receiver_id, a.receiver_source, a.group_id,
                (SELECT room_id FROM room_media_devices WHERE media_device_id = a.id LIMIT 1) AS room_id,
                (SELECT rl.room_id FROM room_links rl
                   JOIN provider_group_media_devices pga ON pga.provider_group_id = rl.provider_group_id
                   WHERE pga.media_device_id = a.id LIMIT 1) AS inherited_room_id,
                p.provider_type, p.credentials
         FROM media_devices a JOIN providers p ON a.provider_id = p.id
         WHERE a.id = ? AND p.enabled = 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| tracing::error!("db error fetching media device: {e}"))?;

    let Some(row) = row else { return Ok(None) };
    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials: String = row.get("credentials");
    let mut device = row_to_device(row);

    match build_media_provider(state, &provider_type, &credentials) {
        Ok(provider) => match provider.get_state(&device_id).await {
            Ok(fresh) => {
                let state_json = serde_json::to_string(&fresh).unwrap_or_default();
                let _ = sqlx::query(
                    "UPDATE media_devices SET last_state = ?, last_seen = datetime('now') WHERE id = ?",
                )
                .bind(&state_json)
                .bind(&device.id)
                .execute(&state.db)
                .await;
                device.state = fresh;
            }
            Err(e) => {
                tracing::debug!("media device {id} unreachable: {e:#}");
                device.state.reachable = Some(false);
            }
        },
        Err(e) => {
            tracing::error!("failed to build media provider: {e:#}");
            device.state.reachable = Some(false);
        }
    }

    // M26: overlay companions' complementary state (now-playing, sources, and
    // their receiver binding) onto this primary — before the receiver overlay,
    // so a companion's binding shows the receiver's volume here too. Each member
    // also contributes a power signal for the composite resolution below.
    let mut media_members: Vec<PowerSignal> = Vec::new();
    for companion in load_companions(state, &device.id).await {
        media_members.push(PowerSignal::media(&companion));
        merge_companion_into(&mut device, &companion);
    }
    // Surface the richest paired remote for control, and fold *every* paired
    // remote's signal into the composite power resolution (see `list_all_devices`).
    let paired = load_paired_remotes(state, device.group_id.as_deref()).await;
    device.remote_id = device
        .group_id
        .as_deref()
        .and_then(|g| best_remote_per_group(&paired).remove(g));
    let remotes: Vec<PowerSignal> = paired
        .iter()
        .filter_map(|p| p.state.as_ref().map(PowerSignal::remote))
        .collect();
    // Composite power/reachability: a fresher companion media_player (or, in
    // standby, the paired remote) corrects a stale/off primary.
    apply_composite_power(&mut device, &media_members, &remotes);

    // For a bound source the receiver owns volume/mute, so show the receiver's
    // values — what the source's own volume slider actually controls. Use the
    // receiver's *cached* state, not a fresh read: push-mode receivers (Onkyo)
    // allow only one eISCP connection, which the push manager holds, so a
    // competing per-request read returns a partial response and would clobber a
    // good cached volume with 0. The push manager keeps last_state current.
    if let Some(rid) = &device.receiver_id
        && let Ok(Some(r)) = sqlx::query(
            "SELECT name, last_state FROM media_devices WHERE id = ? AND enabled = 1 AND shadowed_by IS NULL",
        )
        .bind(rid)
        .fetch_optional(&state.db)
        .await
    {
        device.receiver_name = Some(r.get("name"));
        if let Some(rstate) = r
            .get::<Option<String>, _>("last_state")
            .and_then(|s| serde_json::from_str::<MediaState>(&s).ok())
        {
            device.state.volume = rstate.volume;
            device.state.mute = rstate.mute;
        }
    }
    Ok(Some(device))
}

pub(crate) enum SetMediaOutcome {
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

/// Bind (or, with `receiver_id = None`, unbind) a source media device to a
/// receiver. Stored on the source — many sources may share one receiver. Rejects
/// a missing source/receiver and self-binding; chaining (binding to a device
/// that is itself bound) is rejected so volume can't route in a loop.
pub(crate) async fn set_media_receiver(
    state: &AppState,
    id: &str,
    receiver_id: Option<String>,
    receiver_source: Option<String>,
) -> SetReceiverOutcome {
    if let Some(rid) = &receiver_id {
        if rid == id {
            return SetReceiverOutcome::BadRequest("a device cannot be its own receiver".into());
        }
        let receiver = sqlx::query("SELECT receiver_id FROM media_devices WHERE id = ?")
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
    match sqlx::query("UPDATE media_devices SET receiver_id = ?, receiver_source = ? WHERE id = ?")
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

/// Merge `id` into `primary_id`'s composite **group** (a flat, cross-domain
/// `group_id`), or unmerge `id` from its group with `primary_id = None`. Merging
/// is a **union**: `id`'s whole group folds into the target's group (so merging
/// two composites combines them — the fix for a device that surfaced as two), and
/// any remotes that travelled with `id`'s group follow. Unlike a shadow, members
/// are routed/overlaid, not discarded. Rejects self-merge and an unknown/shadowed
/// target. There's no "primary" to point at and no chains — the group is flat.
pub(crate) async fn set_media_companion(
    state: &AppState,
    id: &str,
    primary_id: Option<String>,
) -> SetCompanionOutcome {
    let Some(pid) = primary_id else {
        // Unmerge: drop `id` out of its group. Any remaining members re-derive a
        // surface; a left-behind singleton is harmless.
        return match sqlx::query("UPDATE media_devices SET group_id = NULL WHERE id = ?")
            .bind(id)
            .execute(&state.db)
            .await
        {
            Ok(r) if r.rows_affected() > 0 => SetCompanionOutcome::Ok,
            Ok(_) => SetCompanionOutcome::NotFound,
            Err(e) => {
                tracing::error!("db error clearing group: {e}");
                SetCompanionOutcome::Db
            }
        };
    };

    if pid == id {
        return SetCompanionOutcome::BadRequest("a device cannot be merged into itself".into());
    }
    // The target must exist and not be a hidden duplicate. Its group is the
    // destination; a standalone target gets a fresh singleton group (its own id).
    let target_group =
        match sqlx::query("SELECT group_id, shadowed_by FROM media_devices WHERE id = ?")
            .bind(&pid)
            .fetch_optional(&state.db)
            .await
        {
            Ok(Some(r)) => {
                if r.get::<Option<String>, _>("shadowed_by").is_some() {
                    return SetCompanionOutcome::BadRequest(
                        "that device is a hidden duplicate".into(),
                    );
                }
                match r.get::<Option<String>, _>("group_id") {
                    Some(g) => g,
                    None => {
                        if let Err(e) =
                            sqlx::query("UPDATE media_devices SET group_id = ? WHERE id = ?")
                                .bind(&pid)
                                .bind(&pid)
                                .execute(&state.db)
                                .await
                        {
                            tracing::error!("db error creating group: {e}");
                            return SetCompanionOutcome::Db;
                        }
                        pid.clone()
                    }
                }
            }
            Ok(None) => return SetCompanionOutcome::BadRequest("unknown target device".into()),
            Err(e) => {
                tracing::error!("db error validating merge target: {e}");
                return SetCompanionOutcome::Db;
            }
        };

    // Fold `id`'s current group (or just `id`) into the target group — media and
    // any remotes that share that group travel together.
    let src_group: Option<String> =
        sqlx::query_scalar::<_, Option<String>>("SELECT group_id FROM media_devices WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| tracing::error!("db error reading source group: {e}"))
            .ok()
            .flatten()
            .flatten();

    let result = if let Some(src) = src_group {
        if src == target_group {
            return SetCompanionOutcome::Ok; // already in the same group
        }
        let m = sqlx::query("UPDATE media_devices SET group_id = ? WHERE group_id = ?")
            .bind(&target_group)
            .bind(&src)
            .execute(&state.db)
            .await;
        let _ = sqlx::query("UPDATE remote_devices SET group_id = ? WHERE group_id = ?")
            .bind(&target_group)
            .bind(&src)
            .execute(&state.db)
            .await;
        m
    } else {
        sqlx::query("UPDATE media_devices SET group_id = ? WHERE id = ?")
            .bind(&target_group)
            .bind(id)
            .execute(&state.db)
            .await
    };
    match result {
        Ok(r) if r.rows_affected() > 0 => SetCompanionOutcome::Ok,
        Ok(_) => SetCompanionOutcome::NotFound,
        Err(e) => {
            tracing::error!("db error merging group: {e}");
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

/// Route a command for a media device, honouring an M22 receiver binding: a
/// bound source sends `volume`/`mute` to its receiver (and switches the receiver
/// input on power-on) while keeping `power`/`source`/`transport` on itself.
/// Unbound devices apply the command directly. Shared by session, `/v1`, and MCP
/// so every surface routes identically.
/// One backing entity of a composite device (M26), for command routing.
struct Backing {
    id: String,
    /// Display name (for the dev routing/precedence diagnostic).
    name: String,
    capabilities: MediaCapabilities,
    /// The receiver this backing's volume/mute route to (M22 binding), if any.
    receiver_id: Option<String>,
    /// This backing routes its volume/mute to a receiver (M22 binding).
    receiver_bound: bool,
    /// This backing is the one actively reporting playback.
    has_now_playing: bool,
    /// Last reported volume — a non-zero one marks the backing actually carrying
    /// audio, so volume routes there even if it isn't the primary.
    volume: u8,
    /// Authority for a contested control: a native backing outranks its HA twin
    /// ([`backing_authority`]), so merging a native TV into an HA device lets the
    /// TV take precedence for power/source while still unioning capabilities.
    priority: u8,
}

// Composite control authority: a **native** backing outranks its **integration**
// (HA) twin for any contested control (source, power, the surfaced remote, and
// capability ties), mirroring de-dup's "native wins". Within a composite the
// members are the *same physical device* surfaced more than once — natively and
// as an HA copy — so native-vs-integration is the only authority distinction that
// carries information. Everything physical is decided **independently of this**:
// volume follows a receiver binding (then the backing actually carrying audio)
// and transport follows the backing actually playing (see [`route_across_backings`]).
// Ties keep the primary-first order, so a single-provider composite is stable.
fn backing_authority(registry: &crate::providers::ProviderRegistry, provider_type: &str) -> u8 {
    match registry.ui_domain(provider_type) {
        Some(crate::providers::ProviderDomain::Integration) => 0,
        _ => 1,
    }
}

/// Route an `MediaCommand` across a composite's backings (`backings[0]` is the
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
///
/// Contested controls go to the **highest-priority capable** backing (ties keep
/// the primary-first order), with two physical-routing overrides that win
/// regardless of priority: volume follows a receiver binding, and transport
/// follows the backing actually playing.
/// Highest-authority backing matching `pred` (ties → earliest, i.e. the
/// primary `backings[0]`), else the primary. The single arbitration the whole
/// composite write path (and the routing diagnostic) shares, so they can't drift.
fn pick_best(backings: &[Backing], pred: impl Fn(&Backing) -> bool) -> &Backing {
    backings
        .iter()
        .filter(|b| pred(b))
        .fold(None::<&Backing>, |acc, b| match acc {
            Some(a) if a.priority >= b.priority => Some(a),
            _ => Some(b),
        })
        .unwrap_or(&backings[0])
}

/// Per-field routing target + a human reason — the canonical decision for each
/// command field, used by both `route_across_backings` (write) and
/// `composite_routing` (the dev precedence diagnostic).
fn route_volume(backings: &[Backing]) -> (&Backing, &'static str) {
    // A receiver binding owns volume (physical routing); else the backing
    // actually carrying audio (non-zero volume); else the primary.
    if let Some(b) = backings.iter().find(|b| b.receiver_bound) {
        (b, "bound to a receiver")
    } else if backings.iter().any(|b| b.volume > 0) {
        (
            pick_best(backings, |b| b.volume > 0),
            "carrying audio (volume > 0)",
        )
    } else {
        (&backings[0], "default — primary (no audio source)")
    }
}

fn route_transport(backings: &[Backing]) -> (&Backing, &'static str) {
    if let Some(b) = backings
        .iter()
        .find(|b| b.capabilities.transport && b.has_now_playing)
    {
        (b, "currently playing")
    } else {
        let b = pick_best(backings, |b| b.capabilities.transport);
        (
            b,
            if b.capabilities.transport {
                "most authoritative transport-capable"
            } else {
                "default — primary (none transport-capable)"
            },
        )
    }
}

fn route_source(backings: &[Backing]) -> (&Backing, &'static str) {
    let b = pick_best(backings, |b| b.capabilities.sources);
    (
        b,
        if b.capabilities.sources {
            "most authoritative source-capable"
        } else {
            "default — primary (none source-capable)"
        },
    )
}

fn route_power(backings: &[Backing]) -> (&Backing, &'static str) {
    (pick_best(backings, |_| true), "most authoritative")
}

fn route_across_backings(cmd: &MediaCommand, backings: &[Backing]) -> Vec<(String, MediaCommand)> {
    let mut parts: std::collections::BTreeMap<String, MediaCommand> =
        std::collections::BTreeMap::new();
    if cmd.volume.is_some() || cmd.mute.is_some() {
        let e = parts
            .entry(route_volume(backings).0.id.clone())
            .or_default();
        e.volume = cmd.volume;
        e.mute = cmd.mute;
    }
    if cmd.transport.is_some() {
        parts
            .entry(route_transport(backings).0.id.clone())
            .or_default()
            .transport = cmd.transport;
    }
    if cmd.source.is_some() {
        parts
            .entry(route_source(backings).0.id.clone())
            .or_default()
            .source = cmd.source.clone();
    }
    if cmd.power.is_some() {
        parts
            .entry(route_power(backings).0.id.clone())
            .or_default()
            .power = cmd.power;
    }
    parts.into_iter().filter(|(_, c)| !c.is_empty()).collect()
}

/// One row of the composite **precedence** diagnostic: which member device a
/// control resolves to, and why. Read-only dev tooling.
#[derive(serde::Serialize)]
pub struct ControlRoute {
    pub control: &'static str,
    pub device_id: String,
    pub device_name: String,
    pub reason: String,
}

/// Compute the per-control precedence map for the composite surfaced at `id`:
/// for each control, which underlying member wins and why. Mirrors the real
/// read/write routing (shares `route_*` + the remote/favorites pickers), so it's
/// an honest window into what actually happens — not a parallel guess.
pub(crate) async fn composite_routing(state: &AppState, id: &str) -> Vec<ControlRoute> {
    let backings = load_composite_backings(state, id).await;
    if backings.is_empty() {
        return Vec::new();
    }
    let row = |control, b: &Backing, reason: String| ControlRoute {
        control,
        device_id: b.id.clone(),
        device_name: b.name.clone(),
        reason,
    };
    let mut out = Vec::new();

    let (b, why) = route_power(&backings);
    out.push(row("power", b, why.into()));

    let (b, why) = route_volume(&backings);
    let mut reason = why.to_string();
    // Volume on a receiver-bound backing physically lands on the receiver.
    if let Some(recv) = b.receiver_id.as_deref()
        && let Some(name) = device_name(state, recv).await
    {
        reason = format!("{why} → {name}");
    }
    out.push(row("volume / mute", b, reason));

    let (b, why) = route_transport(&backings);
    out.push(row("transport", b, why.into()));

    let (b, why) = route_source(&backings);
    out.push(row("source / app", b, why.into()));

    if backings.iter().any(|b| b.capabilities.favorites) {
        let b = pick_best(&backings, |b| b.capabilities.favorites);
        out.push(row(
            "favorites",
            b,
            "most authoritative favorites-capable".into(),
        ));
    }

    // Surfaced remote (highest-authority paired remote across the whole composite).
    let group: Option<String> =
        sqlx::query_scalar("SELECT group_id FROM media_devices WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let paired = load_paired_remotes(state, group.as_deref()).await;
    if let Some(rid) = group
        .as_deref()
        .and_then(|g| best_remote_per_group(&paired).remove(g))
    {
        let name = device_name_remote(state, &rid).await.unwrap_or_default();
        out.push(ControlRoute {
            control: "remote keys / apps",
            device_id: rid,
            device_name: name,
            reason: "highest-authority paired remote".into(),
        });
    }
    out
}

/// A media device's display name, for the routing diagnostic.
async fn device_name(state: &AppState, id: &str) -> Option<String> {
    sqlx::query_scalar::<_, String>("SELECT name FROM media_devices WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
}

/// A remote device's display name, for the routing diagnostic.
async fn device_name_remote(state: &AppState, id: &str) -> Option<String> {
    sqlx::query_scalar::<_, String>("SELECT name FROM remote_devices WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
}

/// The composite's backings (primary first, then companions), or just `[id]`
/// when `id` has no companions. Each carries the capability/binding facts the
/// router needs.
async fn load_composite_backings(state: &AppState, id: &str) -> Vec<Backing> {
    let rows = sqlx::query(
        "SELECT m.id, m.name, m.capabilities, m.receiver_id, m.last_state, p.provider_type,
                (m.id = ?) AS is_primary
         FROM media_devices m JOIN providers p ON m.provider_id = p.id
         WHERE m.id = ?
            OR (m.group_id IS NOT NULL
                AND m.group_id = (SELECT group_id FROM media_devices WHERE id = ?))
         ORDER BY is_primary DESC, m.name",
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
            let st: Option<MediaState> = r
                .get::<Option<String>, _>("last_state")
                .and_then(|s| serde_json::from_str(&s).ok());
            let provider_type: String = r.get("provider_type");
            let volume = st.as_ref().map_or(0, |s| s.volume);
            let receiver_id: Option<String> = r.get("receiver_id");
            Backing {
                id: r.get("id"),
                name: r.get("name"),
                capabilities: serde_json::from_str(&r.get::<String, _>("capabilities"))
                    .unwrap_or_default(),
                receiver_bound: receiver_id.is_some(),
                receiver_id,
                has_now_playing: st.is_some_and(|s| s.now_playing.is_some()),
                volume,
                priority: backing_authority(&state.registry, &provider_type),
            }
        })
        .collect()
}

/// The composite member (highest-priority) whose capability `pred` is true — so an
/// operation routes to whichever row can actually do it, regardless of which is the
/// primary. Falls back to `id` itself when nothing matches.
async fn capable_backing(
    state: &AppState,
    id: &str,
    pred: fn(&MediaCapabilities) -> bool,
) -> String {
    load_composite_backings(state, id)
        .await
        .iter()
        .filter(|b| pred(&b.capabilities))
        .fold(None::<&Backing>, |acc, b| match acc {
            Some(a) if a.priority >= b.priority => Some(a),
            _ => Some(b),
        })
        .map_or_else(|| id.to_string(), |b| b.id.clone())
}

/// Apply a command to a device. If `id` is a composite **primary** (has
/// companions merged in), route each field to the backing that owns it (M26);
/// otherwise drive the single device directly (with its own receiver split).
pub(crate) async fn apply_media_command(
    state: &AppState,
    id: &str,
    cmd: &MediaCommand,
) -> SetMediaOutcome {
    let backings = load_composite_backings(state, id).await;
    // Composite power-**on** also wakes the paired remote (WoL + the provider's
    // `turn_on`) — the reliable way to bring a TV out of standby, where its
    // `media_player` often reports `unavailable`. Fire it **concurrently** with
    // the media command: a standby media_player can hang to a timeout, and the
    // WoL/remote wake must not wait behind it. If the media command failed but the
    // remote woke the box, the composite still reached the requested state, so
    // report success. (Power-off is left to the media_player; the box is the same
    // device and needs no WoL.)
    if cmd.power == Some(true) {
        let (result, woke) = tokio::join!(
            apply_routed(state, id, cmd, &backings),
            wake_paired_remote(state, id),
        );
        if woke && !matches!(result, SetMediaOutcome::Ok) {
            return SetMediaOutcome::Ok;
        }
        return result;
    }
    apply_routed(state, id, cmd, &backings).await
}

/// Route `cmd` to the right backing(s) of a composite (or drive the single device
/// directly), each with its own M22 receiver split. Stops at the first failure.
async fn apply_routed(
    state: &AppState,
    id: &str,
    cmd: &MediaCommand,
    backings: &[Backing],
) -> SetMediaOutcome {
    if backings.len() <= 1 {
        return apply_with_receiver(state, id, cmd).await;
    }
    for (backing_id, sub) in route_across_backings(cmd, backings) {
        match apply_with_receiver(state, &backing_id, &sub).await {
            SetMediaOutcome::Ok => {}
            other => return other,
        }
    }
    SetMediaOutcome::Ok
}

/// Fire a power-**on** at the (enabled) remote paired to media device `media_id`,
/// if any. Returns `true` only when such a remote existed and its power command
/// succeeded. The remote's own handler does the Wake-on-LAN nudge before the
/// provider `turn_on` (see [`crate::api::remote::apply_remote_command`]).
async fn wake_paired_remote(state: &AppState, media_id: &str) -> bool {
    let group: Option<String> =
        sqlx::query_scalar("SELECT group_id FROM media_devices WHERE id = ?")
            .bind(media_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let paired = load_paired_remotes(state, group.as_deref()).await;
    let Some(rid) = group
        .as_deref()
        .and_then(|g| best_remote_per_group(&paired).remove(g))
    else {
        return false;
    };
    tracing::debug!(target: "bifrost::composite", media = %media_id, remote = %rid, "composite power-on: waking paired remote (WoL + turn_on)");
    let woke = matches!(
        crate::api::remote::apply_remote_command(
            state,
            &rid,
            &crate::models::remote::RemoteCommand::Power { on: true },
        )
        .await,
        crate::api::remote::RemoteOutcome::Ok
    );
    if !woke {
        tracing::debug!(target: "bifrost::composite", remote = %rid, "paired-remote wake did not succeed (non-fatal)");
    }
    woke
}

/// Drive one device, routing its volume/mute to a bound receiver (M22) if any.
async fn apply_with_receiver(state: &AppState, id: &str, cmd: &MediaCommand) -> SetMediaOutcome {
    match load_receiver_binding(state, id).await {
        Err(()) => SetMediaOutcome::Db,
        Ok(None) => apply_to_device(state, id, cmd).await,
        Ok(Some((receiver_id, receiver_source))) => {
            let (source_cmd, receiver_cmd) = cmd.split_for_receiver(receiver_source.as_deref());
            // Source first (power/input), so the receiver wakes to an active source.
            if !source_cmd.is_empty() {
                match apply_to_device(state, id, &source_cmd).await {
                    SetMediaOutcome::Ok => {}
                    other => return other,
                }
            }
            if !receiver_cmd.is_empty() {
                return apply_to_device(state, &receiver_id, &receiver_cmd).await;
            }
            SetMediaOutcome::Ok
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
         FROM media_devices a JOIN providers p ON a.provider_id = p.id
         WHERE a.id = ? AND p.enabled = 1 AND a.enabled = 1 AND a.shadowed_by IS NULL
           AND a.receiver_id IS NOT NULL
           AND EXISTS (
               SELECT 1 FROM media_devices r JOIN providers rp ON r.provider_id = rp.id
               WHERE r.id = a.receiver_id AND r.enabled = 1 AND r.shadowed_by IS NULL AND rp.enabled = 1
           )",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| tracing::error!("db error loading receiver binding: {e}"))?;
    Ok(row.map(|r| (r.get("receiver_id"), r.get("receiver_source"))))
}

async fn apply_to_device(state: &AppState, id: &str, cmd: &MediaCommand) -> SetMediaOutcome {
    // A disabled device receives no commands (control lookups skip it).
    let row = sqlx::query(
        "SELECT a.device_id, p.provider_type, p.credentials
         FROM media_devices a JOIN providers p ON a.provider_id = p.id
         WHERE a.id = ? AND p.enabled = 1 AND a.enabled = 1 AND a.shadowed_by IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await;

    let row = match row {
        Ok(Some(r)) => r,
        // No controllable row for this id — a disabled/shadowed/unknown device. This
        // is a silent 404 to the client, so log it: a command quietly going nowhere
        // (e.g. a UI targeting a de-dup-shadowed copy) is otherwise invisible.
        Ok(None) => {
            tracing::debug!(
                target: "bifrost::media",
                device = %id,
                "media command dropped: no enabled, non-shadowed device with this id"
            );
            return SetMediaOutcome::NotFound;
        }
        Err(e) => {
            tracing::error!("db error: {e}");
            return SetMediaOutcome::Db;
        }
    };

    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials: String = row.get("credentials");

    let provider = match build_media_provider(state, &provider_type, &credentials) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to build media provider: {e:#}");
            return SetMediaOutcome::Db;
        }
    };

    match provider.set_state(&device_id, cmd).await {
        Ok(()) => SetMediaOutcome::Ok,
        // Validation failures (unknown source/zone) are the caller's mistake,
        // not a gateway fault; connection problems stay 502s.
        Err(e) if e.to_string().contains("unknown") || e.to_string().contains("not supported") => {
            SetMediaOutcome::BadCommand(e.to_string())
        }
        Err(e) => {
            tracing::error!("media set_state error: {e:#}");
            SetMediaOutcome::ProviderError
        }
    }
}

/// Look up a media device's provider-native id and a live provider built from
/// its (decrypted) credentials. Shared by the favorites services.
enum ProviderLookup {
    Found(String, Box<dyn crate::providers::MediaProvider>),
    NotFound,
    Db,
}

async fn lookup_media_provider(state: &AppState, id: &str) -> ProviderLookup {
    // A disabled device receives no commands (control lookups skip it).
    let row = sqlx::query(
        "SELECT a.device_id, p.provider_type, p.credentials
         FROM media_devices a JOIN providers p ON a.provider_id = p.id
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
    match build_media_provider(state, &provider_type, &credentials) {
        Ok(p) => ProviderLookup::Found(device_id, p),
        Err(e) => {
            tracing::error!("failed to build media provider: {e:#}");
            ProviderLookup::Db
        }
    }
}

/// Best-effort title resolution for the TV content resolver: search the device's
/// libraries for `query` and start the top hit. `true` if something was found and
/// played, `false` if the search matched nothing, the provider has no search, or
/// the device couldn't be reached — so the resolver falls back to opening an app.
pub(crate) async fn search_and_play_on_device(state: &AppState, id: &str, query: &str) -> bool {
    let (device_id, provider) = match lookup_media_provider(state, id).await {
        ProviderLookup::Found(d, p) => (d, p),
        _ => return false,
    };
    match provider.search_and_play(&device_id, query).await {
        Ok(played) => played,
        Err(e) => {
            tracing::debug!("media search unavailable for {id}: {e:#}");
            false
        }
    }
}

// ── Favorites (list + play) ──────────────────────────────────────────────────

pub(crate) enum FavoritesOutcome {
    Ok(Vec<MediaFavorite>),
    NotFound,
    Unreachable,
    Db,
}

pub(crate) async fn list_device_favorites(state: &AppState, id: &str) -> FavoritesOutcome {
    // Favorites live on whichever composite member advertises them (e.g. a Sonos
    // companion merged into a TV), not necessarily the primary surface.
    let id = &capable_backing(state, id, |c| c.favorites).await;
    let (device_id, provider) = match lookup_media_provider(state, id).await {
        ProviderLookup::Found(d, p) => (d, p),
        ProviderLookup::NotFound => return FavoritesOutcome::NotFound,
        ProviderLookup::Db => return FavoritesOutcome::Db,
    };
    match provider.list_favorites(&device_id).await {
        Ok(favs) => FavoritesOutcome::Ok(favs),
        Err(e) => {
            tracing::debug!("media favorites unavailable for {id}: {e:#}");
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
    let id = &capable_backing(state, id, |c| c.favorites).await;
    let (device_id, provider) = match lookup_media_provider(state, id).await {
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
            tracing::error!("media play_favorite error: {e:#}");
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
struct MediaRow {
    device_id: String,
    /// `media_devices.provider_id` — the providers-table row id (which provider
    /// instance owns the device), not the provider-native id.
    provider_row_id: String,
    provider_type: String,
    credentials: String,
}

async fn load_media_row(state: &AppState, id: &str) -> Result<Option<MediaRow>, ()> {
    let row = sqlx::query(
        "SELECT a.device_id, a.provider_id AS provider_row_id, p.provider_type, p.credentials
         FROM media_devices a JOIN providers p ON a.provider_id = p.id
         WHERE a.id = ? AND p.enabled = 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| tracing::error!("db error loading media device: {e}"))?;
    Ok(row.map(|r| MediaRow {
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
            tracing::error!("media grouping error: {e:#}");
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
    let member = match load_media_row(state, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return GroupOutcome::NotFound,
        Err(()) => return GroupOutcome::Db,
    };
    let coordinator = match load_media_row(state, coordinator_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return GroupOutcome::NotFound,
        Err(()) => return GroupOutcome::Db,
    };
    if member.provider_row_id != coordinator.provider_row_id {
        return GroupOutcome::BadRequest(
            "speakers must belong to the same provider to be grouped".into(),
        );
    }
    let provider = match build_media_provider(state, &member.provider_type, &member.credentials) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to build media provider: {e:#}");
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
    let row = match load_media_row(state, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return GroupOutcome::NotFound,
        Err(()) => return GroupOutcome::Db,
    };
    let provider = match build_media_provider(state, &row.provider_type, &row.credentials) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to build media provider: {e:#}");
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

/// Cast content to a media device (the casting seam — a TV/media_player).
/// Resolves the device's provider and calls `play_media` (raw `content_id` +
/// `content_type` passthrough). Skeleton: HA implements it via
/// `media_player.play_media`; richer resolution (app deep-links, title search,
/// the "play X on the bedroom TV" voice path) is future work.
pub(crate) async fn cast_to_device(
    state: &AppState,
    id: &str,
    content_id: &str,
    content_type: &str,
) -> SetMediaOutcome {
    // `play_media` has no capability flag, so cast is tried across the composite's
    // members (primary first): a TV's native row may not cast while its HA
    // companion does — and which is the primary must not matter.
    let backings = load_composite_backings(state, id).await;
    let ids: Vec<String> = if backings.len() <= 1 {
        vec![id.to_string()]
    } else {
        backings.iter().map(|b| b.id.clone()).collect()
    };
    let mut last = SetMediaOutcome::NotFound;
    for bid in &ids {
        match cast_one(state, bid, content_id, content_type).await {
            SetMediaOutcome::Ok => return SetMediaOutcome::Ok,
            // This member can't cast (or failed) — try the next composite member.
            other => last = other,
        }
    }
    last
}

/// Cast to one concrete device (no composite resolution).
async fn cast_one(
    state: &AppState,
    id: &str,
    content_id: &str,
    content_type: &str,
) -> SetMediaOutcome {
    let row = sqlx::query(
        "SELECT a.device_id, p.provider_type, p.credentials
         FROM media_devices a JOIN providers p ON a.provider_id = p.id
         WHERE a.id = ? AND p.enabled = 1 AND a.enabled = 1 AND a.shadowed_by IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await;
    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return SetMediaOutcome::NotFound,
        Err(e) => {
            tracing::error!("db error: {e}");
            return SetMediaOutcome::Db;
        }
    };
    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials: String = row.get("credentials");
    let provider = match build_media_provider(state, &provider_type, &credentials) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to build media provider: {e:#}");
            return SetMediaOutcome::Db;
        }
    };
    tracing::debug!(media = %id, device = %device_id, content_type, content_id, "cast → provider");
    match provider
        .play_media(&device_id, content_id, content_type)
        .await
    {
        Ok(()) => {
            tracing::debug!(media = %id, "cast ok");
            SetMediaOutcome::Ok
        }
        // "does not support casting" is the caller's mistake (wrong device), 422.
        Err(e) if e.to_string().contains("support") => SetMediaOutcome::BadCommand(e.to_string()),
        Err(e) => {
            tracing::error!(media = %id, "cast error: {e:#}");
            SetMediaOutcome::ProviderError
        }
    }
}

pub(crate) fn set_media_status(outcome: SetMediaOutcome) -> axum::response::Response {
    match outcome {
        SetMediaOutcome::Ok => StatusCode::NO_CONTENT.into_response(),
        SetMediaOutcome::NotFound => StatusCode::NOT_FOUND.into_response(),
        SetMediaOutcome::BadCommand(m) => (StatusCode::UNPROCESSABLE_ENTITY, m).into_response(),
        SetMediaOutcome::ProviderError => StatusCode::BAD_GATEWAY.into_response(),
        SetMediaOutcome::Db => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Discover a media provider's devices and upsert them. Returns the count.
/// Called from the shared `/api/providers/{id}/discover` handler.
pub(crate) async fn discover_media_devices(
    state: &AppState,
    provider_row_id: &str,
    provider_type: &str,
    credentials_enc: &str,
) -> Result<usize, StatusCode> {
    let provider = build_media_provider(state, provider_type, credentials_enc).map_err(|e| {
        tracing::error!("failed to build media provider: {e:#}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let devices = provider.discover().await.map_err(|e| {
        tracing::error!("media discovery error: {e:#}");
        StatusCode::BAD_GATEWAY
    })?;

    // Batch the upserts in one transaction (one WAL commit, not one per device).
    let mut tx = state.db.begin().await.map_err(|e| {
        tracing::error!("discover_media_devices: begin failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    for device in &devices {
        let caps = serde_json::to_string(&device.capabilities).unwrap_or_default();
        let state_json = serde_json::to_string(&device.state).unwrap_or_default();
        let kind = match device.kind {
            crate::models::media::MediaDeviceKind::Receiver => "receiver",
            crate::models::media::MediaDeviceKind::Speaker => "speaker",
            crate::models::media::MediaDeviceKind::Tv => "tv",
            crate::models::media::MediaDeviceKind::Zone => "zone",
        };
        let _ = sqlx::query(
            "INSERT INTO media_devices (id, provider_id, device_id, name, provider_name, kind, capabilities, last_state, last_seen, hw_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now'), ?)
             ON CONFLICT (provider_id, device_id)
             DO UPDATE SET name         = CASE WHEN name = provider_name THEN excluded.name ELSE name END,
                           provider_name = excluded.provider_name,
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
        .bind(&device.name)
        .bind(kind)
        .bind(&caps)
        .bind(&state_json)
        .bind(&device.hw_id)
        .execute(&mut *tx)
        .await;
    }
    tx.commit().await.map_err(|e| {
        tracing::error!("discover_media_devices: commit failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(devices.len())
}

// ── Handlers (session-authenticated) ─────────────────────────────────────────

async fn list_devices_handler(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    match list_all_devices(&state).await {
        Ok(devices) => Json(devices).into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn get_device_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match get_device_live(&state, &id).await {
        Ok(Some(device)) => Json(device).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn set_device_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(cmd): Json<MediaCommand>,
) -> impl IntoResponse {
    set_media_status(apply_media_command(&state, &id, &cmd).await)
}

/// Body for casting content to a device (the casting seam). `content_type` is the
/// provider-native kind (HA: `music`/`url`/`app`/`channel`/…). Shared by the
/// session route and `/api/v1`.
#[derive(serde::Deserialize)]
pub(crate) struct CastRequest {
    pub(crate) content_id: String,
    pub(crate) content_type: String,
}

async fn cast_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<CastRequest>,
) -> impl IntoResponse {
    set_media_status(cast_to_device(&state, &id, &req.content_id, &req.content_type).await)
}

async fn list_favorites_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    favorites_response(list_device_favorites(&state, &id).await)
}

async fn play_favorite_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<PlayFavoriteRequest>,
) -> impl IntoResponse {
    play_favorite_response(play_device_favorite(&state, &id, &req.favorite_id).await)
}

async fn group_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<GroupRequest>,
) -> impl IntoResponse {
    group_response(group_devices(&state, &id, &req.coordinator_id).await)
}

async fn ungroup_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    group_response(ungroup_device(&state, &id).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::media::TransportCmd;

    fn backing(
        id: &str,
        transport: bool,
        sources: bool,
        receiver_bound: bool,
        now_playing: bool,
    ) -> Backing {
        backing_pri(id, transport, sources, receiver_bound, now_playing, 1)
    }

    #[allow(clippy::too_many_arguments)]
    fn backing_pri(
        id: &str,
        transport: bool,
        sources: bool,
        receiver_bound: bool,
        now_playing: bool,
        priority: u8,
    ) -> Backing {
        Backing {
            id: id.into(),
            name: id.into(),
            capabilities: MediaCapabilities {
                transport,
                sources,
                ..Default::default()
            },
            receiver_id: receiver_bound.then(|| format!("recv-{id}")),
            receiver_bound,
            has_now_playing: now_playing,
            volume: 0,
            priority,
        }
    }

    fn backing_vol(id: &str, volume: u8) -> Backing {
        Backing {
            id: id.into(),
            name: id.into(),
            capabilities: MediaCapabilities::default(),
            receiver_id: None,
            receiver_bound: false,
            has_now_playing: false,
            volume,
            priority: 1,
        }
    }

    #[test]
    fn volume_routes_to_the_backing_carrying_audio() {
        // A TV (primary) whose media_player reads volume 0, merged with a second
        // view that has the real volume — volume must route to the audio-carrying
        // backing, not the silent primary.
        let backings = vec![backing_vol("tv", 0), backing_vol("audio", 50)];
        let cmd = MediaCommand {
            volume: Some(30),
            ..Default::default()
        };
        let routed: std::collections::HashMap<String, MediaCommand> =
            route_across_backings(&cmd, &backings).into_iter().collect();
        assert_eq!(routed["audio"].volume, Some(30));
        assert!(!routed.contains_key("tv"));
    }

    fn sig(reachable: bool, on: bool) -> PowerSignal {
        PowerSignal { reachable, on }
    }

    #[test]
    fn on_media_view_overrides_a_stale_off_primary() {
        // The exact Bravia bug: the primary media_player reads off/stale while a
        // fresh companion media view (a Cast entity for the same TV) reports on +
        // playing. The composite must read ON — and the same whichever row is the
        // primary (order-independent, M26).
        let r = resolve_composite_power(sig(true, false), &[sig(true, true)], &[]);
        assert!(r.reachable && r.on);
        let r = resolve_composite_power(sig(true, true), &[sig(true, false)], &[]);
        assert!(r.reachable && r.on);
        // Every reachable media view reads off → off.
        let r = resolve_composite_power(sig(true, false), &[sig(true, false)], &[]);
        assert!(r.reachable && !r.on);
    }

    #[test]
    fn reachable_remote_does_not_override_a_reachable_media_view() {
        // A stale paired remote reporting on must not force a powered-off (but
        // reachable) TV on — media views win while any is reachable; the remote is
        // only the standby-wake fallback.
        let r = resolve_composite_power(sig(true, false), &[], &[sig(true, true)]);
        assert!(r.reachable && !r.on);
    }

    #[test]
    fn unreachable_primary_falls_back_to_reachable_remote() {
        // Standby TV: no reachable media view, but the paired remote reports on →
        // the composite is on + reachable.
        let r = resolve_composite_power(sig(false, false), &[], &[sig(true, true)]);
        assert!(r.reachable && r.on);
        // Remote reachable but off → reachable, off.
        let r = resolve_composite_power(sig(false, false), &[], &[sig(true, false)]);
        assert!(r.reachable && !r.on);
    }

    #[test]
    fn fully_offline_composite_stays_unreachable() {
        // Nothing reachable (cold TV) → left as-is; the client still offers WoL.
        let r = resolve_composite_power(
            sig(false, false),
            &[sig(false, false)],
            &[sig(false, false)],
        );
        assert!(!r.reachable && !r.on);
        let r = resolve_composite_power(sig(false, false), &[], &[]);
        assert!(!r.reachable && !r.on);
    }

    #[test]
    fn now_playing_score_prefers_active_content_over_idle() {
        use crate::models::media::{NowPlaying, PlayState};
        let none: Option<NowPlaying> = None;
        let empty = Some(NowPlaying::default());
        let idle_title = Some(NowPlaying {
            title: Some("X".into()),
            play_state: Some(PlayState::Stopped),
            ..Default::default()
        });
        let playing_title = Some(NowPlaying {
            title: Some("X".into()),
            play_state: Some(PlayState::Playing),
            ..Default::default()
        });
        let playing_no_title = Some(NowPlaying {
            play_state: Some(PlayState::Playing),
            ..Default::default()
        });
        // active+content > idle+content > active-no-content > empty == none.
        assert!(now_playing_score(&playing_title) > now_playing_score(&idle_title));
        assert!(now_playing_score(&idle_title) > now_playing_score(&playing_no_title));
        assert!(now_playing_score(&playing_no_title) > now_playing_score(&empty));
        assert_eq!(now_playing_score(&empty), now_playing_score(&none));
        assert_eq!(now_playing_score(&none), 0);
    }

    fn paired(id: &str, priority: u8) -> PairedRemote {
        PairedRemote {
            group_id: "grp".into(),
            remote_id: id.into(),
            priority,
            state: None,
        }
    }

    #[test]
    fn composite_surfaces_the_native_remote_regardless_of_merge_order() {
        // A composite TV paired to both a native vendor remote (priority 1, carries
        // the full IRCC catalogue) and an HA `remote.*` copy (priority 0). The
        // native one must win whichever device was merged into which — the bug was
        // a `LIMIT 1` that could surface the catalogue-less HA copy.
        let native_first = best_remote_per_group(&[paired("native", 1), paired("ha", 0)]);
        assert_eq!(native_first.get("grp").map(String::as_str), Some("native"));
        let ha_first = best_remote_per_group(&[paired("ha", 0), paired("native", 1)]);
        assert_eq!(ha_first.get("grp").map(String::as_str), Some("native"));
    }

    #[test]
    fn equal_priority_remotes_break_ties_deterministically() {
        // Two remotes of the same class → the smallest id, independent of order, so
        // the surfaced remote never flickers with load ordering.
        let a = best_remote_per_group(&[paired("zulu", 1), paired("alpha", 1)]);
        let b = best_remote_per_group(&[paired("alpha", 1), paired("zulu", 1)]);
        assert_eq!(a.get("grp").map(String::as_str), Some("alpha"));
        assert_eq!(b.get("grp").map(String::as_str), Some("alpha"));
    }

    #[test]
    fn backing_authority_puts_native_above_its_ha_twin() {
        let reg = crate::providers::default_registry();
        // The only authority distinction a composite needs: a native provider
        // (vendor TV, receiver, speaker, native remote) outranks its HA copy.
        assert!(backing_authority(&reg, "smarttv") > backing_authority(&reg, "ha"));
        assert!(backing_authority(&reg, "onkyo") > backing_authority(&reg, "ha"));
        assert!(backing_authority(&reg, "sonos") > backing_authority(&reg, "ha"));
        // Same-class natives tie — contested controls then keep the primary-first
        // order, and physical routing (receiver binding, now-playing) decides the
        // rest independently of authority.
        assert_eq!(
            backing_authority(&reg, "onkyo"),
            backing_authority(&reg, "smarttv")
        );
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
        let cmd = MediaCommand {
            power: Some(true),
            volume: Some(30),
            mute: Some(true),
            source: None,
            transport: Some(TransportCmd::Toggle),
        };
        let routed: std::collections::HashMap<String, MediaCommand> =
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
        let cmd = MediaCommand {
            volume: Some(50),
            source: Some("hdmi1".into()),
            ..Default::default()
        };
        let routed: std::collections::HashMap<String, MediaCommand> =
            route_across_backings(&cmd, &backings).into_iter().collect();

        assert_eq!(routed["primary"].volume, Some(50));
        assert_eq!(routed["primary"].source.as_deref(), Some("hdmi1"));
    }

    #[test]
    fn higher_priority_backing_wins_contested_controls() {
        // An HA copy is the primary (backings[0]); a native TV merged in as a
        // companion outranks it. Power and source must route to the native TV,
        // even though it isn't the primary — the priority is its precedence.
        let backings = vec![
            backing_pri("ha", true, true, false, false, 0),
            backing_pri("native_tv", true, true, false, false, 1),
        ];
        let cmd = MediaCommand {
            power: Some(true),
            source: Some("hdmi2".into()),
            transport: Some(TransportCmd::Play),
            ..Default::default()
        };
        let routed: std::collections::HashMap<String, MediaCommand> =
            route_across_backings(&cmd, &backings).into_iter().collect();

        assert_eq!(routed["native_tv"].power, Some(true));
        assert_eq!(routed["native_tv"].source.as_deref(), Some("hdmi2"));
        assert_eq!(routed["native_tv"].transport, Some(TransportCmd::Play));
        assert!(!routed.contains_key("ha"));
    }
}
