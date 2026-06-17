<p align="center">
  <img src="frontend/public/favicon.svg" width="96" height="96" alt="Bifrost" />
</p>

<h1 align="center">Bifrost</h1>

<p align="center">
  A self-hosted smart-home hub — lights, audio, and power behind one fast UI, a clean API, and voice control. One binary, one SQLite file, one Docker image.
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

- [Overview, install & configuration](docs/index.md)
- [Providers](docs/providers.md) — Hue, Govee, LIFX, Onkyo/Integra, Home Assistant
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

## Companion repos

- **[bifrost-kiosk](https://github.com/others-git/bifrost-kiosk)** — a native Android app that turns a wall-mounted tablet into a hard-locked, always-on Bifrost fixture: a full-screen dashboard plus an offline wake-word voice satellite.
- **[bifrost-skills](https://github.com/others-git/bifrost-skills)** — reusable hardware setup runbooks (starting with the tablet wall-kiosk).

## AI usage disclosure

Bifrost is built with heavy use of AI assistance — primarily [Claude Code](https://claude.com/claude-code). AI contributed to code, tests, and documentation throughout. Every change is gated by the same CI as any other (`cargo fmt`, `cargo clippy -D warnings`, the full test suite, and the frontend build), and AI-assisted commits carry a `Co-Authored-By: Claude` trailer.

## License

MIT
