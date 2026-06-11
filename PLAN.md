# Bifrost — Implementation Plan

## Goals

A self-hosted Rust smart home hub that is:
- **More reliable than Home Assistant** for Hue — explicit SSE reconnect, polling fallback, no silent drop
- **Minimal surface area** — one binary, one SQLite file, one Docker image
- **Extensible without code surgery** — new providers plug in via `ProviderFactory` only

---

## Milestone 0 — Foundation ✅ DONE

All tests green (43/43).

### Backend
- [x] Rust lib + bin crate split (`bifrost` lib, `bifrost` bin)
- [x] SQLite via sqlx (runtime queries, in-memory for tests, file for prod)
- [x] Migrations (`migrations/0001_init.sql`) — config, providers, lights, sessions tables
- [x] AES-256-GCM credential encryption (`src/crypto.rs`)
- [x] Argon2id password hashing
- [x] Session management — HttpOnly + SameSite=Strict cookie, 7-day TTL, `sessions` table
- [x] `ProviderFactory` + `ProviderRegistry` — zero match arms outside the registry
- [x] Philips Hue CLIP v2 provider — REST + SSE event stream (`src/providers/hue/`)
- [x] Govee cloud API v2 provider (`src/providers/govee/`)
- [x] `HueConnectionManager` state machine — Disconnected / Connecting / Connected / Reconnecting / Failed
- [x] Exponential backoff (base 1s, cap 60s, ±20% jitter) + polling fallback
- [x] Axum 0.8 REST API — auth, lights, providers
- [x] rust-embed bakes `frontend/dist/` into the binary
- [x] Alpine Docker multi-stage build

### Tests
- [x] `src/crypto.rs` — roundtrip, empty, unique ciphertext, wrong key, tampered, too-short, short secret
- [x] `src/models/mod.rs` — rgb roundtrip, gamut clamp
- [x] `src/providers/mod.rs` — 6 registry unit tests (mock factory)
- [x] `src/providers/hue/mod.rs` — 7 wiremock tests
- [x] `src/providers/govee/mod.rs` — 5 wiremock tests
- [x] `src/connection/mod.rs` — 4 backoff unit tests
- [x] `tests/api.rs` — 12 integration tests (health, auth, lights auth guard, providers)

---

## Milestone 1 — Server Completeness ✅ DONE

50 tests green.

### 1.1 First-run setup endpoint

`POST /api/setup` — sets the password when no config row exists.

```
POST /api/setup
{ "password": "..." }
→ 200 on first call; 409 Conflict thereafter
```

- Insert into `config (id=1, password_hash, setup_complete=1)`
- Return 409 if `setup_complete = 1` already
- Tests: happy path, duplicate call returns 409, weak password rejected (min length)

`GET /api/setup/status` — lets the frontend know whether to show the setup page.

```
GET /api/setup/status
→ { "setup_complete": false }
```

### 1.2 Wire HueConnectionManager into the runtime

`HueConnectionManager` exists but is never started. On app startup, for each enabled Hue provider row, spawn a connection manager task.

- `lib.rs::run()` — after building app state, query `providers WHERE provider_type = 'hue' AND enabled = 1`, build each `HueProvider`, spawn `HueConnectionManager::run()` as a tokio task
- Store `Arc<HueConnectionManager>` per provider ID in `AppState` (or a new `ConnectionRegistry`)
- Managers must be stopped when a provider is deleted via `DELETE /api/providers/{id}`

### 1.3 Persist SSE state to the database

Light-state updates from the SSE stream should be written to `lights.last_state` / `lights.last_seen` so the REST API returns fresh state without requiring manual discovery.

- Subscribe to `HueConnectionManager.events` broadcast channel in a DB writer task
- Match `LightEvent.device_id` against `lights.device_id` and upsert
- This is the core reliability win over Home Assistant

### 1.4 Connection-status API

```
GET /api/providers/{id}/status
→ { "state": "connected", "since_secs": 1240, "last_event_secs": 4 }
```

Exposes `ConnectionState` from each manager so the UI can show a live indicator.

### 1.5 Enhanced health endpoint

```
GET /api/health
→ { "ok": true, "providers": [{ "id": "...", "name": "...", "state": "connected" }] }
```

---

## Milestone 2 — Frontend (React/Vite SPA) ✅ DONE

`tsc && vite build` clean. 50 Rust tests still green.

### 2.1 Project scaffold

