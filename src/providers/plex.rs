//! Plex Media Server feed source (LAN, manual token).
//!
//! The first [`FeedProvider`]: surfaces a library's "recently added" items for
//! the Boards feed widget. Reached at a user-entered server URL (default port
//! 32400) with an `X-Plex-Token` sent as a request header — the token never
//! appears in a URL, and the browser never sees it at all (posters go through
//! the `api::feeds` proxy).
//!
//! Endpoints used (all JSON via `Accept: application/json`):
//! - `GET /` — server identity; `machineIdentifier` builds the `app.plex.tv`
//!   deep links (cached per base URL: it's immutable for a server's lifetime).
//! - `GET /library/sections` — the libraries, for the widget config picker.
//! - `GET /library/sections/{key}/recentlyAdded` — the feed itself.
//! - `GET {thumb path}` / `GET /photo/:/transcode` — posters (the transcoder
//!   downscales server-side so a wall tablet doesn't pull multi-MB originals).
//!
//! TV-library shaping: an episode's tile wears the **show** poster and title
//! (episode thumbs are landscape stills — wrong shape for a poster shelf, and
//! exactly what Plex's own Recently Added row does), with `S2·E5` as the
//! subtitle and the show's rating key as `group_key` so the shared rollup can
//! collapse a binge-import into one tile.

use crate::models::feed::{FeedItem, FeedLibrary};
use crate::providers::{
    CredentialField, FeedProvider, FeedProviderFactory, FieldKind, base_url, cached_client,
    is_safe_asset_path,
};
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

/// The port a Plex Media Server listens on by default.
const PLEX_PORT: u16 = 32400;

// ── plex.tv PIN link pairing ─────────────────────────────────────────────────
//
// The friendly alternative to hunting an X-Plex-Token out of a View-XML URL
// (both paths are supported — the token field still accepts a manual paste).
// Flow, per plex.tv's v2 pins API: mint a PIN (`POST /api/v2/pins?strong=false`
// → 4-char code), the user enters the code at https://plex.tv/link from any
// signed-in device, and polling `GET /api/v2/pins/{id}` — with the SAME
// X-Plex-Client-Identifier that minted it — returns the account's auth token,
// which the local server accepts as an X-Plex-Token.

/// plex.tv's base URL for the PIN flow (tests point this at wiremock).
pub const PLEX_TV_BASE: &str = "https://plex.tv";
const PAIR_TIMEOUT: Duration = Duration::from_secs(10);

/// A freshly minted link PIN. `client_id` must ride every poll for this PIN.
#[derive(Debug)]
pub struct PairStart {
    pub id: i64,
    pub code: String,
    pub client_id: String,
}

fn pair_client() -> Result<Client> {
    Ok(Client::builder().timeout(PAIR_TIMEOUT).build()?)
}

/// Mint a link PIN on plex.tv. The returned `code` is what the user types at
/// plex.tv/link; `id` + `client_id` are what [`pair_check`] polls with.
pub async fn pair_begin(base: &str) -> Result<PairStart> {
    let client_id = uuid::Uuid::new_v4().to_string();
    let reply: Value = pair_client()?
        .post(format!("{base}/api/v2/pins?strong=false"))
        .header("X-Plex-Product", "Bifrost")
        .header("X-Plex-Client-Identifier", &client_id)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .context("could not reach plex.tv")?
        .error_for_status()?
        .json()
        .await
        .context("unexpected reply from plex.tv")?;
    let id = reply["id"]
        .as_i64()
        .ok_or_else(|| anyhow!("plex.tv PIN reply carried no id"))?;
    let code = reply["code"]
        .as_str()
        .filter(|c| !c.is_empty())
        .ok_or_else(|| anyhow!("plex.tv PIN reply carried no code"))?
        .to_string();
    Ok(PairStart {
        id,
        code,
        client_id,
    })
}

/// Poll one link PIN: `Ok(Some(token))` once the user entered the code at
/// plex.tv/link, `Ok(None)` while still pending, `Err` when the PIN expired
/// (plex.tv answers 404) or plex.tv is unreachable.
pub async fn pair_check(base: &str, pin_id: i64, client_id: &str) -> Result<Option<String>> {
    let resp = pair_client()?
        .get(format!("{base}/api/v2/pins/{pin_id}"))
        .header("X-Plex-Client-Identifier", client_id)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .context("could not reach plex.tv")?;
    if resp.status().as_u16() == 404 {
        bail!("the link code expired — click Link to get a new one");
    }
    let reply: Value = resp
        .error_for_status()?
        .json()
        .await
        .context("unexpected reply from plex.tv")?;
    Ok(reply["authToken"]
        .as_str()
        .filter(|t| !t.is_empty())
        .map(str::to_string))
}

