use crate::AppState;
use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
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

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout))
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

    let session_id = Uuid::new_v4().to_string();
    let expires_at = (Utc::now() + chrono::Duration::hours(SESSION_TTL_HOURS))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();

    let _ = sqlx::query("INSERT INTO sessions (id, expires_at) VALUES (?, ?)")
        .bind(&session_id)
        .bind(&expires_at)
        .execute(&state.db)
        .await;

    let cookie = format!(
        "{SESSION_COOKIE}={session_id}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        SESSION_TTL_HOURS * 3600
    );

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

fn extract_session(headers: &HeaderMap) -> Option<String> {
    let cookie_hdr = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie_hdr.split(';').find_map(|part| {
        let part = part.trim();
        part.strip_prefix(SESSION_COOKIE)
            .and_then(|rest| rest.strip_prefix('='))
            .map(|v| v.to_string())
    })
}
