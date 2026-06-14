# Home Assistant API — what Bifrost uses, and the entity-vs-device model

Notes from working against a live HA instance, to inform the `providers::ha`
adapter. See also `references/ha_rest_api.md`, `references/ha_websocket_api.md`,
`references/ha_light_entity.md`, `references/ha_media_player_entity.md`.

## REST (what we poll today)

- **`GET /api/states`** — every entity's `entity_id`, `state`, and `attributes`.
  This is our discovery + state source. Bearer-token auth.
- **`POST /api/services/{domain}/{service}`** — control (`light.turn_on`,
  `homeassistant.turn_on`/`turn_off`, `media_player.*`, `select_source`, …).
- **`POST /api/template`** — render Jinja; we use it for the **Area → entities**
  map (`areas()` / `area_name()` / `area_entities()`) since `/api/states` has no
  area data.

### Critical gap: `/api/states` has no registry metadata

`/api/states` does **not** expose `entity_category`, `device_id`, `disabled_by`,
`hidden_by`, or `platform`. Those live in the **entity registry**, which is
**WebSocket-only**. This is why REST alone can't tell a real device from a
device's config sub-control.

## The entity-vs-device problem

HA's model is **one device → many entities**. A device has a *primary* entity
(its main control) plus auxiliary **`config`** / **`diagnostic`** entities for
its settings/telemetry. Integrations expose lots of these as `switch.*`.

Observed on the dev instance — 14 `switch.*` entities, but only **3** are real
devices:

| entity_id | entity_category | platform | real device? |
|---|---|---|---|
| `switch.bedroom_shelf` | `None` | tplink | ✅ a smart plug |
| `switch.raven_lights` | `None` | tplink | ✅ |
| `switch.couch_string` | `None` | tplink | ✅ |
| `switch.bedroom_shelf_led` | `config` | tplink | ❌ the plug's LED-indicator toggle |
| `switch.office_sonos_..._crossfade` | `config` | sonos | ❌ a Sonos speaker setting |
| `switch.office_sonos_..._loudness` | `config` | sonos | ❌ |
| `switch.office_sonos_..._tv_autoplay` | `config` | sonos | ❌ |
| `switch.office_sonos_..._touch_controls` | `config` | sonos | ❌ |
| `switch.office_sonos_..._status_light` | `config` | sonos | ❌ |
| `switch.office_sonos_..._ungroup_on_autoplay` | `config` | sonos | ❌ |
| *(…and the `_2` duplicates of the Sonos set)* | `config` | sonos | ❌ |

Mapping **every** `switch.*` to a Bifrost device surfaced all 14 — conflating a
device's *state/settings* with the *device*. The Sonos config switches belong to
`media_player.office_sonos_office_sonos` (same `device_id`).

## The fix: filter to *primary* entities via the entity registry (WebSocket)

A primary, user-facing device control is one with **no `entity_category`**, and
not HA-disabled or hidden:

```
primary  ⇔  entity_category is null  AND  disabled_by is null  AND  hidden_by is null
```

Applying this to the dev instance: switches 14 → **3** (the real plugs); all 9
`media_player` entities (incl. `media_player.bedroom_tv`) stay primary — the TV
is not dropped.

### Getting the registry (one-shot WebSocket)

`config/entity_registry/list` over the WS API. Flow:

1. Connect `ws://<host>:8123/api/websocket` (`wss://` for TLS HA).
2. Server sends `{"type":"auth_required"}`.
3. Send `{"type":"auth","access_token":"<long-lived token>"}`.
4. Server sends `{"type":"auth_ok"}` (or `auth_invalid`).
5. Send `{"id":1,"type":"config/entity_registry/list"}`.
6. Server replies `{"id":1,"type":"result","success":true,"result":[ … ]}`.

Each result entry includes `entity_id`, `entity_category` (null | `config` |
`diagnostic`), `device_id`, `disabled_by`, `hidden_by`, `platform`, `name`,
`original_name`, `area_id`, … (the registry has *more* entries than `/api/states`
— it includes disabled/hidden entities).

`config/entity_registry/list_for_display` is a lighter variant with short keys
(`ai`=area_id, `di`=device_id, `en`=name, `ec`=entity-category index).

Bifrost does this as a **one-shot** fetch during discovery (cached briefly), then
filters discovery + area membership to primary entities. If the WS fetch fails it
**falls back to no filtering** (surfaces everything, the old behaviour) rather
than failing discovery.

## Beyond the filter (future)

- **`device_id`** lets us group a device's entities (e.g. the Sonos media_player
  + its config switches) — the basis for proper device-registry import.
- **`conversation.process`** (HA Assist) — the dev instance has a `conversation`
  entity; this is the path for natural-language media ("play X on the TV").
- The same WS connection upgrades to **`subscribe_events`** for real-time
  `state_changed` push (replacing the 30s REST poll) when we want it.

## Auth / security

Long-lived access token (HA → Profile → Security). Bearer for REST,
`access_token` in the WS `auth` message. Stored AES-256-GCM encrypted in
Bifrost like all provider credentials.
