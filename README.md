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
echo "BIFROST_SECRET=$(openssl rand -hex 32)" > .env
docker compose up -d
```

Then open `http://<host>:3000`.

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

## License

MIT