/// One Plex Media Server, addressed by base URL + auth token.
pub struct PlexProvider {
    client: Client,
    /// e.g. `http://192.168.1.10:32400`.
    base_url: String,
    token: String,
}

impl PlexProvider {
    pub fn new(host: impl AsRef<str>, token: impl Into<String>) -> Result<Self> {
        let base = base_url(host.as_ref(), "http", Some(PLEX_PORT));
        let token = token.into();
        // One pooled client for every server: the token rides each request as a
        // header (nothing client-level varies per server), so a per-host key
        // would only mint duplicate pools that live for the process.
        let client = cached_client("plex", || {
            Ok(Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(15))
                .build()?)
        })?;
        Ok(Self {
            client,
            base_url: base,
            token,
        })
    }

    pub fn from_credentials(creds_json: &str) -> Result<Self> {
        let creds: Value = serde_json::from_str(creds_json)?;
        let host = creds["host"]
            .as_str()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .ok_or_else(|| anyhow!("plex credentials missing host"))?;
        let token = creds["token"]
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| anyhow!("plex credentials missing token"))?;
        Self::new(host, token)
    }

    /// GET a JSON endpoint with the token + JSON accept headers attached.
    async fn get_json(&self, path_and_query: &str) -> Result<Value> {
        let resp = self
            .client
            .get(format!("{}{path_and_query}", self.base_url))
            .header("X-Plex-Token", &self.token)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .context("Plex server request failed")?;
        if resp.status().as_u16() == 401 {
            bail!("Plex rejected the token (401) — re-enter the X-Plex-Token credential");
        }
        Ok(resp.error_for_status()?.json().await?)
    }

    /// The server's `machineIdentifier`, fetched once per base URL and cached
    /// process-wide — it's fixed for a server's lifetime, and every deep link
    /// needs it.
    async fn machine_identifier(&self) -> Result<String> {
        static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, String>>> =
            std::sync::OnceLock::new();
        let cache = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
        if let Some(id) = cache
            .lock()
            .expect("plex machine-id cache poisoned")
            .get(&self.base_url)
        {
            return Ok(id.clone());
        }
        let root = self.get_json("/").await?;
        let id = root["MediaContainer"]["machineIdentifier"]
            .as_str()
            .ok_or_else(|| anyhow!("Plex server reply carried no machineIdentifier"))?
            .to_string();
        cache
            .lock()
            .expect("plex machine-id cache poisoned")
            .insert(self.base_url.clone(), id.clone());
        Ok(id)
    }

    /// Deep link for a rating key — what the feed widget's tap action sends to
    /// a TV remote as an app-link launch. This is the `ACTION_VIEW` form the
    /// **Android TV** Plex app resolves to the item's detail screen (the URI
    /// PlexMeetsHomeAssistant fires over adb to launch content on Android TV).
    /// An `app.plex.tv` web URL is NOT among the ATV app's intent filters
    /// (only `watch.plex.tv` public-catalog slugs and the `plex://` scheme),
    /// so it must not be used here.
    fn deep_link(machine_id: &str, rating_key: &str) -> String {
        format!(
            "plex://server://{machine_id}/com.plexapp.plugins.library/library/metadata/{rating_key}"
        )
    }

    /// Map one `recentlyAdded` metadata object into a source-agnostic
    /// [`FeedItem`]. Returns `None` for entries with no usable identity.
    fn map_item(meta: &Value, machine_id: &str) -> Option<FeedItem> {
        let rating_key = meta["ratingKey"].as_str()?.to_string();
        let kind = meta["type"].as_str().unwrap_or("item").to_string();
        let own_title = meta["title"].as_str().unwrap_or("Untitled").to_string();
        let added_at = meta["addedAt"].as_i64().unwrap_or(0);
        let year = meta["year"].as_i64().map(|y| y.to_string());
        let str_of = |k: &str| meta[k].as_str().map(str::to_string);

        // Tile shaping per kind: grouped kinds (episode/season/track) wear
        // their parent's portrait poster + title and deep-link to the parent,
        // so a rolled-up tile needs no special casing downstream.
        let (title, subtitle, image, group_key, link_key) = match kind.as_str() {
            "episode" => (
                str_of("grandparentTitle").unwrap_or_else(|| own_title.clone()),
                match (meta["parentIndex"].as_i64(), meta["index"].as_i64()) {
                    (Some(s), Some(e)) => Some(format!("S{s}\u{b7}E{e}")),
                    _ => Some(own_title.clone()),
                },
                str_of("grandparentThumb").or_else(|| str_of("thumb")),
                str_of("grandparentRatingKey"),
                str_of("grandparentRatingKey").unwrap_or_else(|| rating_key.clone()),
            ),
            "season" => (
                str_of("parentTitle").unwrap_or_else(|| own_title.clone()),
                Some(own_title.clone()),
                str_of("thumb").or_else(|| str_of("parentThumb")),
                str_of("parentRatingKey"),
                str_of("parentRatingKey").unwrap_or_else(|| rating_key.clone()),
            ),
            "track" => (
                str_of("parentTitle").unwrap_or_else(|| own_title.clone()),
                str_of("grandparentTitle").or(Some(own_title.clone())),
                str_of("parentThumb").or_else(|| str_of("thumb")),
                str_of("parentRatingKey"),
                str_of("parentRatingKey").unwrap_or_else(|| rating_key.clone()),
            ),
            "album" => (
                own_title.clone(),
                str_of("parentTitle").or(year.clone()),
                str_of("thumb"),
                None,
                rating_key.clone(),
            ),
            // movie / show / anything new Plex grows: its own poster + year.
            _ => (
                own_title.clone(),
                year.clone(),
                str_of("thumb"),
                None,
                rating_key.clone(),
            ),
        };

        Some(FeedItem {
            id: rating_key,
            title,
            subtitle,
            kind,
            added_at,
            image_path: image.filter(|p| is_safe_asset_path(p)),
            group_key,
            deep_link: Some(Self::deep_link(machine_id, &link_key)),
        })
    }
}

