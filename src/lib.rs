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
        .fallback(serve_frontend)
        .with_state(state)
        .layer(TraceLayer::new_for_http())
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
                tracing::error!("failed to decrypt credentials for provider {id}: {e:#}");
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
        None => tracing::warn!("provider {provider_id} has unknown type '{provider_type}'"),
    }
}

async fn serve_frontend(uri: axum::http::Uri) -> axum::response::Response {
    use axum::response::IntoResponse;
    #[allow(unused_imports)]
    use rust_embed::Embed as _;

    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match FrontendAssets::get(path) {
        Some(content) => axum::response::Response::builder()
            .header("content-type", content.metadata.mimetype())
            .body(axum::body::Body::from(content.data.to_vec()))
            .unwrap(),
        None => match FrontendAssets::get("index.html") {
            Some(content) => axum::response::Response::builder()
                .header("content-type", "text/html")
                .body(axum::body::Body::from(content.data.to_vec()))
                .unwrap(),
            None => axum::http::StatusCode::NOT_FOUND.into_response(),
        },
    }
}
