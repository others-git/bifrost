pub mod api;
pub mod audio;
pub mod config;
pub mod connection;
pub mod crypto;
pub mod db;
pub mod journal;
pub mod models;
pub mod providers;
pub mod wol;

use aes_gcm::Aes256Gcm;
use anyhow::Result;
use axum::Router;
use connection::ConnectionRegistry;
use providers::ProviderRegistry;
use rust_embed::Embed;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::{cors::CorsLayer, trace::TraceLayer};

pub struct AppState {
    pub db: SqlitePool,
    pub registry: ProviderRegistry,
    pub connections: Mutex<ConnectionRegistry>,
    pub started_at: std::time::Instant,
    /// A fresh random id minted once per process start. Clients poll
    /// `GET /api/instance`; a changed value means the server was redeployed, so
    /// the kiosk reloads to pick up the new build. Catches **any** restart —
    /// including backend-only redeploys that leave the frontend bundle unchanged.
    pub instance_id: String,
    /// Instant push of controller commands to a kiosk's live SSE stream
    /// (`GET /api/kiosks/stream`). `kiosks.pending_command` is the fallback for a
    /// kiosk that's offline / mid-reconnect (delivered on its next check-in).
    pub kiosk_commands: tokio::sync::broadcast::Sender<api::kiosks::KioskCommand>,
    /// Poked whenever an automation is created/updated/deleted, so the engine
    /// re-baselines new trigger subjects immediately (a rule made mid-flight
    /// must not wait for a restart) and the demand pollers leave their idle
    /// nap right away. `Arc` so the pollers can hold it without `AppState`.
    pub automations_changed: std::sync::Arc<tokio::sync::Notify>,
    /// The automation engine's watch over devices held by a pending timed
    /// hold: a manual change to one releases it from the hold instead of
    /// being clobbered at restore time.
    pub hold_watch: api::automations::HoldWatch,
    /// Fired whenever device inventory changes outside device state — rename,
    /// glyph, enable/disable, room assignment, manual shadow. Rides the
    /// `/api/events` SSE stream as an `inventory` event so every surface (and
    /// every other client — the wall kiosk) refreshes its device lists live
    /// instead of waiting for a reload. Payload = the changed table.
    pub inventory_events: tokio::sync::broadcast::Sender<String>,
    /// Last occupancy verdict seen per room — the changed-only seam behind the
    /// `bifrost::rooms` occupancy debug log (a verdict flip logs once, the
    /// kiosk scheduler's steady 30s re-reads stay silent). `Arc` so the sensor
    /// DB-writer tasks recompute (and log) on every presence event without
    /// holding `AppState`. Std mutex: never held across an await.
    pub occupancy_seen: api::rooms::OccupancySeen,
    cipher: Aes256Gcm,
    /// Non-reversible fingerprint of the derived credential key — for the startup
    /// diagnostic that catches a silently-changed `BIFROST_SECRET`.
    pub key_fp: String,
}

impl AppState {
    pub fn new(db: SqlitePool, secret: &str, registry: ProviderRegistry) -> Self {
        Self {
            db,
            cipher: crypto::cipher_from_secret(secret),
            key_fp: crypto::key_fingerprint(secret),
            registry,
            connections: Mutex::new(ConnectionRegistry::new()),
            started_at: std::time::Instant::now(),
            instance_id: uuid::Uuid::new_v4().to_string(),
            kiosk_commands: tokio::sync::broadcast::channel(64).0,
            automations_changed: std::sync::Arc::new(tokio::sync::Notify::new()),
            hold_watch: api::automations::HoldWatch::default(),
            inventory_events: tokio::sync::broadcast::channel(64).0,
            occupancy_seen: api::rooms::OccupancySeen::default(),
        }
    }

    pub fn encrypt_credentials(&self, plaintext: &str) -> anyhow::Result<String> {
        crypto::encrypt(&self.cipher, plaintext)
    }

    pub fn decrypt_credentials(&self, encoded: &str) -> anyhow::Result<String> {
        let result = crypto::decrypt(&self.cipher, encoded);
        if result.is_err() {
            // The usual cause is a changed effective key, not tampered data —
            // surface the fingerprint so it can be compared to the startup log.
            tracing::debug!(
                target: "bifrost::crypto",
                key_fp = %self.key_fp,
                "credential decrypt failed — if this key_fp differs from a prior run, BIFROST_SECRET's bytes changed"
            );
        }
        result
    }
}

#[derive(Embed)]
#[folder = "frontend/dist"]
struct FrontendAssets;

