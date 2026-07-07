# Providers

Devices are added through **providers** (Settings → Add Provider). Each provider
discovers its devices automatically. IP-addressable providers offer a **Scan
network** button that finds devices on the LAN and fills in the address; cloud
providers take an account token instead. A single provider can serve more than
one device domain — Home Assistant alone surfaces lights, media, power, remotes,
**and** sensors.

The table below is the **full set of supported devices** and how each connects.

## At a glance

| Provider | Category | Devices | Transport | Live updates | Credentials |
|---|---|---|---|---|---|
| Philips Hue | Light | Lights + motion accessories (motion/light/temperature sensors) | LAN (CLIP v2) | SSE push | Bridge IP + link-button app key |
| Govee | Light | Lights | Cloud API + LAN (UDP) | Poll (≈2 min) | API key and/or LAN interface |
| LIFX | Light | Lights | Cloud API + LAN (UDP) | Poll (≈2 min) | Account token and/or LAN interface |
| Onkyo / Integra | Media | Receivers + zones | LAN (eISCP) | Push | Receiver IP |
| Sonos | Media | Speakers | LAN (UPnP) | Push (events + poll) | Any player's IP |
| Smart TV | Media + Remote | TVs (Sony Bravia) | LAN (vendor HTTP API) | Poll | Auto-discovered IP + PIN pairing |
| Home Assistant | Integration | Lights · media · power · remotes · sensors · everything else (generic) | REST + WebSocket | WebSocket push | Base URL + long-lived token |

---

## Philips Hue

