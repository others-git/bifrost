//! Kiosk controller — register, observe, and manage the wall-tablet companion
//! apps that "check in" to Bifrost.
//!
//! A kiosk is identified by the `bfr_` API key it carries (minted via QR
//! enrollment). It **checks in** on a heartbeat ([`checkin`], key-authenticated)
//! reporting its label / app version / screen state; the server records
//! `last_seen` and returns any **queued command** (`sleep` | `wake` | `lock` |
//! `update`), which the app performs and which is then consumed. Commands are
//! also pushed instantly over the kiosk's live SSE channel ([`stream`]); the
//! queued copy is the offline fallback.
//!
//! Management endpoints ([`list`], [`command`], [`deauth`], [`forget`], and the
//! per-kiosk assignment/config setters) are **session-authenticated** — driven
//! from a mobile/desktop browser, not the kiosk itself. Command semantics:
//! - `sleep` / `wake` — turn the display off/on.
//! - `lock` — force sign-out of the Bifrost WebView session (re-enter password).
//! - `update` — pull the hub-cached APK and self-install ([`crate::api::kiosk_update`]).
//! - **de-auth** — revoke the kiosk's API key (a separate endpoint, not a queued
//!   command): the app's next call 401s and it re-enrolls via a fresh QR scan.
//!
//! Display power saving is server-driven: [`run_scheduler`] issues the same
//! `sleep`/`wake` commands from each kiosk's quiet-hours schedule
//! (`PUT …/schedule`) and its assigned Room's presence (`PUT …/presence`,
//! occupancy from `rooms::room_occupancy`).

use crate::AppState;
use crate::api::apikeys::require_api_key;
use crate::api::auth::Session;
use crate::api::rooms::ControlTarget;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    response::sse::{Event, KeepAlive, Sse},
    routing::{delete, get, post},
};
use futures_util::stream::StreamExt;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

/// A controller command pushed to a kiosk over its live SSE stream ([`stream`]).
/// Broadcast to every stream subscriber; each connection filters for its own
/// [`KioskCommand::kiosk_id`]. Clone-able so the broadcast channel can fan it out.
#[derive(Clone, Debug)]
pub struct KioskCommand {
    pub kiosk_id: String,
    pub command: String,
}

/// A kiosk is "online" if it checked in within this window.
const ONLINE_WINDOW_SECS: i64 = 90;

/// Commands the app performs on check-in. (`deauth` is not here — it's an
/// immediate key revocation, surfaced to the app as a 401, not a queued action.)
/// `update` tells the kiosk to pull the cached APK from the hub and self-install
/// (see [`crate::api::kiosk_update`]).
const VALID_COMMANDS: [&str; 4] = ["sleep", "wake", "lock", "update"];

pub fn router() -> Router<Arc<AppState>> {
    use crate::api::kiosk_update as upd;
    Router::new()
        .route("/checkin", post(checkin))
        .route("/self", get(self_info))
        .route("/self/viewport", axum::routing::put(set_self_viewport))
        .route("/self/noise", post(report_noise))
        .route("/stream", get(stream))
        .route("/", get(list))
        // OTA relay: session triggers/inspects the cache; key-auth endpoints feed
        // the kiosk the manifest + APK over the LAN.
        .route("/update", get(upd::update_status).post(upd::refresh_update))
        .route("/update/config", get(upd::get_config).put(upd::put_config))
        .route("/update/manifest", get(upd::update_manifest))
        .route("/update/apk", get(upd::serve_apk))
        .route("/{id}/command", post(command))
        .route("/{id}/room", axum::routing::put(set_room))
        .route("/{id}/board", axum::routing::put(set_board))
        .route("/{id}/schedule", axum::routing::put(set_schedule))
        .route("/{id}/presence", axum::routing::put(set_presence))
        .route("/{id}/plan", axum::routing::put(set_plan))
        .route("/{id}/mic", axum::routing::put(set_mic))
        .route(
            "/{id}/aware-override",
            axum::routing::put(set_aware_override),
        )
        .route("/{id}/deauth", post(deauth))
        .route("/{id}", delete(forget))
}

#[derive(Deserialize, Default)]
struct CheckinRequest {
    /// Human label for the kiosk (e.g. "Bedroom tablet"). Falls back to the
    /// API key's name on first check-in.
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    app_version: Option<String>,
    #[serde(default)]
    screen_on: Option<bool>,
    // Battery / power telemetry (M29+ apps; absent on older apps / desktop).
    #[serde(default)]
    battery_level: Option<i64>, // 0-100 (%)
    #[serde(default)]
    battery_charging: Option<bool>,
    #[serde(default)]
    battery_voltage_mv: Option<i64>,
    #[serde(default)]
    battery_current_ua: Option<i64>, // signed micro-amps (+ = into battery)
    #[serde(default)]
    battery_temp_dc: Option<i64>, // deci-celsius
    #[serde(default)]
    power_source: Option<String>, // ac | usb | wireless | none
}

#[derive(Serialize)]
struct CheckinResponse {
    /// The command to perform, if any was queued — consumed by this check-in.
    command: Option<String>,
    /// The kiosk's assigned Room **name**, if any — the app adopts it as the
    /// voice context room (so "turn on the lights" resolves to that room).
    room: Option<String>,
    /// The board this kiosk should auto-launch full-screen, if configured. The
    /// web client also reads this via `GET /self`; surfaced here for the app.
    default_board_id: Option<String>,
    /// Microphone presence config — the app starts/stops its on-device sound
    /// LEVEL monitor from this (no audio ever leaves the tablet).
    mic_presence: bool,
    /// low | medium | high (absent = medium).
    mic_sensitivity: Option<String>,
}

