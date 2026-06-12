<p align="center">
  <img src="frontend/public/favicon.svg" width="96" height="96" alt="Bifrost" />
</p>

<h1 align="center">Bifrost</h1>

<p align="center">
  A self-hosted smart home hub for lights. One binary, one SQLite file, one Docker image.
</p>

---

Bifrost bridges your light ecosystems — Philips Hue, Govee, WLED, Tasmota, Shelly — behind a single fast web UI and REST API, with a focus on **connection reliability**: explicit reconnect state machines, exponential backoff, polling fallback, and live push to the browser.

## Why

Smart lighting lives and dies by connection reliability: event streams drop silently, states go stale, and lights stop responding until someone notices. Bifrost treats the connection as the product:

- The Hue SSE stream is owned by a dedicated connection manager with a visible state machine (`connected` / `reconnecting` / `failed`), exponential backoff with jitter, health-check pings, and polling fallback during outages.
- Providers without a push channel (Govee, WLED, Tasmota, Shelly) are kept fresh by a polling manager that feeds the same event pipeline — the UI updates live either way.
- Connection state per provider is always one API call away (`GET /api/providers/{id}/status`) and shown as a badge in the UI.

## Install

### Docker (recommended)

```sh
mkdir bifrost && cd bifrost
curl -fsSLO https://raw.githubusercontent.com/others-git/bifrost/main/docker-compose.yml
test -f .env || echo "BIFROST_SECRET=$(openssl rand -hex 32)" > .env
docker compose up -d
```

Then open `http://<host>:3000`.

### Upgrading

```sh
docker compose pull && docker compose up -d
```

**Keep `BIFROST_SECRET` identical across upgrades** — it is the key that
encrypts your provider credentials. If it changes, the app starts fine but
logs `failed to decrypt credentials` and providers show as disconnected.
Recovery: restore the original secret, or re-enter each provider's
credentials via Settings → **Edit credentials** (lights, scenes, groups, and
floor plans are not affected either way).

### Unraid

A Community Applications template lives at
[`environment/unraid/bifrost.xml`](environment/unraid/bifrost.xml).
Until it's published in CA: **Docker → Add Container → Template** and paste the
raw URL of that file, or drop it into
`/boot/config/plugins/dockerMan/templates-user/`. Fill in the Secret Key
(`openssl rand -hex 32`) — everything else has sane defaults.

### Bare binary

Requires Rust (stable) and Node 20+.

```sh
cd frontend && npm ci && npm run build && cd ..
BIFROST_SECRET=$(openssl rand -hex 32) cargo run --release
```

## First-run setup

1. **Set a password** — the first visit shows the setup page. One password
   protects the whole hub (designed for LAN/VPN; put it behind Tailscale or a
   reverse proxy for remote access).
2. **Add a provider** — Settings → Add Provider:
   - **Hue**: enter the bridge IP (Hue app → Settings → My Hue system →
     Bridge), press the round link button on the bridge, click **Pair**. The
     app key is fetched and filled in automatically.
   - **Govee**: API key from the Govee Home app (Profile → About Us → Apply
     for API Key).
   - **WLED / Tasmota / Shelly**: just the device IP.
3. **Discover** — runs automatically after adding; lights appear on the
   dashboard with live on/off, brightness, and full-RGB color controls.
4. **Tune a light** — click any light card to open the editor: a Hue-style
   color wheel and a vertical brightness bar, anchored next to the light.
   Brightness and color commit with a short debounce.
