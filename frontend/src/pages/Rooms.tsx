// Rooms — the per-room configuration hub. A room combines synced provider
// rooms/zones (links) with the devices it controls — lights, speakers, power,
// and sensors (whose presence kinds feed the room's occupancy). Live room
// control (on/off, color, scenes, quick volume) lives on the Dashboard/Boards.
//
// Each room renders as a "charter" card: an engraved name, its link chips, a
// domain census strip (one glyph niche per device domain, in the domain's
// accent), and a live occupancy rune. Tapping the header — or any census
// niche — expands ONE stacked editor: Members → Audio → Presence → Quick
// controls, with the rare/destructive operations (disable, merge, delete)
// quiet at the bottom instead of shouting on every card face.

import { useEffect, useRef, useState } from "react";
import {
  createRoom,
  getAutomations,
  getKiosks,
  getLights,
  getMediaDevices,
  getPowerDevices,
  getProviderGroups,
  getRooms,
  getScenes,
  getSensors,
  mergeRooms,
  removeRoom,
  setRoomEnabled,
  type Automation,
  type Kiosk,
  type Light,
  type MediaDevice,
  type PowerDevice,
  type ProviderGroupInfo,
  type Room,
  type Scene,
  type SensorDevice,
} from "../api";
import { RoomAudioSection } from "../components/RoomMedia";
import { RoomDevicesPanel } from "../components/RoomDevices";
import { RoomControlsPanel } from "../components/RoomControls";
import { PRESENCE_ACCENT, RoomPresencePanel } from "../components/RoomPresence";
import { Glyph } from "../components/glyphs";
import { SelectRow } from "../components/SelectRow";
import { Modal, useDialogs, type Dialogs } from "../components/dialogs";
import { PageHeader } from "../components/PageHeader";
import { Select } from "../components/Select";
import { useViewport } from "../useViewport";
import { useEvents } from "../useEvents";
import { S, pageShell, tileGrid } from "../styles";
import { alpha, color, font, nicheStyle, radius } from "../theme";
import { Button } from "../components/controls";
import { pickableLights } from "../deviceSelectors";

export function RoomsPage() {
  const { isMobile } = useViewport();
  const dialogs = useDialogs();
  const [rooms, setRooms] = useState<Room[]>([]);
  const [providerGroups, setProviderGroups] = useState<ProviderGroupInfo[]>([]);
  const [lights, setLights] = useState<Light[]>([]);
  const [mediaDevices, setMediaDevices] = useState<MediaDevice[]>([]);
  const [powerDevices, setPowerDevices] = useState<PowerDevice[]>([]);
  const [sensors, setSensors] = useState<SensorDevice[]>([]);
  const [scenes, setScenes] = useState<Scene[]>([]);
  const [kiosks, setKiosks] = useState<Kiosk[]>([]);
  const [automations, setAutomations] = useState<Automation[]>([]);
  const [showAdd, setShowAdd] = useState(false);

  async function load() {
    const [r, pg, md, pd, sd, sc, k, a, l] = await Promise.all([
      getRooms(),
      getProviderGroups(),
      getMediaDevices(),
      getPowerDevices(),
      getSensors(),
      getScenes(),
      getKiosks(),
      getAutomations(),
      getLights(),
    ]);
    setRooms(r);
    setProviderGroups(pg);
    setMediaDevices(md);
    setPowerDevices(pd);
    setSensors(sd);
    setScenes(sc);
    setKiosks(k);
    setAutomations(a);
    if (l !== "unauthorized") setLights(l);
  }

  useEffect(() => {
    load();
  }, []);

  // Live occupancy: a motion event should flip the rune (and the Presence
  // section's readings) without a reload. Debounced — a busy sensor evening
  // shouldn't hammer the rooms endpoint.
  const refreshTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(refreshTimer.current), []);
  const refresh = () => {
    clearTimeout(refreshTimer.current);
    refreshTimer.current = setTimeout(async () => {
      setRooms(await getRooms());
      setSensors(await getSensors());
    }, 400);
  };
  useEvents({ sensor_state: refresh, inventory: refresh });

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
    <div style={pageShell(isMobile)}>
      <PageHeader
        title="Rooms"
        description={
          <>
            A room combines synced provider rooms/zones (links) with the devices it controls —
            lights, speakers, power, and sensors. Tap a room to configure its members, audio
            calibration, <strong>presence</strong>, and quick controls; <strong>Sync</strong> a
            provider (Settings) to refresh links.
          </>
        }
      />

      <div style={tileGrid(340, isMobile)}>
        {rooms.length === 0 && !showAdd && (
          <p style={{ color: "var(--bf-faint)", margin: 0 }}>
            No rooms yet. Sync a provider, paint one in the planner, or add one here.
          </p>
        )}
        {rooms.map((room) => (
          <RoomCharter
            key={room.id}
            room={room}
            allRooms={rooms}
            lights={lights}
            providerGroups={providerGroups}
            mediaDevices={mediaDevices}
            powerDevices={powerDevices}
            sensors={sensors}
            scenes={scenes}
            kiosks={kiosks}
            automations={automations}
            dialogs={dialogs}
            onChanged={load}
            onRemove={() => handleRemove(room.id, room.name)}
          />
        ))}
      </div>

      {showAdd ? (
        <div style={{ ...S.card, border: "1px solid var(--bf-border)", marginTop: "1rem", maxWidth: 620 }}>
          <h3 style={{ margin: "0 0 0.25rem", fontSize: "1rem", color: "var(--bf-dim)" }}>New room</h3>
          <NewRoomForm
            lights={lights}
            onSubmit={async (name, directIds) => {
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
        <Button onClick={() => setShowAdd(true)} style={{ marginTop: "1rem" }}>
          + Add Room
        </Button>
      )}
      {dialogs.element}
    </div>
  );
}

