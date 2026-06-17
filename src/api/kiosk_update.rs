//! Kiosk OTA update **relay**.
//!
//! Kiosks are offline, LAN-only clients — they only ever talk to this hub, never
//! the internet. So the hub (which *does* have internet) is the update
//! distribution point: it fetches the latest signed APK from the `bifrost-kiosk`
//! GitHub release, caches it on disk, and serves it to kiosks over the LAN. A
//! kiosk then pulls the manifest, compares `versionCode` to its own, downloads the
//! APK from the hub, and — being a **device-owner** app — installs it silently.
//!
//! Flow:
//! ```text
//!  GitHub release ──(hub has internet)──▶ hub caches app-release.apk
//!                                              │  (LAN, key-auth)
//!  kiosk ◀── GET /api/kiosks/update/apk ───────┘
//! ```
//!
//! Endpoints (mounted under `/api/kiosks` by [`crate::api::kiosks::router`]):
//! - `GET  /update`           (session) — the cached manifest, for the dashboard.
//! - `POST /update`           (session) — fetch the latest release from GitHub and
//!   cache it; the operator's "check for updates" action.
//! - `GET  /update/manifest`  (key)     — the cached manifest a kiosk compares against.
//! - `GET  /update/apk`       (key)     — the cached APK bytes a kiosk installs.
//!
//! The actual push to a kiosk reuses the existing `update` controller command
//! ([`crate::api::kiosks`]); on receipt the kiosk pulls from the two key-auth
//! endpoints above.

use crate::AppState;
use crate::api::apikeys::require_api_key;
use crate::api::auth::Session;
use anyhow::{Context, Result, anyhow};
use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

const MANIFEST_FILE: &str = "manifest.json";
const APK_FILE: &str = "app-release.apk";

pub const DEFAULT_REPO: &str = "others-git/bifrost-kiosk";
pub const DEFAULT_ASSET: &str = "app-release.apk";

/// Where (and from where) the hub caches the kiosk APK. The **source** (repo +
/// asset) is persisted config, editable from the dashboard ([`get_config`] /
/// [`put_config`]); the cache dir + GitHub base URLs stay env-overridable (infra
/// relocation / tests).
#[derive(Clone)]
pub struct KioskUpdateConfig {
    /// Cache directory holding `app-release.apk` + `manifest.json`.
    pub dir: PathBuf,
    /// GitHub REST API base (default `https://api.github.com`).
    pub api_base: String,
    /// raw.githubusercontent base (default `https://raw.githubusercontent.com`).
    pub raw_base: String,
    /// `owner/repo` of the kiosk app (dashboard-configurable).
    pub repo: String,
    /// The release asset to cache — the model-bundled APK (dashboard-configurable).
    pub asset: String,
}

impl KioskUpdateConfig {
    /// Load with the source (repo/asset) from persisted settings and the
    /// infra knobs (cache dir, GitHub bases) from env/defaults.
    pub async fn load(state: &AppState) -> Self {
        let (repo, asset) = load_source(state).await;
        let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
        Self {
            dir: PathBuf::from(env("BIFROST_KIOSK_UPDATE_DIR", "data/kiosk-update")),
            api_base: env("BIFROST_KIOSK_UPDATE_API_BASE", "https://api.github.com"),
            raw_base: env(
                "BIFROST_KIOSK_UPDATE_RAW_BASE",
                "https://raw.githubusercontent.com",
            ),
            repo,
            asset,
        }
    }
}

/// The persisted `(repo, asset)` source, falling back to defaults when unset/blank.
async fn load_source(state: &AppState) -> (String, String) {
    let row = sqlx::query("SELECT kiosk_update_repo, kiosk_update_asset FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    let pick = |v: Option<String>, default: &str| {
        v.map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| default.to_string())
    };
    match row {
        Some(r) => (
            pick(r.get("kiosk_update_repo"), DEFAULT_REPO),
            pick(r.get("kiosk_update_asset"), DEFAULT_ASSET),
        ),
        None => (DEFAULT_REPO.to_string(), DEFAULT_ASSET.to_string()),
    }
}

/// The cached APK's identity. Served to kiosks (which compare `version_code` to
/// their own `BuildConfig.VERSION_CODE`) and shown on the dashboard.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateManifest {
    pub version_code: i64,
    pub version_name: String,
    pub size: i64,
    /// Hex SHA-256 of the APK, so the kiosk can verify the download before install.
    pub sha256: String,
}

