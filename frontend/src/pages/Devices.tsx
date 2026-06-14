// The "Devices" page is the full device *inventory* — every device Bifrost
// knows about, of every domain (lights, audio, power), regardless of room
// membership. It is the configuration surface: this is where a device is
// enabled/disabled and given a glyph override. Live control (color, volume,
// scenes) lives on the Control/Floor-plan/Audio pages and on Rooms; here we
// only show what was imported and let you configure each device.

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  getLights,
  getAudioDevices,
  getPowerDevices,
  getRooms,
  setPowerEnabled,
  setLightEnabled,
  setAudioEnabled,
  setPowerState,
  setLightGlyph,
  setAudioGlyph,
  setPowerGlyph,
  setLightShadow,
  setAudioShadow,
  setPowerShadow,
  setLightRoom,
  setAudioRoom,
  setPowerRoom,
  type Light,
  type AudioDevice,
  type PowerDevice,
  type PowerKind,
  type Room,
} from "../api";
import { Glyph, GLYPH_OPTIONS, powerKindGlyph, audioKindGlyph } from "../components/glyphs";
import { useViewport } from "../useViewport";

const ACCENT = "#38bdf8"; // sky — the app's default accent

const T = {
  text: "#eae4d6",
  dim: "#97907e",
  faint: "#6b6557",
  card: "#1d1c18",
  cardOff: "#171613",
  cardBorder: "#2c2922",
  good: "#5fb87a",
  bad: "#c2603f",
};

type Domain = "light" | "audio" | "power";

// One normalized row per device, so the inventory renders uniformly no matter
// which domain a device came from. `glyph` is the override (null = none),
// `defaultGlyph` the type-derived fallback; the card shows `glyph ?? default`.
interface Item {
  domain: Domain;
  id: string;
  name: string;
  deviceId: string;
  typeLabel: string;
  enabled: boolean;
  glyph: string | null;
  defaultGlyph: string;
  on: boolean;
  offline: boolean;
  /** Power devices can be toggled from here; lights/audio control elsewhere. */
  togglePower?: boolean;
  /** When set, this is a duplicate hidden under that (canonical) device id. */
  shadowedBy: string | null;
  /** true = an automatic hardware match (authoritative); false = a manual link. */
  shadowAuto: boolean;
  /** Directly-assigned room id, or null (room links aren't reflected here). */
  roomId: string | null;
}

const POWER_KIND_LABEL: Record<PowerKind, string> = {
  switch: "Switch",
  outlet: "Outlet",
  fan: "Fan",
  toggle: "Toggle",
  generic: "Device",
};

const AUDIO_KIND_LABEL: Record<AudioDevice["kind"], string> = {
  receiver: "Receiver",
  speaker: "Speaker",
  tv: "TV",
  zone: "Zone",
};

function lightItem(l: Light): Item {
  return {
    domain: "light",
    id: l.id,
    name: l.name,
    deviceId: l.device_id,
    typeLabel: "Light",
    enabled: l.enabled !== false,
    glyph: l.glyph ?? null,
    defaultGlyph: "bulb",
    on: l.last_state?.on === true,
    offline: l.last_state?.reachable === false,
    shadowedBy: l.shadowed_by ?? null,
    shadowAuto: l.shadow_auto === true,
    roomId: l.room_id ?? null,
  };
}

function audioItem(a: AudioDevice): Item {
  return {
    domain: "audio",
    id: a.id,
    name: a.name,
    deviceId: a.device_id,
    typeLabel: AUDIO_KIND_LABEL[a.kind] ?? "Audio",
    enabled: a.enabled !== false,
    glyph: a.glyph ?? null,
    defaultGlyph: audioKindGlyph(a.kind),
    on: a.state?.power === true,
    offline: a.state?.reachable === false,
    shadowedBy: a.shadowed_by ?? null,
    shadowAuto: a.shadow_auto === true,
    roomId: a.room_id ?? null,
  };
}

function powerItem(p: PowerDevice): Item {
  return {
    domain: "power",
    id: p.id,
    name: p.name,
    deviceId: p.device_id,
    typeLabel: POWER_KIND_LABEL[p.kind] ?? "Device",
    enabled: p.enabled !== false,
    glyph: p.glyph ?? null,
    defaultGlyph: powerKindGlyph(p.kind),
    on: p.state.on,
    offline: p.state.reachable === false,
    togglePower: true,
    shadowedBy: p.shadowed_by ?? null,
    shadowAuto: p.shadow_auto === true,
    roomId: p.room_id ?? null,
  };
}

