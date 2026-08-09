mod helpers;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

use helpers::{audio_mock, bearer_get, bearer_json, create_api_key, setup_onkyo, wled_mock};

// ── Scenes ───────────────────────────────────────────────────────────────────

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

/// Every create route runs its name through the shared `clean_name` guard, so a
/// whitespace-only name is rejected instead of being stored blank.
#[tokio::test]
async fn create_routes_reject_a_whitespace_only_name() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    for uri in ["/api/scenes", "/api/rooms", "/api/api-keys"] {
        let resp = app
            .clone()
            .oneshot(helpers::authed_post(uri, &cookie, r#"{"name":"  "}"#))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY, "{uri}");
    }

    // The public API enforces the same guard behind a Bearer key.
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
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "POST /api/v1/scenes"
    );
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

/// Creating a plan and resizing one enforce the same dimension bounds, so a
/// grid can never be zero-wide or wider than the planner can draw.
#[tokio::test]
async fn plan_routes_reject_bad_dimensions() {
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
            "POST /api/plans body: {body}"
        );
    }

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
            "PUT /api/plans/{{id}}/size body: {body}"
        );
    }
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

// ── Plan rooms ───────────────────────────────────────────────────────────────

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

/// A room cascade carries no colour, so it must leave each member's own colour
/// alone — the bug was a cascade broadcasting one uniform colour on every
/// change. Colour temperature is the deliberate exception: it's the mutually
/// exclusive third mode, so writing one clears the colour.
#[tokio::test]
async fn room_cascade_preserves_member_colour() {
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
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}"),
            &cookie,
            r##"{"on":true,"brightness":60,"color":{"x":0.64,"y":0.33,"brightness":0.6}}"##,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Reading the one light back out of the inventory listing.
    async fn read_light(app: &Router, cookie: &str, light_id: &str) -> serde_json::Value {
        let resp = app
            .clone()
            .oneshot(helpers::authed_get("/api/lights", cookie))
            .await
            .unwrap();
        let lights = helpers::response_json(resp).await;
        lights
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["id"] == light_id)
            .expect("light present")
            .clone()
    }

    // Off then on — both pure-power commands. The device keeps its colour across
    // a power cycle, so the stored state must too (not a colourless default).
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
    let light = read_light(&app, &cookie, &light_id).await;
    assert_eq!(light["last_state"]["on"], true, "{light}");
    assert!(
        (light["last_state"]["color"]["x"].as_f64().unwrap() - 0.64).abs() < 1e-6,
        "colour was wiped by the power cycle: {light}"
    );

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
    let light = read_light(&app, &cookie, &light_id).await;
    assert_eq!(
        light["last_state"]["brightness"].as_f64().unwrap(),
        30.0,
        "brightness not applied: {light}"
    );
    assert!(
        (light["last_state"]["color"]["x"].as_f64().unwrap() - 0.64).abs() < 1e-6,
        "colour was wiped by a brightness-only change: {light}"
    );

    // Switching the light to a white temperature clears the colour, so the UI
    // can tell from `last_state` which mode the light is in.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "PUT",
            &format!("/api/lights/{light_id}"),
            &cookie,
            r#"{"on":true,"color_temp_mirek":300}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let light = read_light(&app, &cookie, &light_id).await;
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

#[tokio::test]
async fn api_responses_are_no_store_except_content_addressed_media() {
    // Live device/feed state must never be served from any cache a kiosk
    // WebView (or proxy) keeps — every /api response defaults to `no-store`.
    // The deliberate exceptions set their own bounded policies: the feed
    // poster proxy (max-age=86400, never immutable) and board background
    // media (immutable, busted by its `?v=` stamp) — verified in their own
    // tests.
    let app = helpers::test_app_with_password().await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/instance")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );

    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    let resp = app
        .clone()
        .oneshot(helpers::authed_get("/api/lights", &cookie))
        .await
        .unwrap();
    assert_eq!(
        resp.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
}
