// Monochrome device glyphs (currentColor stroke), shared by the Control page's
// device buttons and the Devices inventory. Each device *type* has a default
// glyph — a bulb for lights, a speaker for audio, and a per-kind glyph for power
// devices — so a device reads at a glance without its (often long) name. A
// device may also pin an override glyph by name (e.g. a switch that drives an
// LED strip can show the led_strip glyph); see `Glyph` and `ALL_GLYPH_OPTIONS`.

import type { MediaDevice, PowerKind, SensorDevice } from "../api";
import { ACCENT, alpha, T } from "../theme";

const base = {
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.7,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
};

// ── Glyph bodies, keyed by name ──────────────────────────────────────────────
// Each entry is just the inner SVG markup; `Glyph` wraps it in the shared
// <svg> so sizing and stroke styling live in one place.

const BODIES: Record<string, JSX.Element> = {
  logout: (
    // The power symbol (a broken ring + top bar) — the sign-out affordance.
    // Drawn as SVG so it renders everywhere; the `⏻` glyph is missing from many
    // device fonts and shows as tofu.
    <>
      <path d="M12 3v8.5" />
      <path d="M7.8 6.7a6.5 6.5 0 1 0 8.4 0" />
    </>
  ),
  bulb: (
    <>
      <path d="M9.5 18.5h5" />
      <path d="M10.5 21h3" />
      <path d="M12 3a6 6 0 0 0-3.5 10.9c.6.5.9 1 .9 1.7v.4h5.2v-.4c0-.7.3-1.2.9-1.7A6 6 0 0 0 12 3Z" />
    </>
  ),
  led_strip: (
    // A serpentine length of LED strip (a winding ribbon, drawn thick) with a
    // thin connecting wire trailing off the bottom end — reads as a flexible
    // addressable strip with its lead, never a rigid power strip.
    <>
      <path d="M7 6 H17 A3 3 0 0 1 17 12 H7 A3 3 0 0 0 7 18 H17" strokeWidth={2.8} />
      <path d="M17 18q2.6.5 2.8 3.4" strokeWidth={1.2} />
    </>
  ),
  triangle: (
    // A strip of joined triangular light panels (up · down · up) — the Nanoleaf
    // flagship shape, distinct from a plain warning/play triangle.
    <>
      <path d="M3 18 L7.5 7 L12 18 Z" />
      <path d="M7.5 7 L12 18 L16.5 7 Z" />
      <path d="M12 18 L16.5 7 L21 18 Z" />
    </>
  ),
  speaker: (
    <>
      <path d="M4 9.5v5h3.5L13 19V5L7.5 9.5H4Z" />
      <path d="M16.5 9.2a4 4 0 0 1 0 5.6" />
      <path d="M19 7a7 7 0 0 1 0 10" />
    </>
  ),
  receiver: (
    // A wide AV receiver/amp: vent slots on the left, two control knobs right.
    <>
      <rect x="2.5" y="7" width="19" height="10" rx="1.6" />
      <path d="M5.5 10.5h4" />
      <path d="M5.5 13.5h4" />
      <circle cx="15.5" cy="12" r="1.4" />
      <circle cx="18.7" cy="12" r="1.4" />
    </>
  ),
  speaker_group: (
    // Two overlapping speakers — a live multi-speaker sync group.
    <>
      <path d="M3 10v4h2.6L9 17V7L5.6 10H3Z" />
      <path d="M12 10v4h2.6L18 17V7l-3.4 3H12Z" />
      <path d="M20.5 9.4a4 4 0 0 1 0 5.2" />
    </>
  ),
  tv: (
    <>
      <rect x="3" y="5" width="18" height="12" rx="2" />
      <path d="M8 21h8" />
      <path d="M12 17v4" />
    </>
  ),
  remote: (
    // A handheld remote: body, a D-pad ring, and a few buttons.
    <>
      <rect x="7" y="2.5" width="10" height="19" rx="3" />
      <circle cx="12" cy="13" r="2.9" />
      <circle cx="12" cy="13" r="0.7" fill="currentColor" stroke="none" />
      <circle cx="12" cy="6.4" r="0.85" fill="currentColor" stroke="none" />
      <circle cx="10" cy="18.6" r="0.7" fill="currentColor" stroke="none" />
      <circle cx="14" cy="18.6" r="0.7" fill="currentColor" stroke="none" />
    </>
  ),
  outlet: (
    <>
      <rect x="4" y="4" width="16" height="16" rx="3" />
      <line x1="10" y1="9" x2="10" y2="12" />
      <line x1="14" y1="9" x2="14" y2="12" />
      <circle cx="12" cy="15.5" r="0.6" fill="currentColor" stroke="none" />
    </>
  ),
  plug: (
    <>
      <path d="M9 3v5" />
      <path d="M15 3v5" />
      <path d="M6.5 8h11v3a5.5 5.5 0 0 1-11 0V8Z" />
      <path d="M12 16.5V21" />
    </>
  ),
  fan: (
    <>
      <circle cx="12" cy="12" r="1.6" />
      <path d="M12 10.4C12 7 13.5 4.5 16 5c1.6 .9 .7 4-4 5.4Z" />
      <path d="M13.6 12C17 12 19.5 13.5 19 16c-.9 1.6-4 .7-5.4-4Z" />
      <path d="M12 13.6C12 17 10.5 19.5 8 19c-1.6-.9-.7-4 4-5.4Z" />
    </>
  ),
  toggle: (
    <>
      <rect x="3" y="8" width="18" height="8" rx="4" />
      <circle cx="15" cy="12" r="2.4" fill="currentColor" stroke="none" />
    </>
  ),
  switch: (
    <>
      <rect x="6" y="3" width="12" height="18" rx="2.5" />
      <rect x="9.5" y="6.5" width="5" height="7" rx="1.2" />
    </>
  ),
  generic: (
    <>
      <rect x="4" y="4" width="16" height="16" rx="3" />
      <circle cx="12" cy="12" r="2.2" fill="currentColor" stroke="none" />
    </>
  ),
  room: (
    // A house — the room/assignment affordance.
    <>
      <path d="M4 11.3 12 5l8 6.3" />
      <path d="M6 10.2V19h12v-8.8" />
      <path d="M10.3 19v-4.6h3.4V19" />
    </>
  ),
  power: (
    // The power symbol (a broken ring + top bar) — for on/off control buttons.
    <>
      <path d="M12 3v8.5" />
      <path d="M7.8 6.7a6.5 6.5 0 1 0 8.4 0" />
    </>
  ),
  volume: (
    // A speaker cone with sound waves — for a volume control button.
    <>
      <path d="M4 9.5v5h3.5L13 19V5L7.5 9.5H4Z" />
      <path d="M16.5 9.2a4 4 0 0 1 0 5.6" />
      <path d="M19 7a7 7 0 0 1 0 10" />
    </>
  ),
  brightness: (
    // A sun with rays — for a brightness control button.
    <>
      <circle cx="12" cy="12" r="3.6" />
      <path d="M12 2.5v2.4M12 19.1v2.4M2.5 12h2.4M19.1 12h2.4M5.1 5.1l1.7 1.7M17.2 17.2l1.7 1.7M18.9 5.1l-1.7 1.7M6.8 17.2l-1.7 1.7" />
    </>
  ),
  scene: (
    // Three sliders — a saved scene / preset look.
    <>
      <path d="M4 7h16M4 12h16M4 17h16" />
      <circle cx="9" cy="7" r="1.9" fill="currentColor" stroke="none" />
      <circle cx="15.5" cy="12" r="1.9" fill="currentColor" stroke="none" />
      <circle cx="7.5" cy="17" r="1.9" fill="currentColor" stroke="none" />
    </>
  ),
  restore: (
    // A counterclockwise circular arrow — revert / restore to a saved state.
    <>
      <path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
      <path d="M3 3v5h5" />
    </>
  ),
  copy: (
    // Two offset sheets — duplicate.
    <>
      <rect x="9" y="9" width="11" height="11" rx="2" />
      <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
    </>
  ),
  gear: (
    // A cog — configure / settings.
    <>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </>
  ),

  // ── Smart-remote keys (themed, replacing emoji) ──────────────────────────────
  send: (
    // Paper-plane — submit typed text to the TV.
    <>
      <path d="M21 4 3 11l6 2.5L21 4Z" />
      <path d="M21 4 11 20l-2-6.5L21 4Z" />
    </>
  ),
  chevron: (
    // A down chevron — the d-pad rotates it for up/left/right.
    <path d="M5 9l7 7 7-7" />
  ),
  back: (
    // Curved return arrow.
    <>
      <path d="M9 7 4 12l5 5" />
      <path d="M4 12h10a5 5 0 0 1 0 10h-1" />
    </>
  ),
  home: (
    <>
      <path d="M3.5 11.5 12 4l8.5 7.5" />
      <path d="M5.5 10.5V20h13v-9.5" />
    </>
  ),
  menu: <path d="M4 7h16M4 12h16M4 17h16" />,
  mute: (
    // Speaker with a cross — muted.
    <>
      <path d="M4 9.5v5h3.5L13 19V5L7.5 9.5H4Z" />
      <path d="M16.5 9.5l4.5 5M21 9.5l-4.5 5" />
    </>
  ),
  volume_down: (
    // Speaker with one wave — quieter.
    <>
      <path d="M4 9.5v5h3.5L13 19V5L7.5 9.5H4Z" />
      <path d="M16.5 9.2a4 4 0 0 1 0 5.6" />
    </>
  ),
  play_pause: (
    // Play triangle alongside pause bars.
    <>
      <path d="M4 6l7 5-7 5V6Z" fill="currentColor" stroke="none" />
      <path d="M15 6.5v9M19 6.5v9" />
    </>
  ),
  prev: (
    <>
      <path d="M17 6l-7 6 7 6V6Z" fill="currentColor" stroke="none" />
      <path d="M7 6v12" />
    </>
  ),
  next: (
    <>
      <path d="M7 6l7 6-7 6V6Z" fill="currentColor" stroke="none" />
      <path d="M17 6v12" />
    </>
  ),
  play: <path d="M7 5l11 7-11 7V5Z" fill="currentColor" stroke="none" />,
  pause: (
    <>
      <rect x="6.5" y="5" width="3.4" height="14" rx="1" fill="currentColor" stroke="none" />
      <rect x="14.1" y="5" width="3.4" height="14" rx="1" fill="currentColor" stroke="none" />
    </>
  ),
  favorite: (
    // A heart — favourites.
    <path d="M12 20.5 4.6 13a4.6 4.6 0 0 1 7.4-5.3A4.6 4.6 0 0 1 19.4 13L12 20.5Z" />
  ),
  star: <path d="M12 3.6l2.6 5.3 5.8.8-4.2 4.1 1 5.8-5.2-2.7-5.2 2.7 1-5.8L3.6 9.7l5.8-.8L12 3.6Z" />,
  link: (
    // A chain link — a synced provider group / linkage.
    <>
      <path d="M10 13.5a3.5 3.5 0 0 0 5 0l2.5-2.5a3.5 3.5 0 0 0-5-5L11.2 7.3" />
      <path d="M14 10.5a3.5 3.5 0 0 0-5 0L6.5 13a3.5 3.5 0 0 0 5 5l1.3-1.3" />
    </>
  ),
  dice: (
    // A die — randomise / generate.
    <>
      <rect x="4" y="4" width="16" height="16" rx="3.5" />
      <circle cx="9" cy="9" r="1.1" fill="currentColor" stroke="none" />
      <circle cx="15" cy="9" r="1.1" fill="currentColor" stroke="none" />
      <circle cx="12" cy="12" r="1.1" fill="currentColor" stroke="none" />
      <circle cx="9" cy="15" r="1.1" fill="currentColor" stroke="none" />
      <circle cx="15" cy="15" r="1.1" fill="currentColor" stroke="none" />
    </>
  ),
  star_fill: (
    <path d="M12 3.6l2.6 5.3 5.8.8-4.2 4.1 1 5.8-5.2-2.7-5.2 2.7 1-5.8L3.6 9.7l5.8-.8L12 3.6Z" fill="currentColor" stroke="none" />
  ),

  // ── Floor-plan tool palette ────────────────────────────────────────────────
  cursor: (
    // Selection arrow — the view / select tool.
    <path d="M5 4 18 10.5 12.6 12.1 10 18 5 4Z" />
  ),
  floor: (
    // A gridded tile field — paint floor.
    <>
      <rect x="3.5" y="3.5" width="17" height="17" rx="1.5" />
      <path d="M9 3.5v17M14.5 3.5v17M3.5 9h17M3.5 14.5h17" />
    </>
  ),
  wall: (
    // Brick courses — draw walls.
    <>
      <rect x="3.5" y="6" width="17" height="12" rx="1" />
      <path d="M3.5 12h17M11 6v6M7 12v6M15 12v6" />
    </>
  ),
  erase: (
    // An angled eraser block on a baseline.
    <>
      <path d="M8.5 8 4.2 12.3a2 2 0 0 0 0 2.8l3 3h6.3l6.3-6.3a2 2 0 0 0 0-2.8l-2.7-2.7a2 2 0 0 0-2.8 0Z" />
      <path d="M7.2 15.1h6.3" />
      <path d="M4 21h16" />
    </>
  ),
  place: (
    // A map pin with a core — place a device.
    <>
      <path d="M12 21s6-5.4 6-10A6 6 0 0 0 6 11c0 4.6 6 10 6 10Z" />
      <circle cx="12" cy="11" r="2.2" fill="currentColor" stroke="none" />
    </>
  ),
  brush: (
    // A paint roller — the live color brush.
    <>
      <rect x="3.5" y="4" width="13" height="5.5" rx="1.4" />
      <path d="M10 9.5v3h3.5v2.4" />
      <rect x="11.6" y="15.3" width="3.8" height="5.7" rx="1.1" />
    </>
  ),

  // ── Weather condition icons (mapped from a HA weather entity's condition) ──
  wx_sun: (
    <>
      <circle cx="12" cy="12" r="4.2" />
      <path d="M12 2v2.4M12 19.6V22M4.2 4.2l1.7 1.7M18.1 18.1l1.7 1.7M2 12h2.4M19.6 12H22M4.2 19.8l1.7-1.7M18.1 5.9l1.7-1.7" />
    </>
  ),
  wx_moon: <path d="M21 12.8A9 9 0 1 1 11.2 3 7 7 0 0 0 21 12.8Z" />,
  wx_cloud: <path d="M17.5 19a4.5 4.5 0 0 0 .5-9 7 7 0 0 0-13.4 2A4 4 0 0 0 5 19Z" />,
  wx_partly: (
    <>
      <circle cx="7.5" cy="7" r="3" />
      <path d="M7.5 1.7v1.6M2.2 7H3.8M3.6 3.1l1.1 1.1M11.4 3.1l-1.1 1.1" />
      <path d="M17.5 20a4 4 0 0 0 .4-8 6.2 6.2 0 0 0-11.7 1.6A3.6 3.6 0 0 0 6.2 20Z" />
    </>
  ),
  wx_rain: (
    <>
      <path d="M17.5 16a4.2 4.2 0 0 0 .4-8.4 6.6 6.6 0 0 0-12.5 1.7A3.8 3.8 0 0 0 6 16Z" />
      <path d="M8 19l-1 2.4M12 19l-1 2.4M16 19l-1 2.4" />
    </>
  ),
  wx_snow: (
    <>
      <path d="M17.5 15a4.2 4.2 0 0 0 .4-8.4 6.6 6.6 0 0 0-12.5 1.7A3.8 3.8 0 0 0 6 15Z" />
      <circle cx="8" cy="19" r="0.7" fill="currentColor" stroke="none" />
      <circle cx="12" cy="20.5" r="0.7" fill="currentColor" stroke="none" />
      <circle cx="16" cy="19" r="0.7" fill="currentColor" stroke="none" />
    </>
  ),
  wx_storm: (
    <>
      <path d="M17.5 14a4.2 4.2 0 0 0 .4-8.4 6.6 6.6 0 0 0-12.5 1.7A3.8 3.8 0 0 0 6 14Z" />
      <path d="M12 13l-2.5 4H12l-1.2 4 3.2-5h-2.2L13 13Z" fill="currentColor" stroke="none" />
    </>
  ),
  wx_fog: (
    <>
      <path d="M17.5 11.5a4.2 4.2 0 0 0 .4-8.4A6.6 6.6 0 0 0 5.4 4.8 3.8 3.8 0 0 0 6 11.5Z" />
      <path d="M4 15.5h16M6 19h12" />
    </>
  ),
  // ── Sensor icons (read-only inputs) ──────────────────────────────────────────
  motion: (
    // A walking figure — a motion detector's presence signal.
    <>
      <circle cx="13" cy="4.5" r="1.6" />
      <path d="M13 8l-2.5 3 2 2 .5 5" />
      <path d="M10.5 11l-3 1M13 13l3 1.5M8 21l2.5-4" />
    </>
  ),
  occupancy: (
    // A person inside radiating arcs — an occupancy / presence sensor.
    <>
      <circle cx="12" cy="9" r="2.2" />
      <path d="M8.5 15.5a3.5 3.5 0 0 1 7 0" />
      <path d="M5.5 5.5a9 9 0 0 1 13 0M7.8 8a5.7 5.7 0 0 1 8.4 0" />
    </>
  ),
  contact: (
    // A door ajar — a contact (door/window) sensor.
    <>
      <path d="M5 20h11V4L9 6v14" />
      <path d="M5 20h14" />
      <circle cx="11" cy="12" r="0.7" fill="currentColor" stroke="none" />
    </>
  ),
  illuminance: (
    // A sun-and-gauge — an ambient light (lux) sensor.
    <>
      <circle cx="12" cy="11" r="3.2" />
      <path d="M12 4.5v1.6M12 15.9v1.6M4.8 11h1.6M17.6 11h1.6M6.9 5.9l1.1 1.1M16 15l1.1 1.1M6.9 16.1 8 15M16 7l1.1-1.1" />
    </>
  ),
  temperature: (
    // A thermometer bulb — a temperature sensor.
    <>
      <path d="M12 4a2 2 0 0 0-2 2v7.5a3.5 3.5 0 1 0 4 0V6a2 2 0 0 0-2-2Z" />
      <path d="M12 9v5.2" />
    </>
  ),
  humidity: (
    // A droplet — a humidity sensor.
    <path d="M12 3.5c3 3.7 5 6.4 5 9a5 5 0 0 1-10 0c0-2.6 2-5.3 5-9Z" />
  ),
};

