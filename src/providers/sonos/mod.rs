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
//!   GetTransportInfo (play state), GetPositionInfo (track DIDL metadata)
//!
//! Sonos players have no power state; Bifrost maps `power` to "is playing"
//! (`power: false` pauses, `power: true` plays) — the same convention voice
//! assistants use.

use crate::models::audio::{
    AudioCapabilities, AudioCommand, AudioDevice, AudioDeviceKind, AudioState, NowPlaying,
    PlayState, TransportCmd,
};
use crate::providers::discovery::{DeviceDiscovery, SsdpDiscovery};
use crate::providers::{AudioProvider, AudioProviderFactory, CredentialField, FieldKind};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use reqwest::Client;
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

// ── Provider ────────────────────────────────────────────────────────────────

pub struct SonosProvider {
    client: Client,
    /// Seed player base URL (`http://<ip>:1400`); topology fans out from here.
    seed_url: String,
}

impl SonosProvider {
    fn new_with_base(seed_url: impl Into<String>) -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        Ok(Self {
            client,
            seed_url: seed_url.into(),
        })
    }

    pub fn new(host: impl AsRef<str>) -> Result<Self> {
        let host = host.as_ref().trim();
        let base = if host.starts_with("http://") || host.starts_with("https://") {
            host.trim_end_matches('/').to_string()
        } else {
            format!("http://{host}:1400")
        };
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
        Self::new_with_base(base_url)
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

    /// Fetch the household topology from the seed player. Invisible members
    /// (bridges, bonded surrounds) are skipped.
    async fn topology(&self) -> Result<(Vec<Player>, Vec<Group>)> {
        let body = self
            .soap(
                &self.seed_url,
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

    async fn read_state(&self, player: &Player, group: bool) -> Result<AudioState> {
        let (volume, mute) = self.read_volume_mute(player, group).await?;

        let transport_body = self
            .soap(
                &player.base_url,
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
                &player.base_url,
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
        let title = xml_tag(&didl, "dc:title");
        let artist = xml_tag(&didl, "dc:creator");
        let album = xml_tag(&didl, "upnp:album");

        let now_playing = (title.is_some() || play_state.is_some()).then_some(NowPlaying {
            title,
            artist,
            album,
            play_state,
        });

        Ok(AudioState {
            power: play_state == Some(PlayState::Playing),
            volume: volume.min(100),
            mute,
            source: None,
            now_playing,
            reachable: Some(true),
        })
    }

    async fn transport(&self, player: &Player, action: &str) -> Result<()> {
        let args = if action == "Play" {
            "<InstanceID>0</InstanceID><Speed>1</Speed>".to_string()
        } else {
            "<InstanceID>0</InstanceID>".to_string()
        };
        self.soap(
            &player.base_url,
            SoapCall {
                path: "/MediaRenderer/AVTransport/Control",
                service: AV_TRANSPORT,
                action,
                args,
            },
        )
        .await?;
        Ok(())
    }
}

#[async_trait]
impl AudioProvider for SonosProvider {
    fn name(&self) -> &str {
        "sonos"
    }

    async fn discover(&self) -> Result<Vec<AudioDevice>> {
        let (players, groups) = self.topology().await?;
        let by_uuid: std::collections::HashMap<&str, &Player> =
            players.iter().map(|p| (p.uuid.as_str(), p)).collect();
        let mut devices = Vec::with_capacity(players.len() + groups.len());

        for p in &players {
            // A player that drops mid-discovery still gets listed (unreachable),
            // matching how light discovery tolerates flaky devices.
            let state = self.read_state(p, false).await.unwrap_or(AudioState {
                reachable: Some(false),
                ..Default::default()
            });
            devices.push(AudioDevice {
                id: Uuid::new_v4(),
                provider_id: p.uuid.clone(),
                name: p.name.clone(),
                kind: AudioDeviceKind::Speaker,
                capabilities: AudioCapabilities {
                    sources: false,
                    transport: true,
                    now_playing: true,
                },
                state,
            });
        }

        // Multi-player groups surface as zone devices: group volume/mute via
        // the coordinator, transport drives the whole group in sync.
        for g in &groups {
            let Some(coordinator) = by_uuid.get(g.coordinator_uuid.as_str()) else {
                continue;
            };
            let name = g
                .member_uuids
                .iter()
                .filter_map(|u| by_uuid.get(u.as_str()).map(|p| p.name.as_str()))
                .collect::<Vec<_>>()
                .join(" + ");
            let state = self.read_state(coordinator, true).await.unwrap_or(AudioState {
                reachable: Some(false),
                ..Default::default()
            });
            devices.push(AudioDevice {
                id: Uuid::new_v4(),
                provider_id: format!("{GROUP_PREFIX}{}", g.coordinator_uuid),
                name,
                kind: AudioDeviceKind::Zone,
                capabilities: AudioCapabilities {
                    sources: false,
                    transport: true,
                    now_playing: true,
                },
                state,
            });
        }
        Ok(devices)
    }

    async fn get_state(&self, device_id: &str) -> Result<AudioState> {
        let target = self.find_target(device_id).await?;
        self.read_state(&target, device_id.starts_with(GROUP_PREFIX))
            .await
    }

    async fn set_state(&self, device_id: &str, cmd: &AudioCommand) -> Result<()> {
        if cmd.source.is_some() {
            return Err(anyhow!(
                "Sonos source selection is not supported; control playback from a Sonos app, then use transport commands"
            ));
        }
        let group = device_id.starts_with(GROUP_PREFIX);
        let player = self.find_target(device_id).await?;

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
            self.soap(&player.base_url, SoapCall { path, service, action, args })
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
            self.soap(&player.base_url, SoapCall { path, service, action, args })
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
                    if self.read_state(&player, group).await?.power {
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
            self.transport(&player, action).await?;
        }
        Ok(())
    }
}

// ── Factory ─────────────────────────────────────────────────────────────────

pub struct SonosProviderFactory;

impl AudioProviderFactory for SonosProviderFactory {
    fn provider_type(&self) -> &'static str {
        "sonos"
    }

    fn build(&self, credentials_json: &str) -> Result<Box<dyn AudioProvider>> {
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
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
        assert_eq!(
            names,
            vec!["Living Room", "Kitchen", "Den", "Living Room + Kitchen"]
        );
        assert_eq!(devices[0].provider_id, "RINCON_LIVING");
        assert!(
            devices[..3].iter().all(|d| d.kind == AudioDeviceKind::Speaker),
            "players are speakers"
        );
    }

    #[tokio::test]
    async fn discover_exposes_group_as_zone_with_group_volume() {
        let server = MockServer::start().await;
        mount_topology(&server).await;
        mount_playing_state(&server).await;
        mount_group_state(&server).await;

        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        let devices = p.discover().await.unwrap();

        let zone = devices
            .iter()
            .find(|d| d.kind == AudioDeviceKind::Zone)
            .expect("group zone device");
        assert_eq!(zone.provider_id, "group:RINCON_LIVING");
        assert_eq!(zone.name, "Living Room + Kitchen");
        assert_eq!(zone.state.volume, 22, "group volume, not player volume");
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
            &AudioCommand {
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
            &AudioCommand {
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
            &AudioCommand {
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
            &AudioCommand {
                transport: Some(TransportCmd::Next),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn set_state_rejects_source_selection() {
        let server = MockServer::start().await;
        let p = SonosProvider::new_for_test(server.uri()).unwrap();
        let err = p
            .set_state(
                "RINCON_LIVING",
                &AudioCommand {
                    source: Some("spotify".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not supported"));
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

    // SSDP parsing is covered in providers::discovery; here just confirm the
    // factory wires a discoverer for Sonos.
    #[test]
    fn factory_exposes_a_discoverer() {
        use crate::providers::AudioProviderFactory as _;
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
}
