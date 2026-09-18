import { describe, expect, it } from "vitest";
import type { KioskDisplayPolicy } from "./api";
import { policyAlarming, policySummary } from "./kioskPolicy";

const policy = (p: Partial<KioskDisplayPolicy>): KioskDisplayPolicy => ({
  device_owner: null,
  lock_task: null,
  keyguard_disabled: null,
  keyguard_locked: null,
  ...p,
});

describe("the kiosk display-policy summary", () => {
  it("says nothing at all for an app build that reports nothing", () => {
    expect(policySummary(policy({}))).toBeNull();
    expect(policySummary(undefined)).toBeNull();
    // Unknown is not a fault — it must not paint an old tablet as broken.
    expect(policyAlarming(policy({}))).toBe(false);
  });

  it("reads quiet when the kiosk holds every power it needs", () => {
    const p = policy({ device_owner: true, lock_task: true, keyguard_disabled: true, keyguard_locked: false });
    expect(policySummary(p)).toBe("device owner · pinned · keyguard off");
    expect(policyAlarming(p)).toBe(false);
  });

  it("calls out a keyguard the policy could not disable", () => {
    const p = policy({ device_owner: true, lock_task: true, keyguard_disabled: false });
    expect(policySummary(p)).toContain("keyguard ON");
    expect(policyAlarming(p)).toBe(true);
  });

  it("calls out a lock screen that is up right now", () => {
    const p = policy({ device_owner: true, lock_task: true, keyguard_disabled: true, keyguard_locked: true });
    expect(policySummary(p)).toContain("LOCKED NOW");
    expect(policyAlarming(p)).toBe(true);
  });

  it("treats lost device ownership as the fault under all the others", () => {
    const p = policy({ device_owner: false, lock_task: false });
    expect(policySummary(p)).toBe("NOT device owner · not pinned");
    expect(policyAlarming(p)).toBe(true);
  });
});
