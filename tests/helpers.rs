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
/// Foreign keys are enabled to match production (`db::connect`).
pub async fn test_db() -> SqlitePool {
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;
    let opts = SqliteConnectOptions::from_str(":memory:")
        .unwrap()
        .foreign_keys(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
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

/// Build an authenticated POST request with a JSON body and the given session cookie.
#[allow(dead_code)] // used by some, not all, test binaries
pub fn authed_post(uri: &str, cookie: &str, json_body: &str) -> Request<Body> {
    authed_json("POST", uri, cookie, json_body)
}

/// Build an authenticated request with the given method and JSON body.
#[allow(dead_code)]
pub fn authed_json(method: &str, uri: &str, cookie: &str, json_body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookie.split(';').next().unwrap())
        .body(Body::from(json_body.to_string()))
        .unwrap()
}

/// Build an authenticated DELETE request.
#[allow(dead_code)]
pub fn authed_delete(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header(header::COOKIE, cookie.split(';').next().unwrap())
        .body(Body::empty())
        .unwrap()
}

/// Test app with a configured password plus one WLED provider + light whose
/// device URL points at `base_url` (a wiremock server). Returns the app and
/// the seeded light's ID. The light starts on at 80% brightness.
#[allow(dead_code)]
pub async fn test_app_with_light(base_url: &str) -> (Router, String) {
    let db = test_db().await;
    let hash = hash_password(TEST_PASSWORD);
    sqlx::query("INSERT INTO config (id, password_hash, setup_complete) VALUES (1, ?, 1)")
        .bind(&hash)
        .execute(&db)
        .await
        .unwrap();

    let registry = providers::default_registry();
    let state = Arc::new(AppState::new(db.clone(), TEST_SECRET, registry));

    let creds = format!(r#"{{"device_ip":"{base_url}"}}"#);
    let enc = state.encrypt_credentials(&creds).unwrap();
    sqlx::query("INSERT INTO providers (id, provider_type, name, credentials) VALUES (?, 'wled', 'Test WLED', ?)")
        .bind("prov-test-1")
        .bind(&enc)
        .execute(&db)
        .await
        .unwrap();

    let light_id = "light-test-1".to_string();
    sqlx::query(
        "INSERT INTO lights (id, provider_id, device_id, name, capabilities, last_state, last_seen)
         VALUES (?, 'prov-test-1', 'main', 'Test Light', '{}', ?, datetime('now'))",
    )
    .bind(&light_id)
    .bind(r#"{"on":true,"brightness":80.0,"color":null,"color_temp_mirek":null}"#)
    .execute(&db)
    .await
    .unwrap();

    (build_app(state), light_id)
}

/// Test app with a configured password plus one Hue provider whose bridge
/// URL points at `base_url` (a wiremock server), and one discovered light
/// with device_id "light-1". Returns the app and the seeded light's ID.
#[allow(dead_code)]
pub async fn test_app_with_hue_light(base_url: &str) -> (Router, String) {
    let db = test_db().await;
    let hash = hash_password(TEST_PASSWORD);
    sqlx::query("INSERT INTO config (id, password_hash, setup_complete) VALUES (1, ?, 1)")
        .bind(&hash)
        .execute(&db)
        .await
        .unwrap();

    let registry = providers::default_registry();
    let state = Arc::new(AppState::new(db.clone(), TEST_SECRET, registry));

    let creds = format!(r#"{{"bridge_ip":"{base_url}","app_key":"test-key"}}"#);
    let enc = state.encrypt_credentials(&creds).unwrap();
    sqlx::query("INSERT INTO providers (id, provider_type, name, credentials) VALUES (?, 'hue', 'Test Hue', ?)")
        .bind("prov-hue-1")
        .bind(&enc)
        .execute(&db)
        .await
        .unwrap();

    let light_id = "light-hue-1".to_string();
    sqlx::query(
        "INSERT INTO lights (id, provider_id, device_id, name, capabilities, last_state, last_seen)
         VALUES (?, 'prov-hue-1', 'light-1', 'Hue Bulb', '{}', ?, datetime('now'))",
    )
    .bind(&light_id)
    .bind(r#"{"on":true,"brightness":100.0,"color":null,"color_temp_mirek":null}"#)
    .execute(&db)
    .await
    .unwrap();

    (build_app(state), light_id)
}

pub async fn response_json(resp: Response<Body>) -> serde_json::Value {
    let bytes = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}
