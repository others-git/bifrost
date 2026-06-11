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

#[tokio::test]
async fn health_reports_version_and_uptime() {
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
    let body = helpers::response_json(resp).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert!(body["uptime_secs"].is_u64());
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

// ── Hue pairing ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn hue_pair_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/providers/hue/pair")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"bridge_ip":"192.168.1.10"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn hue_pair_returns_app_key_when_bridge_accepts() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let bridge = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "success": { "username": "paired-key-123", "clientkey": "deadbeef" } }
        ])))
        .mount(&bridge)
        .await;

    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"bridge_ip":"{}"}}"#, bridge.uri());
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/providers/hue/pair",
            &cookie,
            &body,
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["app_key"], "paired-key-123");
}

#[tokio::test]
async fn hue_pair_returns_409_when_link_button_not_pressed() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let bridge = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "error": { "type": 101, "address": "", "description": "link button not pressed" } }
        ])))
        .mount(&bridge)
        .await;

    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"bridge_ip":"{}"}}"#, bridge.uri());
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/providers/hue/pair",
            &cookie,
            &body,
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["error"], "link_button_not_pressed");
}

#[tokio::test]
async fn hue_pair_returns_502_when_bridge_unreachable() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Nothing listens on port 9.
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/providers/hue/pair",
            &cookie,
            r#"{"bridge_ip":"127.0.0.1:9"}"#,
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
}

// ── Scenes ───────────────────────────────────────────────────────────────────

async fn wled_mock() -> wiremock::MockServer {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/json/state"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"on": true})))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn scenes_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/scenes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_scene_snapshots_current_light_states() {
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/scenes",
            &cookie,
            r#"{"name":"Movie Night"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["lights"], 1);

    let resp = app
        .oneshot(helpers::authed_get("/api/scenes", &cookie))
        .await
        .unwrap();
    let scenes = helpers::response_json(resp).await;
    assert_eq!(scenes[0]["name"], "Movie Night");
    assert_eq!(scenes[0]["lights"], 1);
}

#[tokio::test]
async fn create_scene_rejects_empty_name() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/scenes",
            &cookie,
            r#"{"name":"  "}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn activate_scene_applies_states_via_provider() {
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/scenes",
            &cookie,
            r#"{"name":"Evening"}"#,
        ))
        .await
        .unwrap();
    let scene_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .oneshot(helpers::authed_post(
            &format!("/api/scenes/{scene_id}/activate"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["applied"], 1);
    assert_eq!(body["failed"], 0);

    // The provider actually received the state write.
    let requests = bridge.received_requests().await.unwrap();
    assert!(
        requests.iter().any(|r| r.url.path() == "/json/state"),
        "no set_state call reached the device"
    );
}

#[tokio::test]
async fn activate_unknown_scene_returns_404() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/scenes/nope/activate",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_scene_removes_it() {
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/scenes",
            &cookie,
            r#"{"name":"Temp"}"#,
        ))
        .await
        .unwrap();
    let scene_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(helpers::authed_delete(
            &format!("/api/scenes/{scene_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get("/api/scenes", &cookie))
        .await
        .unwrap();
    let scenes = helpers::response_json(resp).await;
    assert_eq!(scenes, serde_json::json!([]));
}

// ── Groups ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn groups_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/groups")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_group_with_members_and_list() {
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"name":"Living Room","light_ids":["{light_id}"]}}"#);
    let resp = app
        .clone()
        .oneshot(helpers::authed_post("/api/groups", &cookie, &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .oneshot(helpers::authed_get("/api/groups", &cookie))
        .await
        .unwrap();
    let groups = helpers::response_json(resp).await;
    assert_eq!(groups[0]["name"], "Living Room");
    assert_eq!(groups[0]["light_ids"][0], light_id);
}

#[tokio::test]
async fn create_group_rejects_empty_name() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/groups",
            &cookie,
            r#"{"name":""}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn set_group_state_applies_to_all_members() {
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"name":"All","light_ids":["{light_id}"]}}"#);
    let resp = app
        .clone()
        .oneshot(helpers::authed_post("/api/groups", &cookie, &body))
        .await
        .unwrap();
    let group_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/groups/{group_id}/state"),
            &cookie,
            r#"{"on":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["applied"], 1);
    assert_eq!(body["failed"], 0);

    let requests = bridge.received_requests().await.unwrap();
    assert!(requests.iter().any(|r| r.url.path() == "/json/state"));
}