#[async_trait]
impl FeedProvider for PlexProvider {
    fn name(&self) -> &str {
        "plex"
    }

    async fn libraries(&self) -> Result<Vec<FeedLibrary>> {
        let reply = self.get_json("/library/sections").await?;
        let dirs = reply["MediaContainer"]["Directory"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(dirs
            .iter()
            .filter_map(|d| {
                Some(FeedLibrary {
                    id: d["key"].as_str()?.to_string(),
                    name: d["title"].as_str()?.to_string(),
                    kind: d["type"].as_str().unwrap_or("").to_string(),
                })
            })
            .collect())
    }

    async fn recent(&self, library_id: &str, limit: usize) -> Result<Vec<FeedItem>> {
        // The library id is provider-native but still user-supplied via the
        // widget config — keep it path-safe.
        if library_id.is_empty() || !library_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            bail!("invalid Plex library id");
        }
        let machine_id = self.machine_identifier().await?;
        let reply = self
            .get_json(&format!(
                "/library/sections/{library_id}/recentlyAdded?X-Plex-Container-Start=0&X-Plex-Container-Size={limit}"
            ))
            .await?;
        let metas = reply["MediaContainer"]["Metadata"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(metas
            .iter()
            .filter_map(|m| Self::map_item(m, &machine_id))
            .collect())
    }

    async fn image(
        &self,
        path: &str,
        width: Option<u32>,
        height: Option<u32>,
    ) -> Result<(Vec<u8>, String)> {
        if !is_safe_asset_path(path) {
            bail!("invalid Plex asset path");
        }
        // With a target size, let the server's photo transcoder downscale —
        // posters are multi-MB originals, far more than a board tile needs.
        let url = match (width, height) {
            (Some(w), Some(h)) => {
                let enc: String = url_encode(path);
                format!(
                    "{}/photo/:/transcode?width={w}&height={h}&minSize=1&upscale=1&url={enc}",
                    self.base_url
                )
            }
            _ => format!("{}{path}", self.base_url),
        };
        let resp = self
            .client
            .get(url)
            .header("X-Plex-Token", &self.token)
            .send()
            .await
            .context("Plex image request failed")?
            .error_for_status()?;
        let mime = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();
        Ok((resp.bytes().await?.to_vec(), mime))
    }
}

/// Minimal percent-encoding for a path handed to the transcoder's `url` query
/// parameter (RFC 3986 unreserved characters pass through).
fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

// ── Factory ──────────────────────────────────────────────────────────────────

pub struct PlexFeedFactory;

impl FeedProviderFactory for PlexFeedFactory {
    fn provider_type(&self) -> &'static str {
        "plex"
    }

