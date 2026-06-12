# Bifrost Public API (`/api/v1`)

A key-authenticated REST API for external applications (automation scripts, the
`bifrost-mcp` companion, etc.). A valid key grants full access to lights, rooms,
and scenes — there is no RBAC. The floor plan and provider management are not
exposed; use the web UI for those.

## Authentication

Create a key in **Settings → API keys**. The full key (`bfr_` + 64 hex chars) is
shown exactly once at creation — only a SHA-256 hash is stored, so it cannot be
recovered later. Revoking a key takes effect immediately.

Send the key as a Bearer token on every request:

```
Authorization: Bearer bfr_<your-key>
```

Missing or unknown keys get `401 Unauthorized` on every endpoint.

```bash
curl -H "Authorization: Bearer $BIFROST_KEY" http://bifrost.local:3000/api/v1/lights
```

## Data shapes

### Light

```json
{
  "id": "8b7f…",                 // Bifrost UUID — use this in all /lights/{id} calls
  "provider_id": "ab12…",        // provider-native identifier (informational)
  "provider": "hue",             // hue | govee | wled | tasmota | shelly | lifx | govee-lan
  "name": "Desk lamp",
  "state": { … LightState … },
  "capabilities": {
    "dimmable": true,
    "color_rgb": true,
    "color_temperature": true,
    "hue_gamut": "C"             // A | B | C | null
  },
  "last_seen": "2026-06-11T12:00:00Z"
}
```

### LightState

Sent in full on writes (it is a complete state, not a patch):

```json
{
  "on": true,
  "brightness": 80.0,            // 0–100, null for non-dimmable
  "color": {                     // CIE xyY; null to leave color alone
    "x": 0.4573,
    "y": 0.41,
    "brightness": 1.0            // linear Y, 0.0–1.0
  },
  "color_temp_mirek": null,      // 153–500 (≈6500K–2000K); alternative to color
  "reachable": true              // read-only; ignored on writes
}
```

### Scene (global palette preset)

```json
{
  "id": "f3c2…",
  "name": "Sunset",
  "brightness": 75.0,            // 0–100 or null (colors only)
  "palette": ["#ff7d33", "#ff5e9c", "#ffb04d"]  // 0–6 hex colors
}
```

A scene is room-independent. Applying one with a single color washes the whole
room; multiple colors are distributed round-robin across the room's lights; an
empty palette changes brightness only.

## Endpoints

### Lights

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/lights` | All lights with current state |
| `GET` | `/api/v1/lights/{id}` | One light (404 if unknown) |
| `PUT` | `/api/v1/lights/{id}/state` | Set state; body is a full `LightState` |

`PUT …/state` responds `204 No Content` on success, `404` for an unknown light,
`502` if the provider could not be reached.

```bash
# Turn a light red at half brightness
curl -X PUT -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{"on":true,"brightness":50,"color":{"x":0.675,"y":0.322,"brightness":1.0}}' \
  http://bifrost.local:3000/api/v1/lights/$LIGHT_ID/state
```

### Rooms

A room is Bifrost's user-defined grouping (which may be linked to a provider's
native group). `light_ids` are the *effective* members: lights in the linked
provider group plus any directly assigned.

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/rooms` | All rooms: `[{ id, name, light_ids }]` |
| `PUT` | `/api/v1/rooms/{id}/state` | Apply a `LightState` to every member |
| `POST` | `/api/v1/rooms/{id}/scenes/{scene_id}/apply` | Apply a scene to the room |

Room writes respond `200` with `{ "applied": N, "failed": M }` — per-light
results, since a room can span providers and some members may be offline.
`404` if the room has no members or the scene is unknown.

### Scenes

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/scenes` | All scenes |
| `POST` | `/api/v1/scenes` | Create: `{ name, brightness?, palette? }` → `201 { id }` |
| `POST` | `/api/v1/scenes/from-room/{room_id}` | Capture the room's current lit colors as a new scene: `{ name }` → `201 { id }` |
| `DELETE` | `/api/v1/scenes/{id}` | Delete → `204` |

`POST /scenes` validates: name required, brightness 1–100 if present, palette
entries must be `#rrggbb`, max 6 colors → `422` with a message on violation.
`from-room` returns `422` if nothing in the room is currently lit.

## Status codes

| Code | Meaning |
|---|---|
| `200` / `201` / `204` | Success (body / created / no body) |
| `401` | Missing or revoked API key |
| `404` | Unknown light, room, or scene |
| `422` | Validation failure (message in body) |
| `502` | Provider unreachable (device offline, bridge down) |

## Key management (UI/session only)

Keys are managed with a browser session, not with another key — a leaked key
cannot mint more keys: `GET/POST /api/api-keys`, `DELETE /api/api-keys/{id}`.

## Versioning

The `/api/v1` surface is additive-stable: fields may be added to responses, but
existing fields and routes will not change meaning within v1. Breaking changes
get a new prefix.
