// The shared light editor: a popover anchored next to whatever triggered it,
// with a Hue-style hue/saturation color wheel and a vertical brightness bar.
// Used for lights, rooms, scene palettes, and the planner paint brush.

import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { rgbToHex } from "../api";

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

// Warm whites first (the lived-in defaults), then color anchors.
const SWATCHES = ["#ffffff", "#ffe4b3", "#ffb46b", "#ff7d33", "#ff5e9c", "#8b5cf6", "#3b82f6", "#4ade80"];

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
  hex,
  value,
  onPick,
}: {
  height?: number;
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

  const knobTop = 3 + (1 - value / 100) * (height - 30);

  return (
    <div
      ref={trackRef}
      title={`${value}%`}
      style={{
        position: "relative",
        width: 30,
        height,
        borderRadius: 15,
        border: "1px solid #333",
        background: `linear-gradient(to bottom, ${hex}, #15151a)`,
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
          left: 2,
          top: knobTop,
          width: 24,
          height: 24,
          borderRadius: "50%",
          background: "#fff",
          boxShadow: "0 1px 5px rgba(0,0,0,0.65)",
          pointerEvents: "none",
        }}
      />
    </div>
  );
}

// ── Anchored editor popover ──────────────────────────────────────────────────

export function LightEditor({
  anchor,
  title,
  initialHex,
  initialBrightness = 100,
  showColor = true,
  showBrightness = true,
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
  showColor?: boolean;
  showBrightness?: boolean;
  /** When provided, the editor shows a power switch row. */
  on?: boolean;
  onToggle?: () => void;
  /** Fires live while dragging — callers debounce network sends. */
  onChange: (hex: string, brightness: number) => void;
  onClose: () => void;
  /** Extra controls rendered at the bottom (e.g. a room's scene selector). */
  children?: ReactNode;
}) {
  const panelRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  // Uncontrolled after mount: seeding from props once avoids hex→hsv→hex
  // roundtrip jitter while dragging.
  const [[hue, sat], setHs] = useState<[number, number]>(() => hexToHs(initialHex));
  const [brightness, setBrightness] = useState(initialBrightness);
  const hex = rgbToHex(...hsvToRgb(hue, sat, 1));

  useLayoutEffect(() => {
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
  }, [anchor]);

  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      if (panelRef.current && !panelRef.current.contains(e.target as Node)) onClose();
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
  }, [onClose]);

  function applyColor(h: number, s: number) {
    setHs([h, s]);
    onChange(rgbToHex(...hsvToRgb(h, s, 1)), brightness);
  }

  function applyBrightness(b: number) {
    setBrightness(b);
    onChange(hex, b);
  }

  return createPortal(
    <div
      ref={panelRef}
      style={{
        position: "fixed",
        left: pos?.left ?? 0,
        top: pos?.top ?? 0,
        visibility: pos ? "visible" : "hidden",
        zIndex: 60,
        background: "#1c1c20",
        border: "1px solid #333",
        borderRadius: 14,
        padding: "0.9rem",
        boxShadow: "0 8px 30px rgba(0,0,0,0.6)",
        display: "flex",
        flexDirection: "column",
        gap: "0.7rem",
      }}
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

      <div style={{ display: "flex", gap: "0.8rem", alignItems: "center" }}>
        {showColor && <ColorWheel hue={hue} sat={sat} onPick={applyColor} />}
        {showBrightness && <BrightnessBar hex={showColor ? hex : "#ffd9a0"} value={brightness} onPick={applyBrightness} />}
      </div>

      {showColor && (
        <div style={{ display: "flex", gap: "0.45rem", justifyContent: "center" }}>
          {SWATCHES.map((c) => (
            <button
              key={c}
              onClick={() => {
                const [h, s] = hexToHs(c);
                applyColor(h, s);
              }}
              title={c}
              style={{
                width: 18,
                height: 18,
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
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", borderTop: "1px solid #2a2a2e", paddingTop: "0.6rem" }}>
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
