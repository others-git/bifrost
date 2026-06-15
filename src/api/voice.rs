//! Native voice command pipeline (M23 P1) — the text→action seam.
//!
//! `POST /api/voice/command { text, context? }` turns a natural-language command
//! into device actions, deterministically, **with no model**: a built-in grammar
//! parses the text into [`Command`]s, resolves entities by name (rooms first,
//! then devices), and dispatches through the **same service fns** the session
//! API and MCP use — never a forked control path. Audio (STT) and an LLM
//! fallback for the long tail are later phases (P2/P3); this layer is pure
//! text→action so it's fully unit-testable without either.
//!
//! Coverage: power (room-wide / `{room} lights` / a single device), brightness
//! (absolute + relative), color + color-temp (named, with shade modifiers like
//! "dark red"), volume (absolute + relative), mute, transport, scene, and
//! compound commands ("dim the office and pause the kitchen"). Play-favorite,
//! speaker grouping, read/queries and anaphora are fast-follows.

use crate::AppState;
use crate::api::ai_endpoints::AiEndpoint;
use crate::api::apikeys::require_api_key;
use crate::api::audio::{SetAudioOutcome, apply_audio_command};
use crate::api::auth::require_session;
use crate::api::lights::{SetLightOutcome, apply_light_state};
use crate::api::palette_scenes::apply_scene_to_room;
use crate::api::power::{SetPowerOutcome, apply_power_state};
use crate::api::rooms::{apply_room_state, effective_members};
use crate::models::audio::{AudioCommand, AudioState, TransportCmd};
use crate::models::{Color, LightState};
use axum::{
    Json, Router,
    extract::{Multipart, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;
use std::time::Duration;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/command", post(command_handler))
        .route("/listen", post(listen_handler))
        .route("/vocabulary", get(vocabulary_handler))
}

#[derive(Deserialize)]
struct CommandRequest {
    text: String,
    /// Optional client context — its first field is the device's `room` (id or
    /// name), used to resolve bare references ("turn off the lights" → here).
    #[serde(default)]
    context: Option<CommandContext>,
}

#[derive(Deserialize, Default)]
struct CommandContext {
    #[serde(default)]
    room: Option<String>,
}

#[derive(Serialize)]
struct CommandResponse {
    /// True only if every parsed clause succeeded.
    ok: bool,
    /// One human-readable line summarising what happened (for display / TTS).
    said: String,
    clauses: Vec<ClauseResult>,
}

#[derive(Serialize)]
struct ClauseResult {
    /// The clause text as understood.
    heard: String,
    ok: bool,
    said: String,
}

/// `/listen` response: the recognized transcript (handy for the conversation
/// modal / debugging mis-hears) plus the flattened command result.
#[derive(Serialize)]
struct ListenResponse {
    transcript: String,
    #[serde(flatten)]
    result: CommandResponse,
}

/// Voice endpoints accept **either** an authenticated browser session (the
/// Dashboard / conversation modal) **or** a `bfr_` Bearer API key — the same
/// auth `/api/v1` takes. The latter is what the headless wall-tablet voice
/// satellite uses: it has no login cookie, so it drives the voice seam with a
/// minted key like any other public-API client.
async fn voice_authed(state: &Arc<AppState>, headers: &HeaderMap) -> bool {
    require_session(state, headers).await.is_some()
        || require_api_key(state, headers).await.is_some()
}

async fn command_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CommandRequest>,
) -> impl IntoResponse {
    if !voice_authed(&state, &headers).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let ctx = req.context.unwrap_or_default();
    Json(run_command(&state, &req.text, ctx.room.as_deref()).await).into_response()
}

/// `POST /api/voice/listen` (M23 P2) — the audio ingress. Accepts a multipart
/// upload with an audio `file` (and optional `room` context), transcribes it via
/// the configured `transcription` model role, then runs the recognized text
/// through the **same** [`run_command`] seam as `/command`. Degrades with a clear
/// 503 when no transcription endpoint is configured (grammar control over
/// `/command` keeps working regardless).
async fn listen_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> impl IntoResponse {
    if !voice_authed(&state, &headers).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let mut audio: Option<(String, Vec<u8>)> = None;
    let mut room: Option<String> = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => match field.name() {
                Some("file") | Some("audio") => {
                    let filename = field.file_name().unwrap_or("audio.wav").to_string();
                    match field.bytes().await {
                        Ok(b) => audio = Some((filename, b.to_vec())),
                        Err(_) => {
                            return (StatusCode::BAD_REQUEST, "couldn't read the audio field")
                                .into_response();
                        }
                    }
                }
                Some("room") | Some("context") => {
                    room = field.text().await.ok().filter(|s| !s.trim().is_empty());
                }
                _ => {}
            },
            Ok(None) => break,
            Err(_) => return (StatusCode::BAD_REQUEST, "malformed multipart body").into_response(),
        }
    }

    let Some((filename, bytes)) = audio else {
        return (StatusCode::BAD_REQUEST, "missing audio 'file' field").into_response();
    };

    let Some(ep) = crate::api::ai_endpoints::endpoint_for(&state, "transcription").await else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "no transcription model is configured (set one under AI endpoints)",
        )
            .into_response();
    };

    let text = match transcribe(&ep, filename, bytes).await {
        Ok(t) => t,
        Err(e) => return (StatusCode::BAD_GATEWAY, e).into_response(),
    };

    let result = run_command(&state, &text, room.as_deref()).await;
    Json(ListenResponse {
        transcript: text,
        result,
    })
    .into_response()
}

/// Command-grammar keywords the speech recognizer should always know — the
/// verbs, modifiers, colors, and numbers [`parse`] understands — independent of
/// the home's device names. The kiosk fetches these plus the room/device/scene
/// names (tokenized below) and biases its on-device recognizer to them, so
/// in-domain words aren't misheard ("lights" → "lloyds"). Keep roughly in step
/// with the grammar; extra words are harmless, the point is coverage.
const COMMAND_WORDS: &[&str] = &[
    "turn",
    "on",
    "off",
    "make",
    "set",
    "change",
    "put",
    "switch",
    "toggle",
    "dim",
    "brighten",
    "brighter",
    "dimmer",
    "darker",
    "bright",
    "brightness",
    "up",
    "down",
    "to",
    "by",
    "percent",
    "louder",
    "quieter",
    "volume",
    "mute",
    "unmute",
    "silence",
    "a",
    "bit",
    "little",
    "lot",
    "ton",
    "way",
    "slightly",
    "half",
    "full",
    "max",
    "maximum",
    "minimum",
    "all",
    "everything",
    "here",
    "in",
    "the",
    "and",
    "then",
    "play",
    "pause",
    "stop",
    "resume",
    "next",
    "previous",
    "skip",
    "back",
    "scene",
    "mode",
    "activate",
    "light",
    "lights",
    "lamp",
    "lamps",
    "color",
    "colour",
    "temperature",
    "red",
    "green",
    "blue",
    "white",
    "orange",
    "yellow",
    "purple",
    "violet",
    "pink",
    "magenta",
    "cyan",
    "teal",
    "lime",
    "amber",
    "gold",
    "warm",
    "soft",
    "neutral",
    "cool",
    "daylight",
    "dark",
    "deep",
    "pale",
    "pastel",
    "baby",
    "dull",
    "muted",
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "twenty",
    "thirty",
    "forty",
    "fifty",
    "sixty",
    "seventy",
    "eighty",
    "ninety",
    "hundred",
    "bifrost",
    "hey",
    "ok",
    "okay",
    "please",
    // Wake-word homophones: the small Vosk model has no "bifrost" entry and
    // hears the wake word as "by frost"/"buy frost"/etc. Including these tokens
    // lets a constrained on-device grammar still emit a rendering the client's
    // wake matcher maps back — keeping the wake word recognizable.
    "frost",
    "buy",
    "be",
    "bi",
    "frosch",
];