    fn display_name(&self) -> &'static str {
        "Plex"
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn FeedProvider>> {
        Ok(Box::new(PlexProvider::from_credentials(credentials_json)?))
    }

    fn credentials_schema(&self) -> &'static [CredentialField] {
        &[
            CredentialField {
                name: "host",
                label: "Server URL",
                kind: FieldKind::Url,
                required: true,
                hint: Some(
                    "Your Plex Media Server's address, e.g. http://192.168.1.10:32400 (a bare IP gets :32400 appended).",
                ),
            },
            CredentialField {
                name: "token",
                label: "X-Plex-Token",
                kind: FieldKind::Password,
                required: true,
                hint: Some(
                    "Click Link and enter the code at plex.tv/link \u{2014} or paste a token manually (Plex Web: any item \u{2192} Get Info \u{2192} View XML \u{2192} X-Plex-Token in that page's URL).",
                ),
            },
        ]
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn identity_json() -> Value {
        json!({ "MediaContainer": { "machineIdentifier": "machine-1" } })
    }

    async fn provider_for(server: &MockServer) -> PlexProvider {
        PlexProvider::new(server.uri(), "tok").unwrap()
    }

    #[test]
    fn factory_builds_from_credentials_and_rejects_missing_fields() {
        let f = PlexFeedFactory;
        assert!(
            f.build(r#"{"host":"192.168.1.10","token":"t"}"#).is_ok(),
            "bare IP + token must build (port gets appended)"
        );
        let no_token = f
            .build(r#"{"host":"192.168.1.10"}"#)
            .err()
            .expect("no token");
        assert!(no_token.to_string().contains("token"));
        let no_host = f.build(r#"{"token":"t"}"#).err().expect("no host");
        assert!(no_host.to_string().contains("host"));
    }

    #[tokio::test]
    async fn pair_begin_mints_a_pin_with_identifying_headers() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/pins"))
            .and(query_param("strong", "false"))
            .and(header("X-Plex-Product", "Bifrost"))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(json!({ "id": 12345, "code": "ABCD", "authToken": null })),
            )
            .mount(&server)
            .await;
        let start = pair_begin(&server.uri()).await.unwrap();
        assert_eq!(start.id, 12345);
        assert_eq!(start.code, "ABCD");
        assert!(
            !start.client_id.is_empty(),
            "the client id must be minted so polls can carry it"
        );
    }

    #[tokio::test]
    async fn pair_check_reports_pending_then_token_then_expiry() {
        // Pending: authToken null → None (keep polling).
        let pending = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/pins/7"))
            .and(header("X-Plex-Client-Identifier", "cid-1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "id": 7, "code": "ABCD", "authToken": null })),
            )
            .mount(&pending)
            .await;
        assert_eq!(pair_check(&pending.uri(), 7, "cid-1").await.unwrap(), None);

        // Linked: authToken populated → Some(token).
        let linked = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/pins/7"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "id": 7, "code": "ABCD", "authToken": "tok-99" })),
            )
            .mount(&linked)
            .await;
        assert_eq!(
            pair_check(&linked.uri(), 7, "cid-1").await.unwrap(),
            Some("tok-99".to_string())
        );

        // Expired: plex.tv answers 404 → an actionable error, not a panic/None.
        let expired = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&expired)
            .await;
        let err = pair_check(&expired.uri(), 7, "cid-1")
            .await
            .expect_err("expired pin errors");
        assert!(err.to_string().contains("expired"), "got: {err:#}");
    }

