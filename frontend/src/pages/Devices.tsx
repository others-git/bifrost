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
  setAudioReceiver,
  setAudioCompanion,
  type Light,
  type AudioDevice,
  type PowerDevice,
  type PowerKind,
  type Room,
} from "../api";
import { Glyph, GLYPH_OPTIONS, powerKindGlyph, audioKindGlyph } from "../components/glyphs";
import { PageHeader, SectionLabel } from "../components/PageHeader";
import { Switch } from "../components/controls";
import { MenuItem, menuSurface } from "../components/Select";
import { sheetStyle } from "../components/sheet";
import { useViewport } from "../useViewport";
import { T, ACCENT, alpha } from "../theme";

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
  /** Audio only (M26): when set, this entity is MERGED into that primary device
   * id as its companion — hidden from control, but its capabilities are routed to
   * the primary (not discarded, unlike shadowedBy). */
  companionOf: string | null;
  /** Directly-assigned room id, or null (room links aren't reflected here). */
  roomId: string | null;
  /** Audio only: the device kind, so a source (TV/speaker/zone) can be bound to
   * a receiver while receivers themselves aren't offered the control. */
  audioKind?: AudioDevice["kind"];
  /** Audio only (M22): the receiver this source's volume routes to; null = none. */
  receiverId?: string | null;
  /** Audio only (M22): the receiver input to select when this source plays. */
  receiverSource?: string | null;
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
    companionOf: null,
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
    companionOf: a.companion_of ?? null,
    roomId: a.room_id ?? null,
    audioKind: a.kind,
    receiverId: a.receiver_id ?? null,
    receiverSource: a.receiver_source ?? null,
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
    companionOf: null,
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
  return <Switch on={on} onChange={() => onToggle()} disabled={disabled} vertical />;
}

/// A popover **portaled to `document.body`** and anchored to its trigger button.
/// Portaling matters: a card may be dimmed (`opacity` for offline/disabled), and
/// a child can't escape an ancestor's opacity — so an in-card popover renders
/// translucent. On phones it's a full-width bottom sheet (touch-friendly).
function AnchoredPanel({
  anchor,
  isCompact,
  width = 200,
  onClose,
  children,
}: {
  anchor: HTMLElement | null;
  isCompact: boolean;
  width?: number;
  onClose: () => void;
  children: React.ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);

  useLayoutEffect(() => {
    if (isCompact || !anchor || !ref.current) return;
    const rect = anchor.getBoundingClientRect();
    const w = ref.current.offsetWidth;
    const h = ref.current.offsetHeight;
    let left = Math.min(rect.right - w, window.innerWidth - w - 8); // right-aligned
    left = Math.max(8, left);
    let top = rect.bottom + 6;
    if (top + h > window.innerHeight - 8) top = rect.top - 6 - h; // flip up if needed
    top = Math.max(8, top);
    setPos({ left, top });
  }, [anchor, isCompact]);

  // Same look as the shared `Select` dropdown: frosted surface, gold hairline,
  // sharp frame radius, hover rows — bottom sheet on compact.
  const panelStyle: React.CSSProperties = isCompact
    ? { ...sheetStyle, zIndex: 61, maxHeight: "60vh" }
    : {
        position: "fixed",
        left: pos?.left ?? -9999,
        top: pos?.top ?? -9999,
        visibility: pos ? "visible" : "hidden",
        zIndex: 61,
        width,
        maxHeight: 300,
        overflowY: "auto",
        ...menuSurface,
      };

  return createPortal(
    <>
      <div onClick={onClose} style={{ position: "fixed", inset: 0, zIndex: 60 }} />
      <div ref={ref} style={panelStyle}>
        {children}
      </div>
    </>,
    document.body,
  );
}

