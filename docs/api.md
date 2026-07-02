# Bifrost Public API (`/api/v1`)

A key-authenticated REST API for external applications (automation scripts,
assistants, etc.). A valid key grants full access to every device domain —
lights, rooms (including scenes), media, power, remotes, and read-only sensors —
there is no RBAC. Session-only UI config (floor plan, boards, provider
management) is not exposed; use the web UI for those. The same key also unlocks
the embedded **MCP** server at [`/mcp`](#mcp-endpoint-mcp) for natural-language
control.

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
  "provider": "hue",             // hue | govee | lifx | wled | tasmota | shelly | ha
  "name": "Desk lamp",
  "state": { … LightState … },
  "capabilities": {
    "dimmable": true,
    "color_rgb": true,
    "color_temperature": true,
    "hue_gamut": "C",            // A | B | C | null
    "effects": ["no_effect", "candle", "fire"], // supported dynamic effects; omitted when none
    "segments": 15              // addressable colour segments (Govee strips); omitted when none
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
  "reachable": true,             // read-only; ignored on writes
  "effect": "candle"             // a name from capabilities.effects; "no_effect" clears. Omit to leave unchanged
}
```

### Scene (full-state snapshot)

```json
{
  "id": "f3c2…",
  "name": "Movie Night",
  "created_at": "2026-06-17T21:00:00Z",
  "lights": 3,                  // captured light entries
  "power": 1,                   // captured power-device entries
  "is_default": false,          // the single "Restore Home" preset (home scenes only)
  "room_id": "a1b2…",           // null = whole-home (Home Scene); set = Room Scene
  "room_name": "Living Room"    // null for a home scene
}
```

A scene is a snapshot of each captured light's **full** state (color **or**
temperature **or** effect) plus each power device's on/off, restored verbatim. A
**Home Scene** (`room_id: null`) captures the whole home; a **Room Scene**
captures one room's effective members. Activating a scene re-applies exactly what
was captured.

## Endpoints

### Lights

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/lights` | All lights with current state |
| `GET` | `/api/v1/lights/{id}` | One light (404 if unknown) |
| `PUT` | `/api/v1/lights/{id}/state` | Set state; body is a full `LightState` |
| `PUT` | `/api/v1/lights/{id}/segments` | Set per-segment colours (strips with `capabilities.segments`) |

`PUT …/state` responds `204 No Content` on success, `404` for an unknown light,
`502` if the provider could not be reached.

```bash
# Turn a light red at half brightness
curl -X PUT -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{"on":true,"brightness":50,"color":{"x":0.675,"y":0.322,"brightness":1.0}}' \
  http://bifrost.local:3000/api/v1/lights/$LIGHT_ID/state
```

`PUT …/segments` sets individual LED segments on an addressable strip (only lights
that advertise `capabilities.segments`; Govee today). Each entry carries a 0-indexed
`segment` plus an optional `rgb` (packed 24-bit `0xRRGGBB`) and/or optional
`brightness` (0–100) — set either or both. Same status codes as `…/state` (`502`
if the light has no segment support). Write-only — segment state isn't reported
back in `LightState`.

```bash
# Paint segments 0–2 red, 3–5 blue
curl -X PUT -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{"segments":[{"segment":0,"rgb":16711680},{"segment":1,"rgb":16711680},
                   {"segment":2,"rgb":16711680},{"segment":3,"rgb":255},
                   {"segment":4,"rgb":255},{"segment":5,"rgb":255}]}' \
  http://bifrost.local:3000/api/v1/lights/$LIGHT_ID/segments
```

### Rooms

A room is Bifrost's user-defined grouping (which may be linked to a provider's
native group). `light_ids` are the *effective* members: lights in the linked
provider group plus any directly assigned.

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/rooms` | All enabled rooms: `[{ id, name, light_ids, media_device_ids, power_device_ids }]` — `media_device_ids` / `power_device_ids` are the room's audio/power members; control each via the audio/power endpoints |
| `PUT` | `/api/v1/rooms/{id}/state` | Apply a `LightState` to every member |

Room writes respond `200` with `{ "applied": N, "failed": M }` — per-light
results, since a room can span providers and some members may be offline.
`404` if the room has no members.

### Scenes

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/scenes` | All scenes (home + room-scoped) |
| `POST` | `/api/v1/scenes` | Capture a snapshot: `{ name, room_id? }` → `201 { id, lights, power }`. Omit `room_id` for a whole-home scene; pass a room id to scope it to that room's members. `404` if the room is unknown |
| `POST` | `/api/v1/scenes/{id}/activate` | Re-apply the scene → `200 { applied, failed }` (`404` if unknown/empty) |
| `DELETE` | `/api/v1/scenes/{id}` | Delete → `204` |

