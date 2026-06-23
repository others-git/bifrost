// The shared light editor: a popover anchored next to whatever triggered it,
// with a Hue-style hue/saturation color wheel and a vertical brightness bar.
// Used for lights, rooms, scene palettes, and the planner paint brush.

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { rgbToHex } from "../api";
import { useViewport } from "../useViewport";
import { color, glow, alpha } from "../theme";
import { Segmented, PowerToggle } from "./controls";
import { Flyout, FlyoutHeader } from "./Flyout";

// ── HSV color math (h in degrees, s/v in 0..1) ──────────────────────────────

export function hsvToRgb(h: number, s: number, v: number): [number, number, number] {
  const f = (n: number) => {
    const k = (n + h / 60) % 6;
    return v - v * s * Math.max(0, Math.min(k, 4 - k, 1));
  };
  return [Math.round(f(5) * 255), Math.round(f(3) * 255), Math.round(f(1) * 255)];
}

export function rgbToHsv(r: number, g: number, b: number): [number, number, number] {
  const rn = r / 255, gn = g / 255, bn = b / 255;
  const max = Math.max(rn, gn, bn), min = Math.min(rn, gn, bn);
  const d = max - min;
  let h = 0;
  if (d > 0) {
    if (max === rn) h = 60 * (((gn - bn) / d) % 6);
    else if (max === gn) h = 60 * ((bn - rn) / d + 2);
    else h = 60 * ((rn - gn) / d + 4);
  }
  if (h < 0) h += 360;
  return [h, max === 0 ? 0 : d / max, max];
}

export function hexToRgb(hex: string): [number, number, number] {
  return [
    parseInt(hex.slice(1, 3), 16),
    parseInt(hex.slice(3, 5), 16),
    parseInt(hex.slice(5, 7), 16),
  ];
}

/** Project a hex color onto the wheel: hue + saturation at full value. */
export function hexToHs(hex: string): [number, number] {
  const [h, s] = rgbToHsv(...hexToRgb(hex));
  return [h, s];
}

// ── Light control change contract ───────────────────────────────────────────
//
// Every light control surface (the color wheel, the white/temperature wheel, the
// brightness bar, the swatches) reports a change through this one discriminated
// union. The `field` says *which* dimension moved, so a fan-out caller (a room
// cascade) can apply only that dimension and leave each member light's other
// attributes intact — and so a brightness drag never overwrites a per-light
// color, a color pick never resets brightness, and color vs. white stay distinct.
// Add a new light control by adding a variant here and a case in each caller.

export type LightControlChange =
  | { field: "color"; hex: string }
  | { field: "brightness"; brightness: number }
  | { field: "temp"; mirek: number }
  | { field: "effect"; effect: string };

// Warm whites first (the lived-in defaults), then color anchors.
const SWATCHES = ["#ffffff", "#ffe4b3", "#ffb46b", "#ff7d33", "#ff5e9c", "#8b5cf6", "#3b82f6", "#4ade80"];

// ── Color temperature (mirek ⇄ Kelvin ⇄ sRGB) ───────────────────────────────
//
// Mirek (micro-reciprocal kelvin = 1e6 / K) is the unit Hue uses; lower = cooler.
// The 153–500 span is Hue's tunable-white range (≈6500K cool → ≈2000K warm).

export const MIREK_MIN = 153; // ≈6500K — coolest white
export const MIREK_MAX = 500; // ≈2000K — warmest white

/** Approximate sRGB for a black-body temperature (Tanner Helland's fit), used to
 * render the white wheel's gradient and the preview swatch. */
export function kelvinToRgb(kelvin: number): [number, number, number] {
  const t = Math.max(1000, Math.min(40000, kelvin)) / 100;
  const clamp = (x: number) => Math.max(0, Math.min(255, Math.round(x)));
  const r = t <= 66 ? 255 : 329.698727446 * Math.pow(t - 60, -0.1332047592);
  const g =
    t <= 66
      ? 99.4708025861 * Math.log(t) - 161.1195681661
      : 288.1221695283 * Math.pow(t - 60, -0.0755148492);
  const b = t >= 66 ? 255 : t <= 19 ? 0 : 138.5177312231 * Math.log(t - 10) - 305.0447927307;
  return [clamp(r), clamp(g), clamp(b)];
}

export function mirekToRgb(mirek: number): [number, number, number] {
  return kelvinToRgb(1e6 / mirek);
}

// ── Color wheel ──────────────────────────────────────────────────────────────

