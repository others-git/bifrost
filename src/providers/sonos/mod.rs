//! Sonos local integration via UPnP SOAP on port 1400 — no cloud, no API key.
//!
//! One configured "seed" player is enough: `ZoneGroupTopology#GetZoneGroupState`
//! on the seed returns every player in the household with its IP, so discovery
//! and per-device control both route through topology lookups.
//!
//! Services used (all `POST <player>:1400/...` with a `SOAPACTION` header):
//! - `/ZoneGroupTopology/Control` — household topology (player UUIDs, IPs, names)
//! - `/MediaRenderer/RenderingControl/Control` — Get/SetVolume, Get/SetMute
//! - `/MediaRenderer/AVTransport/Control` — Play/Pause/Stop/Next/Previous,
//!   GetTransportInfo (play state), GetPositionInfo (track DIDL metadata),
//!   SetAVTransportURI (favorites + line-in/TV input switching)
//!
//! Players with a physical input — a line-in jack (amps/ports/Fives) or a TV
//! input (soundbars) — expose it as a selectable `source`; switching to one
//! points the transport at a special URI (`x-rincon-stream:` / `x-sonos-htastream:`),
//! the same mechanism a favorite uses. Capability is read from each player's
//! `device_description.xml` (an `AudioIn` service ⇒ line-in; a soundbar model ⇒ TV).
//!
//! Sonos players have no power state; Bifrost maps `power` to "is playing"
//! (`power: false` pauses, `power: true` plays) — the same convention voice
//! assistants use.

use crate::models::media::MediaEvent;
use crate::models::media::{
    MediaCapabilities, MediaCommand, MediaDevice, MediaDeviceKind, MediaFavorite, MediaState,
    NowPlaying, PlayState, TransportCmd,
};
use crate::providers::discovery::{DeviceDiscovery, SsdpDiscovery};
use crate::providers::{
    CredentialField, Credentials, FieldKind, LanBinding, MediaConnectionMode, MediaProvider,
    MediaProviderFactory, is_portable_hw_id,
};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

// ── XML helpers (targeted extraction; full XML parsing is overkill here) ────

/// Extract the text content of the first `<tag>…</tag>` occurrence.
fn xml_tag(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// Like `xml_tag`, but tolerates attributes on the opening tag — e.g. the
/// `<res protocolInfo="…">URI</res>` element in DIDL-Lite.
fn xml_el(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let s0 = body.find(&open)?;
    let content_start = body[s0..].find('>')? + s0 + 1;
    let close = format!("</{tag}>");
    let end = body[content_start..].find(&close)? + content_start;
    Some(body[content_start..end].to_string())
}

/// Extract an attribute value from the tail of `haystack` starting at `from`.
fn xml_attr(element: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let start = element.find(&needle)? + needle.len();
    let end = element[start..].find('"')? + start;
    Some(element[start..end].to_string())
}

/// Decode the XML entities Sonos uses when nesting XML inside XML.
fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&") // last, so &amp;lt; doesn't double-decode
}

/// A DIDL field's text, with its XML entities decoded. The field contents inside
/// an (already-unescaped) DIDL document are themselves still escaped — a title
/// "Midnight & Angel" arrives as `<dc:title>Midnight &amp; Angel</dc:title>` — so
/// the raw `xml_tag` value must be unescaped a second time, or `&amp;`/`&apos;`
/// leak into the now-playing strings shown in the UI.
fn xml_tag_text(body: &str, tag: &str) -> Option<String> {
    xml_tag(body, tag).map(|v| xml_unescape(&v))
}

// ── SOAP plumbing ───────────────────────────────────────────────────────────

struct SoapCall<'a> {
    /// URL path, e.g. `/MediaRenderer/AVTransport/Control`.
    path: &'a str,
    /// Service URN, e.g. `urn:schemas-upnp-org:service:AVTransport:1`.
    service: &'a str,
    /// Action name, e.g. `Play`.
    action: &'a str,
    /// Pre-rendered argument XML, e.g. `<InstanceID>0</InstanceID><Speed>1</Speed>`.
    args: String,
}

const RENDERING: &str = "urn:schemas-upnp-org:service:RenderingControl:1";
const GROUP_RENDERING: &str = "urn:schemas-upnp-org:service:GroupRenderingControl:1";
const AV_TRANSPORT: &str = "urn:schemas-upnp-org:service:AVTransport:1";
const TOPOLOGY: &str = "urn:schemas-upnp-org:service:ZoneGroupTopology:1";
const CONTENT_DIRECTORY: &str = "urn:schemas-upnp-org:service:ContentDirectory:1";

fn soap_envelope(service: &str, action: &str, args: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:{action} xmlns:u="{service}">{args}</u:{action}></s:Body></s:Envelope>"#
    )
}

// ── Topology model ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Player {
    uuid: String,
    name: String,
    /// `http://<ip>:1400` — derived from the topology Location URL.
    base_url: String,
}

/// A Sonos playback group: members play in sync, controlled through the
/// coordinator. Only groups with 2+ visible members surface as zone devices.
#[derive(Debug, Clone)]
struct Group {
    coordinator_uuid: String,
    member_uuids: Vec<String>,
}

/// Device-id prefix for group zone devices (`group:RINCON_…` = coordinator).
const GROUP_PREFIX: &str = "group:";

/// Per-household cache of known player base URLs (keyed by the configured seed
/// URL), so [`SonosProvider::topology`] can fall back to a *different* player
/// when the seed is offline — every player answers `GetZoneGroupState` the same.
/// The most-recently-answering URL is kept first.
fn topology_cache() -> &'static std::sync::Mutex<HashMap<String, Vec<String>>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Vec<String>>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// The one HTTP client config every Sonos caller shares (see the `"sonos"`
/// [`crate::providers::cached_client`] key): no auth, no per-device config, so
/// one warm pool serves the provider and the LAN binding's identity read alike.
fn build_sonos_client() -> Result<Client> {
    Ok(Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(10))
        .build()?)
}

/// A Sonos player UUID is `RINCON_<mac><suffix>` — the 12-hex MAC follows the
/// `RINCON_` prefix. Extract it as the normalized cross-provider `hw_id` so a
/// player also imported via HA de-dups. `None` if the uuid isn't this shape.
fn sonos_hw_id(uuid: &str) -> Option<String> {
    let rest = uuid.strip_prefix("RINCON_")?;
    let mac: String = rest.chars().take(12).collect();
    crate::providers::mac_hw_id(&mac)
}

/// Parse the (unescaped) ZoneGroupState XML into players and groups.
fn parse_topology(state_xml: &str) -> (Vec<Player>, Vec<Group>) {
    let mut players = Vec::new();
    let mut groups = Vec::new();

    for group_chunk in state_xml.split("<ZoneGroup ").skip(1) {
        let group_el = group_chunk.split('>').next().unwrap_or("");
        let coordinator = xml_attr(group_el, "Coordinator").unwrap_or_default();
        let mut member_uuids = Vec::new();

        for chunk in group_chunk.split("<ZoneGroupMember").skip(1) {
            let element = chunk.split('>').next().unwrap_or("");
            if xml_attr(element, "Invisible").as_deref() == Some("1") {
                continue;
            }
            let (Some(uuid), Some(name), Some(location)) = (
                xml_attr(element, "UUID"),
                xml_attr(element, "ZoneName"),
                xml_attr(element, "Location"),
            ) else {
                continue;
            };
            // Location is e.g. http://192.168.1.50:1400/xml/device_description.xml
            let base_url = location
                .find("/xml/")
                .map(|i| location[..i].to_string())
                .unwrap_or(location);
            member_uuids.push(uuid.clone());
            players.push(Player {
                uuid,
                name,
                base_url,
            });
        }

        if member_uuids.len() >= 2 && !coordinator.is_empty() {
            groups.push(Group {
                coordinator_uuid: coordinator,
                member_uuids,
            });
        }
    }
    (players, groups)
}

// ── Favorites (the FV:2 container) ──────────────────────────────────────────

/// Parse the (unescaped) DIDL-Lite from a `Browse FV:2` Result into favorites.
/// Each `<item>` carries an `id` attribute, a `<dc:title>`, and an optional
/// `<r:description>` (the service/source label).
fn parse_favorites(didl: &str) -> Vec<MediaFavorite> {
    let mut out = Vec::new();
    for chunk in didl.split("<item ").skip(1) {
        let open_el = chunk.split('>').next().unwrap_or("");
        let Some(id) = xml_attr(open_el, "id") else {
            continue;
        };
        let title = xml_tag(chunk, "dc:title")
            .map(|t| xml_unescape(&t))
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let subtitle = xml_tag(chunk, "r:description")
            .map(|d| xml_unescape(&d))
            .filter(|s| !s.is_empty());
        out.push(MediaFavorite {
            id,
            title,
            subtitle,
        });
    }
    out
}

/// Whether a favorite's resource URI is a browseable container (playlist,
/// album, station list) that must be enqueued, versus a single stream that can
/// be set directly as the transport URI.
fn favorite_is_container(uri: &str) -> bool {
    uri.starts_with("x-rincon-cpcontainer:")
        || uri.starts_with("x-rinconplaylist:")
        || uri.starts_with("file:")
}

// ── Physical inputs (line-in / TV) ──────────────────────────────────────────

/// A selectable physical input on a Sonos player: the analog line-in jack
/// (amps/ports/Fives) or the TV input (soundbars). Selecting one points the
/// player's transport at a special URI, exactly like playing a favorite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SonosInput {
    LineIn,
    Tv,
}

impl SonosInput {
    /// User-facing label — also the `source` value echoed back in state and the
    /// token accepted on a `source` command (matched case-insensitively).
    fn label(self) -> &'static str {
        match self {
            SonosInput::LineIn => "Line-In",
            SonosInput::Tv => "TV",
        }
    }

    /// The transport URI that switches a group to this input. `uuid` is the
    /// **owning** player's UUID (the input is physically on that player); for a
    /// group the URI is set on the coordinator but still references the owner.
    fn uri(self, uuid: &str) -> String {
        match self {
            SonosInput::LineIn => format!("x-rincon-stream:{uuid}"),
            SonosInput::Tv => format!("x-sonos-htastream:{uuid}:spdif"),
        }
    }

    /// Recognise the active input from a current transport URI (for read-back).
    fn from_uri(uri: &str) -> Option<Self> {
        if uri.starts_with("x-rincon-stream:") {
            Some(SonosInput::LineIn)
        } else if uri.starts_with("x-sonos-htastream:") {
            Some(SonosInput::Tv)
        } else {
            None
        }
    }
}

/// Derive a player's physical inputs from its `device_description.xml`: an
/// `AudioIn` service in the service list ⇒ a line-in jack; a known soundbar
/// `modelName` ⇒ a TV input. Both signals come from the one description.
fn parse_inputs(device_description: &str) -> Vec<SonosInput> {
    let mut inputs = Vec::new();
    if device_description.contains(":service:AudioIn:") {
        inputs.push(SonosInput::LineIn);
    }
    let model = xml_tag(device_description, "modelName")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ["playbar", "playbase", "beam", "arc", "ray"]
        .iter()
        .any(|m| model.contains(m))
    {
        inputs.push(SonosInput::Tv);
    }
    inputs
}

// ── Provider ────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct SonosProvider {
    client: Client,
    /// Seed player base URL (`http://<ip>:1400`); topology fans out from here.
    seed_url: String,
    /// Prefix for this provider's keys in the process-global caches (topology
    /// fallback URLs, per-player inputs). Empty in production, so the separate
    /// provider instances rebuilt per request for the *same* Sonos system share
    /// one cache (keyed by seed/base URL). Tests set a unique value per instance
    /// so the shared statics can't leak between tests when mock-server ports are
    /// recycled across the run.
    cache_ns: String,
}

/// Heartbeat poll interval behind the push channel — keeps state honest (and the
/// device "alive" while idle) even when GENA events aren't flowing.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

