import type { CSSProperties } from "react";
import { color, radius, font } from "./theme";
import { buttonStyle } from "./components/controls";

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

// Button styles derive from the one shared source (`buttonStyle`) so inline
// `S.button*` spreads and the `<Button>` component can never drift. Prefer
// `<Button variant>` in new code; `S.button*` stays for existing spread sites.
const button = buttonStyle("primary");
const buttonGhost = buttonStyle("ghost");
const buttonDanger = buttonStyle("danger");
const buttonAccent = buttonStyle("accent");

export const S = { center, card, input, button, buttonGhost, buttonDanger, buttonAccent };
