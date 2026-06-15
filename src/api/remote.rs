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
//! A remote is **paired** to its TV's audio device (`paired_audio_id`) when a
//! `media_player` shares its hardware id — set during discovery in
//! [`crate::api::dedup`]-adjacent pairing ([`reconcile_remote_pairings`]).

use crate::AppState;
use crate::api::auth::require_session;
use crate::models::remote::{RemoteCommand, RemoteState};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post, put},
};
use serde::Serialize;
use sqlx::Row;
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/devices", get(list_devices_handler))
        .route("/devices/{id}", get(get_device_handler))
        .route("/devices/{id}/command", post(command_handler))
        .route("/devices/{id}/apps", get(list_apps_handler))
        .route("/devices/{id}/apps/pin", put(pin_app_handler))
        .route("/devices/{id}/enabled", put(set_enabled_handler))
        .route("/devices/{id}/glyph", put(set_glyph_handler))
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
    /// The paired TV audio device id, if this remote controls a known TV.
    pub paired_audio_id: Option<String>,
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
        paired_audio_id: r.get("paired_audio_id"),
    }
}

const SELECT_REMOTE: &str = "SELECT id, provider_id, device_id, name, last_state, last_seen, \
     enabled, glyph, hw_id, paired_audio_id FROM remote_devices";

// ── Provider build / discovery ───────────────────────────────────────────────

pub(crate) fn build_remote_provider(
    state: &AppState,
    provider_type: &str,
    credentials_enc: &str,
) -> anyhow::Result<Box<dyn crate::providers::RemoteProvider>> {
    let creds_json = state.decrypt_credentials(credentials_enc)?;
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

/// Pair each remote to a TV audio device that shares its `hw_id` (same physical
/// box — an Android TV's `remote.*` and `media_player.*` share one HA device).
/// Idempotent; run after discovery. A remote with no hw_id match is left
/// unpaired. Prefers an audio device of TV kind when several share a hw_id.
pub(crate) async fn reconcile_remote_pairings(state: &AppState) {
    let _ = sqlx::query(
        "UPDATE remote_devices
            SET paired_audio_id = (
                SELECT a.id FROM audio_devices a
                 WHERE a.hw_id = remote_devices.hw_id
                   AND a.shadowed_by IS NULL
                 ORDER BY (a.kind = 'tv') DESC
                 LIMIT 1)
          WHERE hw_id IS NOT NULL",
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
}

/// Friendly name for a known Play Store package; falls back to the package id.
/// Small, curated list of the common TV apps — extend as needed.
fn app_display_name(package: &str) -> String {
    let name = match package {
        "com.netflix.ninja" => "Netflix",
        "com.google.android.youtube.tv" | "com.google.android.youtube.tvkids" => "YouTube",
        "com.amazon.amazonvideo.livingroom" => "Prime Video",
        "com.disney.disneyplus" => "Disney+",
        "com.hulu.plus" => "Hulu",
        "com.hbo.hbonow" | "com.wbd.stream" => "Max",
        "com.spotify.tv.android" => "Spotify",
        "com.plexapp.android" => "Plex",
        "org.xbmc.kodi" => "Kodi",
        "tv.twitch.android.app" => "Twitch",
        "com.apple.atve.androidtv.appletv" => "Apple TV",
        "com.google.android.apps.tv.dreamx" => "Screensaver",
        _ => return package.to_string(),
    };
    name.to_string()
}

/// `true` if `activity` looks like a launchable package id (not a deep-link URL).
fn looks_like_package(activity: &str) -> bool {
    !activity.contains("://") && activity.contains('.') && !activity.contains(' ')
}

/// Record that `package` was seen foreground on `remote_id` (a "recent"). Upserts
/// without disturbing an existing pin. No-op for non-package activities.
pub(crate) async fn record_app_seen(state: &AppState, remote_id: &str, package: &str) {
    if !looks_like_package(package) {
        return;
    }
    let _ = sqlx::query(
        "INSERT INTO remote_apps (remote_id, package, name, pinned, last_seen)
         VALUES (?, ?, ?, 0, datetime('now'))
         ON CONFLICT (remote_id, package)
         DO UPDATE SET last_seen = datetime('now'), name = excluded.name",
    )
    .bind(remote_id)
    .bind(package)
    .bind(app_display_name(package))
    .execute(&state.db)
    .await;
}

/// Launchable apps for a remote: pinned first, then recents by most-recent.
pub(crate) async fn list_remote_apps(state: &AppState, remote_id: &str) -> Vec<RemoteApp> {
    sqlx::query(
        "SELECT package, name, pinned, last_seen FROM remote_apps
         WHERE remote_id = ? ORDER BY pinned DESC, last_seen DESC",
    )
    .bind(remote_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .iter()
    .map(|r| RemoteApp {
        package: r.get("package"),
        name: r.get("name"),
        pinned: r.get::<i64, _>("pinned") != 0,
        last_seen: r.get("last_seen"),
    })
    .collect()
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
        "SELECT r.device_id, p.provider_type, p.credentials
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
    let result = match cmd {
        RemoteCommand::Key { key, hold_secs } => {
            provider.send_key(&device_id, *key, *hold_secs).await
        }
        RemoteCommand::Text { text } => provider.send_text(&device_id, text).await,
        RemoteCommand::LaunchApp { activity } => provider.launch_app(&device_id, activity).await,
        RemoteCommand::Power { on } => provider.set_power(&device_id, *on).await,
    };
    match result {
        Ok(()) => {
            // A launched app is, by definition, a "recent" — record it.
            if let RemoteCommand::LaunchApp { activity } = cmd {
                record_app_seen(state, id, activity).await;
            }
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

// ── Handlers (session-authenticated) ─────────────────────────────────────────

async fn list_devices_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(list_remotes(&state).await).into_response()
}

async fn get_device_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match read_remote_state(&state, &id).await {
        Some(s) => Json(s).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
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
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(cmd): Json<RemoteCommand>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    remote_status(apply_remote_command(&state, &id, &cmd).await).into_response()
}

async fn list_apps_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
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
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<PinAppRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    set_app_pin(&state, &id, &req.package, req.pinned)
        .await
        .into_response()
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
    crate::api::set_device_enabled(&state, "remote_devices", &id, req.enabled)
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
    crate::api::set_device_glyph(&state, "remote_devices", &id, req.glyph)
        .await
        .into_response()
}
