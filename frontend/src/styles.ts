import type { CSSProperties } from "react";
import { color, radius, font, glow } from "./theme";

// Shared form/chrome styles for the pages that predate the design system
// (Settings, Rooms, Scenes, dialogs, Login/Setup, FloorPlan). They all consume
// `S`, so these now derive from `theme.ts` — repointing here reskins every one of
// them onto the candlelit gothic-cyber palette at once. New code should import
// semantic tokens from `theme.ts` directly; `S` is the migration surface.

/** Re-exported so existing `import { ACCENT } from "./styles"` sites stay valid
 * and resolve to the one true accent (primary neon). */
export { ACCENT, ACCENT_GLOW } from "./theme";

/** Full-screen centered shell — Login / Setup. */
const center: CSSProperties = {
  display: "flex",
  height: "100vh",
  alignItems: "center",
  justifyContent: "center",
  background: color.void,
};

/** A solid gothic surface card — forms and config panels. Glass is reserved for
 * the live control surfaces (see `glassCard`); forms read better solid. */
const card: CSSProperties = {
  background: color.surface,
  border: `1px solid ${color.hairline}`,
  borderRadius: radius.frame,
  padding: "1.5rem",
  display: "flex",
  flexDirection: "column",
  gap: "0.75rem",
  boxShadow: "inset 0 1px 0 rgba(236,230,240,0.05), 0 12px 30px -16px rgba(0,0,0,0.75)",
};

const input: CSSProperties = {
  padding: "0.6rem 0.8rem",
  borderRadius: radius.sm,
  border: `1px solid ${color.border}`,
  background: color.surfaceOff,
  color: color.text,
  fontFamily: font.body,
  fontSize: "1rem",
  width: "100%",
  boxSizing: "border-box",
};

/** Primary CTA — solid neon on dark ink, with a soft bloom. */
const button: CSSProperties = {
  padding: "0.6rem 1.2rem",
  borderRadius: radius.sm,
  border: "none",
  background: color.cyan,
  color: color.ink,
  fontWeight: 700,
  cursor: "pointer",
  fontSize: "0.95rem",
  boxShadow: glow(color.cyan, 16),
};

const buttonGhost: CSSProperties = {
  padding: "0.5rem 0.9rem",
  borderRadius: radius.sm,
  border: `1px solid ${color.border}`,
  background: "transparent",
  color: color.dim,
  cursor: "pointer",
  fontSize: "0.875rem",
};

const buttonDanger: CSSProperties = {
  padding: "0.5rem 0.9rem",
  borderRadius: radius.sm,
  border: `1px solid ${color.rose}88`,
  background: "transparent",
  color: color.rose,
  cursor: "pointer",
  fontSize: "0.875rem",
};

/** Outlined accent button — the primary "Save" action inside config editors.
 * Defaults to the neon accent; pass a color override for audio (violet). */
const buttonAccent: CSSProperties = {
  padding: "0.45rem 0.9rem",
  borderRadius: radius.sm,
  border: `1px solid ${color.cyan}`,
  background: "transparent",
  color: color.cyan,
  cursor: "pointer",
  fontSize: "0.85rem",
  fontWeight: 600,
};

export const S = { center, card, input, button, buttonGhost, buttonDanger, buttonAccent };