/// The latest release resolved from GitHub, before download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRelease {
    pub tag: String,
    pub apk_url: String,
    pub version_code: i64,
    pub version_name: String,
}

// ── GitHub REST shapes (subset) ────────────────────────────────────────────────

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// Parse `versionCode`/`versionName` out of the kiosk's `app/build.gradle.kts`.
/// We read the *tagged* file (not a release asset) so the version is authoritative
/// without touching the kiosk's release workflow. Pure + unit-tested.
pub fn parse_gradle_version(src: &str) -> Option<(i64, String)> {
    let mut code: Option<i64> = None;
    let mut name: Option<String> = None;
    for line in src.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k.trim() {
            "versionCode" if code.is_none() => code = v.trim().parse().ok(),
            "versionName" if name.is_none() => {
                name = Some(v.trim().trim_matches('"').to_string());
            }
            _ => {}
        }
        if code.is_some() && name.is_some() {
            break;
        }
    }
    Some((code?, name?))
}

/// Read the cached manifest, if a download has been cached.
pub fn load_manifest(dir: &Path) -> Option<UpdateManifest> {
    let bytes = std::fs::read(dir.join(MANIFEST_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Ask GitHub for the latest release and resolve the APK url + version. Sends a
/// User-Agent because the GitHub API rejects requests without one.
pub async fn fetch_remote_release(
    client: &reqwest::Client,
    cfg: &KioskUpdateConfig,
) -> Result<RemoteRelease> {
    let rel: GhRelease = client
        .get(format!(
            "{}/repos/{}/releases/latest",
            cfg.api_base, cfg.repo
        ))
        .header(header::USER_AGENT, "bifrost-hub")
        .send()
        .await
        .context("GitHub release request failed")?
        .error_for_status()?
        .json()
        .await
        .context("GitHub release response wasn't JSON")?;

    let apk_url = rel
        .assets
        .iter()
        .find(|a| a.name == cfg.asset)
        .map(|a| a.browser_download_url.clone())
        .ok_or_else(|| anyhow!("release {} has no '{}' asset", rel.tag_name, cfg.asset))?;

    let gradle = client
        .get(format!(
            "{}/{}/{}/app/build.gradle.kts",
            cfg.raw_base, cfg.repo, rel.tag_name
        ))
        .header(header::USER_AGENT, "bifrost-hub")
        .send()
        .await
        .context("build.gradle.kts request failed")?
        .error_for_status()?
        .text()
        .await?;
    let (version_code, version_name) = parse_gradle_version(&gradle)
        .ok_or_else(|| anyhow!("couldn't parse version from build.gradle.kts"))?;

    Ok(RemoteRelease {
        tag: rel.tag_name,
        apk_url,
        version_code,
        version_name,
    })
}

/// Stream the APK to disk (hashing as we go), then atomically swap it in and write
/// the manifest. Streaming keeps a 100MB+ download off the heap.
pub async fn download_and_cache(
    client: &reqwest::Client,
    remote: &RemoteRelease,
    dir: &Path,
) -> Result<UpdateManifest> {
    tokio::fs::create_dir_all(dir)
        .await
        .context("create kiosk-update dir")?;
    let tmp = dir.join("app-release.apk.part");
    let mut resp = client
        .get(&remote.apk_url)
        .header(header::USER_AGENT, "bifrost-hub")
        .send()
        .await
        .context("APK download request failed")?
        .error_for_status()?;

    let mut file = tokio::fs::File::create(&tmp)
        .await
        .context("create temp APK")?;
    let mut hasher = Sha256::new();
    let mut size: i64 = 0;
    while let Some(chunk) = resp.chunk().await.context("APK download stream error")? {
        hasher.update(&chunk);
        size += chunk.len() as i64;
        file.write_all(&chunk).await.context("write APK chunk")?;
    }
    file.flush().await?;
    drop(file);
    tokio::fs::rename(&tmp, dir.join(APK_FILE))
        .await
        .context("swap in APK")?;

    let manifest = UpdateManifest {
        version_code: remote.version_code,
        version_name: remote.version_name.clone(),
        size,
        sha256: hex::encode(hasher.finalize()),
    };
    tokio::fs::write(
        dir.join(MANIFEST_FILE),
        serde_json::to_vec_pretty(&manifest)?,
    )
    .await
    .context("write manifest")?;
    Ok(manifest)
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        // A 100MB+ APK download over a home uplink can take a while.
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .unwrap_or_default()
}

// ── Handlers ───────────────────────────────────────────────────────────────────

/// `GET /api/kiosks/update` (session) — the cached manifest for the dashboard
/// (null when nothing's been fetched yet).
pub async fn update_status(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    let cfg = KioskUpdateConfig::load(&state).await;
    Json(serde_json::json!({ "cached": load_manifest(&cfg.dir) }))
}

/// `POST /api/kiosks/update` (session) — fetch the latest release from GitHub and
/// cache it. The operator's "check for updates" button. Skips the (large)
/// re-download when the cache already holds that versionCode.
pub async fn refresh_update(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    let cfg = KioskUpdateConfig::load(&state).await;
    let client = http_client();

    let remote = match fetch_remote_release(&client, &cfg).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("kiosk update: fetch failed: {e:#}");
            return (StatusCode::BAD_GATEWAY, e.to_string()).into_response();
        }
    };

    if let Some(cached) = load_manifest(&cfg.dir)
        && cached.version_code >= remote.version_code
        && cfg.dir.join(APK_FILE).exists()
    {
        return Json(serde_json::json!({ "manifest": cached, "downloaded": false }))
            .into_response();
    }

    match download_and_cache(&client, &remote, &cfg.dir).await {
        Ok(manifest) => {
            tracing::info!(
                "kiosk update: cached {} (code {})",
                manifest.version_name,
                manifest.version_code
            );
            Json(serde_json::json!({ "manifest": manifest, "downloaded": true })).into_response()
        }
        Err(e) => {
            tracing::error!("kiosk update: download failed: {e:#}");
            (StatusCode::BAD_GATEWAY, e.to_string()).into_response()
        }
    }
}

/// `GET /api/kiosks/update/manifest` (key) — the cached manifest the kiosk
/// compares its installed version against. 204 when nothing is cached.
pub async fn update_manifest(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if require_api_key(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let cfg = KioskUpdateConfig::load(&state).await;
    match load_manifest(&cfg.dir) {
        Some(m) => Json(m).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

/// `GET /api/kiosks/update/apk` (key) — stream the cached APK to the kiosk.
pub async fn serve_apk(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if require_api_key(&state, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let cfg = KioskUpdateConfig::load(&state).await;
    let path = cfg.dir.join(APK_FILE);
    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let stream = tokio_util::io::ReaderStream::new(file);
    (
        [
            (
                header::CONTENT_TYPE,
                "application/vnd.android.package-archive".to_string(),
            ),
            (header::CONTENT_LENGTH, len.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{APK_FILE}\""),
            ),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

/// The dashboard-editable update source.
#[derive(Serialize, Deserialize)]
pub struct SourceConfig {
    pub repo: String,
    pub asset: String,
}

/// Validate an `owner/repo` slug: exactly one slash, both halves non-empty and
/// free of whitespace/path separators (it goes straight into GitHub URLs).
fn valid_repo(repo: &str) -> bool {
    let parts: Vec<&str> = repo.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| !c.is_whitespace() && c != '/' && c != '\\')
        })
}

/// `GET /api/kiosks/update/config` (session) — the configured update source.
pub async fn get_config(State(state): State<Arc<AppState>>, _: Session) -> impl IntoResponse {
    let (repo, asset) = load_source(&state).await;
    Json(SourceConfig { repo, asset })
}

/// `PUT /api/kiosks/update/config` (session) — set the repo + asset the hub pulls
/// kiosk APKs from. Validated so a bad value can't break the GitHub fetch URL.
pub async fn put_config(
    State(state): State<Arc<AppState>>,
    _: Session,
    Json(req): Json<SourceConfig>,
) -> impl IntoResponse {
    let repo = req.repo.trim();
    let asset = req.asset.trim();
    if !valid_repo(repo) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "repo must be 'owner/name'",
        )
            .into_response();
    }
    if asset.is_empty() || asset.contains('/') || asset.contains(char::is_whitespace) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "asset must be a release file name (e.g. app-release.apk)",
        )
            .into_response();
    }
    match sqlx::query(
        "UPDATE config SET kiosk_update_repo = ?, kiosk_update_asset = ? WHERE id = 1",
    )
    .bind(repo)
    .bind(asset)
    .execute(&state.db)
    .await
    {
        Ok(_) => Json(SourceConfig {
            repo: repo.to_string(),
            asset: asset.to_string(),
        })
        .into_response(),
        Err(e) => {
            tracing::error!("kiosk update: saving source failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn parses_version_from_gradle() {
        let src = r#"
        android {
            defaultConfig {
                applicationId = "live.theundead.bifrost.kiosk"
                versionCode = 7
                versionName = "0.1.6"
            }
        }
        "#;
        assert_eq!(parse_gradle_version(src), Some((7, "0.1.6".to_string())));
    }

    #[test]
    fn valid_repo_requires_owner_slash_name() {
        assert!(valid_repo("others-git/bifrost-kiosk"));
        assert!(!valid_repo("nobody"));
        assert!(!valid_repo("a/b/c"));
        assert!(!valid_repo("/bifrost-kiosk"));
        assert!(!valid_repo("owner/ name"));
        assert!(!valid_repo(""));
    }

    #[test]
    fn version_parse_needs_both_fields() {
        assert_eq!(parse_gradle_version("versionCode = 3"), None);
        assert_eq!(parse_gradle_version("nothing here"), None);
    }

    #[test]
    fn load_manifest_roundtrips() {
        let dir = std::env::temp_dir().join(format!("bf-km-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let m = UpdateManifest {
            version_code: 7,
            version_name: "0.1.6".into(),
            size: 123,
            sha256: "abc".into(),
        };
        std::fs::write(dir.join(MANIFEST_FILE), serde_json::to_vec(&m).unwrap()).unwrap();
        assert_eq!(load_manifest(&dir), Some(m));
        assert_eq!(load_manifest(&dir.join("nope")), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn cfg_for(server: &MockServer, dir: PathBuf) -> KioskUpdateConfig {
        KioskUpdateConfig {
            dir,
            api_base: server.uri(),
            raw_base: server.uri(),
            repo: "o/r".into(),
            asset: "app-release.apk".into(),
        }
    }

    #[tokio::test]
    async fn fetch_remote_release_resolves_url_and_version() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/releases/latest"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tag_name": "v0.1.6",
                "assets": [
                    { "name": "app-release-slim.apk", "browser_download_url": "http://x/slim" },
                    { "name": "app-release.apk", "browser_download_url": "http://x/full" }
                ]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/o/r/v0.1.6/app/build.gradle.kts"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("versionCode = 7\nversionName = \"0.1.6\"\n"),
            )
            .mount(&server)
            .await;

        let cfg = cfg_for(&server, std::env::temp_dir());
        let remote = fetch_remote_release(&reqwest::Client::new(), &cfg)
            .await
            .unwrap();
        assert_eq!(remote.tag, "v0.1.6");
        assert_eq!(remote.apk_url, "http://x/full");
        assert_eq!(remote.version_code, 7);
        assert_eq!(remote.version_name, "0.1.6");
    }

    #[tokio::test]
    async fn fetch_errors_when_asset_missing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/releases/latest"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tag_name": "v0.1.6", "assets": []
            })))
            .mount(&server)
            .await;
        let cfg = cfg_for(&server, std::env::temp_dir());
        assert!(
            fetch_remote_release(&reqwest::Client::new(), &cfg)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn download_and_cache_writes_apk_and_manifest() {
        let server = MockServer::start().await;
        let body = b"FAKEAPKBYTES".to_vec();
        Mock::given(method("GET"))
            .and(path("/full"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;

        let dir = std::env::temp_dir().join(format!("bf-dl-{}", uuid::Uuid::new_v4()));
        let remote = RemoteRelease {
            tag: "v0.1.6".into(),
            apk_url: format!("{}/full", server.uri()),
            version_code: 7,
            version_name: "0.1.6".into(),
        };
        let manifest = download_and_cache(&reqwest::Client::new(), &remote, &dir)
            .await
            .unwrap();

        assert_eq!(manifest.version_code, 7);
        assert_eq!(manifest.size, body.len() as i64);
        let on_disk = std::fs::read(dir.join(APK_FILE)).unwrap();
        assert_eq!(on_disk, body);
        // sha256 matches the cached file.
        let expect = hex::encode(Sha256::digest(&body));
        assert_eq!(manifest.sha256, expect);
        assert_eq!(load_manifest(&dir), Some(manifest));
        std::fs::remove_dir_all(&dir).ok();
    }
}
