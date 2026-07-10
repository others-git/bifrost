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

import {
  rgbToHex,
  rgbToXy,
  xyToRgb,
  type Light,
  type LightCapabilities,
  type LightState,
  type LightStatePatch,
} from "../api";
import { hexToRgb, type LightControlChange } from "./LightEditor";

/** The state-bearing shape the aggregate helpers need — satisfied by `Light` and
 * by the Floor Plan's synthesized `{ capabilities, last_state }` members (whose
 * live state lives in a separate map, not on the `Light` object). */
export type LightLike = Pick<Light, "capabilities"> & { last_state?: LightState };

/** The room-cascade variant of `lightWrite`: attribute changes carry NO power
 * bit, so the backend casts them onto lit members only — dimming, recolouring,
 * or retuning a room never wakes its off lamps. (Room power is its own
 * explicit `{ on }` PUT, and single-device edits keep `lightWrite` — dragging
 * one lamp's own slider is a deliberate act on that lamp.) */
export function roomLightWrite(change: LightControlChange): LightStatePatch {
  const { on: _implicitOn, ...attrs } = lightWrite(change);
  return attrs;
}

/** The niche/glow colour worn while a dynamic effect runs — the drift
 * animation cycles the full hue wheel from this base, and reduced-motion
 * freezes here (the light-domain cyan, so a frozen niche still reads themed). */
export const EFFECT_ACCENT = "#22d3ee";

/** The light's running dynamic effect, or undefined — the one check every
 * surface shares for "should this light wear the effect treatment". */
export function activeEffect(l: LightLike & { last_state?: LightState }): string | undefined {
  const e = l.last_state?.effect;
  return e && !isClearEffect(e) ? e : undefined;
}

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

/** The dynamic effects common to EVERY light in a group — the intersection of
 * each member's catalog, so a chosen effect applies to all of them. Empty when
 * any member has no effects (which hides the group's effects UI). Shared by every
 * aggregate surface (room card, Boards group, Floor Plan) that offers effects. */
export function commonLightEffects(lights: LightLike[]): string[] {
  return lights.reduce<string[]>(
    (acc, l, i) => {
      const e = l.capabilities.effects ?? [];
      return i === 0 ? [...e] : acc.filter((x) => e.includes(x));
    },
    [],
  );
}

/** The running effect to pre-select for a group: defined only when the group has
 * common effects AND every member is currently running the same one. */
export function commonEffect(lights: LightLike[]): string | undefined {
  if (lights.length === 0 || commonLightEffects(lights).length === 0) return undefined;
  const first = lights[0].last_state?.effect;
  return lights.every((l) => l.last_state?.effect === first) ? first : undefined;
}

/** Everything an aggregate light control (room card, Boards group, Floor-Plan
 * room editor) needs to render itself from its member lights — computed one way,
 * everywhere. `hex`/`brightness`/`mirek` describe the **lit** members (a fully-off
 * group reads 0% brightness, not a stale value); `effects`/`commonEffect` are the
 * group intersection; `show*` are the capability union (any member offering it). */
export interface AggregateLight {
  lit: LightLike[];
  anyLit: boolean;
  showColor: boolean;
  showWhite: boolean;
  showBrightness: boolean;
  hex: string;
  brightness: number;
  mirek: number;
  effects: string[];
  commonEffect?: string;
}

export function aggregateLightState(lights: LightLike[]): AggregateLight {
  const lit = lights.filter((l) => l.last_state?.on);
  const firstColor = lit.map((l) => l.last_state?.color).find((c) => !!c);
  return {
    lit,
    anyLit: lit.length > 0,
    showColor: lights.some((l) => l.capabilities.color_rgb),
    showWhite: lights.some((l) => l.capabilities.color_temperature),
    showBrightness: lights.some((l) => l.capabilities.dimmable),
    hex: firstColor
      ? rgbToHex(...xyToRgb(firstColor.x, firstColor.y, firstColor.brightness))
      : "#ffb84d",
    brightness: lit.length
      ? Math.round(lit.reduce((s, l) => s + (l.last_state?.brightness ?? 100), 0) / lit.length)
      : 0,
    mirek: lit.map((l) => l.last_state?.color_temp_mirek).find((m): m is number => m != null) ?? 366,
    effects: commonLightEffects(lights),
    commonEffect: commonEffect(lights),
  };
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
