# Bifrost — Implementation Plan

## Goals

A self-hosted Rust smart home hub that is:
- **More reliable than Home Assistant** for Hue — explicit SSE reconnect, polling fallback, no silent drop
- **Minimal surface area** — one binary, one SQLite file, one Docker image
- **Extensible without code surgery** — new providers plug in via `ProviderFactory` only
- **Full home control via API** — the foundation for the companion `bifrost-mcp` project

---

## Shipped (Milestones 0–8) ✅

| Milestone | What shipped |
|---|---|
| 0 — Foundation | Rust crate, SQLite/sqlx, AES-256-GCM creds, ProviderRegistry, Hue + Govee providers, Alpine Docker |
| 1 — Server completeness | First-run setup, HueConnectionManager wired, SSE→DB state sync, health endpoint |
| 2 — Frontend | React/Vite SPA, Setup/Login/Dashboard/Settings pages |
| 3 — Real-time push | `/api/events` SSE, frontend live state updates |
| 4 — More providers | WLED, Tasmota, Shelly Gen1; schema migration removes type constraint |
| 4.5 — Functional Hue + Govee | Link-button pairing UI, PollingManager, color picker, auto-discovery |
| 5 — Production readiness | Global palette scenes, rooms/groups, Docker Compose, health v2 |
| 6 — LIFX | Cloud REST provider (LAN/UDP deferred) |
| 7 — Floor Plan | Tile editor, wall/room painting, light placement, live glow from SSE |
| 8 — UI overhaul | Shared LightEditor popover, in-app dialogs, Lights page, strip corners |
| 8.x — Polish | Global scenes, Govee LAN (UDP), floor-plan save perf (WAL + transactions), RoomPicker, aurora navbar, electric toggles |

---

## Milestone 9 — Public API for other apps ✅ DONE

- [x] **API keys** — minted in Settings (`bfr_` + 64 hex), SHA-256 hash stored, full key shown
  once, listable/revocable, `Authorization: Bearer` auth (`src/api/apikeys.rs`).
- [x] **`/api/v1` surface** — lights (list/get/set), rooms (list/state/apply-scene), scenes
  (list/create/from-room/delete); thin handlers delegating to the session API's service fns
  (`src/api/v1.rs`).
- [x] **Documented surface** — `API.md`: auth, data shapes, every endpoint, status codes.
- [x] Tests: key management 401s, mint-once + prefix listing, revocation kills access,
  v1 401 (missing + bogus key), light read/write, rooms + scenes full flow.

---

## Milestone 10 — Audio device support ✅ CORE DONE

Shipped: `AudioProvider` trait + parallel audio factory map in `ProviderRegistry`
(`models/audio.rs`, `providers/mod.rs`); Onkyo eISCP provider (codec, PWR/MVL/AMT/SLI,
NTC transport, NSV service selection incl. Spotify, NTI/NAT/NAL/NST metadata, loopback
mock tests); Sonos UPnP provider (seed-host topology fan-out, RenderingControl volume/mute,
AVTransport transport + DIDL metadata, wiremock tests); migration 0012 `audio_devices`;
`/api/audio/*` + `/api/v1/audio/*` routes (session + Bearer, documented in API.md);
shared `/api/providers/{id}/discover` routes audio types; Settings add-provider form
handles audio types via the existing schema flow; Dashboard Audio section (power toggle,
volume, mute, transport, now-playing, 15 s live poll). 133 lib + 88 integration tests.

**Remaining (follow-ups):**
- [ ] Onkyo persistent connection + unsolicited-push subscription → `/api/events`
  (`audio_state` SSE frames) instead of the 15 s dashboard poll
- [ ] Floor Plan: link a room to an audio zone (volume/mute in the room popover)
- [ ] Sonos SSDP discovery without a seed IP; Sonos groups as `zone` devices
- [ ] Onkyo multi-zone (zone2/zone3 as additional devices)

### Original design notes (kept for the follow-ups)

### 10.1 AudioProvider trait

Define a trait analogous to `LightProvider`:

```rust
trait AudioProvider: Send + Sync {
    async fn discover(&self) -> Result<Vec<AudioDevice>>;
    async fn get_state(&self, device_id: &str) -> Result<AudioState>;
    async fn set_state(&self, device_id: &str, cmd: AudioCommand) -> Result<()>;
    // Optional: for receivers that push unsolicited updates
    fn subscribe(&self) -> Option<broadcast::Receiver<AudioEvent>> { None }
}

struct AudioDevice { id, name, provider_id, kind: AudioDeviceKind }
enum AudioDeviceKind { Receiver, Speaker, Zone }

struct AudioState {
    power: bool,
    volume: u8,       // 0–100
    mute: bool,
    source: Option<String>,
    now_playing: Option<NowPlaying>,
}

struct NowPlaying { title, artist, album, elapsed_secs, total_secs, play_state: PlayState }
enum PlayState { Playing, Paused, Stopped }

struct AudioCommand {
    power: Option<bool>,
    volume: Option<u8>,
    mute: Option<bool>,
    source: Option<String>,
    transport: Option<TransportCmd>,
}
enum TransportCmd { Play, Pause, Stop, Next, Prev, Toggle }
```