impl SonosProvider {
    fn new_with_base(seed_url: impl Into<String>) -> Result<Self> {
        // Sonos clients carry no auth or per-device config, so every provider
        // shares one pooled client (keyed `"sonos"`) — keeping UPnP/SOAP
        // connections warm across the many per-request rebuilds rather than
        // re-handshaking each call. See [`crate::providers::cached_client`].
        let client = crate::providers::cached_client("sonos", build_sonos_client)?;
        Ok(Self {
            client,
            seed_url: seed_url.into(),
            cache_ns: String::new(),
        })
    }

    /// Namespaced key into the global caches (see [`SonosProvider::cache_ns`]).
    fn cache_key(&self, url: &str) -> String {
        format!("{}{url}", self.cache_ns)
    }

    pub fn new(host: impl AsRef<str>) -> Result<Self> {
        let base = crate::providers::base_url(host.as_ref(), "http", Some(1400));
        Self::new_with_base(base)
    }

    pub fn from_credentials(creds_json: &str) -> Result<Self> {
        let creds: serde_json::Value = serde_json::from_str(creds_json)?;
        let host = creds["host"]
            .as_str()
            .filter(|h| !h.trim().is_empty())
            .ok_or_else(|| anyhow!("sonos credentials missing host"))?;
        Self::new(host)
    }

    #[cfg(test)]
    pub fn new_for_test(base_url: impl Into<String>) -> Result<Self> {
        // A unique cache namespace per instance fully isolates the process-global
        // caches between (possibly parallel) tests, even when mock-server ports
        // are recycled across the run.
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mut p = Self::new_with_base(base_url)?;
        p.cache_ns = format!(
            "test{}:",
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        Ok(p)
    }

    async fn soap(&self, base_url: &str, call: SoapCall<'_>) -> Result<String> {
        let resp = self
            .client
            .post(format!("{base_url}{}", call.path))
            .header(
                "SOAPACTION",
                format!("\"{}#{}\"", call.service, call.action),
            )
            .header("Content-Type", "text/xml; charset=\"utf-8\"")
            .body(soap_envelope(call.service, call.action, &call.args))
            .send()
            .await
            .with_context(|| format!("Sonos {} failed", call.action))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("Sonos {} returned {status}: {body}", call.action));
        }
        Ok(body)
    }

    /// Fetch the household topology. Any player answers `GetZoneGroupState`
    /// identically, so a single offline player — *including the configured seed*
    /// — must not make the whole household uncontrollable. We try the seed first,
    /// then every other player we've seen before (cached, most-recently-answered
    /// first), and only fail if none respond. Invisible members (bridges, bonded
    /// surrounds) are skipped.
    async fn topology(&self) -> Result<(Vec<Player>, Vec<Group>)> {
        let key = self.cache_key(&self.seed_url);
        let mut candidates: Vec<String> = topology_cache()
            .lock()
            .expect("sonos topology cache poisoned")
            .get(&key)
            .cloned()
            .unwrap_or_default();
        if !candidates.contains(&self.seed_url) {
            candidates.push(self.seed_url.clone());
        }

        let mut last_err: Option<anyhow::Error> = None;
        for url in &candidates {
            match self.fetch_topology(url).await {
                Ok((players, groups)) => {
                    // Remember every player URL for next time, the one that just
                    // answered first so a dead seed is skipped on the next call.
                    let mut urls = vec![url.clone()];
                    urls.extend(
                        players
                            .iter()
                            .map(|p| p.base_url.clone())
                            .filter(|u| u != url),
                    );
                    topology_cache()
                        .lock()
                        .expect("sonos topology cache poisoned")
                        .insert(key.clone(), urls);
                    return Ok((players, groups));
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("Sonos topology unavailable")))
    }

    /// One `GetZoneGroupState` against a specific player URL.
    async fn fetch_topology(&self, base_url: &str) -> Result<(Vec<Player>, Vec<Group>)> {
        let body = self
            .soap(
                base_url,
                SoapCall {
                    path: "/ZoneGroupTopology/Control",
                    service: TOPOLOGY,
                    action: "GetZoneGroupState",
                    args: String::new(),
                },
            )
            .await?;
        let state_xml = xml_unescape(&xml_tag(&body, "ZoneGroupState").unwrap_or_default());
        let (players, groups) = parse_topology(&state_xml);
        if players.is_empty() {
            return Err(anyhow!("Sonos topology returned no visible players"));
        }
        Ok((players, groups))
    }

    /// Resolve a device id to the player to control: a plain player id maps to
    /// itself; a `group:` id maps to the group's coordinator (which drives the
    /// whole group for transport, and carries GroupRenderingControl).
    async fn find_target(&self, device_id: &str) -> Result<Player> {
        let (players, _) = self.topology().await?;
        let uuid = device_id.strip_prefix(GROUP_PREFIX).unwrap_or(device_id);
        players
            .into_iter()
            .find(|p| p.uuid == uuid)
            .ok_or_else(|| anyhow!("unknown Sonos player '{device_id}'"))
    }

    /// Resolve a device id to `(player, coordinator)`: the player to read/set
    /// **volume** on, and the group **coordinator** that owns transport (state +
    /// play/pause/next) and now-playing. Standalone → coordinator is the player.
    async fn resolve(&self, device_id: &str) -> Result<(Player, Player)> {
        let (players, groups) = self.topology().await?;
        let uuid = device_id.strip_prefix(GROUP_PREFIX).unwrap_or(device_id);
        let by_uuid: HashMap<&str, &Player> =
            players.iter().map(|p| (p.uuid.as_str(), p)).collect();
        let player = by_uuid
            .get(uuid)
            .map(|p| (*p).clone())
            .ok_or_else(|| anyhow!("unknown Sonos player '{device_id}'"))?;
        let coord_uuid = groups
            .iter()
            .find(|g| g.member_uuids.iter().any(|m| m == uuid))
            .map(|g| g.coordinator_uuid.as_str())
            .unwrap_or(uuid);
        let coordinator = by_uuid
            .get(coord_uuid)
            .map(|p| (*p).clone())
            .unwrap_or_else(|| player.clone());
        Ok((player, coordinator))
    }

    /// Every player in the sync group `uuid` belongs to, including `uuid` itself
    /// (and just `uuid` when it's standalone). Used to locate the member that
    /// physically owns a named input — a line-in/TV jack lives on one specific
    /// player, but a `source` command may arrive on any member (the Control page
    /// collapses a group onto its coordinator), so the owner must be found across
    /// the whole group rather than assumed to be the targeted player.
    async fn group_members(&self, uuid: &str) -> Vec<Player> {
        let Ok((players, groups)) = self.topology().await else {
            return Vec::new();
        };
        let member_uuids: Vec<&str> = groups
            .iter()
            .find(|g| g.member_uuids.iter().any(|m| m == uuid))
            .map(|g| g.member_uuids.iter().map(String::as_str).collect())
            .unwrap_or_else(|| vec![uuid]);
        players
            .into_iter()
            .filter(|p| member_uuids.contains(&p.uuid.as_str()))
            .collect()
    }

    /// Read volume + mute, per player (RenderingControl) or for a whole group
    /// (GroupRenderingControl on the coordinator).
    async fn read_volume_mute(&self, player: &Player, group: bool) -> Result<(u8, bool)> {
        let (path, service, get_vol, get_mute, vol_tag, mute_tag) = if group {
            (
                "/MediaRenderer/GroupRenderingControl/Control",
                GROUP_RENDERING,
                "GetGroupVolume",
                "GetGroupMute",
                "CurrentVolume",
                "CurrentMute",
            )
        } else {
            (
                "/MediaRenderer/RenderingControl/Control",
                RENDERING,
                "GetVolume",
                "GetMute",
                "CurrentVolume",
                "CurrentMute",
            )
        };
        let args = if group {
            "<InstanceID>0</InstanceID>".to_string()
        } else {
            "<InstanceID>0</InstanceID><Channel>Master</Channel>".to_string()
        };

        let vol_body = self
            .soap(
                &player.base_url,
                SoapCall {
                    path,
                    service,
                    action: get_vol,
                    args: args.clone(),
                },
            )
            .await?;
        let volume: u8 = xml_tag(&vol_body, vol_tag)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let mute_body = self
            .soap(
                &player.base_url,
                SoapCall {
                    path,
                    service,
                    action: get_mute,
                    args,
                },
            )
            .await?;
        let mute = xml_tag(&mute_body, mute_tag).as_deref() == Some("1");
        Ok((volume, mute))
    }

    /// Full state for `player`: volume/mute are **per-player** (each speaker has
    /// its own), but transport + now-playing are a **group** property read from
    /// `coordinator` — a grouped follower mirrors its coordinator, and its own
    /// `AVTransport` reports a slaved/stopped state with no track. `coordinator`
    /// is the player itself when it's standalone.
    async fn read_state(&self, player: &Player, coordinator: &Player) -> Result<MediaState> {
        let (volume, mute) = self.read_volume_mute(player, false).await?;

        let transport_body = self
            .soap(
                &coordinator.base_url,
                SoapCall {
                    path: "/MediaRenderer/AVTransport/Control",
                    service: AV_TRANSPORT,
                    action: "GetTransportInfo",
                    args: "<InstanceID>0</InstanceID>".into(),
                },
            )
            .await?;
        let play_state = match xml_tag(&transport_body, "CurrentTransportState").as_deref() {
            Some("PLAYING") | Some("TRANSITIONING") => Some(PlayState::Playing),
            Some("PAUSED_PLAYBACK") => Some(PlayState::Paused),
            Some("STOPPED") => Some(PlayState::Stopped),
            _ => None,
        };

        let position_body = self
            .soap(
                &coordinator.base_url,
                SoapCall {
                    path: "/MediaRenderer/AVTransport/Control",
                    service: AV_TRANSPORT,
                    action: "GetPositionInfo",
                    args: "<InstanceID>0</InstanceID>".into(),
                },
            )
            .await
            .unwrap_or_default();
        let didl = xml_unescape(&xml_tag(&position_body, "TrackMetaData").unwrap_or_default());
        let title = xml_tag_text(&didl, "dc:title");
        let artist = xml_tag_text(&didl, "dc:creator");
        let album = xml_tag_text(&didl, "upnp:album");
        let artwork_url = absolutize_art(
            xml_tag_text(&didl, "upnp:albumArtURI"),
            &coordinator.base_url,
        );

        let now_playing = (title.is_some() || play_state.is_some()).then_some(NowPlaying {
            title,
            artist,
            album,
            play_state,
            artwork_url,
        });

        // Physical inputs belong to `player`; the *active* one is read from the
        // (group) transport URI on the coordinator.
        let inputs = self.inputs_for(&player.base_url).await;
        let source_list: Vec<String> = inputs.iter().map(|i| i.label().to_string()).collect();
        let track_uri = xml_tag(&position_body, "TrackURI").unwrap_or_default();
        let source = SonosInput::from_uri(&track_uri)
            .filter(|i| inputs.contains(i))
            .map(|i| i.label().to_string());

        Ok(MediaState {
            power: play_state == Some(PlayState::Playing),
            volume: volume.min(100),
            mute,
            source,
            source_list,
            now_playing,
            reachable: Some(true),
            group_coordinator: None, // set by discover from the topology
            // The player's LAN IP, pulled from its base URL (http://<ip>:1400).
            ip: player
                .base_url
                .trim_start_matches("http://")
                .split(['/', ':'])
                .next()
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        })
    }

    /// A player's physical inputs (line-in / TV), fetched from its
    /// `device_description.xml` and memoised by `base_url`. Hardware inputs never
    /// change, so caching keeps `read_state` (a hot poll path) from re-fetching
    /// the description each cycle. A failed fetch isn't cached, so it retries.
    async fn inputs_for(&self, base_url: &str) -> Vec<SonosInput> {
        static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Vec<SonosInput>>>> =
            std::sync::OnceLock::new();
        let cache = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
        let key = self.cache_key(base_url);
        if let Some(inputs) = cache.lock().expect("sonos input cache poisoned").get(&key) {
            return inputs.clone();
        }
        let body = match self
            .client
            .get(format!("{base_url}/xml/device_description.xml"))
            .send()
            .await
        {
            Ok(resp) => resp.text().await.unwrap_or_default(),
            Err(_) => return Vec::new(),
        };
        let inputs = parse_inputs(&body);
        cache
            .lock()
            .expect("sonos input cache poisoned")
            .insert(key, inputs.clone());
        inputs
    }

    async fn transport(&self, player: &Player, action: &str) -> Result<()> {
        let args = if action == "Play" {
            "<InstanceID>0</InstanceID><Speed>1</Speed>".to_string()
        } else {
            "<InstanceID>0</InstanceID>".to_string()
        };
        self.av(player, action, args).await.map(|_| ())
    }

    /// Send one AVTransport action to a player.
    async fn av(&self, player: &Player, action: &str, args: String) -> Result<String> {
        self.soap(
            &player.base_url,
            SoapCall {
                path: "/MediaRenderer/AVTransport/Control",
                service: AV_TRANSPORT,
                action,
                args,
            },
        )
        .await
    }

    /// Browse the household Favorites container (`FV:2`) and return the
    /// unescaped DIDL-Lite result. Favorites are household-wide, so any player
    /// answers the same list — prefer the last player that answered topology
    /// over the raw seed, so an offline seed doesn't break favorites either.
    async fn browse_favorites(&self) -> Result<String> {
        let url = topology_cache()
            .lock()
            .expect("sonos topology cache poisoned")
            .get(&self.cache_key(&self.seed_url))
            .and_then(|urls| urls.first().cloned())
            .unwrap_or_else(|| self.seed_url.clone());
        let body = self
            .soap(
                &url,
                SoapCall {
                    path: "/MediaServer/ContentDirectory/Control",
                    service: CONTENT_DIRECTORY,
                    action: "Browse",
                    args: "<ObjectID>FV:2</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag>\
                           <Filter>*</Filter><StartingIndex>0</StartingIndex>\
                           <RequestedCount>200</RequestedCount><SortCriteria></SortCriteria>"
                        .to_string(),
                },
            )
            .await?;
        Ok(xml_unescape(&xml_tag(&body, "Result").unwrap_or_default()))
    }

    /// Read every player's current full state with live grouping folded in
    /// (`group_coordinator`). The reliable baseline behind the push channel and,
    /// because it touches each player, a liveness probe that keeps idle speakers
    /// from looking dead. Returns `(player uuid, state)` pairs.
    async fn poll_states(&self) -> Result<Vec<(String, MediaState)>> {
        let (players, groups) = self.topology().await?;
        let by_uuid: HashMap<&str, &Player> =
            players.iter().map(|p| (p.uuid.as_str(), p)).collect();
        let mut coordinator_of: HashMap<&str, &str> = HashMap::new();
        for g in &groups {
            for member in &g.member_uuids {
                coordinator_of.insert(member.as_str(), g.coordinator_uuid.as_str());
            }
        }
        let mut out = Vec::with_capacity(players.len());
        for p in &players {
            // Transport state comes from the coordinator (== player when solo).
            let coordinator = coordinator_of
                .get(p.uuid.as_str())
                .and_then(|c| by_uuid.get(*c))
                .copied()
                .unwrap_or(p);
            let mut state = self.read_state(p, coordinator).await.unwrap_or(MediaState {
                reachable: Some(false),
                ..Default::default()
            });
            state.group_coordinator = coordinator_of.get(p.uuid.as_str()).map(|c| c.to_string());
            out.push((p.uuid.clone(), state));
        }
        Ok(out)
    }

    /// The push producer (spawned by `event_stream`): GENA event subscriptions
    /// for instant updates, with a heartbeat poll baseline so state stays honest
    /// even when no events flow (idle speakers, or a deployment where the GENA
    /// callback isn't LAN-reachable, e.g. Docker bridge). Runs until the channel
    /// closes; the `MediaPushManager` owns reconnection.
    async fn run_push(
        self,
        tx: tokio::sync::mpsc::Sender<MediaEvent>,
        initial: Vec<(String, MediaState)>,
    ) {
        // `cache` is the last-emitted state per player; both the poll loop and
        // the GENA callbacks merge into it and emit only real changes.
        let cache: SharedCache = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        for (uuid, state) in initial {
            if emit(&tx, &cache, &uuid, state).await.is_err() {
                return;
            }
        }

        // GENA is best-effort: if it can't be set up (can't bind, no LAN route,
        // device refuses SUBSCRIBE), we log and run poll-only — never fail.
        let mut gena = self.start_gena(&tx, &cache).await;
        if gena.is_none() {
            tracing::info!("sonos: GENA push unavailable; running heartbeat poll only");
        }

        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        ticker.tick().await; // consume the immediate first tick (already seeded)
        loop {
            ticker.tick().await;
            match self.poll_states().await {
                Ok(states) => {
                    for (uuid, state) in states {
                        if emit(&tx, &cache, &uuid, state).await.is_err() {
                            return; // channel closed → manager reconnects
                        }
                    }
                }
                Err(e) => tracing::debug!("sonos heartbeat poll failed: {e:#}"),
            }
            if let Some(g) = gena.as_mut() {
                g.maybe_renew(&self.client).await;
            }
        }
    }
}

