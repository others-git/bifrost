import { describe, expect, it } from "vitest";
import { FEATURES } from "./features";

// A flag's whole job is to be the single authority for whether a surface ships.
// This pins the current intent, so switching one on is a deliberate edit with a
// failing test pointing at it rather than a silent drift.
describe("FEATURES", () => {
  it("keeps the Floor Plan switched off", () => {
    expect(FEATURES.floorPlan).toBe(false);
  });

  it("exposes every flag as a plain boolean", () => {
    for (const [name, value] of Object.entries(FEATURES)) {
      expect(typeof value, `${name} must be a boolean`).toBe("boolean");
    }
  });
});
