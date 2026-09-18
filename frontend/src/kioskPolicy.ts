// The kiosk app's display-policy report, rendered for the Clients view.
//
// This is the layer *under* the page and the stream: a wall tablet stuck behind
// the keyguard keeps checking in, keeps rendering and keeps streaming, and is
// still unusable until someone walks over and swipes it. Kept pure and apart
// from the page so the mapping is testable — a diagnostic that reads the fields
// wrong doesn't fail loudly, it just makes the hub confidently wrong about
// which tablet is broken.

import type { KioskDisplayPolicy } from "./api";

/** One line of policy, or null when the app build reports none. Silence beats
 * a row of "off"s: an app that never sends this is not a kiosk that lost its
 * powers. */
export function policySummary(p: KioskDisplayPolicy | undefined | null): string | null {
  if (!p || p.device_owner == null) return null;
  const parts = [p.device_owner ? "device owner" : "NOT device owner"];
  if (p.lock_task != null) parts.push(p.lock_task ? "pinned" : "not pinned");
  if (p.keyguard_disabled != null) {
    parts.push(p.keyguard_disabled ? "keyguard off" : "keyguard ON (secure lock set?)");
  }
  if (p.keyguard_locked) parts.push("LOCKED NOW");
  return parts.join(" · ");
}

/** Is the policy bad enough to say so in colour? Each of these means a wake can
 * land on a lock screen instead of the dashboard. Unknown (null) is never
 * alarming — only a definite answer is. */
export function policyAlarming(p: KioskDisplayPolicy | undefined | null): boolean {
  if (!p) return false;
  return p.device_owner === false || p.keyguard_disabled === false || p.keyguard_locked === true;
}