// ── The charter card ─────────────────────────────────────────────────────────

type SectionKey = "members" | "audio" | "presence" | "controls";

/** One census niche: a domain glyph + count in the domain accent, lit while the
 * room holds any of that domain. Tapping opens the editor at the right section. */
function CensusNiche({
  glyph,
  count,
  accent,
  title,
  onOpen,
}: {
  glyph: string;
  count: number;
  accent: string;
  title: string;
  onOpen: () => void;
}) {
  return (
    <button
      onClick={onOpen}
      title={`${count} ${title} — configure`}
      style={{
        ...nicheStyle(accent, count > 0),
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        gap: 6,
        minWidth: 56,
        minHeight: 44,
        padding: "0 0.6rem",
        borderRadius: radius.sm,
        cursor: "pointer",
        fontFamily: font.display,
        fontWeight: 700,
        fontSize: "0.85rem",
      }}
    >
      <Glyph name={glyph} size={15} />
      {count}
    </button>
  );
}

/** Engraved section header inside the expanded editor. */
function SectionHeader({ children }: { children: React.ReactNode }) {
  return (
    <div
      style={{
        fontFamily: font.display,
        fontSize: "0.7rem",
        fontWeight: 700,
        letterSpacing: "0.16em",
        textTransform: "uppercase",
        color: color.textAccent,
        borderBottom: `1px solid ${color.hairline}`,
        paddingBottom: "0.3rem",
      }}
    >
      {children}
    </div>
  );
}

