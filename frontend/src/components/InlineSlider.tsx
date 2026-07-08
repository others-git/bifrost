// A compact draggable value bar (0–100) — brightness / volume on a tile. Drag
// (or tap) sets the value; `onChange` fires live, `onCommit` on release. Drags
// don't bubble (so it works inside a draggable board widget). `fill` makes it a
// tall bar that fills its parent; otherwise it's a thin `height`-px line.

import { useRef } from "react";
import { alpha, radius, T } from "../theme";

export function InlineSlider({
  value,
  accent,
  unit = "%",
  height = 20,
  fill = false,
  onChange,
  onCommit,
}: {
  value: number;
  accent: string;
  unit?: string;
  /** Bar height (px). Ignored when `fill`. */
  height?: number;
  /** Fill the parent's height (a tall bar) instead of a fixed height. */
  fill?: boolean;
  onChange: (v: number) => void;
  onCommit: (v: number) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  const pct = (clientX: number) => {
    const r = ref.current!.getBoundingClientRect();
    return Math.round(Math.max(0, Math.min(1, (clientX - r.left) / r.width)) * 100);
  };
  const tall = fill || height >= 40;
  return (
    <div
      ref={ref}
      onPointerDown={(e) => {
        e.preventDefault();
        e.stopPropagation();
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
        dragging.current = true;
        onChange(pct(e.clientX));
      }}
      onPointerMove={(e) => { if (dragging.current) onChange(pct(e.clientX)); }}
      onPointerUp={(e) => { if (dragging.current) { dragging.current = false; onCommit(pct(e.clientX)); } }}
      onPointerCancel={() => { dragging.current = false; }}
      style={{
        position: "relative",
        width: "100%",
        height: fill ? "100%" : height,
        borderRadius: tall ? radius.frame : radius.pill,
        background: "rgba(0,0,0,0.35)",
        border: `1px solid ${T.hairline}`,
        boxShadow: "inset 0 1px 5px rgba(0,0,0,0.45)",
        cursor: "pointer",
        touchAction: "none",
        overflow: "hidden",
      }}
    >
      {/* The column reads like mercury: a translucent accent wash for the body
          (never a solid slab, even at 100%) with a bright glowing meniscus at
          its leading edge carrying the colour. */}
      <div
        style={{
          position: "absolute",
          left: 0,
          top: 0,
          bottom: 0,
          width: `${value}%`,
          background: `linear-gradient(90deg, ${alpha(accent, 0.16)}, ${alpha(accent, 0.5)})`,
          boxShadow: `inset 0 0 14px -5px ${accent}`,
          transition: "width 0.08s",
        }}
      />
      {value > 0 && (
        <div
          aria-hidden
          style={{
            position: "absolute",
            top: 0,
            bottom: 0,
            left: `calc(${value}% - 2px)`,
            width: 2,
            background: accent,
            boxShadow: `0 0 8px ${accent}`,
            transition: "left 0.08s",
          }}
        />
      )}
      <span
        style={{
          position: "absolute",
          right: tall ? 10 : 7,
          bottom: tall ? 6 : 0,
          top: tall ? undefined : 0,
          display: "flex",
          alignItems: "center",
          fontSize: tall ? "0.92rem" : "0.66rem",
          fontWeight: tall ? 600 : 400,
          color: T.text,
          fontVariantNumeric: "tabular-nums",
          textShadow: "0 1px 2px #000",
        }}
      >
        {/* Devices report fractional percents (HA's 0-255 → %); the readout is
            always a whole number. The bar itself keeps the raw value. */}
        {Math.round(value)}
        {unit}
      </span>
    </div>
  );
}
