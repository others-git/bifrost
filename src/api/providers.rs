use crate::AppState;
use crate::api::auth::Session;
use crate::api::lights::build_provider;
use crate::connection::ConnectionStatus;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
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
        .route("/order", put(reorder_providers))
        .route("/types", get(list_types))
        .route("/scan/{provider_type}", post(scan_network))
        .route("/discover-all", get(discover_all))
        .route("/hue/pair", post(hue_pair))
        .route("/smarttv/pair", post(smarttv_pair))
        .route("/{id}", delete(remove_provider))
        .route("/{id}/config", get(provider_config))
        .route("/{id}/credentials", put(update_credentials))
        .route("/{id}/status", get(provider_status))
        .route("/{id}/prune", put(set_prune))
        .route("/{id}/discover", post(discover))
        .route("/{id}/sync-groups", post(sync_groups))
        .route("/{id}/smarttv/pair-remote", post(smarttv_pair_remote))
}

// ── Network auto-detect ─────────────────────────────────────────────────────

/// Scan the LAN for devices of a provider type that supports auto-detect, so
/// the add-provider form can pre-fill the host. Returns the discovered devices
/// (empty if none answered). A scan that couldn't even probe the network
/// (e.g. no host networking) degrades to an empty list rather than an error —
/// "found nothing" is the honest UX. 404 only when the type has no discoverer.
async fn scan_network(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(provider_type): Path<String>,
) -> impl IntoResponse {
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

// ── Auto-discovery: scan every discoverable type for unconfigured devices ────

/// A device found on the LAN that isn't yet behind a configured provider — the
/// "found devices" surface (à la Home Assistant's discovered integrations). Each
/// carries the provider type it answered for plus pre-shaped credentials, so the
/// add-provider form opens pre-filled with one click.
#[derive(Serialize)]
struct FoundDevice {
    provider_type: &'static str,
    type_name: &'static str,
    host: String,
    label: Option<String>,
    credentials: serde_json::Value,
}

/// Pull the host-ish fields out of a decrypted credentials blob, so a found
/// device already covered by a configured provider can be filtered out. Covers
/// the field names the various schemas use for an address.
fn hosts_from_credentials(json: &str) -> Vec<String> {
    const HOST_KEYS: &[&str] = &["host", "bridge_ip", "ip", "host_ip", "address"];
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .map(|obj| {
            HOST_KEYS
                .iter()
                .filter_map(|k| obj.get(*k).and_then(|v| v.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Scan **every** discoverable provider type at once and return the devices that
/// aren't already configured — the one-button "find what's on my network" flow.
/// Credential-free LAN discovery only (SSDP/eISCP/Govee-LAN), so it surfaces
/// gear like Sonos and Onkyo without the user picking a type first. Like
/// [`scan_network`], a probe that can't reach the network degrades to "found
/// nothing" rather than erroring.
async fn discover_all(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    // Addresses already behind a configured provider — so we only surface *new*
    // devices. Decrypt each provider's creds and collect its host fields.
    let mut known_hosts = std::collections::HashSet::new();
    if let Ok(rows) = sqlx::query("SELECT credentials FROM providers")
        .fetch_all(&state.db)
        .await
    {
        for row in &rows {
            let enc: String = row.get("credentials");
            if let Ok(json) = state.decrypt_credentials(&enc) {
                known_hosts.extend(hosts_from_credentials(&json));
            }
        }
    }

    let extra_subnets = crate::api::settings::expanded_subnets(&state).await;
    let budget = if extra_subnets.is_empty() {
        std::time::Duration::from_secs(3)
    } else {
        std::time::Duration::from_secs(8)
    };

    // Every type that advertises auto-detect. Scan them concurrently.
    let types: Vec<(&'static str, &'static str)> = state
        .registry
        .all_types()
        .into_iter()
        .filter(|t| t.supports_discovery)
        .map(|t| (t.provider_type, t.display_name))
        .collect();

    let scans = types.into_iter().map(|(ptype, type_name)| {
        let registry = &state.registry;
        let opts = crate::providers::discovery::ScanOptions {
            timeout: budget,
            extra_subnets: extra_subnets.clone(),
        };
        async move {
            let Some(discoverer) = registry.discoverer(ptype) else {
                return Vec::new();
            };
            match discoverer.scan(&opts).await {
                Ok(devices) => {
                    tracing::debug!(target: "bifrost::discover", ptype, answered = devices.len(), "auto-scan: type probed");
                    devices
                        .into_iter()
                        .map(|d| FoundDevice {
                            provider_type: ptype,
                            type_name,
                            host: d.host,
                            label: d.label,
                            credentials: d.credentials,
                        })
                        .collect()
                }
                Err(e) => {
                    tracing::warn!(target: "bifrost::discover", ptype, "auto-scan: '{ptype}' could not probe: {e:#}");
                    Vec::new()
                }
            }
        }
    });

    tracing::debug!(target: "bifrost::discover", known_hosts = known_hosts.len(), "auto-scan: probing all discoverable provider types");
    let found: Vec<FoundDevice> = futures_util::future::join_all(scans)
        .await
        .into_iter()
        .flatten()
        .filter(|d| !known_hosts.contains(&d.host))
        .collect();
    tracing::debug!(
        target: "bifrost::discover",
        detected = found.len(),
        hosts = ?found.iter().map(|d| format!("{}:{}", d.provider_type, d.host)).collect::<Vec<_>>(),
        "auto-scan: complete (new, not-yet-added devices)",
    );

    Json(found).into_response()
}

// ── List available provider types (for the setup UI) ───────────────────────

async fn list_types(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    Json(state.registry.all_types()).into_response()
}

// ── Configured provider instances ──────────────────────────────────────────

#[derive(Serialize)]
struct ProviderRow {
    id: String,
    provider_type: String,
    /// Human-facing type name (e.g. "Sonos"); falls back to the type key.
    type_name: String,
    /// UI category: "light", "media", or "integration" (matches the add menu).
    domain: String,
    name: String,
    enabled: bool,
    /// When set, a discover removes devices the provider no longer reports.
    prune: bool,
    /// User-controlled sort position on the Devices page (ascending).
    display_order: i64,
    created_at: String,
}

async fn list_providers(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    match sqlx::query(
        "SELECT id, provider_type, name, enabled, prune, display_order, created_at \
         FROM providers ORDER BY display_order, created_at",
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
                    let domain = match state.registry.ui_domain(&provider_type) {
                        Some(crate::providers::ProviderDomain::Media) => "media",
                        Some(crate::providers::ProviderDomain::Integration) => "integration",
                        _ => "light",
                    }
                    .to_string();
                    ProviderRow {
                        type_name,
                        domain,
                        provider_type,
                        id: r.get("id"),
                        name: r.get("name"),
                        enabled: r.get::<i64, _>("enabled") != 0,
                        prune: r.get::<i64, _>("prune") != 0,
                        display_order: r.get("display_order"),
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
struct ReorderRequest {
    /// Provider ids in the desired top-to-bottom order. The client sends the full
    /// set; unknown ids are ignored, and any provider omitted simply keeps its
    /// stored order (tie-broken by creation time on read).
    order: Vec<String>,
}

/// Persist the Devices-page ordering of provider groups: each listed id gets a
/// `display_order` equal to its index, applied in one transaction.
async fn reorder_providers(
    State(state): State<Arc<AppState>>,
    _: Session,
    Json(req): Json<ReorderRequest>,
) -> impl IntoResponse {
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("db error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    for (idx, id) in req.order.iter().enumerate() {
        if let Err(e) = sqlx::query("UPDATE providers SET display_order = ? WHERE id = ?")
            .bind(idx as i64)
            .bind(id)
            .execute(&mut *tx)
            .await
        {
            tracing::error!("db error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    if let Err(e) = tx.commit().await {
        tracing::error!("db error: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    StatusCode::NO_CONTENT.into_response()
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
    _: Session,
    Json(req): Json<AddProviderRequest>,
) -> impl IntoResponse {
    let is_media = state.registry.is_known_media(&req.provider_type);
    if !state.registry.is_known(&req.provider_type) && !is_media {
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
    let build_check = if is_media {
        state
            .registry
            .build_media(&req.provider_type, &creds_json)
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
        "INSERT INTO providers (id, provider_type, name, credentials, display_order)
         VALUES (?, ?, ?, ?, (SELECT COALESCE(MAX(display_order), -1) + 1 FROM providers))",
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
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
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
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<UpdateCredentialsRequest>,
) -> impl IntoResponse {
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
    let build_check = if state.registry.is_known_media(&provider_type) {
        state
            .registry
            .build_media(&provider_type, &creds_json)
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
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    state.connections.lock().await.stop(&id);

    let _ = sqlx::query("DELETE FROM providers WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;

    // Removing a native provider should re-surface any integration copies it was
    // shadowing (and vice-versa); recompute the de-dup shadows.
    crate::api::dedup::reconcile_duplicates(&state).await;

    StatusCode::NO_CONTENT.into_response()
}

async fn provider_status(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let state_lock = state.connections.lock().await.get_state_lock(&id);

    if let Some(lock) = state_lock {
        let cs = lock.read().await;
        return Json(ConnectionStatus::from_state(&cs)).into_response();
    }

    // No background manager. On-demand media providers (e.g. Sonos) are read
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
        Some(t) if state.registry.is_known_media(&t) => "ready",
        _ => "not_managed",
    };
    Json(serde_json::json!({ "state": label })).into_response()
}

// ── Sync provider groups (rooms/zones mirrors) ─────────────────────────────

/// A device domain a provider group's members can belong to. A single area
/// (HA) may carry members in several of these at once.
#[derive(Clone, Copy)]
enum SyncDomain {
    Light,
    Media,
    Power,
}

/// An area merged across the domains that reported it — one mirror, with members
/// grouped by the domain they belong to.
struct MergedArea {
    provider_group_id: String,
    name: String,
    grouped_ref: Option<String>,
    members: Vec<(SyncDomain, Vec<String>)>,
}

/// Match an area's `member_device_ids` (provider-native) to the local device
/// rows of `domain` and (re)populate that domain's `provider_group_*` table.
/// Table/column names are fixed per domain, so the formatted SQL is injection-free.
async fn refresh_group_members(
    state: &AppState,
    provider_row_id: &str,
    mirror_id: &str,
    domain: SyncDomain,
    member_device_ids: &[String],
) {
    let (device_table, member_table, member_col) = match domain {
        SyncDomain::Light => ("lights", "provider_group_lights", "light_id"),
        SyncDomain::Media => (
            "media_devices",
            "provider_group_media_devices",
            "media_device_id",
        ),
        SyncDomain::Power => (
            "power_devices",
            "provider_group_power_devices",
            "power_device_id",
        ),
    };
    // One transaction for the rebuild: a DELETE plus a lookup+insert per member
    // is otherwise N+1 separate WAL commits on every sync.
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("refresh_group_members: begin failed: {e}");
            return;
        }
    };
    let _ = sqlx::query(&format!(
        "DELETE FROM {member_table} WHERE provider_group_id = ?"
    ))
    .bind(mirror_id)
    .execute(&mut *tx)
    .await;
    for device_id in member_device_ids {
        if let Ok(Some(r)) = sqlx::query(&format!(
            "SELECT id FROM {device_table} WHERE provider_id = ? AND device_id = ?"
        ))
        .bind(provider_row_id)
        .bind(device_id)
        .fetch_optional(&mut *tx)
        .await
        {
            let _ = sqlx::query(&format!(
                "INSERT OR IGNORE INTO {member_table} (provider_group_id, {member_col}) VALUES (?, ?)"
            ))
            .bind(mirror_id)
            .bind(r.get::<String, _>("id"))
            .execute(&mut *tx)
            .await;
        }
    }
    if let Err(e) = tx.commit().await {
        tracing::error!("refresh_group_members: commit failed: {e}");
    }
}

/// Refresh this provider's group mirrors and keep Rooms in step:
/// - upsert `provider_groups` (names, native handles) and their members
/// - rename-follow: a room still carrying its inherited name renames with
///   the provider group
/// - mirrors with no linked room get one: an existing room with the same
///   name is linked, otherwise a new room is created
/// - mirrors that vanished from the provider are removed (links cascade)
async fn sync_groups(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
) -> impl IntoResponse {
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

    // A provider can mirror its rooms/areas across several device domains (HA
    // surfaces lights + power; an Area with only switches still has to sync).
    // Gather each domain's groups, then merge by area id so one area → one
    // mirror with members populated per domain. Hue (light) and Sonos (media)
    // serve a single domain, so this collapses to the old behaviour for them.
    let mut domain_groups: Vec<(SyncDomain, Vec<crate::providers::ProviderGroup>)> = Vec::new();
    if state.registry.is_known(&provider_type) {
        match build_provider(&state, &provider_type, &credentials_enc) {
            Ok(p) => match p.discover_groups().await {
                Ok(g) => domain_groups.push((SyncDomain::Light, g)),
                Err(e) => {
                    tracing::error!("light group discovery error: {e:#}");
                    return StatusCode::BAD_GATEWAY.into_response();
                }
            },
            Err(e) => {
                tracing::error!("failed to build provider: {e:#}");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    }
    if state.registry.is_known_power(&provider_type) {
        match crate::api::power::build_power_provider(&state, &provider_type, &credentials_enc) {
            Ok(p) => match p.discover_groups().await {
                Ok(g) => domain_groups.push((SyncDomain::Power, g)),
                Err(e) => {
                    tracing::error!("power group discovery error: {e:#}");
                    return StatusCode::BAD_GATEWAY.into_response();
                }
            },
            Err(e) => {
                tracing::error!("failed to build power provider: {e:#}");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    }
    if state.registry.is_known_media(&provider_type) {
        match crate::api::media::build_media_provider(&state, &provider_type, &credentials_enc) {
            Ok(p) => match p.discover_groups().await {
                Ok(g) => domain_groups.push((SyncDomain::Media, g)),
                Err(e) => {
                    tracing::error!("media group discovery error: {e:#}");
                    return StatusCode::BAD_GATEWAY.into_response();
                }
            },
            Err(e) => {
                tracing::error!("failed to build media provider: {e:#}");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    }

    // Merge by area id, preserving first-seen order so room creation is stable.
    let mut order: Vec<String> = Vec::new();
    let mut merged: std::collections::HashMap<String, MergedArea> =
        std::collections::HashMap::new();
    for (dom, groups) in domain_groups {
        for g in groups {
            let entry = merged
                .entry(g.provider_group_id.clone())
                .or_insert_with(|| {
                    order.push(g.provider_group_id.clone());
                    MergedArea {
                        provider_group_id: g.provider_group_id.clone(),
                        name: g.name.clone(),
                        grouped_ref: g.grouped_ref.clone(),
                        members: Vec::new(),
                    }
                });
            if entry.grouped_ref.is_none() {
                entry.grouped_ref = g.grouped_ref.clone();
            }
            entry.members.push((dom, g.member_device_ids));
        }
    }
    let provider_groups: Vec<MergedArea> = order
        .into_iter()
        .filter_map(|k| merged.remove(&k))
        .collect();

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

        // Refresh members for each domain this area carries (matched to local
        // device rows by provider-native id).
        for (dom, member_ids) in &pg.members {
            refresh_group_members(&state, &id, &mirror_id, *dom, member_ids).await;
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

async fn hue_pair(_: Session, Json(req): Json<HuePairRequest>) -> impl IntoResponse {
    use crate::providers::hue::pairing::{self, PairOutcome};

    // Default to HTTPS: the Bridge Pro only serves the `/api` pairing endpoint
    // over HTTPS and redirects plain HTTP, which downgrades our POST to a GET
    // (bridge error 4). HTTPS-with-self-signed-cert works on every CLIP v2
    // bridge, so this is safe for the older square bridge too.
    let base = if req.bridge_ip.starts_with("http://") || req.bridge_ip.starts_with("https://") {
        req.bridge_ip.clone()
    } else {
        format!("https://{}", req.bridge_ip)
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

// ── Smart-TV (Bravia) PIN pairing ───────────────────────────────────────────

#[derive(Deserialize)]
struct SmartTvPairRequest {
    /// The TV's IP / host (or a full base URL in tests).
    host: String,
    /// The on-screen PIN, when completing pairing. Absent/empty = begin (the TV
    /// then displays a PIN to submit on the next call).
    #[serde(default)]
    pin: Option<String>,
}

/// Two-phase TV pairing: a first call (no `pin`) makes the TV show a PIN; a
/// second call with that `pin` returns the `auth` token to store as the
/// provider's credential alongside `host`.
async fn smarttv_pair(_: Session, Json(req): Json<SmartTvPairRequest>) -> impl IntoResponse {
    use crate::providers::smarttv::{self, SmartTvPairOutcome};

    tracing::debug!(target: "bifrost::smarttv", host = %req.host, step = if req.pin.as_deref().is_some_and(|p| !p.is_empty()) { "submit-pin" } else { "begin" }, "smart-TV pair request");
    let outcome = match req.pin.as_deref() {
        Some(pin) if !pin.is_empty() => smarttv::pair_complete(&req.host, pin)
            .await
            .map(|auth| SmartTvPairOutcome::Paired { auth }),
        _ => smarttv::pair_begin(&req.host).await,
    };
    match outcome {
        Ok(SmartTvPairOutcome::Paired { auth }) => {
            Json(serde_json::json!({ "status": "paired", "auth": auth })).into_response()
        }
        Ok(SmartTvPairOutcome::PinDisplayed) => Json(serde_json::json!({
            "status": "pin_displayed",
            "message": "Enter the PIN shown on the TV screen."
        }))
        .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": "tv_unreachable", "message": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct AtvPairRequest {
    /// The on-screen code to finish pairing. Absent/empty = begin (TV shows it).
    #[serde(default)]
    code: Option<String>,
}

/// Two-phase **Android TV Remote** pairing for an existing smart-TV provider
/// (the modern remote-key transport for Android/Google TV Bravias). A first call
/// (no `code`) makes the TV display a 6-digit code; a second call with that code
/// generates + stores the client certificate in the provider's credentials.
async fn smarttv_pair_remote(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<AtvPairRequest>,
) -> impl IntoResponse {
    use crate::providers::smarttv;

    // Resolve the TV's host from the provider's stored credentials.
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
    if provider_type != "smarttv" {
        return (StatusCode::UNPROCESSABLE_ENTITY, "not a smart-TV provider").into_response();
    }
    let mut creds = match state
        .decrypt_credentials(&credentials_enc)
        .ok()
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| v.as_object().cloned())
    {
        Some(o) => o,
        None => {
            return (StatusCode::UNPROCESSABLE_ENTITY, "credentials unreadable").into_response();
        }
    };
    let host = match creds.get("host").and_then(|v| v.as_str()) {
        Some(h) if !h.trim().is_empty() => h.to_string(),
        _ => return (StatusCode::UNPROCESSABLE_ENTITY, "provider has no host").into_response(),
    };

    match req.code.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        // Phase 2: finish with the code and persist the cert/key into the creds.
        Some(code) => match smarttv::atv_pair_complete(&host, code).await {
            Ok((cert_pem, key_pem)) => {
                creds.insert("atv_cert".into(), cert_pem.into());
                creds.insert("atv_key".into(), key_pem.into());
                let creds_json = serde_json::Value::Object(creds).to_string();
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
                tracing::info!(target: "bifrost::smarttv", provider = %id, host = %host, "ATV remote paired");
                Json(serde_json::json!({ "status": "paired" })).into_response()
            }
            Err(e) => {
                tracing::warn!(target: "bifrost::smarttv", host = %host, "ATV pair finish failed: {e:#}");
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "error": "pair_failed", "message": e.to_string() })),
                )
                    .into_response()
            }
        },
        // Phase 1: begin — the TV displays a code.
        None => {
            match smarttv::atv_pair_begin(&host).await {
                Ok(()) => Json(serde_json::json!({
                    "status": "code_displayed",
                    "message": "Enter the code shown on the TV screen."
                }))
                .into_response(),
                Err(e) => {
                    tracing::warn!(target: "bifrost::smarttv", host = %host, "ATV pair begin failed: {e:#}");
                    (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "error": "tv_unreachable", "message": e.to_string() })),
                )
                    .into_response()
                }
            }
        }
    }
}

// ── Discovery ───────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct DiscoverResponse {
    discovered: usize,
    #[serde(default)]
    pruned: u64,
}

#[derive(Deserialize)]
struct SetPruneRequest {
    prune: bool,
}

/// Set a provider's "prune stale devices on discover" preference.
async fn set_prune(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Json(req): Json<SetPruneRequest>,
) -> impl IntoResponse {
    match sqlx::query("UPDATE providers SET prune = ? WHERE id = ?")
        .bind(i64::from(req.prune))
        .bind(&id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("db error setting prune: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `?prune=true|false` overrides the provider's stored `prune` flag for one run.
#[derive(Deserialize)]
struct DiscoverQuery {
    prune: Option<bool>,
}

async fn discover(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(id): Path<String>,
    Query(q): Query<DiscoverQuery>,
) -> impl IntoResponse {
    let row = sqlx::query(
        "SELECT provider_type, credentials, prune FROM providers WHERE id = ? AND enabled = 1",
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
    // Prune stale devices when the request asks, else when the provider's flag
    // is set. `prune_before` is captured now; rediscovered devices get a newer
    // `last_seen`, so anything older is no longer reported and gets removed.
    let prune = q.prune.unwrap_or(row.get::<i64, _>("prune") != 0);
    let prune_before: String = sqlx::query_scalar("SELECT datetime('now')")
        .fetch_one(&state.db)
        .await
        .unwrap_or_default();

    // A provider type can serve several device domains (Home Assistant does
    // lights + power from one row); discover each domain it's registered for and
    // sum the counts. A hard failure in any domain aborts the whole discover.
    let mut discovered = 0usize;
    tracing::debug!(
        target: "bifrost::discover",
        provider = %id,
        %provider_type,
        prune,
        "discovery started"
    );

    // Light domain.
    if state.registry.is_known(&provider_type) {
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
        tracing::debug!(target: "bifrost::discover", %provider_type, lights = lights.len(), "discovered lights");
        // One transaction for the whole batch: in WAL mode each loose INSERT is
        // its own commit/fsync, so a bridge with dozens of bulbs paid dozens of
        // round-trips. Begin failure falls back to no-op (discovery just reports 0).
        if let Ok(mut tx) = state.db.begin().await {
            for light in &lights {
                let light_id = light.id.to_string();
                let caps = serde_json::to_string(&light.capabilities).unwrap_or_default();
                let state_json = serde_json::to_string(&light.state).unwrap_or_default();
                let _ = sqlx::query(
                    "INSERT INTO lights (id, provider_id, device_id, name, provider_name, capabilities, last_state, last_seen, hw_id)
                     VALUES (?, ?, ?, ?, ?, ?, ?, datetime('now'), ?)
                     ON CONFLICT (provider_id, device_id)
                     DO UPDATE SET name        = CASE WHEN name = provider_name THEN excluded.name ELSE name END,
                                   provider_name = excluded.provider_name,
                                   capabilities = excluded.capabilities,
                                   last_state  = excluded.last_state,
                                   last_seen   = excluded.last_seen,
                                   hw_id       = excluded.hw_id",
                )
                .bind(&light_id)
                .bind(&id)
                .bind(&light.provider_id)
                .bind(&light.name)
                .bind(&light.name)
                .bind(&caps)
                .bind(&state_json)
                .bind(&light.hw_id)
                .execute(&mut *tx)
                .await;
            }
            if let Err(e) = tx.commit().await {
                tracing::error!("discover: commit failed: {e}");
            }
        }
        discovered += lights.len();
    }

    // Power domain (switches/plugs/fans — HA today).
    if state.registry.is_known_power(&provider_type) {
        match crate::api::power::discover_power_devices(
            &state,
            &id,
            &provider_type,
            &credentials_enc,
        )
        .await
        {
            Ok(n) => discovered += n,
            Err(status) => return status.into_response(),
        }
    }

    // Media domain.
    if state.registry.is_known_media(&provider_type) {
        match crate::api::media::discover_media_devices(
            &state,
            &id,
            &provider_type,
            &credentials_enc,
        )
        .await
        {
            Ok(n) => discovered += n,
            Err(status) => return status.into_response(),
        }
    }

    // Remote domain (TV / streamer remotes — HA Android TV today).
    if state.registry.is_known_remote(&provider_type) {
        match crate::api::remote::discover_remote_devices(
            &state,
            &id,
            &provider_type,
            &credentials_enc,
        )
        .await
        {
            Ok(n) => discovered += n,
            Err(status) => return status.into_response(),
        }
    }

    // Prune devices no longer reported — but never on an empty result (a likely
    // transient failure shouldn't wipe a provider's devices).
    let mut pruned = 0u64;
    if prune && discovered > 0 {
        if state.registry.is_known(&provider_type) {
            pruned += prune_stale(&state, &id, "lights", &prune_before).await;
        }
        if state.registry.is_known_power(&provider_type) {
            pruned += prune_stale(&state, &id, "power_devices", &prune_before).await;
        }
        if state.registry.is_known_media(&provider_type) {
            pruned += prune_stale(&state, &id, "media_devices", &prune_before).await;
        }
        if state.registry.is_known_remote(&provider_type) {
            pruned += prune_stale(&state, &id, "remote_devices", &prune_before).await;
        }
    }

    // Collapse any device now reachable both natively and via this integration.
    crate::api::dedup::reconcile_duplicates(&state).await;
    // Pair each TV remote to the TV's media device (shared hardware id).
    crate::api::remote::reconcile_remote_pairings(&state).await;

    Json(DiscoverResponse { discovered, pruned }).into_response()
}

/// Delete a provider's devices in `table` whose `last_seen` predates this
/// discovery run — i.e. the provider didn't report them. `table` is a fixed
/// per-domain identifier, so the formatted SQL is injection-free. Returns the
/// number removed.
async fn prune_stale(state: &AppState, provider_id: &str, table: &str, before: &str) -> u64 {
    sqlx::query(&format!(
        "DELETE FROM {table} WHERE provider_id = ? AND (last_seen IS NULL OR last_seen < ?)"
    ))
    .bind(provider_id)
    .bind(before)
    .execute(&state.db)
    .await
    .map(|r| r.rows_affected())
    .unwrap_or(0)
}