`POST /scenes` snapshots **current live state** — it has no color/brightness
inputs of its own; the body is only `{ name, room_id? }`. A blank `name` →
`422` ("scene name is required"); an unknown `room_id` → `404`. `DELETE` is
idempotent (always `204`, even for an unknown id).

### Audio devices

Receivers and networked speakers (Onkyo via eISCP, Sonos via local UPnP).

```json
{
  "id": "c41a…",
  "provider_id": "9b2e…",
  "name": "Onkyo receiver (192.168.1.40)",
  "kind": "receiver",              // receiver | speaker | tv | zone
  "capabilities": { "sources": true, "transport": true, "now_playing": true },
  "state": {
    "power": true,
    "volume": 35,                  // 0–100
    "mute": false,
    "source": "net",               // current input/app
    "source_list": ["net","tv","Hulu"], // selectable inputs / TV apps (omitted if none); switch by sending one as `source`
    "now_playing": {               // when available
      "title": "Karma Police",
      "artist": "Radiohead",
      "album": "OK Computer",
      "play_state": "playing"      // playing | paused | stopped
    },
    "reachable": true
  }
}
```

A device's `capabilities` may also include `"favorites": true` (Sonos) — the
device exposes saved favorites you can start playing (see below) — and
`"grouping": true` (Sonos speakers) — the speaker can be joined into/out of a
provider-native synced playback group (see below).

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/media/devices` | All media devices (cached state) |
| `GET` | `/api/v1/media/devices/{id}` | One device — live read, refreshes the cache |
| `PUT` | `/api/v1/media/devices/{id}/state` | Send a command (body below) |
| `POST` | `/api/v1/media/devices/{id}/cast` | Cast media to a TV / media player (body below) |
| `POST` | `/api/v1/media/play-on` | Natural-language TV control by name — play a title / open an app (body below) |
| `GET` | `/api/v1/media/devices/{id}/favorites` | List saved favorites (live read) |
| `POST` | `/api/v1/media/devices/{id}/favorites/play` | Start a favorite (body below) |
| `POST` | `/api/v1/media/devices/{id}/group` | Join this speaker into a group (body below) |
| `POST` | `/api/v1/media/devices/{id}/ungroup` | Remove this speaker from its group |
| `PUT` | `/api/v1/media/devices/{id}/receiver` | Bind this source to a receiver (body below) |
| `PUT` | `/api/v1/media/devices/{id}/companion` | Merge this entity into a primary as its companion (body below) |

`PUT …/state` takes a **sparse command** — only the fields present are applied:

```json
{
  "power": true,
  "volume": 40,
  "mute": false,
  "source": "spotify",
  "transport": "play"    // play | pause | stop | next | previous | toggle
}
```

Source names: receiver inputs (`net`, `tv`, `bd`, `cbl`, `bluetooth`, …), raw
Onkyo SLI hex (`"2B"`), or a streaming service (`spotify`, `tunein`, `deezer`,
`tidal`, `airplay`, `internet-radio`) — service names switch the receiver to
NET and select the service in one call. On Sonos, `source` switches to a
physical input — `Line-In` (amps/ports/Fives) or `TV` (soundbars) — when the
player has one (advertised in `source_list`); `power` maps to play/pause.

Responses: `204` success, `404` unknown device, `422` invalid command (e.g.
unknown source — message in body), `502` device unreachable.

A bound source's read (`GET …/{id}`) reports the **receiver's** volume/mute,
and its `receiver_id` / `receiver_source` fields name the binding.

A media device is returned as an **effective device** — composite links are
resolved server-side. A companion's state and `capabilities` are merged into its
primary; a TV (`kind: "tv"`) whose `media_player` shares hardware with a paired,
enabled remote reports that remote's id as **`remote_id`** (else `null`), so a
client can render the unified TV control (keypad + app launch) straight from the
device without a separate remote lookup.

`state.reachable` / `state.power` are **composite-resolved**, not just the
`media_player`'s. The `media_player` is authoritative while reachable; when it is
unreachable (a standby TV reports `unavailable`), Bifrost falls back to the
device's members — the paired remote, companions — so the device reads
`reachable: true` / `power` from whichever member is up, instead of "offline."
With no member reachable it stays unreachable (a cold box) — a client can still
issue `{"power": true}`, which wakes it over Wake-on-LAN. (A future native
smart-TV provider joins this resolution as just another member.)

Power fans across the composite. A `{"power": true}` command drives the
`media_player` **and** wakes the paired remote (a Wake-on-LAN nudge + the
provider's `turn_on`) — the reliable way to bring a TV out of standby, where its
`media_player` often reports unavailable; the command succeeds if either path
works. A bound source's power-on also wakes its receiver and selects
`receiver_source`. Power-**off** is left to the `media_player` (the remote/box is
the same device, and a shared receiver may still serve others).

#### Cast media

Play media on a TV / media player. `content_id` + `content_type` are passed
through to the device's provider (Home Assistant maps them to
`media_player.play_media`; `content_type` is the provider-native kind — `url`,
`music`, `app`, `channel`, `playlist`, …). To **launch an app**, use the
remote's app-launch (`/remote/devices/{id}/command`); to **switch to a known
input/app**, set `source` via `PUT …/state`.

```json
// POST …/{id}/cast
{ "content_id": "https://example.com/stream.m3u8", "content_type": "url" }
```

Responses: `204` success, `404` unknown device, `422` the device doesn't support
casting, `502` device unreachable.

#### Play on a TV by name

Natural-language TV control: name a TV (or its remote) and a phrase, and the
shared content resolver does the right thing. `"open <app>"` launches that app
from the device's catalog; `"play <title>"` searches the TV's libraries and
plays the best match, then falls back to opening the TV's **last-used app** as
the best guess for where the title lives. `device` matches a remote or media
device by id or (case-insensitive, substring) name.

```json
// POST /api/v1/media/play-on
{ "device": "bedroom TV", "query": "play Bob's Burgers" }
```

Responses: `200 { "ok": true, "said": "Playing Bob's Burgers." }` when an action
was taken (`ok: false` with a reason when nothing matched), `404` when no TV or
remote matches the name, `502` when the device couldn't be reached.

#### Bind a source to a receiver

Real AV: a TV / streamer / console feeds audio *through* an AV receiver, which
owns the volume. Bind the source to its receiver and `PUT …/state` to the source
then routes `volume`/`mute` to the receiver, while `power`/`source`/`transport`
stay on the source. Powering the source **on** also wakes the receiver and (if
`receiver_source` is set) switches it to that input. Many sources may share one
receiver. Stored on the source.

```json
// PUT …/{id}/receiver
{
  "receiver_id": "<media device id>",  // null to unbind
  "receiver_source": "Game"             // optional: receiver input to select on power-on
}
```

Responses: `204` success, `404` unknown source device, `422` invalid binding
(self-binding, unknown receiver, or a receiver that is itself bound).

#### Merge a duplicate device (composite)

One physical device can surface as several entities with **complementary**
capabilities (e.g. a smart TV's two `media_player` views — one carries
now-playing, the other the apps). Merge the secondary into a **primary** as its
companion: the companion is hidden from control, but its state and controls are
**routed/overlaid** onto the primary — nothing is lost (unlike a hidden
duplicate). `volume`/`mute` route to whichever backing is receiver-bound,
`transport` to the one reporting playback, `source` to the one with inputs, and
`power` to the primary; the merged read fills now-playing / sources / the
receiver binding from the companion.

```json
// PUT …/{id}/companion
{
  "primary_id": "<media device id>"   // null to unmerge
}
```

Responses: `204` success, `404` unknown device, `422` invalid (self-merge, an
unknown/companion/shadowed primary, or a device that is itself a primary).

#### Favorites

Favorites are the presets the user already saved on the provider (e.g. Sonos
Favorites — playlists, stations). No accounts or search: you list them and
start one by reference.

```json
// GET …/favorites
[
  { "id": "FV:2/12", "title": "Jazz", "subtitle": "Spotify" },
  { "id": "FV:2/3", "title": "BBC Radio 6", "subtitle": "TuneIn" }
]
```

`POST …/favorites/play` takes the id in the body (provider ids contain slashes):

```json
{ "favorite_id": "FV:2/12" }
```

Responses: list → `200` with the array (empty for providers without favorites,
such as Onkyo); play → `204` success, `404` unknown device, `422` unknown
favorite, `502` device unreachable.

#### Grouping (provider-native)

Speakers with `"grouping": true` (Sonos) can be joined into a synced playback
group that plays in sync, controlled through a coordinator — the provider's own
grouping, **independent of Bifrost Rooms**. `POST …/{id}/group` joins the
speaker `{id}` into the group coordinated by another speaker:

```json
{ "coordinator_id": "<another media device id>" }
```

`POST …/{id}/ungroup` removes the speaker from any group (returns it to
standalone playback; idempotent). Both speakers must belong to the same
provider. After a change, the household topology shifts — re-run discovery
(`POST /api/providers/{id}/discover`) to surface the synced-group zone device.

Responses: `204` success, `404` unknown device, `422` invalid (different
providers, or grouping a speaker with itself), `502` device unreachable.

### Power devices

Strictly on/off endpoints — switches, smart plugs, fans, boolean helpers —
surfaced by integration providers (Home Assistant today). A power device has no
capability set; its whole state is `on`. `kind` is presentational (drives the UI
glyph) and is one of `switch | outlet | fan | toggle | generic`.

```json
{
  "id": "5d2f…",
  "device_id": "switch.porch",  // provider-native id (e.g. HA entity_id)
  "name": "Porch",
  "kind": "switch",
  "state": { "on": true, "reachable": true }
}
```

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/power/devices` | All power devices (cached state) |
| `GET` | `/api/v1/power/devices/{id}` | One device — live read, refreshes the cache |
| `PUT` | `/api/v1/power/devices/{id}/state` | Set power: body `{ "on": true|false }` |

