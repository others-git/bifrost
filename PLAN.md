# Bifrost — Implementation Plan

> **How this file is laid out:** open work first (this section), then the shipped
> one-liner log, then full detail — open specs, then completed-milestone archives,
> then reference (goals / out-of-scope / invariants). Milestone numbers are stable
> labels, **not** sequential by date (done & open are interleaved).

## Open milestones — what's left

**Next up (small, scoped):**
- **M29 — Kiosk controller** *(backend DONE in tree; dashboard + app pending)* — see
  and manage the wall-tablet companion apps. A kiosk is identified by its `bfr_`
  key; it **checks in** on a heartbeat (`POST /api/kiosks/checkin`, key-auth) and
  the server returns a queued command. Management is **session-only** (mobile/
  desktop, not the kiosk): `GET /api/kiosks` (clients view), `POST …/{id}/command`
  (`sleep`/`wake`/`lock` — lock = force WebView sign-out), `POST …/{id}/deauth`
  (revoke key → app re-enrolls via QR on 401), `DELETE …/{id}`. Migration 0032,
  `api::kiosks`; documented in `API.md`. **Remaining:** dashboard "Clients" page
  (hidden on kiosk via `BifrostKiosk` UA); companion-app check-in loop + command
  handling + UA suffix (other repo). Later: browser-session clients in the same
  view; server-side kiosk-kind session enforcement.
- **M28 — QR device pairing** *(backend + dashboard DONE in tree; app side pending)* —
  authorize headless devices (the wall tablet) without typing a key. An authed
  session mints a short-lived, single-use enrollment token (`POST /api/enrollment`)
  rendered as a QR on the dashboard (Settings → API keys → "Pair a device"); the
  device scans it and redeems it (`POST /api/enrollment/redeem`) for a normal
  `bfr_` key (revocable like any other). Migration 0031, `api::enrollment`, shared
  `apikeys::mint_api_key`; documented in `API.md`. **Remaining:** the companion
  app's camera/QR scan → redeem step (other repo); then revisit (optional: bind a
  token to one redeem, shorter TTL, "Test connection" affordance).
- **M26 — Composite (merged) devices** *(draft)* — merge *complementary* same-device
  entities into one capability-routed device (the BRAVIA "two media_players" problem:
  one has now-playing, the other the remote). Distinct from de-dup, which is for
  *equivalent* copies. Detail below.

**Design directions (formalize as we add controls):**
- **Normalized "Control" abstraction** — `LightControlChange` (M27) normalized *light*
  controls into one typed change contract; extend to a shared, capability-gated,
  compact-friendly control vocabulary across light/audio/power/remote so each new
  control is declared once and reused by Dashboard / Rooms / Floor-Plan / scenes,
  never forked per view. Detail in the M27 section.

**Flagship — native voice (big, multi-phase):**
- **M23 — Native voice: command control** *(P1 + P2 backend shipped: grammar +
  `/api/voice/command`; pluggable model-role config (`ai_endpoints`) + STT
  `/api/voice/listen`; voice seam now also Bearer-keyed like `/api/v1` so the
  headless wall-tablet satellite can drive it without a session — documented in
  `API.md`)* — remaining: LLM fallback over MCP tool schemas, tablet PTT
  + AI-endpoints Settings UI, wake word. Pluggable local/OSS models, degrades to grammar.
- **M24 — Talk mode: conversation & live translator** *(not started)* — WSS streaming
  pipeline (`/api/voice/stream`); the headline use case is a live two-party translator.

**Feature backlog:**
- **M12.2 — Music services (real Spotify & friends)** *(not started)* — OAuth music-service
  domain that targets an audio device (Spotify Connect).
- **M15 — Scenes capture full device state (audio-aware)** *(not started)* — snapshot
  audio source/volume too, not just lights.
- **M17 — Floor plan on mobile** *(deferred)* — view/control + touch drafting on phones.
- **M18 — Matter device support** *(not started, spike-gated)* — Bifrost as a Matter
  controller for Wi-Fi lights/plugs.

**Deferred items inside shipped milestones:**
- **Richer HA domains as their own type** — `climate.*` / `cover.*` / `lock.*` (M19).
- **De-dup Phase 2 — fuzzy heuristics** — confirmation-gated name/area match (M21).
- **Receiver power-off / input-restore arbitration** — ref-count active sources (M22).

**Open follow-ups (from shipped milestones):**
- [ ] **Onkyo NET preset enumeration** (M12) — needs the receiver's undocumented HTTP API.

**Capability-parity gaps (tracked)** — native capabilities HA exposes that we must build
natively (de-dup makes the native device canonical, so the capability must live there):
- [ ] **Sonos `source` / `source_list`** — derive current-source from the AVTransport URI
  scheme; selectable Line-In / TV via `SetAVTransportURI` + favorites. Detail in the M21
  archive below.

---

