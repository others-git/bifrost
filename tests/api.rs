mod helpers;

use axum::Router;
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
async fn strip_placement_roundtrips_cornered_polyline() {
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

    // An L-shaped run: along the top, then down the right side.
    let body = format!(
        r#"{{"placements":[{{"light_id":"{light_id}","x":2,"y":3,"mount":"n","points":[[7,3],[7,8]]}}]}}"#
    );
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
    assert_eq!(plan["lights"][0]["x"], 2);
    assert_eq!(
        plan["lights"][0]["points"],
        serde_json::json!([[7, 3], [7, 8]])
    );
}

#[tokio::test]
async fn strip_placement_rejects_out_of_bounds_vertex() {
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

    let body = format!(
        r#"{{"placements":[{{"light_id":"{light_id}","x":2,"y":3,"mount":"c","points":[[5,3],[10,3]]}}]}}"#
    );
    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/lights"),
            &cookie,
            &body,
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

#[tokio::test]
async fn resize_plan_updates_dimensions_and_prunes_outside_content() {
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

    // Content both inside and outside the future 6x6 bounds: tile (8,8) and
    // the 'v' wall at the old far boundary (x == 10) must be pruned; the
    // strip starting in bounds at (0,0) but running to (8,8) too.
    let _ = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/layout"),
            &cookie,
            r#"{"tiles":[[0,0],[8,8]],"walls":[{"x":0,"y":0,"dir":"h"},{"x":10,"y":3,"dir":"v"}]}"#,
        ))
        .await
        .unwrap();
    let body = format!(
        r#"{{"placements":[{{"light_id":"{light_id}","x":0,"y":0,"mount":"c","points":[[8,8]]}}]}}"#
    );
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
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/size"),
            &cookie,
            r#"{"width":6,"height":6}"#,
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
    assert_eq!(plan["width"], 6);
    assert_eq!(plan["height"], 6);
    assert_eq!(plan["tiles"].as_array().unwrap().len(), 1);
    assert_eq!(plan["walls"].as_array().unwrap().len(), 1);
    assert_eq!(plan["walls"][0]["dir"], "h");
    assert_eq!(plan["lights"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn resize_plan_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/plans/some-id/size")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"width":6,"height":6}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn resize_plan_rejects_bad_dimensions() {
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

    for body in [r#"{"width":0,"height":6}"#, r#"{"width":6,"height":129}"#] {
        let resp = app
            .clone()
            .oneshot(helpers::authed_json(
                "PUT",
                &format!("/api/plans/{plan_id}/size"),
                &cookie,
                body,
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "body: {body}"
        );
    }
}

// ── Import provider groups ───────────────────────────────────────────────────

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

// ── Rooms ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn rooms_endpoint_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/rooms")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_room_with_direct_lights_and_list() {
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"name":"Office","light_ids":["{light_id}"]}}"#);
    let resp = app
        .clone()
        .oneshot(helpers::authed_post("/api/rooms", &cookie, &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    assert_eq!(rooms[0]["name"], "Office");
    assert_eq!(rooms[0]["light_ids"][0], light_id);
    assert_eq!(rooms[0]["direct_light_ids"][0], light_id);
    assert_eq!(rooms[0]["links"], serde_json::json!([]));
}

#[tokio::test]
async fn create_room_rejects_empty_name() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":" "}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn set_room_state_fans_out_to_direct_members() {
    let device = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&device.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"name":"R","light_ids":["{light_id}"]}}"#);
    let resp = app
        .clone()
        .oneshot(helpers::authed_post("/api/rooms", &cookie, &body))
        .await
        .unwrap();
    let room_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/state"),
            &cookie,
            r#"{"on":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let result = helpers::response_json(resp).await;
    assert_eq!(result["applied"], 1);

    let requests = device.received_requests().await.unwrap();
    assert!(requests.iter().any(|r| r.url.path() == "/json/state"));
}

#[tokio::test]
async fn set_room_state_unknown_room_returns_404() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/rooms/nope/state",
            &cookie,
            r#"{"on":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn set_direct_lights_replaces_membership() {
    let device = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&device.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"name":"R","light_ids":["{light_id}"]}}"#);
    let resp = app
        .clone()
        .oneshot(helpers::authed_post("/api/rooms", &cookie, &body))
        .await
        .unwrap();
    let room_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/lights"),
            &cookie,
            r#"{"light_ids":[]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    assert_eq!(rooms[0]["light_ids"], serde_json::json!([]));
}

