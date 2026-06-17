import { useEffect, useRef, useState } from "react";
import {
  applySceneToRoom,
  getAudioDevices,
  getPaletteScenes,
  getPowerDevices,
  getProviders,
  getRooms,
  getScenes,
  mergePatch,
  restoreDefaultHome,
  rgbToHex,
  rgbToXy,
  savePaletteSceneFromRoom,
  setAudioEnabled,
  setAudioState,
  setLightEnabled,
  setLightState,
  setPowerEnabled,
  setPowerState,
  setRoomState,
  xyToRgb,
  type AudioCommand,
  type AudioDevice,
  type ControlTarget,
  type Light,
  type LightState,
  type LightStatePatch,
  type PaletteScene,
  type PowerDevice,
  type Provider,
  type Room,
  type RoomControl,
  type Scene,
} from "../api";
import { AudioEditor } from "../components/AudioControls";
import { Glyph, powerKindGlyph, audioKindGlyph } from "../components/glyphs";
import { hexToRgb, LightEditor, type LightControlChange } from "../components/LightEditor";
import { T, font, glassCard, radius, color, glow, alpha } from "../theme";
import { CornerFiligree } from "../components/ornament";
import { PageHeader } from "../components/PageHeader";
import { DisableRow, PowerFlyout } from "../components/PowerFlyout";
import { SceneButton, SceneModal } from "../components/scenes";
import { useDialogs, type Dialogs } from "../components/dialogs";
import { useViewport } from "../useViewport";

const label: React.CSSProperties = {
  fontFamily: font.display,
  textTransform: "uppercase",
  letterSpacing: "0.14em",
  fontWeight: 700,
};

interface Props {
  lights: Light[];
  onRefresh: () => void;
  onNavigate: (page: "settings") => void;
}

export function DashboardPage({ lights, onRefresh, onNavigate }: Props) {
  const { isMobile, isCompact } = useViewport();
  const [localLights, setLocalLights] = useState<Light[]>(lights);
  const [powerDevices, setPowerDevices] = useState<PowerDevice[]>([]);
  const [audioDevices, setAudioDevices] = useState<AudioDevice[]>([]);
  const [providers, setProviders] = useState<Provider[]>([]);
  const [rooms, setRooms] = useState<Room[]>([]);
  const [scenes, setScenes] = useState<PaletteScene[]>([]);
  const [homeScenes, setHomeScenes] = useState<Scene[]>([]);
  const dialogs = useDialogs();

  function loadScenes() {
    getPaletteScenes().then(setScenes);
    getScenes().then(setHomeScenes);
  }

  useEffect(() => { setLocalLights(lights); }, [lights]);
  useEffect(() => { getProviders().then(setProviders); }, []);
  useEffect(() => { loadScenes(); }, []);
  // Re-fetch membership + non-light devices alongside light refreshes.
  useEffect(() => {
    getRooms().then(setRooms);
    getPowerDevices().then(setPowerDevices);
    getAudioDevices().then(setAudioDevices);
  }, [lights]);

  // Real-time state: light_state (Hue SSE), audio_state (Onkyo push), and
  // power_state (HA WebSocket push).
  useEffect(() => {
    const es = new EventSource("/api/events");
    es.addEventListener("light_state", (raw) => {
      const { device_id, patch } = JSON.parse((raw as MessageEvent).data) as {
        device_id: string;
        patch: LightStatePatch;
      };
      setLocalLights((prev) =>
        prev.map((l) =>
          l.device_id === device_id ? { ...l, last_state: mergePatch(l.last_state, patch) } : l,
        ),
      );
    });
    es.addEventListener("audio_state", (raw) => {
      const ev = JSON.parse((raw as MessageEvent).data) as {
        provider_id: string;
        device_id: string;
        state: AudioDevice["state"];
      };
      setAudioDevices((prev) =>
        prev.map((d) =>
          d.provider_id === ev.provider_id && d.device_id === ev.device_id
            ? { ...d, state: ev.state }
            : d,
        ),
      );
    });
    es.addEventListener("power_state", (raw) => {
      const ev = JSON.parse((raw as MessageEvent).data) as {
        provider_id: string;
        device_id: string;
        state: PowerDevice["state"];
      };
      setPowerDevices((prev) =>
        prev.map((d) =>
          d.provider_id === ev.provider_id && d.device_id === ev.device_id
            ? { ...d, state: ev.state }
            : d,
        ),
      );
    });
    es.onerror = () => {};
    return () => es.close();
  }, []);

  function onLightUpdate(id: string, state: LightState) {
    setLocalLights((prev) => prev.map((l) => (l.id === id ? { ...l, last_state: state } : l)));
  }
  function onAudioPatch(id: string, patch: Partial<AudioDevice["state"]>) {
    setAudioDevices((prev) =>
      prev.map((d) => (d.id === id ? { ...d, state: { ...d.state, ...patch } } : d)),
    );
  }
  function onPowerToggle(id: string, next: boolean) {
    setPowerDevices((prev) =>
      prev.map((d) => (d.id === id ? { ...d, state: { ...d.state, on: next } } : d)),
    );
    setPowerState(id, next).then((err) => {
      if (err) setPowerDevices((prev) =>
        prev.map((d) => (d.id === id ? { ...d, state: { ...d.state, on: !next } } : d)),
      );
    });
  }
  function onLightSetEnabled(id: string, enabled: boolean) {
    setLocalLights((prev) => prev.map((l) => (l.id === id ? { ...l, enabled } : l)));
    setLightEnabled(id, enabled);
  }
  function onAudioSetEnabled(id: string, enabled: boolean) {
    setAudioDevices((prev) => prev.map((d) => (d.id === id ? { ...d, enabled } : d)));
    setAudioEnabled(id, enabled);
  }
  function onPowerSetEnabled(id: string, enabled: boolean) {
    setPowerDevices((prev) => prev.map((d) => (d.id === id ? { ...d, enabled } : d)));
    setPowerEnabled(id, enabled);
  }

  const onCount = localLights.filter((l) => l.last_state?.on).length;
  const empty = localLights.length === 0 && powerDevices.length === 0 && audioDevices.length === 0;
  const defaultHome = homeScenes.find((s) => s.is_default);

  // Confirmation is the seal's own two-tap arm/confirm gesture, so no modal here.
  async function doRestoreHome() {
    if (!defaultHome) return;
    try {
      await restoreDefaultHome();
      onRefresh();
    } catch (e) {
      await dialogs.alert({ title: "Couldn't restore", message: e instanceof Error ? e.message : String(e) });
    }
  }

  return (
    <div style={{ padding: isMobile ? "1rem 0.85rem" : isCompact ? "1.1rem 1rem" : "2rem", width: "100%", maxWidth: 1100, margin: "0 auto", color: T.text, display: "flex", flexDirection: "column", flex: 1, boxSizing: "border-box" }}>
      <PageHeader
        title="Control"
        status={localLights.length > 0 ? `${onCount} of ${localLights.length} lights on` : undefined}
      />

      {empty ? (
        <div style={{ textAlign: "center", padding: "4rem 0", color: T.faint }}>
          <p style={{ margin: "0 0 0.75rem" }}>No devices found.</p>
          <p style={{ margin: 0, fontSize: "0.875rem" }}>
            Add a provider in{" "}
            <button
              onClick={() => onNavigate("settings")}
              style={{ background: "none", border: "none", color: T.accent, cursor: "pointer", fontSize: "0.875rem", padding: 0 }}
            >
              Settings
            </button>{" "}
            and run discovery.
          </p>
        </div>
      ) : (
        <RoomGrid
          lights={localLights}
          powerDevices={powerDevices}
          audioDevices={audioDevices}
          rooms={rooms}
          providers={providers}
          scenes={scenes}
          dialogs={dialogs}
          onScenesChanged={loadScenes}
          onLightUpdate={onLightUpdate}
          onAudioPatch={onAudioPatch}
          onPowerToggle={onPowerToggle}
          onLightSetEnabled={onLightSetEnabled}
          onAudioSetEnabled={onAudioSetEnabled}
          onPowerSetEnabled={onPowerSetEnabled}
          onChanged={onRefresh}
        />
      )}

      {defaultHome && !empty && (
        <div style={{ marginTop: "auto", display: "flex", justifyContent: "center", paddingTop: "2.2rem" }}>
          <RestoreHomeButton name={defaultHome.name} onRestore={doRestoreHome} />
        </div>
      )}
      {dialogs.element}
    </div>
  );
}

