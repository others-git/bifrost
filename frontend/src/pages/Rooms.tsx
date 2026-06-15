// Rooms — the per-room configuration hub. A room combines synced provider
// rooms/zones (links) with directly assigned lights, plus the audio devices it
// controls (membership + per-room volume offsets). Live room control (on/off,
// color, scenes, quick volume) lives on the Dashboard / Floor Plan.

import { useEffect, useState } from "react";
import {
  createRoom,
  getAudioDevices,
  getLights,
  getPowerDevices,
  getProviderGroups,
  getRooms,
  mergeRooms,
  removeRoom,
  setRoomEnabled,
  type AudioDevice,
  type Light,
  type PowerDevice,
  type ProviderGroupInfo,
  type Room,
} from "../api";
import { RoomVolumeStrip } from "../components/RoomAudio";
import { RoomDevicesPanel } from "../components/RoomDevices";
import { SelectRow } from "../components/SelectRow";
import { useDialogs } from "../components/dialogs";
import { PageHeader } from "../components/PageHeader";
import { useViewport } from "../useViewport";
import { ACCENT, S } from "../styles";

export function RoomsPage() {
  const { isMobile } = useViewport();
  const dialogs = useDialogs();
  const [rooms, setRooms] = useState<Room[]>([]);
  const [providerGroups, setProviderGroups] = useState<ProviderGroupInfo[]>([]);
  const [lights, setLights] = useState<Light[]>([]);
  const [audioDevices, setAudioDevices] = useState<AudioDevice[]>([]);
  const [powerDevices, setPowerDevices] = useState<PowerDevice[]>([]);
  const [showAdd, setShowAdd] = useState(false);

  async function load() {
    setRooms(await getRooms());
    setProviderGroups(await getProviderGroups());
    setAudioDevices(await getAudioDevices());
    setPowerDevices(await getPowerDevices());
    const l = await getLights();
    if (l !== "unauthorized") setLights(l);
  }

  useEffect(() => {
    load();
  }, []);

  async function handleRemove(id: string, name: string) {
    const ok = await dialogs.confirm({
      title: "Delete room",
      message: `Delete room "${name}"? Its scenes and plan bindings go with it.`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!ok) return;
    await removeRoom(id);
    await load();
  }

  return (
    <div style={{ padding: isMobile ? "1rem 0.85rem" : "2rem", maxWidth: 760, margin: "0 auto" }}>
      <PageHeader
        title="Rooms"
        description={
          <>
            A room combines synced provider rooms/zones (links) with the devices it controls —
            lights, speakers, and power devices. Use the <strong>Devices</strong> button to configure
            membership, or <strong>Sync</strong> on a provider (Settings) to refresh links.
          </>
        }
      />

      <div style={{ display: "flex", flexDirection: "column", gap: "0.75rem" }}>
        {rooms.length === 0 && !showAdd && (
          <p style={{ color: "var(--bf-faint)", margin: 0 }}>
            No rooms yet. Sync a provider, paint one in the planner, or add one here.
          </p>
        )}
        {rooms.map((room) => (
          <RoomCard
            key={room.id}
            room={room}
            allRooms={rooms}
            lights={lights}
            providerGroups={providerGroups}
            audioDevices={audioDevices}
            powerDevices={powerDevices}
            onChanged={load}
            onRemove={() => handleRemove(room.id, room.name)}
          />
        ))}
      </div>

      {showAdd ? (
        <div style={{ ...S.card, border: "1px solid var(--bf-border)", marginTop: "1rem" }}>
          <h3 style={{ margin: "0 0 0.25rem", fontSize: "1rem", color: "var(--bf-dim)" }}>New room</h3>
          <RoomEditForm
            lights={lights}
            providerGroups={providerGroups}
            initialName=""
            initialDirect={[]}
            initialLinks={[]}
            submitLabel="Create"
            onSubmit={async (name, directIds, _linkIds) => {
              try {
                await createRoom(name, directIds);
              } catch (e) {
                await dialogs.alert({
                  title: "Could not create room",
                  message: e instanceof Error ? e.message : String(e),
                });
                return;
              }
              setShowAdd(false);
              await load();
            }}
            onCancel={() => setShowAdd(false)}
          />
        </div>
      ) : (
        <button onClick={() => setShowAdd(true)} style={{ ...S.button, marginTop: "1rem" }}>
          + Add Room
        </button>
      )}
      {dialogs.element}
    </div>
  );
}

function RoomCard({
  room,
  allRooms,
  lights,
  providerGroups,
  audioDevices,
  powerDevices,
  onChanged,
  onRemove,
}: {
  room: Room;
  allRooms: Room[];
  lights: Light[];
  providerGroups: ProviderGroupInfo[];
  audioDevices: AudioDevice[];
  powerDevices: PowerDevice[];
  onChanged: () => Promise<void>;
  onRemove: () => void;
}) {
  const dialogs = useDialogs();
  const { isMobile } = useViewport();
  const [editingDevices, setEditingDevices] = useState(false);
  const [merging, setMerging] = useState(false);
  const mergeCandidates = allRooms.filter((r) => r.id !== room.id);
  const speakers = room.audio_devices.length;
  const powerCount = room.power_device_ids.length;

  async function handleMerge(targetId: string) {
    const target = mergeCandidates.find((r) => r.id === targetId);
    if (!target) return;
    const ok = await dialogs.confirm({
      title: "Merge rooms",
      message: `Merge "${room.name}" into "${target.name}"? "${room.name}"'s links, lights, scenes, and plan regions move there, then "${room.name}" is deleted.`,
      confirmLabel: "Merge",
    });
    if (!ok) return;
    setMerging(true);
    try {
      await mergeRooms(targetId, room.id);
      await onChanged();
    } catch (e) {
      await dialogs.alert({ title: "Merge failed", message: e instanceof Error ? e.message : String(e) });
    } finally {
      setMerging(false);
    }
  }

  const mergeSelect =
    mergeCandidates.length > 0 ? (
      <select
        value=""
        disabled={merging}
        onChange={(e) => { if (e.target.value) handleMerge(e.target.value); }}
        title="Merge this room into another (this room is deleted)"
        style={{ ...S.input, width: "auto", padding: "0.3rem 0.5rem", fontSize: "0.8rem", cursor: "pointer" }}
      >
        <option value="">{merging ? "Merging…" : "Merge into…"}</option>
        {mergeCandidates.map((r) => (
          <option key={r.id} value={r.id}>{r.name}</option>
        ))}
      </select>
    ) : null;

  const titleBlock = (
    <div style={{ minWidth: 0, flex: isMobile ? 1 : undefined }}>
      <div style={{ fontWeight: 600 }}>
        {room.name}
        {!room.enabled && (
          <span style={{ marginLeft: "0.5rem", color: "#a86", fontSize: "0.72rem", border: "1px solid #543", borderRadius: 4, padding: "0 0.35rem" }}>
            disabled
          </span>
        )}
      </div>
      <div style={{ color: "var(--bf-dim)", fontSize: "0.8rem", marginTop: "0.25rem", display: "flex", gap: "0.5rem", flexWrap: "wrap" }}>
        <span>{room.light_ids.length} light{room.light_ids.length !== 1 ? "s" : ""}</span>
        {speakers > 0 && <span>· {speakers} speaker{speakers !== 1 ? "s" : ""}</span>}
        {powerCount > 0 && <span>· {powerCount} power</span>}
        {room.links.map((l) => (
          <span
            key={l.provider_group_id}
            title={l.domain === "audio" ? "Synced audio room/zone" : "Synced provider room/zone"}
            style={{ border: "1px solid var(--bf-border)", borderRadius: 4, padding: "0 0.35rem", color: "#9a9", fontSize: "0.72rem" }}
          >
            {l.domain === "audio" ? "♪" : "⇄"} {l.name}
          </span>
        ))}
      </div>
    </div>
  );

  return (
    <div style={{ ...S.card, gap: "0.6rem", opacity: room.enabled ? 1 : 0.55 }}>
      <div
        style={{
          display: "flex",
          flexDirection: isMobile ? "column" : "row",
          alignItems: isMobile ? "stretch" : "center",
          justifyContent: "space-between",
          gap: isMobile ? "0.6rem" : "1rem",
        }}
      >
        {/* On mobile, the Merge select rides next to the title to free up the
            action row below. On desktop it lives with the other buttons. */}
        {isMobile ? (
          <div style={{ display: "flex", alignItems: "flex-start", justifyContent: "space-between", gap: "0.6rem" }}>
            {titleBlock}
            {mergeSelect}
          </div>
        ) : (
          titleBlock
        )}
        <div style={{ display: "flex", gap: "0.5rem", flexWrap: "wrap" }}>
          {!isMobile && mergeSelect}
          <button
            onClick={() => setEditingDevices((v) => !v)}
            style={{ ...S.buttonGhost, ...(editingDevices ? { borderColor: ACCENT, color: ACCENT } : {}) }}
          >
            Devices
          </button>
          <button
            onClick={async () => { await setRoomEnabled(room.id, !room.enabled); await onChanged(); }}
            title={room.enabled ? "Hide this room from the Dashboard and Floor Plan" : "Show this room again"}
            style={S.buttonGhost}
          >
            {room.enabled ? "Disable" : "Enable"}
          </button>
          <button onClick={onRemove} style={S.buttonDanger}>Remove</button>
        </div>
      </div>

      {/* Live room volume (fans out to all members). */}
      <RoomVolumeStrip room={room} devices={audioDevices} />

      {editingDevices && (
        <RoomDevicesPanel
          room={room}
          lights={lights}
          audioDevices={audioDevices}
          powerDevices={powerDevices}
          providerGroups={providerGroups}
          onSaved={() => { setEditingDevices(false); onChanged(); }}
          onCancel={() => setEditingDevices(false)}
        />
      )}
    </div>
  );
}

function RoomEditForm({
  lights,
  providerGroups,
  initialName,
  initialDirect,
  initialLinks,
  submitLabel,
  nameLocked,
  onSubmit,
  onCancel,
}: {
  lights: Light[];
  providerGroups: ProviderGroupInfo[];
  initialName: string;
  initialDirect: string[];
  initialLinks: string[];
  submitLabel: string;
  nameLocked?: boolean;
  onSubmit: (name: string, directIds: string[], linkIds: string[]) => Promise<void>;
  onCancel: () => void;
}) {
  const [name, setName] = useState(initialName);
  const [direct, setDirect] = useState<Set<string>>(new Set(initialDirect));
  const [links, setLinks] = useState<Set<string>>(new Set(initialLinks));
  const [saving, setSaving] = useState(false);

  function toggleSet(setter: React.Dispatch<React.SetStateAction<Set<string>>>, id: string) {
    setter((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  // Lights already covered by a selected link (shown, but as link members).
  const linkedLightIds = new Set(
    providerGroups.filter((pg) => links.has(pg.id)).flatMap((pg) => pg.light_ids),
  );

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setSaving(true);
    try {
      await onSubmit(name.trim(), [...direct], [...links]);
    } finally {
      setSaving(false);
    }
  }

  return (
    <form onSubmit={submit} style={{ display: "flex", flexDirection: "column", gap: "0.75rem" }}>
      {!nameLocked && (
        <label style={labelStyle}>
          <span>Name</span>
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. Living Room" style={S.input} required autoFocus />
        </label>
      )}

      {providerGroups.some((pg) => pg.domain === "light") && (
        <div style={{ display: "flex", flexDirection: "column", gap: "0.3rem" }}>
          <span style={{ fontSize: "0.875rem", color: "var(--bf-dim)" }}>
            Linked provider rooms <span style={{ color: "var(--bf-faint)" }}>(membership syncs automatically)</span>
          </span>
          {providerGroups
            .filter((pg) => pg.domain === "light")
            .map((pg) => (
              <SelectRow key={pg.id} checked={links.has(pg.id)} onToggle={() => toggleSet(setLinks, pg.id)}>
                ⇄ {pg.name}
                <span style={{ color: "var(--bf-faint)", fontSize: "0.75rem" }}>
                  {pg.light_ids.length} light{pg.light_ids.length !== 1 ? "s" : ""}
                </span>
              </SelectRow>
            ))}
        </div>
      )}

      {providerGroups.some((pg) => pg.domain === "audio") && (
        <div style={{ display: "flex", flexDirection: "column", gap: "0.3rem" }}>
          <span style={{ fontSize: "0.875rem", color: "var(--bf-dim)" }}>
            Linked audio rooms/zones <span style={{ color: "var(--bf-faint)" }}>(adds the room's speakers)</span>
          </span>
          {providerGroups
            .filter((pg) => pg.domain === "audio")
            .map((pg) => (
              <SelectRow key={pg.id} checked={links.has(pg.id)} onToggle={() => toggleSet(setLinks, pg.id)}>
                ♪ {pg.name}
                <span style={{ color: "var(--bf-faint)", fontSize: "0.75rem" }}>
                  {pg.audio_device_ids.length} device{pg.audio_device_ids.length !== 1 ? "s" : ""}
                </span>
              </SelectRow>
            ))}
        </div>
      )}

      <div style={{ display: "flex", flexDirection: "column", gap: "0.3rem" }}>
        <span style={{ fontSize: "0.875rem", color: "var(--bf-dim)" }}>Direct lights</span>
        {lights.map((l) => {
          const viaLink = linkedLightIds.has(l.id);
          return (
            <SelectRow
              key={l.id}
              checked={viaLink || direct.has(l.id)}
              disabled={viaLink}
              onToggle={() => toggleSet(setDirect, l.id)}
            >
              {l.name}
              {viaLink && <span style={{ fontSize: "0.72rem", color: "var(--bf-faint)" }}>(via link)</span>}
            </SelectRow>
          );
        })}
      </div>

      <div style={{ display: "flex", gap: "0.5rem" }}>
        <button type="submit" style={S.buttonAccent} disabled={saving}>
          {saving ? "Saving…" : submitLabel}
        </button>
        <button type="button" onClick={onCancel} style={S.buttonGhost}>
          Cancel
        </button>
      </div>
    </form>
  );
}

const labelStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: "0.3rem",
  fontSize: "0.875rem",
  color: "var(--bf-dim)",
};
