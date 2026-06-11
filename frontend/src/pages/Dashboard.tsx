import { useEffect, useRef, useState } from "react";
import {
  activateScene,
  createScene,
  getProviders,
  getRooms,
  getScenes,
  mergePatch,
  removeScene,
  rgbToHex,
  rgbToXy,
  setLightState,
  setRoomState,
  xyToRgb,
  type Light,
  type LightState,
  type LightStatePatch,
  type Provider,
  type Room,
  type Scene,
} from "../api";
import { hexToRgb, LightEditor } from "../components/LightEditor";
import { useDialogs } from "../components/dialogs";
import { S } from "../styles";

interface Props {
  lights: Light[];
  onRefresh: () => void;
  onNavigate: (page: "settings") => void;
}

export function DashboardPage({ lights, onRefresh, onNavigate }: Props) {
  // Local copy so SSE events can update individual lights without a full server round-trip.
  const [localLights, setLocalLights] = useState<Light[]>(lights);
  const [providers, setProviders] = useState<Provider[]>([]);
  const [rooms, setRooms] = useState<Room[]>([]);

  // Keep in sync when the parent does a full refresh (authoritative server state wins).
  useEffect(() => { setLocalLights(lights); }, [lights]);
  useEffect(() => { getProviders().then(setProviders); }, []);
  // Re-fetch rooms alongside light refreshes so membership stays current.
  useEffect(() => { getRooms().then(setRooms); }, [lights]);

  // Real-time light state from Hue SSE → our SSE → browser.
  useEffect(() => {
    const es = new EventSource("/api/events");

    es.addEventListener("light_state", (raw) => {
      const { device_id, patch } = JSON.parse((raw as MessageEvent).data) as {
        device_id: string;
        patch: LightStatePatch;
      };
      setLocalLights((prev) =>
        prev.map((l) =>
          l.device_id === device_id ? { ...l, last_state: mergePatch(l.last_state, patch) } : l
        )
      );
    });

    // Browser reconnects automatically on error; nothing to do here.
    es.onerror = () => {};

    return () => es.close();
  }, []); // open once per mount — reconnect is handled by the browser

  function handleLocalUpdate(id: string, state: LightState) {
    setLocalLights((prev) =>
      prev.map((l) => (l.id === id ? { ...l, last_state: state } : l))
    );
  }

  return (
    <div style={{ padding: "2rem", maxWidth: 960, margin: "0 auto" }}>
      {localLights.length > 0 && <SceneBar onActivated={onRefresh} />}
      {localLights.length === 0 ? (
        <div style={{ textAlign: "center", padding: "4rem 0", color: "#666" }}>
          <p style={{ margin: "0 0 0.75rem" }}>No lights found.</p>
          <p style={{ margin: 0, fontSize: "0.875rem" }}>
            Add a provider in{" "}
            <button
              onClick={() => onNavigate("settings")}
              style={{
                background: "none",
                border: "none",
                color: "#f90",
                cursor: "pointer",
                fontSize: "0.875rem",
                padding: 0,
              }}
            >
              Settings
            </button>{" "}
            and run discovery.
          </p>
        </div>
      ) : (
        <RoomSections
          lights={localLights}
          rooms={rooms}
          providers={providers}
          onLocalUpdate={handleLocalUpdate}
          onChanged={onRefresh}
        />
      )}
    </div>
  );
}

/**
 * Lights grouped under one section per room, with all-on/all-off in the
 * header. Lights that belong to no room fall back to per-provider sections.
 */
function RoomSections({
  lights,
  rooms,
  providers,
  onLocalUpdate,
  onChanged,
}: {
  lights: Light[];
  rooms: Room[];
  providers: Provider[];
  onLocalUpdate: (id: string, state: LightState) => void;
  onChanged: () => void;
}) {
  const lightById = new Map(lights.map((l) => [l.id, l]));
  const assigned = new Set<string>();

  const roomSections = rooms
    .map((room) => {
      const members = room.light_ids
        .map((id) => lightById.get(id))
        .filter((l): l is Light => l !== undefined);
      for (const l of members) assigned.add(l.id);
      return { room, members };
    })
    .filter((s) => s.members.length > 0)
    .sort((a, b) => a.room.name.localeCompare(b.room.name));

  const providerName = new Map(providers.map((p) => [p.id, p.name]));
  const leftovers = new Map<string, Light[]>();
  for (const l of lights) {
    if (assigned.has(l.id)) continue;
    leftovers.set(l.provider_id, [...(leftovers.get(l.provider_id) ?? []), l]);
  }
  const leftoverSections = [...leftovers.entries()].sort((a, b) =>
    (providerName.get(a[0]) ?? "").localeCompare(providerName.get(b[0]) ?? ""),
  );

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "1.5rem" }}>
      {roomSections.map(({ room, members }) => (
        <LightSection
          key={room.id}
          title={room.name}
          lights={members}
          roomId={room.id}
          onLocalUpdate={onLocalUpdate}
          onChanged={onChanged}
        />
      ))}
      {leftoverSections.map(([providerId, sectionLights]) => (
        <LightSection
          key={providerId}
          title={roomSections.length > 0
            ? `${providerName.get(providerId) ?? "Other"} — no room`
            : providerName.get(providerId) ?? "Other"}
          lights={sectionLights}
          onLocalUpdate={onLocalUpdate}
          onChanged={onChanged}
        />
      ))}
    </div>
  );
}

