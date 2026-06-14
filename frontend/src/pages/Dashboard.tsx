import { useEffect, useRef, useState } from "react";
import {
  applySceneToRoom,
  getAudioDevices,
  getPaletteScenes,
  getPowerDevices,
  getProviders,
  getRooms,
  mergePatch,
  rgbToHex,
  rgbToXy,
  savePaletteSceneFromRoom,
  setAudioEnabled,
  setLightEnabled,
  setLightState,
  setPowerEnabled,
  setPowerState,
  setRoomState,
  xyToRgb,
  type AudioDevice,
  type Light,
  type LightState,
  type LightStatePatch,
  type PaletteScene,
  type PowerDevice,
  type Provider,
  type Room,
} from "../api";
import { AudioEditor } from "../components/AudioControls";
import { Glyph, powerKindGlyph, audioKindGlyph } from "../components/glyphs";
import { hexToRgb, LightEditor } from "../components/LightEditor";
import { DisableRow, PowerFlyout } from "../components/PowerFlyout";
import { SceneButton, SceneModal } from "../components/scenes";
import { useDialogs, type Dialogs } from "../components/dialogs";
import { useViewport } from "../useViewport";

// ── Lamplight theme ──────────────────────────────────────────────────────────
const T = {
  text: "#eae4d6",
  dim: "#97907e",
  faint: "#6b6557",
  accent: "#38bdf8",
  audio: "#a78bfa",
  panel: "linear-gradient(176deg, #1a1916 0%, #141311 100%)",
  panelBorder: "#2b2822",
  card: "#1d1c18",
  cardOff: "#171613",
  cardBorder: "#2c2922",
  hairline: "#242118",
};

