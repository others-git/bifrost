// One dispatcher for per-device control fly-outs. `DeviceControl` routes by
// domain to the right shared editor — never a forked control path — so the
// Dashboard, Floor Plan, and Rooms open the same surface and a new domain is
// added in exactly one place. The light case goes through `LightFlyout`, the
// device-driven adapter that turns the presentational `LightEditor` into a
// `{ light }`-shaped control (deriving the wheels + owning the commit), so the
// light↔state math no longer lives copy-pasted in every page.

import { useRef, type ReactNode } from "react";
import {
  rgbToHex,
  rgbToXy,
  xyToRgb,
  setLightState,
  setLightSegments,
  type Light,
  type LightState,
  type MediaDevice,
  type PowerDevice,
} from "../api";
import { hexToRgb, LightEditor, type LightControlChange } from "./LightEditor";
import { MediaEditor } from "./MediaControls";
import { PowerFlyout } from "./PowerFlyout";

type Anchor = HTMLElement | { x: number; y: number };

/** Per-device light fly-out: the device-driven wrapper around `LightEditor`.
 * Derives the initial wheel values + which modes to show from the light's
 * capabilities and last state, and owns the debounced commit — optimistic
 * `onLocalPatch`, then `setLightState` after 200ms. Colour / white / effect are
 * three mutually-exclusive modes: picking one clears the others, and a plain
 * brightness/colour tweak clears any running effect so it never re-fires. Each
 * field is gated on the light's capability. (Consolidates the identical logic
 * that lived in Dashboard's LightButton and the Floor Plan's per-device editor.) */
export function LightFlyout({
  light,
  anchor,
  onLocalPatch,
  onClose,
  children,
}: {
  light: Light;
  anchor: Anchor;
  /** Optimistic UI update; the network commit is owned here. */
  onLocalPatch: (id: string, state: LightState) => void;
  onClose: () => void;
  /** Extra controls (a DisableRow, a SceneButton) rendered at the editor's foot. */
  children?: ReactNode;
}) {
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const st = light.last_state;
  const isOn = st?.on ?? false;
  const hex = st?.color
    ? rgbToHex(...xyToRgb(st.color.x, st.color.y, st.color.brightness))
    : "#ffb84d";
  // White mode = a reported temperature and no colour.
  const whiteMode = st?.color_temp_mirek != null && !st?.color;

  function commit(next: LightState) {
    onLocalPatch(light.id, next);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setLightState(light.id, next), 200);
  }

  function onChange(change: LightControlChange) {
    // Send only the dimension that changed. Brightness and on/off are independent
    // of the colour/temp/effect mode, so a brightness change carries no mode and
    // the server preserves (and re-asserts) any running effect instead of dropping
    // it. The three modes are mutually exclusive — the server clears the other two
    // when one is set — so we never need to send them together.
    const next: LightState = { on: true };
    if (change.field === "brightness") {
      if (light.capabilities.dimmable) next.brightness = change.brightness;
    } else if (change.field === "color") {
      if (light.capabilities.color_rgb) next.color = rgbToXy(...hexToRgb(change.hex));
    } else if (change.field === "temp") {
      if (light.capabilities.color_temperature) next.color_temp_mirek = change.mirek;
    } else if (change.field === "effect") {
      next.effect = change.effect;
    }
    commit(next);
  }

  return (
    <LightEditor
      anchor={anchor}
      title={light.name}
      initialHex={hex}
      initialBrightness={st?.brightness ?? 100}
      initialMirek={st?.color_temp_mirek ?? 366}
      initialMode={whiteMode ? "white" : "color"}
      showColor={light.capabilities.color_rgb}
      showWhite={light.capabilities.color_temperature}
      showBrightness={light.capabilities.dimmable}
      effects={light.capabilities.effects}
      initialEffect={st?.effect}
      segments={light.capabilities.segments}
      onSegments={(segs) => setLightSegments(light.id, segs)}
      on={isOn}
      onToggle={() => commit({ ...(st ?? { on: false }), on: !isOn })}
      onChange={onChange}
      onClose={onClose}
    >
      {children}
    </LightEditor>
  );
}

/** Domain-tagged control props — a discriminated union so each domain carries
 * exactly the callbacks its editor needs. */
export type DeviceControlProps =
  | {
      domain: "light";
      light: Light;
      onLocalPatch: (id: string, state: LightState) => void;
      children?: ReactNode;
    }
  | {
      domain: "media";
      device: MediaDevice;
      onLocalPatch: (id: string, patch: Partial<MediaDevice["state"]>) => void;
      onSetEnabled?: (enabled: boolean) => void;
      receiverName?: string;
    }
  | {
      domain: "power";
      device: PowerDevice;
      onToggle: (next: boolean) => void;
      onSetEnabled?: (enabled: boolean) => void;
    };

/** The single per-device control entry point: render the editor for whatever
 * domain the device belongs to. */
export function DeviceControl(props: DeviceControlProps & { anchor: Anchor; onClose: () => void }) {
  if (props.domain === "light") {
    return (
      <LightFlyout light={props.light} anchor={props.anchor} onLocalPatch={props.onLocalPatch} onClose={props.onClose}>
        {props.children}
      </LightFlyout>
    );
  }
  if (props.domain === "media") {
    return (
      <MediaEditor
        device={props.device}
        anchor={props.anchor}
        onLocalPatch={props.onLocalPatch}
        onSetEnabled={props.onSetEnabled}
        onClose={props.onClose}
        receiverName={props.receiverName}
      />
    );
  }
  return (
    <PowerFlyout
      device={props.device}
      anchor={props.anchor}
      onToggle={props.onToggle}
      onSetEnabled={props.onSetEnabled}
      onClose={props.onClose}
    />
  );
}