#[tokio::test]
async fn delete_room_removes_it() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"Tmp"}"#,
        ))
        .await
        .unwrap();
    let room_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(helpers::authed_delete(
            &format!("/api/rooms/{room_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    assert_eq!(helpers::response_json(resp).await, serde_json::json!([]));
}

// ── Provider-group sync ──────────────────────────────────────────────────────

async fn hue_bridge_with_room(room_name: &str) -> wiremock::MockServer {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let bridge = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/clip/v2/resource/room"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{
                "id": "room-1",
                "metadata": {"name": room_name},
                "children": [{"rid": "dev-1", "rtype": "device"}],
                "services": [{"rid": "gl-1", "rtype": "grouped_light"}]
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
    bridge
}

#[tokio::test]
async fn sync_groups_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/providers/x/sync-groups")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn sync_creates_mirror_and_room_with_linked_members() {
    let bridge = hue_bridge_with_room("Living Room").await;
    let (app, light_id) = helpers::test_app_with_hue_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers/prov-hue-1/sync-groups",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["synced"], 1);
    assert_eq!(body["rooms_created"], 1);

    // Mirror exists.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/provider-groups", &cookie))
        .await
        .unwrap();
    let mirrors = helpers::response_json(resp).await;
    assert_eq!(mirrors[0]["name"], "Living Room");
    assert_eq!(mirrors[0]["light_ids"][0], light_id);

    // Room exists with membership flowing through the link.
    let resp = app
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    assert_eq!(rooms[0]["name"], "Living Room");
    assert_eq!(rooms[0]["light_ids"][0], light_id);
    assert_eq!(rooms[0]["direct_light_ids"], serde_json::json!([]));
    assert_eq!(rooms[0]["links"][0]["name"], "Living Room");
}

#[tokio::test]
async fn sync_rename_follows_while_room_keeps_inherited_name() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    let bridge = hue_bridge_with_room("Office").await;
    let (app, _light_id) = helpers::test_app_with_hue_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let _ = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers/prov-hue-1/sync-groups",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();

    // The provider renames the room to "Studio".
    bridge.reset().await;
    Mock::given(method("GET"))
        .and(path("/clip/v2/resource/room"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{
                "id": "room-1",
                "metadata": {"name": "Studio"},
                "children": [{"rid": "dev-1", "rtype": "device"}],
                "services": [{"rid": "gl-1", "rtype": "grouped_light"}]
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

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers/prov-hue-1/sync-groups",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Rename followed; still exactly one room.
    let resp = app
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    assert_eq!(rooms.as_array().unwrap().len(), 1);
    assert_eq!(rooms[0]["name"], "Studio");
}

#[tokio::test]
async fn sync_links_existing_room_by_name_instead_of_duplicating() {
    let bridge = hue_bridge_with_room("Office").await;
    let (app, _light_id) = helpers::test_app_with_hue_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // The user already has a room called "Office" (e.g. from the planner).
    let _ = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"Office"}"#,
        ))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers/prov-hue-1/sync-groups",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    let body = helpers::response_json(resp).await;
    assert_eq!(body["rooms_created"], 0);
    assert_eq!(body["rooms_linked"], 1);

    let resp = app
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    assert_eq!(
        rooms.as_array().unwrap().len(),
        1,
        "sync duplicated the room"
    );
    assert_eq!(rooms[0]["links"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn sync_links_existing_room_case_insensitively() {
    let bridge = hue_bridge_with_room("Living Room").await;
    let (app, _light_id) = helpers::test_app_with_hue_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // User-created room differs only in case from the provider's group.
    let _ = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"Living room"}"#,
        ))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers/prov-hue-1/sync-groups",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    let body = helpers::response_json(resp).await;
    assert_eq!(
        body["rooms_created"], 0,
        "case-mismatch duplicated the room"
    );
    assert_eq!(body["rooms_linked"], 1);

    let resp = app
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    assert_eq!(rooms.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn create_room_rejects_case_insensitive_duplicate_name() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"Office"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"office"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn merge_rooms_moves_members_and_deletes_source() {
    let server = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&server.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Target (empty) and source (owns the light).
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"Living Room"}"#,
        ))
        .await
        .unwrap();
    let target_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            &format!(r#"{{"name":"Living room 2","light_ids":["{light_id}"]}}"#),
        ))
        .await
        .unwrap();
    let source_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Merge source into target.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/rooms/{target_id}/merge"),
            &cookie,
            &format!(r#"{{"source_room_id":"{source_id}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Source gone; target owns the light.
    let resp = app
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    assert_eq!(rooms.as_array().unwrap().len(), 1);
    assert_eq!(rooms[0]["id"], target_id.as_str());
    assert_eq!(rooms[0]["light_ids"][0], light_id);
}