## M25 — Remote control + `BifrostRemote` — DONE (in tree, unreleased)

A generic, reusable **virtual-remote** surface for TVs (Android TV today, any
provider that exposes a remote later). A new **`remote` device domain**, paired
to its TV, drives D-pad/keys + app launch; the frontend `BifrostRemote` renders a
smart-remote with dynamic app buttons. See the phase breakdown below (kept current
as each phase lands).

**Grounding (HA Android TV Remote, probed live):** per TV, the integration adds a
`remote.<tv>` entity (`current_activity` = foreground app package; `supported_features: 4`)
and a paired `media_player.<tv>` (`app_id`/`app_name`). Control via the `remote`
domain services — `send_command { command, hold_secs }` (keycodes: `DPAD_*`,
`BUTTON_A/B/X/Y`, `BACK/HOME/MENU/INFO/GUIDE/SETTINGS/SEARCH/ASSIST/POWER`, `0-9`,
media/volume/channel, `text:<str>`), `turn_on { activity }` (launch app by Play
Store package **or** deep-link URL), `turn_off`/`toggle`. No installed-app
enumeration (`activity_list` empty) → Bifrost tracks recents + lets the user pin.
`media_player.play_media { media_content_type: app|url|channel }` is a second
launch path that also feeds the "play named content" north-star.

**Deferred (was the prior 'next'):** richer HA domains (`climate`/`cover`/`lock`),
HA device-registry grouping, and the Sonos `source`/`source_list` parity gap —
now future milestones / the parity-gaps list below. **M19 is DONE** (multi-domain
HA, push, power live state, Tier B HA-Assist fallback).

### M25 phases (all shipped to tree)

Decisions (settled): a **standalone `remote` device domain** (`models::remote`),
each remote **paired** to its TV (`media_player`) by hardware id; app buttons are
**recents (auto, from `current_activity`) ∪ user-pinned**. The `BifrostRemote`
frontend is provider-agnostic; the HA Android TV Remote is the first backend.

**Phase 1 — `remote` domain + HA `RemoteProvider` — DONE:**
- [x] `models::remote` — `RemoteDevice` (lean: `on` + `current_app`) + canonical
  `RemoteKey` enum (Up/Down/Left/Right/Select/Back/Home/Menu/VolUp/VolDown/Mute/
  PlayPause/Next/Previous/Power) + `RemoteCommand` tagged union (key/text/
  launch_app/power); providers map keys to their native command.
- [x] `RemoteProvider` trait + `RemoteProviderFactory`; registry wiring
  (`register_remote`/`build_remote`/`is_known_remote`/`remote_factories`),
  `/api/providers/{id}/discover` **additive** over remote + prune.
- [x] HA impl (`HaRemoteFactory`): `discover` (`remote.*`), `get_state`
  (`current_activity`), `send_key` → `remote.send_command` (canonical→Android
  keycode), `send_text` → `text:<str>`, `launch_app` → `remote.turn_on
  { activity }`, `set_power` → `turn_on`/`turn_off`.
- [x] Migration `0027_remote_devices` (enabled/glyph/hw_id/`paired_audio_id`).
  Wiremock tests (discover/send_key/launch_app/mapping/factory) + integration
  (401 + discover→list→command).
- [x] **Early from P2:** `api::remote` session routes (`/api/remote/devices`,
  `…/{id}` live read, `…/{id}/command`, enable/glyph) + the service layer
  (`apply_remote_command`/`read_remote_state`/`list_remotes`); auto-pairing
  `reconcile_remote_pairings` (match remote↔TV audio by `hw_id`, prefer TV kind)
  runs after every discovery.

**Phase 2 — public surfaces (`v1` + MCP) — DONE:**
- [x] `/api/v1/remote/devices` (+ `/{id}` live read, `/{id}/command`), Bearer, over
  the shared service fns; documented in `API.md` (command tagged union + keys).
- [x] MCP tools `list_remotes` / `press_remote_key` / `launch_app` (name-resolved,
  key parsed from canonical string); documented in `MCP.md`. Session routes +
  `hw_id` pairing already landed in P1. Integration tests: session, `v1` (401 +
  command), discover→list→command.

**Phase 3 — app tracking (recents + pinned) — DONE:**
- [x] `remote_apps` store (migration `0028`: package/name/pinned/last_seen).
  Recents recorded from `current_activity` on every live read
  (`record_app_seen`, no-op for non-package activities); `list_remote_apps`
  returns **pinned-first then recents**; pin/unpin via `PUT …/{id}/apps/pin`
  (`set_app_pin`, inserts a never-seen package so it can be added pre-launch);
  `app_display_name` package→friendly-name registry (Netflix/YouTube/Prime/
  Disney+/…). Session routes `GET …/{id}/apps` + `PUT …/{id}/apps/pin`.
  Integration test `remote_apps_record_recents_pin_and_order` (recents + 401 +
  pin/unpin + ordering). **Icons** are a frontend package→glyph map (P4), not
  stored in the DB.

