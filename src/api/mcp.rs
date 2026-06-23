//! Embedded MCP server — the third surface over the shared service layer.
//!
//! Served at `/mcp` as a Streamable HTTP endpoint (stateless, JSON responses),
//! gated by the same Bearer API keys as `/api/v1`. Tools call the shared
//! service functions (`api::lights` / `api::media` / `api::rooms` /
//! `api::scenes`) directly — no HTTP hop — so the MCP surface can't
//! drift from the session and public APIs. There is no stdio transport;
//! stdio-only clients can bridge with the standard `mcp-remote` shim.
//!
//! LLM ergonomics: every tool that targets a light, room, scene, or media
//! device accepts an id **or** a case-insensitive name/substring; on no (or
//! ambiguous) match the error lists the valid options so the assistant can
//! self-correct.

use crate::AppState;
use crate::api::apikeys::require_api_key;
use crate::api::lights::{SetLightOutcome, apply_light_state, list_all_lights};
use crate::api::media::{
    FavoritesOutcome, GroupOutcome, PlayFavoriteOutcome, SetMediaOutcome, SetReceiverOutcome,
    apply_media_command, cast_to_device, get_device_live, group_devices, list_all_devices,
    list_device_favorites, play_device_favorite, set_media_receiver, ungroup_device,
};
use crate::api::power::{SetPowerOutcome, apply_power_state, list_all_power_devices};
use crate::api::remote::{
    RemoteOutcome, ResolveOutcome, apply_remote_command, list_remotes, resolve_and_play,
};
use crate::api::rooms::{apply_room_state, effective_members, list_public_rooms};
use crate::api::scenes::{SceneCaptureError, apply_scene_entries, capture_scene, list_all_scenes};
use crate::models::media::{MediaCommand, MediaFavorite, TransportCmd};
use crate::models::remote::{RemoteCommand, RemoteKey};
use crate::models::{Color, LightState};
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use sqlx::Row;
use std::sync::Arc;

/// Build the `/mcp` service: the rmcp Streamable HTTP endpoint wrapped in the
/// Bearer-key auth middleware. Mounted with `nest_service` in `build_app`.
pub fn service(state: Arc<AppState>) -> axum::Router {
    let config = StreamableHttpServerConfig::default()
        // Tools-only server: every call is an independent request/response, so
        // run stateless (no Mcp-Session-Id bookkeeping) with plain JSON bodies.
        .with_stateful_mode(false)
        .with_json_response(true)
        // Bifrost is reached via LAN/Tailscale IPs, not just localhost; the
        // Bearer gate (not Host validation) is what protects this endpoint.
        .disable_allowed_hosts();

    let mcp = StreamableHttpService::<BifrostMcp, LocalSessionManager>::new(
        {
            let state = Arc::clone(&state);
            move || Ok(BifrostMcp::new(Arc::clone(&state)))
        },
        Arc::default(),
        config,
    );

    axum::Router::new()
        .fallback_service(mcp)
        .layer(middleware::from_fn_with_state(state, require_bearer))
}

/// 401 unless the request carries a valid `Authorization: Bearer bfr_…` key —
/// the exact same check as `/api/v1`.
async fn require_bearer(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    if require_api_key(&state, req.headers()).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(req).await
}

// ── Server ───────────────────────────────────────────────────────────────────

pub struct BifrostMcp {
    state: Arc<AppState>,
    tool_router: ToolRouter<Self>,
}

impl BifrostMcp {
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for BifrostMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("bifrost", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Bifrost smart-home control. Rooms aggregate lights and media devices; \
                 scenes are saved full-state snapshots (room-scoped or whole-home) that \
                 restore each light's color/temperature/effect + power on/off. Tools accept \
                 an id or a case-insensitive name/substring for lights, rooms, scenes and \
                 media devices. Start with get_home_state for a full snapshot.",
            )
    }
}

