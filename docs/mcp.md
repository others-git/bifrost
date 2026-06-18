# Bifrost MCP — embedded tool surface

Bifrost embeds a [Model Context Protocol](https://modelcontextprotocol.io)
server **inside the Bifrost binary** so an AI assistant can control the whole
home in natural language. It is the **third surface over the shared service
layer**, alongside the session API (`/api/*`) and the public API (`/api/v1/*`) —
the tools call the same `api::{lights,audio,rooms,scenes}` service
functions directly (no HTTP hop, no Bearer round-trip), so they can't drift from
the REST surface and reuse the real color math (`models::Color`).

- **Where:** `src/api/mcp.rs`, mounted at `/mcp` in `build_app` (`src/lib.rs`).
- **Transport:** **Streamable HTTP only** (stateless, JSON responses). There is
  no stdio server; stdio-only clients bridge to `/mcp` with the standard
  `mcp-remote` shim. See [MCP-Add rationale](#why-embedded) below.
- **This file is the source of truth for what the server exposes** — kept here,
  alongside the API it shares, so tool targets can't drift from the endpoints.
  When you add or change a service function an assistant should reach, update the
  mapping below.

Ultimate goal: AI-driven whole-home control. Target client: **Whisperr** (the
voice/LLM pipeline). Tools are designed for natural-language ergonomics — resolve
rooms, scenes, devices, and favorites by **name**, not just id.

## Conventions

- **Auth:** a Bifrost API key (`bfr_…`, minted in Settings), sent as
  `Authorization: Bearer`. Identical to `/api/v1`: only the SHA-256 hash is
  stored, revocation is immediate, a missing/invalid key returns `401` before any
  MCP processing.
- **Name resolution:** tools accept an id **or** a case-insensitive name/
  substring. Resolution order is exact id → exact name → name substring; on no
  match (or an ambiguous substring) the tool returns an error that **lists the
  valid options** so the assistant can self-correct. Implemented once in
  `mcp::resolve`.
- **Sparse writes:** mirror the service layer — only the fields the user named
  are sent. `set_light`/`set_room` default `on` to true (asking for color or
  brightness implies the light should be lit).
- **Error shape:** recoverable problems (wrong name, unreachable device, bad
  argument) come back as a **tool result with `isError: true`** and a readable
  message; only infrastructure failures (DB errors) are protocol-level errors.

## Tools — shipped (embedded)

All tools below are served natively from `src/api/mcp.rs`.

| Tool | Shared service fn(s) | Notes |
|---|---|---|
| `get_home_state` | `list_public_rooms` + `list_all_lights` + `list_all_scenes` + `list_all_devices` + `list_all_power_devices` | One-call context snapshot (rooms with member ids, lights, scenes, audio devices, power devices) |
| `list_lights` | `list_all_lights` | each light carries `capabilities.effects` (the dynamic-effect names it supports) |
| `set_light` | `apply_light_state` | light by id/name; hex → CIE xy via `Color::from_rgb`; optional `effect` (a name from `capabilities.effects`, e.g. "candle"/"breathe"/"Sunrise") supersedes color/temp |
| `set_room` | `effective_members` + `apply_room_state` | room by id/name; fan-out to member lights |
| `activate_scene` | `apply_scene_entries` | scene by id/name; re-applies the captured full state (color/temp/effect + power). A Room Scene restores its room, a Home Scene the whole house |
| `save_room_scene` | `capture_scene(room_id)` | snapshot a room's current full state (each light's color/temp/effect + power) as a new Room Scene |
| `save_home_scene` | `capture_scene(None)` | snapshot the whole home's current state as a new Home Scene |
| `set_audio` | `apply_audio_command` | device by id/name; power/volume/mute/source/transport |
| `get_audio_state` | `get_device_live` | live read incl. now-playing |
| `list_audio_favorites` | `list_device_favorites` | Sonos Favorites; empty for Onkyo |
| `play_audio_favorite` | `list_device_favorites` + `play_device_favorite` | resolve `favorite` (id/title/substring) then play. *"play my jazz favorite in the office."* |
| `group_speakers` | `group_devices` (per member) | join speakers under a coordinator. *"play the kitchen and living room together."* |
| `ungroup_speaker` | `ungroup_device` | remove a speaker from its synced group |
| `bind_receiver` | `set_audio_receiver` | bind a source (TV/streamer) to a receiver that owns its volume + the input to switch to on power-on; omit `receiver` to unbind. *"route the living room TV's sound through the AV receiver on the Game input."* |
| `list_power_devices` | `list_all_power_devices` | switches / plugs / fans / toggles with on/off state + kind |
| `set_power` | `apply_power_state` | turn a power device on/off, by id or name. *"turn off the porch switch."* |
| `list_remotes` | `list_remotes` | TVs / streamers with on state + current foreground app |
| `press_remote_key` | `apply_remote_command` (key) | press a canonical key (up/down/…/select/back/home/play_pause/power), by id or name. *"go back on the bedroom TV."* |
| `launch_app` | `apply_remote_command` (launch_app) | open an app by Play Store package id or deep-link URL. *"open Netflix on the bedroom TV."* |

## Target tools — not yet built

| Tool | Maps to | When |
|---|---|---|
| `list_audio_devices` | `list_all_devices` | If a standalone audio list is wanted beyond `get_home_state`. |
| Tier-2 music search/play | a future music-service API | After PLAN.md Milestone 12.2 (Spotify OAuth + Connect) lands. |
| Audio-in-scenes awareness | scene snapshot incl. audio source/volume | After PLAN.md Milestone 15. |

## Maintenance checklist

When the service layer (or `/api/v1`) changes:

1. Add/adjust the tool in `src/api/mcp.rs` (tool fn + `schemars` param struct),
   backed by the shared service fn — never a forked control path.
2. Update the **mapping tables** above (shipped vs target).
3. Cover it per CLAUDE.md: a tool-level test in `tests/api.rs` driving `/mcp`
   (the service fns themselves are already covered), plus unit tests for any new
   resolution/parsing helper.
4. Cross-link from [the roadmap](https://github.com/others-git/bifrost/blob/main/PLAN.md) if it's part of a milestone, and note the
   endpoint in [the API reference](api.md) if a new REST route backs it.

## Why embedded

Bifrost's founding ethos is **one binary, one SQLite file, one Docker image**. An
embedded MCP server keeps that: no separate repo, no Node runtime, no parallel
deploy. CLAUDE.md already mandates that the session and public APIs delegate to
the **same shared service functions** so they can't drift; the MCP surface is
just a third caller of those functions, which is why it reuses the real Rust
color math instead of re-implementing hex→CIE xy.

**Transport choice — Streamable HTTP only.** MCP has two transports. *stdio* has
the client spawn a subprocess over stdin/stdout — a long-running daemon can't own
per-client stdin/stdout, so "embedding" stdio just means shipping a second
process. *Streamable HTTP* is a single endpoint (one path, POST + optional SSE
upgrade) that mounts as just another Axum route — one server, many clients, no
subprocess. A network-reachable `/mcp` (behind Tailscale/VPN, per Bifrost's
access model) needs no per-machine install. So Bifrost serves **only** Streamable
HTTP; the rare stdio-only client uses the off-the-shelf `mcp-remote` bridge
rather than us maintaining a stdio artifact.

**Tooling.** Built on **`rmcp`** (the official `modelcontextprotocol/rust-sdk`),
which provides the Axum-mountable `StreamableHttpService`. Run in stateless +
JSON-response mode: every tool call is an independent request/response, so there
is no `Mcp-Session-Id` bookkeeping. The endpoint is Bearer-gated (not protected
by `rmcp`'s default localhost Host-allowlist, which is disabled since Bifrost is
reached by LAN/Tailscale IP); the API key is the security boundary.

### Fate of the old `bifrost-mcp` TS repo

Superseded. The earlier plan was a separate TypeScript stdio server wrapping
`/api/v1` over the network (re-implementing the color math in TS). That
separate-repo framing of Milestone 11 is **retired** — MCP is now a first-class
Bifrost surface, not a parallel client.