/// Split a room/device/scene name into lowercase word tokens, dropping pure
/// numbers (the grammar handles those as number words) and single letters.
fn name_words(name: &str, out: &mut std::collections::BTreeSet<String>) {
    for tok in name.split(|ch: char| !ch.is_ascii_alphanumeric()) {
        let t = tok.trim().to_lowercase();
        if t.len() >= 2 && t.chars().any(|ch| ch.is_ascii_alphabetic()) {
            out.insert(t);
        }
    }
}

#[derive(Serialize)]
struct Vocabulary {
    words: Vec<String>,
}

/// `GET /api/voice/vocabulary` — the word list the kiosk biases its on-device
/// speech recognizer to: the command grammar's keywords plus every enabled room,
/// device, and scene name (tokenized). Constraining recognition to this domain
/// cuts mishears dramatically. Same auth as the rest of the voice seam (Bearer
/// key or session).
async fn vocabulary_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !voice_authed(&state, &headers).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut words: std::collections::BTreeSet<String> =
        COMMAND_WORDS.iter().map(|w| (*w).to_string()).collect();
    for ent in rooms(&state).await {
        name_words(&ent.name, &mut words);
    }
    for ent in lights(&state).await {
        name_words(&ent.name, &mut words);
    }
    for ent in audio(&state).await {
        name_words(&ent.name, &mut words);
    }
    for ent in power(&state).await {
        name_words(&ent.name, &mut words);
    }
    for ent in scenes(&state).await {
        name_words(&ent.name, &mut words);
    }
    Json(Vocabulary {
        words: words.into_iter().collect(),
    })
    .into_response()
}

/// POST audio to an OpenAI-compatible `/audio/transcriptions` endpoint and
/// return the recognized text. The timeout is generous because CPU-only STT of a
/// few seconds of speech can take a while.
async fn transcribe(ep: &AiEndpoint, filename: String, bytes: Vec<u8>) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let part = reqwest::multipart::Part::bytes(bytes).file_name(filename);
    let form = reqwest::multipart::Form::new()
        .text("model", ep.model.clone())
        .part("file", part);
    let mut req = client
        .post(format!("{}/audio/transcriptions", ep.base_url))
        .multipart(form);
    if let Some(k) = &ep.api_key {
        req = req.bearer_auth(k);
    }
    let resp = req.send().await.map_err(|e| {
        if e.is_connect() {
            "couldn't reach the transcription endpoint".to_string()
        } else if e.is_timeout() {
            "the transcription endpoint timed out".to_string()
        } else {
            e.to_string()
        }
    })?;
    if !resp.status().is_success() {
        return Err(format!("transcription endpoint returned {}", resp.status()));
    }
    #[derive(Deserialize)]
    struct Transcription {
        #[serde(default)]
        text: String,
    }
    let body: Transcription = resp
        .json()
        .await
        .map_err(|_| "transcription endpoint returned an unexpected body".to_string())?;
    let text = body.text.trim().to_string();
    if text.is_empty() {
        return Err("the transcription was empty".to_string());
    }
    Ok(text)
}