/// Pick a device's glyph override. "Use type default" clears it.
function GlyphPicker({
  anchor,
  isCompact,
  current,
  onPick,
  onClose,
}: {
  anchor: HTMLElement | null;
  isCompact: boolean;
  current: string | null;
  onPick: (glyph: string | null) => void;
  onClose: () => void;
}) {
  return (
    <AnchoredPanel anchor={anchor} isCompact={isCompact} onClose={onClose}>
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
                height: isCompact ? 44 : 32,
                borderRadius: 7,
                cursor: "pointer",
                color: active ? ACCENT : T.dim,
                background: active ? "rgba(56,189,248,0.12)" : "transparent",
                border: `1px solid ${active ? "rgba(56,189,248,0.4)" : "transparent"}`,
              }}
            >
              <Glyph name={g.name} size={isCompact ? 22 : 18} />
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
          padding: isCompact ? "0.6rem" : "0.32rem",
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
  isCompact,
  onPick,
  onClose,
}: {
  anchor: HTMLElement | null;
  rooms: Room[];
  current: string | null;
  isCompact: boolean;
  onPick: (roomId: string | null) => void;
  onClose: () => void;
}) {
  const choices: { id: string | null; name: string }[] = [
    { id: null, name: "No room" },
    ...rooms.map((r) => ({ id: r.id as string | null, name: r.name })),
  ];
  return (
    <AnchoredPanel anchor={anchor} isCompact={isCompact} onClose={onClose}>
      {isCompact && (
        <div style={{ color: T.dim, fontSize: "0.78rem", padding: "0.3rem 0.6rem 0.5rem" }}>
          Assign room
        </div>
      )}
      {choices.map((c) => {
        const active = (c.id ?? null) === current;
        return (
          <MenuItem key={c.id ?? "__none"} active={active} compact={isCompact} onClick={() => onPick(c.id)}>
            <span style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
              <span style={{ width: 16, display: "grid", placeItems: "center", flexShrink: 0 }}>
                {active ? "✓" : ""}
              </span>
              {c.name}
            </span>
          </MenuItem>
        );
      })}
    </AnchoredPanel>
  );
}

/// Bind a source audio device (TV / streamer / console) to a receiver (M22):
/// its volume/mute then route to the receiver, and — if an input is chosen — the
/// receiver switches to that input when the source plays. Two sections: pick the
/// receiver, then (once bound, if the receiver enumerates inputs) the input to
/// switch to. Anchored popover on desktop, bottom sheet on phones.
function ReceiverPicker({
  anchor,
  isCompact,
  sourceId,
  devices,
  currentReceiver,
  currentSource,
  onPick,
  onClose,
}: {
  anchor: HTMLElement | null;
  isCompact: boolean;
  sourceId: string;
  devices: AudioDevice[];
  currentReceiver: string | null;
  currentSource: string | null;
  onPick: (receiverId: string | null, receiverSource: string | null) => void;
  onClose: () => void;
}) {
  // Candidate receivers: receivers/zones, not the source itself, and not already
  // bound elsewhere (the backend rejects chaining, so don't offer it).
  const candidates = devices.filter(
    (a) =>
      a.id !== sourceId &&
      (a.kind === "receiver" || a.kind === "zone") &&
      !a.receiver_id &&
      a.enabled !== false,
  );
  const bound = currentReceiver ? devices.find((a) => a.id === currentReceiver) : null;
  const inputs = bound?.state?.source_list ?? [];
  const row = (active: boolean, label: React.ReactNode) => (
    <span style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
      <span style={{ width: 16, display: "grid", placeItems: "center", flexShrink: 0 }}>{active ? "✓" : ""}</span>
      {label}
    </span>
  );
  return (
    <AnchoredPanel anchor={anchor} isCompact={isCompact} onClose={onClose}>
      <div style={{ color: T.dim, fontSize: "0.78rem", padding: "0.3rem 0.6rem 0.4rem" }}>
        Route volume to
      </div>
      <MenuItem active={!currentReceiver} compact={isCompact} onClick={() => onPick(null, null)}>
        {row(!currentReceiver, "Not bound")}
      </MenuItem>
      {candidates.length === 0 && !bound ? (
        <div style={{ color: T.faint, fontSize: "0.78rem", padding: "0.2rem 0.6rem 0.5rem" }}>
          No receivers found.
        </div>
      ) : (
        candidates.map((r) => {
          const active = currentReceiver === r.id;
          return (
            // Keep the chosen input only while staying on the same receiver.
            <MenuItem key={r.id} active={active} compact={isCompact} onClick={() => onPick(r.id, active ? currentSource : null)}>
              {row(active, r.name)}
            </MenuItem>
          );
        })
      )}
      {bound && inputs.length > 0 && (
        <>
          <div
            style={{
              color: T.dim,
              fontSize: "0.78rem",
              padding: "0.5rem 0.6rem 0.4rem",
              borderTop: `1px solid ${T.cardBorder}`,
              marginTop: "0.3rem",
            }}
          >
            Switch {bound.name} to
          </div>
          <MenuItem active={!currentSource} compact={isCompact} onClick={() => onPick(currentReceiver, null)}>
            {row(!currentSource, "Don’t switch input")}
          </MenuItem>
          {inputs.map((src) => {
            const active = currentSource === src;
            return (
              <MenuItem key={src} active={active} compact={isCompact} onClick={() => onPick(currentReceiver, src)}>
                {row(active, src)}
              </MenuItem>
            );
          })}
        </>
      )}
    </AnchoredPanel>
  );
}

