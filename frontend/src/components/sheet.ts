import type { CSSProperties } from "react";

/** Full-width bottom-sheet styling for fly-outs on phones, where an anchored
 * popover doesn't fit. Slides up from the bottom edge with a rounded top. */
export const sheetStyle: CSSProperties = {
  position: "fixed",
  left: 0,
  right: 0,
  bottom: 0,
  width: "100%",
  maxHeight: "85vh",
  overflowY: "auto",
  zIndex: 60,
  background: "#1c1c20",
  borderTop: "1px solid #333",
  borderRadius: "16px 16px 0 0",
  padding: "0.9rem",
  paddingBottom: "calc(0.9rem + env(safe-area-inset-bottom))",
  boxShadow: "0 -8px 30px rgba(0,0,0,0.6)",
  display: "flex",
  flexDirection: "column",
  gap: "0.7rem",
};
