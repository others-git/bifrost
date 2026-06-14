//! Hue bridge link-button pairing.
//!
//! Flow: the user presses the physical link button on the bridge, then within
//! 30 seconds we POST to `https://<bridge>/api` with a devicetype. The bridge
//! answers `[{"success":{"username":"...","clientkey":"..."}}]`, and that
//! username becomes the `hue-application-key` for all CLIP v2 requests.
//!
//! If the button was not pressed the bridge answers error type 101.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

const DEVICETYPE: &str = "bifrost#server";
const PAIR_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, PartialEq)]
pub enum PairOutcome {
    /// Pairing succeeded; use this as the `app_key` credential.
    Paired { app_key: String },
    /// The bridge rejected the request because the link button wasn't pressed.
    LinkButtonNotPressed,
}

#[derive(Debug, Deserialize)]
struct PairReplyItem {
    #[serde(default)]
    success: Option<PairSuccess>,
    #[serde(default)]
    error: Option<PairError>,
}

#[derive(Debug, Deserialize)]
struct PairSuccess {
    username: String,
}

#[derive(Debug, Deserialize)]
struct PairError {
    #[serde(rename = "type")]
    kind: i64,
    description: String,
}

/// Attempt one pairing handshake against `bridge_base` (e.g. `http://192.168.1.10`).
///
/// The caller decides whether to retry — typically the UI shows "press the
/// link button" and the user clicks pair again.
pub async fn pair(bridge_base: &str) -> Result<PairOutcome> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        // Don't follow redirects: a bridge that redirects http→https would
        // turn our POST /api into a GET / (bridge error 4). We want the POST
        // to land directly, so callers should pass an https base.
        .redirect(reqwest::redirect::Policy::none())
        .timeout(PAIR_TIMEOUT)
        .build()?;

    let reply: Vec<PairReplyItem> = client
        .post(format!("{bridge_base}/api"))
        .json(&json!({ "devicetype": DEVICETYPE, "generateclientkey": true }))
        .send()
        .await
        .context("could not reach the Hue bridge")?
        .error_for_status()?
        .json()
        .await
        .context("unexpected reply from the Hue bridge")?;

    let item = reply
        .into_iter()
        .next()
        .context("empty reply from the Hue bridge")?;

    if let Some(success) = item.success {
        return Ok(PairOutcome::Paired {
            app_key: success.username,
        });
    }
    if let Some(error) = item.error {
        if error.kind == 101 {
            return Ok(PairOutcome::LinkButtonNotPressed);
        }
        anyhow::bail!("bridge error {}: {}", error.kind, error.description);
    }
    anyhow::bail!("bridge reply had neither success nor error");
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn pair_returns_app_key_on_success() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api"))
            .and(body_partial_json(serde_json::json!({
                "devicetype": "bifrost#server"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "success": { "username": "abc123key", "clientkey": "deadbeef" } }
            ])))
            .mount(&server)
            .await;

        let outcome = pair(&server.uri()).await.unwrap();
        assert_eq!(
            outcome,
            PairOutcome::Paired {
                app_key: "abc123key".into()
            }
        );
    }

    #[tokio::test]
    async fn pair_reports_link_button_not_pressed() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "error": { "type": 101, "address": "", "description": "link button not pressed" } }
            ])))
            .mount(&server)
            .await;

        let outcome = pair(&server.uri()).await.unwrap();
        assert_eq!(outcome, PairOutcome::LinkButtonNotPressed);
    }

    #[tokio::test]
    async fn pair_propagates_other_bridge_errors() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "error": { "type": 7, "address": "", "description": "invalid value" } }
            ])))
            .mount(&server)
            .await;

        let err = pair(&server.uri()).await.unwrap_err();
        assert!(err.to_string().contains("invalid value"));
    }

    #[tokio::test]
    async fn pair_does_not_follow_redirect_to_get() {
        // A Bridge Pro redirects plain HTTP to HTTPS; following it would
        // downgrade our POST /api to a GET / (bridge error 4). We must not
        // follow the redirect — it should surface as an error instead.
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/"))
            .mount(&server)
            .await;

        assert!(pair(&server.uri()).await.is_err());
    }

    #[tokio::test]
    async fn pair_fails_on_unreachable_bridge() {
        // Port 9 (discard) — nothing is listening.
        assert!(pair("http://127.0.0.1:9").await.is_err());
    }

    #[tokio::test]
    async fn pair_fails_on_empty_reply() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        assert!(pair(&server.uri()).await.is_err());
    }
}