export function ColorWheel({
  size = 176,
  hue,
  sat,
  onPick,
}: {
  size?: number;
  hue: number;
  sat: number;
  onPick: (hue: number, sat: number) => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const dragging = useRef(false);

  useEffect(() => {
    const canvas = canvasRef.current!;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const px = Math.round(size * dpr);
    canvas.width = px;
    canvas.height = px;
    const ctx = canvas.getContext("2d")!;
    const img = ctx.createImageData(px, px);
    const R = px / 2;
    for (let y = 0; y < px; y++) {
      for (let x = 0; x < px; x++) {
        const dx = x - R + 0.5;
        const dy = y - R + 0.5;
        const d = Math.hypot(dx, dy);
        if (d > R + 1) continue; // transparent corner
        const h = ((Math.atan2(dy, dx) * 180) / Math.PI + 360) % 360;
        const [r, g, b] = hsvToRgb(h, Math.min(1, d / R), 1);
        const i = (y * px + x) * 4;
        img.data[i] = r;
        img.data[i + 1] = g;
        img.data[i + 2] = b;
        img.data[i + 3] = Math.round(255 * Math.max(0, Math.min(1, R + 1 - d)));
      }
    }
    ctx.putImageData(img, 0, 0);
  }, [size]);

  function pick(e: React.PointerEvent) {
    const rect = canvasRef.current!.getBoundingClientRect();
    const R = rect.width / 2;
    const dx = e.clientX - rect.left - R;
    const dy = e.clientY - rect.top - R;
    const h = ((Math.atan2(dy, dx) * 180) / Math.PI + 360) % 360;
    const s = Math.min(1, Math.hypot(dx, dy) / (R - 10));
    onPick(h, s);
  }

  const R = size / 2;
  const kr = Math.min(1, sat) * (R - 10);
  const kx = R + Math.cos((hue * Math.PI) / 180) * kr;
  const ky = R + Math.sin((hue * Math.PI) / 180) * kr;
  const [cr, cg, cb] = hsvToRgb(hue, sat, 1);

  return (
    <div
      style={{ position: "relative", width: size, height: size, touchAction: "none", flexShrink: 0 }}
      onPointerDown={(e) => {
        dragging.current = true;
        e.currentTarget.setPointerCapture(e.pointerId);
        pick(e);
      }}
      onPointerMove={(e) => {
        if (dragging.current) pick(e);
      }}
      onPointerUp={() => {
        dragging.current = false;
      }}
    >
      <canvas
        ref={canvasRef}
        style={{
          width: size,
          height: size,
          display: "block",
          borderRadius: "50%",
          cursor: "crosshair",
          // Gold filigree rim + a faint inner darkening for HUD depth.
          boxShadow: `inset 0 0 22px -12px #000, 0 0 0 1px ${color.hairline}`,
        }}
      />
      {/* Selection thumb: the chosen color, ringed in white over a dark contrast
          edge (so it reads on any hue) and haloed in its own color — "lit". */}
      <div
        style={{
          position: "absolute",
          left: kx - 11,
          top: ky - 11,
          width: 22,
          height: 22,
          borderRadius: "50%",
          border: "2px solid #fff",
          boxSizing: "border-box",
          background: `rgb(${cr},${cg},${cb})`,
          boxShadow: `0 0 0 1px rgba(0,0,0,0.5), 0 0 14px rgb(${cr},${cg},${cb}), 0 1px 6px rgba(0,0,0,0.6)`,
          pointerEvents: "none",
        }}
      />
    </div>
  );
}

// ── Brightness slider (horizontal) ───────────────────────────────────────────

/** A small sun mark for the brightness control — a filled disc with eight rays. */
function SunGlyph({ size, tint }: { size: number; tint: string }) {
  const c = size / 2;
  return (
    <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} style={{ flexShrink: 0 }} aria-hidden>
      <circle cx={c} cy={c} r={size * 0.24} fill={tint} />
      {Array.from({ length: 8 }, (_, i) => {
        const a = (i * Math.PI) / 4;
        return (
          <line
            key={i}
            x1={c + Math.cos(a) * size * 0.36}
            y1={c + Math.sin(a) * size * 0.36}
            x2={c + Math.cos(a) * size * 0.5}
            y2={c + Math.sin(a) * size * 0.5}
            stroke={tint}
            strokeWidth={size * 0.08}
            strokeLinecap="round"
          />
        );
      })}
    </svg>
  );
}

/** The brightness control: a single tactile row — a sun mark, a thick rounded
 * track that fills from off → the current colour, an overhanging lit knob, and the
 * live value. Sits under the mode tabs and shows in **every** mode (Color / White /
 * Effects / Segments), since brightness is independent of which one is active.
 * Pass `label` (e.g. "Relative brightness") to caption it; the default is unlabelled
 * (the sun says it all). */
export function BrightnessSlider({
  hex,
  value,
  onPick,
  compact,
  label = "Brightness",
}: {
  hex: string;
  value: number; // 1..100
  onPick: (value: number) => void;
  compact: boolean;
  label?: string;
}) {
  const trackRef = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);

  function pick(e: React.PointerEvent) {
    const rect = trackRef.current!.getBoundingClientRect();
    const f = (e.clientX - rect.left) / rect.width;
    onPick(Math.max(1, Math.min(100, Math.round(f * 100))));
  }

  const h = compact ? 24 : 18;
  const knob = h + 8; // overhangs the track for a tactile, modern handle

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 5 }}>
      {label !== "Brightness" && (
        <span style={{ fontSize: compact ? "0.72rem" : "0.66rem", color: color.dim, letterSpacing: "0.06em", textTransform: "uppercase" }}>
          {label}
        </span>
      )}
      <div style={{ display: "flex", alignItems: "center", gap: compact ? 12 : 9 }}>
        <SunGlyph size={compact ? 19 : 15} tint={color.dim} />
        <div
          ref={trackRef}
          title={`${value}%`}
          onPointerDown={(e) => {
            dragging.current = true;
            e.currentTarget.setPointerCapture(e.pointerId);
            pick(e);
          }}
          onPointerMove={(e) => {
            if (dragging.current) pick(e);
          }}
          onPointerUp={() => {
            dragging.current = false;
          }}
          style={{
            position: "relative",
            flex: 1,
            height: h,
            borderRadius: h / 2,
            border: `1px solid ${color.hairline}`,
            background: `linear-gradient(to right, ${color.surfaceOff}, ${hex})`,
            boxShadow: "inset 0 0 14px -8px #000",
            touchAction: "none",
            cursor: "pointer",
          }}
        >
          <div
            style={{
              position: "absolute",
              top: (h - knob) / 2,
              left: `${value}%`,
              transform: "translateX(-50%)",
              width: knob,
              height: knob,
              borderRadius: "50%",
              background: "#fff",
              boxShadow: `0 0 0 1px rgba(0,0,0,0.45), 0 0 12px ${hex}, 0 2px 6px rgba(0,0,0,0.6)`,
              pointerEvents: "none",
            }}
          />
        </div>
        <span
          style={{
            minWidth: 38,
            textAlign: "right",
            fontSize: compact ? "0.9rem" : "0.8rem",
            fontWeight: 600,
            color: color.text,
            fontVariantNumeric: "tabular-nums",
          }}
        >
          {value}%
        </span>
      </div>
    </div>
  );
}

// ── White / color-temperature wheel ─────────────────────────────────────────

/** A Hue-style "white" picker: a disc filled with a warm→cool gradient where the
 * horizontal position selects the color temperature (mirek). Shares the disc
 * shape and drag feel of [`ColorWheel`] so the Color/White toggle just swaps one
 * for the other. The dot follows the finger; only its x maps to temperature. */
