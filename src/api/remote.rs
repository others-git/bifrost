//! Remote-control API: list virtual remotes (TV / streamer D-pad + app launch),
//! read live state, and send commands (canonical keys, text, app launch, power).
//!
//! Mirrors the other device APIs: the behaviour lives in service functions
//! shared by the session routes here, the Bearer-key `v1` routes, and MCP — so
//! the three surfaces can't drift. A "command" is the tagged [`RemoteCommand`]
//! union; the provider maps Bifrost's canonical [`RemoteKey`] to its native
//! vocabulary. Reads hit the device live and refresh `last_state`, falling back
//! to the cache (`reachable: false`) when unreachable.
//!
//! A remote is **paired** to its TV's media device (`paired_media_id`) when a
//! `media_player` shares its hardware id — set during discovery in
//! [`crate::api::dedup`]-adjacent pairing ([`reconcile_remote_pairings`]).

use crate::AppState;
use crate::api::auth::Session;
use crate::models::remote::app_display_name;
use crate::models::remote::{RemoteCommand, RemoteCommandInfo, RemoteState};
use anyhow::Context as _;
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
        .route("/devices/{id}/command", post(command_handler))
        .route("/devices/{id}/commands", get(list_commands_handler))
        .route("/devices/{id}/commands/pin", put(pin_command_handler))
        .route("/devices/{id}/apps", get(list_apps_handler))
        .route("/devices/{id}/apps/pin", put(pin_app_handler))
        .merge(crate::api::basic_inventory_router(
            "/devices",
            "remote_devices",
        ))
}

// ── Wire shape ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub(crate) struct RemoteDeviceRow {
    pub id: String,
    pub provider_id: String,
    pub device_id: String,
    pub name: String,
    pub state: RemoteState,
    pub last_seen: Option<String>,
    pub enabled: bool,
    pub glyph: Option<String>,
    pub hw_id: Option<String>,
    /// The composite **group** this remote belongs to (shared with its TV's
    /// media rows), if paired to a known TV. Replaces the old `paired_media_id`.
    pub group_id: Option<String>,
}

fn row_to_remote(r: &sqlx::sqlite::SqliteRow) -> RemoteDeviceRow {
    let state = r
        .get::<Option<String>, _>("last_state")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    RemoteDeviceRow {
        id: r.get("id"),
        provider_id: r.get("provider_id"),
        device_id: r.get("device_id"),
        name: r.get("name"),
        state,
        last_seen: r.get("last_seen"),
        enabled: r.get::<i64, _>("enabled") != 0,
        glyph: r.get("glyph"),
        hw_id: r.get("hw_id"),
        group_id: r.try_get("group_id").ok().flatten(),
    }
}

const SELECT_REMOTE: &str = "SELECT id, provider_id, device_id, name, last_state, last_seen, \
     enabled, glyph, hw_id, group_id FROM remote_devices";

// ── Provider build / discovery ───────────────────────────────────────────────

pub(crate) fn build_remote_provider(
    state: &AppState,
    provider_type: &str,
    credentials_enc: &str,
) -> anyhow::Result<Box<dyn crate::providers::RemoteProvider>> {
    // Name the provider in the chain — a bare \"decryption failed\" doesn't
    // say WHICH provider needs its credentials re-entered.
    let creds_json = state
        .decrypt_credentials(credentials_enc)
        .with_context(|| format!("{provider_type} credentials"))?;
    state.registry.build_remote(provider_type, &creds_json)
}

