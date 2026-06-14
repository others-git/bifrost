# Bifrost — Implementation Plan

## Active Milestone — Home Assistant as a high-class integration

Make HA a first-class integration that surfaces *real* devices cleanly and is
controllable by name (UI + voice/MCP). Recently landed: multi-domain HA (lights,
power, audio/TV), the **entity-registry primary-entity filter** (one-shot WS,
drops `config` sub-entities), **per-provider pruning** + global Discover/Sync/
Prune+Sync, source/app switching, device enable/disable, the Control page. **Up
next:** play named content on the TV (the voice north-star — `media_player.play_media`
vs delegating to HA Assist), then WebSocket `subscribe_events` push and HA
device-registry grouping. Open items live under *Open milestones* below; the
per-milestone detail is in **M19**.

## Goals

A self-hosted Rust smart home hub that is:
- **More reliable than Home Assistant** for Hue — explicit SSE reconnect, polling fallback, no silent drop
- **Minimal surface area** — one binary, one SQLite file, one Docker image
- **Extensible without code surgery** — new providers plug in via `ProviderFactory` only
- **Full home control via API** — and a natural-language surface via the embedded MCP server (`/mcp`)

---

## Completed milestones (one-liners)

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
| 9 — Public API | API keys (`bfr_`, SHA-256 hash, `Authorization: Bearer`); `/api/v1` lights/rooms/scenes as thin handlers over the session service fns; documented in `API.md` |
| 10 — Audio devices | `AudioProvider` trait + audio factory map; Onkyo eISCP (PWR/MVL/AMT/SLI, NET transport, NSV incl. Spotify, metadata, zone 2, persistent push → `/api/events`) + Sonos UPnP (topology fan-out, volume/mute, transport + DIDL, groups as `zone` devices); `/api/audio/*` + `/api/v1/audio/*`; Settings + Dashboard/Floor-Plan controls |
| 11 — Embedded MCP server | MCP is a **first-class Bifrost surface**, not a separate repo: `src/api/mcp.rs` serves a Streamable HTTP server at `/mcp` (Bearer-gated, same keys as `/api/v1`) via `rmcp`, with 13 tools (`get_home_state`, `set_light`, `set_room`, `apply_scene`/`apply_scene_all`, `save_scene_from_room`, `set_audio`, `get_audio_state`, `list_audio_favorites`/`play_audio_favorite`, `group_speakers`/`ungroup_speaker`) calling the shared service layer directly + name resolution. Streamable HTTP only (stdio clients bridge via `mcp-remote`); the old `bifrost-mcp` TS repo is retired. Catalogue in [MCP.md](MCP.md) |
| 12.1 — Audio favorites | `list_favorites`/`play_favorite` + `AudioFavorite`; Sonos via ContentDirectory `Browse FV:2` + play-by-reference (stream vs queue); session + v1 routes, Audio-page list |
| 13 — Audio rooms → Bifrost Rooms | `discover_groups` (Sonos, one group per player) wraps into the shared `provider_groups`/`room_links` machinery; provider-card button unified to **"Sync"** for both domains |
| 14 — Multi-device room audio | Migration 0018 `room_audio_devices`; room volume/mute **fans out** to all audio members with a per-room offset; `PUT /rooms/{id}/audio` + `/audio/state`; Floor-Plan member/offset panel |
| 16 — Mobile / PWA | Responsive SPA via `useViewport()` (`isMobile` ≤640px, tablets get desktop); bottom tab bar, bottom-sheet fly-outs, compact 2-up cards, enlarged tap targets; installable PWA; Floor Plan hidden on phones (→ M17) |
| 20 — Control page + device enable/disable | **Control** page (renamed from "Lights"): whole-home, **two-column** rooms on desktop (CSS columns; single column on mobile), each room a row of **glyph buttons** — one per member device (light/power/audio), type-glyph not name — each opening its **own fly-out** (`LightEditor` / `PowerFlyout` / `AudioEditor`) showing the full name. Shared `components/glyphs.tsx`. **Device enable/disable** across all domains: migration `0022` adds `enabled` to lights/audio_devices/power_devices; a disabled device stays tracked + a room member but gets **no commands** (control lookups skip it → 404) and drops from room control (effective members for lights; client-side on Control for power/audio). `PUT /api/{lights,audio/devices,power/devices}/{id}/enabled`; disable in each fly-out + the Devices page. |
| 19 — Home Assistant ("high-class" provider) | `providers::ha` — one adapter (`base_url` + long-lived token), shown under an **"Integration"** add-provider category, that surfaces multiple HA device domains from one provider row. **Lights**: `light.*` → Bifrost lights + **HA Areas → Rooms** via the shared Sync flow. **Power**: new lean `models::power` domain (switch/fan/plug → `PowerKind` glyph, on/off only) wired end-to-end on the backend — multi-domain registry (`register_power`), additive discover, `power_devices` table (migration 0019), `api::power` + `/api/v1/power` + MCP tools. REST poll (30s). HA `media_player`/audio still **on hold**. Remaining: power live-polling/frontend/room-membership. See [Milestone 19 detail](#milestone-19--home-assistant-high-class-provider) |

### Open follow-ups from shipped milestones

- [ ] **Sonos SSDP discovery without a seed IP** (M10, deferred — seed-host
  fan-out already covers whole households).
- [x] **MCP `list_audio_favorites` / `play_audio_favorite`** (M12) — shipped as
  embedded MCP tools in M11. Tracked in [MCP.md](MCP.md).
- [ ] **Onkyo NET presets** (M12, deferred) — eISCP exposes service *selection*
  (`NSV`) but not preset *enumeration*; needs the receiver's undocumented HTTP API.
- [ ] **Onkyo room/zone wrapping** (M13) — `discover_groups` left default-empty
  (its `main`/`zone2` names aren't room names); add when there's a real need.

---

## Milestone 19 — Home Assistant ("high-class" provider) — IN PROGRESS

One adapter (`providers::ha`) that surfaces *any* HA integration as Bifrost
devices and mirrors HA's structure into Bifrost — so Bifrost becomes a fast,
reliable control surface on top of HA's ~1000 integrations, while native Hue
stays direct. Added by `base_url` + a long-lived access token. **North star:
effectively any device HA can manage becomes controllable in Bifrost** — which is
why HA is its own **"Integration"** category in the add-provider UI
(`ProviderDomain::Integration`), not filed under a single device domain.

**Shipped:**
- [x] `light.*` entities → Bifrost lights (`LightProvider`): discover, get/set
  state (brightness %, RGB, color-temp via kelvin), reachability.
- [x] **HA Areas → Bifrost Rooms**: `discover_groups` renders the area→entities
  registry via `/api/template` and wraps each area (with light members) into the
  shared `provider_groups`/`room_links` machinery — imported by the **Sync** button.
- [x] Registered (functionally) on the light domain; **categorised as
  "Integration"** in the add-provider menu; 30s local REST poll; wiremock tests
  for the provider impl, area mapping, and factory build.

**More HA entity domains — the north star (effectively any HA device controllable):**

Design decision (DRY, split): strictly-on/off devices share **one generic
"power device" domain** (so a switch/plug/fan isn't "a light with empty
capabilities"), distinguished only by a `PowerKind` glyph; genuinely richer
devices (climate, covers, …) get their **own domain** so each domain's state
shape stays honest.

- [x] **`PowerDevice` domain foundation** (`models::power`): lean model (state is
  just `on`) + `PowerKind` glyph enum (Switch / Outlet / Fan / Toggle / Generic);
  `PowerProvider` trait; HA impl mapping `switch.*` / `fan.*` / `input_boolean.*`
  (control via the domain-agnostic `homeassistant.turn_on`/`off` service) and
  power-area `discover_groups`; model + wiremock tests.
- [x] **Multi-domain registry** ("one HA provider, many domains"): `register_power`
  + `power_factories` map + `is_known_power`/`build_power`; `HaPowerFactory`
  registered **alongside** `HaLightFactory` (same `"ha"` row). The
  `/api/providers/{id}/discover` handler is now **additive** — it discovers every
  domain a provider serves (lights ∪ power ∪ audio) and sums the counts.
- [x] **Power backend wired**: migration `0019_power_devices` (no `capabilities`
  column — the lean domain has none); `api::power` service layer; **session**
  (`/api/power/devices…`) + **`/api/v1/power/devices…`** routes (list / live get /
  set on-off); **MCP** tools `list_power_devices` + `set_power` (name-resolved),
  and power devices added to `get_home_state`. Registry/factory/discover/control
  covered by unit + wiremock + in-memory-SQLite integration tests.
- [x] **"Devices" page = full device inventory** (`frontend/src/pages/Devices.tsx`,
  nav entry): every device of every domain (lights, audio, power), grouped by
  domain, regardless of room membership — the **configuration** surface (not
  control). Each row shows the device's glyph, name/id, reachability dot, and an
  enable/disable control; power devices also keep a vertical on/off toggle
  (optimistic, reconciles on error). Earlier it was power-only; widening it back
  to all domains fixed the "Devices shows only switches" regression.
- [x] **Per-device glyph override** — migration `0024_device_glyph` adds a nullable
  `glyph` to lights/audio_devices/power_devices (NULL = type default). Shared
  `set_device_glyph` helper + `PUT /api/{lights,audio/devices,power/devices}/{id}/glyph`.
  Frontend: a single by-name glyph registry (`components/glyphs.tsx`, `Glyph` /
  `GLYPH_OPTIONS`, incl. a new `led_strip` glyph for switches that drive LED strip);
  the Devices page has a glyph picker per device, and the Control page + fly-outs
  render the **effective** glyph (`device.glyph ?? type default`).
- [x] **Trim to first-class providers**: `govee-lan` / `shelly` / `tasmota` / `wled`
  unregistered from `default_registry` (code kept on disk, dropped from the
  add-provider menu); first-class set is Hue, Govee (Cloud), Home Assistant, Onkyo,
  Sonos. `wled` stays as the generic mockable light in `tests/helpers.rs::test_registry`.
- [x] **Power devices as Room members + unified Rooms config**: migration
  `0020_room_power_devices`; `PUT /rooms/{id}/power`; `power_device_ids` added to
  the session and public (`/v1` + MCP) room shapes; merge moves power (and audio)
  membership. Frontend: the per-room **Lights/Audio buttons collapse into one
  "Devices" button** → a generic, grouped config panel (`RoomDevices.tsx`) —
  linked rooms/areas, lights, speakers (with offset), power — driven by a reusable
  membership section so future device classes drop in; mobile-friendly (stacked,
  scroll-capped lists).
- [x] **Multi-domain Area→Room sync**: `sync_groups` now gathers Areas across
  **every** domain a provider serves (lights ∪ power ∪ audio) and merges by area,
  so an HA Area with only switches/the TV still syncs into a Room (previously
  light-only, so areas without lights silently didn't sync). Migration
  `0021_provider_group_power`; `effective_power_member_ids` now unions direct +
  linked members; `provider-groups` listing reports members of all domains; merge
  also fixed to move audio membership. Wiremock test covers a power-only area.
- [ ] **Power: remaining wiring** — live polling into the event pipeline (state
  currently refreshes on a 30s poll + live GET, not pushed); **room-level control
  fan-out** to power members (room on/off drives lights+audio, not yet power).
  Provider-list `domain` label still shows "light" for HA (cosmetic).
- [ ] **Richer domains as their own type**, when their state surface justifies it:
  `climate.*` (setpoint/mode/current temp), `cover.*` (position), `lock.*`. Each
  is a new domain (model + control surface + UI), grouped where applicable.
- [ ] **WebSocket push** (`references/ha_websocket_api.md`): a `subscribe_events`
  push manager for `state_changed`, for instant updates instead of 30s polling.
  Needs a generic light push channel (today only Hue SSE + polling exist).
- [x] **Audio domain (media_player → TVs/speakers)**: `HaAudioFactory` registered
  via the multi-domain path, so HA `media_player.*` (the Android TV, speakers)
  surface as audio devices with power/volume/mute/source/transport on the Audio
  page + audio API/MCP. `all_types` deduped so HA stays one menu entry (Integration);
  `ui_domain` labels the provider card "Integration", not "Audio". Reads on demand.
  Wiremock + integration tests.
- [x] **Primary-entity filter (one-shot WebSocket entity registry)**: HA exposes a
  device's *settings* as extra entities (e.g. Sonos crossfade/loudness `switch.*`,
  a plug's LED-indicator) — `entity_category: config`/`diagnostic`. That's not in
  REST `/api/states`, so discovery now does a **cached one-shot WS** call
  (`config/entity_registry/list`, `tokio-tungstenite`) and surfaces only **primary**
  entities (no category, not disabled/hidden). Live-verified: 14 switches → 3 real;
  the TV stays. WS failure degrades to unfiltered. Findings in **`HA-API.md`**. Next:
  use the registry's `device_id` to group a device's entities (device-registry import).
- [x] **Per-provider pruning + global Discover/Sync/Prune+Sync**: a `providers.prune`
  flag (migration 0023) + toggle on each provider card — when set, discovery removes
  devices the provider no longer reports (cleans up orphaned config entities), cascading
  out of rooms. `POST …/discover?prune=true|false` overrides per run; never prunes on an
  empty result (transient-failure guard). New global Settings buttons run it across all
  providers: **Discover** (devices), **Sync** (devices + rooms), **Prune + Sync** (force).
- [x] **Source / app switching (Tier A)**: `AudioState.source_list` — selectable
  inputs / a smart TV's installed apps (HA `source_list`), surfaced through the
  audio API/`v1`/MCP and a source dropdown on the Audio device card; switch by
  sending the name as `source` (the existing `select_source` path). Generic across
  providers (Onkyo/Sonos can populate it later for free). Wiremock + UI.
- [ ] **Play named content — Tier B (the voice north-star, filed for later)**:
  "play Bob's Burgers on the bedroom TV" — launching a *specific title* inside an
  app, beyond switching to it. Brittle/app-specific (deep-link URLs via
  `media_player.play_media`, or `browse_media` over WebSocket). Design fork:
  Bifrost resolves content itself, **or** delegate the NL media intent to **HA
  Assist** (`conversation.process`) and let HA resolve it (preferred — fits the
  Whisperr voice pipeline, reuses HA's media resolution). A TV is more than audio
  (app launch/video) — may warrant its own media domain. **Decide the fork before
  building.**
- [x] **De-dup (Phase 1)** — see [Milestone 21](#milestone-21--cross-provider-de-dup).
  A device imported both natively and via HA no longer shows twice: matched by
  hardware id, the native copy wins and the HA copy is shadowed.
- [ ] **Device registry import** (optional): HA *devices* (grouping multiple
  entities) beyond Areas, if entity-flat import proves insufficient. Phase 1 of
  de-dup already fetches the device registry, so the join data is in hand.

---

## Milestone 21 — Cross-provider de-dup

A physical device reachable both natively (Hue/Govee/Sonos/Onkyo) and via Home
Assistant imports as two rows. De-dup recognises them by **hardware id** and
collapses the integration copy under the native one — native always wins, since
a direct provider is faster/more reliable than going through HA.

**Phase 1 — shipped:**
- [x] **Machinery**: migration `0025_device_dedup` adds `hw_id` (normalized
  `mac:<hex>` via `providers::mac_hw_id`, MAC-48 or EUI-64), `shadowed_by`, and
  `shadow_auto` to lights/audio/power. A shadowed device stays tracked but drops
  from control + room membership (`shadowed_by IS NULL` guards) and the Devices
  page collapses it under its canonical, with a "hidden duplicate" row.
- [x] **Reconciler** (`api::dedup::reconcile_duplicates`): after every discovery
  and provider delete, cluster by `hw_id` and auto-shadow the integration copy
  (`registry.ui_domain(type) == Integration`) under a native canonical. Exact-MAC
  only, so auto-apply is safe; removing the native side re-surfaces the copy.
- [x] **hw_id sources**: HA reads it from the **device registry** over WebSocket
  (`config/device_registry/list` → `connections` MAC, joined to the entity
  registry's `device_id`); Govee from its device MAC. End-to-end **Govee ↔ HA**
  de-dup works and is unit-tested.
- [x] **Manual link** `PUT /api/{lights,audio/devices,power/devices}/{id}/shadow`
  (`set_device_shadow`, `shadow_auto = 0`) — the no-hw_id fallback / override, and
  an **Unlink** action; the reconciler never clobbers a manual link.
- [x] **Phase 1.x — broaden native hw_id**: every first-class provider now stamps
  a `hw_id` where the hardware exposes one — **Hue** (Zigbee MAC, joining
  `/resource/device`'s `light`+`zigbee_connectivity` services with
  `/resource/zigbee_connectivity`'s `mac_address`, best-effort so discovery never
  fails on it), **Sonos** (MAC parsed from the `RINCON_<mac>…` uuid), **Onkyo**
  (MAC from the `NRI` device-info reply, on the main zone). So a Hue/Govee/Sonos/
  Onkyo device also imported via HA now de-dups, native-canonical. Govee LAN /
  WLED / Tasmota / Shelly stay `None` (no MAC on hand; unregistered anyway).
- [x] **HA hw_id from `identifiers`, not just `connections`**: HA's Onkyo integration
  keys the receiver device by its **MAC string in `identifiers`** (`["onkyo",
  "0009b0e82343"]`), not as a `("mac", …)` connection — so the native receiver (NRI
  MAC) and the HA `media_player.onkyo` weren't matching. `ha_device_hw_id` now prefers
  a connection MAC, then falls back to a MAC-shaped identifier (`mac_hw_id` rejects
  non-hardware strings, keeping false-positive risk negligible). Native receiver wins.
- [ ] **Phase 2 — heuristics (deferred)**: only if exact-MAC coverage proves
  insufficient, a confirmation-gated fuzzy match (name/area) — never auto.

**Capability parity (standing rule):** when HA exposes a capability a first-class
provider lacks, build it into the native provider rather than leaning on the HA
copy — de-dup makes the native device canonical, so the capability must live there.
**Process:** any capability flagged as missing on a native provider (e.g. while
de-dup hides the HA copy that had it) **must be filed in the parity-gaps list
below** in the same change that flags it, so we never silently lose a capability.

### Capability-parity gaps (tracked)

Native-provider capabilities HA exposes that we still need to build natively.
Add a row whenever a gap is flagged; check it off when the native provider has it.

- [ ] **Sonos `source` / `source_list` (input + current-source).** Native Sonos
  reports `source: None` / `source_list: []` and *rejects* `select_source`; HA's
  Sonos `media_player` exposes both (e.g. shows "Spotify Connect" as the current
  source). De-dup hides the HA copy, so we lost it. Buildable natively:
  **(1)** current-source readout derived from the AVTransport track URI scheme
  (`x-rincon-stream:` = Line-In, `x-sonos-htastream:` = TV, `x-sonosapi-*`/
  `x-sonos-spotify:` = the service, `x-rincon:` = following a group); **(2)**
  selectable Line-In / TV via `SetAVTransportURI` + favorites (already have
  list/play) wired through `select_source`. Not doable: making "Spotify Connect"
  itself a switch target (no UPnP path — it's a read-only current-source value).

---

## Milestone 22 — Bind a receiver to its source devices (TV / streamer) — NOT STARTED

Real-world AV: N source devices (a TV, a streamer, a console) feed audio **through an
AV receiver**, which is the thing that actually controls **volume**. Today Bifrost
models them as independent audio devices, so a room shows a receiver *and* a TV with
duplicate/overlapping controls and the "wrong" thing owning volume.

Idea: let a user **bind** a source device (TV/"thing") to a receiver. After binding:
- **Volume/mute** for the bound source is **routed to the receiver** (the receiver is
  the volume authority; optionally also drives receiver input/power).
- **Playback** (play/pause/next, and the M19 north-star "play Bob's Burgers") stays on
  the **TV/source** (`media_player.play_media` / HA Assist).
- The room/Control surface shows **one combined control** for the bound pair (source
  glyph for transport + receiver for volume), not two.

Open questions: persist the binding (new table vs a field on the source device); how it
interacts with receiver input selection (binding could also imply "switch the receiver
to input X when this source plays"); multi-source receivers (one receiver, several bound
sources). Relates to the M19 TV-media work and the audio-group derivation (M21/zones).

# Open milestones

## Milestone 12.2 — Music services: real Spotify (and friends) — NOT STARTED

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
