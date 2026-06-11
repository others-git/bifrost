use crate::AppState;
use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;

const MIN_PASSWORD_LEN: usize = 8;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", post(setup))
        .route("/status", get(setup_status))
}

#[derive(Serialize)]
struct SetupStatus {
    setup_complete: bool,
}

pub async fn setup_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let complete = sqlx::query("SELECT setup_complete FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .map(|r: sqlx::sqlite::SqliteRow| r.get::<i64, _>("setup_complete") != 0)
        .unwrap_or(false);

    Json(SetupStatus {
        setup_complete: complete,
    })
}

#[derive(Deserialize)]
struct SetupRequest {
    password: String,
}

async fn setup(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetupRequest>,
) -> impl IntoResponse {
    let already = sqlx::query("SELECT setup_complete FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .map(|r: sqlx::sqlite::SqliteRow| r.get::<i64, _>("setup_complete") != 0)
        .unwrap_or(false);

    if already {
        return StatusCode::CONFLICT.into_response();
    }

    if req.password.len() < MIN_PASSWORD_LEN {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("password must be at least {MIN_PASSWORD_LEN} characters"),
        )
            .into_response();
    }

    let hash = match Argon2::default()
        .hash_password(req.password.as_bytes(), &SaltString::generate(&mut OsRng))
    {
        Ok(h) => h.to_string(),
        Err(e) => {
            tracing::error!("argon2 error during setup: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    match sqlx::query("INSERT INTO config (id, password_hash, setup_complete) VALUES (1, ?, 1)")
        .bind(&hash)
        .execute(&state.db)
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => {
            tracing::error!("db error during setup: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_password_len_is_reasonable() {
        // Evaluated at compile time; fails to build if the constant is ever lowered below 8.
        const { assert!(MIN_PASSWORD_LEN >= 8) };
    }
}
