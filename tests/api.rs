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
    // The hub's wall clock — RFC 3339 with a UTC offset, so a TZ-skewed
    // deployment (hour plans consulting the wrong hour) is visible at a glance.
    let t = body["server_time"].as_str().unwrap();
    assert!(
        chrono::DateTime::parse_from_rfc3339(t).is_ok(),
        "server_time should be RFC 3339: {t}"
    );
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

/// A PURE power-only provider type (Kasa — no light/media factory to piggyback
/// on) end to end: `POST /api/providers` used to 400 with "unknown
/// provider_type" for exactly this shape (the validation only ever checked
/// `is_known`/`is_known_media`), and even past that, nothing started a live
/// poller for it (the connection dispatch's fallback just logged "unknown
/// type" — there was no "power-only, no other domain" branch). Both are real
/// bugs this test locks in the fix for, against the real HTTP route and the
/// real connection manager — not just the provider module's own unit tests.
///
/// No mock device is needed: `device_ip` has no port field in Kasa's schema
/// (every add always targets the fixed protocol port 9999), so this points at
/// loopback where nothing listens on 9999 within the test sandbox — the
/// add/build path has no I/O (confirmed: `KasaProvider::from_credentials`
/// only parses the IP), and the poller's very first tick fails fast
/// (ECONNREFUSED, not a timeout), which is exactly what this test needs: proof
/// the manager is running and attempting, not proof a real plug answers (the
/// provider's own `discover`/`get_state` behaviour against a real device is
/// covered by src/providers/kasa/mod.rs's mock-TCP-listener tests).
#[tokio::test]
async fn kasa_power_only_provider_adds_and_starts_polling() {
    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers",
            &cookie,
            r#"{"name":"Raven Lights","provider_type":"kasa","credentials":{"device_ip":"127.0.0.1"}}"#,
        ))
        .await
        .unwrap();
    // The bug: this used to be 400 "unknown provider_type 'kasa'".
    assert_eq!(resp.status(), StatusCode::CREATED);
    let id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Confirm it's a REAL entry in the registry (build_power actually ran) —
    // `all_types`/`ui_domain` regressions are covered at the unit level
    // (src/providers/mod.rs); this is the type actually persisted.
    let stored: String = sqlx::query_scalar("SELECT provider_type FROM providers WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(stored, "kasa");

    // The other bug: the connection manager must actually be running for this
    // provider (not silently absent) — poll for it to leave "disconnected".
    // A closed loopback port does NOT refuse fast in every environment (this
    // one included — verified empirically: a connect to a closed port here
    // hangs the full timeout rather than an instant ECONNREFUSED), so the
    // first observable transition only lands once the provider's own connect
    // timeout (5s) elapses and `PowerPollingManager` reports "reconnecting" —
    // an absent manager would never move off "disconnected" at all, which is
    // the actual thing this asserts.
    let mut engaged = false;
    for _ in 0..140 {
        let status = helpers::response_json(
            app.clone()
                .oneshot(helpers::authed_get(
                    &format!("/api/providers/{id}/status"),
                    &cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        if status["state"] == "reconnecting" || status["state"] == "connecting" {
            engaged = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        engaged,
        "power polling manager never engaged — start_manager_for's power-only branch didn't run"
    );
}

/// `GET /api/providers`' own `domain` field must also know about power-only
/// providers — a separate match from `ProviderRegistry::ui_domain` that used
/// to fall through to its `_ => "light"` catch-all, mislabeling every Kasa
/// (and any future power-only) row in the Devices UI.
#[tokio::test]
async fn list_providers_reports_power_domain_for_kasa() {
    let (app, _state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    app.clone()
        .oneshot(helpers::authed_post(
            "/api/providers",
            &cookie,
            r#"{"name":"Raven Lights","provider_type":"kasa","credentials":{"device_ip":"127.0.0.1"}}"#,
        ))
        .await
        .unwrap();

    let body = helpers::response_json(
        app.oneshot(helpers::authed_get("/api/providers", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let kasa = body
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["provider_type"] == "kasa")
        .unwrap();
    assert_eq!(kasa["domain"], "power");
}

/// The internal `kiosk` provider row (one per paired kiosk, backing its mic
/// sensor's `sensor_devices.provider_id` FK) has no credentials to edit and
/// isn't something a user ever adds/removes — it must never surface as a
/// normal provider card.
#[tokio::test]
async fn list_providers_hides_internal_kiosk_sensor_provider() {
    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "wall tablet").await;

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/kiosks/checkin")
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    let kiosks = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let kiosk_id = kiosks[0]["id"].as_str().unwrap().to_string();
    app.clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{kiosk_id}/mic"),
            &cookie,
            r#"{"enabled":true,"sensitivity":"high"}"#,
        ))
        .await
        .unwrap();
    let exists: Option<String> =
        sqlx::query_scalar("SELECT id FROM providers WHERE provider_type = 'kiosk'")
            .fetch_optional(&state.db)
            .await
            .unwrap();
    assert!(
        exists.is_some(),
        "the kiosk-sensors provider row must exist in the DB"
    );

    let body = helpers::response_json(
        app.oneshot(helpers::authed_get("/api/providers", &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert!(
        body.as_array()
            .unwrap()
            .iter()
            .all(|p| p["provider_type"] != "kiosk"),
        "the internal kiosk-sensors row must never appear in the providers list"
    );
}

/// A Smart-TV row's connected vendor rides in its credentials (`brand`,
/// stamped by discovery — absent means Bravia, the default vendor) so the UI
/// can label a Bravia vs. a generic Android TV box distinctly instead of both
/// reading as an identical, unlabeled "Smart TV" card.
#[tokio::test]
async fn list_providers_reports_smarttv_brand() {
    let (app, _state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    app.clone()
        .oneshot(helpers::authed_post(
            "/api/providers",
            &cookie,
            r#"{"name":"Living Room","provider_type":"smarttv","credentials":{"host":"127.0.0.1"}}"#,
        ))
        .await
        .unwrap();
    app.clone()
        .oneshot(helpers::authed_post(
            "/api/providers",
            &cookie,
            r#"{"name":"Bedroom Dongle","provider_type":"smarttv","credentials":{"host":"127.0.0.2","brand":"androidtv"}}"#,
        ))
        .await
        .unwrap();

    let body = helpers::response_json(
        app.oneshot(helpers::authed_get("/api/providers", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let providers = body.as_array().unwrap();
    let bravia = providers
        .iter()
        .find(|p| p["name"] == "Living Room")
        .unwrap();
    let androidtv = providers
        .iter()
        .find(|p| p["name"] == "Bedroom Dongle")
        .unwrap();
    // Unset brand resolves to "bravia" (build_vendor's own default) rather
    // than an absent/null field, so the UI always has a concrete label.
    assert_eq!(bravia["brand"], "bravia");
    assert_eq!(androidtv["brand"], "androidtv");
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
async fn kiosk_login_exchanges_key_for_session() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "wall tablet").await;

    // No / unknown key cookie → 401.
    for cookie_hdr in ["", "bfr_key=bfr_nope"] {
        let mut req = Request::builder().method("POST").uri("/api/auth/kiosk");
        if !cookie_hdr.is_empty() {
            req = req.header(header::COOKIE, cookie_hdr);
        }
        let resp = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "cookie={cookie_hdr:?}"
        );
    }

    // The real key cookie → 200 + a session cookie that authenticates the dashboard.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/kiosk")
                .header(header::COOKIE, format!("bfr_key={key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        set_cookie.contains("bifrost_session="),
        "no session cookie minted"
    );

    let session = set_cookie.split(';').next().unwrap();
    let resp = app
        .oneshot(helpers::authed_get("/api/providers", session))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "minted session should authenticate"
    );
}

#[tokio::test]
async fn kiosk_default_board_set_and_self_resolves_it() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "wall tablet").await;

    // Check in (Bearer key) to register the kiosk row.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/kiosks/checkin")
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The clients list shows it with no board yet.
    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let id = list[0]["id"].as_str().unwrap().to_string();
    assert!(list[0]["default_board_id"].is_null());

    // Create a board and assign it as this kiosk's default (from the main client).
    let board = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_post(
                "/api/dashboards",
                &cookie,
                r#"{"name":"Wall"}"#,
            ))
            .await
            .unwrap(),
    )
    .await;
    let board_id = board["id"].as_str().unwrap().to_string();
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{id}/board"),
            &cookie,
            &format!(r#"{{"board_id":"{board_id}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The kiosk resolves its own assignment via /self (bfr_key cookie auth).
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/kiosks/self")
                .header(header::COOKIE, format!("bfr_key={key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        helpers::response_json(resp).await["default_board_id"],
        board_id
    );

    // /self without the kiosk key cookie → 401.
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/kiosks/self")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn kiosk_reports_its_viewport_and_the_clients_list_carries_it() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "wall tablet").await;

    // Register the kiosk row.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/kiosks/checkin")
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The kiosk-served web client reports its CSS viewport (bfr_key cookie auth).
    let put_viewport = |body: &'static str, with_key: bool| {
        let mut b = Request::builder()
            .method("PUT")
            .uri("/api/kiosks/self/viewport")
            .header(header::CONTENT_TYPE, "application/json");
        if with_key {
            b = b.header(header::COOKIE, format!("bfr_key={key}"));
        }
        b.body(Body::from(body)).unwrap()
    };
    let resp = app
        .clone()
        .oneshot(put_viewport(r#"{"w":893,"h":533}"#, true))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The clients list carries it, for the Boards preview device menu.
    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(list[0]["viewport_w"], 893);
    assert_eq!(list[0]["viewport_h"], 533);

    // Nonsense dimensions are rejected; no cookie → 401.
    let resp = app
        .clone()
        .oneshot(put_viewport(r#"{"w":2,"h":533}"#, true))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let resp = app
        .oneshot(put_viewport(r#"{"w":893,"h":533}"#, false))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn kiosk_hour_plan_roundtrips_and_validates() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "wall tablet").await;

    // Register the kiosk row.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/kiosks/checkin")
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let id = list[0]["id"].as_str().unwrap().to_string();
    assert!(list[0]["hour_modes"].is_null(), "no plan painted yet");

    // Paint a plan: overnight asleep, morning aware, day awake, evening aware.
    let plan = "SSSSSSAAWWWWWWWWWWAAAASS";
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{id}/plan"),
            &cookie,
            &format!(r#"{{"enabled":true,"hour_modes":"{plan}","timeout_secs":300}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(list[0]["hour_modes"], plan);
    assert_eq!(list[0]["schedule_enabled"], true);
    assert_eq!(list[0]["presence_timeout_secs"], 300);

    // Wrong length and junk characters are rejected.
    for bad in ["SSS", "SSSSSSAAWWWWWWWWWWAAAASX"] {
        let resp = app
            .clone()
            .oneshot(helpers::authed_json(
                "PUT",
                &format!("/api/kiosks/{id}/plan"),
                &cookie,
                &format!(r#"{{"enabled":true,"hour_modes":"{bad}"}}"#),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }
}

/// Drives a real scheduler pass against the real schema — the tick's SELECT and
/// its row reads must agree (a column read but not fetched panics sqlx's `get`
/// and killed the scheduler task; the pure plan helpers couldn't catch that).
#[tokio::test]
async fn scheduler_tick_enforces_the_hour_plan() {
    use std::collections::HashMap;

    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "wall tablet").await;

    // Register the kiosk row.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/kiosks/checkin")
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let id = list[0]["id"].as_str().unwrap().to_string();

    // Paint: midnight→noon awake, noon→midnight asleep.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{id}/plan"),
            &cookie,
            r#"{"enabled":true,"hour_modes":"WWWWWWWWWWWWSSSSSSSSSSSS"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let pending = |app: Router, cookie: String| async move {
        let list = helpers::response_json(
            app.oneshot(helpers::authed_get("/api/kiosks", &cookie))
                .await
                .unwrap(),
        )
        .await;
        list[0]["pending_command"].clone()
    };

    let mut desired = HashMap::new();
    let mut present = HashMap::new();

    // An awake hour queues a wake…
    bifrost::api::kiosks::scheduler_tick(&state, &mut desired, &mut present, 3, 3 * 60).await;
    assert_eq!(pending(app.clone(), cookie.clone()).await, "wake");

    // …and an asleep hour flips it to sleep (edge-triggered on the change).
    bifrost::api::kiosks::scheduler_tick(&state, &mut desired, &mut present, 15, 15 * 60).await;
    assert_eq!(pending(app.clone(), cookie.clone()).await, "sleep");
}

/// `PUT /api/kiosks/{id}/aware-override` — the HTTP contract: a valid mixed-
/// domain target list roundtrips through `GET /api/kiosks`, and an unknown
/// domain is rejected before anything is stored.
#[tokio::test]
async fn aware_override_roundtrips_and_validates_domain() {
    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "wall tablet").await;
    sqlx::query(
        "INSERT INTO providers (id, provider_type, name, credentials) VALUES ('p','wled','P','x')",
    )
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO power_devices (id, provider_id, device_id, name, kind, last_state) VALUES ('pw1','p','d1','Amp','switch','{\"on\":false}')")
        .execute(&state.db)
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/kiosks/checkin")
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let id = list[0]["id"].as_str().unwrap().to_string();
    assert_eq!(list[0]["aware_override_targets"], serde_json::json!([]));

    // An unknown domain is rejected before anything is stored.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{id}/aware-override"),
            &cookie,
            r#"{"targets":[{"domain":"climate","id":"pw1"}]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // A valid target list roundtrips.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{id}/aware-override"),
            &cookie,
            r#"{"targets":[{"domain":"power","id":"pw1"}]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        list[0]["aware_override_targets"],
        serde_json::json!([{"domain":"power","id":"pw1"}])
    );
    // Mode absent in the PUT → keep_on, the pre-mode behaviour.
    assert_eq!(list[0]["aware_override_mode"], "keep_on");

    // An explicit keep_off mode roundtrips too.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{id}/aware-override"),
            &cookie,
            r#"{"targets":[{"domain":"power","id":"pw1"}],"mode":"keep_off"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(list[0]["aware_override_mode"], "keep_off");

    // An unknown kiosk id 404s.
    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/kiosks/nope/aware-override",
            &cookie,
            r#"{"targets":[]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// The actual behaviour: an Aware hour with no presence input normally leaves
/// the kiosk ungoverned (no command queued) — but with an override target
/// that's ON, the scheduler forces the screen awake regardless, and clears the
/// moment the device is off again.
#[tokio::test]
async fn aware_override_forces_awake_while_the_target_device_is_on() {
    use std::collections::HashMap;

    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "wall tablet").await;
    sqlx::query(
        "INSERT INTO providers (id, provider_type, name, credentials) VALUES ('p','wled','P','x')",
    )
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO power_devices (id, provider_id, device_id, name, kind, last_state) VALUES ('pw1','p','d1','Amp','switch','{\"on\":false}')")
        .execute(&state.db)
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/kiosks/checkin")
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let id = list[0]["id"].as_str().unwrap().to_string();

    // All-Aware plan, no room assigned — presence never governs on its own.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{id}/plan"),
            &cookie,
            r#"{"enabled":true,"hour_modes":"AAAAAAAAAAAAAAAAAAAAAAAA"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{id}/aware-override"),
            &cookie,
            r#"{"targets":[{"domain":"power","id":"pw1"}]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let pending = |app: Router, cookie: String| async move {
        let list = helpers::response_json(
            app.oneshot(helpers::authed_get("/api/kiosks", &cookie))
                .await
                .unwrap(),
        )
        .await;
        list[0]["pending_command"].clone()
    };

    let mut desired = HashMap::new();
    let mut present = HashMap::new();

    // Device off, no room, no presence input → nothing governs this hour.
    bifrost::api::kiosks::scheduler_tick(&state, &mut desired, &mut present, 10, 10 * 60).await;
    assert_eq!(
        pending(app.clone(), cookie.clone()).await,
        serde_json::Value::Null
    );

    // Flip the device on — the override forces the screen awake.
    sqlx::query("UPDATE power_devices SET last_state = '{\"on\":true}' WHERE id = 'pw1'")
        .execute(&state.db)
        .await
        .unwrap();
    bifrost::api::kiosks::scheduler_tick(&state, &mut desired, &mut present, 11, 11 * 60).await;
    assert_eq!(pending(app.clone(), cookie.clone()).await, "wake");
}

/// The keep_off flavour: while the target device is on, an Aware hour forces
/// the kiosk ASLEEP — "movie night, kill the tablet glow" — even in a room
/// with no presence input at all (which would otherwise leave it ungoverned).
#[tokio::test]
async fn aware_override_keep_off_forces_asleep_while_the_target_device_is_on() {
    use std::collections::HashMap;

    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "wall tablet").await;
    sqlx::query(
        "INSERT INTO providers (id, provider_type, name, credentials) VALUES ('p','wled','P','x')",
    )
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO power_devices (id, provider_id, device_id, name, kind, last_state) VALUES ('pw1','p','d1','TV plug','switch','{\"on\":false}')")
        .execute(&state.db)
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/kiosks/checkin")
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let id = list[0]["id"].as_str().unwrap().to_string();

    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{id}/plan"),
            &cookie,
            r#"{"enabled":true,"hour_modes":"AAAAAAAAAAAAAAAAAAAAAAAA"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{id}/aware-override"),
            &cookie,
            r#"{"targets":[{"domain":"power","id":"pw1"}],"mode":"keep_off"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let pending = |app: Router, cookie: String| async move {
        let list = helpers::response_json(
            app.oneshot(helpers::authed_get("/api/kiosks", &cookie))
                .await
                .unwrap(),
        )
        .await;
        list[0]["pending_command"].clone()
    };

    let mut desired = HashMap::new();
    let mut present = HashMap::new();

    // Device off, no room, no presence input → nothing governs this hour.
    bifrost::api::kiosks::scheduler_tick(&state, &mut desired, &mut present, 10, 10 * 60).await;
    assert_eq!(
        pending(app.clone(), cookie.clone()).await,
        serde_json::Value::Null
    );

    // Flip the device on — keep_off forces the screen asleep.
    sqlx::query("UPDATE power_devices SET last_state = '{\"on\":true}' WHERE id = 'pw1'")
        .execute(&state.db)
        .await
        .unwrap();
    bifrost::api::kiosks::scheduler_tick(&state, &mut desired, &mut present, 11, 11 * 60).await;
    assert_eq!(pending(app.clone(), cookie.clone()).await, "sleep");
}

#[tokio::test]
async fn reorder_providers_persists_display_order() {
    let (app, state) = helpers::test_app_with_password_and_state().await;
    for (id, name) in [("p-a", "Alpha"), ("p-b", "Bravo"), ("p-c", "Charlie")] {
        sqlx::query("INSERT INTO providers (id, provider_type, name, credentials) VALUES (?, 'wled', ?, 'x')")
            .bind(id)
            .bind(name)
            .execute(&state.db)
            .await
            .unwrap();
    }
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Reorder to Charlie, Alpha, Bravo.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/providers/order",
            &cookie,
            r#"{"order":["p-c","p-a","p-b"]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The list now reflects that order.
    let resp = app
        .oneshot(helpers::authed_get("/api/providers", &cookie))
        .await
        .unwrap();
    let body = helpers::response_json(resp).await;
    let ids: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["p-c", "p-a", "p-b"]);
}

#[tokio::test]
async fn reorder_providers_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/providers/order")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"order":["x"]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn smarttv_pair_remote_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/providers/whatever/smarttv/pair-remote")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn smarttv_pair_remote_unknown_provider_returns_404() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/providers/nope/smarttv/pair-remote",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn smarttv_pair_remote_rejects_non_smarttv_provider() {
    let (app, state) = helpers::test_app_with_password_and_state().await;
    sqlx::query("INSERT INTO providers (id, provider_type, name, credentials) VALUES ('p1', 'wled', 'Strip', 'x')")
        .execute(&state.db)
        .await
        .unwrap();
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/providers/p1/smarttv/pair-remote",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    // Wrong provider type is the caller's mistake, not a server/gateway fault.
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn smarttv_pair_remote_begin_unreachable_returns_502() {
    let (app, state) = helpers::test_app_with_password_and_state().await;
    // A smart-TV provider whose host points at a closed local port: the begin
    // phase reaches atv_pair_begin and fails fast (connection refused) → 502.
    let enc = state
        .encrypt_credentials(r#"{"host":"127.0.0.1","brand":"bravia"}"#)
        .unwrap();
    sqlx::query("INSERT INTO providers (id, provider_type, name, credentials) VALUES ('tv1', 'smarttv', 'BRAVIA', ?)")
        .bind(&enc)
        .execute(&state.db)
        .await
        .unwrap();
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/providers/tv1/smarttv/pair-remote",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
}

/// The host relocator (connection::relocate): a smart-TV provider whose stored
/// host went dead is re-bound to a scan candidate ONLY when that candidate
/// proves the same hardware identity — live ScalarWeb MAC for a Bravia, the
/// discovery-carried hw_id for an Android TV. A wrong identity is refused.
#[tokio::test]
async fn smarttv_relocator_rebinds_only_to_an_identity_proven_host() {
    use bifrost::connection::relocate::{lost_tvs, relocate_with_candidates};
    use bifrost::providers::discovery::DiscoveredDevice;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let (_app, state) = helpers::test_app_with_password_and_state().await;
    let creds_host = |enc: &str| {
        let j = state.decrypt_credentials(enc).unwrap();
        serde_json::from_str::<serde_json::Value>(&j).unwrap()["host"]
            .as_str()
            .unwrap()
            .to_string()
    };
    async fn stored_creds(db: &sqlx::SqlitePool, id: &str) -> String {
        sqlx::query_scalar::<_, String>("SELECT credentials FROM providers WHERE id = ?")
            .bind(id)
            .fetch_one(db)
            .await
            .unwrap()
    }

    // The TV at its NEW address answers getSystemInformation with its MAC.
    let tv = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sony/system"))
        .and(body_string_contains("getSystemInformation"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": [{ "name": "BRAVIA", "macAddr": "AA:BB:CC:DD:EE:FF" }],
            "id": 1
        })))
        .mount(&tv)
        .await;
    let new_host = tv.uri().trim_start_matches("http://").to_string();

    // The provider still pins a dead host; the hardware id lives on its device
    // row (the creds themselves never got one — the fallback path).
    let enc = state
        .encrypt_credentials(r#"{"host":"127.0.0.1:1","auth":"cookie"}"#)
        .unwrap();
    sqlx::query(
        "INSERT INTO providers (id, provider_type, name, credentials) VALUES ('tv1','smarttv','BRAVIA',?)",
    )
    .bind(&enc)
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, hw_id)
         VALUES ('m1','tv1','main','BRAVIA','mac:aabbccddeeff')",
    )
    .execute(&state.db)
    .await
    .unwrap();

    // Detected as lost, with the device row supplying the expected identity.
    let lost = lost_tvs(&state).await;
    assert_eq!(lost.len(), 1);
    assert_eq!(lost[0].expected_hw, "mac:aabbccddeeff");
    assert_eq!(lost[0].host, "127.0.0.1:1");

    // A different TV at a candidate host (wrong MAC) is refused.
    let stranger = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sony/system"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": [{ "name": "Other", "macAddr": "11:22:33:44:55:66" }],
            "id": 1
        })))
        .mount(&stranger)
        .await;
    let stranger_host = stranger.uri().trim_start_matches("http://").to_string();
    relocate_with_candidates(
        &state,
        lost,
        &[DiscoveredDevice {
            host: stranger_host,
            label: None,
            credentials: serde_json::json!({}),
        }],
    )
    .await;
    assert_eq!(
        creds_host(&stored_creds(&state.db, "tv1").await),
        "127.0.0.1:1",
        "a wrong-identity candidate must never be adopted"
    );

    // The true TV is adopted (an hw-mismatched candidate is skipped cheaply),
    // and the auth cookie survives the rewrite.
    let lost = lost_tvs(&state).await;
    relocate_with_candidates(
        &state,
        lost,
        &[
            DiscoveredDevice {
                host: "10.0.0.9".into(),
                label: None,
                credentials: serde_json::json!({"hw_id": "mac:ffffffffffff"}),
            },
            DiscoveredDevice {
                host: new_host.clone(),
                label: None,
                credentials: serde_json::json!({}),
            },
        ],
    )
    .await;
    let enc = stored_creds(&state.db, "tv1").await;
    assert_eq!(creds_host(&enc), new_host);
    let j: serde_json::Value =
        serde_json::from_str(&state.decrypt_credentials(&enc).unwrap()).unwrap();
    assert_eq!(j["auth"], "cookie", "auth must survive the host rewrite");

    // tv1 is reachable again (the mock server answers TCP) → no longer lost.
    // An Android TV rebinds on the discovery-carried hardware id alone.
    let enc2 = state
        .encrypt_credentials(
            r#"{"host":"127.0.0.1:1","brand":"androidtv","hw_id":"mac:0102030405ff"}"#,
        )
        .unwrap();
    sqlx::query(
        "INSERT INTO providers (id, provider_type, name, credentials) VALUES ('tv2','smarttv','Dongle',?)",
    )
    .bind(&enc2)
    .execute(&state.db)
    .await
    .unwrap();
    let lost = lost_tvs(&state).await;
    assert_eq!(lost.len(), 1, "only the dongle should be lost now");
    assert_eq!(lost[0].provider_id, "tv2");
    relocate_with_candidates(
        &state,
        lost,
        &[DiscoveredDevice {
            host: "192.0.2.7".into(),
            label: None,
            credentials: serde_json::json!({"hw_id": "mac:0102030405ff", "brand": "androidtv"}),
        }],
    )
    .await;
    assert_eq!(
        creds_host(&stored_creds(&state.db, "tv2").await),
        "192.0.2.7"
    );
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

#[tokio::test]
async fn nanoleaf_pair_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/providers/nanoleaf/pair")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"host":"192.168.1.20"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn nanoleaf_pair_returns_token_when_in_pairing_mode() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let controller = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/new"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "auth_token": "nl-tok" })),
        )
        .mount(&controller)
        .await;

    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"host":"{}"}}"#, controller.uri());
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/providers/nanoleaf/pair",
            &cookie,
            &body,
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["auth_token"], "nl-tok");
}