/** Devices grouped into one box per room, laid out in two columns on desktop
 * (a single column on mobile). Lights with no room fall into per-provider boxes. */
function RoomGrid({
  lights,
  powerDevices,
  audioDevices,
  rooms,
  providers,
  scenes,
  dialogs,
  onScenesChanged,
  onLightUpdate,
  onAudioPatch,
  onPowerToggle,
  onLightSetEnabled,
  onAudioSetEnabled,
  onPowerSetEnabled,
  onChanged,
}: {
  lights: Light[];
  powerDevices: PowerDevice[];
  audioDevices: AudioDevice[];
  rooms: Room[];
  providers: Provider[];
  scenes: PaletteScene[];
  dialogs: Dialogs;
  onScenesChanged: () => void;
  onLightUpdate: (id: string, state: LightState) => void;
  onAudioPatch: (id: string, patch: Partial<AudioDevice["state"]>) => void;
  onPowerToggle: (id: string, next: boolean) => void;
  onLightSetEnabled: (id: string, enabled: boolean) => void;
  onAudioSetEnabled: (id: string, enabled: boolean) => void;
  onPowerSetEnabled: (id: string, enabled: boolean) => void;
  onChanged: () => void;
}) {
  const { isMobile } = useViewport();
  const lightById = new Map(lights.map((l) => [l.id, l]));
  const powerById = new Map(powerDevices.map((d) => [d.id, d]));
  const audioById = new Map(audioDevices.map((d) => [d.id, d]));
  const assigned = new Set<string>();

  // Disabled devices keep their room membership but drop out of room control.
  const roomSections = rooms
    .map((room) => {
      const roomLights = room.light_ids
        .map((id) => lightById.get(id))
        .filter((l): l is Light => !!l && l.enabled !== false);
      const power = room.power_device_ids
        .map((id) => powerById.get(id))
        .filter((d): d is PowerDevice => !!d && d.enabled !== false);
      const members = room.audio_devices
        .map((m) => audioById.get(m.audio_device_id))
        .filter((d): d is AudioDevice => !!d && d.enabled !== false);
      // M22 combined control: a receiver that is the volume-target of another
      // member in this room isn't shown on its own — the bound source's control
      // already drives it (volume routes there), so the pair reads as one device.
      const boundReceivers = new Set(
        members.map((d) => d.receiver_id).filter((id): id is string => !!id),
      );
      const audio = members.filter((d) => !boundReceivers.has(d.id));
      for (const l of roomLights) assigned.add(l.id);
      return { room, lights: roomLights, power, audio };
    })
    .filter((s) => s.room.enabled && (s.lights.length + s.power.length + s.audio.length) > 0)
    .sort((a, b) => a.room.name.localeCompare(b.room.name));

  // Unassigned lights stay visible (per provider). Power/audio devices live on
  // their own pages until they're added to a room. Disabled devices never show
  // on Control — they're configured on the Devices page, not controlled here.
  const providerName = new Map(providers.map((p) => [p.id, p.name]));
  const leftovers = new Map<string, Light[]>();
  for (const l of lights) {
    if (assigned.has(l.id) || l.enabled === false) continue;
    leftovers.set(l.provider_id, [...(leftovers.get(l.provider_id) ?? []), l]);
  }
  const leftoverSections = [...leftovers.entries()].sort((a, b) =>
    (providerName.get(a[0]) ?? "").localeCompare(providerName.get(b[0]) ?? ""),
  );

  // Resolve a bound source's receiver name (the receiver may be collapsed out of
  // the room's member list, but still lives in the global device map).
  const receiverNameFor = (d: AudioDevice) =>
    d.receiver_id ? audioById.get(d.receiver_id)?.name : undefined;

  const common = {
    scenes,
    dialogs,
    onScenesChanged,
    onLightUpdate,
    onAudioPatch,
    onPowerToggle,
    onLightSetEnabled,
    onAudioSetEnabled,
    onPowerSetEnabled,
    onChanged,
    receiverNameFor,
  };

  return (
    <div style={{ columnCount: isMobile ? 1 : 2, columnGap: "1.1rem" }}>
      {roomSections.map(({ room, lights, power, audio }, i) => (
        <RoomBox key={room.id} index={i} name={room.name} roomId={room.id} lights={lights} power={power} audio={audio} controls={room.controls} {...common} />
      ))}
      {leftoverSections.map(([providerId, sectionLights], i) => (
        <RoomBox
          key={providerId}
          index={roomSections.length + i}
          name={
            roomSections.length > 0
              ? `${providerName.get(providerId) ?? "Other"} · no room`
              : providerName.get(providerId) ?? "Other"
          }
          lights={sectionLights}
          power={[]}
          audio={[]}
          controls={[]}
          {...common}
        />
      ))}
    </div>
  );
}