/// Parse `text` into clauses and execute each. Shared seam: STT (P2) and any
/// other ingress call this with the recognized text.
async fn run_command(state: &AppState, text: &str, context_room: Option<&str>) -> CommandResponse {
    let clauses = parse(text);
    if clauses.is_empty() {
        return CommandResponse {
            ok: false,
            said: "Sorry, I didn't catch that.".into(),
            clauses: vec![],
        };
    }
    let mut results = Vec::new();
    for clause in clauses {
        results.push(match clause {
            Clause::Cmd { heard, command } => dispatch(state, &heard, command, context_room).await,
            Clause::Noop { heard } => ClauseResult {
                heard,
                ok: true,
                said: "Okay.".into(),
            },
            // Native grammar couldn't parse this clause. Fall back to HA Assist
            // (the long tail — e.g. "play <title> on the TV"); if no HA provider
            // is configured or it errors, report the original miss.
            Clause::Unparsed { heard } => match ha_assist_fallback(state, &heard).await {
                Some((said, ok)) => ClauseResult { heard, ok, said },
                None => ClauseResult {
                    said: format!("I didn't understand \"{heard}\"."),
                    heard,
                    ok: false,
                },
            },
        });
    }
    let ok = results.iter().all(|r| r.ok);
    let said = results
        .iter()
        .map(|r| r.said.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    CommandResponse {
        ok,
        said,
        clauses: results,
    }
}

/// Delegate an utterance the native grammar couldn't parse to HA Assist. Returns
/// `Some((spoken_response, succeeded))` when an HA provider is configured and
/// reachable, or `None` when there's no HA provider or the call fails (so the
/// caller keeps the native "didn't understand" response). Native-first, HA-fallback.
async fn ha_assist_fallback(state: &AppState, text: &str) -> Option<(String, bool)> {
    let row = sqlx::query("SELECT credentials FROM providers WHERE provider_type = 'ha' LIMIT 1")
        .fetch_optional(&state.db)
        .await
        .ok()??;
    let creds_enc: String = row.get("credentials");
    let creds = state.decrypt_credentials(&creds_enc).ok()?;
    let provider = crate::providers::ha::HaProvider::from_credentials(&creds).ok()?;
    match provider.converse(text).await {
        Ok(result) => Some(result),
        Err(e) => {
            tracing::debug!("HA Assist fallback failed: {e:#}");
            None
        }
    }
}

// ── Dispatch (resolve entity → shared service fn) ────────────────────────────

async fn dispatch(
    state: &AppState,
    heard: &str,
    command: Command,
    context_room: Option<&str>,
) -> ClauseResult {
    let result = match command {
        Command::Power {
            target,
            lights_only,
            on,
        } => power_cmd(state, target, lights_only, on, context_room).await,
        Command::Brightness { target, percent } => {
            let st = LightState {
                on: true,
                brightness: Some(percent as f32),
                ..Default::default()
            };
            apply_light_target(state, target, st, context_room, &format!("{percent}%")).await
        }
        Command::Relative {
            target,
            domain,
            up,
            mag,
        } => relative_cmd(state, target, domain, up, mag, context_room).await,
        Command::Color { target, rgb, label } => {
            color_cmd(state, target, rgb, &label, context_room).await
        }
        Command::ColorTemp { target, temp } => temp_cmd(state, target, &temp, context_room).await,
        Command::Scene { target, scene } => apply_scene(state, target, &scene, context_room).await,
        Command::Volume { target, percent } => {
            audio_cmd(
                state,
                target,
                audio_set(|c| c.volume = Some(percent)),
                "Set",
                "volume",
            )
            .await
        }
        Command::Mute { target, mute } => {
            let verb = if mute { "Muted" } else { "Unmuted" };
            audio_cmd(
                state,
                target,
                audio_set(move |c| c.mute = Some(mute)),
                verb,
                "",
            )
            .await
        }
        Command::Transport { target, cmd } => {
            audio_cmd(
                state,
                target,
                audio_set(move |c| c.transport = Some(cmd)),
                "Sent",
                "",
            )
            .await
        }
    };
    match result {
        Ok(said) => ClauseResult {
            heard: heard.to_string(),
            ok: true,
            said,
        },
        Err(said) => ClauseResult {
            heard: heard.to_string(),
            ok: false,
            said,
        },
    }
}

fn audio_set(f: impl FnOnce(&mut AudioCommand)) -> AudioCommand {
    let mut c = AudioCommand::default();
    f(&mut c);
    c
}

/// Power a room (all members), a room's lights only, or a single device.
async fn power_cmd(
    state: &AppState,
    target: Target,
    lights_only: bool,
    on: bool,
    context_room: Option<&str>,
) -> Result<String, String> {
    let verb = if on { "Turned on" } else { "Turned off" };
    match resolve_target(state, &target, context_room).await? {
        Resolved::Room { id, name } => {
            if lights_only {
                let members = effective_members(state, &id).await;
                if members.is_empty() {
                    return Err(format!("{name} has no lights."));
                }
                let st = LightState {
                    on,
                    ..Default::default()
                };
                for m in members {
                    let _ = apply_light_state(state, &m.light_id, &st).await;
                }
                Ok(format!("{verb} the {name} lights."))
            } else {
                let members = effective_members(state, &id).await;
                let st = LightState {
                    on,
                    ..Default::default()
                };
                apply_room_state(state, &id, &st, members).await;
                Ok(format!("{verb} {name}."))
            }
        }
        Resolved::Light { id, name } => {
            let st = LightState {
                on,
                ..Default::default()
            };
            map_light(
                apply_light_state(state, &id, &st).await,
                format!("{verb} {name}."),
            )
        }
        Resolved::Power { id, name } => match apply_power_state(state, &id, on).await {
            SetPowerOutcome::Ok => Ok(format!("{verb} {name}.")),
            SetPowerOutcome::NotFound => Err(format!("{name} isn't available.")),
            SetPowerOutcome::ProviderError => Err(format!("Couldn't reach {name}.")),
            SetPowerOutcome::Db => Err("Something went wrong.".into()),
        },
        Resolved::Audio { id, name } => {
            let cmd = audio_set(move |c| c.power = Some(on));
            map_audio(
                apply_audio_command(state, &id, &cmd).await,
                format!("{verb} {name}."),
            )
        }
        Resolved::Everywhere => {
            let st = LightState {
                on,
                ..Default::default()
            };
            for room_id in all_room_ids(state).await {
                let members = effective_members(state, &room_id).await;
                apply_room_state(state, &room_id, &st, members).await;
            }
            Ok(format!("{verb} everything."))
        }
    }
}

/// Apply a light-state to a light-capable target (room / single light / every
/// room), returning a "Set {name} to {describe}." message. Power/audio targets
/// can't take light state, so they error. Shared by brightness/color/color-temp.
///
/// A *room* target goes through `apply_room_state`, which (because `st` carries
/// brightness/color) is a lighting-attribute change — it drives the room's
/// lights (via the native group call where possible) and, crucially, does **not**
/// power on the room's audio/power members.
async fn apply_light_target(
    state: &AppState,
    target: Target,
    st: LightState,
    context_room: Option<&str>,
    describe: &str,
) -> Result<String, String> {
    match resolve_target(state, &target, context_room).await? {
        Resolved::Room { id, name } => {
            let members = effective_members(state, &id).await;
            if members.is_empty() {
                return Err(format!("{name} has no lights."));
            }
            apply_room_state(state, &id, &st, members).await;
            Ok(format!("Set {name} to {describe}."))
        }
        Resolved::Light { id, name } => map_light(
            apply_light_state(state, &id, &st).await,
            format!("Set {name} to {describe}."),
        ),
        Resolved::Everywhere => {
            for room_id in all_room_ids(state).await {
                let members = effective_members(state, &room_id).await;
                apply_room_state(state, &room_id, &st, members).await;
            }
            Ok(format!("Set everything to {describe}."))
        }
        Resolved::Power { name, .. } | Resolved::Audio { name, .. } => {
            Err(format!("{name} can't do that."))
        }
    }
}

async fn color_cmd(
    state: &AppState,
    target: Target,
    rgb: (u8, u8, u8),
    label: &str,
    context_room: Option<&str>,
) -> Result<String, String> {
    let st = LightState {
        on: true,
        color: Some(Color::from_rgb(rgb.0, rgb.1, rgb.2)),
        ..Default::default()
    };
    apply_light_target(state, target, st, context_room, label).await
}

async fn temp_cmd(
    state: &AppState,
    target: Target,
    temp: &str,
    context_room: Option<&str>,
) -> Result<String, String> {
    let mirek = temp_mirek(temp).ok_or_else(|| format!("I don't know \"{temp}\"."))?;
    let st = LightState {
        on: true,
        color_temp_mirek: Some(mirek),
        ..Default::default()
    };
    apply_light_target(state, target, st, context_room, temp).await
}

/// Move a current 0–100 level by `mag` of itself: up → ×(1+mag), down → ×(1−mag),
/// clamped to 1–100. Raising from off jumps to a 50% base (can't scale zero).
fn scaled(current: u8, up: bool, mag: f32) -> u8 {
    let c = current as f32;
    let v = if up {
        if c < 1.0 { 50.0 } else { c * (1.0 + mag) }
    } else {
        c * (1.0 - mag)
    };
    v.round().clamp(1.0, 100.0) as u8
}

/// A light's last-known brightness (cached), defaulting to full when unknown.
async fn cached_brightness(state: &AppState, light_id: &str) -> u8 {
    cached_light_state(state, light_id)
        .await
        .and_then(|s| s.brightness)
        .map(|b| b.round().clamp(0.0, 100.0) as u8)
        .unwrap_or(100)
}

async fn cached_light_state(state: &AppState, id: &str) -> Option<LightState> {
    sqlx::query("SELECT last_state FROM lights WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.get::<Option<String>, _>("last_state"))
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// An audio device's last-known volume (cached), defaulting to 0.
async fn cached_volume(state: &AppState, id: &str) -> u8 {
    sqlx::query("SELECT last_state FROM audio_devices WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.get::<Option<String>, _>("last_state"))
        .and_then(|s| serde_json::from_str::<AudioState>(&s).ok())
        .map(|s| s.volume)
        .unwrap_or(0)
}

/// "dim the office", "turn it up", "louder" — scale the level by % of current.
async fn relative_cmd(
    state: &AppState,
    target: Target,
    domain: RelDomain,
    up: bool,
    mag: f32,
    context_room: Option<&str>,
) -> Result<String, String> {
    let resolved = resolve_target(state, &target, context_room).await?;
    let volume = match domain {
        RelDomain::Volume => true,
        RelDomain::Brightness => false,
        RelDomain::Auto => matches!(resolved, Resolved::Audio { .. }),
    };
    if volume {
        // Volume: an audio device, or a room with a single audio member.
        let (id, name) = match &resolved {
            Resolved::Audio { id, name } => (id.clone(), name.clone()),
            Resolved::Room { id, name } => lone_room_audio(state, id)
                .await
                .ok_or_else(|| format!("Which speaker in {name}?"))?,
            _ => return Err("Which speaker?".into()),
        };
        let v = scaled(cached_volume(state, &id).await, up, mag);
        let cmd = audio_set(move |c| c.volume = Some(v));
        let dir = if up { "up" } else { "down" };
        return map_audio(
            apply_audio_command(state, &id, &cmd).await,
            format!("Turned {name} {dir}."),
        );
    }
    // Brightness: per light, scaled from each light's own current level.
    let verb = if up { "Brightened" } else { "Dimmed" };
    match resolved {
        Resolved::Light { id, name } => {
            nudge_light(state, &id, up, mag).await;
            Ok(format!("{verb} {name}."))
        }
        Resolved::Room { id, name } => {
            let members = effective_members(state, &id).await;
            if members.is_empty() {
                return Err(format!("{name} has no lights."));
            }
            for m in members {
                nudge_light(state, &m.light_id, up, mag).await;
            }
            Ok(format!("{verb} {name}."))
        }
        Resolved::Everywhere => {
            for room_id in all_room_ids(state).await {
                for m in effective_members(state, &room_id).await {
                    nudge_light(state, &m.light_id, up, mag).await;
                }
            }
            Ok(format!("{verb} everything."))
        }
        Resolved::Power { name, .. } | Resolved::Audio { name, .. } => {
            Err(format!("{name} can't be dimmed."))
        }
    }
}

/// Scale one light's brightness by `mag` of its current cached level.
async fn nudge_light(state: &AppState, light_id: &str, up: bool, mag: f32) {
    let b = scaled(cached_brightness(state, light_id).await, up, mag);
    let st = LightState {
        on: true,
        brightness: Some(b as f32),
        ..Default::default()
    };
    let _ = apply_light_state(state, light_id, &st).await;
}

async fn apply_scene(
    state: &AppState,
    target: Target,
    scene: &str,
    context_room: Option<&str>,
) -> Result<String, String> {
    let scene_id = match resolve_ent(scene, &scenes(state).await) {
        Resolved2::One(id, _) => id,
        Resolved2::None => return Err(format!("I don't know a scene called \"{scene}\".")),
        Resolved2::Many(names) => {
            return Err(format!("Which scene — {}?", names.join(", ")));
        }
    };
    let rooms: Vec<(String, String)> = match resolve_target(state, &target, context_room).await? {
        Resolved::Room { id, name } => vec![(id, name)],
        Resolved::Everywhere => all_rooms(state)
            .await
            .into_iter()
            .map(|e| (e.id, e.name))
            .collect(),
        _ => return Err("Scenes apply to rooms.".into()),
    };
    let mut applied = 0;
    for (room_id, _) in &rooms {
        if apply_scene_to_room(state, &scene_id, room_id)
            .await
            .is_some()
        {
            applied += 1;
        }
    }
    if applied == 0 {
        Err("Couldn't apply that scene.".into())
    } else if rooms.len() == 1 {
        Ok(format!("Applied {scene} to {}.", rooms[0].1))
    } else {
        Ok(format!("Applied {scene} to {applied} rooms."))
    }
}

/// Volume / mute / transport — single audio device (or a room's lone audio member).
async fn audio_cmd(
    state: &AppState,
    target: Target,
    cmd: AudioCommand,
    verb: &str,
    noun: &str,
) -> Result<String, String> {
    // Volume/mute/transport target an audio device; a room name resolves only if
    // it has exactly one audio member (room-wide audio fan-out is a fast-follow).
    let (id, name) = match resolve_audio_target(state, &target).await? {
        Some(pair) => pair,
        None => return Err("Which speaker?".into()),
    };
    let tail = if noun.is_empty() {
        format!("{name}.")
    } else {
        format!("{name} {noun}.")
    };
    map_audio(
        apply_audio_command(state, &id, &cmd).await,
        format!("{verb} {tail}"),
    )
}

fn map_light(outcome: SetLightOutcome, ok_msg: String) -> Result<String, String> {
    match outcome {
        SetLightOutcome::Ok => Ok(ok_msg),
        SetLightOutcome::NotFound => Err("That light isn't available.".into()),
        SetLightOutcome::ProviderError => Err("Couldn't reach that light.".into()),
        SetLightOutcome::Db => Err("Something went wrong.".into()),
    }
}

fn map_audio(outcome: SetAudioOutcome, ok_msg: String) -> Result<String, String> {
    match outcome {
        SetAudioOutcome::Ok => Ok(ok_msg),
        SetAudioOutcome::NotFound => Err("That speaker isn't available.".into()),
        SetAudioOutcome::BadCommand(m) => Err(m),
        SetAudioOutcome::ProviderError => Err("Couldn't reach that speaker.".into()),
        SetAudioOutcome::Db => Err("Something went wrong.".into()),
    }
}

// ── Entity resolution ────────────────────────────────────────────────────────

struct Ent {
    id: String,
    name: String,
}

enum Resolved {
    Room { id: String, name: String },
    Light { id: String, name: String },
    Power { id: String, name: String },
    Audio { id: String, name: String },
    Everywhere,
}

/// Resolve a [`Target`] for a room-capable action: rooms first, then a single
/// device of any domain. `Here` uses the client's context room.
async fn resolve_target(
    state: &AppState,
    target: &Target,
    context_room: Option<&str>,
) -> Result<Resolved, String> {
    let name = match target {
        Target::Everywhere => return Ok(Resolved::Everywhere),
        Target::Here => match context_room {
            Some(r) => r.to_string(),
            None => return Err("Which room?".into()),
        },
        Target::Named(n) => n.clone(),
    };
    if let Resolved2::One(id, name) = resolve_ent(&name, &rooms(state).await) {
        return Ok(Resolved::Room { id, name });
    }
    if let Resolved2::One(id, name) = resolve_ent(&name, &lights(state).await) {
        return Ok(Resolved::Light { id, name });
    }
    if let Resolved2::One(id, name) = resolve_ent(&name, &power(state).await) {
        return Ok(Resolved::Power { id, name });
    }
    if let Resolved2::One(id, name) = resolve_ent(&name, &audio(state).await) {
        return Ok(Resolved::Audio { id, name });
    }
    Err(format!("I couldn't find \"{name}\"."))
}

/// Resolve an audio target: an audio device by name, or a room with exactly one
/// audio member (Here → the context room's lone audio member).
async fn resolve_audio_target(
    state: &AppState,
    target: &Target,
) -> Result<Option<(String, String)>, String> {
    match target {
        Target::Everywhere => Err("Pick one speaker.".into()),
        Target::Named(n) => match resolve_ent(n, &audio(state).await) {
            Resolved2::One(id, name) => Ok(Some((id, name))),
            Resolved2::Many(names) => Err(format!("Which speaker — {}?", names.join(", "))),
            // Maybe it's a room with one audio member.
            Resolved2::None => match resolve_ent(n, &rooms(state).await) {
                Resolved2::One(room_id, _) => Ok(lone_room_audio(state, &room_id).await),
                _ => Ok(None),
            },
        },
        Target::Here => Ok(None), // context-room audio fan-out is a fast-follow
    }
}

/// The single audio device in a room, if it has exactly one (id, name).
async fn lone_room_audio(state: &AppState, room_id: &str) -> Option<(String, String)> {
    let rows = sqlx::query(
        "SELECT a.id, a.name FROM audio_devices a
         WHERE a.shadowed_by IS NULL AND a.companion_of IS NULL AND a.enabled = 1 AND a.id IN (
             SELECT audio_device_id FROM room_audio_devices WHERE room_id = ?1
         )",
    )
    .bind(room_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    match rows.as_slice() {
        [one] => Some((one.get("id"), one.get("name"))),
        _ => None,
    }
}

enum Resolved2 {
    One(String, String),
    None,
    Many(Vec<String>),
}

/// Match `query` against `ents`: exact id, then exact name, then name substring
/// (case-insensitive) — the same heuristic as `mcp::resolve`.
fn resolve_ent(query: &str, ents: &[Ent]) -> Resolved2 {
    if let Some(e) = ents.iter().find(|e| e.id == query) {
        return Resolved2::One(e.id.clone(), e.name.clone());
    }
    let q = query.trim().to_lowercase();
    let exact: Vec<&Ent> = ents.iter().filter(|e| e.name.to_lowercase() == q).collect();
    let matches: Vec<&Ent> = if exact.is_empty() {
        ents.iter()
            .filter(|e| e.name.to_lowercase().contains(&q))
            .collect()
    } else {
        exact
    };
    match matches.as_slice() {
        [one] => Resolved2::One(one.id.clone(), one.name.clone()),
        [] => Resolved2::None,
        many => Resolved2::Many(many.iter().map(|e| e.name.clone()).collect()),
    }
}

async fn ents(state: &AppState, sql: &str) -> Vec<Ent> {
    sqlx::query(sql)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| Ent {
            id: r.get("id"),
            name: r.get("name"),
        })
        .collect()
}

async fn rooms(state: &AppState) -> Vec<Ent> {
    ents(
        state,
        "SELECT id, name FROM rooms WHERE enabled = 1 ORDER BY created_at",
    )
    .await
}
async fn all_rooms(state: &AppState) -> Vec<Ent> {
    rooms(state).await
}
async fn all_room_ids(state: &AppState) -> Vec<String> {
    rooms(state).await.into_iter().map(|e| e.id).collect()
}
async fn lights(state: &AppState) -> Vec<Ent> {
    ents(
        state,
        "SELECT id, name FROM lights WHERE enabled = 1 AND shadowed_by IS NULL ORDER BY name",
    )
    .await
}
async fn power(state: &AppState) -> Vec<Ent> {
    ents(
        state,
        "SELECT id, name FROM power_devices WHERE enabled = 1 AND shadowed_by IS NULL ORDER BY name",
    )
    .await
}
async fn audio(state: &AppState) -> Vec<Ent> {
    ents(
        state,
        "SELECT id, name FROM audio_devices WHERE enabled = 1 AND shadowed_by IS NULL AND companion_of IS NULL ORDER BY name",
    )
    .await
}
async fn scenes(state: &AppState) -> Vec<Ent> {
    ents(
        state,
        "SELECT id, name FROM palette_scenes ORDER BY created_at",
    )
    .await
}

// ── Grammar (pure) ───────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum Target {
    Named(String),
    Here,
    Everywhere,
}

/// Which level a relative change targets. `Auto` ("turn up/down") is resolved
/// by the target's type at dispatch: an audio device → volume, else brightness.
#[derive(Debug, PartialEq, Clone, Copy)]
enum RelDomain {
    Brightness,
    Volume,
    Auto,
}

#[derive(Debug, PartialEq)]
enum Command {
    Power {
        target: Target,
        lights_only: bool,
        on: bool,
    },
    Brightness {
        target: Target,
        percent: u8,
    },
    /// Relative brightness/volume change — `mag` is the fraction of the *current*
    /// value to move (0.5 = halve/×1.5); `domain` Auto resolves by target type.
    Relative {
        target: Target,
        domain: RelDomain,
        up: bool,
        mag: f32,
    },
    /// Set color — RGB computed at parse from a base color + optional shade
    /// modifier ("dark red", "light blue"); `label` is the spoken form.
    Color {
        target: Target,
        rgb: (u8, u8, u8),
        label: String,
    },
    /// Set white temperature by name (validated against [`TEMPS`]).
    ColorTemp {
        target: Target,
        temp: String,
    },
    Volume {
        target: Target,
        percent: u8,
    },
    Mute {
        target: Target,
        mute: bool,
    },
    Transport {
        target: Target,
        cmd: TransportCmd,
    },
    Scene {
        target: Target,
        scene: String,
    },
}

#[derive(Debug, PartialEq)]
enum Clause {
    Cmd { heard: String, command: Command },
    Noop { heard: String },
    Unparsed { heard: String },
}

/// Normalize, split into clauses, and parse each.
fn parse(text: &str) -> Vec<Clause> {
    let norm = normalize(text);
    if norm.is_empty() {
        return vec![];
    }
    split_clauses(&norm)
        .into_iter()
        .map(|c| parse_clause(&c))
        .collect()
}

/// Lowercase, strip the wake word and leading politeness, collapse whitespace.
fn normalize(text: &str) -> String {
    let mut s = text.to_lowercase();
    for p in [".", ",", "!", "?", ";"] {
        s = s.replace(p, &format!("{p} "));
    }
    let mut s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    // Wake word.
    for w in ["hey bifrost", "ok bifrost", "okay bifrost", "bifrost"] {
        if let Some(rest) = s.strip_prefix(w) {
            s = rest.trim_start_matches([' ', ',']).to_string();
            break;
        }
    }
    // Leading politeness.
    for p in [
        "could you please",
        "can you please",
        "could you",
        "can you",
        "would you",
        "please",
    ] {
        if let Some(rest) = s.strip_prefix(p) {
            s = rest.trim().to_string();
            break;
        }
    }
    s.trim().to_string()
}

/// Split a normalized utterance into clauses on "and" / "then" / commas.
fn split_clauses(s: &str) -> Vec<String> {
    s.replace(" then ", " and ")
        .replace(',', " and ")
        .split(" and ")
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect()
}

fn parse_clause(clause: &str) -> Clause {
    let heard = clause.to_string();
    match parse_command(clause) {
        Some(Some(command)) => Clause::Cmd { heard, command },
        Some(None) => Clause::Noop { heard },
        None => Clause::Unparsed { heard },
    }
}

/// `None` = unparsed; `Some(None)` = a no-op (cancel); `Some(Some(c))` = a command.
fn parse_command(c: &str) -> Option<Option<Command>> {
    let c = c.trim();
    if matches!(c, "never mind" | "nevermind" | "cancel" | "stop it") {
        return Some(None);
    }
    // Scene: "{name} scene/mode", optionally "in {room}" / "everywhere".
    if let Some(cmd) = parse_scene(c) {
        return Some(Some(cmd));
    }
    // Bare transport verbs.
    if let Some(cmd) = parse_transport(c) {
        return Some(Some(cmd));
    }
    if let Some(cmd) = parse_mute(c) {
        return Some(Some(cmd));
    }
    if let Some(cmd) = parse_volume(c) {
        return Some(Some(cmd));
    }
    if let Some(cmd) = parse_brightness(c) {
        return Some(Some(cmd));
    }
    // Relative ("dim", "louder", "turn up") — no explicit number.
    if let Some(cmd) = parse_relative(c) {
        return Some(Some(cmd));
    }
    if let Some(cmd) = parse_power(c) {
        return Some(Some(cmd));
    }
    // After power, so "turn off X" wins over a trailing color word.
    if let Some(cmd) = parse_color(c) {
        return Some(Some(cmd));
    }
    None
}

/// "dim/brighten {X}", "turn up/down {X}", "{X} louder/quieter/brighter/dimmer",
/// "volume up/down" — a level nudge with no explicit number.
fn parse_relative(c: &str) -> Option<Command> {
    let mag = if c.contains("a bit") || c.contains("a little") || c.contains("slightly") {
        0.25
    } else if c.contains("a lot") || c.contains("a ton") || c.contains(" way ") {
        0.75
    } else {
        0.5
    };
    // Drop magnitude phrases so they don't pollute the target.
    let mut s = c.to_string();
    for p in ["a bit", "a little", "slightly", "a lot", "a ton"] {
        s = s.replace(p, " ");
    }
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.as_str();

    let (domain, up, rest): (RelDomain, bool, &str) = if let Some(r) = s.strip_prefix("dim ") {
        (RelDomain::Brightness, false, r)
    } else if s == "dim" {
        (RelDomain::Brightness, false, "")
    } else if let Some(r) = s.strip_prefix("brighten ") {
        (RelDomain::Brightness, true, r)
    } else if s == "brighten" {
        (RelDomain::Brightness, true, "")
    } else if let Some(r) = s.strip_prefix("turn up ") {
        relative_turn(true, r)
    } else if let Some(r) = s.strip_prefix("turn down ") {
        relative_turn(false, r)
    } else if s == "turn up" || s == "turn it up" {
        (RelDomain::Auto, true, "")
    } else if s == "turn down" || s == "turn it down" {
        (RelDomain::Auto, false, "")
    } else if let Some(r) = s.strip_prefix("volume up") {
        (RelDomain::Volume, true, r.trim())
    } else if let Some(r) = s.strip_prefix("volume down") {
        (RelDomain::Volume, false, r.trim())
    } else if let Some(h) = suffix_word(s, "louder") {
        (RelDomain::Volume, true, h)
    } else if let Some(h) = suffix_word(s, "quieter") {
        (RelDomain::Volume, false, h)
    } else if let Some(h) = suffix_word(s, "brighter") {
        (RelDomain::Brightness, true, h)
    } else if let Some(h) = suffix_word(s, "dimmer").or_else(|| suffix_word(s, "darker")) {
        (RelDomain::Brightness, false, h)
    } else {
        return None;
    };
    let rest = strip_verb(rest);
    let target = if rest.is_empty() {
        Target::Here
    } else {
        parse_target(rest).0
    };
    Some(Command::Relative {
        target,
        domain,
        up,
        mag,
    })
}

/// "turn up/down {rest}": a trailing "volume" forces the volume domain; else the
/// domain is auto (resolved by the target type at dispatch).
fn relative_turn(up: bool, rest: &str) -> (RelDomain, bool, &str) {
    let rest = rest.trim();
    match rest.strip_suffix("volume") {
        Some(h) => (RelDomain::Volume, up, h.trim()),
        None => (RelDomain::Auto, up, rest),
    }
}

/// If `s` ends with word `w` on a boundary, return the head before it.
fn suffix_word<'a>(s: &'a str, w: &str) -> Option<&'a str> {
    if s == w {
        return Some("");
    }
    s.strip_suffix(w)
        .filter(|h| h.ends_with(' '))
        .map(|h| h.trim_end())
}

/// Strip a leading action verb ("make/turn/set/…") from a target phrase.
fn strip_verb(s: &str) -> &str {
    let s = s.trim();
    for v in ["make", "turn", "set", "change", "put"] {
        if let Some(r) = s.strip_prefix(v)
            && (r.is_empty() || r.starts_with(' '))
        {
            return r.trim();
        }
    }
    s
}

/// Named colors → RGB. Whites that are really temperatures live in [`TEMPS`].
const COLORS: &[(&str, (u8, u8, u8))] = &[
    ("red", (255, 0, 0)),
    ("green", (0, 255, 0)),
    ("blue", (0, 0, 255)),
    ("white", (255, 255, 255)),
    ("orange", (255, 136, 0)),
    ("yellow", (255, 214, 0)),
    ("purple", (160, 0, 255)),
    ("violet", (148, 0, 211)),
    ("pink", (255, 105, 180)),
    ("magenta", (255, 0, 255)),
    ("cyan", (0, 255, 255)),
    ("teal", (0, 160, 160)),
    ("lime", (170, 255, 0)),
    ("amber", (255, 160, 0)),
    ("gold", (255, 200, 0)),
];

/// White-temperature names → mirek (1e6 / kelvin). Multi-word entries first so
/// "warm white" matches before "warm"/"white".
const TEMPS: &[(&str, u16)] = &[
    ("warm white", 370),
    ("soft white", 370),
    ("neutral white", 250),
    ("cool white", 200),
    ("daylight", 154),
    ("warm", 370),
    ("neutral", 250),
    ("cool", 200),
];

fn temp_mirek(name: &str) -> Option<u16> {
    TEMPS.iter().find(|(n, _)| *n == name).map(|(_, m)| *m)
}

/// Shade modifiers that adjust a base color ("dark red", "light blue"). Returns
/// the transformed RGB. Unlisted words aren't modifiers (kept as the target).
fn shade(modifier: &str, (r, g, b): (u8, u8, u8)) -> Option<(u8, u8, u8)> {
    let darken = |c: u8, f: f32| (c as f32 * f).round() as u8;
    let lighten = |c: u8, f: f32| (c as f32 + (255.0 - c as f32) * f).round() as u8;
    let toward = |c: u8, t: f32, f: f32| (c as f32 + (t - c as f32) * f).round() as u8;
    Some(match modifier {
        "dark" | "deep" => (darken(r, 0.45), darken(g, 0.45), darken(b, 0.45)),
        "light" | "pale" | "soft" | "pastel" | "baby" => {
            (lighten(r, 0.6), lighten(g, 0.6), lighten(b, 0.6))
        }
        "dull" | "muted" => (
            toward(r, 128.0, 0.4),
            toward(g, 128.0, 0.4),
            toward(b, 128.0, 0.4),
        ),
        // Known but no transform (base colors are already vivid) — still consumed
        // so the word isn't mistaken for the target.
        "bright" | "vivid" | "neon" | "electric" | "hot" => (r, g, b),
        _ => return None,
    })
}

/// Parse "make/turn/set {target} [shade] {color}" (or "[shade] {color} in
/// {room}"). Temps (whites) match before plain colors; a base color may be
/// preceded by a shade modifier. RGB is computed here so dispatch is infallible.
fn parse_color(c: &str) -> Option<Command> {
    // An explicit room may trail via "in {room}" ("dark red in the office").
    let (head, room) = match c.rsplit_once(" in ") {
        Some((h, room)) => (h.trim(), Some(room)),
        None => (c, None),
    };
    for (phrase, _) in TEMPS {
        if let Some(pre) = ends_with_phrase(head, phrase) {
            let target = room.map_or_else(|| color_target(pre), |r| parse_target(r).0);
            return Some(Command::ColorTemp {
                target,
                temp: (*phrase).to_string(),
            });
        }
    }
    for (word, base) in COLORS {
        let Some(pre) = ends_with_phrase(head, word) else {
            continue;
        };
        // A trailing modifier on `pre` ("… dark") shades the color and is consumed;
        // a bare modifier ("dark red", no target) leaves an empty target → here.
        let (rgb, label, target_pre) = match pre.rsplit_once(' ') {
            Some((before, last)) if shade(last, *base).is_some() => (
                shade(last, *base).unwrap(),
                format!("{last} {word}"),
                before,
            ),
            _ if shade(pre.trim(), *base).is_some() => (
                shade(pre.trim(), *base).unwrap(),
                format!("{} {word}", pre.trim()),
                "",
            ),
            _ => (*base, (*word).to_string(), pre),
        };
        let target = room.map_or_else(|| color_target(target_pre), |r| parse_target(r).0);
        return Some(Command::Color { target, rgb, label });
    }
    None
}

/// If `c` ends with the word/phrase `w` on a word boundary, return the head
/// before it (so "blueberry" doesn't match "blue").
fn ends_with_phrase<'a>(c: &'a str, w: &str) -> Option<&'a str> {
    if c == w {
        return Some("");
    }
    c.strip_suffix(w)
        .filter(|h| h.ends_with(' '))
        .map(|h| h.trim_end())
}