/// Pick a primary to MERGE this audio entity into (M26) — its controls route to
/// that device. The lossless counterpart to "mark as duplicate" (shadow). Compact
/// icon trigger (the dense inventory row); its dropdown uses the shared menu look.
function MergePicker({
  anchor,
  isCompact,
  candidates,
  onPick,
  onClose,
}: {
  anchor: HTMLElement | null;
  isCompact: boolean;
  candidates: Item[];
  onPick: (primaryId: string) => void;
  onClose: () => void;
}) {
  return (
    <AnchoredPanel anchor={anchor} isCompact={isCompact} width={240} onClose={onClose}>
      <div style={{ color: T.dim, fontSize: "0.72rem", padding: "0.3rem 0.6rem 0.4rem", lineHeight: 1.35 }}>
        Merge into… <span style={{ color: T.faint }}>(same physical device — combines controls)</span>
      </div>
      {candidates.length === 0 ? (
        <div style={{ color: T.faint, fontSize: "0.78rem", padding: "0.2rem 0.6rem 0.5rem" }}>
          No other audio devices to merge into.
        </div>
      ) : (
        candidates.map((c) => (
          <MenuItem key={c.id} compact={isCompact} onClick={() => onPick(c.id)}>
            <span style={{ display: "flex", alignItems: "center", gap: "0.6rem" }}>
              <Glyph name={c.glyph ?? c.defaultGlyph} size={18} />
              {c.name}
            </span>
          </MenuItem>
        ))
      )}
    </AnchoredPanel>
  );
}

