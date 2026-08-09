mod helpers;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

use helpers::{
    add_onkyo_device, audio_mock, bearer_get, bearer_json, create_api_key, ha_remote_mock,
    heard_volume_set, setup_onkyo, setup_sonos, sonos_mock, sonos_pair_ids, volume_set_count,
    wait_for_command, wait_for_volume_set, wled_mock,
};

// ── Audio devices (Onkyo provider through the full API stack) ─────────────────

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
async fn audio_group_rejects_invalid_pairings() {
    let server = sonos_mock::start_pair().await;
    let (port, _recorded) = audio_mock::spawn(audio_mock::receiver_state()).await;
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    setup_sonos(&app, &cookie, &server.uri()).await;
    let (living, kitchen) = sonos_pair_ids(&app, &cookie).await;
    let onkyo = setup_onkyo(&app, &cookie, port).await;

    // A speaker can't coordinate itself.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "POST",
            &format!("/api/media/devices/{kitchen}/group"),
            &cookie,
            &format!(r#"{{"coordinator_id":"{kitchen}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "grouping a speaker with itself"
    );

    // An unknown member is a 404, not a validation failure.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "POST",
            "/api/media/devices/nope/group",
            &cookie,
            &format!(r#"{{"coordinator_id":"{living}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "unknown member");

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
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "grouping across providers"
    );
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