/// The target phrase before a color word — strip the verb and any "to".
/// (Unknown adjectives that aren't shade modifiers stay in the target; a truly
/// creative color like "ogre green" simply won't resolve and is the LLM
/// fallback's job in P3.)
fn color_target(head: &str) -> Target {
    let mut h = strip_verb(head);
    h = h
        .strip_prefix("colour ")
        .or(h.strip_prefix("color "))
        .unwrap_or(h);
    h = h.strip_prefix("to ").unwrap_or(h).trim();
    h = h.strip_suffix(" to").unwrap_or(h).trim();
    parse_target(h).0
}

fn parse_scene(c: &str) -> Option<Command> {
    let (head, kw) = if let Some(h) = c.strip_suffix(" scene") {
        (h, true)
    } else if let Some(h) = c.strip_suffix(" mode") {
        (h, true)
    } else {
        ("", false)
    };
    // Also handle "{name} scene in {room}" / "{name} scene everywhere".
    let (scene_part, target) = if kw {
        (head.to_string(), Target::Here)
    } else if let Some(idx) = c.find(" scene ").or_else(|| c.find(" mode ")) {
        let scene = c[..idx].to_string();
        let rest = c[idx..]
            .trim_start_matches(" scene ")
            .trim_start_matches(" mode ");
        (scene, target_from_rest(rest))
    } else {
        return None;
    };
    let scene = strip_articles(&scene_part);
    if scene.is_empty() {
        return None;
    }
    Some(Command::Scene {
        target,
        scene: scene.to_string(),
    })
}