export function ColorTempWheel({
  size = 176,
  mirek,
  onPick,
}: {
  size?: number;
  mirek: number;
  onPick: (mirek: number) => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const dragging = useRef(false);

  useEffect(() => {
    const canvas = canvasRef.current!;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const px = Math.round(size * dpr);
    canvas.width = px;
    canvas.height = px;
    const ctx = canvas.getContext("2d")!;
    for (let x = 0; x < px; x++) {
      // Left = warmest (MIREK_MAX), right = coolest (MIREK_MIN).
      const f = x / (px - 1);
      const m = MIREK_MAX - f * (MIREK_MAX - MIREK_MIN);
      const [r, g, b] = mirekToRgb(m);
      ctx.fillStyle = `rgb(${r},${g},${b})`;
      ctx.fillRect(x, 0, 1, px);
    }
  }, [size]);

  function pick(e: React.PointerEvent) {
    const rect = canvasRef.current!.getBoundingClientRect();
    const f = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    onPick(Math.round(MIREK_MAX - f * (MIREK_MAX - MIREK_MIN)));
  }

  const f = (MIREK_MAX - mirek) / (MIREK_MAX - MIREK_MIN);
  const kx = f * size;
  const ky = size / 2;
  const [tr, tg, tb] = mirekToRgb(mirek);

  return (
    <div
      style={{ position: "relative", width: size, height: size, touchAction: "none", flexShrink: 0 }}
      onPointerDown={(e) => {
        dragging.current = true;
        e.currentTarget.setPointerCapture(e.pointerId);
        pick(e);
      }}
      onPointerMove={(e) => {
        if (dragging.current) pick(e);
      }}
      onPointerUp={() => {
        dragging.current = false;
      }}
    >
      <canvas
        ref={canvasRef}
        style={{
          width: size,
          height: size,
          display: "block",
          borderRadius: "50%",
          cursor: "crosshair",
          boxShadow: `inset 0 0 22px -12px #000, 0 0 0 1px ${color.hairline}`,
        }}
      />
      <div
        style={{
          position: "absolute",
          left: kx - 11,
          top: ky - 11,
          width: 22,
          height: 22,
          borderRadius: "50%",
          border: "2px solid #fff",
          boxSizing: "border-box",
          background: `rgb(${tr},${tg},${tb})`,
          boxShadow: `0 0 0 1px rgba(0,0,0,0.5), 0 0 14px rgb(${tr},${tg},${tb}), 0 1px 6px rgba(0,0,0,0.6)`,
          pointerEvents: "none",
        }}
      />
    </div>
  );
}

// ── Mode toggle (Color / White / Effects) ────────────────────────────────────

export type EditorMode = "color" | "white" | "effects" | "segments";

/** "no_effect"/"off" are the canonical clear tokens; everything else is a
 * provider-native effect name (`color_loop`, `candle`, `Sunrise`, …) — humanize
 * underscores/hyphens to Title Case for display. */
export function effectLabel(e: string): string {
  if (e === "no_effect" || e === "off" || e === "none" || e === "None") return "Off";
  return e
    .replace(/[_-]+/g, " ")
    .replace(/\b\w/g, (c) => c.toUpperCase())
    .trim();
}

/** Segmented pill that switches the editor between the color wheel, the white
 * (color-temperature) wheel, and the effects grid. Only the modes the light
 * supports are offered. Touch-sized on compact viewports. */
function ModeToggle({
  mode,
  modes,
  onChange,
  compact,
}: {
  mode: EditorMode;
  modes: EditorMode[];
  onChange: (m: EditorMode) => void;
  compact: boolean;
}) {
  // Expressive tabs: Color previews the spectrum, White the warm→cool range,
  // Effects a shimmer. Those gradients are intentionally literal (not theme accents).
  const ALL: Record<EditorMode, { label: string; activeBg: string }> = {
    color: { label: "Color", activeBg: "linear-gradient(90deg, #ff7d8a, #8b5cf6 55%, #38bdf8)" },
    white: { label: "White", activeBg: "linear-gradient(90deg, #ffd9a0, #cfe4ff)" },
    effects: { label: "Effects", activeBg: "linear-gradient(90deg, #38bdf8, #a78bfa 60%, #f0abfc)" },
    // A smooth spectrum — the addressable strip, without the noisy hard stripes.
    segments: { label: "Segments", activeBg: "linear-gradient(90deg, #ff6b6b, #ffd23d, #4ade80, #38bdf8, #a78bfa)" },
  };
  return (
    <Segmented
      value={mode}
      onChange={(m) => onChange(m as EditorMode)}
      compact={compact}
      options={modes.map((m) => ({ value: m, label: ALL[m].label, activeBg: ALL[m].activeBg }))}
    />
  );
}

// ── Effects grid ─────────────────────────────────────────────────────────────

export type EffectCategory = { name: string; effects: string[] };

// Theme buckets, matched by keyword on the effect name (first rule wins). These
// are provider-agnostic — any provider that ships a big built-in catalog (Govee
// today; future ones) sorts into the same tabs. Anything unmatched → "Other".
const CATEGORY_RULES: { name: string; re: RegExp }[] = [
  { name: "Calm", re: /sleep|medit|calm|quiet|sooth|relax|care|read|study|business|work|leisure|dream|night|morning|afternoon|warm|cozy|sweet|gentle|soft|retro|profound|longing|awaken/i },
  { name: "Nature", re: /forest|leaf|leaves|sea|ocean|river|lake|sky|aurora|rainbow|sunrise|sunset|dusk|sun|star|universe|galaxy|cosmos|moon|glacier|canyon|desert|gobi|karst|cave|corn|flower|wave|ripple|grass|spring|summer|fall|autumn|winter|snow|cloud|lightning|meteor|water|rain|downpour|sail|underwater/i },
  { name: "Fire", re: /fire|flame|candle|ember|lava|volcano/i },
  { name: "Holiday", re: /christmas|halloween|valentine|easter|thanksgiving|new\s?year|birthday|mother|father|ghost|firework|carnival|sled|gift|holiday|spooky/i },
  { name: "Party", re: /party|disco|danc|club|rave|tango|cha-?cha|waltz|ballet|jazz|poppin|electro|breaking|latin|pole|street|festiv|celebrat/i },
  { name: "Play", re: /game|racing|shoot|fight|adventure|puzzle|cards|cosplay|siren|crossing|movie|comed|drama|thriller|romance|documentar|sci-?fi|science fiction|suspense|war|action|sport/i },
  { name: "Vivid", re: /energetic|excite|happy|active|crazy|heartbeat|tension|rush|fascinat|cheerful|breath|pulse|twinkle|spark|glisten|prism|opal|flash|swing|flow|release|stack|enthusi|color|loop|morph|move|chase|enchant|sunbeam|disco/i },
];

/** Group provider effect names into theme categories (reusable across providers).
 * Returns only non-empty buckets, in rule order with "Other" last. */
export function categorizeEffects(effects: string[]): EffectCategory[] {
  const buckets = new Map<string, string[]>();
  for (const e of effects) {
    const rule = CATEGORY_RULES.find((r) => r.re.test(e));
    const name = rule ? rule.name : "Other";
    (buckets.get(name) ?? buckets.set(name, []).get(name)!).push(e);
  }
  const order = [...CATEGORY_RULES.map((r) => r.name), "Other"];
  return order
    .filter((n) => buckets.has(n))
    .map((name) => ({ name, effects: buckets.get(name)! }));
}

// Above this many effects, the panel adds a search box + category tabs so a big
// catalog (Govee strips ship ~200+) stays navigable. Smaller lists (Hue/LIFX)
// render the plain grid.
const SEARCHABLE_THRESHOLD = 14;

/** The Effects tab body. A small catalog is a plain responsive grid of tiles; a
 * large one (Govee scenes) gains a search field + theme category tabs that filter
 * the grid. The active effect is cyan-lit with an animated shimmer bar. */
function EffectsPanel({
  effects,
  active,
  onPick,
  compact,
}: {
  effects: string[];
  active?: string;
  onPick: (effect: string) => void;
  compact: boolean;
}) {
  const advanced = effects.length > SEARCHABLE_THRESHOLD;
  const categories = useMemo(() => (advanced ? categorizeEffects(effects) : []), [effects, advanced]);
  const [cat, setCat] = useState("All");
  const [query, setQuery] = useState("");

  const q = query.trim().toLowerCase();
  const visible = useMemo(() => {
    let list = effects;
    if (cat !== "All") list = categories.find((c) => c.name === cat)?.effects ?? effects;
    if (q) list = list.filter((e) => (effectLabel(e) + " " + e).toLowerCase().includes(q));
    return list;
  }, [effects, categories, cat, q]);

  const tabs = useMemo(
    () => [{ name: "All", effects } as EffectCategory, ...categories],
    [effects, categories],
  );

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: compact ? "0.6rem" : "0.5rem" }}>
      {advanced && (
        <>
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={`Search ${effects.length} effects…`}
            style={{
              width: "100%",
              boxSizing: "border-box",
              padding: compact ? "0.6rem 0.8rem" : "0.4rem 0.6rem",
              fontSize: compact ? "0.95rem" : "0.82rem",
              color: color.text,
              background: alpha(color.text, 0.05),
              border: `1px solid ${color.hairline}`,
              borderRadius: 10,
              outline: "none",
            }}
          />
          {/* Category tabs — a wrapping chip row so every category stays visible
              (no horizontally-clipped tab at the panel edge). */}
          <div
            style={{
              display: "flex",
              flexWrap: "wrap",
              gap: "0.35rem",
            }}
          >
            {tabs.map((t) => {
              const on = cat === t.name;
              return (
                <button
                  key={t.name}
                  onClick={() => setCat(t.name)}
                  style={{
                    flexShrink: 0,
                    padding: compact ? "0.35rem 0.7rem" : "0.25rem 0.55rem",
                    borderRadius: 999,
                    fontSize: compact ? "0.78rem" : "0.72rem",
                    fontWeight: on ? 600 : 500,
                    whiteSpace: "nowrap",
                    cursor: "pointer",
                    color: on ? color.ink : color.dim,
                    background: on ? color.cyan : alpha(color.text, 0.05),
                    border: `1px solid ${on ? color.cyan : color.hairline}`,
                    boxShadow: on ? glow(color.cyan, 10) : "none",
                    transition: "background .15s ease, color .15s ease",
                  }}
                >
                  {t.name}
                  <span style={{ opacity: 0.6, marginLeft: 4 }}>{t.effects.length}</span>
                </button>
              );
            })}
          </div>
        </>
      )}

      <div
        style={{
          display: "grid",
          gridTemplateColumns: compact ? "1fr 1fr" : "repeat(auto-fill, minmax(118px, 1fr))",
          gap: compact ? "0.55rem" : "0.45rem",
          maxHeight: advanced ? (compact ? 360 : 300) : compact ? "none" : 280,
          overflowY: "auto",
        }}
      >
        {visible.map((e) => {
          const isActive = active === e;
          const isOff = effectLabel(e) === "Off";
          return (
            <button
              key={e}
              onClick={() => onPick(e)}
              style={{
                position: "relative",
                overflow: "hidden",
                display: "flex",
                flexDirection: "column",
                alignItems: "flex-start",
                gap: "0.35rem",
                minHeight: compact ? 60 : 52,
                padding: compact ? "0.6rem 0.7rem" : "0.5rem 0.6rem",
                borderRadius: 12,
                cursor: "pointer",
                textAlign: "left",
                color: isActive ? color.text : color.dim,
                background: isActive
                  ? `linear-gradient(150deg, ${alpha(color.cyan, 0.22)}, ${alpha(color.cyan, 0.05)})`
                  : `linear-gradient(${alpha(color.text, 0.05)}, ${alpha(color.text, 0.015)})`,
                border: `1px solid ${isActive ? alpha(color.cyan, 0.7) : color.hairline}`,
                boxShadow: isActive ? glow(color.cyan, 16) : "none",
                transition: "background .15s ease, color .15s ease, box-shadow .2s ease, border-color .15s ease",
              }}
            >
              <span
                aria-hidden
                style={{
                  fontSize: "0.95rem",
                  lineHeight: 1,
                  filter: isActive ? "none" : "grayscale(1) opacity(0.55)",
                }}
              >
                {effectGlyph(e, isOff)}
              </span>
              <span
                style={{
                  fontSize: compact ? "0.82rem" : "0.78rem",
                  fontWeight: isActive ? 600 : 500,
                  letterSpacing: "0.01em",
                  whiteSpace: "nowrap",
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  width: "100%",
                }}
              >
                {effectLabel(e)}
              </span>
              {isActive && !isOff && (
                <span
                  aria-hidden
                  style={{
                    position: "absolute",
                    left: 0,
                    bottom: 0,
                    height: 2,
                    width: "100%",
                    background: `linear-gradient(90deg, transparent, ${color.cyan}, transparent)`,
                    animation: "bifrostEffectShimmer 1.8s ease-in-out infinite",
                  }}
                />
              )}
            </button>
          );
        })}
        {visible.length === 0 && (
          <div style={{ gridColumn: "1 / -1", color: color.faint, fontSize: "0.8rem", padding: "0.6rem 0.2rem" }}>
            No effects match “{query}”.
          </div>
        )}
      </div>
    </div>
  );
}