#[tokio::test]
async fn nanoleaf_pair_returns_409_when_not_in_pairing_mode() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let controller = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/new"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&controller)
        .await;

    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let body = format!(r#"{{"host":"{}"}}"#, controller.uri());
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/providers/nanoleaf/pair",
            &cookie,
            &body,
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["error"], "not_in_pairing_mode");
}

#[tokio::test]
async fn nanoleaf_pair_returns_502_when_controller_unreachable() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Nothing listens on port 9.
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/providers/nanoleaf/pair",
            &cookie,
            r#"{"host":"127.0.0.1:9"}"#,
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
async fn recapture_overwrites_a_scene_in_place() {
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/scenes",
            &cookie,
            r#"{"name":"Default"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let scene_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Unauthenticated overwrite is rejected.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/scenes/{scene_id}/recapture"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Overwrite re-snapshots into the same scene (same id, not a duplicate).
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/scenes/{scene_id}/recapture"),
            &cookie,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["id"], scene_id);
    assert_eq!(body["lights"], 1);

    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/scenes", &cookie))
        .await
        .unwrap();
    let scenes = helpers::response_json(resp).await;
    assert_eq!(
        scenes.as_array().unwrap().len(),
        1,
        "overwrite must not duplicate"
    );

    // Overwriting an unknown scene → 404.
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/scenes/nope/recapture",
            &cookie,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dashboards_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/dashboards")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dashboard_crud_persists_name_and_widget_layout() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/dashboards",
            &cookie,
            r#"{"name":"Living Room"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let board = helpers::response_json(resp).await;
    let id = board["id"].as_str().unwrap().to_string();
    assert_eq!(board["name"], "Living Room");
    assert_eq!(board["widgets"].as_array().unwrap().len(), 0);
    // Aspect defaults to 16:9 when omitted.
    assert_eq!(board["aspect"], "16:9");

    // A board can be created with a custom (normalized) aspect ratio.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/dashboards",
            &cookie,
            r#"{"name":"Wall","aspect":" 4 : 3 "}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(helpers::response_json(resp).await["aspect"], "4:3");

    // Empty name is rejected.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/dashboards",
            &cookie,
            r#"{"name":"  "}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Rename + change aspect + save a widget layout (full replacement).
    let body = r#"{"name":"Lounge","aspect":"21:9","widgets":[{"id":"w1","type":"device","x":0,"y":0,"w":2,"h":2,"config":{"device_id":"d1","domain":"light"}}]}"#;
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/dashboards/{id}"),
            &cookie,
            body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Read back — name + widget box + config persisted verbatim.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            &format!("/api/dashboards/{id}"),
            &cookie,
        ))
        .await
        .unwrap();
    let board = helpers::response_json(resp).await;
    assert_eq!(board["name"], "Lounge");
    assert_eq!(board["aspect"], "21:9");
    let widgets = board["widgets"].as_array().unwrap();
    assert_eq!(widgets.len(), 1);
    assert_eq!(widgets[0]["type"], "device");
    assert_eq!(widgets[0]["w"], 2);
    assert_eq!(widgets[0]["config"]["device_id"], "d1");

    // Delete → gone.
    let resp = app
        .clone()
        .oneshot(helpers::authed_delete(
            &format!("/api/dashboards/{id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .oneshot(helpers::authed_get(
            &format!("/api/dashboards/{id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dashboard_background_spec_and_media_roundtrip() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/dashboards",
            &cookie,
            r#"{"name":"Wall"}"#,
        ))
        .await
        .unwrap();
    let board = helpers::response_json(resp).await;
    let id = board["id"].as_str().unwrap().to_string();
    assert!(board["background"].is_null(), "new board has no background");

    // Save a background spec (opaque JSON, stored verbatim like widget config).
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/dashboards/{id}"),
            &cookie,
            r#"{"background":{"kind":"preset","preset":"synthwave","scrim":0.3,"speed":1.5}}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let board = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get(
                &format!("/api/dashboards/{id}"),
                &cookie,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(board["background"]["preset"], "synthwave");
    assert_eq!(board["background"]["scrim"], 0.3);

    // A name-only update leaves the background untouched (double-option seam).
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/dashboards/{id}"),
            &cookie,
            r#"{"name":"Wall 2"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let board = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get(
                &format!("/api/dashboards/{id}"),
                &cookie,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(board["background"]["preset"], "synthwave");

    // Upload media: wrong mime is refused, a png roundtrips with its type.
    let put_media = |mime: &'static str, bytes: &'static [u8]| {
        Request::builder()
            .method("PUT")
            .uri(format!("/api/dashboards/{id}/background/media"))
            .header(header::COOKIE, &cookie)
            .header(header::CONTENT_TYPE, mime)
            .body(Body::from(bytes))
            .unwrap()
    };
    let resp = app
        .clone()
        .oneshot(put_media("text/plain", b"nope"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let resp = app
        .clone()
        .oneshot(put_media("image/png", b"\x89PNG fake bytes"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            &format!("/api/dashboards/{id}/background/media"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/png"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"\x89PNG fake bytes");

    // Clear the spec (explicit null) and the media; both read back gone.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/dashboards/{id}"),
            &cookie,
            r#"{"background":null}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .clone()
        .oneshot(helpers::authed_delete(
            &format!("/api/dashboards/{id}/background/media"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let board = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get(
                &format!("/api/dashboards/{id}"),
                &cookie,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(board["background"].is_null());
    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            &format!("/api/dashboards/{id}/background/media"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Media upload against an unknown board 404s; unauthenticated 401s.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/dashboards/nope/background/media")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "image/png")
                .body(Body::from(&b"x"[..]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/dashboards/{id}/background/media"))
                .header(header::CONTENT_TYPE, "image/png")
                .body(Body::from(&b"x"[..]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
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
async fn home_scene_captures_and_reapplies_a_light_effect() {
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    let light_effect = |app: Router, cookie: String, id: String| async move {
        let resp = app
            .oneshot(helpers::authed_get(&format!("/api/lights/{id}"), &cookie))
            .await
            .unwrap();
        helpers::response_json(resp).await["last_state"]["effect"].clone()
    };

    // Put the light into a dynamic effect.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}"),
            &cookie,
            r#"{"on":true,"effect":"candle"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        light_effect(app.clone(), cookie.clone(), light_id.clone()).await,
        "candle"
    );

    // Snapshot a whole-home scene — it captures the active effect.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/scenes",
            &cookie,
            r#"{"name":"Cozy"}"#,
        ))
        .await
        .unwrap();
    let scene_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Switch the light back to a plain colour — effect/colour are exclusive
    // modes, so the stale effect is cleared from last_state.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}"),
            &cookie,
            r#"{"on":true,"color":{"x":0.5,"y":0.4,"brightness":0.8}}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        light_effect(app.clone(), cookie.clone(), light_id.clone())
            .await
            .is_null(),
        "switching to a colour clears the effect"
    );

    // Activating the scene reapplies the captured effect.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/scenes/{scene_id}/activate"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        light_effect(app, cookie, light_id).await,
        "candle",
        "the scene restored the effect"
    );
}

#[tokio::test]
async fn room_scene_captures_only_its_room_members() {
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Give the light a known state so it's snapshottable.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}"),
            &cookie,
            r#"{"on":true,"brightness":60}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // A room that *contains* the light, and one that doesn't.
    let with = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            &format!(r#"{{"name":"With","light_ids":["{light_id}"]}}"#),
        ))
        .await
        .unwrap();
    let with_id = helpers::response_json(with).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let without = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"Empty","light_ids":[]}"#,
        ))
        .await
        .unwrap();
    let empty_id = helpers::response_json(without).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let capture = |room_id: String| {
        let app = app.clone();
        let cookie = cookie.clone();
        async move {
            let resp = app
                .oneshot(helpers::authed_post(
                    "/api/scenes",
                    &cookie,
                    &format!(r#"{{"name":"S","room_id":"{room_id}"}}"#),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::CREATED);
            helpers::response_json(resp).await["lights"]
                .as_i64()
                .unwrap()
        }
    };

    // The room with the light captures it; the empty room captures nothing —
    // proving the snapshot is scoped to the room's members, not all lights.
    assert_eq!(capture(with_id).await, 1);
    assert_eq!(capture(empty_id).await, 0);
}

#[tokio::test]
async fn palettes_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/palettes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn import_palettes_endpoint_returns_count() {
    // wled exposes no palettes, so the count is 0 — but the endpoint + auth work.
    let bridge = wled_mock().await;
    let (app, _a, _b) = helpers::test_app_with_two_lights(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_post("/api/palettes/import", &cookie, "{}"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(helpers::response_json(resp).await["imported"], 0);
}

#[tokio::test]
async fn apply_palette_distributes_colours_across_room_lights() {
    let bridge = wled_mock().await;
    let (app, light_a, light_b, db) = helpers::test_app_with_two_lights_db(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // A room containing both lights.
    let room = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            &format!(r#"{{"name":"Den","light_ids":["{light_a}","{light_b}"]}}"#),
        ))
        .await
        .unwrap();
    let room_id = helpers::response_json(room).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Seed a two-colour palette directly (no create endpoint — import is the path).
    sqlx::query(
        "INSERT INTO palettes (id, name, source, source_id, colors) VALUES ('pal-1', 'Tropical', 'hue', 'scene-1', ?)",
    )
    .bind(r#"[{"xy":[0.6,0.3],"brightness":80.0},{"xy":[0.2,0.5]}]"#)
    .execute(&db)
    .await
    .unwrap();

    // It shows up in the listing.
    let list = app
        .clone()
        .oneshot(helpers::authed_get("/api/palettes", &cookie))
        .await
        .unwrap();
    let palettes = helpers::response_json(list).await;
    assert_eq!(palettes[0]["name"], "Tropical");
    assert_eq!(palettes[0]["colors"].as_array().unwrap().len(), 2);

    // Apply distributes across both members.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/palettes/pal-1/apply",
            &cookie,
            &format!(r#"{{"room_id":"{room_id}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["applied"], 2);
    assert_eq!(body["failed"], 0);

    // Both lights received a state write.
    let writes = bridge
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/json/state")
        .count();
    assert!(writes >= 2, "expected ≥2 device writes, got {writes}");
}

#[tokio::test]
async fn apply_unknown_palette_returns_404() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/palettes/nope/apply",
            &cookie,
            r#"{"room_id":"whatever"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
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
async fn home_scene_default_set_list_and_restore() {
    let bridge = wled_mock().await;
    let (app, _light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Snapshot the home (1 light, 0 power devices in this fixture).
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/scenes",
            &cookie,
            r#"{"name":"Home"}"#,
        ))
        .await
        .unwrap();
    let body = helpers::response_json(resp).await;
    assert_eq!(body["lights"], 1);
    assert_eq!(body["power"], 0);
    let scene_id = body["id"].as_str().unwrap().to_string();

    // No default yet → Restore Home is a 404.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/scenes/restore-default",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Mark it default → listed as such.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/scenes/{scene_id}/default"),
            &cookie,
            r#"{"default":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/scenes", &cookie))
        .await
        .unwrap();
    assert_eq!(helpers::response_json(resp).await[0]["is_default"], true);

    // Restore Home now applies the default scene through the provider.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/scenes/restore-default",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(helpers::response_json(resp).await["applied"], 1);
    assert!(
        bridge
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path() == "/json/state"),
        "Restore Home didn't reach the device"
    );

    // Unset the default → Restore Home 404s again.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/scenes/{scene_id}/default"),
            &cookie,
            r#"{"default":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_post(
            "/api/scenes/restore-default",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn home_scene_routes_require_session() {
    let app = helpers::test_app_with_password().await;
    for (method, uri, bodytext) in [
        ("POST", "/api/scenes/restore-default", "{}"),
        ("PUT", "/api/scenes/some-id/default", r#"{"default":true}"#),
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(bodytext))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
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
                .uri("/api/plans/some-id/media")
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
        r#"{{"placements":[{{"media_device_id":"{device_id}","x":2,"y":5,"mount":"e"}}]}}"#
    );
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/plans/{plan_id}/media"),
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
    assert_eq!(plan["media"][0]["media_device_id"], device_id);
    assert_eq!(plan["media"][0]["x"], 2);
    assert_eq!(plan["media"][0]["mount"], "e");
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
            &format!("/api/plans/{plan_id}/media"),
            &cookie,
            r#"{"placements":[{"media_device_id":"ghost","x":1,"y":1,"mount":"c"}]}"#,
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
async fn room_attribute_cascade_touches_lit_lights_only() {
    // Dimming a room must dim what's shining and NEVER wake an off lamp — an
    // attribute-only room patch (no `on`) casts onto lit members exclusively.
    // Each light gets its own mock server so "the off light's provider was
    // never called" is directly observable.
    let mock_lit = wled_mock().await;
    let mock_dark = wled_mock().await;
    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    for (pid, lid, base, st) in [
        (
            "prov-lit",
            "light-lit",
            mock_lit.uri(),
            r#"{"on":true,"brightness":80.0}"#,
        ),
        (
            "prov-dark",
            "light-dark",
            mock_dark.uri(),
            r#"{"on":false,"brightness":80.0}"#,
        ),
    ] {
        let enc = state
            .encrypt_credentials(&format!(r#"{{"device_ip":"{base}"}}"#))
            .unwrap();
        sqlx::query(
            "INSERT INTO providers (id, provider_type, name, credentials) VALUES (?, 'wled', ?, ?)",
        )
        .bind(pid)
        .bind(pid)
        .bind(&enc)
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO lights (id, provider_id, device_id, name, capabilities, last_state, last_seen)
             VALUES (?, ?, 'main', ?, '{}', ?, datetime('now'))",
        )
        .bind(lid)
        .bind(pid)
        .bind(lid)
        .bind(st)
        .execute(&state.db)
        .await
        .unwrap();
    }
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"R","light_ids":["light-lit","light-dark"]}"#,
        ))
        .await
        .unwrap();
    let room_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Attribute-only patch: the lit light dims, the dark one is untouched.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/state"),
            &cookie,
            r#"{"brightness":40}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let result = helpers::response_json(resp).await;
    assert_eq!(result["applied"], 1, "only the lit member takes the cast");
    assert!(
        mock_dark.received_requests().await.unwrap().is_empty(),
        "the off light's provider must not be called by a brightness cast"
    );
    assert!(
        mock_lit
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path() == "/json/state"),
        "the lit light takes the brightness"
    );
    let lights = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/lights", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let by = |id: &str| {
        lights
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["id"] == id)
            .cloned()
            .unwrap()
    };
    assert_eq!(
        by("light-dark")["last_state"]["on"],
        false,
        "off lamp stays off"
    );
    assert_eq!(by("light-lit")["last_state"]["brightness"], 40.0);

    // An EXPLICIT `on` is power intent — "turn the room on at 55" wakes both.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/state"),
            &cookie,
            r#"{"on":true,"brightness":55}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(helpers::response_json(resp).await["applied"], 2);
    assert!(
        !mock_dark.received_requests().await.unwrap().is_empty(),
        "explicit on powers the off member too"
    );
}

#[tokio::test]
async fn room_attribute_cascade_with_nothing_lit_is_a_no_op() {
    // A brightness cast over an all-off room does nothing — and reports success
    // (a 404 here would read as an error toast for a perfectly valid gesture).
    let device = wled_mock().await;
    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let enc = state
        .encrypt_credentials(&format!(r#"{{"device_ip":"{}"}}"#, device.uri()))
        .unwrap();
    sqlx::query("INSERT INTO providers (id, provider_type, name, credentials) VALUES ('p1', 'wled', 'W', ?)")
        .bind(&enc)
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO lights (id, provider_id, device_id, name, capabilities, last_state, last_seen)
         VALUES ('l1', 'p1', 'main', 'L', '{}', '{\"on\":false}', datetime('now'))",
    )
    .execute(&state.db)
    .await
    .unwrap();
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"R","light_ids":["l1"]}"#,
        ))
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
            &format!("/api/rooms/{room_id}/state"),
            &cookie,
            r#"{"brightness":40}"#,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a no-op cast is not an error"
    );
    let result = helpers::response_json(resp).await;
    assert_eq!(result["applied"], 0);
    assert!(
        device.received_requests().await.unwrap().is_empty(),
        "nothing lit — no provider traffic"
    );
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
async fn direct_room_assignment_overrides_provider_group_inheritance() {
    // A device synced into a room via its provider-group link, then directly
    // assigned to another room (Devices page), must appear ONLY in the assigned
    // room — the direct assignment de-registers it from the inherited room, so it
    // never duplicates across both.
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

    // A second, empty room to move the light into.
    let gym = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_json(
                "POST",
                "/api/rooms",
                &cookie,
                r#"{"name":"Gym","light_ids":[]}"#,
            ))
            .await
            .unwrap(),
    )
    .await;
    let gym_id = gym["id"].as_str().unwrap().to_string();

    // Move the light to the Gym from the device side.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}/room"),
            &cookie,
            &format!(r#"{{"room_id":"{gym_id}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let rooms = helpers::response_json(
        app.oneshot(helpers::authed_get("/api/rooms", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let by_name = |name: &str| -> Vec<String> {
        rooms
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .map(|r| {
                r["light_ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    assert!(
        by_name("Gym").contains(&light_id),
        "light is in the assigned room"
    );
    assert!(
        !by_name("Living Room").contains(&light_id),
        "direct assignment suppresses the inherited-room membership (no duplicate)"
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
async fn v1_play_on_requires_auth() {
    let app = helpers::test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/media/play-on")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"device":"bedroom TV","query":"play x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn v1_play_on_404_when_no_tv_matches() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "k").await;
    // No remotes/TVs configured in the fixture → the resolver finds none → 404.
    let resp = app
        .oneshot(bearer_json(
            "POST",
            "/api/v1/media/play-on",
            &key,
            r#"{"device":"bedroom TV","query":"play Bob's Burgers"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn session_play_on_requires_session() {
    let app = helpers::test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/media/play-on")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"device":"tv","query":"open netflix"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
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
async fn kiosk_checkin_stores_battery_telemetry() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "Hall tablet").await;

    let r = app
        .clone()
        .oneshot(bearer_json(
            "POST",
            "/api/kiosks/checkin",
            &key,
            r#"{"app_version":"0.2","screen_on":false,"battery_level":42,
                "battery_charging":true,"battery_voltage_mv":4001,
                "battery_current_ua":850000,"battery_temp_dc":245,"power_source":"ac"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let list = helpers::response_json(
        app.oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let kiosk = &list.as_array().unwrap()[0];
    assert_eq!(kiosk["battery_level"], 42);
    assert_eq!(kiosk["battery_charging"], true);
    assert_eq!(kiosk["battery_voltage_mv"], 4001);
    assert_eq!(kiosk["battery_current_ua"], 850000);
    assert_eq!(kiosk["battery_temp_dc"], 245);
    assert_eq!(kiosk["power_source"], "ac");
    // The telemetry is optional — a check-in without it (older app) still works.
    assert_eq!(kiosk["screen_on"], false);
}

#[tokio::test]
async fn kiosk_schedule_set_reflected_and_validated() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "Bedroom tablet").await;

    // Register the kiosk via a check-in.
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
    let kiosk = &list.as_array().unwrap()[0];
    // No schedule by default.
    assert_eq!(kiosk["schedule_enabled"], false);
    assert!(kiosk["sleep_at"].is_null());
    let kiosk_id = kiosk["id"].as_str().unwrap().to_string();

    // An enabled schedule needs distinct valid HH:MM times — reject bad input.
    for bad in [
        r#"{"enabled":true,"sleep_at":"23:00","wake_at":"25:00"}"#, // hour out of range
        r#"{"enabled":true,"sleep_at":"07:00","wake_at":"07:00"}"#, // equal endpoints
        r#"{"enabled":true,"sleep_at":null,"wake_at":"07:00"}"#,    // missing time
    ] {
        let r = app
            .clone()
            .oneshot(helpers::authed_json(
                "PUT",
                &format!("/api/kiosks/{kiosk_id}/schedule"),
                &cookie,
                bad,
            ))
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "rejected: {bad}"
        );
    }

    // A valid schedule is stored and normalized (zero-padded).
    let r = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{kiosk_id}/schedule"),
            &cookie,
            r#"{"enabled":true,"sleep_at":"23:0","wake_at":"7:30"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NO_CONTENT);

    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let kiosk = &list.as_array().unwrap()[0];
    assert_eq!(kiosk["schedule_enabled"], true);
    assert_eq!(kiosk["sleep_at"], "23:00");
    assert_eq!(kiosk["wake_at"], "07:30");

    // Disabling keeps the times (so toggling off doesn't lose them).
    let r = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{kiosk_id}/schedule"),
            &cookie,
            r#"{"enabled":false,"sleep_at":"23:00","wake_at":"07:30"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let kiosk = &list.as_array().unwrap()[0];
    assert_eq!(kiosk["schedule_enabled"], false);
    assert_eq!(kiosk["sleep_at"], "23:00");
}

#[tokio::test]
async fn kiosk_schedule_requires_session() {
    let app = helpers::test_app_with_password().await;
    let r = app
        .oneshot(anon_json(
            "PUT",
            "/api/kiosks/x/schedule",
            r#"{"enabled":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
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
async fn kiosk_command_accepts_update() {
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
            r#"{"command":"update"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn kiosk_update_config_roundtrips_and_validates() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Defaults are returned before anything is set.
    let body = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks/update/config", &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body["repo"], "others-git/bifrost-kiosk");
    assert_eq!(body["asset"], "app-release.apk");

    // A bad repo slug is rejected.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/kiosks/update/config",
            &cookie,
            r#"{"repo":"nobody","asset":"app-release.apk"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // A valid update persists and reads back.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/kiosks/update/config",
            &cookie,
            r#"{"repo":"acme/kiosk","asset":"app-release-slim.apk"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(
        app.oneshot(helpers::authed_get("/api/kiosks/update/config", &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body["repo"], "acme/kiosk");
    assert_eq!(body["asset"], "app-release-slim.apk");
}

#[tokio::test]
async fn kiosk_update_config_requires_session() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/kiosks/update/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn kiosk_update_endpoints_require_api_key() {
    let app = helpers::test_app_with_password().await;
    for uri in ["/api/kiosks/update/manifest", "/api/kiosks/update/apk"] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "{uri} should need a key"
        );
    }
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

    // Public: capture the room as a Room Scene, list it, activate it, delete it.
    let resp = app
        .clone()
        .oneshot(bearer_json(
            "POST",
            "/api/v1/scenes",
            &key,
            &format!(r#"{{"name":"Movie","room_id":"{room_id}"}}"#),
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
    assert_eq!(scenes[0]["name"], "Movie");
    // It's scoped to the room (a Room Scene, not whole-home).
    assert_eq!(scenes[0]["room_id"], room_id);

    let resp = app
        .clone()
        .oneshot(bearer_json(
            "POST",
            &format!("/api/v1/scenes/{scene_id}/activate"),
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
async fn v1_scene_create_rejects_empty_name() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "app").await;

    let resp = app
        .oneshot(bearer_json(
            "POST",
            "/api/v1/scenes",
            &key,
            r#"{"name":"  "}"#,
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
        .oneshot(helpers::authed_get("/api/media/devices", cookie))
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
        ("GET", "/api/media/devices", "{}"),
        ("GET", "/api/media/devices/some-id", "{}"),
        ("PUT", "/api/media/devices/some-id/state", "{}"),
        ("GET", "/api/media/devices/some-id/favorites", "{}"),
        (
            "POST",
            "/api/media/devices/some-id/favorites/play",
            r#"{"favorite_id":"x"}"#,
        ),
        (
            "POST",
            "/api/media/devices/some-id/group",
            r#"{"coordinator_id":"x"}"#,
        ),
        ("POST", "/api/media/devices/some-id/ungroup", "{}"),
        (
            "PUT",
            "/api/media/devices/some-id/receiver",
            r#"{"receiver_id":"x"}"#,
        ),
        (
            "PUT",
            "/api/media/devices/some-id/companion",
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
    // Discovery multiplexes over the shared eISCP link, whose test-mode
    // heartbeat (100ms, 2 silent probes → reconnect + backoff) can tear the
    // socket mid-exchange when a loaded CI scheduler starves the mock's task —
    // a transient 502 against a healthy mock. Retry through the turbulence;
    // a real regression still fails after the deadline.
    let mut status = StatusCode::INTERNAL_SERVER_ERROR;
    // A generous ladder (~15s budget): a starved release runner (tests share
    // the box with a Docker build) can stall the mock long enough for several
    // heartbeat breaks in a row — each retry is cheap, and a real regression
    // still fails once the ladder is exhausted.
    for attempt in 0..20u32 {
        let resp = app
            .clone()
            .oneshot(helpers::authed_post(
                &format!("/api/providers/{provider_id}/discover"),
                cookie,
                "{}",
            ))
            .await
            .unwrap();
        status = resp.status();
        if status == StatusCode::OK {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(
            250 + 50 * u64::from(attempt),
        ))
        .await;
    }
    assert_eq!(
        status,
        StatusCode::OK,
        "onkyo mock discovery never succeeded"
    );
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/media/devices", cookie))
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

/// Wait for a specific eISCP command substring to arrive at a mock device.
async fn wait_for_command(
    recorded: &std::sync::Arc<tokio::sync::Mutex<Vec<String>>>,
    needle: &str,
) -> bool {
    // Generous ceiling: under a fully parallel `cargo test` run the eISCP mock
    // sockets contend for the runtime and a 2s budget flaked ~1 run in 3
    // (rotating between this family's tests). The loop returns the moment the
    // command lands, so the ceiling only costs time on a genuine failure.
    for _ in 0..200 {
        if recorded.lock().await.iter().any(|c| c.contains(needle)) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn receiver_mirrors_the_bound_sources_power_both_ways() {
    let (port_s, src_cmds) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let (port_r, rcv_cmds) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let source = add_onkyo_device(&app, &cookie, port_s, "TV Zone", &[]).await;
    let receiver = add_onkyo_device(
        &app,
        &cookie,
        port_r,
        "Receiver",
        std::slice::from_ref(&source),
    )
    .await;
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/media/devices/{source}/receiver"),
            &cookie,
            &format!(r#"{{"receiver_id":"{receiver}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The bound pair is one appliance: power-off on the source takes the
    // receiver down with it…
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/media/devices/{source}/state"),
            &cookie,
            r#"{"power":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        wait_for_command(&src_cmds, "PWR00").await,
        "the source must power off"
    );
    assert!(
        wait_for_command(&rcv_cmds, "PWR00").await,
        "the receiver must mirror the source's power-off"
    );

    // …and power-on wakes both.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/media/devices/{source}/state"),
            &cookie,
            r#"{"power":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        wait_for_command(&src_cmds, "PWR01").await,
        "the source must power on"
    );
    assert!(
        wait_for_command(&rcv_cmds, "PWR01").await,
        "the receiver must wake with the source"
    );
}

#[tokio::test]
async fn composite_power_mirrors_to_a_companions_bound_receiver() {
    // The BRAVIA shape: power routes to the composite's most authoritative
    // member, but the receiver binding lives on a DIFFERENT member (the HA
    // twin). The bound pair is one appliance — composite power must still
    // reach the receiver.
    let (port_a, cmds_a) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let (port_b, cmds_b) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let (port_r, rcv_cmds) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let a = add_onkyo_device(&app, &cookie, port_a, "TV", &[]).await;
    let b = add_onkyo_device(&app, &cookie, port_b, "TV twin", std::slice::from_ref(&a)).await;
    let receiver = add_onkyo_device(&app, &cookie, port_r, "AVR", &[a.clone(), b.clone()]).await;

    // Merge b into a → one composite. The surface direction is derived; find it.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/media/devices/{b}/companion"),
            &cookie,
            &format!(r#"{{"primary_id":"{a}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/media/devices", &cookie))
        .await
        .unwrap();
    let devices = helpers::response_json(resp).await;
    let companion_of = |id: &str| {
        devices
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == id)
            .and_then(|d| d["companion_of"].as_str().map(str::to_string))
    };
    let (surface, member) = if companion_of(&a).is_some() {
        (b.clone(), a.clone())
    } else {
        (a.clone(), b.clone())
    };
    let (surface_cmds, member_cmds) = if surface == a {
        (&cmds_a, &cmds_b)
    } else {
        (&cmds_b, &cmds_a)
    };

    // Bind the NON-surface member to the receiver — power will route to the
    // surface, so only the cross-member mirror can reach the receiver.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/media/devices/{member}/receiver"),
            &cookie,
            &format!(r#"{{"receiver_id":"{receiver}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Power-on the composite: the surface wakes AND the receiver mirrors on.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/media/devices/{surface}/state"),
            &cookie,
            r#"{"power":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        wait_for_command(surface_cmds, "PWR01").await,
        "the composite's power member must power on"
    );
    assert!(
        wait_for_command(&rcv_cmds, "PWR01").await,
        "the receiver bound to the companion must mirror power-on"
    );
    // The bound member is the same physical device the surface already woke —
    // it must not receive its own power command.
    assert!(
        !member_cmds.lock().await.iter().any(|c| c == "PWR01"),
        "the bound member itself must not be sent power"
    );

    // …and power-off takes the receiver down too.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/media/devices/{surface}/state"),
            &cookie,
            r#"{"power":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        wait_for_command(&rcv_cmds, "PWR00").await,
        "the receiver must mirror the composite's power-off"
    );
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
            &format!("/api/media/devices/{source}/receiver"),
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
            &format!("/api/media/devices/{source}/receiver"),
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
            &format!("/api/media/devices/{source}/receiver"),
            &cookie,
            &format!(r#"{{"receiver_id":"{receiver}","receiver_source":"Game"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            &format!("/api/media/devices/{source}"),
            &cookie,
        ))
        .await
        .unwrap();
    let dev = helpers::response_json(resp).await;
    assert_eq!(dev["receiver_id"], receiver);
    assert_eq!(dev["receiver_source"], "Game");
    // The read resolves the bound receiver's name so the "Volume → <receiver>"
    // overlay renders from the device alone, with no per-surface id lookup.
    assert_eq!(dev["receiver_name"], "Onkyo receiver (127.0.0.1)");

    // Chaining is rejected: the receiver can't itself be bound to a bound device.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/media/devices/{receiver}/receiver"),
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
            &format!("/api/media/devices/{source}/receiver"),
            &cookie,
            r#"{"receiver_id":null}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_get(
            &format!("/api/media/devices/{source}"),
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
            &format!("/api/media/devices/{companion}/companion"),
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
            &format!("/api/media/devices/{companion}/companion"),
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
            &format!("/api/media/devices/{companion}/companion"),
            &cookie,
            &format!(r#"{{"primary_id":"{primary}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The two now form one composite group. Resolution is direction-independent:
    // exactly one is the derived surface, the other its (hidden) companion
    // pointing at it — we don't care which way the surface fell.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/media/devices", &cookie))
        .await
        .unwrap();
    let list = helpers::response_json(resp).await;
    let get = |id: &str| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == id)
            .unwrap()
            .clone()
    };
    let (comp, prim) = (get(&companion), get(&primary));
    let grouped = (comp["companion_of"] == serde_json::json!(primary)
        && prim["companion_of"].is_null())
        || (prim["companion_of"] == serde_json::json!(companion) && comp["companion_of"].is_null());
    assert!(grouped, "merged devices should form one composite group");

    // Merging again the other way is idempotent (already one group), not an error
    // — the flat group model has no directional "chain" to reject.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/media/devices/{primary}/companion"),
            &cookie,
            &format!(r#"{{"primary_id":"{companion}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Unmerge.
    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/media/devices/{companion}/companion"),
            &cookie,
            r#"{"primary_id":null}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

/// "Unravel" a composite: `POST …/dissolve` clears the whole group so every
/// member becomes a standalone device again — the one-click cleanup for a
/// composite that grew wrong.
#[tokio::test]
async fn dissolve_composite_unmerges_the_whole_group() {
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

    // Merge them into one composite.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/media/devices/{companion}/companion"),
            &cookie,
            &format!(r#"{{"primary_id":"{primary}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Unravel: dissolve the composite.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/media/devices/{primary}/dissolve"),
            &cookie,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Both are standalone again — neither is a companion of the other.
    let list = helpers::response_json(
        app.oneshot(helpers::authed_get("/api/media/devices", &cookie))
            .await
            .unwrap(),
    )
    .await;
    for id in [&primary, &companion] {
        let d = list
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == id.as_str())
            .unwrap();
        assert!(
            d["companion_of"].is_null(),
            "device {id} should be standalone after unravel: {d}"
        );
    }
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
            &format!("/api/media/devices/{source}/receiver"),
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
            &format!("/api/media/devices/{source}/state"),
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
            &format!("/api/media/devices/{source}/receiver"),
            &cookie,
            r#"{"receiver_id":null}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/media/devices/{source}/state"),
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
            &format!("/api/media/devices/{source}/receiver"),
            &cookie,
            &format!(r#"{{"receiver_id":"{receiver}"}}"#),
        ))
        .await
        .unwrap();

    let list = helpers::response_json(
        app.oneshot(helpers::authed_get("/api/media/devices", &cookie))
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
            &format!("/api/rooms/{room_id}/media"),
            &cookie,
            &format!(
                r#"{{"devices":[{{"media_device_id":"{source}"}},{{"media_device_id":"{receiver}"}}]}}"#
            ),
        ))
        .await
        .unwrap();
    // Bind the source to the receiver.
    app.clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/media/devices/{source}/receiver"),
            &cookie,
            &format!(r#"{{"receiver_id":"{receiver}"}}"#),
        ))
        .await
        .unwrap();

    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/media/state"),
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
        .oneshot(helpers::authed_get("/api/media/devices", cookie))
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
            &format!("/api/media/devices/{device_id}/favorites"),
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
            &format!("/api/media/devices/{device_id}/favorites/play"),
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
    assert_eq!(mirrors[0]["domain"], "media");
    assert_eq!(mirrors[0]["media_device_ids"][0], device_id);
    assert_eq!(mirrors[0]["light_ids"].as_array().unwrap().len(), 0);

    // The room exists, links the audio mirror, and its audio device resolves
    // through that link.
    let resp = app
        .oneshot(helpers::authed_get("/api/rooms", &cookie))
        .await
        .unwrap();
    let rooms = helpers::response_json(resp).await;
    assert_eq!(rooms[0]["name"], "Living Room");
    assert_eq!(rooms[0]["media_devices"][0]["media_device_id"], device_id);
    assert_eq!(rooms[0]["links"][0]["name"], "Living Room");
    assert_eq!(rooms[0]["links"][0]["domain"], "media");
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
            &format!("/api/media/devices/{device_id}/favorites/play"),
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
        .oneshot(helpers::authed_get("/api/media/devices", cookie))
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
            &format!("/api/media/devices/{kitchen}/group"),
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
            &format!("/api/media/devices/{kitchen}/ungroup"),
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
            &format!("/api/media/devices/{kitchen}/group"),
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
            "/api/media/devices/nope/group",
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
            &format!("/api/media/devices/{kitchen}/group"),
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
    assert_eq!(onkyo["kind"], "media");
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
            &format!("/api/media/devices/{device_id}"),
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
            &format!("/api/media/devices/{device_id}/state"),
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
            &format!("/api/media/devices/{device_id}/state"),
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
            "/api/media/devices/nope/state",
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
                .uri("/api/v1/media/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // List + live get with key.
    let resp = app
        .clone()
        .oneshot(bearer_get("/api/v1/media/devices", &key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let devices = helpers::response_json(resp).await;
    assert_eq!(devices.as_array().unwrap().len(), 1);

    let resp = app
        .clone()
        .oneshot(bearer_get(
            &format!("/api/v1/media/devices/{device_id}"),
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
            &format!("/api/v1/media/devices/{device_id}/state"),
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
                .uri("/api/rooms/some-id/media")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"devices":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn room_media_membership_skips_disabled_devices() {
    // A disabled media device is never a valid room member — the save guard
    // drops it, so a stale disabled member (e.g. an HA duplicate later
    // disabled) self-cleans on any room save instead of lingering.
    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    sqlx::query("INSERT INTO providers (id, provider_type, name, credentials) VALUES ('p', 'ha', 'HA', 'x')")
        .execute(&state.db)
        .await
        .unwrap();
    for (id, en) in [("enabled-tv", 1), ("disabled-dup", 0)] {
        sqlx::query(
            "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, enabled)
             VALUES (?, 'p', ?, 'Bedroom TV', 'tv', '{}', '{}', ?)",
        )
        .bind(id)
        .bind(id)
        .bind(en)
        .execute(&state.db)
        .await
        .unwrap();
    }
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/rooms",
            &cookie,
            r#"{"name":"Bedroom","light_ids":[]}"#,
        ))
        .await
        .unwrap();
    let room_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Try to add BOTH — the disabled one must be dropped.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/media"),
            &cookie,
            r#"{"devices":[{"media_device_id":"enabled-tv","volume_offset":0},{"media_device_id":"disabled-dup","volume_offset":0}]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let members: Vec<String> =
        sqlx::query_scalar("SELECT media_device_id FROM room_media_devices WHERE room_id = ?")
            .bind(&room_id)
            .fetch_all(&state.db)
            .await
            .unwrap();
    assert_eq!(
        members,
        vec!["enabled-tv".to_string()],
        "only the enabled device persists"
    );
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
            &format!("/api/rooms/{room_id}/media"),
            &cookie,
            r#"{"devices":[{"media_device_id":"nope"}]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/rooms/nope/media",
            &cookie,
            &format!(r#"{{"devices":[{{"media_device_id":"{device_id}"}}]}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Add the device with an offset; see it (with offset) in session + v1.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/media"),
            &cookie,
            &format!(r#"{{"devices":[{{"media_device_id":"{device_id}","volume_offset":-6}}]}}"#),
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
    assert_eq!(rooms[0]["media_devices"][0]["media_device_id"], device_id);
    assert_eq!(rooms[0]["media_devices"][0]["volume_offset"], -6);

    let key = create_api_key(&app, &cookie, "mcp").await;
    let resp = app
        .clone()
        .oneshot(bearer_get("/api/v1/rooms", &key))
        .await
        .unwrap();
    assert_eq!(
        helpers::response_json(resp).await[0]["media_device_ids"][0],
        device_id
    );

    // Clear with an empty list.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/media"),
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
        helpers::response_json(resp).await[0]["media_devices"],
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
        .oneshot(helpers::authed_get("/api/media/devices", &cookie))
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
        r#"{{"devices":[{{"media_device_id":"{dev_a}","volume_offset":0}},{{"media_device_id":"{dev_b}","volume_offset":-6}}]}}"#
    );
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/media"),
            &cookie,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/media/state"),
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
            &format!("/api/rooms/{room_id}/media/state"),
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
    assert_eq!(onkyo["domain"], "media");
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
        "activate_scene",
        "save_room_scene",
        "save_home_scene",
        "set_media",
        "play_media_favorite",
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
            "get_media_state",
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
            "get_media_state",
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
    assert!(snapshot["media_devices"].is_array());
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
async fn voice_speak_without_auth_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/voice/speak")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"text":"hello"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn voice_speak_without_tts_endpoint_returns_503() {
    // Talk-back degrades gracefully: with no `tts` role configured, /speak says so
    // with a 503 rather than failing opaquely (text control is unaffected).
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_json(
            "POST",
            "/api/voice/speak",
            &cookie,
            r#"{"text":"hello"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn voice_speak_synthesizes_via_configured_tts() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Mock TTS endpoint returning audio bytes for the configured `tts` role.
    let tts = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/mpeg")
                .set_body_bytes(b"ID3speech".to_vec()),
        )
        .mount(&tts)
        .await;

    let cfg = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/ai-endpoints/tts",
            &cookie,
            &format!(r#"{{"base_url":"{}","model":"tts-1"}}"#, tts.uri()),
        ))
        .await
        .unwrap();
    assert!(
        cfg.status().is_success(),
        "configuring tts: {:?}",
        cfg.status()
    );

    let resp = app
        .oneshot(helpers::authed_json(
            "POST",
            "/api/voice/speak",
            &cookie,
            r#"{"text":"hello there"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "audio/mpeg"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"ID3speech");
}

#[tokio::test]
async fn voice_speak_honors_disabled_tts_endpoint() {
    // Disabling a role must actually take effect: a configured-but-disabled `tts`
    // endpoint is treated as absent, so /speak degrades to 503 (the Settings
    // Enabled switch flips this flag).
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Configure tts, then disable it.
    for body in [
        r#"{"base_url":"http://127.0.0.1:9/v1","model":"tts-1","enabled":true}"#,
        r#"{"base_url":"http://127.0.0.1:9/v1","model":"tts-1","enabled":false}"#,
    ] {
        let cfg = app
            .clone()
            .oneshot(helpers::authed_json(
                "PUT",
                "/api/ai-endpoints/tts",
                &cookie,
                body,
            ))
            .await
            .unwrap();
        assert!(
            cfg.status().is_success(),
            "configuring tts: {:?}",
            cfg.status()
        );
    }

    let resp = app
        .oneshot(helpers::authed_json(
            "POST",
            "/api/voice/speak",
            &cookie,
            r#"{"text":"hello"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
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
            &format!("/api/media/devices/{audio_id}/room"),
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

#[tokio::test]
async fn room_effect_drives_lights_not_audio() {
    // A room-wide dynamic effect (the room editor's Effects tab) is a lighting
    // attribute like color — it must reach the member lights but never power on
    // the room's speakers (an effect-only patch has no brightness/color/temp, so
    // it must not be mistaken for a pure on/off power command).
    let (port, audio_cmds) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let bridge = wled_mock().await;
    let (app, light_id) = helpers::test_app_with_light(&bridge.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let audio_id = add_onkyo_device(&app, &cookie, port, "AV", &[]).await;

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
            &format!("/api/media/devices/{audio_id}/room"),
            &cookie,
            &format!(r#"{{"room_id":"{room_id}"}}"#),
        ))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/state"),
            &cookie,
            r#"{"on":true,"effect":"candle"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The effect was persisted onto the member light …
    let light = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get(
                &format!("/api/lights/{light_id}"),
                &cookie,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(light["last_state"]["effect"], "candle", "{light}");
    // … but the audio member was never powered on (no PWR set command).
    let powered = audio_cmds
        .lock()
        .await
        .iter()
        .any(|m| m.starts_with("PWR") && !m.contains("QSTN"));
    assert!(!powered, "room effect must not power on the room's audio");
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
async fn sensor_devices_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/sensors/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn sensor_devices_list_is_empty_by_default() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let list = helpers::response_json(
        app.oneshot(helpers::authed_get("/api/sensors/devices", &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(list.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn v1_sensors_without_key_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/sensors/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn v1_sensors_list_with_key_returns_ok() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "k").await;
    let resp = app
        .clone()
        .oneshot(bearer_get("/api/v1/sensors/devices", &key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let list = helpers::response_json(resp).await;
    assert_eq!(list.as_array().unwrap().len(), 0);

    // Unknown sensor id → 404 (not a 500 / empty body).
    let resp = app
        .oneshot(bearer_get("/api/v1/sensors/devices/nope", &key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn sensor_rules_require_a_session() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/automations")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn sensor_rule_crud_and_validation() {
    let ha = ha_remote_mock().await;
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    sqlx::query(
        "INSERT INTO sensor_devices (id, provider_id, device_id, name, kind, last_state)
         VALUES ('m1', ?, 'binary_sensor.hall', 'Hall motion', 'motion', '{\"reading\":{\"bool\":false}}')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();

    // A threshold trigger on a motion sensor can never fire — rejected.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "POST",
            "/api/automations",
            &cookie,
            r#"{"trigger":{"kind":"sensor","sensor_id":"m1","event":{"kind":"rose_above","value":5}},
                "steps":[{"actions":[{"kind":"power","device_id":"p1","on":true}]}]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // A rule with no actions is rejected.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "POST",
            "/api/automations",
            &cookie,
            r#"{"trigger":{"kind":"sensor","sensor_id":"m1","event":{"kind":"became_true"}},"steps":[]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Happy path: motion → room on, gated to darkness, overnight window.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "POST",
            "/api/automations",
            &cookie,
            r#"{"name":"Hall night light",
                "trigger":{"kind":"sensor","sensor_id":"m1","event":{"kind":"became_true"}},
                "conditions":[{"kind":"time_window","start":"21:00","end":"06:00"}],
                "steps":[{"actions":[{"kind":"room","room_id":"r1","state":{"on":true,"brightness":30}}]}],
                "cooldown_secs":60}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let rule = helpers::response_json(resp).await;
    let rule_id = rule["id"].as_str().unwrap().to_string();
    assert_eq!(rule["name"], "Hall night light");
    assert_eq!(rule["trigger"]["kind"], "sensor");
    assert_eq!(rule["trigger"]["event"]["kind"], "became_true");
    assert!(rule["last_fired_at"].is_null());

    // Update flips it to a clear-for rule.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/automations/{rule_id}"),
            &cookie,
            r#"{"name":"Hall off","enabled":false,
                "trigger":{"kind":"sensor","sensor_id":"m1","event":{"kind":"clear_for","secs":600}},
                "steps":[{"actions":[{"kind":"room","room_id":"r1","state":{"on":false}}]}]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let rule = helpers::response_json(resp).await;
    assert_eq!(rule["trigger"]["event"]["secs"], 600);
    assert_eq!(rule["enabled"], false);

    // List reflects the stored rule; delete removes it.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/automations", &cookie))
        .await
        .unwrap();
    assert_eq!(
        helpers::response_json(resp).await.as_array().unwrap().len(),
        1
    );
    let resp = app
        .clone()
        .oneshot(helpers::authed_delete(
            &format!("/api/automations/{rule_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .oneshot(helpers::authed_get("/api/automations", &cookie))
        .await
        .unwrap();
    assert!(
        helpers::response_json(resp)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
}

/// Per-step conditions: a rule's "then" is a list of steps, each optionally
/// gated. Two steps target different switches; the second is gated by a
/// condition that passes in one run (an all-day time window) and fails in the
/// other (a reading gate on a sensor that doesn't exist → fails closed) — so
/// only the ungated step runs then. Proves steps gate independently at fire time.
#[tokio::test]
async fn per_step_conditions_gate_each_step_independently() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // First run: an always-true window → both steps run. Second run: a gate on
    // a nonexistent sensor (fails closed) → only the ungated step runs.
    let passing = r#"{"kind":"time_window","start":"00:00","end":"00:00"}"#;
    let failing = r#"{"kind":"sensor_below","sensor_id":"ghost","value":10}"#;
    for (gate, expect_second) in [(passing, true), (failing, false)] {
        let ha = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/services/homeassistant/turn_on"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&ha)
            .await;
        let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
        let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
        for (id, entity) in [("always", "switch.always"), ("gated", "switch.gated")] {
            sqlx::query(
                "INSERT INTO power_devices (id, provider_id, device_id, name, kind, last_state)
                 VALUES (?, ?, ?, ?, 'switch', '{\"on\":false}')",
            )
            .bind(id)
            .bind(&prov_id)
            .bind(entity)
            .bind(entity)
            .execute(&db)
            .await
            .unwrap();
        }
        let body = format!(
            r#"{{"name":"Stepped","trigger":{{"kind":"manual"}},"steps":[
                {{"actions":[{{"kind":"power","device_id":"always","on":true}}]}},
                {{"conditions":[{gate}],
                  "actions":[{{"kind":"power","device_id":"gated","on":true}}]}}
              ]}}"#
        );
        let resp = app
            .clone()
            .oneshot(helpers::authed_post("/api/automations", &cookie, &body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let id = helpers::response_json(resp).await["id"]
            .as_str()
            .unwrap()
            .to_string();
        let resp = app
            .clone()
            .oneshot(helpers::authed_post(
                &format!("/api/automations/{id}/run"),
                &cookie,
                "{}",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let reqs = ha.received_requests().await.unwrap();
        let hit = |needle: &str| {
            reqs.iter()
                .any(|r| String::from_utf8_lossy(&r.body).contains(needle))
        };
        assert!(hit("switch.always"), "the ungated step always runs");
        assert_eq!(
            hit("switch.gated"),
            expect_second,
            "gated step should run={expect_second} for gate {gate}"
        );
    }
}

#[tokio::test]
async fn run_automation_executes_actions_immediately() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let ha = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/services/homeassistant/turn_on"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&ha)
        .await;
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    sqlx::query(
        "INSERT INTO sensor_devices (id, provider_id, device_id, name, kind, last_state)
         VALUES ('m1', ?, 'binary_sensor.hall', 'Hall motion', 'motion', '{\"reading\":{\"bool\":false}}')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO power_devices (id, provider_id, device_id, name, kind, last_state)
         VALUES ('p1', ?, 'switch.fan', 'Fan', 'switch', '{\"on\":false}')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO automations (id, sensor_id, trigger_json, actions_json)
         VALUES ('a1', 'm1', '{\"kind\":\"sensor\",\"sensor_id\":\"m1\",\"event\":{\"kind\":\"became_true\"}}',
                 '[{\"conditions\":[],\"actions\":[{\"kind\":\"power\",\"device_id\":\"p1\",\"on\":true}]}]')",
    )
    .execute(&db)
    .await
    .unwrap();

    // Run now: actions execute (skipping trigger/conditions), last-fired stamps.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/automations/a1/run",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        ha.received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path() == "/api/services/homeassistant/turn_on"),
        "manual run must reach the power provider"
    );
    let stamped: Option<String> =
        sqlx::query_scalar("SELECT last_fired_at FROM automations WHERE id = 'a1'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert!(stamped.is_some(), "manual run must stamp last_fired_at");

    // Unknown id → 404.
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/automations/nope/run",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// A MANUAL rule (the AIO button's engine): saves with no event input, never
/// event-fires, and `POST /{id}/run` executes a mixed action list — including
/// the new `app` action, which launches through the shared remote command
/// path (recents recording and all).
#[tokio::test]
async fn manual_rule_with_app_action_runs_on_demand() {
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let ha = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/services/homeassistant/turn_off"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&ha)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/services/remote/turn_on"))
        .and(body_string_contains("com.hulu.livingroomplus"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&ha)
        .await;
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    sqlx::query(
        "INSERT INTO power_devices (id, provider_id, device_id, name, kind, last_state)
         VALUES ('p1', ?, 'switch.lamp', 'Lamp', 'switch', '{\"on\":true}')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO remote_devices (id, provider_id, device_id, name, last_state)
         VALUES ('r1', ?, 'remote.bedroom_tv', 'Bedroom TV', '{}')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();

    // Create through the real route — validation must accept a manual trigger.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/automations",
            &cookie,
            r#"{"name":"Bedroom movie","enabled":true,
                "trigger":{"kind":"manual"},
                "conditions":[],
                "steps":[{"actions":[
                  {"kind":"power","device_id":"p1","on":false},
                  {"kind":"app","remote_id":"r1","app":"com.hulu.livingroomplus"}
                ]}]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/automations/{id}/run"),
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let reqs = ha.received_requests().await.unwrap();
    assert!(
        reqs.iter()
            .any(|r| r.url.path() == "/api/services/homeassistant/turn_off"),
        "the power action must reach the provider"
    );
    assert!(
        reqs.iter()
            .any(|r| r.url.path() == "/api/services/remote/turn_on"),
        "the app action must launch through the shared remote path"
    );
    // The launch registered as a recent in the app catalog — proof it went
    // through apply_remote_command, not a bespoke dispatch.
    let seen: Option<String> =
        sqlx::query_scalar("SELECT package FROM remote_apps WHERE remote_id = 'r1'")
            .fetch_optional(&db)
            .await
            .unwrap();
    assert_eq!(seen.as_deref(), Some("com.hulu.livingroomplus"));
}

/// The toggle action is *relative*: it reads the device's cached power and
/// applies the inverse (what a macro button wants). A device whose state is on
/// gets turned off, off gets turned on, and an unknown state is skipped.
#[tokio::test]
async fn toggle_action_inverts_cached_power_state() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let ha = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/services/homeassistant/turn_off"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&ha)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/services/homeassistant/turn_on"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&ha)
        .await;
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // The fan is currently ON.
    sqlx::query(
        "INSERT INTO power_devices (id, provider_id, device_id, name, kind, last_state)
         VALUES ('fan1', ?, 'switch.fan', 'Ceiling Fan', 'fan', '{\"on\":true}')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO automations (id, trigger_json, actions_json)
         VALUES ('t1', '{\"kind\":\"manual\"}',
                 '[{\"conditions\":[],\"actions\":[{\"kind\":\"toggle\",\"domain\":\"power\",\"device_id\":\"fan1\"}]}]')",
    )
    .execute(&db)
    .await
    .unwrap();

    // Run: ON → the toggle must call turn_OFF.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/automations/t1/run",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        ha.received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path() == "/api/services/homeassistant/turn_off"),
        "toggling an ON device must turn it off"
    );

    // Flip the cache to OFF and run again: the toggle must now turn ON.
    sqlx::query("UPDATE power_devices SET last_state = '{\"on\":false}' WHERE id = 'fan1'")
        .execute(&db)
        .await
        .unwrap();
    let resp = app
        .oneshot(helpers::authed_post(
            "/api/automations/t1/run",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        ha.received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path() == "/api/services/homeassistant/turn_on"),
        "toggling an OFF device must turn it on"
    );
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

/// `list_remotes` hides an integration (HA) remote when a native remote shares
/// its composite group — the same physical TV reached two ways. Their hardware
/// ids differ (each provider reports its own), so exact-MAC shadowing can't
/// catch it; the group is the join. A lone integration remote (no native
/// provider for its TV) must still show.
#[tokio::test]
async fn list_remotes_hides_the_integration_duplicate_sharing_a_group() {
    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let enc = state.encrypt_credentials("{}").unwrap();
    for (id, ptype) in [("nativep", "smarttv"), ("hap", "ha")] {
        sqlx::query(
            "INSERT INTO providers (id, provider_type, name, credentials) VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(ptype)
        .bind(ptype)
        .bind(&enc)
        .execute(&state.db)
        .await
        .unwrap();
    }
    // Two remotes for one TV share group 'g1' (native + HA duplicate), each
    // with its own MAC; a third HA remote is alone in 'g2'.
    for (id, prov, name, hw, group) in [
        ("r-native", "nativep", "BRAVIA", "mac:aa", "g1"),
        ("r-ha-dup", "hap", "BRAVIA VU1", "mac:bb", "g1"),
        ("r-ha-solo", "hap", "Bedroom TV", "mac:cc", "g2"),
    ] {
        sqlx::query(
            "INSERT INTO remote_devices (id, provider_id, device_id, name, hw_id, group_id)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(prov)
        .bind(id)
        .bind(name)
        .bind(hw)
        .bind(group)
        .execute(&state.db)
        .await
        .unwrap();
    }

    let body = helpers::response_json(
        app.oneshot(helpers::authed_get("/api/remote/devices", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"BRAVIA"),
        "the native remote stays: {names:?}"
    );
    assert!(
        names.contains(&"Bedroom TV"),
        "the lone HA remote stays: {names:?}"
    );
    assert!(
        !names.contains(&"BRAVIA VU1"),
        "the HA duplicate sharing a group with the native must hide: {names:?}"
    );
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
async fn media_device_exposes_paired_remote_id() {
    let ha = ha_remote_mock().await;
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Seed a TV media device and an enabled remote paired to it — the linkage the
    // hw_id reconciler establishes for an Android TV's media_player + remote.
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, hw_id, group_id)
         VALUES ('tv1', ?, 'media_player.bedroom_tv', 'Bedroom TV', 'tv', '{}', '{\"power\":true,\"volume\":0,\"mute\":false}', 'mac:aa', 'g1')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO remote_devices (id, provider_id, device_id, name, enabled, hw_id, group_id)
         VALUES ('rem1', ?, 'remote.bedroom_tv', 'Bedroom TV', 1, 'mac:aa', 'g1')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    // A standalone speaker (no remote) and a disabled remote both read as null.
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, hw_id, group_id)
         VALUES ('spk1', ?, 'media_player.kitchen', 'Kitchen', 'speaker', '{}', '{\"power\":false,\"volume\":0,\"mute\":false}', 'mac:bb', 'g2')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO remote_devices (id, provider_id, device_id, name, enabled, hw_id, group_id)
         VALUES ('rem2', ?, 'remote.kitchen', 'Kitchen', 0, 'mac:bb', 'g2')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();

    let resp = app
        .oneshot(helpers::authed_get("/api/media/devices", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let devices = helpers::response_json(resp).await;
    let by = |id: &str| {
        devices
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("{id} not in media list"))
    };
    // The TV surfaces its paired remote as part of the effective device.
    assert_eq!(by("tv1")["remote_id"], "rem1");
    // A speaker whose only paired remote is disabled reports no remote.
    assert!(by("spk1")["remote_id"].is_null());
}

#[tokio::test]
async fn standby_tv_reads_on_and_reachable_via_paired_remote() {
    let ha = ha_remote_mock().await;
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // The TV's media_player reads unavailable (reachable:false, power:false) — the
    // standby case — but its paired remote reports the box on + reachable.
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, group_id)
         VALUES ('tv1', ?, 'media_player.bedroom_tv', 'Bedroom TV', 'tv', '{}', '{\"power\":false,\"volume\":0,\"mute\":false,\"reachable\":false}', 'g1')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO remote_devices (id, provider_id, device_id, name, enabled, group_id, last_state)
         VALUES ('rem1', ?, 'remote.bedroom_tv', 'Bedroom TV', 1, 'g1', '{\"on\":true,\"reachable\":true}')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();

    let resp = app
        .oneshot(helpers::authed_get("/api/media/devices", &cookie))
        .await
        .unwrap();
    let devices = helpers::response_json(resp).await;
    let tv = devices
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "tv1")
        .expect("tv in list");
    // The composite resolves on + reachable from the remote — not "offline".
    assert_eq!(tv["state"]["power"], true);
    assert_eq!(tv["state"]["reachable"], true);
}

#[tokio::test]
async fn remote_surfaces_on_the_primary_when_paired_to_a_companion() {
    let ha = ha_remote_mock().await;
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Two rows for one physical TV: a surface (primary) the user merged the other
    // into, and the merged-in companion the remote is hw-paired to. The remote
    // belongs to the composite, so it must surface on the primary regardless of
    // which row it's literally paired to.
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, group_id)
         VALUES ('tvP', ?, 'media_player.tv', 'BRAVIA', 'tv', '{}', '{\"power\":true,\"volume\":0,\"mute\":false}', 'g')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, group_id)
         VALUES ('tvC', ?, 'bravia.tv', 'BRAVIA', 'speaker', '{}', '{\"power\":true,\"volume\":0,\"mute\":false}', 'g')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO remote_devices (id, provider_id, device_id, name, enabled, group_id)
         VALUES ('rem', ?, 'remote.tv', 'BRAVIA', 1, 'g')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();

    let resp = app
        .oneshot(helpers::authed_get("/api/media/devices", &cookie))
        .await
        .unwrap();
    let devices = helpers::response_json(resp).await;
    let primary = devices
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "tvP")
        .expect("primary in list");
    assert_eq!(primary["remote_id"], "rem");
}

#[tokio::test]
async fn dev_composite_routing_reports_control_precedence() {
    let ha = ha_remote_mock().await;
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // A TV primary + a merged companion + a paired remote — the canonical composite.
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, group_id)
         VALUES ('tvP', ?, 'media_player.tv', 'BRAVIA', 'tv', '{\"transport\":true,\"sources\":true}', '{\"power\":true,\"volume\":0,\"mute\":false}', 'g')",
    )
    .bind(&prov_id).execute(&db).await.unwrap();
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, group_id)
         VALUES ('tvC', ?, 'bravia.tv', 'Living Room TV', 'speaker', '{\"transport\":true}', '{\"power\":true,\"volume\":40,\"mute\":false}', 'g')",
    )
    .bind(&prov_id).execute(&db).await.unwrap();
    sqlx::query(
        "INSERT INTO remote_devices (id, provider_id, device_id, name, enabled, group_id)
         VALUES ('rem', ?, 'remote.tv', 'BRAVIA', 1, 'g')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query("UPDATE config SET dev_mode = 1 WHERE id = 1")
        .execute(&db)
        .await
        .unwrap();

    let resp = app
        .oneshot(helpers::authed_get("/api/dev/media/tvP/routing", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let routes = helpers::response_json(resp).await;
    let routes = routes.as_array().unwrap();
    let by = |c: &str| {
        routes
            .iter()
            .find(|r| r["control"] == c)
            .unwrap_or_else(|| panic!("no route {c}"))
    };
    // Volume routes to the companion that actually carries audio (volume 40).
    assert_eq!(by("volume / mute")["device_id"], "tvC");
    // The paired remote surfaces, named.
    assert_eq!(by("remote keys / apps")["device_id"], "rem");
}

#[tokio::test]
async fn dev_composite_routing_404_when_dev_mode_off() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_get(
            "/api/dev/media/whatever/routing",
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn inventory_mutations_announce_on_the_sse_channel() {
    // A rename (and glyph/enable/room/shadow) must fire the app-wide
    // `inventory` broadcast, so Control / Boards / other clients refresh their
    // device lists live instead of showing the old name until a reload.
    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    sqlx::query(
        "INSERT INTO providers (id, name, provider_type, credentials) VALUES ('p1','P','wled','x')",
    )
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO lights (id, provider_id, device_id, name, provider_name, capabilities, last_state)
         VALUES ('l1','p1','d1','Nanoleaf Light Panels','Nanoleaf Light Panels','{}','{}')",
    )
    .execute(&state.db)
    .await
    .unwrap();

    let mut rx = state.inventory_events.subscribe();
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/lights/l1/name")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"name":"Wall Panel"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        rx.try_recv().expect("rename announces an inventory event"),
        "lights"
    );

    // A failed mutation (unknown id) stays silent.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/lights/nope/name")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"name":"X"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(rx.try_recv().is_err(), "a 404 must not announce");
}

#[tokio::test]
async fn friendly_name_sticks_and_reverts() {
    let ha = ha_remote_mock().await;
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, provider_name, kind, capabilities, last_state)
         VALUES ('amp', ?, 'main', 'Onkyo receiver (192.168.1.34)', 'Onkyo receiver (192.168.1.34)', 'receiver', '{}', '{\"power\":true,\"volume\":0,\"mute\":false}')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();

    // Rename to a friendly name.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/media/devices/amp/name",
            &cookie,
            r#"{"name":"Living Room Amp"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let name: String = sqlx::query_scalar("SELECT name FROM media_devices WHERE id = 'amp'")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(name, "Living Room Amp");

    // Clearing it reverts to the provider's discovered name.
    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/media/devices/amp/name",
            &cookie,
            r#"{"name":""}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let name: String = sqlx::query_scalar("SELECT name FROM media_devices WHERE id = 'amp'")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(name, "Onkyo receiver (192.168.1.34)");
}

#[tokio::test]
async fn dev_device_raw_returns_ha_attributes() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let ha = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/states/climate.bedroom"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entity_id": "climate.bedroom",
            "state": "heat",
            "attributes": {
                "supported_features": 1,
                "current_temperature": 19.5,
                "temperature": 21,
                "hvac_modes": ["off", "heat", "cool"],
                "friendly_name": "Bedroom"
            }
        })))
        .mount(&ha)
        .await;

    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    sqlx::query("UPDATE config SET dev_mode = 1 WHERE id = 1")
        .execute(&db)
        .await
        .unwrap();

    let resp = app
        .oneshot(helpers::authed_get(
            &format!("/api/dev/devices/{prov_id}/climate.bedroom/raw"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = helpers::response_json(resp).await;
    assert_eq!(j["domain"], "climate");
    assert_eq!(j["supported_features"], 1);
    assert_eq!(j["attributes"]["current_temperature"], 19.5);
}

#[tokio::test]
async fn dev_event_journal_serves_and_clears_entries() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Off by default → invisible.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/dev/events", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    enable_dev_mode(&app, &cookie).await;

    // Seed the process-wide journal directly (tests install no tracing
    // subscriber; the layer's capture has its own unit test).
    bifrost::journal::Journal::global().record(
        "DEBUG",
        "bifrost::automation",
        "rule fired".into(),
        Default::default(),
    );
    bifrost::journal::Journal::global().record(
        "DEBUG",
        "bifrost::voice",
        "heard".into(),
        Default::default(),
    );

    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            "/api/dev/events?target=bifrost::automation",
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    let entries = body["entries"].as_array().unwrap();
    // Other parallel tests may journal too — assert on our own entry, not counts.
    assert!(
        entries
            .iter()
            .any(|e| e["message"] == "rule fired" && e["target"] == "bifrost::automation"),
        "filtered read must include the automation entry"
    );
    assert!(
        entries.iter().all(|e| e["target"]
            .as_str()
            .unwrap()
            .starts_with("bifrost::automation")),
        "target filter must exclude other areas"
    );
    let last_seq = body["last_seq"].as_u64().unwrap();
    assert!(last_seq >= 2);

    // The cursor advances past everything already seen.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            &format!("/api/dev/events?after={last_seq}"),
            &cookie,
        ))
        .await
        .unwrap();
    let body = helpers::response_json(resp).await;
    assert!(body["entries"].as_array().unwrap().is_empty());

    // Clear empties the buffer.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post("/api/dev/events/clear", &cookie, "{}"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn dev_device_raw_404_when_dev_mode_off() {
    let ha = wiremock::MockServer::start().await; // never hit
    let (app, prov_id, _db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // dev_mode is off by default → the whole dev surface 404s.
    let resp = app
        .oneshot(helpers::authed_get(
            &format!("/api/dev/devices/{prov_id}/climate.bedroom/raw"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn generic_devices_list_and_control_via_api() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let ha = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/states"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "entity_id": "cover.blinds", "state": "open",
              "attributes": { "current_position": 60, "friendly_name": "Blinds" } }
        ])))
        .mount(&ha)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/services/cover/set_cover_position"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&ha)
        .await;

    let (app, prov_id) = helpers::test_app_with_ha(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // List surfaces the cover as a generic device with a position control.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/generic/devices", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let devices = helpers::response_json(resp).await;
    let d = &devices.as_array().unwrap()[0];
    assert_eq!(d["device_id"], "cover.blinds");
    assert_eq!(d["kind"], "cover");
    assert_eq!(d["provider_id"], prov_id);
    assert!(
        d["controls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["key"] == "position")
    );

    // A control write routes to the mapped HA service.
    let body = format!(
        r#"{{"provider_id":"{prov_id}","device_id":"cover.blinds","key":"position","value":40}}"#
    );
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/generic/devices/control",
            &cookie,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let reqs = ha.received_requests().await.unwrap();
    assert!(
        reqs.iter()
            .any(|r| r.url.path() == "/api/services/cover/set_cover_position"),
        "control write did not reach the cover service"
    );
}

#[tokio::test]
async fn generic_devices_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/generic/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn smarttv_pair_without_session_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/providers/smarttv/pair")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"host":"1.2.3.4"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn smarttv_pair_begin_reports_pin_displayed() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // A Bravia answers the first (unauthenticated) actRegister with 401 + shows a PIN.
    let tv = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sony/accessControl"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&tv)
        .await;

    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let body = format!(r#"{{"host":"{}"}}"#, tv.uri());
    let resp = app
        .oneshot(helpers::authed_json(
            "POST",
            "/api/providers/smarttv/pair",
            &cookie,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        helpers::response_json(resp).await["status"],
        "pin_displayed"
    );
}

#[tokio::test]
async fn media_power_on_also_wakes_paired_remote() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let ha = MockServer::start().await;
    for svc in [
        "media_player/turn_on",
        "media_player/turn_off",
        "remote/turn_on",
        "remote/turn_off",
    ] {
        Mock::given(method("POST"))
            .and(path(format!("/api/services/{svc}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&ha)
            .await;
    }
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, group_id)
         VALUES ('tv1', ?, 'media_player.bedroom_tv', 'Bedroom TV', 'tv', '{\"sources\":true}', '{\"power\":false,\"volume\":0,\"mute\":false}', 'g1')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO remote_devices (id, provider_id, device_id, name, enabled, group_id)
         VALUES ('rem1', ?, 'remote.bedroom_tv', 'Bedroom TV', 1, 'g1')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();

    // Power-on through the media device must drive media_player AND wake the
    // paired remote (the reliable WoL/turn_on standby wake).
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/media/devices/tv1/state",
            &cookie,
            r#"{"power":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let reqs = ha.received_requests().await.unwrap();
    assert!(
        reqs.iter()
            .any(|r| r.url.path() == "/api/services/media_player/turn_on"),
        "media_player was not powered on"
    );
    assert!(
        reqs.iter()
            .any(|r| r.url.path() == "/api/services/remote/turn_on"),
        "the paired remote was not woken on composite power-on"
    );

    // Power-off is left to the media_player — it must NOT fan to the remote.
    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/media/devices/tv1/state",
            &cookie,
            r#"{"power":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let reqs = ha.received_requests().await.unwrap();
    assert!(
        !reqs
            .iter()
            .any(|r| r.url.path() == "/api/services/remote/turn_off"),
        "power-off should not fan to the remote"
    );
}

#[tokio::test]
async fn disabled_companion_stops_merging_state_and_power() {
    let ha = ha_remote_mock().await;
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // A composite whose companion was disabled on the Devices page: its stale
    // cached state (on, volume 40) must not keep merging into the effective
    // device or feeding the composite power resolution.
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, group_id)
         VALUES ('tv1', ?, 'media_player.bedroom_tv', 'Bedroom TV', 'tv', '{}', '{\"power\":false,\"volume\":0,\"mute\":false}', 'g1')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, enabled, group_id)
         VALUES ('comp1', ?, 'media_player.bedroom_tv_cast', 'Bedroom TV Cast', 'speaker', '{}', '{\"power\":true,\"volume\":40,\"mute\":false}', 0, 'g1')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();

    let resp = app
        .oneshot(helpers::authed_get("/api/media/devices", &cookie))
        .await
        .unwrap();
    let devices = helpers::response_json(resp).await;
    let by = |id: &str| {
        devices
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("{id} not in media list"))
    };
    // The disabled companion's stale on/volume don't leak onto the surface…
    assert_eq!(by("tv1")["state"]["power"], false);
    assert_eq!(by("tv1")["state"]["volume"], 0);
    // …but it's still marked as a member, so the inventory keeps it collapsed.
    assert_eq!(by("comp1")["companion_of"], "tv1");
}

#[tokio::test]
async fn disabled_companion_does_not_attract_routed_commands() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let ha = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/services/media_player/volume_set"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&ha)
        .await;
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // The disabled companion's cached volume (40) would otherwise mark it as
    // "the backing carrying audio" and attract the routed volume command — which
    // it would then refuse (disabled), failing the whole command.
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, group_id)
         VALUES ('tv1', ?, 'media_player.bedroom_tv', 'Bedroom TV', 'tv', '{}', '{\"power\":true,\"volume\":0,\"mute\":false}', 'g1')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, enabled, group_id)
         VALUES ('comp1', ?, 'media_player.bedroom_tv_cast', 'Bedroom TV Cast', 'speaker', '{}', '{\"power\":true,\"volume\":40,\"mute\":false}', 0, 'g1')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();

    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/media/devices/tv1/state",
            &cookie,
            r#"{"volume":20}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let reqs = ha.received_requests().await.unwrap();
    let volume_calls: Vec<_> = reqs
        .iter()
        .filter(|r| r.url.path() == "/api/services/media_player/volume_set")
        .collect();
    assert_eq!(
        volume_calls.len(),
        1,
        "volume must reach exactly one backing"
    );
    let body: serde_json::Value = serde_json::from_slice(&volume_calls[0].body).unwrap();
    assert_eq!(
        body["entity_id"], "media_player.bedroom_tv",
        "volume routed to the disabled companion instead of the enabled surface"
    );
}

#[tokio::test]
async fn merging_a_hidden_duplicate_is_rejected() {
    let ha = ha_remote_mock().await;
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state)
         VALUES ('tv1', ?, 'media_player.tv', 'TV', 'tv', '{}', '{\"power\":false,\"volume\":0,\"mute\":false}')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    // A de-dup-shadowed copy — a row is never both shadowed and a group member,
    // so merging it must be rejected, not quietly grouped.
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, shadowed_by)
         VALUES ('dup1', ?, 'media_player.tv_ha', 'TV (HA)', 'tv', '{}', '{\"power\":false,\"volume\":0,\"mute\":false}', 'tv1')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();

    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/media/devices/dup1/companion",
            &cookie,
            r#"{"primary_id":"tv1"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn shadowing_a_grouped_row_migrates_its_composite_to_the_canonical() {
    let ha = ha_remote_mock().await;
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // The orphaned-composite sequence: an HA TV carries the group (and its
    // paired remote); the native row arrives later, standalone. Shadowing the
    // HA row must hand its group — remote included — to the canonical row,
    // never strand them on a hidden one.
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, group_id)
         VALUES ('tvOld', ?, 'media_player.tv', 'TV (HA)', 'tv', '{}', '{\"power\":false,\"volume\":0,\"mute\":false}', 'g1')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO remote_devices (id, provider_id, device_id, name, enabled, group_id)
         VALUES ('rem1', ?, 'remote.tv', 'TV Remote', 1, 'g1')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state)
         VALUES ('tvNew', ?, 'bravia.tv', 'TV (native)', 'tv', '{}', '{\"power\":false,\"volume\":0,\"mute\":false}')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();

    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/media/devices/tvOld/shadow",
            &cookie,
            r#"{"shadowed_by":"tvNew"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The canonical row adopted the group; the shadowed row left it.
    let group_of = |id: &str| {
        let db = db.clone();
        let id = id.to_string();
        async move {
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT group_id FROM media_devices WHERE id = ?",
            )
            .bind(&id)
            .fetch_one(&db)
            .await
            .unwrap()
        }
    };
    assert_eq!(group_of("tvNew").await.as_deref(), Some("g1"));
    assert_eq!(group_of("tvOld").await, None);
    // The paired remote now surfaces on the canonical row's effective device.
    let devices = helpers::response_json(
        app.oneshot(helpers::authed_get("/api/media/devices", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let tv_new = devices
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "tvNew")
        .expect("canonical row in list");
    assert_eq!(tv_new["remote_id"], "rem1");
}

#[tokio::test]
async fn shadowing_a_grouped_row_folds_into_the_canonicals_group() {
    let ha = ha_remote_mock().await;
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Both rows already carry groups: shadowing one folds its whole group
    // (remotes included) into the canonical's group.
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, group_id)
         VALUES ('tvOld', ?, 'media_player.tv', 'TV (HA)', 'tv', '{}', '{\"power\":false,\"volume\":0,\"mute\":false}', 'g1')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO remote_devices (id, provider_id, device_id, name, enabled, group_id)
         VALUES ('rem1', ?, 'remote.tv', 'TV Remote (HA)', 1, 'g1')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, group_id)
         VALUES ('tvNew', ?, 'bravia.tv', 'TV (native)', 'tv', '{}', '{\"power\":false,\"volume\":0,\"mute\":false}', 'g2')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();

    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/media/devices/tvOld/shadow",
            &cookie,
            r#"{"shadowed_by":"tvNew"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let remote_group: Option<String> =
        sqlx::query_scalar("SELECT group_id FROM remote_devices WHERE id = 'rem1'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(remote_group.as_deref(), Some("g2"));
    let old_group: Option<String> =
        sqlx::query_scalar("SELECT group_id FROM media_devices WHERE id = 'tvOld'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(old_group, None);
}

#[tokio::test]
async fn shadowed_member_is_never_elected_surface() {
    let ha = ha_remote_mock().await;
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // The tv-kind member would win surface election on kind — but it's shadowed,
    // so the composite must be represented by the visible member instead of
    // vanishing behind a hidden surface.
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, shadowed_by, group_id)
         VALUES ('a_tv', ?, 'media_player.tv', 'TV', 'tv', '{}', '{\"power\":false,\"volume\":0,\"mute\":false}', 'elsewhere', 'g1')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state, group_id)
         VALUES ('b_spk', ?, 'media_player.tv_speaker', 'TV Speaker', 'speaker', '{}', '{\"power\":false,\"volume\":0,\"mute\":false}', 'g1')",
    )
    .bind(&prov_id)
    .execute(&db)
    .await
    .unwrap();

    let devices = helpers::response_json(
        app.oneshot(helpers::authed_get("/api/media/devices", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let by = |id: &str| {
        devices
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("{id} not in media list"))
    };
    assert!(
        by("b_spk")["companion_of"].is_null(),
        "the visible member must be the surface"
    );
    assert_eq!(by("a_tv")["companion_of"], "b_spk");
}

#[tokio::test]
async fn assistant_say_requires_auth_and_a_real_device() {
    // The TV voice-command route: unauthenticated is 401; an unknown device is
    // 404; a real smart-TV device with no TTS endpoint configured reports a
    // clear bad-request rather than a mystery 502.
    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Unauthenticated.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/media/devices/x/assistant")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"text":"hi"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Unknown device → 404.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/media/devices/nope/assistant",
            &cookie,
            r#"{"text":"hi"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // A real smart-TV device, but no TTS endpoint configured → a clear 400
    // (bad command), not an opaque gateway error.
    let enc = state
        .encrypt_credentials(r#"{"host":"192.0.2.9","brand":"androidtv"}"#)
        .unwrap();
    sqlx::query(
        "INSERT INTO providers (id, provider_type, name, credentials) VALUES ('ptv', 'smarttv', 'TV', ?)",
    )
    .bind(&enc)
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, kind, capabilities, last_state)
         VALUES ('tv1', 'ptv', '192.0.2.9', 'Bedroom TV', 'tv', '{}', '{}')",
    )
    .execute(&state.db)
    .await
    .unwrap();
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/media/devices/tv1/assistant",
            &cookie,
            r#"{"text":"what is the weather"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "no TTS configured is a clear bad-command, not a 502"
    );
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&body);
    assert!(
        body.contains("text-to-speech"),
        "explains what's missing: {body}"
    );

    // Empty text is rejected before any provider work.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/media/devices/tv1/assistant",
            &cookie,
            r#"{"text":"   "}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn scan_filters_hosts_behind_configured_providers() {
    // An already-added TV answers the Android-TV probes too — it must not be
    // offered as addable next to itself. The per-type scan applies the same
    // known-hosts filter as the all-types scan. (The discoverers probe the
    // real network here and find nothing in CI; the seeded provider's host
    // simply must never appear.)
    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let enc = state
        .encrypt_credentials(r#"{"host":"192.0.2.22","brand":"bravia"}"#)
        .unwrap();
    sqlx::query(
        "INSERT INTO providers (id, provider_type, name, credentials) VALUES ('ptv', 'smarttv', 'TV', ?)",
    )
    .bind(&enc)
    .execute(&state.db)
    .await
    .unwrap();
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers/scan/smarttv",
            &cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let found = helpers::response_json(resp).await;
    assert!(
        found
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["host"] != "192.0.2.22"),
        "a configured host must be filtered from scan results: {found}"
    );
}

#[tokio::test]
async fn remote_apps_pull_the_tvs_installed_catalog() {
    // The launcher lists what's INSTALLED on the TV (appControl.getApplicationList
    // through the remote's provider), not just apps observed in the foreground —
    // and keeps each app's vendor launch URI so a never-opened app launches with
    // the exact token the TV expects.
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let tv = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sony/appControl"))
        .and(body_string_contains("getApplicationList"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": [[
                { "title": "Netflix", "uri": "com.netflix.ninja-com.netflix.ninja.MainActivity" },
                { "title": "Plex", "uri": "com.plexapp.android-com.plexapp.plex.activities.SplashActivity" }
            ]],
            "id": 1
        })))
        .mount(&tv)
        .await;

    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let host = tv.uri().trim_start_matches("http://").to_string();
    let enc = state
        .encrypt_credentials(&format!(r#"{{"host":"{host}"}}"#))
        .unwrap();
    sqlx::query(
        "INSERT INTO providers (id, provider_type, name, credentials) VALUES ('ptv', 'smarttv', 'TV', ?)",
    )
    .bind(&enc)
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO remote_devices (id, provider_id, device_id, name) VALUES ('r1', 'ptv', ?, 'BRAVIA')",
    )
    .bind(&host)
    .execute(&state.db)
    .await
    .unwrap();

    let apps = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/remote/devices/r1/apps", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let apps = apps.as_array().unwrap();
    assert_eq!(
        apps.len(),
        2,
        "the full installed catalog is listed: {apps:?}"
    );
    let netflix = apps.iter().find(|a| a["name"] == "Netflix").unwrap();
    assert_eq!(netflix["package"], "com.netflix.ninja");
    assert_eq!(
        netflix["activity"], "com.netflix.ninja-com.netflix.ninja.MainActivity",
        "the vendor launch URI rides along"
    );
    assert_eq!(netflix["pinned"], false);

    // Pinning + a later re-sync keep the catalog identity stable (idempotent
    // upsert: pin preserved, title/activity refreshed, no duplicate rows).
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/remote/devices/r1/apps/pin",
            &cookie,
            r#"{"package":"com.netflix.ninja","pinned":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let apps = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/remote/devices/r1/apps", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let apps = apps.as_array().unwrap();
    assert_eq!(apps.len(), 2, "re-sync must not duplicate rows");
    assert_eq!(apps[0]["name"], "Netflix", "pinned floats first");
    assert_eq!(apps[0]["pinned"], true);
    assert_eq!(
        apps[0]["activity"],
        "com.netflix.ninja-com.netflix.ninja.MainActivity"
    );

    // A row keyed by a full launch URI (minted by pre-normalization launch
    // recording) merges into its bare-package row on the next sync — recency
    // carried over, no duplicate tile.
    sqlx::query(
        "INSERT INTO remote_apps (remote_id, package, name, pinned, last_seen)
         VALUES ('r1', 'com.plexapp.android-com.plexapp.plex.activities.SplashActivity', 'Plex', 0, datetime('now'))",
    )
    .execute(&state.db)
    .await
    .unwrap();
    let apps = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/remote/devices/r1/apps", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let apps = apps.as_array().unwrap();
    let plex: Vec<_> = apps.iter().filter(|a| a["name"] == "Plex").collect();
    assert_eq!(
        plex.len(),
        1,
        "launch-URI row merges, not duplicates: {apps:?}"
    );
    assert_eq!(plex[0]["package"], "com.plexapp.android");
    assert!(
        plex[0]["last_seen"].is_string(),
        "the URI row's recency carries into the merged row"
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
async fn remote_command_pin_route_is_session_gated_and_persists() {
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

    // Unauthenticated pin is rejected like every other remote route.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/remote/devices/{remote_id}/commands/pin"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"token":"AAAA","pinned":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Pinning, then unpinning, a native command token both succeed (insert + delete).
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/remote/devices/{remote_id}/commands/pin"),
            &cookie,
            r#"{"token":"AAAA","pinned":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Re-pinning the same token is idempotent (ON CONFLICT DO NOTHING).
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/remote/devices/{remote_id}/commands/pin"),
            &cookie,
            r#"{"token":"AAAA","pinned":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/remote/devices/{remote_id}/commands/pin"),
            &cookie,
            r#"{"token":"AAAA","pinned":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
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

/// Kiosk microphone presence, end to end through the shared sensor pipeline:
/// enabling the mic mints a real occupancy sensor assigned to the kiosk's room;
/// a noise edge posted by the kiosk flips the ROOM's occupancy (persist →
/// occupancy recompute all ride the same writer task as any provider); moving
/// the kiosk moves the sensor's membership; disabling removes the sensor.
#[tokio::test]
async fn kiosk_mic_becomes_a_room_occupancy_sensor() {
    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "wall tablet").await;

    // Register the kiosk and give it a room.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/kiosks/checkin")
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let kiosks = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let kiosk_id = kiosks[0]["id"].as_str().unwrap().to_string();
    assert_eq!(kiosks[0]["mic_presence"], false);
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
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{kiosk_id}/room"),
            &cookie,
            &format!(r#"{{"room_id":"{room_id}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Enable the mic: a real occupancy sensor appears, assigned to the room.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{kiosk_id}/mic"),
            &cookie,
            r#"{"enabled":true,"sensitivity":"high"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let (sensor_id, kind): (String, String) = sqlx::query_as(
        "SELECT id, kind FROM sensor_devices WHERE provider_id = 'kiosk-sensors' AND device_id = ?",
    )
    .bind(&kiosk_id)
    .fetch_one(&state.db)
    .await
    .expect("mic sensor row must exist");
    assert_eq!(kind, "occupancy");
    let member_room: Option<String> =
        sqlx::query_scalar("SELECT room_id FROM room_sensor_devices WHERE sensor_device_id = ?")
            .bind(&sensor_id)
            .fetch_optional(&state.db)
            .await
            .unwrap();
    assert_eq!(member_room.as_deref(), Some(room_id.as_str()));
    let kiosks = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(kiosks[0]["mic_presence"], true);
    assert_eq!(kiosks[0]["mic_sensitivity"], "high");

    // The kiosk reports an elevated edge → the ROOM reads occupied (the event
    // rides the shared writer pipeline, so poll for the async persist).
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/kiosks/self/noise")
                .header(header::COOKIE, format!("bfr_key={key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"elevated":true,"level":-21.5}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let mut occupied = false;
    for _ in 0..200 {
        let rooms = helpers::response_json(
            app.clone()
                .oneshot(helpers::authed_get("/api/rooms", &cookie))
                .await
                .unwrap(),
        )
        .await;
        let room = rooms
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == room_id.as_str())
            .unwrap()
            .clone();
        if room["occupancy"] == true {
            occupied = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(occupied, "an elevated noise edge must flip room occupancy");
    let kiosks = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(kiosks[0]["mic_level"], -21.5);

    // Unassigning the kiosk's room moves the sensor's membership with it.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{kiosk_id}/room"),
            &cookie,
            r#"{"room_id":null}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let member_room: Option<String> =
        sqlx::query_scalar("SELECT room_id FROM room_sensor_devices WHERE sensor_device_id = ?")
            .bind(&sensor_id)
            .fetch_optional(&state.db)
            .await
            .unwrap();
    assert_eq!(member_room, None);

    // Disable: the sensor row (and any membership) is gone; junk sensitivity 422s.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{kiosk_id}/mic"),
            &cookie,
            r#"{"enabled":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let remaining: Option<String> =
        sqlx::query_scalar("SELECT id FROM sensor_devices WHERE provider_id = 'kiosk-sensors'")
            .fetch_optional(&state.db)
            .await
            .unwrap();
    assert_eq!(remaining, None);
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{kiosk_id}/mic"),
            &cookie,
            r#"{"enabled":true,"sensitivity":"eleven"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn room_sensor_membership_and_presence_opt_out_roundtrip() {
    let ha = ha_remote_mock().await;
    let (app, prov_id, db) = helpers::test_app_with_ha_db(&ha.uri()).await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Two presence sensors: one detecting, one clear.
    for (id, detecting) in [("hall", true), ("couch", false)] {
        sqlx::query(
            "INSERT INTO sensor_devices (id, provider_id, device_id, name, kind, last_state)
             VALUES (?, ?, ?, ?, 'motion', ?)",
        )
        .bind(id)
        .bind(&prov_id)
        .bind(format!("binary_sensor.{id}"))
        .bind(id)
        .bind(format!(r#"{{"reading":{{"bool":{detecting}}}}}"#))
        .execute(&db)
        .await
        .unwrap();
    }

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

    // Assign both sensors; the room lists them and reads occupied (hall detects).
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/sensors"),
            &cookie,
            r#"{"sensor_ids":["hall","couch"]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let room_json = |rooms: serde_json::Value| {
        rooms
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == room_id)
            .unwrap()
            .clone()
    };
    let rooms = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/rooms", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let room = room_json(rooms);
    assert_eq!(room["sensor_ids"], serde_json::json!(["couch", "hall"]));
    assert_eq!(room["direct_sensor_ids"].as_array().unwrap().len(), 2);
    assert_eq!(room["occupancy"], serde_json::json!(true));

    // Opt the detecting sensor out → still a member, but the room reads empty.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/presence"),
            &cookie,
            r#"{"excluded_sensor_ids":["hall"]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let rooms = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/rooms", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let room = room_json(rooms);
    assert_eq!(room["sensor_ids"], serde_json::json!(["couch", "hall"]));
    assert_eq!(room["presence_excluded"], serde_json::json!(["hall"]));
    assert_eq!(room["occupancy"], serde_json::json!(false));

    // Unknown ids are rejected on both endpoints.
    for (path, body) in [
        ("sensors", r#"{"sensor_ids":["nope"]}"#),
        ("presence", r#"{"excluded_sensor_ids":["nope"]}"#),
    ] {
        let resp = app
            .clone()
            .oneshot(helpers::authed_json(
                "PUT",
                &format!("/api/rooms/{room_id}/{path}"),
                &cookie,
                body,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
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
        .oneshot(helpers::authed_get("/api/media/devices", &cookie))
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
async fn disabled_power_device_drops_out_of_room_membership() {
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
    app.clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/rooms/{room_id}/power"),
            &cookie,
            &format!(r#"{{"power_device_ids":["{id}"]}}"#),
        ))
        .await
        .unwrap();

    // Enabled → the room lists it as a member.
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
    assert_eq!(room["power_device_ids"], serde_json::json!([id]));

    // Disable it → it must drop out of room membership (was leaking before).
    app.clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/power/devices/{id}/enabled"),
            &cookie,
            r#"{"enabled":false}"#,
        ))
        .await
        .unwrap();
    let resp = app
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
    assert_eq!(room["power_device_ids"], serde_json::json!([]));
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

// ── Developer mode (/api/dev, gated behind config.dev_mode) ──────────────────

/// Flip dev mode on via the partial settings PUT (session-authed, ungated).
async fn enable_dev_mode(app: &Router, cookie: &str) {
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/settings",
            cookie,
            r#"{"dev_mode":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "enabling dev mode failed");
}

#[tokio::test]
async fn dev_routes_404_when_dev_mode_off() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    // Even a valid session gets 404 — the surface doesn't exist in production.
    let resp = app
        .oneshot(helpers::authed_get("/api/dev/info", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dev_routes_401_when_on_but_unauthenticated() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    enable_dev_mode(&app, &cookie).await;
    // No cookie, no bearer key.
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/dev/info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dev_info_ok_with_session_when_on() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    enable_dev_mode(&app, &cookie).await;
    let resp = app
        .oneshot(helpers::authed_get("/api/dev/info", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["dev_mode"], true);
    assert!(body["version"].is_string(), "missing version: {body}");
}

#[tokio::test]
async fn dev_info_ok_with_bearer_when_on() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    enable_dev_mode(&app, &cookie).await;
    let key = create_api_key(&app, &cookie, "dev script").await;
    let resp = app
        .oneshot(bearer_get("/api/dev/info", &key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn settings_partial_put_preserves_other_field() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Save a subnet (dev_mode omitted — should stay default false).
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/settings",
            &cookie,
            r#"{"expanded_lan_scan":["192.168.5.0/24"]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["dev_mode"], false);
    assert_eq!(body["expanded_lan_scan"][0], "192.168.5.0/24");

    // Toggle dev_mode only — the subnet must survive (partial update).
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            "/api/settings",
            &cookie,
            r#"{"dev_mode":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["dev_mode"], true, "dev_mode not set: {body}");
    assert_eq!(
        body["expanded_lan_scan"][0], "192.168.5.0/24",
        "subnet clobbered by a dev_mode-only PUT: {body}"
    );
}

// ── Auto-discovery (/api/providers/discover-all) ─────────────────────────────

#[tokio::test]
async fn discover_all_requires_session() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/providers/discover-all")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn discover_all_returns_a_json_array_when_authed() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_get("/api/providers/discover-all", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    // No devices on the test host's LAN — but the shape is always an array.
    assert!(body.is_array(), "expected an array, got {body}");
}

// ── Casting (/api/media/devices/{id}/cast) ───────────────────────────────────

#[tokio::test]
async fn cast_requires_session() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/media/devices/some-id/cast")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"content_id":"x","content_type":"url"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn v1_cast_requires_key() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/media/devices/some-id/cast")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"content_id":"x","content_type":"url"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn v1_cast_with_key_reaches_the_cast_service() {
    // Bearer-authed cast for a nonexistent device → the shared cast_to_device
    // returns NotFound → 404. Proves the v1 route is wired through auth to the
    // same service the session route uses (the provider call itself is covered by
    // the HA play_media wiremock test).
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "auto").await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/media/devices/nonexistent/cast")
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"content_id":"x","content_type":"url"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── Light segments (per-segment colour control) ──────────────────────────────

#[tokio::test]
async fn v1_segments_without_key_returns_401() {
    let app = helpers::test_app_with_password().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/lights/some-id/segments")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"segments":[{"segment":0,"rgb":255}]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn segments_on_provider_without_support_returns_502() {
    // The route is wired through to the provider; WLED has no segment control, so
    // its default `set_segments` errors → BAD_GATEWAY (proves the call reaches it).
    let (app, light_id) = helpers::test_app_with_light("http://127.0.0.1:1").await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}/segments"),
            &cookie,
            r#"{"segments":[{"segment":0,"rgb":16711680}]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
}
