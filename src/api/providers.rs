use crate::AppState;
use crate::api::auth::require_session;
use crate::api::lights::build_provider;
use crate::connection::ConnectionStatus;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_providers).post(add_provider))
        .route("/types", get(list_types))
        .route("/hue/pair", post(hue_pair))
        .route("/{id}", delete(remove_provider))
        .route("/{id}/credentials", put(update_credentials))
        .route("/{id}/status", get(provider_status))
        .route("/{id}/discover", post(discover))
        .route("/{id}/sync-groups", post(sync_groups))
}

// ── List available provider types (for the setup UI) ───────────────────────

async fn list_types(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(state.registry.all_types()).into_response()
}

// ── Configured provider instances ──────────────────────────────────────────

#[derive(Serialize)]
struct ProviderRow {
    id: String,
    provider_type: String,
    name: String,
    enabled: bool,
    created_at: String,
}

async fn list_providers(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    match sqlx::query(
        "SELECT id, provider_type, name, enabled, created_at FROM providers ORDER BY created_at",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|r: sqlx::sqlite::SqliteRow| ProviderRow {
                    id: r.get("id"),
                    provider_type: r.get("provider_type"),
                    name: r.get("name"),
                    enabled: r.get::<i64, _>("enabled") != 0,
                    created_at: r.get("created_at"),
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => {
            tracing::error!("db error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct AddProviderRequest {
    name: String,
    provider_type: String,
    /// Shape must match the schema returned by `GET /api/providers/types`.
    credentials: serde_json::Value,
}

async fn add_provider(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<AddProviderRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    if !state.registry.is_known(&req.provider_type) {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "unknown provider_type '{}'; see GET /api/providers/types",
                req.provider_type
            ),
        )
            .into_response();
    }

    let creds_json = req.credentials.to_string();

    // Smoke-test: try building the provider now so bad credentials fail fast.
    if let Err(e) = state.registry.build(&req.provider_type, &creds_json) {
        return (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response();
    }

    let encrypted = match state.encrypt_credentials(&creds_json) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("encryption error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let id = Uuid::new_v4().to_string();
    match sqlx::query(
        "INSERT INTO providers (id, provider_type, name, credentials) VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&req.provider_type)
    .bind(&req.name)
    .bind(&encrypted)
    .execute(&state.db)
    .await
    {
        Ok(_) => {
            // Start the right manager (SSE or polling) for the new provider immediately.
            {
                let mut connections = state.connections.lock().await;
                crate::start_manager_for(
                    &mut connections,
                    &state,
                    &id,
                    &req.provider_type,
                    &creds_json,
                );
            }
            (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response()
        }
        Err(e) => {
            tracing::error!("db error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
struct UpdateCredentialsRequest {
    credentials: serde_json::Value,
}

/// Replace an existing provider's credentials in place — the recovery path
/// when BIFROST_SECRET changed or a key was rotated. Keeps the provider row
/// (and therefore all lights, scenes, groups, and plan placements) intact.
async fn update_credentials(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<UpdateCredentialsRequest>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let row = sqlx::query("SELECT provider_type FROM providers WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await;
    let provider_type: String = match row {
        Ok(Some(r)) => r.get("provider_type"),
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let creds_json = req.credentials.to_string();

    // Smoke-test before persisting, like add_provider does.
    if let Err(e) = state.registry.build(&provider_type, &creds_json) {
        return (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response();
    }

    let encrypted = match state.encrypt_credentials(&creds_json) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("encryption error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if let Err(e) = sqlx::query(
        "UPDATE providers SET credentials = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(&encrypted)
    .bind(&id)
    .execute(&state.db)
    .await
    {
        tracing::error!("db error: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // Restart the connection manager with the fresh credentials.
    {
        let mut connections = state.connections.lock().await;
        connections.stop(&id);
        crate::start_manager_for(&mut connections, &state, &id, &provider_type, &creds_json);
    }

    StatusCode::NO_CONTENT.into_response()
}

async fn remove_provider(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    state.connections.lock().await.stop(&id);

    let _ = sqlx::query("DELETE FROM providers WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;

    StatusCode::NO_CONTENT.into_response()
}

async fn provider_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let state_lock = state.connections.lock().await.get_state_lock(&id);

    match state_lock {
        None => Json(serde_json::json!({ "state": "not_managed" })).into_response(),
        Some(lock) => {
            let cs = lock.read().await;
            Json(ConnectionStatus::from_state(&cs)).into_response()
        }
    }
}

// ── Sync provider groups (rooms/zones mirrors) ─────────────────────────────

/// Refresh this provider's group mirrors and keep Rooms in step:
/// - upsert `provider_groups` (names, native handles) and their members
/// - rename-follow: a room still carrying its inherited name renames with
///   the provider group
/// - mirrors with no linked room get one: an existing room with the same
///   name is linked, otherwise a new room is created
/// - mirrors that vanished from the provider are removed (links cascade)
async fn sync_groups(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let row = sqlx::query(
        "SELECT provider_type, credentials FROM providers WHERE id = ? AND enabled = 1",
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await;

    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let provider_type: String = row.get("provider_type");
    let credentials_enc: String = row.get("credentials");

    let provider = match build_provider(&state, &provider_type, &credentials_enc) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to build provider: {e:#}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let provider_groups = match provider.discover_groups().await {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("group discovery error: {e:#}");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };

    // Existing mirrors: provider_group_id → (mirror id, name)
    let existing: std::collections::HashMap<String, (String, String)> = sqlx::query(
        "SELECT id, provider_group_id, name FROM provider_groups WHERE provider_id = ?",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| {
        (
            r.get::<String, _>("provider_group_id"),
            (r.get::<String, _>("id"), r.get::<String, _>("name")),
        )
    })
    .collect();

    let mut synced = 0usize;
    let mut rooms_created = 0usize;
    let mut rooms_linked = 0usize;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for pg in &provider_groups {
        seen.insert(pg.provider_group_id.clone());

        // Upsert the mirror.
        let (mirror_id, old_name) = match existing.get(&pg.provider_group_id) {
            Some((mid, old)) => {
                let _ = sqlx::query(
                    "UPDATE provider_groups SET name = ?, grouped_ref = ? WHERE id = ?",
                )
                .bind(&pg.name)
                .bind(&pg.grouped_ref)
                .bind(mid)
                .execute(&state.db)
                .await;
                (mid.clone(), Some(old.clone()))
            }
            None => {
                let mid = Uuid::new_v4().to_string();
                let _ = sqlx::query(
                    "INSERT INTO provider_groups (id, provider_id, provider_group_id, name, grouped_ref)
                     VALUES (?, ?, ?, ?, ?)",
                )
                .bind(&mid)
                .bind(&id)
                .bind(&pg.provider_group_id)
                .bind(&pg.name)
                .bind(&pg.grouped_ref)
                .execute(&state.db)
                .await;
                (mid, None)
            }
        };

        // Refresh members (matched to discovered lights by device id).
        let _ = sqlx::query("DELETE FROM provider_group_lights WHERE provider_group_id = ?")
            .bind(&mirror_id)
            .execute(&state.db)
            .await;
        for device_id in &pg.member_device_ids {
            if let Ok(Some(r)) =
                sqlx::query("SELECT id FROM lights WHERE provider_id = ? AND device_id = ?")
                    .bind(&id)
                    .bind(device_id)
                    .fetch_optional(&state.db)
                    .await
            {
                let _ = sqlx::query(
                    "INSERT OR IGNORE INTO provider_group_lights (provider_group_id, light_id) VALUES (?, ?)",
                )
                .bind(&mirror_id)
                .bind(r.get::<String, _>("id"))
                .execute(&state.db)
                .await;
            }
        }

        // Rename-follow: rooms still carrying the inherited name move with it.
        if let Some(old) = &old_name
            && old != &pg.name
        {
            let _ = sqlx::query(
                "UPDATE rooms SET name = ?, inherited_name = ?
                 WHERE inherited_name = ? AND name = ?
                   AND id IN (SELECT room_id FROM room_links WHERE provider_group_id = ?)",
            )
            .bind(&pg.name)
            .bind(&pg.name)
            .bind(old)
            .bind(old)
            .bind(&mirror_id)
            .execute(&state.db)
            .await;
        }

        // Ensure a room links this mirror.
        let linked = sqlx::query("SELECT 1 FROM room_links WHERE provider_group_id = ?")
            .bind(&mirror_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .is_some();
        if !linked {
            // Case-insensitive: "Living room" must link the Hue "Living Room"
            // rather than spawning a duplicate.
            let room_id = match sqlx::query(
                "SELECT id, inherited_name FROM rooms WHERE name = ? COLLATE NOCASE",
            )
            .bind(&pg.name)
            .fetch_optional(&state.db)
            .await
            {
                Ok(Some(r)) => {
                    let rid: String = r.get("id");
                    if r.get::<Option<String>, _>("inherited_name").is_none() {
                        let _ = sqlx::query("UPDATE rooms SET inherited_name = ? WHERE id = ?")
                            .bind(&pg.name)
                            .bind(&rid)
                            .execute(&state.db)
                            .await;
                    }
                    rooms_linked += 1;
                    rid
                }
                _ => {
                    let rid = Uuid::new_v4().to_string();
                    let _ = sqlx::query(
                        "INSERT INTO rooms (id, name, inherited_name) VALUES (?, ?, ?)",
                    )
                    .bind(&rid)
                    .bind(&pg.name)
                    .bind(&pg.name)
                    .execute(&state.db)
                    .await;
                    rooms_created += 1;
                    rid
                }
            };
            let _ = sqlx::query(
                "INSERT OR IGNORE INTO room_links (room_id, provider_group_id) VALUES (?, ?)",
            )
            .bind(&room_id)
            .bind(&mirror_id)
            .execute(&state.db)
            .await;
        }

        synced += 1;
    }

    // Mirrors that vanished from the provider.
    for (pg_id, (mirror_id, _)) in &existing {
        if !seen.contains(pg_id) {
            let _ = sqlx::query("DELETE FROM provider_groups WHERE id = ?")
                .bind(mirror_id)
                .execute(&state.db)
                .await;
        }
    }

    Json(serde_json::json!({
        "synced": synced,
        "rooms_created": rooms_created,
        "rooms_linked": rooms_linked,
    }))
    .into_response()
}

// ── Hue link-button pairing ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct HuePairRequest {
    /// Bridge IP or full base URL (the latter is used by tests).
    bridge_ip: String,
}

async fn hue_pair(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<HuePairRequest>,
) -> impl IntoResponse {
    use crate::providers::hue::pairing::{self, PairOutcome};

    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let base = if req.bridge_ip.starts_with("http://") || req.bridge_ip.starts_with("https://") {
        req.bridge_ip.clone()
    } else {
        format!("http://{}", req.bridge_ip)
    };

    match pairing::pair(&base).await {
        Ok(PairOutcome::Paired { app_key }) => {
            Json(serde_json::json!({ "app_key": app_key })).into_response()
        }
        Ok(PairOutcome::LinkButtonNotPressed) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "link_button_not_pressed",
                "message": "Press the round link button on the Hue bridge, then try again within 30 seconds."
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": "bridge_unreachable", "message": e.to_string() })),
        )
            .into_response(),
    }
}

// ── Discovery ───────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct DiscoverResponse {
    discovered: usize,
}

async fn discover(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let row = sqlx::query(
        "SELECT provider_type, credentials FROM providers WHERE id = ? AND enabled = 1",
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await;

    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let provider_type: String = row.get("provider_type");
    let credentials_enc: String = row.get("credentials");

    let provider = match build_provider(&state, &provider_type, &credentials_enc) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to build provider: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let lights = match provider.discover().await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("discovery error: {e:#}");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };

    for light in &lights {
        let light_id = light.id.to_string();
        let caps = serde_json::to_string(&light.capabilities).unwrap_or_default();
        let state_json = serde_json::to_string(&light.state).unwrap_or_default();
        let _ = sqlx::query(
            "INSERT INTO lights (id, provider_id, device_id, name, capabilities, last_state, last_seen)
             VALUES (?, ?, ?, ?, ?, ?, datetime('now'))
             ON CONFLICT (provider_id, device_id)
             DO UPDATE SET name        = excluded.name,
                           capabilities = excluded.capabilities,
                           last_state  = excluded.last_state,
                           last_seen   = excluded.last_seen",
        )
        .bind(&light_id)
        .bind(&id)
        .bind(&light.provider_id)
        .bind(&light.name)
        .bind(&caps)
        .bind(&state_json)
        .execute(&state.db)
        .await;
    }

    Json(DiscoverResponse {
        discovered: lights.len(),
    })
    .into_response()
}
