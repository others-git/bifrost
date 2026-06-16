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

#[tokio::test]
async fn instance_endpoint_is_stable_within_a_process_and_unauthenticated() {
    let app = helpers::test_app().await;
    let get = || {
        app.clone().oneshot(
            Request::builder()
                .uri("/api/instance")
                .body(Body::empty())
                .unwrap(),
        )
    };

    let r1 = get().await.unwrap();
    assert_eq!(r1.status(), StatusCode::OK); // no auth required — kiosk polls pre-login
    let b1 = helpers::response_json(r1).await;
    assert_eq!(b1["version"], env!("CARGO_PKG_VERSION"));
    let id = b1["instance_id"].as_str().unwrap().to_string();
    assert!(!id.is_empty(), "instance_id should be a non-empty nonce");

    // Same process → same id, so a steady client never spuriously reloads.
    let b2 = helpers::response_json(get().await.unwrap()).await;
    assert_eq!(b2["instance_id"], id);
}

#[tokio::test]
async fn instance_id_differs_across_processes() {
    // A restart/redeploy mints a fresh id — the signal the kiosk reloads on.
    let a = helpers::response_json(
        helpers::test_app()
            .await
            .oneshot(
                Request::builder()
                    .uri("/api/instance")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let b = helpers::response_json(
        helpers::test_app()
            .await
            .oneshot(
                Request::builder()
                    .uri("/api/instance")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_ne!(a["instance_id"], b["instance_id"]);
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
                    r#"{"name":"Test","provider_type":"nonexistent_provider","credentials":{}}"#,
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
async fn audio_placement_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/plans/some-id/audio")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"placements":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn audio_placement_roundtrips_with_mount() {
    let (port, _) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let device_id = setup_onkyo(&app, &cookie, port).await;

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
        r#"{{"placements":[{{"audio_device_id":"{device_id}","x":2,"y":5,"mount":"e"}}]}}"#
    );
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/audio"),
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
    assert_eq!(plan["audio"][0]["audio_device_id"], device_id);
    assert_eq!(plan["audio"][0]["x"], 2);
    assert_eq!(plan["audio"][0]["mount"], "e");
}

#[tokio::test]
async fn audio_placement_rejects_unknown_device() {
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
            &format!("/api/plans/{plan_id}/audio"),
            &cookie,
            r#"{"placements":[{"audio_device_id":"ghost","x":1,"y":1,"mount":"c"}]}"#,
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

    // device_ip overwritten with a non-string — the merged creds fail the
    // smoke build, so the update is rejected.
    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/providers/prov-test-1/credentials",
            &cookie,
            r#"{"credentials":{"device_ip":123}}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn update_credentials_merges_blank_fields_keeping_stored_values() {
    // Submitting an empty string for a field must keep the stored value rather
    // than wipe it — this is how the edit form keeps secrets the user left
    // blank. WLED has only device_ip; a blank submission is a no-op, and the
    // stored value must survive (verified via the config endpoint).
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/providers/prov-test-1/credentials",
            &cookie,
            r#"{"credentials":{"device_ip":""}}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get(
            "/api/providers/prov-test-1/config",
            &cookie,
        ))
        .await
        .unwrap();
    let body = helpers::response_json(resp).await;
    assert_eq!(
        body["values"]["device_ip"],
        bridge.uri(),
        "blank update wiped the stored device_ip"
    );
}

// ── Provider config (prefill the edit form) ─────────────────────────────────

#[tokio::test]
async fn provider_config_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/providers/some-id/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn provider_config_unknown_provider_returns_404() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_get("/api/providers/nope/config", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn provider_config_returns_non_secret_values_for_prefill() {
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .oneshot(helpers::authed_get(
            "/api/providers/prov-test-1/config",
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["provider_type"], "wled");
    assert_eq!(body["decryptable"], true);
    // device_ip is non-secret, so it's returned to prefill the form.
    assert_eq!(body["values"]["device_ip"], bridge.uri());
}

#[tokio::test]
async fn provider_config_omits_secret_fields() {
    // Govee's only credential is the api_key (a password-kind secret). The
    // config endpoint must never echo it back to the client.
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "POST",
            "/api/providers",
            &cookie,
            r#"{"name":"Govee","provider_type":"govee","credentials":{"api_key":"super-secret"}}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = helpers::response_json(resp).await;
    let id = created["id"].as_str().unwrap();

    let resp = app
        .oneshot(helpers::authed_get(
            &format!("/api/providers/{id}/config"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["decryptable"], true);
    assert!(
        body["values"].get("api_key").is_none(),
        "secret api_key leaked in config response: {body}"
    );
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
async fn room_power_cycle_preserves_light_color() {
    // Regression: toggling a room off then on is a *pure power* command and must
    // not wipe the stored colour — the device keeps it across a power cycle, so
    // the UI must re-sync to the real colour, not a colourless default.
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

    // Drive the light to a known colour.
    let cresp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}"),
            &cookie,
            r##"{"on":true,"brightness":60,"color":{"x":0.64,"y":0.33,"brightness":0.6}}"##,
        ))
        .await
        .unwrap();
    assert_eq!(cresp.status(), StatusCode::NO_CONTENT);

    // Power the room off, then back on — both pure-power commands.
    for on in ["false", "true"] {
        let resp = app
            .clone()
            .oneshot(helpers::authed_json(
                "PUT",
                &format!("/api/rooms/{room_id}/state"),
                &cookie,
                &format!(r#"{{"on":{on}}}"#),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // The stored colour must survive the off→on cycle (and `on` is back to true).
    let resp = app
        .oneshot(helpers::authed_get("/api/lights", &cookie))
        .await
        .unwrap();
    let lights = helpers::response_json(resp).await;
    let light = lights
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == light_id.as_str())
        .expect("light present");
    assert_eq!(light["last_state"]["on"], true, "{light}");
    assert!(
        (light["last_state"]["color"]["x"].as_f64().unwrap() - 0.64).abs() < 1e-6,
        "colour was wiped by the power cycle: {light}"
    );
}

#[tokio::test]
async fn room_brightness_change_preserves_light_color() {
    // A room brightness change carries no colour, so it must scale brightness
    // while leaving each member light's own colour intact (e.g. a scene's). The
    // bug: the cascade used to broadcast one uniform colour on every change.
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

    // Known colour + brightness.
    let cresp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}"),
            &cookie,
            r##"{"on":true,"brightness":60,"color":{"x":0.64,"y":0.33,"brightness":0.6}}"##,
        ))
        .await
        .unwrap();
    assert_eq!(cresp.status(), StatusCode::NO_CONTENT);

    // Room brightness-only change (no colour in the body).
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/state"),
            &cookie,
            r#"{"on":true,"brightness":30}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(helpers::authed_get("/api/lights", &cookie))
        .await
        .unwrap();
    let lights = helpers::response_json(resp).await;
    let light = lights
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == light_id.as_str())
        .expect("light present");
    assert_eq!(
        light["last_state"]["brightness"].as_f64().unwrap(),
        30.0,
        "brightness not applied: {light}"
    );
    assert!(
        (light["last_state"]["color"]["x"].as_f64().unwrap() - 0.64).abs() < 1e-6,
        "colour was wiped by a brightness-only change: {light}"
    );
}

#[tokio::test]
async fn light_color_temp_change_clears_color() {
    // Colour and colour temperature are mutually exclusive: switching a light to
    // a white temperature must clear the cached colour, so the UI can tell from
    // `last_state` which mode the light is in.
    let device = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&device.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let cresp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}"),
            &cookie,
            r##"{"on":true,"color":{"x":0.64,"y":0.33,"brightness":0.6}}"##,
        ))
        .await
        .unwrap();
    assert_eq!(cresp.status(), StatusCode::NO_CONTENT);

    let tresp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}"),
            &cookie,
            r#"{"on":true,"color_temp_mirek":300}"#,
        ))
        .await
        .unwrap();
    assert_eq!(tresp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get("/api/lights", &cookie))
        .await
        .unwrap();
    let lights = helpers::response_json(resp).await;
    let light = lights
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == light_id.as_str())
        .expect("light present");
    assert_eq!(light["last_state"]["color_temp_mirek"], 300, "{light}");
    assert!(
        light["last_state"]["color"].is_null(),
        "colour not cleared when switching to white: {light}"
    );
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

