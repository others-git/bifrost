// Control fly-out for a power device (switch / plug / fan / toggle): the device's
// full name, its kind, and an on/off switch. Anchored next to its trigger on
// desktop; a bottom sheet on phones — matching LightEditor / AudioEditor.

import type { PowerDevice } from "../api";
import { Glyph, powerKindGlyph } from "./glyphs";
import { useViewport } from "../useViewport";
import { color, alpha } from "../theme";
import { Flyout, FlyoutHeader } from "./Flyout";

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
  const on = device.state.on;
  const offline = device.state.reachable === false;

  return (
    <Flyout anchor={anchor} onClose={onClose} width={240} gap="0.8rem">
      {/* Power is the gold domain — tint the shared header gold when on. */}
      <FlyoutHeader
        title={device.name}
        subtitle={
          <>
            {KIND_LABEL[device.kind] ?? device.kind}
            {offline && <span style={{ color: color.rose }}> · offline</span>}
          </>
        }
        icon={<Glyph name={device.glyph ?? powerKindGlyph(device.kind)} size={24} />}
        accent={on ? color.gold : color.dim}
        onClose={onClose}
      />

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
          border: `1px solid ${on ? alpha(color.gold, 0.55) : alpha(color.text, 0.14)}`,
          background: on
            ? `linear-gradient(180deg, ${alpha(color.gold, 0.26)}, ${alpha(color.gold, 0.07)})`
            : alpha(color.text, 0.05),
          color: on ? color.goldBright : color.dim,
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
    </Flyout>
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
  const { isCompact } = useViewport();
  const tone = enabled ? "#9a6b5a" : "#6fae84";
  return (
    <button
      onClick={() => onSetEnabled(!enabled)}
      title={
        enabled
          ? "Stop sending commands and hide from room control (stays in the room)"
          : "Resume control of this device"
      }
      style={
        isCompact
          ? {
              background: "rgba(255,255,255,0.04)",
              border: `1px solid ${alpha(tone, 0.33)}`,
              borderRadius: 10,
              color: tone,
              cursor: "pointer",
              fontSize: "0.9rem",
              padding: "0.7rem 1rem",
              minHeight: 44,
              width: "100%",
              textAlign: "center",
            }
          : {
              background: "none",
              border: "none",
              color: tone,
              cursor: "pointer",
              fontSize: "0.74rem",
              padding: "0.2rem 0",
              alignSelf: "flex-start",
            }
      }
    >
      {enabled ? "Disable device" : "Enable device"}
    </button>
  );
}