/// Parse a trailing "in {room}" / "everywhere" fragment into a Target.
fn target_from_rest(rest: &str) -> Target {
    let rest = rest.trim();
    if let Some(room) = rest.strip_prefix("in ") {
        parse_target(room).0
    } else if matches!(rest, "everywhere" | "whole house" | "the whole house") {
        Target::Everywhere
    } else if rest.is_empty() {
        Target::Here
    } else {
        parse_target(rest).0
    }
}

fn parse_transport(c: &str) -> Option<Command> {
    // "play X in Y" is play-favorite (fast-follow) — don't treat as transport.
    if c.starts_with("play ") && (c.contains(" in ") || c.contains(" on ")) {
        return None;
    }
    let (verb, rest) = split_first(c);
    let cmd = match verb {
        "pause" => TransportCmd::Pause,
        "play" | "resume" => TransportCmd::Play,
        "stop" => TransportCmd::Stop,
        "next" | "skip" => TransportCmd::Next,
        "previous" | "back" => TransportCmd::Previous,
        _ => return None,
    };
    let target = if rest.trim().is_empty() {
        Target::Here
    } else {
        parse_target(rest).0
    };
    Some(Command::Transport { target, cmd })
}

fn parse_mute(c: &str) -> Option<Command> {
    let (mute, rest) = if let Some(r) = c.strip_prefix("mute ") {
        (true, r)
    } else if c == "mute" {
        (true, "")
    } else if let Some(r) = c.strip_prefix("unmute ") {
        (false, r)
    } else if c == "unmute" {
        (false, "")
    } else if let Some(r) = c.strip_prefix("silence ") {
        (true, r)
    } else {
        return None;
    };
    let target = if rest.trim().is_empty() {
        Target::Here
    } else {
        parse_target(rest).0
    };
    Some(Command::Mute { target, mute })
}

