// One dispatcher for per-device control fly-outs. `DeviceControl` routes by
// domain to the right shared editor — never a forked control path — so the
// Dashboard, Floor Plan, and Rooms open the same surface and a new domain is
// added in exactly one place. The light case goes through `LightFlyout`, the
// device-driven adapter that turns the presentational `LightEditor` into a
// `{ light }`-shaped control (deriving the wheels + owning the commit), so the
// light↔state math no longer lives copy-pasted in every page.

import { useEffect, useRef, type ReactNode } from "react";
import {
  rgbToHex,
  xyToRgb,
  setLightState,
  setLightSegments,
  getMediaDevice,
  type Light,
  type LightState,
  type MediaDevice,
  type PowerDevice,
} from "../api";
import { LightEditor, type LightControlChange } from "./LightEditor";
import { lightOptimistic, lightSupports, lightWrite } from "./lightControl";
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

  // Power is its own independent dimension: send just `{ on }` so toggling never
  // re-asserts a colour/effect (the server preserves the running mode on an
  // on/off-only write). The optimistic patch keeps the full look for the UI.
  function togglePower() {
    const next = !isOn;
    onLocalPatch(light.id, { ...(st ?? { on: false }), on: next });
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setLightState(light.id, { on: next }), 200);
  }

  function onChange(change: LightControlChange) {
    // The shared light-control rule (see `lightControl.ts`): update the UI
    // optimistically with the full resolved state, but **send only the dimension
    // that moved** — never a stale colour/effect riding along with a brightness
    // tweak. The server merges the minimal patch and preserves the rest.
    if (!lightSupports(change, light.capabilities)) return;
    onLocalPatch(light.id, lightOptimistic(light.last_state, change));
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setLightState(light.id, lightWrite(change)), 200);
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
      onToggle={togglePower}
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
  // Immediately pull a fresh live read the moment a media control opens, so its
  // display info (now-playing, volume, source, reachability) is current right away
  // — poll/GENA-push providers (Sonos especially) otherwise serve the last cached
  // read. The single-device media GET round-trips to the device server-side, then
  // we patch local state. Lights are push-live (Hue SSE) and a live read could
  // clobber a cached effect a provider doesn't report; power is on/off (push-live)
  // — neither needs a refresh here.
  const refreshMediaId = props.domain === "media" ? props.device.id : null;
  const patchMedia = props.domain === "media" ? props.onLocalPatch : null;
  useEffect(() => {
    if (!refreshMediaId || !patchMedia) return;
    let alive = true;
    getMediaDevice(refreshMediaId).then((d) => {
      if (alive && d) patchMedia(d.id, d.state);
    });
    return () => {
      alive = false;
    };
  }, [refreshMediaId, patchMedia]);

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
