// The shared light editor: a popover anchored next to whatever triggered it,
// with a Hue-style hue/saturation color wheel and a vertical brightness bar.
// Used for lights, rooms, scene palettes, and the planner paint brush.

import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { rgbToHex } from "../api";
import { useViewport } from "../useViewport";
import { sheetStyle } from "./sheet";
import { color, radius } from "../theme";

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
  | { field: "temp"; mirek: number };

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
        style={{ width: size, height: size, display: "block", borderRadius: "50%", cursor: "crosshair" }}
      />
      <div
        style={{
          position: "absolute",
          left: kx - 10,
          top: ky - 10,
          width: 20,
          height: 20,
          borderRadius: "50%",
          border: "3px solid #fff",
          boxSizing: "border-box",
          background: `rgb(${cr},${cg},${cb})`,
          boxShadow: "0 1px 5px rgba(0,0,0,0.65)",
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
        border: `1px solid ${color.border}`,
        background: `linear-gradient(to bottom, ${hex}, ${color.surfaceOff})`,
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
          boxShadow: "0 1px 5px rgba(0,0,0,0.65)",
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
        style={{ width: size, height: size, display: "block", borderRadius: "50%", cursor: "crosshair" }}
      />
      <div
        style={{
          position: "absolute",
          left: kx - 11,
          top: ky - 11,
          width: 22,
          height: 22,
          borderRadius: "50%",
          border: "3px solid #fff",
          boxShadow: "0 0 0 1px rgba(0,0,0,0.45), 0 1px 4px rgba(0,0,0,0.5)",
          pointerEvents: "none",
        }}
      />
    </div>
  );
}

// ── Color / White mode toggle ────────────────────────────────────────────────

/** Segmented pill that switches the editor between the color wheel and the white
 * (color-temperature) wheel — the Hue "Color / White" tabs. Touch-sized on
 * compact viewports per the shared control conventions. */