#[tokio::test]
async fn set_group_state_unknown_group_returns_404() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/groups/nope/state",
            &cookie,
            r#"{"on":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn set_members_replaces_membership() {
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"name":"G","light_ids":["{light_id}"]}}"#);
    let resp = app
        .clone()
        .oneshot(helpers::authed_post("/api/groups", &cookie, &body))
        .await
        .unwrap();
    let group_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Replace membership with the empty set.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/groups/{group_id}/lights"),
            &cookie,
            r#"{"light_ids":[]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get("/api/groups", &cookie))
        .await
        .unwrap();
    let groups = helpers::response_json(resp).await;
    assert_eq!(groups[0]["light_ids"], serde_json::json!([]));
}

#[tokio::test]
async fn delete_group_removes_it() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/groups",
            &cookie,
            r#"{"name":"Temp"}"#,
        ))
        .await
        .unwrap();
    let group_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(helpers::authed_delete(
            &format!("/api/groups/{group_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get("/api/groups", &cookie))
        .await
        .unwrap();
    assert_eq!(helpers::response_json(resp).await, serde_json::json!([]));
}

// ── Floor plans ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn plans_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/plans")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_plan_and_list() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/plans",
            &cookie,
            r#"{"name":"Ground Floor","width":50,"height":40}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .oneshot(helpers::authed_get("/api/plans", &cookie))
        .await
        .unwrap();
    let plans = helpers::response_json(resp).await;
    assert_eq!(plans[0]["name"], "Ground Floor");
    assert_eq!(plans[0]["width"], 50);
    assert_eq!(plans[0]["height"], 40);
    assert_eq!(plans[0]["lights"], 0);
}

#[tokio::test]
async fn create_plan_rejects_bad_dimensions() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    for body in [
        r#"{"name":"Bad","width":0,"height":40}"#,
        r#"{"name":"Bad","width":50,"height":129}"#,
    ] {
        let resp = app
            .clone()
            .oneshot(helpers::authed_post("/api/plans", &cookie, body))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "body: {body}"
        );
    }
}

