//! Kiosk controller — register, observe, and manage the wall-tablet companion
//! apps that "check in" to Bifrost.
//!
//! A kiosk is identified by the `bfr_` API key it carries (minted via QR
//! enrollment). It **checks in** on a heartbeat ([`checkin`], key-authenticated)
//! reporting its label / app version / screen state; the server records
//! `last_seen` and returns any **queued command** (`sleep` | `wake` | `lock`),
//! which the app performs and which is then consumed.
//!
//! Management endpoints ([`list`], [`command`], [`deauth`], [`forget`]) are
//! **session-authenticated** — driven from a mobile/desktop browser, not the
//! kiosk itself. Command semantics:
//! - `sleep` / `wake` — turn the display off/on.
//! - `lock` — force sign-out of the Bifrost WebView session (re-enter password).
//! - **de-auth** — revoke the kiosk's API key (a separate endpoint, not a queued
//!   command): the app's next call 401s and it re-enrolls via a fresh QR scan.

use crate::AppState;
use crate::api::apikeys::require_api_key;
use crate::api::auth::Session;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    response::sse::{Event, KeepAlive, Sse},
    routing::{delete, get, post},
};
use futures_util::stream::{Stream, StreamExt};
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

    Json(CheckinResponse { command, room }).into_response()
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
    // Battery / power telemetry from the latest check-in (null on older apps).
    battery_level: Option<i64>,
    battery_charging: Option<bool>,
    battery_voltage_mv: Option<i64>,
    battery_current_ua: Option<i64>,
    battery_temp_dc: Option<i64>,
    power_source: Option<String>,
}

/// `GET /api/kiosks` (session) — the clients view: every registered kiosk with
/// its check-in status. Session-only, so it isn't reachable with a kiosk key.
async fn list(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    let rows = sqlx::query(&format!(
        "SELECT id, name, app_version, screen_on, last_seen, pending_command, room_id,
                battery_level, battery_charging, battery_voltage_mv, battery_current_ua,
                battery_temp_dc, power_source,
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
                    battery_level: r.get("battery_level"),
                    battery_charging: r.get::<Option<i64>, _>("battery_charging").map(|v| v != 0),
                    battery_voltage_mv: r.get("battery_voltage_mv"),
                    battery_current_ua: r.get("battery_current_ua"),
                    battery_temp_dc: r.get("battery_temp_dc"),
                    power_source: r.get("power_source"),
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
    match sqlx::query("UPDATE kiosks SET pending_command = ? WHERE id = ?")
        .bind(cmd)
        .bind(&id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            // Push to any live stream now; the row above covers offline kiosks.
            let _ = state.kiosk_commands.send(KioskCommand {
                kiosk_id: id.clone(),
                command: cmd.to_string(),
            });
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error queuing kiosk command: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `GET /api/kiosks/stream` (API-key auth) — the kiosk's live command channel.
/// Opened by the kiosk after it checks in; controller commands ([`command`]) are
/// pushed here instantly as SSE `command` events instead of waiting for the next
/// poll. Requires the kiosk to be registered (so we can resolve its id); if not,
/// 404 and the app retries after its next heartbeat.
async fn stream(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>> + Send + 'static>, StatusCode> {
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

    Ok(Sse::new(events).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    ))
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
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT,
        Ok(_) => StatusCode::NOT_FOUND,
        Err(e) => {
            tracing::error!("db error setting kiosk room: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
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
