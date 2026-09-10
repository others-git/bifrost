import { describe, expect, it } from "vitest";
import {
  aggregateLightState,
  commonEffect,
  commonLightEffects,
  isClearEffect,
  lightSupports,
  lightWrite,
  roomLightWrite,
  type LightLike,
} from "./lightControl";
import type { LightCapabilities } from "../api";

const caps = (over: Partial<LightCapabilities> = {}): LightCapabilities => ({
  dimmable: true,
  color_rgb: true,
  color_temperature: true,
  effects: [],
  ...over,
});

const light = (over: Partial<LightLike> = {}): LightLike => ({
  capabilities: caps(),
  last_state: { on: true },
  ...over,
});

// ── The room-state invariant ────────────────────────────────────────────────
//
// This is the rule the backend enforces and the UI must not violate: an
// explicit `on` is a POWER intent for the whole room (it fans out to switches
// and speakers), while an attribute change carrying NO `on` is cast onto lit
// lights only. Sending a stray `on: true` alongside a brightness drag would
// wake every off lamp in the room and power its speakers.
describe("roomLightWrite — the room cascade never carries power", () => {
  it("strips the implicit on from every attribute change", () => {
    for (const change of [
      { field: "brightness", brightness: 40 },
      { field: "color", hex: "#ff0000" },
      { field: "temp", mirek: 250 },
      { field: "effect", effect: "Flames" },
    ] as const) {
      const patch = roomLightWrite(change);
      expect(patch, `${change.field} must not carry power`).not.toHaveProperty("on");
    }
  });

  it("still carries the moved dimension", () => {
    expect(roomLightWrite({ field: "brightness", brightness: 40 })).toEqual({ brightness: 40 });
    expect(roomLightWrite({ field: "temp", mirek: 250 })).toEqual({ color_temp_mirek: 250 });
    expect(roomLightWrite({ field: "effect", effect: "Flames" })).toEqual({ effect: "Flames" });
  });

  it("sends only the dimension that moved — never a stale colour or effect", () => {
    expect(Object.keys(roomLightWrite({ field: "brightness", brightness: 40 }))).toEqual([
      "brightness",
    ]);
    expect(Object.keys(roomLightWrite({ field: "effect", effect: "Flames" }))).toEqual(["effect"]);
  });
});

describe("lightWrite — a single lamp's own edit does imply power", () => {
  it("carries on:true, because adjusting one light is a deliberate act on it", () => {
    expect(lightWrite({ field: "brightness", brightness: 40 })).toEqual({
      on: true,
      brightness: 40,
    });
  });

  it("never mixes two mode dimensions in one write", () => {
    // Colour / temp / effect are mutually exclusive modes server-side; a write
    // naming two would be resolved as one and silently drop the other.
    const modes = ["color", "color_temp_mirek", "effect"];
    for (const change of [
      { field: "color", hex: "#ff0000" },
      { field: "temp", mirek: 250 },
      { field: "effect", effect: "Flames" },
    ] as const) {
      const named = modes.filter((m) => m in lightWrite(change));
      expect(named, `${change.field} named ${named.join("+")}`).toHaveLength(1);
    }
  });
});

describe("isClearEffect", () => {
  it("treats every provider's clear token as 'no effect'", () => {
    for (const t of ["", "no_effect", "off", "none", "  No_Effect  ", "OFF"]) {
      expect(isClearEffect(t), t).toBe(true);
    }
    expect(isClearEffect("Flames")).toBe(false);
  });
});

describe("lightSupports — an aggregate fans out only to capable members", () => {
  it("skips a non-dimmable light for brightness", () => {
    expect(lightSupports({ field: "brightness", brightness: 40 }, caps({ dimmable: false }))).toBe(
      false,
    );
    expect(lightSupports({ field: "brightness", brightness: 40 }, caps())).toBe(true);
  });

  it("skips a light whose catalog lacks the effect", () => {
    expect(lightSupports({ field: "effect", effect: "Flames" }, caps({ effects: [] }))).toBe(false);
    expect(
      lightSupports({ field: "effect", effect: "Flames" }, caps({ effects: ["Flames"] })),
    ).toBe(true);
  });
});

describe("commonLightEffects", () => {
  it("is the intersection across members", () => {
    expect(
      commonLightEffects([
        light({ capabilities: caps({ effects: ["A", "B", "C"] }) }),
        light({ capabilities: caps({ effects: ["B", "C", "D"] }) }),
      ]),
    ).toEqual(["B", "C"]);
  });

  it("is empty when any member offers none, so the group hides its effects UI", () => {
    expect(
      commonLightEffects([
        light({ capabilities: caps({ effects: ["A"] }) }),
        light({ capabilities: caps({ effects: [] }) }),
      ]),
    ).toEqual([]);
  });
});

describe("commonEffect", () => {
  it("is defined only when every member runs the same one", () => {
    const withFx = (effect?: string) =>
      light({ capabilities: caps({ effects: ["A", "B"] }), last_state: { on: true, effect } });
    expect(commonEffect([withFx("A"), withFx("A")])).toBe("A");
    expect(commonEffect([withFx("A"), withFx("B")])).toBeUndefined();
  });
});

describe("aggregateLightState", () => {
  it("reads 0% for a fully-off group rather than a stale value", () => {
    const agg = aggregateLightState([
      light({ last_state: { on: false, brightness: 80 } }),
      light({ last_state: { on: false, brightness: 60 } }),
    ]);
    expect(agg.anyLit).toBe(false);
    expect(agg.brightness).toBe(0);
  });

  it("averages the LIT members only", () => {
    const agg = aggregateLightState([
      light({ last_state: { on: true, brightness: 80 } }),
      light({ last_state: { on: true, brightness: 60 } }),
      light({ last_state: { on: false, brightness: 0 } }),
    ]);
    expect(agg.brightness).toBe(70);
  });

  it("takes capabilities as the union so any member offering a control shows it", () => {
    const agg = aggregateLightState([
      light({ capabilities: caps({ color_rgb: false, dimmable: false }) }),
      light({ capabilities: caps({ color_rgb: true, dimmable: false }) }),
    ]);
    expect(agg.showColor).toBe(true);
    expect(agg.showBrightness).toBe(false);
  });
});