- **Category** Light · **Transport** LAN, CLIP v2 over HTTPS to the bridge · **Live** Server-Sent Events push (changes appear instantly).
- **Setup** Enter the bridge IP — or use **Scan network** (Bifrost finds the bridge via SSDP) — then press the bridge's physical **link button** when prompted to mint an application key.
- **Capabilities** RGB color, color temperature, brightness, and **dynamic effects** (the bridge's CLIP v2 effects — candle, fire, sparkle, prism, …). Hue **rooms and zones** import as Bifrost Rooms, driven with native one-call group control. Hue **motion accessories** import as sensors — motion, light level, and temperature ride the same SSE stream, and a Hue room's sensors land in the linked Bifrost Room (feeding its occupancy).

Hue's ~10 req/s rate limit is handled with a per-bridge write pacer, so room-wide
fan-outs don't drop commands.

## Govee

- **Category** Light · **Transport** Govee Cloud API **and/or local LAN (UDP)** · **Live** polling.
- **Setup** Supply your **API key** (Govee Home app → Profile → About Us → Apply for API Key). **Local LAN control is on by default** — just turn on **“LAN Control”** for each device in the Govee Home app. (The LAN-interface field is advanced: leave blank for all interfaces / `0.0.0.0`, set a specific IP only if multi-homed.)
- **LAN-preferred control** — Bifrost controls each device over your **local network** whenever that device supports and has LAN Control enabled (faster, no cloud round-trip, no daily quota), and automatically **falls back to the cloud** for any device that isn't LAN-reachable. Only *some* Govee models support LAN, so this is decided per device. A host that can't reach the LAN (or has no API key) still works — it just uses whichever transport is available. The **Devices page shows how each device is reached** (a `LAN`/`Cloud` pill on the card, with detail in the expandable row).
- **Capabilities** RGB color, color temperature, brightness, and **effects** = the device's **dynamic light scenes** — the built-in catalogue *plus* your own DIY scenes (often 100+ on a strip; the effects picker has search + categories for this). Effects are applied via the cloud. No native rooms.

## LIFX

- **Category** Light · **Transport** LIFX Cloud API **and/or local LAN (UDP)** · **Live** polling.
- **Setup** Supply your **access token** ([cloud.lifx.com/settings](https://cloud.lifx.com/settings)). **Local LAN control is on by default** (LIFX LAN is on by default on the bulbs — no per-device toggle needed). (The LAN-interface field is advanced: blank = all interfaces / `0.0.0.0`.)
- **LAN-preferred control** — plain colour/brightness/power goes over your **local network** whenever a bulb is reachable (faster, no quota, works during a cloud outage), falling back to the **cloud** for any bulb that isn't, and for **effects** (effects run via the cloud). The Devices page shows each bulb's `LAN`/`Cloud` connection. A host that can't reach the LAN (or has no token) still works on whichever transport is available.
- **Capabilities** RGB color, color temperature, brightness, and **firmware effects** — `off`/`breathe`/`pulse` on every color bulb, `move` on multizone strips (Z/Beam), and `morph`/`flame` on matrix bulbs (Tile/Candle). **LIFX groups import as Bifrost Rooms** with one-call group control (cloud).

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

## Smart TV

- **Category** Media + Remote · **Transport** the TV's own LAN HTTP API · **Live** polling.
- **Capabilities** power (on/off, with the composite Wake-on-LAN/remote wake), volume, mute, transport, and a full **remote** (D-pad, navigation/media keys, app launch) — one TV surfaces as both a media device and a remote, unified into the **AIO TV control**.
- **Vendors** **Sony Bravia** today (the ScalarWeb JSON API for state/power/audio + IRCC for key codes). The integration is a vendor-agnostic framework (`BifrostSmartTv`); more brands are added as self-contained vendor files.

### Pairing a Sony Bravia (PIN)

Bravia control is authorised by a token you get through an on-screen **PIN**, so
there's nothing to copy off a website. The TV and your Bifrost host must be on the
**same network**. Scan network probes twice over: an SSDP multicast search plus an
HTTP sweep of the local subnet that asks each host the (unauthenticated) ScalarWeb
identity question — so the TV is found even when it dozes through the multicast
probe, and in Docker without host networking.

**1 — Allow control on the TV (one-time).** The exact menu wording differs by model:

- **Google TV / Android TV Bravia** (2015 and later): being on the same network is
  usually enough — the registration dialog (with the PIN) pops up the first time
  Bifrost pairs. If it never appears, enable the TV's IP/remote-device control under
  **Settings → Network & Internet** (look for *Home network*, *IP control*, or
  *Remote device / renderer*) and make sure the TV isn't in a store/restricted mode.
- **Older Linux Bravia** (KDL-series): **Settings → Network → Home Network Setup →
  IP Control** — set **Authentication** to *Normal and Pre-Shared Key* and turn
  **Simple IP Control** **On**.

**2 — Pair in Bifrost.**

1. **Settings → Providers → Add Provider**, and choose **Smart TV** as the type.
2. Click **Scan network** and pick your TV — its **IP auto-fills** and the vendor
   (Bravia) is auto-selected. (Or type the TV's IP into the *TV IP address* field.)
3. Click **Pair**. Bifrost asks the TV to register Bifrost; the TV shows a **4-digit
   PIN** on screen. (If the TV's IP-control **Authentication is set to "None"**, it
   has no pairing service at all — Pair reports **"No pairing needed"** and you can
   add the provider without a token.)
4. Type that PIN into the field that appears and click **Submit PIN**. On success the
   *Pairing token* is filled in for you (✓ Paired with TV).
5. Give the provider a **name** and click **Add**. Bifrost discovers the TV as a media
   device **and** a remote (one physical box), and you control it from the unified TV
   fly-out.

**Notes**

- The token is long-lived; you only re-pair if it's revoked (a TV factory reset, or
  Bifrost removed from the TV's list of registered remote devices).
- "Pair" greys out until the **TV IP** is set. If pairing reports the TV is
  unreachable, confirm the IP and that step 1 is done.
- Power-on uses the composite wake (Wake-on-LAN + the remote), so a TV in standby
  comes up even when its API is briefly unreachable.

!!! note "Newest integration"
    The Smart TV framework and the Bravia vendor are new and the least
    hardware-tested — please report anything off.

## Home Assistant

The **high-class integration**: one connection surfaces *any* Home Assistant
integration as Bifrost devices across **five domains from a single provider** —
lights, media (media players: TVs and speakers), power (switches, plugs, fans,
helpers), **remotes** (Android TV / streamers), and **sensors** (motion,
occupancy, contact, light level, temperature, humidity). HA **Areas import as
Bifrost Rooms**.

- **Category** Integration · **Transport** HA REST + a persistent WebSocket · **Live** WebSocket push — every domain stays live on one connection.
- **Setup** HA base URL (e.g. `http://homeassistant.local:8123`) + a **Long-Lived Access Token** (HA → Profile → Security → Long-Lived Access Tokens → Create Token).
- **Capabilities** lights pass through color / temperature / brightness and the entity's effect list; media players expose power / volume / mute / source / transport / now-playing and join-unjoin grouping; switches, plugs, and fans are on/off; remotes send keys, text, and app launches; sensors are read-only readings (presence kinds feed room occupancy). Named-content requests ("play Bob's Burgers on the bedroom TV") fall back to HA Assist.
- **Everything else** — HA device types Bifrost doesn't natively model (climate, covers, locks, helpers, vacuums, …) still surface as generic **"Other devices"** on the Devices page, with controls derived live from the entity's state. A new HA device type appears there with no Bifrost change.

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
