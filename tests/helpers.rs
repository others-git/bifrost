//! Shared test fixtures for integration tests.
//!
//! Each test crate uses only a subset of these, so the module allows dead code.
#![allow(dead_code)]

use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header};
use bifrost::{AppState, build_app, providers};
use sqlx::SqlitePool;
use std::sync::Arc;
use tower::ServiceExt;

pub const TEST_SECRET: &str = "test-secret-key-32-bytes-exactly";
pub const TEST_PASSWORD: &str = "correct-horse-battery-staple";

/// Create an in-memory SQLite pool with all migrations applied.
/// Foreign keys are enabled to match production (`db::connect`).
pub async fn test_db() -> SqlitePool {
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;
    let opts = SqliteConnectOptions::from_str(":memory:")
        .unwrap()
        .foreign_keys(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    pool
}

/// The production registry plus WLED — the generic, easily-mockable light
/// provider the integration tests are built on. WLED is unregistered in
/// production (`default_registry`), so tests opt it back in here.
fn test_registry() -> providers::ProviderRegistry {
    let mut r = providers::default_registry();
    r.register(providers::wled::WledProviderFactory);
    r
}

/// Hash a password with reduced Argon2 params suitable for testing speed.
pub fn hash_password(password: &str) -> String {
    use argon2::{Algorithm, Params, Version};
    let params = Params::new(1024, 1, 1, None).unwrap();
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let salt = SaltString::generate(&mut OsRng);
    argon2
        .hash_password(password.as_bytes(), &salt)
        .unwrap()
        .to_string()
}

/// Build a complete test app with an in-memory DB and the default provider registry.
pub async fn test_app() -> Router {
    let db = test_db().await;
    let registry = test_registry();
    let state = Arc::new(AppState::new(db, TEST_SECRET, registry));
    build_app(state)
}

/// Build a test app whose DB already has a configured password.
pub async fn test_app_with_password() -> Router {
    let db = test_db().await;
    let hash = hash_password(TEST_PASSWORD);
    sqlx::query("INSERT INTO config (id, password_hash, setup_complete) VALUES (1, ?, 1)")
        .bind(&hash)
        .execute(&db)
        .await
        .unwrap();
    let registry = test_registry();
    let state = Arc::new(AppState::new(db, TEST_SECRET, registry));
    build_app(state)
}

/// Like [`test_app_with_password`] but also hands back the state, so a test can
/// observe internals (e.g. the kiosk command broadcast channel).
pub async fn test_app_with_password_and_state() -> (Router, Arc<AppState>) {
    let db = test_db().await;
    let hash = hash_password(TEST_PASSWORD);
    sqlx::query("INSERT INTO config (id, password_hash, setup_complete) VALUES (1, ?, 1)")
        .bind(&hash)
        .execute(&db)
        .await
        .unwrap();
    let state = Arc::new(AppState::new(db, TEST_SECRET, test_registry()));
    (build_app(state.clone()), state)
}

/// POST /api/auth/login and return the Set-Cookie header value on success.
pub async fn login(app: &Router, password: &str) -> String {
    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(r#"{{"password":"{password}"}}"#)))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "login failed");
    resp.headers()
        .get(header::SET_COOKIE)
        .expect("no Set-Cookie header")
        .to_str()
        .unwrap()
        .to_string()
}

/// Build an authenticated GET request with the given session cookie.
pub fn authed_get(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::COOKIE, cookie.split(';').next().unwrap()) // send only name=value
        .body(Body::empty())
        .unwrap()
}

/// Build an authenticated POST request with a JSON body and the given session cookie.
#[allow(dead_code)] // used by some, not all, test binaries
pub fn authed_post(uri: &str, cookie: &str, json_body: &str) -> Request<Body> {
    authed_json("POST", uri, cookie, json_body)
}

/// Build an authenticated request with the given method and JSON body.
#[allow(dead_code)]
pub fn authed_json(method: &str, uri: &str, cookie: &str, json_body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookie.split(';').next().unwrap())
        .body(Body::from(json_body.to_string()))
        .unwrap()
}

/// Build an authenticated DELETE request.
#[allow(dead_code)]
pub fn authed_delete(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header(header::COOKIE, cookie.split(';').next().unwrap())
        .body(Body::empty())
        .unwrap()
}

