import { useEffect, useRef, useState } from "react";
import {
  applySceneToRoom,
  getPaletteScenes,
  getProviders,
  getRooms,
  mergePatch,
  rgbToHex,
  rgbToXy,
  savePaletteSceneFromRoom,
  setLightState,
  setRoomState,
  xyToRgb,
  type Light,
  type LightState,
  type LightStatePatch,
  type PaletteScene,
  type Provider,
  type Room,
} from "../api";
import { hexToRgb, LightEditor } from "../components/LightEditor";
import { SceneButton, SceneModal } from "../components/scenes";
import { useDialogs, type Dialogs } from "../components/dialogs";
import { useViewport } from "../useViewport";

// ── Lamplight theme ──────────────────────────────────────────────────────────
// Warm-tinted darks (charcoal with a candle cast) instead of neutral grays;
// lit cards glow in their actual color. Deliberately not a stock component look.
const T = {
  text: "#eae4d6",
  dim: "#97907e",
  faint: "#6b6557",
  accent: "#38bdf8",
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
  // Local copy so SSE events can update individual lights without a full server round-trip.
  const [localLights, setLocalLights] = useState<Light[]>(lights);
  const [providers, setProviders] = useState<Provider[]>([]);
  const [rooms, setRooms] = useState<Room[]>([]);
  const [scenes, setScenes] = useState<PaletteScene[]>([]);
  const dialogs = useDialogs();

  function loadScenes() {
    getPaletteScenes().then(setScenes);
  }

  // Keep in sync when the parent does a full refresh (authoritative server state wins).
  useEffect(() => { setLocalLights(lights); }, [lights]);
  useEffect(() => { getProviders().then(setProviders); }, []);
  useEffect(() => { loadScenes(); }, []);
  // Re-fetch rooms alongside light refreshes so membership stays current.
  // Keep all rooms (incl. disabled) so their members still count as "assigned";
  // RoomSections renders only the enabled ones, so a disabled room's lights are
  // hidden rather than leaking into the "no room" bucket.
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

  const onCount = localLights.filter((l) => l.last_state?.on).length;

  return (
    <div style={{ padding: isMobile ? "1rem 0.85rem" : "2rem", maxWidth: 1020, margin: "0 auto", color: T.text }}>
      <header style={{ marginBottom: "1.4rem" }}>
        <div style={{ display: "flex", alignItems: "baseline", gap: "0.9rem" }}>
          <h1 style={{ ...label, margin: 0, fontSize: "1rem", letterSpacing: "0.22em", color: T.text }}>
            Lights
          </h1>
          {localLights.length > 0 && (
            <span style={{ fontSize: "0.78rem", color: T.dim }}>
              {onCount} of {localLights.length} on
            </span>
          )}
        </div>
        {/* A faint bifrost rule under the page title. */}
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

      {localLights.length === 0 ? (
        <div style={{ textAlign: "center", padding: "4rem 0", color: T.faint }}>
          <p style={{ margin: "0 0 0.75rem" }}>No lights found.</p>
          <p style={{ margin: 0, fontSize: "0.875rem" }}>
            Add a provider in{" "}
            <button
              onClick={() => onNavigate("settings")}
              style={{
                background: "none",
                border: "none",
                color: T.accent,
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
          scenes={scenes}
          dialogs={dialogs}
          onScenesChanged={loadScenes}
          onLocalUpdate={handleLocalUpdate}
          onChanged={onRefresh}
        />
      )}
      {dialogs.element}
    </div>
  );
}

/**
 * Lights grouped into one framed box per room; the box header carries the
 * room-wide controls. Lights in no room land in muted per-provider boxes.
 */
function RoomSections({
  lights,
  rooms,
  providers,
  scenes,
  dialogs,
  onScenesChanged,
  onLocalUpdate,
  onChanged,
}: {
  lights: Light[];
  rooms: Room[];
  providers: Provider[];
  scenes: PaletteScene[];
  dialogs: Dialogs;
  onScenesChanged: () => void;
  onLocalUpdate: (id: string, state: LightState) => void;
  onChanged: () => void;
}) {
  const { isMobile } = useViewport();
  const lightById = new Map(lights.map((l) => [l.id, l]));
  const assigned = new Set<string>();

  const roomSections = rooms
    .map((room) => {
      const members = room.light_ids
        .map((id) => lightById.get(id))
        .filter((l): l is Light => l !== undefined);
      // Mark members of every room (even disabled) as assigned, so a disabled
      // room's lights don't fall through into the "no room" buckets below.
      for (const l of members) assigned.add(l.id);
      return { room, members };
    })
    .filter((s) => s.members.length > 0 && s.room.enabled)
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
    <div style={{ display: "flex", flexDirection: "column", gap: isMobile ? "0.7rem" : "1.1rem" }}>
      {roomSections.map(({ room, members }) => (
        <RoomBox
          key={room.id}
          name={room.name}
          lights={members}
          roomId={room.id}
          scenes={scenes}
          dialogs={dialogs}
          onScenesChanged={onScenesChanged}
          onLocalUpdate={onLocalUpdate}
          onChanged={onChanged}
        />
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
          scenes={scenes}
          dialogs={dialogs}
          onScenesChanged={onScenesChanged}
          onLocalUpdate={onLocalUpdate}
          onChanged={onChanged}
        />
      ))}
    </div>
  );
}

/** Hex colors of the lit members (name order) for the room's gradient ridge. */
function litHexes(lights: Light[]): string[] {
  return lights
    .filter((l) => l.last_state?.on && l.last_state.color)
    .map((l) => {
      const c = l.last_state!.color!;
      return rgbToHex(...xyToRgb(c.x, c.y, c.brightness));
    });
}

/**
 * A room as a container: framed box, gradient ridge echoing the lit members'
 * colors, header with the room's own color/brightness widget (cascades to all
 * members) and a single on/off toggle. Lights stay individually editable inside.
 * Without `roomId` (unassigned lights) the frame is muted and uncontrolled.
 */
function RoomBox({
  name,
  lights,
  roomId,
  scenes,
  dialogs,
  onScenesChanged,
  onLocalUpdate,
  onChanged,
}: {
  name: string;
  lights: Light[];
  roomId?: string;
  scenes: PaletteScene[];
  dialogs: Dialogs;
  onScenesChanged: () => void;
  onLocalUpdate: (id: string, state: LightState) => void;
  onChanged: () => void;
}) {
  const { isMobile } = useViewport();
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

  // A room with exactly one light is redundant — the header's room toggle
  // already controls that light. Collapse to just the header (no inner card),
  // surfacing the light's name + level in the subtitle.
  const collapsed = !!roomId && lights.length === 1;
  const subtitle = collapsed
    ? (() => {
        const one = lights[0];
        const st = one.last_state;
        const tail = st?.on
          ? one.capabilities.dimmable
            ? ` · ${Math.round(st.brightness ?? 100)}%`
            : " · on"
          : " · off";
        return `${one.name}${tail}`;
      })()
    : `${lights.length} light${lights.length !== 1 ? "s" : ""}${
        roomId ? (anyOn ? ` · ${lit.length} on` : " · off") : ""
      }`;

  const hexes = litHexes(lights);
  const roomHex = hexes[0] ?? "#ffb84d";
  const avgBrightness = lit.length
    ? Math.round(lit.reduce((sum, l) => sum + (l.last_state?.brightness ?? 100), 0) / lit.length)
    : 100;

  // The ridge along the top of the box: lit members' colors as a gradient,
  // a faint ember when the room is dark.
  const ridge =
    hexes.length > 1
      ? `linear-gradient(90deg, ${hexes
          .map((h, i) => `${h} ${Math.round((i / (hexes.length - 1)) * 100)}%`)
          .join(", ")})`
      : hexes.length === 1
        ? `linear-gradient(90deg, ${hexes[0]}, ${hexes[0]}33)`
        : "linear-gradient(90deg, rgba(56,189,248,0.35), transparent 70%)";

  /** Drive every member from the room widget; members keep their own editors. */
  function cascade(nextHex: string, nextBrightness: number) {
    if (!roomId) return;
    const color = showColor ? rgbToXy(...hexToRgb(nextHex)) : undefined;
    for (const l of lights) {
      onLocalUpdate(l.id, {
        ...(l.last_state ?? { on: true }),
        on: true,
        brightness: l.capabilities.dimmable ? nextBrightness : l.last_state?.brightness,
        color: l.capabilities.color_rgb && color ? color : l.last_state?.color,
      });
    }
    clearTimeout(commitTimer.current);
    commitTimer.current = setTimeout(() => {
      setRoomState(roomId, { on: true, brightness: nextBrightness, color });
    }, 200);
  }

  async function toggleAll() {
    if (!roomId) return;
    const next = !anyOn;
    setBusy(true);
    for (const l of lights) {
      onLocalUpdate(l.id, { ...(l.last_state ?? { on: false }), on: next });
    }
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

  /** Capture this room's current colors + brightness as a new global scene. */
  async function saveAsScene() {
    if (!roomId) return;
    if (!anyOn) {
      await dialogs.alert({
        title: "Nothing to save",
        message: "No lights in this room are on — turn some on first.",
      });
      return;
    }
    const name = await dialogs.prompt({
      title: "Save room as scene",
      message: "Saves this room's current colors and brightness as a reusable scene.",
      placeholder: "Scene name",
      confirmLabel: "Save",
    });
    if (!name?.trim()) return;
    try {
      await savePaletteSceneFromRoom(roomId, name.trim());
      onScenesChanged();
    } catch (e) {
      await dialogs.alert({ title: "Couldn't save scene", message: String(e) });
    }
  }

  return (
    <section
      style={{
        background: roomId ? T.panel : "transparent",
        border: `1px solid ${roomId ? T.panelBorder : T.hairline}`,
        borderStyle: roomId ? "solid" : "dashed",
        borderRadius: 16,
        overflow: "hidden",
        boxShadow: roomId ? "inset 0 1px 0 rgba(255,255,255,0.035)" : "none",
      }}
    >
      <div aria-hidden style={{ height: 2, background: ridge, opacity: anyOn ? 0.9 : 0.5 }} />

      {/* The whole header is the room's color/brightness trigger. */}
      <header
        ref={tuneRef}
        onClick={() => { if (tunable) setEditing((v) => !v); }}
        title={tunable ? "Set the whole room's color and brightness" : undefined}
        style={{
          display: "flex",
          alignItems: "center",
          gap: isMobile ? "0.5rem" : "0.7rem",
          padding: isMobile ? "0.5rem 0.7rem 0.45rem" : "0.75rem 1rem 0.7rem",
          borderBottom: collapsed ? "none" : `1px solid ${T.hairline}`,
          cursor: tunable ? "pointer" : "default",
        }}
      >
        {roomId && (
          // A color dot echoing the room's light — purely indicative now; the
          // whole header opens the palette.
          <span
            aria-hidden
            style={{
              width: 18,
              height: 18,
              flexShrink: 0,
              borderRadius: "50%",
              border: "1px solid rgba(255,255,255,0.22)",
              background: anyOn
                ? `radial-gradient(circle at 35% 30%, #ffffff44, transparent 45%), ${roomHex}`
                : "#3a372e",
              boxShadow: anyOn ? `0 0 12px -3px ${roomHex}` : "none",
              transition: "box-shadow 0.2s, background 0.2s",
            }}
          />
        )}
        <span
          style={{
            ...label,
            fontSize: "0.82rem",
            color: roomId ? "#d8cfba" : T.faint,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {name}
        </span>
        <span style={{ fontSize: "0.72rem", color: T.faint, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {subtitle}
        </span>

        <span style={{ flex: 1 }} />

        {roomId && <VerticalToggle on={anyOn} onToggle={toggleAll} disabled={busy} />}
      </header>

      {!collapsed && (
        <div
          style={{
            display: "grid",
            gridTemplateColumns: isMobile
              ? "repeat(2, minmax(0, 1fr))"
              : "repeat(auto-fill, minmax(215px, 1fr))",
            gap: isMobile ? "0.4rem" : "0.7rem",
            padding: isMobile ? "0.55rem 0.6rem 0.65rem" : "0.85rem 1rem 1rem",
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
      )}

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
          {/* Scene selector lives inside the room palette, not on individual lights. */}
          <SceneButton
            onClick={() => {
              setEditing(false);
              setScenesOpen(true);
            }}
          />
        </LightEditor>
      )}

      {scenesOpen && (
        <SceneModal
          roomName={name}
          scenes={scenes}
          busy={busy}
          onApply={async (id) => {
            await applyScene(id);
            setScenesOpen(false);
          }}
          onSave={saveAsScene}
          onClose={() => setScenesOpen(false)}
        />
      )}
    </section>
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
  const { isMobile } = useViewport();
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
        onClick={() => { if (editable) setEditing((v) => !v); }}
        title={editable ? "Open the light editor" : undefined}
        style={{
          display: "flex",
          flexDirection: "row",
          alignItems: "center",
          justifyContent: "space-between",
          gap: isMobile ? "0.35rem" : "0.75rem",
          padding: isMobile ? "0.45rem 0.55rem" : "0.8rem 0.95rem",
          borderRadius: isMobile ? 10 : 12,
          // Lit cards carry a wash of their own color in the corner and a soft
          // glow; off cards sink into the room box.
          background: isOn
            ? `radial-gradient(130% 130% at 88% 0%, ${hex}1f, transparent 55%), ${T.card}`
            : T.cardOff,
          border: `1px solid ${isOn ? `${hex}40` : T.cardBorder}`,
          boxShadow: isOn
            ? `0 0 24px -10px ${hex}cc, inset 0 1px 0 rgba(255,255,255,0.04)`
            : "inset 0 1px 0 rgba(255,255,255,0.02)",
          cursor: editable ? "pointer" : "default",
          opacity: offline ? 0.45 : 1,
          transition: "background 0.25s, border-color 0.25s, box-shadow 0.25s",
          ...(editing ? { outline: `1px solid ${T.accent}` } : {}),
        }}
      >
        <div style={{ minWidth: 0, display: "flex", flexDirection: "column", gap: isMobile ? "0.1rem" : "0.3rem" }}>
          <span
            style={{
              fontWeight: 600,
              fontSize: isMobile ? "0.8rem" : "0.92rem",
              color: isOn ? T.text : T.dim,
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
              transition: "color 0.25s",
            }}
          >
            {light.name}
          </span>
          <span style={{ display: "inline-flex", alignItems: "center", gap: "0.4rem", fontSize: "0.72rem", color: T.faint }}>
            {isOn && light.capabilities.color_rgb && (
              <span style={{ width: 9, height: 9, borderRadius: "50%", background: hex, boxShadow: `0 0 6px ${hex}`, display: "inline-block" }} />
            )}
            {isOn ? (light.capabilities.dimmable ? `${Math.round(brightness)}%` : "on") : "off"}
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
function VerticalToggle({
  on,
  onToggle,
  disabled,
}: {
  on: boolean;
  onToggle: () => void;
  disabled?: boolean;
}) {
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
        // Glassy track: translucent fill + blur; charged with an ice-blue
        // current when on. Restrained glow — electric, not neon signage.
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
          background: on
            ? "linear-gradient(180deg, #ffffff, #d6f1ff)"
            : "rgba(255,255,255,0.4)",
          boxShadow: on
            ? "0 0 8px rgba(125,211,252,0.9), 0 1px 2px rgba(0,0,0,0.45)"
            : "0 1px 2px rgba(0,0,0,0.35)",
          transition: "top 0.2s, background 0.2s, box-shadow 0.2s",
        }}
      />
    </button>
  );
}