fn parse_volume(c: &str) -> Option<Command> {
    if !c.contains("volume") {
        return None;
    }
    let percent = find_percent(c)?;
    // Strip "volume" and level words to leave the target.
    let cleaned = c
        .replace("set ", " ")
        .replace("the volume", " ")
        .replace("volume", " ")
        .replace("to ", " ");
    let cleaned = remove_number_tokens(&cleaned);
    let target = parse_target(cleaned.trim()).0;
    Some(Command::Volume { target, percent })
}

fn parse_brightness(c: &str) -> Option<Command> {
    // Absolute only in P1: requires an explicit percentage.
    let percent = find_percent(c)?;
    if !(c.contains("set ")
        || c.contains(" to ")
        || c.contains('%')
        || c.contains("percent")
        || c.contains("brightness")
        || c.contains("dim"))
    {
        return None;
    }
    let cleaned = c
        .replace("set ", " ")
        .replace("brightness", " ")
        .replace("dim ", " ")
        .replace(" to ", " ");
    let cleaned = remove_number_tokens(&cleaned);
    let (target, _) = parse_target(cleaned.trim());
    Some(Command::Brightness { target, percent })
}

fn parse_power(c: &str) -> Option<Command> {
    if c == "lights out" {
        return Some(Command::Power {
            target: Target::Here,
            lights_only: true,
            on: false,
        });
    }
    // Prefix verbs: "turn on/off X", "switch on/off X", "shut off/down X", "kill X".
    let (on, rest) = if let Some(r) = c.strip_prefix("turn on ") {
        (true, r)
    } else if let Some(r) = c.strip_prefix("turn off ") {
        (false, r)
    } else if let Some(r) = c.strip_prefix("switch on ") {
        (true, r)
    } else if let Some(r) = c.strip_prefix("switch off ") {
        (false, r)
    } else if let Some(r) = c.strip_prefix("shut off ") {
        (false, r)
    } else if let Some(r) = c.strip_prefix("shut down ") {
        (false, r)
    } else if let Some(r) = c.strip_prefix("kill ") {
        (false, r)
    } else {
        // Suffix form: "X on" / "X off".
        if let Some(r) = c.strip_suffix(" on") {
            (true, r)
        } else if let Some(r) = c.strip_suffix(" off") {
            (false, r)
        } else {
            return None;
        }
    };
    let (target, lights_only) = parse_target(rest);
    Some(Command::Power {
        target,
        lights_only,
        on,
    })
}

