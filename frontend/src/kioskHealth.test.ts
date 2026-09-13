// The kiosk's self-report is a diagnostic, so the mapping is the part worth
// pinning: a field silently dropped or mis-scaled here doesn't fail loudly, it
// just makes the hub confidently wrong about which tablet is broken.

import { describe, expect, it, vi } from "vitest";
import type { StreamHealth } from "./useEvents";

const health: StreamHealth = {
  readyState: 0,
  sinceEvent: 900_123.4,
  sinceBeat: 900_456.7,
  openFor: null,
  reconnects: 13,
  silent: true,
};

vi.mock("./useEvents", () => ({
  streamHealth: () => health,
  SILENCE_LIMIT_MS: 70_000,
}));

const { healthReport } = await import("./kioskHealth");

describe("the kiosk health report", () => {
  it("carries the counters the badge shows, as whole ms", () => {
    const r = healthReport();
    expect(r.since_event_ms).toBe(900_123);
    expect(r.since_beat_ms).toBe(900_457); // rounded, not truncated
    expect(r.reconnects).toBe(13);
    // 0 = CONNECTING. Reported verbatim: "stuck connecting" and "closed" are
    // different faults, and collapsing them is what left the last one unnamed.
    expect(r.ready_state).toBe(0);
  });

  it("reports how long the page has been loaded", () => {
    // The shape of every failure only a reload has fixed — the hub can't see
    // it any other way, since a WebView never reconnects to announce itself.
    const r = healthReport(Date.now() + 5_000);
    expect(r.page_age_ms).toBeGreaterThanOrEqual(5_000);
    expect(r.page_age_ms).toBeLessThan(60_000);
  });

  it("passes a never-seen counter through as null rather than zero", () => {
    // Zero would read as "an event just arrived" — the opposite of the truth.
    health.sinceEvent = null;
    expect(healthReport().since_event_ms).toBeNull();
  });
});
