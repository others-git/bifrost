//! Pluggable AI model-role endpoints (M23 P2) — the config layer for the
//! native voice pipeline's *optional* models.
//!
//! Each role (`transcription` = STT, `chat` = NLU/LLM fallback, `tts` = speech
//! out) is configured as one **OpenAI-compatible** HTTP endpoint, so the user
//! can point Bifrost at whatever they run locally (Ollama / llama.cpp /
//! faster-whisper / LocalAI / their own server). Bifrost ships **zero weights
//! and zero runtimes** — this is purely the thin-client config, mirroring how
//! providers are configured: a row per role with an encrypted optional key, a
//! per-row **Test** action, and graceful degradation (the grammar pipeline
//! works with no endpoints configured at all).
//!
//! Routes (all session-gated):
//! - `GET    /api/ai-endpoints`          — list configured roles (key redacted).
//! - `PUT    /api/ai-endpoints/{role}`   — create/update one role.
//! - `DELETE /api/ai-endpoints/{role}`   — remove one role.
//! - `POST   /api/ai-endpoints/{role}/test` — probe reachability (`GET /models`).

use crate::AppState;
use crate::api::auth::Session;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;
use std::time::Duration;

/// The model roles the voice pipeline understands. Each is optional and
/// independently configured; an unknown role is rejected at the API boundary.
pub(crate) const ROLES: &[&str] = &["transcription", "chat", "tts"];

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_endpoints))
        .route(
            "/{role}",
            axum::routing::put(put_endpoint).delete(delete_endpoint),
        )
        .route("/{role}/test", post(test_endpoint))
}

