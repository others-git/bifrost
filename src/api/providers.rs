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
        .route("/scan/{provider_type}", post(scan_network))
        .route("/hue/pair", post(hue_pair))
        .route("/{id}", delete(remove_provider))
        .route("/{id}/config", get(provider_config))
        .route("/{id}/credentials", put(update_credentials))
        .route("/{id}/status", get(provider_status))
        .route("/{id}/discover", post(discover))
        .route("/{id}/sync-groups", post(sync_groups))
}

// ── Network auto-detect ─────────────────────────────────────────────────────

/// Scan the LAN for devices of a provider type that supports auto-detect, so
/// the add-provider form can pre-fill the host. Returns the discovered devices
/// (empty if none answered). A scan that couldn't even probe the network
/// (e.g. no host networking) degrades to an empty list rather than an error —
/// "found nothing" is the honest UX. 404 only when the type has no discoverer.
async fn scan_network(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(provider_type): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let Some(discoverer) = state.registry.discoverer(&provider_type) else {
        return (
            StatusCode::NOT_FOUND,
            "auto-detect is not supported for this provider type",
        )
            .into_response();
    };

    // Expanded-LAN: any configured private subnets widen the HTTP sweep. With
    // extras present a full sweep takes longer, so allow a larger budget.
    let extra_subnets = crate::api::settings::expanded_subnets(&state).await;
    let budget = if extra_subnets.is_empty() {
        std::time::Duration::from_secs(2)
    } else {
        std::time::Duration::from_secs(8)
    };
    let opts = crate::providers::discovery::ScanOptions {
        timeout: budget,
        extra_subnets,
    };

    match discoverer.scan(&opts).await {
        Ok(devices) => Json(devices).into_response(),
        Err(e) => {
            tracing::warn!("network scan for '{provider_type}' could not probe: {e:#}");
            Json(Vec::<crate::providers::discovery::DiscoveredDevice>::new()).into_response()
        }
    }
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
    /// Human-facing type name (e.g. "Sonos"); falls back to the type key.
    type_name: String,
    /// "light" or "audio".
    domain: String,
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
                .map(|r: sqlx::sqlite::SqliteRow| {
                    let provider_type: String = r.get("provider_type");
                    let type_name = state
                        .registry
                        .display_name(&provider_type)
                        .unwrap_or(provider_type.as_str())
                        .to_string();
                    let domain = if state.registry.is_known_audio(&provider_type) {
                        "audio"
                    } else {
                        "light"
                    }
                    .to_string();
                    ProviderRow {
                        type_name,
                        domain,
                        provider_type,
                        id: r.get("id"),
                        name: r.get("name"),
                        enabled: r.get::<i64, _>("enabled") != 0,
                        created_at: r.get("created_at"),
                    }
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

    let is_audio = state.registry.is_known_audio(&req.provider_type);
    if !state.registry.is_known(&req.provider_type) && !is_audio {
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
    let build_check = if is_audio {
        state
            .registry
            .build_audio(&req.provider_type, &creds_json)
            .map(|_| ())
    } else {
        state
            .registry
            .build(&req.provider_type, &creds_json)
            .map(|_| ())
    };
    if let Err(e) = build_check {
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

// ── Current configuration (to prefill the edit form) ───────────────────────

#[derive(Serialize)]
struct ProviderConfig {
    name: String,
    provider_type: String,
    /// Current values for non-secret fields (e.g. host/IP). Password-kind
    /// fields are deliberately omitted — secrets are never sent to the client;
    /// the edit form leaves them blank and the update path keeps them as-is.
    values: serde_json::Map<String, serde_json::Value>,
    /// False when the stored credentials can't be decrypted (e.g. the
    /// BIFROST_SECRET changed). The form then re-collects every field, secrets
    /// included, because there's nothing to merge onto.
    decryptable: bool,
}

/// Return a provider's current non-secret configuration so the edit form can
/// prefill the IP/host without the user re-typing everything.
async fn provider_config(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_session(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let row = sqlx::query("SELECT provider_type, name, credentials FROM providers WHERE id = ?")
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
    let name: String = row.get("name");
    let credentials_enc: String = row.get("credentials");

    // Which fields are secret (never returned to the client)?
    let secret: std::collections::HashSet<&str> = state
        .registry
        .schema(&provider_type)
        .unwrap_or(&[])
        .iter()
        .filter(|f| matches!(f.kind, crate::providers::FieldKind::Password))
        .map(|f| f.name)
        .collect();

    let (values, decryptable) = match state.decrypt_credentials(&credentials_enc) {
        Ok(json) => {
            let mut map = serde_json::Map::new();
            if let Ok(serde_json::Value::Object(obj)) =
                serde_json::from_str::<serde_json::Value>(&json)
            {
                for (k, v) in obj {
                    if !secret.contains(k.as_str()) {
                        map.insert(k, v);
                    }
                }
            }
            (map, true)
        }
        Err(_) => (serde_json::Map::new(), false),
    };

    Json(ProviderConfig {
        name,
        provider_type,
        values,
        decryptable,
    })
    .into_response()
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

    let row = sqlx::query("SELECT provider_type, credentials FROM providers WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await;
    let (provider_type, credentials_enc): (String, String) = match row {
        Ok(Some(r)) => (r.get("provider_type"), r.get("credentials")),
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Merge the submitted fields over the currently-stored ones, so the edit
    // form can change just the IP while leaving secret fields (app keys, API
    // keys) blank to keep them. A blank string means "unchanged". When the old
    // credentials can't be decrypted (e.g. BIFROST_SECRET changed) we start
    // from nothing — the form re-collects every field in that case.
    let mut merged = state
        .decrypt_credentials(&credentials_enc)
        .ok()
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| match v {
            serde_json::Value::Object(o) => Some(o),
            _ => None,
        })
        .unwrap_or_default();
    if let Some(obj) = req.credentials.as_object() {
        for (k, v) in obj {
            // An empty string keeps the stored value (don't wipe a secret).
            if v.as_str() == Some("") {
                continue;
            }
            merged.insert(k.clone(), v.clone());
        }
    }
    let creds_json = serde_json::Value::Object(merged).to_string();

    // Smoke-test before persisting, like add_provider does.
    let build_check = if state.registry.is_known_audio(&provider_type) {
        state
            .registry
            .build_audio(&provider_type, &creds_json)
            .map(|_| ())
    } else {
        state
            .registry
            .build(&provider_type, &creds_json)
            .map(|_| ())
    };
    if let Err(e) = build_check {
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

    if let Some(lock) = state_lock {
        let cs = lock.read().await;
        return Json(ConnectionStatus::from_state(&cs)).into_response();
    }

    // No background manager. On-demand audio providers (e.g. Sonos) are read
    // live per request, so they're operational, not broken — report "ready".
    let provider_type: Option<String> =
        sqlx::query("SELECT provider_type FROM providers WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .map(|r| r.get("provider_type"));

    let label = match provider_type {
        Some(t) if state.registry.is_known_audio(&t) => "ready",
        _ => "not_managed",
    };
    Json(serde_json::json!({ "state": label })).into_response()
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

    // Audio providers (Sonos) mirror their rooms/zones through the same
    // provider_groups + room_links machinery as lights; only the provider build
    // and the member table differ by domain.
    let is_audio = state.registry.is_known_audio(&provider_type);
    let provider_groups = if is_audio {
        match crate::api::audio::build_audio_provider(&state, &provider_type, &credentials_enc) {
            Ok(p) => match p.discover_groups().await {
                Ok(g) => g,
                Err(e) => {
                    tracing::error!("audio group discovery error: {e:#}");
                    return StatusCode::BAD_GATEWAY.into_response();
                }
            },
            Err(e) => {
                tracing::error!("failed to build audio provider: {e:#}");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    } else {
        match build_provider(&state, &provider_type, &credentials_enc) {
            Ok(p) => match p.discover_groups().await {
                Ok(g) => g,
                Err(e) => {
                    tracing::error!("group discovery error: {e:#}");
                    return StatusCode::BAD_GATEWAY.into_response();
                }
            },
            Err(e) => {
                tracing::error!("failed to build provider: {e:#}");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
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

        // Refresh members (matched to discovered devices by device id). Audio
        // groups populate provider_group_audio_devices; light groups the
        // existing provider_group_lights.
        if is_audio {
            let _ =
                sqlx::query("DELETE FROM provider_group_audio_devices WHERE provider_group_id = ?")
                    .bind(&mirror_id)
                    .execute(&state.db)
                    .await;
            for device_id in &pg.member_device_ids {
                if let Ok(Some(r)) = sqlx::query(
                    "SELECT id FROM audio_devices WHERE provider_id = ? AND device_id = ?",
                )
                .bind(&id)
                .bind(device_id)
                .fetch_optional(&state.db)
                .await
                {
                    let _ = sqlx::query(
                        "INSERT OR IGNORE INTO provider_group_audio_devices (provider_group_id, audio_device_id) VALUES (?, ?)",
                    )
                    .bind(&mirror_id)
                    .bind(r.get::<String, _>("id"))
                    .execute(&state.db)
                    .await;
                }
            }
        } else {
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

    // Audio providers populate audio_devices instead of lights.
    if state.registry.is_known_audio(&provider_type) {
        return match crate::api::audio::discover_audio_devices(
            &state,
            &id,
            &provider_type,
            &credentials_enc,
        )
        .await
        {
            Ok(discovered) => Json(DiscoverResponse { discovered }).into_response(),
            Err(status) => status.into_response(),
        };
    }

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