function litHexes(lights: Light[]): string[] {
  return lights
    .filter((l) => l.last_state?.on && l.last_state.color)
    .map((l) => {
      const c = l.last_state!.color!;
      return rgbToHex(...xyToRgb(c.x, c.y, c.brightness));
    });
}

/** Collapse a room's audio members into control entries: speakers playing in a
 * live sync group (sharing `group_coordinator`) become a single entry driven by
 * the coordinator; everything else is its own entry. A grouped coordinator whose
 * other members aren't in this room degrades to a solo entry. Derived from the
 * members — no group device is stored (see `models::audio::AudioState`). */
function groupedAudio(audio: AudioDevice[]): { coordinator: AudioDevice; members: AudioDevice[] }[] {
  const byCoordinator = new Map<string, AudioDevice[]>();
  const solo: AudioDevice[] = [];
  for (const d of audio) {
    const coord = d.state.group_coordinator;
    if (coord) {
      const arr = byCoordinator.get(coord) ?? [];
      arr.push(d);
      byCoordinator.set(coord, arr);
    } else {
      solo.push(d);
    }
  }
  const entries: { coordinator: AudioDevice; members: AudioDevice[] }[] = [];
  for (const [coord, members] of byCoordinator) {
    if (members.length >= 2) {
      const coordinator = members.find((m) => m.provider_id === coord) ?? members[0];
      entries.push({ coordinator, members });
    } else {
      solo.push(...members); // lone grouped member here → show on its own
    }
  }
  for (const d of solo) entries.push({ coordinator: d, members: [d] });
  return entries;
}

/** A room: framed plate whose gold corner filigree breathes its lit lights'
 * colors (brass when off), a header with
 * room-wide light controls (color/brightness cascade + scenes + on/off), and a
 * row of one glyph button per member device. Each device opens its own fly-out. */
