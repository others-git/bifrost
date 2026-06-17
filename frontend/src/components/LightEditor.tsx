// The shared light editor: a popover anchored next to whatever triggered it,
// with a Hue-style hue/saturation color wheel and a vertical brightness bar.
// Used for lights, rooms, scene palettes, and the planner paint brush.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { rgbToHex } from "../api";
import { useViewport } from "../useViewport";
import { color, glow, alpha } from "../theme";
import { Segmented, Switch } from "./controls";
import { Flyout, FlyoutHeader, FlyoutSection } from "./Flyout";

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

// ── Vertical brightness bar ──────────────────────────────────────────────────

export function BrightnessBar({
  height = 176,
  width = 30,
  hex,
  value,
  onPick,
}: {
  height?: number;
  width?: number;
  hex: string;
  value: number; // 1..100
  onPick: (value: number) => void;
}) {
  const trackRef = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);

  function pick(e: React.PointerEvent) {
    const rect = trackRef.current!.getBoundingClientRect();
    const f = 1 - (e.clientY - rect.top) / rect.height;
    onPick(Math.max(1, Math.min(100, Math.round(f * 100))));
  }

  const knob = width - 6;
  const knobTop = 3 + (1 - value / 100) * (height - knob - 6);

  return (
    <div
      ref={trackRef}
      title={`${value}%`}
      style={{
        position: "relative",
        width,
        height,
        borderRadius: width / 2,
        border: `1px solid ${color.hairline}`,
        background: `linear-gradient(to bottom, ${hex}, ${color.surfaceOff})`,
        boxShadow: "inset 0 0 14px -8px #000",
        touchAction: "none",
        cursor: "pointer",
        flexShrink: 0,
      }}
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
      <div
        style={{
          position: "absolute",
          left: 3,
          top: knobTop,
          width: knob,
          height: knob,
          borderRadius: "50%",
          background: "#fff",
          // Haloed in the current color, ringed for contrast — matches the wheel thumb.
          boxShadow: `0 0 0 1px rgba(0,0,0,0.4), 0 0 10px ${hex}, 0 1px 5px rgba(0,0,0,0.6)`,
          pointerEvents: "none",
        }}
      />
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

export type EditorMode = "color" | "white" | "effects";

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

/** The Effects tab body: a responsive grid of effect tiles. The active effect is
 * cyan-lit with an animated shimmer bar; the rest sit on hairline glass. Replaces
 * the old afterthought chip row — effects are now a first-class picker. */
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
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: compact ? "1fr 1fr" : "repeat(auto-fill, minmax(118px, 1fr))",
        gap: compact ? "0.55rem" : "0.45rem",
        maxHeight: compact ? "none" : 280,
        overflowY: "auto",
      }}
    >
      {effects.map((e) => {
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
  const availableModes: EditorMode[] = [
    ...(showColor ? (["color"] as const) : []),
    ...(showWhite ? (["white"] as const) : []),
    ...(hasEffects ? (["effects"] as const) : []),
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

  return (
    <Flyout anchor={anchor} onClose={onClose}>
      {/* Lights are the cyan domain — the shared header carries that accent. */}
      <FlyoutHeader title={title ?? "Color"} accent={color.cyan} onClose={onClose} />

      {availableModes.length > 1 && (
        <ModeToggle mode={mode} modes={availableModes} onChange={setMode} compact={isCompact} />
      )}

      {effectsActive ? (
        <EffectsPanel effects={effects ?? []} active={effect} onPick={applyEffect} compact={isCompact} />
      ) : (
        <>
          <div style={{ display: "flex", gap: isCompact ? "1.4rem" : "0.8rem", alignItems: "center", justifyContent: "center" }}>
            {colorActive && <ColorWheel size={isCompact ? 240 : 176} hue={hue} sat={sat} onPick={applyColor} />}
            {whiteActive && <ColorTempWheel size={isCompact ? 240 : 176} mirek={mirek} onPick={applyTemp} />}
            {showBrightness && (
              <BrightnessBar
                height={isCompact ? 240 : 176}
                width={isCompact ? 44 : 30}
                hex={whiteActive ? whiteHex : colorActive ? hex : "#ffd9a0"}
                value={brightness}
                onPick={applyBrightness}
              />
            )}
          </div>

          {colorActive && (
            <div style={{ display: "flex", gap: isCompact ? "0.6rem" : "0.45rem", justifyContent: "center", flexWrap: "wrap" }}>
              {SWATCHES.map((c) => (
                <button
                  key={c}
                  onClick={() => {
                    const [h, s] = hexToHs(c);
                    applyColor(h, s);
                  }}
                  title={c}
                  style={{
                    width: isCompact ? 40 : 18,
                    height: isCompact ? 40 : 18,
                    borderRadius: "50%",
                    border: c === hex ? `2px solid ${color.gold}` : `1px solid ${alpha(color.text, 0.22)}`,
                    background: c,
                    cursor: "pointer",
                    padding: 0,
                    boxShadow: c === hex ? glow(color.gold, 12) : "none",
                    transform: c === hex ? "scale(1.18)" : "scale(1)",
                    transition: "transform .12s ease, box-shadow .15s ease",
                  }}
                />
              ))}
            </div>
          )}
        </>
      )}

      {onToggle && (
        <FlyoutSection label="Power" inline>
          <Switch on={!!on} onChange={() => onToggle()} />
        </FlyoutSection>
      )}

      {children}
    </Flyout>
  );
}