const label: React.CSSProperties = {
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
  const { isMobile } = useViewport();
  const [localLights, setLocalLights] = useState<Light[]>(lights);
  const [powerDevices, setPowerDevices] = useState<PowerDevice[]>([]);
  const [audioDevices, setAudioDevices] = useState<AudioDevice[]>([]);
  const [providers, setProviders] = useState<Provider[]>([]);
  const [rooms, setRooms] = useState<Room[]>([]);
  const [scenes, setScenes] = useState<PaletteScene[]>([]);
  const dialogs = useDialogs();

  function loadScenes() {
    getPaletteScenes().then(setScenes);
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

  // Real-time state: light_state (Hue SSE) and audio_state (Onkyo push).
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

  return (
    <div style={{ padding: isMobile ? "1rem 0.85rem" : "2rem", maxWidth: 1100, margin: "0 auto", color: T.text }}>
      <header style={{ marginBottom: "1.4rem" }}>
        <div style={{ display: "flex", alignItems: "baseline", gap: "0.9rem" }}>
          <h1 style={{ ...label, margin: 0, fontSize: "1rem", letterSpacing: "0.22em", color: T.text }}>
            Control
          </h1>
          {localLights.length > 0 && (
            <span style={{ fontSize: "0.78rem", color: T.dim }}>
              {onCount} of {localLights.length} lights on
            </span>
          )}
        </div>
        <div
          aria-hidden
          style={{
            marginTop: "0.7rem",
            height: 1,
            background:
              "linear-gradient(90deg, rgba(56,189,248,0.55), rgba(167,139,250,0.3) 35%, rgba(244,114,182,0.18) 70%, transparent)",
          }}
        />
      </header>

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
      {roomSections.map(({ room, lights, power, audio }) => (
        <RoomBox key={room.id} name={room.name} roomId={room.id} lights={lights} power={power} audio={audio} {...common} />
      ))}
      {leftoverSections.map(([providerId, sectionLights]) => (
        <RoomBox
          key={providerId}
          name={
            roomSections.length > 0
              ? `${providerName.get(providerId) ?? "Other"} · no room`
              : providerName.get(providerId) ?? "Other"
          }
          lights={sectionLights}
          power={[]}
          audio={[]}
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

/** A room: framed box with a gradient ridge from its lit lights, a header with
 * room-wide light controls (color/brightness cascade + scenes + on/off), and a
 * row of one glyph button per member device. Each device opens its own fly-out. */
function RoomBox({
  name,
  roomId,
  lights,
  power,
  audio,
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
  name: string;
  roomId?: string;
  lights: Light[];
  power: PowerDevice[];
  audio: AudioDevice[];
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
  const showColor = lights.some((l) => l.capabilities.color_rgb);
  const showBrightness = lights.some((l) => l.capabilities.dimmable);
  const tunable = !!roomId && (showColor || showBrightness);

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

  const ridge =
    hexes.length > 1
      ? `linear-gradient(90deg, ${hexes.map((h, i) => `${h} ${Math.round((i / (hexes.length - 1)) * 100)}%`).join(", ")})`
      : hexes.length === 1
        ? `linear-gradient(90deg, ${hexes[0]}, ${hexes[0]}33)`
        : "linear-gradient(90deg, rgba(56,189,248,0.35), transparent 70%)";

  function cascade(nextHex: string, nextBrightness: number) {
    if (!roomId) return;
    const color = showColor ? rgbToXy(...hexToRgb(nextHex)) : undefined;
    for (const l of lights) {
      onLightUpdate(l.id, {
        ...(l.last_state ?? { on: true }),
        on: true,
        brightness: l.capabilities.dimmable ? nextBrightness : l.last_state?.brightness,
        color: l.capabilities.color_rgb && color ? color : l.last_state?.color,
      });
    }
    clearTimeout(commitTimer.current);
    commitTimer.current = setTimeout(() => { setRoomState(roomId, { on: true, brightness: nextBrightness, color }); }, 200);
  }

  async function toggleAll() {
    if (!roomId) return;
    const next = !anyOn;
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

  const hasLights = lights.length > 0;

  return (
    <section
      style={{
        breakInside: "avoid",
        marginBottom: "1.1rem",
        background: roomId ? T.panel : "transparent",
        border: `1px solid ${roomId ? T.panelBorder : T.hairline}`,
        borderStyle: roomId ? "solid" : "dashed",
        borderRadius: 16,
        overflow: "hidden",
        boxShadow: roomId ? "inset 0 1px 0 rgba(255,255,255,0.035)" : "none",
      }}
    >
      <div aria-hidden style={{ height: 2, background: ridge, opacity: anyOn ? 0.9 : 0.5 }} />

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
        <span style={{ ...label, fontSize: "0.8rem", color: roomId ? "#d8cfba" : T.faint, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {name}
        </span>
        <span style={{ fontSize: "0.7rem", color: T.faint, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {subtitle}
        </span>
        <span style={{ flex: 1 }} />
        {roomId && hasLights && <VerticalToggle on={anyOn} onToggle={toggleAll} disabled={busy} />}
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
          showColor={showColor}
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
  children,
}: {
  on: boolean;
  accent: string;
  offline?: boolean;
  title: string;
  active: boolean;
  buttonRef: React.Ref<HTMLButtonElement>;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      ref={buttonRef}
      onClick={onClick}
      title={title}
      aria-label={title}
      style={{
        width: 52,
        height: 52,
        flexShrink: 0,
        display: "grid",
        placeItems: "center",
        borderRadius: 12,
        cursor: "pointer",
        color: on ? accent : T.dim,
        background: on
          ? `radial-gradient(120% 120% at 50% 0%, ${accent}22, transparent 60%), ${T.card}`
          : T.cardOff,
        border: `1px solid ${active ? T.accent : on ? `${accent}55` : T.cardBorder}`,
        boxShadow: on ? `0 0 20px -8px ${accent}` : "inset 0 1px 0 rgba(255,255,255,0.03)",
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

  function handleEditorChange(nextHex: string, nextBrightness: number) {
    const next: LightState = {
      ...(light.last_state ?? { on: true }),
      on: true,
      brightness: light.capabilities.dimmable ? nextBrightness : light.last_state?.brightness,
      color: light.capabilities.color_rgb ? rgbToXy(...hexToRgb(nextHex)) : light.last_state?.color,
    };
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
      >
        <Glyph name={light.glyph ?? "bulb"} />
      </GlyphButton>
      {editing && ref.current && (
        <LightEditor
          anchor={ref.current}
          title={light.name}
          initialHex={hex}
          initialBrightness={brightness}
          showColor={light.capabilities.color_rgb}
          showBrightness={light.capabilities.dimmable}
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

/** On/off as a vertical sliding switch — up is on. */
function VerticalToggle({ on, onToggle, disabled }: { on: boolean; onToggle: () => void; disabled?: boolean }) {
  return (
    <button
      onClick={(e) => { e.stopPropagation(); onToggle(); }}
      disabled={disabled}
      aria-label={on ? "Turn off" : "Turn on"}
      style={{
        flexShrink: 0,
        width: 24,
        height: 44,
        borderRadius: 12,
        border: `1px solid ${on ? "rgba(125,211,252,0.55)" : "rgba(255,255,255,0.12)"}`,
        cursor: disabled ? "default" : "pointer",
        background: on
          ? "linear-gradient(180deg, rgba(125,211,252,0.45), rgba(34,211,238,0.12) 70%), rgba(10,25,36,0.55)"
          : "rgba(255,255,255,0.055)",
        boxShadow: on
          ? "0 0 16px -4px rgba(56,189,248,0.75), inset 0 1px 0 rgba(255,255,255,0.25)"
          : "inset 0 1px 0 rgba(255,255,255,0.06)",
        backdropFilter: "blur(4px)",
        WebkitBackdropFilter: "blur(4px)",
        position: "relative",
        transition: "background 0.2s, box-shadow 0.2s, border-color 0.2s",
      }}
    >
      <span
        style={{
          position: "absolute",
          left: 2,
          top: on ? 2 : 22,
          width: 18,
          height: 18,
          borderRadius: "50%",
          background: on ? "linear-gradient(180deg, #ffffff, #d6f1ff)" : "rgba(255,255,255,0.4)",
          boxShadow: on ? "0 0 8px rgba(125,211,252,0.9), 0 1px 2px rgba(0,0,0,0.45)" : "0 1px 2px rgba(0,0,0,0.35)",
          transition: "top 0.2s, background 0.2s, box-shadow 0.2s",
        }}
      />
    </button>
  );
}
