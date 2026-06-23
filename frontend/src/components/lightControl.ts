// The single source of truth for turning a light-control gesture into a write.
//
// Every surface that drives lights — the per-device fly-out (`DeviceControl`),
// the Dashboard/Floor-Plan room cascades, and the Boards device/group widgets —
// routes through these helpers instead of hand-building a `LightState`. That
// keeps one rule everywhere: a control change sends **only the dimension that
// moved**. A brightness tweak is `{ on, brightness }` — never a stale colour or
// effect riding along (which the backend's mode-exclusion would mis-resolve, e.g.
// a colour clobbering a just-set effect). The server merges the patch onto the
// light's cached state and preserves the untouched dimensions.

import { rgbToXy, type LightCapabilities, type LightState } from "../api";
import { hexToRgb, type LightControlChange } from "./LightEditor";

/** The provider-native tokens that all mean "no effect is running" (mirrors the
 * backend's `is_clear_effect`); such a pick clears effect mode rather than
 * entering it. */
export function isClearEffect(effect: string): boolean {
  return ["", "no_effect", "off", "none"].includes(effect.trim().toLowerCase());
}

/** The minimal patch to **send** for a control change — only the moved dimension
 * (always with `on: true`, since adjusting a light implies turning it on). The
 * backend applies mode mutual-exclusion and preserves everything else, so we
 * never include colour/temp/effect unless that is what changed. */
export function lightWrite(change: LightControlChange): LightState {
  switch (change.field) {
    case "brightness":
      return { on: true, brightness: change.brightness };
    case "color":
      return { on: true, color: rgbToXy(...hexToRgb(change.hex)) };
    case "temp":
      return { on: true, color_temp_mirek: change.mirek };
    case "effect":
      return { on: true, effect: change.effect };
  }
}

/** Whether a light can actually take this change, so an aggregate (room/group)
 * fans a moved dimension out only to capable members — a brightness change skips
 * non-dimmable lights, an effect skips lights whose catalog lacks it, etc. */
export function lightSupports(change: LightControlChange, caps: LightCapabilities): boolean {
  switch (change.field) {
    case "brightness":
      return caps.dimmable;
    case "color":
      return caps.color_rgb;
    case "temp":
      return caps.color_temperature;
    case "effect":
      return (caps.effects ?? []).includes(change.effect);
  }
}

/** The optimistic local state after a change, for instant UI feedback —
 * mirrors the backend's mode mutual-exclusion (colour / temp / effect clear one
 * another; brightness and power are independent) so the UI matches what the
 * server will store. */
export function lightOptimistic(
  prev: LightState | undefined,
  change: LightControlChange,
): LightState {
  const s: LightState = { ...(prev ?? { on: false }), on: true };
  switch (change.field) {
    case "brightness":
      s.brightness = change.brightness;
      break;
    case "color":
      s.color = rgbToXy(...hexToRgb(change.hex));
      s.color_temp_mirek = undefined;
      s.effect = undefined;
      break;
    case "temp":
      s.color_temp_mirek = change.mirek;
      s.color = undefined;
      s.effect = undefined;
      break;
    case "effect":
      if (isClearEffect(change.effect)) {
        s.effect = undefined;
      } else {
        s.effect = change.effect;
        s.color = undefined;
        s.color_temp_mirek = undefined;
      }
      break;
  }
  return s;
}