#[tokio::test]
async fn layout_roundtrips_tiles_and_walls() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/plans",
            &cookie,
            r#"{"name":"P","width":10,"height":10}"#,
        ))
        .await
        .unwrap();
    let plan_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Two floor tiles; an 'h' wall on the top edge of (0,0); a 'v' wall on the
    // far right boundary (x == width is legal for 'v').
    let layout = r#"{
        "tiles": [[0,0],[1,0]],
        "walls": [{"x":0,"y":0,"dir":"h"},{"x":10,"y":3,"dir":"v"}]
    }"#;
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/layout"),
            &cookie,
            layout,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get(
            &format!("/api/plans/{plan_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    let plan = helpers::response_json(resp).await;
    assert_eq!(plan["tiles"].as_array().unwrap().len(), 2);
    assert_eq!(plan["walls"].as_array().unwrap().len(), 2);
    assert_eq!(plan["walls"][1]["x"], 10);
    assert_eq!(plan["walls"][1]["dir"], "v");
}

#[tokio::test]
async fn layout_rejects_out_of_bounds_tile() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/plans",
            &cookie,
            r#"{"name":"P","width":10,"height":10}"#,
        ))
        .await
        .unwrap();
    let plan_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/layout"),
            &cookie,
            r#"{"tiles":[[10,0]],"walls":[]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn lights_placement_roundtrips_with_mount() {
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/plans",
            &cookie,
            r#"{"name":"P","width":10,"height":10}"#,
        ))
        .await
        .unwrap();
    let plan_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let body = format!(r#"{{"placements":[{{"light_id":"{light_id}","x":3,"y":4,"mount":"n"}}]}}"#);
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/lights"),
            &cookie,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get(
            &format!("/api/plans/{plan_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    let plan = helpers::response_json(resp).await;
    assert_eq!(plan["lights"][0]["light_id"], light_id);
    assert_eq!(plan["lights"][0]["x"], 3);
    assert_eq!(plan["lights"][0]["mount"], "n");
}

#[tokio::test]
async fn lights_placement_rejects_unknown_light() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/plans",
            &cookie,
            r#"{"name":"P","width":10,"height":10}"#,
        ))
        .await
        .unwrap();
    let plan_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/lights"),
            &cookie,
            r#"{"placements":[{"light_id":"ghost","x":1,"y":1,"mount":"c"}]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn get_unknown_plan_returns_404() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_get("/api/plans/nope", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_plan_cascades_children() {
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/plans",
            &cookie,
            r#"{"name":"P","width":10,"height":10}"#,
        ))
        .await
        .unwrap();
    let plan_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let _ = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/layout"),
            &cookie,
            r#"{"tiles":[[0,0]],"walls":[{"x":0,"y":0,"dir":"h"}]}"#,
        ))
        .await
        .unwrap();
    let body = format!(r#"{{"placements":[{{"light_id":"{light_id}","x":0,"y":0,"mount":"c"}}]}}"#);
    let _ = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/lights"),
            &cookie,
            &body,
        ))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(helpers::authed_delete(
            &format!("/api/plans/{plan_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get(
            &format!("/api/plans/{plan_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── Import provider groups ───────────────────────────────────────────────────

#[tokio::test]
async fn import_groups_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/providers/some-id/import-groups")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn import_groups_creates_local_groups_from_hue_rooms() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let bridge = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/clip/v2/resource/room"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{
                "id": "room-1",
                "metadata": {"name": "Living Room"},
                "children": [{"rid": "dev-1", "rtype": "device"}]
            }]
        })))
        .mount(&bridge)
        .await;
    Mock::given(method("GET"))
        .and(path("/clip/v2/resource/device"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"id": "dev-1", "services": [{"rid": "light-1", "rtype": "light"}]}]
        })))
        .mount(&bridge)
        .await;
    Mock::given(method("GET"))
        .and(path("/clip/v2/resource/zone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
        .mount(&bridge)
        .await;

    let (app, light_id) = helpers::test_app_with_hue_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers/prov-hue-1/import-groups",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["imported"], 1);
    assert_eq!(body["found"], 1);

    // The local group exists with the matched light as its member.
    let resp = app
        .oneshot(helpers::authed_get("/api/groups", &cookie))
        .await
        .unwrap();
    let groups = helpers::response_json(resp).await;
    assert_eq!(groups[0]["name"], "Living Room");
    assert_eq!(groups[0]["light_ids"][0], light_id);
}

#[tokio::test]
async fn import_groups_reimport_updates_membership_without_duplicates() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let bridge = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/clip/v2/resource/room"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{
                "id": "room-1",
                "metadata": {"name": "Living Room"},
                "children": [{"rid": "dev-1", "rtype": "device"}]
            }]
        })))
        .mount(&bridge)
        .await;
    Mock::given(method("GET"))
        .and(path("/clip/v2/resource/device"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"id": "dev-1", "services": [{"rid": "light-1", "rtype": "light"}]}]
        })))
        .mount(&bridge)
        .await;
    Mock::given(method("GET"))
        .and(path("/clip/v2/resource/zone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
        .mount(&bridge)
        .await;

    let (app, _light_id) = helpers::test_app_with_hue_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    for _ in 0..2 {
        let resp = app
            .clone()
            .oneshot(helpers::authed_post(
                "/api/providers/prov-hue-1/import-groups",
                &cookie,
                "{}",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    let resp = app
        .oneshot(helpers::authed_get("/api/groups", &cookie))
        .await
        .unwrap();
    let groups = helpers::response_json(resp).await;
    assert_eq!(
        groups.as_array().unwrap().len(),
        1,
        "re-import duplicated the group"
    );
    assert_eq!(groups[0]["light_ids"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn import_groups_unknown_provider_returns_404() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/providers/nope/import-groups",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── Update provider credentials ──────────────────────────────────────────────

#[tokio::test]
async fn update_credentials_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/providers/some-id/credentials")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"credentials":{}}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn update_credentials_takes_effect_for_subsequent_requests() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Provider initially points at old_device; credentials are then updated
    // to point at new_device. Discovery must hit the NEW device.
    let old_device = wled_mock().await;
    let new_device = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/json/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "name": "Replacement Strip"
        })))
        .mount(&new_device)
        .await;
    Mock::given(method("GET"))
        .and(path("/json/state"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "on": true, "bri": 128
        })))
        .mount(&new_device)
        .await;

    let (app, _light_id) = helpers::test_app_with_light(&old_device.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(
        r#"{{"credentials":{{"device_ip":"{}"}}}}"#,
        new_device.uri()
    );
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/providers/prov-test-1/credentials",
            &cookie,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_post(
            "/api/providers/prov-test-1/discover",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let requests = new_device.received_requests().await.unwrap();
    assert!(
        requests.iter().any(|r| r.url.path() == "/json/info"),
        "discovery did not use the updated credentials"
    );
}

#[tokio::test]
async fn update_credentials_rejects_invalid_shape() {
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Missing device_ip — the smoke build must reject it.
    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/providers/prov-test-1/credentials",
            &cookie,
            r#"{"credentials":{}}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn update_credentials_unknown_provider_returns_404() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/providers/nope/credentials",
            &cookie,
            r#"{"credentials":{"device_ip":"10.0.0.1"}}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── Plan rooms ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn rooms_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/plans/some-id/rooms")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"rooms":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Full room lifecycle: create a room over a placed light → auto-group with
/// that light; move the room away → membership empties; remove the room →
/// group disappears.
#[tokio::test]
async fn room_auto_group_follows_room_and_placements() {
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Plan with a light placed at (2, 2).
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/plans",
            &cookie,
            r#"{"name":"P","width":10,"height":10}"#,
        ))
        .await
        .unwrap();
    let plan_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let body = format!(r#"{{"placements":[{{"light_id":"{light_id}","x":2,"y":2,"mount":"c"}}]}}"#);
    let _ = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/lights"),
            &cookie,
            &body,
        ))
        .await
        .unwrap();

    // Room covering (2,2).
    let rooms = r#"{"rooms":[{"id":"","name":"Office","tiles":[[2,2],[2,3],[3,2],[3,3]]}]}"#;
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/rooms"),
            &cookie,
            rooms,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The auto-group exists with the placed light.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/groups", &cookie))
        .await
        .unwrap();
    let groups = helpers::response_json(resp).await;
    assert_eq!(groups[0]["name"], "Office");
    assert_eq!(groups[0]["light_ids"][0], light_id);

    // GET plan returns the room with its group linkage.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            &format!("/api/plans/{plan_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    let plan = helpers::response_json(resp).await;
    let room_id = plan["rooms"][0]["id"].as_str().unwrap().to_string();
    let group_id = plan["rooms"][0]["group_id"].as_str().unwrap().to_string();
    assert_eq!(plan["rooms"][0]["name"], "Office");

    // Move the room away from the light → group membership empties,
    // same group id (stable linkage).
    let rooms = format!(r#"{{"rooms":[{{"id":"{room_id}","name":"Office","tiles":[[8,8]]}}]}}"#);
    let _ = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/rooms"),
            &cookie,
            &rooms,
        ))
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/groups", &cookie))
        .await
        .unwrap();
    let groups = helpers::response_json(resp).await;
    assert_eq!(groups[0]["id"], group_id);
    assert_eq!(groups[0]["light_ids"], serde_json::json!([]));

    // Remove the room entirely → its group is deleted.
    let _ = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/rooms"),
            &cookie,
            r#"{"rooms":[]}"#,
        ))
        .await
        .unwrap();
    let resp = app
        .oneshot(helpers::authed_get("/api/groups", &cookie))
        .await
        .unwrap();
    assert_eq!(helpers::response_json(resp).await, serde_json::json!([]));
}

