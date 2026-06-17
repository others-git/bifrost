// Monochrome device glyphs (currentColor stroke), shared by the Control page's
// device buttons and the Devices inventory. Each device *type* has a default
// glyph — a bulb for lights, a speaker for audio, and a per-kind glyph for power
// devices — so a device reads at a glance without its (often long) name. A
// device may also pin an override glyph by name (e.g. a switch that drives an
// LED strip can show the led_strip glyph); see `Glyph` and `GLYPH_OPTIONS`.

import type { AudioDevice, PowerKind } from "../api";

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
    // A thin, wavy length of flexible LED tape (the two edges) with its LED
    // chips dotted along it — reads as a soft ribbon, never a rigid power strip.
    <>
      <path d="M1.5 11 C6 8.5 8.5 8.5 12 11 S18 13.5 22.5 11" />
      <path d="M1.5 12.8 C6 10.3 8.5 10.3 12 12.8 S18 15.3 22.5 12.8" />
      <circle cx="3.8" cy="10.4" r="0.6" fill="currentColor" stroke="none" />
      <circle cx="7.9" cy="9.7" r="0.6" fill="currentColor" stroke="none" />
      <circle cx="12" cy="11.9" r="0.6" fill="currentColor" stroke="none" />
      <circle cx="16.1" cy="14.1" r="0.6" fill="currentColor" stroke="none" />
      <circle cx="20.2" cy="13.4" r="0.6" fill="currentColor" stroke="none" />
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
};

/// User-pickable override glyphs, with display labels (the per-kind power glyphs
/// are reachable too, so a fan-driving switch can borrow the fan icon).
export const GLYPH_OPTIONS: { name: string; label: string }[] = [
  { name: "bulb", label: "Bulb" },
  { name: "led_strip", label: "LED strip" },
  { name: "speaker", label: "Speaker" },
  { name: "speaker_group", label: "Speaker group" },
  { name: "receiver", label: "Receiver" },
  { name: "tv", label: "TV" },
  { name: "outlet", label: "Outlet" },
  { name: "plug", label: "Plug" },
  { name: "switch", label: "Switch" },
  { name: "toggle", label: "Toggle" },
  { name: "fan", label: "Fan" },
  { name: "generic", label: "Generic" },
];

/// Glyphs offered for a configured room control button (the "Add Control" flow).
/// Control-meaningful first (power/volume/brightness/scene), then device glyphs
/// so a button can read as the thing it drives.
export const CONTROL_GLYPH_OPTIONS: { name: string; label: string }[] = [
  { name: "power", label: "Power" },
  { name: "volume", label: "Volume" },
  { name: "brightness", label: "Brightness" },
  { name: "scene", label: "Scene" },
  { name: "bulb", label: "Bulb" },
  { name: "led_strip", label: "LED strip" },
  { name: "speaker", label: "Speaker" },
  { name: "fan", label: "Fan" },
  { name: "plug", label: "Plug" },
  { name: "tv", label: "TV" },
  { name: "generic", label: "Generic" },
];

/// Render any glyph by name. Unknown names fall back to the generic glyph.
export function Glyph({ name, size = 22 }: { name: string; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" {...base}>
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
export function audioKindGlyph(kind: AudioDevice["kind"]): string {
  switch (kind) {
    case "tv":
      return "tv";
    case "receiver":
      return "receiver";
    default:
      return "speaker";
  }
}

export function LightGlyph({ size = 22 }: { size?: number }) {
  return <Glyph name="bulb" size={size} />;
}

export function AudioGlyph({ size = 22 }: { size?: number }) {
  return <Glyph name="speaker" size={size} />;
}

export function PowerGlyph({ kind, size = 22 }: { kind: PowerKind; size?: number }) {
  return <Glyph name={powerKindGlyph(kind)} size={size} />;
}