const SET_ENABLED: Record<Domain, (id: string, enabled: boolean) => Promise<void>> = {
  light: setLightEnabled,
  audio: setAudioEnabled,
  power: setPowerEnabled,
};
const SET_GLYPH: Record<Domain, (id: string, glyph: string | null) => Promise<void>> = {
  light: setLightGlyph,
  audio: setAudioGlyph,
  power: setPowerGlyph,
};
const SET_SHADOW: Record<Domain, (id: string, shadowedBy: string | null) => Promise<void>> = {
  light: setLightShadow,
  audio: setAudioShadow,
  power: setPowerShadow,
};
const SET_ROOM: Record<Domain, (id: string, roomId: string | null) => Promise<void>> = {
  light: setLightRoom,
  audio: setAudioRoom,
  power: setPowerRoom,
};

function Toggle({
  on,
  disabled,
  onToggle,
}: {
  on: boolean;
  disabled?: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      onClick={onToggle}
      disabled={disabled}
      aria-label={on ? "Turn off" : "Turn on"}
      title={on ? "Turn off" : "Turn on"}
      style={{
        // Vertical, like a physical wall switch: up = on, down = off.
        flexShrink: 0,
        width: 26,
        height: 46,
        borderRadius: 13,
        border: `1px solid ${on ? "rgba(56,189,248,0.6)" : "rgba(255,255,255,0.12)"}`,
        cursor: disabled ? "not-allowed" : "pointer",
        opacity: disabled ? 0.5 : 1,
        background: on
          ? "linear-gradient(0deg, rgba(56,189,248,0.12) 25%, rgba(56,189,248,0.5)), rgba(16,22,30,0.55)"
          : "rgba(255,255,255,0.06)",
        boxShadow: on ? `0 0 14px -4px ${ACCENT}` : "inset 0 1px 0 rgba(255,255,255,0.06)",
        position: "relative",
        transition: "background 0.2s, box-shadow 0.2s, border-color 0.2s",
      }}
    >
      <span
        style={{
          position: "absolute",
          left: 2,
          top: on ? 2 : 23,
          width: 20,
          height: 20,
          borderRadius: "50%",
          background: on ? "linear-gradient(180deg, #ffffff, #d9f1ff)" : "rgba(255,255,255,0.4)",
          boxShadow: on ? `0 0 8px ${ACCENT}` : "0 1px 2px rgba(0,0,0,0.35)",
          transition: "top 0.2s",
        }}
      />
    </button>
  );
}

/// A popover **portaled to `document.body`** and anchored to its trigger button.
/// Portaling matters: a card may be dimmed (`opacity` for offline/disabled), and
/// a child can't escape an ancestor's opacity — so an in-card popover renders
/// translucent. On phones it's a full-width bottom sheet (touch-friendly).
function AnchoredPanel({
  anchor,
  isMobile,
  width = 200,
  onClose,
  children,
}: {
  anchor: HTMLElement | null;
  isMobile: boolean;
  width?: number;
  onClose: () => void;
  children: React.ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);

  useLayoutEffect(() => {
    if (isMobile || !anchor || !ref.current) return;
    const rect = anchor.getBoundingClientRect();
    const w = ref.current.offsetWidth;
    const h = ref.current.offsetHeight;
    let left = Math.min(rect.right - w, window.innerWidth - w - 8); // right-aligned
    left = Math.max(8, left);
    let top = rect.bottom + 6;
    if (top + h > window.innerHeight - 8) top = rect.top - 6 - h; // flip up if needed
    top = Math.max(8, top);
    setPos({ left, top });
  }, [anchor, isMobile]);

  const panelStyle: React.CSSProperties = isMobile
    ? {
        position: "fixed",
        left: 0,
        right: 0,
        bottom: 0,
        zIndex: 61,
        maxHeight: "60vh",
        overflowY: "auto",
        borderRadius: "16px 16px 0 0",
        padding: "0.6rem 0.6rem calc(1.2rem + env(safe-area-inset-bottom))",
      }
    : {
        position: "fixed",
        left: pos?.left ?? -9999,
        top: pos?.top ?? -9999,
        visibility: pos ? "visible" : "hidden",
        zIndex: 61,
        width,
        maxHeight: 300,
        overflowY: "auto",
        borderRadius: 10,
        padding: "0.45rem",
      };

  return createPortal(
    <>
      <div onClick={onClose} style={{ position: "fixed", inset: 0, zIndex: 60 }} />
      <div
        ref={ref}
        style={{
          background: "#22201b",
          border: `1px solid ${T.cardBorder}`,
          boxShadow: "0 12px 30px -10px rgba(0,0,0,0.7)",
          ...panelStyle,
        }}
      >
        {children}
      </div>
    </>,
    document.body,
  );
}

