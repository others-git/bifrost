mod helpers;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

use helpers::{anon_json, audio_mock, create_api_key, setup_onkyo, wled_mock};

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

/// End to end over the wire: a device event reaches an ALREADY-OPEN stream even
/// though its provider's push channel was created afterwards. A wall tablet
/// holds this connection for days, so anything that restarts a manager (a
/// credential edit, a pairing, a relocate rebind) must not leave the board
/// silently deaf until someone reloads it.
#[tokio::test]
async fn events_stream_carries_a_channel_created_after_the_client_connected() {
    use futures_util::StreamExt;

    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .oneshot(helpers::authed_get("/api/events", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let mut body = resp.into_body().into_data_stream();

    // Mint the pseudo-provider's sensor channel now that the client is already
    // listening, then push a reading through it.
    bifrost::api::kiosks::ensure_kiosk_sensor_channel(&state).await;
    let tx = {
        let connections = state.connections.lock().await;
        connections
            .sensor_sender(bifrost::api::kiosks::KIOSK_SENSOR_PROVIDER)
            .expect("sensor channel")
    };
    tx.send(bifrost::connection::SensorEvent {
        device_id: "sensor-1".into(),
        state: bifrost::models::sensor::SensorState::boolean(true),
    })
    .unwrap();

    let mut seen = String::new();
    let found = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(Ok(chunk)) = body.next().await {
            seen.push_str(&String::from_utf8_lossy(&chunk));
            if seen.contains("event: sensor_state") {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(found, "no sensor_state frame on the stream; saw: {seen:?}");
    assert!(
        seen.contains("sensor-1"),
        "frame lost its device id: {seen:?}"
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

// ── Auth gates ──────────────────────────────────────────────────────────────

//
// One table per auth mechanism, so a new route's gate is a row rather than a
// copy of the same fifteen-line test. Bodies are well-formed, so what answers
// is the auth check and not body validation.

#[tokio::test]
async fn session_gated_routes_return_401() {
    let app = helpers::test_app_with_password().await;
    for (method, uri, body) in [
        ("GET", "/api/events", ""),
        ("GET", "/api/lights", ""),
        ("GET", "/api/lights/some-id", ""),
        ("PUT", "/api/lights/some-id/glyph", r#"{"glyph":"bulb"}"#),
        ("PUT", "/api/lights/some-id/room", r#"{"room_id":"x"}"#),
        (
            "PUT",
            "/api/lights/some-id/shadow",
            r#"{"shadowed_by":"x"}"#,
        ),
        ("GET", "/api/providers/types", ""),
        ("GET", "/api/providers/discover-all", ""),
        ("GET", "/api/providers/some-id/config", ""),
        ("PUT", "/api/providers/order", r#"{"order":["x"]}"#),
        (
            "PUT",
            "/api/providers/some-id/credentials",
            r#"{"credentials":{}}"#,
        ),
        ("PUT", "/api/providers/x/prune", r#"{"prune":true}"#),
        ("POST", "/api/providers/x/sync-groups", ""),
        ("POST", "/api/providers/scan/onkyo", ""),
        (
            "POST",
            "/api/providers/hue/pair",
            r#"{"bridge_ip":"192.168.1.10"}"#,
        ),
        (
            "POST",
            "/api/providers/nanoleaf/pair",
            r#"{"host":"192.168.1.20"}"#,
        ),
        (
            "POST",
            "/api/providers/smarttv/pair",
            r#"{"host":"1.2.3.4"}"#,
        ),
        ("POST", "/api/providers/whatever/smarttv/pair-remote", "{}"),
        ("GET", "/api/scenes", ""),
        ("POST", "/api/scenes/restore-default", "{}"),
        ("PUT", "/api/scenes/some-id/default", r#"{"default":true}"#),
        ("GET", "/api/dashboards", ""),
        ("GET", "/api/palettes", ""),
        ("GET", "/api/plans", ""),
        (
            "PUT",
            "/api/plans/some-id/size",
            r#"{"width":6,"height":6}"#,
        ),
        ("PUT", "/api/plans/some-id/rooms", r#"{"rooms":[]}"#),
        ("PUT", "/api/plans/some-id/media", r#"{"placements":[]}"#),
        ("GET", "/api/rooms", ""),
        ("PUT", "/api/rooms/some-id/enabled", r#"{"enabled":false}"#),
        ("PUT", "/api/rooms/some-id/media", r#"{"devices":[]}"#),
        ("PUT", "/api/rooms/some-id/controls", r#"{"controls":[]}"#),
        ("GET", "/api/media/devices", ""),
        ("GET", "/api/media/devices/some-id", ""),
        ("PUT", "/api/media/devices/some-id/state", "{}"),
        ("GET", "/api/media/devices/some-id/favorites", ""),
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
        (
            "POST",
            "/api/media/devices/some-id/cast",
            r#"{"content_id":"x","content_type":"url"}"#,
        ),
        (
            "POST",
            "/api/media/play-on",
            r#"{"device":"tv","query":"open netflix"}"#,
        ),
        ("GET", "/api/power/devices", ""),
        ("GET", "/api/sensors/devices", ""),
        ("GET", "/api/remote/devices", ""),
        ("GET", "/api/generic/devices", ""),
        ("POST", "/api/enrollment", "{}"),
        ("GET", "/api/kiosks", ""),
        ("POST", "/api/kiosks/x/command", r#"{"command":"sleep"}"#),
        ("PUT", "/api/kiosks/x/schedule", r#"{"enabled":false}"#),
        ("GET", "/api/kiosks/update/config", ""),
        ("GET", "/api/api-keys", ""),
        ("POST", "/api/api-keys", r#"{"name":"x"}"#),
        ("GET", "/api/settings", ""),
        ("PUT", "/api/settings", r#"{"expanded_lan_scan":[]}"#),
        ("GET", "/api/ai-endpoints", ""),
        ("GET", "/api/voice/vocabulary", ""),
        (
            "POST",
            "/api/voice/command",
            r#"{"text":"turn off the office"}"#,
        ),
        ("POST", "/api/voice/speak", r#"{"text":"hello"}"#),
        ("GET", "/api/feeds/sources", ""),
        ("GET", "/api/feeds/some-id/libraries", ""),
        ("GET", "/api/feeds/some-id/recent?library=1", ""),
        ("GET", "/api/feeds/some-id/image?path=%2Fx", ""),
    ] {
        let resp = app
            .clone()
            .oneshot(anon_json(method, uri, body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

/// Routes a kiosk reaches with its API key rather than a browser session: an
/// anonymous request is still rejected.
#[tokio::test]
async fn api_key_gated_routes_return_401() {
    let app = helpers::test_app_with_password().await;
    for (method, uri, body) in [
        ("POST", "/api/kiosks/checkin", "{}"),
        ("GET", "/api/kiosks/stream", ""),
        ("GET", "/api/kiosks/update/manifest", ""),
        ("GET", "/api/kiosks/update/apk", ""),
    ] {
        let resp = app
            .clone()
            .oneshot(anon_json(method, uri, body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

/// The public API is Bearer-only — a session cookie doesn't open it and an
/// anonymous request never does.
#[tokio::test]
async fn v1_routes_without_bearer_return_401() {
    let app = helpers::test_app_with_password().await;
    for (method, uri, body) in [
        ("GET", "/api/v1/lights", ""),
        (
            "PUT",
            "/api/v1/lights/some-id/segments",
            r#"{"segments":[{"segment":0,"rgb":255}]}"#,
        ),
        ("GET", "/api/v1/sensors/devices", ""),
        ("GET", "/api/v1/power/devices", ""),
        (
            "POST",
            "/api/v1/media/play-on",
            r#"{"device":"bedroom TV","query":"play x"}"#,
        ),
    ] {
        let resp = app
            .clone()
            .oneshot(anon_json(method, uri, body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

// ── Lights — auth guard ─────────────────────────────────────────────────────

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
    use bifrost::connection::relocate::{lost_devices, relocate_with_candidates};
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
    let lost = lost_devices(&state).await;
    assert_eq!(lost.len(), 1);
    assert_eq!(lost[0].known_hw, vec!["mac:aabbccddeeff".to_string()]);
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
    let lost = lost_devices(&state).await;
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
    let lost = lost_devices(&state).await;
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

/// The relocator is provider-agnostic: it heals ANY type that declares a
/// `LanBinding`, reading the address from that type's own credential field
/// (`device_ip` for Kasa, not `host`) and proving identity through the type's
/// own check — here a plug's `get_sysinfo` MAC against its recorded `hw_id`.
#[tokio::test]
async fn relocator_heals_a_non_tv_provider_through_its_own_host_field() {
    use bifrost::connection::relocate::{lost_devices, relocate_with_candidates};
    use bifrost::providers::discovery::DiscoveredDevice;

    let (_app, state) = helpers::test_app_with_password_and_state().await;
    let stored_ip = |enc: &str| {
        let j = state.decrypt_credentials(enc).unwrap();
        serde_json::from_str::<serde_json::Value>(&j).unwrap()["device_ip"]
            .as_str()
            .unwrap()
            .to_string()
    };
    async fn creds_of(db: &sqlx::SqlitePool, id: &str) -> String {
        sqlx::query_scalar::<_, String>("SELECT credentials FROM providers WHERE id = ?")
            .bind(id)
            .fetch_one(db)
            .await
            .unwrap()
    }

    let enc = state
        .encrypt_credentials(r#"{"device_ip":"127.0.0.1:1"}"#)
        .unwrap();
    sqlx::query(
        "INSERT INTO providers (id, provider_type, name, credentials) VALUES ('k1','kasa','Raven Lights',?)",
    )
    .bind(&enc)
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO power_devices (id, provider_id, device_id, name, kind, hw_id)
         VALUES ('p1','k1','main','Raven Lights','outlet','mac:aabbccddeeff')",
    )
    .execute(&state.db)
    .await
    .unwrap();

    // Lost, with the identity read from the POWER table (not media) and the
    // address read from `device_ip` (not `host`).
    let lost = lost_devices(&state).await;
    assert_eq!(lost.len(), 1);
    assert_eq!(lost[0].provider_type, "kasa");
    assert_eq!(lost[0].host, "127.0.0.1:1");
    assert_eq!(lost[0].known_hw, vec!["mac:aabbccddeeff".to_string()]);

    // A candidate carrying a DIFFERENT hardware id is refused; the plug is not
    // reachable to interrogate live either, so nothing is adopted.
    let healed = relocate_with_candidates(
        &state,
        lost,
        &[DiscoveredDevice {
            host: "192.0.2.50".into(),
            label: None,
            credentials: serde_json::json!({"device_ip": "192.0.2.50", "hw_id": "mac:112233445566"}),
        }],
    )
    .await;
    assert!(healed.is_empty());
    assert_eq!(
        stored_ip(&creds_of(&state.db, "k1").await),
        "127.0.0.1:1",
        "a wrong-identity candidate must never be adopted"
    );

    // The real plug, found by the broadcast leg that stamps its MAC, is adopted.
    let lost = lost_devices(&state).await;
    let healed = relocate_with_candidates(
        &state,
        lost,
        &[DiscoveredDevice {
            host: "192.0.2.51".into(),
            label: None,
            credentials: serde_json::json!({"device_ip": "192.0.2.51", "hw_id": "mac:aabbccddeeff"}),
        }],
    )
    .await;
    assert_eq!(healed, vec!["k1".to_string()]);
    assert_eq!(stored_ip(&creds_of(&state.db, "k1").await), "192.0.2.51");
}

/// A candidate sitting at an address ANOTHER provider row is already configured
/// at is never adopted: it's a device Bifrost already manages, so rebinding onto
/// it would point two rows at one device instead of healing anything. This is
/// reachable for a household-scoped binding — every Sonos player's MAC is
/// recorded under the row that discovered it, so a sibling row's host really
/// does satisfy the identity check.
#[tokio::test]
async fn relocator_never_adopts_a_host_another_provider_already_owns() {
    use bifrost::connection::relocate::{lost_devices, relocate_with_candidates};
    use bifrost::providers::discovery::DiscoveredDevice;

    let (_app, state) = helpers::test_app_with_password_and_state().await;

    // Two Sonos rows for one household. Row A's seed is dead; row B is healthy
    // and configured at 192.0.2.60.
    for (id, host) in [("sonosA", "127.0.0.1:1"), ("sonosB", "192.0.2.60")] {
        let enc = state
            .encrypt_credentials(&format!(r#"{{"host":"{host}"}}"#))
            .unwrap();
        sqlx::query(
            "INSERT INTO providers (id, provider_type, name, credentials) VALUES (?,'sonos',?,?)",
        )
        .bind(id)
        .bind(id)
        .bind(&enc)
        .execute(&state.db)
        .await
        .unwrap();
    }
    // Row A discovered the whole household, so row B's player is among its
    // known hardware ids — the identity check would happily accept it.
    sqlx::query(
        "INSERT INTO media_devices (id, provider_id, device_id, name, hw_id)
         VALUES ('s1','sonosA','RINCON_A','Kitchen','mac:949f3e123456'),
                ('s2','sonosA','RINCON_B','Den','mac:949f3e654321')",
    )
    .execute(&state.db)
    .await
    .unwrap();

    let lost = lost_devices(&state).await;
    assert_eq!(lost.len(), 1, "only row A's host is dead");
    assert_eq!(lost[0].provider_id, "sonosA");

    let healed = relocate_with_candidates(
        &state,
        lost,
        &[DiscoveredDevice {
            host: "192.0.2.60".into(),
            label: None,
            credentials: serde_json::json!({"host": "192.0.2.60"}),
        }],
    )
    .await;
    assert!(
        healed.is_empty(),
        "an address another row already owns must never be adopted"
    );
}

/// A provider with nothing that could ever prove a replacement is skipped before
/// the scan, not swept for forever. A `host:<ip>` hardware id is derived from the
/// very address that changed, so it can never match a moved device.
#[tokio::test]
async fn relocator_skips_a_provider_no_candidate_could_ever_prove() {
    use bifrost::connection::relocate::lost_devices;

    let (_app, state) = helpers::test_app_with_password_and_state().await;
    let enc = state
        .encrypt_credentials(
            r#"{"host":"127.0.0.1:1","brand":"androidtv","hw_id":"host:10.0.0.5"}"#,
        )
        .unwrap();
    sqlx::query(
        "INSERT INTO providers (id, provider_type, name, credentials) VALUES ('tv','smarttv','Dongle',?)",
    )
    .bind(&enc)
    .execute(&state.db)
    .await
    .unwrap();

    assert!(
        lost_devices(&state).await.is_empty(),
        "unreachable, but no scan could ever produce a provable candidate"
    );

    // Give it a real hardware id and it becomes relocatable again.
    let enc = state
        .encrypt_credentials(
            r#"{"host":"127.0.0.1:1","brand":"androidtv","hw_id":"mac:aabbccddeeff"}"#,
        )
        .unwrap();
    sqlx::query("UPDATE providers SET credentials = ? WHERE id = 'tv'")
        .bind(&enc)
        .execute(&state.db)
        .await
        .unwrap();
    assert_eq!(lost_devices(&state).await.len(), 1);
}

/// A cloud-only provider declares no `LanBinding`, so the relocator ignores it
/// entirely — there is no stored LAN address to go stale.
#[tokio::test]
async fn relocator_ignores_providers_with_no_lan_binding() {
    use bifrost::connection::relocate::lost_devices;

    let (_app, state) = helpers::test_app_with_password_and_state().await;
    let enc = state
        .encrypt_credentials(r#"{"host":"127.0.0.1:1","token":"t"}"#)
        .unwrap();
    sqlx::query(
        "INSERT INTO providers (id, provider_type, name, credentials) VALUES ('h1','ha','Home Assistant',?)",
    )
    .bind(&enc)
    .execute(&state.db)
    .await
    .unwrap();

    assert!(
        lost_devices(&state).await.is_empty(),
        "a user-typed server URL is not a discovered device address"
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

// ── Provider pairing (link-button style) ─────────────────────────────────────

/// The pairing route is a thin map from what the bridge answers to an HTTP
/// outcome: accepted → the app key, refused → 409 with a machine-readable
/// reason, unreachable → 502. (The protocol itself is covered by the provider's
/// own unit tests.)
#[tokio::test]
async fn hue_pair_maps_outcomes_to_status() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    for (reply, expect_status, expect_field) in [
        (
            Some(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "success": { "username": "paired-key-123", "clientkey": "deadbeef" } }
            ]))),
            StatusCode::OK,
            Some(("app_key", "paired-key-123")),
        ),
        (
            Some(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "error": { "type": 101, "address": "", "description": "link button not pressed" } }
            ]))),
            StatusCode::CONFLICT,
            Some(("error", "link_button_not_pressed")),
        ),
        // Nothing listens on port 9 → the bridge is unreachable.
        (None, StatusCode::BAD_GATEWAY, None),
    ] {
        let (_bridge, bridge_ip) = match reply {
            Some(template) => {
                let bridge = MockServer::start().await;
                Mock::given(method("POST"))
                    .and(path("/api"))
                    .respond_with(template)
                    .mount(&bridge)
                    .await;
                let ip = bridge.uri();
                (Some(bridge), ip)
            }
            None => (None, "127.0.0.1:9".to_string()),
        };

        let body = format!(r#"{{"bridge_ip":"{bridge_ip}"}}"#);
        let resp = app
            .clone()
            .oneshot(helpers::authed_post(
                "/api/providers/hue/pair",
                &cookie,
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), expect_status, "{bridge_ip}");
        if let Some((field, value)) = expect_field {
            let json = helpers::response_json(resp).await;
            assert_eq!(json[field], value, "{json}");
        }
    }
}

/// Nanoleaf's equivalent map: a token when the controller is in pairing mode, a
/// 409 when its power button wasn't held (the controller answers 403), 502 when
/// it can't be reached.
#[tokio::test]
async fn nanoleaf_pair_maps_outcomes_to_status() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    for (reply, expect_status, expect_field) in [
        (
            Some(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "auth_token": "nl-tok" })),
            ),
            StatusCode::OK,
            Some(("auth_token", "nl-tok")),
        ),
        (
            Some(ResponseTemplate::new(403)),
            StatusCode::CONFLICT,
            Some(("error", "not_in_pairing_mode")),
        ),
        (None, StatusCode::BAD_GATEWAY, None),
    ] {
        let (_controller, host) = match reply {
            Some(template) => {
                let controller = MockServer::start().await;
                Mock::given(method("POST"))
                    .and(path("/api/v1/new"))
                    .respond_with(template)
                    .mount(&controller)
                    .await;
                let uri = controller.uri();
                (Some(controller), uri)
            }
            None => (None, "127.0.0.1:9".to_string()),
        };

        let body = format!(r#"{{"host":"{host}"}}"#);
        let resp = app
            .clone()
            .oneshot(helpers::authed_post(
                "/api/providers/nanoleaf/pair",
                &cookie,
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), expect_status, "{host}");
        if let Some((field, value)) = expect_field {
            let json = helpers::response_json(resp).await;
            assert_eq!(json[field], value, "{json}");
        }
    }
}

// ── Import provider groups ───────────────────────────────────────────────────

// ── Update provider credentials ──────────────────────────────────────────────

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

// ── Provider network auto-detect (POST /api/providers/scan/{type}) ────────────

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
        .clone()
        .oneshot(helpers::authed_get("/api/settings", &cookie))
        .await
        .unwrap();
    assert_eq!(
        helpers::response_json(resp).await["expanded_lan_scan"],
        serde_json::json!(["192.168.1.0/24", "10.0.0.0/24"])
    );

    // A public subnet is rejected outright — the scan never leaves the LAN.
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
async fn kiosk_screenshot_roundtrips_and_gates() {
    // The remote-eyes debug flow: controller sends the `screenshot` command,
    // the kiosk uploads its capture (bfr_key cookie), the controller reads it
    // back (session). One latest-wins image per kiosk.
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
    let kiosk_id = list[0]["id"].as_str().unwrap().to_string();
    assert!(list[0]["screenshot_at"].is_null(), "no capture yet");

    // The `screenshot` command is a valid controller command.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            &format!("/api/kiosks/{kiosk_id}/command"),
            &cookie,
            r#"{"command":"screenshot"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Upload: kiosk key cookie + an image mime. A junk mime is refused.
    let upload = |mime: &'static str, body: &'static [u8], with_key: bool| {
        let mut b = Request::builder()
            .method("POST")
            .uri("/api/kiosks/self/screenshot")
            .header(header::CONTENT_TYPE, mime);
        if with_key {
            b = b.header(header::COOKIE, format!("bfr_key={key}"));
        }
        b.body(Body::from(body)).unwrap()
    };
    let resp = app
        .clone()
        .oneshot(upload("image/jpeg", b"fakejpegbytes", true))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .clone()
        .oneshot(upload("text/html", b"<html>", true))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let resp = app
        .clone()
        .oneshot(upload("image/jpeg", b"x", false))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Read back (session): bytes + mime + captured-at header; list shows the stamp.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            &format!("/api/kiosks/{kiosk_id}/screenshot"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/jpeg"
    );
    assert!(resp.headers().get("x-captured-at").is_some());
    assert_eq!(
        resp.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store",
        "a debug capture must never be cached"
    );
    let bytes = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    assert_eq!(bytes.as_ref(), b"fakejpegbytes");
    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert!(!list[0]["screenshot_at"].is_null(), "capture stamp visible");

    // Unauthed read → 401.
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/kiosks/{kiosk_id}/screenshot"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