#[tokio::test]
async fn set_room_enabled_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/rooms/some-id/enabled")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":false}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn disabled_room_stays_in_settings_list_but_is_hidden_from_v1() {
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"name":"Office","light_ids":["{light_id}"]}}"#);
    let resp = app
        .clone()
        .oneshot(helpers::authed_post("/api/rooms", &cookie, &body))
        .await
        .unwrap();
    let room_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Rooms default to enabled.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    assert_eq!(helpers::response_json(resp).await[0]["enabled"], true);

    // Disable it.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/enabled"),
            &cookie,
            r#"{"enabled":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Still listed (and flagged) for the session API / Settings.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    assert_eq!(rooms.as_array().unwrap().len(), 1);
    assert_eq!(rooms[0]["enabled"], false);

    // Hidden from the public API.
    let key = create_api_key(&app, &cookie, "k").await;
    let resp = app
        .oneshot(bearer_get("/api/v1/rooms", &key))
        .await
        .unwrap();
    assert_eq!(
        helpers::response_json(resp).await.as_array().unwrap().len(),
        0
    );
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
async fn light_reports_inherited_room_from_a_synced_group_link() {
    // A light that's in a room only via a synced provider-group link (no direct
    // assignment) must report that room as inherited_room_id, so the Devices page
    // shows its effective room instead of "No room".
    let bridge = hue_bridge_with_room("Living Room").await;
    let (app, light_id) = helpers::test_app_with_hue_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    app.clone()
        .oneshot(helpers::authed_post(
            "/api/providers/prov-hue-1/sync-groups",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();

    let rooms = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/rooms", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let room_id = rooms[0]["id"].as_str().unwrap().to_string();

    let lights = helpers::response_json(
        app.oneshot(helpers::authed_get("/api/lights", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let light = lights
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == light_id)
        .expect("light present");
    assert!(light["room_id"].is_null(), "no direct assignment");
    assert_eq!(
        light["inherited_room_id"], room_id,
        "effective room via the link"
    );
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

// ── Device enrollment (QR pairing) ───────────────────────────────────────────

fn anon_json(method: &str, uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn enrollment_create_requires_session() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(anon_json("POST", "/api/enrollment", "{}"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn enrollment_redeem_rejects_unknown_token() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(anon_json(
            "POST",
            "/api/enrollment/redeem",
            r#"{"token":"deadbeef","device_name":"x"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn enrollment_full_flow_mints_a_usable_revocable_key() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // 1. Authed session mints a pairing token (what the dashboard renders as a QR).
    let mint = app
        .clone()
        .oneshot(helpers::authed_post("/api/enrollment", &cookie, "{}"))
        .await
        .unwrap();
    assert_eq!(mint.status(), StatusCode::OK);
    let mint = helpers::response_json(mint).await;
    let token = mint["token"].as_str().unwrap().to_string();
    assert_eq!(mint["expires_in_secs"], 300);

    // 2. The headless device redeems it (no session) for a real key.
    let redeem = app
        .clone()
        .oneshot(anon_json(
            "POST",
            "/api/enrollment/redeem",
            &format!(r#"{{"token":"{token}","device_name":"Bedroom tablet"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(redeem.status(), StatusCode::CREATED);
    let redeem = helpers::response_json(redeem).await;
    let key = redeem["key"].as_str().unwrap().to_string();
    assert!(key.starts_with("bfr_"));

    // 3. The minted key actually authenticates the public API + voice seam.
    let v1 = app
        .clone()
        .oneshot(bearer_get("/api/v1/lights", &key))
        .await
        .unwrap();
    assert_eq!(v1.status(), StatusCode::OK);

    // 4. It's a normal key — listed in Settings under the device name, revocable.
    let list = app
        .clone()
        .oneshot(helpers::authed_get("/api/api-keys", &cookie))
        .await
        .unwrap();
    let keys = helpers::response_json(list).await;
    assert!(
        keys.as_array()
            .unwrap()
            .iter()
            .any(|k| k["name"] == "Bedroom tablet"),
        "enrolled key should appear in the key list: {keys}"
    );
}

#[tokio::test]
async fn enrollment_token_is_single_use() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let mint = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_post("/api/enrollment", &cookie, "{}"))
            .await
            .unwrap(),
    )
    .await;
    let token = mint["token"].as_str().unwrap().to_string();
    let redeem = |t: String| {
        app.clone().oneshot(anon_json(
            "POST",
            "/api/enrollment/redeem",
            &format!(r#"{{"token":"{t}"}}"#),
        ))
    };

    assert_eq!(
        redeem(token.clone()).await.unwrap().status(),
        StatusCode::CREATED
    );
    // Second redemption of the same token is rejected.
    assert_eq!(
        redeem(token).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

// ── Kiosk controller ─────────────────────────────────────────────────────────

#[tokio::test]
async fn kiosk_checkin_requires_api_key() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(anon_json("POST", "/api/kiosks/checkin", "{}"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn kiosk_list_and_command_require_session() {
    let app = helpers::test_app_with_password().await;
    for (method, uri) in [("GET", "/api/kiosks"), ("POST", "/api/kiosks/x/command")] {
        let resp = app
            .clone()
            .oneshot(anon_json(method, uri, r#"{"command":"sleep"}"#))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
}

#[tokio::test]
async fn kiosk_checkin_registers_and_command_is_delivered_once() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "Bedroom tablet").await;

    // First check-in registers the kiosk; nothing queued yet.
    let r = app
        .clone()
        .oneshot(bearer_json(
            "POST",
            "/api/kiosks/checkin",
            &key,
            r#"{"app_version":"0.1","screen_on":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert!(helpers::response_json(r).await["command"].is_null());

    // It shows up in the session-only clients view, online + authorized.
    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let kiosk = &list.as_array().unwrap()[0];
    assert_eq!(kiosk["name"], "Bedroom tablet");
    assert_eq!(kiosk["online"], true);
    assert_eq!(kiosk["authorized"], true);
    let kiosk_id = kiosk["id"].as_str().unwrap().to_string();

    // Queue a lock; the next check-in delivers it, the one after is clear.
    let q = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/kiosks/{kiosk_id}/command"),
            &cookie,
            r#"{"command":"lock"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(q.status(), StatusCode::NO_CONTENT);

    let r1 = helpers::response_json(
        app.clone()
            .oneshot(bearer_json("POST", "/api/kiosks/checkin", &key, "{}"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r1["command"], "lock", "command delivered on check-in");
    let r2 = helpers::response_json(
        app.clone()
            .oneshot(bearer_json("POST", "/api/kiosks/checkin", &key, "{}"))
            .await
            .unwrap(),
    )
    .await;
    assert!(r2["command"].is_null(), "command consumed (delivered once)");
}

#[tokio::test]
async fn kiosk_stream_requires_api_key() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(anon_json("GET", "/api/kiosks/stream", ""))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn kiosk_stream_is_not_found_until_registered() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "Bedroom tablet").await;
    // Authed with a real key, but the kiosk hasn't checked in yet — no row to
    // resolve, so the stream 404s and the app retries after a heartbeat.
    let resp = app
        .oneshot(bearer_json("GET", "/api/kiosks/stream", &key, ""))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn kiosk_command_is_pushed_to_the_live_stream() {
    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "Bedroom tablet").await;

    // Register the kiosk, then start listening on the push channel as the live
    // stream would.
    app.clone()
        .oneshot(bearer_json("POST", "/api/kiosks/checkin", &key, "{}"))
        .await
        .unwrap();
    let mut rx = state.kiosk_commands.subscribe();

    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let id = list.as_array().unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Queueing a command pushes it instantly to subscribers (not just the poll).
    let q = app
        .oneshot(helpers::authed_post(
            &format!("/api/kiosks/{id}/command"),
            &cookie,
            r#"{"command":"sleep"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(q.status(), StatusCode::NO_CONTENT);

    let pushed = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("command was not pushed within 1s")
        .expect("broadcast recv");
    assert_eq!(pushed.kiosk_id, id);
    assert_eq!(pushed.command, "sleep");
}

#[tokio::test]
async fn kiosk_room_assignment_flows_to_checkin_and_list() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "Bedroom tablet").await;

    // Register the kiosk and create a room.
    app.clone()
        .oneshot(bearer_json("POST", "/api/kiosks/checkin", &key, "{}"))
        .await
        .unwrap();
    app.clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"Bedroom"}"#,
        ))
        .await
        .unwrap();

    let rooms = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/rooms", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let room_id = rooms.as_array().unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let kiosks = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let kid = kiosks.as_array().unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Assign the room.
    let put = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{kid}/room"),
            &cookie,
            &format!(r#"{{"room_id":"{room_id}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::NO_CONTENT);

    // The list reflects the assignment, and check-in hands the kiosk the room name.
    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(list.as_array().unwrap()[0]["room_id"], room_id);

    let checkin = helpers::response_json(
        app.oneshot(bearer_json("POST", "/api/kiosks/checkin", &key, "{}"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(checkin["room"], "Bedroom");
}

#[tokio::test]
async fn kiosk_command_rejects_unknown_verb() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "tablet").await;
    app.clone()
        .oneshot(bearer_json("POST", "/api/kiosks/checkin", &key, "{}"))
        .await
        .unwrap();
    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let id = list.as_array().unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = app
        .oneshot(helpers::authed_post(
            &format!("/api/kiosks/{id}/command"),
            &cookie,
            r#"{"command":"explode"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn kiosk_deauth_revokes_the_key_and_marks_unauthorized() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "tablet").await;
    app.clone()
        .oneshot(bearer_json("POST", "/api/kiosks/checkin", &key, "{}"))
        .await
        .unwrap();
    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let id = list.as_array().unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let d = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/kiosks/{id}/deauth"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(d.status(), StatusCode::NO_CONTENT);

    // The revoked key no longer authenticates a check-in.
    let r = app
        .clone()
        .oneshot(bearer_json("POST", "/api/kiosks/checkin", &key, "{}"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // The kiosk row survives, now flagged as needing re-pair.
    let list = helpers::response_json(
        app.oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(list.as_array().unwrap()[0]["authorized"], false);
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
    // Well-formed bodies so a route's auth check (not body validation) is what
    // rejects the request.
    for (method, uri, body) in [
        ("GET", "/api/audio/devices", "{}"),
        ("GET", "/api/audio/devices/some-id", "{}"),
        ("PUT", "/api/audio/devices/some-id/state", "{}"),
        ("GET", "/api/audio/devices/some-id/favorites", "{}"),
        (
            "POST",
            "/api/audio/devices/some-id/favorites/play",
            r#"{"favorite_id":"x"}"#,
        ),
        (
            "POST",
            "/api/audio/devices/some-id/group",
            r#"{"coordinator_id":"x"}"#,
        ),
        ("POST", "/api/audio/devices/some-id/ungroup", "{}"),
        (
            "PUT",
            "/api/audio/devices/some-id/receiver",
            r#"{"receiver_id":"x"}"#,
        ),
        (
            "PUT",
            "/api/audio/devices/some-id/companion",
            r#"{"primary_id":"x"}"#,
        ),
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

/// Add an Onkyo provider pointed at `port`, discover it, and return the id of
/// the audio device that wasn't in `existing` (so two providers can be told
/// apart). Used by the M22 receiver-binding tests, which need two devices.
async fn add_onkyo_device(
    app: &Router,
    cookie: &str,
    port: u16,
    name: &str,
    existing: &[String],
) -> String {
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers",
            cookie,
            &format!(
                r#"{{"name":"{name}","provider_type":"onkyo","credentials":{{"host":"127.0.0.1","port":{port}}}}}"#
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
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/audio/devices", cookie))
        .await
        .unwrap();
    let devices = helpers::response_json(resp).await;
    devices
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap().to_string())
        .find(|id| !existing.contains(id))
        .expect("a newly-discovered device")
}

/// True if the recorded eISCP stream contains a master-volume *set* (an `MVL`
/// command that isn't the `MVLQSTN` read).
fn heard_volume_set(recorded: &[String]) -> bool {
    recorded
        .iter()
        .any(|m| m.starts_with("MVL") && !m.contains("QSTN"))
}

/// Count master-volume *sets* in the recorded eISCP stream.
fn volume_set_count(recorded: &[String]) -> usize {
    recorded
        .iter()
        .filter(|m| m.starts_with("MVL") && !m.contains("QSTN"))
        .count()
}

/// Poll a mock's recorded stream until a volume set lands (commands flow through
/// the shared link actor asynchronously, so they arrive a beat after the HTTP
/// response). Returns false if none arrives within ~2s.
async fn wait_for_volume_set(recorded: &std::sync::Arc<tokio::sync::Mutex<Vec<String>>>) -> bool {
    for _ in 0..40 {
        if heard_volume_set(&recorded.lock().await) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn audio_receiver_binding_crud_and_validation() {
    let (port_s, _) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let (port_r, _) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let source = add_onkyo_device(&app, &cookie, port_s, "Source", &[]).await;
    let receiver = add_onkyo_device(
        &app,
        &cookie,
        port_r,
        "Receiver",
        std::slice::from_ref(&source),
    )
    .await;

    // A device can't be its own receiver.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{source}/receiver"),
            &cookie,
            &format!(r#"{{"receiver_id":"{source}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // An unknown receiver id is rejected.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{source}/receiver"),
            &cookie,
            r#"{"receiver_id":"does-not-exist"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Bind source → receiver with an input to select.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{source}/receiver"),
            &cookie,
            &format!(r#"{{"receiver_id":"{receiver}","receiver_source":"Game"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            &format!("/api/audio/devices/{source}"),
            &cookie,
        ))
        .await
        .unwrap();
    let dev = helpers::response_json(resp).await;
    assert_eq!(dev["receiver_id"], receiver);
    assert_eq!(dev["receiver_source"], "Game");

    // Chaining is rejected: the receiver can't itself be bound to a bound device.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{receiver}/receiver"),
            &cookie,
            &format!(r#"{{"receiver_id":"{source}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Clearing the binding removes it (and its stored input).
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{source}/receiver"),
            &cookie,
            r#"{"receiver_id":null}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get(
            &format!("/api/audio/devices/{source}"),
            &cookie,
        ))
        .await
        .unwrap();
    let dev = helpers::response_json(resp).await;
    assert!(dev["receiver_id"].is_null());
    assert!(dev["receiver_source"].is_null());
}

#[tokio::test]
async fn audio_companion_link_crud_and_validation() {
    let (port_a, _) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let (port_b, _) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let primary = add_onkyo_device(&app, &cookie, port_a, "TV", &[]).await;
    let companion = add_onkyo_device(
        &app,
        &cookie,
        port_b,
        "TV speaker",
        std::slice::from_ref(&primary),
    )
    .await;

    // (401 for this route is covered by the parameterized unauth table above.)

    // A device can't be its own companion.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{companion}/companion"),
            &cookie,
            &format!(r#"{{"primary_id":"{companion}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Unknown primary is rejected.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{companion}/companion"),
            &cookie,
            r#"{"primary_id":"does-not-exist"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Merge companion → primary.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{companion}/companion"),
            &cookie,
            &format!(r#"{{"primary_id":"{primary}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The link is recorded on the companion.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/audio/devices", &cookie))
        .await
        .unwrap();
    let list = helpers::response_json(resp).await;
    let comp = list
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == companion)
        .unwrap();
    assert_eq!(comp["companion_of"], primary);

    // No chains: merging into a device that is itself a companion is rejected.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{primary}/companion"),
            &cookie,
            &format!(r#"{{"primary_id":"{companion}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Unmerge.
    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{companion}/companion"),
            &cookie,
            r#"{"primary_id":null}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn audio_receiver_binding_routes_volume_to_receiver() {
    let (port_s, src_cmds) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let (port_r, rcv_cmds) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let source = add_onkyo_device(&app, &cookie, port_s, "Source", &[]).await;
    let receiver = add_onkyo_device(
        &app,
        &cookie,
        port_r,
        "Receiver",
        std::slice::from_ref(&source),
    )
    .await;

    // Bind, then set volume on the source.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{source}/receiver"),
            &cookie,
            &format!(r#"{{"receiver_id":"{receiver}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{source}/state"),
            &cookie,
            r#"{"volume":33}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The volume landed on the receiver, not the source.
    assert!(
        wait_for_volume_set(&rcv_cmds).await,
        "receiver should have received the volume command"
    );
    assert!(
        !heard_volume_set(&src_cmds.lock().await),
        "source must not receive volume while bound"
    );

    // Unbind and set volume again — now it lands on the device itself.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{source}/receiver"),
            &cookie,
            r#"{"receiver_id":null}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{source}/state"),
            &cookie,
            r#"{"volume":50}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        wait_for_volume_set(&src_cmds).await,
        "an unbound device controls its own volume"
    );
}

#[tokio::test]
async fn list_overlays_bound_source_with_receiver_volume() {
    // The list endpoint (Control/Rooms/Floor-Plan load from it) must show a bound
    // source the *receiver's* volume, like the single-device read does. Give the
    // two devices distinct volumes so the overlay is unambiguous: source 0x0A=10,
    // receiver 0x1E=30 — the bound source must report 30, not its own 10.
    let source_state = std::collections::HashMap::from([
        ("PWR", "01".to_string()),
        ("MVL", "0A".to_string()),
        ("AMT", "00".to_string()),
        ("SLI", "12".to_string()),
    ]);
    let (port_s, _) = audio_mock::spawn(source_state).await;
    let (port_r, _) = audio_mock::spawn(audio_mock::receiver_state()).await; // MVL 1E = 30
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let source = add_onkyo_device(&app, &cookie, port_s, "Source", &[]).await;
    let receiver = add_onkyo_device(
        &app,
        &cookie,
        port_r,
        "Receiver",
        std::slice::from_ref(&source),
    )
    .await;

    app.clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{source}/receiver"),
            &cookie,
            &format!(r#"{{"receiver_id":"{receiver}"}}"#),
        ))
        .await
        .unwrap();

    let list = helpers::response_json(
        app.oneshot(helpers::authed_get("/api/audio/devices", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let src = list
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == source)
        .unwrap();
    assert_eq!(
        src["state"]["volume"], 30,
        "list must overlay the bound source with the receiver's volume (30), not its own (10)"
    );
}

#[tokio::test]
async fn room_volume_skips_a_bound_receiver_member() {
    // Source + receiver both in the room; the source is bound to the receiver.
    // Room volume must reach the receiver exactly once (routed via the source),
    // not twice (also directly), which would race on the volume value.
    let (port_s, _) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let (port_r, rcv_cmds) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let source = add_onkyo_device(&app, &cookie, port_s, "Source", &[]).await;
    let receiver = add_onkyo_device(
        &app,
        &cookie,
        port_r,
        "Receiver",
        std::slice::from_ref(&source),
    )
    .await;

    let room_id = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_post(
                "/api/rooms",
                &cookie,
                r#"{"name":"Den","light_ids":[]}"#,
            ))
            .await
            .unwrap(),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Both devices are room audio members.
    app.clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/audio"),
            &cookie,
            &format!(
                r#"{{"devices":[{{"audio_device_id":"{source}"}},{{"audio_device_id":"{receiver}"}}]}}"#
            ),
        ))
        .await
        .unwrap();
    // Bind the source to the receiver.
    app.clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{source}/receiver"),
            &cookie,
            &format!(r#"{{"receiver_id":"{receiver}"}}"#),
        ))
        .await
        .unwrap();

    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/audio/state"),
            &cookie,
            r#"{"volume":40}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Wait for the routed volume to land (commands flow through the link actor
    // asynchronously), then confirm the receiver was driven exactly once — not
    // twice (direct + via the bound source).
    assert!(wait_for_volume_set(&rcv_cmds).await, "receiver was driven");
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(
        volume_set_count(&rcv_cmds.lock().await),
        1,
        "receiver should be driven exactly once"
    );
}

/// A wiremock Sonos household: one standalone player plus a Favorites list.
/// Enough for discovery to create a device row and for the favorites endpoints
/// to browse/play against.
mod sonos_mock {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn esc(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    fn envelope(inner: &str) -> String {
        format!(
            r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body>{inner}</s:Body></s:Envelope>"#
        )
    }

    /// Mount the topology response plus generic per-service handlers (one
    /// response per service path covers every action the provider sends:
    /// Get/Set volume+mute, transport reads, Play, enqueue, group join/leave).
    async fn mount_household(server: &MockServer, topo: &str) {
        Mock::given(method("POST"))
            .and(path("/ZoneGroupTopology/Control"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(envelope(&format!(
                    "<ZoneGroupState>{}</ZoneGroupState>",
                    esc(topo)
                ))),
            )
            .mount(server)
            .await;

        Mock::given(method("POST"))
            .and(path("/MediaRenderer/RenderingControl/Control"))
            .respond_with(ResponseTemplate::new(200).set_body_string(envelope(
                "<CurrentVolume>30</CurrentVolume><CurrentMute>0</CurrentMute>",
            )))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/MediaRenderer/AVTransport/Control"))
            .respond_with(ResponseTemplate::new(200).set_body_string(envelope(
                "<CurrentTransportState>STOPPED</CurrentTransportState>",
            )))
            .mount(server)
            .await;

        let didl = concat!(
            r#"<DIDL-Lite xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:r="urn:schemas-rinconnetworks-com:metadata-1-0/">"#,
            r#"<item id="FV:2/12" parentID="FV:2" restricted="true">"#,
            r#"<dc:title>Jazz</dc:title><r:description>Spotify</r:description>"#,
            r#"<res protocolInfo="x-rincon-cpcontainer:*:*:*">x-rincon-cpcontainer:1006206cspotify</res>"#,
            r#"<r:resMD>&lt;DIDL-Lite&gt;&lt;/DIDL-Lite&gt;</r:resMD></item>"#,
            r#"<item id="FV:2/3" parentID="FV:2" restricted="true">"#,
            r#"<dc:title>BBC Radio 6</dc:title><r:description>TuneIn</r:description>"#,
            r#"<res protocolInfo="x-sonosapi-stream:*:*:*">x-sonosapi-stream:s12345?sid=254</res>"#,
            r#"<r:resMD>&lt;DIDL-Lite&gt;&lt;/DIDL-Lite&gt;</r:resMD></item></DIDL-Lite>"#,
        );
        Mock::given(method("POST"))
            .and(path("/MediaServer/ContentDirectory/Control"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(envelope(&format!("<Result>{}</Result>", esc(didl)))),
            )
            .mount(server)
            .await;
    }

    /// A single standalone Living Room player.
    pub async fn start() -> MockServer {
        let server = MockServer::start().await;
        let base = server.uri();
        let topo = format!(
            r#"<ZoneGroups><ZoneGroup Coordinator="RINCON_LIVING" ID="RINCON_LIVING:1"><ZoneGroupMember UUID="RINCON_LIVING" Location="{base}/xml/device_description.xml" ZoneName="Living Room"/></ZoneGroup></ZoneGroups>"#
        );
        mount_household(&server, &topo).await;
        server
    }

    /// Two standalone players — Living Room + Kitchen — so they can be grouped.
    pub async fn start_pair() -> MockServer {
        let server = MockServer::start().await;
        let base = server.uri();
        let topo = format!(
            r#"<ZoneGroups><ZoneGroup Coordinator="RINCON_LIVING" ID="RINCON_LIVING:1"><ZoneGroupMember UUID="RINCON_LIVING" Location="{base}/xml/device_description.xml" ZoneName="Living Room"/></ZoneGroup><ZoneGroup Coordinator="RINCON_KITCHEN" ID="RINCON_KITCHEN:1"><ZoneGroupMember UUID="RINCON_KITCHEN" Location="{base}/xml/device_description.xml" ZoneName="Kitchen"/></ZoneGroup></ZoneGroups>"#
        );
        mount_household(&server, &topo).await;
        server
    }
}

/// Add a Sonos provider pointed at the wiremock household, discover it, and
/// return the player's Bifrost device id.
async fn setup_sonos(app: &Router, cookie: &str, base_uri: &str) -> String {
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers",
            cookie,
            &format!(
                r#"{{"name":"Sonos","provider_type":"sonos","credentials":{{"host":"{base_uri}"}}}}"#
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

    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/audio/devices", cookie))
        .await
        .unwrap();
    let devices = helpers::response_json(resp).await;
    devices[0]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn audio_favorites_lists_provider_favorites() {
    let server = sonos_mock::start().await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let device_id = setup_sonos(&app, &cookie, &server.uri()).await;

    let resp = app
        .oneshot(helpers::authed_get(
            &format!("/api/audio/devices/{device_id}/favorites"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let favs = helpers::response_json(resp).await;
    let favs = favs.as_array().unwrap();
    assert_eq!(favs.len(), 2);
    assert_eq!(favs[0]["id"], "FV:2/12");
    assert_eq!(favs[0]["title"], "Jazz");
    assert_eq!(favs[0]["subtitle"], "Spotify");
}

#[tokio::test]
async fn audio_play_favorite_starts_playback() {
    let server = sonos_mock::start().await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let device_id = setup_sonos(&app, &cookie, &server.uri()).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "POST",
            &format!("/api/audio/devices/{device_id}/favorites/play"),
            &cookie,
            r#"{"favorite_id":"FV:2/12"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The container favorite is enqueued and the queue is played.
    let actions: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter_map(|r| {
            r.headers
                .get("SOAPACTION")
                .map(|v| v.to_str().unwrap_or("").to_string())
        })
        .collect();
    assert!(
        actions.iter().any(|a| a.contains("AddURIToQueue")),
        "expected an enqueue: {actions:?}"
    );
    assert!(
        actions.iter().any(|a| a.contains("#Play\"")),
        "expected Play: {actions:?}"
    );
}

#[tokio::test]
async fn sync_wraps_sonos_rooms_into_bifrost_rooms() {
    let server = sonos_mock::start().await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let device_id = setup_sonos(&app, &cookie, &server.uri()).await;

    // The provider id for the sync URL.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/providers", &cookie))
        .await
        .unwrap();
    let provs = helpers::response_json(resp).await;
    let provider_id = provs[0]["id"].as_str().unwrap().to_string();

    // Sync the Sonos rooms into Bifrost Rooms.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/providers/{provider_id}/sync-groups"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["synced"], 1);
    assert_eq!(body["rooms_created"], 1);

    // The audio mirror exists, tagged as the audio domain with the speaker.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/provider-groups", &cookie))
        .await
        .unwrap();
    let mirrors = helpers::response_json(resp).await;
    assert_eq!(mirrors[0]["name"], "Living Room");
    assert_eq!(mirrors[0]["domain"], "audio");
    assert_eq!(mirrors[0]["audio_device_ids"][0], device_id);
    assert_eq!(mirrors[0]["light_ids"].as_array().unwrap().len(), 0);

    // The room exists, links the audio mirror, and its audio device resolves
    // through that link.
    let resp = app
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    assert_eq!(rooms[0]["name"], "Living Room");
    assert_eq!(rooms[0]["audio_devices"][0]["audio_device_id"], device_id);
    assert_eq!(rooms[0]["links"][0]["name"], "Living Room");
    assert_eq!(rooms[0]["links"][0]["domain"], "audio");
}

#[tokio::test]
async fn audio_play_favorite_unknown_id_returns_422() {
    let server = sonos_mock::start().await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let device_id = setup_sonos(&app, &cookie, &server.uri()).await;

    let resp = app
        .oneshot(helpers::authed_json(
            "POST",
            &format!("/api/audio/devices/{device_id}/favorites/play"),
            &cookie,
            r#"{"favorite_id":"FV:2/999"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

/// Return (Living Room id, Kitchen id) from a discovered two-player household.
async fn sonos_pair_ids(app: &Router, cookie: &str) -> (String, String) {
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/audio/devices", cookie))
        .await
        .unwrap();
    let devices = helpers::response_json(resp).await;
    let find = |name: &str| {
        devices
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == name)
            .unwrap_or_else(|| panic!("no device named {name}"))["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    (find("Living Room"), find("Kitchen"))
}

#[tokio::test]
async fn audio_group_joins_speaker_to_coordinator() {
    let server = sonos_mock::start_pair().await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    setup_sonos(&app, &cookie, &server.uri()).await;
    let (living, kitchen) = sonos_pair_ids(&app, &cookie).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "POST",
            &format!("/api/audio/devices/{kitchen}/group"),
            &cookie,
            &format!(r#"{{"coordinator_id":"{living}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The member was pointed at the coordinator via the x-rincon scheme.
    let joined = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .any(|r| String::from_utf8_lossy(&r.body).contains("x-rincon:RINCON_LIVING"));
    assert!(
        joined,
        "expected a SetAVTransportURI join to the coordinator"
    );
}

#[tokio::test]
async fn audio_ungroup_makes_speaker_standalone() {
    let server = sonos_mock::start_pair().await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    setup_sonos(&app, &cookie, &server.uri()).await;
    let (_living, kitchen) = sonos_pair_ids(&app, &cookie).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "POST",
            &format!("/api/audio/devices/{kitchen}/ungroup"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let standalone = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter_map(|r| r.headers.get("SOAPACTION").and_then(|v| v.to_str().ok()))
        .any(|a| a.contains("BecomeCoordinatorOfStandaloneGroup"));
    assert!(standalone, "expected the player to become standalone");
}

#[tokio::test]
async fn audio_group_with_itself_returns_422() {
    let server = sonos_mock::start_pair().await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    setup_sonos(&app, &cookie, &server.uri()).await;
    let (_living, kitchen) = sonos_pair_ids(&app, &cookie).await;

    let resp = app
        .oneshot(helpers::authed_json(
            "POST",
            &format!("/api/audio/devices/{kitchen}/group"),
            &cookie,
            &format!(r#"{{"coordinator_id":"{kitchen}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn audio_group_unknown_device_returns_404() {
    let server = sonos_mock::start_pair().await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    setup_sonos(&app, &cookie, &server.uri()).await;
    let (living, _kitchen) = sonos_pair_ids(&app, &cookie).await;

    let resp = app
        .oneshot(helpers::authed_json(
            "POST",
            "/api/audio/devices/nope/group",
            &cookie,
            &format!(r#"{{"coordinator_id":"{living}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn audio_group_across_providers_returns_422() {
    let server = sonos_mock::start_pair().await;
    let (port, _recorded) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    setup_sonos(&app, &cookie, &server.uri()).await;
    let (_living, kitchen) = sonos_pair_ids(&app, &cookie).await;
    let onkyo = setup_onkyo(&app, &cookie, port).await;

    // A Sonos speaker can't coordinate an Onkyo receiver (different provider).
    let resp = app
        .oneshot(helpers::authed_json(
            "POST",
            &format!("/api/audio/devices/{kitchen}/group"),
            &cookie,
            &format!(r#"{{"coordinator_id":"{onkyo}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
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
                .body(Body::from(r#"{"devices":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn room_audio_members_set_list_and_clear() {
    let (port, _) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let device_id = setup_onkyo(&app, &cookie, port).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"Den","light_ids":[]}"#,
        ))
        .await
        .unwrap();
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
            r#"{"devices":[{"audio_device_id":"nope"}]}"#,
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
            &format!(r#"{{"devices":[{{"audio_device_id":"{device_id}"}}]}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Add the device with an offset; see it (with offset) in session + v1.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/audio"),
            &cookie,
            &format!(r#"{{"devices":[{{"audio_device_id":"{device_id}","volume_offset":-6}}]}}"#),
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
    assert_eq!(rooms[0]["audio_devices"][0]["audio_device_id"], device_id);
    assert_eq!(rooms[0]["audio_devices"][0]["volume_offset"], -6);

    let key = create_api_key(&app, &cookie, "mcp").await;
    let resp = app
        .clone()
        .oneshot(bearer_get("/api/v1/rooms", &key))
        .await
        .unwrap();
    assert_eq!(
        helpers::response_json(resp).await[0]["audio_device_ids"][0],
        device_id
    );

    // Clear with an empty list.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/audio"),
            &cookie,
            r#"{"devices":[]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    assert_eq!(
        helpers::response_json(resp).await[0]["audio_devices"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn room_controls_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/rooms/some-id/controls")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"controls":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn room_controls_set_validate_list_and_clear() {
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"Den","light_ids":[]}"#,
        ))
        .await
        .unwrap();
    let room_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Unknown kind → 422.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/controls"),
            &cookie,
            r#"{"controls":[{"kind":"frobnicate","glyph":"power","targets":[]}]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Unknown target device → 422.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/controls"),
            &cookie,
            r#"{"controls":[{"kind":"power","glyph":"power","targets":[{"domain":"light","id":"ghost"}]}]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // A scene control with no scene_id → 422.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/controls"),
            &cookie,
            r#"{"controls":[{"kind":"scene","glyph":"scene"}]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Valid power control targeting the light → 204, then listed on the room.
    let body = format!(
        r#"{{"controls":[{{"kind":"power","glyph":"power","label":"All","targets":[{{"domain":"light","id":"{light_id}"}}]}}]}}"#
    );
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/controls"),
            &cookie,
            &body,
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
    let ctrl = &rooms[0]["controls"][0];
    assert_eq!(ctrl["kind"], "power");
    assert_eq!(ctrl["glyph"], "power");
    assert_eq!(ctrl["label"], "All");
    assert_eq!(ctrl["targets"][0]["domain"], "light");
    assert_eq!(ctrl["targets"][0]["id"], light_id);
    assert!(ctrl["id"].as_str().is_some_and(|s| !s.is_empty()));

    // Clear with an empty list.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/controls"),
            &cookie,
            r#"{"controls":[]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    assert_eq!(
        helpers::response_json(resp).await[0]["controls"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn room_audio_state_fans_out_with_offsets() {
    // Two receivers in one room; the room volume fans out to both, with each
    // device's per-room offset applied (clamped 0–100). Onkyo master volume is
    // hex: 40 = 0x28, 34 = 0x22.
    let (port_a, rec_a) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let (port_b, rec_b) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    setup_onkyo(&app, &cookie, port_a).await;
    setup_onkyo(&app, &cookie, port_b).await;

    // Both devices share a name, so fetch the two ids directly.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/audio/devices", &cookie))
        .await
        .unwrap();
    let devs = helpers::response_json(resp).await;
    let dev_a = devs[0]["id"].as_str().unwrap().to_string();
    let dev_b = devs[1]["id"].as_str().unwrap().to_string();

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"Office","light_ids":[]}"#,
        ))
        .await
        .unwrap();
    let room_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let body = format!(
        r#"{{"devices":[{{"audio_device_id":"{dev_a}","volume_offset":0}},{{"audio_device_id":"{dev_b}","volume_offset":-6}}]}}"#
    );
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/audio"),
            &cookie,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/audio/state"),
            &cookie,
            r#"{"volume":40}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let cmds_a = rec_a.lock().await.clone();
    let cmds_b = rec_b.lock().await.clone();
    // One device gets 40 (offset 0 → 0x28), the other 34 (offset -6 → 0x22).
    // dev_a/dev_b map to the two mocks in an unknown order, so check both.
    let got28 = cmds_a.contains(&"MVL28".to_string()) || cmds_b.contains(&"MVL28".to_string());
    let got22 = cmds_a.contains(&"MVL22".to_string()) || cmds_b.contains(&"MVL22".to_string());
    assert!(got28, "a device → 40: {cmds_a:?} {cmds_b:?}");
    assert!(got22, "a device → 34 (offset -6): {cmds_a:?} {cmds_b:?}");
}

#[tokio::test]
async fn room_audio_state_without_members_returns_404() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"Empty","light_ids":[]}"#,
        ))
        .await
        .unwrap();
    let room_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/audio/state"),
            &cookie,
            r#"{"volume":30}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
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
    // Every IP-addressable provider supports auto-detect. (tasmota/shelly are
    // unregistered in production; wled stays as the generic test light.)
    for t in ["onkyo", "sonos", "hue", "wled"] {
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

// ── Provider display names + audio domain in listings ─────────────────────────

#[tokio::test]
async fn provider_types_carry_display_names() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .oneshot(helpers::authed_get("/api/providers/types", &cookie))
        .await
        .unwrap();
    let types = helpers::response_json(resp).await;
    let name_of = |t: &str| {
        types
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["provider_type"] == t)
            .unwrap()["display_name"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(name_of("hue"), "Philips Hue");
    assert_eq!(name_of("sonos"), "Sonos");
    assert_eq!(name_of("onkyo"), "Onkyo / Integra");
}

#[tokio::test]
async fn audio_provider_lists_with_name_domain_and_ready_status() {
    let (port, _) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    setup_onkyo(&app, &cookie, port).await;

    // The provider listing carries the friendly name + audio domain.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/providers", &cookie))
        .await
        .unwrap();
    let providers = helpers::response_json(resp).await;
    let onkyo = providers
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["provider_type"] == "onkyo")
        .expect("onkyo provider present");
    assert_eq!(onkyo["type_name"], "Onkyo / Integra");
    assert_eq!(onkyo["domain"], "audio");
}

#[tokio::test]
async fn sonos_provider_is_push_managed() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Sonos is a Push audio provider (GENA + heartbeat poll), so adding it
    // starts a connection manager.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers",
            &cookie,
            r#"{"name":"Sonos","provider_type":"sonos","credentials":{"host":"192.168.1.50"}}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Its status reflects a real managed connection (not "not_managed"); the host
    // is unreachable in the test, so it's mid-connect/reconnect rather than ready.
    let resp = app
        .oneshot(helpers::authed_get(
            &format!("/api/providers/{id}/status"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let label = helpers::response_json(resp).await["state"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        ["disconnected", "connecting", "connected", "reconnecting"].contains(&label.as_str()),
        "expected a managed connection state, got {label:?}"
    );
}

// ── Embedded MCP surface (/mcp) ──────────────────────────────────────────────

/// POST a JSON-RPC message to the Streamable HTTP MCP endpoint with a Bearer
/// key. The endpoint runs stateless + json-response, so each call is a plain
/// request/response with an `application/json` body.
fn mcp_request(key: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/mcp")
        // rmcp's Streamable HTTP transport requires a parseable Host header
        // (DNS-rebinding guard); real HTTP clients always send one.
        .header(header::HOST, "localhost")
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn mcp_tool_call(key: &str, tool: &str, args: serde_json::Value) -> Request<Body> {
    mcp_request(
        key,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": tool, "arguments": args },
        }),
    )
}

/// Pull the first text content block out of a `tools/call` JSON-RPC response.
fn mcp_result_text(body: &serde_json::Value) -> String {
    body["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn mcp_without_bearer_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mcp_with_invalid_key_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(mcp_tool_call(
            "bfr_not_a_real_key",
            "get_home_state",
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mcp_tools_list_exposes_the_tool_set() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "mcp").await;

    let resp = app
        .oneshot(mcp_request(
            &key,
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    let names: Vec<&str> = body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for expected in [
        "get_home_state",
        "set_light",
        "set_room",
        "apply_scene",
        "set_audio",
        "play_audio_favorite",
        "group_speakers",
        "bind_receiver",
    ] {
        assert!(
            names.contains(&expected),
            "missing tool {expected} in {names:?}"
        );
    }
}

#[tokio::test]
async fn mcp_bind_receiver_binds_and_unbinds() {
    let (port_s, _) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let (port_r, _) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let source = add_onkyo_device(&app, &cookie, port_s, "Source", &[]).await;
    let receiver = add_onkyo_device(
        &app,
        &cookie,
        port_r,
        "Receiver",
        std::slice::from_ref(&source),
    )
    .await;
    let key = create_api_key(&app, &cookie, "mcp").await;

    // Bind the source to the receiver via the MCP tool.
    let resp = app
        .clone()
        .oneshot(mcp_tool_call(
            &key,
            "bind_receiver",
            serde_json::json!({"device": source, "receiver": receiver, "receiver_source": "Game"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(mcp_result_text(&body), "ok");

    // get_audio_state reflects the binding on the source.
    let resp = app
        .clone()
        .oneshot(mcp_tool_call(
            &key,
            "get_audio_state",
            serde_json::json!({"device": source}),
        ))
        .await
        .unwrap();
    let body = helpers::response_json(resp).await;
    let dev: serde_json::Value = serde_json::from_str(&mcp_result_text(&body)).unwrap();
    assert_eq!(dev["receiver_id"], receiver);
    assert_eq!(dev["receiver_source"], "Game");

    // Omitting `receiver` unbinds.
    let resp = app
        .clone()
        .oneshot(mcp_tool_call(
            &key,
            "bind_receiver",
            serde_json::json!({"device": source}),
        ))
        .await
        .unwrap();
    let body = helpers::response_json(resp).await;
    assert_eq!(mcp_result_text(&body), "ok");

    let resp = app
        .oneshot(mcp_tool_call(
            &key,
            "get_audio_state",
            serde_json::json!({"device": source}),
        ))
        .await
        .unwrap();
    let body = helpers::response_json(resp).await;
    let dev: serde_json::Value = serde_json::from_str(&mcp_result_text(&body)).unwrap();
    assert!(dev["receiver_id"].is_null());
}

#[tokio::test]
async fn mcp_get_home_state_returns_snapshot() {
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "mcp").await;

    let resp = app
        .oneshot(mcp_tool_call(&key, "get_home_state", serde_json::json!({})))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    let snapshot: serde_json::Value =
        serde_json::from_str(&mcp_result_text(&body)).expect("tool returns JSON text");
    // The seeded "Test Light" shows up in the lights array.
    let light_names: Vec<&str> = snapshot["lights"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"].as_str().unwrap())
        .collect();
    assert!(light_names.contains(&"Test Light"), "{light_names:?}");
    assert!(snapshot["rooms"].is_array());
    assert!(snapshot["scenes"].is_array());
    assert!(snapshot["audio_devices"].is_array());
}

#[tokio::test]
async fn mcp_set_light_resolves_by_name_and_drives_provider() {
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "mcp").await;

    // Resolve the light by a case-insensitive substring of its name.
    let resp = app
        .oneshot(mcp_tool_call(
            &key,
            "set_light",
            serde_json::json!({ "light": "test", "brightness": 40.0, "color": "#ff8800" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["result"]["isError"], false, "tool errored: {body}");

    // The provider actually received the state write.
    let requests = bridge.received_requests().await.unwrap();
    assert!(
        requests.iter().any(|r| r.url.path() == "/json/state"),
        "no set_state call reached the device"
    );
}

#[tokio::test]
async fn mcp_set_light_unknown_name_lists_available() {
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "mcp").await;

    let resp = app
        .oneshot(mcp_tool_call(
            &key,
            "set_light",
            serde_json::json!({ "light": "nonexistent", "on": true }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["result"]["isError"], true);
    // The error names the available light so the assistant can self-correct.
    assert!(mcp_result_text(&body).contains("Test Light"));
}

// ── Voice command seam (M23 P1) ──────────────────────────────────────────────

#[tokio::test]
async fn voice_vocabulary_requires_auth() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/voice/vocabulary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn voice_vocabulary_lists_command_words_and_device_names() {
    // The kiosk biases its on-device recognizer to this list. It must include
    // the grammar's command words AND the home's device-name words (so "test
    // light" is recognizable and not misheard).
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .oneshot(helpers::authed_get("/api/voice/vocabulary", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    let words: Vec<String> = body["words"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap().to_string())
        .collect();
    for w in ["turn", "off", "lights", "brightness"] {
        assert!(words.iter().any(|x| x == w), "missing command word '{w}'");
    }
    // "test light" → tokenized into the vocabulary.
    assert!(
        words.iter().any(|x| x == "test"),
        "device-name word missing from vocabulary: {words:?}"
    );
}

#[tokio::test]
async fn voice_command_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/voice/command")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"text":"turn off the office"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn voice_command_accepts_bearer_api_key() {
    // The headless wall-tablet voice satellite has no session — it authenticates
    // with a `bfr_` Bearer key like any /api/v1 client. The voice seam must
    // accept it and drive the device just as a session would.
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "tablet").await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/voice/command")
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"text":"bifrost, turn off test light"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["ok"], true, "bearer-authed command failed: {body}");

    let requests = bridge.received_requests().await.unwrap();
    assert!(
        requests.iter().any(|r| r.url.path() == "/json/state"),
        "no set_state call reached the device"
    );
}

#[tokio::test]
async fn voice_command_resolves_light_by_name_and_drives_provider() {
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // "turn off test light" → resolve the light by name → drive its provider.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "POST",
            "/api/voice/command",
            &cookie,
            r#"{"text":"bifrost, turn off test light"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["ok"], true, "command failed: {body}");
    assert!(
        body["said"].as_str().unwrap().contains("Turned off"),
        "{body}"
    );

    let requests = bridge.received_requests().await.unwrap();
    assert!(
        requests.iter().any(|r| r.url.path() == "/json/state"),
        "no set_state call reached the device"
    );
}

#[tokio::test]
async fn voice_llm_fallback_resolves_an_unparsed_clause_and_drives_the_device() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // A clause the native grammar can't parse, rescued by the configured `chat`
    // model: it returns one tool call, which maps to a Command and dispatches.
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Mock chat endpoint — returns a set_power tool call for the test light.
    let chat = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "tool_calls": [{
                "id": "c1", "type": "function",
                "function": {
                    "name": "set_power",
                    "arguments": "{\"target\":\"test light\",\"on\":false}"
                }
            }]}}]
        })))
        .mount(&chat)
        .await;

    // Point the `chat` role at the mock.
    let cfg = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/ai-endpoints/chat",
            &cookie,
            &format!(r#"{{"base_url":"{}","model":"m"}}"#, chat.uri()),
        ))
        .await
        .unwrap();
    assert!(
        cfg.status().is_success(),
        "configuring chat endpoint: {:?}",
        cfg.status()
    );

    // "abracadabra" is unparseable by the grammar → LLM fallback → set_power.
    let resp = app
        .oneshot(helpers::authed_json(
            "POST",
            "/api/voice/command",
            &cookie,
            r#"{"text":"abracadabra"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["ok"], true, "llm-fallback command failed: {body}");

    // The mapped Command drove the light's provider.
    let reqs = bridge.received_requests().await.unwrap();
    assert!(
        reqs.iter().any(|r| r.url.path() == "/json/state"),
        "the LLM-resolved command never reached the device"
    );
}

#[tokio::test]
async fn voice_falls_back_to_ha_assist_for_unparsed_commands() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // An HA mock that answers the Assist conversation endpoint.
    let ha = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/conversation/process"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "response": {
                "speech": { "plain": { "speech": "Playing Bob's Burgers on the TV." } },
                "response_type": "action_done"
            }
        })))
        .mount(&ha)
        .await;

    let (app, _prov_id) = helpers::test_app_with_ha(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // A media-launch intent the native grammar can't parse → delegated to HA.
    let resp = app
        .oneshot(helpers::authed_json(
            "POST",
            "/api/voice/command",
            &cookie,
            r#"{"text":"play Bob's Burgers on the TV"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["ok"], true, "{body}");
    assert!(
        body["said"]
            .as_str()
            .unwrap()
            .contains("Playing Bob's Burgers"),
        "expected HA Assist speech, got {body}"
    );

    let reqs = ha.received_requests().await.unwrap();
    assert!(
        reqs.iter()
            .any(|r| r.url.path() == "/api/conversation/process"),
        "the unparsed command did not reach HA Assist"
    );
}

#[tokio::test]
async fn voice_command_unknown_target_reports_not_found() {
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .oneshot(helpers::authed_json(
            "POST",
            "/api/voice/command",
            &cookie,
            r#"{"text":"turn off the dungeon"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["ok"], false);
    assert!(body["said"].as_str().unwrap().contains("dungeon"), "{body}");
}

#[tokio::test]
async fn voice_command_compound_runs_each_clause() {
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // One clause resolves (the light), one doesn't (no such room) — partial.
    let resp = app
        .oneshot(helpers::authed_json(
            "POST",
            "/api/voice/command",
            &cookie,
            r#"{"text":"turn on test light and turn off the dungeon"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["clauses"].as_array().unwrap().len(), 2);
    assert_eq!(body["clauses"][0]["ok"], true);
    assert_eq!(body["clauses"][1]["ok"], false);
    assert_eq!(body["ok"], false, "compound ok only if all clauses ok");
}

#[tokio::test]
async fn voice_relative_dim_drives_the_light() {
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // "dim test light" → relative brightness down → drives the provider.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "POST",
            "/api/voice/command",
            &cookie,
            r#"{"text":"dim test light"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["ok"], true, "{body}");
    assert!(body["said"].as_str().unwrap().contains("Dimmed"), "{body}");
    let reqs = bridge.received_requests().await.unwrap();
    assert!(reqs.iter().any(|r| r.url.path() == "/json/state"));
}

#[tokio::test]
async fn voice_room_color_touches_lights_not_audio() {
    // Color/brightness on a room must drive only its lights — never power on the
    // room's speakers (regression: "make the studio blue" was starting Sonos).
    let (port, audio_cmds) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let audio_id = add_onkyo_device(&app, &cookie, port, "AV", &[]).await;

    // A room with the light, plus the audio device as a member.
    let room_id = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_json(
                "POST",
                "/api/rooms",
                &cookie,
                &format!(r#"{{"name":"Studio","light_ids":["{light_id}"]}}"#),
            ))
            .await
            .unwrap(),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    app.clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/audio/devices/{audio_id}/room"),
            &cookie,
            &format!(r#"{{"room_id":"{room_id}"}}"#),
        ))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "POST",
            "/api/voice/command",
            &cookie,
            r#"{"text":"make the studio blue"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(helpers::response_json(resp).await["ok"], true);

    // The light got a state write …
    let reqs = bridge.received_requests().await.unwrap();
    assert!(reqs.iter().any(|r| r.url.path() == "/json/state"));
    // … but the audio member was never powered on (no PWR set command).
    let powered = audio_cmds
        .lock()
        .await
        .iter()
        .any(|m| m.starts_with("PWR") && !m.contains("QSTN"));
    assert!(!powered, "room color must not power on the room's audio");
}

// ── Power devices (HA multi-domain discover/control) ─────────────────────────

/// A wiremock HA serving power entities plus the domain-agnostic toggle service.
async fn ha_power_mock() -> wiremock::MockServer {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/states"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "entity_id": "switch.porch", "state": "on",
              "attributes": { "friendly_name": "Porch" } },
            { "entity_id": "fan.bedroom", "state": "off",
              "attributes": { "friendly_name": "Bedroom Fan" } }
        ])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/services/homeassistant/turn_off"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn power_devices_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/power/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn v1_power_without_key_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/power/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn discover_ha_populates_power_devices_with_kinds() {
    let ha = ha_power_mock().await;
    let (app, prov_id) = helpers::test_app_with_ha(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Discover runs across HA's domains (lights + power); the mock has only
    // power entities, so the two switches/fans are what land.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/providers/{prov_id}/discover"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(helpers::response_json(resp).await["discovered"], 2);

    let resp = app
        .oneshot(helpers::authed_get("/api/power/devices", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    let porch = arr
        .iter()
        .find(|d| d["device_id"] == "switch.porch")
        .unwrap();
    assert_eq!(porch["kind"], "switch");
    assert_eq!(porch["state"]["on"], true);
    let fan = arr
        .iter()
        .find(|d| d["device_id"] == "fan.bedroom")
        .unwrap();
    assert_eq!(fan["kind"], "fan");
    assert_eq!(fan["state"]["on"], false);
}

async fn ha_remote_mock() -> wiremock::MockServer {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let entity = serde_json::json!(
        { "entity_id": "remote.bedroom_tv", "state": "on",
          "attributes": { "friendly_name": "Bedroom TV",
                          "current_activity": "com.netflix.ninja", "supported_features": 4 } }
    );
    Mock::given(method("GET"))
        .and(path("/api/states"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([entity.clone()])))
        .mount(&server)
        .await;
    // Single-entity live read (used by GET /remote/devices/{id}).
    Mock::given(method("GET"))
        .and(path("/api/states/remote.bedroom_tv"))
        .respond_with(ResponseTemplate::new(200).set_body_json(entity))
        .mount(&server)
        .await;
    for svc in ["send_command", "turn_on", "turn_off"] {
        Mock::given(method("POST"))
            .and(path(format!("/api/services/remote/{svc}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
    }
    server
}

#[tokio::test]
async fn remote_devices_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/remote/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn discover_ha_populates_remotes_then_command_drives_service() {
    let ha = ha_remote_mock().await;
    let (app, prov_id) = helpers::test_app_with_ha(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/providers/{prov_id}/discover"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(helpers::response_json(resp).await["discovered"], 1);

    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/remote/devices", &cookie))
        .await
        .unwrap();
    let remotes = helpers::response_json(resp).await;
    let remote = &remotes.as_array().unwrap()[0];
    assert_eq!(remote["device_id"], "remote.bedroom_tv");
    assert_eq!(remote["state"]["current_app"], "com.netflix.ninja");
    let remote_id = remote["id"].as_str().unwrap().to_string();

    // Send a canonical key; it must reach HA's remote.send_command.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "POST",
            &format!("/api/remote/devices/{remote_id}/command"),
            &cookie,
            r#"{"key":{"key":"select"}}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let reqs = ha.received_requests().await.unwrap();
    assert!(
        reqs.iter()
            .any(|r| r.url.path() == "/api/services/remote/send_command"),
        "the key press did not reach HA send_command"
    );
}

#[tokio::test]
async fn remote_apps_record_recents_pin_and_order() {
    let ha = ha_remote_mock().await;
    let (app, prov_id) = helpers::test_app_with_ha(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    app.clone()
        .oneshot(helpers::authed_post(
            &format!("/api/providers/{prov_id}/discover"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    let remotes = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/remote/devices", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let remote_id = remotes.as_array().unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // The apps list is session-gated like every other remote route.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/remote/devices/{remote_id}/apps"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // A live read of one device records its foreground app as a "recent".
    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            &format!("/api/remote/devices/{remote_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let apps = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get(
                &format!("/api/remote/devices/{remote_id}/apps"),
                &cookie,
            ))
            .await
            .unwrap(),
    )
    .await;
    let arr = apps.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["package"], "com.netflix.ninja");
    assert_eq!(arr[0]["name"], "Netflix"); // friendly name from the registry
    assert_eq!(arr[0]["pinned"], false);
    assert!(arr[0]["last_seen"].is_string());

    // Pinning a never-seen package inserts it; pinned apps sort first.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/remote/devices/{remote_id}/apps/pin"),
            &cookie,
            r#"{"package":"com.disney.disneyplus","pinned":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let apps = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get(
                &format!("/api/remote/devices/{remote_id}/apps"),
                &cookie,
            ))
            .await
            .unwrap(),
    )
    .await;
    let arr = apps.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["package"], "com.disney.disneyplus"); // pinned first
    assert_eq!(arr[0]["name"], "Disney+");
    assert_eq!(arr[0]["pinned"], true);
    assert_eq!(arr[1]["package"], "com.netflix.ninja");

    // Unpinning leaves it tracked as a recent (still listed, no longer pinned).
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/remote/devices/{remote_id}/apps/pin"),
            &cookie,
            r#"{"package":"com.disney.disneyplus","pinned":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let apps = helpers::response_json(
        app.oneshot(helpers::authed_get(
            &format!("/api/remote/devices/{remote_id}/apps"),
            &cookie,
        ))
        .await
        .unwrap(),
    )
    .await;
    let arr = apps.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert!(arr.iter().all(|a| a["pinned"] == false));
}

#[tokio::test]
async fn v1_remote_requires_bearer_and_drives_command() {
    let ha = ha_remote_mock().await;
    let (app, prov_id) = helpers::test_app_with_ha(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // No bearer → 401.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/remote/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    app.clone()
        .oneshot(helpers::authed_post(
            &format!("/api/providers/{prov_id}/discover"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    let key = create_api_key(&app, &cookie, "k").await;

    let resp = app
        .clone()
        .oneshot(bearer_get("/api/v1/remote/devices", &key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let remotes = helpers::response_json(resp).await;
    let remote_id = remotes.as_array().unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .oneshot(bearer_json(
            "POST",
            &format!("/api/v1/remote/devices/{remote_id}/command"),
            &key,
            r#"{"launch_app":{"activity":"com.netflix.ninja"}}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let reqs = ha.received_requests().await.unwrap();
    assert!(
        reqs.iter()
            .any(|r| r.url.path() == "/api/services/remote/turn_on"),
        "launch_app did not reach HA remote.turn_on"
    );
}

#[tokio::test]
async fn set_power_device_drives_ha_toggle_service() {
    let ha = ha_power_mock().await;
    let (app, prov_id) = helpers::test_app_with_ha(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    app.clone()
        .oneshot(helpers::authed_post(
            &format!("/api/providers/{prov_id}/discover"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/power/devices", &cookie))
        .await
        .unwrap();
    let body = helpers::response_json(resp).await;
    let porch_id = body
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["device_id"] == "switch.porch")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/power/devices/{porch_id}/state"),
            &cookie,
            r#"{"on":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let reqs = ha.received_requests().await.unwrap();
    assert!(
        reqs.iter()
            .any(|r| r.url.path() == "/api/services/homeassistant/turn_off"),
        "no homeassistant.turn_off call reached HA"
    );
}

#[tokio::test]
async fn room_power_membership_roundtrips_and_lists() {
    let ha = ha_power_mock().await;
    let (app, prov_id) = helpers::test_app_with_ha(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Discover HA's power devices.
    app.clone()
        .oneshot(helpers::authed_post(
            &format!("/api/providers/{prov_id}/discover"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/power/devices", &cookie))
        .await
        .unwrap();
    let devices = helpers::response_json(resp).await;
    let porch_id = devices.as_array().unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Pick the porch switch specifically.
    let porch_id = devices
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["device_id"] == "switch.porch")
        .map(|d| d["id"].as_str().unwrap().to_string())
        .unwrap_or(porch_id);

    // Create a room.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"Garage","light_ids":[]}"#,
        ))
        .await
        .unwrap();
    let room_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Assign the power device to the room.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/power"),
            &cookie,
            &format!(r#"{{"power_device_ids":["{porch_id}"]}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The room now lists it (session shape) …
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    let room = rooms
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == room_id)
        .unwrap();
    assert_eq!(room["power_device_ids"], serde_json::json!([porch_id]));

    // … and an unknown device id is rejected.
    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/power"),
            &cookie,
            r#"{"power_device_ids":["nope"]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn room_on_off_fans_out_to_power_members() {
    let ha = ha_power_mock().await;
    let (app, prov_id) = helpers::test_app_with_ha(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    app.clone()
        .oneshot(helpers::authed_post(
            &format!("/api/providers/{prov_id}/discover"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    let devices = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/power/devices", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let porch_id = devices
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["device_id"] == "switch.porch")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // A room whose only member is the power device (no lights).
    let room_id = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_post(
                "/api/rooms",
                &cookie,
                r#"{"name":"Garage","light_ids":[]}"#,
            ))
            .await
            .unwrap(),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    app.clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/power"),
            &cookie,
            &format!(r#"{{"power_device_ids":["{porch_id}"]}}"#),
        ))
        .await
        .unwrap();

    // Turning the (light-less) room off must reach the power member.
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
    let reqs = ha.received_requests().await.unwrap();
    assert!(
        reqs.iter()
            .any(|r| r.url.path() == "/api/services/homeassistant/turn_off"),
        "room off didn't fan out to the power member"
    );
}

#[tokio::test]
async fn sync_groups_mirrors_ha_area_with_only_power_devices() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    let ha = ha_power_mock().await; // serves /api/states with switch.porch + fan.bedroom
    // An Area that contains a switch (and a light that isn't discovered) — it
    // must still sync on the strength of its power member.
    Mock::given(method("POST"))
        .and(path("/api/template"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"[{"area_id":"garage","name":"Garage","entities":["switch.porch","light.ghost","sensor.x"]}]"#,
        ))
        .mount(&ha)
        .await;

    let (app, prov_id) = helpers::test_app_with_ha(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Discover devices, then sync areas → rooms.
    app.clone()
        .oneshot(helpers::authed_post(
            &format!("/api/providers/{prov_id}/discover"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/providers/{prov_id}/sync-groups"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["synced"], 1, "the Garage area should sync: {body}");

    // A "Garage" room now exists and carries the switch as a power member (via
    // the synced link, not direct membership).
    let resp = app
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    let garage = rooms
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "Garage")
        .expect("Garage room created from the synced area");
    assert_eq!(
        garage["power_device_ids"].as_array().unwrap().len(),
        1,
        "Garage should contain the porch switch via its link: {garage}"
    );
}

#[tokio::test]
async fn discover_ha_surfaces_media_player_as_audio_device() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let ha = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/states"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "entity_id": "media_player.bedroom_tv", "state": "playing",
              "attributes": { "friendly_name": "Bedroom TV", "device_class": "tv",
                              "volume_level": 0.3, "supported_features": 0 } }
        ])))
        .mount(&ha)
        .await;

    let (app, prov_id) = helpers::test_app_with_ha(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Multi-domain discover includes the audio (media_player) domain now.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/providers/{prov_id}/discover"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(helpers::response_json(resp).await["discovered"], 1);

    // The TV shows up on the audio device list.
    let resp = app
        .oneshot(helpers::authed_get("/api/audio/devices", &cookie))
        .await
        .unwrap();
    let devices = helpers::response_json(resp).await;
    let tv = devices
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["device_id"] == "media_player.bedroom_tv")
        .expect("Bedroom TV surfaced as an audio device");
    assert_eq!(tv["name"], "Bedroom TV");
    // HA's `device_class: "tv"` is preserved as a first-class kind so the UI
    // can identify it as a TV rather than a generic speaker/receiver.
    assert_eq!(tv["kind"], "tv");
}

#[tokio::test]
async fn disabled_power_device_rejects_commands_but_stays_listed() {
    let ha = ha_power_mock().await;
    let (app, prov_id) = helpers::test_app_with_ha(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    app.clone()
        .oneshot(helpers::authed_post(
            &format!("/api/providers/{prov_id}/discover"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/power/devices", &cookie))
        .await
        .unwrap();
    let id = helpers::response_json(resp).await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Disable it.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/power/devices/{id}/enabled"),
            &cookie,
            r#"{"enabled":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // A command is now refused (no command reaches the device) …
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/power/devices/{id}/state"),
            &cookie,
            r#"{"on":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // … but it's still tracked, flagged disabled.
    let resp = app
        .oneshot(helpers::authed_get("/api/power/devices", &cookie))
        .await
        .unwrap();
    let devices = helpers::response_json(resp).await;
    let dev = devices
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == id)
        .unwrap();
    assert_eq!(dev["enabled"], false);
}

#[tokio::test]
async fn set_light_glyph_overrides_then_clears() {
    let server = wiremock::MockServer::start().await;
    let (app, light_id) = helpers::test_app_with_light(&server.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // A fresh light has no glyph override.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            &format!("/api/lights/{light_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert!(helpers::response_json(resp).await["glyph"].is_null());

    // Pin the led_strip glyph.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}/glyph"),
            &cookie,
            r#"{"glyph":"led_strip"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            &format!("/api/lights/{light_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(helpers::response_json(resp).await["glyph"], "led_strip");

    // Clearing it (null) returns to the type default.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}/glyph"),
            &cookie,
            r#"{"glyph":null}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get(
            &format!("/api/lights/{light_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert!(helpers::response_json(resp).await["glyph"].is_null());
}

#[tokio::test]
async fn set_light_glyph_requires_session() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/lights/some-id/glyph")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"glyph":"bulb"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn set_light_shadow_links_and_clears() {
    let server = wiremock::MockServer::start().await;
    let (app, light_id) = helpers::test_app_with_light(&server.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Manually shadow the light under an arbitrary canonical id.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}/shadow"),
            &cookie,
            r#"{"shadowed_by":"some-canonical-id"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            &format!("/api/lights/{light_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    let body = helpers::response_json(resp).await;
    assert_eq!(body["shadowed_by"], "some-canonical-id");
    assert_eq!(body["shadow_auto"], false); // a manual link

    // Clearing it (null) makes the device visible again.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}/shadow"),
            &cookie,
            r#"{"shadowed_by":null}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get(
            &format!("/api/lights/{light_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert!(helpers::response_json(resp).await["shadowed_by"].is_null());
}

#[tokio::test]
async fn set_light_room_assigns_and_clears() {
    let server = wiremock::MockServer::start().await;
    let (app, light_id) = helpers::test_app_with_light(&server.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Create a room to assign into.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"Living Room"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let room_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Assign the light to the room from the device side.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}/room"),
            &cookie,
            &format!(r#"{{"room_id":"{room_id}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            &format!("/api/lights/{light_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(helpers::response_json(resp).await["room_id"], room_id);

    // Clearing (null) removes it from the room.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}/room"),
            &cookie,
            r#"{"room_id":null}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get(
            &format!("/api/lights/{light_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert!(helpers::response_json(resp).await["room_id"].is_null());
}

#[tokio::test]
async fn set_light_room_unknown_room_is_not_found() {
    let server = wiremock::MockServer::start().await;
    let (app, light_id) = helpers::test_app_with_light(&server.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}/room"),
            &cookie,
            r#"{"room_id":"no-such-room"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn set_light_room_requires_session() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/lights/some-id/room")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"room_id":"x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn set_light_shadow_requires_session() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/lights/some-id/shadow")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"shadowed_by":"x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn set_power_glyph_overrides_device() {
    let ha = ha_power_mock().await;
    let (app, prov_id) = helpers::test_app_with_ha(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    app.clone()
        .oneshot(helpers::authed_post(
            &format!("/api/providers/{prov_id}/discover"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/power/devices", &cookie))
        .await
        .unwrap();
    let id = helpers::response_json(resp).await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/power/devices/{id}/glyph"),
            &cookie,
            r#"{"glyph":"led_strip"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get("/api/power/devices", &cookie))
        .await
        .unwrap();
    let devices = helpers::response_json(resp).await;
    let dev = devices
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == id)
        .unwrap();
    assert_eq!(dev["glyph"], "led_strip");
}

#[tokio::test]
async fn set_prune_requires_session() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/providers/x/prune")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"prune":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn discover_with_prune_removes_devices_no_longer_reported() {
    let ha = ha_power_mock().await; // reports switch.porch + fan.bedroom
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // First discover → 2 devices.
    app.clone()
        .oneshot(helpers::authed_post(
            &format!("/api/providers/{prov_id}/discover"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();

    // Seed a stale device the provider no longer reports (old last_seen).
    sqlx::query(
        "INSERT INTO power_devices (id, provider_id, device_id, name, kind, last_state, last_seen)
         VALUES ('ghost', ?, 'switch.ghost', 'Ghost', 'switch', '{\"on\":false}', '2000-01-01 00:00:00')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();

    // A plain discover keeps it (additive).
    app.clone()
        .oneshot(helpers::authed_post(
            &format!("/api/providers/{prov_id}/discover"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/power/devices", &cookie))
        .await
        .unwrap();
    assert_eq!(
        helpers::response_json(resp).await.as_array().unwrap().len(),
        3
    );

    // Discover with ?prune=true removes the stale one, keeps the reported two.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/providers/{prov_id}/discover?prune=true"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["discovered"], 2);
    assert_eq!(body["pruned"], 1);

    let resp = app
        .oneshot(helpers::authed_get("/api/power/devices", &cookie))
        .await
        .unwrap();
    let devices = helpers::response_json(resp).await;
    let ids: Vec<&str> = devices
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["device_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(
        !ids.contains(&"switch.ghost"),
        "stale device pruned: {ids:?}"
    );
}

// ── AI endpoints config + voice /listen (M23 P2) ─────────────────────────────

#[tokio::test]
async fn ai_endpoints_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/ai-endpoints")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ai_endpoints_crud_roundtrip_redacts_key() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Empty to start.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/ai-endpoints", &cookie))
        .await
        .unwrap();
    let body = helpers::response_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 0);

    // Create the transcription role with a key.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/ai-endpoints/transcription",
            &cookie,
            r#"{"base_url":"http://localhost:9000/v1","model":"whisper-1","api_key":"sk-secret"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["has_key"], true);
    assert!(body.get("api_key").is_none(), "key must never be returned");

    // List shows it, key redacted to has_key.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/ai-endpoints", &cookie))
        .await
        .unwrap();
    let body = helpers::response_json(resp).await;
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["role"], "transcription");
    assert_eq!(arr[0]["base_url"], "http://localhost:9000/v1");
    assert_eq!(arr[0]["has_key"], true);
    assert!(arr[0].get("api_key").is_none());

    // Delete.
    let resp = app
        .clone()
        .oneshot(helpers::authed_delete(
            "/api/ai-endpoints/transcription",
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .oneshot(helpers::authed_get("/api/ai-endpoints", &cookie))
        .await
        .unwrap();
    let body = helpers::response_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn ai_endpoints_put_rejects_unknown_role() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/ai-endpoints/bogus",
            &cookie,
            r#"{"base_url":"http://x:1/v1","model":"m"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ai_endpoints_put_rejects_non_http_base_url() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/ai-endpoints/chat",
            &cookie,
            r#"{"base_url":"localhost:1234","model":"llama"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn ai_endpoints_test_probes_models_endpoint() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{ "id": "whisper-1" }]
        })))
        .mount(&server)
        .await;

    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    app.clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/ai-endpoints/transcription",
            &cookie,
            &format!(r#"{{"base_url":"{}","model":"whisper-1"}}"#, server.uri()),
        ))
        .await
        .unwrap();

    let resp = app
        .oneshot(helpers::authed_post(
            "/api/ai-endpoints/transcription/test",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["ok"], true, "{body}");
}

/// Build a minimal multipart/form-data body with one audio `file` part.
fn multipart_audio(boundary: &str, bytes: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"clip.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

#[tokio::test]
async fn voice_listen_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let boundary = "BIFROSTTEST";
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/voice/listen")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart_audio(boundary, b"FAKE")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn voice_listen_without_transcription_endpoint_is_503() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let boundary = "BIFROSTTEST";
    let req = Request::builder()
        .method("POST")
        .uri("/api/voice/listen")
        .header(header::COOKIE, cookie.split(';').next().unwrap())
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(multipart_audio(boundary, b"FAKE")))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn voice_listen_transcribes_then_drives_the_light() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // STT endpoint that "hears" a command targeting the seeded light.
    let stt = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "bifrost, turn off test light"
        })))
        .mount(&stt)
        .await;

    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Configure the transcription role to point at the STT mock.
    app.clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/ai-endpoints/transcription",
            &cookie,
            &format!(r#"{{"base_url":"{}","model":"whisper-1"}}"#, stt.uri()),
        ))
        .await
        .unwrap();

    let boundary = "BIFROSTTEST";
    let req = Request::builder()
        .method("POST")
        .uri("/api/voice/listen")
        .header(header::COOKIE, cookie.split(';').next().unwrap())
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(multipart_audio(boundary, b"RIFFfakeaudio")))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["transcript"], "bifrost, turn off test light");
    assert_eq!(body["ok"], true, "{body}");

    // The recognized command reached the light's provider.
    let requests = bridge.received_requests().await.unwrap();
    assert!(
        requests.iter().any(|r| r.url.path() == "/json/state"),
        "transcribed command never drove the device"
    );
}
