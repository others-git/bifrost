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

## Milestone 10 — Audio device support ✅ DONE

Shipped: `AudioProvider` trait + parallel audio factory map in `ProviderRegistry`
(`models/audio.rs`, `providers/mod.rs`); Onkyo eISCP provider (codec, PWR/MVL/AMT/SLI,
NTC transport, NSV service selection incl. Spotify, NTI/NAT/NAL/NST metadata, loopback
mock tests); Sonos UPnP provider (seed-host topology fan-out, RenderingControl volume/mute,
AVTransport transport + DIDL metadata, wiremock tests); migration 0012 `audio_devices`;
`/api/audio/*` + `/api/v1/audio/*` routes (session + Bearer, documented in API.md);
shared `/api/providers/{id}/discover` routes audio types; Settings add-provider form
handles audio types via the existing schema flow; Dashboard Audio section (power toggle,
volume, mute, transport, now-playing, 15 s live poll). 133 lib + 88 integration tests.

**Follow-ups (all shipped in v0.7.0):**
- [x] Onkyo persistent connection + unsolicited-push subscription → `/api/events`
  `audio_state` frames (`AudioPushManager`, `AudioConnectionMode::Push`,
  `event_stream` on the trait; dashboard updates instantly, 30 s fallback poll)
- [x] Floor Plan room ↔ audio link: migration 0013 `room_audio`,
  `PUT /api/rooms/{id}/audio`, ♪ volume/mute strip on each room card,
  `audio_device_id` in room listings (session + v1 + MCP `get_home_state`)
- [x] Onkyo zone 2: `ZoneCodes` family (ZPW/ZVL/ZMT/SLZ, NTZ transport),
  ZPW probe in discovery, per-zone push-event routing
- [x] Sonos groups as `zone` devices: `parse_topology` extracts multi-member
  groups; `group:<coordinator>` devices control GroupRenderingControl volume
  and group-wide transport
- [ ] Sonos SSDP discovery without a seed IP (deferred — seed-host fan-out
  covers whole households already)

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

## Milestone 11 — Companion MCP server (`bifrost-mcp`, separate repo) ✅ v0.1.0

Built at `/mnt/d/REPOS/bifrost-mcp` (local git repo — **no GitHub remote yet**; create one
and push when ready). TypeScript stdio MCP server (`@modelcontextprotocol/sdk` 1.29), verified
end-to-end against a live hub. Tools: `get_home_state`, `list_lights`, `set_light`, `set_room`,
`apply_scene`, `apply_scene_all`, `save_scene_from_room`, `set_audio`, `get_audio_state`.
Rooms/scenes resolvable by name (LLM ergonomics); hex → CIE xy uses Bifrost's own matrix.

**The MCP tool roadmap — current tools and targets — is maintained in
[MCP.md](MCP.md)** (kept in this repo so tool targets stay in sync with the
`/api/v1` endpoints they wrap). `bifrost-mcp` itself is edited from its own repo.

### Original design notes

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

## Milestone 12 — Audio favorites & music services

The audio stack can switch a device *to* a service (Onkyo `source: "spotify"`)
and drive transport, but can't pick **what** plays. Two tiers close that gap.

### Tier 1 — Favorites/presets ✅ DONE

Play what the user already saved in the Sonos/Onkyo app — no accounts, no
search, reuse the local protocols.

Shipped: `AudioProvider::list_favorites` / `play_favorite` (default no-op);
`AudioFavorite` model; `AudioCapabilities.favorites` (serde-default, so old
cached state still deserializes); **Sonos** implementation via ContentDirectory
`Browse FV:2` (parse DIDL items) + play by reference — `SetAVTransportURI`+`Play`
for streams, queue path (`RemoveAllTracksFromQueue`→`AddURIToQueue`→play the
queue) for containers/playlists; reuses the existing `soap`/`topology`/
`find_target`/`transport` helpers. Session + Bearer routes
(`GET …/favorites`, `POST …/favorites/play` — id in body, since Sonos ids hold
slashes); Audio-page favorites list (lazy-loaded); wiremock provider tests +
API tests; documented in API.md.

- [x] Trait methods + `AudioFavorite` + `favorites` capability
- [x] Sonos favorites (browse + play, container vs stream)
- [x] Session + v1 routes, frontend, docs, tests
- [ ] **MCP `list_audio_favorites` / `play_audio_favorite`** — the v1 endpoints
  exist; the tools belong to the separate bifrost-mcp repo. Tracked as a target
  in [MCP.md](MCP.md), built when that project is next touched.
