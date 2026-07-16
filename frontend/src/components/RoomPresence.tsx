// The room's Presence section: which of its sensors count toward occupancy,
// live readings, and what consumes the verdict (kiosk displays, automations).
//
// Model: every enabled presence member COUNTS by default — the toggles here are
// opt-outs (`room_presence_excluded`), so a sensor synced in later participates
// automatically. Toggles commit immediately (they're single switches, and the
// occupancy readout above them is the feedback loop). Environmental sensors
// (temperature / lux / contact / humidity) list read-only — they're room data,
// not presence inputs.

import { useState } from "react";
import {
  sensorReadingText,
  setRoomPresence,
  type Automation,
  type Kiosk,
  type Room,
  type SensorDevice,
} from "../api";
import { alpha, color } from "../theme";
import { Glyph } from "./glyphs";
import { SelectRow } from "./SelectRow";

/** Presence accent — the "live / detected" green shared with online badges. */
export const PRESENCE_ACCENT = color.good;

export function isPresenceKind(kind: SensorDevice["kind"]): boolean {
  return kind === "motion" || kind === "occupancy";
}

/** True when this automation reads the room's aggregate occupancy — as its
 * trigger or as a gate. */
function readsRoomOccupancy(a: Automation, roomId: string): boolean {
  if (a.trigger.kind === "room" && a.trigger.room_id === roomId) return true;
  return a.conditions.some((c) => c.kind === "room_is" && c.room_id === roomId);
}

export function RoomPresencePanel({
  room,
  sensors,
  kiosks,
  automations,
  onChanged,
}: {
  room: Room;
  sensors: SensorDevice[];
  kiosks: Kiosk[];
  automations: Automation[];
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState(false);

  const members = room.sensor_ids
    .map((id) => sensors.find((s) => s.id === id))
    .filter((s): s is SensorDevice => !!s);
  const presence = members.filter((s) => isPresenceKind(s.kind));
  const ambient = members.filter((s) => !isPresenceKind(s.kind));
  const excluded = new Set(room.presence_excluded);

  async function toggle(sensorId: string) {
    if (busy) return;
    setBusy(true);
    try {
      const next = new Set(excluded);
      if (next.has(sensorId)) next.delete(sensorId);
      else next.add(sensorId);
      await setRoomPresence(room.id, [...next]);
      onChanged();
    } finally {
      setBusy(false);
    }
  }

  // Consumers of this room's occupancy — the cross-links that answer "what
  // will change if I flip these toggles?".
  // A kiosk reads this room's presence when its display plan has any Aware
  // hour (or the legacy presence flag is still on, pre-plan).
  const drivenKiosks = kiosks.filter(
    (k) =>
      k.room_id === room.id &&
      (k.hour_modes ? k.schedule_enabled && k.hour_modes.includes("A") : k.presence_enabled),
  );
  const drivenRules = automations.filter((a) => readsRoomOccupancy(a, room.id));
  const consumers: string[] = [
    ...drivenKiosks.map((k) => `${k.name || "kiosk"} display`),
    ...(drivenRules.length
      ? [`${drivenRules.length} automation${drivenRules.length !== 1 ? "s" : ""}`]
      : []),
  ];

  const verdict =
    room.occupancy == null ? (
      <span style={{ color: color.faint }}>no presence input</span>
    ) : (
      <span
        style={{
          color: room.occupancy ? PRESENCE_ACCENT : color.dim,
          display: "inline-flex",
          alignItems: "center",
          gap: "0.3rem",
          textShadow: room.occupancy ? `0 0 10px ${alpha(PRESENCE_ACCENT, 0.6)}` : undefined,
        }}
      >
        <Glyph name="motion" size={13} />
        {room.occupancy ? "occupied" : "empty"}
      </span>
    );

  if (members.length === 0) {
    return (
      <p style={{ margin: 0, fontSize: "0.8rem", color: color.faint }}>
        No sensors in this room yet. Add one under Members, assign one on the Devices page, or
        Sync a provider whose room carries a motion sensor.
      </p>
    );
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.55rem" }}>
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          fontSize: "0.8rem",
          color: color.dim,
        }}
      >
        <span>Room reads:</span>
        {verdict}
      </div>

      {presence.length === 0 ? (
        <p style={{ margin: 0, fontSize: "0.8rem", color: color.faint }}>
          None of this room's sensors detect presence (motion / occupancy), so occupancy stays
          unknown here.
        </p>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: "0.25rem" }}>
          {presence.map((s) => {
            const counts = !excluded.has(s.id);
            const detecting = s.state.reading && "bool" in s.state.reading && s.state.reading.bool;
            return (
              <SelectRow
                key={s.id}
                accent={PRESENCE_ACCENT}
                checked={counts}
                onToggle={() => toggle(s.id)}
              >
                <span style={{ display: "inline-grid", placeItems: "center", verticalAlign: "-2px" }}>
                  <Glyph name={s.glyph ?? s.kind} size={14} />
                </span>{" "}
                {s.name}
                <span
                  style={{
                    marginLeft: "auto",
                    fontSize: "0.72rem",
                    color: detecting ? PRESENCE_ACCENT : color.faint,
                  }}
                >
                  {sensorReadingText(s)}
                  {!counts && " · doesn't count"}
                </span>
              </SelectRow>
            );
          })}
        </div>
      )}

      {ambient.length > 0 && (
        <div style={{ display: "flex", flexDirection: "column", gap: "0.15rem" }}>
          {ambient.map((s) => (
            <div
              key={s.id}
              style={{
                display: "flex",
                alignItems: "center",
                gap: "0.6rem",
                padding: "0.35rem 0.6rem",
                fontSize: "0.8rem",
                color: color.dim,
              }}
            >
              <span style={{ display: "inline-grid", placeItems: "center", color: color.faint }}>
                <Glyph name={s.glyph ?? s.kind} size={14} />
              </span>
              <span style={{ flex: 1, minWidth: 0 }}>{s.name}</span>
              <span style={{ color: color.faint }}>{sensorReadingText(s)}</span>
            </div>
          ))}
        </div>
      )}

      <div style={{ fontSize: "0.75rem", color: color.faint }}>
        {consumers.length > 0 ? (
          <>
            Drives: <span style={{ color: color.dim }}>{consumers.join(" · ")}</span>
          </>
        ) : (
          <>
            Nothing reads this room's presence yet — assign a kiosk to it (Settings → Clients)
            or trigger an automation on it.
          </>
        )}
      </div>
    </div>
  );
}
