import { useEffect, useRef, useState } from "react";
import {
  getMediaDevices,
  getPowerDevices,
  getProviders,
  getRooms,
  getScenes,
  mergePatch,
  restoreDefaultHome,
  setMediaEnabled,
  setLightEnabled,
  setPowerEnabled,
  setPowerState,
  type MediaDevice,
  type Light,
  type LightState,
  type LightStatePatch,
  type PowerDevice,
  type Provider,
  type Room,
  type RoomControl,
  type Scene,
} from "../api";
import { Glyph } from "../components/glyphs";
import { RoomCard, litHexes, roomMembers } from "../components/RoomCard";
import { T, font, glassCard, radius, color, glow, alpha } from "../theme";
import { pageShell } from "../styles";
import { CornerFiligree } from "../components/ornament";
import { PageHeader } from "../components/PageHeader";
import { useDialogs, type Dialogs } from "../components/dialogs";
import { useMediaQuery, useViewport } from "../useViewport";

interface Props {
  lights: Light[];
  onRefresh: () => void;
  onNavigate: (page: "settings") => void;
}

export function DashboardPage({ lights, onRefresh, onNavigate }: Props) {
  const { isMobile, isCompact } = useViewport();
  const [localLights, setLocalLights] = useState<Light[]>(lights);
  const [powerDevices, setPowerDevices] = useState<PowerDevice[]>([]);
  const [mediaDevices, setMediaDevices] = useState<MediaDevice[]>([]);
  const [providers, setProviders] = useState<Provider[]>([]);
  const [rooms, setRooms] = useState<Room[]>([]);
  // All scenes (home + room-scoped); per-room subsets are filtered downstream.
  const [scenes, setScenes] = useState<Scene[]>([]);
  const dialogs = useDialogs();

  function loadScenes() {
    getScenes().then(setScenes);
  }
  const homeScenes = scenes;

  useEffect(() => { setLocalLights(lights); }, [lights]);
  useEffect(() => { getProviders().then(setProviders); }, []);
  useEffect(() => { loadScenes(); }, []);
  // Re-fetch membership + non-light devices alongside light refreshes.
  useEffect(() => {
    getRooms().then(setRooms);
    getPowerDevices().then(setPowerDevices);
    getMediaDevices().then(setMediaDevices);
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
    es.addEventListener("media_state", (raw) => {
      const ev = JSON.parse((raw as MessageEvent).data) as {
        provider_id: string;
        device_id: string;
        state: MediaDevice["state"];
      };
      setMediaDevices((prev) =>
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
  function onMediaPatch(id: string, patch: Partial<MediaDevice["state"]>) {
    setMediaDevices((prev) =>
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
  function onMediaSetEnabled(id: string, enabled: boolean) {
    setMediaDevices((prev) => prev.map((d) => (d.id === id ? { ...d, enabled } : d)));
    setMediaEnabled(id, enabled);
  }
  function onPowerSetEnabled(id: string, enabled: boolean) {
    setPowerDevices((prev) => prev.map((d) => (d.id === id ? { ...d, enabled } : d)));
    setPowerEnabled(id, enabled);
  }

  const onCount = localLights.filter((l) => l.last_state?.on).length;
  const empty = localLights.length === 0 && powerDevices.length === 0 && mediaDevices.length === 0;
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
    <div style={{ ...pageShell(isMobile), ...(isCompact && !isMobile ? { padding: "1.1rem 1rem" } : {}), color: T.text, display: "flex", flexDirection: "column", flex: 1 }}>
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
          mediaDevices={mediaDevices}
          rooms={rooms}
          providers={providers}
          scenes={scenes}
          dialogs={dialogs}
          onScenesChanged={loadScenes}
          onLightUpdate={onLightUpdate}
          onMediaPatch={onMediaPatch}
          onPowerToggle={onPowerToggle}
          onLightSetEnabled={onLightSetEnabled}
          onMediaSetEnabled={onMediaSetEnabled}
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
  mediaDevices,
  rooms,
  providers,
  scenes,
  dialogs,
  onScenesChanged,
  onLightUpdate,
  onMediaPatch,
  onPowerToggle,
  onLightSetEnabled,
  onMediaSetEnabled,
  onPowerSetEnabled,
  onChanged,
}: {
  lights: Light[];
  powerDevices: PowerDevice[];
  mediaDevices: MediaDevice[];
  rooms: Room[];
  providers: Provider[];
  scenes: Scene[];
  dialogs: Dialogs;
  onScenesChanged: () => void;
  onLightUpdate: (id: string, state: LightState) => void;
  onMediaPatch: (id: string, patch: Partial<MediaDevice["state"]>) => void;
  onPowerToggle: (id: string, next: boolean) => void;
  onLightSetEnabled: (id: string, enabled: boolean) => void;
  onMediaSetEnabled: (id: string, enabled: boolean) => void;
  onPowerSetEnabled: (id: string, enabled: boolean) => void;
  onChanged: () => void;
}) {
  const { isMobile } = useViewport();
  // Masonry column count scales with the window — the page is full-bleed, so
  // a big monitor gets more columns instead of a centered strip.
  const wide = useMediaQuery("(min-width: 1700px)");
  const ultraWide = useMediaQuery("(min-width: 2400px)");
  const assigned = new Set<string>();

  // Membership (enabled filter + bound-receiver fold) comes from the one shared
  // rule in `roomMembers` — the same lists every surface's room card renders.
  const roomSections = rooms
    .map((room) => {
      const members = roomMembers(room, lights, powerDevices, mediaDevices);
      for (const l of members.lights) assigned.add(l.id);
      return { room, ...members };
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

  const common = {
    scenes,
    dialogs,
    onScenesChanged,
    onLightUpdate,
    onMediaPatch,
    onPowerToggle,
    onLightSetEnabled,
    onMediaSetEnabled,
    onPowerSetEnabled,
    onChanged,
  };

  return (
    <div style={{ columnCount: isMobile ? 1 : ultraWide ? 4 : wide ? 3 : 2, columnGap: "1.1rem" }}>
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

/** A room on the Control page: the framed glass plate (gold corner filigree
 * breathing its lit lights' colors, brass when off; a dashed outline for the
 * no-room leftovers) around the one shared `RoomCard`. */
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
  onMediaPatch,
  onPowerToggle,
  onLightSetEnabled,
  onMediaSetEnabled,
  onPowerSetEnabled,
  onChanged,
}: {
  index?: number;
  name: string;
  roomId?: string;
  lights: Light[];
  power: PowerDevice[];
  audio: MediaDevice[];
  controls: RoomControl[];
  scenes: Scene[];
  dialogs: Dialogs;
  onScenesChanged: () => void;
  onLightUpdate: (id: string, state: LightState) => void;
  onMediaPatch: (id: string, patch: Partial<MediaDevice["state"]>) => void;
  onPowerToggle: (id: string, next: boolean) => void;
  onLightSetEnabled: (id: string, enabled: boolean) => void;
  onMediaSetEnabled: (id: string, enabled: boolean) => void;
  onPowerSetEnabled: (id: string, enabled: boolean) => void;
  onChanged: () => void;
}) {
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
      {roomId && <CornerFiligree colors={litHexes(lights)} />}
      <RoomCard
        variant="page"
        name={name}
        roomId={roomId}
        lights={lights}
        power={power}
        audio={audio}
        controls={controls}
        scenes={scenes}
        dialogs={dialogs}
        onScenesChanged={onScenesChanged}
        onLightUpdate={onLightUpdate}
        onMediaPatch={onMediaPatch}
        onPowerToggle={onPowerToggle}
        onLightSetEnabled={onLightSetEnabled}
        onMediaSetEnabled={onMediaSetEnabled}
        onPowerSetEnabled={onPowerSetEnabled}
        onChanged={onChanged}
      />
    </section>
  );
}

/** The whole-home "Restore Home" action — a gilded **power seal**: a round gothic
 * emblem (double filigree rings echoing the room-card corners, a restore arrow at
 * its heart) over a slow breathing gold aura, with an engraved label beneath.
 * Gold is the ornament/power accent, setting it apart from the cyan per-device
 * controls; the seal form makes the "bring everything back" action ceremonial. */
export function RestoreHomeButton({ name, onRestore }: { name: string; onRestore: () => void }) {
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

