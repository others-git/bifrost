mod helpers;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

use helpers::{bearer_get, bearer_json, create_api_key, ha_remote_mock};

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
    // `areas` drives the panel's filter menu, so it must list the targets the
    // server really emitted — including ones the current filter excludes, or
    // picking an area would collapse the menu to that single choice.
    let areas = body["areas"].as_array().unwrap();
    assert!(areas.iter().any(|a| a == "bifrost::automation"));
    assert!(
        areas.iter().any(|a| a == "bifrost::voice"),
        "areas must span the whole buffer, not the filtered view"
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

/// dev_mode is off by default, so the whole dev surface 404s even for a valid
/// session — in production it doesn't exist at all.
#[tokio::test]
async fn dev_routes_404_when_dev_mode_off() {
    let app = helpers::test_app_with_password().await;
    let cookie = helpers::login(&app, helpers::TEST_PASSWORD).await;
    for uri in [
        "/api/dev/info",
        "/api/dev/media/whatever/routing",
        "/api/dev/devices/some-provider/climate.bedroom/raw",
    ] {
        let resp = app
            .clone()
            .oneshot(helpers::authed_get(uri, &cookie))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{uri}");
    }
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

// ── Light segments (per-segment colour control) ──────────────────────────────

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
