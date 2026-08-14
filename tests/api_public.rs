mod helpers;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

use bifrost::api::kiosks::SchedulerState;
use helpers::{anon_json, bearer_get, bearer_json, create_api_key, wled_mock};

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

    let mut sched = SchedulerState::default();

    // An awake hour queues a wake…
    bifrost::api::kiosks::scheduler_tick(&state, &mut sched, 3, 3 * 60).await;
    assert_eq!(pending(app.clone(), cookie.clone()).await, "wake");

    // …and an asleep hour flips it to sleep (edge-triggered on the change).
    bifrost::api::kiosks::scheduler_tick(&state, &mut sched, 15, 15 * 60).await;
    assert_eq!(pending(app.clone(), cookie.clone()).await, "sleep");
}

/// The reconcile backstop: a display command is at-most-once, so anything that
/// re-lights the panel after it lands (a tap, an OTA relaunch, a dropped
/// command) used to leave the hub believing the kiosk was already asleep — for
/// the rest of the Off block, i.e. all night. During a forced hour the pass now
/// compares its verdict against the screen the kiosk itself reports and
/// re-asserts.
#[tokio::test]
async fn a_forced_hour_reasserts_sleep_when_the_kiosk_reports_its_screen_on() {
    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "wall tablet").await;

    // The kiosk heartbeat, reporting a lit screen. Also consumes whatever is
    // queued — exactly like the real app's 10s check-in.
    let checkin = |app: Router, key: String| async move {
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/kiosks/checkin")
                    .header(header::AUTHORIZATION, format!("Bearer {key}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"screen_on":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    };
    checkin(app.clone(), key.clone()).await;

    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let id = list[0]["id"].as_str().unwrap().to_string();

    // Asleep around the clock: inside one long Off block there is no later edge
    // to fall back on.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/kiosks/{id}/plan"),
            &cookie,
            r#"{"enabled":true,"hour_modes":"SSSSSSSSSSSSSSSSSSSSSSSS"}"#,
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

    // Zero grace: the reconcile window is a comfort feature, not the behaviour
    // under test.
    let mut sched = SchedulerState::with_grace(std::time::Duration::ZERO);

    // The hour's edge queues the sleep…
    bifrost::api::kiosks::scheduler_tick(&state, &mut sched, 2, 2 * 60).await;
    assert_eq!(pending(app.clone(), cookie.clone()).await, "sleep");

    // …the kiosk picks it up but its screen is still on (it woke again).
    checkin(app.clone(), key.clone()).await;
    assert_eq!(
        pending(app.clone(), cookie.clone()).await,
        serde_json::Value::Null
    );

    // Same hour, no edge — the backstop re-asserts anyway.
    bifrost::api::kiosks::scheduler_tick(&state, &mut sched, 2, 2 * 60).await;
    assert_eq!(pending(app.clone(), cookie.clone()).await, "sleep");
}

/// The backstop stops at forced hours. In an **Aware** hour presence is the
/// authority and a manual override is meant to hold until the room's next flip,
/// so a screen that disagrees is left alone.
#[tokio::test]
async fn an_aware_hour_leaves_a_disagreeing_screen_alone() {
    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let key = create_api_key(&app, &cookie, "wall tablet").await;

    // A room whose occupancy sensor reads "detected" → presence governs.
    sqlx::query(
        "INSERT INTO providers (id, provider_type, name, credentials) VALUES ('p','wled','P','x')",
    )
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO rooms (id, name) VALUES ('r1','Living Room')")
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO sensor_devices (id, provider_id, device_id, name, kind, last_state)
         VALUES ('s1','p','d1','Motion','motion','{\"reading\":{\"bool\":true},\"reachable\":true}')",
    )
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO room_sensor_devices (room_id, sensor_device_id) VALUES ('r1','s1')")
        .execute(&state.db)
        .await
        .unwrap();

    // Heartbeat reporting a DARK screen (someone blanked it by hand).
    let checkin = |app: Router, key: String| async move {
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/kiosks/checkin")
                    .header(header::AUTHORIZATION, format!("Bearer {key}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"screen_on":false}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    };
    checkin(app.clone(), key.clone()).await;

    let list = helpers::response_json(
        app.clone()
            .oneshot(helpers::authed_get("/api/kiosks", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let id = list[0]["id"].as_str().unwrap().to_string();

    for (path, body) in [
        (
            format!("/api/kiosks/{id}/plan"),
            r#"{"enabled":true,"hour_modes":"AAAAAAAAAAAAAAAAAAAAAAAA"}"#.to_string(),
        ),
        (
            format!("/api/kiosks/{id}/room"),
            r#"{"room_id":"r1"}"#.to_string(),
        ),
    ] {
        let resp = app
            .clone()
            .oneshot(helpers::authed_json("PUT", &path, &cookie, &body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT, "{path}");
    }

    let pending = |app: Router, cookie: String| async move {
        let list = helpers::response_json(
            app.oneshot(helpers::authed_get("/api/kiosks", &cookie))
                .await
                .unwrap(),
        )
        .await;
        list[0]["pending_command"].clone()
    };

    let mut sched = SchedulerState::with_grace(std::time::Duration::ZERO);

    // Occupied room → wake, delivered on the next heartbeat.
    bifrost::api::kiosks::scheduler_tick(&state, &mut sched, 10, 10 * 60).await;
    assert_eq!(pending(app.clone(), cookie.clone()).await, "wake");
    checkin(app.clone(), key.clone()).await;

    // The screen is dark against an "awake" verdict — and stays that way.
    bifrost::api::kiosks::scheduler_tick(&state, &mut sched, 10, 10 * 60).await;
    assert_eq!(
        pending(app.clone(), cookie.clone()).await,
        serde_json::Value::Null
    );
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

    let mut sched = SchedulerState::default();

    // Device off, no room, no presence input → nothing governs this hour.
    bifrost::api::kiosks::scheduler_tick(&state, &mut sched, 10, 10 * 60).await;
    assert_eq!(
        pending(app.clone(), cookie.clone()).await,
        serde_json::Value::Null
    );

    // Flip the device on — the override forces the screen awake.
    sqlx::query("UPDATE power_devices SET last_state = '{\"on\":true}' WHERE id = 'pw1'")
        .execute(&state.db)
        .await
        .unwrap();
    bifrost::api::kiosks::scheduler_tick(&state, &mut sched, 11, 11 * 60).await;
    assert_eq!(pending(app.clone(), cookie.clone()).await, "wake");
}

/// The keep_off flavour: while the target device is on, an Aware hour forces
/// the kiosk ASLEEP — "movie night, kill the tablet glow" — even in a room
/// with no presence input at all (which would otherwise leave it ungoverned).
#[tokio::test]
async fn aware_override_keep_off_forces_asleep_while_the_target_device_is_on() {
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

    let mut sched = SchedulerState::default();

    // Device off, no room, no presence input → nothing governs this hour.
    bifrost::api::kiosks::scheduler_tick(&state, &mut sched, 10, 10 * 60).await;
    assert_eq!(
        pending(app.clone(), cookie.clone()).await,
        serde_json::Value::Null
    );

    // Flip the device on — keep_off forces the screen asleep.
    sqlx::query("UPDATE power_devices SET last_state = '{\"on\":true}' WHERE id = 'pw1'")
        .execute(&state.db)
        .await
        .unwrap();
    bifrost::api::kiosks::scheduler_tick(&state, &mut sched, 11, 11 * 60).await;
    assert_eq!(pending(app.clone(), cookie.clone()).await, "sleep");
}

// ── Public API (/api/v1) + API keys ──────────────────────────────────────────

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

// ── Device enrollment (QR pairing) ───────────────────────────────────────────

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
async fn v1_rejects_a_well_formed_but_unknown_key() {
    let app = helpers::test_app_with_password().await;
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

// ── Casting (/api/media/devices/{id}/cast) ───────────────────────────────────

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

// ── Content feeds (recently-added widget) ────────────────────────────────────

/// Stand up the app with a Plex feed source added through the real
/// add-provider flow (so the credential encrypt/build path is exercised),
/// pointed at a wiremock Plex server. Returns (app, cookie, provider_id).
async fn app_with_plex(server_uri: &str) -> (Router, String, String) {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers",
            &cookie,
            &format!(
                r#"{{"name":"Plex","provider_type":"plex","credentials":{{"host":"{server_uri}","token":"tok"}}}}"#
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "add plex provider");
    let body = helpers::response_json(resp).await;
    let id = body["id"].as_str().unwrap().to_string();
    (app, cookie, id)
}

#[tokio::test]
async fn feed_provider_type_is_offered_and_addable() {
    use wiremock::MockServer;
    let server = MockServer::start().await;
    let (app, cookie, id) = app_with_plex(&server.uri()).await;

    // The add menu lists plex under its own "feed" kind…
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/providers/types", &cookie))
        .await
        .unwrap();
    let types = helpers::response_json(resp).await;
    let plex = types
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["provider_type"] == "plex")
        .expect("plex in /api/providers/types");
    assert_eq!(plex["kind"], "feed");

    // …the configured row reads as domain "feed"…
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/providers", &cookie))
        .await
        .unwrap();
    let rows = helpers::response_json(resp).await;
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id.as_str())
        .expect("plex row listed");
    assert_eq!(row["domain"], "feed");

    // …and with no manager it still reports operational, not broken.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            &format!("/api/providers/{id}/status"),
            &cookie,
        ))
        .await
        .unwrap();
    let status = helpers::response_json(resp).await;
    assert_eq!(status["state"], "ready");

    // The feed source appears in the widget config's source list.
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/feeds/sources", &cookie))
        .await
        .unwrap();
    let sources = helpers::response_json(resp).await;
    let src = sources.as_array().unwrap();
    assert_eq!(src.len(), 1);
    assert_eq!(src[0]["id"], id.as_str());
    assert_eq!(src[0]["type_name"], "Plex");
}

#[tokio::test]
async fn feed_libraries_and_recent_serve_rolled_up_tiles() {
    use wiremock::matchers::{header as h, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({ "MediaContainer": { "machineIdentifier": "m-int" } }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/library/sections"))
        .and(h("X-Plex-Token", "tok"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "MediaContainer": { "Directory": [
                { "key": "2", "title": "TV Shows", "type": "show" },
            ]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/library/sections/2/recentlyAdded"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "MediaContainer": { "Metadata": [
                { "ratingKey": "10", "type": "episode", "title": "Ep A",
                  "grandparentTitle": "Show X", "grandparentRatingKey": "1",
                  "grandparentThumb": "/library/metadata/1/thumb/1",
                  "parentIndex": 1, "index": 3, "addedAt": 300 },
                { "ratingKey": "11", "type": "episode", "title": "Ep B",
                  "grandparentTitle": "Show X", "grandparentRatingKey": "1",
                  "grandparentThumb": "/library/metadata/1/thumb/1",
                  "parentIndex": 1, "index": 2, "addedAt": 200 },
                { "ratingKey": "12", "type": "movie", "title": "Solo Film",
                  "thumb": "/library/metadata/12/thumb/1", "year": 2024, "addedAt": 250 },
            ]}
        })))
        .mount(&server)
        .await;

    let (app, cookie, id) = app_with_plex(&server.uri()).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            &format!("/api/feeds/{id}/libraries"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let libs = helpers::response_json(resp).await;
    assert_eq!(libs[0]["name"], "TV Shows");

    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            &format!("/api/feeds/{id}/recent?library=2&limit=6"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let entries = helpers::response_json(resp).await;
    let entries = entries.as_array().unwrap();
    // Two episodes of Show X collapse into one tile (newest first), the
    // movie stays its own tile between them by timestamp.
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["title"], "Show X");
    assert_eq!(entries[0]["count"], 2);
    assert_eq!(entries[0]["subtitle"], "2 new episodes");
    assert_eq!(entries[1]["title"], "Solo Film");
    assert_eq!(entries[1]["count"], 1);
}

#[tokio::test]
async fn feed_image_proxy_serves_bytes_and_contains_paths() {
    use wiremock::matchers::{header as h, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/library/metadata/1/thumb/1"))
        .and(h("X-Plex-Token", "tok"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(b"posterbytes".to_vec())
                .insert_header("content-type", "image/jpeg"),
        )
        .mount(&server)
        .await;

    let (app, cookie, id) = app_with_plex(&server.uri()).await;

    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            &format!("/api/feeds/{id}/image?path=%2Flibrary%2Fmetadata%2F1%2Fthumb%2F1"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/jpeg"
    );
    // Bounded, never `immutable`: a kiosk WebView has no cache eviction of
    // its own, so a transiently-degenerate poster body cached immutable
    // outlived every deploy. A day self-heals.
    let cc = resp
        .headers()
        .get(header::CACHE_CONTROL)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        cc.contains("max-age=86400") && !cc.contains("immutable"),
        "got: {cc}"
    );
    let bytes = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    assert_eq!(bytes.as_ref(), b"posterbytes");

    // An absolute or protocol-relative path must be rejected before any fetch
    // — the proxy joins onto the provider's own base URL only (no SSRF).
    for bad in ["http%3A%2F%2Fevil.example%2Fx", "%2F%2Fevil.example%2Fx"] {
        let resp = app
            .clone()
            .oneshot(helpers::authed_get(
                &format!("/api/feeds/{id}/image?path={bad}"),
                &cookie,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{bad}");
    }
}

#[tokio::test]
async fn feed_recent_on_unknown_provider_returns_404() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .clone()
        .oneshot(helpers::authed_get(
            "/api/feeds/nope/recent?library=1",
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn plex_pair_flow_mints_a_code_then_polls_to_a_token() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let plex_tv = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/pins"))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(serde_json::json!({ "id": 42, "code": "WXYZ", "authToken": null })),
        )
        .mount(&plex_tv)
        .await;
    // First poll: still pending; second: linked.
    Mock::given(method("GET"))
        .and(path("/api/v2/pins/42"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "id": 42, "code": "WXYZ", "authToken": null })),
        )
        .up_to_n_times(1)
        .mount(&plex_tv)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v2/pins/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({ "id": 42, "code": "WXYZ", "authToken": "linked-token" }),
        ))
        .mount(&plex_tv)
        .await;

    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Unauthenticated → 401 (the code mints nothing without a session).
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/providers/plex/pair")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Begin: mints the code the user types at plex.tv/link.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers/plex/pair",
            &cookie,
            &format!(r#"{{"base":"{}"}}"#, plex_tv.uri()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::response_json(resp).await;
    assert_eq!(body["status"], "code_displayed");
    assert_eq!(body["code"], "WXYZ");
    let pin_id = body["pin_id"].as_i64().unwrap();
    let client_id = body["client_id"].as_str().unwrap().to_string();

    // Poll: pending until the user enters the code…
    let poll_body = format!(
        r#"{{"base":"{}","pin_id":{pin_id},"client_id":"{client_id}"}}"#,
        plex_tv.uri()
    );
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers/plex/pair",
            &cookie,
            &poll_body,
        ))
        .await
        .unwrap();
    assert_eq!(helpers::response_json(resp).await["status"], "pending");

    // …then the account token arrives, ready to store as the `token` credential.
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/providers/plex/pair",
            &cookie,
            &poll_body,
        ))
        .await
        .unwrap();
    let body = helpers::response_json(resp).await;
    assert_eq!(body["status"], "paired");
    assert_eq!(body["token"], "linked-token");
}

#[tokio::test]
async fn feeds_accept_a_kiosk_key_cookie_without_a_session() {
    // A wall kiosk renders the feed widget with only its `bfr_key` cookie once
    // its minted session lapses — the poster proxy and feed reads must keep
    // working off the key alone (the kiosk speaks only to Bifrost, so a 401
    // here blanks every poster on the wall).
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/api-keys",
            &cookie,
            r#"{"name":"kiosk"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let key = helpers::response_json(resp).await["key"]
        .as_str()
        .unwrap()
        .to_string();

    let with_key_cookie = |uri: &str, k: &str| {
        Request::builder()
            .uri(uri)
            .header(header::COOKIE, format!("bfr_key={k}"))
            .body(Body::empty())
            .unwrap()
    };

    // Key cookie alone (no session) → authorized.
    let resp = app
        .clone()
        .oneshot(with_key_cookie("/api/feeds/sources", &key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // A bogus key is still rejected.
    let resp = app
        .clone()
        .oneshot(with_key_cookie("/api/feeds/sources", "bfr_not_a_real_key"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dashboard_changes_announce_on_the_inventory_stream() {
    // Board edits ride the shared SSE stream (payload "dashboards") so an open
    // Boards view — a wall kiosk above all — re-reads the layout live instead
    // of holding the stale board until someone navigates away and back.
    let (app, state) = helpers::test_app_with_password_and_state().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let mut rx = state.inventory_events.subscribe();

    let resp = app
        .clone()
        .oneshot(helpers::authed_post(
            "/api/dashboards",
            &cookie,
            r#"{"name":"Wall"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let board_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(rx.recv().await.unwrap(), "dashboards", "create announces");

    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/dashboards/{board_id}"),
            &cookie,
            r#"{"widgets":[{"id":"w1","type":"clock","x":0,"y":0,"w":8,"h":6,"config":{}}]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        rx.recv().await.unwrap(),
        "dashboards",
        "layout save announces"
    );

    let resp = app
        .clone()
        .oneshot(helpers::authed_delete(
            &format!("/api/dashboards/{board_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(rx.recv().await.unwrap(), "dashboards", "delete announces");
}