    #[tokio::test]
    async fn libraries_lists_sections_with_the_token_as_a_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/library/sections"))
            .and(header("X-Plex-Token", "tok"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "MediaContainer": { "Directory": [
                    { "key": "1", "title": "Movies", "type": "movie" },
                    { "key": "2", "title": "TV Shows", "type": "show" },
                ]}
            })))
            .mount(&server)
            .await;

        let libs = provider_for(&server).await.libraries().await.unwrap();
        assert_eq!(libs.len(), 2);
        assert_eq!(libs[0].id, "1");
        assert_eq!(libs[1].name, "TV Shows");
        assert_eq!(libs[1].kind, "show");
    }

    #[tokio::test]
    async fn recent_shapes_episodes_as_show_tiles_with_group_keys() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(identity_json()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/library/sections/2/recentlyAdded"))
            .and(query_param("X-Plex-Container-Size", "10"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "MediaContainer": { "Metadata": [
                    {
                        "ratingKey": "900", "type": "episode", "title": "The We We Are",
                        "grandparentTitle": "Severance", "grandparentRatingKey": "500",
                        "grandparentThumb": "/library/metadata/500/thumb/1",
                        "parentIndex": 2, "index": 5, "addedAt": 1750000000
                    },
                    {
                        "ratingKey": "901", "type": "movie", "title": "Dune",
                        "thumb": "/library/metadata/901/thumb/9",
                        "year": 2021, "addedAt": 1749000000
                    },
                ]}
            })))
            .mount(&server)
            .await;

        let items = provider_for(&server).await.recent("2", 10).await.unwrap();
        assert_eq!(items.len(), 2);

        // Episode tile wears the SHOW: title, portrait poster, group key, and a
        // deep link to the show's details (not the episode's).
        let ep = &items[0];
        assert_eq!(ep.title, "Severance");
        assert_eq!(ep.subtitle.as_deref(), Some("S2\u{b7}E5"));
        assert_eq!(
            ep.image_path.as_deref(),
            Some("/library/metadata/500/thumb/1")
        );
        assert_eq!(ep.group_key.as_deref(), Some("500"));
        assert_eq!(
            ep.deep_link.as_deref(),
            Some("plex://server://machine-1/com.plexapp.plugins.library/library/metadata/500")
        );

        // Movie tile: own title/year/poster, ungrouped, self deep link.
        let movie = &items[1];
        assert_eq!(movie.title, "Dune");
        assert_eq!(movie.subtitle.as_deref(), Some("2021"));
        assert!(movie.group_key.is_none());
        assert!(
            movie
                .deep_link
                .as_deref()
                .unwrap()
                .ends_with("/library/metadata/901")
        );
    }

    #[tokio::test]
    async fn recent_rejects_a_path_breaking_library_id() {
        let server = MockServer::start().await;
        let p = provider_for(&server).await;
        assert!(p.recent("2/../secrets", 5).await.is_err());
        assert!(p.recent("", 5).await.is_err());
    }

    #[tokio::test]
    async fn recent_surfaces_a_rejected_token_as_an_actionable_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let err = provider_for(&server)
            .await
            .recent("1", 5)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("token"), "got: {err:#}");
    }

    #[tokio::test]
    async fn image_fetches_raw_path_or_transcodes_when_sized() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/library/metadata/1/thumb/2"))
            .and(header("X-Plex-Token", "tok"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"rawbytes".to_vec())
                    .insert_header("content-type", "image/png"),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/photo/:/transcode"))
            .and(query_param("width", "240"))
            .and(query_param("url", "/library/metadata/1/thumb/2"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"small".to_vec())
                    .insert_header("content-type", "image/jpeg"),
            )
            .mount(&server)
            .await;

        let p = provider_for(&server).await;
        let (bytes, mime) = p
            .image("/library/metadata/1/thumb/2", None, None)
            .await
            .unwrap();
        assert_eq!(
            (bytes.as_slice(), mime.as_str()),
            (&b"rawbytes"[..], "image/png")
        );

        let (bytes, mime) = p
            .image("/library/metadata/1/thumb/2", Some(240), Some(360))
            .await
            .unwrap();
        assert_eq!(
            (bytes.as_slice(), mime.as_str()),
            (&b"small"[..], "image/jpeg")
        );
    }

    #[tokio::test]
    async fn image_refuses_an_absolute_url() {
        let server = MockServer::start().await;
        let p = provider_for(&server).await;
        assert!(p.image("http://evil.example/x", None, None).await.is_err());
        assert!(p.image("//evil.example/x", None, None).await.is_err());
    }
}