`PUT …/state` responds `204` on success, `404` unknown device, `502` if the
device could not be reached.

### Remotes

A virtual smart-remote for a TV / streamer (Android TV Remote via Home Assistant
today). State is `on` plus the foreground app (`current_app`, a package id).

```json
{
  "id": "9a1b…",
  "device_id": "remote.bedroom_tv",  // provider-native id (e.g. HA entity_id)
  "name": "Bedroom TV",
  "state": { "on": true, "current_app": "com.netflix.ninja", "reachable": true }
}
```

`GET /remote/devices/{id}` returns just the live `state` object (the
`RemoteState`), not the full device record.

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/remote/devices` | All remotes (cached state) |
| `GET` | `/api/v1/remote/devices/{id}` | One remote — live read, refreshes the cache |
| `POST` | `/api/v1/remote/devices/{id}/command` | Send a command (see below) |

The command body is a tagged union — exactly one variant:

```json
{ "key": { "key": "select", "hold_secs": 0.0 } }   // canonical key press
{ "text": { "text": "hello" } }                      // type literal text
{ "launch_app": { "activity": "com.netflix.ninja" } } // package id OR deep-link URL
{ "power": { "on": true } }                          // power on/off
```

Canonical keys: `up`, `down`, `left`, `right`, `select`, `back`, `home`, `menu`,
`volume_up`, `volume_down`, `mute`, `play_pause`, `next`, `previous`, `power`.
`POST …/command` responds `204` on success, `404` unknown remote, `502` if it
could not be reached.

### Sensors (read-only)

Motion, occupancy, contact, illuminance, temperature, and humidity inputs,
surfaced by Philips Hue (motion accessories) and Home Assistant. Sensors are
**read-only** — there is no set-state route. `kind` is one of
`motion | occupancy | contact | illuminance | temperature | humidity | generic`;
`state.reading` is a self-describing tagged value — `{"bool": true}` for
motion/occupancy/contact, `{"number": 21.5}` for illuminance/temperature/humidity
— and `unit` (°C, lx, %) is present when the provider reports one. Presence kinds
(`motion`/`occupancy`) also feed each Room's occupancy aggregation server-side.

```json
{
  "id": "c4e8…",
  "device_id": "binary_sensor.hallway_motion",
  "name": "Hallway motion",
  "kind": "motion",
  "unit": null,
  "state": { "reading": { "bool": true }, "reachable": true }
}
```

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/sensors/devices` | All sensors (cached state) |
| `GET` | `/api/v1/sensors/devices/{id}` | One sensor — live read, refreshes the cache |