function DeviceCard({
  item,
  rooms,
  audioDevices,
  mergeCandidates,
  onToggle,
  onSetEnabled,
  onSetGlyph,
  onSetRoom,
  onSetReceiver,
  onMerge,
}: {
  item: Item;
  rooms: Room[];
  audioDevices: AudioDevice[];
  /** M26: other visible same-domain devices this audio entity could merge into. */
  mergeCandidates: Item[];
  onToggle: (next: boolean) => void;
  onSetEnabled: (enabled: boolean) => void;
  onSetGlyph: (glyph: string | null) => void;
  onSetRoom: (roomId: string | null) => void;
  onSetReceiver: (receiverId: string | null, receiverSource: string | null) => void;
  onMerge: (primaryId: string) => void;
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
  const [receiverPicking, setReceiverPicking] = useState(false);
  const [mergePicking, setMergePicking] = useState(false);
  const glyphBtnRef = useRef<HTMLButtonElement>(null);
  const roomBtnRef = useRef<HTMLButtonElement>(null);
  const receiverBtnRef = useRef<HTMLButtonElement>(null);
  const mergeBtnRef = useRef<HTMLButtonElement>(null);
  const { isCompact } = useViewport();
  const roomName = item.roomId ? (rooms.find((r) => r.id === item.roomId)?.name ?? null) : null;
  // A source device (TV/streamer/console — any audio that isn't itself a
  // receiver) can route its volume to a receiver.
  const isAudioSource = item.domain === "audio" && item.audioKind !== "receiver";
  const boundReceiver = item.receiverId
    ? (audioDevices.find((a) => a.id === item.receiverId)?.name ?? null)
    : null;
  // When expanded, wrap at word boundaries — never mid-word (`anywhere` shreds a
  // name into single letters once the column is narrow).
  const clamp: React.CSSProperties = expanded
    ? { whiteSpace: "normal", overflowWrap: "break-word", wordBreak: "normal" }
    : { whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" };
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: "0.7rem",
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
          isCompact={isCompact}
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

      {/* Action cluster: tight internal gap + fixed width so it never crushes
          the name (the cause of the per-letter wrapping). */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: "0.4rem",
          flexShrink: 0,
          marginLeft: "auto",
        }}
      >
      {isAudioSource && (
        <>
          <button
            ref={receiverBtnRef}
            onClick={() => setReceiverPicking((v) => !v)}
            title={
              boundReceiver
                ? `Volume → ${boundReceiver} — click to change`
                : "Bind to a receiver (route volume)"
            }
            style={{
              flexShrink: 0,
              width: 34,
              height: 34,
              borderRadius: 9,
              display: "grid",
              placeItems: "center",
              color: item.receiverId ? ACCENT : T.faint,
              background: item.receiverId ? "rgba(56,189,248,0.10)" : "rgba(255,255,255,0.03)",
              border: item.receiverId
                ? `1px solid rgba(56,189,248,0.35)`
                : `1px solid ${T.cardBorder}`,
              cursor: "pointer",
            }}
          >
            <Glyph name="receiver" size={18} />
          </button>
          {receiverPicking && (
            <ReceiverPicker
              anchor={receiverBtnRef.current}
              isCompact={isCompact}
              sourceId={item.id}
              devices={audioDevices}
              currentReceiver={item.receiverId ?? null}
              currentSource={item.receiverSource ?? null}
              onPick={(rid, rsrc) => {
                onSetReceiver(rid, rsrc);
                if (!rid) setReceiverPicking(false);
              }}
              onClose={() => setReceiverPicking(false)}
            />
          )}
        </>
      )}

      {item.domain === "audio" && mergeCandidates.length > 0 && (
        <>
          <button
            ref={mergeBtnRef}
            onClick={() => setMergePicking((v) => !v)}
            title="Merge into another device (same physical device — combines controls, nothing hidden)"
            style={{
              flexShrink: 0,
              width: 34,
              height: 34,
              borderRadius: 9,
              display: "grid",
              placeItems: "center",
              color: ACCENT,
              background: alpha(ACCENT, 0.08),
              border: `1px solid ${mergePicking ? ACCENT : T.cardBorder}`,
              cursor: "pointer",
              fontSize: "1rem",
              lineHeight: 1,
            }}
          >
            ⧉
          </button>
          {mergePicking && (
            <MergePicker
              anchor={mergeBtnRef.current}
              isCompact={isCompact}
              candidates={mergeCandidates}
              onPick={(primaryId) => {
                onMerge(primaryId);
                setMergePicking(false);
              }}
              onClose={() => setMergePicking(false)}
            />
          )}
        </>
      )}

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
          isCompact={isCompact}
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

/// An audio entity merged into a primary device (M26 composite) — collapsed, with
/// its controls routed to the primary (lossless, unlike a hidden duplicate).
function MergedCompanion({
  item,
  primary,
  onUnmerge,
}: {
  item: Item;
  primary: string | undefined;
  onUnmerge: () => void;
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
        borderLeft: `2px solid ${alpha(ACCENT, 0.33)}`,
        marginLeft: "0.4rem",
      }}
    >
      <span style={{ color: T.faint, display: "grid", placeItems: "center", opacity: 0.7 }}>
        <Glyph name={item.glyph ?? item.defaultGlyph} size={16} />
      </span>
      <span style={{ color: T.dim }}>{item.name}</span>
      <span>— merged into{primary ? ` ${primary}` : " another device"} (controls combined)</span>
      <button
        onClick={onUnmerge}
        title="Unmerge — show this device on its own again"
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
        Unmerge
      </button>
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
  // Raw audio devices kept around so the receiver-binding picker has names +
  // each receiver's input list (the normalized Item drops device state).
  const [audioDevices, setAudioDevices] = useState<AudioDevice[]>([]);
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
    setAudioDevices(audio);
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

  async function setReceiver(
    item: Item,
    receiverId: string | null,
    receiverSource: string | null,
  ) {
    setItems((prev) =>
      prev.map((d) => (d.id === item.id ? { ...d, receiverId, receiverSource } : d)),
    );
    setAudioDevices((prev) =>
      prev.map((a) =>
        a.id === item.id ? { ...a, receiver_id: receiverId, receiver_source: receiverSource } : a,
      ),
    );
    await setAudioReceiver(item.id, receiverId, receiverSource);
  }

  // Clear a manual duplicate link so the device shows up on its own again.
  async function unlink(item: Item) {
    setItems((prev) =>
      prev.map((d) => (d.id === item.id ? { ...d, shadowedBy: null, shadowAuto: false } : d)),
    );
    await SET_SHADOW[item.domain](item.id, null);
  }

  // M26: merge an audio entity into a primary as its companion (lossless — its
  // controls route to the primary), or unmerge.
  async function setCompanion(item: Item, primaryId: string) {
    setItems((prev) => prev.map((d) => (d.id === item.id ? { ...d, companionOf: primaryId } : d)));
    await setAudioCompanion(item.id, primaryId);
  }
  async function unmerge(item: Item) {
    setItems((prev) => prev.map((d) => (d.id === item.id ? { ...d, companionOf: null } : d)));
    await setAudioCompanion(item.id, null);
  }

  const byId = new Map(items.map((d) => [d.id, d] as const));

  return (
    <div style={{ padding: isMobile ? "1.2rem 1rem 2rem" : "2rem 2.5rem" }}>
      <PageHeader
        title="Devices"
        description="Every device Bifrost has imported, of every kind. This is where you enable/disable a device and pin a glyph (click its icon). Live control lives on the Control, Audio, and Rooms pages."
        actions={
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
        }
      />

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
          // Duplicates collapse under their canonical (shadow, lossy); merged
          // companions (M26, lossless) collapse under their primary. Both drop
          // from the visible grid.
          const visible = group.filter((d) => !d.shadowedBy && !d.companionOf);
          const shadowed = group.filter((d) => d.shadowedBy);
          const companions = group.filter((d) => d.companionOf);
          return (
            <section key={domain} style={{ marginBottom: "1.8rem" }}>
              <SectionLabel style={{ marginBottom: "0.7rem" }}>
                {title}
                <span style={{ color: T.faint, fontWeight: 400 }}> · {visible.length}</span>
              </SectionLabel>
              <div
                style={{
                  display: "grid",
                  // Phones: one column. Everything else: cards at least 360px so
                  // the glyph + name + action cluster fit on one row without
                  // crushing the name; `min(100%, …)` keeps a single card from
                  // overflowing a narrow viewport. 280px was too tight on desktop
                  // too (the power cards' toggle + buttons left no room for names).
                  gridTemplateColumns: isMobile
                    ? "1fr"
                    : "repeat(auto-fill, minmax(min(100%, 360px), 1fr))",
                  gap: "0.7rem",
                }}
              >
                {visible.map((d) => (
                  <DeviceCard
                    key={d.id}
                    item={d}
                    rooms={rooms}
                    audioDevices={audioDevices}
                    mergeCandidates={visible.filter((s) => s.id !== d.id)}
                    onToggle={(next) => toggle(d, next)}
                    onSetEnabled={(en) => setEnabled(d, en)}
                    onSetGlyph={(g) => setGlyph(d, g)}
                    onSetRoom={(r) => setRoom(d, r)}
                    onSetReceiver={(rid, rsrc) => setReceiver(d, rid, rsrc)}
                    onMerge={(primaryId) => setCompanion(d, primaryId)}
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
              {companions.length > 0 && (
                <div style={{ marginTop: "0.7rem" }}>
                  {companions.map((d) => (
                    <MergedCompanion
                      key={d.id}
                      item={d}
                      primary={d.companionOf ? byId.get(d.companionOf)?.name : undefined}
                      onUnmerge={() => unmerge(d)}
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
