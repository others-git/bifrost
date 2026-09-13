// A kiosk page telling the hub how its own event stream is doing.
//
// A wall tablet fails in three layers, and the hub could only ever see two of
// them: the native app's check-in says the DEVICE is alive, and the subscriber
// registry says the STREAM is. Whether the PAGE in between is still running —
// and what it believes it is receiving — lived only in the corner badge on the
// tablet's own screen. That means walking to the tablet to read it, and a
// reload (the thing everyone tries first) erases the evidence.
//
// So the page posts the same counters the badge renders. Two properties make
// this worth its 60s tick:
//
//   • It authenticates with the `bfr_key` cookie, not the session — so it keeps
//     reporting through exactly the failure it exists to report. A
//     session-gated report would go silent in the same breath as the stream.
//   • The report's own age is a signal. A fresh report with a growing
//     `since_beat_ms` means the page is alive and its stream is refused or
//     dropped; a report that stops arriving while the app still checks in means
//     the WebView itself is frozen. Those look identical from the hub side
//     otherwise, and they need opposite fixes.

import { useEffect } from "react";
import { IS_KIOSK, reportKioskHealth } from "./api";
import { streamHealth } from "./useEvents";

/** When this page was loaded. A WebView up for weeks is the shape of every
 * failure that only a reload has ever fixed, so the hub should know. */
const PAGE_LOADED_AT = Date.now();

/** Slow on purpose: this is a diagnostic, not telemetry. Fast enough that a
 * kiosk looked at during a fault has a recent sample, slow enough to be
 * invisible next to the app's own 10s check-in. */
export const HEALTH_REPORT_INTERVAL_MS = 60_000;

/** The payload, from the live counters. Exported for the test — the mapping is
 * the part worth pinning, since a wrong field here is a diagnostic that lies. */
export function healthReport(now = Date.now()) {
  const h = streamHealth();
  return {
    since_event_ms: h.sinceEvent === null ? null : Math.round(h.sinceEvent),
    since_beat_ms: h.sinceBeat === null ? null : Math.round(h.sinceBeat),
    reconnects: h.reconnects,
    ready_state: h.readyState,
    page_age_ms: now - PAGE_LOADED_AT,
  };
}

/** Post this page's stream health to the hub on a slow tick, and whenever the
 * screen comes back — a wake is exactly when a wall tablet's stream is most
 * likely to have died, and when someone is most likely to be looking. */
export function useKioskHealthReport() {
  useEffect(() => {
    if (!IS_KIOSK) return;
    const post = () => void reportKioskHealth(healthReport());
    post();
    const timer = setInterval(post, HEALTH_REPORT_INTERVAL_MS);
    const onVisible = () => {
      if (document.visibilityState === "visible") post();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, []);
}