/// Pick a device's glyph override. "Use type default" clears it.
function GlyphPicker({
  anchor,
  isMobile,
  current,
  onPick,
  onClose,
}: {
  anchor: HTMLElement | null;
  isMobile: boolean;
  current: string | null;
  onPick: (glyph: string | null) => void;
  onClose: () => void;
}) {
  return (
    <AnchoredPanel anchor={anchor} isMobile={isMobile} onClose={onClose}>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(5, 1fr)", gap: "0.3rem" }}>
        {GLYPH_OPTIONS.map((g) => {
          const active = current === g.name;
          return (
            <button
              key={g.name}
              onClick={() => onPick(g.name)}
              title={g.label}
              style={{
                display: "grid",
                placeItems: "center",
                height: isMobile ? 44 : 32,
                borderRadius: 7,
                cursor: "pointer",
                color: active ? ACCENT : T.dim,
                background: active ? "rgba(56,189,248,0.12)" : "transparent",
                border: `1px solid ${active ? "rgba(56,189,248,0.4)" : "transparent"}`,
              }}
            >
              <Glyph name={g.name} size={isMobile ? 22 : 18} />
            </button>
          );
        })}
      </div>
      <button
        onClick={() => onPick(null)}
        style={{
          marginTop: "0.5rem",
          width: "100%",
          background: "none",
          border: `1px solid ${T.cardBorder}`,
          borderRadius: 7,
          color: current === null ? ACCENT : T.dim,
          cursor: "pointer",
          fontSize: "0.78rem",
          padding: isMobile ? "0.6rem" : "0.32rem",
        }}
      >
        Use type default
      </button>
    </AnchoredPanel>
  );
}

/// Pick the device's room. Portaled + anchored popover on desktop; a bottom
/// sheet on phones (mobile-friendly, large tap targets). "No room" clears it.
function RoomPicker({
  anchor,
  rooms,
  current,
  isMobile,
  onPick,
  onClose,
}: {
  anchor: HTMLElement | null;
  rooms: Room[];
  current: string | null;
  isMobile: boolean;
  onPick: (roomId: string | null) => void;
  onClose: () => void;
}) {
  const choices: { id: string | null; name: string }[] = [
    { id: null, name: "No room" },
    ...rooms.map((r) => ({ id: r.id as string | null, name: r.name })),
  ];
  return (
    <AnchoredPanel anchor={anchor} isMobile={isMobile} onClose={onClose}>
      {isMobile && (
        <div style={{ color: T.dim, fontSize: "0.78rem", padding: "0.3rem 0.6rem 0.5rem" }}>
          Assign room
        </div>
      )}
      {choices.map((c) => {
        const active = (c.id ?? null) === current;
        return (
          <button
            key={c.id ?? "__none"}
            onClick={() => onPick(c.id)}
            style={{
              display: "flex",
              alignItems: "center",
              gap: "0.5rem",
              width: "100%",
              textAlign: "left",
              background: active ? "rgba(56,189,248,0.12)" : "transparent",
              border: "none",
              borderRadius: 8,
              color: active ? ACCENT : c.id ? T.text : T.faint,
              cursor: "pointer",
              fontSize: isMobile ? "0.95rem" : "0.82rem",
              padding: isMobile ? "0.7rem 0.6rem" : "0.45rem 0.5rem",
            }}
          >
            <span style={{ width: 16, display: "grid", placeItems: "center", flexShrink: 0 }}>
              {active ? "✓" : ""}
            </span>
            {c.name}
          </button>
        );
      })}
    </AnchoredPanel>
  );
}