/// Interpret a target phrase → (Target, lights_only). Strips articles and a
/// trailing "lights"/"light"; maps "here"/"everything" to their variants.
fn parse_target(phrase: &str) -> (Target, bool) {
    let p = strip_articles(phrase.trim());
    let mut tokens: Vec<&str> = p.split_whitespace().collect();
    let mut lights_only = false;
    if matches!(tokens.last().copied(), Some("lights") | Some("light")) {
        lights_only = true;
        tokens.pop();
    }
    let name = tokens.join(" ");
    let target = match name.as_str() {
        "" | "the" | "a" | "my" | "here" | "in here" | "this room" | "it" | "this" => Target::Here,
        "everything" | "everywhere" | "all" | "house" | "whole house" => Target::Everywhere,
        other => Target::Named(other.to_string()),
    };
    (target, lights_only)
}

fn strip_articles(s: &str) -> &str {
    let s = s.trim();
    for a in ["the ", "a ", "my ", "all the "] {
        if let Some(rest) = s.strip_prefix(a) {
            return rest.trim();
        }
    }
    s
}

fn split_first(s: &str) -> (&str, &str) {
    match s.trim().split_once(' ') {
        Some((a, b)) => (a, b),
        None => (s.trim(), ""),
    }
}

/// Find a 0–100 percentage in the clause: "50%", "50 percent", or "to 50".
fn find_percent(c: &str) -> Option<u8> {
    let tokens: Vec<&str> = c.split_whitespace().collect();
    for (i, tok) in tokens.iter().enumerate() {
        let digits: String = tok.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        if digits.is_empty() {
            continue;
        }
        let trailing = &tok[digits.len()..];
        let is_pct = trailing.starts_with('%')
            || trailing == "%"
            || tokens.get(i + 1).is_some_and(|n| n.starts_with("percent"))
            || i > 0 && tokens[i - 1] == "to";
        if (is_pct || trailing.is_empty())
            && let Ok(n) = digits.parse::<u16>()
        {
            return Some(n.min(100) as u8);
        }
    }
    None
}