- [ ] **Onkyo NET presets — deferred.** eISCP exposes service *selection*
  (`NSV`) but not preset *enumeration*; listing saved presets needs the
  receiver's undocumented HTTP API. Onkyo reports no favorites for now.

### Tier 2 — Real Spotify (and friends) — NOT STARTED

"Search Spotify, pick a track/playlist, play it here." A new **music-service**
domain that *targets* an audio device, independent of Sonos/Onkyo. Per-service.

- [ ] OAuth (authorization-code + refresh) — a new credential model beyond the
  current static-field form: a registered Spotify app, a `/oauth/callback`
  route, encrypted token storage with refresh. Requires Spotify Premium.
- [ ] Search/browse — a content-picker UI (meaningfully more than toggles).
- [ ] Playback targeting via **Spotify Connect**: list Connect devices (Sonos
  and many receivers expose themselves) and transfer/start playback on one.
- [ ] Generalise: the same shape for Tidal/Apple Music later, each its own
  OAuth + API integration.

Tier 3 (constructing DIDL URIs for arbitrary tracks / scraping Onkyo's NLA menu)
stays out of scope — brittle and firmware-dependent.

---

## Milestone 13 — Audio providers' rooms wrap into Bifrost Rooms ✅ DONE

Audio providers expose their own room/zone abstraction (each Sonos player carries
its ZoneName); these now wrap into Bifrost Rooms through the **same**
`provider_groups` + `room_links` machinery as light providers, and the
provider-card button is unified to **"Sync"** for both domains.

Shipped: `AudioProvider::discover_groups` (default empty) with a **Sonos** impl
(one group per visible player — `provider_group_id = uuid`, `name = ZoneName`);
migration 0015 `provider_group_audio_devices` (audio analog of
`provider_group_lights`); `sync_groups` generalised — branches provider build +
member table on `is_known_audio`, shares the mirror-upsert / rename-follow /
link-or-create-room / prune logic. A room's `audio_device_id` resolves to a
linked audio group's device (manual `room_audio` still overrides), via a shared
`room_audio_device_id` helper used by both the session and v1 room listings.
`/api/provider-groups` and room `links` now carry `domain` (+ `audio_device_ids`);
the Room editor lists audio rooms/zones as linkable `♪` entries. wiremock provider
test + API sync test.

- [ ] Onkyo room/zone wrapping — trait method left default-empty (its
  `main`/`zone2` names aren't room names); add when there's a real need.

---

## Milestone 14 — Rooms aggregate multiple audio devices + volume fan-out

A Room can contain **any number of audio devices** (e.g. two Sonos in one
physical office). Room volume/mute **fans out** to all audio members instead of
a single linked device. Because a given percentage is not the same loudness on
every speaker, add **per-device, per-room volume offsets** (human-ear
calibration): room→20% sets device A to 20% and device B to 20%+offset.

- [ ] Model: room ↔ multiple audio devices — replace the single `room_audio`
  row with a room↔audio membership table; the synced audio provider-group link
  feeds it.
- [ ] Per-device-per-room offset (signed %), clamped to 0–100 after applying.
- [ ] Room volume/mute command fans out to all audio members with offsets applied.
- [ ] UI: room volume control + per-device offset calibration in the room editor.

## Milestone 15 — Scenes capture full device state (audio-aware)

Make **save-scene** future-proof: snapshot *all* member device state, not just
lights. For audio, capture the **selected source/playlist/station** (e.g. a
Spotify radio station) — explicitly **ignore** the transient now-playing track.
Applying a scene restores lights + audio source/volume together.

- [ ] Extend the scene snapshot to audio device state (selected source/content,
  volume, mute; skip now-playing track).
- [ ] Capture the selected provider content reference where the protocol exposes
  it (Onkyo NET service; Sonos favorite / queue origin).
- [ ] Apply path restores audio state alongside lights.

## Milestone 16 — Mobile / responsive web UI + PWA ✅ DONE

One responsive SPA (no separate native app); adapts by viewport via the
`useViewport()` hook (`frontend/src/useViewport.ts`) — `isMobile` ≤640px is the
switch, **tablets get the desktop layout**. Installable as a PWA.

- [x] `useViewport()` hook + breakpoints.
- [x] Mobile nav: bottom tab bar + slim top bar (`App.tsx`); sidebar on desktop/tablet.
- [x] PWA: `manifest.webmanifest`, theme-color + apple meta, `sw.js` (network-first
  HTML / cache-first assets, skips `/api`), registered in `main.tsx` (prod only).
- [x] Fly-outs (`LightEditor`/`AudioEditor`) become bottom sheets on phones
  (`components/sheet.ts`); responsive page padding.
- [x] **Floor Plan hidden on mobile for now** — nav button filtered + page shows a
  "available on a larger screen" notice (`App.tsx`). The full mobile view/control
  mode is deferred to M17.
- [x] Compact audio cards on phones (2-up grid, `compact` mode on `AudioControls`).
- [x] Compact Dashboard on phones: light cards go 2-up with smaller padding/fonts;
  tighter room-box headers, gaps, and grid (`RoomBox`/`LightCard`).
- [x] Stack wide button rows on phones (Settings provider cards: name/status on
  top, action buttons wrap below).
- [x] Tap targets on phones: enlarged range-slider thumbs (`index.html` media
  query); on/off toggles are already 44px tall. (Compact audio transport stays
  small by design for the 2-up cards.)
- [x] PWA icons: rasterized `icon-192.png` / `icon-512.png` (any) + `maskable-512.png`
  (maskable, content pulled into the safe zone); PNG `apple-touch-icon`.

## Milestone 17 — Floor plan on mobile (view/control + touch drafting) (deferred)

Currently the Floor Plan is hidden on phones (M16). This milestone brings it to
mobile: first a **view/control** mode (tap a device → bottom-sheet controls,
pinch-to-zoom + two-finger pan, editor rail hidden), then full **touch drafting**
(tiles/walls/placement/rooms) with gesture disambiguation (draw vs pan vs pinch).

- [ ] Mobile view/control mode + pinch/pan gestures.
- [ ] Touch drafting toolset.

## Milestone 18 — Matter device support (later) — NOT STARTED

Support **Matter** (the CSA's local, IP-based smart-home interop standard) so
Bifrost can control Matter devices natively. Strategically important — it's where
the ecosystem is heading — and a strong fit for Bifrost's local-first,
reliability-focused, provider-pluggable model.

**Target:** Bifrost acts as a Matter **controller / commissioner** — onboards
devices onto its fabric and controls them. Matter lights/plugs map cleanly onto
the existing `LightProvider` model (On/Off, Level Control, Color Control clusters
→ on / brightness / color). Audio is out of scope (Matter has no audio device
class; Matter Casting is a separate thing).

**Hard parts / decisions (spike before committing):**
- **Rust controller maturity is the key risk.** `rs-matter` (project-chip) is
  primarily for building Matter *devices*; controller/commissioner support is
  early. A full controller may instead need the official C++ Matter SDK (chip)
  via FFI or a **sidecar process** (e.g. wrapping `chip-tool`), which complicates
  the "one static musl binary" ethos. Choose: pure-Rust vs. sidecar vs. embed.
- **Commissioning UX** — a new onboarding flow beyond the credential form: enter
  a pairing code / scan the QR, pick the network (Wi-Fi creds or Thread dataset).
  Persist fabric/operational credentials encrypted, like all creds.
- **Thread dependency** — many Matter devices use Thread, which needs a **Thread
  Border Router** (Apple/Google/dedicated). Bifrost won't be one initially:
  **start with Wi-Fi/Ethernet Matter devices**; document Thread devices as
  requiring an existing border router.
- **Networking** — Matter needs IPv6 + mDNS reachability; note the Docker/
  host-networking implications (same class of constraint as LAN auto-detect).

**Initial scope (when picked up):**
- [ ] Spike: evaluate `rs-matter` controller support vs. a chip sidecar; pick the path.
- [ ] `MatterProvider` (`LightProvider`) for On/Off + dimmable + color devices;
  registered in `ProviderRegistry` like any other provider.
- [ ] Commissioning flow (pairing code/QR, Wi-Fi onboarding) + encrypted fabric
  credential storage.
- [ ] Map Matter clusters ↔ `LightState`; provider + factory-build tests.
- [ ] Docs: prerequisites (IPv6/mDNS; Thread border router for Thread devices).

**Deferred within Matter:** hosting a Thread Border Router; device types beyond
lights/plugs (sensors, locks, thermostats — would need new Bifrost domains);
Bifrost *as a Matter bridge* (exposing its own devices to other Matter controllers).

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