function DeviceCard({
  item,
  rooms,
  onToggle,
  onSetEnabled,
  onSetGlyph,
  onSetRoom,
}: {
  item: Item;
  rooms: Room[];
  onToggle: (next: boolean) => void;
  onSetEnabled: (enabled: boolean) => void;
  onSetGlyph: (glyph: string | null) => void;
  onSetRoom: (roomId: string | null) => void;
}) {
  const offline = item.offline;
  const disabled = !item.enabled;
  const on = item.on && !disabled;
  const effectiveGlyph = item.glyph ?? item.defaultGlyph;
  // Names are truncated to keep cards compact; tap the text to reveal the full
  // name + id (the card grows in height only, never wider).
  const [expanded, setExpanded] = useState(false);
  const [picking, setPicking] = useState(false);
  const [roomPicking, setRoomPicking] = useState(false);
  const glyphBtnRef = useRef<HTMLButtonElement>(null);
  const roomBtnRef = useRef<HTMLButtonElement>(null);
  const { isMobile } = useViewport();
  const roomName = item.roomId ? (rooms.find((r) => r.id === item.roomId)?.name ?? null) : null;
  const clamp: React.CSSProperties = expanded
    ? { whiteSpace: "normal", overflowWrap: "anywhere" }
    : { whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" };
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: "0.85rem",
        padding: "0.85rem 1rem",
        borderRadius: 12,
        background: on ? T.card : T.cardOff,
        border: `1px solid ${on ? "rgba(56,189,248,0.22)" : T.cardBorder}`,
        opacity: disabled ? 0.45 : offline ? 0.6 : 1,
        minWidth: 0,
        boxSizing: "border-box",
        position: "relative",
      }}
    >
      <button
        ref={glyphBtnRef}
        onClick={() => setPicking((v) => !v)}
        title={`Glyph: ${item.glyph ?? "type default"} — click to change`}
        style={{
          flexShrink: 0,
          width: 38,
          height: 38,
          borderRadius: 9,
          display: "grid",
          placeItems: "center",
          color: on ? ACCENT : T.dim,
          background: on ? "rgba(56,189,248,0.10)" : "rgba(255,255,255,0.03)",
          border: item.glyph ? `1px solid rgba(56,189,248,0.35)` : "1px solid transparent",
          cursor: "pointer",
        }}
      >
        <Glyph name={effectiveGlyph} />
      </button>
      {picking && (
        <GlyphPicker
          anchor={glyphBtnRef.current}
          isMobile={isMobile}
          current={item.glyph}
          onPick={(g) => {
            onSetGlyph(g);
            setPicking(false);
          }}
          onClose={() => setPicking(false)}
        />
      )}

      <div
        onClick={() => setExpanded((v) => !v)}
        title={expanded ? item.name : `${item.name} — tap to expand`}
        style={{ minWidth: 0, flex: 1, cursor: "pointer" }}
      >
        <div
          style={{
            color: T.text,
            fontSize: "0.95rem",
            fontWeight: 600,
            ...clamp,
          }}
        >
          {item.name}
        </div>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: "0.4rem",
            fontSize: "0.72rem",
            color: T.faint,
            marginTop: 2,
            minWidth: 0,
            flexWrap: expanded ? "wrap" : "nowrap",
          }}
        >
          <span style={{ flexShrink: 0 }}>{item.typeLabel}</span>
          <span style={{ flexShrink: 0 }}>·</span>
          <span style={{ minWidth: 0, ...clamp }}>{item.deviceId}</span>
          {offline && (
            <>
              <span style={{ flexShrink: 0 }}>·</span>
              <span style={{ flexShrink: 0, color: T.bad }}>offline</span>
            </>
          )}
        </div>
      </div>

      <button
        ref={roomBtnRef}
        onClick={() => setRoomPicking((v) => !v)}
        title={roomName ? `Room: ${roomName} — click to change` : "Assign to a room"}
        style={{
          flexShrink: 0,
          width: 34,
          height: 34,
          borderRadius: 9,
          display: "grid",
          placeItems: "center",
          color: item.roomId ? ACCENT : T.faint,
          background: item.roomId ? "rgba(56,189,248,0.10)" : "rgba(255,255,255,0.03)",
          border: item.roomId ? `1px solid rgba(56,189,248,0.35)` : `1px solid ${T.cardBorder}`,
          cursor: "pointer",
        }}
      >
        <Glyph name="room" size={18} />
      </button>
      {roomPicking && (
        <RoomPicker
          anchor={roomBtnRef.current}
          rooms={rooms}
          current={item.roomId}
          isMobile={isMobile}
          onPick={(r) => {
            onSetRoom(r);
            setRoomPicking(false);
          }}
          onClose={() => setRoomPicking(false)}
        />
      )}

      <span
        aria-hidden
        title={offline ? "Unreachable" : on ? "On" : "Off"}
        style={{
          flexShrink: 0,
          width: 8,
          height: 8,
          borderRadius: "50%",
          background: offline ? T.bad : on ? T.good : "rgba(255,255,255,0.18)",
          boxShadow: !offline && on ? `0 0 8px ${T.good}` : "none",
        }}
      />
      {disabled ? (
        <button
          onClick={() => onSetEnabled(true)}
          title="Resume control of this device"
          style={{ flexShrink: 0, background: "none", border: `1px solid ${T.cardBorder}`, borderRadius: 8, color: "#6fae84", cursor: "pointer", fontSize: "0.74rem", padding: "0.3rem 0.55rem" }}
        >
          Enable
        </button>
      ) : (
        <>
          <button
            onClick={() => onSetEnabled(false)}
            title="Stop sending commands and hide from room control (stays in its room)"
            style={{ flexShrink: 0, background: "none", border: "none", color: T.faint, cursor: "pointer", fontSize: "0.74rem", padding: "0 0.2rem" }}
          >
            Disable
          </button>
          {item.togglePower && (
            <Toggle on={on} disabled={offline} onToggle={() => onToggle(!on)} />
          )}
        </>
      )}
    </div>
  );
}