/// Build the Axum router wired to the given state. Exported for integration tests.
pub fn build_app(state: Arc<AppState>) -> Router {
    Router::new()
        .nest("/api", api::router())
        // MCP (Streamable HTTP, Bearer-gated) — the third surface over the
        // shared service layer, alongside /api (session) and /api/v1 (Bearer).
        .nest_service("/mcp", api::mcp::service(Arc::clone(&state)))
        .fallback(serve_frontend)
        .with_state(state)
        // Tag every request span with method + path at INFO, so a failure log
        // (and any error the handler emits) says *which* request failed — the
        // default span is DEBUG-level, so at `info` the bare "502 Bad Gateway"
        // line carried no method/path/context.
        .layer(TraceLayer::new_for_http().make_span_with(
            |req: &axum::http::Request<axum::body::Body>| {
                tracing::info_span!(
                    "http",
                    method = %req.method(),
                    path = %req.uri().path(),
                )
            },
        ))
        .layer(CorsLayer::permissive())
}

/// Result of comparing the running credential-key fingerprint to the DB's record.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyCheck {
    /// No prior record — recorded the current fingerprint (fresh DB / first run).
    FirstRun,
    /// The running key matches the stored fingerprint.
    Match,
    /// The key changed since last boot — stored credentials won't decrypt.
    Changed { stored: String },
}

/// Compare `key_fp` (a one-way fingerprint of the derived credential key) against
/// the value persisted in `credential_key_check`, recording it on first run.
/// Catches a silently-changed `BIFROST_SECRET` at startup — the usual cause of
/// "same secret, can't decrypt". Stores only the fingerprint, never the secret or
/// key. **Insert-once:** a mismatch is reported but does NOT overwrite the stored
/// value, so it keeps flagging on every boot until the correct secret is restored
/// (the stored fingerprint stays authoritative for what the data needs).
pub async fn verify_credential_key(db: &SqlitePool, key_fp: &str) -> KeyCheck {
    let stored: Option<String> =
        sqlx::query_scalar("SELECT key_fp FROM credential_key_check WHERE id = 1")
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    match stored {
        Some(prev) if prev == key_fp => KeyCheck::Match,
        Some(prev) => KeyCheck::Changed { stored: prev },
        None => {
            let _ = sqlx::query(
                "INSERT OR IGNORE INTO credential_key_check (id, key_fp) VALUES (1, ?)",
            )
            .bind(key_fp)
            .execute(db)
            .await;
            KeyCheck::FirstRun
        }
    }
}

/// Entry point called by `main`. Reads env vars, connects to DB, runs the server.
pub async fn run() -> Result<()> {
    use tracing_subscriber::filter::{LevelFilter, Targets};
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};

    // Console logging keeps the RUST_LOG contract; the journal layer captures
    // Bifrost's own events at DEBUG **independently**, so the in-app event log
    // (Settings → Developer) works without restarting with a louder filter.
    tracing_subscriber::registry()
        .with(fmt::layer().with_filter(EnvFilter::from_default_env()))
        .with(
            journal::JournalLayer.with_filter(
                Targets::new()
                    .with_target("bifrost", LevelFilter::DEBUG)
                    // Device state pushes log at TRACE (changed-only) so they
                    // reach the journal without spamming a `bifrost=debug`
                    // console; see `connection::journal_state_push`.
                    .with_target("bifrost::events", LevelFilter::TRACE),
            ),
        )
        .init();

    let cfg = config::Config::from_env()?;
    let db = db::connect(&cfg.database_url).await?;
    let registry = providers::default_registry();
    let state = Arc::new(AppState::new(db, &cfg.secret, registry));

    // Credential-key diagnostic: this fingerprint is derived purely from
    // BIFROST_SECRET and must be identical on every run, or stored credentials
    // won't decrypt. If it changes while you believe the secret is unchanged, the
    // secret's bytes differ at runtime (trailing whitespace, a stale exported env
    // var shadowing .env, or a change only past the first 32 bytes).
    tracing::info!(
        target: "bifrost::crypto",
        key_fp = %state.key_fp,
        secret_len = cfg.secret.len(),
        "credential key ready"
    );
    match verify_credential_key(&state.db, &state.key_fp).await {
        KeyCheck::FirstRun => tracing::info!(
            target: "bifrost::crypto", key_fp = %state.key_fp,
            "recorded credential key fingerprint (first run / fresh database)"
        ),
        KeyCheck::Match => tracing::debug!(
            target: "bifrost::crypto", key_fp = %state.key_fp,
            "credential key matches the stored fingerprint — stored credentials will decrypt"
        ),
        KeyCheck::Changed { stored } => tracing::error!(
            target: "bifrost::crypto", stored_fp = %stored, current_fp = %state.key_fp,
            "BIFROST_SECRET CHANGED since the last boot — credentials encrypted under the previous secret will NOT decrypt. Restore the previous BIFROST_SECRET, or re-enter every provider credential."
        ),
    }

    start_managers(&state).await;

    // Enforce kiosk scheduled quiet hours (display power saving) in the background.
    tokio::spawn(api::kiosks::run_scheduler(Arc::clone(&state)));
    // Sensor automations: edge-triggered rules over the sensor push streams.
    tokio::spawn(api::automations::run_engine(Arc::clone(&state)));

    let app = build_app(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr).await?;
    tracing::info!("bifrost listening on http://{}", cfg.bind_addr);

    axum::serve(listener, app).await?;
    Ok(())
}