#[tokio::test]
async fn merge_room_into_itself_returns_422() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"Office"}"#,
        ))
        .await
        .unwrap();
    let id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .oneshot(helpers::authed_post(
            &format!("/api/rooms/{id}/merge"),
            &cookie,
            &format!(r#"{{"source_room_id":"{id}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn room_state_uses_native_group_control_when_fully_linked() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    let bridge = hue_bridge_with_room("Living Room").await;
    Mock::given(method("PUT"))
        .and(path("/clip/v2/resource/grouped_light/gl-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
        .mount(&bridge)
        .await;

    let (app, _light_id) = helpers::test_app_with_hue_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let _ = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers/prov-hue-1/sync-groups",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    let room_id = rooms[0]["id"].as_str().unwrap().to_string();

    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/state"),
            &cookie,
            r#"{"on":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let result = helpers::response_json(resp).await;
    assert_eq!(result["applied"], 1);

    let requests = bridge.received_requests().await.unwrap();
    let grouped_puts = requests
        .iter()
        .filter(|r| r.url.path() == "/clip/v2/resource/grouped_light/gl-1")
        .count();
    let per_light_puts = requests
        .iter()
        .filter(|r| r.url.path().starts_with("/clip/v2/resource/light/"))
        .count();
    assert_eq!(grouped_puts, 1, "expected one native grouped_light call");
    assert_eq!(per_light_puts, 0, "native path must not fan out per light");
}

#[tokio::test]
async fn single_color_scene_uses_native_group_control_when_fully_linked() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    let bridge = hue_bridge_with_room("Living Room").await;
    Mock::given(method("PUT"))
        .and(path("/clip/v2/resource/grouped_light/gl-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
        .mount(&bridge)
        .await;

    let (app, _light_id) = helpers::test_app_with_hue_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let _ = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers/prov-hue-1/sync-groups",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    let room_id = rooms[0]["id"].as_str().unwrap().to_string();

    // A single-color palette drives every member to the same state, so the
    // scene apply must collapse to one grouped_light call, not per-light PUTs.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/palette-scenes",
            &cookie,
            r##"{"name":"Warm","brightness":40,"palette":["#ff8800"]}"##,
        ))
        .await
        .unwrap();
    let scene_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .oneshot(helpers::authed_post(
            &format!("/api/rooms/{room_id}/scenes/{scene_id}/apply"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let result = helpers::response_json(resp).await;
    assert_eq!(result["applied"], 1);

    let requests = bridge.received_requests().await.unwrap();
    let grouped_puts = requests
        .iter()
        .filter(|r| r.url.path() == "/clip/v2/resource/grouped_light/gl-1")
        .count();
    let per_light_puts = requests
        .iter()
        .filter(|r| r.url.path().starts_with("/clip/v2/resource/light/"))
        .count();
    assert_eq!(
        grouped_puts, 1,
        "single-color scene must use one group call"
    );
    assert_eq!(
        per_light_puts, 0,
        "uniform scene must not fan out per light"
    );
}

// ── Palette scenes (global) ──────────────────────────────────────────────────

#[tokio::test]
async fn palette_scene_create_apply_distributes_palette() {
    let device = wled_mock().await;
    let (app, light_a, light_b) = helpers::test_app_with_two_lights(&device.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"name":"R","light_ids":["{light_a}","{light_b}"]}}"#);
    let resp = app
        .clone()
        .oneshot(helpers::authed_post("/api/rooms", &cookie, &body))
        .await
        .unwrap();
    let room_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Scenes are global: created without a room, then applied to one.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/palette-scenes",
            &cookie,
            r##"{"name":"Duo","brightness":50,"palette":["#ff0000","#0000ff"]}"##,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let scene_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/rooms/{room_id}/scenes/{scene_id}/apply"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let result = helpers::response_json(resp).await;
    assert_eq!(result["applied"], 2);

    let requests = device.received_requests().await.unwrap();
    let bodies: Vec<serde_json::Value> = requests
        .iter()
        .filter(|r| r.url.path() == "/json/state" && r.method.as_str() == "POST")
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(bodies.len(), 2);
    assert_ne!(
        bodies[0]["seg"][0]["col"][0], bodies[1]["seg"][0]["col"][0],
        "palette was not distributed"
    );

    // The global scene list returns it.
    let resp = app
        .oneshot(helpers::authed_get("/api/palette-scenes", &cookie))
        .await
        .unwrap();
    let scenes = helpers::response_json(resp).await;
    assert_eq!(scenes[0]["name"], "Duo");
}

#[tokio::test]
async fn palette_scene_rejects_bad_palette_and_brightness() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    for bad in [
        r#"{"name":"X","palette":["red"]}"#,
        r#"{"name":"X","brightness":150,"palette":[]}"#,
        r#"{"name":"  ","palette":[]}"#,
    ] {
        let resp = app
            .clone()
            .oneshot(helpers::authed_post("/api/palette-scenes", &cookie, bad))
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
async fn palette_scene_save_from_room_captures_lit_colors() {
    let device = wled_mock().await;
    let (app, light_a, light_b) = helpers::test_app_with_two_lights(&device.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"name":"R","light_ids":["{light_a}","{light_b}"]}}"#);
    let resp = app
        .clone()
        .oneshot(helpers::authed_post("/api/rooms", &cookie, &body))
        .await
        .unwrap();
    let room_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Drive one light to a known color so the room has something to capture.
    let _ = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_a}"),
            &cookie,
            r##"{"on":true,"brightness":60,"color":{"x":0.6,"y":0.35,"brightness":0.6}}"##,
        ))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/palette-scenes/from-room/{room_id}"),
            &cookie,
            r#"{"name":"Captured"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .oneshot(helpers::authed_get("/api/palette-scenes", &cookie))
        .await
        .unwrap();
    let scenes = helpers::response_json(resp).await;
    assert_eq!(scenes[0]["name"], "Captured");
    assert!(
        scenes[0]["brightness"].as_f64().unwrap() > 0.0,
        "captured scene should record a brightness"
    );
}

#[tokio::test]
async fn palette_scene_save_from_room_with_nothing_lit_is_422() {
    let device = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&device.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"name":"R","light_ids":["{light_id}"]}}"#);
    let resp = app
        .clone()
        .oneshot(helpers::authed_post("/api/rooms", &cookie, &body))
        .await
        .unwrap();
    let room_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Ensure the room is fully dark before capturing.
    let _ = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}"),
            &cookie,
            r#"{"on":false}"#,
        ))
        .await
        .unwrap();

    let resp = app
        .oneshot(helpers::authed_post(
            &format!("/api/palette-scenes/from-room/{room_id}"),
            &cookie,
            r#"{"name":"Nope"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn palette_scene_delete_removes_it() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/palette-scenes",
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
            &format!("/api/palette-scenes/{scene_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get("/api/palette-scenes", &cookie))
        .await
        .unwrap();
    assert_eq!(helpers::response_json(resp).await, serde_json::json!([]));
}

// ── Planner add-on-save ──────────────────────────────────────────────────────

#[tokio::test]
async fn planner_region_creates_room_and_adds_placed_lights_on_save() {
    let device = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&device.uri()).await;
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

    // Region first (creates and binds a Room).
    let _ = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/rooms"),
            &cookie,
            r#"{"rooms":[{"id":"","name":"Den","tiles":[[2,2]]}]}"#,
        ))
        .await
        .unwrap();

    // Place a light inside the region; saving placements adds it to the Room.
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

    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    assert_eq!(rooms[0]["name"], "Den");
    assert_eq!(rooms[0]["direct_light_ids"][0], light_id);

    // Add-on-save is additive: moving the light out does NOT remove it.
    let _ = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/lights"),
            &cookie,
            &format!(r#"{{"placements":[{{"light_id":"{light_id}","x":8,"y":8,"mount":"c"}}]}}"#),
        ))
        .await
        .unwrap();
    let resp = app
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    assert_eq!(
        rooms[0]["direct_light_ids"][0], light_id,
        "save must never remove members"
    );
}

