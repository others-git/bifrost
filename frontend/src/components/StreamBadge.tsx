// Dev-mode readout of the shared `/api/events` connection's health.
//
// A wall tablet is a locked-down WebView: no devtools, no console, no way to
// ask it anything. So when a board looks stale the single most useful thing is
// for the screen itself to say whether its event stream is alive — otherwise
// "the buttons stopped working" is indistinguishable from "the house is quiet",
// and the only evidence anyone can gather is a reload, which destroys it.
//
// Three states worth telling apart at a glance:
//   • live      — beating, events flowing. The stream is not your problem.
//   • quiet     — beating, but no device events for a long while. The
//                 connection is fine; the HUB isn't sending. Look server-side
//                 (`GET /api/dev/streams` names this client and its counters).
//   • silent    — the beat itself has stopped, or the EventSource is closed.
//                 This connection is dead; the watchdog should be reconnecting.
//
// Purely an indicator: `pointerEvents: none` so it can never swallow a tap
// meant for a control underneath it.

import { useEffect, useState } from "react";
import { color, font, radius, alpha, glow, space } from "../theme";
import { streamHealth, SILENCE_LIMIT_MS, type StreamHealth } from "../useEvents";

/** No device event for this long, while the beat still arrives, reads as the
 * hub having gone quiet. Generous: a still house genuinely sends nothing, so
 * this is "worth a look", not "broken". */
const QUIET_AFTER_MS = 10 * 60_000;

type Level = "live" | "quiet" | "silent";

function level(h: StreamHealth): Level {
  // readyState 1 === OPEN. Anything else (CONNECTING, CLOSED, or no object at
  // all) means we are not currently receiving.
  if (h.readyState !== 1 || h.silent) return "silent";
  if (h.sinceEvent !== null && h.sinceEvent > QUIET_AFTER_MS) return "quiet";
  return "live";
}

const TINT: Record<Level, string> = {
  live: color.good,
  quiet: color.gold,
  silent: color.rose,
};

/** "4s" / "12m" / "3h" — compact enough for a corner, exact enough to act on. */
function ago(ms: number | null): string {
  if (ms === null) return "—";
  const s = Math.floor(ms / 1000);
  if (s < 90) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 90) return `${m}m`;
  return `${Math.floor(m / 60)}h`;
}

export function StreamBadge() {
  const [health, setHealth] = useState<StreamHealth>(streamHealth);

  useEffect(() => {
    const t = setInterval(() => setHealth(streamHealth()), 1000);
    return () => clearInterval(t);
  }, []);

  const lvl = level(health);
  const tint = TINT[lvl];

  // The numbers that actually discriminate, in the order you'd read them:
  // last event, last beat, and how many times this page has had to reconnect
  // (a climbing count is its own finding — the stream keeps dying).
  const detail = `evt ${ago(health.sinceEvent)} · hb ${ago(health.sinceBeat)}${
    health.reconnects > 0 ? ` · rc ${health.reconnects}` : ""
  }`;

  return (
    <div
      // Not interactive: a diagnostic must never be able to eat a control tap.
      style={{
        position: "fixed",
        right: space.sm,
        bottom: space.sm,
        zIndex: 9999,
        pointerEvents: "none",
        display: "flex",
        alignItems: "center",
        gap: space.xs,
        padding: `${space.xs}px ${space.sm}px`,
        borderRadius: radius.pill,
        background: alpha(color.void, 0.82),
        border: `1px solid ${alpha(tint, 0.45)}`,
        boxShadow: lvl === "live" ? "none" : glow(tint, 14),
        color: color.dim,
        fontFamily: font.body,
        fontSize: "0.62rem",
        letterSpacing: "0.04em",
        whiteSpace: "nowrap",
      }}
      title={`SSE ${lvl} — readyState ${health.readyState}, open ${ago(health.openFor)}, silence limit ${
        SILENCE_LIMIT_MS / 1000
      }s`}
    >
      <span
        style={{
          width: 7,
          height: 7,
          borderRadius: radius.pill,
          background: tint,
          boxShadow: glow(tint, 10),
          flex: "none",
        }}
      />
      <span>{detail}</span>
    </div>
  );
}