#[async_trait]
impl MediaProvider for SonosProvider {
    fn name(&self) -> &str {
        "sonos"
    }

    async fn discover(&self) -> Result<Vec<MediaDevice>> {
        // Individual players are the only stored devices. A Sonos *group* is a
        // transient, provider-native grouping — not a thing we persist. Per
        // "derive from members", grouped playback is surfaced from the member
        // speakers (a Bifrost Room controls them together), never as a separate
        // inventory object. The topology's groups are still read for live
        // grouping state, just not turned into devices.
        let (players, groups) = self.topology().await?;
        let by_uuid: HashMap<&str, &Player> =
            players.iter().map(|p| (p.uuid.as_str(), p)).collect();
        // Each grouped player → its group coordinator, so a speaker's state can
        // advertise the live grouping the UI derives a combined control from.
        let mut coordinator_of: HashMap<&str, &str> = HashMap::new();
        for g in &groups {
            for member in &g.member_uuids {
                coordinator_of.insert(member.as_str(), g.coordinator_uuid.as_str());
            }
        }
        let mut devices = Vec::with_capacity(players.len());

        for p in &players {
            // A player that drops mid-discovery still gets listed (unreachable),
            // matching how light discovery tolerates flaky devices. Transport
            // state is read from the coordinator (== player when standalone).
            let coordinator = coordinator_of
                .get(p.uuid.as_str())
                .and_then(|c| by_uuid.get(*c))
                .copied()
                .unwrap_or(p);
            let mut state = self.read_state(p, coordinator).await.unwrap_or(MediaState {
                reachable: Some(false),
                ..Default::default()
            });
            state.group_coordinator = coordinator_of.get(p.uuid.as_str()).map(|c| c.to_string());
            devices.push(MediaDevice {
                id: Uuid::new_v4(),
                provider_id: p.uuid.clone(),
                name: p.name.clone(),
                kind: MediaDeviceKind::Speaker,
                capabilities: MediaCapabilities {
                    // Line-in / TV input, if this player has one (from read_state).
                    sources: !state.source_list.is_empty(),
                    transport: true,
                    now_playing: true,
                    favorites: true,
                    // Individual players can be grouped/ungrouped with others.
                    grouping: true,
                },
                state,
                hw_id: sonos_hw_id(&p.uuid),
            });
        }

        Ok(devices)
    }

    async fn get_state(&self, device_id: &str) -> Result<MediaState> {
        let (player, coordinator) = self.resolve(device_id).await?;
        self.read_state(&player, &coordinator).await
    }

    async fn set_state(&self, device_id: &str, cmd: &MediaCommand) -> Result<()> {
        let group = device_id.starts_with(GROUP_PREFIX);
        // Volume/mute act on the player; transport acts on the group coordinator.
        let (player, coordinator) = self.resolve(device_id).await?;

        // Switch to a physical input (line-in / TV): point the group transport
        // at the input URI, then play — the same SetAVTransportURI mechanism as
        // a favorite. The URI references the owning player even on a group.
        if let Some(source) = &cmd.source {
            // A physical input lives on one specific player, but the command may
            // arrive on any member of the sync group (the Control page collapses a
            // group onto its coordinator). Find the member that actually owns the
            // named input so the switch lands regardless of which id we were given;
            // the URI then references that owner even though it's set on the group
            // coordinator's transport.
            let members = self.group_members(&player.uuid).await;
            let mut owner_input = None;
            for member in &members {
                let inputs = self.inputs_for(&member.base_url).await;
                if let Some(input) = inputs
                    .iter()
                    .copied()
                    .find(|i| i.label().eq_ignore_ascii_case(source))
                {
                    owner_input = Some((member.clone(), input));
                    break;
                }
            }
            let (owner, input) = match owner_input {
                Some(found) => found,
                None => {
                    let mut available = Vec::new();
                    for member in &members {
                        for input in self.inputs_for(&member.base_url).await {
                            available.push(input.label());
                        }
                    }
                    return Err(anyhow!(
                        "Sonos source '{source}' is not available in this group (available: {})",
                        if available.is_empty() {
                            "none".to_string()
                        } else {
                            available.join(", ")
                        }
                    ));
                }
            };
            let uri = input.uri(&owner.uuid);
            tracing::debug!(
                target: "bifrost::sonos",
                device_id,
                source = %source,
                matched = input.label(),
                owner = %owner.uuid,
                %uri,
                coordinator = %coordinator.uuid,
                coordinator_url = %coordinator.base_url,
                "switching Sonos to physical input (SetAVTransportURI + Play)"
            );
            self.av(
                &coordinator,
                "SetAVTransportURI",
                format!(
                    "<InstanceID>0</InstanceID><CurrentURI>{uri}</CurrentURI>\
                     <CurrentURIMetaData></CurrentURIMetaData>"
                ),
            )
            .await?;
            self.transport(&coordinator, "Play").await?;
        }

        if let Some(volume) = cmd.volume {
            let (path, service, action, args) = if group {
                (
                    "/MediaRenderer/GroupRenderingControl/Control",
                    GROUP_RENDERING,
                    "SetGroupVolume",
                    format!(
                        "<InstanceID>0</InstanceID><DesiredVolume>{}</DesiredVolume>",
                        volume.min(100)
                    ),
                )
            } else {
                (
                    "/MediaRenderer/RenderingControl/Control",
                    RENDERING,
                    "SetVolume",
                    format!(
                        "<InstanceID>0</InstanceID><Channel>Master</Channel><DesiredVolume>{}</DesiredVolume>",
                        volume.min(100)
                    ),
                )
            };
            self.soap(
                &player.base_url,
                SoapCall {
                    path,
                    service,
                    action,
                    args,
                },
            )
            .await?;
        }

        if let Some(mute) = cmd.mute {
            let (path, service, action, args) = if group {
                (
                    "/MediaRenderer/GroupRenderingControl/Control",
                    GROUP_RENDERING,
                    "SetGroupMute",
                    format!(
                        "<InstanceID>0</InstanceID><DesiredMute>{}</DesiredMute>",
                        if mute { 1 } else { 0 }
                    ),
                )
            } else {
                (
                    "/MediaRenderer/RenderingControl/Control",
                    RENDERING,
                    "SetMute",
                    format!(
                        "<InstanceID>0</InstanceID><Channel>Master</Channel><DesiredMute>{}</DesiredMute>",
                        if mute { 1 } else { 0 }
                    ),
                )
            };
            self.soap(
                &player.base_url,
                SoapCall {
                    path,
                    service,
                    action,
                    args,
                },
            )
            .await?;
        }

        // power maps to play/pause; an explicit transport command wins.
        let transport_action = match (cmd.transport, cmd.power) {
            (Some(t), _) => Some(match t {
                TransportCmd::Play => "Play",
                TransportCmd::Pause => "Pause",
                TransportCmd::Stop => "Stop",
                TransportCmd::Next => "Next",
                TransportCmd::Previous => "Previous",
                // Sonos has no toggle action; resolve from current state.
                TransportCmd::Toggle => {
                    if self.read_state(&player, &coordinator).await?.power {
                        "Pause"
                    } else {
                        "Play"
                    }
                }
            }),
            (None, Some(true)) => Some("Play"),
            (None, Some(false)) => Some("Pause"),
            (None, None) => None,
        };
        if let Some(action) = transport_action {
            // Transport drives the whole group → send to the coordinator.
            self.transport(&coordinator, action).await?;
        }
        Ok(())
    }

