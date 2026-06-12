// A tappable selectable row — a full-width label with a larger checkbox and a
// clear selected state. Friendlier than a bare checkbox on touch. Used by the
// room light/link/speaker pickers.

import type { ReactNode } from "react";

export function SelectRow({
  checked,
  onToggle,
  disabled,
  accent = "#38bdf8",
  children,
}: {
  checked: boolean;
  onToggle: () => void;
  disabled?: boolean;
  accent?: string;
  children: ReactNode;
}) {
  return (
    <label
      style={{
        display: "flex",
        alignItems: "center",
        gap: "0.6rem",
        padding: "0.55rem 0.6rem",
        borderRadius: 8,
        minHeight: 42,
        border: `1px solid ${checked ? accent : "transparent"}`,
        background: checked ? `${accent}1f` : "rgba(255,255,255,0.02)",
        color: disabled ? "#666" : "#ddd",
        cursor: disabled ? "default" : "pointer",
        fontSize: "0.9rem",
      }}
    >
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={onToggle}
        style={{ width: 18, height: 18, accentColor: accent, flexShrink: 0, cursor: disabled ? "default" : "pointer" }}
      />
      <span style={{ flex: 1, minWidth: 0, display: "flex", alignItems: "center", gap: "0.45rem", flexWrap: "wrap" }}>
        {children}
      </span>
    </label>
  );
}
