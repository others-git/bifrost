# Rooms & devices

How to organise your home in Bifrost: group devices into **rooms**, and tidy the
**device inventory** (glyphs, de-duplication, receiver binding, TV remotes, and
merging). Live control lives on the **Dashboard** and **Boards**;
*configuration* lives on the **Rooms** and **Devices** pages. (A **Floor Plan**
visualization also exists — its nav entry shows only with dev mode enabled.)

---

## Rooms

A **room** is Bifrost's core grouping. It can hold **any mix of device types** —
lights, speakers/receivers, power devices (switches, plugs, fans), and sensors —
and is the high-level control surface: turning a room on/off fans out to every
member, a brightness change touches its lights, and volume/mute fan out to its
speakers.

Room **configuration** (membership, links, audio calibration, quick buttons,
enable/disable, merge) is on the **Rooms page**. Live control (on/off, colour,
scenes, quick volume) stays on the Dashboard / Boards.

### Members

A room aggregates members two ways:

- **Direct assignment** — add lights/audio/power devices to the room on the Rooms
  page, or assign a single device to a room from its row on the **Devices page**
  (one direct room per device).
- **Linked provider groups (Sync)** — many providers already define their own
  groupings (Hue rooms/zones, Sonos rooms, Home Assistant *Areas*). Press
  **Sync** on a provider (Settings → Providers, or **Sync all**) and Bifrost
  mirrors those native groups and **links** them to Bifrost rooms — creating a
  room if one of the same name doesn't exist, or linking into the existing one.
  Sync covers both lights and audio.

A room's **effective members** are the linked groups' devices **plus** any
directly-assigned ones, so you can start from a provider's layout and tweak.

### Audio members & per-room volume

Speakers added to a room get a **per-room volume offset** — a trim (e.g. −10 / +5)
so a quiet speaker and a loud one even out when you set one room volume. A room's
volume/mute fans out to every media member with its offset applied. Configure the
media members and their offsets on the Rooms page.

### Quick-control buttons

Each room can carry **quick-control buttons** — small glyph buttons shown on the
room's Control card, left of its power button. Each does one thing on a chosen set
of members: **power** (toggle), **volume** or **brightness** (open the shared
editor scoped to those members), or **scene** (apply a saved scene). Add them on
the Rooms page.

### Occupancy (presence sensors)

A room with **motion or occupancy sensors** among its members gets a live,
provider-agnostic **occupancy** state — a Hue motion accessory and a Home
Assistant `binary_sensor` are interchangeable presence inputs. Presence-driven
behaviour reads this room-level state rather than any one sensor; the first
consumer is the wall tablet's display power saving (blank while the room is
empty, wake on motion — see [Kiosk control](api.md#kiosk-control-apikiosks)).

### Automations

Sensors, room occupancy, and device power can drive **automations** — "when
this, then that" rules that run in the hub. See [Automations](automations.md).

### Enable / disable & merge

- **Disable** a room to hide it from control without deleting its setup.
- **Merge** two rooms into one (Rooms page → merge). Useful when a synced provider
  group and a room you made by hand are really the same place — merging folds the
  members together instead of leaving duplicates.

### Scenes

A **scene** is a saved snapshot of live state, re-applied later:

- A **Home scene** captures the whole home (every light + power device).
- A **Room scene** captures just one room's effective members.

One Home scene can be the **Restore Home** default — a one-tap reset after a power
blip resets bulbs. Manage scenes on the **Scenes page**; save/apply a room's
scenes from its control fly-out. (See also the [`/scenes` API](api.md#scenes).)

---

## Devices

The **Devices page** is the full inventory — every device of every domain,
regardless of room: lights, media, power, remotes, **sensors** (read-only
readings — motion, contact, light level, temperature, humidity), and an **"Other
devices"** section for anything Home Assistant exposes that Bifrost doesn't
natively model (climate, covers, locks, …), with controls derived live from the
entity. It's a **configuration** surface, not a control one: enable or disable a
device, pin a glyph, assign it to a room, and resolve the special cases below.

### Glyph override

Each device shows a glyph derived from its type/kind. Pin a different one by name
(e.g. a switch that actually drives an LED strip can show the strip glyph). The
override is used everywhere the device appears.

### Cross-provider de-duplication (shadowing)

A physical device reachable **both natively and through Home Assistant** (e.g. a
Hue bulb that HA also exposes) imports as **two rows**. Bifrost clusters them by
**hardware id** (MAC) after each discovery and **shadows** the integration copy
under the native one — **native always wins**. The shadowed copy stays tracked but
is hidden from control and room membership, and collapsed on the Devices page.

Matching is exact-MAC only (safe to auto-apply). For a device that exposes no
hardware id, link a duplicate **manually** from the Devices page. When you flag a
capability the native provider is missing versus its HA twin, file it — de-dup
hides the HA copy, so an unfiled gap is a silently lost feature.

### Binding a source to a receiver

A **source** device (TV, streamer, console) that has no volume of its own can be
**bound to a receiver / AVR** that does. Once bound:

- **Volume & mute** route to the receiver.
- **Power, input/source, and transport** stay on the source.
- The receiver **mirrors the source's power**: powering the source on wakes the
  receiver and switches it to the source's input; powering the source off takes
  the receiver down with it — the bound pair behaves as one appliance.

Many sources can bind to one receiver. Configure this on the Devices page; it maps
to [`PUT /audio/devices/{id}/receiver`](api.md#bind-a-source-to-a-receiver).

### TVs & the on-screen remote

A TV/streamer can expose a **virtual remote** — opened from the **Remote**
button on the TV's audio fly-out. Navigation has two forms, remembered per
client: the **Scrying Glass**, a gesture slab you use with your eyes on the TV
(flick anywhere = one arrow press, drag-and-hold = auto-repeat for long rails,
tap = OK, two-finger tap = back, with haptic ticks where the device supports
them), and a classic **Keys** cross for those who want targets — plus discrete
Back/Home/Menu, transport, volume, text entry, and app-launch tiles. On a
desktop, the physical keyboard drives it too (arrows / Enter / Backspace)
while the remote is open. The remote **auto-pairs** to its TV: when a `remote` entity and a
`media_player` share a hardware id (Home Assistant commonly exposes both for one
TV), Bifrost links them during discovery — no manual step. The app tiles are the
TV's **own installed catalog** when the vendor can enumerate it (a Bravia's app
list, refreshed each time the launcher opens), so everything installed is
launchable — not just apps you've already opened; foreground-observed apps fill
in for remotes without a catalog. App names are tidied to their brand (e.g.
*Hulu*, *Prime Video*), and you can pin the ones you use.

With the remote paired, the TV also **pushes its state live**: the foreground
app shows as the TV's now-playing everywhere ("Netflix" on the card, boards,
and now-playing widgets — Android TVs don't report this over their HTTP API),
and volume/power changes made with the TV's own remote appear immediately
(and fire device-trigger automations without waiting on a poll).

If the TV's volume is owned by a receiver, bind it (above) and the remote's
volume keys drive the receiver.

### Merging complementary devices (composite)

Sometimes one physical device shows up as **two complementary entities** — for
example HA surfaces a TV's transport on one entity and its input list on another.
**Merge** them into one on the Devices page: pick a **primary**, and the
companion's state and controls (now-playing, sources, a receiver binding) merge
*into* it. This differs from shadowing — a composite **keeps** the companion's
abilities (union), whereas a shadow **discards** the duplicate. Maps to
[`PUT /audio/devices/{id}/companion`](api.md#merge-a-duplicate-device-composite).
