mod helpers;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

use helpers::{add_onkyo_device, audio_mock, create_api_key, wled_mock};

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
async fn mcp_rejects_missing_and_invalid_bearer() {
    let app = helpers::test_app_with_password().await;

    // No Authorization header at all.
    let resp = app
        .clone()
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
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "no bearer");

    // A well-formed but unknown key.
    let resp = app
        .oneshot(mcp_tool_call(
            "bfr_not_a_real_key",
            "get_home_state",
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "unknown key");
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
async fn voice_command_dispatches_named_relative_and_compound_phrases() {
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

    // "dim test light" → a relative brightness command down the same path.
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

    // A compound phrase runs each clause: one resolves (the light), one doesn't
    // (no such room), so the result is partial.
    let resp = app
        .clone()
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
async fn voice_speak_without_a_usable_tts_endpoint_returns_503() {
    // Talk-back degrades gracefully: with no usable `tts` role, /speak says so
    // with a 503 rather than failing opaquely (text control is unaffected).
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;

    // Nothing configured at all.
    let resp = app
        .clone()
        .oneshot(helpers::authed_json(
            "POST",
            "/api/voice/speak",
            &cookie,
            r#"{"text":"hello"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "no tts endpoint"
    );

    // Configured, then disabled — disabling a role must actually take effect, so
    // a disabled `tts` endpoint is treated as absent (the Settings Enabled
    // switch flips this flag).
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
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "disabled tts endpoint"
    );
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

// ── AI endpoints config + voice /listen (M23 P2) ─────────────────────────────

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
