import type { CSSProperties } from "react";
import { color, radius, glass } from "../theme";

/** Full-width bottom-sheet styling for fly-outs on compact viewports, where an
 * anchored popover doesn't fit. Frosted gothic glass that slides up from the
 * bottom edge with a rounded, gold-hairline top — matching the live cards. */
export const sheetStyle: CSSProperties = {
  position: "fixed",
  left: 0,
  right: 0,
  bottom: 0,
  width: "100%",
  maxHeight: "85vh",
  overflowY: "auto",
  zIndex: 60,
  background: glass.background,
  backdropFilter: glass.backdropFilter,
  WebkitBackdropFilter: glass.WebkitBackdropFilter,
  borderTop: `1px solid ${color.hairline}`,
  borderRadius: `${radius.frame}px ${radius.frame}px 0 0`,
  padding: "0.9rem",
  paddingBottom: "calc(0.9rem + env(safe-area-inset-bottom))",
  boxShadow: "0 -10px 34px rgba(0,0,0,0.7)",
  color: color.text,
  display: "flex",
  flexDirection: "column",
  gap: "0.7rem",
};