/** A small emoji cue for an effect tile, matched by keyword so provider-native
 * names (Hue `candle`, LIFX `flame`, Govee `Sunrise`, …) get a fitting icon. */
function effectGlyph(effect: string, isOff: boolean): string {
  if (isOff) return "○";
  const e = effect.toLowerCase();
  if (/fire|flame|candle|ember/.test(e)) return "🔥";
  if (/spark|glint|glisten|prism|opal|twinkl/.test(e)) return "✨";
  if (/breath|pulse|beat/.test(e)) return "💓";
  if (/sun|dawn|sunrise|morning/.test(e)) return "🌅";
  if (/sunset|dusk|night/.test(e)) return "🌆";
  if (/move|march|chase|scan|run/.test(e)) return "🌊";
  if (/morph|loop|cycle|rainbow|color/.test(e)) return "🌈";
  if (/cloud|sky|aurora/.test(e)) return "☁️";
  if (/party|disco|festiv|rave/.test(e)) return "🎉";
  return "🌟";
}

// ── Segment editor (addressable LED strips) ──────────────────────────────────

/** A per-segment write: a colour (`rgb`, 24-bit) and/or a brightness (0–100). */
export type SegmentColorChange = { segment: number; rgb?: number; brightness?: number };

/** Pack a `#rrggbb` hex into the 24-bit `0xRRGGBB` int Govee's segment API wants. */
export function hexToRgbInt(hex: string): number {
  const [r, g, b] = hexToRgb(hex);
  return (r << 16) | (g << 8) | b;
}