#[tokio::test]
async fn planner_region_rename_renames_bound_room() {
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

    let _ = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/rooms"),
            &cookie,
            r#"{"rooms":[{"id":"pr-1","name":"Den","tiles":[[1,1]]}]}"#,
        ))
        .await
        .unwrap();

    let _ = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/rooms"),
            &cookie,
            r#"{"rooms":[{"id":"pr-1","name":"Study","tiles":[[1,1]]}]}"#,
        ))
        .await
        .unwrap();

    let resp = app
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    assert_eq!(
        rooms.as_array().unwrap().len(),
        1,
        "rename must not duplicate"
    );
    assert_eq!(rooms[0]["name"], "Study");
}

// ── Public API (/api/v1) + API keys ──────────────────────────────────────────

/// Mint an API key via the session-authenticated management endpoint and
/// return the one-time plaintext key.
async fn create_api_key(app: &Router, cookie: &str, name: &str) -> String {
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/api-keys",
            cookie,
            &format!(r#"{{"name":"{name}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = helpers::response_json(resp).await;
    body["key"].as_str().unwrap().to_string()
}

fn bearer_get(uri: &str, key: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .body(Body::empty())
        .unwrap()
}

fn bearer_json(method: &str, uri: &str, key: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn api_keys_management_requires_session() {
    let app = helpers::test_app_with_password().await;
    for (method, uri) in [("GET", "/api/api-keys"), ("POST", "/api/api-keys")] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":"x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

#[tokio::test]
async fn create_api_key_returns_key_once_and_lists_with_prefix() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/api-keys",
            &cookie,
            r#"{"name":"Home Assistant"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = helpers::response_json(resp).await;
    let key = body["key"].as_str().unwrap();
    assert!(key.starts_with("bfr_"));
    assert_eq!(body["prefix"].as_str().unwrap(), &key[..12]);

    // The list shows the prefix but never the full key.
    let resp = app
        .oneshot(helpers::authed_get("/api/api-keys", &cookie))
        .await
        .unwrap();
    let list = helpers::response_json(resp).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["name"], "Home Assistant");
    assert_eq!(list[0]["prefix"], &key[..12]);
    assert!(list[0].get("key").is_none(), "list must not leak the key");
}

#[tokio::test]
async fn create_api_key_rejects_empty_name() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/api-keys",
            &cookie,
            r#"{"name":"  "}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn revoke_api_key_invalidates_public_access() {
    let device = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&device.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "temp").await;

    // Works before revocation.
    let resp = app
        .clone()
        .oneshot(bearer_get("/api/v1/lights", &key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Find the key id and revoke it.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/api-keys", &cookie))
        .await
        .unwrap();
    let list = helpers::response_json(resp).await;
    let id = list[0]["id"].as_str().unwrap().to_string();
    let resp = app
        .clone()
        .oneshot(helpers::authed_delete(
            &format!("/api/api-keys/{id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Now rejected.
    let resp = app
        .oneshot(bearer_get("/api/v1/lights", &key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn v1_lights_without_key_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/lights")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // A well-formed but unknown key is also rejected.
    let resp = app
        .oneshot(bearer_get("/api/v1/lights", "bfr_deadbeef"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn v1_lists_and_gets_lights_with_valid_key() {
    let device = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&device.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "reader").await;

    let resp = app
        .clone()
        .oneshot(bearer_get("/api/v1/lights", &key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let lights = helpers::response_json(resp).await;
    assert_eq!(lights.as_array().unwrap().len(), 1);
    assert_eq!(lights[0]["id"], light_id);

    let resp = app
        .clone()
        .oneshot(bearer_get(&format!("/api/v1/lights/{light_id}"), &key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(helpers::response_json(resp).await["name"], "Test Light");

    let resp = app
        .oneshot(bearer_get("/api/v1/lights/nope", &key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn v1_set_light_state_drives_provider() {
    let device = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&device.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "writer").await;

    let resp = app
        .oneshot(bearer_json(
            "PUT",
            &format!("/api/v1/lights/{light_id}/state"),
            &key,
            r#"{"on":true,"brightness":42.0}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let requests = device.received_requests().await.unwrap();
    assert!(requests.iter().any(|r| r.url.path() == "/json/state"));
}

#[tokio::test]
async fn v1_rooms_state_and_scenes_full_flow() {
    let device = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&device.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "app").await;

    // Create a room (with the light) via the session API.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            &format!(r#"{{"name":"Den","light_ids":["{light_id}"]}}"#),
        ))
        .await
        .unwrap();
    let room_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Public: the room appears with its member.
    let resp = app
        .clone()
        .oneshot(bearer_get("/api/v1/rooms", &key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let rooms = helpers::response_json(resp).await;
    assert_eq!(rooms[0]["name"], "Den");
    assert_eq!(rooms[0]["light_ids"][0], light_id);

    // Public: set room state.
    let resp = app
        .clone()
        .oneshot(bearer_json(
            "PUT",
            &format!("/api/v1/rooms/{room_id}/state"),
            &key,
            r#"{"on":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(helpers::response_json(resp).await["applied"], 1);

    // Public: create a global scene, list it, apply it to the room, delete it.
    let resp = app
        .clone()
        .oneshot(bearer_json(
            "POST",
            "/api/v1/scenes",
            &key,
            r##"{"name":"Warm","brightness":40,"palette":["#ff8800"]}"##,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let scene_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(bearer_get("/api/v1/scenes", &key))
        .await
        .unwrap();
    let scenes = helpers::response_json(resp).await;
    assert_eq!(scenes.as_array().unwrap().len(), 1);
    assert_eq!(scenes[0]["name"], "Warm");

    let resp = app
        .clone()
        .oneshot(bearer_json(
            "POST",
            &format!("/api/v1/rooms/{room_id}/scenes/{scene_id}/apply"),
            &key,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(helpers::response_json(resp).await["applied"], 1);

    let resp = app
        .clone()
        .oneshot(bearer_json(
            "DELETE",
            &format!("/api/v1/scenes/{scene_id}"),
            &key,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn v1_scene_create_rejects_bad_color() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "app").await;

    let resp = app
        .oneshot(bearer_json(
            "POST",
            "/api/v1/scenes",
            &key,
            r#"{"name":"Bad","palette":["red"]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

// ── Audio devices (Onkyo provider through the full API stack) ─────────────────

mod audio_mock {
    use bifrost::providers::onkyo::{decode_packet, encode_packet};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    /// Loopback eISCP receiver: answers `…QSTN` from a scripted state table,
    /// echoes accepted commands, records everything it hears.
    pub async fn spawn(scripted: HashMap<&'static str, String>) -> (u16, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let recorded: Arc<Mutex<Vec<String>>> = Arc::default();
        let rec = Arc::clone(&recorded);

        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let scripted = scripted.clone();
                let rec = Arc::clone(&rec);
                tokio::spawn(async move {
                    let mut buf: Vec<u8> = Vec::new();
                    let mut chunk = [0u8; 1024];
                    loop {
                        let Ok(n) = sock.read(&mut chunk).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        while let Some((msg, consumed)) = decode_packet(&buf) {
                            buf.drain(..consumed);
                            if msg.len() < 3 {
                                continue;
                            }
                            rec.lock().await.push(msg.clone());
                            let (code, data) = msg.split_at(3);
                            let reply = if data == "QSTN" {
                                format!(
                                    "{code}{}",
                                    scripted.get(code).cloned().unwrap_or("N/A".into())
                                )
                            } else {
                                msg.clone()
                            };
                            let _ = sock.write_all(&encode_packet(&reply)).await;
                        }
                    }
                });
            }
        });

        (port, recorded)
    }

    pub fn receiver_state() -> HashMap<&'static str, String> {
        HashMap::from([
            ("PWR", "01".to_string()),
            ("MVL", "1E".to_string()), // 30
            ("AMT", "00".to_string()),
            ("SLI", "12".to_string()), // tv
        ])
    }
}

/// Add an Onkyo provider pointed at the loopback mock and run discovery.
/// Returns the audio device's Bifrost id.
async fn setup_onkyo(app: &Router, cookie: &str, port: u16) -> String {
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers",
            cookie,
            &format!(
                r#"{{"name":"AV","provider_type":"onkyo","credentials":{{"host":"127.0.0.1","port":{port}}}}}"#
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let provider_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/providers/{provider_id}/discover"),
            cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(helpers::response_json(resp).await["discovered"], 1);

    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/audio/devices", cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let devices = helpers::response_json(resp).await;
    devices[0]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn audio_routes_require_session() {
    let app = helpers::test_app_with_password().await;
    for (method, uri) in [
        ("GET", "/api/audio/devices"),
        ("GET", "/api/audio/devices/some-id"),
        ("PUT", "/api/audio/devices/some-id/state"),
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

#[tokio::test]
async fn provider_types_include_audio_domain() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .oneshot(helpers::authed_get("/api/providers/types", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let types = helpers::response_json(resp).await;
    let onkyo = types
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["provider_type"] == "onkyo")
        .expect("onkyo registered");
    assert_eq!(onkyo["kind"], "audio");
    let hue = types
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["provider_type"] == "hue")
        .unwrap();
    assert_eq!(hue["kind"], "light");
}

#[tokio::test]
async fn onkyo_discover_lists_device_with_live_state() {
    let (port, _) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let device_id = setup_onkyo(&app, &cookie, port).await;

    let resp = app
        .oneshot(helpers::authed_get(
            &format!("/api/audio/devices/{device_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let device = helpers::response_json(resp).await;
    assert_eq!(device["kind"], "receiver");
    assert_eq!(device["state"]["power"], true);
    assert_eq!(device["state"]["volume"], 30);
    assert_eq!(device["state"]["source"], "tv");
}

#[tokio::test]
async fn audio_set_state_drives_receiver_and_validates() {
    let (port, recorded) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let device_id = setup_onkyo(&app, &cookie, port).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{device_id}/state"),
            &cookie,
            r#"{"power":true,"volume":45,"source":"spotify"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let cmds = recorded.lock().await.clone();
    assert!(cmds.contains(&"PWR01".to_string()), "{cmds:?}");
    assert!(cmds.contains(&"MVL2D".to_string()), "45 = 0x2D: {cmds:?}");
    assert!(cmds.contains(&"SLI2B".to_string()), "{cmds:?}");
    assert!(cmds.contains(&"NSV0A0".to_string()), "{cmds:?}");

    // Unknown source → 422 with the offending name.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{device_id}/state"),
            &cookie,
            r#"{"source":"kazoo"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Unknown device → 404.
    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/audio/devices/nope/state",
            &cookie,
            r#"{"volume":10}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn v1_audio_requires_key_and_mirrors_session_routes() {
    let (port, recorded) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let device_id = setup_onkyo(&app, &cookie, port).await;
    let key = create_api_key(&app, &cookie, "audio-app").await;

    // No key → 401.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/audio/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // List + live get with key.
    let resp = app
        .clone()
        .oneshot(bearer_get("/api/v1/audio/devices", &key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let devices = helpers::response_json(resp).await;
    assert_eq!(devices.as_array().unwrap().len(), 1);

    let resp = app
        .clone()
        .oneshot(bearer_get(
            &format!("/api/v1/audio/devices/{device_id}"),
            &key,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(helpers::response_json(resp).await["state"]["volume"], 30);

    // Transport command through v1.
    let resp = app
        .oneshot(bearer_json(
            "PUT",
            &format!("/api/v1/audio/devices/{device_id}/state"),
            &key,
            r#"{"transport":"pause"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        recorded.lock().await.contains(&"NTCPAUSE".to_string()),
        "{:?}",
        recorded.lock().await
    );
}

// ── Room ↔ audio device link ──────────────────────────────────────────────────

#[tokio::test]
async fn room_audio_link_requires_session() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/rooms/some-id/audio")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"audio_device_id":"x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn room_audio_link_set_list_and_clear() {
    let (port, _) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let device_id = setup_onkyo(&app, &cookie, port).await;

    // A room to link.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"Den","light_ids":[]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let room_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Unknown device → 422; unknown room → 404.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/audio"),
            &cookie,
            r#"{"audio_device_id":"nope"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/rooms/nope/audio",
            &cookie,
            &format!(r#"{{"audio_device_id":"{device_id}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Link, and see it in both the session and v1 room listings.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/audio"),
            &cookie,
            &format!(r#"{{"audio_device_id":"{device_id}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    assert_eq!(rooms[0]["audio_device_id"], device_id);

    let key = create_api_key(&app, &cookie, "mcp").await;
    let resp = app
        .clone()
        .oneshot(bearer_get("/api/v1/rooms", &key))
        .await
        .unwrap();
    assert_eq!(
        helpers::response_json(resp).await[0]["audio_device_id"],
        device_id
    );

    // Clear with null.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/audio"),
            &cookie,
            r#"{"audio_device_id":null}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    assert_eq!(
        helpers::response_json(resp).await[0]["audio_device_id"],
        serde_json::Value::Null
    );
}

// ── Provider network auto-detect (POST /api/providers/scan/{type}) ────────────

#[tokio::test]
async fn provider_scan_requires_session() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/providers/scan/onkyo")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn provider_scan_unsupported_type_returns_404() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Govee is cloud (API key, no LAN IP) → no discoverer → 404.
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/providers/scan/govee",
            &cookie,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn provider_scan_supported_type_returns_device_array() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Onkyo supports discovery; nothing answers the broadcast in the test
    // environment, so the result is a (possibly empty) JSON array, never an error.
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/providers/scan/onkyo",
            &cookie,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(helpers::response_json(resp).await.is_array());
}

#[tokio::test]
async fn provider_types_flag_discovery_support() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .oneshot(helpers::authed_get("/api/providers/types", &cookie))
        .await
        .unwrap();
    let types = helpers::response_json(resp).await;
    let flag = |t: &str| {
        types
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["provider_type"] == t)
            .unwrap()["supports_discovery"]
            .as_bool()
            .unwrap()
    };
    // Every IP-addressable provider supports auto-detect.
    for t in ["onkyo", "sonos", "hue", "wled", "tasmota", "shelly"] {
        assert!(flag(t), "{t} should advertise auto-detect");
    }
    // Cloud providers (token, no LAN IP) do not.
    assert!(!flag("govee"), "govee is cloud — no auto-detect");
}

// ── Settings: Expanded-LAN scan subnets ───────────────────────────────────────

#[tokio::test]
async fn settings_require_session() {
    let app = helpers::test_app_with_password().await;
    for (method, body) in [("GET", None), ("PUT", Some(r#"{"expanded_lan_scan":[]}"#))] {
        let mut builder = Request::builder().method(method).uri("/api/settings");
        if body.is_some() {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        let resp = app
            .clone()
            .oneshot(builder.body(Body::from(body.unwrap_or(""))).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{method}");
    }
}

#[tokio::test]
async fn settings_roundtrip_normalises_private_subnets() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Defaults to empty.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/settings", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        helpers::response_json(resp).await["expanded_lan_scan"],
        serde_json::json!([])
    );

    // A host address is normalised to its /24 base.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/settings",
            &cookie,
            r#"{"expanded_lan_scan":["192.168.1.50","10.0.0.0/24"]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        helpers::response_json(resp).await["expanded_lan_scan"],
        serde_json::json!(["192.168.1.0/24", "10.0.0.0/24"])
    );

    // Persisted across reads.
    let resp = app
        .oneshot(helpers::authed_get("/api/settings", &cookie))
        .await
        .unwrap();
    assert_eq!(
        helpers::response_json(resp).await["expanded_lan_scan"],
        serde_json::json!(["192.168.1.0/24", "10.0.0.0/24"])
    );
}

#[tokio::test]
async fn settings_reject_public_subnet() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/settings",
            &cookie,
            r#"{"expanded_lan_scan":["8.8.8.0/24"]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