/// A duplicate device collapsed under its canonical one. Auto matches (exact
/// hardware id) are authoritative and just explained; a manual link can be undone.
function HiddenDuplicate({
  item,
  canonical,
  onUnlink,
}: {
  item: Item;
  canonical: string | undefined;
  onUnlink: () => void;
}) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: "0.6rem",
        padding: "0.45rem 0.7rem",
        fontSize: "0.78rem",
        color: T.faint,
        borderLeft: `2px solid ${T.cardBorder}`,
        marginLeft: "0.4rem",
      }}
    >
      <span style={{ color: T.faint, display: "grid", placeItems: "center", opacity: 0.7 }}>
        <Glyph name={item.glyph ?? item.defaultGlyph} size={16} />
      </span>
      <span style={{ color: T.dim }}>{item.name}</span>
      <span>
        — hidden duplicate{canonical ? ` of ${canonical}` : ""}
        {item.shadowAuto ? " (matched by hardware id)" : ""}
      </span>
      {!item.shadowAuto && (
        <button
          onClick={onUnlink}
          title="Unlink — show this device on its own again"
          style={{
            marginLeft: "auto",
            background: "none",
            border: `1px solid ${T.cardBorder}`,
            borderRadius: 7,
            color: T.dim,
            cursor: "pointer",
            fontSize: "0.72rem",
            padding: "0.2rem 0.5rem",
          }}
        >
          Unlink
        </button>
      )}
    </div>
  );
}

const SECTIONS: { domain: Domain; title: string }[] = [
  { domain: "light", title: "Lights" },
  { domain: "audio", title: "Audio" },
  { domain: "power", title: "Power" },
];

