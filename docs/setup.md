# Setup guide

This is the end-to-end path from nothing to a working Bifrost hub: install, first
run, adding devices, wiring up voice, pairing the wall tablet, and minting API
keys. For deeper reference see **[Providers](providers.md)**, the
**[Public API](api.md)**, and the **[MCP server](mcp.md)**.

## 1. Install

### Docker (recommended)

```sh
mkdir bifrost && cd bifrost
curl -fsSLO https://raw.githubusercontent.com/others-git/bifrost/main/docker-compose.yml
test -f .env || echo "BIFROST_SECRET=$(openssl rand -hex 32)" > .env
docker compose up -d
```

Then open `http://<host>:3000`.

The bundled compose file uses `network_mode: host` so device auto-detect can reach
the LAN — SSDP/eISCP broadcast and the subnet sweep don't cross a bridged
container's NAT. To run bridged, swap `network_mode: host` for a `ports:` mapping;
runtime control of already-added devices still works, but the **Scan network**
buttons won't find anything.

Upgrade with `docker compose pull && docker compose up -d`.

!!! tip "On Unraid?"
    Bifrost is in **[Community Applications](https://ca.unraid.net/apps/bifrost-0ehyi200nwfgp8)** —
    search **Bifrost** on the Apps tab and apply the template (set
    `BIFROST_SECRET`, keep host networking). See
    **[bifrost-unraid](https://github.com/others-git/bifrost-unraid)** for the
    template and step-by-step install.

!!! warning "Keep `BIFROST_SECRET` stable across upgrades"
    It encrypts your provider credentials at rest (AES-256-GCM). If it changes,
    the app still starts but logs `failed to decrypt credentials` and providers
    show disconnected — recovery is restoring the original secret or re-entering
    every provider's credentials.

### Bare binary

Requires Rust (stable) and Node 20+.

```sh
cd frontend && npm ci && npm run build && cd ..
BIFROST_SECRET=$(openssl rand -hex 32) cargo run --release
```

## 2. Configuration

Everything is configured via environment variables (a `.env` file works too):

| Variable | Default | Notes |
|---|---|---|
| `BIFROST_SECRET` | — (**required**) | Encrypts provider credentials at rest with AES-256-GCM. Use 32+ random chars (`openssl rand -hex 32`). Changing it orphans stored credentials. |
| `DATABASE_URL` | `sqlite://bifrost.db` | SQLite only. In Docker, point at a volume, e.g. `sqlite:///data/bifrost.db`. |
| `BIND_ADDR` | `0.0.0.0:3000` | Listen address. |
| `RUST_LOG` | — | Log filter, e.g. `bifrost=info` or `bifrost=debug`. |

Bifrost is designed for a trusted network. There is a single hub password (no
per-user accounts / RBAC) — put it behind **Tailscale**, a VPN, or a
reverse proxy with TLS for remote access.

## 3. First run

1. **Set the hub password.** On first load you choose one password that protects
   the whole hub. (Kiosks paired by QR can skip this login — see
   [the wall tablet](#6-pair-the-wall-tablet) below.)
2. **Add a provider** (next section) — discovery runs automatically and devices
   appear on the dashboard with live controls.
3. **Build Rooms.** On the **Rooms** page, combine synced provider groups with
   directly-assigned devices. A Room aggregates any mix of lights, audio, and
   power devices and becomes the high-level control surface (power, brightness,
   volume/mute fan-out, quick-control buttons).
4. **Scenes & floor plan.** Save **Scenes** (whole-home or room-scoped full-state
   snapshots — each light's color/temperature/effect plus every switch's on/off)
   and restore them in one tap. On the **Floor Plan** page, paint a rough 2D
   layout and place devices on it for an alternate live dashboard.

## 4. Add providers

Settings → **Add Provider**. Pick the provider type and fill in its credentials;
IP-addressable types offer a **Scan network** button that auto-fills the address.

| Provider | Category | You'll need |
|---|---|---|
| Philips Hue | Light | Bridge IP (auto-detectable) + press the bridge **link button** to mint an app key |
| Govee | Light | An API key (Govee Home app → Profile → About Us → Apply for API Key). Local LAN-preferred control is **on by default** — enable **“LAN Control”** per device in the Govee Home app |
| LIFX | Light | An access token ([cloud.lifx.com/settings](https://cloud.lifx.com/settings)). Local LAN-preferred control is **on by default** (LIFX LAN is on by default on the bulbs) |
| Onkyo / Integra | Audio | Receiver IP (auto-detectable) + optional port (eISCP, default 60128). Enable **Network Standby** for remote power-on |
| Sonos | Audio | Any one player's IP (auto-detectable) — the household is discovered from it |
| Home Assistant | Integration | Base URL (e.g. `http://homeassistant.local:8123`) + a **Long-Lived Access Token** (HA → Profile → Security → Long-Lived Access Tokens) |

After adding a provider, click **Sync** to mirror its native groupings (Hue
rooms/zones, LIFX groups, Sonos rooms, HA Areas) into Bifrost Rooms. A device
reachable both natively and through Home Assistant is automatically
**de-duplicated** — the native copy wins and the HA copy is hidden. See
**[Providers](providers.md)** for transports, live-update modes, and per-provider
capabilities.

Next, organise everything into rooms and tidy the inventory (glyphs, receiver
binding, TV remotes, merging duplicates) — see **[Rooms & devices](rooms-and-devices.md)**.

## 5. Voice & assistants

Bifrost can be driven by voice and by AI assistants, three ways:

- **Native voice pipeline** — a deterministic grammar parses spoken commands
  (power, brightness, color, color-temperature, volume, mute, transport, scenes,
  relative nudges) and resolves targets by room/device name. Fast and offline; it
  needs no configuration.
- **LLM fallback** — any phrasing the grammar can't parse is handed to a
  configurable **chat model** (any OpenAI-compatible endpoint — e.g. a local model
  via Ollama) that maps it to one action through the *same* dispatch path.
  Configure it under Settings (`PUT /api/ai-endpoints/chat`). With none
  configured, the native pipeline simply handles what it can; failing both, an
  unparsed clause can fall through to **Home Assistant Assist** if HA is added.
- **Embedded MCP server** — a first-class Model Context Protocol surface at `/mcp`
  exposes the home as assistant tools (lights, audio, power, rooms, scenes,
  remotes). Gated by the same API keys as the public API. See
  **[MCP server](mcp.md)**.

Configure the **chat** (NLU fallback) and **transcription** (speech-to-text)
models — plus an optional **tts** voice — under **Settings → AI endpoints**. Each
is one OpenAI-compatible server (base URL + model + optional key), so a local
Ollama / faster-whisper or any hosted API works; the **Test** button probes the
endpoint's `/models`. Everything degrades gracefully: with nothing configured the
deterministic grammar still runs over `POST /api/voice/command` (text), and
`POST /api/voice/listen` (audio) returns `503` only until a transcription model is
set — so text control never depends on STT being up. All voice endpoints are gated
by the same Bearer API keys as `/api/v1`.

## 6. Pair the wall tablet

The **[bifrost-kiosk](https://github.com/others-git/bifrost-kiosk)** Android app
turns a wall-mounted tablet into a hard-locked, always-on fixture (full-screen
dashboard + offline wake-word voice satellite). Pairing is by **QR code**, so no
key is ever typed on the tablet:

1. In an authenticated dashboard session, generate a pairing QR (it carries a
   short-lived, single-use enrollment token; `POST /api/enrollment`, TTL 5 min).
2. The tablet app scans it and redeems the token for its own `bfr_` API key
   (`POST /api/enrollment/redeem`) — which then shows up in **Settings → API
   keys** and is revocable like any other.
3. Manage paired kiosks from a phone/desktop session under **Clients** (sleep /
   wake / lock the screen, de-authorize, check the app version).

Authorized kiosks skip the password login and exchange their key for a session
automatically. See the enrollment and kiosk endpoint details in the
**[Public API](api.md)**.

## 7. Mint API keys

For your own scripts, automations, or assistant clients:

1. Settings → **API keys** → create a key. The full key (`bfr_` + 64 hex chars) is
   shown **once** at creation — copy it then; only a SHA-256 hash is stored, so it
   can't be recovered later. Revoking takes effect immediately.
2. Send it as a Bearer token on every request:

   ```
   Authorization: Bearer bfr_<your-key>
   ```

   ```sh
   curl -H "Authorization: Bearer $BIFROST_KEY" http://<host>:3000/api/v1/lights
   ```

The same key unlocks `/api/v1` (lights, rooms, scenes, audio, power, remotes), the
`/api/voice/*` endpoints, and the embedded MCP server at `/mcp`. Keys are managed
only with a browser session — a leaked key can't mint more keys. Full reference:
**[Public API](api.md)**.

## Companion repos

- **[bifrost-kiosk](https://github.com/others-git/bifrost-kiosk)** — the native
  Android wall-tablet app.
- **[bifrost-skills](https://github.com/others-git/bifrost-skills)** — reusable
  hardware setup runbooks (starting with the tablet wall-kiosk).
- **[bifrost-unraid](https://github.com/others-git/bifrost-unraid)** — the Unraid
  Community Applications Docker template.