/** Render a segment colour at a given brightness (0–100) by scaling toward black,
 * with a floor so a dim segment still reads as lit rather than off. */
function dimColor(hex: string, pct: number): string {
  const [r, g, b] = hexToRgb(hex);
  const f = 0.22 + 0.78 * (Math.max(0, Math.min(100, pct)) / 100);
  return `rgb(${Math.round(r * f)}, ${Math.round(g * f)}, ${Math.round(b * f)})`;
}

// Govee-style fixed palette + the wheel; the last entry is a "clear" sentinel.
const SEG_SWATCHES = ["#ff2d2d", "#ff8a2b", "#ffe23d", "#42dd56", "#2238ff", "#3ee8ff", "#9b2bff", "#ffffff"];
// How many cells per row in the serpentine strip layout (mirrors the Govee app).
const SEGMENT_COLS = 5;

/** Per-segment editor for addressable strips (Govee), modelled on the Govee app:
 * the strip is drawn as a **serpentine** grid of segment cells, each with a
 * selection check and its own brightness %. Tap cells (or "Select all") to choose
 * a set, then a colour (swatch/wheel) or the relative-brightness slider applies to
 * the selection. Segment state isn't reported back by the provider, so cells track
 * only what's been set this session (display-only); writes are debounced. */
function SegmentEditor({
  count,
  onApply,
  compact,
}: {
  count: number;
  onApply: (segments: SegmentColorChange[]) => void;
  compact: boolean;
}) {
  // `null` = an unset segment (shows the neutral ribbon, like the Govee app's
  // uncoloured strip); a hex string = a colour the user applied this session.
  const [colors, setColors] = useState<(string | null)[]>(() => Array(count).fill(null));
  const [brightness, setBrightness] = useState<number[]>(() => Array(count).fill(100));
  const [selected, setSelected] = useState<Set<number>>(() => new Set());
  const [[hue, sat], setHs] = useState<[number, number]>(() => hexToHs("#ff5e9c"));
  const [relBrightness, setRelBrightness] = useState(100);
  const allSelected = selected.size === count && count > 0;

  // Debounce network writes so a wheel/slider drag doesn't spam the strip; a
  // discrete pick (swatch/clear) sends immediately.
  const sendTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  function send(segs: SegmentColorChange[], immediate = false) {
    if (segs.length === 0) return;
    clearTimeout(sendTimer.current);
    if (immediate) onApply(segs);
    else sendTimer.current = setTimeout(() => onApply(segs), 150);
  }

  function toggle(i: number) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(i)) next.delete(i);
      else next.add(i);
      const first = [...next].sort((a, b) => a - b)[0];
      if (first !== undefined) setRelBrightness(brightness[first]);
      return next;
    });
  }

  function selectAllOrNone() {
    setSelected(allSelected ? new Set() : new Set(Array.from({ length: count }, (_, i) => i)));
  }

  function applyColor(hex: string, immediate = false) {
    if (selected.size === 0) return;
    const idxs = [...selected];
    setColors((prev) => {
      const next = [...prev];
      for (const i of idxs) next[i] = hex;
      return next;
    });
    send(idxs.map((i) => ({ segment: i, rgb: hexToRgbInt(hex) })), immediate);
  }

  function clearSelected() {
    if (selected.size === 0) return;
    const idxs = [...selected];
    setColors((prev) => {
      const next = [...prev];
      for (const i of idxs) next[i] = null; // back to the neutral ribbon
      return next;
    });
    send(idxs.map((i) => ({ segment: i, rgb: 0 })), true);
  }

  function applyBrightness(b: number) {
    setRelBrightness(b);
    if (selected.size === 0) return;
    const idxs = [...selected];
    setBrightness((prev) => {
      const next = [...prev];
      for (const i of idxs) next[i] = b;
      return next;
    });
    send(idxs.map((i) => ({ segment: i, brightness: b })));
  }

  // ── LED-strip geometry (SVG), modelled on the Govee app ───────────────────
  // A clean, flat winding ribbon (one continuous tangent path — straight runs +
  // semicircular U-bends), divided into thin rectangular segments by dashed
  // dividers, with a select checkbox above each segment and its brightness %
  // below. No glow. A set segment tints its slice of the ribbon. Fixed viewBox
  // scaled to the container, so it sizes up cleanly on a mobile sheet.
  const cols = Math.max(1, Math.min(count, SEGMENT_COLS));
  const numRows = Math.ceil(count / cols);
  const RIB = 20; // ribbon thickness (thin → rectangular segments)
  const SEGH = RIB - 6; // tinted-segment height (inset, so the ribbon frames it)
  const PCT = 11; // brightness-% label height (sits *above* the ribbon)
  const ABOVE = 4 + PCT; // % band above each ribbon row
  const BOTPAD = 8; // room below the last row so a selection glow can breathe
  const PITCH = RIB + ABOVE + 14; // row-to-row spacing
  const TURN = PITCH / 2; // U-bend radius (chord = PITCH = diameter → semicircle)
  const VW = 300;
  const MX = TURN + RIB / 2 + 3;
  const innerW = VW - 2 * MX;
  const step = innerW / cols;
  const segW = step - 4; // a segment's tinted width
  const rowY = (r: number) => 6 + ABOVE + RIB / 2 + r * PITCH;
  const colX = (k: number) => MX + (k + 0.5) * step;
  const ledX = (i: number) => {
    const r = Math.floor(i / cols);
    const k = r % 2 === 1 ? cols - 1 - (i % cols) : i % cols; // serpentine
    return colX(k);
  };
  const rowOf = (i: number) => Math.floor(i / cols);
  const VH = rowY(numRows - 1) + RIB / 2 + BOTPAD;

  // One continuous ribbon centreline. The last row stops just past its final
  // segment so a short row leaves no dangling tail.
  let ribbonPath = `M ${MX} ${rowY(0)}`;
  for (let r = 0; r < numRows; r++) {
    const right = r % 2 === 0;
    let farX: number;
    if (r === numRows - 1) {
      const m = count - r * cols;
      farX = right ? colX(m - 1) + segW / 2 + 4 : colX(cols - m) - segW / 2 - 4;
    } else {
      farX = right ? MX + innerW : MX;
    }
    ribbonPath += ` L ${farX.toFixed(1)} ${rowY(r)}`;
    if (r < numRows - 1) {
      const turnX = right ? MX + innerW : MX;
      ribbonPath += ` A ${TURN} ${TURN} 0 0 ${right ? 1 : 0} ${turnX} ${rowY(r + 1)}`;
    }
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: compact ? "0.7rem" : "0.55rem" }}>
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          fontSize: compact ? "0.82rem" : "0.74rem",
          color: color.dim,
        }}
      >
        <span>{selected.size > 0 ? `${selected.size} of ${count} selected` : "Segments"}</span>
        <button
          onClick={selectAllOrNone}
          style={{ background: "none", border: "none", cursor: "pointer", color: color.cyan, fontSize: "inherit", padding: 0 }}
        >
          {allSelected ? "Clear selection" : "Select all"}
        </button>
      </div>

      {/* The strip: a clean winding ribbon split into segments, each with a select
          checkbox above and its brightness % below. */}
      <div
        style={{
          padding: compact ? "0.5rem 0.4rem" : "0.4rem 0.3rem",
          borderRadius: 14,
          background: "#0c0c12",
          border: `1px solid ${color.hairline}`,
        }}
      >
        <svg viewBox={`0 0 ${VW} ${VH}`} width="100%" style={{ display: "block", height: "auto" }}>
          <defs>
            {/* Soft cyan halo for a selected segment. */}
            <filter id="bf-seg-glow" x="-80%" y="-80%" width="260%" height="260%">
              <feGaussianBlur stdDeviation="2.6" />
            </filter>
          </defs>

          {/* The ribbon: a flat slate band (darker edge + lighter face), one path. */}
          <path d={ribbonPath} fill="none" stroke="#2f3344" strokeWidth={RIB} strokeLinecap="round" strokeLinejoin="round" />
          <path d={ribbonPath} fill="none" stroke="#3c4154" strokeWidth={RIB - 5} strokeLinecap="round" strokeLinejoin="round" />

          {/* Tinted slices for segments the user has coloured. */}
          {Array.from({ length: count }, (_, i) =>
            colors[i] ? (
              <rect
                key={`seg${i}`}
                x={ledX(i) - segW / 2}
                y={rowY(rowOf(i)) - SEGH / 2}
                width={segW}
                height={SEGH}
                rx={3}
                fill={dimColor(colors[i]!, brightness[i])}
              />
            ) : null,
          )}

          {/* Dashed dividers between adjacent same-row segments. */}
          {Array.from({ length: Math.max(0, count - 1) }, (_, i) =>
            rowOf(i) === rowOf(i + 1) ? (
              <line
                key={`div${i}`}
                x1={(ledX(i) + ledX(i + 1)) / 2}
                x2={(ledX(i) + ledX(i + 1)) / 2}
                y1={rowY(rowOf(i)) - RIB / 2 + 2}
                y2={rowY(rowOf(i)) + RIB / 2 - 2}
                stroke="rgba(255,255,255,0.22)"
                strokeWidth={1}
                strokeDasharray="2 2.5"
              />
            ) : null,
          )}

          {/* Selection: a cyan glow + crisp outline around the segment itself. */}
          {Array.from({ length: count }, (_, i) => {
            if (!selected.has(i)) return null;
            const x = ledX(i);
            const ry = rowY(rowOf(i));
            const sx = x - segW / 2 - 1.5;
            const sy = ry - RIB / 2 - 1.5;
            const sw = segW + 3;
            const sh = RIB + 3;
            return (
              <g key={`sel${i}`} style={{ pointerEvents: "none" }}>
                <rect x={sx} y={sy} width={sw} height={sh} rx={5} fill="none" stroke={color.cyan} strokeWidth={4} opacity={0.7} filter="url(#bf-seg-glow)" />
                <rect x={sx} y={sy} width={sw} height={sh} rx={5} fill="none" stroke={color.cyan} strokeWidth={1.6} />
              </g>
            );
          })}

          {/* Per segment: brightness % above, plus a full-column hit target. */}
          {Array.from({ length: count }, (_, i) => {
            const x = ledX(i);
            const ry = rowY(rowOf(i));
            const on = selected.has(i);
            const pctY = ry - RIB / 2 - 5; // baseline just above the ribbon
            return (
              <g key={`seg-ctl${i}`} onClick={() => toggle(i)} style={{ cursor: "pointer" }}>
                <title>{`Segment ${i + 1} · ${brightness[i]}%`}</title>
                <text
                  x={x}
                  y={pctY}
                  textAnchor="middle"
                  style={{
                    fontSize: 10,
                    fontWeight: on ? 600 : 400,
                    fill: on ? color.cyan : color.dim,
                    fontVariantNumeric: "tabular-nums",
                  }}
                >
                  {brightness[i]}%
                </text>
                <rect
                  x={x - step / 2}
                  y={ry - RIB / 2 - ABOVE}
                  width={step}
                  height={ABOVE + RIB + 4}
                  fill="transparent"
                />
              </g>
            );
          })}
        </svg>
      </div>

      {/* Relative brightness for the selection (Govee's "Relative brightness"). */}
      <BrightnessSlider
        hex="#ffd9a0"
        value={relBrightness}
        onPick={applyBrightness}
        compact={compact}
        label="Relative brightness"
      />

      {/* Fixed palette + a clear ("no colour") swatch. */}
      <div style={{ display: "flex", gap: compact ? "0.55rem" : "0.4rem", justifyContent: "center", flexWrap: "wrap" }}>
        {SEG_SWATCHES.map((c) => (
          <button
            key={c}
            onClick={() => applyColor(c, true)}
            title={c}
            style={{
              width: compact ? 38 : 26,
              height: compact ? 38 : 26,
              borderRadius: 8,
              border: `1px solid ${alpha(color.text, 0.22)}`,
              background: c,
              cursor: "pointer",
              padding: 0,
            }}
          />
        ))}
        <button
          onClick={clearSelected}
          title="Clear colour"
          style={{
            width: compact ? 38 : 26,
            height: compact ? 38 : 26,
            borderRadius: 8,
            border: `1px solid ${alpha(color.text, 0.22)}`,
            background: `linear-gradient(135deg, transparent 47%, #e0506a 47%, #e0506a 53%, transparent 53%), ${color.surfaceOff}`,
            cursor: "pointer",
            padding: 0,
          }}
        />
      </div>

      {/* Fine colour control. */}
      <div style={{ display: "flex", justifyContent: "center" }}>
        <ColorWheel
          size={compact ? 220 : 168}
          hue={hue}
          sat={sat}
          onPick={(h, s) => {
            setHs([h, s]);
            applyColor(rgbToHex(...hsvToRgb(h, s, 1)));
          }}
        />
      </div>

      {selected.size === 0 && (
        <div style={{ textAlign: "center", fontSize: compact ? "0.78rem" : "0.72rem", color: color.faint }}>
          Tap segments (or “Select all”) to colour them.
        </div>
      )}
    </div>
  );
}