function LightSection({
  title,
  lights,
  roomId,
  onLocalUpdate,
  onChanged,
}: {
  title: string;
  lights: Light[];
  /** When set, the header gets room-wide On/Off buttons. */
  roomId?: string;
  onLocalUpdate: (id: string, state: LightState) => void;
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState(false);

  async function setAll(on: boolean) {
    if (!roomId) return;
    setBusy(true);
    try {
      await setRoomState(roomId, { on });
      onChanged();
    } finally {
      setBusy(false);
    }
  }

  return (
    <section>
      <h2
        style={{
          margin: "0 0 0.6rem",
          fontSize: "0.8rem",
          fontWeight: 600,
          color: "#777",
          textTransform: "uppercase",
          letterSpacing: "0.08em",
          display: "flex",
          alignItems: "center",
          gap: "0.5rem",
        }}
      >
        {title}
        <span style={{ color: "#555", textTransform: "none", letterSpacing: 0 }}>
          {lights.length} light{lights.length !== 1 ? "s" : ""}
        </span>
        {roomId && (
          <span style={{ display: "inline-flex", gap: "0.35rem", marginLeft: "auto" }}>
            <button
              onClick={() => setAll(true)}
              disabled={busy}
              style={{ ...S.buttonGhost, padding: "0.2rem 0.55rem", fontSize: "0.72rem" }}
            >
              On
            </button>
            <button
              onClick={() => setAll(false)}
              disabled={busy}
              style={{ ...S.buttonGhost, padding: "0.2rem 0.55rem", fontSize: "0.72rem" }}
            >
              Off
            </button>
          </span>
        )}
      </h2>
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(auto-fill, minmax(220px, 1fr))",
          gap: "1rem",
        }}
      >
        {lights.map((light) => (
          <LightCard
            key={light.id}
            light={light}
            onLocalUpdate={onLocalUpdate}
            onChanged={onChanged}
          />
        ))}
      </div>
    </section>
  );
}

