<p align="center">
  <img src="frontend/public/favicon.svg" width="96" height="96" alt="Bifrost" />
</p>

<h1 align="center">Bifrost</h1>

<p align="center">
  A self-hosted smart-home hub — control your devices from one web UI, a REST API, and voice.
</p>

<p align="center">
  <a href="https://github.com/others-git/bifrost/releases"><img src="https://img.shields.io/github/v/release/others-git/bifrost?sort=semver&color=6f42c1" alt="Latest release" /></a>
  <a href="https://github.com/others-git/bifrost/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/others-git/bifrost/ci.yml?branch=main&label=CI" alt="CI" /></a>
  <a href="https://github.com/others-git/bifrost/blob/main/LICENSE"><img src="https://img.shields.io/github/license/others-git/bifrost" alt="License" /></a>
  <a href="https://others-git.github.io/bifrost/"><img src="https://img.shields.io/badge/docs-online-blue" alt="Docs" /></a>
  <a href="https://github.com/others-git/bifrost/pkgs/container/bifrost"><img src="https://img.shields.io/badge/ghcr.io-bifrost-2496ed?logo=docker&logoColor=white" alt="Container image" /></a>
  <a href="https://ca.unraid.net/apps/bifrost-0ehyi200nwfgp8"><img src="https://img.shields.io/badge/Unraid-Community%20Apps-e8512f" alt="Unraid Community Apps" /></a>
</p>

---

Bifrost unifies your smart-home devices behind a single, fast, self-hosted control
surface — a web dashboard, a REST API, an embedded assistant (MCP) server, and
natural-language voice. It's built around **reliability**: every device connection
is owned by a state machine with reconnection, backoff, and live push to the
browser, and a read that can't reach a device falls back to cached state rather
than failing.

## Documentation

📖 **Full docs: <https://others-git.github.io/bifrost/>**

- [Overview](docs/index.md) — what Bifrost is and how it's built
- [Setup guide](docs/setup.md) — install, configuration, first-run, voice, the wall tablet
- [Providers](docs/providers.md) — Hue, Govee, LIFX, Onkyo/Integra, Sonos, Home Assistant
- [Public API](docs/api.md) — the Bearer-key `/api/v1` REST surface
- [MCP server](docs/mcp.md) — the embedded assistant tool catalogue

## Quick start (Docker)

```sh
mkdir bifrost && cd bifrost
curl -fsSLO https://raw.githubusercontent.com/others-git/bifrost/main/docker-compose.yml
test -f .env || echo "BIFROST_SECRET=$(openssl rand -hex 32)" > .env
docker compose up -d
```

Then open `http://<host>:3000`. **Keep `BIFROST_SECRET` identical across upgrades** —
it encrypts your provider credentials at rest. See the
[install guide](docs/index.md#install) for bare-binary builds and configuration.

On **Unraid**? Bifrost is in [Community Applications](https://ca.unraid.net/apps/bifrost-0ehyi200nwfgp8) —
search **Bifrost** on the Apps tab and apply the template.

## Companion repos

- **[bifrost-kiosk](https://github.com/others-git/bifrost-kiosk)** — a native Android app that turns a wall-mounted tablet into a hard-locked, always-on Bifrost fixture: a full-screen dashboard plus an offline wake-word voice satellite.
- **[bifrost-skills](https://github.com/others-git/bifrost-skills)** — reusable hardware setup runbooks (starting with the tablet wall-kiosk).
- **[bifrost-unraid](https://github.com/others-git/bifrost-unraid)** — the [Unraid Community Applications](https://ca.unraid.net/apps/bifrost-0ehyi200nwfgp8) Docker template.

## AI usage disclosure

Bifrost is built with heavy use of AI assistance — primarily [Claude Code](https://claude.com/claude-code). AI contributed to code, tests, and documentation throughout. Every change is gated by the same CI as any other (`cargo fmt`, `cargo clippy -D warnings`, the full test suite, and the frontend build), and AI-assisted commits carry a `Co-Authored-By: Claude` trailer.

## License

MIT