function RoomBox({
  index = 0,
  name,
  roomId,
  lights,
  power,
  audio,
  controls,
  scenes,
  dialogs,
  onScenesChanged,
  onLightUpdate,
  onAudioPatch,
  onPowerToggle,
  onLightSetEnabled,
  onAudioSetEnabled,
  onPowerSetEnabled,
  onChanged,
  receiverNameFor,
}: {
  index?: number;
  name: string;
  roomId?: string;
  lights: Light[];
  power: PowerDevice[];
  audio: AudioDevice[];
  controls: RoomControl[];
  scenes: PaletteScene[];
  dialogs: Dialogs;
  onScenesChanged: () => void;
  onLightUpdate: (id: string, state: LightState) => void;
  onAudioPatch: (id: string, patch: Partial<AudioDevice["state"]>) => void;
  onPowerToggle: (id: string, next: boolean) => void;
  onLightSetEnabled: (id: string, enabled: boolean) => void;
  onAudioSetEnabled: (id: string, enabled: boolean) => void;
  onPowerSetEnabled: (id: string, enabled: boolean) => void;
  onChanged: () => void;
  receiverNameFor?: (d: AudioDevice) => string | undefined;
}) {
  const { isCompact } = useViewport();
  const tuneRef = useRef<HTMLDivElement>(null);
  const [editing, setEditing] = useState(false);
  const [scenesOpen, setScenesOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const commitTimer = useRef<ReturnType<typeof setTimeout>>(undefined);

  const lit = lights.filter((l) => l.last_state?.on);
  const anyOn = lit.length > 0;
  // Room-level power reflects ALL member domains (a speakers-only room can still
  // be powered). The master power button uses this; the header dot stays light-
  // centric (it breathes the lit color).
  const roomAnyOn = anyOn || power.some((d) => d.state.on) || audio.some((d) => d.state.power);
  const canPower = !!roomId && lights.length + power.length + audio.length > 0;
  const showColor = lights.some((l) => l.capabilities.color_rgb);
  const showWhite = lights.some((l) => l.capabilities.color_temperature);
  const showBrightness = lights.some((l) => l.capabilities.dimmable);
  const tunable = !!roomId && (showColor || showWhite || showBrightness);
  const roomMirek =
    lit.map((l) => l.last_state?.color_temp_mirek).find((m): m is number => m != null) ?? 366;

  const counts = [
    lights.length && `${lights.length} light${lights.length !== 1 ? "s" : ""}`,
    power.length && `${power.length} switch${power.length !== 1 ? "es" : ""}`,
    audio.length && `${audio.length} speaker${audio.length !== 1 ? "s" : ""}`,
  ].filter(Boolean);
  const subtitle = counts.join(" · ");

  const hexes = litHexes(lights);
  const roomHex = hexes[0] ?? "#ffb84d";
  const avgBrightness = lit.length
    ? Math.round(lit.reduce((sum, l) => sum + (l.last_state?.brightness ?? 100), 0) / lit.length)
    : 100;

  function cascade(change: LightControlChange) {
    if (!roomId) return;
    // Effects are a per-light control (each light's supported set differs), not a
    // room-wide cascade — the room editor doesn't surface them.
    if (change.field === "effect") return;
    // Adjust only the dimension the user moved, per light by capability. A room
    // brightness change must not overwrite each member's own color (e.g. set by a
    // scene); a color or white change is mutually exclusive (set one, clear the
    // other). The room PUT carries just the changed field — the backend merges it
    // into each light's cached state and preserves the untouched dimensions.
    for (const l of lights) {
      const next: LightState = { ...(l.last_state ?? { on: true }), on: true };
      if (change.field === "brightness") {
        if (l.capabilities.dimmable) next.brightness = change.brightness;
      } else if (change.field === "color") {
        if (l.capabilities.color_rgb) {
          next.color = rgbToXy(...hexToRgb(change.hex));
          next.color_temp_mirek = undefined;
        }
      } else if (l.capabilities.color_temperature) {
        next.color_temp_mirek = change.mirek;
        next.color = undefined;
      }
      onLightUpdate(l.id, next);
    }
    const patch: LightState =
      change.field === "color"
        ? { on: true, color: rgbToXy(...hexToRgb(change.hex)) }
        : change.field === "temp"
          ? { on: true, color_temp_mirek: change.mirek }
          : { on: true, brightness: change.brightness };
    clearTimeout(commitTimer.current);
    commitTimer.current = setTimeout(() => { setRoomState(roomId, patch); }, 200);
  }

  async function toggleAll() {
    if (!roomId) return;
    const next = !roomAnyOn;
    setBusy(true);
    for (const l of lights) onLightUpdate(l.id, { ...(l.last_state ?? { on: false }), on: next });
    try {
      await setRoomState(roomId, { on: next });
      onChanged();
    } finally {
      setBusy(false);
    }
  }

  async function applyScene(sceneId: string) {
    if (!roomId || !sceneId) return;
    setBusy(true);
    try {
      await applySceneToRoom(roomId, sceneId);
      onChanged();
    } finally {
      setBusy(false);
    }
  }

  async function saveAsScene() {
    if (!roomId) return;
    if (!anyOn) {
      await dialogs.alert({ title: "Nothing to save", message: "No lights in this room are on — turn some on first." });
      return;
    }
    const sceneName = await dialogs.prompt({
      title: "Save room as scene",
      message: "Saves this room's current colors and brightness as a reusable scene.",
      placeholder: "Scene name",
      confirmLabel: "Save",
    });
    if (!sceneName?.trim()) return;
    try {
      await savePaletteSceneFromRoom(roomId, sceneName.trim());
      onScenesChanged();
    } catch (e) {
      await dialogs.alert({ title: "Couldn't save scene", message: String(e) });
    }
  }

  return (
    <section
      className="bifrost-card"
      style={{
        breakInside: "avoid",
        marginBottom: "1.1rem",
        position: "relative",
        overflow: "hidden",
        animationDelay: `${Math.min(index, 8) * 60}ms`,
        ...(roomId
          ? glassCard
          : {
              background: "transparent",
              border: `1px dashed ${T.hairline}`,
              borderRadius: radius.frame,
            }),
      }}
    >
      {roomId && <CornerFiligree colors={hexes} />}

      <header
        ref={tuneRef}
        onClick={() => { if (tunable) setEditing((v) => !v); }}
        title={tunable ? "Set the whole room's color and brightness" : undefined}
        style={{
          display: "flex",
          alignItems: "center",
          gap: isCompact ? "0.5rem" : "0.7rem",
          padding: isCompact ? "0.5rem 0.7rem 0.45rem" : "0.7rem 1rem",
          borderBottom: `1px solid ${T.hairline}`,
          cursor: tunable ? "pointer" : "default",
        }}
      >
        {roomId && (
          <span
            aria-hidden
            style={{
              width: 16,
              height: 16,
              flexShrink: 0,
              borderRadius: "50%",
              border: "1px solid rgba(255,255,255,0.22)",
              background: anyOn
                ? `radial-gradient(circle at 35% 30%, #ffffff44, transparent 45%), ${roomHex}`
                : "#3a372e",
              boxShadow: anyOn ? `0 0 12px -3px ${roomHex}` : "none",
            }}
          />
        )}
        <div style={{ minWidth: 0, flex: 1, display: "flex", flexDirection: "column" }}>
          <span style={{ ...label, fontSize: "0.82rem", color: roomId ? "#d8cfba" : T.faint, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {name}
          </span>
          <span style={{ fontSize: "0.7rem", color: T.faint, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {subtitle}
          </span>
        </div>
        {(canPower || controls.length > 0) && (
          <div
            onClick={(e) => e.stopPropagation()}
            style={{ display: "flex", alignItems: "center", gap: isCompact ? "0.3rem" : "0.4rem", flexShrink: 0 }}
          >
            {controls.map((c) => (
              <RoomControlButton
                key={c.id ?? `${c.kind}-${c.glyph}`}
                control={c}
                roomId={roomId}
                lights={lights}
                power={power}
                audio={audio}
                onLightUpdate={onLightUpdate}
                onPowerToggle={onPowerToggle}
                onAudioPatch={onAudioPatch}
                onChanged={onChanged}
                size={isCompact ? 38 : 42}
              />
            ))}
            {canPower && (
              <GlyphButton
                on={roomAnyOn}
                accent={T.accent}
                title={roomAnyOn ? "Turn room off" : "Turn room on"}
                active={false}
                buttonRef={null}
                onClick={toggleAll}
                size={isCompact ? 38 : 42}
              >
                <Glyph name="power" size={isCompact ? 18 : 20} />
              </GlyphButton>
            )}
          </div>
        )}
      </header>

      <div
        style={{
          display: "flex",
          flexWrap: "wrap",
          gap: isCompact ? "0.4rem" : "0.5rem",
          padding: isCompact ? "0.6rem" : "0.8rem 1rem",
        }}
      >
        {lights.map((l) => (
          <LightButton key={l.id} light={l} onLightUpdate={onLightUpdate} onSetEnabled={onLightSetEnabled} onChanged={onChanged} />
        ))}
        {power.map((d) => (
          <PowerButton key={d.id} device={d} onToggle={onPowerToggle} onSetEnabled={onPowerSetEnabled} />
        ))}
        {groupedAudio(audio).map((entry) =>
          entry.members.length >= 2 ? (
            <AudioButton
              key={`grp-${entry.coordinator.id}`}
              device={entry.coordinator}
              groupMembers={entry.members}
              onAudioPatch={onAudioPatch}
              onSetEnabled={onAudioSetEnabled}
            />
          ) : (
            <AudioButton
              key={entry.coordinator.id}
              device={entry.coordinator}
              onAudioPatch={onAudioPatch}
              onSetEnabled={onAudioSetEnabled}
              receiverName={receiverNameFor?.(entry.coordinator)}
            />
          ),
        )}
      </div>

      {editing && tuneRef.current && (
        <LightEditor
          anchor={tuneRef.current}
          title={name}
          initialHex={roomHex}
          initialBrightness={avgBrightness}
          initialMirek={roomMirek}
          showColor={showColor}
          showWhite={showWhite}
          showBrightness={showBrightness}
          on={anyOn}
          onToggle={toggleAll}
          onChange={cascade}
          onClose={() => setEditing(false)}
        >
          <SceneButton onClick={() => { setEditing(false); setScenesOpen(true); }} />
        </LightEditor>
      )}

      {scenesOpen && (
        <SceneModal
          roomName={name}
          scenes={scenes}
          busy={busy}
          onApply={async (id) => { await applyScene(id); setScenesOpen(false); }}
          onSave={saveAsScene}
          onClose={() => setScenesOpen(false)}
        />
      )}
    </section>
  );
}

/** A user-configured quick-control button on a room's header (see migration
 * 0034 / RoomControlsPanel). `power` toggles its targets and `scene` applies a
 * scene directly; `brightness`/`volume` open the shared LightEditor/AudioEditor
 * scoped to the targets (fanning to all of them). */
function RoomControlButton({
  control,
  roomId,
  lights,
  power,
  audio,
  onLightUpdate,
  onPowerToggle,
  onAudioPatch,
  onChanged,
  size,
}: {
  control: RoomControl;
  roomId?: string;
  lights: Light[];
  power: PowerDevice[];
  audio: AudioDevice[];
  onLightUpdate: (id: string, state: LightState) => void;
  onPowerToggle: (id: string, next: boolean) => void;
  onAudioPatch: (id: string, patch: Partial<AudioDevice["state"]>) => void;
  onChanged: () => void;
  size: number;
}) {
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const commitTimer = useRef<ReturnType<typeof setTimeout>>(undefined);

  const has = (domain: ControlTarget["domain"], id: string) =>
    control.targets.some((t) => t.domain === domain && t.id === id);
  const tLights = lights.filter((l) => has("light", l.id));
  const tPower = power.filter((d) => has("power", d.id));
  const tAudio = audio.filter((d) => has("audio", d.id));

  // A non-scene control whose targets have all been removed/disabled has nothing
  // to act on — drop it rather than render a dead button.
  if (control.kind !== "scene" && tLights.length + tPower.length + tAudio.length === 0) {
    return null;
  }

  const anyOn =
    control.kind === "scene"
      ? false
      : tLights.some((l) => l.last_state?.on) ||
        tPower.some((d) => d.state.on) ||
        tAudio.some((d) => d.state.power);

  const accent =
    control.kind === "volume" ? T.audio : control.kind === "brightness" ? "#ffb84d" : T.accent;

  function togglePower() {
    const next = !anyOn;
    for (const l of tLights) {
      const s: LightState = { ...(l.last_state ?? { on: false }), on: next };
      onLightUpdate(l.id, s);
      setLightState(l.id, s);
    }
    for (const d of tPower) onPowerToggle(d.id, next);
    for (const d of tAudio) {
      onAudioPatch(d.id, { power: next });
      setAudioState(d.id, { power: next });
    }
  }

  async function applyScene() {
    if (!roomId || !control.scene_id) return;
    await applySceneToRoom(roomId, control.scene_id);
    onChanged();
  }

  // Brightness cascade across the target lights (per-light by capability),
  // debounced — mirrors the room-header cascade.
  function cascade(change: LightControlChange) {
    if (change.field === "effect") return; // per-light control, not a room cascade
    const updates: [string, LightState][] = [];
    for (const l of tLights) {
      const next: LightState = { ...(l.last_state ?? { on: true }), on: true };
      if (change.field === "brightness") {
        if (l.capabilities.dimmable) next.brightness = change.brightness;
      } else if (change.field === "color") {
        if (l.capabilities.color_rgb) {
          next.color = rgbToXy(...hexToRgb(change.hex));
          next.color_temp_mirek = undefined;
        }
      } else if (l.capabilities.color_temperature) {
        next.color_temp_mirek = change.mirek;
        next.color = undefined;
      }
      onLightUpdate(l.id, next);
      updates.push([l.id, next]);
    }
    clearTimeout(commitTimer.current);
    commitTimer.current = setTimeout(() => {
      for (const [id, s] of updates) setLightState(id, s);
    }, 200);
  }

  // Volume control fans changes to every target audio device. The AudioEditor
  // commits its own `device`; this wrapper fans the same command to the rest.
  function fanAudio(id: string, patch: Partial<AudioDevice["state"]>) {
    const cmd: AudioCommand = {};
    if (patch.volume !== undefined) cmd.volume = patch.volume;
    if (patch.mute !== undefined) cmd.mute = patch.mute;
    if (patch.power !== undefined) cmd.power = patch.power;
    for (const d of tAudio) {
      onAudioPatch(d.id, patch);
      if (d.id !== id && Object.keys(cmd).length > 0) setAudioState(d.id, cmd);
    }
  }

  function onClick() {
    if (control.kind === "power") togglePower();
    else if (control.kind === "scene") applyScene();
    else setOpen((v) => !v);
  }

  const litT = tLights.filter((l) => l.last_state?.on);
  const firstColor = litT.map((l) => l.last_state?.color).find((c) => c);
  const initHex = firstColor
    ? rgbToHex(...xyToRgb(firstColor.x, firstColor.y, firstColor.brightness))
    : "#ffb84d";
  const initBrightness = litT.length
    ? Math.round(litT.reduce((s, l) => s + (l.last_state?.brightness ?? 100), 0) / litT.length)
    : 100;
  const initMirek =
    litT.map((l) => l.last_state?.color_temp_mirek).find((m): m is number => m != null) ?? 366;
  const title = control.label || control.kind;

  return (
    <>
      <GlyphButton
        on={anyOn}
        accent={accent}
        title={title}
        active={open}
        buttonRef={ref}
        onClick={onClick}
        size={size}
      >
        <Glyph name={control.glyph} size={size <= 40 ? 18 : 20} />
      </GlyphButton>
      {open && control.kind === "brightness" && ref.current && (
        <LightEditor
          anchor={ref.current}
          title={title}
          initialHex={initHex}
          initialBrightness={initBrightness}
          initialMirek={initMirek}
          showColor={tLights.some((l) => l.capabilities.color_rgb)}
          showWhite={tLights.some((l) => l.capabilities.color_temperature)}
          showBrightness={tLights.some((l) => l.capabilities.dimmable)}
          on={anyOn}
          onToggle={togglePower}
          onChange={cascade}
          onClose={() => setOpen(false)}
        />
      )}
      {open && control.kind === "volume" && tAudio[0] && ref.current && (
        <AudioEditor
          device={tAudio[0]}
          anchor={ref.current}
          onLocalPatch={fanAudio}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );
}

/** The whole-home "Restore Home" action — a gilded **power seal**: a round gothic
 * emblem (double filigree rings echoing the room-card corners, a restore arrow at
 * its heart) over a slow breathing gold aura, with an engraved label beneath.
 * Gold is the ornament/power accent, setting it apart from the cyan per-device
 * controls; the seal form makes the "bring everything back" action ceremonial. */
function RestoreHomeButton({ name, onRestore }: { name: string; onRestore: () => void }) {
  const [hover, setHover] = useState(false);
  // Two-tap safety: the first tap *arms* the seal (glyph spins, label flips to
  // "Confirm"); a second tap within 5s fires the restore. The arm auto-clears on
  // timeout so a stray tap never leaves it primed.
  const [armed, setArmed] = useState(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  function clearTimer() {
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }
  // Drop the pending timeout if the seal unmounts mid-arm.
  useEffect(() => clearTimer, []);

  function handleClick() {
    if (armed) {
      clearTimer();
      setArmed(false);
      onRestore();
      return;
    }
    setArmed(true);
    clearTimer();
    timerRef.current = setTimeout(() => {
      setArmed(false);
      timerRef.current = null;
    }, 5000);
  }

  // Match the app's primary accent (nav + room power buttons), not a bespoke gold.
  const g = T.accent;
  const size = 64;
  // Armed reads "hotter": brighter fill, tighter ring, stronger glow.
  const lit = hover || armed;
  return (
    <div
      onPointerEnter={() => setHover(true)}
      onPointerLeave={() => setHover(false)}
      style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: "0.7rem" }}
    >
      <div style={{ position: "relative", display: "grid", placeItems: "center" }}>
        {/* Breathing gold aura behind the seal. */}
        <span
          aria-hidden
          className="bifrost-seal-breathe"
          style={{
            position: "absolute",
            width: size + 26,
            height: size + 26,
            borderRadius: "50%",
            background: `radial-gradient(circle, ${alpha(g, armed ? 0.62 : 0.4)}, transparent 68%)`,
            pointerEvents: "none",
          }}
        />
        <button
          onClick={handleClick}
          title={armed ? "Tap again to confirm" : `Restore the whole home to "${name}"`}
          aria-label={armed ? "Confirm restore home" : "Restore Home"}
          aria-pressed={armed}
          style={{
            position: "relative",
            width: size,
            height: size,
            borderRadius: "50%",
            display: "grid",
            placeItems: "center",
            cursor: "pointer",
            color: g,
            background: `radial-gradient(circle at 50% 32%, ${alpha(g, lit ? 0.3 : 0.16)}, transparent 62%), ${color.surface}`,
            border: `1px solid ${alpha(g, armed ? 0.95 : hover ? 0.78 : 0.5)}`,
            boxShadow: `${glow(g, armed ? 44 : hover ? 36 : 22)}, inset 0 0 20px -8px ${g}`,
            backdropFilter: "blur(10px)",
            WebkitBackdropFilter: "blur(10px)",
            transform: lit ? "scale(1.06)" : "scale(1)",
            transition: "transform .2s ease, box-shadow .3s ease, border-color .3s ease, color .2s ease, background .3s ease",
          }}
        >
          {/* Inner filigree ring — the engraved-seal double edge. */}
          <span
            aria-hidden
            style={{ position: "absolute", inset: 6, borderRadius: "50%", border: `1px solid ${alpha(g, armed ? 0.55 : 0.32)}`, pointerEvents: "none" }}
          />
          <span
            className={armed ? "bifrost-seal-spin" : undefined}
            style={{ display: "grid", placeItems: "center" }}
          >
            <Glyph name="restore" size={26} />
          </span>
        </button>
      </div>
      <span
        style={{
          fontFamily: font.display,
          textTransform: "uppercase",
          letterSpacing: "0.24em",
          fontSize: "0.72rem",
          fontWeight: 600,
          color: g,
          textShadow: `0 0 12px ${alpha(g, armed ? 0.8 : 0.5)}`,
          transition: "color .2s ease, text-shadow .2s ease",
        }}
      >
        {armed ? "Confirm" : name}
      </span>
    </div>
  );
}

// ── Device glyph buttons ──────────────────────────────────────────────────────

/** Shared shell: a square button showing a device-type glyph, glowing in its
 * accent when on. The full name lives in the fly-out it opens. */
function GlyphButton({
  on,
  accent,
  offline,
  title,
  active,
  buttonRef,
  onClick,
  onLongPress,
  size = 52,
  children,
}: {
  on: boolean;
  accent: string;
  offline?: boolean;
  title: string;
  active: boolean;
  buttonRef: React.Ref<HTMLButtonElement>;
  onClick: () => void;
  /** Press-and-hold (~500ms) action; suppresses the click that would follow.
   * Used as a quick power toggle so a tap still opens the fly-out. */
  onLongPress?: () => void;
  size?: number;
  children: React.ReactNode;
}) {
  const holdTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const fired = useRef(false);

  function startHold() {
    fired.current = false;
    if (!onLongPress) return;
    holdTimer.current = setTimeout(() => {
      fired.current = true;
      onLongPress();
    }, 500);
  }
  function cancelHold() {
    clearTimeout(holdTimer.current);
  }
  function handleClick() {
    // Swallow the click that fires after a long-press completes.
    if (fired.current) {
      fired.current = false;
      return;
    }
    onClick();
  }

  return (
    <button
      ref={buttonRef}
      onClick={handleClick}
      onPointerDown={startHold}
      onPointerUp={cancelHold}
      onPointerLeave={cancelHold}
      onPointerCancel={cancelHold}
      title={onLongPress ? `${title} — hold to toggle power` : title}
      aria-label={title}
      style={{
        width: size,
        height: size,
        flexShrink: 0,
        display: "grid",
        placeItems: "center",
        borderRadius: radius.md,
        cursor: "pointer",
        color: on ? accent : T.dim,
        // On: lit niche — accent top-light over glass + outer neon bloom. Off: a
        // recessed gothic niche with a faint gold-hairline edge.
        background: on
          ? `radial-gradient(130% 130% at 50% 0%, ${alpha(accent, 0.19)}, transparent 62%), ${color.surface}`
          : color.surfaceOff,
        border: `1px solid ${active ? color.cyan : on ? `${alpha(accent, 0.40)}` : color.hairline}`,
        boxShadow: on
          ? `${glow(accent, 22)}, inset 0 0 16px -9px ${accent}`
          : "inset 0 1px 0 rgba(236,230,240,0.04), inset 0 0 18px -13px #000",
        textShadow: on ? `0 0 12px ${alpha(accent, 0.67)}` : undefined,
        opacity: offline ? 0.4 : 1,
        transition: "color 0.2s, background 0.2s, border-color 0.2s, box-shadow 0.2s",
      }}
    >
      {children}
    </button>
  );
}

function LightButton({
  light,
  onLightUpdate,
  onSetEnabled,
  onChanged,
}: {
  light: Light;
  onLightUpdate: (id: string, state: LightState) => void;
  onSetEnabled: (id: string, enabled: boolean) => void;
  onChanged: () => void;
}) {
  const ref = useRef<HTMLButtonElement>(null);
  const [editing, setEditing] = useState(false);
  const commitTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const isOn = light.last_state?.on ?? false;
  const offline = light.last_state?.reachable === false;
  const serverColor = light.last_state?.color;
  const hex = serverColor ? rgbToHex(...xyToRgb(serverColor.x, serverColor.y, serverColor.brightness)) : "#ffb84d";
  const brightness = light.last_state?.brightness ?? 100;
  const mirek = light.last_state?.color_temp_mirek ?? 366;
  // A light is in white mode when it reports a temperature and no color.
  const whiteMode = light.last_state?.color_temp_mirek != null && !light.last_state?.color;

  function handleEditorChange(change: LightControlChange) {
    // Only the moved dimension changes; color and temperature are exclusive. The
    // previous `effect` is deliberately cleared (undefined → omitted from the PUT):
    // it rides along only on an actual effect pick, so a colour/brightness tweak
    // never re-fires a transient effect (LIFX breathe/pulse) on the provider.
    const next: LightState = { ...(light.last_state ?? {}), on: true, effect: undefined };
    if (change.field === "brightness") {
      if (light.capabilities.dimmable) next.brightness = change.brightness;
    } else if (change.field === "color") {
      if (light.capabilities.color_rgb) {
        next.color = rgbToXy(...hexToRgb(change.hex));
        next.color_temp_mirek = undefined;
      }
    } else if (change.field === "temp") {
      if (light.capabilities.color_temperature) {
        next.color_temp_mirek = change.mirek;
        next.color = undefined;
      }
    } else if (change.field === "effect") {
      next.effect = change.effect;
    }
    onLightUpdate(light.id, next);
    clearTimeout(commitTimer.current);
    commitTimer.current = setTimeout(() => { setLightState(light.id, next); }, 200);
  }

  async function toggle() {
    const next: LightState = { ...(light.last_state ?? { on: false }), on: !isOn };
    onLightUpdate(light.id, next);
    await setLightState(light.id, next);
    onChanged();
  }

  return (
    <>
      <GlyphButton
        on={isOn}
        accent={isOn ? hex : "#ffb84d"}
        offline={offline}
        title={light.name}
        active={editing}
        buttonRef={ref}
        onClick={() => setEditing((v) => !v)}
        onLongPress={toggle}
      >
        <Glyph name={light.glyph ?? "bulb"} />
      </GlyphButton>
      {editing && ref.current && (
        <LightEditor
          anchor={ref.current}
          title={light.name}
          initialHex={hex}
          initialBrightness={brightness}
          initialMirek={mirek}
          initialMode={whiteMode ? "white" : "color"}
          showColor={light.capabilities.color_rgb}
          showWhite={light.capabilities.color_temperature}
          showBrightness={light.capabilities.dimmable}
          effects={light.capabilities.effects}
          initialEffect={light.last_state?.effect}
          on={isOn}
          onToggle={toggle}
          onChange={handleEditorChange}
          onClose={() => setEditing(false)}
        >
          <DisableRow
            enabled={light.enabled !== false}
            onSetEnabled={(en) => { onSetEnabled(light.id, en); if (!en) setEditing(false); }}
          />
        </LightEditor>
      )}
    </>
  );
}

function PowerButton({
  device,
  onToggle,
  onSetEnabled,
}: {
  device: PowerDevice;
  onToggle: (id: string, next: boolean) => void;
  onSetEnabled: (id: string, enabled: boolean) => void;
}) {
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const offline = device.state.reachable === false;
  return (
    <>
      <GlyphButton
        on={device.state.on}
        accent={T.accent}
        offline={offline}
        title={device.name}
        active={open}
        buttonRef={ref}
        onClick={() => setOpen((v) => !v)}
        onLongPress={() => onToggle(device.id, !device.state.on)}
      >
        <Glyph name={device.glyph ?? powerKindGlyph(device.kind)} />
      </GlyphButton>
      {open && ref.current && (
        <PowerFlyout
          device={device}
          anchor={ref.current}
          onToggle={(next) => onToggle(device.id, next)}
          onSetEnabled={(en) => onSetEnabled(device.id, en)}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );
}

function AudioButton({
  device,
  groupMembers,
  onAudioPatch,
  onSetEnabled,
  receiverName,
}: {
  device: AudioDevice;
  /** When set (≥2), this button represents a live sync group coordinated by
   * `device`; it shows the group glyph and lists the members. */
  groupMembers?: AudioDevice[];
  onAudioPatch: (id: string, patch: Partial<AudioDevice["state"]>) => void;
  onSetEnabled: (id: string, enabled: boolean) => void;
  /** M22: name of the receiver this source's volume routes to, if bound. */
  receiverName?: string;
}) {
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const offline = device.state.reachable === false;
  const grouped = !!groupMembers && groupMembers.length >= 2;
  const title = grouped ? groupMembers!.map((m) => m.name).join(" + ") : device.name;
  function togglePower() {
    const next = !device.state.power;
    onAudioPatch(device.id, { power: next });
    setAudioState(device.id, { power: next });
  }
  return (
    <>
      <GlyphButton
        on={device.state.power}
        accent={T.audio}
        offline={offline}
        title={title}
        active={open}
        buttonRef={ref}
        onClick={() => setOpen((v) => !v)}
        onLongPress={togglePower}
      >
        <Glyph name={grouped ? "speaker_group" : (device.glyph ?? audioKindGlyph(device.kind))} />
      </GlyphButton>
      {open && ref.current && (
        <AudioEditor
          device={device}
          anchor={ref.current}
          onLocalPatch={onAudioPatch}
          onSetEnabled={(en) => { onSetEnabled(device.id, en); if (!en) setOpen(false); }}
          onClose={() => setOpen(false)}
          receiverName={receiverName}
        />
      )}
    </>
  );
}

