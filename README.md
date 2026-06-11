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

Home Assistant is a great platform, but its Hue integration is known to silently drop the bridge's event stream. Bifrost treats the connection as the product:

- The Hue SSE stream is owned by a dedicated connection manager with a visible state machine (`connected` / `reconnecting` / `failed`), exponential backoff with jitter, health-check pings, and polling fallback during outages.
- Providers without a push channel (Govee, WLED, Tasmota, Shelly) are kept fresh by a polling manager that feeds the same event pipeline — the UI updates live either way.
- Connection state per provider is always one API call away (`GET /api/providers/{id}/status`) and shown as a badge in the UI.

## Quickstart (Docker)

```sh
git clone https://github.com/others-git/bifrost && cd bifrost
echo "BIFROST_SECRET=$(openssl rand -hex 32)" > .env
docker compose up -d
```

Open `http://localhost:3000`, set a password, add a provider, discover lights.

## Quickstart (bare binary)

Requires Rust (stable) and Node 20+.

```sh
cd frontend && npm ci && npm run build && cd ..
BIFROST_SECRET=$(openssl rand -hex 32) cargo run --release
```

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
| Philips Hue | LAN (CLIP v2) | SSE push | Enter the bridge IP, press the link button, click **Pair** — Bifrost fetches the app key for you |
| Govee | Cloud API v2 | Polling | API key from the Govee Home app (Profile → About Us → Apply for API Key) |
| WLED | LAN REST | Polling | Device IP |
| Tasmota | LAN REST | Polling | Device IP |
| Shelly (Gen1) | LAN REST | Polling | Device IP |

Adding a provider type is intentionally mechanical: implement two traits, register one factory line, write wiremock tests. See `src/providers/wled/mod.rs` for the template and `CLAUDE.md` for the rules.

## Features

- **Single-password auth** — HttpOnly, SameSite=Strict session cookie. Designed for LAN/VPN use; put it behind Tailscale or a reverse proxy for remote access.
- **Live dashboard** — on/off, brightness, full-RGB color; state changes from physical switches or other apps stream in over `GET /api/events` (SSE).
- **Scenes** — snapshot every light's current state, re-apply in parallel with one click.
- **Groups** — control a room at once (`PUT /api/groups/{id}/state`).
- **Encrypted credentials** — provider secrets are AES-256-GCM encrypted before they touch the database.

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
```

## Development

```sh
cargo test                                 # 100+ tests: unit + wiremock + API integration
cargo clippy --all-targets -- -D warnings  # CI-enforced
cd frontend && npm run build               # tsc + vite
```

The test suite never touches the network — external HTTP is wiremock, the DB is in-memory SQLite. CI runs fmt, clippy, tests, and the frontend build on every push.

## License

MIT
