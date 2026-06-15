// Unified room device configuration. One panel — instead of separate
// lights/audio editors — that groups every kind of member a Bifrost Room can
// hold: synced provider rooms/areas (links), lights, speakers (with per-room
// volume offset), and power devices. Membership sections are driven by a
// generic helper so adding a future device class is a few lines, not a new
// editor. One Save commits all of it.
//
// Mobile: sections stack; each device list scrolls within a capped height so a
// room with many devices stays manageable on a small screen.

import { useMemo, useState, type ReactNode } from "react";
import {
  setRoomAudioDevices,
  setRoomDirectLights,
  setRoomLinks,
  setRoomPowerDevices,
  type AudioDevice,
  type Light,
  type PowerDevice,
  type ProviderGroupInfo,
  type Room,
} from "../api";
import { SelectRow } from "./SelectRow";
import { S } from "../styles";

// Lights/power rows use SelectRow's default sky accent; speakers use violet to
// match the Audio page.
const AUDIO_ACCENT = "#a78bfa";

function toggle(set: Set<string>, id: string): Set<string> {
  const next = new Set(set);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next;
}

/** A titled group of selectable rows. The generic building block for the
 * membership sections (links, lights, power); audio is specialised below. */
function Section({
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
  audioDevices,
  powerDevices,
  providerGroups,
  onSaved,
  onCancel,
}: {
  room: Room;
  lights: Light[];
  audioDevices: AudioDevice[];
  powerDevices: PowerDevice[];
  providerGroups: ProviderGroupInfo[];
  onSaved: () => void;
  onCancel: () => void;
}) {
  const [links, setLinks] = useState<Set<string>>(
    () => new Set(room.links.map((l) => l.provider_group_id)),
  );
  const [directLights, setDirectLights] = useState<Set<string>>(
    () => new Set(room.direct_light_ids),
  );
  const [power, setPower] = useState<Set<string>>(() => new Set(room.power_device_ids));
  // audio device id → per-room offset (members only).
  const [audio, setAudio] = useState<Map<string, number>>(
    () => new Map(room.audio_devices.map((m) => [m.audio_device_id, m.volume_offset])),
  );
  const [saving, setSaving] = useState(false);

  // Lights already covered by a selected link — shown, but locked.
  const linkedLightIds = useMemo(
    () =>
      new Set(
        providerGroups.filter((pg) => links.has(pg.id)).flatMap((pg) => pg.light_ids),
      ),
    [providerGroups, links],
  );

  async function save() {
    setSaving(true);
    try {
      await setRoomLinks(room.id, [...links]);
      await setRoomDirectLights(room.id, [...directLights]);
      await setRoomPowerDevices(room.id, [...power]);
      await setRoomAudioDevices(
        room.id,
        [...audio.entries()].map(([id, off]) => ({ audio_device_id: id, volume_offset: off })),
      );
      onSaved();
    } finally {
      setSaving(false);
    }
  }

  function groupSummary(pg: ProviderGroupInfo): string {
    const parts: string[] = [];
    if (pg.light_ids.length) parts.push(`${pg.light_ids.length} light${pg.light_ids.length !== 1 ? "s" : ""}`);
    if (pg.audio_device_ids.length) parts.push(`${pg.audio_device_ids.length} speaker${pg.audio_device_ids.length !== 1 ? "s" : ""}`);
    if (pg.power_device_ids.length) parts.push(`${pg.power_device_ids.length} power`);
    return parts.join(" · ") || "empty";
  }

  return (
    <div
      style={{
        borderTop: "1px solid var(--bf-surfaceHi)",
        paddingTop: "0.7rem",
        marginTop: "0.2rem",
        display: "flex",
        flexDirection: "column",
        gap: "0.9rem",
      }}
    >
      {providerGroups.length > 0 && (
        <Section title="Linked rooms / areas" hint="(membership syncs automatically)">
          {providerGroups.map((pg) => (
            <SelectRow
              key={pg.id}
              accent={pg.domain === "audio" ? AUDIO_ACCENT : undefined}
              checked={links.has(pg.id)}
              onToggle={() => setLinks((s) => toggle(s, pg.id))}
            >
              {pg.domain === "audio" ? "♪" : "⇄"} {pg.name}
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
        title="Speakers"
        hint="(per-room volume offset)"
        empty="No audio devices discovered yet."
        isEmpty={audioDevices.length === 0}
      >
        {audioDevices.map((d) => {
          const off = audio.get(d.id);
          const isMember = off !== undefined;
          return (
            <div
              key={d.id}
              style={{
                borderRadius: 8,
                border: `1px solid ${isMember ? AUDIO_ACCENT : "transparent"}`,
                background: isMember ? `${AUDIO_ACCENT}1f` : "rgba(255,255,255,0.02)",
                padding: "0.4rem 0.6rem",
                display: "flex",
                flexDirection: "column",
                gap: "0.4rem",
              }}
            >
              <label style={{ display: "flex", alignItems: "center", gap: "0.6rem", fontSize: "0.9rem", color: "var(--bf-dim)", cursor: "pointer", minHeight: 30 }}>
                <input
                  type="checkbox"
                  checked={isMember}
                  onChange={() =>
                    setAudio((prev) => {
                      const n = new Map(prev);
                      if (isMember) n.delete(d.id);
                      else n.set(d.id, 0);
                      return n;
                    })
                  }
                  style={{ width: 18, height: 18, accentColor: AUDIO_ACCENT, flexShrink: 0, cursor: "pointer" }}
                />
                <span style={{ flex: 1, minWidth: 0 }}>{d.name}</span>
              </label>
              {isMember && (
                <div style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
                  <span style={{ fontSize: "0.72rem", color: "var(--bf-dim)", width: 40 }}>offset</span>
                  <input
                    type="range"
                    min={-50}
                    max={50}
                    value={off}
                    onChange={(e) => setAudio((prev) => new Map(prev).set(d.id, Number(e.target.value)))}
                    style={{ flex: 1, accentColor: AUDIO_ACCENT }}
                  />
                  <span style={{ fontSize: "0.72rem", color: "var(--bf-dim)", width: 34, textAlign: "right" }}>
                    {off! > 0 ? `+${off}` : off}%
                  </span>
                </div>
              )}
            </div>
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

      <div style={{ display: "flex", gap: "0.5rem", position: "sticky", bottom: 0 }}>
        <button onClick={save} disabled={saving} style={S.buttonAccent}>
          {saving ? "Saving…" : "Save"}
        </button>
        <button onClick={onCancel} style={S.buttonGhost}>
          Cancel
        </button>
      </div>
    </div>
  );
}