function ModeToggle({
  mode,
  onChange,
  compact,
}: {
  mode: "color" | "white";
  onChange: (m: "color" | "white") => void;
  compact: boolean;
}) {
  const seg = (m: "color" | "white", label: string) => {
    const active = mode === m;
    return (
      <button
        onClick={() => onChange(m)}
        aria-pressed={active}
        style={{
          flex: 1,
          padding: compact ? "0.55rem 0" : "0.3rem 0",
          minHeight: compact ? 40 : undefined,
          fontSize: compact ? "0.9rem" : "0.78rem",
          fontWeight: 600,
          letterSpacing: "0.02em",
          borderRadius: 9,
          border: "none",
          cursor: "pointer",
          color: active ? "#0c0c0e" : "#cfcfd6",
          background: active
            ? m === "white"
              ? "linear-gradient(90deg, #ffd9a0, #cfe4ff)"
              : "linear-gradient(90deg, #ff7d8a, #8b5cf6 55%, #38bdf8)"
            : "transparent",
        }}
      >
        {label}
      </button>
    );
  };
  return (
    <div
      style={{
        display: "flex",
        gap: 4,
        padding: 3,
        borderRadius: 12,
        background: "rgba(255,255,255,0.06)",
        border: "1px solid rgba(255,255,255,0.08)",
      }}
    >
      {seg("color", "Color")}
      {seg("white", "White")}
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
  initialMode?: "color" | "white";
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
  const panelRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  // Uncontrolled after mount: seeding from props once avoids hex→hsv→hex
  // roundtrip jitter while dragging.
  const [[hue, sat], setHs] = useState<[number, number]>(() => hexToHs(initialHex));
  const [brightness, setBrightness] = useState(initialBrightness);
  const [mirek, setMirek] = useState(initialMirek);
  // Color vs. white are mutually exclusive; only offer the toggle when the light
  // supports both. Default to the light's current mode.
  const canToggle = showColor && showWhite;
  const [mode, setMode] = useState<"color" | "white">(
    () => initialMode ?? (showColor ? "color" : "white"),
  );
  const hex = rgbToHex(...hsvToRgb(hue, sat, 1));

  useLayoutEffect(() => {
    // On phones the editor is a bottom sheet — no anchor math needed.
    if (isCompact) return;
    const panel = panelRef.current;
    if (!panel) return;
    const rect =
      anchor instanceof HTMLElement
        ? anchor.getBoundingClientRect()
        : new DOMRect(anchor.x, anchor.y, 0, 0);
    const w = panel.offsetWidth;
    const h = panel.offsetHeight;
    let left = rect.right + 12;
    if (left + w > window.innerWidth - 8) left = rect.left - 12 - w;
    left = Math.max(8, Math.min(window.innerWidth - w - 8, left));
    let top = rect.top + rect.height / 2 - h / 2;
    top = Math.max(8, Math.min(window.innerHeight - h - 8, top));
    setPos({ left, top });
  }, [anchor, isCompact]);

  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      const target = e.target as Node;
      if (panelRef.current?.contains(target)) return;
      // Clicks on the trigger element are handled by the trigger itself (so it
      // can toggle the editor closed); don't also self-close here.
      if (anchor instanceof HTMLElement && anchor.contains(target)) return;
      onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    // Defer so the click that opened the editor doesn't immediately close it.
    const t = setTimeout(() => document.addEventListener("pointerdown", onDown), 0);
    window.addEventListener("keydown", onKey);
    return () => {
      clearTimeout(t);
      document.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose, anchor]);

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

  const whiteHex = rgbToHex(...mirekToRgb(mirek));
  // Exactly one wheel renders for any capability combination.
  const colorActive = showColor && (!showWhite || mode === "color");
  const whiteActive = showWhite && (!showColor || mode === "white");

  return createPortal(
    <div
      ref={panelRef}
      style={
        isCompact
          ? sheetStyle
          : {
              position: "fixed",
              left: pos?.left ?? 0,
              top: pos?.top ?? 0,
              visibility: pos ? "visible" : "hidden",
              zIndex: 60,
              background: color.surface,
              border: `1px solid ${color.hairline}`,
              borderRadius: radius.frame,
              padding: "0.9rem",
              boxShadow: "0 12px 34px rgba(0,0,0,0.7)",
              display: "flex",
              flexDirection: "column",
              gap: "0.7rem",
            }
      }
    >
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: "1rem" }}>
        <span style={{ fontWeight: 600, fontSize: "0.9rem", color: "#eee", maxWidth: 200, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {title ?? "Color"}
        </span>
        <button
          onClick={onClose}
          aria-label="Close"
          style={{ background: "none", border: "none", color: "#777", cursor: "pointer", fontSize: "1.15rem", lineHeight: 1, padding: 0 }}
        >
          ×
        </button>
      </div>

      {canToggle && <ModeToggle mode={mode} onChange={setMode} compact={isCompact} />}

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
                border: c === hex ? "2px solid #fff" : "1px solid rgba(255,255,255,0.25)",
                background: c,
                cursor: "pointer",
                padding: 0,
              }}
            />
          ))}
        </div>
      )}

      {onToggle && (
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", borderTop: "1px solid var(--bf-border)", paddingTop: "0.6rem" }}>
          <span style={{ fontSize: "0.8rem", color: "#999" }}>Power</span>
          <button
            onClick={onToggle}
            aria-label={on ? "Turn off" : "Turn on"}
            style={{
              width: 44,
              height: 24,
              borderRadius: 12,
              // Glassy electric pill, matching the page toggles.
              border: `1px solid ${on ? "rgba(125,211,252,0.55)" : "rgba(255,255,255,0.12)"}`,
              cursor: "pointer",
              background: on
                ? "linear-gradient(90deg, rgba(125,211,252,0.45), rgba(34,211,238,0.12) 70%), rgba(10,25,36,0.55)"
                : "rgba(255,255,255,0.07)",
              boxShadow: on
                ? "0 0 14px -4px rgba(56,189,248,0.75), inset 0 1px 0 rgba(255,255,255,0.25)"
                : "inset 0 1px 0 rgba(255,255,255,0.06)",
              position: "relative",
              transition: "background 0.2s, box-shadow 0.2s, border-color 0.2s",
            }}
          >
            <span
              style={{
                position: "absolute",
                top: 2,
                left: on ? 22 : 2,
                width: 18,
                height: 18,
                borderRadius: "50%",
                background: on
                  ? "linear-gradient(180deg, #ffffff, #d6f1ff)"
                  : "rgba(255,255,255,0.4)",
                boxShadow: on
                  ? "0 0 8px rgba(125,211,252,0.9), 0 1px 2px rgba(0,0,0,0.45)"
                  : "0 1px 2px rgba(0,0,0,0.35)",
                transition: "left 0.2s, background 0.2s, box-shadow 0.2s",
              }}
            />
          </button>
        </div>
      )}

      {children}
    </div>,
    document.body,
  );
}