```
frontend/
  src/
    api.ts          # typed fetch wrappers for all REST endpoints
    main.tsx
    App.tsx         # route guard: redirect to /setup or /login if needed
    pages/
      Setup.tsx     # first-run password form
      Login.tsx
      Dashboard.tsx # light list + controls
      Settings.tsx  # providers: list, add, delete, discover
    components/
      LightCard.tsx        # on/off toggle, brightness slider, color picker
      ProviderStatus.tsx   # connection state badge
      AddProviderForm.tsx  # dynamic form from /api/providers/types schema
```

### 2.2 Setup page (`/setup`)

- Rendered when `GET /api/setup/status` returns `{ "setup_complete": false }`
- Single password + confirm form → `POST /api/setup`
- Redirects to login on success

### 2.3 Login page (`/login`)

- Password form → `POST /api/auth/login`
- On success the browser receives the `bifrost_session` cookie; redirect to `/`

### 2.4 Dashboard (`/`)

- `GET /api/lights` → grid of `LightCard`s
- Each card: name, on/off toggle, brightness slider (if dimmable), color picker (if color_rgb)
- Toggle/slider commits `PUT /api/lights/{id}` with debounce (~200ms for sliders)
- Empty state with call-to-action to add a provider

### 2.5 Settings (`/settings`)

- **Providers tab**
  - List from `GET /api/providers`
  - Connection status badge per provider from `GET /api/providers/{id}/status`
  - "Discover lights" button → `POST /api/providers/{id}/discover`
  - "Remove" → `DELETE /api/providers/{id}` with confirmation dialog
  - "Add provider" → drawer with provider-type picker + dynamic form from `/api/providers/types` schema
- **Security tab** (future: change password)

### 2.6 Build integration

- `npm run build` outputs to `frontend/dist/`
- `Dockerfile` already runs this before `cargo build`
- Dev: `VITE_API_BASE=http://localhost:3000` proxy in `vite.config.ts`

---

## Milestone 3 — Real-Time Push ✅ DONE

53 Rust tests green. Frontend TypeScript build clean.

The dashboard currently needs a page refresh to see state changes. Fix with a push channel.

### 3.1 Server-Sent Events endpoint for the frontend

```
GET /api/events          (requires session cookie)
Content-Type: text/event-stream

data: {"type":"light_state","device_id":"abc","state":{...}}
data: {"type":"provider_status","provider_id":"xyz","state":"reconnecting"}
```

- Subscribe to the `HueConnectionManager` broadcast channel(s)
- Write each `LightEvent` as an SSE frame
- Keep-alive ping every 15s
- Session auth same as REST endpoints

### 3.2 Frontend event consumer

- `useEffect` opens `EventSource('/api/events')` on Dashboard mount
- On `light_state` event: update the matching card's state in React state
- On `provider_status` event: update the badge in Settings

---

## Milestone 4 — Additional Providers ✅ WLED DONE

61 tests green.

The registry pattern makes this mechanical. Each new provider is:
1. New directory `src/providers/<name>/mod.rs`
2. Implement `LightProvider` (discover, get_state, set_state)
3. Implement `ProviderFactory` (provider_type, build, credentials_schema)
4. One line in `default_registry()`: `r.register(NameFactory);`
5. wiremock tests for all three trait methods

### Completed

- [x] **WLED** — `src/providers/wled/mod.rs`. `GET /json/info` + `GET /json/state` for discovery; `POST /json/state` for control. bri 0–255 ↔ brightness 0–100. Segment colour via `seg[0].col[0]` [R,G,B]. 8 wiremock tests.
- [x] `migrations/0002_drop_provider_type_constraint.sql` — removes hard-coded `CHECK (provider_type IN ('hue','govee'))` by recreating the table. No further schema migrations needed for new providers.

- [x] **Tasmota** — `src/providers/tasmota/mod.rs`. `GET /cm?cmnd=Status 0` for discovery; `GET /cm?cmnd=State` for state; `GET /cm?cmnd=Backlog Power {ON|OFF}[; Dimmer {0-100}][; Color {RRGGBB}]` for control. Dimmer 0–100 maps directly to brightness. 8 wiremock tests.
- [x] **Shelly Gen1** — `src/providers/shelly/mod.rs`. `GET /settings` for name; `GET /light/0` for state; `GET /light/0?turn={on|off}&brightness={0-100}` for control. 8 wiremock tests.

### Remaining candidates (priority order)

| Provider | Protocol | Notes |
|---|---|---|
| LIFX | UDP LAN + HTTPS cloud | LAN preferred; UDP makes this significantly more complex than REST providers |

---

## Milestone 4.5 — Functional Hue + Govee ✅ DONE

93 tests green. Closes the gaps between "compiles and passes tests" and "usable":

