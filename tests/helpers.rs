//! Shared test fixtures for integration tests.

use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header};
use bifrost::{AppState, build_app, providers};
use sqlx::SqlitePool;
use std::sync::Arc;
use tower::ServiceExt;

pub const TEST_SECRET: &str = "test-secret-key-32-bytes-exactly";
pub const TEST_PASSWORD: &str = "correct-horse-battery-staple";

/// Create an in-memory SQLite pool with all migrations applied.
pub async fn test_db() -> SqlitePool {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    pool
}

/// Hash a password with reduced Argon2 params suitable for testing speed.
pub fn hash_password(password: &str) -> String {
    use argon2::{Algorithm, Params, Version};
    let params = Params::new(1024, 1, 1, None).unwrap();
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let salt = SaltString::generate(&mut OsRng);
    argon2
        .hash_password(password.as_bytes(), &salt)
        .unwrap()
        .to_string()
}

/// Build a complete test app with an in-memory DB and the default provider registry.
pub async fn test_app() -> Router {
    let db = test_db().await;
    let registry = providers::default_registry();
    let state = Arc::new(AppState::new(db, TEST_SECRET, registry));
    build_app(state)
}

/// Build a test app whose DB already has a configured password.
pub async fn test_app_with_password() -> Router {
    let db = test_db().await;
    let hash = hash_password(TEST_PASSWORD);
    sqlx::query("INSERT INTO config (id, password_hash, setup_complete) VALUES (1, ?, 1)")
        .bind(&hash)
        .execute(&db)
        .await
        .unwrap();
    let registry = providers::default_registry();
    let state = Arc::new(AppState::new(db, TEST_SECRET, registry));
    build_app(state)
}

/// POST /api/auth/login and return the Set-Cookie header value on success.
pub async fn login(app: &Router, password: &str) -> String {
    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(r#"{{"password":"{password}"}}"#)))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "login failed");
    resp.headers()
        .get(header::SET_COOKIE)
        .expect("no Set-Cookie header")
        .to_str()
        .unwrap()
        .to_string()
}

/// Build an authenticated GET request with the given session cookie.
pub fn authed_get(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::COOKIE, cookie.split(';').next().unwrap()) // send only name=value
        .body(Body::empty())
        .unwrap()
}

pub async fn response_json(resp: Response<Body>) -> serde_json::Value {
    let bytes = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}