5. **Scenes** — set your lights how you like them, then *+ Save scene* on the
   dashboard. Activating a scene re-applies every captured state in parallel.
   Rooms also carry palette scenes (a name + brightness + colors spread across
   the room's lights), with Hue-like presets.
6. **Rooms** — Settings → Rooms: combine synced provider rooms/zones with
   directly assigned lights; the dashboard groups lights by room with per-room
   On/Off.
7. **Floor plan** — the *Floor Plan* tab. Paint your house layout (floor tiles +
   thin walls, Sims-style), then place your real lights on it — wall-mounted
   on edges or ceiling-mounted in tile centres, and drag to lay an LED strip
   that can corner. The plan doubles as a live dashboard: lights glow with
   their actual color and brightness, update in real time, and open the editor
   on click. Multiple lights on one spot (a multi-bulb fixture) cluster into a
   single dot with a ×N badge.

## Configuration

Everything is configured via environment variables (a `.env` file works too):

| Variable | Default | Notes |
|---|---|---|
| `BIFROST_SECRET` | — (required) | Encrypts provider credentials at rest with AES-256-GCM. 32+ random chars. Changing it orphans stored credentials. |
| `DATABASE_URL` | `sqlite://bifrost.db` | SQLite only. In Docker: `sqlite:///data/bifrost.db` on a volume. |
| `BIND_ADDR` | `0.0.0.0:3000` | Listen address. |
| `RUST_LOG` | — | e.g. `bifrost=info` or `bifrost=debug`. |

## Providers

### Lights

| Provider | Transport | Live updates | Setup |
|---|---|---|---|
| Philips Hue | LAN (CLIP v2) | SSE push | Bridge IP + link-button pairing in the UI |
| Govee | Cloud API v2 | Polling | API key from the Govee Home app |
| Govee LAN | LAN (UDP) | Polling | LAN control enabled in the Govee Home app |
| WLED | LAN REST | Polling | Device IP |
| Tasmota | LAN REST | Polling | Device IP |
| Shelly (Gen1) | LAN REST | Polling | Device IP |

### Audio

| Provider | Transport | Live updates | Setup |
|---|---|---|---|
| Onkyo / Integra | LAN (eISCP, TCP 60128) | Push (persistent socket) | Receiver IP; enable Network Standby for remote power-on |
| Sonos | LAN (UPnP SOAP, port 1400) | Polling | Any one player's IP — the rest of the household is discovered from it |

Receivers expose power, volume, mute, input selection (including one-call
streaming-service switching: `spotify`, `tunein`, `deezer`, `tidal`, `airplay`),
playback transport, and now-playing metadata. Onkyo zone 2 appears as its own
device; Sonos playback groups appear as zone devices with group volume.
Volume-knob turns and track changes on an Onkyo push to the UI instantly.
Rooms can link an audio device (♪ on the Floor Plan room card) for in-room
volume/mute.

Adding a provider type is intentionally mechanical: implement two traits, register one factory line, write wiremock tests. See `src/providers/wled/mod.rs` for the template (lights), `src/providers/sonos/mod.rs` (audio), and `CLAUDE.md` for the rules.

## API

**Public API for external apps:** `/api/v1` is key-authenticated (mint keys in
Settings → API keys) and documented in [API.md](API.md) — lights, rooms, scenes,
and audio devices. The companion **[bifrost-mcp](../bifrost-mcp)** project wraps
it as Model Context Protocol tools so an AI assistant can drive the whole home.

All endpoints below are under `/api` and (except setup/login/health) require the session cookie.

```
POST /api/setup                      first-run password
POST /api/auth/login                 → session cookie
GET  /api/health                     { ok, version, uptime_secs, providers[] }

GET  /api/lights                     cached lights
PUT  /api/lights/{id}                set state { on, brightness, color, color_temp_mirek }
GET  /api/events                     SSE stream of live light_state + audio_state events

GET  /api/audio/devices              audio devices (receivers, speakers, zones)
GET  /api/audio/devices/{id}         live state read (refreshes the cache)
PUT  /api/audio/devices/{id}/state   { power?, volume?, mute?, source?, transport? }
PUT  /api/rooms/{id}/audio           link/unlink an audio device to a room

GET  /api/providers                  configured providers
POST /api/providers                  add { name, provider_type, credentials }
POST /api/providers/hue/pair         link-button pairing → { app_key }
POST /api/providers/{id}/discover    refresh device list
GET  /api/providers/{id}/status      connection state machine snapshot

GET/POST /api/scenes                 list / snapshot current states
POST /api/scenes/{id}/activate       apply in parallel

GET/POST /api/rooms                  list / create rooms (links + direct lights)
PUT  /api/rooms/{id}/state           broadcast one state to all members
POST /api/rooms/{id}/merge           absorb another room
GET/POST /api/rooms/{id}/scenes      list / create palette scenes
POST /api/rooms/{id}/scenes/{sid}/apply   apply a palette scene

GET/POST /api/plans                  floor plans
PUT  /api/plans/{id}/layout          tiles + walls (bulk editor save)
PUT  /api/plans/{id}/size            resize the grid (prunes out-of-bounds)
PUT  /api/plans/{id}/lights          light placements (tile + mount + strip)
PUT  /api/plans/{id}/rooms           painted room regions
```

## Development

```sh
cargo test                                 # 154 tests: unit + wiremock + API integration
cargo clippy --all-targets -- -D warnings  # CI-enforced
cd frontend && npm run build               # tsc + vite
```

The test suite never touches the network — external HTTP is wiremock, the DB is in-memory SQLite. CI runs fmt, clippy, tests, and the frontend build on every push; tags matching `v*` publish a Docker image to GHCR and create a GitHub release.

## Rooms architecture

> Status: **implemented**. Bifrost Rooms are an abstraction layer over synced
> provider-group mirrors — there is no separate "groups" concept.

### The model

```
  PROVIDER LAYER (synced mirrors, read-only)
  ┌──────────────────────────┐   ┌──────────────────────────┐
  │ hue • room "Office"      │   │ hue • zone "Downstairs"  │
  │   lights: L1, L2         │   │   lights: L1, L4         │
  └────────────┬─────────────┘   └────────────┬─────────────┘
               │ link                          │ link
               ▼                               ▼
  ROOM LAYER (Bifrost abstraction, user-owned)
  ┌─────────────────────────────────────────────────────────┐
  │ Room "Office"                                           │
  │  ├─ linked provider groups: hue/Office                  │
  │  ├─ direct lights: G1 (Govee desk strip — no native     │
  │  │                     grouping concept on that provider)│
  │  ├─ plan region: tiles on "Ground Floor"                │
  │  └─ scenes: "Relax", "Energize", custom palettes        │
  │                                                         │
  │  effective members = union(linked groups) ∪ direct      │
  └─────────────────────────────────────────────────────────┘
```

- **Provider groups are mirrors.** A sync (manual button now, periodic later)
  refreshes their names and members from the provider. They are never edited
  in Bifrost.
- **A Room references mirrors instead of copying them.** When the Hue app
  moves a bulb between rooms, the next sync updates the mirror and every
  Room linking it follows automatically — nothing to re-import.
- **Direct lights cover the gaps.** Providers without native grouping
  (Govee, WLED, single Tasmota/Shelly devices) attach to a Room directly.
- **Rooms are the only user-facing grouping.** Scenes (snapshot + palette),
  on/off fan-out, the dashboard chips, and the planner controller all hang
  off Rooms. Plain "groups" disappear from the UI.

Decisions (2026-06-11): sync stays manual for now; one plan region per room;
planner placements **add on save** (never remove — noted next to the Save
button); rename-follow is on while a room keeps its inherited name.

### Schema

```sql
provider_groups        id, provider_id, provider_group_id, name, kind (room|zone)
provider_group_lights  provider_group_id, light_id          -- refreshed on sync
rooms                  id, name
room_links             room_id, provider_group_id           -- the abstraction
room_lights            room_id, light_id                    -- direct additions
plan_rooms             ...existing, but → room_id (region binds to a Room)
room_scenes            ...existing group_scenes, but → room_id
```

### Control path

`PUT /api/rooms/{id}/state` resolves effective members, then per provider:

- If every member from provider P comes via one linked provider group **and P
  supports native group control** (Hue `grouped_light`: one API call for the
  whole room), use it — faster and atomic, exactly how the Hue app behaves.
- Otherwise fan out per light in parallel.

### Planner binding

Painting a region binds it to a Room (new regions create one). Lights placed
inside a region are **added** to its Room when the plan is saved — saving
never removes members; manage membership in Settings. Renaming a region
renames the Room and stops provider rename-follow (you took naming
ownership).

### Room scenes

Hue-like palette scenes hang off rooms: a name, an optional brightness, and
a color palette distributed round-robin across the room's lights (stable
order). The plan-view room controller has chips to apply them, an inline
editor, and presets (Relax, Energize, Read, Nightlight, Sunset, Aurora).

### Migration

The `0007_rooms` migration converts all pre-existing groups into Rooms with
direct lights (scenes and plan bindings follow). Which groups were provider
imports wasn't recorded, so run **Sync rooms** once per provider — mirrors
are created and linked to same-named Rooms automatically.

## License

MIT
