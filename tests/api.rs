mod helpers;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

// ── Setup ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn setup_status_returns_false_on_empty_db() {
    let app = helpers::test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/setup/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["setup_complete"], false);
}

#[tokio::test]
async fn setup_status_returns_true_after_password_set() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/setup/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["setup_complete"], true);
}

#[tokio::test]
async fn setup_sets_password_and_allows_login() {
    let app = helpers::test_app().await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/setup")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"password":"correct-horse-battery-staple"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"password":"correct-horse-battery-staple"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn setup_returns_409_if_already_configured() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/setup")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"password":"new-password-1234"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn setup_rejects_short_password() {
    let app = helpers::test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/setup")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"password":"short"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

// ── Events (SSE) ────────────────────────────────────────────────────────────

#[tokio::test]
async fn events_without_session_returns_401() {
    let app = helpers::test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn events_with_session_returns_sse_content_type() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_get("/api/events", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        ct.contains("text/event-stream"),
        "expected text/event-stream, got {ct}"
    );
}

// ── Health ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn health_endpoint_returns_ok() {
    let app = helpers::test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ── Auth ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn login_wrong_password_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"password":"wrong"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_correct_password_sets_httponly_cookie() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"password":"{}"}}"#,
                    helpers::TEST_PASSWORD
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(cookie.contains("bifrost_session="), "cookie name missing");
    assert!(
        cookie.to_lowercase().contains("httponly"),
        "HttpOnly flag missing"
    );
    assert!(
        cookie.to_lowercase().contains("samesite=strict"),
        "SameSite=Strict missing"
    );
}

#[tokio::test]
async fn login_when_no_config_returns_401() {
    // App with no password set (empty DB).
    let app = helpers::test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"password":"anything"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ── Lights — auth guard ─────────────────────────────────────────────────────

#[tokio::test]
async fn list_lights_without_session_returns_401() {
    let app = helpers::test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/lights")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_light_without_session_returns_401() {
    let app = helpers::test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/lights/some-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn list_lights_with_valid_session_returns_empty_array() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_get("/api/lights", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body, serde_json::json!([]));
}

// ── Providers ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_provider_types_requires_auth() {
    let app = helpers::test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/providers/types")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn list_provider_types_returns_hue_and_govee() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_get("/api/providers/types", &cookie))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    let types: Vec<String> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["provider_type"].as_str().unwrap().to_string())
        .collect();
    assert!(types.contains(&"hue".to_string()));
    assert!(types.contains(&"govee".to_string()));
}

#[tokio::test]
async fn add_provider_with_unknown_type_returns_400() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let cookie_val = cookie.split(';').next().unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/providers")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie_val)
                .body(Body::from(
                    r#"{"name":"Test","provider_type":"lifx","credentials":{}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn list_providers_with_valid_session_returns_empty_array() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_get("/api/providers", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body, serde_json::json!([]));
}

#[tokio::test]
async fn logout_clears_session() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let cookie_val = cookie.split(';').next().unwrap();

    // Logout.
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/logout")
                .header(header::COOKIE, cookie_val)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Subsequent authenticated request should be rejected.
    let resp = app
        .oneshot(helpers::authed_get("/api/lights", cookie_val))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