// HA weather conditions → a small icon set. Keeps the Boards weather widget
// glanceable without a per-condition glyph.
export function weatherGlyph(condition: string | undefined | null): string {
  switch ((condition ?? "").toLowerCase()) {
    case "sunny":
    case "clear":
      return "wx_sun";
    case "clear-night":
      return "wx_moon";
    case "partlycloudy":
      return "wx_partly";
    case "cloudy":
    case "exceptional":
      return "wx_cloud";
    case "rainy":
    case "pouring":
    case "hail":
      return "wx_rain";
    case "snowy":
    case "snowy-rainy":
      return "wx_snow";
    case "lightning":
    case "lightning-rainy":
      return "wx_storm";
    case "fog":
    case "windy":
    case "windy-variant":
      return "wx_fog";
    default:
      return "wx_cloud";
  }
}

// Human-readable label for a HA weather condition token.
export function weatherLabel(condition: string | undefined | null): string {
  const c = (condition ?? "").trim();
  if (!c) return "—";
  return c
    .replace(/-/g, " ")
    .replace(/\b\w/g, (m) => m.toUpperCase());
}

/// The full palette of pickable glyphs, with display labels — the single source
/// every glyph picker (device override, room control, board button/control)
/// renders, so they all show the same complete set. Add a glyph to `BODIES` and a
/// row here and it appears everywhere.
export const ALL_GLYPH_OPTIONS: { name: string; label: string }[] = [
  // Devices
  { name: "bulb", label: "Bulb" },
  { name: "led_strip", label: "LED strip" },
  { name: "triangle", label: "Triangle panels" },
  { name: "speaker", label: "Speaker" },
  { name: "speaker_group", label: "Speaker group" },
  { name: "receiver", label: "Receiver" },
  { name: "tv", label: "TV" },
  { name: "remote", label: "Remote" },
  { name: "outlet", label: "Outlet" },
  { name: "plug", label: "Plug" },
  { name: "switch", label: "Switch" },
  { name: "toggle", label: "Toggle" },
  { name: "fan", label: "Fan" },
  { name: "room", label: "Room" },
  { name: "home", label: "Home" },
  { name: "generic", label: "Generic" },
  // Sensors
  { name: "motion", label: "Motion" },
  { name: "occupancy", label: "Occupancy" },
  { name: "contact", label: "Contact" },
  { name: "illuminance", label: "Light level" },
  { name: "temperature", label: "Temperature" },
  { name: "humidity", label: "Humidity" },
  // Actions / control
  { name: "power", label: "Power" },
  { name: "brightness", label: "Brightness" },
  { name: "volume", label: "Volume" },
  { name: "volume_down", label: "Volume down" },
  { name: "mute", label: "Mute" },
  { name: "scene", label: "Scene" },
  { name: "restore", label: "Restore" },
  { name: "copy", label: "Copy" },
  { name: "play", label: "Play" },
  { name: "pause", label: "Pause" },
  { name: "play_pause", label: "Play / pause" },
  { name: "prev", label: "Previous" },
  { name: "next", label: "Next" },
  { name: "send", label: "Send" },
  { name: "favorite", label: "Favorite" },
  { name: "star", label: "Star" },
  { name: "link", label: "Link" },
  // Weather
  { name: "wx_sun", label: "Sunny" },
  { name: "wx_moon", label: "Clear night" },
  { name: "wx_cloud", label: "Cloudy" },
  { name: "wx_partly", label: "Partly cloudy" },
  { name: "wx_rain", label: "Rain" },
  { name: "wx_snow", label: "Snow" },
  { name: "wx_storm", label: "Storm" },
  { name: "wx_fog", label: "Fog" },
];

