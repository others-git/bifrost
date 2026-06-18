# Providers

Devices are added through **providers** (Settings → Add Provider). Each provider
discovers its devices automatically. IP-addressable providers offer a **Scan
network** button that finds devices on the LAN and fills in the address; cloud
providers take an account token instead. A single provider can serve more than
one device domain — Home Assistant alone surfaces lights, audio, power, **and**
remotes.

## At a glance

| Provider | Category | Devices | Transport | Live updates | Credentials |
|---|---|---|---|---|---|
| Philips Hue | Light | Lights | LAN (CLIP v2) | SSE push | Bridge IP + link-button app key |
| Govee | Light | Lights | Cloud API | Poll (≈2 min) | API key |
| LIFX | Light | Lights | Cloud API | Poll (≈2 min) | Account token |
| Onkyo / Integra | Audio | Receivers + zones | LAN (eISCP) | Push | Receiver IP |
| Sonos | Audio | Speakers | LAN (UPnP) | Push (events + poll) | Any player's IP |
| Home Assistant | Integration | Lights · audio · power · remotes | REST + WebSocket | WebSocket push | Base URL + long-lived token |

---

## Philips Hue

- **Category** Light · **Transport** LAN, CLIP v2 over HTTPS to the bridge · **Live** Server-Sent Events push (changes appear instantly).
- **Setup** Enter the bridge IP — or use **Scan network** (Bifrost finds the bridge via SSDP) — then press the bridge's physical **link button** when prompted to mint an application key.
- **Capabilities** RGB color, color temperature, brightness, and **dynamic effects** (the bridge's CLIP v2 effects — candle, fire, sparkle, prism, …). Hue **rooms and zones** import as Bifrost Rooms, driven with native one-call group control.

Hue's ~10 req/s rate limit is handled with a per-bridge write pacer, so room-wide
fan-outs don't drop commands.

## Govee (Cloud)

- **Category** Light · **Transport** Govee Cloud API · **Live** polling.
- **Setup** API key — Govee Home app → Profile → About Us → Apply for API Key.
- **Capabilities** RGB color, color temperature, brightness, and **effects** = the device's **dynamic light scenes** — the built-in catalogue *plus* your own DIY scenes (often 100+ on a strip; the effects picker has search + categories for this). No native rooms.

## LIFX (Cloud)

- **Category** Light · **Transport** LIFX Cloud API (Bearer token) · **Live** polling.
- **Setup** Personal access token from [cloud.lifx.com/settings](https://cloud.lifx.com/settings).
- **Capabilities** RGB color, color temperature, brightness, and **firmware effects** — `off`/`breathe`/`pulse` on every color bulb, `move` on multizone strips (Z/Beam), and `morph`/`flame` on matrix bulbs (Tile/Candle). **LIFX groups import as Bifrost Rooms** with one-call group control.

## Onkyo / Integra

- **Category** Audio · **Transport** LAN eISCP, one persistent socket per receiver · **Live** push — the receiver echoes every change on the open connection.
- **Setup** Receiver IP — or **Scan network** (UDP discovery). **Enable Network Standby** on the receiver so Bifrost can power it on remotely.
- **Capabilities** power, volume, mute, input/streaming-service selection (including NET services like Spotify / TIDAL / TuneIn), playback transport, and now-playing. A second output (**zone 2**) appears as its own device.

Onkyo receivers are the target for **receiver binding** — a TV or streamer can route
its volume here, so "turn the TV up" controls the right box.

## Sonos

- **Category** Audio · **Transport** LAN UPnP · **Live** push (UPnP event subscriptions with a heartbeat-poll baseline).
- **Setup** Any one player's IP — or **Scan network**. The rest of the household is discovered from it.
- **Capabilities** power, volume, mute, transport, now-playing, **Favorites** (saved stations / playlists), and live **sync grouping** — grouped players collapse into one control. Each player imports as its own Room. Choose what to play from a Sonos app, then drive it from Bifrost's transport.

!!! note "Newer integration"
    Sonos control, favorites, and grouping are implemented and addable; it's
    less battle-tested than the other providers, so report anything off.

## Home Assistant

The **high-class integration**: one connection surfaces *any* Home Assistant
integration as Bifrost devices across **four domains from a single provider** —
lights, audio (media players: TVs and speakers), power (switches, plugs, fans,
helpers), and **remotes** (Android TV / streamers). HA **Areas import as Bifrost
Rooms**.

- **Category** Integration · **Transport** HA REST + a persistent WebSocket · **Live** WebSocket push — every domain stays live on one connection.
- **Setup** HA base URL (e.g. `http://homeassistant.local:8123`) + a **Long-Lived Access Token** (HA → Profile → Security → Long-Lived Access Tokens → Create Token).
- **Capabilities** lights pass through color / temperature / brightness and the entity's effect list; media players expose power / volume / mute / source / transport / now-playing and join-unjoin grouping; switches, plugs, and fans are on/off; remotes send keys, text, and app launches. Named-content requests ("play Bob's Burgers on the bedroom TV") fall back to HA Assist.

### De-duplication

A physical device reachable **both** natively (Hue / Govee / Sonos / Onkyo) **and**
through Home Assistant would otherwise import twice. Bifrost matches the two by
hardware MAC and **hides the HA copy under the native one** — *native always
wins* — so you control each device through its richest provider. (When HA exposes
a capability the native provider lacks, that capability gets built natively rather
than deferring to the hidden HA copy.)

---

## Adding & extending

- Every IP-addressable provider supports **Scan network** auto-detect; cloud providers take an account token.
- Adding a *new* provider type is intentionally mechanical — implement the provider trait, register one factory line, and write tests — so the supported set grows over time.