Add `AudioProviderFactory` and a separate `AudioProviderRegistry` (same pattern as lights).
Persist credentials encrypted (AES-256-GCM).

### 10.2 Onkyo / Integra provider

**Protocol:** eISCP (Ethernet Integra Serial Control Protocol) over TCP port 60128.

#### eISCP framing (exact)

Every message is a 16-byte header + UTF-8 ISCP payload:

```
Bytes   Value           Meaning
0–3     b"ISCP"         magic
4–7     16 (u32 BE)     header size (always 16)
8–11    N (u32 BE)      data size (byte length of payload)
12      0x01            version
13–15   0x00 0x00 0x00  reserved
16+     !1<CMD><DATA>\r ISCP payload
```

Payload format: `!` start, `1` unit type (receiver), 3-char command code, variable data, `\r` terminator.
Receiving: strip trailing `\x1a` (EOF marker) and any `\r` / `\n`.
Query any command by appending `QSTN` as the data (e.g. `!1MVLQSTN\r`).

#### Bi-directional (critical design note)

The receiver **pushes unsolicited updates** on the same persistent TCP connection whenever state
changes — physical remote, track transitions, volume knob, input switching. There is no subscribe
mechanism; there is also **no way to distinguish a response from an unsolicited push** — both are
identical packets. Implementation must maintain a background reader task draining the socket
continuously. A single `Arc<Mutex<TcpStream>>` does not work; use separate read/write halves
(`split()`) with the reader on a dedicated task forwarding events to a `broadcast::Sender<AudioEvent>`.

#### Core audio commands

| Code | Data | Meaning |
|------|------|---------|
| `PWR` | `01` / `00` / `QSTN` | Power on/standby/query |
| `MVL` | `00`–`64` hex / `UP` / `DOWN` / `QSTN` | Master volume (hex, 0x00–0x64 = 0–100) |
| `AMT` | `00` / `01` / `TG` / `QSTN` | Mute off/on/toggle/query |
| `SLI` | `2B` / `29` / `2A` + others | Input selector; `2B` = NET (network services) |

#### NET / streaming transport

Use `NTZ` for modern receivers (Spotify built-in); `NTC` for older Net-Tune models. Commands identical:

| Data | Action |
|------|--------|
| `PLAY` / `STOP` / `PAUSE` / `P/P` | Playback control / toggle |
| `TRUP` / `TRDN` | Next / previous track |
| `CHUP` / `CHDN` | Channel up/down (internet radio) |
| `REPEAT` / `RANDOM` / `REP/SHF` | Repeat / shuffle |
| `UP` / `DOWN` / `LEFT` / `RIGHT` / `SELECT` / `RETURN` | Menu navigation |

#### Service selection (NSV)

`NSV` + 2-char hex service code selects a streaming service:

| Code | Service |
|------|---------|
| `00` | Music Server (DLNA) |
| `0A` | **Spotify** |
| `0E` | TuneIn Radio |
| `12` | Deezer |
| `13` | iHeartRadio |
| `18` | AirPlay |
| `19` | TIDAL |
| `F2` | Internet Radio |

To activate Spotify: send `SLI2B` (select NET input), then `NSV0A0` (select Spotify, no account prompt).

#### Metadata (read-only, support QSTN; also pushed unsolicited on track change)

| Code | Returns |
|------|---------|
| `NTI` | Track title (UTF-8, 64 chars max) |
| `NAT` | Artist name |
| `NAL` | Album name |
| `NTM` | Elapsed/total time `mm:ss/mm:ss` |
| `NTR` | Current track / total tracks `cccc/tttt` |
| `NST` | 3-char play state `prs`: `p`=play state (S/P/p/F/R/E), `r`=repeat, `s`=shuffle |
| `NRI` | XML blob of receiver capabilities (model, firmware, supported services) — query once on connect |

#### Implementation notes

- One persistent TCP connection per receiver; reconnect with backoff on drop.
- On connect: send `NRIQSTN` to learn capabilities; send `PWRQSTN`, `MVLQSTN`, `AMTQSTN`,
  `SLIQSTN`, `NSTQSTN`, `NTIQSTN`, `NATQSTN`, `NALQSTN` to seed initial state.
