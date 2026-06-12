# Bifrost Public API (`/api/v1`)

A small REST API for third-party apps (Home Assistant, scripts, dashboards) to
read and control your lights and rooms. It mirrors what the Bifrost UI can do
for **lights** and **rooms** — including scenes — but not the floor plan or
provider configuration.

## Authentication

All `/api/v1` requests require a **client API key** sent as a Bearer token:

```
Authorization: Bearer bfr_xxxxxxxxxxxxxxxx…
```

There is no RBAC: any valid key has full access to lights and rooms.

### Managing keys

Keys are created and revoked from the Bifrost UI (**Settings → API keys**), or
via the session-authenticated management endpoints below. A key's full value is
shown **once**, at creation — only a SHA-256 hash is stored, so it cannot be
recovered later. Lost a key? Revoke it and mint a new one.

| Method | Path | Body | Notes |
|---|---|---|---|
| `GET` | `/api/api-keys` | — | List keys (id, name, prefix, timestamps). Never returns the key. |
| `POST` | `/api/api-keys` | `{"name": "Home Assistant"}` | Returns `{id, name, key, prefix}` — `key` is shown only here. |
| `DELETE` | `/api/api-keys/{id}` | — | Revoke a key immediately. |

These management endpoints use the UI session cookie, not a Bearer key.

A request with a missing or unknown key gets **401 Unauthorized**.

## Conventions

- Base URL: `http(s)://<host>/api/v1`
- Request and response bodies are JSON.
- A light/room **state** object:
  ```json
  {
    "on": true,
    "brightness": 80.0,          // 0–100, optional
    "color": { "x": 0.45, "y": 0.41, "brightness": 0.8 },  // CIE xy + 0–1 Y, optional
    "color_temp_mirek": 280       // 153–500, optional
  }
  ```
  Only `on` is required; omit fields you don't want to set.

## Lights

| Method | Path | Description |
|---|---|---|
| `GET` | `/lights` | List all lights with capabilities and last known state. |
| `GET` | `/lights/{id}` | One light. `404` if unknown. |
| `PUT` | `/lights/{id}/state` | Set a light's state. Body: a state object. |

`PUT /lights/{id}/state` returns `204 No Content` on success, `404` if the light
is unknown, `502` if the provider rejected the call.

```bash
curl -X PUT https://bifrost.local/api/v1/lights/$ID/state \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{"on": true, "brightness": 60}'
```

## Rooms

Rooms are the Bifrost-level grouping (not provider groups). Membership is the
union of linked provider-group lights and directly-assigned lights.

| Method | Path | Description |
|---|---|---|
| `GET` | `/rooms` | List rooms: `{id, name, light_ids}`. |
| `PUT` | `/rooms/{id}/state` | Drive every light in the room to one state. |

`PUT /rooms/{id}/state` returns `{"applied": N, "failed": M}`. It uses native
group control (e.g. one Hue `grouped_light` call) where possible and fans out
per-light otherwise. `404` if the room has no members.

```bash
curl -X PUT https://bifrost.local/api/v1/rooms/$ROOM/state \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{"on": false}'
```

## Scenes

Scenes are **global** palette presets — a name, an optional brightness, and a
color palette — not tied to any room. You define a scene once and apply it to
whichever room you like. A single-color (or brightness-only) scene drives the
whole room uniformly; a multi-color palette is distributed round-robin across
the room's lights in a stable order.

| Method | Path | Description |
|---|---|---|
| `GET` | `/scenes` | List all scenes. |
| `POST` | `/scenes` | Create a scene. |
| `POST` | `/scenes/from-room/{room_id}` | Save a room's current colors as a new scene. Body: `{"name": "…"}`. |
| `DELETE` | `/scenes/{id}` | Delete a scene. |
| `POST` | `/rooms/{room_id}/scenes/{scene_id}/apply` | Apply a scene to a room. Returns `{applied, failed}`. `404` if the scene or room is unknown. |

Create body:

```json
{
  "name": "Warm",
  "brightness": 40,                 // optional, 1–100
  "palette": ["#ff8800", "#ffd9a0"] // #rrggbb hex colors
}
```

Invalid palette colors or an out-of-range brightness return `422 Unprocessable
Entity`. `POST /scenes/from-room/{room_id}` returns `422` if no light in the room
is currently on, and `404` if the room is unknown.

```bash
# Define a scene, then apply it to a room.
SCENE=$(curl -s -X POST https://bifrost.local/api/v1/scenes \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{"name":"Sunset","brightness":75,"palette":["#ff7d33","#ff5e9c"]}' | jq -r .id)

curl -X POST https://bifrost.local/api/v1/rooms/$ROOM/scenes/$SCENE/apply \
  -H "Authorization: Bearer $KEY"
```

## Not exposed

The floor plan, provider credentials, and provider/group sync are intentionally
out of scope for the public API.

## Status codes

| Code | Meaning |
|---|---|
| `200` | OK (with body) |
| `204` | Success, no body (light state set) |
| `400`/`422` | Malformed or invalid request body |
| `401` | Missing or invalid API key |
| `404` | Unknown light, room, or scene |
| `502` | A provider rejected the upstream call |