/// Start a connection manager (SSE or polling, per the factory's `ConnectionMode`)
/// for every enabled provider in the DB.
async fn start_managers(state: &Arc<AppState>) {
    use sqlx::Row;

    let rows = match sqlx::query(
        "SELECT id, name, provider_type, credentials FROM providers WHERE enabled = 1",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("could not load providers at startup: {e}");
            return;
        }
    };

    let mut connections = state.connections.lock().await;
    for row in rows {
        let id: String = row.get("id");
        let name: String = row.get("name");
        let provider_type: String = row.get("provider_type");
        let creds_enc: String = row.get("credentials");

        let creds_json = match state.decrypt_credentials(&creds_enc) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!(
                    "failed to decrypt credentials for provider \"{name}\" ({provider_type}, {id}): {e:#}. \
                     This almost always means BIFROST_SECRET changed since the provider \
                     was added. Restore the original secret, or re-enter this provider's \
                     credentials in Settings (Edit credentials) — lights, scenes, groups \
                     and plans are unaffected."
                );
                continue;
            }
        };

        start_manager_for(&mut connections, state, &id, &provider_type, &creds_json);
    }
}

/// Dispatch one provider to the right manager based on its registry connection mode.
/// Used at startup and when a provider is added at runtime.
pub fn start_manager_for(
    connections: &mut ConnectionRegistry,
    state: &AppState,
    provider_id: &str,
    provider_type: &str,
    creds_json: &str,
) {
    use providers::ConnectionMode;

    match state.registry.connection_mode(provider_type) {
        Some(ConnectionMode::Sse) => {
            // The SSE stream is Hue-specific; the HueConnectionManager is the
            // single owner of bridge stream reconnection.
            match providers::hue::HueProvider::from_credentials(creds_json) {
                Ok(provider) => {
                    tracing::info!("starting SSE connection manager for provider {provider_id}");
                    connections.start_sse(
                        provider_id.to_string(),
                        provider,
                        state.db.clone(),
                        state.inventory_events.clone(),
                        state.occupancy_seen.clone(),
                    );
                }
                Err(e) => tracing::error!("failed to build provider {provider_id}: {e:#}"),
            }
        }
        Some(ConnectionMode::HaPush) => {
            // HA pushes every device domain over one WebSocket; build the concrete
            // provider directly (like the Sse arm builds HueProvider) so the push
            // manager can fan state_changed onto the light/media/power pipelines.
            match providers::ha::HaProvider::from_credentials(creds_json) {
                Ok(provider) => {
                    tracing::info!("starting HA push manager for provider {provider_id}");
                    connections.start_ha_push(
                        provider_id.to_string(),
                        provider,
                        state.db.clone(),
                        state.inventory_events.clone(),
                        state.occupancy_seen.clone(),
                    );
                }
                Err(e) => tracing::error!("failed to build HA provider {provider_id}: {e:#}"),
            }
        }
        Some(ConnectionMode::Poll { interval_secs }) => {
            match state.registry.build(provider_type, creds_json) {
                Ok(provider) => {
                    tracing::info!(
                        "starting polling manager for provider {provider_id} (every {interval_secs}s)"
                    );
                    connections.start_polling(
                        provider_id.to_string(),
                        provider,
                        std::time::Duration::from_secs(interval_secs),
                        state.db.clone(),
                        state.inventory_events.clone(),
                    );
                }
                Err(e) => tracing::error!("failed to build provider {provider_id}: {e:#}"),
            }
        }
        None => match state.registry.media_connection_mode(provider_type) {
            Some(providers::MediaConnectionMode::Push) => {
                match state.registry.build_media(provider_type, creds_json) {
                    Ok(provider) => {
                        tracing::info!("starting media push manager for provider {provider_id}");
                        connections.start_media_push(
                            provider_id.to_string(),
                            provider,
                            state.db.clone(),
                        );
                    }
                    Err(e) => {
                        tracing::error!("failed to build media provider {provider_id}: {e:#}")
                    }
                }
            }
            Some(providers::MediaConnectionMode::OnDemand) => {
                // State is read live per request; the demand poller stays idle
                // unless an automation watches one of this provider's devices
                // as a trigger input (then it polls tightly so out-of-band
                // changes — the TV's own remote — fire rules within seconds).
                match state.registry.build_media(provider_type, creds_json) {
                    Ok(provider) => {
                        tracing::info!(
                            "media provider {provider_id} ({provider_type}) reads on demand (+ automation demand-poll)"
                        );
                        // A second instance carries the provider's optional push
                        // channel (a Smart TV's paired ATV session).
                        let push_provider =
                            state.registry.build_media(provider_type, creds_json).ok();
                        connections.start_media_demand_polling(
                            provider_id.to_string(),
                            provider,
                            state.db.clone(),
                            std::sync::Arc::clone(&state.automations_changed),
                            push_provider,
                        );
                    }
                    Err(e) => {
                        tracing::error!("failed to build media provider {provider_id}: {e:#}")
                    }
                }
            }
            None => tracing::warn!("provider {provider_id} has unknown type '{provider_type}'"),
        },
    }
}