    async fn list_favorites(&self, _device_id: &str) -> Result<Vec<MediaFavorite>> {
        Ok(parse_favorites(&self.browse_favorites().await?))
    }

    async fn play_favorite(&self, device_id: &str, favorite_id: &str) -> Result<()> {
        // Playing on a grouped follower must drive the whole group → coordinator.
        let (_, target) = self.resolve(device_id).await?;
        let didl = self.browse_favorites().await?;

        // Find this favorite's <item> block, then its resource URI + metadata.
        let item = didl
            .split("<item ")
            .skip(1)
            .find(|chunk| {
                let el = chunk.split('>').next().unwrap_or("");
                xml_attr(el, "id").as_deref() == Some(favorite_id)
            })
            .ok_or_else(|| anyhow!("unknown Sonos favorite '{favorite_id}'"))?;
        let uri = xml_el(item, "res")
            .filter(|u| !u.is_empty())
            .ok_or_else(|| anyhow!("Sonos favorite '{favorite_id}' has no playable resource"))?;
        // resMD stays singly-escaped (as Sonos returns it) — that's the exact
        // form the metadata argument expects, so it's passed through verbatim.
        let meta = xml_tag(item, "r:resMD").unwrap_or_default();

        if favorite_is_container(&uri) {
            // Replace the queue with the favorite, then play the queue.
            let _ = self
                .av(
                    &target,
                    "RemoveAllTracksFromQueue",
                    "<InstanceID>0</InstanceID>".into(),
                )
                .await; // best-effort: an empty queue is fine
            self.av(
                &target,
                "AddURIToQueue",
                format!(
                    "<InstanceID>0</InstanceID><EnqueuedURI>{uri}</EnqueuedURI>\
                     <EnqueuedURIMetaData>{meta}</EnqueuedURIMetaData>\
                     <DesiredFirstTrackNumberEnqueued>0</DesiredFirstTrackNumberEnqueued>\
                     <EnqueueAsNext>0</EnqueueAsNext>"
                ),
            )
            .await?;
            self.av(
                &target,
                "SetAVTransportURI",
                format!(
                    "<InstanceID>0</InstanceID>\
                     <CurrentURI>x-rincon-queue:{}#0</CurrentURI>\
                     <CurrentURIMetaData></CurrentURIMetaData>",
                    target.uuid
                ),
            )
            .await?;
        } else {
            // A single stream (radio, track) can be set as the transport URI.
            self.av(
                &target,
                "SetAVTransportURI",
                format!(
                    "<InstanceID>0</InstanceID><CurrentURI>{uri}</CurrentURI>\
                     <CurrentURIMetaData>{meta}</CurrentURIMetaData>"
                ),
            )
            .await?;
        }

        self.transport(&target, "Play").await
    }

    async fn group(&self, device_id: &str, coordinator_id: &str) -> Result<()> {
        if device_id == coordinator_id {
            return Err(anyhow!("a speaker cannot be grouped with itself"));
        }
        let member = self.find_target(device_id).await?;
        // A player joins a group by pointing its transport at the coordinator
        // via the x-rincon scheme; the coordinator then drives playback.
        let coord_uuid = coordinator_id
            .strip_prefix(GROUP_PREFIX)
            .unwrap_or(coordinator_id);
        self.av(
            &member,
            "SetAVTransportURI",
            format!(
                "<InstanceID>0</InstanceID>\
                 <CurrentURI>x-rincon:{coord_uuid}</CurrentURI>\
                 <CurrentURIMetaData></CurrentURIMetaData>"
            ),
        )
        .await
        .map(|_| ())
    }

    async fn ungroup(&self, device_id: &str) -> Result<()> {
        let player = self.find_target(device_id).await?;
        // Leaving a group = becoming the coordinator of your own standalone
        // group. Harmless (and idempotent) on an already-standalone player.
        self.av(
            &player,
            "BecomeCoordinatorOfStandaloneGroup",
            "<InstanceID>0</InstanceID>".into(),
        )
        .await
        .map(|_| ())
    }

    async fn discover_groups(&self) -> Result<Vec<crate::providers::ProviderGroup>> {
        use crate::providers::ProviderGroup;
        // Each Sonos player carries its room name (ZoneName); the room *is* the
        // player. Transient playback groups (the `group:` zone devices) are not
        // rooms, so they're excluded here.
        let (players, _groups) = self.topology().await?;
        let mut seen = std::collections::HashSet::new();
        Ok(players
            .into_iter()
            .filter(|p| seen.insert(p.uuid.clone()))
            .map(|p| ProviderGroup {
                provider_group_id: p.uuid.clone(),
                name: p.name,
                member_device_ids: vec![p.uuid],
                grouped_ref: None,
            })
            .collect())
    }

    async fn event_stream(&self) -> Result<tokio::sync::mpsc::Receiver<MediaEvent>> {
        // Up-front reachability check so the manager treats an unreachable Sonos
        // as a connect failure (→ backoff) rather than a silently-dead channel.
        let initial = self.poll_states().await?;
        let (tx, rx) = tokio::sync::mpsc::channel::<MediaEvent>(64);
        let provider = self.clone();
        tokio::spawn(async move { provider.run_push(tx, initial).await });
        Ok(rx)
    }
}

// ── GENA event subscriptions (UPnP push) ─────────────────────────────────────
//
// Sonos players push state changes to a callback URL we host (RenderingControl =
// volume/mute, AVTransport = transport/track, ZoneGroupTopology = regrouping).
// Subscriptions are renewed before their timeout. This is layered *on top of* the
// heartbeat poll, so it's strictly best-effort: when the callback isn't
// LAN-reachable (Docker bridge, firewall) the poll still keeps state honest.

/// Last-emitted state per player uuid, shared by the poll loop and the GENA
/// callback handlers (which merge partial events into it).
type SharedCache = Arc<tokio::sync::Mutex<HashMap<String, MediaState>>>;

/// Renew subscriptions well before their (1800s) timeout.
const RENEW_EVERY: Duration = Duration::from_secs(900);

/// Emit a device's full state iff it changed since the last emit (the writer and
/// broadcast treat events as full snapshots). `Err` = channel closed.
async fn emit(
    tx: &tokio::sync::mpsc::Sender<MediaEvent>,
    cache: &SharedCache,
    uuid: &str,
    state: MediaState,
) -> Result<(), ()> {
    {
        let mut c = cache.lock().await;
        let unchanged = c.get(uuid).is_some_and(|prev| {
            serde_json::to_string(prev).ok() == serde_json::to_string(&state).ok()
        });
        if unchanged {
            return Ok(());
        }
        c.insert(uuid.to_string(), state.clone());
    }
    tx.send(MediaEvent {
        device_id: uuid.to_string(),
        state,
    })
    .await
    .map_err(|_| ())
}

/// Shared state for the callback HTTP server.
#[derive(Clone)]
struct PushCtx {
    tx: tokio::sync::mpsc::Sender<MediaEvent>,
    cache: SharedCache,
}

impl PushCtx {
    /// Apply one NOTIFY body for `uuid`'s `svc` (`rc`/`av`/`zgt`) to the cache,
    /// emitting the merged full state for any device whose state changed.
    async fn apply(&self, uuid: &str, svc: &str, body: &str) {
        match svc {
            "rc" => {
                let Some(ev) = extract_property(body, "LastChange") else {
                    return;
                };
                let (vol, mute) = parse_rendering_lastchange(&ev);
                let next = {
                    let mut c = self.cache.lock().await;
                    let st = c.entry(uuid.to_string()).or_default();
                    if let Some(v) = vol {
                        st.volume = v;
                    }
                    if let Some(m) = mute {
                        st.mute = m;
                    }
                    st.reachable = Some(true);
                    st.clone()
                };
                let _ = emit(&self.tx, &self.cache, uuid, next).await;
            }
            "av" => {
                let Some(ev) = extract_property(body, "LastChange") else {
                    return;
                };
                let (play, mut now_playing) = parse_avtransport_lastchange(&ev);
                let next = {
                    let mut c = self.cache.lock().await;
                    let st = c.entry(uuid.to_string()).or_default();
                    if let Some(p) = play {
                        st.power = p == PlayState::Playing;
                    }
                    if let Some(mut np) = now_playing.take() {
                        // The event's albumArtURI is usually relative; resolve it
                        // against the player's cached IP (dropped if unknown —
                        // a relative URL would 404 against the Bifrost origin).
                        np.artwork_url = match (&st.ip, np.artwork_url.take()) {
                            (Some(ip), art) => absolutize_art(art, &format!("http://{ip}:1400")),
                            (None, art) => art.filter(|u| u.starts_with("http")),
                        };
                        st.now_playing = Some(np);
                    }
                    st.reachable = Some(true);
                    st.clone()
                };
                let _ = emit(&self.tx, &self.cache, uuid, next).await;
            }
            "zgt" => {
                let Some(zgs) = extract_property(body, "ZoneGroupState") else {
                    return;
                };
                let (_players, groups) = parse_topology(&zgs);
                let mut coordinator_of: HashMap<String, String> = HashMap::new();
                for g in &groups {
                    for m in &g.member_uuids {
                        coordinator_of.insert(m.clone(), g.coordinator_uuid.clone());
                    }
                }
                // Recompute each known player's coordinator; emit the changed ones.
                let updates: Vec<(String, MediaState)> = {
                    let mut c = self.cache.lock().await;
                    let mut out = Vec::new();
                    for (u, st) in c.iter_mut() {
                        let coord = coordinator_of.get(u).cloned();
                        if st.group_coordinator != coord {
                            st.group_coordinator = coord;
                            out.push((u.clone(), st.clone()));
                        }
                    }
                    out
                };
                for (u, state) in updates {
                    let _ = emit(&self.tx, &self.cache, &u, state).await;
                }
            }
            _ => {}
        }
    }
}

/// The callback endpoint. A fallback handler so it matches the custom `NOTIFY`
/// method regardless of axum's method routing. Path is `/n/{uuid}/{svc}`.
async fn notify_handler(
    axum::extract::State(ctx): axum::extract::State<PushCtx>,
    req: axum::extract::Request,
) -> axum::http::StatusCode {
    let path = req.uri().path().to_string();
    let body = match axum::body::to_bytes(req.into_body(), 64 * 1024).await {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(_) => return axum::http::StatusCode::OK,
    };
    let mut segs = path.trim_start_matches('/').split('/');
    if segs.next() == Some("n")
        && let (Some(uuid), Some(svc)) = (segs.next(), segs.next())
    {
        ctx.apply(uuid, svc, &body).await;
    }
    axum::http::StatusCode::OK // GENA wants a 200 ack
}