/// Drop numeric / percent / "percent" tokens so what's left can be the target.
fn remove_number_tokens(c: &str) -> String {
    c.split_whitespace()
        .filter(|t| {
            let t = t.trim_end_matches('%');
            let is_number = !t.is_empty() && t.chars().all(|ch| ch.is_ascii_digit());
            !is_number && t != "percent"
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(text: &str) -> Command {
        match parse(text).into_iter().next() {
            Some(Clause::Cmd { command, .. }) => command,
            other => panic!("expected a command, got {other:?}"),
        }
    }

    #[test]
    fn power_off_a_room() {
        assert_eq!(
            cmd("bifrost, turn off the office"),
            Command::Power {
                target: Target::Named("office".into()),
                lights_only: false,
                on: false
            }
        );
    }

    #[test]
    fn power_off_room_lights_only() {
        assert_eq!(
            cmd("turn off the office lights"),
            Command::Power {
                target: Target::Named("office".into()),
                lights_only: true,
                on: false
            }
        );
    }

    #[test]
    fn suffix_on_form() {
        assert_eq!(
            cmd("kitchen on"),
            Command::Power {
                target: Target::Named("kitchen".into()),
                lights_only: false,
                on: true
            }
        );
    }

    #[test]
    fn lights_out_is_here_lights_off() {
        assert_eq!(
            cmd("lights out"),
            Command::Power {
                target: Target::Here,
                lights_only: true,
                on: false
            }
        );
    }

    #[test]
    fn brightness_absolute_percent() {
        assert_eq!(
            cmd("set the office to 30%"),
            Command::Brightness {
                target: Target::Named("office".into()),
                percent: 30
            }
        );
        assert_eq!(
            cmd("dim the kitchen to 20 percent"),
            Command::Brightness {
                target: Target::Named("kitchen".into()),
                percent: 20
            }
        );
    }

    #[test]
    fn volume_absolute() {
        assert_eq!(
            cmd("set the office volume to 40"),
            Command::Volume {
                target: Target::Named("office".into()),
                percent: 40
            }
        );
    }

    #[test]
    fn mute_and_unmute() {
        assert_eq!(
            cmd("mute the living room"),
            Command::Mute {
                target: Target::Named("living room".into()),
                mute: true
            }
        );
        assert_eq!(
            cmd("unmute the office"),
            Command::Mute {
                target: Target::Named("office".into()),
                mute: false
            }
        );
    }

    #[test]
    fn transport_verbs() {
        assert_eq!(
            cmd("pause the office"),
            Command::Transport {
                target: Target::Named("office".into()),
                cmd: TransportCmd::Pause
            }
        );
        assert_eq!(
            cmd("next"),
            Command::Transport {
                target: Target::Here,
                cmd: TransportCmd::Next
            }
        );
    }

    #[test]
    fn scene_here_and_in_room_and_everywhere() {
        assert_eq!(
            cmd("movie scene"),
            Command::Scene {
                target: Target::Here,
                scene: "movie".into()
            }
        );
        assert_eq!(
            cmd("relax scene in the living room"),
            Command::Scene {
                target: Target::Named("living room".into()),
                scene: "relax".into()
            }
        );
        assert_eq!(
            cmd("cozy mode everywhere"),
            Command::Scene {
                target: Target::Everywhere,
                scene: "cozy".into()
            }
        );
    }

    #[test]
    fn color_named() {
        assert_eq!(
            cmd("bifrost, turn the office lights blue"),
            Command::Color {
                target: Target::Named("office".into()),
                rgb: (0, 0, 255),
                label: "blue".into()
            }
        );
        assert_eq!(
            cmd("make the kitchen red"),
            Command::Color {
                target: Target::Named("kitchen".into()),
                rgb: (255, 0, 0),
                label: "red".into()
            }
        );
        assert_eq!(
            cmd("set the office to green"),
            Command::Color {
                target: Target::Named("office".into()),
                rgb: (0, 255, 0),
                label: "green".into()
            }
        );
    }

    #[test]
    fn color_with_shade_modifier() {
        // "dark red" → red darkened, target preserved, modifier not glued to it.
        match cmd("make the office dark red") {
            Command::Color { target, rgb, label } => {
                assert_eq!(target, Target::Named("office".into()));
                assert_eq!(label, "dark red");
                assert_eq!(rgb, (115, 0, 0)); // 255 * 0.45 ≈ 115
            }
            other => panic!("expected color, got {other:?}"),
        }
        // "light blue" → blue lightened toward white.
        match cmd("turn the bedroom light blue") {
            Command::Color { target, rgb, label } => {
                assert_eq!(target, Target::Named("bedroom".into()));
                assert_eq!(label, "light blue");
                assert_eq!(rgb, (153, 153, 255)); // r,g lightened; b stays 255
            }
            other => panic!("expected color, got {other:?}"),
        }
    }

    #[test]
    fn bare_shade_color_has_no_target() {
        assert_eq!(
            cmd("dark red"),
            Command::Color {
                target: Target::Here,
                rgb: (115, 0, 0),
                label: "dark red".into()
            }
        );
    }

    #[test]
    fn color_temp_whites() {
        assert_eq!(
            cmd("make the office warm white"),
            Command::ColorTemp {
                target: Target::Named("office".into()),
                temp: "warm white".into()
            }
        );
        // "warm white" wins over "white"/"warm".
        assert_eq!(
            cmd("daylight in the office"),
            Command::ColorTemp {
                target: Target::Named("office".into()),
                temp: "daylight".into()
            }
        );
    }

    #[test]
    fn power_off_wins_over_trailing_word() {
        // "turn off X" must stay power even if X isn't a color.
        assert_eq!(
            cmd("turn off the office"),
            Command::Power {
                target: Target::Named("office".into()),
                lights_only: false,
                on: false
            }
        );
    }

    #[test]
    fn relative_dim_and_brighten() {
        assert_eq!(
            cmd("dim the office"),
            Command::Relative {
                target: Target::Named("office".into()),
                domain: RelDomain::Brightness,
                up: false,
                mag: 0.5
            }
        );
        assert_eq!(
            cmd("brighten the kitchen a lot"),
            Command::Relative {
                target: Target::Named("kitchen".into()),
                domain: RelDomain::Brightness,
                up: true,
                mag: 0.75
            }
        );
        assert_eq!(
            cmd("make the office dimmer a bit"),
            Command::Relative {
                target: Target::Named("office".into()),
                domain: RelDomain::Brightness,
                up: false,
                mag: 0.25
            }
        );
    }

    #[test]
    fn relative_volume() {
        assert_eq!(
            cmd("make the office louder"),
            Command::Relative {
                target: Target::Named("office".into()),
                domain: RelDomain::Volume,
                up: true,
                mag: 0.5
            }
        );
        assert_eq!(
            cmd("turn up the volume"),
            Command::Relative {
                target: Target::Here,
                domain: RelDomain::Volume,
                up: true,
                mag: 0.5
            }
        );
    }

    #[test]
    fn relative_turn_up_is_auto() {
        assert_eq!(
            cmd("turn up the office"),
            Command::Relative {
                target: Target::Named("office".into()),
                domain: RelDomain::Auto,
                up: true,
                mag: 0.5
            }
        );
    }

    #[test]
    fn scaled_is_percent_of_current() {
        assert_eq!(scaled(80, false, 0.5), 40); // dim: halve
        assert_eq!(scaled(20, false, 0.5), 10);
        assert_eq!(scaled(40, true, 0.5), 60); // brighten: +50%
        assert_eq!(scaled(0, true, 0.5), 50); // up from off → base
        assert_eq!(scaled(2, false, 0.5), 1); // clamps ≥ 1
    }

    #[test]
    fn everything_target() {
        assert_eq!(
            cmd("turn off everything"),
            Command::Power {
                target: Target::Everywhere,
                lights_only: false,
                on: false
            }
        );
    }

    #[test]
    fn compound_splits_into_clauses() {
        let clauses = parse("turn off the office and pause the kitchen");
        assert_eq!(clauses.len(), 2);
        assert!(matches!(
            &clauses[0],
            Clause::Cmd {
                command: Command::Power { on: false, .. },
                ..
            }
        ));
        assert!(matches!(
            &clauses[1],
            Clause::Cmd {
                command: Command::Transport {
                    cmd: TransportCmd::Pause,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn play_with_content_is_not_transport() {
        // "play X in Y" is play-favorite (fast-follow), left unparsed for now.
        assert!(matches!(
            parse("play jazz in the office").into_iter().next(),
            Some(Clause::Unparsed { .. })
        ));
    }

    #[test]
    fn cancel_is_noop() {
        assert!(matches!(
            parse("never mind").into_iter().next(),
            Some(Clause::Noop { .. })
        ));
    }

    #[test]
    fn gibberish_is_unparsed() {
        assert!(matches!(
            parse("flibbertigibbet the wozzle").into_iter().next(),
            Some(Clause::Unparsed { .. })
        ));
    }

    #[tokio::test]
    async fn transcribe_posts_multipart_and_returns_text() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "text": "turn off the office" })),
            )
            .mount(&server)
            .await;

        let ep = AiEndpoint {
            base_url: server.uri(),
            model: "whisper-1".into(),
            api_key: None,
        };
        let text = transcribe(&ep, "clip.wav".into(), b"RIFFfake".to_vec())
            .await
            .unwrap();
        assert_eq!(text, "turn off the office");

        // The server received a multipart POST carrying our file part.
        let reqs = server.received_requests().await.unwrap();
        let ct = reqs[0]
            .headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.starts_with("multipart/form-data"), "got: {ct}");
    }

    #[tokio::test]
    async fn transcribe_errors_on_empty_text() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "text": "  " })),
            )
            .mount(&server)
            .await;
        let ep = AiEndpoint {
            base_url: server.uri(),
            model: "whisper-1".into(),
            api_key: None,
        };
        assert!(
            transcribe(&ep, "c.wav".into(), vec![1, 2, 3])
                .await
                .is_err()
        );
    }
}
