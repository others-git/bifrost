import { describe, expect, it } from "vitest";
import { adoptHubTime, hubHourMinute, hubNow, hubRender } from "./hubClock";

// 2026-01-15T03:30:00Z
const MS = Date.UTC(2026, 0, 15, 3, 30, 0);

describe("hubClock", () => {
  it("reads the hour in the hub's named zone, not the device's", () => {
    expect(hubHourMinute(MS, "America/New_York", -300)).toEqual({ hour: 22, minute: 30 });
    expect(hubHourMinute(MS, "Asia/Kolkata", 330)).toEqual({ hour: 9, minute: 0 });
  });

  it("falls back to the UTC offset when the hub zone has no name", () => {
    expect(hubHourMinute(MS, null, 120)).toEqual({ hour: 5, minute: 30 });
    const r = hubRender(MS, null, 120);
    expect(r.timeZone).toBe("UTC");
    expect(r.date.getTime()).toBe(MS + 120 * 60_000);
  });

  it("measures skew against the round-trip midpoint", () => {
    const sent = Date.now();
    // Hub is 90s ahead of this device; reply took 200ms.
    adoptHubTime({ now_ms: sent + 100 + 90_000, timezone: "UTC", utc_offset_min: 0 }, sent, sent + 200);
    const skew = hubNow() - Date.now();
    expect(skew).toBeGreaterThan(89_000);
    expect(skew).toBeLessThan(91_000);
  });
});
