pub mod auth;
pub mod events;
pub mod lights;
pub mod providers;
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
        .nest("/lights", lights::router())
        .nest("/providers", providers::router())
        .nest("/setup", setup::router())
        .route("/health", get(health))
}

#[derive(Serialize)]
struct HealthResponse {
    ok: bool,
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
        let provider_type: String = row.get("provider_type");

        let conn_state = if provider_type == "hue" {
            if let Some(lock) = connections.get_state_lock(&id) {
                lock.read().await.label().to_string()
            } else {
                "not_started".to_string()
            }
        } else {
            "ok".to_string()
        };

        providers.push(ProviderHealth {
            id,
            name,
            state: conn_state,
        });
    }

    Json(HealthResponse {
        ok: true,
        providers,
    })
}