/// Active GENA subscriptions + the callback server task. Dropping it aborts the
/// server (so a reconnect doesn't leak listeners).
struct Gena {
    /// (full event-subscription URL, SID), for renewal.
    subs: Vec<(String, String)>,
    last_renew: Instant,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Gena {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Gena {
    async fn maybe_renew(&mut self, client: &Client) {
        if self.last_renew.elapsed() < RENEW_EVERY {
            return;
        }
        for (url, sid) in &self.subs {
            let _ = gena_renew(client, url, sid).await;
        }
        self.last_renew = Instant::now();
    }
}

/// The local source IP the OS would use to reach `host` — the callback host the
/// players POST back to. (UDP connect sends nothing; it just picks the route.)
fn local_ip_for(host: &str) -> Option<std::net::IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect((host, 1400)).ok()?;
    sock.local_addr().ok().map(|a| a.ip())
}

/// SUBSCRIBE to a service's event URL; returns the SID on success.
async fn gena_subscribe(client: &Client, event_url: &str, callback: &str) -> Option<String> {
    let method = reqwest::Method::from_bytes(b"SUBSCRIBE").ok()?;
    let resp = client
        .request(method, event_url)
        .header("CALLBACK", format!("<{callback}>"))
        .header("NT", "upnp:event")
        .header("TIMEOUT", "Second-1800")
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.headers()
        .get("SID")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// RENEW an existing subscription (SUBSCRIBE with the SID, no CALLBACK/NT).
async fn gena_renew(client: &Client, event_url: &str, sid: &str) {
    if let Ok(method) = reqwest::Method::from_bytes(b"SUBSCRIBE") {
        let _ = client
            .request(method, event_url)
            .header("SID", sid)
            .header("TIMEOUT", "Second-1800")
            .timeout(Duration::from_secs(5))
            .send()
            .await;
    }
}

impl SonosProvider {
    /// Best-effort GENA setup: detect the callback IP, bind a listener, serve the
    /// callback, and SUBSCRIBE each player's RenderingControl + AVTransport and
    /// the household ZoneGroupTopology. `None` if push can't be established (the
    /// caller then runs poll-only).
    async fn start_gena(
        &self,
        tx: &tokio::sync::mpsc::Sender<MediaEvent>,
        cache: &SharedCache,
    ) -> Option<Gena> {
        let host = self
            .seed_url
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .split(':')
            .next()?
            .to_string();
        let local_ip = local_ip_for(&host)?;
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.ok()?;
        let port = listener.local_addr().ok()?.port();
        let (players, _groups) = self.topology().await.ok()?;

        let ctx = PushCtx {
            tx: tx.clone(),
            cache: cache.clone(),
        };
        let app = axum::Router::new().fallback(notify_handler).with_state(ctx);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let base_cb = format!("http://{local_ip}:{port}/n");
        let mut subs = Vec::new();
        for p in &players {
            for (svc_path, tag) in [
                ("/MediaRenderer/RenderingControl/Event", "rc"),
                ("/MediaRenderer/AVTransport/Event", "av"),
            ] {
                let url = format!("{}{svc_path}", p.base_url);
                let cb = format!("{base_cb}/{}/{tag}", p.uuid);
                if let Some(sid) = gena_subscribe(&self.client, &url, &cb).await {
                    subs.push((url, sid));
                }
            }
        }
        // ZoneGroupTopology is household-wide; subscribe once on the seed.
        let zgt = format!("{}/ZoneGroupTopology/Event", self.seed_url);
        if let Some(sid) = gena_subscribe(&self.client, &zgt, &format!("{base_cb}/seed/zgt")).await
        {
            subs.push((zgt, sid));
        }

        if subs.is_empty() {
            server.abort(); // no device accepted a subscription → poll-only
            return None;
        }
        Some(Gena {
            subs,
            last_renew: Instant::now(),
            server,
        })
    }
}

// ── GENA NOTIFY parsers (pure) ───────────────────────────────────────────────

/// Pull an escaped-XML property (`LastChange`, `ZoneGroupState`) out of a GENA
/// `propertyset` NOTIFY body and unescape it to real XML.
fn extract_property(body: &str, name: &str) -> Option<String> {
    xml_tag(body, name).map(|v| xml_unescape(&v))
}

/// `val` attribute of the first self-closing `<tag … val="…"/>` in a LastChange
/// Event (e.g. `TransportState`, `CurrentTrackMetaData`).
fn lastchange_val(event: &str, tag: &str) -> Option<String> {
    for chunk in event.split(&format!("<{tag} ")).skip(1) {
        let el = chunk.split('>').next().unwrap_or("");
        if let Some(v) = xml_attr(el, "val") {
            return Some(v);
        }
    }
    None
}

/// `val` of the `<tag channel="…" val="…"/>` matching `channel` (RenderingControl
/// reports per-channel; we want `Master`).
fn lastchange_channel_val(event: &str, tag: &str, channel: &str) -> Option<String> {
    for chunk in event.split(&format!("<{tag} ")).skip(1) {
        let el = chunk.split('>').next().unwrap_or("");
        if xml_attr(el, "channel").as_deref() == Some(channel) {
            return xml_attr(el, "val");
        }
    }
    None
}

/// (volume 0–100, mute) from a RenderingControl LastChange Event.
fn parse_rendering_lastchange(event: &str) -> (Option<u8>, Option<bool>) {
    let volume = lastchange_channel_val(event, "Volume", "Master").and_then(|v| v.parse().ok());
    let mute = lastchange_channel_val(event, "Mute", "Master").map(|v| v == "1");
    (volume, mute)
}

/// (play state, now-playing) from an AVTransport LastChange Event.
fn parse_avtransport_lastchange(event: &str) -> (Option<PlayState>, Option<NowPlaying>) {
    let play = lastchange_val(event, "TransportState").and_then(|v| match v.as_str() {
        "PLAYING" | "TRANSITIONING" => Some(PlayState::Playing),
        "PAUSED_PLAYBACK" => Some(PlayState::Paused),
        "STOPPED" => Some(PlayState::Stopped),
        _ => None,
    });
    let didl = lastchange_val(event, "CurrentTrackMetaData")
        .map(|v| xml_unescape(&v))
        .unwrap_or_default();
    let title = xml_tag_text(&didl, "dc:title");
    let artist = xml_tag_text(&didl, "dc:creator");
    let album = xml_tag_text(&didl, "upnp:album");
    // Left as reported (usually a relative `/getaa?…` path) — the GENA apply
    // path absolutizes it against the player's cached IP.
    let artwork_url = xml_tag_text(&didl, "upnp:albumArtURI");
    let now_playing = (title.is_some() || play.is_some()).then_some(NowPlaying {
        title,
        artist,
        album,
        play_state: play,
        artwork_url,
    });
    (play, now_playing)
}

/// Absolutize a DIDL `albumArtURI` against a player's base URL
/// (`http://<ip>:1400`) — Sonos usually reports a relative `/getaa?…` path,
/// but streaming services sometimes hand back a full URL.
fn absolutize_art(uri: Option<String>, base_url: &str) -> Option<String> {
    let uri = uri.filter(|u| !u.is_empty())?;
    if uri.starts_with("http://") || uri.starts_with("https://") {
        return Some(uri);
    }
    let sep = if uri.starts_with('/') { "" } else { "/" };
    Some(format!("{base_url}{sep}{uri}"))
}

// ── Factory ─────────────────────────────────────────────────────────────────

pub struct SonosProviderFactory;

/// The seed player is reached at a stored IP, so a DHCP change strands the
/// household. Identity is the player UUID in its `device_description.xml`
/// (`RINCON_<mac>` → the same `hw_id` discovery records).
///
/// A candidate matching **any** of the household's known players is accepted,
/// not just the original seed: a Sonos provider row is an entry point, not a
/// device — topology fans out from whichever player answers — so re-seeding on a
/// sibling is exactly as correct and heals a household whose original seed was
/// unplugged rather than re-addressed.
struct SonosLanBinding;

/// UPnP port every Sonos player serves.
const SONOS_PORT: u16 = 1400;

#[async_trait]
impl LanBinding for SonosLanBinding {
    fn host_field(&self) -> &'static str {
        "host"
    }

    fn probe_port(&self, _creds: &Credentials) -> u16 {
        SONOS_PORT
    }

    fn can_verify(&self, _creds: &Credentials, known_hw: &[String]) -> bool {
        known_hw.iter().any(|h| is_portable_hw_id(h))
    }

    async fn is_same_device(&self, host: &str, _creds: &Credentials, known_hw: &[String]) -> bool {
        if known_hw.is_empty() {
            return false; // nothing to compare against
        }
        let base = crate::providers::base_url(host, "http", Some(SONOS_PORT));
        // The same pooled client the provider itself uses — a per-call
        // `Client::builder()` would open a fresh connection pool for every
        // candidate and drift from the provider's own timeouts.
        let Ok(client) = crate::providers::cached_client("sonos", build_sonos_client) else {
            return false;
        };
        let Ok(resp) = client
            .get(format!("{base}/xml/device_description.xml"))
            .send()
            .await
        else {
            return false;
        };
        let Ok(body) = resp.text().await else {
            return false;
        };
        // <UDN>uuid:RINCON_<mac>01400</UDN>
        xml_tag(&body, "UDN")
            .and_then(|udn| sonos_hw_id(udn.trim().trim_start_matches("uuid:")))
            .is_some_and(|hw| known_hw.contains(&hw))
    }
}

impl MediaProviderFactory for SonosProviderFactory {
    fn provider_type(&self) -> &'static str {
        "sonos"
    }

    fn display_name(&self) -> &'static str {
        "Sonos"
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn MediaProvider>> {
        Ok(Box::new(SonosProvider::from_credentials(credentials_json)?))
    }

