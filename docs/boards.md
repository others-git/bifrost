# Boards

Boards are your own dashboards — a grid of widgets you arrange to control and watch
your home at a glance. A board is well suited to a wall-mounted tablet, but works
anywhere.

## Building a board

1. Open **Boards** and tap **+ Board**, give it a name, and pick its **aspect
   ratio** — match the screen you'll show it on (16:9 for most monitors, or a
   tablet's own ratio such as **18.5:9** for a Galaxy A9; you can type a custom one).
   The board is shaped to that ratio and scales to fill the screen. Optionally
   **start from a room** — the board is seeded with that room's card, lights,
   speakers, and switches, ready to rearrange. You can have as many boards as you
   like (one per room, or one for the morning, …) — switch between them with the
   tabs.
2. Tap **Edit**, then **+ Widget** to add a widget and choose what it controls.
3. In edit mode, **drag a widget** to move it and **drag its bottom-right corner**
   to resize. The grid snaps to keep things tidy. Positions and sizes are
   **proportional**, so a board looks the same on a phone, a desktop, and a wall
   tablet — every widget scales to fill the screen it's shown on.
4. Tap **Done** to use the board. Tap a widget's ⚙ to reconfigure it, ⧉ to
   duplicate it (same setup, new spot), or ✕ to remove it. **Undo** (or Ctrl+Z)
   reverts the last layout change — a stray drag or removal is one tap away from
   restored.

## Widgets

- **Room** — a whole room's card, exactly as on the Control page: its lit dot and
  name, its quick-control buttons, a room power toggle that fans out to every
  member (lights, switches, speakers) server-side, and one glyph button per member
  device — speakers playing in sync collapse into a single grouped button, and each
  button opens that device's full fly-out (hold one to quick-toggle its power).
  Tapping anywhere else on the header opens the shared colour/brightness editor
  over the room's lights, with the room's scenes behind its Scenes button.
- **Device tile** — one light, speaker/TV, or switch. The tile glows in the
  device's real colour. Drag the bar to set brightness or volume, the round button
  toggles power, and tapping it opens the full controls.
- **Device group** — control several devices of one kind together (all the
  living-room lights, a set of speakers) with one brightness/volume bar and a master
  power toggle.
- **Button** — the compact form of a group: one glyph button that toggles a chosen
  set of devices, without the bar.
- **Now playing** — what's playing on a speaker or TV, with album art and its
  volume.
- **Scene** — apply a saved scene, or **Restore Home**.
- **Custom control** — a single button you configure: power, brightness, volume, or
  a scene, acting on any devices you pick.
- **Sensor** — a live reading (temperature, humidity, …) from a Home Assistant
  device.
- **Weather** — current conditions from a Home Assistant weather entity: a
  condition icon, the temperature, and humidity. Uses whatever weather
  integration Home Assistant already has — no extra setup or API key.
- **Recently added** — the newest items in a media library (needs a **feed
  source** such as Plex, added under Settings → Providers), as a wide **poster
  shelf** or a tall **vertical list** — pick the layout, the source, the
  library, and how many tiles to show; new episodes of one
  show roll up into a single tile ("3 new episodes") so a binge import can't
  flood the shelf. Optionally load **more on scroll** — extra items past the
  visible set, revealed by dragging the shelf/list (the widget itself never
  grows). Optionally, **tapping a poster opens that item on a TV** —
  pick which TV (any device with a remote) and which app to open; items launch
  straight to their detail page where the app supports it, and the TV wakes
  from standby if needed.
- **Clock** and **Label** — a clock, or text to title and section a board.

Every widget updates **live** — change a light elsewhere and the board reflects it
immediately.

Each widget also has an optional **name** if you'd rather show your own label than
the device's.

## Kiosk mode

Tap **Kiosk** to fill the whole screen with the board (tap **✕ Exit** to leave) —
the always-on look for a wall display.

### Auto-launch on a wall tablet

A paired kiosk can open straight into a board, full-screen, on load — no tapping.
Set it **per device** from a normal (non-kiosk) browser: **Settings → Clients**, and
on the kiosk's row pick its **board** (next to its room). That tablet then auto-opens
that board in kiosk mode every time it loads. Different tablets can show different
boards. Choose "No board" to turn auto-launch off.
