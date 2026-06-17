# Providers

Devices are added through **providers**. Each is added in the UI (Settings → Add
Provider) and discovered automatically.

## Lights

| Provider | Transport | Live updates | Setup |
|---|---|---|---|
| Philips Hue | LAN (CLIP v2) | SSE push | Bridge IP + link-button pairing in the UI |
| Govee | Cloud API | Polling | API key from the Govee Home app |
| LIFX | Cloud API | Polling | Account token from the LIFX app; LIFX groups import as Rooms |

## Audio

| Provider | Transport | Live updates | Setup |
|---|---|---|---|
| Onkyo / Integra | LAN (eISCP) | Push (persistent socket) | Receiver IP; enable Network Standby for remote power-on |

Receivers expose power, volume, mute, input/streaming-service selection, playback
transport, and now-playing metadata; a second output (e.g. zone 2) appears as its
own device. A **source** device (TV, streamer, console) can be **bound to a
receiver** that owns its volume, so "turn the TV up" routes to the right box.

## Integrations

| Provider | Surfaces | Live updates | Setup |
|---|---|---|---|
| Home Assistant | Lights, audio (media players), and power (switches/plugs/fans) from a single connection | WebSocket push | Base URL + a long-lived access token; HA Areas import as Rooms |

Home Assistant is a **high-class** provider: one adapter surfaces *any* HA
integration as Bifrost devices across all three domains, and a physical device
reachable both natively (Hue/Govee/…) and via HA is automatically de-duplicated
so the native copy wins.

## Discovery & setup

Every IP-addressable provider supports **auto-detect** — a "Scan network for
devices" button finds devices on the LAN and fills in the address. Cloud
providers take an account token instead.

!!! note "Adding a provider is mechanical"
    Implement the provider trait, register one factory line, write tests — the
    shared discovery engine and connection managers do the rest.
