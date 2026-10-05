// The hub's clock, on the client.
//
// Every time-of-day readout (the board clock, the day/night sky, the "now"
// marker on an hour plan, "fired 5m ago") shows HUB time: the hub's
// NTP-corrected clock in the hub's configured zone. The hour plans and timers
// those readouts sit next to run on the hub, so a tablet with a drifting
// clock, or a laptop in another zone, must not paint a different hour than
// the one the hub will act on.
//
// Skew is measured against `GET /api/time` (midpoint of the round trip) and
// re-measured every few minutes; until the first reply lands, readouts fall
// back to the device's own clock and zone.

import { useEffect, useState } from "react";
import { getHubTime, type HubTime } from "./api";

const RESYNC_MS = 5 * 60_000;

let skewMs = 0;
let zone: string | null = null;
let offsetMin: number | null = null;
let started = false;
const listeners = new Set<() => void>();

/** Adopt a `/api/time` reply measured between `sentAt` and `receivedAt`
 * (device ms). Exported for tests. */
export function adoptHubTime(t: HubTime, sentAt: number, receivedAt: number) {
  skewMs = t.now_ms - (sentAt + receivedAt) / 2;
  zone = t.timezone && zoneSupported(t.timezone) ? t.timezone : null;
  offsetMin = t.utc_offset_min;
  listeners.forEach((l) => l());
}

/** Re-measure hub time now (after a zone change, say). */
export async function syncHubClock() {
  const sentAt = Date.now();
  const t = await getHubTime().catch(() => null);
  if (t) adoptHubTime(t, sentAt, Date.now());
}

function ensureStarted() {
  if (started) return;
  started = true;
  void syncHubClock();
  setInterval(() => void syncHubClock(), RESYNC_MS);
}

function zoneSupported(tz: string): boolean {
  try {
    new Intl.DateTimeFormat("en-US", { timeZone: tz });
    return true;
  } catch {
    return false;
  }
}

/** Hub "now", in epoch ms. */
export function hubNow(): number {
  ensureStarted();
  return Date.now() + skewMs;
}

/** How an instant renders in hub time: a zone Intl understands, or — for a
 * hub with no named zone — the instant shifted by its UTC offset and read as
 * UTC. `null` offset = not synced yet: the device's own zone. */
export function hubRender(
  ms: number,
  z: string | null = zone,
  off: number | null = offsetMin,
): { date: Date; timeZone: string | undefined } {
  if (z) return { date: new Date(ms), timeZone: z };
  if (off != null) return { date: new Date(ms + off * 60_000), timeZone: "UTC" };
  return { date: new Date(ms), timeZone: undefined };
}

/** Hour (0–23) and minute of an instant in hub time. */
export function hubHourMinute(
  ms: number = hubNow(),
  z: string | null = zone,
  off: number | null = offsetMin,
): { hour: number; minute: number } {
  const { date, timeZone } = hubRender(ms, z, off);
  const parts = new Intl.DateTimeFormat("en-US", {
    timeZone,
    hour: "numeric",
    minute: "numeric",
    hourCycle: "h23",
  }).formatToParts(date);
  const num = (type: string) => Number(parts.find((p) => p.type === type)?.value ?? 0);
  return { hour: num("hour") % 24, minute: num("minute") };
}

/** `toLocaleTimeString` in hub time. */
export function formatHubTime(ms: number, opts: Intl.DateTimeFormatOptions = {}): string {
  const { date, timeZone } = hubRender(ms);
  return date.toLocaleTimeString(undefined, { ...opts, timeZone });
}

/** `toLocaleDateString` in hub time. */
export function formatHubDate(ms: number, opts: Intl.DateTimeFormatOptions = {}): string {
  const { date, timeZone } = hubRender(ms);
  return date.toLocaleDateString(undefined, { ...opts, timeZone });
}

/** `toLocaleString` in hub time. */
export function formatHubDateTime(ms: number, opts: Intl.DateTimeFormatOptions = {}): string {
  const { date, timeZone } = hubRender(ms);
  return date.toLocaleString(undefined, { ...opts, timeZone });
}

/** Hub now, re-rendered every `tickMs` and whenever a sync moves the clock. */
export function useHubNow(tickMs: number): number {
  const [now, setNow] = useState(hubNow);
  useEffect(() => {
    const update = () => setNow(hubNow());
    listeners.add(update);
    const t = setInterval(update, tickMs);
    return () => {
      listeners.delete(update);
      clearInterval(t);
    };
  }, [tickMs]);
  return now;
}
