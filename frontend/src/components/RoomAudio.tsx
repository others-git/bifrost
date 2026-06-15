// Per-room audio controls, split into a live volume/mute strip (used on the
// Floor Plan and the Rooms page) and a membership + offset config editor (Rooms
// page). Volume/mute fans out to every audio device in the room; each device's
// per-room offset is applied server-side.

import { useEffect, useRef, useState } from "react";
import {
  setRoomAudioDevices,
  setRoomAudioState,
  type AudioDevice,
  type Room,
  type RoomAudioMember,
} from "../api";
import { Button } from "./controls";
import { alpha } from "../theme";

const ACCENT = "#a78bfa";

function members(room: Room, devices: AudioDevice[]) {
  const resolved = room.audio_devices
    .map((m) => ({ m, dev: devices.find((d) => d.id === m.audio_device_id) }))
    .filter((x): x is { m: RoomAudioMember; dev: AudioDevice } => !!x.dev);
  // M22: a receiver that is another member's volume-target is driven through the
  // bound source, so don't count it as its own room volume target (avoids a
  // double-apply / inflated speaker count).
  const boundTargets = new Set(
    resolved.map((x) => x.dev.receiver_id).filter((id): id is string => !!id),
  );
  return resolved.filter((x) => !boundTargets.has(x.dev.id));
}

/** Live room volume + mute — fans out to all the room's audio members. */
export function RoomVolumeStrip({ room, devices }: { room: Room; devices: AudioDevice[] }) {
  const mem = members(room, devices);
  // Room level seeds from the first member's level minus its offset (a "room
  // level" proxy, since room volume isn't persisted); muted if all are muted.
  const seedVol = mem.length
    ? Math.max(0, Math.min(100, mem[0].dev.state.volume - mem[0].m.volume_offset))
    : 0;
  const seedMute = mem.length > 0 && mem.every((x) => x.dev.state.mute);
  const [volume, setVolume] = useState(seedVol);
  const [mute, setMute] = useState(seedMute);
  const volumeTimer = useRef<ReturnType<typeof setTimeout>>(undefined);

  useEffect(() => {
    setVolume(seedVol);
    setMute(seedMute);
  }, [room.id, mem.length]); // eslint-disable-line react-hooks/exhaustive-deps

  if (mem.length === 0) return null;

  function commitVolume(v: number) {
    setVolume(v);
    clearTimeout(volumeTimer.current);
    volumeTimer.current = setTimeout(() => setRoomAudioState(room.id, { volume: v }), 250);
  }
  function toggleMute() {
    setMute(!mute);
    setRoomAudioState(room.id, { mute: !mute });
  }

  return (
    <div style={{ display: "flex", alignItems: "center", gap: "0.45rem" }}>
      <span style={{ fontSize: "0.85rem", color: ACCENT }} title={`Room volume → ${mem.length} speaker${mem.length !== 1 ? "s" : ""}`}>
        ♪
      </span>
      <button
        onClick={toggleMute}
        title={mute ? "Unmute room" : "Mute room"}
        style={{ background: "none", border: "none", cursor: "pointer", padding: 0, fontSize: "0.8rem", opacity: mute ? 1 : 0.5 }}
      >
        {mute ? "🔇" : "🔊"}
      </button>
      <input
        type="range"
        min={0}
        max={100}
        value={volume}
        onChange={(e) => commitVolume(Number(e.target.value))}
        style={{ flex: 1, accentColor: ACCENT }}
      />
      <span style={{ fontSize: "0.68rem", color: "var(--bf-faint)", width: 22, textAlign: "right" }}>{volume}</span>
    </div>
  );
}

/** Choose which audio devices belong to the room and calibrate each one's
 * per-room volume offset. Saves the explicit membership; synced audio-group
 * links contribute on top. */
export function RoomAudioEditor({
  room,
  devices,
  onChanged,
}: {
  room: Room;
  devices: AudioDevice[];
  onChanged: () => void;
}) {
  // device id → offset (member) or undefined (not a member).
  const [draft, setDraft] = useState<Map<string, number | undefined>>(new Map());
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    const d = new Map<string, number | undefined>();
    for (const m of room.audio_devices) d.set(m.audio_device_id, m.volume_offset);
    setDraft(d);
  }, [room.id, room.audio_devices]);

  async function save() {
    setSaving(true);
    try {
      const list = [...draft.entries()]
        .filter(([, off]) => off !== undefined)
        .map(([id, off]) => ({ audio_device_id: id, volume_offset: off as number }));
      await setRoomAudioDevices(room.id, list);
      onChanged();
    } finally {
      setSaving(false);
    }
  }

  if (devices.length === 0) {
    return <span style={{ fontSize: "0.8rem", color: "var(--bf-faint)" }}>No audio devices discovered yet.</span>;
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.3rem" }}>
      {devices.map((d) => {
        const off = draft.get(d.id);
        const isMember = off !== undefined;
        return (
          <div
            key={d.id}
            style={{
              borderRadius: 8,
              border: `1px solid ${isMember ? ACCENT : "transparent"}`,
              background: isMember ? `${alpha(ACCENT, 0.12)}` : "rgba(255,255,255,0.02)",
              padding: "0.5rem 0.6rem",
              display: "flex",
              flexDirection: "column",
              gap: "0.45rem",
            }}
          >
            <label style={{ display: "flex", alignItems: "center", gap: "0.6rem", fontSize: "0.9rem", color: "var(--bf-dim)", cursor: "pointer", minHeight: 26 }}>
              <input
                type="checkbox"
                checked={isMember}
                onChange={() =>
                  setDraft((prev) => {
                    const n = new Map(prev);
                    n.set(d.id, isMember ? undefined : 0);
                    return n;
                  })
                }
                style={{ width: 18, height: 18, accentColor: ACCENT, flexShrink: 0, cursor: "pointer" }}
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
                  onChange={(e) => setDraft((prev) => new Map(prev).set(d.id, Number(e.target.value)))}
                  style={{ flex: 1, accentColor: ACCENT }}
                />
                <span style={{ fontSize: "0.72rem", color: "var(--bf-dim)", width: 34, textAlign: "right" }}>
                  {off! > 0 ? `+${off}` : off}%
                </span>
              </div>
            )}
          </div>
        );
      })}
      <Button variant="accent"
        onClick={save}
        disabled={saving} style={{ alignSelf: "flex-start", marginTop: "0.3rem", borderColor: ACCENT, color: ACCENT }}
      >
        {saving ? "Saving…" : "Save audio"}
      </Button>
    </div>
  );
}
