// Control fly-out for a power device (switch / plug / fan / toggle): the device's
// full name, its kind, and an on/off switch. Anchored next to its trigger on
// desktop; a bottom sheet on phones — matching LightEditor / AudioEditor.

import type { PowerDevice } from "../api";
import { Glyph, powerKindGlyph } from "./glyphs";
import { useViewport } from "../useViewport";
import { color } from "../theme";
import { Flyout, FlyoutHeader } from "./Flyout";
import { PowerToggle } from "./controls";

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
  const { isCompact } = useViewport();
  const on = device.state.on;
  const offline = device.state.reachable === false;

  return (
    <Flyout anchor={anchor} onClose={onClose} width={240} gap="0.8rem">
      {/* Power is the gold domain. For a power-only device, on/off is the whole
          control, so the power button is the body hero (big), with the device's
          kind glyph identifying it in the header. */}
      <FlyoutHeader
        title={device.name}
        subtitle={
          <>
            {KIND_LABEL[device.kind] ?? device.kind}
            {offline && <span style={{ color: color.rose }}> · offline</span>}
          </>
        }
        icon={<Glyph name={device.glyph ?? powerKindGlyph(device.kind)} size={22} />}
        accent={on ? color.gold : color.dim}
        onClose={onClose}
      />

      <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: "0.55rem", padding: "0.5rem 0 0.3rem" }}>
        <PowerToggle
          on={on}
          accent={color.gold}
          disabled={offline}
          onToggle={() => onToggle(!on)}
          size={isCompact ? 84 : 64}
        />
        <span
          style={{
            fontSize: "0.9rem",
            fontWeight: 600,
            letterSpacing: "0.05em",
            color: offline ? color.rose : on ? color.goldBright : color.dim,
          }}
        >
          {offline ? "Offline" : on ? "On" : "Off"}
        </span>
      </div>

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
  // A secondary action — a quiet text link, centred + roomier to tap on compact,
  // never a heavy full-width button that dominates a sparse fly-out.
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
        color: tone,
        cursor: "pointer",
        fontSize: isCompact ? "0.85rem" : "0.74rem",
        padding: isCompact ? "0.5rem 0.8rem" : "0.2rem 0",
        alignSelf: isCompact ? "center" : "flex-start",
      }}
    >
      {enabled ? "Disable device" : "Enable device"}
    </button>
  );
}