**Phase 4 — `BifrostRemote` frontend — DONE:**
- [x] `components/BifrostRemote.tsx` — provider-agnostic remote: D-pad + select,
  back/home/menu, power, volume (±/mute), transport (prev/play-pause/next), and
  dynamic **app buttons** (pinned ∪ recents from `getRemoteApps`, tap to launch,
  ☆/★ to pin; current app highlighted). Live state poll for power/current-app.
  Full-width bottom sheet on compact, centred modal on desktop. `api.ts` remote
  types + fns (`getRemoteDevices`/`getRemoteState`/`sendRemoteCommand`/
  `getRemoteApps`/`setRemoteAppPin`). The TV's `AudioEditor` fly-out (used by
  Control **and** Floor Plan) gets a **📺 Remote** button when a remote is paired
  to that TV (`paired_audio_id`), opening the overlay; the fly-out's outside-click
  is suppressed while the remote is up. `tsc` + `vite build` clean.

**M25 is feature-complete** (all 4 phases). Ships with the next release bump.

---

## M27 — Tunable white + the control-change contract — DONE (in tree, unreleased)

A "white light" picker (Hue's Color/White tabs) plus the first step of a
**normalized control design** for the growing set of light controls.

**Frontend (`components/LightEditor.tsx`):**
- **`ColorTempWheel`** — a disc filled with a warm→cool gradient where the x
  position selects color temperature in **mirek** (`MIREK_MIN` 153 ≈6500K …
  `MIREK_MAX` 500 ≈2000K). Shares the disc shape/drag feel of `ColorWheel`, so the
  **`ModeToggle`** ("Color / White") just swaps one wheel for the other. `mirekToRgb`
  (Tanner-Helland fit) renders the gradient + the brightness-bar tint.
- New props `showWhite` / `initialMirek` / `initialMode`; the toggle only appears
  when a light has both `color_rgb` and `color_temperature`. Wired through
  Dashboard (`LightButton`, `RoomBox`), FloorPlan, and the scene palette editor.
- **`LightControlChange`** — the formalized contract: every light control surface
  (wheel, white wheel, brightness bar, swatches) emits one discriminated change
  `{ field: "color" | "brightness" | "temp"; … }`. A fan-out caller (room cascade)
  applies **only** the moved dimension, so brightness never stomps per-light color,
  and color ⇄ white stay mutually exclusive. *Add a new light control by adding a
  variant + a case in each caller.* This is the seed of the broader "Control"
  normalization (see the open design note below).

**Backend (`api::lights::persist_light_state`):** now **merges** the attributes
present in a command into the cached `last_state` instead of overwriting the row —
partial commands (pure on/off, brightness-only cascade, a white-temp change) keep
the dimensions they didn't touch. **Color and color-temperature are mutually
exclusive**: setting `color` clears `color_temp_mirek` and vice-versa, so
`last_state` is an honest signal of the light's active mode. Removed the old
`pure_power` special-case (the merge subsumes it). Tests:
`room_brightness_change_preserves_light_color`, `light_color_temp_change_clears_color`
(+ the existing `room_power_cycle_preserves_light_color` still green).

**Open design note — formalize a reusable "Control" abstraction.** `LightControlChange`
normalizes *light* controls; the next step is a shared control vocabulary across
domains (light / audio / power / remote) so every new control surface (picker,
slider, toggle, segmented mode) is declared once and reused by Dashboard, Rooms,
Floor-Plan fly-outs, and scenes — never re-implemented per view. Capture the
common shape (a labelled, capability-gated, compact-friendly control emitting a
typed change to the shared service layer) as controls accrue.

---

## M26 — Composite (merged) devices — draft (not started)

One physical device can surface as several Bifrost
devices with **complementary** (not equivalent) capabilities. Real case: a Sony
BRAVIA appears as two HA `media_player` entities — the **BRAVIA integration**
(now-playing/media + discrete HDMI sources, but no `device_class` → mis-classed
"Speaker") and the **Android-TV Remote** (`current_activity`/app + the paired
remote, but no media metadata). Neither is a superset, so today the user sets two
cards and each is half-right.

**Merge ≠ de-dup.** Keep both relationships:
- **De-dup (shadow)** — for *equivalent* copies (native Hue ⊇ HA-Hue): hide the
  lesser, native wins, lossless. Unchanged.
- **Merge (composite)** — for *complementary* copies of one physical device:
  **union** the capabilities and route each to the backing that actually has it.
  Shadowing here is a capability loss, which is why this milestone exists.

**Precedent (this is a generalization, not a new paradigm):** M22 receiver
binding already does capability-routed splitting + overlaid reads
(`AudioCommand::split_for_receiver`: volume/mute→receiver, power/source/transport
→source; source read overlays receiver volume/mute), and the remote↔TV pairing
(`paired_audio_id`) already links one logical device across domains.

**Target UX (user's words):** each physical device shows **once** on the control
panel; click the TV → **all** related controls + activity + remote in one place.

Field routing for the BRAVIA case:

| Capability | Backing |
|---|---|
| Now-playing / transport | the media-reporting entity (BRAVIA integration) |
| Current app + launch + D-pad/keys | the Android-TV entity (via the paired remote) |
| Volume / mute / power | whichever supports it (precedence: primary, else companion) |
| Source list | **union** of both |

### M26 phases (draft)

- [x] **Phase 1 — model + service — DONE.** `audio_devices.companion_of`
  (migration `0029`) links a secondary entity to a **primary**. Read merge
  (`merge_companion_into`): now-playing / source / `source_list` ∪ / **the
  companion's receiver binding** overlaid onto the primary, capabilities unioned;
  companions stay listed (marked) and merge into their primary in
  `list_all_devices` + `get_device_live`. Write routing (`route_across_backings`,
  pure + unit-tested): **volume/mute → a receiver-bound backing** (the agreed
  precedence — a receiver wins, not "primary-first"), transport → the playback
  owner, source → the inputs backing, power → primary; each part applied via
  `apply_with_receiver` so the M22 receiver split still runs (no recursion).
  `set_audio_companion` (self/chain/unknown/shadowed guards) + session & `/v1`
  routes; `API.md` documented. Routing unit tests + CRUD/validation/401
  integration test. *(Remote `paired_audio_id` re-point: deferred to P4 — the
  remote already surfaces via the TV's fly-out.)*
- [x] **Phase 2 — honest classification — DONE.** `audio_kind`
  (`src/providers/ha/mod.rs`) now also classifies **TV** when the entity reports
  a running app (`app_id`/`app_name`), not just `device_class == tv` — fixes the
  mis-classed-Speaker bug. Unit-tested. (The paired-`remote.*` signal + auto-
  suggested merges fold into P3.)
- [x] **Phase 3 — link UI — DONE.** Devices page: a **⧉ "Merge into…"** control on
  audio cards (`MergePicker` → `setAudioCompanion`); merged companions collapse
  under their primary (`MergedCompanion` row, "merged into X — controls combined")
  with an **Unmerge** action. The lossless counterpart to "mark as duplicate"
  (the lossy shadow link was deliberately *not* shipped). *(Auto-suggested matches
  deferred — manual link only, no silent merges.)*
- [x] **Phase 4 — control surface — DONE.** Companions drop from control like
  shadows: backend room-audio-member + voice queries gain `companion_of IS NULL`;
  the Audio page filters them; the Devices page collapses them. The merged
  **primary** carries the union (now-playing + volume→receiver + source +
  transport) via the Phase-1 read-merge, and the 📺 remote already surfaces via
  the TV's fly-out — so one card has everything.

**M26 is feature-complete** (Phases 1–4). The BRAVIA merges into one TV with
nothing hidden. *(Future, not blocking: auto-suggested merges from kind+area;
Floor-Plan was already covered by the shared `AudioEditor`.)*

### Decisions settled while building

- Capability precedence: **volume/mute → a receiver-bound backing** (a receiver
  wins over the primary — the key correction), transport → the playback owner,
  source → the inputs backing, power → the primary.
- Auto-detect heuristic (HA area + normalized name + TV-ish kind → *suggest*,
  require manual confirm) — don't over-reach.
- Cross-domain scope: audio merge + the existing remote pairing only, for now.

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
| 6 — LIFX | Cloud REST `LightProvider` (`api.lifx.com`, Bearer token) — discover/set/get, HSBK ⇄ Bifrost xy-colour/mirek, MAC-serial `hw_id` for de-dup; LIFX groups mirrored as Bifrost Rooms (`discover_groups`) with one-call `group_id:` native group control (`set_group_state`); registered + wiremock-tested. LAN/UDP still deferred. |
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
| 21 — Cross-provider de-dup | Hardware-id (`mac:`) matching **shadows** the HA copy under the native one (native wins); manual link/unlink as the no-hw_id fallback. Phase 1 shipped (Phase 2 fuzzy heuristics deferred). Detail in archive below. |
| 22 — Receiver binding | Source devices (TV/streamer/console) **bind** to an AV receiver that owns their volume/mute; powering a source on wakes + switches the receiver to its input. Onkyo single-connection `OnkyoLink`. Phases 1+2 shipped. Detail in archive below. |
| 25 — Remote control + `BifrostRemote` | `remote` device domain + HA Android-TV provider; session/`v1`/MCP surfaces; TV pairing by `hw_id`; app recents/pins; the `BifrostRemote` frontend. Feature-complete in tree (unreleased). Detail above. |
| 27 — Tunable white + control-change contract | LightEditor gains a Hue-style **Color / White toggle** with a warm→cool **color-temperature wheel** (`ColorTempWheel`, mirek 153–500) for lights with `color_temperature`. Backed by a formalized **`LightControlChange`** union (`color`/`brightness`/`temp`) every light control emits, so a fan-out caller adjusts only the moved dimension. `persist_light_state` now **merges** present attributes (partial commands keep untouched dimensions) with **color⇄temp exclusivity** (set one, clear the other) — fixing the room-brightness-stomps-color bug and giving the UI an honest active-mode signal. Detail below. |

### Open follow-ups from shipped milestones

- [x] **Sonos SSDP discovery without a seed IP** (M10) — **already shipped**: the
  add-provider **Scan network** button runs `SsdpDiscovery` (ZonePlayer
  `M-SEARCH` → `LOCATION` host), auto-fills the seed, and the household fans out.
  No seed IP needs to be known.
- [x] **MCP `list_audio_favorites` / `play_audio_favorite`** (M12) — shipped as
  embedded MCP tools in M11. Tracked in [MCP.md](MCP.md).
- [ ] **Onkyo NET presets** (M12, deferred) — eISCP exposes service *selection*
  (`NSV`) but not preset *enumeration*; needs the receiver's undocumented HTTP API.

---

# Completed — milestone detail (archive)

*Shipped milestones, kept for reference/grep. Skip unless you need the deep
history; the one-liner log above is the index. (M25 detail is at the top.)*

## Milestone 19 — Home Assistant ("high-class" provider) — DONE

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
- [x] **Room on/off fan-out to power (and audio) members**: `apply_room_state`
  (shared by session, `/v1`, MCP `set_room`) now drives a room's `on` to its audio
  members (power-only, routed so a bound source still wakes its receiver) **and**
  its power-device members — previously room on/off was lights-only. Light-less
  rooms (only switches/speakers) are now controllable. Covered by an integration test.
- [x] **Power: live push wired** — `HaPushManager` emits `PowerEvent`s onto a new
  power pipeline (`power_db_writer_task` → `power_devices.last_state`; SSE
  `power_state`, consumed on the Dashboard). Provider-list `domain` label already
  reads "Integration" for HA via `ui_domain` (the earlier "light" note was stale).
- [ ] **Richer domains as their own type** *(deferred past M19 — its own future
  milestone)*, when their state surface justifies it: `climate.*` (setpoint/mode/
  current temp), `cover.*` (position), `lock.*`. Each is a new domain (model +
  control surface + UI), grouped where applicable.
- [x] **WebSocket push** (`references/ha_websocket_api.md`): `HaPushManager` keeps
  one persistent `subscribe_events`/`state_changed` WebSocket open
  (`ConnectionMode::HaPush`, built from the concrete `HaProvider` like Hue's SSE)
  and fans each change onto the light / audio / **power** pipelines via
  `HaProvider::push_events` + `classify_push` — instant updates instead of 30 s
  polling. Mock-WS + classify unit tests; reconnection owned by the manager with
  the shared backoff.
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
- [x] **Play named content — Tier B (the voice north-star)**: "play Bob's Burgers
  on the bedroom TV" — launching a *specific title* inside an app. **Fork decided:
  native-first, HA-Assist fallback.** Bifrost's deterministic grammar (`api::voice`)
  parses what it can; any clause it can't (the brittle, app-specific long tail) is
  delegated to **HA Assist** — `HaProvider::converse` → `POST /api/conversation/
  process`, returning `(speech, ok)` from `response.speech.plain.speech` /
  `response_type`. The fallback fires per-`Unparsed`-clause in `run_command`
  (`ha_assist_fallback`), so a configured HA resolves and acts on the intent while
  no-HA installs keep the native "didn't understand". Reuses HA's media resolution
  across all integrations; wiremock (`converse`) + integration (`voice` → Assist)
  tests. *Future, if needed:* a dedicated media domain for richer TV control
  (browse, deep-link `media_player.play_media`) — a TV is more than audio.
- [x] **De-dup (Phase 1)** — see [Milestone 21](#milestone-21--cross-provider-de-dup).
  A device imported both natively and via HA no longer shows twice: matched by
  hardware id, the native copy wins and the HA copy is shadowed.

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

## Milestone 22 — Bind a receiver to its source devices (TV / streamer) — PHASE 1 + 2 SHIPPED

Real-world AV: N source devices (a TV, a streamer, a console) feed audio **through an
AV receiver**, which is the thing that actually controls **volume**. Bifrost modelled
them as independent audio devices, so a room showed a receiver *and* a TV with
duplicate/overlapping controls and the "wrong" thing owning volume.

**Decisions (settled):** a **many-to-one** binding (many sources → one receiver),
**stored on the source** (`audio_devices.receiver_id` + `receiver_source`, migration
0026 — no new table). Binding **implies switching the receiver to the source's input**
when the source powers on, where the receiver enumerates one. A future big MCP win
("turn on the TV" lands sound on the right input by voice).

**Phase 1 — shipped:**
- [x] **Storage** (migration `0026_audio_receiver`): `receiver_id` (the receiver an
  audio source routes volume/mute to; NULL = unbound) + `receiver_source` (the receiver
  input to select when the source becomes active) on `audio_devices`, stored on the
  source. A dangling `receiver_id` (receiver deleted/disabled) is treated as unbound.
- [x] **Routing** (`AudioCommand::split_for_receiver` + `apply_audio_command`): a bound
  source sends `volume`/`mute` to the receiver and keeps `power`/`source`/`transport`
  on itself. Powering the source **on** wakes the receiver and switches it to
  `receiver_source`; powering **off** is *not* propagated (many sources may share one
  receiver). Routing lives in the shared service fn, so session, `/v1`, **and MCP**
  (`set_audio`) all route identically — the MCP win comes for free.
- [x] **Binding API**: `PUT /api/audio/devices/{id}/receiver` + `/api/v1/...`
  (`set_audio_receiver`), `null` to unbind. Rejects self-binding, an unknown receiver,
  and chaining (binding to a device that is itself bound) → 422. A bound source's read
  overlays the **receiver's** (cached) volume/mute, and exposes `receiver_id` /
  `receiver_source`.
- [x] **UI**: a receiver-bind control on each source device on the **Devices** page —
  pick the receiver, then (once bound, if the receiver enumerates inputs) the input to
  switch to. Anchored popover on desktop, bottom sheet on phones/tablets.
- [x] Unit tests (the command split) + API tests (binding CRUD/validation; volume
  routes to the receiver, not the source).

**Phase 2 — shipped:**
- [x] **One combined control** on the Control page: a receiver that is the
  volume-target of another audio member **in the same room** is collapsed out of the
  member list, so the bound source's glyph represents the pair (its volume already
  routes to the receiver). The source's fly-out (`AudioEditor`/`AudioControls`) shows a
  **"Volume → {receiver}"** hint by the slider.
- [x] **Receiver volume overlay** on a bound source reads the receiver's **cached**
  state (kept fresh by the push manager), not a competing live read — a live read of a
  push-managed receiver (Onkyo) returns a partial result and would clobber the volume
  with 0. (Initially tried a live overlay; reverted after it surfaced the Onkyo
  single-connection bug below.)
- [x] **MCP `bind_receiver` tool** (bind/unbind by name; resolves source + receiver,
  optional `receiver_source`). Routing already benefited MCP `set_audio`; this adds
  *creating* the binding by voice. `get_home_state` already carries the binding fields.

**Phase 2 — also shipped:**
- [x] **Combined pair on Rooms + Floor Plan**: `RoomVolumeStrip` (Rooms page + Floor
  Plan) drops a bound receiver from its member set, and the Floor-Plan `AudioEditor`
  shows the "Volume → {receiver}" hint. Backend `set_room_audio_state` skips a bound
  receiver in the room volume fan-out (`receiver_targets_within`), so room volume hits
  the receiver once via the source, not twice. Covered by an integration test.
- [x] **Onkyo single shared connection (off hold)**: a receiver honors ~one eISCP
  connection, so the old "connection per operation" model meant every read/write opened
  a socket that kicked the persistent push channel — stale/0 volumes and a ~1s push
  bounce per command. Fixed with a process-global per-receiver **`OnkyoLink`** actor
  that owns the one socket and multiplexes the push stream + all reads/writes over it
  (reconnects with backoff, re-queries state on reconnect). Onkyo is now **active**
  (off hold). Test `reads_and_push_share_one_connection` asserts one socket for N ops;
  `get_state` also now errors (instead of reporting 0) on a partial/missing volume read.

**Phase 2 — deferred:**
- [ ] Optionally drive **receiver power-off / input restore** with smarter multi-source
  arbitration (when the *last* bound source powers off). Needs design (ref-counting
  active sources per receiver).

# Open — milestone detail

*Full specs for the open backlog listed at the top. (M26 detail is near the top,
right after M25.)*

## Milestone 23 — Native voice: command control — IN PROGRESS (flagship; P1 + P2 backend shipped)

Bifrost owns its **own voice pipeline** rather than handing off to Home Assistant —
because we have first-class providers, voice should drive them directly. A
mic-equipped tablet (the wall-fixture use case) is the first voice ingress, but the
engine is ingress-agnostic. **This decides the M19 Tier-B fork: native NL control,
HA Assist only as a last resort for HA-only *content* launch.** (Free-form
conversation / live translation is its own milestone — see **M24**.)

**Guiding constraints (hard rules — apply to M24 too):**
- **Models are pluggable, local-runnable, open-source — never mandated.** Bifrost
  ships **zero weights and zero runtimes**; it's a thin client to whatever the user
  runs. Assume most run **lightweight CPU-only** models.
- **Don't dictate usage.** Give many ways to use it; the engine is ingress- and
  client-agnostic (tablet, phone, future). Context is supplied by the client, not
  imposed by Bifrost.
- **Degrade gracefully.** Core control must work with **zero models configured**
  (built-in grammar). STT/LLM/TTS are optional upgrades; one being down never mutes
  the hub.

**Pluggable model roles** *(backend shipped)* — config mirrors providers: an
`ai_endpoints` table (migration 0030), one row per role (`base_url` + `model` +
optional **encrypted** `api_key` + enabled), with session CRUD at
`/api/ai-endpoints/{role}` and a per-row **Test** (`POST …/test` → `GET {base_url}/models`
reachability probe). `api::ai_endpoints::endpoint_for` is the shared accessor the
pipeline reads; an unconfigured/disabled role just means that capability is absent
(grammar control over `/command` keeps working). *Remaining: the Settings CRUD **UI**.*
All roles are **OpenAI-compatible HTTP** so Ollama / llama.cpp / faster-whisper /
LocalAI / the user's own *whisperr* all work unchanged:
- `transcription` — STT (`POST {base_url}/audio/transcriptions`, multipart).
- `chat` — NLU / conversation (`POST {base_url}/chat/completions`, tool-calling).
- `tts` — speech out (`POST {base_url}/audio/speech`), optional.

**Predefined models / dev stack — a *separate, optional* artifact, never in the binary.**
Since Bifrost mandates nothing, "predefined but optional" models live outside it, and
it doubles as the thing we develop against ("we'll need something for dev anyway").
Both options below are *just another endpoint* to Bifrost — it never special-cases them:
- **Preferred first: assemble existing OSS** via a checked-in `docker-compose.voice.yml`
  — e.g. **LocalAI** (one OpenAI-compatible server for STT+chat+TTS) or
  faster-whisper-server + Ollama + Piper, with small CPU models. Zero new code to
  maintain; serves as both the dev harness and a turnkey "voice pack."
- **Only if that's clunky: a companion repo** (a thin harness around whisper.cpp + a
  small chat model + Piper) exposing the same roles — a branded turnkey. Re-introduces
  the model-serving maintenance we deliberately kept out of the binary, so defer it.

### Command pipeline ("bifrost, turn off the office lights")
**P1 shipped** (`src/api/voice.rs`, `POST /api/voice/command { text, context? }`, session-
gated): pure grammar (wake-word + politeness strip, clause split on `and`/`then`/`,`) →
**power** (room-wide / `{room} lights` / single light·power·audio · `everything`),
**brightness** (absolute + **relative** %-of-current), **color** (named→RGB, with shade
modifiers — "dark red"/"light blue") + **color-temp** (`warm white`/`daylight`→mirek),
**volume** (absolute + relative), **mute**, **transport**, **scene** (room / `in {room}`
/ everywhere), with entity resolution mirroring `mcp::resolve` and dispatch through the
shared service fns. Returns `{ ok, said, clauses[] }`. 23 grammar unit tests + 6 seam API
tests. Color/brightness on a room touch **lights only** (don't power the room's audio).
**Fast-follows (P1.x):** play-favorite, group/ungroup, read/queries, anaphora.
- [ ] **Grammar engine** (Rust, built-in, deterministic, 0 models): strip wake prefix +
  politeness/filler; verb families — on/off, brightness (abs %, relative `dim/brighten`,
  qualitative `half/max`), color (named→hex), color-temp (`warm/cool/daylight`→mirek),
  scene (`{scene} scene/mode`, `… in {room}`, `… everywhere`), volume (abs/relative/qual),
  mute, transport (play/pause/stop/next/previous), play-favorite (`play X in Y`),
  speaker group/ungroup. **Entity = `mcp::resolve`** (rooms first, then lights/audio/power;
  articles/plurals stripped). Ambiguous → return `resolve`'s candidate list as a
  disambiguation question.
- [ ] **Targeting/scoping**: bare **`{room}`** → **room-wide** control (`set_room` /
  room audio fan-out — already built, all members); **`{room} lights`** → that room's
  **light members only** (fan-out); `here`/`this room`/bare command → the **client
  context room**; `everything`/`whole house` → whole-home. `turn up/down` → volume if the
  target has audio, else brightness.
- [ ] **Read/queries** (in scope): "is the office on?", "what's the volume in the
  kitchen?", "are any lights on?" → read from `get_home_state`/device state, answer in
  `said` (read-only, good for talk-back).
- [ ] **Compound commands**: split on `and`/`then`/`,` and run each clause through the
  grammar; a clause the grammar can't parse falls to the LLM (per-clause), so
  "dim the office and play jazz" works.
- [ ] **Relative steps = % of *current*** (not fixed points): a step is **50% of the
  current value**, applied directionally — `dim`/`down` → ×0.5 (80→40, 20→10);
  `up`/`brighten` → +50% (×1.5, clamped 100). `a bit/slightly` = 25% of current;
  `a lot/way` = 75%. Floors: result clamps ≥1 (or off for "all the way down");
  `up`/`brighten` from 0/off turns on at a base (50%). Qualitative absolutes unchanged
  (`half`=50, `max`=100, `low`=20). All tunable.
- [ ] *(later)* **Anaphora** ("turn *it* off", "*them* up") via short conversation
  memory (last entity) — deferred; the conversation modal can hold it.
- [ ] **LLM fallback** (only on grammar miss / low confidence / unparsed clause): feed the
  model the **MCP tool schemas** (generated from the registry so they can't drift) + a
  names-only `get_home_state` snapshot; **single tool call, no agent loop** for v1;
  validate the entity resolves, then dispatch through the **same service fns** the
  grammar/MCP use.
- [x] **`POST /api/voice/command { text, context? }`** → `{ ok, said, clauses }` — the
  pure text→action seam, fully unit-testable (no audio). **(P1 — shipped.)**
- [x] **`POST /api/voice/listen` (audio)** → transcription role → command pipeline.
  **(P2 — shipped.)** Multipart `file` (+ optional `room`) → the configured
  `transcription` endpoint (`POST {base_url}/audio/transcriptions`) → the same
  `run_command` seam as `/command`; returns `{ transcript, ok, said, clauses }`.
  Degrades to a clear 503 when no transcription model is configured. wiremock
  (`transcribe`) + integration (`/listen` → STT mock → drives the device).
- [ ] **Tablet PTT UI** (push-to-talk) — the spike ingress; record → POST → show result.

### Cross-cutting
- [ ] **Client room context — modular, not prescriptive.** A per-request `context`
  (first field: `room`) lets bare references resolve ("turn off the lights" → *this
  device's room*; "turn it up"). The **client** decides what to send and how it
  persists (a wall tablet pins its room; a phone may send none). Bifrost just honors the
  context — it does not impose a device/room registry. Shared with M24.
- [ ] **Conversation modal** (frontend): shows the live "current conversation"
  (heard → did). **Configurable to persist to disk** for longer debugging sessions
  (retention configurable). Builds trust + surfaces mis-hears. Reused by M24.
- [ ] **Privacy/reliability**: audio stays on the LAN (endpoints are local); creds
  encrypted; session-gated; nothing recorded until PTT/wake; grammar keeps working if
  models are down.

### Later phases
- [ ] **Wake word** ("bifrost") — openWakeWord in-browser, custom-trained; on-device VAD,
  audio leaves the tablet only after the wake word. (PTT covers the spike.)
- [ ] Multi-tablet / multi-ingress polish.
- [ ] **Spoken confirmation** via the optional `tts` role (a short `said` read aloud).

### Phasing
P1 grammar + `/api/voice/command` seam (+ tests) **✓** · P2 STT role + `/api/voice/listen`
**(backend ✓)** + tablet PTT + client room context · P3 LLM fallback over MCP tool
schemas · P4 conversation modal (+ optional disk persistence) · P5 wake word + optional
spoken `tts`.

### Open questions
- TTS voice/lang selection surface (per client? per command?).
- Conversation persistence format (table vs files) + retention defaults.

## Milestone 24 — Talk mode: conversation & live translator — NOT STARTED

A continuous back-and-forth voice mode — free-form conversation, and the headline
use case **a live translator between two people** — **as close to real-time as
possible over a WebSocket** (`/api/voice/stream`, WSS), not HTTP round-trips. Builds on
M23's pluggable roles (`transcription` / `chat` / `tts`) and the conversation modal;
inherits M23's hard constraints (pluggable/local/open models, graceful degradation).
**Distinct from M23** because it's a streaming bidirectional pipeline, not a
one-shot command.

- [ ] **WSS streaming transport**: client streams mic frames; server VAD-chunks →
  streaming/chunked STT → (converse or translate) → response → **streaming TTS** frames
  back. Target ~1–2s turn latency; partial/interim results where the backend supports
  them.
- [ ] **Translator flow**: STT (lang A) → translate (chat/translation model) → TTS
  (lang B), pluggable each; two-party turn-taking; per-side language selection.
- [ ] **Converse flow**: free chat (optionally with the home/tool context so "…and turn
  the lights down" mid-conversation still acts), rendered in the shared conversation
  modal.
- [ ] Backend-streaming is preferred; **degrade to chunked** when an endpoint can't
  stream.

### Open questions
- **Real-time on CPU is the hard part.** Whisper isn't natively streaming, so translator
  turn-taking depends on which open backend streams well enough (whisper-streaming,
  faster-whisper+VAD, realtime servers). **Spike this latency before committing** to the
  ~1–2s target — it may dictate the recommended dev/turnkey stack in M23.
- Barge-in / turn detection (when has a speaker finished?) for natural two-party flow.

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
