use crate::AppState;
use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::{
    Json, Router,
    extract::{FromRequestParts, State},
    http::{HeaderMap, StatusCode, header, request::Parts},
    response::IntoResponse,
    routing::post,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

const SESSION_COOKIE: &str = "bifrost_session";
const SESSION_TTL_HOURS: i64 = 24 * 7;

const KIOSK_KEY_COOKIE: &str = "bfr_key";

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/kiosk", post(kiosk_login))
}

/// Create a session row and return the `Set-Cookie` header value for it. Shared by
/// the password login and the kiosk key exchange.
async fn mint_session_cookie(state: &Arc<AppState>) -> String {
    let session_id = Uuid::new_v4().to_string();
    let expires_at = (Utc::now() + chrono::Duration::hours(SESSION_TTL_HOURS))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let _ = sqlx::query("INSERT INTO sessions (id, expires_at) VALUES (?, ?)")
        .bind(&session_id)
        .bind(&expires_at)
        .execute(&state.db)
        .await;
    format!(
        "{SESSION_COOKIE}={session_id}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        SESSION_TTL_HOURS * 3600
    )
}

/// `POST /api/auth/kiosk` — a paired kiosk trades its `bfr_` key (carried in the
/// `bfr_key` cookie the kiosk WebView sets) for a real dashboard session, so an
/// authorized wall fixture never has to show the password login. Validates the
/// key like any public-API call; 401 if absent/unknown.
async fn kiosk_login(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    let Some(key) = extract_cookie(&headers, KIOSK_KEY_COOKIE) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if crate::api::apikeys::validate_key(&state, &key)
        .await
        .is_none()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let cookie = mint_session_cookie(&state).await;
    let mut out = HeaderMap::new();
    out.insert(header::SET_COOKIE, cookie.parse().unwrap());
    (out, Json(LoginResponse { ok: true })).into_response()
}

#[derive(Deserialize)]
pub struct LoginRequest {
    password: String,
}

#[derive(Serialize)]
struct LoginResponse {
    ok: bool,
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> impl IntoResponse {
    let row = sqlx::query("SELECT password_hash FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await;

    let hash_str: String = match row {
        Ok(Some(r)) => r.get("password_hash"),
        _ => return (StatusCode::UNAUTHORIZED, "not configured").into_response(),
    };

    let hash = match PasswordHash::new(&hash_str) {
        Ok(h) => h,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "bad hash").into_response(),
    };

    if Argon2::default()
        .verify_password(req.password.as_bytes(), &hash)
        .is_err()
    {
        return (StatusCode::UNAUTHORIZED, "wrong password").into_response();
    }

    let cookie = mint_session_cookie(&state).await;
    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, cookie.parse().unwrap());
    (headers, Json(LoginResponse { ok: true })).into_response()
}

pub async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Some(session_id) = extract_session(&headers) {
        let _ = sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(&session_id)
            .execute(&state.db)
            .await;
    }

    let clear = format!("{SESSION_COOKIE}=; HttpOnly; Path=/; Max-Age=0");
    let mut out = HeaderMap::new();
    out.insert(header::SET_COOKIE, clear.parse().unwrap());
    (out, Json(LoginResponse { ok: true })).into_response()
}

/// Returns the session id if the request carries a valid, non-expired session.
pub async fn require_session(state: &Arc<AppState>, headers: &HeaderMap) -> Option<String> {
    let session_id = extract_session(headers)?;
    let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    let row = sqlx::query(
        "UPDATE sessions SET last_used = datetime('now')
         WHERE id = ? AND expires_at > ?
         RETURNING id",
    )
    .bind(&session_id)
    .bind(&now)
    .fetch_optional(&state.db)
    .await
    .ok()??;

    Some(row.get("id"))
}

/// Request extractor that admits only an authenticated session, rejecting with
/// `401` before the handler body runs. Replaces the repeated
/// `if require_session(&state, &headers).await.is_none() { return 401 }` guard:
/// a session-only handler takes `_: Session`, and public routes simply omit it.
/// (Need the session id in a handler? Call [`require_session`] directly.)
pub struct Session;

impl FromRequestParts<Arc<AppState>> for Session {
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        if require_session(state, &parts.headers).await.is_some() {
            Ok(Session)
        } else {
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

/// Request extractor admitting an authenticated session **or** a paired
/// kiosk's `bfr_key` cookie (validated exactly like a public-API Bearer key,
/// so it grants nothing the key doesn't already have). For read surfaces a
/// wall kiosk renders directly (the feed widget's data + poster proxy): the
/// kiosk talks only to Bifrost, and its *key* is its durable identity — the
/// session it mints from that key can lapse mid-run, and a lapsed session
/// must not blank a wall fixture that is still holding a valid key.
pub struct SessionOrKiosk;

impl FromRequestParts<Arc<AppState>> for SessionOrKiosk {
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        // Journal-visible request trace (debug — the RUST_LOG console stays
        // quiet). A kiosk's requests are attributable by its distinctive UA,
        // which is what makes "are the wall tablet's poster fetches arriving
        // at all, and how do they auth?" answerable from Settings → Developer
        // instead of guesswork — the exact question a poster-less kiosk poses.
        let is_kiosk_ua = parts
            .headers
            .get(axum::http::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ua| ua.contains("BifrostKiosk"));
        let path = parts.uri.path().to_string();
        if require_session(state, &parts.headers).await.is_some() {
            tracing::debug!(target: "bifrost::feeds", kiosk_ua = is_kiosk_ua, %path, auth = "session", "feed request");
            return Ok(SessionOrKiosk);
        }
        if let Some(key) = kiosk_cookie_key(&parts.headers)
            && crate::api::apikeys::validate_key(state, &key)
                .await
                .is_some()
        {
            tracing::debug!(target: "bifrost::feeds", kiosk_ua = is_kiosk_ua, %path, auth = "kiosk_key", "feed request");
            return Ok(SessionOrKiosk);
        }
        // The rejection would otherwise be invisible: extractor 401s never
        // reach a handler, so a kiosk whose cookies stopped flowing looked
        // identical to a kiosk that never asked.
        tracing::warn!(
            target: "bifrost::feeds",
            kiosk_ua = is_kiosk_ua,
            %path,
            has_session_cookie = extract_session(&parts.headers).is_some(),
            has_key_cookie = kiosk_cookie_key(&parts.headers).is_some(),
            "feed request rejected (no valid session or kiosk key)"
        );
        Err(StatusCode::UNAUTHORIZED)
    }
}

fn extract_session(headers: &HeaderMap) -> Option<String> {
    extract_cookie(headers, SESSION_COOKIE)
}

/// The raw `bfr_` API key a kiosk WebView carries in its `bfr_key` cookie, if
/// present. Lets a kiosk-served page resolve *its own* kiosk record server-side.
pub(crate) fn kiosk_cookie_key(headers: &HeaderMap) -> Option<String> {
    extract_cookie(headers, KIOSK_KEY_COOKIE)
}

/// Pull a named cookie's value out of the `Cookie` header.
fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookie_hdr = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie_hdr.split(';').find_map(|part| {
        let part = part.trim();
        part.strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
            .map(|v| v.to_string())
    })
}
