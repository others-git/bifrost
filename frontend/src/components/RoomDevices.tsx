// Unified room membership editor — the Members section of a room's config.
// Groups every kind of member a Bifrost Room can hold: synced provider
// rooms/areas (links), lights, sensors, and power devices. Membership sections
// are driven by a generic helper so adding a future device class is a few
// lines, not a new editor. One Save commits all of it. (Audio membership +
// per-room offsets live in the Audio section — volume calibration needs its
// own room.)
//
// Mobile: sections stack; each device list scrolls within a capped height so a
// room with many devices stays manageable on a small screen.

import { useMemo, useState, type ReactNode } from "react";
import {
  setRoomDirectLights,
  setRoomLinks,
  setRoomPowerDevices,
  setRoomSensors,
  sensorReadingText,
  type Light,
  type PowerDevice,
  type ProviderGroupInfo,
  type Room,
  type SensorDevice,
} from "../api";
import { color } from "../theme";
import { SelectRow } from "./SelectRow";
import { Button } from "./controls";
import { Glyph } from "./glyphs";
import { PRESENCE_ACCENT } from "./RoomPresence";

function toggle(set: Set<string>, id: string): Set<string> {
  const next = new Set(set);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next;
}

/** A titled group of selectable rows. The generic building block for the
 * membership sections (links, lights, sensors, power). */
export function Section({
  title,
  hint,
  empty,
  isEmpty,
  children,
}: {
  title: string;
  hint?: string;
  empty?: string;
  isEmpty?: boolean;
  children: ReactNode;
}) {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.3rem" }}>
      <span style={{ fontSize: "0.8rem", color: "var(--bf-dim)", fontWeight: 600 }}>
        {title}
        {hint && <span style={{ color: "var(--bf-faint)", fontWeight: 400 }}> {hint}</span>}
      </span>
      {isEmpty ? (
        <span style={{ fontSize: "0.78rem", color: "var(--bf-faint)", padding: "0.2rem 0" }}>{empty}</span>
      ) : (
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: "0.25rem",
            maxHeight: 240,
            overflowY: "auto",
          }}
        >
          {children}
        </div>
      )}
    </div>
  );
}