Live freshness normally arrives on the provider's push channel (Hue SSE / Home
Assistant WebSocket), so the cached list is current in practice; the single-device
read forces a poll for providers without push.

## Status codes

| Code | Meaning |
|---|---|
| `200` / `201` / `204` | Success (body / created / no body) |
| `401` | Missing or revoked API key |
| `404` | Unknown device, room, or scene |
| `422` | Validation failure (message in body) |
| `502` | Provider unreachable (device offline, bridge down) |

## Key management (UI/session only)

Keys are managed with a browser session, not with another key — a leaked key
cannot mint more keys: `GET/POST /api/api-keys`, `DELETE /api/api-keys/{id}`.

## Device enrollment (QR pairing)

Headless devices (the wall-tablet voice satellite) get a key without anyone
typing one. An authenticated dashboard session mints a short-lived, single-use
token; the device scans a QR carrying it and redeems it for a normal `bfr_` key
(which then shows up in **Settings → API keys** and is revocable like any other).

| Method | Path | Auth | Purpose |
|---|---|---|---|
| `POST` | `/api/enrollment` | session | Mint a pairing token → `{ token, expires_at, expires_in_secs }` (TTL 5 min) |
| `POST` | `/api/enrollment/redeem` | **token** (no key/session) | `{ token, device_name? }` → `201 { key, prefix, name }` |