export function DevicesPage() {
  const [items, setItems] = useState<Item[]>([]);
  const [rooms, setRooms] = useState<Room[]>([]);
  const [loading, setLoading] = useState(true);
  const { isMobile } = useViewport();

  const refresh = useCallback(async () => {
    const [lights, audio, power, roomList] = await Promise.all([
      getLights(),
      getAudioDevices(),
      getPowerDevices(),
      getRooms(),
    ]);
    const lightItems = lights === "unauthorized" ? [] : lights.map(lightItem);
    setItems([...lightItems, ...audio.map(audioItem), ...power.map(powerItem)]);
    setRooms(roomList);
    setLoading(false);
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  async function toggle(item: Item, next: boolean) {
    if (item.domain !== "power") return;
    // Optimistic — reflect immediately, reconcile on error.
    setItems((prev) => prev.map((d) => (d.id === item.id ? { ...d, on: next } : d)));
    const err = await setPowerState(item.id, next);
    if (err) {
      setItems((prev) => prev.map((d) => (d.id === item.id ? { ...d, on: !next } : d)));
    }
  }

  async function setEnabled(item: Item, enabled: boolean) {
    setItems((prev) => prev.map((d) => (d.id === item.id ? { ...d, enabled } : d)));
    await SET_ENABLED[item.domain](item.id, enabled);
  }

  async function setGlyph(item: Item, glyph: string | null) {
    setItems((prev) => prev.map((d) => (d.id === item.id ? { ...d, glyph } : d)));
    await SET_GLYPH[item.domain](item.id, glyph);
  }

  async function setRoom(item: Item, roomId: string | null) {
    setItems((prev) => prev.map((d) => (d.id === item.id ? { ...d, roomId } : d)));
    await SET_ROOM[item.domain](item.id, roomId);
  }

  // Clear a manual duplicate link so the device shows up on its own again.
  async function unlink(item: Item) {
    setItems((prev) =>
      prev.map((d) => (d.id === item.id ? { ...d, shadowedBy: null, shadowAuto: false } : d)),
    );
    await SET_SHADOW[item.domain](item.id, null);
  }

  const byId = new Map(items.map((d) => [d.id, d] as const));

  return (
    <div style={{ padding: isMobile ? "1.2rem 1rem 2rem" : "2rem 2.5rem" }}>
      <div
        style={{
          display: "flex",
          alignItems: "baseline",
          justifyContent: "space-between",
          gap: "1rem",
          marginBottom: "0.3rem",
        }}
      >
        <h2 style={{ margin: 0, fontSize: "1.4rem", color: T.text }}>Devices</h2>
        <button
          onClick={() => refresh()}
          style={{
            padding: "0.35rem 0.8rem",
            borderRadius: 8,
            border: `1px solid ${T.cardBorder}`,
            background: "transparent",
            color: T.dim,
            cursor: "pointer",
            fontSize: "0.8rem",
          }}
        >
          Refresh
        </button>
      </div>
      <p style={{ margin: "0 0 1.4rem", color: T.faint, fontSize: "0.85rem", maxWidth: 560 }}>
        Every device Bifrost has imported, of every kind. This is where you
        enable/disable a device and pin a glyph (click its icon). Live control
        lives on the Control, Audio, and Rooms pages.
      </p>

      {loading ? (
        <div style={{ color: T.faint, fontSize: "0.9rem" }}>Loading…</div>
      ) : items.length === 0 ? (
        <div
          style={{
            color: T.dim,
            fontSize: "0.9rem",
            border: `1px dashed ${T.cardBorder}`,
            borderRadius: 12,
            padding: "1.5rem",
            maxWidth: 560,
          }}
        >
          No devices yet. Add a provider (Settings → Add Provider) and click{" "}
          <strong>Sync</strong> on it to import its devices.
        </div>
      ) : (
        SECTIONS.map(({ domain, title }) => {
          const group = items.filter((d) => d.domain === domain);
          if (group.length === 0) return null;
          // Duplicates collapse: a shadowed device hides under its canonical one.
          const visible = group.filter((d) => !d.shadowedBy);
          const shadowed = group.filter((d) => d.shadowedBy);
          return (
            <section key={domain} style={{ marginBottom: "1.8rem" }}>
              <h3
                style={{
                  margin: "0 0 0.7rem",
                  fontSize: "0.8rem",
                  fontWeight: 600,
                  textTransform: "uppercase",
                  letterSpacing: "0.06em",
                  color: T.dim,
                }}
              >
                {title}
                <span style={{ color: T.faint, fontWeight: 400 }}> · {visible.length}</span>
              </h3>
              <div
                style={{
                  display: "grid",
                  gridTemplateColumns: isMobile ? "1fr" : "repeat(auto-fill, minmax(280px, 1fr))",
                  gap: "0.7rem",
                }}
              >
                {visible.map((d) => (
                  <DeviceCard
                    key={d.id}
                    item={d}
                    rooms={rooms}
                    onToggle={(next) => toggle(d, next)}
                    onSetEnabled={(en) => setEnabled(d, en)}
                    onSetGlyph={(g) => setGlyph(d, g)}
                    onSetRoom={(r) => setRoom(d, r)}
                  />
                ))}
              </div>
              {shadowed.length > 0 && (
                <div style={{ marginTop: "0.7rem" }}>
                  {shadowed.map((d) => (
                    <HiddenDuplicate
                      key={d.id}
                      item={d}
                      canonical={d.shadowedBy ? byId.get(d.shadowedBy)?.name : undefined}
                      onUnlink={() => unlink(d)}
                    />
                  ))}
                </div>
              )}
            </section>
          );
        })
      )}
    </div>
  );
}