#[tokio::test]
async fn placing_a_light_updates_room_group_membership() {
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/plans",
            &cookie,
            r#"{"name":"P","width":10,"height":10}"#,
        ))
        .await
        .unwrap();
    let plan_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Room first, no lights placed yet.
    let _ = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/rooms"),
            &cookie,
            r#"{"rooms":[{"id":"","name":"Bedroom","tiles":[[5,5]]}]}"#,
        ))
        .await
        .unwrap();

    // Now place the light inside the room → membership updates via PUT lights.
    let body = format!(r#"{{"placements":[{{"light_id":"{light_id}","x":5,"y":5,"mount":"c"}}]}}"#);
    let _ = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/lights"),
            &cookie,
            &body,
        ))
        .await
        .unwrap();

    let resp = app
        .oneshot(helpers::authed_get("/api/groups", &cookie))
        .await
        .unwrap();
    let groups = helpers::response_json(resp).await;
    assert_eq!(groups[0]["light_ids"][0], light_id);
}

#[tokio::test]
async fn scene_activation_scoped_to_light_ids() {
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/scenes",
            &cookie,
            r#"{"name":"S"}"#,
        ))
        .await
        .unwrap();
    let scene_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Scoped to a different light: nothing applies.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/scenes/{scene_id}/activate"),
            &cookie,
            r#"{"light_ids":["someone-else"]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["applied"], 0);

    // Scoped to our light: applies.
    let resp = app
        .oneshot(helpers::authed_post(
            &format!("/api/scenes/{scene_id}/activate"),
            &cookie,
            &format!(r#"{{"light_ids":["{light_id}"]}}"#),
        ))
        .await
        .unwrap();
    let body = helpers::response_json(resp).await;
    assert_eq!(body["applied"], 1);
}