// ── Tool parameter shapes ────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetLightRequest {
    /// Light id, or a case-insensitive name/substring.
    pub light: String,
    /// Power. Omitted = true (setting brightness/color implies on).
    pub on: Option<bool>,
    /// Brightness 0–100.
    pub brightness: Option<f32>,
    /// Hex color like "#ff8800" (converted to CIE xy with Bifrost's matrix).
    pub color: Option<String>,
    /// Color temperature in mirek (153–500 ≈ 6500K–2000K).
    pub color_temp_mirek: Option<u16>,
    /// A dynamic effect name from the light's `effects` capability (e.g.
    /// "candle", "breathe", "Sunrise"). Use the clear value the light advertises
    /// ("no_effect"/"off") to stop one. Supersedes color/temperature.
    pub effect: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetRoomRequest {
    /// Room id, or a case-insensitive name/substring.
    pub room: String,
    /// Power. Omitted = true (setting brightness/color implies on).
    pub on: Option<bool>,
    /// Brightness 0–100.
    pub brightness: Option<f32>,
    /// Hex color like "#ff8800".
    pub color: Option<String>,
    /// Color temperature in mirek (153–500 ≈ 6500K–2000K).
    pub color_temp_mirek: Option<u16>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ActivateSceneRequest {
    /// Scene id, or a case-insensitive name/substring. A scene is self-scoped —
    /// a room scene restores its room, a home scene the whole house.
    pub scene: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SaveRoomSceneRequest {
    /// Room whose current full state to snapshot (id or name).
    pub room: String,
    /// Name for the new room scene.
    pub name: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SaveHomeSceneRequest {
    /// Name for the new whole-home scene.
    pub name: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetMediaRequest {
    /// Media device id, or a case-insensitive name/substring.
    pub device: String,
    /// Power on/off.
    pub power: Option<bool>,
    /// Volume 0–100.
    pub volume: Option<u8>,
    /// Mute on/off.
    pub mute: Option<bool>,
    /// Switch input / app: a source name from the device's `source_list` (a
    /// smart TV's apps like "Hulu", or a receiver's inputs). Call get_media_state
    /// first to see the exact available names.
    pub source: Option<String>,
    /// Transport command: play, pause, stop, next, previous, or toggle.
    pub transport: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MediaDeviceRequest {
    /// Media device id, or a case-insensitive name/substring.
    pub device: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PlayFavoriteRequest {
    /// Media device id, or a case-insensitive name/substring.
    pub device: String,
    /// Favorite id, exact title, or title substring (case-insensitive).
    pub favorite: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CastMediaRequest {
    /// Target TV / media player id, or a case-insensitive name/substring.
    pub device: String,
    /// Provider-native content id — e.g. a media URL, an app id, or a channel number.
    pub content_id: String,
    /// Provider-native content type (HA `media_content_type`): music, url, app,
    /// channel, playlist, … Call get_media_state first to see what the device supports.
    pub content_type: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GroupSpeakersRequest {
    /// Speakers to join (ids or names). The coordinator may be included; it is skipped.
    pub speakers: Vec<String>,
    /// The speaker whose playback the group follows (id or name).
    pub coordinator: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct BindReceiverRequest {
    /// The source media device (TV / streamer / console) to bind, id or name.
    pub device: String,
    /// The receiver that owns volume for this source, id or name. Omit (null)
    /// to *unbind* the device so it controls its own volume again.
    pub receiver: Option<String>,
    /// Optional receiver input to switch to when the source powers on (a name
    /// from the receiver's `source_list`, e.g. "Game", "BD/DVD").
    pub receiver_source: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetPowerRequest {
    /// Power device id, or a case-insensitive name/substring (switch, plug, fan, …).
    pub device: String,
    /// Desired power: true = on, false = off.
    pub on: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PressRemoteKeyRequest {
    /// Remote/TV id, or a case-insensitive name/substring (e.g. "Bedroom TV").
    pub device: String,
    /// One canonical key: up, down, left, right, select, back, home, menu,
    /// volume_up, volume_down, mute, play_pause, next, previous, power.
    pub key: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct LaunchAppRequest {
    /// Remote/TV id, or a case-insensitive name/substring (e.g. "Bedroom TV").
    pub device: String,
    /// App to launch: a Play Store package id (`com.netflix.ninja`) or a
    /// deep-link URL (`https://www.youtube.com/watch?v=…`).
    pub app: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PlayOnRequest {
    /// The TV/streamer (or its remote), by id or case-insensitive name (e.g.
    /// "Bedroom TV"). A TV is accepted directly; its paired remote is used.
    pub device: String,
    /// What to do, in natural language: an app to open ("open Netflix" or just
    /// "Netflix") or something to play ("play Bob's Burgers"). A title that
    /// doesn't name a known app opens the TV's last-used app as the best guess.
    pub query: String,
}

// ── Tools ────────────────────────────────────────────────────────────────────

#[tool_router]
impl BifrostMcp {
    #[tool(
        description = "One-call snapshot of the whole home: rooms (with member light/media ids), lights, scenes, media devices, and power devices (switches/plugs/fans)."
    )]
    async fn get_home_state(&self) -> Result<CallToolResult, ErrorData> {
        // The reads are independent — run them concurrently rather than serially
        // awaiting each.
        let (rooms, lights, scenes, media_devices, power_devices) = tokio::join!(
            list_public_rooms(&self.state),
            list_all_lights(&self.state),
            list_all_scenes(&self.state),
            list_all_devices(&self.state),
            list_all_power_devices(&self.state),
        );
        let lights = lights.map_err(|()| internal("listing lights"))?;
        let scenes = scenes.map_err(|()| internal("listing scenes"))?;
        let media_devices = media_devices.map_err(|()| internal("listing media devices"))?;
        let power_devices = power_devices.map_err(|()| internal("listing power devices"))?;
        Ok(ok_json(serde_json::json!({
            "rooms": rooms,
            "lights": lights,
            "scenes": scenes,
            "media_devices": media_devices,
            "power_devices": power_devices,
        })))
    }

    #[tool(description = "List all lights with their ids, names and last known state.")]
    async fn list_lights(&self) -> Result<CallToolResult, ErrorData> {
        let lights = list_all_lights(&self.state)
            .await
            .map_err(|()| internal("listing lights"))?;
        Ok(ok_json(serde_json::json!(lights)))
    }

    #[tool(
        description = "Set one light's state: power, brightness (0–100), hex color, color temperature (mirek), and/or a dynamic effect. Only the given fields change behaviour; omitting `on` implies on."
    )]
    async fn set_light(
        &self,
        Parameters(req): Parameters<SetLightRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let lights = named_lights(&self.state).await?;
        let id = match resolve("light", &req.light, &lights) {
            Ok(id) => id,
            Err(e) => return Ok(e),
        };
        let new_state = match build_light_state(
            req.on,
            req.brightness,
            req.color,
            req.color_temp_mirek,
            req.effect,
        ) {
            Ok(s) => s,
            Err(e) => return Ok(fail(e)),
        };
        match apply_light_state(&self.state, &id, &new_state).await {
            SetLightOutcome::Ok => Ok(ok_text("ok")),
            SetLightOutcome::NotFound => Ok(fail("light not found or its provider is disabled")),
            SetLightOutcome::ProviderError => Ok(fail("the light's provider could not be reached")),
            SetLightOutcome::Db => Err(internal("setting light state")),
        }
    }

    #[tool(
        description = "Set every light in a room: power, brightness (0–100), hex color, and/or color temperature (mirek). Omitting `on` implies on."
    )]
    async fn set_room(
        &self,
        Parameters(req): Parameters<SetRoomRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let rooms = named_rooms(&self.state).await?;
        let id = match resolve("room", &req.room, &rooms) {
            Ok(id) => id,
            Err(e) => return Ok(e),
        };
        let new_state = match build_light_state(
            req.on,
            req.brightness,
            req.color,
            req.color_temp_mirek,
            None,
        ) {
            Ok(s) => s,
            Err(e) => return Ok(fail(e)),
        };
        let members = effective_members(&self.state, &id).await;
        let (applied, failed) = apply_room_state(&self.state, &id, &new_state, members).await;
        if applied == 0 && failed == 0 {
            return Ok(fail("the room has no controllable members"));
        }
        Ok(ok_json(
            serde_json::json!({ "applied": applied, "failed": failed }),
        ))
    }

    #[tool(
        description = "Activate a saved scene by id or name, restoring the captured light + power state (effects included). A room scene restores its room; a home scene the whole house."
    )]
    async fn activate_scene(
        &self,
        Parameters(req): Parameters<ActivateSceneRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let scenes = named_scenes(&self.state).await?;
        let scene_id = match resolve("scene", &req.scene, &scenes) {
            Ok(id) => id,
            Err(e) => return Ok(e),
        };
        match apply_scene_entries(&self.state, &scene_id, None).await {
            Some((applied, failed)) => Ok(ok_json(
                serde_json::json!({ "applied": applied, "failed": failed }),
            )),
            None => Ok(fail("scene not found or empty")),
        }
    }

    #[tool(
        description = "Save a room's current full state (each light's color/temperature/effect + power on/off) as a new named Room Scene."
    )]
    async fn save_room_scene(
        &self,
        Parameters(req): Parameters<SaveRoomSceneRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let rooms = named_rooms(&self.state).await?;
        let room_id = match resolve("room", &req.room, &rooms) {
            Ok(id) => id,
            Err(e) => return Ok(e),
        };
        save_scene_result(capture_scene(&self.state, &req.name, Some(&room_id)).await)
    }

    #[tool(
        description = "Save the whole home's current full state (every light + power device) as a new named Home Scene."
    )]
    async fn save_home_scene(
        &self,
        Parameters(req): Parameters<SaveHomeSceneRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        save_scene_result(capture_scene(&self.state, &req.name, None).await)
    }

    #[tool(
        description = "Control an media device: power, volume (0–100), mute, input source, and/or transport (play/pause/stop/next/previous/toggle). Only the given fields are sent."
    )]
    async fn set_media(
        &self,
        Parameters(req): Parameters<SetMediaRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let devices = named_media(&self.state).await?;
        let id = match resolve("media device", &req.device, &devices) {
            Ok(id) => id,
            Err(e) => return Ok(e),
        };
        let transport = match req.transport.as_deref() {
            None => None,
            Some(t) => match parse_transport(t) {
                Some(cmd) => Some(cmd),
                None => {
                    return Ok(fail(format!(
                        "unknown transport command '{t}'; use play, pause, stop, next, previous or toggle"
                    )));
                }
            },
        };
        let cmd = MediaCommand {
            power: req.power,
            volume: req.volume,
            mute: req.mute,
            source: req.source,
            transport,
        };
        match apply_media_command(&self.state, &id, &cmd).await {
            SetMediaOutcome::Ok => Ok(ok_text("ok")),
            SetMediaOutcome::NotFound => {
                Ok(fail("media device not found or its provider is disabled"))
            }
            SetMediaOutcome::BadCommand(m) => Ok(fail(m)),
            SetMediaOutcome::ProviderError => Ok(fail("the device could not be reached")),
            SetMediaOutcome::Db => Err(internal("sending media command")),
        }
    }

    #[tool(
        description = "Live state of one media device: power/volume/mute/source, now-playing metadata, and `source_list` (selectable inputs / a smart TV's apps) where available."
    )]
    async fn get_media_state(
        &self,
        Parameters(req): Parameters<MediaDeviceRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let devices = named_media(&self.state).await?;
        let id = match resolve("media device", &req.device, &devices) {
            Ok(id) => id,
            Err(e) => return Ok(e),
        };
        match get_device_live(&self.state, &id).await {
            Ok(Some(device)) => Ok(ok_json(serde_json::json!(device))),
            Ok(None) => Ok(fail("media device not found")),
            Err(()) => Err(internal("reading media device")),
        }
    }

    #[tool(
        description = "List an media device's saved favorites (e.g. Sonos Favorites: stations, playlists)."
    )]
    async fn list_media_favorites(
        &self,
        Parameters(req): Parameters<MediaDeviceRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let devices = named_media(&self.state).await?;
        let id = match resolve("media device", &req.device, &devices) {
            Ok(id) => id,
            Err(e) => return Ok(e),
        };
        match list_device_favorites(&self.state, &id).await {
            FavoritesOutcome::Ok(favs) => Ok(ok_json(serde_json::json!(favs))),
            FavoritesOutcome::NotFound => Ok(fail("media device not found")),
            FavoritesOutcome::Unreachable => {
                Ok(fail("the device could not be reached or has no favorites"))
            }
            FavoritesOutcome::Db => Err(internal("listing favorites")),
        }
    }

    #[tool(
        description = "Play one of an media device's saved favorites, matched by id, exact title, or title substring. E.g. play the 'jazz' favorite in the office."
    )]
    async fn play_media_favorite(
        &self,
        Parameters(req): Parameters<PlayFavoriteRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let devices = named_media(&self.state).await?;
        let id = match resolve("media device", &req.device, &devices) {
            Ok(id) => id,
            Err(e) => return Ok(e),
        };
        let favs = match list_device_favorites(&self.state, &id).await {
            FavoritesOutcome::Ok(favs) => favs,
            FavoritesOutcome::NotFound => return Ok(fail("media device not found")),
            FavoritesOutcome::Unreachable => {
                return Ok(fail("the device could not be reached or has no favorites"));
            }
            FavoritesOutcome::Db => return Err(internal("listing favorites")),
        };
        let favorite = match resolve_favorite(&req.favorite, &favs) {
            Ok(f) => f,
            Err(e) => return Ok(e),
        };
        match play_device_favorite(&self.state, &id, &favorite).await {
            PlayFavoriteOutcome::Ok => Ok(ok_text("ok")),
            PlayFavoriteOutcome::NotFound => Ok(fail("media device not found")),
            PlayFavoriteOutcome::BadFavorite(m) => Ok(fail(m)),
            PlayFavoriteOutcome::Unreachable => Ok(fail("the device could not be reached")),
            PlayFavoriteOutcome::Db => Err(internal("playing favorite")),
        }
    }

    #[tool(
        description = "Cast media to a TV / media player by provider-native content id + type (HA media_player.play_media). For launching a streaming app prefer the remote's launch_app; for switching to a known input/app prefer set_media's `source`."
    )]
    async fn cast_media(
        &self,
        Parameters(req): Parameters<CastMediaRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let devices = named_media(&self.state).await?;
        let id = match resolve("media device", &req.device, &devices) {
            Ok(id) => id,
            Err(e) => return Ok(e),
        };
        match cast_to_device(&self.state, &id, &req.content_id, &req.content_type).await {
            SetMediaOutcome::Ok => Ok(ok_text("ok")),
            SetMediaOutcome::NotFound => {
                Ok(fail("media device not found or its provider is disabled"))
            }
            SetMediaOutcome::BadCommand(m) => Ok(fail(m)),
            SetMediaOutcome::ProviderError => Ok(fail("the device could not be reached")),
            SetMediaOutcome::Db => Err(internal("casting media")),
        }
    }

    #[tool(
        description = "Join speakers into a synced playback group under a coordinator (e.g. play the kitchen and living room together). Sonos-native grouping, independent of Bifrost Rooms."
    )]
    async fn group_speakers(
        &self,
        Parameters(req): Parameters<GroupSpeakersRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let devices = named_media(&self.state).await?;
        let coordinator_id = match resolve("media device", &req.coordinator, &devices) {
            Ok(id) => id,
            Err(e) => return Ok(e),
        };
        if req.speakers.is_empty() {
            return Ok(fail("no speakers given"));
        }
        let mut results = Vec::new();
        for speaker in &req.speakers {
            let member_id = match resolve("media device", speaker, &devices) {
                Ok(id) => id,
                Err(e) => return Ok(e),
            };
            if member_id == coordinator_id {
                continue;
            }
            let outcome = match group_devices(&self.state, &member_id, &coordinator_id).await {
                GroupOutcome::Ok => "ok".to_string(),
                GroupOutcome::NotFound => "not found".to_string(),
                GroupOutcome::BadRequest(m) => m,
                GroupOutcome::Unreachable => "unreachable".to_string(),
                GroupOutcome::Db => return Err(internal("grouping speakers")),
            };
            results.push(serde_json::json!({ "speaker": speaker, "result": outcome }));
        }
        Ok(ok_json(serde_json::json!(results)))
    }

    #[tool(description = "Remove a speaker from its synced playback group.")]
    async fn ungroup_speaker(
        &self,
        Parameters(req): Parameters<MediaDeviceRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let devices = named_media(&self.state).await?;
        let id = match resolve("media device", &req.device, &devices) {
            Ok(id) => id,
            Err(e) => return Ok(e),
        };
        match ungroup_device(&self.state, &id).await {
            GroupOutcome::Ok => Ok(ok_text("ok")),
            GroupOutcome::NotFound => Ok(fail("media device not found")),
            GroupOutcome::BadRequest(m) => Ok(fail(m)),
            GroupOutcome::Unreachable => Ok(fail("the device could not be reached")),
            GroupOutcome::Db => Err(internal("ungrouping speaker")),
        }
    }

    #[tool(
        description = "List the home's power devices (switches, smart plugs, fans, toggles) with their on/off state and kind."
    )]
    async fn list_power_devices(&self) -> Result<CallToolResult, ErrorData> {
        let devices = list_all_power_devices(&self.state)
            .await
            .map_err(|()| internal("listing power devices"))?;
        Ok(ok_json(serde_json::json!(devices)))
    }

    #[tool(
        description = "Turn a power device (switch, plug, fan, toggle) on or off, resolved by id or name."
    )]
    async fn set_power(
        &self,
        Parameters(req): Parameters<SetPowerRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let devices = named_power(&self.state).await?;
        let id = match resolve("power device", &req.device, &devices) {
            Ok(id) => id,
            Err(e) => return Ok(e),
        };
        match apply_power_state(&self.state, &id, req.on).await {
            SetPowerOutcome::Ok => Ok(ok_text("ok")),
            SetPowerOutcome::NotFound => {
                Ok(fail("power device not found or its provider is disabled"))
            }
            SetPowerOutcome::ProviderError => Ok(fail("the device could not be reached")),
            SetPowerOutcome::Db => Err(internal("setting power state")),
        }
    }

    #[tool(
        description = "List the home's remotes (TVs / streamers controllable by a virtual remote), with on state and current foreground app."
    )]
    async fn list_remotes(&self) -> Result<CallToolResult, ErrorData> {
        Ok(ok_json(serde_json::json!(list_remotes(&self.state).await)))
    }

    #[tool(
        description = "Press a remote/TV key. `key` is one of: up, down, left, right, select, back, home, menu, volume_up, volume_down, mute, play_pause, next, previous, power. Device resolved by id or name."
    )]
    async fn press_remote_key(
        &self,
        Parameters(req): Parameters<PressRemoteKeyRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let remotes = named_remotes(&self.state).await?;
        let id = match resolve("remote", &req.device, &remotes) {
            Ok(id) => id,
            Err(e) => return Ok(e),
        };
        let key: RemoteKey = match serde_json::from_value(serde_json::json!(req.key)) {
            Ok(k) => k,
            Err(_) => return Ok(fail(format!("unknown remote key '{}'", req.key))),
        };
        let cmd = RemoteCommand::Key {
            key,
            hold_secs: None,
        };
        Ok(remote_outcome(
            apply_remote_command(&self.state, &id, &cmd).await,
        ))
    }

    #[tool(
        description = "Launch an app on a TV by Play Store package id (com.netflix.ninja) or a deep-link URL. Device resolved by id or name. E.g. 'open Netflix on the bedroom TV'."
    )]
    async fn launch_app(
        &self,
        Parameters(req): Parameters<LaunchAppRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let remotes = named_remotes(&self.state).await?;
        let id = match resolve("remote", &req.device, &remotes) {
            Ok(id) => id,
            Err(e) => return Ok(e),
        };
        let cmd = RemoteCommand::LaunchApp {
            activity: req.app.clone(),
        };
        Ok(remote_outcome(
            apply_remote_command(&self.state, &id, &cmd).await,
        ))
    }

    #[tool(
        description = "Open an app or play something on a TV by name. \"open Netflix on the bedroom TV\" launches the app; \"play Bob's Burgers on the bedroom TV\" searches the TV's libraries for the title and plays the best match, falling back to opening the TV's last-used app when nothing matches. Prefer this over launch_app for natural requests — it resolves app names, real titles, and the last-used app."
    )]
    async fn play_on(
        &self,
        Parameters(req): Parameters<PlayOnRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        match resolve_and_play(&self.state, &req.device, &req.query).await {
            ResolveOutcome::Played(title) => Ok(ok_text(format!("Playing {title}."))),
            ResolveOutcome::Launched(name) => Ok(ok_text(format!("Opened {name}."))),
            ResolveOutcome::OpenedPreferred(name) => Ok(ok_text(format!(
                "Opened {name} (the last app used) — look there for what you asked for."
            ))),
            ResolveOutcome::NoMatch => Ok(fail("couldn't find a matching app on that device")),
            ResolveOutcome::NoRemote => Ok(fail("no TV or remote found by that name")),
            ResolveOutcome::Failed => Ok(fail("the device could not be reached")),
        }
    }

    #[tool(
        description = "Bind a source media device (a TV / streamer / console) to an AV receiver that owns its volume: afterwards volume/mute for the source route to the receiver, and powering the source on switches the receiver to the given input. Omit `receiver` to unbind. E.g. 'route the living room TV's sound through the AV receiver on the Game input.'"
    )]
    async fn bind_receiver(
        &self,
        Parameters(req): Parameters<BindReceiverRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let devices = named_media(&self.state).await?;
        let id = match resolve("media device", &req.device, &devices) {
            Ok(id) => id,
            Err(e) => return Ok(e),
        };
        let receiver_id = match &req.receiver {
            None => None,
            Some(r) => match resolve("receiver", r, &devices) {
                Ok(rid) => Some(rid),
                Err(e) => return Ok(e),
            },
        };
        match set_media_receiver(&self.state, &id, receiver_id, req.receiver_source).await {
            SetReceiverOutcome::Ok => Ok(ok_text("ok")),
            SetReceiverOutcome::NotFound => Ok(fail("media device not found")),
            SetReceiverOutcome::BadRequest(m) => Ok(fail(m)),
            SetReceiverOutcome::Db => Err(internal("binding receiver")),
        }
    }
}

