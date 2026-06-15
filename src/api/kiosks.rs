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
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

/// A kiosk is "online" if it checked in within this window.
const ONLINE_WINDOW_SECS: i64 = 90;

/// Commands the app performs on check-in. (`deauth` is not here — it's an
/// immediate key revocation, surfaced to the app as a 401, not a queued action.)
const VALID_COMMANDS: [&str; 3] = ["sleep", "wake", "lock"];

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/checkin", post(checkin))
        .route("/", get(list))
        .route("/{id}/command", post(command))
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
}

#[derive(Serialize)]
struct CheckinResponse {
    /// The command to perform, if any was queued — consumed by this check-in.
    command: Option<String>,
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
        "INSERT INTO kiosks (id, api_key_id, name, app_version, screen_on, last_seen)
         VALUES (?, ?, ?, ?, ?, datetime('now'))
         ON CONFLICT(api_key_id) DO UPDATE SET
             name        = excluded.name,
             app_version = excluded.app_version,
             screen_on   = excluded.screen_on,
             last_seen   = datetime('now')
         RETURNING id, pending_command",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&key_id)
    .bind(&name)
    .bind(&req.app_version)
    .bind(req.screen_on.map(i64::from))
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

    Json(CheckinResponse { command }).into_response()
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
}

/// `GET /api/kiosks` (session) — the clients view: every registered kiosk with
/// its check-in status. Session-only, so it isn't reachable with a kiosk key.
async fn list(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    let rows = sqlx::query(&format!(
        "SELECT id, name, app_version, screen_on, last_seen, pending_command,
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

/// `POST /api/kiosks/{id}/command` (session) — queue a command for the kiosk to
/// pick up on its next check-in.
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
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error queuing kiosk command: {e}");
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