/// Test app with a configured password plus one WLED provider + light whose
/// device URL points at `base_url` (a wiremock server). Returns the app and
/// the seeded light's ID. The light starts on at 80% brightness.
#[allow(dead_code)]
pub async fn test_app_with_light(base_url: &str) -> (Router, String) {
    let db = test_db().await;
    let hash = hash_password(TEST_PASSWORD);
    sqlx::query("INSERT INTO config (id, password_hash, setup_complete) VALUES (1, ?, 1)")
        .bind(&hash)
        .execute(&db)
        .await
        .unwrap();

    let registry = test_registry();
    let state = Arc::new(AppState::new(db.clone(), TEST_SECRET, registry));

    let creds = format!(r#"{{"device_ip":"{base_url}"}}"#);
    let enc = state.encrypt_credentials(&creds).unwrap();
    sqlx::query("INSERT INTO providers (id, provider_type, name, credentials) VALUES (?, 'wled', 'Test WLED', ?)")
        .bind("prov-test-1")
        .bind(&enc)
        .execute(&db)
        .await
        .unwrap();

    let light_id = "light-test-1".to_string();
    sqlx::query(
        "INSERT INTO lights (id, provider_id, device_id, name, capabilities, last_state, last_seen)
         VALUES (?, 'prov-test-1', 'main', 'Test Light', '{}', ?, datetime('now'))",
    )
    .bind(&light_id)
    .bind(r#"{"on":true,"brightness":80.0,"color":null,"color_temp_mirek":null}"#)
    .execute(&db)
    .await
    .unwrap();

    (build_app(state), light_id)
}

/// Like `test_app_with_light` but seeds two lights ("Light A" at device
/// "main", "Light B" at device "second") so distribution behaviour is
/// observable. Returns (app, light_a_id, light_b_id).
#[allow(dead_code)]
pub async fn test_app_with_two_lights(base_url: &str) -> (Router, String, String) {
    let db = test_db().await;
    let hash = hash_password(TEST_PASSWORD);
    sqlx::query("INSERT INTO config (id, password_hash, setup_complete) VALUES (1, ?, 1)")
        .bind(&hash)
        .execute(&db)
        .await
        .unwrap();

    let registry = test_registry();
    let state = Arc::new(AppState::new(db.clone(), TEST_SECRET, registry));

    let creds = format!(r#"{{"device_ip":"{base_url}"}}"#);
    let enc = state.encrypt_credentials(&creds).unwrap();
    sqlx::query("INSERT INTO providers (id, provider_type, name, credentials) VALUES (?, 'wled', 'Test WLED', ?)")
        .bind("prov-test-1")
        .bind(&enc)
        .execute(&db)
        .await
        .unwrap();

    for (id, device, name) in [
        ("light-a", "main", "Light A"),
        ("light-b", "second", "Light B"),
    ] {
        sqlx::query(
            "INSERT INTO lights (id, provider_id, device_id, name, capabilities, last_state, last_seen)
             VALUES (?, 'prov-test-1', ?, ?, '{}', ?, datetime('now'))",
        )
        .bind(id)
        .bind(device)
        .bind(name)
        .bind(r#"{"on":true,"brightness":80.0,"color":null,"color_temp_mirek":null}"#)
        .execute(&db)
        .await
        .unwrap();
    }

    (build_app(state), "light-a".into(), "light-b".into())
}

/// Like `test_app_with_two_lights` but also hands back the DB pool, so a test can
/// seed rows (e.g. a palette) that have no create endpoint.
/// Returns (app, light_a_id, light_b_id, db).
#[allow(dead_code)]
pub async fn test_app_with_two_lights_db(base_url: &str) -> (Router, String, String, SqlitePool) {
    let db = test_db().await;
    let hash = hash_password(TEST_PASSWORD);
    sqlx::query("INSERT INTO config (id, password_hash, setup_complete) VALUES (1, ?, 1)")
        .bind(&hash)
        .execute(&db)
        .await
        .unwrap();

    let registry = test_registry();
    let state = Arc::new(AppState::new(db.clone(), TEST_SECRET, registry));

    let creds = format!(r#"{{"device_ip":"{base_url}"}}"#);
    let enc = state.encrypt_credentials(&creds).unwrap();
    sqlx::query("INSERT INTO providers (id, provider_type, name, credentials) VALUES (?, 'wled', 'Test WLED', ?)")
        .bind("prov-test-1")
        .bind(&enc)
        .execute(&db)
        .await
        .unwrap();

    for (id, device, name) in [
        ("light-a", "main", "Light A"),
        ("light-b", "second", "Light B"),
    ] {
        sqlx::query(
            "INSERT INTO lights (id, provider_id, device_id, name, capabilities, last_state, last_seen)
             VALUES (?, 'prov-test-1', ?, ?, '{}', ?, datetime('now'))",
        )
        .bind(id)
        .bind(device)
        .bind(name)
        .bind(r#"{"on":true,"brightness":80.0,"color":null,"color_temp_mirek":null}"#)
        .execute(&db)
        .await
        .unwrap();
    }

    (build_app(state), "light-a".into(), "light-b".into(), db)
}

/// Test app with a configured password plus one Hue provider whose bridge
/// URL points at `base_url` (a wiremock server), and one discovered light
/// with device_id "light-1". Returns the app and the seeded light's ID.
#[allow(dead_code)]
pub async fn test_app_with_hue_light(base_url: &str) -> (Router, String) {
    let db = test_db().await;
    let hash = hash_password(TEST_PASSWORD);
    sqlx::query("INSERT INTO config (id, password_hash, setup_complete) VALUES (1, ?, 1)")
        .bind(&hash)
        .execute(&db)
        .await
        .unwrap();

    let registry = test_registry();
    let state = Arc::new(AppState::new(db.clone(), TEST_SECRET, registry));

    let creds = format!(r#"{{"bridge_ip":"{base_url}","app_key":"test-key"}}"#);
    let enc = state.encrypt_credentials(&creds).unwrap();
    sqlx::query("INSERT INTO providers (id, provider_type, name, credentials) VALUES (?, 'hue', 'Test Hue', ?)")
        .bind("prov-hue-1")
        .bind(&enc)
        .execute(&db)
        .await
        .unwrap();

    let light_id = "light-hue-1".to_string();
    sqlx::query(
        "INSERT INTO lights (id, provider_id, device_id, name, capabilities, last_state, last_seen)
         VALUES (?, 'prov-hue-1', 'light-1', 'Hue Bulb', '{}', ?, datetime('now'))",
    )
    .bind(&light_id)
    .bind(r#"{"on":true,"brightness":100.0,"color":null,"color_temp_mirek":null}"#)
    .execute(&db)
    .await
    .unwrap();

    (build_app(state), light_id)
}

/// Test app with a configured password plus one Home Assistant provider whose
/// `base_url` points at `base_url` (a wiremock server). Nothing is discovered
/// yet — call `POST /api/providers/{id}/discover` to populate. Returns the app
/// and the HA provider's id.
#[allow(dead_code)]
pub async fn test_app_with_ha(base_url: &str) -> (Router, String) {
    let db = test_db().await;
    let hash = hash_password(TEST_PASSWORD);
    sqlx::query("INSERT INTO config (id, password_hash, setup_complete) VALUES (1, ?, 1)")
        .bind(&hash)
        .execute(&db)
        .await
        .unwrap();

    let registry = test_registry();
    let state = Arc::new(AppState::new(db.clone(), TEST_SECRET, registry));

    let creds = format!(r#"{{"base_url":"{base_url}","token":"test-token"}}"#);
    let enc = state.encrypt_credentials(&creds).unwrap();
    let provider_id = "prov-ha-1".to_string();
    sqlx::query(
        "INSERT INTO providers (id, provider_type, name, credentials) VALUES (?, 'ha', 'HA', ?)",
    )
    .bind(&provider_id)
    .bind(&enc)
    .execute(&db)
    .await
    .unwrap();

    (build_app(state), provider_id)
}

/// Like `test_app_with_ha`, but also returns the DB pool so a test can seed rows
/// directly (e.g. a stale device to exercise pruning).
#[allow(dead_code)]
pub async fn test_app_with_ha_db(base_url: &str) -> (Router, String, SqlitePool) {
    let db = test_db().await;
    let hash = hash_password(TEST_PASSWORD);
    sqlx::query("INSERT INTO config (id, password_hash, setup_complete) VALUES (1, ?, 1)")
        .bind(&hash)
        .execute(&db)
        .await
        .unwrap();

    let registry = test_registry();
    let state = Arc::new(AppState::new(db.clone(), TEST_SECRET, registry));
    let creds = format!(r#"{{"base_url":"{base_url}","token":"test-token"}}"#);
    let enc = state.encrypt_credentials(&creds).unwrap();
    let provider_id = "prov-ha-1".to_string();
    sqlx::query(
        "INSERT INTO providers (id, provider_type, name, credentials) VALUES (?, 'ha', 'HA', ?)",
    )
    .bind(&provider_id)
    .bind(&enc)
    .execute(&db)
    .await
    .unwrap();

    (build_app(state), provider_id, db)
}

pub async fn response_json(resp: Response<Body>) -> serde_json::Value {
    let bytes = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

pub async fn wled_mock() -> wiremock::MockServer {
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

/// Mint an API key via the session-authenticated management endpoint and
/// return the one-time plaintext key.
pub async fn create_api_key(app: &Router, cookie: &str, name: &str) -> String {
    let resp = app
        .clone()
        .oneshot(authed_post(
            "/api/api-keys",
            cookie,
            &format!(r#"{{"name":"{name}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = response_json(resp).await;
    body["key"].as_str().unwrap().to_string()
}

pub fn bearer_get(uri: &str, key: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .body(Body::empty())
        .unwrap()
}

pub fn bearer_json(method: &str, uri: &str, key: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

pub fn anon_json(method: &str, uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

pub mod audio_mock {
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
pub async fn setup_onkyo(app: &Router, cookie: &str, port: u16) -> String {
    let resp = app
        .clone()
        .oneshot(authed_post(
            "/api/providers",
            cookie,
            &format!(
                r#"{{"name":"AV","provider_type":"onkyo","credentials":{{"host":"127.0.0.1","port":{port}}}}}"#
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let provider_id = response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(authed_post(
            &format!("/api/providers/{provider_id}/discover"),
            cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(response_json(resp).await["discovered"], 1);

    let resp = app
        .clone()
        .oneshot(authed_get("/api/media/devices", cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let devices = response_json(resp).await;
    devices[0]["id"].as_str().unwrap().to_string()
}

/// Add an Onkyo provider pointed at `port`, discover it, and return the id of
/// the audio device that wasn't in `existing` (so two providers can be told
/// apart). Used by the M22 receiver-binding tests, which need two devices.
pub async fn add_onkyo_device(
    app: &Router,
    cookie: &str,
    port: u16,
    name: &str,
    existing: &[String],
) -> String {
    let resp = app
        .clone()
        .oneshot(authed_post(
            "/api/providers",
            cookie,
            &format!(
                r#"{{"name":"{name}","provider_type":"onkyo","credentials":{{"host":"127.0.0.1","port":{port}}}}}"#
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let provider_id = response_json(resp).await["id"]
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
            .oneshot(authed_post(
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
        .oneshot(authed_get("/api/media/devices", cookie))
        .await
        .unwrap();
    let devices = response_json(resp).await;
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
pub fn heard_volume_set(recorded: &[String]) -> bool {
    recorded
        .iter()
        .any(|m| m.starts_with("MVL") && !m.contains("QSTN"))
}

/// Count master-volume *sets* in the recorded eISCP stream.
pub fn volume_set_count(recorded: &[String]) -> usize {
    recorded
        .iter()
        .filter(|m| m.starts_with("MVL") && !m.contains("QSTN"))
        .count()
}

/// Poll a mock's recorded stream until a volume set lands (commands flow through
/// the shared link actor asynchronously, so they arrive a beat after the HTTP
/// response). Returns false if none arrives within ~2s.
pub async fn wait_for_volume_set(
    recorded: &std::sync::Arc<tokio::sync::Mutex<Vec<String>>>,
) -> bool {
    for _ in 0..40 {
        if heard_volume_set(&recorded.lock().await) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    false
}

/// Wait for a specific eISCP command substring to arrive at a mock device.
pub async fn wait_for_command(
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

/// A wiremock Sonos household: one standalone player plus a Favorites list.
/// Enough for discovery to create a device row and for the favorites endpoints
/// to browse/play against.
pub mod sonos_mock {
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
pub async fn setup_sonos(app: &Router, cookie: &str, base_uri: &str) -> String {
    let resp = app
        .clone()
        .oneshot(authed_post(
            "/api/providers",
            cookie,
            &format!(
                r#"{{"name":"Sonos","provider_type":"sonos","credentials":{{"host":"{base_uri}"}}}}"#
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let provider_id = response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(authed_post(
            &format!("/api/providers/{provider_id}/discover"),
            cookie,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(authed_get("/api/media/devices", cookie))
        .await
        .unwrap();
    let devices = response_json(resp).await;
    devices[0]["id"].as_str().unwrap().to_string()
}

/// Return (Living Room id, Kitchen id) from a discovered two-player household.
pub async fn sonos_pair_ids(app: &Router, cookie: &str) -> (String, String) {
    let resp = app
        .clone()
        .oneshot(authed_get("/api/media/devices", cookie))
        .await
        .unwrap();
    let devices = response_json(resp).await;
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

pub async fn ha_remote_mock() -> wiremock::MockServer {
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