The redeem route is intentionally unauthenticated — the device has no credential
yet — but is gated on a valid, unexpired, **unused** token, which only an authed
session can create. Redemption is atomic and single-use; a replayed token gets
`401`.

The dashboard renders the QR as JSON `{ "v": 1, "base_url": "<origin>", "token": "<token>" }`.
A client scans it, then `POST`s `{ token, device_name }` to
`base_url + /api/enrollment/redeem` and stores the returned `key` for all
subsequent `/api/v1` + `/api/voice` calls.

## Kiosk control (`/api/kiosks`)

Manage the wall-tablet companion apps. A kiosk is identified by the `bfr_` key it
was paired with (via enrollment); it **checks in** on a heartbeat and the server
hands back a queued command. Management endpoints are **session-only** (driven
from a phone/desktop), so they aren't reachable with a kiosk key — and the
companion app sets a `BifrostKiosk/<ver>` User-Agent suffix so the frontend hides
this view on the kiosk itself.

| Method | Path | Auth | Purpose |
|---|---|---|---|
| `POST` | `/api/kiosks/checkin` | **kiosk key** | Heartbeat: `{ name?, app_version?, screen_on?, battery/power telemetry… }` → `{ command, room, default_board_id }` (queued command, consumed; `room` is the assigned Room's name — the app's voice context) |
| `GET` | `/api/kiosks/self` | **kiosk cookie** | The kiosk's own record (`{ id, name, default_board_id }`) — how a kiosk-served Boards page resolves its auto-launch board |
| `GET` | `/api/kiosks/stream` | **kiosk key** | Live SSE command channel — controller commands are pushed here instantly; the queued copy is the offline fallback |
| `GET` | `/api/kiosks` | session | List kiosks: check-in status, assignments, schedule/presence config, battery telemetry |
| `POST` | `/api/kiosks/{id}/command` | session | Queue `{ command }` — one of `sleep`, `wake`, `lock`, `update` |
| `PUT` | `/api/kiosks/{id}/room` | session | Assign the kiosk to a Room (`{ room_id }`, null clears) — its voice context and presence source |
| `PUT` | `/api/kiosks/{id}/board` | session | Set the board to auto-launch full-screen (`{ board_id }`, null clears) |
| `PUT` | `/api/kiosks/{id}/schedule` | session | Scheduled quiet hours: `{ enabled, sleep_at, wake_at }` (server-local `"HH:MM"`; both required and distinct when enabled) |
| `PUT` | `/api/kiosks/{id}/presence` | session | Presence-driven blanking: `{ enabled, timeout_secs? }` (no-motion grace, clamped 30–3600 s) |
| `POST` | `/api/kiosks/{id}/deauth` | session | Revoke the kiosk's key (it must re-enroll) |
| `DELETE` | `/api/kiosks/{id}` | session | Forget a kiosk record |

The OTA update relay lives under `/api/kiosks/update` (session triggers/inspects
the hub-side APK cache; key-auth `manifest`/`apk` routes feed the kiosk over the
LAN).

**Command semantics** (the app performs these on push or check-in):
- `sleep` / `wake` — turn the display off / on.
- `lock` — force sign-out of the Bifrost WebView session (re-enter password).
- `update` — pull the hub-cached APK and self-install.
- **de-auth** is *not* a queued command — it revokes the key immediately, so the
  app's next call gets `401` and it re-enrolls via a fresh QR scan.

