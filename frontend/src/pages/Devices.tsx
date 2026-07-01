// The "Devices" page is the full device *inventory* — every device Bifrost
// knows about, of every domain (lights, audio, power), regardless of room
// membership. It is the configuration surface: this is where a device is
// enabled/disabled and given a glyph override. Live control (color, volume,
// scenes) lives on the Control/Floor-plan/Audio pages and on Rooms; here we
// only show what was imported and let you configure each device.

import { useCallback, useEffect, useRef, useState } from "react";
import {
  getLights,
  getMediaDevices,
  getPowerDevices,
  getSensors,
  getRooms,
  getProviders,
  getRemoteDevices,
  getSettings,
  getDeviceRaw,
  getCompositeRouting,
  type ControlRoute,
  discoverAllDevices,
  type FoundDevice,
  type DeviceRaw,
  setPowerEnabled,
  setLightEnabled,
  setMediaEnabled,
  setPowerState,
  setLightGlyph,
  setMediaGlyph,
  setPowerGlyph,
  setLightName,
  setMediaName,
  setPowerName,
  setLightShadow,
  setMediaShadow,
  setPowerShadow,
  setLightRoom,
  setMediaRoom,
  setPowerRoom,
  setSensorEnabled,
  setSensorGlyph,
  setSensorName,
  setSensorShadow,
  setSensorRoom,
  sensorReadingText,
  type SensorDevice,
  setMediaReceiver,
  setMediaCompanion,
  setProviderOrder,
  type Light,
  type MediaDevice,
  type PowerDevice,
  type PowerKind,
  type Room,
  type Provider,
  type RemoteDevice,
} from "../api";
import { Glyph, GlyphGrid, GLYPH_OPTIONS, powerKindGlyph, mediaKindGlyph, sensorKindGlyph } from "../components/glyphs";
import { PageHeader, SectionLabel } from "../components/PageHeader";
import { Switch, Segmented } from "../components/controls";
import { GenericDevicesSection } from "../components/GenericDevices";
import type { AddPrefill } from "./Settings";
import { MenuItem } from "../components/Select";
import { AnchoredPanel } from "../components/AnchoredPanel";
import { useViewport } from "../useViewport";
import { T, ACCENT, alpha } from "../theme";

type Domain = "light" | "media" | "power" | "sensor";

// One normalized row per device, so the inventory renders uniformly no matter
// which domain a device came from. `glyph` is the override (null = none),
// `defaultGlyph` the type-derived fallback; the card shows `glyph ?? default`.
interface Item {
  domain: Domain;
  id: string;
  name: string;
  deviceId: string;
  /** The provider this device was imported from — the page's top-level grouping. */
  providerId: string;
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
  /** Room via a synced provider-group link, shown when there's no direct roomId
   * so an implicitly-grouped device reads as its effective room, not "No room". */
  inheritedRoomId: string | null;
  /** Audio only: the device kind, so a source (TV/speaker/zone) can be bound to
   * a receiver while receivers themselves aren't offered the control. */
  mediaKind?: MediaDevice["kind"];
  /** Audio only (M22): the receiver this source's volume routes to; null = none. */
  receiverId?: string | null;
  /** Audio only (M22): the receiver input to select when this source plays. */
  receiverSource?: string | null;
  /** How a multi-transport provider (Govee) is reaching this device: "lan" |
   * "cloud". Undefined for single-transport providers. */
  transport?: string | null;
  /** The device's network address, when the provider reports one. */
  ip?: string | null;
  /** Sensor only: the human-readable current reading (e.g. "Detected", "480 lx"),
   * shown as the status line in place of on/off. */
  readingText?: string | null;
}

const POWER_KIND_LABEL: Record<PowerKind, string> = {
  switch: "Switch",
  outlet: "Outlet",
  fan: "Fan",
  toggle: "Toggle",
  generic: "Device",
};

const SENSOR_KIND_LABEL: Record<SensorDevice["kind"], string> = {
  motion: "Motion",
  occupancy: "Occupancy",
  contact: "Contact",
  illuminance: "Light level",
  temperature: "Temperature",
  humidity: "Humidity",
  generic: "Sensor",
};

const AUDIO_KIND_LABEL: Record<MediaDevice["kind"], string> = {
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
    providerId: l.provider_id,
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
    inheritedRoomId: l.inherited_room_id ?? null,
    transport: l.last_state?.transport ?? null,
    ip: l.last_state?.ip ?? null,
  };
}

function mediaItem(a: MediaDevice): Item {
  return {
    domain: "media",
    id: a.id,
    name: a.name,
    deviceId: a.device_id,
    providerId: a.provider_id,
    typeLabel: AUDIO_KIND_LABEL[a.kind] ?? "Media",
    enabled: a.enabled !== false,
    glyph: a.glyph ?? null,
    defaultGlyph: mediaKindGlyph(a.kind),
    on: a.state?.power === true,
    offline: a.state?.reachable === false,
    shadowedBy: a.shadowed_by ?? null,
    shadowAuto: a.shadow_auto === true,
    companionOf: a.companion_of ?? null,
    roomId: a.room_id ?? null,
    inheritedRoomId: a.inherited_room_id ?? null,
    mediaKind: a.kind,
    receiverId: a.receiver_id ?? null,
    receiverSource: a.receiver_source ?? null,
    ip: a.state?.ip ?? null,
  };
}

function powerItem(p: PowerDevice): Item {
  return {
    domain: "power",
    id: p.id,
    name: p.name,
    deviceId: p.device_id,
    providerId: p.provider_id,
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
    inheritedRoomId: p.inherited_room_id ?? null,
  };
}