export function RoomDevicesPanel({
  room,
  lights,
  powerDevices,
  sensors,
  providerGroups,
  onSaved,
}: {
  room: Room;
  lights: Light[];
  powerDevices: PowerDevice[];
  sensors: SensorDevice[];
  providerGroups: ProviderGroupInfo[];
  onSaved: () => void;
}) {
  const [links, setLinks] = useState<Set<string>>(
    () => new Set(room.links.map((l) => l.provider_group_id)),
  );
  const [directLights, setDirectLights] = useState<Set<string>>(
    () => new Set(room.direct_light_ids),
  );
  const [power, setPower] = useState<Set<string>>(() => new Set(room.power_device_ids));
  const [directSensors, setDirectSensors] = useState<Set<string>>(
    () => new Set(room.direct_sensor_ids),
  );
  const [saving, setSaving] = useState(false);

  // Members already covered by a selected link — shown, but locked.
  const linkedLightIds = useMemo(
    () =>
      new Set(
        providerGroups.filter((pg) => links.has(pg.id)).flatMap((pg) => pg.light_ids),
      ),
    [providerGroups, links],
  );
  const linkedSensorIds = useMemo(
    () =>
      new Set(
        providerGroups
          .filter((pg) => links.has(pg.id))
          .flatMap((pg) => pg.sensor_device_ids),
      ),
    [providerGroups, links],
  );

  async function save() {
    setSaving(true);
    try {
      await setRoomLinks(room.id, [...links]);
      await setRoomDirectLights(room.id, [...directLights]);
      await setRoomPowerDevices(room.id, [...power]);
      await setRoomSensors(room.id, [...directSensors]);
      onSaved();
    } finally {
      setSaving(false);
    }
  }

  function groupSummary(pg: ProviderGroupInfo): string {
    const parts: string[] = [];
    if (pg.light_ids.length) parts.push(`${pg.light_ids.length} light${pg.light_ids.length !== 1 ? "s" : ""}`);
    if (pg.media_device_ids.length) parts.push(`${pg.media_device_ids.length} speaker${pg.media_device_ids.length !== 1 ? "s" : ""}`);
    if (pg.power_device_ids.length) parts.push(`${pg.power_device_ids.length} power`);
    if (pg.sensor_device_ids.length) parts.push(`${pg.sensor_device_ids.length} sensor${pg.sensor_device_ids.length !== 1 ? "s" : ""}`);
    return parts.join(" · ") || "empty";
  }

  const selectableSensors = sensors.filter(
    (s) =>
      (s.enabled !== false && !s.shadowed_by) ||
      directSensors.has(s.id) ||
      linkedSensorIds.has(s.id),
  );

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.9rem" }}>
      {providerGroups.length > 0 && (
        <Section title="Linked rooms / areas" hint="(membership syncs automatically)">
          {providerGroups.map((pg) => (
            <SelectRow
              key={pg.id}
              accent={pg.domain === "media" ? color.violet : undefined}
              checked={links.has(pg.id)}
              onToggle={() => setLinks((s) => toggle(s, pg.id))}
            >
              <span style={{ display: "inline-grid", placeItems: "center", verticalAlign: "-2px" }}>
                <Glyph name={pg.domain === "media" ? "speaker" : "link"} size={14} />
              </span>{" "}
              {pg.name}
              <span style={{ color: "var(--bf-faint)", fontSize: "0.75rem" }}>{groupSummary(pg)}</span>
            </SelectRow>
          ))}
        </Section>
      )}

      <Section title="Lights" empty="No lights discovered yet." isEmpty={lights.length === 0}>
        {lights.map((l) => {
          const viaLink = linkedLightIds.has(l.id);
          return (
            <SelectRow
              key={l.id}
              checked={viaLink || directLights.has(l.id)}
              disabled={viaLink}
              onToggle={() => setDirectLights((s) => toggle(s, l.id))}
            >
              {l.name}
              {viaLink && <span style={{ fontSize: "0.72rem", color: "var(--bf-faint)" }}>(via link)</span>}
            </SelectRow>
          );
        })}
      </Section>

      <Section
        title="Sensors"
        hint="(presence sensors feed the room's occupancy — tune which count under Presence)"
        empty="No sensors yet — a Hue motion sensor or HA binary_sensor appears here after Sync."
        isEmpty={selectableSensors.length === 0}
      >
        {selectableSensors.map((s) => {
          const viaLink = linkedSensorIds.has(s.id);
          return (
            <SelectRow
              key={s.id}
              accent={PRESENCE_ACCENT}
              checked={viaLink || directSensors.has(s.id)}
              disabled={viaLink}
              onToggle={() => setDirectSensors((prev) => toggle(prev, s.id))}
            >
              <span style={{ display: "inline-grid", placeItems: "center", verticalAlign: "-2px" }}>
                <Glyph name={s.glyph ?? s.kind} size={14} />
              </span>{" "}
              {s.name}
              <span style={{ color: "var(--bf-faint)", fontSize: "0.72rem" }}>
                {sensorReadingText(s)}
                {viaLink && " · via link"}
              </span>
            </SelectRow>
          );
        })}
      </Section>

      <Section
        title="Power"
        hint="(switches, plugs, fans)"
        empty="No power devices yet — add an integration and Sync."
        isEmpty={powerDevices.length === 0}
      >
        {powerDevices.map((p) => (
          <SelectRow key={p.id} checked={power.has(p.id)} onToggle={() => setPower((s) => toggle(s, p.id))}>
            {p.name}
            <span style={{ color: "var(--bf-faint)", fontSize: "0.72rem" }}>{p.kind}</span>
          </SelectRow>
        ))}
      </Section>

      <div style={{ display: "flex", gap: "0.5rem" }}>
        <Button variant="accent" onClick={save} disabled={saving}>
          {saving ? "Saving…" : "Save members"}
        </Button>
      </div>
    </div>
  );
}
