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
4. **Scenes** — set your lights how you like them, then *+ Save scene* on the
   dashboard. Activating a scene re-applies every captured state in parallel.
5. **Groups** — Settings → Groups: pick lights into a room; the dashboard
   gets per-group On/Off chips.
6. **Floor plan** — the *Plan* tab. Paint your house layout (floor tiles +
   thin walls, Sims-style), then place your real lights on it — wall-mounted
   on edges or ceiling-mounted in tile centres. The plan doubles as a live
   dashboard: lights glow with their actual color and brightness, update in
   real time, and toggle on click. Multiple lights on one spot (a multi-bulb
   fixture) cluster into a single dot with a ×N badge.

## Configuration

Everything is configured via environment variables (a `.env` file works too):

| Variable | Default | Notes |
|---|---|---|
| `BIFROST_SECRET` | — (required) | Encrypts provider credentials at rest with AES-256-GCM. 32+ random chars. Changing it orphans stored credentials. |
| `DATABASE_URL` | `sqlite://bifrost.db` | SQLite only. In Docker: `sqlite:///data/bifrost.db` on a volume. |
| `BIND_ADDR` | `0.0.0.0:3000` | Listen address. |
| `RUST_LOG` | — | e.g. `bifrost=info` or `bifrost=debug`. |

## Providers

| Provider | Transport | Live updates | Setup |
|---|---|---|---|
| Philips Hue | LAN (CLIP v2) | SSE push | Bridge IP + link-button pairing in the UI |
| Govee | Cloud API v2 | Polling | API key from the Govee Home app |
| WLED | LAN REST | Polling | Device IP |
| Tasmota | LAN REST | Polling | Device IP |
| Shelly (Gen1) | LAN REST | Polling | Device IP |

Adding a provider type is intentionally mechanical: implement two traits, register one factory line, write wiremock tests. See `src/providers/wled/mod.rs` for the template and `CLAUDE.md` for the rules.

## API

All endpoints are under `/api` and (except setup/login/health) require the session cookie.

```
POST /api/setup                      first-run password
POST /api/auth/login                 → session cookie
GET  /api/health                     { ok, version, uptime_secs, providers[] }

GET  /api/lights                     cached lights
PUT  /api/lights/{id}                set state { on, brightness, color, color_temp_mirek }
GET  /api/events                     SSE stream of live light-state events

GET  /api/providers                  configured providers
POST /api/providers                  add { name, provider_type, credentials }
POST /api/providers/hue/pair         link-button pairing → { app_key }
POST /api/providers/{id}/discover    refresh device list
GET  /api/providers/{id}/status      connection state machine snapshot

GET/POST /api/scenes                 list / snapshot current states
POST /api/scenes/{id}/activate       apply in parallel
GET/POST /api/groups                 list / create
PUT  /api/groups/{id}/state          broadcast one state to all members

GET/POST /api/plans                  floor plans
PUT  /api/plans/{id}/layout          tiles + walls (bulk editor save)
PUT  /api/plans/{id}/lights          light placements (tile + mount point)
```

## Development

```sh
cargo test                                 # 116 tests: unit + wiremock + API integration
cargo clippy --all-targets -- -D warnings  # CI-enforced
cd frontend && npm run build               # tsc + vite
```

The test suite never touches the network — external HTTP is wiremock, the DB is in-memory SQLite. CI runs fmt, clippy, tests, and the frontend build on every push; tags matching `v*` publish a Docker image to GHCR and create a GitHub release.

## Rooms architecture (proposal — under review)

> Status: design for review, not yet implemented. The goal: **Bifrost Rooms
> become an abstraction layer over imported provider groups**, instead of a
> third competing copy of "which lights belong together".

### The problem today

Three things currently claim to define groupings, and they fight:

1. **Manual groups** — created in Settings, membership hand-picked.
2. **Imported provider groups** — "Import rooms" copies Hue rooms/zones into
   local groups *once*; rename a room in the Hue app and Bifrost drifts.
3. **Planner room auto-groups** — each painted room owns a group whose
   membership is overwritten from tile placements on every save.

A Hue room "Office" imported as a group and a planner room "Office" collide
on name, and each save/import silently rewrites the other's membership.

### Proposed model

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

### Schema sketch

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

- If every member from provider P came via one linked provider group **and P
  supports native group control** (Hue `grouped_light`: one API call for the
  whole room), use it — faster and atomic, exactly how the Hue app behaves.
- Otherwise fan out per light in parallel (current behaviour).

### Planner binding

Painting a region binds the Room to floor space; it no longer *defines*
membership. Placed lights inside a region that aren't already members
(via links or direct) prompt: "Add to room?" — placement suggests, the Room
decides. (Open question 3 offers a stricter alternative.)

### Migration of existing data

- Existing groups created by "Import rooms" → become `provider_groups`
  mirrors + a Room linking each.
- Existing planner-room groups → Rooms with their plan region; current
  members become direct lights.
- Remaining manual groups → Rooms with direct lights.
- `group_scenes` rows move to `room_scenes`.

### Open questions for review

1. **Sync cadence** — manual "Sync" button only, or also automatic
   (piggyback on the existing polling cycle)?
2. **One room, many floors?** Can a Room have regions on multiple plans
   (e.g. a stairwell)? Proposal: yes, region rows are per-plan.
3. **Placement semantics** — prompt-to-add (proposed) vs. auto-add placed
   lights as direct members (current behaviour, surprise-prone) vs. purely
   visual (placement never affects membership).
4. **Naming collisions** — when a sync renames hue/Office to "Studio", does
   the Room auto-rename if it was created from that link? Proposal: only if
   the Room still has the exact name it inherited.

## License

MIT