/// `POST /api/kiosks/checkin` (API-key auth) — the kiosk heartbeat. Upserts the
/// kiosk keyed by its API key, refreshes `last_seen`/state, and returns + clears
/// any queued command.
async fn checkin(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<CheckinRequest>>,
) -> impl IntoResponse {
    let Some(key_id) = require_api_key(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let req = body.map(|Json(b)| b).unwrap_or_default();

    // Fall back to the API key's name when the app doesn't send a label.
    let key_name: Option<String> = sqlx::query_scalar("SELECT name FROM api_keys WHERE id = ?")
        .bind(&key_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    let name = req
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or(key_name)
        .unwrap_or_else(|| "Kiosk".to_string());

    let row = sqlx::query(
        "INSERT INTO kiosks (id, api_key_id, name, app_version, screen_on,
                             battery_level, battery_charging, battery_voltage_mv,
                             battery_current_ua, battery_temp_dc, power_source, last_seen)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
         ON CONFLICT(api_key_id) DO UPDATE SET
             name               = excluded.name,
             app_version        = excluded.app_version,
             screen_on          = excluded.screen_on,
             battery_level      = excluded.battery_level,
             battery_charging   = excluded.battery_charging,
             battery_voltage_mv = excluded.battery_voltage_mv,
             battery_current_ua = excluded.battery_current_ua,
             battery_temp_dc    = excluded.battery_temp_dc,
             power_source       = excluded.power_source,
             last_seen          = datetime('now')
         RETURNING id, pending_command",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&key_id)
    .bind(&name)
    .bind(&req.app_version)
    .bind(req.screen_on.map(i64::from))
    .bind(req.battery_level)
    .bind(req.battery_charging.map(i64::from))
    .bind(req.battery_voltage_mv)
    .bind(req.battery_current_ua)
    .bind(req.battery_temp_dc)
    .bind(&req.power_source)
    .fetch_one(&state.db)
    .await;

    let row = match row {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("kiosk check-in db error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let kiosk_id: String = row.get("id");
    let command: Option<String> = row.get("pending_command");

    // Consume the command so it's delivered at most once.
    if command.is_some() {
        let _ = sqlx::query("UPDATE kiosks SET pending_command = NULL WHERE id = ?")
            .bind(&kiosk_id)
            .execute(&state.db)
            .await;
    }

    // The assigned room's name (if any) — the app adopts it as voice context.
    let room: Option<String> = sqlx::query_scalar(
        "SELECT r.name FROM kiosks k JOIN rooms r ON k.room_id = r.id WHERE k.id = ?",
    )
    .bind(&kiosk_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let cfg = sqlx::query(
        "SELECT default_board_id, mic_presence, mic_sensitivity FROM kiosks WHERE id = ?",
    )
    .bind(&kiosk_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    Json(CheckinResponse {
        command,
        room,
        default_board_id: cfg.as_ref().and_then(|r| r.get("default_board_id")),
        mic_presence: cfg
            .as_ref()
            .map(|r| r.get::<i64, _>("mic_presence") != 0)
            .unwrap_or(false),
        mic_sensitivity: cfg.as_ref().and_then(|r| r.get("mic_sensitivity")),
    })
    .into_response()
}

#[derive(Serialize)]
struct SelfResponse {
    id: String,
    name: String,
    /// The board to auto-launch full-screen on this kiosk, if configured.
    default_board_id: Option<String>,
}

/// `GET /api/kiosks/self` — the kiosk asks *which kiosk am I and what should I
/// show*. Resolved from the `bfr_key` cookie the WebView carries (not a session,
/// which isn't tied to a kiosk), so the web client can auto-launch its assigned
/// board. 401 without a valid kiosk key; 404 if the key isn't a registered kiosk.
async fn self_info(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    let Some(key) = crate::api::auth::kiosk_cookie_key(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(key_id) = crate::api::apikeys::validate_key(&state, &key).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let row = sqlx::query("SELECT id, name, default_board_id FROM kiosks WHERE api_key_id = ?")
        .bind(&key_id)
        .fetch_optional(&state.db)
        .await;
    match row {
        Ok(Some(r)) => Json(SelfResponse {
            id: r.get("id"),
            name: r.get("name"),
            default_board_id: r.get("default_board_id"),
        })
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error resolving kiosk self: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
struct ViewportRequest {
    w: i64,
    h: i64,
}

/// `PUT /api/kiosks/self/viewport` — the kiosk-served web client reports its
/// own CSS viewport (`window.innerWidth × innerHeight`). Same `bfr_key`-cookie
/// auth as `GET /self`. Feeds the Boards preview device list: the exact pixel
/// size a board renders at on that wall tablet, measured, not guessed.
async fn set_self_viewport(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ViewportRequest>,
) -> impl IntoResponse {
    let Some(key) = crate::api::auth::kiosk_cookie_key(&headers) else {
        return StatusCode::UNAUTHORIZED;
    };
    let Some(key_id) = crate::api::apikeys::validate_key(&state, &key).await else {
        return StatusCode::UNAUTHORIZED;
    };
    // Sanity clamp: a viewport outside these bounds is a client bug, not data.
    if !(100..=20_000).contains(&req.w) || !(100..=20_000).contains(&req.h) {
        return StatusCode::UNPROCESSABLE_ENTITY;
    }
    match sqlx::query("UPDATE kiosks SET viewport_w = ?, viewport_h = ? WHERE api_key_id = ?")
        .bind(req.w)
        .bind(req.h)
        .bind(&key_id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT,
        Ok(_) => StatusCode::NOT_FOUND, // valid key but not a registered kiosk
        Err(e) => {
            tracing::error!("db error setting kiosk viewport: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

#[derive(Serialize)]
struct KioskRow {
    id: String,
    name: String,
    app_version: Option<String>,
    screen_on: Option<bool>,
    last_seen: Option<String>,
    /// Checked in within [`ONLINE_WINDOW_SECS`].
    online: bool,
    /// A command is queued and not yet picked up.
    pending_command: Option<String>,
    /// `false` once de-authed (its key was revoked) — it must re-enroll.
    authorized: bool,
    /// Assigned Room id (its voice context), or null. Set via `PUT …/room`.
    room_id: Option<String>,
    /// Board to auto-launch full-screen, or null. Set via `PUT …/board`.
    default_board_id: Option<String>,
    /// Scheduled quiet hours (display power saving). When `schedule_enabled`, the
    /// scheduler sleeps the display at `sleep_at` and wakes it at `wake_at`
    /// (server-local "HH:MM"). Set via `PUT …/schedule`.
    schedule_enabled: bool,
    sleep_at: Option<String>,
    wake_at: Option<String>,
    /// Presence-driven display (power saving): when enabled, the scheduler blanks
    /// the display while the kiosk's assigned Room is unoccupied and wakes it on
    /// motion. `presence_timeout_secs` is the no-motion grace before sleeping.
    presence_enabled: bool,
    presence_timeout_secs: i64,
    // Battery / power telemetry from the latest check-in (null on older apps).
    battery_level: Option<i64>,
    battery_charging: Option<bool>,
    battery_voltage_mv: Option<i64>,
    battery_current_ua: Option<i64>,
    battery_temp_dc: Option<i64>,
    power_source: Option<String>,
    /// The kiosk's CSS viewport (self-reported via `PUT /self/viewport`) —
    /// drives the Boards preview device list. Null until it first reports.
    viewport_w: Option<i64>,
    viewport_h: Option<i64>,
    /// Per-hour display plan (24 chars of W/S/A, mig 0059) — see `PUT …/plan`.
    /// Null = no plan painted; the legacy sleep window + presence flag govern.
    hour_modes: Option<String>,
    /// Microphone presence (mig 0061): the kiosk's mic doubles as a room
    /// occupancy sensor (level-only, computed on-device).
    mic_presence: bool,
    mic_sensitivity: Option<String>,
    /// Last reported sound level (dBFS) — telemetry for the Clients view.
    mic_level: Option<f64>,
    /// Devices that keep an Aware hour awake regardless of presence (mig
    /// 0062) — see `PUT …/aware-override`.
    aware_override_targets: Vec<ControlTarget>,
}

/// `GET /api/kiosks` (session) — the clients view: every registered kiosk with
/// its check-in status. Session-only, so it isn't reachable with a kiosk key.
async fn list(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    let rows = sqlx::query(&format!(
        "SELECT id, name, app_version, screen_on, last_seen, pending_command, room_id,
                default_board_id, schedule_enabled, sleep_at, wake_at,
                presence_enabled, presence_timeout_secs,
                battery_level, battery_charging, battery_voltage_mv, battery_current_ua,
                battery_temp_dc, power_source, viewport_w, viewport_h, hour_modes,
                mic_presence, mic_sensitivity, mic_level, aware_override_targets,
                api_key_id IS NOT NULL AS authorized,
                (last_seen > datetime('now', '-{ONLINE_WINDOW_SECS} seconds')) AS online
         FROM kiosks ORDER BY name"
    ))
    .fetch_all(&state.db)
    .await;

    match rows {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|r| KioskRow {
                    id: r.get("id"),
                    name: r.get("name"),
                    app_version: r.get("app_version"),
                    screen_on: r.get::<Option<i64>, _>("screen_on").map(|v| v != 0),
                    last_seen: r.get("last_seen"),
                    online: r.get::<Option<i64>, _>("online").unwrap_or(0) != 0,
                    pending_command: r.get("pending_command"),
                    authorized: r.get::<i64, _>("authorized") != 0,
                    room_id: r.get("room_id"),
                    default_board_id: r.get("default_board_id"),
                    schedule_enabled: r.get::<i64, _>("schedule_enabled") != 0,
                    sleep_at: r.get("sleep_at"),
                    wake_at: r.get("wake_at"),
                    presence_enabled: r.get::<i64, _>("presence_enabled") != 0,
                    presence_timeout_secs: r.get("presence_timeout_secs"),
                    battery_level: r.get("battery_level"),
                    battery_charging: r.get::<Option<i64>, _>("battery_charging").map(|v| v != 0),
                    battery_voltage_mv: r.get("battery_voltage_mv"),
                    battery_current_ua: r.get("battery_current_ua"),
                    battery_temp_dc: r.get("battery_temp_dc"),
                    power_source: r.get("power_source"),
                    viewport_w: r.get("viewport_w"),
                    viewport_h: r.get("viewport_h"),
                    hour_modes: r.get("hour_modes"),
                    mic_presence: r.get::<i64, _>("mic_presence") != 0,
                    mic_sensitivity: r.get("mic_sensitivity"),
                    mic_level: r.get("mic_level"),
                    aware_override_targets: parse_aware_targets(r.get("aware_override_targets")),
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => {
            tracing::error!("db error listing kiosks: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
struct CommandRequest {
    command: String,
}

/// `POST /api/kiosks/{id}/command` (session) — deliver a command to the kiosk.
/// It's **pushed instantly** to the kiosk's live SSE stream ([`stream`]) and
/// also stored in `pending_command` as the fallback for a kiosk that's offline
/// or mid-reconnect (consumed on its next check-in).
async fn command(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<CommandRequest>,
) -> impl IntoResponse {
    let cmd = req.command.trim();
    if !VALID_COMMANDS.contains(&cmd) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("unknown command; expected one of {VALID_COMMANDS:?}"),
        )
            .into_response();
    }
    match queue_kiosk_command(&state, &id, cmd).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error queuing kiosk command: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Queue a command for a kiosk and push it to any live SSE stream now. The stored
/// `pending_command` is the fallback for a kiosk that's offline / mid-reconnect
/// (consumed on its next check-in). Shared by the session route ([`command`]) and
/// the [`run_scheduler`] background loop, so both surfaces deliver identically.
/// `Ok(false)` = no kiosk row matched `id`.
async fn queue_kiosk_command(state: &AppState, id: &str, cmd: &str) -> Result<bool, sqlx::Error> {
    let affected = sqlx::query("UPDATE kiosks SET pending_command = ? WHERE id = ?")
        .bind(cmd)
        .bind(id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if affected == 0 {
        return Ok(false);
    }
    // Push to any live stream now; the stored row above covers offline kiosks.
    let _ = state.kiosk_commands.send(KioskCommand {
        kiosk_id: id.to_string(),
        command: cmd.to_string(),
    });
    Ok(true)
}

/// `GET /api/kiosks/stream` (API-key auth) — the kiosk's live command channel.
/// Opened by the kiosk after it checks in; controller commands ([`command`]) are
/// pushed here instantly as SSE `command` events instead of waiting for the next
/// poll. Requires the kiosk to be registered (so we can resolve its id); if not,
/// 404 and the app retries after its next heartbeat.
async fn stream(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<axum::response::Response, StatusCode> {
    let Some(key_id) = require_api_key(&state, &headers).await else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let kiosk_id: Option<String> = sqlx::query_scalar("SELECT id FROM kiosks WHERE api_key_id = ?")
        .bind(&key_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    let Some(kiosk_id) = kiosk_id else {
        return Err(StatusCode::NOT_FOUND);
    };

    let rx = state.kiosk_commands.subscribe();
    let events = BroadcastStream::new(rx)
        .filter_map(|r| std::future::ready(r.ok()))
        .filter_map(move |cmd| {
            std::future::ready((cmd.kiosk_id == kiosk_id).then(|| {
                Ok::<Event, Infallible>(Event::default().event("command").data(cmd.command))
            }))
        });

    let sse = Sse::new(events).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    );
    Ok(crate::api::sse_unbuffered(sse))
}

#[derive(Deserialize)]
struct SetRoomRequest {
    /// Target Room id, or null to clear the assignment.
    room_id: Option<String>,
}

/// `PUT /api/kiosks/{id}/room` (session) — assign the kiosk to a Bifrost Room
/// (its voice context), or clear it with a null `room_id`. The kiosk adopts the
/// room on its next check-in.
async fn set_room(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetRoomRequest>,
) -> impl IntoResponse {
    match sqlx::query("UPDATE kiosks SET room_id = ? WHERE id = ?")
        .bind(&req.room_id)
        .bind(&id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            // The kiosk's mic sensor (when present) follows its room.
            if let Ok(Some(sensor_id)) = sqlx::query_scalar::<_, String>(
                "SELECT id FROM sensor_devices WHERE provider_id = ? AND device_id = ?",
            )
            .bind(KIOSK_SENSOR_PROVIDER)
            .bind(&id)
            .fetch_optional(&state.db)
            .await
            {
                sync_mic_sensor_room(&state.db, &sensor_id, req.room_id.as_deref()).await;
            }
            state.occupancy_seen.poke();
            StatusCode::NO_CONTENT
        }
        Ok(_) => StatusCode::NOT_FOUND,
        Err(e) => {
            tracing::error!("db error setting kiosk room: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

#[derive(Deserialize)]
struct SetBoardRequest {
    /// Board id to auto-launch full-screen, or null to clear it.
    board_id: Option<String>,
}

/// `PUT /api/kiosks/{id}/board` (session) — set (or clear) the board this kiosk
/// auto-launches full-screen on load. Configured from a main (non-kiosk) client;
/// the kiosk picks it up on its next load via `GET /self`.
async fn set_board(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetBoardRequest>,
) -> impl IntoResponse {
    match sqlx::query("UPDATE kiosks SET default_board_id = ? WHERE id = ?")
        .bind(&req.board_id)
        .bind(&id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT,
        Ok(_) => StatusCode::NOT_FOUND,
        Err(e) => {
            tracing::error!("db error setting kiosk board: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

#[derive(Deserialize)]
struct SetScheduleRequest {
    /// Whether the scheduler drives this kiosk's display.
    enabled: bool,
    /// Local "HH:MM" the display sleeps at (required when `enabled`).
    #[serde(default)]
    sleep_at: Option<String>,
    /// Local "HH:MM" the display wakes at (required when `enabled`).
    #[serde(default)]
    wake_at: Option<String>,
}

/// `PUT /api/kiosks/{id}/schedule` (session) — set the kiosk's scheduled quiet
/// hours (display power saving). When enabled, both times must be a distinct,
/// valid "HH:MM"; the times are normalized (zero-padded) before storage. When
/// disabled, any valid times are kept (so toggling off doesn't lose them) but the
/// scheduler ignores the kiosk. The [`run_scheduler`] loop applies the change on
/// its next tick.
async fn set_schedule(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetScheduleRequest>,
) -> impl IntoResponse {
    let sleep = req.sleep_at.as_deref().and_then(parse_hhmm);
    let wake = req.wake_at.as_deref().and_then(parse_hhmm);
    if req.enabled && !matches!((sleep, wake), (Some(s), Some(w)) if s != w) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "an enabled schedule needs distinct, valid HH:MM sleep_at and wake_at",
        )
            .into_response();
    }
    match sqlx::query(
        "UPDATE kiosks SET schedule_enabled = ?, sleep_at = ?, wake_at = ? WHERE id = ?",
    )
    .bind(i64::from(req.enabled))
    .bind(sleep.map(fmt_hhmm))
    .bind(wake.map(fmt_hhmm))
    .bind(&id)
    .execute(&state.db)
    .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            state.occupancy_seen.poke();
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error setting kiosk schedule: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
struct SetPresenceRequest {
    /// Whether presence-driven blanking governs this kiosk.
    enabled: bool,
    /// No-motion grace before sleeping, in seconds. Clamped to [30, 3600] when
    /// present; unchanged when omitted.
    #[serde(default)]
    timeout_secs: Option<i64>,
}

/// `PUT /api/kiosks/{id}/presence` (session) — enable/disable presence-driven
/// display blanking and set the no-motion timeout. Presence uses the kiosk's
/// assigned Room (set via `PUT …/room`); with no room, or a room without presence
/// sensors, the setting is stored but the scheduler simply doesn't act on it.
async fn set_presence(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetPresenceRequest>,
) -> impl IntoResponse {
    // Update the timeout only when provided (clamped to a sane band), so toggling
    // the switch never silently resets a configured grace.
    let result = match req.timeout_secs {
        Some(secs) => {
            let secs = secs.clamp(30, 3600);
            sqlx::query(
                "UPDATE kiosks SET presence_enabled = ?, presence_timeout_secs = ? WHERE id = ?",
            )
            .bind(i64::from(req.enabled))
            .bind(secs)
            .bind(&id)
            .execute(&state.db)
            .await
        }
        None => {
            sqlx::query("UPDATE kiosks SET presence_enabled = ? WHERE id = ?")
                .bind(i64::from(req.enabled))
                .bind(&id)
                .execute(&state.db)
                .await
        }
    };
    match result {
        Ok(r) if r.rows_affected() > 0 => {
            state.occupancy_seen.poke();
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error setting kiosk presence: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
struct SetPlanRequest {
    /// Master switch — reuses `schedule_enabled` (off = the display is unmanaged).
    enabled: bool,
    /// 24 chars, one per local hour: 'W' awake · 'S' asleep · 'A' aware
    /// (presence-controlled: wake on motion, off after the no-motion timer).
    hour_modes: String,
    /// The aware-hours screen-off timer (seconds); omitted = keep current.
    #[serde(default)]
    timeout_secs: Option<i64>,
}

/// `PUT /api/kiosks/{id}/plan` (session) — the per-hour display plan (mig
/// 0059): one paintable 24-hour timeline replacing the sleep-window + presence
/// toggle pair. Writing a plan supersedes the legacy fields on this kiosk (the
/// scheduler prefers `hour_modes` whenever it's set).
async fn set_plan(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetPlanRequest>,
) -> impl IntoResponse {
    if req.hour_modes.len() != 24
        || !req
            .hour_modes
            .bytes()
            .all(|b| matches!(b, b'W' | b'S' | b'A'))
    {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "hour_modes must be exactly 24 characters of W (awake), S (asleep), A (aware)",
        )
            .into_response();
    }
    let timeout = req.timeout_secs.map(|t| t.clamp(30, 3600));
    let result = match timeout {
        Some(t) => {
            sqlx::query(
                "UPDATE kiosks SET schedule_enabled = ?, hour_modes = ?, presence_timeout_secs = ? WHERE id = ?",
            )
            .bind(i64::from(req.enabled))
            .bind(&req.hour_modes)
            .bind(t)
            .bind(&id)
            .execute(&state.db)
            .await
        }
        None => {
            sqlx::query("UPDATE kiosks SET schedule_enabled = ?, hour_modes = ? WHERE id = ?")
                .bind(i64::from(req.enabled))
                .bind(&req.hour_modes)
                .bind(&id)
                .execute(&state.db)
                .await
        }
    };
    match result {
        Ok(r) if r.rows_affected() > 0 => {
            state.occupancy_seen.poke();
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error setting kiosk plan: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Parse a "HH:MM" 24-hour clock string into minutes-since-midnight (0..=1439).
/// Tolerant of surrounding whitespace; rejects anything out of range or malformed.
fn parse_hhmm(s: &str) -> Option<u16> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u16 = h.parse().ok()?;
    let m: u16 = m.parse().ok()?;
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Format minutes-since-midnight back to a zero-padded "HH:MM".
fn fmt_hhmm(min: u16) -> String {
    format!("{:02}:{:02}", min / 60, min % 60)
}

/// Whether the display *should be awake* at `now` (minutes-since-midnight) given a
/// quiet window that runs `[sleep, wake)` (asleep inside it). The window wraps past
/// midnight when `sleep > wake` (e.g. 23:00 → 07:00). Equal endpoints mean an empty
/// window (always awake) — the API rejects that for an enabled schedule, but this
/// stays defined for safety.
fn desired_awake_at(sleep: u16, wake: u16, now: u16) -> bool {
    let asleep = if sleep < wake {
        now >= sleep && now < wake
    } else if sleep > wake {
        now >= sleep || now < wake
    } else {
        false
    };
    !asleep
}

/// One hour's display mode from the painted plan (mig 0059).
#[derive(Clone, Copy, PartialEq, Debug)]
enum PlanMode {
    /// Screen forced on.
    Awake,
    /// Screen forced off — beats an occupied room, same as legacy quiet hours.
    Asleep,
    /// Presence-controlled: wake on motion, off after the no-motion timer.
    Aware,
}

/// The plan's mode for a local hour. `None` for a malformed plan or an
/// out-of-range hour — the caller falls back to the legacy policy pair.
fn plan_mode(hour_modes: &str, hour: usize) -> Option<PlanMode> {
    match hour_modes.as_bytes().get(hour)? {
        b'W' => Some(PlanMode::Awake),
        b'S' => Some(PlanMode::Asleep),
        b'A' => Some(PlanMode::Aware),
        _ => None,
    }
}

/// The desired screen state for a plan hour. Awake/Asleep are absolute; an
/// Aware hour follows the room's presence verdict and governs nothing when the
/// room has no presence input (`None` — leave the kiosk alone).
fn plan_desired(mode: PlanMode, present: Option<bool>) -> Option<bool> {
    match mode {
        PlanMode::Awake => Some(true),
        PlanMode::Asleep => Some(false),
        PlanMode::Aware => present,
    }
}

/// Combine the two display-power policies into a single desired-awake verdict.
/// `schedule_awake` is the quiet-hours verdict (`None` = no schedule); `present`
/// is the presence verdict (`None` = presence disabled or the room has no
/// presence sensors). Precedence:
/// (1) **Quiet hours is a hard sleep window** — if the schedule says asleep, stay
/// asleep even if the room is occupied (a wall tablet shouldn't light the room at
/// 3am). (2) Otherwise **presence governs** when it has a reading (motion → awake,
/// empty → asleep). (3) Otherwise fall back to the schedule verdict. `None` =
/// neither policy governs this kiosk (leave it alone).
fn combined_desired_awake(schedule_awake: Option<bool>, present: Option<bool>) -> Option<bool> {
    if schedule_awake == Some(false) {
        return Some(false); // quiet hours wins
    }
    if let Some(present) = present {
        return Some(present); // daytime: follow the room
    }
    schedule_awake
}

/// Background loop enforcing each kiosk's display-power policies (scheduled quiet
/// hours + presence-driven blanking) by emitting the existing `sleep`/`wake`
/// commands. **Edge-triggered:** it tracks the last desired state per kiosk and
/// sends only on a change, so a manual wake persists until a policy boundary. The
/// tracking map is in-memory, so a restart re-reconciles every governed kiosk on
/// the first tick. Presence keeps a per-kiosk "last seen occupied" instant to
/// apply the no-motion timeout grace.
pub async fn run_scheduler(state: Arc<AppState>) {
    use chrono::Timelike;
    use std::collections::HashMap;
    use std::time::Instant;

    let mut last_desired: HashMap<String, bool> = HashMap::new();
    let mut last_present: HashMap<String, Instant> = HashMap::new();
    let mut ticker = tokio::time::interval(Duration::from_secs(30));
    loop {
        // An occupancy flip (or a display-policy config edit) pokes an
        // immediate pass — presence wake at push latency; the 30s tick is the
        // fallback that catches hour boundaries and grace-timeout expiry.
        tokio::select! {
            _ = ticker.tick() => {}
            _ = state.occupancy_seen.poked() => {}
        }
        let now = chrono::Local::now();
        scheduler_tick(
            &state,
            &mut last_desired,
            &mut last_present,
            now.hour() as usize,
            (now.hour() * 60 + now.minute()) as u16,
        )
        .await;
    }
}

/// One scheduler pass, extracted from [`run_scheduler`] so tests can drive it
/// against the real schema. That coverage is load-bearing: the tick's SELECT
/// once dropped a column its row handler `get`s (sqlx panics on a missing
/// column), which killed the scheduler task on its first governed row — no
/// unit test of the pure helpers could see it.
pub async fn scheduler_tick(
    state: &Arc<AppState>,
    last_desired: &mut std::collections::HashMap<String, bool>,
    last_present: &mut std::collections::HashMap<String, std::time::Instant>,
    hour: usize,
    now_min: u16,
) {
    use std::collections::HashSet;
    use std::time::{Duration as StdDuration, Instant};

    // Any kiosk governed by *either* policy.
    let rows = sqlx::query(
        "SELECT id, room_id, schedule_enabled, sleep_at, wake_at,
                    presence_enabled, presence_timeout_secs, hour_modes,
                    aware_override_targets
             FROM kiosks
             WHERE schedule_enabled = 1 OR presence_enabled = 1",
    )
    .fetch_all(&state.db)
    .await;
    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(target: "bifrost::kiosk", "scheduler db read failed: {e}");
            return;
        }
    };

    let mut governed = HashSet::new();
    for row in rows {
        let id: String = row.get("id");

        // A painted hour plan (mig 0059) supersedes the legacy pair; the
        // master switch is schedule_enabled either way.
        let plan_enabled = row.get::<i64, _>("schedule_enabled") != 0;
        let mode = row
            .get::<Option<String>, _>("hour_modes")
            .filter(|_| plan_enabled)
            .and_then(|m| plan_mode(&m, hour));

        // Quiet-hours verdict (legacy path only).
        let schedule_awake = if mode.is_none() && plan_enabled {
            let sleep = row
                .get::<Option<String>, _>("sleep_at")
                .as_deref()
                .and_then(parse_hhmm);
            let wake = row
                .get::<Option<String>, _>("wake_at")
                .as_deref()
                .and_then(parse_hhmm);
            match (sleep, wake) {
                (Some(s), Some(w)) if s != w => Some(desired_awake_at(s, w, now_min)),
                _ => None,
            }
        } else {
            None
        };

        // Presence verdict — needed for an Aware hour, or the legacy flag.
        // (Needs an assigned room with presence sensors; the grace timer
        // keeps the screen up until the room has been empty long enough.)
        let wants_presence = match mode {
            Some(PlanMode::Aware) => true,
            Some(_) => false,
            None => row.get::<i64, _>("presence_enabled") != 0,
        };
        let present = if wants_presence {
            match row.get::<Option<String>, _>("room_id") {
                Some(room_id) => {
                    let timeout = StdDuration::from_secs(
                        row.get::<i64, _>("presence_timeout_secs").max(0) as u64,
                    );
                    match crate::api::rooms::room_occupancy(state, &room_id).await {
                        Some(true) => {
                            last_present.insert(id.clone(), Instant::now());
                            Some(true)
                        }
                        // Empty room: stay "present" until the grace elapses.
                        Some(false) => {
                            Some(last_present.get(&id).is_some_and(|t| t.elapsed() < timeout))
                        }
                        None => None, // no presence sensors → presence doesn't govern
                    }
                }
                None => None,
            }
        } else {
            None
        };

        // Aware override: while any configured device is on, the room reads
        // occupied regardless of actual presence — "don't let the screen
        // sleep from a no-motion timeout while the TV is playing". Only
        // meaningful during an Aware hour (the only mode presence governs at
        // all); refreshes the grace timer too, so turning the device back off
        // hands smoothly to the normal no-motion countdown instead of going
        // dark the instant it's off.
        let present = if mode == Some(PlanMode::Aware) {
            let targets = parse_aware_targets(row.get("aware_override_targets"));
            let mut overridden = false;
            for t in &targets {
                let Some(domain) = (match t.domain.as_str() {
                    "light" => Some(crate::models::automation::TriggerDeviceDomain::Light),
                    "media" => Some(crate::models::automation::TriggerDeviceDomain::Media),
                    "power" => Some(crate::models::automation::TriggerDeviceDomain::Power),
                    _ => None,
                }) else {
                    continue;
                };
                if crate::api::automations::cached_device_on(state, domain, &t.id).await
                    == Some(true)
                {
                    overridden = true;
                    break;
                }
            }
            if overridden {
                last_present.insert(id.clone(), Instant::now());
                Some(true)
            } else {
                present
            }
        } else {
            present
        };

        let desired = match mode {
            Some(m) => plan_desired(m, present),
            None => combined_desired_awake(schedule_awake, present),
        };
        let Some(awake) = desired else {
            continue; // nothing governs this hour (e.g. aware but no sensors)
        };
        governed.insert(id.clone());

        if last_desired.get(&id) == Some(&awake) {
            continue; // already in the desired state — nothing to send.
        }
        let cmd = if awake { "wake" } else { "sleep" };
        match queue_kiosk_command(state, &id, cmd).await {
            Ok(_) => {
                tracing::debug!(
                    target: "bifrost::kiosk",
                    kiosk_id = %id, %cmd, now = %fmt_hhmm(now_min),
                    schedule = ?schedule_awake, presence = ?present,
                    "display-power command"
                );
                last_desired.insert(id, awake);
            }
            Err(e) => tracing::warn!(
                target: "bifrost::kiosk", kiosk_id = %id,
                "scheduler failed to queue {cmd}: {e}"
            ),
        }
    }
    // Forget kiosks no longer governed, so re-enabling one reconciles it afresh.
    last_desired.retain(|id, _| governed.contains(id));
    last_present.retain(|id, _| governed.contains(id));
}

// ── Microphone presence: the kiosk mic as a room occupancy sensor ────────────

/// The internal pseudo-provider that owns kiosk microphone sensors. One row in
/// `providers` (type `kiosk`, seeded on first use) so the sensors are REAL
/// `sensor_devices` rows — room presence, the kiosk scheduler, automations,
/// SSE, and the Devices page all see them through the existing machinery.
pub const KIOSK_SENSOR_PROVIDER: &str = "kiosk-sensors";

/// Idempotently seed the pseudo-provider row and its manager-less sensor push
/// channel (`ConnectionRegistry::ensure_sensor_push_channel`). Called lazily by
/// the mic endpoints and at startup, so readings always have a pipeline.
pub async fn ensure_kiosk_sensor_channel(state: &Arc<AppState>) {
    let enc = state.encrypt_credentials("{}").unwrap_or_default();
    let _ = sqlx::query(
        "INSERT OR IGNORE INTO providers (id, provider_type, name, credentials)
         VALUES (?, 'kiosk', 'Kiosk sensors', ?)",
    )
    .bind(KIOSK_SENSOR_PROVIDER)
    .bind(&enc)
    .execute(&state.db)
    .await;
    let mut connections = state.connections.lock().await;
    connections.ensure_sensor_push_channel(
        KIOSK_SENSOR_PROVIDER.to_string(),
        state.db.clone(),
        state.occupancy_seen.clone(),
    );
}

/// Point a kiosk's mic sensor at its kiosk's room (direct assignment; the
/// membership moves whenever the kiosk is reassigned).
async fn sync_mic_sensor_room(db: &sqlx::SqlitePool, sensor_id: &str, room_id: Option<&str>) {
    let _ = sqlx::query("DELETE FROM room_sensor_devices WHERE sensor_device_id = ?")
        .bind(sensor_id)
        .execute(db)
        .await;
    if let Some(room_id) = room_id {
        let _ = sqlx::query(
            "INSERT OR IGNORE INTO room_sensor_devices (room_id, sensor_device_id) VALUES (?, ?)",
        )
        .bind(room_id)
        .bind(sensor_id)
        .execute(db)
        .await;
    }
}

#[derive(Deserialize)]
struct SetMicRequest {
    /// Turn the mic occupancy sensor on/off for this kiosk.
    enabled: bool,
    /// low | medium | high. Omitted = keep the stored value.
    #[serde(default)]
    sensitivity: Option<String>,
}

/// `PUT /api/kiosks/{id}/mic` (session) — enable/disable microphone presence.
/// Enabling creates the kiosk's `sensor_devices` row (kind `occupancy`) under
/// [`KIOSK_SENSOR_PROVIDER`] and assigns it to the kiosk's room; disabling
/// removes it (membership cascades). The app itself picks the change up on its
/// next check-in and starts/stops the on-device level monitor.
async fn set_mic(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetMicRequest>,
) -> impl IntoResponse {
    if let Some(sens) = req.sensitivity.as_deref()
        && !["low", "medium", "high"].contains(&sens)
    {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "sensitivity must be low, medium, or high",
        )
            .into_response();
    }
    let row = sqlx::query("SELECT name, room_id FROM kiosks WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await;
    let (kiosk_name, room_id): (String, Option<String>) = match row {
        Ok(Some(r)) => (r.get("name"), r.get("room_id")),
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error reading kiosk: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let _ = sqlx::query(
        "UPDATE kiosks SET mic_presence = ?, mic_sensitivity = COALESCE(?, mic_sensitivity)
         WHERE id = ?",
    )
    .bind(i64::from(req.enabled))
    .bind(&req.sensitivity)
    .bind(&id)
    .execute(&state.db)
    .await;

    if req.enabled {
        ensure_kiosk_sensor_channel(&state).await;
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT id FROM sensor_devices WHERE provider_id = ? AND device_id = ?",
        )
        .bind(KIOSK_SENSOR_PROVIDER)
        .bind(&id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
        let sensor_id = match existing {
            Some(sid) => sid,
            None => {
                let sid = Uuid::new_v4().to_string();
                let name = format!("{kiosk_name} sound");
                let initial =
                    serde_json::to_string(&crate::models::sensor::SensorState::boolean(false))
                        .unwrap_or_default();
                let _ = sqlx::query(
                    "INSERT INTO sensor_devices (id, provider_id, device_id, name, provider_name, kind, last_state, last_seen)
                     VALUES (?, ?, ?, ?, ?, 'occupancy', ?, datetime('now'))",
                )
                .bind(&sid)
                .bind(KIOSK_SENSOR_PROVIDER)
                .bind(&id)
                .bind(&name)
                .bind(&name)
                .bind(&initial)
                .execute(&state.db)
                .await;
                sid
            }
        };
        sync_mic_sensor_room(&state.db, &sensor_id, room_id.as_deref()).await;
    } else {
        // Membership rows cascade with the sensor.
        let _ = sqlx::query("DELETE FROM sensor_devices WHERE provider_id = ? AND device_id = ?")
            .bind(KIOSK_SENSOR_PROVIDER)
            .bind(&id)
            .execute(&state.db)
            .await;
    }
    // Either direction changes what counts toward the room's occupancy.
    state.occupancy_seen.poke();
    crate::api::notify_inventory(&state, "sensor_devices");
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct NoiseRequest {
    /// The on-device verdict: sound level is elevated above the ambient baseline.
    elevated: bool,
    /// Current level (dBFS) — telemetry only, shown on the Clients view.
    #[serde(default)]
    level: Option<f64>,
}

/// `POST /api/kiosks/self/noise` — the kiosk app reports an elevated/quiet edge
/// (same `bfr_key` cookie auth as `/self`). The reading is injected into the
/// shared sensor pipeline (persist → journal → occupancy poke → automations →
/// SSE) via the pseudo-provider's push channel — never a parallel path. A
/// report for a kiosk whose mic is disabled is dropped (a stale app config
/// between check-ins, not an error).
async fn report_noise(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<NoiseRequest>,
) -> impl IntoResponse {
    let Some(key) = crate::api::auth::kiosk_cookie_key(&headers) else {
        return StatusCode::UNAUTHORIZED;
    };
    let Some(key_id) = crate::api::apikeys::validate_key(&state, &key).await else {
        return StatusCode::UNAUTHORIZED;
    };
    let row = sqlx::query("SELECT id, mic_presence FROM kiosks WHERE api_key_id = ?")
        .bind(&key_id)
        .fetch_optional(&state.db)
        .await;
    let (kiosk_id, mic_on): (String, bool) = match row {
        Ok(Some(r)) => (r.get("id"), r.get::<i64, _>("mic_presence") != 0),
        Ok(None) => return StatusCode::NOT_FOUND,
        Err(e) => {
            tracing::error!("db error resolving kiosk: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };
    if let Some(level) = req.level {
        let _ = sqlx::query("UPDATE kiosks SET mic_level = ? WHERE id = ?")
            .bind(level)
            .bind(&kiosk_id)
            .execute(&state.db)
            .await;
    }
    if !mic_on {
        return StatusCode::NO_CONTENT;
    }
    ensure_kiosk_sensor_channel(&state).await;
    let sender = {
        let connections = state.connections.lock().await;
        connections.sensor_sender(KIOSK_SENSOR_PROVIDER)
    };
    if let Some(tx) = sender {
        let _ = tx.send(crate::connection::SensorEvent {
            device_id: kiosk_id,
            state: crate::models::sensor::SensorState::boolean(req.elevated),
        });
    }
    StatusCode::NO_CONTENT
}

// ── Aware override: keep the screen awake while a device is on ───────────────

/// Decode a kiosk row's stored override targets; a missing/malformed value
/// degrades to "no override" rather than failing the whole listing (matches
/// `list_room_controls`'s tolerance for a stale row).
fn parse_aware_targets(raw: Option<String>) -> Vec<ControlTarget> {
    raw.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct SetAwareOverrideRequest {
    /// Replaces the kiosk's override list wholesale. Empty = no override
    /// (Aware hours follow presence only, the default).
    targets: Vec<ControlTarget>,
}

/// `PUT /api/kiosks/{id}/aware-override` (session) — set the devices that keep
/// an Aware hour's screen awake regardless of presence while any of them is
/// on (`scheduler_tick` reads this list — see its doc comment). No existence
/// check on each target: a removed device just never reads "on" again, the
/// same tolerance `room_controls` targets already have for a stale reference.
async fn set_aware_override(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetAwareOverrideRequest>,
) -> impl IntoResponse {
    for t in &req.targets {
        if !["light", "media", "power"].contains(&t.domain.as_str()) {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("unknown target domain '{}'", t.domain),
            )
                .into_response();
        }
    }
    let json = serde_json::to_string(&req.targets).unwrap_or_else(|_| "[]".into());
    match sqlx::query("UPDATE kiosks SET aware_override_targets = ? WHERE id = ?")
        .bind(&json)
        .bind(&id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            // A config edit reconciles the screen immediately, same as every
            // other display-policy setter.
            state.occupancy_seen.poke();
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error setting kiosk aware override: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/kiosks/{id}/deauth` (session) — revoke the kiosk's API key. Its
/// next request 401s and the app re-enrolls via a fresh QR scan. The kiosk row
/// survives (its `api_key_id` cascades to NULL) so the clients view shows it as
/// awaiting re-pair.
async fn deauth(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let key_id: Option<Option<String>> =
        sqlx::query_scalar("SELECT api_key_id FROM kiosks WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.db)
            .await
            .ok();

    match key_id {
        Some(Some(kid)) => {
            // Revoking the key cascades api_key_id → NULL on the kiosk row.
            let _ = sqlx::query("DELETE FROM api_keys WHERE id = ?")
                .bind(&kid)
                .execute(&state.db)
                .await;
            StatusCode::NO_CONTENT.into_response()
        }
        // Kiosk exists but already de-authed (no key) — idempotent success.
        Some(None) => StatusCode::NO_CONTENT.into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `DELETE /api/kiosks/{id}` (session) — forget a kiosk record entirely (e.g. a
/// decommissioned tablet). Does not revoke its key — use de-auth for that first.
async fn forget(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match sqlx::query("DELETE FROM kiosks WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error forgetting kiosk: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{desired_awake_at, fmt_hhmm, parse_hhmm};

    #[test]
    fn parse_hhmm_accepts_valid_clock_times() {
        assert_eq!(parse_hhmm("00:00"), Some(0));
        assert_eq!(parse_hhmm("07:30"), Some(450));
        assert_eq!(parse_hhmm("23:59"), Some(1439));
        assert_eq!(parse_hhmm(" 9:05 "), Some(545)); // trimmed, single-digit hour
    }

    #[test]
    fn parse_hhmm_rejects_malformed_or_out_of_range() {
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("12:60"), None);
        assert_eq!(parse_hhmm("12"), None);
        assert_eq!(parse_hhmm("noon"), None);
        assert_eq!(parse_hhmm(""), None);
    }

    #[test]
    fn fmt_hhmm_zero_pads_and_roundtrips() {
        assert_eq!(fmt_hhmm(0), "00:00");
        assert_eq!(fmt_hhmm(545), "09:05");
        assert_eq!(fmt_hhmm(1439), "23:59");
        for m in [0u16, 61, 450, 720, 1439] {
            assert_eq!(parse_hhmm(&fmt_hhmm(m)), Some(m));
        }
    }

    #[test]
    fn desired_awake_within_a_wrapping_overnight_window() {
        // Quiet 23:00 → 07:00: asleep late night and early morning, awake by day.
        let (sleep, wake) = (23 * 60, 7 * 60);
        assert!(!desired_awake_at(sleep, wake, 23 * 60)); // 23:00 — sleep boundary (asleep)
        assert!(!desired_awake_at(sleep, wake, 3 * 60)); // 03:00 — asleep
        assert!(desired_awake_at(sleep, wake, 7 * 60)); // 07:00 — wake boundary (awake)
        assert!(desired_awake_at(sleep, wake, 12 * 60)); // noon — awake
        assert!(desired_awake_at(sleep, wake, 22 * 60 + 59)); // 22:59 — still awake
    }

    #[test]
    fn desired_awake_within_a_same_day_window() {
        // Quiet 01:00 → 05:00 (non-wrapping): asleep only in that band.
        let (sleep, wake) = (60, 300);
        assert!(desired_awake_at(sleep, wake, 0)); // 00:00 — awake
        assert!(!desired_awake_at(sleep, wake, 60)); // 01:00 — asleep (inclusive start)
        assert!(!desired_awake_at(sleep, wake, 240)); // 04:00 — asleep
        assert!(desired_awake_at(sleep, wake, 300)); // 05:00 — awake (exclusive end)
        assert!(desired_awake_at(sleep, wake, 600)); // 10:00 — awake
    }

    #[test]
    fn desired_awake_equal_endpoints_is_always_awake() {
        assert!(desired_awake_at(600, 600, 600));
        assert!(desired_awake_at(600, 600, 0));
    }

    #[test]
    fn combined_quiet_hours_is_a_hard_sleep_override() {
        use super::combined_desired_awake as c;
        // Schedule asleep beats an occupied room — no lighting the room at 3am.
        assert_eq!(c(Some(false), Some(true)), Some(false));
        assert_eq!(c(Some(false), None), Some(false));
    }

    #[test]
    fn combined_presence_governs_outside_quiet_hours() {
        use super::combined_desired_awake as c;
        // Daytime (schedule awake or absent): follow the room.
        assert_eq!(c(Some(true), Some(true)), Some(true));
        assert_eq!(c(Some(true), Some(false)), Some(false));
        assert_eq!(c(None, Some(true)), Some(true));
        assert_eq!(c(None, Some(false)), Some(false));
    }

    #[test]
    fn combined_falls_back_to_schedule_and_none_when_ungoverned() {
        use super::combined_desired_awake as c;
        // Presence gives no reading → schedule verdict stands.
        assert_eq!(c(Some(true), None), Some(true));
        // Neither policy governs → leave the kiosk alone.
        assert_eq!(c(None, None), None);
    }

    #[test]
    fn plan_mode_reads_the_hour_and_rejects_junk() {
        use super::{PlanMode, plan_mode};
        let plan = "SSSSSSAAWWWWWWWWWWAAAASS"; // 24 chars
        assert_eq!(plan_mode(plan, 0), Some(PlanMode::Asleep));
        assert_eq!(plan_mode(plan, 6), Some(PlanMode::Aware));
        assert_eq!(plan_mode(plan, 12), Some(PlanMode::Awake));
        assert_eq!(plan_mode(plan, 23), Some(PlanMode::Asleep));
        // Out of range / malformed → None (caller falls back to legacy).
        assert_eq!(plan_mode(plan, 24), None);
        assert_eq!(plan_mode("XXXX", 1), None);
        assert_eq!(plan_mode("", 0), None);
    }

    #[test]
    fn plan_desired_awake_asleep_absolute_aware_follows_presence() {
        use super::{PlanMode, plan_desired};
        // Absolute hours ignore the room entirely.
        assert_eq!(plan_desired(PlanMode::Awake, Some(false)), Some(true));
        assert_eq!(plan_desired(PlanMode::Asleep, Some(true)), Some(false));
        // Aware follows presence — and governs nothing without sensors.
        assert_eq!(plan_desired(PlanMode::Aware, Some(true)), Some(true));
        assert_eq!(plan_desired(PlanMode::Aware, Some(false)), Some(false));
        assert_eq!(plan_desired(PlanMode::Aware, None), None);
    }
}