// ── Group scenes (palette scenes) ────────────────────────────────────────────

#[tokio::test]
async fn group_scenes_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/groups/g1/scenes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_and_list_group_scene() {
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"name":"G","light_ids":["{light_id}"]}}"#);
    let resp = app
        .clone()
        .oneshot(helpers::authed_post("/api/groups", &cookie, &body))
        .await
        .unwrap();
    let group_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/groups/{group_id}/scenes"),
            &cookie,
            r##"{"name":"Sunset","brightness":60,"palette":["#ff7d33","#ffb04d"]}"##,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .oneshot(helpers::authed_get(
            &format!("/api/groups/{group_id}/scenes"),
            &cookie,
        ))
        .await
        .unwrap();
    let scenes = helpers::response_json(resp).await;
    assert_eq!(scenes[0]["name"], "Sunset");
    assert_eq!(scenes[0]["brightness"], 60.0);
    assert_eq!(scenes[0]["palette"][0], "#ff7d33");
}

#[tokio::test]
async fn create_group_scene_rejects_bad_palette() {
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"name":"G","light_ids":["{light_id}"]}}"#);
    let resp = app
        .clone()
        .oneshot(helpers::authed_post("/api/groups", &cookie, &body))
        .await
        .unwrap();
    let group_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    for bad in [
        r#"{"name":"X","palette":["red"]}"#,
        r#"{"name":"X","brightness":150,"palette":[]}"#,
        r#"{"name":"  ","palette":[]}"#,
    ] {
        let resp = app
            .clone()
            .oneshot(helpers::authed_post(
                &format!("/api/groups/{group_id}/scenes"),
                &cookie,
                bad,
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "body: {bad}"
        );
    }
}

#[tokio::test]
async fn apply_group_scene_distributes_palette_across_lights() {
    let device = wled_mock().await;
    let (app, light_a, light_b) = helpers::test_app_with_two_lights(&device.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"name":"Room","light_ids":["{light_a}","{light_b}"]}}"#);
    let resp = app
        .clone()
        .oneshot(helpers::authed_post("/api/groups", &cookie, &body))
        .await
        .unwrap();
    let group_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/groups/{group_id}/scenes"),
            &cookie,
            r##"{"name":"Duo","brightness":50,"palette":["#ff0000","#0000ff"]}"##,
        ))
        .await
        .unwrap();
    let scene_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .oneshot(helpers::authed_post(
            &format!("/api/groups/{group_id}/scenes/{scene_id}/apply"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let result = helpers::response_json(resp).await;
    assert_eq!(result["applied"], 2);
    assert_eq!(result["failed"], 0);

    // Two set_state calls, each with a *different* colour from the palette.
    let requests = device.received_requests().await.unwrap();
    let bodies: Vec<serde_json::Value> = requests
        .iter()
        .filter(|r| r.url.path() == "/json/state" && r.method.as_str() == "POST")
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(bodies.len(), 2);
    let col0 = bodies[0]["seg"][0]["col"][0].clone();
    let col1 = bodies[1]["seg"][0]["col"][0].clone();
    assert!(
        col0.is_array() && col1.is_array(),
        "colours missing in device writes"
    );
    assert_ne!(
        col0, col1,
        "palette was not distributed — both lights got the same colour"
    );
}

#[tokio::test]
async fn apply_unknown_group_scene_returns_404() {
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"name":"G","light_ids":["{light_id}"]}}"#);
    let resp = app
        .clone()
        .oneshot(helpers::authed_post("/api/groups", &cookie, &body))
        .await
        .unwrap();
    let group_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .oneshot(helpers::authed_post(
            &format!("/api/groups/{group_id}/scenes/nope/apply"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_group_scene_removes_it() {
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"name":"G","light_ids":["{light_id}"]}}"#);
    let resp = app
        .clone()
        .oneshot(helpers::authed_post("/api/groups", &cookie, &body))
        .await
        .unwrap();
    let group_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/groups/{group_id}/scenes"),
            &cookie,
            r#"{"name":"Tmp","palette":[]}"#,
        ))
        .await
        .unwrap();
    let scene_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(helpers::authed_delete(
            &format!("/api/groups/{group_id}/scenes/{scene_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get(
            &format!("/api/groups/{group_id}/scenes"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(helpers::response_json(resp).await, serde_json::json!([]));
}
