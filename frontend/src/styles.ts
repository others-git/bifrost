import type { CSSProperties } from "react";

/** Electric accent — replaces the old orange (#f90) chrome everywhere. */
export const ACCENT = "#38bdf8";
/** Soft glow of the accent, for box-shadows. */
export const ACCENT_GLOW = "rgba(56,189,248,0.65)";

const center: CSSProperties = {
  display: "flex",
  height: "100vh",
  alignItems: "center",
  justifyContent: "center",
  background: "#111",
};

const card: CSSProperties = {
  background: "#1e1e1e",
  borderRadius: 12,
  padding: "1.5rem",
  display: "flex",
  flexDirection: "column",
  gap: "0.75rem",
};

const input: CSSProperties = {
  padding: "0.6rem 0.8rem",
  borderRadius: 8,
  border: "1px solid #333",
  background: "#111",
  color: "#f0f0f0",
  fontSize: "1rem",
  width: "100%",
  boxSizing: "border-box",
};

const button: CSSProperties = {
  padding: "0.6rem 1.2rem",
  borderRadius: 8,
  border: "none",
  background: ACCENT,
  color: "#03121c",
  fontWeight: 700,
  cursor: "pointer",
  fontSize: "0.95rem",
};

const buttonGhost: CSSProperties = {
  padding: "0.5rem 0.9rem",
  borderRadius: 8,
  border: "1px solid #444",
  background: "transparent",
  color: "#ccc",
  cursor: "pointer",
  fontSize: "0.875rem",
};

const buttonDanger: CSSProperties = {
  padding: "0.5rem 0.9rem",
  borderRadius: 8,
  border: "1px solid #a33",
  background: "transparent",
  color: "#f66",
  cursor: "pointer",
  fontSize: "0.875rem",
};

export const S = { center, card, input, button, buttonGhost, buttonDanger };