// ── Anchored editor popover ──────────────────────────────────────────────────

export function LightEditor({
  anchor,
  title,
  initialHex,
  initialBrightness = 100,
  initialMirek = 366, // ≈2700K, a warm-white default
  showColor = true,
  showBrightness = true,
  showWhite = false,
  initialMode,
  effects,
  initialEffect,
  segments,
  onSegments,
  on,
  onToggle,
  onChange,
  onClose,
  children,
}: {
  /** Element or screen point the popover anchors next to. */
  anchor: HTMLElement | { x: number; y: number };
  title?: string;
  initialHex: string;
  initialBrightness?: number;
  /** Current color temperature in mirek, when the light supports white. */
  initialMirek?: number;
  showColor?: boolean;
  showBrightness?: boolean;
  /** Show the white / color-temperature wheel (light has tunable white). */
  showWhite?: boolean;
  /** Which wheel to open on. Defaults to the light's current mode (white if it's
   * showing a temperature and no color), else color. */
  initialMode?: EditorMode;
  /** Dynamic effects the light supports (provider names; "no_effect" = clear).
   * When non-empty, an Effects tab renders. */
  effects?: string[];
  /** The currently-active effect, if any. */
  initialEffect?: string;
  /** Number of addressable colour segments (Govee strips). With `onSegments`, a
   * Segments tab renders. */
  segments?: number;
  /** Apply per-segment colour/brightness (write-only). Required for the Segments tab. */
  onSegments?: (segments: SegmentColorChange[]) => void;
  /** When provided, the editor shows a power switch row. */
  on?: boolean;
  onToggle?: () => void;
  /** Fires live while dragging — callers debounce network sends. The change's
   * `field` says which control moved, so a room cascade can adjust *only* that
   * dimension and leave each member light's other attributes intact. */
  onChange: (change: LightControlChange) => void;
  onClose: () => void;
  /** Extra controls rendered at the bottom (e.g. a room's scene selector). */
  children?: ReactNode;
}) {
  const { isCompact } = useViewport();
  // Uncontrolled after mount: seeding from props once avoids hex→hsv→hex
  // roundtrip jitter while dragging.
  const [[hue, sat], setHs] = useState<[number, number]>(() => hexToHs(initialHex));
  const [brightness, setBrightness] = useState(initialBrightness);
  const [mirek, setMirek] = useState(initialMirek);
  // The editor has up to three mutually-exclusive modes (Color wheel / White
  // wheel / Effects grid); offer a tab only for the ones this light supports.
  const hasEffects = !!effects && effects.length > 0;
  const hasSegments = !!segments && segments > 0 && !!onSegments;
  const availableModes: EditorMode[] = [
    ...(showColor ? (["color"] as const) : []),
    ...(showWhite ? (["white"] as const) : []),
    ...(hasEffects ? (["effects"] as const) : []),
    ...(hasSegments ? (["segments"] as const) : []),
  ];
  const [mode, setMode] = useState<EditorMode>(
    () => initialMode ?? (showColor ? "color" : showWhite ? "white" : "effects"),
  );
  const hex = rgbToHex(...hsvToRgb(hue, sat, 1));

  function applyColor(h: number, s: number) {
    setHs([h, s]);
    onChange({ field: "color", hex: rgbToHex(...hsvToRgb(h, s, 1)) });
  }

  function applyBrightness(b: number) {
    setBrightness(b);
    onChange({ field: "brightness", brightness: b });
  }

  function applyTemp(m: number) {
    setMirek(m);
    onChange({ field: "temp", mirek: m });
  }

  const [effect, setEffect] = useState(initialEffect);
  function applyEffect(e: string) {
    setEffect(e);
    onChange({ field: "effect", effect: e });
  }

  const whiteHex = rgbToHex(...mirekToRgb(mirek));
  // Exactly one mode renders at a time; `mode` is always one of `availableModes`.
  const colorActive = mode === "color";
  const whiteActive = mode === "white";
  const effectsActive = mode === "effects";
  const segmentsActive = mode === "segments";

  // The light "casts" its current colour onto the whole fly-out, under every tab,
  // dimmer as brightness drops. White mode casts its warm/cool white; effects and
  // segments inherit the base colour.
  const castColor = whiteActive ? whiteHex : hex;
  const castStrength = showBrightness ? 0.06 + 0.3 * (brightness / 100) : 0.26;
  // Size the desktop popover to the tabs (3–4 modes don't fit beside a 176px
  // wheel, which left them squished); the compact sheet is always full-width.
  const flyoutWidth = availableModes.length >= 3 ? Math.max(268, availableModes.length * 78) : undefined;

  return (
    <Flyout anchor={anchor} onClose={onClose} width={flyoutWidth} ambientColor={castColor} ambientStrength={castStrength}>
      {/* Lights are the cyan domain — the shared header carries that accent.
          Power is the glyph button at the far left, lit cyan when on. */}
      <FlyoutHeader
        title={title ?? "Color"}
        accent={color.cyan}
        leading={onToggle ? <PowerToggle on={!!on} accent={color.cyan} onToggle={onToggle} /> : undefined}
        onClose={onClose}
      />

      {availableModes.length > 1 && (
        <ModeToggle mode={mode} modes={availableModes} onChange={setMode} compact={isCompact} />
      )}

      {/* Brightness lives next to the tab selector and persists across every mode
          (Color / White / Effects), since it's an independent dimension. */}
      {showBrightness && (
        <BrightnessSlider
          hex={whiteActive ? whiteHex : colorActive ? hex : "#ffd9a0"}
          value={brightness}
          onPick={applyBrightness}
          compact={isCompact}
        />
      )}

      {effectsActive ? (
        <EffectsPanel effects={effects ?? []} active={effect} onPick={applyEffect} compact={isCompact} />
      ) : segmentsActive ? (
        <SegmentEditor count={segments ?? 0} onApply={onSegments!} compact={isCompact} />
      ) : (
        <>
          {(() => {
            const wheelSize = isCompact ? 240 : 176;
            const bf = brightness / 100;
            return (
              <div style={{ display: "flex", alignItems: "center", justifyContent: "center", padding: isCompact ? "0.6rem 0" : "0.4rem 0" }}>
                <div style={{ position: "relative", lineHeight: 0 }}>
                  {colorActive && <ColorWheel size={wheelSize} hue={hue} sat={sat} onPick={applyColor} />}
                  {whiteActive && <ColorTempWheel size={wheelSize} mirek={mirek} onPick={applyTemp} />}
                  {/* Gentle WYSIWYG dim so a low brightness reads on the wheel too
                      — kept light so the hues stay easy to pick. */}
                  <div
                    aria-hidden
                    style={{
                      position: "absolute",
                      inset: 0,
                      borderRadius: "50%",
                      background: "#000",
                      opacity: (1 - bf) * 0.22,
                      pointerEvents: "none",
                    }}
                  />
                </div>
              </div>
            );
          })()}

          {colorActive && (
            <div style={{ display: "flex", gap: isCompact ? "0.6rem" : "0.5rem", justifyContent: "center", flexWrap: "wrap" }}>
              {SWATCHES.map((c) => {
                const active = c === hex;
                return (
                  <button
                    key={c}
                    onClick={() => {
                      const [h, s] = hexToHs(c);
                      applyColor(h, s);
                    }}
                    title={c}
                    style={{
                      width: isCompact ? 40 : 26,
                      height: isCompact ? 40 : 26,
                      borderRadius: isCompact ? 12 : 8,
                      border: active ? `2px solid ${color.gold}` : `1px solid ${alpha(color.text, 0.22)}`,
                      background: c,
                      cursor: "pointer",
                      padding: 0,
                      boxShadow: active ? glow(color.gold, 12) : "inset 0 0 8px -5px #000",
                      transform: active ? "scale(1.12)" : "scale(1)",
                      transition: "transform .12s ease, box-shadow .15s ease",
                    }}
                  />
                );
              })}
            </div>
          )}
        </>
      )}

      {children}
    </Flyout>
  );
}
