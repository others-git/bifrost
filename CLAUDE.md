# Bifrost — project directives for Claude

## High-level overview

Bifrost is a **smart-home control hub**.

- **Rooms are the core abstraction.** Provider-native groupings (Hue rooms/zones, Sonos rooms, …) are mirrored as `provider_groups` and wrapped by **Bifrost Rooms** (linked via `room_links`); the **"Sync"** button on a provider does this for both light and audio domains. A Room aggregates **any number of member devices** (lights and audio) and is the high-level control surface — power (all members), brightness (member lights), and volume/mute fanned out to the room's audio members, each with a **per-room volume offset** for loudness calibration (`room_audio_devices`, `PUT /rooms/{id}/audio/state`). Room controls are not a fixed list; they grow with the device mix, so keep this section current as capabilities land. **Per-room *configuration*** (lights/links membership, audio devices + offsets, enable/disable, merge) lives on the dedicated **Rooms page** (`frontend/src/pages/Rooms.tsx`); live control (on/off, color, scenes, quick volume) stays on the Dashboard / Floor Plan.
- **Scenes** capture and re-apply member device state. Today: light color/brightness (a *save-scene* snapshot, plus global color palettes applied to a room). The design goal is to capture **all** device state so scenes stay future-proof — for audio, the *selected* source/playlist/station (e.g. a Spotify radio station), **not** the transient now-playing track. Audio-in-scenes is planned — see `PLAN.md`.
- **Per-device control:** every device is also controllable independently of any Room (`PUT /api/lights/{id}`, `PUT /api/audio/devices/{id}/state`).
- **Floor planner** — a rough 2D plan of the home: paint tiles/walls, place devices roughly where they physically reside, and bind painted regions to Bifrost Rooms. Devices and rooms are controlled directly from the plan via **fly-outs** (popovers). It is purely an *alternate visualization* of the same devices — its controls reuse the same central control components and API actions as the Dashboard/Rooms (`LightEditor`, scene controls, `setLightState`/`setRoomState`/`setAudioState`), never a forked control path.
- **Everything is exposed via the API**, kept in sync with docs: `API.md` (public `/api/v1`, Bearer-key) and `MCP.md` (companion `bifrost-mcp` tool roadmap).

### Current providers

Registered in `ProviderRegistry::default_registry()` (`src/providers/mod.rs`). **Keep this list current when adding a provider.**

| Domain | Type key | Name | Transport |
|---|---|---|---|
| Light | `hue` | Philips Hue | SSE push |
| Light | `govee` | Govee (Cloud) | poll |
| Light | `govee-lan` | Govee (LAN) | poll |
| Light | `shelly` | Shelly | poll |
| Light | `tasmota` | Tasmota | poll |
| Light | `wled` | WLED | poll |
| Audio | `onkyo` | Onkyo / Integra | eISCP push (incl. zone 2) |
| Audio | `sonos` | Sonos | on-demand UPnP (incl. groups, favorites) |

## Test coverage is mandatory

Every public function, method, and non-trivial private helper must have test coverage.
There are no exceptions. This is not negotiable.

### Where tests live

| What | Where |
|---|---|
| Pure logic (crypto, models, color math, backoff) | `#[cfg(test)]` module at the bottom of the same file |
| Provider HTTP behaviour (Hue, Govee, future integrations) | `#[cfg(test)]` module in the provider file, using `wiremock` |
| API layer (Axum routes, auth, DB) | `tests/api.rs` using an in-memory SQLite app fixture |
| Cross-cutting integration | `tests/` as named files |

### Rules

- **New provider?** The `LightProvider` impl AND the `ProviderFactory::build` path must both be covered by wiremock tests before the code is considered done.
- **New API route?** At minimum: happy path + unauthenticated request returns 401.
- **New crypto helper?** Roundtrip test + at least one failure-mode test (wrong key, tampered data).
- **The full CI gate must pass locally** before any change is considered complete — CI runs all three and fails on any:
  ```
  cargo fmt --check
  cargo clippy --all-targets -- -D warnings
  cargo test
  ```
  Running only `cargo test` is not enough; fmt and clippy (with `-D warnings`) are equally blocking.
- Do not silence warnings with `#[allow(dead_code)]` to make tests pass. Fix the code.

### Test style

- Prefer real behaviour over mocks. Use `wiremock` for external HTTP; use real in-memory SQLite for DB tests.
- Test public contracts, not implementation details. Avoid testing private internals directly.
- Each test has one clear assertion purpose. Name it after the behaviour: `discover_returns_empty_list_when_bridge_has_no_lights`, not `test1`.
- Inline test helpers are fine; large shared fixtures go in `tests/helpers.rs`.

## Other directives

- The `ProviderRegistry` is the single place where provider types are registered. Do not add provider-type match arms anywhere else.
- Credentials are encrypted with AES-256-GCM before persisting. Never store plaintext credentials in the DB.
- The Hue connection manager must be the only code that reconnects to the bridge SSE stream. Do not open a second stream anywhere.
- The Floor Planner is a visualization layer only. Device and room controls there must be fly-outs that reuse the shared control components and API actions the Dashboard/Rooms use — never reimplement control UI or logic per view.
- The session API (`/api/*`) and the public API (`/api/v1/*`) delegate to the **same shared service functions** (in `api::lights` / `api::audio` / `api::rooms`). Put control/behaviour in the service layer, not duplicated per router, so the two surfaces can't drift.
- Migrations are **append-only**: add the next `migrations/NNNN_*.sql`; never edit a migration that has shipped.
- Frontend has no CSS framework — inline style objects plus `frontend/src/styles.ts` (`S`, `ACCENT`). Reuse the shared control components (`LightEditor`, `components/scenes`, `components/dialogs`) rather than re-implementing controls.
- **Responsive:** inline styles can't express media queries, so branch layout on the **`useViewport()`** hook (`frontend/src/useViewport.ts`) — `isMobile` (≤640px) is the switch; **tablets get the desktop layout**. Phones use a bottom tab bar (`App.tsx`), and anchored fly-outs (`LightEditor`/`AudioEditor`) become bottom sheets via `components/sheet.ts`.
- **Reliability is a primary goal** (it's why Bifrost exists vs Home Assistant). Connection managers own reconnection with shared backoff; a live read that can't reach a device falls back to cached state with `reachable: false` rather than failing the whole request.
- **Keep docs in sync as part of the change:** new `/api/v1` route → `API.md`; new assistant-facing capability → `MCP.md`; new provider → the Current providers table above.

## Adding a provider

1. Implement the factory + `LightProvider`/`AudioProvider` (and `discover_groups` if the provider has native rooms/zones to wrap into Bifrost Rooms).
2. Register it in `ProviderRegistry::default_registry()` (`src/providers/mod.rs`) — the only place provider types are registered.
3. Add a row to the **Current providers** table above.
4. wiremock tests for the provider impl **and** the factory `build` path (mandatory — see Test coverage).
5. Declare `credentials_schema` so the add-provider form renders the right fields.

## See also

`README.md` (architecture, schema, dev commands), `PLAN.md` (roadmap), `API.md` (public API), `MCP.md` (companion MCP tool roadmap).