- Credentials: `host` (required), `port` (default 60128).
- Provider type string: `"onkyo"`.
- Discovery: UDP broadcast on port 60128, ISCP discovery packet `!xECNQSTN\r`
  (header with unit type `x` for broadcast), parse `ECN` response for IP + model.

#### Tests

Loopback TCP listener in the test that speaks eISCP framing, records received commands,
and sends scripted responses including unsolicited pushes. Mirror the govee-lan pattern
(tokio task as mock device).

### 10.3 Sonos provider

**Protocol:** Sonos S2 local HTTP API (REST/JSON, no cloud required).

- **Discovery:** SSDP multicast on `239.255.255.250:1900`,
  filter `ST: urn:schemas-upnp-org:device:ZonePlayer:1`, parse `LOCATION` header for player IP.
- **Household / group model:** `GET http://<ip>:1400/api/v1/households` → groups → players.
  Expose groups as "zones" (the unit users control); players are members.
- **State:** `GET /api/v1/players/{id}/playerVolume`, `/api/v1/players/{id}/playbackStatus`
- **Control:** `POST /api/v1/players/{id}/playerVolume` `{volume, muted}`;
  `POST /api/v1/players/{id}/playback/play|pause|skip{Next,Previous}|togglePlayPause`
- **Now playing:** `GET /api/v1/players/{id}/playbackMetadata` → title, artist, album, service
- **Group volume:** `GET/POST /api/v1/groups/{id}/groupVolume`
- Credentials: none required (pure LAN); optional `bind_addr`. Provider type: `"sonos"`.
- Tests: wiremock against the JSON endpoints; mock SSDP via loopback UDP.

### 10.4 Audio API routes

Mirror the lights API shape so the MCP layer treats audio and lights uniformly:

```
GET    /api/audio/devices              list all audio devices with current state
GET    /api/audio/devices/:id          single device state
PUT    /api/audio/devices/:id/state    set power / volume / mute / source / transport
GET    /api/audio/zones                list zones (Sonos groups, Onkyo zones)
PUT    /api/audio/zones/:id/state      zone-level volume / mute / transport
```

Auth: session cookie or API key (same as all other routes). All routes return 401 if unauthenticated.

### 10.5 Audio on the Dashboard / Floor Plan

- **Dashboard:** audio zone cards alongside room (light) cards — power toggle, volume slider,
  now-playing metadata line, source indicator.
- **Floor Plan:** room controller optionally linked to an audio zone (volume knob + mute toggle
  in the room popover).
- **Settings:** add/remove audio providers, trigger discovery, show connection state.

---

## Milestone 11 — Companion MCP server (`bifrost-mcp`, separate repo)

A Model Context Protocol server wrapping the Bifrost REST API as MCP tools, so an AI assistant
(Claude, Whisperr + LLM pipeline, etc.) can control the whole home through natural language.
Requires Milestone 9 (API keys) to be complete first.

**Planned tools:**

| Tool | Description |
|------|-------------|
| `get_home_state` | Snapshot of all rooms + audio zones (single call for context) |
| `list_rooms` | Rooms with current on/off, scene, linked audio zone |
| `list_lights` | All lights with state |
| `list_audio_devices` | All audio devices/zones with state + now-playing |
| `set_light` | `{ light_id, on?, brightness?, color? }` |
| `set_room_scene` | `{ room_id, scene_name_or_id }` |
| `set_room_lights` | `{ room_id, on?, brightness?, color? }` — whole-room control |
| `set_audio` | `{ device_or_zone_id, power?, volume?, mute?, source?, transport? }` |
| `apply_scene_all` | Apply a named scene to all rooms simultaneously |

**Auth:** Bifrost API key in the MCP server config (env var), sent as `Authorization: Bearer`.
Never stored in Bifrost DB in plaintext; the key hash lives in Bifrost, the key itself lives only
in the MCP config.

**Transport:** stdio MCP (compatible with Claude Desktop, `claude --mcp`, Whisperr, etc.)

**Language:** TypeScript with the `@modelcontextprotocol/sdk` package (separate git repo).

---

## Out of scope (for now)

- Multi-user / RBAC — single shared password + API keys is the design
- MQTT broker — would enable many devices but adds operational complexity
- Mobile app — the SPA is responsive; native apps are not planned
- Cloud relay / remote access — use Tailscale/VPN at the network layer
- Schedules / automations — stretch goal, cron-style SQLite triggers

---

## Key invariants (enforced by CLAUDE.md)

- Every public function and non-trivial helper has test coverage. `cargo test` must be green.
- New provider: wiremock/loopback tests before the code is considered done.
- New API route: happy path + unauthenticated-returns-401.
- Credentials encrypted with AES-256-GCM. Never stored in plaintext.
- `HueConnectionManager` is the only code that opens the bridge SSE stream.
- `ProviderRegistry` is the only place provider types are registered.
