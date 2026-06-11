pub mod auth;
pub mod events;
pub mod groups;
pub mod lights;
pub mod providers;
pub mod scenes;
pub mod setup;

use crate::AppState;
use axum::{Json, Router, extract::State, response::IntoResponse, routing::get};
use serde::Serialize;
use sqlx::Row;
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .nest("/auth", auth::router())
        .nest("/events", events::router())
        .nest("/groups", groups::router())
        .nest("/lights", lights::router())
        .nest("/providers", providers::router())
        .nest("/scenes", scenes::router())
        .nest("/setup", setup::router())
        .route("/health", get(health))
}

#[derive(Serialize)]
struct HealthResponse {
    ok: bool,
    version: &'static str,
    uptime_secs: u64,
    providers: Vec<ProviderHealth>,
}

#[derive(Serialize)]
struct ProviderHealth {
    id: String,
    name: String,
    state: String,
}

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rows = sqlx::query(
        "SELECT id, name, provider_type FROM providers WHERE enabled = 1 ORDER BY created_at",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let connections = state.connections.lock().await;
    let mut providers = Vec::with_capacity(rows.len());

    for row in &rows {
        let id: String = row.get("id");
        let name: String = row.get("name");

        // Every enabled provider has a manager (SSE or polling) with a state lock.
        let conn_state = if let Some(lock) = connections.get_state_lock(&id) {
            lock.read().await.label().to_string()
        } else {
            "not_started".to_string()
        };

        providers.push(ProviderHealth {
            id,
            name,
            state: conn_state,
        });
    }

    Json(HealthResponse {
        ok: true,
        version: env!("CARGO_PKG_VERSION"),
        uptime_secs: state.started_at.elapsed().as_secs(),
        providers,
    })
}