- [x] **Hue link-button pairing** — `src/providers/hue/pairing.rs` + `POST /api/providers/hue/pair`. User enters the bridge IP, presses the link button, clicks Pair; the server POSTs `{"devicetype":"bifrost#server","generateclientkey":true}` and returns the app key (409 if the button wasn't pressed, 502 if unreachable). No more manual curl. 5 wiremock + 4 API tests.
- [x] **`ConnectionMode` on `ProviderFactory`** — `Sse` (Hue) or `Poll{interval_secs}` (default 120s, everything else). Startup and `POST /api/providers` dispatch via `start_manager_for()`; the `provider_type == "hue"` string match in the API layer is gone.
- [x] **`PollingManager`** — keeps Govee/WLED/Tasmota/Shelly state fresh without a push channel: discover → per-device `get_state` → broadcast on the same `LightEvent` pipeline as Hue SSE, so `/api/events` and the DB writer work identically. Connected/Reconnecting state machine with the shared backoff. 4 unit tests with a scripted provider.
- [x] **Frontend: Hue pair flow** — Add-provider form grows a Pair button on the app-key field (hue only), with link-button guidance and auto-fill on success.
- [x] **Frontend: auto-discovery** — adding a provider immediately runs discovery; lights appear without a second click.
- [x] **Frontend: color picker** — `LightCard` shows a color input for `color_rgb` lights; hex → CIE xy via the same Wide RGB D65 matrix as the server (`rgbToXy` in `api.ts`), 200ms debounce.

---

## Milestone 5 — Production Readiness

### 5.1 Scenes ✅ DONE

`migrations/0003_scenes_and_groups.sql` + `src/api/scenes.rs`. 106 tests green.

- [x] `GET /api/scenes` — list with per-scene light counts
- [x] `POST /api/scenes {name}` — snapshot `last_state` of every light
- [x] `POST /api/scenes/{id}/activate` — apply all entries in parallel via providers; returns `{applied, failed}`
- [x] `DELETE /api/scenes/{id}`
- [x] Dashboard scene bar: activate / save / delete
- [x] FK enforcement enabled on the SQLite pool (`foreign_keys(true)`) — the schema's `ON DELETE CASCADE` clauses were inert before this
- [x] Tests: 401, snapshot, empty-name 422, activate-via-wiremock-device, 404, delete

### 5.2 Light groups / rooms ✅ DONE (API)

`src/api/groups.rs`:

- [x] `GET /api/groups` — list with member light IDs
- [x] `POST /api/groups {name, light_ids}` / `DELETE /api/groups/{id}`
- [x] `PUT /api/groups/{id}/lights` — replace membership
- [x] `PUT /api/groups/{id}/state` — broadcast a state to all members in parallel; `{applied, failed}`
- [x] Tests: 401, create+list, empty-name 422, group state via wiremock device, 404, membership replace, delete
- [x] Groups UI — Settings: create groups with light checkboxes, edit membership, delete; Dashboard: per-group On/Off chips

### 5.3 Schedules / automations

Stretch goal. Cron-style triggers stored in SQLite, executed by a tokio interval task.

### 5.4 Docker Compose reference ✅ DONE

`docker-compose.yml` at the repo root (BIFROST_SECRET required via `.env`), plus
`README.md` with quickstarts (Docker + bare binary), an env-var table, provider
setup notes, API overview, and `LICENSE` (MIT).

### 5.5 Observability ✅ PARTIAL

- [x] `RUST_LOG` documented in README and set in compose (`bifrost=info`)
- [x] `GET /api/health` now reports `version` + `uptime_secs` alongside per-provider connection state
- [ ] Optional: Prometheus metrics endpoint (`/metrics`) — reconnect counts, event rates

---

## Out of scope (for now)

- Multi-user / RBAC — single shared password is the design
- MQTT broker — would enable many devices but adds operational complexity
- Mobile app — the SPA is responsive; native apps are not planned
- Cloud relay / remote access — use Tailscale/VPN at the network layer

---

## Key invariants (enforced by CLAUDE.md)

- Every public function and non-trivial helper has test coverage. `cargo test` must be green.
- New provider: wiremock tests before the code is considered done.
- New API route: happy path + unauthenticated-returns-401.
- Credentials encrypted with AES-256-GCM. Never stored in plaintext.
- `HueConnectionManager` is the only code that opens the bridge SSE stream.
- `ProviderRegistry` is the only place provider types are registered.

------

new feature:

a a "DIY" grid feature where a house can be represented by building a floor plan using 1x1 "sqquare foot" files (think Sims 4 houses but 2d) and lights can be placed. The "light"s should correspond to the actual lights. 

