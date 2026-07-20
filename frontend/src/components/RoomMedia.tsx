// Per-room audio controls, split into a live volume/mute strip (used on the
// Floor Plan and the Rooms page) and a membership + offset config editor (Rooms
// page). Volume/mute fans out to every audio device in the room; each device's
// per-room offset is applied server-side.

import { useEffect, useMemo, useRef, useState } from "react";
import {
  setMediaState,
  setRoomMediaDevices,
  setRoomMediaState,
  type MediaDevice,
  type ProviderGroupInfo,
  type Room,
  type RoomMediaMember,
} from "../api";
import { Button } from "./controls";
import { Glyph } from "./glyphs";
import { alpha } from "../theme";
import { pickableMedia } from "../deviceSelectors";

const ACCENT = "#a78bfa";

function members(room: Room, devices: MediaDevice[]) {
  const resolved = room.media_devices
    .map((m) => ({ m, dev: devices.find((d) => d.id === m.media_device_id) }))
    .filter((x): x is { m: RoomMediaMember; dev: MediaDevice } => !!x.dev);
  // M22: a receiver that is another member's volume-target is driven through the
  // bound source, so don't count it as its own room volume target (avoids a
  // double-apply / inflated speaker count).
  const boundTargets = new Set(
    resolved.map((x) => x.dev.receiver_id).filter((id): id is string => !!id),
  );
  return resolved.filter((x) => !boundTargets.has(x.dev.id));
}

/** Live room volume + mute — fans out to all the room's audio members. */
export function RoomVolumeStrip({ room, devices }: { room: Room; devices: MediaDevice[] }) {
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
    volumeTimer.current = setTimeout(() => setRoomMediaState(room.id, { volume: v }), 250);
  }
  function toggleMute() {
    setMute(!mute);
    setRoomMediaState(room.id, { mute: !mute });
  }

  return (
    <div style={{ display: "flex", alignItems: "center", gap: "0.45rem" }}>
      <span style={{ color: ACCENT, display: "grid", placeItems: "center" }} title={`Room volume → ${mem.length} speaker${mem.length !== 1 ? "s" : ""}`}>
        <Glyph name="speaker_group" size={15} />
      </span>
      <button
        onClick={toggleMute}
        title={mute ? "Unmute room" : "Mute room"}
        style={{ background: "none", border: "none", cursor: "pointer", padding: 0, color: "inherit", display: "grid", placeItems: "center", opacity: mute ? 1 : 0.5 }}
      >
        <Glyph name={mute ? "mute" : "volume"} size={15} />
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

/** One member speaker's live volume — the shared per-device control plane
 * (`PUT /media/devices/{id}/state`), debounced like every other slider. The
 * calibration feedback loop: run the sweep below, watch/drag real levels here,
 * trim offsets until the room balances. */
function MemberLevel({ device }: { device: MediaDevice }) {
  const [level, setLevel] = useState(device.state.volume);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);

  useEffect(() => setLevel(device.state.volume), [device.id, device.state.volume]);

  function commit(v: number) {
    setLevel(v);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setMediaState(device.id, { volume: v }), 250);
  }

  return (
    <div style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
      <span style={{ fontSize: "0.72rem", color: "var(--bf-dim)", width: 40 }}>level</span>
      <input
        type="range"
        min={0}
        max={100}
        value={level}
        onChange={(e) => commit(Number(e.target.value))}
        style={{ flex: 1, accentColor: ACCENT }}
      />
      <span style={{ fontSize: "0.72rem", color: "var(--bf-dim)", width: 34, textAlign: "right" }}>
        {level}
      </span>
    </div>
  );
}

/** The room's Audio section: membership, per-room offsets, each member's LIVE
 * level, and the room sweep. Never assumes one speaker per room — every member
 * shows its own honest level; the sweep is a fan-out test action, not a "room
 * level" readout. Membership + offsets commit on Save; levels apply live. */
export function RoomAudioSection({
  room,
  devices,
  providerGroups,
  onSaved,
}: {
  room: Room;
  devices: MediaDevice[];
  providerGroups: ProviderGroupInfo[];
  onSaved: () => void;
}) {
  // device id → offset (explicit member) or undefined (not explicit).
  const [draft, setDraft] = useState<Map<string, number | undefined>>(new Map());
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    const d = new Map<string, number | undefined>();
    for (const m of room.media_devices) d.set(m.media_device_id, m.volume_offset);
    setDraft(d);
  }, [room.id, room.media_devices]);

  // Devices arriving via a synced audio-group link: members even without an
  // explicit row. Ticking one just sets its offset (an explicit row on top).
  const linkedIds = useMemo(() => {
    const linked = new Set(room.links.map((l) => l.provider_group_id));
    return new Set(
      providerGroups.filter((pg) => linked.has(pg.id)).flatMap((pg) => pg.media_device_ids),
    );
  }, [room.links, providerGroups]);

  async function save() {
    setSaving(true);
    try {
      const ok = new Set(pickableMedia(devices).map((d) => d.id));
      const list = [...draft.entries()]
        .filter(([id, off]) => off !== undefined && ok.has(id))
        .map(([id, off]) => ({ media_device_id: id, volume_offset: off as number }));
      await setRoomMediaDevices(room.id, list);
      onSaved();
    } finally {
      setSaving(false);
    }
  }

  // Offer only controllable devices — enabled, not shadowed or merged. A
  // disabled device is never a valid member; the backend save drops any stale
  // disabled member too, so this can't strand one (control already ignores it).
  const selectable = pickableMedia(devices);

  if (selectable.length === 0) {
    return <span style={{ fontSize: "0.8rem", color: "var(--bf-faint)" }}>No audio devices discovered yet.</span>;
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.3rem" }}>
      {selectable.map((d) => {
        const off = draft.get(d.id);
        const explicit = off !== undefined;
        const isMember = explicit || linkedIds.has(d.id);
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
            <label style={{ display: "flex", alignItems: "center", gap: "0.6rem", fontSize: "0.9rem", color: "var(--bf-dim)", cursor: "pointer", minHeight: 30 }}>
              <input
                type="checkbox"
                checked={explicit}
                onChange={() =>
                  setDraft((prev) => {
                    const n = new Map(prev);
                    n.set(d.id, explicit ? undefined : 0);
                    return n;
                  })
                }
                style={{ width: 18, height: 18, accentColor: ACCENT, flexShrink: 0, cursor: "pointer" }}
              />
              <span style={{ flex: 1, minWidth: 0 }}>{d.name}</span>
              {!explicit && isMember && (
                <span style={{ fontSize: "0.72rem", color: "var(--bf-faint)" }}>via link</span>
              )}
            </label>
            {isMember && <MemberLevel device={d} />}
            {explicit && (
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

      {room.media_devices.length > 0 && (
        <div style={{ display: "flex", flexDirection: "column", gap: "0.2rem", marginTop: "0.3rem" }}>
          <span style={{ fontSize: "0.72rem", color: "var(--bf-faint)" }}>
            Room sweep — one volume fanned to every member with its offset applied
          </span>
          <RoomVolumeStrip room={room} devices={devices} />
        </div>
      )}

      <Button variant="accent"
        onClick={save}
        disabled={saving} style={{ alignSelf: "flex-start", marginTop: "0.3rem", borderColor: ACCENT, color: ACCENT }}
      >
        {saving ? "Saving…" : "Save audio"}
      </Button>
    </div>
  );
}