function RoomCharter({
  room,
  allRooms,
  lights,
  providerGroups,
  mediaDevices,
  powerDevices,
  sensors,
  scenes,
  kiosks,
  automations,
  dialogs,
  onChanged,
  onRemove,
}: {
  room: Room;
  allRooms: Room[];
  lights: Light[];
  providerGroups: ProviderGroupInfo[];
  mediaDevices: MediaDevice[];
  powerDevices: PowerDevice[];
  sensors: SensorDevice[];
  scenes: Scene[];
  kiosks: Kiosk[];
  automations: Automation[];
  // Owned by RoomsPage (which renders `dialogs.element`). A RoomCharter must
  // NOT call useDialogs() itself: that returns a separate instance whose
  // element is never mounted, so confirm()/alert() would hang forever.
  dialogs: Dialogs;
  onChanged: () => Promise<void>;
  onRemove: () => void;
}) {
  const { isMobile, isCompact } = useViewport();
  const [open, setOpen] = useState(false);
  const [merging, setMerging] = useState(false);
  const sectionRefs = useRef<Partial<Record<SectionKey, HTMLDivElement | null>>>({});
  const mergeCandidates = allRooms.filter((r) => r.id !== room.id);

  function openAt(key: SectionKey) {
    setOpen(true);
    // Wait a frame for the editor to mount before scrolling to the section.
    requestAnimationFrame(() =>
      setTimeout(
        () => sectionRefs.current[key]?.scrollIntoView({ behavior: "smooth", block: "start" }),
        60,
      ),
    );
  }

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

  const section = (key: SectionKey, title: string, body: React.ReactNode) => (
    <div
      ref={(el) => {
        sectionRefs.current[key] = el;
      }}
      style={{ display: "flex", flexDirection: "column", gap: "0.6rem", scrollMarginTop: 70 }}
    >
      <SectionHeader>{title}</SectionHeader>
      {body}
    </div>
  );

  // The editor body — identical sections whether it renders inline (compact)
  // or inside the desktop modal.
  const editorSections = (
    <>
      {section(
        "members",
        "Members",
        <RoomDevicesPanel
          room={room}
          lights={lights}
          powerDevices={powerDevices}
          sensors={sensors}
          providerGroups={providerGroups}
          onSaved={onChanged}
        />,
      )}
      {section(
        "audio",
        "Audio",
        <RoomAudioSection
          room={room}
          devices={mediaDevices}
          providerGroups={providerGroups}
          onSaved={onChanged}
        />,
      )}
      {section(
        "presence",
        "Presence",
        <RoomPresencePanel
          room={room}
          sensors={sensors}
          kiosks={kiosks}
          automations={automations}
          onChanged={onChanged}
        />,
      )}
      {section(
        "controls",
        "Quick controls",
        <RoomControlsPanel
          room={room}
          lights={lights}
          mediaDevices={mediaDevices}
          powerDevices={powerDevices}
          scenes={scenes}
          onSaved={onChanged}
          onCancel={() => setOpen(false)}
        />,
      )}

      {/* Rare + destructive operations, quiet at the bottom. */}
      <div
        style={{
          gridColumn: "1 / -1",
          display: "flex",
          gap: "0.5rem",
          flexWrap: "wrap",
          alignItems: "center",
          borderTop: `1px solid ${color.hairline}`,
          paddingTop: "0.7rem",
        }}
      >
        <Button variant="ghost"
          onClick={async () => { await setRoomEnabled(room.id, !room.enabled); await onChanged(); }}
          title={room.enabled ? "Hide this room from the Dashboard and Floor Plan" : "Show this room again"}
        >
          {room.enabled ? "Disable" : "Enable"}
        </Button>
        {mergeCandidates.length > 0 && (
          <Select
            value={undefined}
            disabled={merging}
            onChange={(id) => handleMerge(id)}
            placeholder={merging ? "Merging…" : "Merge into…"}
            title="Merge this room into another (this room is deleted)"
            options={mergeCandidates.map((r) => ({ value: r.id, label: r.name }))}
          />
        )}
        <span style={{ flex: 1 }} />
        <Button variant="danger" onClick={onRemove}>Delete room</Button>
      </div>
    </>
  );

  return (
    <div
      style={{
        ...S.card,
        gap: "0.55rem",
        // The engraved name is the card's own headroom; the shared card
        // padding reads as dead space above it.
        paddingTop: "0.9rem",
        opacity: room.enabled ? 1 : 0.55,
      }}
    >
      {/* Header — the whole row is the expand/collapse affordance. */}
      <div
        role="button"
        tabIndex={0}
        onClick={() => setOpen((v) => !v)}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            setOpen((v) => !v);
          }
        }}
        title={open ? "Collapse" : "Configure this room"}
        style={{
          display: "flex",
          alignItems: "center",
          gap: "0.6rem",
          cursor: "pointer",
          minHeight: 44,
        }}
      >
        <span
          style={{
            fontFamily: font.display,
            fontWeight: 700,
            fontSize: "1.05rem",
            letterSpacing: "0.05em",
            color: color.text,
            minWidth: 0,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {room.name}
        </span>
        {!room.enabled && (
          <span
            style={{
              color: "#a86",
              fontSize: "0.72rem",
              border: "1px solid #543",
              borderRadius: 4,
              padding: "0 0.35rem",
              flexShrink: 0,
            }}
          >
            disabled
          </span>
        )}
        <span style={{ flex: 1 }} />
        {/* Occupancy rune — config feedback, not control: lit while the room's
            counting presence sensors read occupied; absent without any. */}
        {room.occupancy != null && (
          <span
            title={room.occupancy ? "Room reads occupied" : "Room reads empty"}
            style={{
              display: "grid",
              placeItems: "center",
              flexShrink: 0,
              color: room.occupancy ? PRESENCE_ACCENT : color.faint,
              filter: room.occupancy
                ? `drop-shadow(0 0 6px ${alpha(PRESENCE_ACCENT, 0.8)})`
                : undefined,
            }}
          >
            <Glyph name="motion" size={16} />
          </span>
        )}
        <span
          aria-hidden
          style={{
            display: "grid",
            placeItems: "center",
            flexShrink: 0,
            color: color.faint,
            transform: open ? "rotate(180deg)" : undefined,
            transition: "transform 0.2s ease",
          }}
        >
          <Glyph name="chevron" size={14} />
        </span>
      </div>

      {/* Link chips. */}
      {room.links.length > 0 && (
        <div style={{ display: "flex", gap: "0.4rem", flexWrap: "wrap" }}>
          {room.links.map((l) => (
            <span
              key={l.provider_group_id}
              title={l.domain === "media" ? "Synced audio room/zone" : "Synced provider room/zone"}
              style={{
                border: "1px solid var(--bf-border)",
                borderRadius: 4,
                padding: "0 0.35rem",
                color: "#9a9",
                fontSize: "0.72rem",
              }}
            >
              <span style={{ display: "inline-grid", placeItems: "center", verticalAlign: "-2px" }}>
                <Glyph name={l.domain === "media" ? "speaker" : "link"} size={13} />
              </span>{" "}
              {l.name}
            </span>
          ))}
        </div>
      )}

      {/* Census strip — the room's contents at a glance, one niche per domain. */}
      <div style={{ display: "flex", gap: "0.45rem", flexWrap: "wrap" }}>
        <CensusNiche glyph="bulb" count={room.light_ids.length} accent={color.cyan} title="lights" onOpen={() => openAt("members")} />
        <CensusNiche glyph="speaker" count={room.media_devices.length} accent={color.violet} title="speakers" onOpen={() => openAt("audio")} />
        <CensusNiche glyph="power" count={room.power_device_ids.length} accent={color.gold} title="power devices" onOpen={() => openAt("members")} />
        <CensusNiche glyph="motion" count={room.sensor_ids.length} accent={PRESENCE_ACCENT} title="sensors" onOpen={() => openAt("presence")} />
      </div>

      {open &&
        (isCompact ? (
          // Compact: expand inline — a single column growing in place reads
          // naturally on a stacked page.
          <div
            style={{
              ...tileGrid(420, isMobile, "1.1rem 1.6rem"),
              borderTop: `1px solid ${color.hairline}`,
              paddingTop: "0.8rem",
              marginTop: "0.2rem",
            }}
          >
            {editorSections}
          </div>
        ) : (
          // Desktop: the editor opens as a wide modal — the app's config idiom —
          // so opening a room never reflows the tile grid around it.
          <Modal title={room.name} width={1100} onClose={() => setOpen(false)}>
            <div style={tileGrid(420, false, "1.1rem 1.6rem")}>{editorSections}</div>
          </Modal>
        ))}
    </div>
  );
}

