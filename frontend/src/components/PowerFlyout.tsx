// Control fly-out for a power device (switch / plug / fan / toggle): the device's
// full name, its kind, and an on/off switch. Anchored next to its trigger on
// desktop; a bottom sheet on phones — matching LightEditor / AudioEditor.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { PowerDevice } from "../api";
import { Glyph, powerKindGlyph } from "./glyphs";
import { sheetStyle } from "./sheet";
import { useViewport } from "../useViewport";

const ACCENT = "#38bdf8";

const KIND_LABEL: Record<string, string> = {
  switch: "Switch",
  outlet: "Outlet",
  fan: "Fan",
  toggle: "Toggle",
  generic: "Device",
};

export function PowerFlyout({
  device,
  anchor,
  onToggle,
  onSetEnabled,
  onClose,
}: {
  device: PowerDevice;
  anchor: HTMLElement | { x: number; y: number };
  onToggle: (next: boolean) => void;
  /** Enable/disable the device. Disabling drops it from room control. */
  onSetEnabled?: (enabled: boolean) => void;
  onClose: () => void;
}) {
  const { isMobile } = useViewport();
  const panelRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const on = device.state.on;
  const offline = device.state.reachable === false;

  useLayoutEffect(() => {
    if (isMobile) return;
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
  }, [anchor, isMobile]);

  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      if (panelRef.current && !panelRef.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    const t = setTimeout(() => document.addEventListener("pointerdown", onDown), 0);
    window.addEventListener("keydown", onKey);
    return () => {
      clearTimeout(t);
      document.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  return createPortal(
    <div
      ref={panelRef}
      style={
        isMobile
          ? sheetStyle
          : {
              position: "fixed",
              left: pos?.left ?? 0,
              top: pos?.top ?? 0,
              visibility: pos ? "visible" : "hidden",
              zIndex: 60,
              width: 240,
              background: "#1c1c20",
              border: "1px solid #333",
              borderRadius: 14,
              padding: "0.9rem",
              boxShadow: "0 8px 30px rgba(0,0,0,0.6)",
              display: "flex",
              flexDirection: "column",
              gap: "0.8rem",
            }
      }
    >
      <div style={{ display: "flex", alignItems: "center", gap: "0.7rem" }}>
        <span style={{ color: on ? ACCENT : "#888", flexShrink: 0 }}>
          <Glyph name={device.glyph ?? powerKindGlyph(device.kind)} size={24} />
        </span>
        <div style={{ minWidth: 0, flex: 1 }}>
          <div style={{ fontWeight: 600, fontSize: "0.95rem", color: "#eee", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {device.name}
          </div>
          <div style={{ fontSize: "0.7rem", color: "#6b6557" }}>
            {KIND_LABEL[device.kind] ?? device.kind}
            {offline && <span style={{ color: "#c2603f" }}> · offline</span>}
          </div>
        </div>
        <button
          onClick={onClose}
          aria-label="Close"
          style={{ background: "none", border: "none", color: "#777", cursor: "pointer", fontSize: "1.15rem", lineHeight: 1, padding: 0 }}
        >
          ×
        </button>
      </div>

      <button
        onClick={() => onToggle(!on)}
        disabled={offline}
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          gap: "0.5rem",
          padding: "0.7rem",
          borderRadius: 10,
          border: `1px solid ${on ? "rgba(56,189,248,0.55)" : "rgba(255,255,255,0.14)"}`,
          background: on
            ? "linear-gradient(180deg, rgba(56,189,248,0.28), rgba(56,189,248,0.08))"
            : "rgba(255,255,255,0.05)",
          color: on ? "#dff3ff" : "#bbb",
          cursor: offline ? "not-allowed" : "pointer",
          opacity: offline ? 0.5 : 1,
          fontSize: "0.9rem",
          fontWeight: 600,
        }}
      >
        {on ? "On" : "Off"} — tap to turn {on ? "off" : "on"}
      </button>

      {onSetEnabled && (
        <DisableRow
          enabled={device.enabled !== false}
          onSetEnabled={(en) => {
            onSetEnabled(en);
            if (!en) onClose();
          }}
        />
      )}
    </div>,
    document.body,
  );
}

/** Footer link to disable/enable a device — shared wording across fly-outs. */
export function DisableRow({
  enabled,
  onSetEnabled,
}: {
  enabled: boolean;
  onSetEnabled: (enabled: boolean) => void;
}) {
  return (
    <button
      onClick={() => onSetEnabled(!enabled)}
      title={
        enabled
          ? "Stop sending commands and hide from room control (stays in the room)"
          : "Resume control of this device"
      }
      style={{
        background: "none",
        border: "none",
        color: enabled ? "#9a6b5a" : "#6fae84",
        cursor: "pointer",
        fontSize: "0.74rem",
        padding: "0.2rem 0",
        alignSelf: "flex-start",
      }}
    >
      {enabled ? "Disable device" : "Enable device"}
    </button>
  );
}