// ── Result helpers ───────────────────────────────────────────────────────────

fn ok_text(text: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![Content::text(text)])
}

fn ok_json(value: serde_json::Value) -> CallToolResult {
    ok_text(value.to_string())
}

/// A tool-level failure the assistant can read and recover from (wrong name,
/// unreachable device, …) — kept inside the result, not a protocol error.
fn fail(text: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![Content::text(text)])
}

/// An infrastructure failure (DB error) — a real protocol-level error.
fn internal(what: &str) -> ErrorData {
    ErrorData::internal_error(format!("internal error while {what}"), None)
}

// ── Name resolution ──────────────────────────────────────────────────────────

struct Named {
    id: String,
    name: String,
}

/// Resolve `query` against `items`: exact id, then exact name, then name
/// substring (both case-insensitive). No or ambiguous match → a `fail` result
/// listing the valid names.
fn resolve(kind: &str, query: &str, items: &[Named]) -> Result<String, CallToolResult> {
    if let Some(item) = items.iter().find(|i| i.id == query) {
        return Ok(item.id.clone());
    }
    let q = query.trim().to_lowercase();
    let exact: Vec<&Named> = items
        .iter()
        .filter(|i| i.name.to_lowercase() == q)
        .collect();
    let matches = if exact.is_empty() {
        items
            .iter()
            .filter(|i| i.name.to_lowercase().contains(&q))
            .collect()
    } else {
        exact
    };
    match matches.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => Err(fail(format!(
            "no {kind} matching '{query}'. Available: {}",
            join_names(items)
        ))),
        many => Err(fail(format!(
            "'{query}' is ambiguous — it matches: {}",
            many.iter()
                .map(|i| i.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn join_names(items: &[Named]) -> String {
    if items.is_empty() {
        return "(none)".to_string();
    }
    items
        .iter()
        .map(|i| i.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Favorite resolution mirrors `resolve`: id, exact title, then substring.
fn resolve_favorite(query: &str, favs: &[MediaFavorite]) -> Result<String, CallToolResult> {
    let items: Vec<Named> = favs
        .iter()
        .map(|f| Named {
            id: f.id.clone(),
            name: f.title.clone(),
        })
        .collect();
    resolve("favorite", query, &items)
}

async fn named_lights(state: &AppState) -> Result<Vec<Named>, ErrorData> {
    named_query(state, "SELECT id, name FROM lights ORDER BY name", "lights").await
}

async fn named_rooms(state: &AppState) -> Result<Vec<Named>, ErrorData> {
    named_query(
        state,
        "SELECT id, name FROM rooms WHERE enabled = 1 ORDER BY created_at",
        "rooms",
    )
    .await
}

async fn named_media(state: &AppState) -> Result<Vec<Named>, ErrorData> {
    named_query(
        state,
        "SELECT id, name FROM media_devices ORDER BY name",
        "media devices",
    )
    .await
}

async fn named_power(state: &AppState) -> Result<Vec<Named>, ErrorData> {
    named_query(
        state,
        "SELECT id, name FROM power_devices ORDER BY name",
        "power devices",
    )
    .await
}

async fn named_remotes(state: &AppState) -> Result<Vec<Named>, ErrorData> {
    named_query(
        state,
        "SELECT id, name FROM remote_devices WHERE enabled = 1 ORDER BY name",
        "remotes",
    )
    .await
}

/// Map a remote control outcome to an MCP tool result.
fn remote_outcome(outcome: RemoteOutcome) -> CallToolResult {
    match outcome {
        RemoteOutcome::Ok => ok_text("ok"),
        RemoteOutcome::NotFound => fail("remote not found or its provider is disabled"),
        RemoteOutcome::ProviderError => fail("the remote could not be reached"),
        RemoteOutcome::Db => fail("database error sending remote command"),
    }
}

async fn named_scenes(state: &AppState) -> Result<Vec<Named>, ErrorData> {
    let scenes = list_all_scenes(state)
        .await
        .map_err(|()| internal("listing scenes"))?;
    Ok(scenes
        .into_iter()
        .map(|s| Named {
            id: s.id,
            name: s.name,
        })
        .collect())
}

/// Map a scene-capture outcome to the MCP tool result (shared by save_room_scene
/// and save_home_scene).
fn save_scene_result(
    result: Result<crate::api::scenes::SceneCapture, SceneCaptureError>,
) -> Result<CallToolResult, ErrorData> {
    match result {
        Ok(c) => Ok(ok_json(
            serde_json::json!({ "id": c.id, "lights": c.lights, "power": c.power }),
        )),
        Err(SceneCaptureError::EmptyName) => Ok(fail("scene name is required")),
        Err(SceneCaptureError::RoomNotFound) => Ok(fail("room not found")),
        Err(SceneCaptureError::NotFound) => Ok(fail("scene not found")),
        Err(SceneCaptureError::Db) => Err(internal("saving scene")),
    }
}

/// Parse "#rrggbb" (case-insensitive) → RGB. None for anything else.
fn parse_hex_color(s: &str) -> Option<(u8, u8, u8)> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some((
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ))
}

async fn named_query(state: &AppState, sql: &str, what: &str) -> Result<Vec<Named>, ErrorData> {
    sqlx::query(sql)
        .fetch_all(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("db error listing {what}: {e}");
            internal("listing names")
        })
        .map(|rows| {
            rows.into_iter()
                .map(|r| Named {
                    id: r.get("id"),
                    name: r.get("name"),
                })
                .collect()
        })
}

// ── State building ───────────────────────────────────────────────────────────

/// Build the full `LightState` the service layer expects from the sparse tool
/// arguments. Omitting `on` means on — an assistant setting color/brightness
/// wants the light lit.
fn build_light_state(
    on: Option<bool>,
    brightness: Option<f32>,
    color: Option<String>,
    color_temp_mirek: Option<u16>,
    effect: Option<String>,
) -> Result<LightState, String> {
    let color = match color {
        None => None,
        Some(hex) => match parse_hex_color(&hex) {
            Some((r, g, b)) => Some(Color::from_rgb(r, g, b)),
            None => return Err(format!("invalid hex color '{hex}'; use e.g. #ff8800")),
        },
    };
    if let Some(b) = brightness
        && !(0.0..=100.0).contains(&b)
    {
        return Err("brightness must be 0–100".to_string());
    }
    Ok(LightState {
        on: on.unwrap_or(true),
        brightness,
        color,
        color_temp_mirek,
        reachable: None,
        effect: effect.filter(|e| !e.is_empty()),
        transport: None,
        ip: None,
    })
}

fn parse_transport(t: &str) -> Option<TransportCmd> {
    match t.trim().to_lowercase().as_str() {
        "play" => Some(TransportCmd::Play),
        "pause" => Some(TransportCmd::Pause),
        "stop" => Some(TransportCmd::Stop),
        "next" => Some(TransportCmd::Next),
        "previous" => Some(TransportCmd::Previous),
        "toggle" => Some(TransportCmd::Toggle),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(pairs: &[(&str, &str)]) -> Vec<Named> {
        pairs
            .iter()
            .map(|(id, name)| Named {
                id: id.to_string(),
                name: name.to_string(),
            })
            .collect()
    }

    #[test]
    fn resolve_matches_exact_id_first() {
        let items = named(&[("id-1", "Office"), ("id-2", "id-1")]);
        assert_eq!(resolve("room", "id-1", &items).unwrap(), "id-1");
    }

    #[test]
    fn resolve_prefers_exact_name_over_substring() {
        let items = named(&[("id-1", "Office"), ("id-2", "Office Desk")]);
        assert_eq!(resolve("room", "office", &items).unwrap(), "id-1");
    }

    #[test]
    fn resolve_matches_case_insensitive_substring() {
        let items = named(&[("id-1", "Living Room"), ("id-2", "Kitchen")]);
        assert_eq!(resolve("room", "living", &items).unwrap(), "id-1");
    }

    #[test]
    fn resolve_no_match_lists_available_names() {
        let items = named(&[("id-1", "Office"), ("id-2", "Kitchen")]);
        let err = resolve("room", "garage", &items).unwrap_err();
        let text = format!("{:?}", err.content);
        assert!(
            text.contains("Office") && text.contains("Kitchen"),
            "{text}"
        );
        assert_eq!(err.is_error, Some(true));
    }

    #[test]
    fn resolve_ambiguous_substring_lists_candidates() {
        let items = named(&[("id-1", "Office Desk"), ("id-2", "Office Shelf")]);
        let err = resolve("light", "office", &items).unwrap_err();
        let text = format!("{:?}", err.content);
        assert!(
            text.contains("Office Desk") && text.contains("Office Shelf"),
            "{text}"
        );
    }

    #[test]
    fn resolve_favorite_by_title_substring() {
        let favs = vec![
            MediaFavorite {
                id: "FV:2/1".into(),
                title: "Smooth Jazz Radio".into(),
                subtitle: None,
            },
            MediaFavorite {
                id: "FV:2/2".into(),
                title: "Morning News".into(),
                subtitle: None,
            },
        ];
        assert_eq!(resolve_favorite("jazz", &favs).unwrap(), "FV:2/1");
    }

    #[test]
    fn build_light_state_converts_hex_and_defaults_on() {
        let s = build_light_state(None, Some(50.0), Some("#ff8800".into()), None, None).unwrap();
        assert!(s.on);
        assert_eq!(s.brightness, Some(50.0));
        let c = s.color.expect("color set");
        assert!(c.x > 0.0 && c.y > 0.0);
    }

    #[test]
    fn build_light_state_rejects_bad_hex_and_brightness() {
        assert!(build_light_state(None, None, Some("nope".into()), None, None).is_err());
        assert!(build_light_state(None, Some(150.0), None, None, None).is_err());
    }

    #[test]
    fn build_light_state_carries_effect_and_drops_empty() {
        let s = build_light_state(None, None, None, None, Some("candle".into())).unwrap();
        assert_eq!(s.effect.as_deref(), Some("candle"));
        // An empty effect string is treated as "no effect", not a literal "".
        let s = build_light_state(None, None, None, None, Some(String::new())).unwrap();
        assert_eq!(s.effect, None);
    }

    #[test]
    fn parse_transport_accepts_known_commands_only() {
        assert_eq!(parse_transport("Play"), Some(TransportCmd::Play));
        assert_eq!(parse_transport(" toggle "), Some(TransportCmd::Toggle));
        assert_eq!(parse_transport("rewind"), None);
    }
}