function sensorItem(s: SensorDevice): Item {
  const detecting =
    (s.kind === "motion" || s.kind === "occupancy") &&
    !!s.state.reading &&
    "bool" in s.state.reading &&
    s.state.reading.bool;
  return {
    domain: "sensor",
    id: s.id,
    name: s.name,
    deviceId: s.device_id,
    providerId: s.provider_id,
    typeLabel: SENSOR_KIND_LABEL[s.kind] ?? "Sensor",
    enabled: s.enabled !== false,
    glyph: s.glyph ?? null,
    defaultGlyph: sensorKindGlyph(s.kind),
    // A detecting presence sensor lights its niche; other kinds never read "on".
    on: detecting,
    offline: s.state.reachable === false,
    shadowedBy: s.shadowed_by ?? null,
    shadowAuto: s.shadow_auto === true,
    companionOf: null,
    roomId: s.room_id ?? null,
    inheritedRoomId: s.inherited_room_id ?? null,
    readingText: sensorReadingText(s),
  };
}

const SET_ENABLED: Record<Domain, (id: string, enabled: boolean) => Promise<void>> = {
  light: setLightEnabled,
  media: setMediaEnabled,
  power: setPowerEnabled,
  sensor: setSensorEnabled,
};
const SET_GLYPH: Record<Domain, (id: string, glyph: string | null) => Promise<void>> = {
  light: setLightGlyph,
  media: setMediaGlyph,
  power: setPowerGlyph,
  sensor: setSensorGlyph,
};
const SET_NAME: Record<Domain, (id: string, name: string | null) => Promise<void>> = {
  light: setLightName,
  media: setMediaName,
  power: setPowerName,
  sensor: setSensorName,
};
const SET_SHADOW: Record<Domain, (id: string, shadowedBy: string | null) => Promise<void>> = {
  light: setLightShadow,
  media: setMediaShadow,
  power: setPowerShadow,
  sensor: setSensorShadow,
};
const SET_ROOM: Record<Domain, (id: string, roomId: string | null) => Promise<void>> = {
  light: setLightRoom,
  media: setMediaRoom,
  power: setPowerRoom,
  sensor: setSensorRoom,
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
      <GlyphGrid
        options={GLYPH_OPTIONS}
        value={current}
        onPick={onPick}
        size={isCompact ? 44 : 36}
      />
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
  inherited = false,
  isCompact,
  onPick,
  onClose,
}: {
  anchor: HTMLElement | null;
  rooms: Room[];
  current: string | null;
  /** `current` comes from a synced group link, not a direct assignment. */
  inherited?: boolean;
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
      {inherited && current && (
        <div style={{ color: T.dim, fontSize: "0.74rem", padding: "0.3rem 0.6rem 0.5rem", lineHeight: 1.4 }}>
          In this room via a synced group. Pick one to set a direct override.
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
  devices: MediaDevice[];
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

/// A small uppercase status pill. `bad` (offline — a fault, red) reads louder
/// than `muted` (disabled — a deliberate, set-aside state, neutral grey).
function StatusPill({ label, tone }: { label: string; tone: "bad" | "muted" }) {
  const bad = tone === "bad";
  return (
    <span
      style={{
        flexShrink: 0,
        fontSize: "0.6rem",
        fontWeight: 700,
        letterSpacing: "0.07em",
        textTransform: "uppercase",
        color: bad ? T.bad : T.dim,
        background: bad ? alpha(T.bad, 0.13) : "rgba(255,255,255,0.05)",
        border: `1px solid ${bad ? alpha(T.bad, 0.4) : T.cardBorder}`,
        borderRadius: 999,
        padding: "0.06rem 0.42rem",
        lineHeight: 1.35,
      }}
    >
      {label}
    </span>
  );
}

/** Map the per-device transport (multi-transport providers only) to display text.
 * LAN is the preferred, local path; cloud is the fallback. */
function connectionInfo(
  transport: string | null | undefined,
): { short: string; long: string; lan: boolean } | null {
  if (transport === "lan") return { short: "LAN", long: "Local network (LAN)", lan: true };
  if (transport === "cloud") return { short: "Cloud", long: "Cloud API", lan: false };
  return null;
}

/** A small pill on the card face showing how the device is reached. */
function ConnectionPill({ info }: { info: { short: string; lan: boolean } }) {
  return (
    <span
      title={info.lan ? "Controlled over your local network" : "Controlled via the cloud API"}
      style={{
        flexShrink: 0,
        fontSize: "0.6rem",
        fontWeight: 700,
        letterSpacing: "0.07em",
        textTransform: "uppercase",
        color: info.lan ? ACCENT : T.dim,
        background: info.lan ? alpha(ACCENT, 0.12) : "rgba(255,255,255,0.05)",
        border: `1px solid ${info.lan ? alpha(ACCENT, 0.4) : T.cardBorder}`,
        borderRadius: 999,
        padding: "0.06rem 0.42rem",
        lineHeight: 1.35,
      }}
    >
      {info.short}
    </span>
  );
}

const detailBtnStyle: React.CSSProperties = {
  background: "rgba(255,255,255,0.04)",
  border: `1px solid ${T.cardBorder}`,
  borderRadius: 8,
  color: T.text,
  cursor: "pointer",
  fontSize: "0.78rem",
  padding: "0.22rem 0.55rem",
};

/** One labelled line inside a device's expanded detail panel. */
function DetailRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div style={{ display: "flex", gap: "0.7rem", alignItems: "baseline" }}>
      <span
        style={{
          flexShrink: 0,
          width: 78,
          color: T.faint,
          fontSize: "0.64rem",
          fontWeight: 600,
          textTransform: "uppercase",
          letterSpacing: "0.05em",
        }}
      >
        {label}
      </span>
      <span style={{ minWidth: 0, flex: 1, color: T.text, fontSize: "0.82rem", overflowWrap: "anywhere" }}>
        {children}
      </span>
    </div>
  );
}

/** Inline editor for a device's friendly name — click the name to rename; an
 * empty value reverts to the provider's discovered name. (A Bifrost convention so
 * crazy provider names like "Onkyo receiver (192.168.1.34)" can be made sane.) */
function NameEditor({ name, onSave }: { name: string; onSave: (name: string | null) => void }) {
  const [editing, setEditing] = useState(false);
  const [val, setVal] = useState(name);
  useEffect(() => {
    setVal(name);
  }, [name]);

  if (!editing) {
    return (
      <button
        onClick={() => {
          setVal(name);
          setEditing(true);
        }}
        title="Rename — set a friendly name (clear to revert)"
        style={{
          background: "none",
          border: "none",
          padding: 0,
          color: T.text,
          font: "inherit",
          textAlign: "left",
          cursor: "text",
          borderBottom: `1px dashed ${T.cardBorder}`,
        }}
      >
        {name}
      </button>
    );
  }
  function commit() {
    setEditing(false);
    const t = val.trim();
    if (t !== name) onSave(t === "" ? null : t);
  }
  return (
    <input
      autoFocus
      value={val}
      onChange={(e) => setVal(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
        else if (e.key === "Escape") setEditing(false);
      }}
      onBlur={commit}
      placeholder="Friendly name…"
      style={{
        width: "100%",
        background: alpha(T.text, 0.06),
        border: `1px solid ${alpha(ACCENT, 0.4)}`,
        borderRadius: 7,
        padding: "0.2rem 0.45rem",
        color: T.text,
        font: "inherit",
      }}
    />
  );
}

function DeviceCard({
  item,
  rooms,
  mediaDevices,
  mergeCandidates,
  onToggle,
  onSetEnabled,
  onSetGlyph,
  onSetName,
  onSetRoom,
  onSetReceiver,
  onMerge,
  devMode,
}: {
  item: Item;
  rooms: Room[];
  mediaDevices: MediaDevice[];
  /** M26: other visible same-domain devices this audio entity could merge into. */
  mergeCandidates: Item[];
  onToggle: (next: boolean) => void;
  onSetEnabled: (enabled: boolean) => void;
  onSetGlyph: (glyph: string | null) => void;
  /** Set the friendly name (null/empty reverts to the provider name). */
  onSetName: (name: string | null) => void;
  onSetRoom: (roomId: string | null) => void;
  onSetReceiver: (receiverId: string | null, receiverSource: string | null) => void;
  onMerge: (primaryId: string) => void;
  /** Developer mode — surfaces the raw upstream device data in the details. */
  devMode: boolean;
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
  const [raw, setRaw] = useState<DeviceRaw | null>(null);
  const [rawLoading, setRawLoading] = useState(false);
  async function loadRaw() {
    setRawLoading(true);
    setRaw(await getDeviceRaw(item.providerId, item.deviceId));
    setRawLoading(false);
  }
  const glyphBtnRef = useRef<HTMLButtonElement>(null);
  const roomBtnRef = useRef<HTMLButtonElement>(null);
  const receiverBtnRef = useRef<HTMLButtonElement>(null);
  const mergeBtnRef = useRef<HTMLButtonElement>(null);
  const { isCompact } = useViewport();
  // Effective room: a direct assignment, else the room reached via a synced
  // provider-group link. Shown so an implicitly-grouped device reads as its
  // real room rather than "No room".
  const effectiveRoomId = item.roomId ?? item.inheritedRoomId;
  const isInheritedRoom = !item.roomId && !!item.inheritedRoomId;
  const roomName = effectiveRoomId
    ? (rooms.find((r) => r.id === effectiveRoomId)?.name ?? null)
    : null;
  // A source device (TV/streamer/console — any audio that isn't itself a
  // receiver) can route its volume to a receiver.
  const isMediaSource = item.domain === "media" && item.mediaKind !== "receiver";
  const boundReceiver = item.receiverId
    ? (mediaDevices.find((a) => a.id === item.receiverId)?.name ?? null)
    : null;
  const conn = connectionInfo(item.transport);
  // Distinct visual languages so the two muted states never read alike:
  //  • disabled — a deliberate, set-aside device: dashed border, neutral, dimmer.
  //  • offline  — a fault that wants attention: solid red-tinted border + a red
  //    inset edge, kept brighter than disabled so it stands out rather than hides.
  // (disabled wins when both: we aren't managing it, so the fault is moot.)
  const cardStyle: React.CSSProperties = disabled
    ? { background: T.cardOff, border: `1px dashed ${T.cardBorder}`, opacity: 0.62 }
    : offline
      ? {
          background: T.cardOff,
          border: `1px solid ${alpha(T.bad, 0.45)}`,
          boxShadow: `inset 3px 0 0 ${T.bad}`,
          opacity: 0.92,
        }
      : {
          background: on ? T.card : T.cardOff,
          border: `1px solid ${on ? "rgba(56,189,248,0.22)" : T.cardBorder}`,
          opacity: 1,
        };
  const statusText = disabled
    ? "Disabled"
    : offline
      ? "Offline — unreachable"
      : item.readingText != null
        ? item.readingText // sensors show their reading, not on/off
        : on
          ? "On"
          : "Off";
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        borderRadius: 12,
        minWidth: 0,
        boxSizing: "border-box",
        position: "relative",
        ...cardStyle,
      }}
    >
      {/* Face — the whole row toggles the detail panel; the interactive controls
          (glyph, enable/disable, power, chevron) stop propagation so they act on
          their own and aren't swallowed by the row click. Secondary config (room,
          receiver, merge) lives in the detail panel so the face stays uncrowded. */}
      <div
        onClick={() => setExpanded((v) => !v)}
        role="button"
        aria-expanded={expanded}
        title={expanded ? "Hide details" : `${item.name} — tap for details`}
        style={{ display: "flex", alignItems: "center", gap: "0.7rem", padding: "0.75rem 0.9rem", minWidth: 0, cursor: "pointer" }}
      >
        <button
          ref={glyphBtnRef}
          onClick={(e) => {
            e.stopPropagation();
            setPicking((v) => !v);
          }}
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

        <div style={{ minWidth: 0, flex: 1 }}>
          {/* Name + status: the device's state (offline/disabled) sits right beside
              the name, not buried after the truncated id. */}
          <div style={{ display: "flex", alignItems: "center", gap: "0.45rem", minWidth: 0 }}>
            <span style={{ color: T.text, fontSize: "0.95rem", fontWeight: 600, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis", minWidth: 0 }}>
              {item.name}
            </span>
            {offline && <StatusPill label="Offline" tone="bad" />}
            {disabled && <StatusPill label="Disabled" tone="muted" />}
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
            }}
          >
            <span style={{ flexShrink: 0 }}>{item.typeLabel}</span>
            <span style={{ minWidth: 0, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
              · {item.deviceId}
            </span>
            {conn && <ConnectionPill info={conn} />}
          </div>
        </div>

        <div
          onClick={(e) => e.stopPropagation()}
          style={{ display: "flex", alignItems: "center", gap: "0.45rem", flexShrink: 0 }}
        >
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
              {item.togglePower && <Toggle on={on} disabled={offline} onToggle={() => onToggle(!on)} />}
            </>
          )}
          <button
            onClick={() => setExpanded((v) => !v)}
            aria-label={expanded ? "Hide details" : "Show details"}
            style={{
              flexShrink: 0,
              background: "none",
              border: "none",
              color: T.faint,
              cursor: "pointer",
              fontSize: "0.9rem",
              lineHeight: 1,
              padding: "0 0.1rem",
              transform: expanded ? "rotate(180deg)" : "none",
              transition: "transform .15s ease",
            }}
          >
            ⌄
          </button>
        </div>
      </div>

      {/* Detail drop-down: identity, connection, and the device's configuration. */}
      {expanded && (
        <div
          style={{
            borderTop: `1px solid ${T.cardBorder}`,
            padding: "0.7rem 0.9rem 0.85rem",
            display: "flex",
            flexDirection: "column",
            gap: "0.5rem",
          }}
        >
          <DetailRow label="Name">
            <NameEditor name={item.name} onSave={onSetName} />
          </DetailRow>
          <DetailRow label="Device ID">
            <span style={{ fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace", fontSize: "0.78rem" }}>
              {item.deviceId}
            </span>
          </DetailRow>
          {item.ip && (
            <DetailRow label="IP address">
              <span style={{ fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace", fontSize: "0.78rem" }}>
                {item.ip}
              </span>
            </DetailRow>
          )}
          {conn && (
            <DetailRow label="Connection">
              {conn.long}
              <span style={{ color: T.faint, fontSize: "0.72rem" }}>
                {conn.lan ? " · preferred (local)" : " · fallback"}
              </span>
            </DetailRow>
          )}
          <DetailRow label="Status">{statusText}</DetailRow>
          <DetailRow label="Room">
            <button ref={roomBtnRef} onClick={() => setRoomPicking((v) => !v)} style={detailBtnStyle}>
              {roomName ? (isInheritedRoom ? `${roomName} · linked` : roomName) : "Assign…"}
            </button>
            {roomPicking && (
              <RoomPicker
                anchor={roomBtnRef.current}
                rooms={rooms}
                current={effectiveRoomId}
                inherited={isInheritedRoom}
                isCompact={isCompact}
                onPick={(r) => {
                  onSetRoom(r);
                  setRoomPicking(false);
                }}
                onClose={() => setRoomPicking(false)}
              />
            )}
          </DetailRow>
          {isMediaSource && (
            <DetailRow label="Receiver">
              <button ref={receiverBtnRef} onClick={() => setReceiverPicking((v) => !v)} style={detailBtnStyle}>
                {boundReceiver ? `Volume → ${boundReceiver}` : "Bind receiver…"}
              </button>
              {receiverPicking && (
                <ReceiverPicker
                  anchor={receiverBtnRef.current}
                  isCompact={isCompact}
                  sourceId={item.id}
                  devices={mediaDevices}
                  currentReceiver={item.receiverId ?? null}
                  currentSource={item.receiverSource ?? null}
                  onPick={(rid, rsrc) => {
                    onSetReceiver(rid, rsrc);
                    if (!rid) setReceiverPicking(false);
                  }}
                  onClose={() => setReceiverPicking(false)}
                />
              )}
            </DetailRow>
          )}
          {item.domain === "media" && mergeCandidates.length > 0 && (
            <DetailRow label="Merge">
              <button ref={mergeBtnRef} onClick={() => setMergePicking((v) => !v)} style={detailBtnStyle}>
                Merge into…
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
            </DetailRow>
          )}
          {devMode && (
            <DetailRow label="Upstream · dev">
              {raw ? (
                <RawUpstream data={raw} />
              ) : (
                <button onClick={loadRaw} disabled={rawLoading} style={detailBtnStyle}>
                  {rawLoading ? "Loading…" : "Load raw device data"}
                </button>
              )}
            </DetailRow>
          )}
        </div>
      )}
    </div>
  );
}

/** Dev-only readout of a device's raw upstream representation — the source's
 * state + every attribute (incl. `supported_features`), so unmodelled
 * capabilities are visible at a glance. */
function RawUpstream({ data }: { data: DeviceRaw }) {
  if (data.note) {
    return <span style={{ color: T.faint, fontSize: "0.76rem" }}>{data.note}</span>;
  }
  const attrs = data.attributes ?? {};
  const mono = "ui-monospace, SFMono-Regular, Menlo, monospace";
  return (
    <div style={{ fontFamily: mono, fontSize: "0.72rem", display: "flex", flexDirection: "column", gap: 2, maxWidth: 440 }}>
      <div style={{ color: T.dim }}>
        {data.domain} · state: <span style={{ color: T.text }}>{String(data.state)}</span>
      </div>
      {data.supported_features != null && (
        <div style={{ color: T.dim }}>
          supported_features: <span style={{ color: T.text }}>{data.supported_features}</span>
        </div>
      )}
      {Object.entries(attrs)
        .filter(([k]) => k !== "supported_features")
        .map(([k, v]) => (
          <div key={k} style={{ display: "flex", gap: 6 }}>
            <span style={{ color: T.faint, flexShrink: 0 }}>{k}</span>
            <span style={{ color: T.dim, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {JSON.stringify(v)}
            </span>
          </div>
        ))}
      {data.generic_preview && data.generic_preview.length > 0 && (
        <div style={{ marginTop: 6, color: T.dim, whiteSpace: "normal" }}>
          <span style={{ color: T.faint }}>would model as </span>
          {data.generic_preview.map((c) => `${c.type}(${c.key})`).join(", ")}
        </div>
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
  { domain: "media", title: "Media" },
  { domain: "power", title: "Power" },
  { domain: "sensor", title: "Sensors" },
];

/** Precedence panel: which member device each control resolves to, + why.
 * Fetched live from the dev routing endpoint so it mirrors real read/write
 * routing (never a client-side guess). */
function CompositeRouting({ id }: { id: string }) {
  const [routes, setRoutes] = useState<ControlRoute[] | null>(null);
  useEffect(() => {
    getCompositeRouting(id).then(setRoutes);
  }, [id]);
  if (!routes || routes.length === 0) return null;
  return (
    <div style={{ marginTop: "0.55rem", borderTop: `1px solid ${T.cardBorder}`, paddingTop: "0.5rem" }}>
      <div style={{ color: T.faint, fontSize: "0.66rem", letterSpacing: "0.06em", textTransform: "uppercase", marginBottom: "0.35rem" }}>
        Control precedence
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: "0.25rem" }}>
        {routes.map((r) => (
          <div key={r.control} style={{ display: "flex", gap: "0.5rem", fontSize: "0.76rem", lineHeight: 1.3 }}>
            <span style={{ minWidth: 120, flexShrink: 0, color: T.dim }}>{r.control}</span>
            <span style={{ color: ACCENT }}>{r.device_name || r.device_id.slice(0, 8)}</span>
            <span style={{ color: T.faint }}>· {r.reason}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

/** One member of a composite (its role within the aggregate + a short detail). */
interface CompositeMember {
  role: string;
  name: string;
  detail: string;
}
/** A "composite" device — a single control surface that aggregates the members
 * of one logical-device **group** (a TV + its paired remote, several merged
 * `media_player` views of one box). A receiver binding is NOT a composite — it's
 * a separate shared-receiver abstraction. Surfaced read-only in dev mode. */
interface CompositeView {
  id: string;
  name: string;
  kindLabel: string;
  members: CompositeMember[];
}

function mediaCapsSummary(cap: MediaDevice["capabilities"]): string {
  const flags = [
    cap.sources && "sources",
    cap.transport && "transport",
    cap.now_playing && "now-playing",
    cap.favorites && "favorites",
    cap.grouping && "grouping",
  ].filter(Boolean) as string[];
  return flags.length ? flags.join(" · ") : "power only";
}

/** Derive composite devices from the loaded inventory. Anchored on whatever media
 * device is the user-facing control surface (not TV-specific), so new composite
 * shapes show up here automatically as they're added. */
function buildComposites(
  media: MediaDevice[],
  remotes: RemoteDevice[],
  items: Item[],
): CompositeView[] {
  const out: CompositeView[] = [];
  for (const a of media) {
    if (a.shadowed_by || a.companion_of) continue; // a hidden/merged entity isn't a surface
    // A composite is defined purely by its group: merged companions + a paired
    // remote. A receiver binding is a *separate* abstraction (a shared receiver
    // owning volume, many sources → one receiver) — NOT composite membership, so
    // it never makes a lone device a composite. (Where volume actually routes,
    // incl. to a receiver, is shown in the Control-precedence panel below.)
    const members: CompositeMember[] = [];
    // Pairing is resolved server-side onto the effective device (`remote_id`).
    const remote = a.remote_id ? remotes.find((r) => r.id === a.remote_id) : undefined;
    if (remote) members.push({ role: "Remote", name: remote.name, detail: "D-pad · keys · app launch" });
    for (const it of items) {
      if (it.companionOf === a.id) members.push({ role: "Companion", name: it.name, detail: it.typeLabel });
    }
    if (members.length === 0) continue;
    out.push({
      id: a.id,
      name: a.name,
      kindLabel: AUDIO_KIND_LABEL[a.kind] ?? "Media",
      members: [
        { role: "Primary", name: a.name, detail: mediaCapsSummary(a.capabilities) },
        ...members,
      ],
    });
  }
  return out;
}

export function DevicesPage({ onAddDetected }: { onAddDetected?: (p: AddPrefill) => void }) {
  const [tab, setTab] = useState<"controlled" | "detected">("controlled");
  const [items, setItems] = useState<Item[]>([]);
  const [rooms, setRooms] = useState<Room[]>([]);
  const [providers, setProviders] = useState<Provider[]>([]);
  // Raw audio devices kept around so the receiver-binding picker has names +
  // each receiver's input list (the normalized Item drops device state).
  const [mediaDevices, setMediaDevices] = useState<MediaDevice[]>([]);
  // Remotes + dev mode drive the dev-only Composite-devices diagnostic panel.
  const [remotes, setRemotes] = useState<RemoteDevice[]>([]);
  const [devMode, setDevMode] = useState(false);
  const [loading, setLoading] = useState(true);
  // Live pointer-drag reordering of provider groups: `drag` drives the render
  // (the floating section's offset + where the others make room); `sectionRefs`
  // measure layout at grab time; `dragInfo` holds the immutable grab snapshot.
  const [drag, setDrag] = useState<{ id: string; dy: number; target: number; h: number } | null>(
    null,
  );
  const sectionRefs = useRef<Map<string, HTMLElement>>(new Map());
  const dragInfo = useRef<{
    id: string;
    centers: number[];
    originalIndex: number;
    target: number;
    startY: number;
  } | null>(null);
  const { isMobile } = useViewport();

  const refresh = useCallback(async () => {
    const [lights, audio, power, sensors, roomList, providerList, remoteList, settings] =
      await Promise.all([
        getLights(),
        getMediaDevices(),
        getPowerDevices(),
        getSensors(),
        getRooms(),
        getProviders(),
        getRemoteDevices(),
        getSettings(),
      ]);
    const lightItems = lights === "unauthorized" ? [] : lights.map(lightItem);
    setItems([
      ...lightItems,
      ...audio.map(mediaItem),
      ...power.map(powerItem),
      ...sensors.map(sensorItem),
    ]);
    setMediaDevices(audio);
    setRooms(roomList);
    setProviders(providerList);
    setRemotes(remoteList);
    setDevMode(!!settings.dev_mode);
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

  // Set a friendly name (null/empty reverts to the provider's). A revert needs the
  // server's provider name, so refresh after to pick it up.
  async function setName(item: Item, name: string | null) {
    if (name) setItems((prev) => prev.map((d) => (d.id === item.id ? { ...d, name } : d)));
    await SET_NAME[item.domain](item.id, name);
    if (!name) refresh();
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
    setMediaDevices((prev) =>
      prev.map((a) =>
        a.id === item.id ? { ...a, receiver_id: receiverId, receiver_source: receiverSource } : a,
      ),
    );
    await setMediaReceiver(item.id, receiverId, receiverSource);
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
    await setMediaCompanion(item.id, primaryId);
  }
  async function unmerge(item: Item) {
    setItems((prev) => prev.map((d) => (d.id === item.id ? { ...d, companionOf: null } : d)));
    await setMediaCompanion(item.id, null);
  }

  const byId = new Map(items.map((d) => [d.id, d] as const));

  // Dev-only diagnostic: devices that aggregate several underlying entities.
  const composites = devMode ? buildComposites(mediaDevices, remotes, items) : [];

  // Merge (M26) matches the *same physical device*, which a different provider
  // may also serve — so candidates are all visible audio devices, not just the
  // current provider's (preserves behaviour from before the provider grouping).
  const visibleAudio = items.filter(
    (d) => d.domain === "media" && !d.shadowedBy && !d.companionOf,
  );

  // Top-level grouping is by provider, in the user-saved `display_order` (the
  // API already returns providers in that order). Providers with devices are the
  // reorderable list; any provider id a device still references but that's no
  // longer in the provider list (orphans) renders last, non-reorderable, so
  // nothing silently disappears.
  const hasDevices = (pid: string) => items.some((d) => d.providerId === pid);
  const visibleProviders = providers.filter((p) => hasDevices(p.id));
  const hiddenProviders = providers.filter((p) => !hasDevices(p.id));
  const orphanIds = [
    ...new Set(
      items
        .filter((d) => !providers.some((p) => p.id === d.providerId))
        .map((d) => d.providerId),
    ),
  ];

  // Persist a new provider order: visible (reordered) ahead of the
  // device-less ones, sending the full id list so the server order is total.
  function applyOrder(nextVisible: Provider[]) {
    const next = [...nextVisible, ...hiddenProviders];
    setProviders(next);
    void setProviderOrder(next.map((p) => p.id));
  }
  // Move the provider at `index` one slot up (-1) or down (+1).
  function moveProvider(index: number, dir: -1 | 1) {
    const j = index + dir;
    if (j < 0 || j >= visibleProviders.length) return;
    const next = [...visibleProviders];
    [next[index], next[j]] = [next[j], next[index]];
    applyOrder(next);
  }
  const arrowBtnStyle = (disabled: boolean): React.CSSProperties => ({
    width: 24,
    height: 22,
    display: "grid",
    placeItems: "center",
    borderRadius: 6,
    border: `1px solid ${T.cardBorder}`,
    background: "transparent",
    color: disabled ? T.faint : T.dim,
    cursor: disabled ? "default" : "pointer",
    fontSize: "0.58rem",
    lineHeight: 1,
    opacity: disabled ? 0.4 : 1,
    padding: 0,
  });
  const reorderable = visibleProviders.length > 1;

  // ── Pointer-drag reordering ────────────────────────────────────────────────
  // The grabbed section follows the cursor 1:1; the rest stay in their DOM slots
  // and just slide by one section-height to open a gap where the drop will land.
  // Layout never changes mid-drag (only transforms), so the centers captured at
  // grab time stay valid for deciding the target index. Commit happens on release.
  const GROUP_GAP = 32; // section marginBottom (2rem) — part of a slot's height
  function beginDrag(e: React.PointerEvent, pid: string, index: number) {
    if (e.button !== 0 && e.pointerType === "mouse") return;
    e.preventDefault();
    const order = visibleProviders.map((p) => p.id);
    const rects = order.map((id) => sectionRefs.current.get(id)?.getBoundingClientRect());
    if (rects.some((r) => !r)) return;
    const centers = rects.map((r) => r!.top + r!.height / 2);
    const h = rects[index]!.height + GROUP_GAP;
    dragInfo.current = { id: pid, centers, originalIndex: index, target: index, startY: e.clientY };
    e.currentTarget.setPointerCapture(e.pointerId);
    setDrag({ id: pid, dy: 0, target: index, h });
  }
  function moveDrag(e: React.PointerEvent) {
    const info = dragInfo.current;
    if (!info) return;
    const dy = e.clientY - info.startY;
    const draggedCenter = info.centers[info.originalIndex] + dy;
    // Walk the (fixed) centers to find the slot the cursor now sits in.
    let target = info.originalIndex;
    while (target < info.centers.length - 1 && draggedCenter > info.centers[target + 1]) target++;
    while (target > 0 && draggedCenter < info.centers[target - 1]) target--;
    info.target = target;
    setDrag((d) => (d ? { ...d, dy, target } : d));
  }
  function endDrag() {
    const info = dragInfo.current;
    if (info && info.target !== info.originalIndex) {
      const next = [...visibleProviders];
      const [moved] = next.splice(info.originalIndex, 1);
      next.splice(info.target, 0, moved);
      applyOrder(next);
    }
    dragInfo.current = null;
    setDrag(null);
  }
  // The transform a section gets during a drag: the grabbed one floats with the
  // cursor; the others shift by ±one slot to vacate the target gap.
  function dragStyle(pid: string, index: number): React.CSSProperties {
    if (!drag) return {};
    if (drag.id === pid) {
      return {
        transform: `translateY(${drag.dy}px)`,
        zIndex: 20,
        position: "relative",
        opacity: 0.97,
        boxShadow: "0 14px 30px -10px rgba(0,0,0,0.65)",
        transition: "none",
        cursor: "grabbing",
      };
    }
    const from = visibleProviders.findIndex((p) => p.id === drag.id);
    let shift = 0;
    if (from < drag.target && index > from && index <= drag.target) shift = -drag.h;
    else if (from > drag.target && index < from && index >= drag.target) shift = drag.h;
    return {
      transform: shift ? `translateY(${shift}px)` : undefined,
      transition: "transform .18s ease",
      position: "relative",
    };
  }

  // One provider's devices for a single domain: the card grid plus the collapsed
  // hidden-duplicate and merged-companion rails. `caption` labels the domain, and
  // is shown only when a provider spans more than one (else it's redundant with
  // the provider header).
  function renderDomain(group: Item[], caption: string | null) {
    const visible = group.filter((d) => !d.shadowedBy && !d.companionOf);
    const shadowed = group.filter((d) => d.shadowedBy);
    const companions = group.filter((d) => d.companionOf);
    return (
      <div>
        {caption && (
          <SectionLabel style={{ marginBottom: "0.5rem", fontSize: "0.7rem", color: T.faint }}>
            {caption}
            <span style={{ fontWeight: 400 }}> · {visible.length}</span>
          </SectionLabel>
        )}
        <div
          style={{
            display: "grid",
            // Phones: one column. Everything else: cards at least 360px so the
            // glyph + name + action cluster fit one row without crushing the
            // name. (A plain `360px` track-min — not `minmax(min(100%, 360px), …)`
            // — because that `min()` form trips an auto-fill column-count quirk
            // that packs an extra, overflowing column on desktop; the guard is
            // moot here since this grid only renders above the mobile breakpoint.)
            gridTemplateColumns: isMobile ? "1fr" : "repeat(auto-fill, minmax(380px, 1fr))",
            gap: "0.8rem",
            // Each card sizes to its own content — without this, grid rows stretch
            // every card to the tallest, so expanding one card's detail panel makes
            // its row-mates grow too (the phantom "expand").
            alignItems: "start",
          }}
        >
          {visible.map((d) => (
            <DeviceCard
              key={d.id}
              item={d}
              rooms={rooms}
              mediaDevices={mediaDevices}
              mergeCandidates={visibleAudio.filter((s) => s.id !== d.id)}
              onToggle={(next) => toggle(d, next)}
              onSetEnabled={(en) => setEnabled(d, en)}
              onSetGlyph={(g) => setGlyph(d, g)}
              onSetName={(n) => setName(d, n)}
              onSetRoom={(r) => setRoom(d, r)}
              onSetReceiver={(rid, rsrc) => setReceiver(d, rid, rsrc)}
              onMerge={(primaryId) => setCompanion(d, primaryId)}
              devMode={devMode}
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
      </div>
    );
  }

  return (
    <div style={{ padding: isMobile ? "1.2rem 1rem 2rem" : "2rem 2.5rem" }}>
      <PageHeader
        title="Devices"
        description="Every device Bifrost has imported, grouped by the provider it came from. This is where you enable/disable a device and pin a glyph (click its icon). Live control lives on the Control, Audio, and Rooms pages."
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

      <div style={{ marginBottom: "1.4rem", maxWidth: 360 }}>
        <Segmented
          value={tab}
          onChange={setTab}
          variant="outline"
          accent={ACCENT}
          options={[
            { value: "controlled", label: "Controlled" },
            { value: "detected", label: "Detected" },
          ]}
        />
      </div>

      {tab === "detected" ? (
        <DetectedDevices onAdd={onAddDetected} />
      ) : (
        <>
      {devMode && composites.length > 0 && (
        <section style={{ marginBottom: "2rem" }}>
          <SectionLabel style={{ fontSize: "0.7rem", color: T.faint, marginBottom: "0.6rem" }}>
            Composite devices · dev
          </SectionLabel>
          <div style={{ display: "flex", flexDirection: "column", gap: "0.6rem", maxWidth: 560 }}>
            {composites.map((c) => (
              <div
                key={c.id}
                style={{
                  border: `1px solid ${T.cardBorder}`,
                  borderRadius: 12,
                  padding: "0.7rem 0.9rem",
                  background: alpha(ACCENT, 0.04),
                }}
              >
                <div style={{ display: "flex", alignItems: "baseline", gap: "0.5rem", marginBottom: "0.55rem" }}>
                  <span style={{ fontWeight: 600, color: T.text, fontSize: "0.9rem" }}>{c.name}</span>
                  <span style={{ color: T.faint, fontSize: "0.68rem", letterSpacing: "0.07em", textTransform: "uppercase" }}>
                    {c.kindLabel}
                  </span>
                </div>
                <div style={{ display: "flex", flexDirection: "column", gap: "0.3rem" }}>
                  {c.members.map((m, i) => (
                    <div key={i} style={{ display: "flex", gap: "0.5rem", fontSize: "0.78rem", lineHeight: 1.3 }}>
                      <span style={{ minWidth: 120, flexShrink: 0, color: T.dim }}>{m.role}</span>
                      <span style={{ color: T.text }}>{m.name}</span>
                      {m.detail && <span style={{ color: T.faint }}>· {m.detail}</span>}
                    </div>
                  ))}
                </div>
                <CompositeRouting id={c.id} />
              </div>
            ))}
          </div>
        </section>
      )}

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
        [
          ...visibleProviders.map((p, i) => ({ pid: p.id, provider: p as Provider | undefined, index: i })),
          ...orphanIds.map((pid) => ({ pid, provider: undefined as Provider | undefined, index: -1 })),
        ].map(({ pid, provider, index }) => {
          const provItems = items.filter((d) => d.providerId === pid);
          if (provItems.length === 0) return null;
          // Domains this provider actually has devices in (an integration like
          // Home Assistant spans several; most providers just one).
          const domainsPresent = SECTIONS.filter(({ domain }) =>
            provItems.some((d) => d.domain === domain),
          );
          const multiDomain = domainsPresent.length > 1;
          const total = provItems.filter((d) => !d.shadowedBy && !d.companionOf).length;
          const canReorder = index >= 0 && reorderable;
          return (
            <section
              key={pid}
              ref={(el) => {
                if (el) sectionRefs.current.set(pid, el);
                else sectionRefs.current.delete(pid);
              }}
              style={{ marginBottom: "2rem", ...dragStyle(pid, index) }}
            >
              <div style={{ display: "flex", alignItems: "center", gap: "0.5rem", marginBottom: "0.9rem" }}>
                {canReorder && (
                  <div style={{ display: "flex", alignItems: "center", gap: "0.25rem", flexShrink: 0 }}>
                    {/* Grip = pointer-drag (mouse + touch); arrows = a11y / precise nudge. */}
                    <span
                      onPointerDown={(e) => beginDrag(e, pid, index)}
                      onPointerMove={moveDrag}
                      onPointerUp={endDrag}
                      onPointerCancel={endDrag}
                      title="Drag to reorder"
                      style={{
                        cursor: drag?.id === pid ? "grabbing" : "grab",
                        color: T.dim,
                        padding: "0 0.15rem",
                        touchAction: "none",
                        userSelect: "none",
                        fontSize: "1.05rem",
                        lineHeight: 1,
                      }}
                    >
                      ⠿
                    </span>
                    <button
                      onClick={() => moveProvider(index, -1)}
                      disabled={index === 0}
                      title="Move up"
                      style={arrowBtnStyle(index === 0)}
                    >
                      ▲
                    </button>
                    <button
                      onClick={() => moveProvider(index, 1)}
                      disabled={index === visibleProviders.length - 1}
                      title="Move down"
                      style={arrowBtnStyle(index === visibleProviders.length - 1)}
                    >
                      ▼
                    </button>
                  </div>
                )}
                <SectionLabel style={{ fontSize: "0.95rem", color: T.text }}>
                  {provider?.name ?? "Unknown provider"}
                  <span style={{ color: T.faint, fontWeight: 400, letterSpacing: "0.08em" }}>
                    {/* Only show the type when it differs from the instance name, so
                        a provider the user named after its type doesn't read
                        "Govee · Govee". */}
                    {provider?.type_name && provider.type_name !== provider.name
                      ? ` · ${provider.type_name}`
                      : ""}{" "}
                    · {total}
                  </span>
                </SectionLabel>
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: "1.1rem" }}>
                {domainsPresent.map(({ domain, title }) => (
                  <div key={domain}>
                    {renderDomain(
                      provItems.filter((d) => d.domain === domain),
                      multiDomain ? title : null,
                    )}
                  </div>
                ))}
              </div>
            </section>
          );
        })
      )}
          <GenericDevicesSection />
        </>
      )}
    </div>
  );
}

/** The "Detected" tab: devices found on the network that aren't added yet. Scans
 * on open and re-scans periodically; "Add" hands the device to the Settings add
 * form (pre-filled), where pairing / keys are completed. */
function DetectedDevices({ onAdd }: { onAdd?: (p: AddPrefill) => void }) {
  const [scanning, setScanning] = useState(false);
  const [scanned, setScanned] = useState(false);
  const [found, setFound] = useState<FoundDevice[]>([]);

  const scan = useCallback(async () => {
    setScanning(true);
    setFound(await discoverAllDevices());
    setScanned(true);
    setScanning(false);
  }, []);

  // Scan on open, then re-scan every couple of minutes while the tab is viewed.
  useEffect(() => {
    scan();
    const t = setInterval(scan, 120_000);
    return () => clearInterval(t);
  }, [scan]);

  return (
    <div style={{ maxWidth: 620 }}>
      <div style={{ display: "flex", alignItems: "center", gap: "0.8rem", marginBottom: "1rem" }}>
        <button
          onClick={scan}
          disabled={scanning}
          style={{
            padding: "0.35rem 0.9rem",
            borderRadius: 8,
            border: `1px solid ${T.cardBorder}`,
            background: "transparent",
            color: T.dim,
            cursor: scanning ? "default" : "pointer",
            fontSize: "0.8rem",
          }}
        >
          {scanning ? "Scanning…" : "Scan now"}
        </button>
        <span style={{ color: T.faint, fontSize: "0.76rem" }}>Re-scans automatically every few minutes.</span>
      </div>

      {found.length === 0 ? (
        <div
          style={{
            color: T.dim,
            fontSize: "0.85rem",
            border: `1px dashed ${T.cardBorder}`,
            borderRadius: 12,
            padding: "1.5rem",
          }}
        >
          {scanned
            ? "No new devices detected. Auto-detect finds LAN gear that announces itself (Sonos, Onkyo, Sony TVs); cloud providers (Hue, Govee, LIFX) need a key — add those from Settings → Providers."
            : "Scanning the network…"}
        </div>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: "0.6rem" }}>
          {found.map((d) => (
            <div
              key={`${d.provider_type}:${d.host}`}
              style={{
                display: "flex",
                alignItems: "center",
                justifyContent: "space-between",
                gap: "1rem",
                border: `1px solid ${T.cardBorder}`,
                borderRadius: 12,
                padding: "0.7rem 0.9rem",
                background: alpha(ACCENT, 0.04),
              }}
            >
              <div style={{ minWidth: 0 }}>
                <div style={{ fontWeight: 600, color: T.text, fontSize: "0.9rem" }}>
                  {d.label ?? d.host}
                </div>
                <div style={{ color: T.faint, fontSize: "0.75rem" }}>
                  {d.type_name} · {d.host}
                </div>
              </div>
              <button
                onClick={() =>
                  onAdd?.({
                    provider_type: d.provider_type,
                    name: d.label ?? d.type_name,
                    credentials: Object.fromEntries(
                      Object.entries(d.credentials).map(([k, v]) => [k, String(v)]),
                    ),
                  })
                }
                style={{
                  flexShrink: 0,
                  padding: "0.4rem 1rem",
                  borderRadius: 8,
                  border: `1px solid ${ACCENT}`,
                  background: alpha(ACCENT, 0.12),
                  color: T.text,
                  cursor: "pointer",
                  fontSize: "0.82rem",
                }}
              >
                Add
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
