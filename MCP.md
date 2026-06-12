# Bifrost MCP — goals, tools & targets

The **companion MCP server (`bifrost-mcp`)** wraps Bifrost's public `/api/v1`
surface as [Model Context Protocol](https://modelcontextprotocol.io) tools, so
an AI assistant can control the whole home in natural language. It is a
**separate repository** (local at `/mnt/d/REPOS/bifrost-mcp`, TypeScript stdio
server; no GitHub remote yet) and is **not** modified from this repo.

This file is the **source of truth for what that server should expose** — kept
here, alongside the API it wraps, so tool targets can't drift from the
endpoints. When you add or change a `/api/v1` route, update the mapping below
and mark any new tool as a target; implement it in `bifrost-mcp` when that
project is next touched.

Ultimate goal: AI-driven whole-home control. Target client: **Whisperr** (the
voice/LLM pipeline). Design for natural-language ergonomics — resolve rooms,
scenes, and favorites by **name**, not just id.

## Conventions

- **Auth:** Bifrost API key (`bfr_…`, minted in Settings) in the server's
  `BIFROST_API_KEY` env var, sent as `Authorization: Bearer`. The key itself
  never touches Bifrost's DB — only its SHA-256 hash is stored.
- **Transport:** stdio MCP (Claude Desktop, `claude --mcp`, Whisperr, …).
- **Name resolution:** tools accept an id **or** a case-insensitive name/
  substring; on no match, the error lists the valid options.
- **Sparse writes:** mirror the API — only the fields the user named are sent.

## Current tools — shipped (`bifrost-mcp` v0.1.0)

| Tool | Maps to | Notes |
|---|---|---|
| `get_home_state` | `GET /rooms` + `/lights` + `/scenes` + `/audio/devices` | One-call context snapshot |
| `list_lights` | `GET /lights` | |
| `set_light` | `PUT /lights/{id}/state` | hex → CIE xy with Bifrost's matrix |
| `set_room` | `PUT /rooms/{id}/state` | room by id **or name** |
| `apply_scene` | `POST /rooms/{id}/scenes/{scene_id}/apply` | scene by id **or name** |
| `apply_scene_all` | fan-out `apply_scene` over all rooms | whole-home look |
| `save_scene_from_room` | `POST /scenes/from-room/{room_id}` | |
| `set_audio` | `PUT /audio/devices/{id}/state` | power/volume/mute/source/transport |
| `get_audio_state` | `GET /audio/devices/{id}` | live read incl. now-playing |

## Target tools — not yet built

### Audio favorites (endpoints exist — see [API.md](API.md))

| Tool | Maps to | Behaviour |
|---|---|---|
| `list_audio_favorites` | `GET /audio/devices/{id}/favorites` | List a device's saved favorites (Sonos Favorites; empty for Onkyo). |
| `play_audio_favorite` | `GET …/favorites` then `POST …/favorites/play` | Resolve `favorite` (id, exact name, or substring) against the list, then play `{ "favorite_id": <id> }`. Headline use case: *"play my jazz favorite in the office."* |

Reference shape for `play_audio_favorite` (matches the reverted draft, kept here
so it can be rebuilt verbatim in `bifrost-mcp`):

```ts
// inputSchema: { device_id: string, favorite: string }
const favs = await api("GET", `/audio/devices/${device_id}/favorites`);
const q = favorite.trim().toLowerCase();
const match =
  favs.find((f) => f.id === favorite) ??
  favs.find((f) => f.title.toLowerCase() === q) ??
  favs.find((f) => f.title.toLowerCase().includes(q));
// no match → error listing available titles
await api("POST", `/audio/devices/${device_id}/favorites/play`, { favorite_id: match.id });
```

### Other candidates (open)

| Tool | Maps to | When |
|---|---|---|
| `list_audio_devices` | `GET /audio/devices` | If a standalone audio list is wanted beyond `get_home_state`. |
| Tier-2 music search/play | a future music-service API | After PLAN.md Milestone 12 Tier 2 (Spotify OAuth + Connect) lands. |

## Maintenance checklist

When `/api/v1` changes:

1. Update the **mapping tables** above (current vs target).
2. New capability an assistant should reach? Add a **target tool** row with its
   endpoint and resolution behaviour.
3. Cross-link from [PLAN.md](PLAN.md) if it's part of a milestone.
4. Implement in `bifrost-mcp` (separate repo) and move the row from *target* to
   *current*, noting the `bifrost-mcp` version.