    fn credentials_schema(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "host",
            label: "Any Sonos player's IP address",
            kind: FieldKind::IpAddress,
            required: true,
            hint: Some("One player is enough — the rest of the household is discovered from it"),
        }]
    }

    fn discoverer(&self) -> Option<Box<dyn DeviceDiscovery>> {
        // Any LOCATION-bearing ZonePlayer reply is enough; the household is
        // discovered from that one player at runtime.
        Some(Box::new(SsdpDiscovery::new(
            "urn:schemas-upnp-org:device:ZonePlayer:1",
            "",
            "Sonos",
            "host",
        )))
    }

    fn lan_binding(&self) -> Option<Box<dyn LanBinding>> {
        Some(Box::new(SonosLanBinding))
    }

    fn connection_mode(&self) -> MediaConnectionMode {
        // Push: GENA event subscriptions for instant updates, with a heartbeat
        // poll baseline (see `event_stream`).
        MediaConnectionMode::Push
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn lan_binding_proves_identity_by_the_players_udn() {
        use crate::providers::LanBinding as _;
        let b = SonosLanBinding;
        assert_eq!(b.host_field(), "host");
        assert_eq!(b.probe_port(&serde_json::Map::new()), SONOS_PORT);

        let player = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/xml/device_description.xml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "<root><device><UDN>uuid:RINCON_949F3E12345601400</UDN></device></root>",
            ))
            .mount(&player)
            .await;
        let host = player.uri(); // full http://127.0.0.1:PORT — kept verbatim
        let creds = serde_json::Map::new();

        assert!(
            b.is_same_device(&host, &creds, &["mac:949f3e123456".to_string()])
                .await
        );
        // Any known household player counts — the row is an entry point, and
        // topology fans out from whichever player answers.
        assert!(
            b.is_same_device(
                &host,
                &creds,
                &[
                    "mac:aaaaaaaaaaaa".to_string(),
                    "mac:949f3e123456".to_string()
                ]
            )
            .await
        );
        // A stranger's player is refused, as is an empty identity set.
        assert!(
            !b.is_same_device(&host, &creds, &["mac:112233445566".to_string()])
                .await
        );
        assert!(!b.is_same_device(&host, &creds, &[]).await);
    }

    #[test]
    fn sonos_hw_id_extracts_mac_from_rincon_uuid() {
        // RINCON_<mac><suffix> → the 12-hex MAC after the prefix.
        assert_eq!(
            sonos_hw_id("RINCON_949F3E1234560140 0".trim()),
            Some("mac:949f3e123456".to_string())
        );
        assert_eq!(
            sonos_hw_id("RINCON_949F3E12345601400"),
            Some("mac:949f3e123456".to_string())
        );
        // Not a RINCON uuid → no hardware id.
        assert_eq!(sonos_hw_id("group:RINCON_x"), None);
        assert_eq!(sonos_hw_id("something-else"), None);
    }

    // ── XML helpers ──────────────────────────────────────────────────────────

    #[test]
    fn xml_tag_extracts_first_occurrence() {
        let body = "<a><CurrentVolume>42</CurrentVolume><CurrentVolume>9</CurrentVolume></a>";
        assert_eq!(xml_tag(body, "CurrentVolume").as_deref(), Some("42"));
        assert_eq!(xml_tag(body, "Missing"), None);
    }

    #[test]
    fn xml_attr_reads_quoted_attribute_values() {
        let el = r#"ZoneGroupMember UUID="RINCON_A" ZoneName="Kitchen" Invisible="1""#;
        assert_eq!(xml_attr(el, "UUID").as_deref(), Some("RINCON_A"));
        assert_eq!(xml_attr(el, "ZoneName").as_deref(), Some("Kitchen"));
        assert_eq!(xml_attr(el, "Nope"), None);
    }

    #[test]
    fn xml_unescape_decodes_nested_xml_entities() {
        assert_eq!(
            xml_unescape("&lt;item&gt;Q &amp; A&lt;/item&gt;"),
            "<item>Q & A</item>"
        );
    }

    // ── Mock helpers ─────────────────────────────────────────────────────────

    fn soap_ok(action: &str, service_short: &str, inner: &str) -> String {
        format!(
            r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><u:{action}Response xmlns:u="urn:schemas-upnp-org:service:{service_short}:1">{inner}</u:{action}Response></s:Body></s:Envelope>"#
        )
    }

    /// Mount per-player GetVolume/GetMute (RenderingControl) on a server.
    async fn mount_volume(server: &MockServer, volume: u8) {
        Mock::given(method("POST"))
            .and(path("/MediaRenderer/RenderingControl/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{RENDERING}#GetVolume\"").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "GetVolume",
                "RenderingControl",
                &format!("<CurrentVolume>{volume}</CurrentVolume>"),
            )))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/MediaRenderer/RenderingControl/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{RENDERING}#GetMute\"").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "GetMute",
                "RenderingControl",
                "<CurrentMute>0</CurrentMute>",
            )))
            .mount(server)
            .await;
    }

    /// Topology XML (escaped, as Sonos returns it): Living Room + Kitchen play
    /// as one group (coordinator: Living Room); Den stands alone; a bridge is
    /// invisible. Every Location points back at the mock server.
    fn topology_response(base: &str) -> String {
        let raw = format!(
            r#"<ZoneGroups><ZoneGroup Coordinator="RINCON_LIVING" ID="RINCON_LIVING:1"><ZoneGroupMember UUID="RINCON_LIVING" Location="{base}/xml/device_description.xml" ZoneName="Living Room"/><ZoneGroupMember UUID="RINCON_KITCHEN" Location="{base}/xml/device_description.xml" ZoneName="Kitchen"/><ZoneGroupMember UUID="RINCON_BRIDGE" Location="{base}/xml/device_description.xml" ZoneName="BRIDGE" Invisible="1"/></ZoneGroup><ZoneGroup Coordinator="RINCON_DEN" ID="RINCON_DEN:1"><ZoneGroupMember UUID="RINCON_DEN" Location="{base}/xml/device_description.xml" ZoneName="Den"/></ZoneGroup></ZoneGroups>"#
        );
        let escaped = raw
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;");
        soap_ok(
            "GetZoneGroupState",
            "ZoneGroupTopology",
            &format!("<ZoneGroupState>{escaped}</ZoneGroupState>"),
        )
    }

    async fn mount_group_state(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/MediaRenderer/GroupRenderingControl/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{GROUP_RENDERING}#GetGroupVolume\"").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "GetGroupVolume",
                "GroupRenderingControl",
                "<CurrentVolume>22</CurrentVolume>",
            )))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/MediaRenderer/GroupRenderingControl/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{GROUP_RENDERING}#GetGroupMute\"").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "GetGroupMute",
                "GroupRenderingControl",
                "<CurrentMute>0</CurrentMute>",
            )))
            .mount(server)
            .await;
    }

    async fn mount_topology(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/ZoneGroupTopology/Control"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(topology_response(&server.uri())),
            )
            .mount(server)
            .await;
    }

    async fn mount_playing_state(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/MediaRenderer/RenderingControl/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{RENDERING}#GetVolume\"").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "GetVolume",
                "RenderingControl",
                "<CurrentVolume>35</CurrentVolume>",
            )))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/MediaRenderer/RenderingControl/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{RENDERING}#GetMute\"").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "GetMute",
                "RenderingControl",
                "<CurrentMute>0</CurrentMute>",
            )))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/MediaRenderer/AVTransport/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{AV_TRANSPORT}#GetTransportInfo\"").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "GetTransportInfo",
                "AVTransport",
                "<CurrentTransportState>PLAYING</CurrentTransportState>",
            )))
            .mount(server)
            .await;
        let didl = r#"<DIDL-Lite><item><dc:title>Karma Police</dc:title><dc:creator>Radiohead</dc:creator><upnp:album>OK Computer</upnp:album></item></DIDL-Lite>"#
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        Mock::given(method("POST"))
            .and(path("/MediaRenderer/AVTransport/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{AV_TRANSPORT}#GetPositionInfo\"").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "GetPositionInfo",
                "AVTransport",
                &format!("<TrackMetaData>{didl}</TrackMetaData>"),
            )))
            .mount(server)
            .await;
    }

    // ── Behaviour ────────────────────────────────────────────────────────────

    #[test]
    fn parse_topology_finds_players_and_multi_member_groups() {
        let xml = r#"<ZoneGroups><ZoneGroup Coordinator="A" ID="A:1"><ZoneGroupMember UUID="A" Location="http://h1:1400/xml/d.xml" ZoneName="One"/><ZoneGroupMember UUID="B" Location="http://h2:1400/xml/d.xml" ZoneName="Two"/></ZoneGroup><ZoneGroup Coordinator="C" ID="C:1"><ZoneGroupMember UUID="C" Location="http://h3:1400/xml/d.xml" ZoneName="Solo"/></ZoneGroup></ZoneGroups>"#;
        let (players, groups) = parse_topology(xml);
        assert_eq!(players.len(), 3);
        assert_eq!(players[0].base_url, "http://h1:1400");
        assert_eq!(groups.len(), 1, "single-member groups are not zones");
        assert_eq!(groups[0].coordinator_uuid, "A");
        assert_eq!(groups[0].member_uuids, vec!["A", "B"]);
    }

    #[tokio::test]
    async fn discover_lists_visible_players_and_skips_invisible() {
        let server = MockServer::start().await;
        mount_topology(&server).await;
        mount_playing_state(&server).await;
        mount_group_state(&server).await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        let devices = p.discover().await.unwrap();

        let names: Vec<_> = devices.iter().map(|d| d.name.as_str()).collect();
        // Only the visible players — the group ("Living Room + Kitchen") is not a device.
        assert_eq!(names, vec!["Living Room", "Kitchen", "Den"]);
        assert_eq!(devices[0].provider_id, "RINCON_LIVING");
        assert!(
            devices.iter().all(|d| d.kind == MediaDeviceKind::Speaker),
            "players are speakers"
        );
        assert!(
            devices.iter().all(|d| d.capabilities.favorites),
            "every Sonos device advertises favorites"
        );
        assert!(
            devices.iter().all(|d| d.capabilities.grouping),
            "individual players can be grouped"
        );
    }

    #[tokio::test]
    async fn discover_surfaces_players_only_not_the_group() {
        // A Sonos group is a transient provider-native grouping; per "derive
        // from members" it is never stored as its own device — only the
        // individual players are. (Group playback is still controllable via the
        // coordinator and the existing group/ungroup calls.)
        let server = MockServer::start().await;
        mount_topology(&server).await;
        mount_playing_state(&server).await;
        mount_group_state(&server).await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        let devices = p.discover().await.unwrap();

        assert!(
            devices.iter().all(|d| d.kind == MediaDeviceKind::Speaker),
            "only individual speakers are surfaced, never a group device"
        );
        assert!(
            devices
                .iter()
                .all(|d| !d.provider_id.starts_with(GROUP_PREFIX)),
            "no group:* device is stored"
        );

        // Grouping is derived from the members instead: the two grouped players
        // advertise their coordinator; the standalone one is solo.
        let coord = |pid: &str| {
            devices
                .iter()
                .find(|d| d.provider_id == pid)
                .and_then(|d| d.state.group_coordinator.clone())
        };
        assert_eq!(coord("RINCON_LIVING").as_deref(), Some("RINCON_LIVING"));
        assert_eq!(coord("RINCON_KITCHEN").as_deref(), Some("RINCON_LIVING"));
        assert_eq!(coord("RINCON_DEN"), None, "Den is not grouped");
    }

    #[tokio::test]
    async fn group_joins_member_to_coordinator_via_x_rincon() {
        use wiremock::matchers::body_string_contains;
        let server = MockServer::start().await;
        mount_topology(&server).await;
        Mock::given(method("POST"))
            .and(path("/MediaRenderer/AVTransport/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{AV_TRANSPORT}#SetAVTransportURI\"").as_str(),
            ))
            .and(body_string_contains("x-rincon:RINCON_LIVING"))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "SetAVTransportURI",
                "AVTransport",
                "",
            )))
            .expect(1)
            .mount(&server)
            .await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        // Kitchen joins the group coordinated by Living Room.
        p.group("RINCON_KITCHEN", "RINCON_LIVING").await.unwrap();
    }

    #[tokio::test]
    async fn group_rejects_grouping_a_player_with_itself() {
        let server = MockServer::start().await;
        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        let err = p.group("RINCON_LIVING", "RINCON_LIVING").await.unwrap_err();
        assert!(err.to_string().contains("itself"), "{err}");
    }

    #[tokio::test]
    async fn ungroup_makes_player_a_standalone_coordinator() {
        let server = MockServer::start().await;
        mount_topology(&server).await;
        mount_av_action(&server, "BecomeCoordinatorOfStandaloneGroup", Some(1)).await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        p.ungroup("RINCON_KITCHEN").await.unwrap();
    }

    #[tokio::test]
    async fn set_group_volume_uses_group_rendering_control() {
        let server = MockServer::start().await;
        mount_topology(&server).await;

        Mock::given(method("POST"))
            .and(path("/MediaRenderer/GroupRenderingControl/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{GROUP_RENDERING}#SetGroupVolume\"").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "SetGroupVolume",
                "GroupRenderingControl",
                "",
            )))
            .expect(1)
            .mount(&server)
            .await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        p.set_state(
            "group:RINCON_LIVING",
            &MediaCommand {
                volume: Some(40),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn get_state_reads_volume_mute_play_state_and_track() {
        let server = MockServer::start().await;
        mount_topology(&server).await;
        mount_playing_state(&server).await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        let s = p.get_state("RINCON_KITCHEN").await.unwrap();

        assert!(s.power, "PLAYING maps to power on");
        assert_eq!(s.volume, 35);
        assert!(!s.mute);
        let np = s.now_playing.expect("track metadata");
        assert_eq!(np.title.as_deref(), Some("Karma Police"));
        assert_eq!(np.artist.as_deref(), Some("Radiohead"));
        assert_eq!(np.album.as_deref(), Some("OK Computer"));
        assert_eq!(np.play_state, Some(PlayState::Playing));
    }

    #[tokio::test]
    async fn get_state_for_unknown_player_errors() {
        let server = MockServer::start().await;
        mount_topology(&server).await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        let err = p.get_state("RINCON_GARAGE").await.unwrap_err();
        assert!(err.to_string().contains("RINCON_GARAGE"));
    }

    #[tokio::test]
    async fn set_state_sends_volume_and_mute_soap_actions() {
        let server = MockServer::start().await;
        mount_topology(&server).await;

        Mock::given(method("POST"))
            .and(path("/MediaRenderer/RenderingControl/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{RENDERING}#SetVolume\"").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "SetVolume",
                "RenderingControl",
                "",
            )))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/MediaRenderer/RenderingControl/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{RENDERING}#SetMute\"").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "SetMute",
                "RenderingControl",
                "",
            )))
            .expect(1)
            .mount(&server)
            .await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        p.set_state(
            "RINCON_LIVING",
            &MediaCommand {
                volume: Some(20),
                mute: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        // .expect(1) on each mock verifies on drop.
    }

    #[tokio::test]
    async fn set_state_power_off_sends_pause() {
        let server = MockServer::start().await;
        mount_topology(&server).await;

        Mock::given(method("POST"))
            .and(path("/MediaRenderer/AVTransport/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{AV_TRANSPORT}#Pause\"").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "Pause",
                "AVTransport",
                "",
            )))
            .expect(1)
            .mount(&server)
            .await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        p.set_state(
            "RINCON_LIVING",
            &MediaCommand {
                power: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn set_state_transport_next_sends_next_action() {
        let server = MockServer::start().await;
        mount_topology(&server).await;

        Mock::given(method("POST"))
            .and(path("/MediaRenderer/AVTransport/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{AV_TRANSPORT}#Next\"").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "Next",
                "AVTransport",
                "",
            )))
            .expect(1)
            .mount(&server)
            .await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        p.set_state(
            "RINCON_KITCHEN",
            &MediaCommand {
                transport: Some(TransportCmd::Next),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }

    /// A device description advertising a line-in jack (AudioIn service).
    fn device_description(model: &str, with_audio_in: bool) -> String {
        let audio_in = if with_audio_in {
            r#"<service><serviceType>urn:schemas-upnp-org:service:AudioIn:1</serviceType><controlURL>/MediaRenderer/AudioIn/Control</controlURL></service>"#
        } else {
            ""
        };
        format!(
            r#"<root><device><modelName>{model}</modelName><serviceList>{audio_in}</serviceList></device></root>"#
        )
    }

    async fn mount_device_description(server: &MockServer, body: String) {
        Mock::given(method("GET"))
            .and(path("/xml/device_description.xml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    }

    #[test]
    fn parse_inputs_detects_line_in_from_audio_in_service() {
        assert_eq!(
            parse_inputs(&device_description("Sonos Five", true)),
            vec![SonosInput::LineIn]
        );
    }

    #[test]
    fn parse_inputs_detects_tv_from_soundbar_model() {
        assert_eq!(
            parse_inputs(&device_description("Sonos Beam", false)),
            vec![SonosInput::Tv]
        );
    }

    #[test]
    fn parse_inputs_none_for_plain_speaker() {
        assert!(parse_inputs(&device_description("Sonos One", false)).is_empty());
    }

    #[test]
    fn sonos_input_uris_reference_the_owning_player() {
        assert_eq!(
            SonosInput::LineIn.uri("RINCON_LIVING"),
            "x-rincon-stream:RINCON_LIVING"
        );
        assert_eq!(
            SonosInput::Tv.uri("RINCON_LIVING"),
            "x-sonos-htastream:RINCON_LIVING:spdif"
        );
        assert_eq!(
            SonosInput::from_uri("x-rincon-stream:RINCON_X"),
            Some(SonosInput::LineIn)
        );
        assert_eq!(SonosInput::from_uri("x-rincon:RINCON_X"), None);
    }

    #[tokio::test]
    async fn set_state_switches_to_line_in() {
        let server = MockServer::start().await;
        mount_topology(&server).await;
        mount_device_description(&server, device_description("Sonos Five", true)).await;
        mount_av_action(&server, "SetAVTransportURI", Some(1)).await;
        mount_av_action(&server, "Play", Some(1)).await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        p.set_state(
            "RINCON_LIVING",
            &MediaCommand {
                source: Some("line-in".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        let set_uri = requests
            .iter()
            .find(|r| {
                r.headers
                    .get("SOAPACTION")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.contains("SetAVTransportURI"))
            })
            .expect("SetAVTransportURI was sent");
        let body = std::str::from_utf8(&set_uri.body).unwrap();
        assert!(
            body.contains("x-rincon-stream:RINCON_LIVING"),
            "expected line-in URI in SetAVTransportURI body, got: {body}"
        );
    }

    /// The Control page collapses a sync group onto its coordinator, so a
    /// `source` command can arrive on the coordinator even though the line-in jack
    /// physically lives on a *different* group member. The switch must still land:
    /// find the member that owns the input, reference it in the URI, and set it on
    /// the coordinator's transport.
    #[tokio::test]
    async fn set_state_line_in_resolves_input_owner_across_group() {
        let coord = MockServer::start().await; // RINCON_LIVING — no line-in jack
        let owner = MockServer::start().await; // RINCON_KITCHEN — owns the jack

        // A two-member group coordinated by LIVING; members point at the two
        // distinct base URLs so their device descriptions differ.
        let raw = format!(
            r#"<ZoneGroups><ZoneGroup Coordinator="RINCON_LIVING" ID="RINCON_LIVING:1"><ZoneGroupMember UUID="RINCON_LIVING" Location="{coord}/xml/device_description.xml" ZoneName="Living Room"/><ZoneGroupMember UUID="RINCON_KITCHEN" Location="{owner}/xml/device_description.xml" ZoneName="Kitchen"/></ZoneGroup></ZoneGroups>"#,
            coord = coord.uri(),
            owner = owner.uri(),
        );
        let escaped = raw
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;");
        let topology = soap_ok(
            "GetZoneGroupState",
            "ZoneGroupTopology",
            &format!("<ZoneGroupState>{escaped}</ZoneGroupState>"),
        );
        Mock::given(method("POST"))
            .and(path("/ZoneGroupTopology/Control"))
            .respond_with(ResponseTemplate::new(200).set_body_string(topology))
            .mount(&coord)
            .await;

        // Coordinator advertises no input; the member owns the line-in jack.
        mount_device_description(&coord, device_description("Sonos One", false)).await;
        mount_device_description(&owner, device_description("Sonos Five", true)).await;
        // Transport commands land on the coordinator.
        mount_av_action(&coord, "SetAVTransportURI", Some(1)).await;
        mount_av_action(&coord, "Play", Some(1)).await;

        let p = SonosProvider::new_for_test(coord.uri()).unwrap();
        // Target the coordinator — what the collapsed Control-page entry sends.
        p.set_state(
            "RINCON_LIVING",
            &MediaCommand {
                source: Some("line-in".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let requests = coord.received_requests().await.unwrap();
        let set_uri = requests
            .iter()
            .find(|r| {
                r.headers
                    .get("SOAPACTION")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.contains("SetAVTransportURI"))
            })
            .expect("SetAVTransportURI sent to the coordinator");
        let body = std::str::from_utf8(&set_uri.body).unwrap();
        assert!(
            body.contains("x-rincon-stream:RINCON_KITCHEN"),
            "expected the URI to reference the input-owning member, got: {body}"
        );
    }

    #[tokio::test]
    async fn set_state_rejects_unknown_source() {
        let server = MockServer::start().await;
        mount_topology(&server).await;
        mount_device_description(&server, device_description("Sonos One", false)).await;
        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        let err = p
            .set_state(
                "RINCON_LIVING",
                &MediaCommand {
                    source: Some("line-in".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not available"), "{err}");
    }

    #[tokio::test]
    async fn topology_with_no_players_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/ZoneGroupTopology/Control"))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "GetZoneGroupState",
                "ZoneGroupTopology",
                "<ZoneGroupState></ZoneGroupState>",
            )))
            .mount(&server)
            .await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        assert!(p.discover().await.is_err());
    }

    #[tokio::test]
    async fn topology_falls_back_to_another_player_when_seed_is_down() {
        // A live player that answers topology, and a seed pointed at an
        // unresolvable host — mimicking the configured seed going dark. The
        // `.invalid` TLD never resolves (RFC 6761), so the seed fetch fails fast.
        let live = MockServer::start().await;
        mount_topology(&live).await;
        let dead_seed = "http://sonos-dead-seed.invalid:1400".to_string();
        let p = SonosProvider::new_for_test(&dead_seed).unwrap();

        // Cache mirrors a household first seen via the (now-dead) seed: seed
        // first, the live player second — exactly the state after the seed dies.
        let key = p.cache_key(&dead_seed);
        topology_cache()
            .lock()
            .unwrap()
            .insert(key.clone(), vec![dead_seed.clone(), live.uri()]);

        let (players, _) = p.topology().await.unwrap();
        assert!(
            players.iter().any(|pl| pl.uuid == "RINCON_LIVING"),
            "topology should have come from the live fallback player"
        );

        // The live player is now remembered first, so the dead seed is skipped.
        let cached = topology_cache().lock().unwrap().get(&key).cloned();
        assert_eq!(
            cached.unwrap().first().map(String::as_str),
            Some(live.uri().as_str())
        );
    }

    // ── Favorites ────────────────────────────────────────────────────────────

    /// Favorites DIDL with one container (Spotify playlist) and one stream
    /// (radio), wrapped in a Browse SOAP response exactly as Sonos returns it:
    /// the DIDL is escaped once inside `<Result>`, and each item's `<r:resMD>`
    /// is itself escaped (so it survives one unescape singly-escaped).
    fn favorites_result() -> String {
        let raw_didl = concat!(
            r#"<DIDL-Lite xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:r="urn:schemas-rinconnetworks-com:metadata-1-0/">"#,
            r#"<item id="FV:2/12" parentID="FV:2" restricted="true">"#,
            r#"<dc:title>Jazz</dc:title><r:description>Spotify</r:description>"#,
            r#"<res protocolInfo="x-rincon-cpcontainer:*:*:*">x-rincon-cpcontainer:1006206cspotify%3aplaylist</res>"#,
            r#"<r:resMD>&lt;DIDL-Lite&gt;&lt;item id=&quot;1&quot;&gt;&lt;dc:title&gt;Jazz&lt;/dc:title&gt;&lt;/item&gt;&lt;/DIDL-Lite&gt;</r:resMD>"#,
            r#"</item>"#,
            r#"<item id="FV:2/3" parentID="FV:2" restricted="true">"#,
            r#"<dc:title>BBC Radio 6</dc:title><r:description>TuneIn</r:description>"#,
            r#"<res protocolInfo="x-sonosapi-stream:*:*:*">x-sonosapi-stream:s12345?sid=254</res>"#,
            r#"<r:resMD>&lt;DIDL-Lite&gt;radio&lt;/DIDL-Lite&gt;</r:resMD>"#,
            r#"</item></DIDL-Lite>"#,
        );
        let escaped = raw_didl
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;");
        soap_ok(
            "Browse",
            "ContentDirectory",
            &format!("<Result>{escaped}</Result>"),
        )
    }

    async fn mount_favorites(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/MediaServer/ContentDirectory/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{CONTENT_DIRECTORY}#Browse\"").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(favorites_result()))
            .mount(server)
            .await;
    }

    async fn mount_av_action(server: &MockServer, action: &str, expect: Option<u64>) {
        let m = Mock::given(method("POST"))
            .and(path("/MediaRenderer/AVTransport/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{AV_TRANSPORT}#{action}\"").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                action,
                "AVTransport",
                "",
            )));
        match expect {
            Some(n) => m.expect(n).mount(server).await,
            None => m.mount(server).await,
        }
    }

    #[test]
    fn parse_favorites_extracts_id_title_and_subtitle() {
        let didl = r#"<DIDL-Lite><item id="FV:2/12"><dc:title>Jazz</dc:title><r:description>Spotify</r:description></item><item id="FV:2/3"><dc:title>BBC Radio 6</dc:title></item></DIDL-Lite>"#;
        let favs = parse_favorites(didl);
        assert_eq!(favs.len(), 2);
        assert_eq!(favs[0].id, "FV:2/12");
        assert_eq!(favs[0].title, "Jazz");
        assert_eq!(favs[0].subtitle.as_deref(), Some("Spotify"));
        assert_eq!(favs[1].title, "BBC Radio 6");
        assert_eq!(favs[1].subtitle, None);
    }

    #[test]
    fn favorite_is_container_detects_playlists_vs_streams() {
        assert!(favorite_is_container(
            "x-rincon-cpcontainer:1006206cspotify"
        ));
        assert!(favorite_is_container("x-rinconplaylist:RINCON_x#0"));
        assert!(favorite_is_container("file:///jffs/settings/savedqueues"));
        assert!(!favorite_is_container("x-sonosapi-stream:s12345?sid=254"));
        assert!(!favorite_is_container("x-sonosapi-radio:ST%3a..."));
    }

    #[tokio::test]
    async fn list_favorites_browses_fv2_and_parses_items() {
        let server = MockServer::start().await;
        mount_favorites(&server).await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        let favs = p.list_favorites("RINCON_LIVING").await.unwrap();

        assert_eq!(
            favs.iter().map(|f| f.title.as_str()).collect::<Vec<_>>(),
            vec!["Jazz", "BBC Radio 6"]
        );
        assert_eq!(favs[0].id, "FV:2/12");
        assert_eq!(favs[1].subtitle.as_deref(), Some("TuneIn"));
    }

    #[tokio::test]
    async fn play_favorite_container_enqueues_then_plays_the_queue() {
        let server = MockServer::start().await;
        mount_topology(&server).await;
        mount_favorites(&server).await;
        mount_av_action(&server, "RemoveAllTracksFromQueue", None).await;
        mount_av_action(&server, "AddURIToQueue", Some(1)).await;
        mount_av_action(&server, "SetAVTransportURI", Some(1)).await;
        mount_av_action(&server, "Play", Some(1)).await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        p.play_favorite("RINCON_LIVING", "FV:2/12").await.unwrap();
        // .expect(1) on each action verifies the enqueue-then-play sequence on drop.
    }

    #[tokio::test]
    async fn play_favorite_stream_sets_transport_uri_directly() {
        let server = MockServer::start().await;
        mount_topology(&server).await;
        mount_favorites(&server).await;
        // No AddURIToQueue mounted: if the stream path wrongly enqueued, the
        // unmatched request would 404 and fail the call.
        mount_av_action(&server, "SetAVTransportURI", Some(1)).await;
        mount_av_action(&server, "Play", Some(1)).await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        p.play_favorite("RINCON_KITCHEN", "FV:2/3").await.unwrap();
    }

    #[tokio::test]
    async fn play_favorite_unknown_id_errors() {
        let server = MockServer::start().await;
        mount_topology(&server).await;
        mount_favorites(&server).await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        let err = p
            .play_favorite("RINCON_LIVING", "FV:2/999")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown"), "{err}");
    }

    #[tokio::test]
    async fn discover_groups_returns_one_group_per_player() {
        let server = MockServer::start().await;
        mount_topology(&server).await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        let groups = p.discover_groups().await.unwrap();

        // Each visible player is a room; the transient Living Room + Kitchen
        // playback group is not.
        let names: Vec<_> = groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["Living Room", "Kitchen", "Den"]);
        let lr = groups.iter().find(|g| g.name == "Living Room").unwrap();
        assert_eq!(lr.provider_group_id, "RINCON_LIVING");
        assert_eq!(lr.member_device_ids, vec!["RINCON_LIVING".to_string()]);
        assert!(lr.grouped_ref.is_none());
    }

    // SSDP parsing is covered in providers::discovery; here just confirm the
    // factory wires a discoverer for Sonos.
    #[test]
    fn factory_exposes_a_discoverer() {
        use crate::providers::MediaProviderFactory as _;
        assert!(SonosProviderFactory.discoverer().is_some());
    }

    // ── Factory ─────────────────────────────────────────────────────────────

    #[test]
    fn factory_requires_host() {
        let f = SonosProviderFactory;
        assert!(f.build(r#"{"host":"192.168.1.50"}"#).is_ok());
        assert!(f.build(r#"{}"#).is_err());
        assert!(f.build(r#"{"host":""}"#).is_err());
    }

    // ── GENA push (event parsing + poll baseline) ────────────────────────────

    #[test]
    fn parses_rendering_lastchange_master_channel() {
        let ev = r#"<Event><InstanceID val="0"><Volume channel="Master" val="42"/><Volume channel="LF" val="100"/><Mute channel="Master" val="1"/></InstanceID></Event>"#;
        assert_eq!(parse_rendering_lastchange(ev), (Some(42), Some(true)));
    }

    #[test]
    fn parses_avtransport_lastchange_state_and_track() {
        // The title's `&` is double-escaped on the wire (`&amp;amp;`): the DIDL is
        // XML-escaped inside the event, and the field text is itself escaped — so it
        // must be decoded twice or `&amp;` leaks into the now-playing string.
        let didl = "&lt;DIDL-Lite&gt;&lt;item&gt;&lt;dc:title&gt;Midnight &amp;amp; Angel&lt;/dc:title&gt;\
                    &lt;dc:creator&gt;Artist&lt;/dc:creator&gt;\
                    &lt;upnp:albumArtURI&gt;/getaa?s=1&amp;amp;u=track&lt;/upnp:albumArtURI&gt;&lt;/item&gt;&lt;/DIDL-Lite&gt;";
        let ev = format!(
            r#"<Event><InstanceID val="0"><TransportState val="PLAYING"/><CurrentTrackMetaData val="{didl}"/></InstanceID></Event>"#
        );
        let (play, np) = parse_avtransport_lastchange(&ev);
        assert_eq!(play, Some(PlayState::Playing));
        let np = np.unwrap();
        assert_eq!(np.title.as_deref(), Some("Midnight & Angel"));
        assert_eq!(np.artist.as_deref(), Some("Artist"));
        assert_eq!(np.play_state, Some(PlayState::Playing));
        // Kept relative here (double-unescaped like the other fields); the GENA
        // apply path absolutizes it against the player's cached IP.
        assert_eq!(np.artwork_url.as_deref(), Some("/getaa?s=1&u=track"));
    }

    #[test]
    fn absolutize_art_joins_relative_and_keeps_absolute() {
        let base = "http://192.168.1.50:1400";
        assert_eq!(
            absolutize_art(Some("/getaa?s=1&u=x".into()), base).as_deref(),
            Some("http://192.168.1.50:1400/getaa?s=1&u=x")
        );
        assert_eq!(
            absolutize_art(Some("getaa?u=x".into()), base).as_deref(),
            Some("http://192.168.1.50:1400/getaa?u=x")
        );
        assert_eq!(
            absolutize_art(Some("https://cdn.example/art.jpg".into()), base).as_deref(),
            Some("https://cdn.example/art.jpg")
        );
        assert_eq!(absolutize_art(Some(String::new()), base), None);
        assert_eq!(absolutize_art(None, base), None);
    }

    #[test]
    fn extract_property_unescapes_inner_xml() {
        let body = "<e:propertyset><e:property><LastChange>&lt;Event&gt;&lt;x/&gt;&lt;/Event&gt;</LastChange></e:property></e:propertyset>";
        assert_eq!(
            extract_property(body, "LastChange").as_deref(),
            Some("<Event><x/></Event>")
        );
    }

    #[tokio::test]
    async fn event_stream_emits_initial_state_with_grouping() {
        // GENA SUBSCRIBE has no wiremock mount → falls back to poll-only; the
        // initial poll still emits each player's full state + derived grouping.
        let server = MockServer::start().await;
        mount_topology(&server).await;
        mount_playing_state(&server).await;
        mount_group_state(&server).await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        let mut rx = p.event_stream().await.unwrap();

        let mut seen = std::collections::HashMap::new();
        for _ in 0..3 {
            let ev = tokio::time::timeout(Duration::from_secs(3), rx.recv())
                .await
                .expect("event within timeout")
                .expect("channel open");
            seen.insert(ev.device_id, ev.state);
        }
        assert_eq!(seen["RINCON_LIVING"].volume, 35);
        assert_eq!(
            seen["RINCON_LIVING"].group_coordinator.as_deref(),
            Some("RINCON_LIVING")
        );
        assert_eq!(
            seen["RINCON_KITCHEN"].group_coordinator.as_deref(),
            Some("RINCON_LIVING")
        );
        assert!(seen["RINCON_DEN"].group_coordinator.is_none());
    }

    // ── Group coordination (transport routes to the coordinator) ─────────────

    #[tokio::test]
    async fn resolve_maps_grouped_follower_to_coordinator() {
        let server = MockServer::start().await;
        mount_topology(&server).await;
        let p = SonosProvider::new_for_test(server.uri()).unwrap();

        // Kitchen follows Living Room → coordinator is Living Room.
        let (player, coord) = p.resolve("RINCON_KITCHEN").await.unwrap();
        assert_eq!(player.uuid, "RINCON_KITCHEN");
        assert_eq!(coord.uuid, "RINCON_LIVING");
        // Den is standalone → it coordinates itself.
        let (player, coord) = p.resolve("RINCON_DEN").await.unwrap();
        assert_eq!(player.uuid, "RINCON_DEN");
        assert_eq!(coord.uuid, "RINCON_DEN");
    }

    /// A grouped follower's transport + now-playing must come from the
    /// **coordinator** (its own AVTransport reports a slaved/stopped state),
    /// while volume stays the follower's own. Two mock servers prove the routing:
    /// the follower server has *no* AVTransport mock, so reading transport from it
    /// (the old bug) would 404.
    #[tokio::test]
    async fn follower_reports_coordinator_transport_but_own_volume() {
        let coord = MockServer::start().await;
        let follow = MockServer::start().await;

        // Topology (served by the seed = coordinator): both in one group.
        let raw = format!(
            r#"<ZoneGroups><ZoneGroup Coordinator="RINCON_COORD" ID="RINCON_COORD:1"><ZoneGroupMember UUID="RINCON_COORD" Location="{c}/xml/d.xml" ZoneName="Office Sonos"/><ZoneGroupMember UUID="RINCON_FOLLOW" Location="{f}/xml/d.xml" ZoneName="Office Sonos"/></ZoneGroup></ZoneGroups>"#,
            c = coord.uri(),
            f = follow.uri()
        );
        let escaped = raw
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;");
        Mock::given(method("POST"))
            .and(path("/ZoneGroupTopology/Control"))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "GetZoneGroupState",
                "ZoneGroupTopology",
                &format!("<ZoneGroupState>{escaped}</ZoneGroupState>"),
            )))
            .mount(&coord)
            .await;

        // Coordinator: PLAYING "HAUNT ME", and its own volume 11.
        let didl = r#"<DIDL-Lite><item><dc:title>HAUNT ME</dc:title><dc:creator>Johnny Goth</dc:creator></item></DIDL-Lite>"#
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        mount_volume(&coord, 11).await;
        Mock::given(method("POST"))
            .and(path("/MediaRenderer/AVTransport/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{AV_TRANSPORT}#GetTransportInfo\"").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "GetTransportInfo",
                "AVTransport",
                "<CurrentTransportState>PLAYING</CurrentTransportState>",
            )))
            .mount(&coord)
            .await;
        Mock::given(method("POST"))
            .and(path("/MediaRenderer/AVTransport/Control"))
            .and(header(
                "SOAPACTION",
                format!("\"{AV_TRANSPORT}#GetPositionInfo\"").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(soap_ok(
                "GetPositionInfo",
                "AVTransport",
                &format!("<TrackMetaData>{didl}</TrackMetaData>"),
            )))
            .mount(&coord)
            .await;

        // Follower: only volume (14) — NO AVTransport mock on purpose.
        mount_volume(&follow, 14).await;

        let p = SonosProvider::new_for_test(coord.uri()).unwrap();
        let st = p.get_state("RINCON_FOLLOW").await.unwrap();

        assert_eq!(st.volume, 14, "volume is the follower's own");
        assert!(st.power, "playing — transport came from the coordinator");
        assert_eq!(
            st.now_playing.unwrap().title.as_deref(),
            Some("HAUNT ME"),
            "now-playing came from the coordinator"
        );
    }
}
