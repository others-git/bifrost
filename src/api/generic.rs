//! Generic "passthrough" device API — the controllable long tail of source
//! device types Bifrost doesn't natively model (climate, cover, lock, `number`,
//! `select`, `button`, …). Devices are read **live** from every provider that
//! registers a generic factory (Home Assistant today), not persisted: a thin,
//! always-fresh escape hatch. The session routes here delegate to the shared
//! service fns so the surface can grow to `/v1`/MCP without forking.

use crate::AppState;
use crate::api::auth::Session;
use crate::models::generic::GenericDevice;
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, put},
};
use serde::Deserialize;
use serde_json::Value;
use sqlx::Row;
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/devices", get(list_handler))
        .route("/devices/control", put(set_control_handler))
}

/// Every generic device across all enabled providers that serve the domain,
/// discovered live. `provider_id` is stamped on each so a control write can route
/// back to it.
pub(crate) async fn list_generic_devices(state: &AppState) -> Vec<GenericDevice> {
    let rows =
        sqlx::query("SELECT id, provider_type, credentials FROM providers WHERE enabled = 1")
            .fetch_all(&state.db)
            .await
            .map_err(|e| tracing::error!("db error listing providers: {e}"))
            .unwrap_or_default();
    let mut out = Vec::new();
    for row in &rows {
        let ptype: String = row.get("provider_type");
        if !state.registry.has_generic(&ptype) {
            continue;
        }
        let pid: String = row.get("id");
        let Ok(creds) = state.decrypt_credentials(&row.get::<String, _>("credentials")) else {
            continue;
        };
        let Ok(provider) = state.registry.build_generic(&ptype, &creds) else {
            continue;
        };
        match provider.discover().await {
            Ok(devices) => {
                for mut d in devices {
                    d.provider_id = pid.clone();
                    out.push(d);
                }
            }
            Err(e) => {
                tracing::debug!(target: "bifrost::generic", provider = %pid, "generic discover failed: {e:#}");
            }
        }
    }
    tracing::debug!(target: "bifrost::generic", devices = out.len(), "generic devices listed");
    out
}

pub(crate) enum SetControlOutcome {
    Ok,
    NotFound,
    BadCommand(String),
    ProviderError,
    Db,
}

/// Apply a control write to one generic device (`key` + JSON `value`), routing to
/// the device's provider.
pub(crate) async fn set_generic_control(
    state: &AppState,
    provider_id: &str,
    device_id: &str,
    key: &str,
    value: &Value,
) -> SetControlOutcome {
    let row = sqlx::query(
        "SELECT provider_type, credentials FROM providers WHERE id = ? AND enabled = 1",
    )
    .bind(provider_id)
    .fetch_optional(&state.db)
    .await;
    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return SetControlOutcome::NotFound,
        Err(e) => {
            tracing::error!("db error: {e}");
            return SetControlOutcome::Db;
        }
    };
    let ptype: String = row.get("provider_type");
    let Ok(creds) = state.decrypt_credentials(&row.get::<String, _>("credentials")) else {
        return SetControlOutcome::Db;
    };
    let provider = match state.registry.build_generic(&ptype, &creds) {
        Ok(p) => p,
        Err(_) => return SetControlOutcome::NotFound,
    };
    tracing::debug!(target: "bifrost::generic", device = %device_id, key, ?value, "generic control →");
    match provider.set_control(device_id, key, value).await {
        Ok(()) => SetControlOutcome::Ok,
        // An unmapped control is the caller's mistake (422), not a gateway fault.
        Err(e) if e.to_string().contains("no service mapping") => {
            SetControlOutcome::BadCommand(e.to_string())
        }
        Err(e) => {
            tracing::error!(target: "bifrost::generic", "set_control error: {e:#}");
            SetControlOutcome::ProviderError
        }
    }
}

async fn list_handler(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    Json(list_generic_devices(&state).await).into_response()
}

#[derive(Deserialize)]
struct SetControlRequest {
    provider_id: String,
    device_id: String,
    key: String,
    #[serde(default)]
    value: Value,
}

async fn set_control_handler(
    State(state): State<Arc<AppState>>,
    _: Session,
    Json(req): Json<SetControlRequest>,
) -> impl IntoResponse {
    match set_generic_control(
        &state,
        &req.provider_id,
        &req.device_id,
        &req.key,
        &req.value,
    )
    .await
    {
        SetControlOutcome::Ok => StatusCode::NO_CONTENT.into_response(),
        SetControlOutcome::NotFound => StatusCode::NOT_FOUND.into_response(),
        SetControlOutcome::BadCommand(m) => (StatusCode::UNPROCESSABLE_ENTITY, m).into_response(),
        SetControlOutcome::ProviderError => StatusCode::BAD_GATEWAY.into_response(),
        SetControlOutcome::Db => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