function SceneBar({ onActivated }: { onActivated: () => void }) {
  const dialogs = useDialogs();
  const [scenes, setScenes] = useState<Scene[]>([]);
  const [busy, setBusy] = useState("");

  async function load() {
    setScenes(await getScenes());
  }

  useEffect(() => { load(); }, []);

  async function handleActivate(id: string) {
    setBusy(id);
    try {
      await activateScene(id);
      onActivated(); // refresh light states from the server
    } finally {
      setBusy("");
    }
  }

  async function handleSave() {
    const name = await dialogs.prompt({
      title: "Save scene",
      message: "Saves the current state of all lights.",
      placeholder: "Scene name",
      confirmLabel: "Save",
    });
    if (!name?.trim()) return;
    await createScene(name.trim());
    await load();
  }

  async function handleRemove(id: string, name: string) {
    const ok = await dialogs.confirm({
      title: "Delete scene",
      message: `Delete scene "${name}"?`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!ok) return;
    await removeScene(id);
    await load();
  }

  return (
    <div style={{ display: "flex", flexWrap: "wrap", gap: "0.5rem", alignItems: "center", marginBottom: "1.25rem" }}>
      {scenes.map((s) => (
        <span key={s.id} style={{ display: "inline-flex" }}>
          <button
            onClick={() => handleActivate(s.id)}
            disabled={busy === s.id}
            title={`Apply "${s.name}" (${s.lights} light${s.lights !== 1 ? "s" : ""})`}
            style={{ ...S.buttonGhost, borderRadius: "6px 0 0 6px" }}
          >
            {busy === s.id ? "…" : s.name}
          </button>
          <button
            onClick={() => handleRemove(s.id, s.name)}
            title="Delete scene"
            style={{ ...S.buttonGhost, borderRadius: "0 6px 6px 0", borderLeft: "none", padding: "0.45rem 0.55rem", color: "#866" }}
          >
            ×
          </button>
        </span>
      ))}
      <button onClick={handleSave} style={S.buttonGhost} title="Save the current light states as a scene">
        + Save scene
      </button>
      {dialogs.element}
    </div>
  );
}

function LightCard({
  light,
  onLocalUpdate,
  onChanged,
}: {
  light: Light;
  onLocalUpdate: (id: string, state: LightState) => void;
  onChanged: () => void;
}) {
  const cardRef = useRef<HTMLDivElement>(null);
  const [editing, setEditing] = useState(false);
  const commitTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const isOn = light.last_state?.on ?? false;
  const offline = light.last_state?.reachable === false;

  const serverColor = light.last_state?.color;
  const hex = serverColor
    ? rgbToHex(...xyToRgb(serverColor.x, serverColor.y, serverColor.brightness))
    : "#ffb84d";
  const brightness = light.last_state?.brightness ?? 100;
  const editable = !offline && (light.capabilities.color_rgb || light.capabilities.dimmable);

  function handleEditorChange(nextHex: string, nextBrightness: number) {
    const next: LightState = {
      ...(light.last_state ?? { on: true }),
      on: true,
      brightness: light.capabilities.dimmable ? nextBrightness : light.last_state?.brightness,
      color: light.capabilities.color_rgb ? rgbToXy(...hexToRgb(nextHex)) : light.last_state?.color,
    };
    onLocalUpdate(light.id, next);
    clearTimeout(commitTimer.current);
    commitTimer.current = setTimeout(() => { setLightState(light.id, next); }, 200);
  }

  async function toggle() {
    const next: LightState = { ...(light.last_state ?? { on: false }), on: !isOn };
    onLocalUpdate(light.id, next);   // optimistic update
    await setLightState(light.id, next);
    onChanged();                      // fallback full refresh (catches Govee etc.)
  }

  return (
    <>
      <div
        ref={cardRef}
        onClick={() => { if (editable) setEditing(true); }}
        title={editable ? "Open the light editor" : undefined}
        style={{
          ...S.card,
          flexDirection: "row",
          alignItems: "center",
          justifyContent: "space-between",
          gap: "0.75rem",
          cursor: editable ? "pointer" : "default",
          opacity: offline ? 0.45 : isOn ? 1 : 0.6,
          transition: "opacity 0.2s",
          ...(editing ? { outline: "1px solid #f90" } : {}),
        }}
      >
        <div style={{ minWidth: 0, display: "flex", flexDirection: "column", gap: "0.3rem" }}>
          <span
            style={{
              fontWeight: 600,
              fontSize: "0.95rem",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {light.name}
          </span>
          <span style={{ display: "inline-flex", alignItems: "center", gap: "0.4rem", fontSize: "0.75rem", color: "#888" }}>
            {isOn && light.capabilities.color_rgb && (
              <span style={{ width: 10, height: 10, borderRadius: "50%", background: hex, border: "1px solid rgba(255,255,255,0.25)", display: "inline-block" }} />
            )}
            {isOn ? (light.capabilities.dimmable ? `${brightness}%` : "on") : "off"}
          </span>
        </div>
        {offline ? (
          <span
            title="The provider reports this device as unreachable"
            style={{
              flexShrink: 0,
              fontSize: "0.7rem",
              color: "#c66",
              border: "1px solid #533",
              borderRadius: 4,
              padding: "0.1rem 0.4rem",
            }}
          >
            offline
          </span>
        ) : (
          <VerticalToggle on={isOn} onToggle={toggle} />
        )}
      </div>
      {editing && cardRef.current && (
        <LightEditor
          anchor={cardRef.current}
          title={light.name}
          initialHex={hex}
          initialBrightness={brightness}
          showColor={light.capabilities.color_rgb}
          showBrightness={light.capabilities.dimmable}
          on={isOn}
          onToggle={toggle}
          onChange={handleEditorChange}
          onClose={() => setEditing(false)}
        />
      )}
    </>
  );
}

/** On/off as a vertical sliding switch — up is on. */
function VerticalToggle({ on, onToggle }: { on: boolean; onToggle: () => void }) {
  return (
    <button
      onClick={(e) => { e.stopPropagation(); onToggle(); }}
      aria-label={on ? "Turn off" : "Turn on"}
      style={{
        flexShrink: 0,
        width: 24,
        height: 44,
        borderRadius: 12,
        border: "none",
        cursor: "pointer",
        background: on ? "#f90" : "#444",
        position: "relative",
        transition: "background 0.2s",
      }}
    >
      <span
        style={{
          position: "absolute",
          left: 3,
          top: on ? 3 : 23,
          width: 18,
          height: 18,
          borderRadius: "50%",
          background: "#fff",
          transition: "top 0.2s",
        }}
      />
    </button>
  );
}