async fn serve_frontend(uri: axum::http::Uri) -> axum::response::Response {
    use axum::response::IntoResponse;
    #[allow(unused_imports)]
    use rust_embed::Embed as _;

    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match FrontendAssets::get(path) {
        Some(content) => {
            // mime_guess doesn't always know `.webmanifest`; serve the PWA
            // manifest with the spec type so install works reliably.
            let content_type = if path.ends_with(".webmanifest") {
                "application/manifest+json"
            } else {
                content.metadata.mimetype()
            };
            axum::response::Response::builder()
                .header("content-type", content_type)
                .header("cache-control", cache_control_for(path))
                .body(axum::body::Body::from(content.data.to_vec()))
                .unwrap()
        }
        None => match FrontendAssets::get("index.html") {
            // SPA fallback — always the HTML entry, so never cache it.
            Some(content) => axum::response::Response::builder()
                .header("content-type", "text/html")
                .header("cache-control", "no-cache")
                .body(axum::body::Body::from(content.data.to_vec()))
                .unwrap(),
            None => axum::http::StatusCode::NOT_FOUND.into_response(),
        },
    }
}

/// Cache policy for an embedded frontend asset. Vite fingerprints everything under
/// `assets/` (e.g. `index-abc123.js`), so those are immutable and cache forever;
/// the HTML entry and other unhashed files must **revalidate every load** so a new
/// deploy's bundle is picked up — kiosk WebViews otherwise serve a stale
/// `index.html` (pinned to an old bundle hash) indefinitely.
fn cache_control_for(path: &str) -> &'static str {
    if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

#[cfg(test)]
mod frontend_cache_tests {
    use super::cache_control_for;

    #[test]
    fn hashed_assets_are_immutable_html_entry_is_revalidated() {
        assert_eq!(
            cache_control_for("assets/index-abc123.js"),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(
            cache_control_for("assets/index-abc123.css"),
            "public, max-age=31536000, immutable"
        );
        // The entry HTML and other unhashed files must always revalidate.
        assert_eq!(cache_control_for("index.html"), "no-cache");
        assert_eq!(cache_control_for("favicon.svg"), "no-cache");
        assert_eq!(cache_control_for("manifest.webmanifest"), "no-cache");
    }
}

#[cfg(test)]
mod credential_key_tests {
    use super::{KeyCheck, verify_credential_key};
    use sqlx::SqlitePool;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;

    async fn mem_db() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str(":memory:")
            .unwrap()
            .foreign_keys(true);
        let db = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("./migrations").run(&db).await.unwrap();
        db
    }

    #[tokio::test]
    async fn records_then_matches_then_flags_a_changed_secret() {
        let db = mem_db().await;
        // First boot records the fingerprint.
        assert_eq!(
            verify_credential_key(&db, "fp-aaaa").await,
            KeyCheck::FirstRun
        );
        // Same secret → match.
        assert_eq!(verify_credential_key(&db, "fp-aaaa").await, KeyCheck::Match);
        // A different key is flagged, reporting the stored (authoritative) fp.
        assert_eq!(
            verify_credential_key(&db, "fp-bbbb").await,
            KeyCheck::Changed {
                stored: "fp-aaaa".into()
            }
        );
        // A mismatch must NOT overwrite the stored fp — it keeps flagging.
        assert_eq!(
            verify_credential_key(&db, "fp-bbbb").await,
            KeyCheck::Changed {
                stored: "fp-aaaa".into()
            }
        );
        // Restoring the original secret clears the alarm.
        assert_eq!(verify_credential_key(&db, "fp-aaaa").await, KeyCheck::Match);
    }
}