/// Discover a provider's remotes and upsert them. Returns the count discovered.
/// Called by the additive `/api/providers/{id}/discover` handler.
pub(crate) async fn discover_remote_devices(
    state: &AppState,
    provider_row_id: &str,
    provider_type: &str,
    credentials_enc: &str,
) -> Result<usize, StatusCode> {
    let provider = build_remote_provider(state, provider_type, credentials_enc).map_err(|e| {
        tracing::error!("failed to build remote provider: {e:#}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let devices = provider.discover().await.map_err(|e| {
        tracing::error!("remote discovery error: {e:#}");
        StatusCode::BAD_GATEWAY
    })?;
    for device in &devices {
        let state_json = serde_json::to_string(&device.state).unwrap_or_default();
        let _ = sqlx::query(
            "INSERT INTO remote_devices (id, provider_id, device_id, name, last_state, last_seen, hw_id)
             VALUES (?, ?, ?, ?, ?, datetime('now'), ?)
             ON CONFLICT (provider_id, device_id)
             DO UPDATE SET name       = excluded.name,
                           last_state = excluded.last_state,
                           last_seen  = excluded.last_seen,
                           hw_id      = excluded.hw_id",
        )
        .bind(device.id.to_string())
        .bind(provider_row_id)
        .bind(&device.provider_id)
        .bind(&device.name)
        .bind(&state_json)
        .bind(&device.hw_id)
        .execute(&state.db)
        .await;
        // Recents are recorded on live reads (`read_remote_state`), keyed by the
        // device's stable stored id — not here, where `device.id` is a fresh uuid
        // that an upsert discards on a re-discovery (the row keeps its first id).
    }
    Ok(devices.len())
}

/// Pair each remote to a TV media device that shares its `hw_id` (same physical
/// box — an Android TV's `remote.*` and `media_player.*` share one HA device).
/// Idempotent; run after discovery. A remote with no hw_id match is left
/// unpaired. Prefers an media device of TV kind when several share a hw_id.
pub(crate) async fn reconcile_remote_pairings(state: &AppState) {
    // 1) Ensure each non-shadowed media device that shares a hw_id with a remote
    //    has a composite group (a singleton on its own id if it isn't merged into
    //    one) — so the remote has a group to join.
    let _ = sqlx::query(
        "UPDATE media_devices SET group_id = id
          WHERE group_id IS NULL AND shadowed_by IS NULL AND hw_id IS NOT NULL
            AND hw_id IN (SELECT hw_id FROM remote_devices WHERE hw_id IS NOT NULL)",
    )
    .execute(&state.db)
    .await;

    // 2) Join each *unpaired* remote (group_id NULL) to its TV's group by hw_id,
    //    preferring a `tv`-kind media device. `group_id IS NULL` means a manual
    //    merge is never clobbered by a later discovery.
    let _ = sqlx::query(
        "UPDATE remote_devices
            SET group_id = (
                SELECT a.group_id FROM media_devices a
                 WHERE a.hw_id = remote_devices.hw_id
                   AND a.shadowed_by IS NULL
                 ORDER BY (a.kind = 'tv') DESC
                 LIMIT 1)
          WHERE hw_id IS NOT NULL AND group_id IS NULL",
    )
    .execute(&state.db)
    .await;
}

// ── App tracking (recents + pinned) ──────────────────────────────────────────

/// One launchable app on a remote's TV: a Play Store package, a friendly name,
/// whether the user pinned it, and when it was last seen foreground.
#[derive(Debug, Serialize)]
pub(crate) struct RemoteApp {
    pub package: String,
    pub name: String,
    pub pinned: bool,
    pub last_seen: Option<String>,
    /// The vendor launch URI from the TV's catalog, when known — the exact
    /// token to launch with (a bare package doesn't launch on every vendor).
    pub activity: Option<String>,
}

/// `true` if `activity` looks like a launchable package id (not a deep-link URL).
fn looks_like_package(activity: &str) -> bool {
    !activity.contains("://") && activity.contains('.') && !activity.contains(' ')
}

/// Record that `package` was seen foreground on `remote_id` (a "recent"). Upserts
/// without disturbing an existing pin. No-op for non-package activities.
/// A vendor launch URI (`<package>-<activity>`, what the catalog launches with)
/// normalizes to its bare package — the identity every other source (ATV push,
/// HA `current_activity`, the catalog itself) uses; recording the raw URI
/// minted a second row per launched app (Plex twice: once from the catalog,
/// once keyed by its launch URI).
pub(crate) async fn record_app_seen(state: &AppState, remote_id: &str, package: &str) {
    let package = package.split_once('-').map_or(package, |(p, _)| p);
    if !looks_like_package(package) {
        return;
    }
    // On conflict only the recency moves — the stored name may be the TV
    // catalog's real title, which the package-derived guess must not clobber.
    let _ = sqlx::query(
        "INSERT INTO remote_apps (remote_id, package, name, pinned, last_seen)
         VALUES (?, ?, ?, 0, datetime('now'))
         ON CONFLICT (remote_id, package)
         DO UPDATE SET last_seen = datetime('now')",
    )
    .bind(remote_id)
    .bind(package)
    .bind(app_display_name(package))
    .execute(&state.db)
    .await;
}

/// Launchable apps for a remote: pinned first, then recents by most-recent,
/// then the rest of the TV's catalog by name. A row that came from the device
/// catalog (it has an `activity`) keeps the TV's own title; anything else
/// (observed-only foreground packages) re-derives its display name from the
/// package on read, so the brand-keyword/prettify logic stays authoritative
/// for rows the TV never named.
pub(crate) async fn list_remote_apps(state: &AppState, remote_id: &str) -> Vec<RemoteApp> {
    sqlx::query(
        "SELECT package, name, activity, pinned, last_seen FROM remote_apps
         WHERE remote_id = ?
         ORDER BY pinned DESC, (last_seen IS NULL), last_seen DESC, name COLLATE NOCASE",
    )
    .bind(remote_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .iter()
    .map(|r| {
        let package: String = r.get("package");
        let activity: Option<String> = r.get("activity");
        let stored: Option<String> = r.get("name");
        let name = match (&activity, stored) {
            (Some(_), Some(n)) if !n.trim().is_empty() => n,
            _ => app_display_name(&package),
        };
        RemoteApp {
            name,
            pinned: r.get::<i64, _>("pinned") != 0,
            last_seen: r.get("last_seen"),
            activity,
            package,
        }
    })
    .collect()
}

/// Pull the TV's installed-app catalog through the remote's provider and fold
/// it into `remote_apps` (title + launch URI per bare package; pins and recency
/// untouched) — the launcher lists what's INSTALLED, not just what's been seen
/// in the foreground. Best-effort: an unreachable TV keeps the cached rows.
pub(crate) async fn sync_app_catalog(state: &AppState, remote_id: &str) {
    let row = sqlx::query(
        "SELECT r.device_id, p.provider_type, p.credentials
           FROM remote_devices r JOIN providers p ON r.provider_id = p.id
          WHERE r.id = ? AND p.enabled = 1 AND r.enabled = 1",
    )
    .bind(remote_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    let Some(row) = row else { return };
    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials_enc: String = row.get("credentials");
    let provider = match build_remote_provider(state, &provider_type, &credentials_enc) {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(target: "bifrost::remote", remote = %remote_id, "app catalog: provider build failed: {e:#}");
            return;
        }
    };
    let apps = match provider.list_apps(&device_id).await {
        Ok(a) => a,
        Err(e) => {
            tracing::debug!(target: "bifrost::remote", remote = %remote_id, "app catalog read failed (keeping cached): {e:#}");
            return;
        }
    };
    for app in &apps {
        let _ = sqlx::query(
            "INSERT INTO remote_apps (remote_id, package, name, activity, pinned, last_seen)
             VALUES (?, ?, ?, ?, 0, NULL)
             ON CONFLICT (remote_id, package)
             DO UPDATE SET name = excluded.name, activity = excluded.activity",
        )
        .bind(remote_id)
        .bind(&app.package)
        .bind(&app.name)
        .bind(&app.activity)
        .execute(&state.db)
        .await;
    }
    // Sweep rows keyed by a launch URI (`<package>-<activity>`) into their
    // bare-package row: carry the pin and freshest recency over, then drop the
    // duplicate. Cleans up rows minted before launch recording normalized.
    let rows = sqlx::query(
        "SELECT package, pinned, last_seen FROM remote_apps WHERE remote_id = ? AND instr(package, '-') > 0",
    )
    .bind(remote_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    for row in rows {
        let uri: String = row.get("package");
        let Some((bare, _)) = uri.split_once('-') else {
            continue;
        };
        let pinned: i64 = row.get("pinned");
        let last_seen: Option<String> = row.get("last_seen");
        let merged = sqlx::query(
            "UPDATE remote_apps
                SET pinned = MAX(pinned, ?),
                    last_seen = COALESCE(MAX(last_seen, ?), last_seen, ?)
              WHERE remote_id = ? AND package = ?",
        )
        .bind(pinned)
        .bind(&last_seen)
        .bind(&last_seen)
        .bind(remote_id)
        .bind(bare)
        .execute(&state.db)
        .await;
        if matches!(merged, Ok(r) if r.rows_affected() > 0) {
            let _ = sqlx::query("DELETE FROM remote_apps WHERE remote_id = ? AND package = ?")
                .bind(remote_id)
                .bind(&uri)
                .execute(&state.db)
                .await;
            tracing::debug!(target: "bifrost::remote", remote = %remote_id, uri = %uri, into = %bare, "merged a launch-URI app row into its package row");
        }
    }
    tracing::debug!(target: "bifrost::remote", remote = %remote_id, apps = apps.len(), "app catalog synced from the device");
}

/// Pin or unpin an app on a remote. Pinning a never-seen package inserts it
/// (so the user can add an app before it's ever been foreground).
pub(crate) async fn set_app_pin(
    state: &AppState,
    remote_id: &str,
    package: &str,
    pinned: bool,
) -> StatusCode {
    let res = sqlx::query(
        "INSERT INTO remote_apps (remote_id, package, name, pinned, last_seen)
         VALUES (?, ?, ?, ?, NULL)
         ON CONFLICT (remote_id, package) DO UPDATE SET pinned = excluded.pinned",
    )
    .bind(remote_id)
    .bind(package)
    .bind(app_display_name(package))
    .bind(i64::from(pinned))
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(e) => {
            tracing::error!("db error pinning app: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

// ── Service layer (shared by session / v1 / MCP) ─────────────────────────────

/// Outcome of a control/read call, mapped to HTTP by each surface.
#[derive(Debug)]
pub(crate) enum RemoteOutcome {
    Ok,
    NotFound,
    ProviderError,
    Db,
}

/// Resolve a remote to its provider, then run `cmd`. A disabled remote, a
/// disabled provider, or an unknown id yields `NotFound` (no command sent).
pub(crate) async fn apply_remote_command(
    state: &AppState,
    id: &str,
    cmd: &RemoteCommand,
) -> RemoteOutcome {
    let row = sqlx::query(
        "SELECT r.device_id, r.hw_id, p.provider_type, p.credentials
           FROM remote_devices r JOIN providers p ON r.provider_id = p.id
          WHERE r.id = ? AND p.enabled = 1 AND r.enabled = 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await;
    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return RemoteOutcome::NotFound,
        Err(e) => {
            tracing::error!("db error: {e}");
            return RemoteOutcome::Db;
        }
    };
    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials_enc: String = row.get("credentials");
    let provider = match build_remote_provider(state, &provider_type, &credentials_enc) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to build remote provider: {e:#}");
            return RemoteOutcome::Db;
        }
    };
    tracing::debug!(remote = %id, device = %device_id, command = ?cmd, "remote command → provider");
    let result = match cmd {
        RemoteCommand::Key { key, hold_secs } => {
            provider.send_key(&device_id, *key, *hold_secs).await
        }
        RemoteCommand::Text { text } => provider.send_text(&device_id, text).await,
        RemoteCommand::LaunchApp { activity } => provider.launch_app(&device_id, activity).await,
        RemoteCommand::Power { on } => {
            // Wake-on-LAN nudge first: a TV in network standby won't answer the
            // provider's `turn_on`, but its NIC will answer a magic packet. No-op
            // for non-MAC ids; failures are non-fatal (the turn_on still runs).
            if *on
                && let Some(hw) = row.get::<Option<String>, _>("hw_id")
                && let Err(e) = crate::wol::wake(&hw).await
            {
                tracing::debug!("WoL nudge for {device_id} failed (non-fatal): {e:#}");
            }
            provider.set_power(&device_id, *on).await
        }
        RemoteCommand::Native { token } => provider.send_native(&device_id, token).await,
    };
    match result {
        Ok(()) => {
            // A launched app is, by definition, a "recent" — record it.
            if let RemoteCommand::LaunchApp { activity } = cmd {
                record_app_seen(state, id, activity).await;
            }
            tracing::debug!(remote = %id, "remote command ok");
            RemoteOutcome::Ok
        }
        Err(e) => {
            tracing::error!("remote command failed for {device_id}: {e:#}");
            RemoteOutcome::ProviderError
        }
    }
}

/// Live-read a remote's state, refreshing the cache; falls back to cache on
/// error. Returns `None` if the remote/provider is unknown or disabled.
pub(crate) async fn read_remote_state(state: &AppState, id: &str) -> Option<RemoteState> {
    let row = sqlx::query(
        "SELECT r.device_id, r.last_state, p.provider_type, p.credentials
           FROM remote_devices r JOIN providers p ON r.provider_id = p.id
          WHERE r.id = ? AND p.enabled = 1 AND r.enabled = 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .ok()??;
    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials_enc: String = row.get("credentials");
    let cached: RemoteState = row
        .get::<Option<String>, _>("last_state")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let provider = build_remote_provider(state, &provider_type, &credentials_enc).ok()?;
    match provider.get_state(&device_id).await {
        Ok(mut live) => {
            if live.reachable.is_none() {
                live.reachable = Some(true);
            }
            let json = serde_json::to_string(&live).unwrap_or_default();
            let _ = sqlx::query(
                "UPDATE remote_devices SET last_state = ?, last_seen = datetime('now') WHERE id = ?",
            )
            .bind(&json)
            .bind(id)
            .execute(&state.db)
            .await;
            // Record the foreground app as a "recent" (HA gives no app list).
            if let Some(app) = live.current_app.as_deref() {
                record_app_seen(state, id, app).await;
            }
            Some(live)
        }
        Err(e) => {
            tracing::debug!("remote get_state failed, using cache: {e:#}");
            Some(RemoteState {
                reachable: Some(false),
                ..cached
            })
        }
    }
}

pub(crate) async fn list_remotes(state: &AppState) -> Vec<RemoteDeviceRow> {
    sqlx::query(&format!("{SELECT_REMOTE} ORDER BY name"))
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
        .iter()
        .map(row_to_remote)
        .collect()
}

/// The remote's **expanded** command catalogue (the keys beyond the canonical set
/// it exposes, e.g. a Bravia's full IRCC list). Empty for remotes without one.
pub(crate) async fn list_remote_commands(state: &AppState, id: &str) -> Vec<RemoteCommandInfo> {
    let row = sqlx::query(
        "SELECT r.device_id, p.provider_type, p.credentials
           FROM remote_devices r JOIN providers p ON r.provider_id = p.id
          WHERE r.id = ? AND p.enabled = 1 AND r.enabled = 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    let Some(row) = row else { return Vec::new() };
    let device_id: String = row.get("device_id");
    let provider_type: String = row.get("provider_type");
    let credentials_enc: String = row.get("credentials");
    let Ok(provider) = build_remote_provider(state, &provider_type, &credentials_enc) else {
        return Vec::new();
    };
    let commands = provider.list_commands(&device_id).await.unwrap_or_else(|e| {
        tracing::debug!(target: "bifrost::smarttv", remote = %id, "list_commands failed: {e:#}");
        Vec::new()
    });
    let pinned: std::collections::HashSet<String> =
        sqlx::query("SELECT token FROM remote_command_pins WHERE remote_id = ?")
            .bind(id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default()
            .iter()
            .map(|r| r.get::<String, _>("token"))
            .collect();
    overlay_pins(commands, &pinned)
}

/// Mark each command pinned when its token is a favourite. Provider order is
/// preserved — the UI lifts the pinned ones into a favourites strip above the
/// full catalogue, so neither order nor membership depends on the pin set.
fn overlay_pins(
    mut commands: Vec<RemoteCommandInfo>,
    pinned: &std::collections::HashSet<String>,
) -> Vec<RemoteCommandInfo> {
    for c in &mut commands {
        c.pinned = pinned.contains(&c.token);
    }
    commands
}

/// Pin or unpin a native ("Full remote") command on a remote — the user's
/// favourites. Presence in `remote_command_pins` *is* the pin, so pinning inserts
/// and unpinning deletes. Mirrors [`set_app_pin`]; session-only UI config.
pub(crate) async fn set_command_pin(
    state: &AppState,
    remote_id: &str,
    token: &str,
    pinned: bool,
) -> StatusCode {
    let res = if pinned {
        sqlx::query(
            "INSERT INTO remote_command_pins (remote_id, token) VALUES (?, ?)
             ON CONFLICT (remote_id, token) DO NOTHING",
        )
        .bind(remote_id)
        .bind(token)
        .execute(&state.db)
        .await
    } else {
        sqlx::query("DELETE FROM remote_command_pins WHERE remote_id = ? AND token = ?")
            .bind(remote_id)
            .bind(token)
            .execute(&state.db)
            .await
    };
    match res {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(e) => {
            tracing::error!("db error pinning command: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

// ── Handlers (session-authenticated) ─────────────────────────────────────────

async fn list_devices_handler(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    Json(list_remotes(&state).await).into_response()
}

async fn get_device_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match read_remote_state(&state, &id).await {
        Some(s) => Json(s).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn list_commands_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    Json(list_remote_commands(&state, &id).await).into_response()
}

/// Body for pinning/unpinning a native command on a remote.
#[derive(serde::Deserialize)]
struct PinCommandRequest {
    token: String,
    pinned: bool,
}

async fn pin_command_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<PinCommandRequest>,
) -> impl IntoResponse {
    set_command_pin(&state, &id, &req.token, req.pinned)
        .await
        .into_response()
}

pub(crate) fn remote_status(outcome: RemoteOutcome) -> StatusCode {
    match outcome {
        RemoteOutcome::Ok => StatusCode::NO_CONTENT,
        RemoteOutcome::NotFound => StatusCode::NOT_FOUND,
        RemoteOutcome::ProviderError => StatusCode::BAD_GATEWAY,
        RemoteOutcome::Db => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn command_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(cmd): Json<RemoteCommand>,
) -> impl IntoResponse {
    remote_status(apply_remote_command(&state, &id, &cmd).await).into_response()
}

async fn list_apps_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    sync_app_catalog(&state, &id).await;
    Json(list_remote_apps(&state, &id).await).into_response()
}

/// Body for pinning/unpinning an app on a remote.
#[derive(serde::Deserialize)]
struct PinAppRequest {
    package: String,
    pinned: bool,
}

async fn pin_app_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<PinAppRequest>,
) -> impl IntoResponse {
    set_app_pin(&state, &id, &req.package, req.pinned)
        .await
        .into_response()
}

// ── Content resolver (TV backlog #1) — matching core ─────────────────────────
//
// Pure logic for turning a spoken app/title query into a launchable app on a
// remote's TV. The service layer feeds these the device's app catalog
// (`list_remote_apps`) and acts on the result; kept pure so the matching rules
// are unit-tested without a device or DB.

/// `false` for surfaces that aren't a user app to launch or to treat as the
/// "last-used" app — the screensaver, the launcher, and system UIs.
fn is_launchable_app(app: &RemoteApp) -> bool {
    if app.name == "Screensaver" {
        return false;
    }
    let p = app.package.to_ascii_lowercase();
    const SYSTEM: &[&str] = &[
        "dream",
        "screensaver",
        "backdrop",
        "launcher",
        "systemui",
        "settings",
        "inputmethod",
        "packageinstaller",
        "tvrecommendations",
        "frameworkpackagestubs",
    ];
    !SYSTEM.iter().any(|kw| p.contains(kw))
}

/// Best app match for `query` among the catalog, ignoring system surfaces. Tries,
/// in order: exact (case-insensitive) friendly name, name prefix, name substring,
/// then package substring.
fn match_app<'a>(query: &str, apps: &'a [RemoteApp]) -> Option<&'a RemoteApp> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return None;
    }
    let nm = |a: &RemoteApp| a.name.to_ascii_lowercase();
    let find = |pred: &dyn Fn(&RemoteApp) -> bool| -> Option<&'a RemoteApp> {
        apps.iter()
            .filter(|a| is_launchable_app(a))
            .find(|a| pred(a))
    };
    find(&|a| nm(a) == q)
        .or_else(|| find(&|a| nm(a).starts_with(&q)))
        .or_else(|| find(&|a| nm(a).contains(&q)))
        .or_else(|| find(&|a| a.package.to_ascii_lowercase().contains(&q)))
}

/// The device's most-recently-used launchable app — the "preferred app" fast path
/// for a title query (e.g. someone who only ever uses Hulu). The app with the
/// latest `last_seen`; `None` if nothing usable has been seen.
fn preferred_app(apps: &[RemoteApp]) -> Option<&RemoteApp> {
    apps.iter()
        .filter(|a| is_launchable_app(a) && a.last_seen.is_some())
        .max_by(|a, b| a.last_seen.cmp(&b.last_seen))
}

/// What [`resolve_and_play`] did, for the caller to phrase a reply.
pub(crate) enum ResolveOutcome {
    /// Launched the app the query named (its friendly name).
    Launched(String),
    /// Resolved a title to actual content and started playing it (the title we
    /// were asked for) — the TV's media search found and cast a match.
    Played(String),
    /// Opened the target app directly onto its search results for the title
    /// (deep link): `(title, app friendly name)` — the native "(app, title)"
    /// path when no richer content search exists.
    SearchedIn(String, String),
    /// Title intent we couldn't map to an app or resolve to content — opened the
    /// TV's last-used app as the best guess for where the title lives (its
    /// friendly name). Callers may add an HA-Assist fallback.
    OpenedPreferred(String),
    /// No app matched the query.
    NoMatch,
    /// No remote/TV resolved from the device name.
    NoRemote,
    /// Reaching the device failed.
    Failed,
}

/// Resolve a "device" name/id to the remote that drives its TV — a remote
/// directly, or a media device (TV) whose paired remote we use.
async fn resolve_remote(state: &AppState, query: &str) -> Option<String> {
    let q = query.trim();
    if q.is_empty() {
        return None;
    }
    // 1) a remote by id or exact (case-insensitive) name
    if let Ok(Some(id)) = sqlx::query_scalar::<_, String>(
        "SELECT id FROM remote_devices WHERE enabled = 1 AND (id = ?1 OR lower(name) = lower(?1)) LIMIT 1",
    )
    .bind(q)
    .fetch_optional(&state.db)
    .await
    {
        return Some(id);
    }
    // 2) a media device (TV) by id/name → its paired remote; else a substring
    //    match on either the remote's or the TV's name.
    sqlx::query_scalar::<_, String>(
        "SELECT r.id FROM remote_devices r
           LEFT JOIN media_devices m ON m.group_id = r.group_id AND r.group_id IS NOT NULL
         WHERE r.enabled = 1 AND (m.id = ?1 OR lower(m.name) = lower(?1)
              OR lower(r.name) LIKE '%' || lower(?1) || '%'
              OR lower(m.name) LIKE '%' || lower(?1) || '%') LIMIT 1",
    )
    .bind(q)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
}

async fn launch(state: &AppState, remote_id: &str, app: &RemoteApp) -> ResolveOutcome {
    let cmd = RemoteCommand::LaunchApp {
        // The catalog's vendor URI is the exact launch token; a bare package is
        // the fallback for apps only ever observed in the foreground.
        activity: app.activity.clone().unwrap_or_else(|| app.package.clone()),
    };
    match apply_remote_command(state, remote_id, &cmd).await {
        RemoteOutcome::Ok => ResolveOutcome::Launched(app.name.clone()),
        _ => ResolveOutcome::Failed,
    }
}

/// Resolve a natural phrase to a TV action. `device` is the TV/remote name/id;
/// `query` is "open \<app\>" or "play/watch \<title\>" (or bare). An app intent
/// (or a bare/“play” phrase that names a known app) launches that app; a title
/// we can't map to an app opens the device's **last-used app** as the best guess
/// for where it lives — the preferred-app fast path. (True in-app/title search is
/// the next step; for now the caller can fall back to HA Assist for the title.)
/// Split a `"TITLE on APP"` phrase when the suffix names a catalog app
/// ("bob's burgers on hulu" → ("bob's burgers", Hulu)). Splits at the LAST
/// ` on ` whose suffix matches, so a title containing "on" survives
/// ("carry on on hulu"). `None` when no suffix names an app.
fn split_title_on_app<'a>(q: &str, apps: &'a [RemoteApp]) -> Option<(String, &'a RemoteApp)> {
    let lower = q.to_ascii_lowercase();
    let mut at = lower.len();
    while let Some(i) = lower[..at].rfind(" on ") {
        let (head, tail) = (q[..i].trim(), q[i + 4..].trim());
        if !head.is_empty()
            && let Some(app) = match_app(tail, apps)
        {
            return Some((head.to_string(), app));
        }
        at = i;
    }
    None
}

/// Open `app` directly onto its search results for `title` when a deep-link
/// template exists; else just launch the app. The native "(app, title)" path.
async fn launch_for_title(
    state: &AppState,
    remote_id: &str,
    app: &RemoteApp,
    title: &str,
) -> ResolveOutcome {
    if let Some(link) = crate::models::remote::app_search_deep_link(&app.package, title) {
        let cmd = RemoteCommand::LaunchApp { activity: link };
        return match apply_remote_command(state, remote_id, &cmd).await {
            RemoteOutcome::Ok => ResolveOutcome::SearchedIn(title.to_string(), app.name.clone()),
            _ => ResolveOutcome::Failed,
        };
    }
    launch(state, remote_id, app).await
}

pub(crate) async fn resolve_and_play(
    state: &AppState,
    device: &str,
    query: &str,
) -> ResolveOutcome {
    let Some(remote_id) = resolve_remote(state, device).await else {
        return ResolveOutcome::NoRemote;
    };
    let q = query.trim();
    let lower = q.to_ascii_lowercase();
    let apps = list_remote_apps(state, &remote_id).await;

    // Explicit app intent: "open/launch/start <app>".
    if let Some(name) = ["open ", "launch ", "start "]
        .iter()
        .find_map(|v| lower.strip_prefix(v).map(|_| q[v.len()..].trim()))
    {
        return match match_app(name, &apps) {
            Some(app) => launch(state, &remote_id, app).await,
            None => ResolveOutcome::NoMatch,
        };
    }

    // Title/bare intent: strip a leading play-verb. If the remainder names a
    // known app, open it ("play netflix").
    let title = ["play ", "watch ", "put on "]
        .iter()
        .find_map(|v| lower.strip_prefix(v).map(|_| q[v.len()..].trim()))
        .unwrap_or(q);
    if let Some(app) = match_app(title, &apps) {
        return launch(state, &remote_id, app).await;
    }
    // "TITLE on APP" pins the target: open that app straight onto its search
    // for the title (deep link) — deterministic, no assistant in the loop.
    if let Some((t, app)) = split_title_on_app(title, &apps) {
        return launch_for_title(state, &remote_id, app, &t).await;
    }
    // A real title: try to resolve it to actual content and play it on the TV
    // (its paired media device's search). Only if that finds nothing do we fall
    // back to opening the last-used app as the best guess for where it lives.
    if !title.is_empty()
        && let Some(media_id) = paired_media_id(state, &remote_id).await
        && crate::api::media::search_and_play_on_device(state, &media_id, title).await
    {
        return ResolveOutcome::Played(title.to_string());
    }
    match preferred_app(&apps) {
        // Best guess = the last-used app; when we know its search deep link,
        // land on the title's results there rather than just its home screen.
        Some(app) => match launch_for_title(state, &remote_id, app, title).await {
            ResolveOutcome::Launched(n) => ResolveOutcome::OpenedPreferred(n),
            other => other,
        },
        None => ResolveOutcome::NoMatch,
    }
}

/// Request body for the `play-on` REST surface (session + `v1`): a TV/remote name
/// or id and a natural phrase ("play Bob's Burgers", "open Netflix").
#[derive(Deserialize)]
pub(crate) struct PlayOnInput {
    pub device: String,
    pub query: String,
}

/// Reply body for `play-on`: whether an action was taken and a spoken-style line.
#[derive(Serialize)]
pub(crate) struct PlayOnResult {
    pub ok: bool,
    pub said: String,
}

/// Run the TV content resolver and shape it as an HTTP response — the shared body
/// behind the session and `v1` `play-on` routes (MCP/voice phrase the same
/// [`ResolveOutcome`] their own way). `404` when no TV/remote matched the name,
/// `502` when the device couldn't be reached, else `200 {ok, said}`.
pub(crate) async fn play_on_response(
    state: &AppState,
    device: &str,
    query: &str,
) -> impl IntoResponse {
    let (status, ok, said) = match resolve_and_play(state, device, query).await {
        ResolveOutcome::Played(title) => (StatusCode::OK, true, format!("Playing {title}.")),
        ResolveOutcome::SearchedIn(title, app) => (
            StatusCode::OK,
            true,
            format!("Opened {app} search for {title}."),
        ),
        ResolveOutcome::Launched(name) => (StatusCode::OK, true, format!("Opened {name}.")),
        ResolveOutcome::OpenedPreferred(name) => (
            StatusCode::OK,
            true,
            format!("Opened {name} (the last app used)."),
        ),
        ResolveOutcome::NoMatch => (
            StatusCode::OK,
            false,
            "couldn't find a matching app or title on that device".to_string(),
        ),
        ResolveOutcome::NoRemote => (
            StatusCode::NOT_FOUND,
            false,
            "no TV or remote found by that name".to_string(),
        ),
        ResolveOutcome::Failed => (
            StatusCode::BAD_GATEWAY,
            false,
            "the device could not be reached".to_string(),
        ),
    };
    (status, Json(PlayOnResult { ok, said }))
}

/// The media device (TV) paired to a remote, if any — the surface the content
/// resolver searches when turning a title into playable content.
async fn paired_media_id(state: &AppState, remote_id: &str) -> Option<String> {
    // A media device (TV preferred) sharing the remote's composite group.
    sqlx::query_scalar::<_, String>(
        "SELECT m.id FROM media_devices m
          WHERE m.group_id IS NOT NULL
            AND m.group_id = (SELECT group_id FROM remote_devices WHERE id = ?)
            AND m.shadowed_by IS NULL
          ORDER BY (m.kind = 'tv') DESC LIMIT 1",
    )
    .bind(remote_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::{
        RemoteApp, RemoteCommandInfo, app_display_name, is_launchable_app, match_app, overlay_pins,
        preferred_app, split_title_on_app,
    };
    use crate::models::remote::prettify_package;

    #[test]
    fn overlay_pins_marks_favourites_and_keeps_provider_order() {
        let cmds = vec![
            RemoteCommandInfo {
                name: "Home".into(),
                token: "AAAA".into(),
                ..Default::default()
            },
            RemoteCommandInfo {
                name: "Input".into(),
                token: "BBBB".into(),
                ..Default::default()
            },
            RemoteCommandInfo {
                name: "Netflix".into(),
                token: "CCCC".into(),
                ..Default::default()
            },
        ];
        let pinned = std::collections::HashSet::from(["CCCC".to_string(), "AAAA".to_string()]);
        let out = overlay_pins(cmds, &pinned);
        // Order is the provider's, untouched; only the pinned flag is overlaid.
        assert_eq!(
            out.iter().map(|c| c.token.as_str()).collect::<Vec<_>>(),
            ["AAAA", "BBBB", "CCCC"]
        );
        assert!(out[0].pinned); // AAAA
        assert!(!out[1].pinned); // BBBB
        assert!(out[2].pinned); // CCCC
    }

    #[test]
    fn overlay_pins_with_no_favourites_leaves_all_unpinned() {
        let cmds = vec![RemoteCommandInfo {
            name: "Home".into(),
            token: "AAAA".into(),
            ..Default::default()
        }];
        let out = overlay_pins(cmds, &std::collections::HashSet::new());
        assert!(out.iter().all(|c| !c.pinned));
    }

    fn app(package: &str, last_seen: Option<&str>) -> RemoteApp {
        RemoteApp {
            name: app_display_name(package),
            package: package.to_string(),
            pinned: false,
            last_seen: last_seen.map(str::to_string),
            activity: None,
        }
    }

    #[test]
    fn split_title_on_app_pins_the_target() {
        let apps = vec![app("com.hulu.plus", None), app("com.netflix.ninja", None)];
        // Basic: "TITLE on APP".
        let (t, a) = split_title_on_app("bob's burgers on hulu", &apps).unwrap();
        assert_eq!(t, "bob's burgers");
        assert_eq!(a.package, "com.hulu.plus");
        // A title containing "on" splits at the LAST matching " on ".
        let (t, a) = split_title_on_app("carry on on netflix", &apps).unwrap();
        assert_eq!(t, "carry on");
        assert_eq!(a.package, "com.netflix.ninja");
        // Suffix that names no app → not a pinned target.
        assert!(split_title_on_app("planet earth on bluray", &apps).is_none());
        // No bare "on APP" with an empty title.
        assert!(split_title_on_app(" on hulu", &apps).is_none());
    }

    #[test]
    fn match_app_resolves_by_friendly_name_and_skips_screensaver() {
        let apps = vec![
            app("com.hulu.plus", Some("2026-06-19 10:00:00")),
            app(
                "com.google.android.apps.tv.dreamx",
                Some("2026-06-19 11:00:00"),
            ),
            app("com.netflix.ninja", None),
        ];
        assert_eq!(match_app("hulu", &apps).unwrap().package, "com.hulu.plus");
        assert_eq!(
            match_app("netflix", &apps).unwrap().package,
            "com.netflix.ninja"
        );
        // The screensaver is never a launch target, even by name.
        assert!(match_app("screensaver", &apps).is_none());
        assert!(match_app("nope", &apps).is_none());
        assert!(match_app("", &apps).is_none());
    }

    #[test]
    fn preferred_app_is_the_most_recently_used_non_system_app() {
        let apps = vec![
            app("com.hulu.plus", Some("2026-06-19 10:00:00")),
            // Newer, but it's the screensaver — must be excluded.
            app(
                "com.google.android.apps.tv.dreamx",
                Some("2026-06-19 23:59:00"),
            ),
            app("com.netflix.ninja", Some("2026-06-18 09:00:00")),
        ];
        assert_eq!(preferred_app(&apps).unwrap().package, "com.hulu.plus");
        // Nothing seen → no preference.
        assert!(preferred_app(&[app("com.hulu.plus", None)]).is_none());
    }

    #[test]
    fn is_launchable_app_excludes_launcher_and_settings() {
        assert!(!is_launchable_app(&app(
            "com.google.android.tvlauncher",
            None
        )));
        assert!(!is_launchable_app(&app("com.android.tv.settings", None)));
        assert!(is_launchable_app(&app("com.hulu.plus", None)));
    }

    #[test]
    fn app_display_name_matches_brand_across_package_variants() {
        // The reported bug: a Hulu package variant fell through to the raw id.
        assert_eq!(app_display_name("com.hulu.livingroomplus"), "Hulu");
        assert_eq!(app_display_name("com.hulu.plus"), "Hulu");
        assert_eq!(app_display_name("com.netflix.ninja"), "Netflix");
        assert_eq!(app_display_name("com.google.android.youtube.tv"), "YouTube");
        assert_eq!(
            app_display_name("com.google.android.youtube.tvkids"),
            "YouTube Kids"
        );
        assert_eq!(
            app_display_name("com.amazon.amazonvideo.livingroom"),
            "Prime Video"
        );
        assert_eq!(app_display_name("com.disney.disneyplus"), "Disney+");
    }

    #[test]
    fn app_display_name_prettifies_unknown_packages() {
        // Unknown brand → capitalized vendor segment, not the raw dotted id.
        assert_eq!(app_display_name("com.foobar.tv"), "Foobar");
        assert_eq!(prettify_package("com.acmecorp.player"), "Acmecorp");
        // Non-package strings (deep links / plain text) pass through untouched.
        assert_eq!(
            prettify_package("https://youtube.com/watch"),
            "https://youtube.com/watch"
        );
        assert_eq!(prettify_package("HDMI 1"), "HDMI 1");
    }
}