// ── New-room form ────────────────────────────────────────────────────────────

function NewRoomForm({
  lights,
  onSubmit,
  onCancel,
}: {
  lights: Light[];
  onSubmit: (name: string, directIds: string[]) => Promise<void>;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const [direct, setDirect] = useState<Set<string>>(new Set());
  const [saving, setSaving] = useState(false);

  function toggleDirect(id: string) {
    setDirect((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  // Lights already claimed by a provider group are still offered — the new room
  // takes a DIRECT assignment; link membership is configured after creation.
  const selectable = pickableLights(lights);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setSaving(true);
    try {
      await onSubmit(name.trim(), [...direct]);
    } finally {
      setSaving(false);
    }
  }

  return (
    <form onSubmit={submit} style={{ display: "flex", flexDirection: "column", gap: "0.75rem" }}>
      <label
        style={{
          display: "flex",
          flexDirection: "column",
          gap: "0.3rem",
          fontSize: "0.875rem",
          color: "var(--bf-dim)",
        }}
      >
        <span>Name</span>
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="e.g. Living Room"
          style={S.input}
          required
          autoFocus
        />
      </label>

      <div style={{ display: "flex", flexDirection: "column", gap: "0.3rem" }}>
        <span style={{ fontSize: "0.875rem", color: "var(--bf-dim)" }}>
          Direct lights <span style={{ color: "var(--bf-faint)" }}>(links, sensors, and audio are configured after creation)</span>
        </span>
        {selectable.map((l) => (
          <SelectRow key={l.id} checked={direct.has(l.id)} onToggle={() => toggleDirect(l.id)}>
            {l.name}
          </SelectRow>
        ))}
      </div>

      <div style={{ display: "flex", gap: "0.5rem" }}>
        <Button variant="accent" type="submit" disabled={saving}>
          {saving ? "Saving…" : "Create"}
        </Button>
        <Button variant="ghost" type="button" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </form>
  );
}
