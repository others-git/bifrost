pub mod api;
pub mod config;
pub mod connection;
pub mod crypto;
pub mod db;
pub mod models;
pub mod providers;

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
    cipher: Aes256Gcm,
}

impl AppState {
    pub fn new(db: SqlitePool, secret: &str, registry: ProviderRegistry) -> Self {
        Self {
            db,
            cipher: crypto::cipher_from_secret(secret),
            registry,
            connections: Mutex::new(ConnectionRegistry::new()),
            started_at: std::time::Instant::now(),
            instance_id: uuid::Uuid::new_v4().to_string(),
            kiosk_commands: tokio::sync::broadcast::channel(64).0,
        }
    }

    pub fn encrypt_credentials(&self, plaintext: &str) -> anyhow::Result<String> {
        crypto::encrypt(&self.cipher, plaintext)
    }

    pub fn decrypt_credentials(&self, encoded: &str) -> anyhow::Result<String> {
        crypto::decrypt(&self.cipher, encoded)
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

/// Entry point called by `main`. Reads env vars, connects to DB, runs the server.
pub async fn run() -> Result<()> {
    use tracing_subscriber::{EnvFilter, fmt};

    fmt().with_env_filter(EnvFilter::from_default_env()).init();

    let cfg = config::Config::from_env()?;
    let db = db::connect(&cfg.database_url).await?;
    let registry = providers::default_registry();
    let state = Arc::new(AppState::new(db, &cfg.secret, registry));

    start_managers(&state).await;

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

    let rows =
        match sqlx::query("SELECT id, provider_type, credentials FROM providers WHERE enabled = 1")
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
        let provider_type: String = row.get("provider_type");
        let creds_enc: String = row.get("credentials");

        let creds_json = match state.decrypt_credentials(&creds_enc) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!(
                    "failed to decrypt credentials for provider {id}: {e:#}. \
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
                    connections.start_sse(provider_id.to_string(), provider, state.db.clone());
                }
                Err(e) => tracing::error!("failed to build provider {provider_id}: {e:#}"),
            }
        }
        Some(ConnectionMode::HaPush) => {
            // HA pushes every device domain over one WebSocket; build the concrete
            // provider directly (like the Sse arm builds HueProvider) so the push
            // manager can fan state_changed onto the light/audio/power pipelines.
            match providers::ha::HaProvider::from_credentials(creds_json) {
                Ok(provider) => {
                    tracing::info!("starting HA push manager for provider {provider_id}");
                    connections.start_ha_push(provider_id.to_string(), provider, state.db.clone());
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
                    );
                }
                Err(e) => tracing::error!("failed to build provider {provider_id}: {e:#}"),
            }
        }
        None => match state.registry.audio_connection_mode(provider_type) {
            Some(providers::AudioConnectionMode::Push) => {
                match state.registry.build_audio(provider_type, creds_json) {
                    Ok(provider) => {
                        tracing::info!("starting audio push manager for provider {provider_id}");
                        connections.start_audio_push(
                            provider_id.to_string(),
                            provider,
                            state.db.clone(),
                        );
                    }
                    Err(e) => {
                        tracing::error!("failed to build audio provider {provider_id}: {e:#}")
                    }
                }
            }
            Some(providers::AudioConnectionMode::OnDemand) => {
                // State is read live per request; nothing to keep alive.
                tracing::info!("audio provider {provider_id} ({provider_type}) reads on demand");
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