/// Room/board control & button glyph picker — the full palette, with the four
/// control-meaningful glyphs hoisted to the front so a button reads as its action.
export const CONTROL_GLYPH_OPTIONS: { name: string; label: string }[] = [
  { name: "power", label: "Power" },
  { name: "volume", label: "Volume" },
  { name: "brightness", label: "Brightness" },
  { name: "scene", label: "Scene" },
  ...ALL_GLYPH_OPTIONS.filter((g) => !["power", "volume", "brightness", "scene"].includes(g.name)),
];

/// Render any glyph by name. Unknown names fall back to the generic glyph.
export function Glyph({ name, size = 22 }: { name: string; size?: number | string }) {
  return (
    // `display:block` removes the inline-baseline descender space an inline <svg>
    // otherwise carries, which would push the glyph visually high in any centered
    // (grid/flex) container — keeps every glyph truly centred in its niche.
    <svg width={size} height={size} viewBox="0 0 24 24" {...base} style={{ display: "block" }}>
      {BODIES[name] ?? BODIES.generic}
    </svg>
  );
}

/// The default glyph name for a power device of the given kind.
export function powerKindGlyph(kind: PowerKind): string {
  switch (kind) {
    case "outlet":
    case "fan":
    case "toggle":
    case "switch":
      return kind;
    default:
      return "generic";
  }
}

