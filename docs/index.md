<p align="center">
  <img src="assets/favicon.svg" width="96" height="96" alt="Bifrost" />
</p>

<h1 align="center">Bifrost</h1>

<p align="center">
  A self-hosted smart-home hub — control your devices from one web UI, a REST API, and voice.
</p>

---

Bifrost is a self-hosted smart-home control hub. It brings your supported devices — direct integrations plus anything from Home Assistant — under one web dashboard, a REST API, an embedded assistant (MCP) server, and natural-language voice. It aims to keep device connections live and show each device's real state: connections reconnect on their own with backoff, and a read that can't reach a device falls back to cached state rather than failing the request. See **[Providers](providers.md)** for the full list of supported devices.

## What it does

- **Rooms are the core abstraction.** A Room aggregates any mix of devices — lights, speakers/receivers, switches and plugs — and is the high-level control surface: power the whole room, set brightness across its lights, fan volume/mute out to its media members (each with a per-room loudness offset). Provider-native groupings (e.g. Hue rooms/zones) are mirrored and wrapped into Rooms with one **Sync** click. Each room can also be given **configurable quick-control buttons** — one-tap power/volume/brightness over a chosen set of its devices, or a scene — that sit next to its power button on the dashboard.
- **Five device domains, each with its own state shape.** Lights (RGB + color-temperature + brightness + **dynamic effects**), media (receivers, speakers, TVs — power, volume, mute, source/streaming-service, transport, now-playing), power (strictly on/off switches, plugs, fans), virtual **remotes** (D-pad keys + app launch for TVs and streamers), and read-only **sensors** (motion, occupancy, contact, light level, temperature, humidity — presence sensors aggregate into per-room occupancy) — each modelled separately rather than squeezed into one generic shape. A TV or streamer's volume can be **bound to an AV receiver** so it controls the right box. Anything else Home Assistant exposes (climate, covers, locks, …) still surfaces as a generic **"Other devices"** control.
- **Scenes.** Save full-state snapshots — each light's color/temperature/**effect** plus every switch's on/off — and restore them in one tap, scoped to the whole home or a single room.
- **Floor planner.** Paint a rough 2D plan of your home (floor tiles + walls), drop devices roughly where they physically are, and bind painted regions to Rooms. The plan doubles as a live dashboard — devices glow with their real color/brightness and open the same controls used everywhere else.
- **Boards.** Compose your own dashboards from widgets — room cards, device tiles, groups, buttons, now-playing (with album art), scenes, sensors, weather, clocks — on a fixed-aspect grid that renders identically on any screen. A board can be seeded from a room, run full-screen as a kiosk, and a paired wall tablet can auto-launch one (see [Boards](boards.md)).
- **Voice control.** Speak commands in natural language; a deterministic grammar handles the common cases instantly, and anything it can't parse falls through to a local LLM that maps it to the same actions (see [Voice & assistants](#voice-assistants)).
- **Everything is exposed.** A key-authenticated public API and an embedded **Model Context Protocol** server let external apps and AI assistants drive the whole home.

## Providers

Devices are added through **providers** — Philips Hue (lights + sensors), Govee
and LIFX (lights), Onkyo / Integra and Sonos (audio), Smart TV (Sony Bravia —
media + remote), and Home Assistant (a high-class integration spanning lights,
media, power, remotes, **and** sensors). Each is added in the UI (Settings → Add
Provider) and discovered automatically; IP-addressable ones support a "Scan
network" auto-detect. See **[Providers](providers.md)** for the full list,
transports, and setup.

New to Bifrost? Start with the **[Setup guide](setup.md)** — install, first-run,
adding providers, voice, the wall tablet, and API keys, end to end.

## Voice & assistants

Bifrost is driven hands-free by **spoken voice** — a deterministic grammar
(offline, no model) that falls back to an optional LLM and then to Home Assistant
Assist, with spoken talk-back — and by **AI assistants** through the embedded
**MCP server** at `/mcp`. Everything is optional and degrades gracefully; the
models are pluggable and can run fully local. See **[Voice & assistants](voice.md)**
for the whole picture and **[MCP server](mcp.md)** for the assistant tools.

The **public API** (`/api/v1`, Bearer-key, mint keys in Settings → API keys) covers lights, rooms, scenes, media, power, remotes, and sensors — documented in **[Public API](api.md)**. Devices can be paired to headless clients by scanning a QR code (no key typing).

## Install

```sh
mkdir bifrost && cd bifrost
curl -fsSLO https://raw.githubusercontent.com/others-git/bifrost/main/docker-compose.yml
test -f .env || echo "BIFROST_SECRET=$(openssl rand -hex 32)" > .env
docker compose up -d
```

Then open `http://<host>:3000`. The **[Setup guide](setup.md)** covers this in
full — bare-binary builds, all configuration variables, first-run, adding each
provider, voice, the wall tablet, and API keys.

## Wall-tablet & companion repos

- **[bifrost-kiosk](https://github.com/others-git/bifrost-kiosk)** — a native Android app that turns a wall-mounted tablet into a hard-locked, always-on Bifrost fixture: a full-screen dashboard plus an offline wake-word voice satellite. Pair it by scanning a QR code. The hub manages it remotely — over-the-air updates, an auto-launched board, and display power saving (scheduled quiet hours + room-presence blanking).
- **[bifrost-skills](https://github.com/others-git/bifrost-skills)** — reusable hardware setup runbooks (starting with the tablet wall-kiosk).

## AI usage disclosure

Bifrost is built with heavy use of AI assistance — primarily [Claude Code](https://claude.com/claude-code). AI contributed to code, tests, and documentation throughout. Every change is gated by the same CI as any other (`cargo fmt`, `cargo clippy -D warnings`, the full test suite, and the frontend build), and AI-assisted commits carry a `Co-Authored-By: Claude` trailer.

## License

MIT