/// A configured endpoint with its key **decrypted** — for internal pipeline use
/// only (never serialized to a client). Construct via [`endpoint_for`].
#[derive(Clone, Debug)]
pub(crate) struct AiEndpoint {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

/// Load `role`'s stored config, decrypting the key. `enabled_only` filters out a
/// disabled row (the pipeline wants enabled rows; the Test action wants any).
async fn load(state: &AppState, role: &str, enabled_only: bool) -> Option<AiEndpoint> {
    let sql = if enabled_only {
        "SELECT base_url, model, api_key FROM ai_endpoints WHERE role = ? AND enabled = 1"
    } else {
        "SELECT base_url, model, api_key FROM ai_endpoints WHERE role = ?"
    };
    let row = sqlx::query(sql)
        .bind(role)
        .fetch_optional(&state.db)
        .await
        .ok()??;
    // A key that fails to decrypt (e.g. rotated secret) is treated as absent
    // rather than failing the whole lookup.
    let api_key = row
        .get::<Option<String>, _>("api_key")
        .and_then(|enc| state.decrypt_credentials(&enc).ok());
    Some(AiEndpoint {
        base_url: row
            .get::<String, _>("base_url")
            .trim_end_matches('/')
            .to_string(),
        model: row.get("model"),
        api_key,
    })
}

/// The **enabled** endpoint for `role`, with its key decrypted, or `None` when
/// the role is unconfigured/disabled — callers then degrade gracefully.
pub(crate) async fn endpoint_for(state: &AppState, role: &str) -> Option<AiEndpoint> {
    load(state, role, true).await
}

// ── Views / requests ─────────────────────────────────────────────────────────

#[derive(Serialize)]
struct EndpointView {
    role: String,
    base_url: String,
    model: String,
    /// Whether a (non-empty) key is stored — the key itself is never returned.
    has_key: bool,
    enabled: bool,
}

#[derive(Deserialize)]
struct PutEndpoint {
    base_url: String,
    model: String,
    /// Optional bearer key. Three states: a non-empty string sets a new key, an
    /// empty string clears it, and **omitting** the field preserves the stored
    /// one (so the redacted edit form doesn't need to round-trip the secret).
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

/// An OpenAI-compatible base URL must be an absolute http(s) URL (local servers
/// are plain http on the LAN). We don't require a path so `…/v1` is optional.
fn valid_base_url(s: &str) -> bool {
    let s = s.trim();
    (s.starts_with("http://") || s.starts_with("https://")) && s.len() > "https://".len()
}

// ── Handlers ─────────────────────────────────────────────────────────────────

async fn list_endpoints(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    let rows = sqlx::query(
        "SELECT role, base_url, model, api_key, enabled FROM ai_endpoints ORDER BY role",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let views: Vec<EndpointView> = rows
        .into_iter()
        .map(|r| EndpointView {
            role: r.get("role"),
            base_url: r.get("base_url"),
            model: r.get("model"),
            has_key: r
                .get::<Option<String>, _>("api_key")
                .is_some_and(|k| !k.is_empty()),
            enabled: r.get::<i64, _>("enabled") != 0,
        })
        .collect();
    Json(views).into_response()
}

async fn put_endpoint(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(role): Path<String>,
    Json(req): Json<PutEndpoint>,
) -> impl IntoResponse {
    if !ROLES.contains(&role.as_str()) {
        return (
            StatusCode::NOT_FOUND,
            format!(
                "unknown role '{role}'; expected one of {}",
                ROLES.join(", ")
            ),
        )
            .into_response();
    }
    let base_url = req.base_url.trim().to_string();
    if !valid_base_url(&base_url) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "base_url must be an http(s) URL".to_string(),
        )
            .into_response();
    }
    let model = req.model.trim().to_string();
    if model.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "model is required".to_string(),
        )
            .into_response();
    }

    // Resolve the key to store: set (encrypt), clear, or preserve the existing.
    let stored_key: Option<String> = match req.api_key.as_deref() {
        Some("") => None,
        Some(k) => match state.encrypt_credentials(k) {
            Ok(enc) => Some(enc),
            Err(e) => {
                tracing::error!("encryption error: {e}");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        },
        None => sqlx::query_scalar::<_, Option<String>>(
            "SELECT api_key FROM ai_endpoints WHERE role = ?",
        )
        .bind(&role)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .flatten(),
    };

    let res = sqlx::query(
        "INSERT INTO ai_endpoints (role, base_url, model, api_key, enabled, updated_at)
         VALUES (?, ?, ?, ?, ?, datetime('now'))
         ON CONFLICT(role) DO UPDATE SET
             base_url = excluded.base_url,
             model = excluded.model,
             api_key = excluded.api_key,
             enabled = excluded.enabled,
             updated_at = datetime('now')",
    )
    .bind(&role)
    .bind(&base_url)
    .bind(&model)
    .bind(&stored_key)
    .bind(req.enabled as i64)
    .execute(&state.db)
    .await;

    match res {
        Ok(_) => Json(EndpointView {
            role,
            base_url,
            model,
            has_key: stored_key.is_some_and(|k| !k.is_empty()),
            enabled: req.enabled,
        })
        .into_response(),
        Err(e) => {
            tracing::error!("db error saving ai endpoint: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn delete_endpoint(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(role): Path<String>,
) -> impl IntoResponse {
    let _ = sqlx::query("DELETE FROM ai_endpoints WHERE role = ?")
        .bind(&role)
        .execute(&state.db)
        .await;
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Serialize)]
struct TestResult {
    ok: bool,
    message: String,
}

async fn test_endpoint(
    State(state): State<Arc<AppState>>,
    _: Session,
    Path(role): Path<String>,
) -> impl IntoResponse {
    // Test the *stored* config (enabled or not), so a row can be verified before
    // it's switched on.
    let Some(ep) = load(&state, &role, false).await else {
        return (
            StatusCode::NOT_FOUND,
            "configure this role before testing it",
        )
            .into_response();
    };
    let (ok, message) = match probe_models(&ep).await {
        Ok(()) => (true, "Endpoint reachable.".to_string()),
        Err(e) => (false, e),
    };
    Json(TestResult { ok, message }).into_response()
}

/// Probe an endpoint's reachability with `GET {base_url}/models` — the one call
/// every OpenAI-compatible server answers, so it validates connectivity and auth
/// uniformly across roles without needing role-specific payloads.
pub(crate) async fn probe_models(ep: &AiEndpoint) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.get(format!("{}/models", ep.base_url));
    if let Some(k) = &ep.api_key {
        req = req.bearer_auth(k);
    }
    match req.send().await {
        Ok(r) if r.status().is_success() => Ok(()),
        Ok(r) if r.status() == reqwest::StatusCode::UNAUTHORIZED => {
            Err("the endpoint rejected the API key (401)".into())
        }
        Ok(r) => Err(format!("the endpoint returned {}", r.status())),
        Err(e) if e.is_connect() => Err("couldn't connect to the endpoint".into()),
        Err(e) if e.is_timeout() => Err("the endpoint timed out".into()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn valid_base_url_accepts_http_and_https() {
        assert!(valid_base_url("http://localhost:8080/v1"));
        assert!(valid_base_url("https://api.example.com"));
        assert!(!valid_base_url("localhost:8080"));
        assert!(!valid_base_url("ftp://x"));
        assert!(!valid_base_url("https://"));
        assert!(!valid_base_url(""));
    }

    #[tokio::test]
    async fn probe_models_succeeds_on_2xx_and_sends_bearer() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header("authorization", "Bearer secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{ "id": "whisper-1" }]
            })))
            .mount(&server)
            .await;
        let ep = AiEndpoint {
            base_url: server.uri(),
            model: "whisper-1".into(),
            api_key: Some("secret".into()),
        };
        assert!(probe_models(&ep).await.is_ok());
    }

    #[tokio::test]
    async fn probe_models_reports_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let ep = AiEndpoint {
            base_url: server.uri(),
            model: "m".into(),
            api_key: None,
        };
        let err = probe_models(&ep).await.unwrap_err();
        assert!(err.contains("401"), "got: {err}");
    }
}