/// The default glyph name for an audio device of the given kind — a TV gets the
/// TV glyph, a receiver the amp glyph, everything else a speaker. Driven by the
/// device's `kind` (HA's `device_class`, Onkyo/Sonos topology), no per-device
/// special-casing.
export function mediaKindGlyph(kind: MediaDevice["kind"]): string {
  switch (kind) {
    case "tv":
      return "tv";
    case "receiver":
      return "receiver";
    default:
      return "speaker";
  }
}

/// The default glyph name for a sensor of the given kind — the kind name doubles
/// as the glyph name for every modeled kind, with `generic` as the fallback.
export function sensorKindGlyph(kind: SensorDevice["kind"]): string {
  const known = ["motion", "occupancy", "contact", "illuminance", "temperature", "humidity"];
  return known.includes(kind) ? kind : "generic";
}

export function LightGlyph({ size = 22 }: { size?: number }) {
  return <Glyph name="bulb" size={size} />;
}

export function MediaGlyph({ size = 22 }: { size?: number }) {
  return <Glyph name="speaker" size={size} />;
}

export function PowerGlyph({ kind, size = 22 }: { kind: PowerKind; size?: number }) {
  return <Glyph name={powerKindGlyph(kind)} size={size} />;
}

/** The one glyph-picker grid, reused everywhere a glyph is chosen (device glyph
 * override, room-control buttons, board button/control icons). Renders the given
 * `options` as a themed flex-wrap of square buttons, the selected one cyan-lit.
 * Callers own the surrounding chrome (anchored panel / sheet) and any "Auto"
 * (clear-override) affordance — this is just the grid. */
export function GlyphGrid({
  options,
  value,
  onPick,
  size = 40,
}: {
  options: { name: string; label: string }[];
  value: string | null;
  onPick: (name: string) => void;
  size?: number;
}) {
  return (
    <div style={{ display: "flex", flexWrap: "wrap", gap: "0.35rem" }}>
      {options.map((g) => {
        const on = value === g.name;
        return (
          <button
            key={g.name}
            type="button"
            title={g.label}
            aria-label={g.label}
            onClick={() => onPick(g.name)}
            style={{
              width: size,
              height: size,
              display: "grid",
              placeItems: "center",
              borderRadius: 8,
              cursor: "pointer",
              color: on ? ACCENT : T.dim,
              background: on ? alpha(ACCENT, 0.12) : "rgba(255,255,255,0.03)",
              border: `1px solid ${on ? ACCENT : T.cardBorder}`,
            }}
          >
            <Glyph name={g.name} size={Math.round(size / 2)} />
          </button>
        );
      })}
    </div>
  );
}
