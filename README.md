<p align="center">
  <img src="frontend/public/favicon.svg" width="96" height="96" alt="Bifrost" />
</p>

<h1 align="center">Bifrost</h1>

<p align="center">
  A self-hosted smart-home hub — lights, audio, and power behind one fast UI, a clean API, and voice control. One binary, one SQLite file, one Docker image.
</p>

---

Bifrost unifies your smart-home devices behind a single, fast, self-hosted control surface — a web dashboard, a REST API, an embedded assistant (MCP) server, and natural-language voice. It is built around **reliability**: every device connection is owned by a state machine with reconnection, backoff, and live push to the browser, and a read that can't reach a device falls back to cached state rather than failing.

## What it does

- **Rooms are the core abstraction.** A Room aggregates any mix of devices — lights, speakers/receivers, switches and plugs — and is the high-level control surface: power the whole room, set brightness across its lights, fan volume/mute out to its audio members (each with a per-room loudness offset). Provider-native groupings (e.g. Hue rooms/zones) are mirrored and wrapped into Rooms with one **Sync** click.
- **Three device domains, modelled honestly.** Lights (full RGB + color-temperature + brightness), audio (receivers, speakers, zones — power, volume, mute, source/streaming-service, transport, now-playing), and power (strictly on/off switches, plugs, fans). Each keeps its own state shape rather than being forced into a generic blob.
- **Scenes.** Snapshot your lights and re-apply them in parallel, plus per-room **palette scenes** (a name + brightness + a color palette spread across the room) with presets.
- **Floor planner.** Paint a rough 2D plan of your home (floor tiles + walls), drop devices roughly where they physically are, and bind painted regions to Rooms. The plan doubles as a live dashboard — devices glow with their real color/brightness and open the same controls used everywhere else.
- **Voice control.** Speak commands in natural language; a deterministic grammar handles the common cases instantly, and anything it can't parse falls through to a local LLM that maps it to the same actions (see [Voice & assistants](#voice--assistants)).
- **Everything is exposed.** A key-authenticated public API and an embedded **Model Context Protocol** server let external apps and AI assistants drive the whole home.

## Providers

Devices are added through **providers**. Each is added in the UI (Settings → Add Provider) and discovered automatically.

### Lights

| Provider | Transport | Live updates | Setup |
|---|---|---|---|
| Philips Hue | LAN (CLIP v2) | SSE push | Bridge IP + link-button pairing in the UI |
| Govee | Cloud API | Polling | API key from the Govee Home app |
| LIFX | Cloud API | Polling | Account token from the LIFX app; LIFX groups import as Rooms |

### Audio

| Provider | Transport | Live updates | Setup |
|---|---|---|---|
| Onkyo / Integra | LAN (eISCP) | Push (persistent socket) | Receiver IP; enable Network Standby for remote power-on |

Receivers expose power, volume, mute, input/streaming-service selection, playback transport, and now-playing metadata; a second output (e.g. zone 2) appears as its own device. A **source** device (TV, streamer, console) can be **bound to a receiver** that owns its volume, so "turn the TV up" routes to the right box.

Every IP-addressable provider supports **auto-detect** — a "Scan network for devices" button finds devices on the LAN and fills in the address. Cloud providers take an account token instead.

> Adding a provider is intentionally mechanical: implement the provider trait, register one factory line, write tests. The shared discovery engine and connection managers do the rest.

## Voice & assistants

Bifrost is designed to be driven by voice and by AI assistants, three ways:

- **Native voice pipeline.** A deterministic grammar parses spoken commands (power, brightness, color, color-temperature, volume, mute, transport, scenes, relative nudges) and resolves targets by room/device name. It's fast and offline.
- **LLM fallback.** Any phrasing the grammar can't parse is handed to a configurable **chat model** (any OpenAI-compatible endpoint — e.g. a local model via Ollama) that maps it to exactly one action through the *same* dispatch path. Configure the `chat` endpoint in Settings; with none configured, the native pipeline simply handles what it can.
- **Embedded MCP server.** A first-class Model Context Protocol surface at `/mcp` exposes the home as assistant tools, so an AI client can control lights, audio, power, rooms, and scenes in natural language. See [MCP.md](MCP.md).

The **public API** (`/api/v1`, Bearer-key, mint keys in Settings → API keys) covers lights, rooms, scenes, audio, and power — documented in [API.md](API.md). Devices can be paired to headless clients by scanning a QR code (no key typing).

## Install

### Docker (recommended)

```sh
mkdir bifrost && cd bifrost
curl -fsSLO https://raw.githubusercontent.com/others-git/bifrost/main/docker-compose.yml
test -f .env || echo "BIFROST_SECRET=$(openssl rand -hex 32)" > .env
docker compose up -d
```

Then open `http://<host>:3000`.

The bundled compose file uses `network_mode: host` so device auto-detect can reach the LAN (SSDP/eISCP broadcast and the subnet sweep don't cross a bridged container's NAT). To run bridged, swap `network_mode: host` for a `ports:` mapping — runtime control of already-added devices still works, but network scanning won't find anything.

**Keep `BIFROST_SECRET` identical across upgrades** — it encrypts your provider credentials at rest. If it changes, the app starts but logs `failed to decrypt credentials` and providers show disconnected; recovery is restoring the original secret or re-entering credentials.

Upgrade with `docker compose pull && docker compose up -d`.

### Bare binary

Requires Rust (stable) and Node 20+.

```sh
cd frontend && npm ci && npm run build && cd ..
BIFROST_SECRET=$(openssl rand -hex 32) cargo run --release
```

## First-run setup

1. **Set a password** — one password protects the hub (designed for LAN/VPN; put it behind Tailscale or a reverse proxy for remote access).
2. **Add a provider** — Settings → Add Provider; discovery runs automatically and devices appear on the dashboard with live controls.
3. **Build Rooms** — Settings → Rooms: combine synced provider groups with directly-assigned devices.
4. **Scenes & floor plan** — save scenes from the dashboard; paint your layout on the Floor Plan tab and place devices on it.

## Configuration

Everything is configured via environment variables (a `.env` file works too):

| Variable | Default | Notes |
|---|---|---|
| `BIFROST_SECRET` | — (required) | Encrypts provider credentials at rest with AES-256-GCM. 32+ random chars. Changing it orphans stored credentials. |
| `DATABASE_URL` | `sqlite://bifrost.db` | SQLite only. In Docker: `sqlite:///data/bifrost.db` on a volume. |
| `BIND_ADDR` | `0.0.0.0:3000` | Listen address. |
| `RUST_LOG` | — | e.g. `bifrost=info` or `bifrost=debug`. |

## Wall-tablet & companion repos

- **[bifrost-kiosk](https://github.com/others-git/bifrost-kiosk)** — a native Android app that turns a wall-mounted tablet into a hard-locked, always-on Bifrost fixture: a full-screen dashboard plus an offline wake-word voice satellite. Pair it by scanning a QR code.
- **[bifrost-skills](https://github.com/others-git/bifrost-skills)** — reusable hardware setup runbooks (starting with the tablet wall-kiosk).

## AI usage disclosure

Bifrost is built with heavy use of AI assistance — primarily [Claude Code](https://claude.com/claude-code). AI contributed to code, tests, and documentation throughout. Every change is gated by the same CI as any other (`cargo fmt`, `cargo clippy -D warnings`, the full test suite, and the frontend build), and AI-assisted commits carry a `Co-Authored-By: Claude` trailer.

## License

MIT
