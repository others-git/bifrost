pub mod apikeys;
pub mod audio;
pub mod auth;
pub mod events;
pub mod lights;
pub mod palette_scenes;
pub mod plans;
pub mod providers;
pub mod rooms;
pub mod scenes;
pub mod setup;
pub mod v1;

use crate::AppState;
use axum::{Json, Router, extract::State, response::IntoResponse, routing::get};
use serde::Serialize;
use sqlx::Row;
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .nest("/api-keys", apikeys::router())
        .nest("/audio", audio::router())
        .nest("/auth", auth::router())
        .nest("/events", events::router())
        .nest("/lights", lights::router())
        .nest("/palette-scenes", palette_scenes::router())
        .nest("/plans", plans::router())
        .nest("/provider-groups", rooms::provider_groups_router())
        .nest("/providers", providers::router())
        .nest("/rooms", rooms::router())
        .nest("/scenes", scenes::router())
        .nest("/setup", setup::router())
        .nest("/v1", v1::router())
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