**Display power saving** is server-driven: a background scheduler issues the same
`sleep`/`wake` commands from each kiosk's quiet-hours schedule and, by day, from
its assigned Room's occupancy (presence sensors) — a manual wake holds until the
next boundary. Configure both per kiosk in **Settings → Clients**.

**Companion-app contract:** check in every ~30–60s with the paired key; act on a
returned `command` (and clear nothing — the server consumes it); treat a `401`
from any authed call as "de-authed" → drop to QR enrollment. Set the WebView
User-Agent to include `BifrostKiosk/<version>`.

## Voice control (`/api/voice`)

Natural-language command endpoints, gated by the **same Bearer API keys** as
`/api/v1` (a browser session also works, for the web conversation modal). This is
the contract the headless wall-tablet **voice satellite** uses — it has no login
cookie, so it sends a minted `bfr_` key like any other public-API client.

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/api/voice/command` | Run a **text** command (fallback chain below) |
| `POST` | `/api/voice/listen` | Upload **audio**; server transcribes (configured STT) then runs it |
| `POST` | `/api/voice/speak` | Synthesize **spoken audio** for `text` via the configured TTS model; returns the audio bytes |
| `GET` | `/api/voice/vocabulary` | `{ words: [...] }` — the command-grammar keywords plus every enabled room/device/scene name (tokenized). A device with on-device STT (the wall tablet) biases its recognizer to this list so in-domain words aren't misheard. |

A clause is resolved **native-first**: the deterministic grammar parses what it can;
a clause it can't parse falls to the configured **`chat` LLM** (OpenAI-compatible
tool-calling — it maps the phrasing to the same internal command and dispatches via
the shared service layer); failing that, to **Home Assistant Assist**. Each fallback
is optional — with neither configured, an unparsed clause returns "I didn't
understand". Configure the `chat` model under `PUT /api/ai-endpoints/chat`.

`/api/voice/command` — JSON in, JSON out:

```bash
curl -X POST -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{"text":"bifrost, turn off the office", "context":{"room":"office"}}' \
  http://bifrost.local:3000/api/voice/command
```

```jsonc
// request
{ "text": "...", "context": { "room": "<id-or-name>" } }  // context optional; room disambiguates bare references
// response
{ "ok": true, "said": "Turned off the office.", "clauses": [ { "heard": "...", "ok": true, "said": "..." } ] }
```

`/api/voice/listen` — `multipart/form-data` with an audio `file` field (and an
optional `room` text field). Returns the same shape plus the recognized
`transcript`. Returns `503` when no transcription model is configured (so text
control over `/command` never depends on STT being up):

```bash
curl -X POST -H "Authorization: Bearer $KEY" \
  -F file=@utterance.wav -F room=office \
  http://bifrost.local:3000/api/voice/listen
```

`/api/voice/speak` — JSON in (`{ "text": "...", "voice": "alloy", "format": "mp3" }`;
`voice`/`format` optional), audio bytes out (the `Content-Type` reflects the
synthesized format). Spoken talk-back for a reply — pass it the `said` line from a
command. Returns `503` when no TTS model is configured:

```bash
curl -X POST -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{"text":"Turned off the office."}' \
  http://bifrost.local:3000/api/voice/speak --output reply.mp3
```

## MCP endpoint (`/mcp`)

Bifrost embeds a [Model Context Protocol](https://modelcontextprotocol.io)
server so an AI assistant can control the home in natural language. It is served
at **`POST /mcp`** as a **Streamable HTTP** endpoint (stateless, JSON responses)
and gated by the **same Bearer API keys** as `/api/v1`:

```
Authorization: Bearer bfr_<your-key>
Content-Type: application/json
Accept: application/json, text/event-stream
```

A missing or invalid key returns `401` before any MCP processing. The MCP tools
call the same shared service layer as the routes above, so behaviour can't drift
from the REST surface. Tools resolve lights, rooms, scenes, and media devices by
**id or case-insensitive name/substring**. The tool catalogue and mapping live
in [MCP server](mcp.md). stdio-only clients can bridge to this endpoint with the
standard `mcp-remote` shim — there is no separate stdio server.

## Versioning

The `/api/v1` surface is additive-stable: fields may be added to responses, but
existing fields and routes will not change meaning within v1. Breaking changes
get a new prefix.
