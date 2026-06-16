// Gothic ornament primitives — the victorian-gothic chrome of the design system.
// Pure decoration (pointer-events: none).
//
// `CornerFiligree` doubles as a *status light*. It paints ONE card-wide gradient
// of the room's live light colours and masks it into four tapered corner
// brackets — so the colours flow continuously around the frame (each corner
// reveals its local slice of the same gradient, never a repeat). A dormant room
// (off / no colour) shows dull, tarnished brass — patina'd, not gilded.

import type { CSSProperties } from "react";

/** Dull tarnished brass for a dormant frame — darker and desaturated than the
 * bright `color.gold` ornament; reads as "off / un-lit", not gilded. */
const TARNISH = ["var(--bf-tarnishHi)", "var(--bf-tarnishLo)", "var(--bf-tarnish)"];

/** Four tapered corner brackets that frame the nearest positioned ancestor like
 * an engraved plate. `colors` = the room's lit light hexes (the gradient flows
 * through them); empty/undefined → tarnished brass. */
export function CornerFiligree({
  inset = 2,
  len = 20,
  thickness = 2.5,
  colors,
}: {
  inset?: number;
  len?: number;
  thickness?: number;
  colors?: string[];
}) {
  const lit = !!colors && colors.length > 0;
  const palette = lit ? colors! : TARNISH;
  // One continuous gradient across the whole card. The mask (below) reveals only
  // the corner brackets, so a single field's colours flow around the frame.
  const stops = palette.length === 1 ? [palette[0], palette[0]] : palette;
  const grad = `linear-gradient(125deg, ${stops.join(", ")})`;

  // Build the L-bracket mask: 8 arms (4 corners × 2), each tapering from solid at
  // the corner vertex to transparent at its open end.
  const fade = (dir: string) =>
    `linear-gradient(to ${dir}, #000 0%, #000 30%, transparent 100%)`;
  const layers: { img: string; pos: string; size: string }[] = [];
  for (const v of ["top", "bottom"] as const) {
    for (const h of ["left", "right"] as const) {
      const pos = `${h} ${inset}px ${v} ${inset}px`;
      layers.push({ img: fade(h === "left" ? "right" : "left"), pos, size: `${len}px ${thickness}px` });
      layers.push({ img: fade(v === "top" ? "bottom" : "top"), pos, size: `${thickness}px ${len}px` });
    }
  }
  const maskImage = layers.map((l) => l.img).join(", ");
  const maskPosition = layers.map((l) => l.pos).join(", ");
  const maskSize = layers.map((l) => l.size).join(", ");
  const maskRepeat = layers.map(() => "no-repeat").join(", ");

  // The gradient + mask live on the inner div; the glow lives on the outer one.
  // CSS applies `filter` BEFORE `mask`, so a drop-shadow on the masked element
  // would itself be clipped away — nesting lets the glow follow the bracket shape.
  const masked: CSSProperties = {
    position: "absolute",
    inset: 0,
    background: grad,
    maskImage,
    maskPosition,
    maskSize,
    maskRepeat,
    WebkitMaskImage: maskImage,
    WebkitMaskPosition: maskPosition,
    WebkitMaskSize: maskSize,
    WebkitMaskRepeat: maskRepeat,
  };

  return (
    <div
      aria-hidden
      style={{
        position: "absolute",
        inset: 0,
        pointerEvents: "none",
        opacity: lit ? 0.95 : 0.7,
        filter: lit ? `drop-shadow(0 0 5px ${palette[0]})` : "none",
      }}
    >
      <div style={masked} />
    </div>
  );
}
