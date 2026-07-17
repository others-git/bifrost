// The shared room control — ONE room card for every surface. The Control page
// (`RoomBox`) and the Boards Room widget both render `RoomCard`; they provide
// only their frame (glass section vs. board plate) and a `variant` for density.
// Alongside it live the device glyph buttons the card's body is made of
// (`LightButton`/`PowerButton`/`MediaButton` on the generic `GlyphButton`
// niche), plus the membership (`roomMembers`) and lit-colour (`litHexes`)
// derivations, so no surface re-implements any part of the card.

import { useRef, useState } from "react";
import {
  activateScene,
  createScene,
  lightChromaHex,
  lightHex,
  removeScene,
  setLightState,
  setMediaState,
  setRoomState,
  type ControlTarget,
  type Light,
  type LightState,
  type MediaDevice,
  type PowerDevice,
  type Room,
  type RoomControl,
  type Scene,
} from "../api";
import { DeviceControl } from "./DeviceControl";
import { Glyph, powerKindGlyph, mediaKindGlyph } from "./glyphs";
import { LightEditor, type LightControlChange } from "./LightEditor";
import {
  aggregateLightState,
  lightOptimistic,
  lightSupports,
  lightWrite,
} from "./lightControl";
import { MediaEditor, fanMediaCommand } from "./MediaControls";
import { DisableRow } from "./PowerFlyout";
import { SceneButton, SceneModal } from "./scenes";
import { type Dialogs } from "./dialogs";
import { T, font, nicheStyle, radius } from "../theme";
import { EFFECT_ACCENT, activeEffect, roomLightWrite } from "./lightControl";
import { useViewport } from "../useViewport";

/** The engraved room-card title — the Control page's header type. */
const titleType: React.CSSProperties = {
  fontFamily: font.display,
  textTransform: "uppercase",
  letterSpacing: "0.14em",
  fontWeight: 700,
};

const ELLIPSIS: React.CSSProperties = {
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
};

/** Bloom room inside the board-widget body's scrollport (padding + matching
 * negative margin): must cover the niche glow's visible halo (~16px, see the
 * `sz.bodyPad` comment) or the scroll container shears it flat at the edges. */
const WIDGET_BLOOM_PAD = 18;

/** The lit member lights' colours — drives the card's filigree/dot/plate. */
export function litHexes(lights: Light[]): string[] {
  return lights
    .filter((l) => l.last_state?.on && (l.last_state.color || activeEffect(l)))
    // Effect mode carries no colour (it IS the mode) — wear the effect base.
    // Everything else is the lamp's pure CHROMA (`lightChromaHex`): plates,
    // filigree, and dots say the HUE, while intensity is carried by the
    // plate's charge — so saturated and pastel lamps read as the same
    // material at the same dimmer level.
    .map((l) => (l.last_state!.color ? lightChromaHex(l) : EFFECT_ACCENT));
}

/** A room's controllable members, by the one shared rule: disabled devices drop
 * out of room control (they keep membership — see the Devices page), and a
 * receiver that is the volume target of another member folds into that source's
 * control instead of showing on its own. Preserves the room's member order. */
export function roomMembers(
  room: Room,
  lights: Light[],
  power: PowerDevice[],
  media: MediaDevice[],
): { lights: Light[]; power: PowerDevice[]; audio: MediaDevice[] } {
  const lightById = new Map(lights.map((l) => [l.id, l]));
  const powerById = new Map(power.map((d) => [d.id, d]));
  const mediaById = new Map(media.map((d) => [d.id, d]));
  const roomLights = room.light_ids
    .map((id) => lightById.get(id))
    .filter((l): l is Light => !!l && l.enabled !== false);
  const roomPower = room.power_device_ids
    .map((id) => powerById.get(id))
    .filter((d): d is PowerDevice => !!d && d.enabled !== false);
  const members = room.media_devices
    .map((m) => mediaById.get(m.media_device_id))
    .filter((d): d is MediaDevice => !!d && d.enabled !== false);
  const boundReceivers = new Set(
    members.map((d) => d.receiver_id).filter((id): id is string => !!id),
  );
  return { lights: roomLights, power: roomPower, audio: members.filter((d) => !boundReceivers.has(d.id)) };
}

/** Collapse a room's audio members into control entries: speakers playing in a
 * live sync group (sharing `group_coordinator`) become a single entry driven by
 * the coordinator; everything else is its own entry. A grouped coordinator whose
 * other members aren't in this room degrades to a solo entry. Derived from the
 * members — no group device is stored (see `models::audio::MediaState`). */
export function groupedAudio(audio: MediaDevice[]): { coordinator: MediaDevice; members: MediaDevice[] }[] {
  const byCoordinator = new Map<string, MediaDevice[]>();
  const solo: MediaDevice[] = [];
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
  const entries: { coordinator: MediaDevice; members: MediaDevice[] }[] = [];
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

// ── Device glyph buttons ──────────────────────────────────────────────────────

/** Shared shell: a square button showing a device-type glyph, glowing in its
 * accent when on. The full name lives in the fly-out it opens. */
export function GlyphButton({
  on,
  accent,
  offline,
  title,
  active,
  effect,
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
  /** A dynamic effect is running: the lit niche drifts through the hue wheel
   * (glyph, border, and glow together) instead of wearing one static colour. */
  effect?: boolean;
  buttonRef: React.Ref<HTMLButtonElement>;
  onClick: () => void;
  /** Press-and-hold (~500ms) action; suppresses the click that would follow.
   * Used as a quick power toggle so a tap still opens the fly-out. */
  onLongPress?: () => void;
  /** Square px size, or a CSS length (e.g. "100%") to fill its container. */
  size?: number | string;
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
      className={on && effect ? "bifrost-effect-drift" : undefined}
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
        // The lit-niche surface, shared with the Boards widget plates.
        ...nicheStyle(accent, on, active),
        opacity: offline ? 0.4 : 1,
        transition: "color 0.2s, background 0.2s, border-color 0.2s, box-shadow 0.2s",
      }}
    >
      {children}
    </button>
  );
}

export function LightButton({
  light,
  onLightUpdate,
  onSetEnabled,
  onChanged,
}: {
  light: Light;
  onLightUpdate: (id: string, state: LightState) => void;
  /** Omitted on surfaces without device config (e.g. Boards) — hides the disable row. */
  onSetEnabled?: (id: string, enabled: boolean) => void;
  onChanged: () => void;
}) {
  const ref = useRef<HTMLButtonElement>(null);
  const [editing, setEditing] = useState(false);
  const isOn = light.last_state?.on ?? false;
  const offline = light.last_state?.reachable === false;
  // A running effect owns the niche: it wears the effect base (the drift
  // animation cycles it through the wheel) rather than a stale/fallback colour.
  const fx = !!activeEffect(light);
  const hex = fx ? EFFECT_ACCENT : lightHex(light);

  // Quick power toggle (long-press) — refreshes after, unlike the editor's
  // debounced live commits, so a power flip reconciles against the server.
  async function toggle() {
    const next = !isOn;
    // Power is independent: send only `{ on }` so a flip never re-asserts a
    // colour/effect (the backend preserves the running mode). UI keeps the look.
    onLightUpdate(light.id, { ...(light.last_state ?? { on: false }), on: next });
    const err = await setLightState(light.id, { on: next });
    // Revert the optimistic flip if the write failed (e.g. light unreachable).
    if (err) onLightUpdate(light.id, { ...(light.last_state ?? { on: false }), on: isOn });
    onChanged();
  }

  return (
    <>
      <GlyphButton
        on={isOn}
        accent={isOn ? hex : "#ffb84d"}
        offline={offline}
        title={fx ? `${light.name} — playing ${light.last_state?.effect}` : light.name}
        active={editing}
        effect={fx}
        buttonRef={ref}
        onClick={() => setEditing((v) => !v)}
        onLongPress={toggle}
      >
        <Glyph name={light.glyph ?? "bulb"} />
      </GlyphButton>
      {editing && ref.current && (
        <DeviceControl
          domain="light"
          light={light}
          anchor={ref.current}
          onLocalPatch={onLightUpdate}
          onClose={() => setEditing(false)}
        >
          {onSetEnabled && (
            <DisableRow
              enabled={light.enabled !== false}
              onSetEnabled={(en) => { onSetEnabled(light.id, en); if (!en) setEditing(false); }}
            />
          )}
        </DeviceControl>
      )}
    </>
  );
}

export function PowerButton({
  device,
  onToggle,
  onSetEnabled,
}: {
  device: PowerDevice;
  onToggle: (id: string, next: boolean) => void;
  /** Omitted on surfaces without device config (e.g. Boards) — hides the disable row. */
  onSetEnabled?: (id: string, enabled: boolean) => void;
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
        <DeviceControl
          domain="power"
          device={device}
          anchor={ref.current}
          onToggle={(next) => onToggle(device.id, next)}
          onSetEnabled={onSetEnabled ? (en) => onSetEnabled(device.id, en) : undefined}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );
}

export function MediaButton({
  device,
  groupMembers,
  onMediaPatch,
  onSetEnabled,
}: {
  device: MediaDevice;
  /** When set (≥2), this button represents a live sync group coordinated by
   * `device`; it shows the group glyph and lists the members. */
  groupMembers?: MediaDevice[];
  onMediaPatch: (id: string, patch: Partial<MediaDevice["state"]>) => void;
  /** Omitted on surfaces without device config (e.g. Boards) — hides the disable row. */
  onSetEnabled?: (id: string, enabled: boolean) => void;
}) {
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const offline = device.state.reachable === false;
  const grouped = !!groupMembers && groupMembers.length >= 2;
  const title = grouped ? groupMembers!.map((m) => m.name).join(" + ") : device.name;
  function togglePower() {
    const next = !device.state.power;
    onMediaPatch(device.id, { power: next });
    // Revert the optimistic flip if the device didn't accept it (e.g. offline).
    setMediaState(device.id, { power: next }).then((err) => {
      if (err) onMediaPatch(device.id, { power: !next });
    });
  }
  return (
    <>
      <GlyphButton
        on={device.state.power}
        accent={T.media}
        offline={offline}
        title={title}
        active={open}
        buttonRef={ref}
        onClick={() => setOpen((v) => !v)}
        onLongPress={togglePower}
      >
        <Glyph name={grouped ? "speaker_group" : (device.glyph ?? mediaKindGlyph(device.kind))} />
      </GlyphButton>
      {open && ref.current && (
        <DeviceControl
          domain="media"
          device={device}
          anchor={ref.current}
          onLocalPatch={onMediaPatch}
          onSetEnabled={onSetEnabled ? (en) => { onSetEnabled(device.id, en); if (!en) setOpen(false); } : undefined}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );
}

/** A user-configured quick-control button on a room's header (see migration
 * 0034 / RoomControlsPanel). `power` toggles its targets and `scene` applies a
 * scene directly; `brightness`/`volume` open the shared LightEditor/MediaEditor
 * scoped to the targets (fanning to all of them). */
export function RoomControlButton({
  control,
  lights,
  power,
  audio,
  onLightUpdate,
  onPowerToggle,
  onMediaPatch,
  onChanged,
  size,
}: {
  control: RoomControl;
  lights: Light[];
  power: PowerDevice[];
  audio: MediaDevice[];
  onLightUpdate: (id: string, state: LightState) => void;
  onPowerToggle: (id: string, next: boolean) => void;
  onMediaPatch: (id: string, patch: Partial<MediaDevice["state"]>) => void;
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
  const tAudio = audio.filter((d) => has("media", d.id));

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
    control.kind === "volume" ? T.media : control.kind === "brightness" ? "#ffb84d" : T.accent;

  function togglePower() {
    const next = !anyOn;
    // Each optimistic flip reverts if its device rejects the write (offline).
    for (const l of tLights) {
      onLightUpdate(l.id, { ...(l.last_state ?? { on: false }), on: next }); // power only — preserves each light's mode
      setLightState(l.id, { on: next }).then((err) => {
        if (err) onLightUpdate(l.id, { ...(l.last_state ?? { on: false }), on: !next });
      });
    }
    for (const d of tPower) onPowerToggle(d.id, next);
    for (const d of tAudio) {
      onMediaPatch(d.id, { power: next });
      setMediaState(d.id, { power: next }).then((err) => {
        if (err) onMediaPatch(d.id, { power: !next });
      });
    }
  }

  async function applyScene() {
    if (!control.scene_id) return;
    await activateScene(control.scene_id);
    onChanged();
  }

  // Brightness cascade across the target lights (per-light by capability),
  // debounced — mirrors the room-header cascade.
  function cascade(change: LightControlChange) {
    if (change.field === "effect") return; // per-light control, not a room cascade
    // Fan only the moved dimension to the targeted lights — minimal write per
    // light (shared `lightControl` rule), optimistic full state for the UI.
    const ids = tLights.filter((l) => lightSupports(change, l.capabilities)).map((l) => l.id);
    for (const l of tLights) {
      const opt = lightSupports(change, l.capabilities)
        ? lightOptimistic(l.last_state, change)
        : { ...(l.last_state ?? { on: true }), on: true };
      onLightUpdate(l.id, opt);
    }
    clearTimeout(commitTimer.current);
    commitTimer.current = setTimeout(() => {
      for (const id of ids) setLightState(id, lightWrite(change));
    }, 200);
  }

  // Volume control fans changes to every target audio device. The MediaEditor
  // commits its own `device`; this wrapper fans the same command to the rest.
  function fanAudio(id: string, patch: Partial<MediaDevice["state"]>) {
    fanMediaCommand(tAudio, id, patch, { onPatch: onMediaPatch, commit: setMediaState });
  }

  function onClick() {
    if (control.kind === "power") togglePower();
    else if (control.kind === "scene") applyScene();
    else setOpen((v) => !v);
  }

  // Shared aggregate readouts for the targeted lights (hex / 0%-when-off
  // brightness / mirek).
  const agg = aggregateLightState(tLights);
  const initHex = agg.hex;
  const initBrightness = agg.brightness;
  const initMirek = agg.mirek;
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
        <MediaEditor
          device={tAudio[0]}
          anchor={ref.current}
          onLocalPatch={fanAudio}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );
}

// ── The room card ─────────────────────────────────────────────────────────────

/** One room's whole control: a header with the lit dot, name and member counts,
 * the room's quick-control buttons, and a room power toggle that fans out to
 * every member server-side (one room-state PUT), over a body of one glyph
 * button per member device (sync groups collapsed). The entire header is the
 * button that opens the room-wide LightEditor (colour/brightness cascade, with
 * the room's scenes behind its Scenes button); the header buttons opt out.
 *
 * The caller provides the frame (Control's glass section / a board's plate) and
 * `variant` picks the density: `page` is the padded, flowing Control card;
 * `widget` is the dense board form whose body scrolls inside a fixed height. */
export function RoomCard({
  variant,
  name,
  roomId,
  lights,
  power,
  audio,
  controls,
  scenes,
  dialogs,
  interactive = true,
  onScenesChanged,
  onLightUpdate,
  onMediaPatch,
  onPowerToggle,
  onLightSetEnabled,
  onMediaSetEnabled,
  onPowerSetEnabled,
  onChanged,
}: {
  variant: "page" | "widget";
  name: string;
  /** Absent for a non-room section (unassigned lights on Control) — no power
   * button, no dot, no room-wide editor. */
  roomId?: string;
  lights: Light[];
  power: PowerDevice[];
  audio: MediaDevice[];
  controls: RoomControl[];
  scenes: Scene[];
  dialogs: Dialogs;
  /** False while a board is in edit mode — the header stops opening the editor. */
  interactive?: boolean;
  onScenesChanged: () => void;
  onLightUpdate: (id: string, state: LightState) => void;
  onMediaPatch: (id: string, patch: Partial<MediaDevice["state"]>) => void;
  onPowerToggle: (id: string, next: boolean) => void;
  /** Omitted on surfaces without device config (e.g. Boards) — hides disable rows. */
  onLightSetEnabled?: (id: string, enabled: boolean) => void;
  onMediaSetEnabled?: (id: string, enabled: boolean) => void;
  onPowerSetEnabled?: (id: string, enabled: boolean) => void;
  onChanged: () => void;
}) {
  const { isCompact } = useViewport();
  const headerRef = useRef<HTMLElement>(null);
  const [editing, setEditing] = useState(false);
  const [scenesOpen, setScenesOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const commitTimer = useRef<ReturnType<typeof setTimeout>>(undefined);

  const page = variant === "page";
  // Density: the Control card breathes; the board form packs the same parts
  // into a fixed-size plate.
  const sz = page
    ? {
        dot: 16,
        title: "0.82rem",
        sub: "0.7rem",
        btn: isCompact ? 44 : 42,
        btnGlyph: 20,
        btnGap: isCompact ? "0.35rem" : "0.4rem",
        headerGap: isCompact ? "0.5rem" : "0.7rem",
        headerPad: isCompact ? "0.5rem 0.7rem 0.45rem" : "0.7rem 1rem",
        bodyGap: isCompact ? "0.4rem" : "0.5rem",
        bodyPad: isCompact ? "0.6rem" : "0.8rem 1rem",
      }
    : {
        dot: 14,
        title: "0.78rem",
        sub: "0.66rem",
        btn: 32,
        btnGlyph: 15,
        btnGap: "0.3rem",
        headerGap: "0.5rem",
        headerPad: 0,
        bodyGap: "0.4rem",
        // The widget body is a scroll container, so its padding box CLIPS —
        // zero padding sheared the buttons' neon glows flat at the top row
        // (the Control card's padded body never shows this). The padding gives
        // the glow room to bloom; the matching negative margin (below) hands
        // the space back so the layout sits exactly where it did. Must cover
        // the niche glow's practical extent: `glow(c, 22)` = 22px blur −6px
        // spread ≈ 16px of visible halo.
        bodyPad: WIDGET_BLOOM_PAD,
      };

  const lit = lights.filter((l) => l.last_state?.on);
  const anyOn = lit.length > 0;
  // Room-level power reflects ALL member domains (a speakers-only room can still
  // be powered). The master power button uses this; the header dot stays light-
  // centric (it breathes the lit color).
  const roomAnyOn = anyOn || power.some((d) => d.state.on) || audio.some((d) => d.state.power);
  const total = lights.length + power.length + audio.length;
  const canPower = !!roomId && total > 0;
  const showColor = lights.some((l) => l.capabilities.color_rgb);
  const showWhite = lights.some((l) => l.capabilities.color_temperature);
  const showBrightness = lights.some((l) => l.capabilities.dimmable);
  // Editor readouts (effect intersection + running effect, mirek, avg brightness)
  // all come from the one shared aggregator.
  const agg = aggregateLightState(lights);
  const tunable = !!roomId && (showColor || showWhite || showBrightness || agg.effects.length > 0);

  const counts = [
    lights.length && `${lights.length} light${lights.length !== 1 ? "s" : ""}`,
    power.length && `${power.length} switch${power.length !== 1 ? "es" : ""}`,
    audio.length && `${audio.length} speaker${audio.length !== 1 ? "s" : ""}`,
  ].filter(Boolean);
  const subtitle = counts.join(" · ");

  const hexes = litHexes(lights);
  const roomHex = hexes[0] ?? "#ffb84d";

  function cascade(change: LightControlChange) {
    if (!roomId) return;
    // An effect is inherently per-light (a uniform room PUT can't express it), so
    // fan it out only to members whose catalog has it, each carrying just the
    // effect — see the shared `lightControl` rule (never a colour alongside it,
    // which the backend would resolve as colour-mode and drop the effect).
    // A room-level cast touches LIT members only — dimming/recolouring the room
    // must never wake an off lamp (turn-on-at-X is a different command: scenes,
    // or the automation editor's "turn on and…").
    if (change.field === "effect") {
      const ids = lights
        .filter((l) => l.last_state?.on && lightSupports(change, l.capabilities))
        .map((l) => l.id);
      for (const l of lights) {
        if (ids.includes(l.id)) onLightUpdate(l.id, lightOptimistic(l.last_state, change));
      }
      clearTimeout(commitTimer.current);
      commitTimer.current = setTimeout(() => {
        for (const id of ids) setLightState(id, lightWrite(change));
      }, 200);
      return;
    }
    // Adjust only the dimension the user moved. Optimistically resolve each LIT
    // member (keeping its untouched dimensions), then drive the room with one
    // minimal power-free PUT — the backend casts it onto lit members only.
    for (const l of lights) {
      if (!l.last_state?.on) continue;
      const opt = lightSupports(change, l.capabilities)
        ? lightOptimistic(l.last_state, change)
        : l.last_state;
      onLightUpdate(l.id, opt);
    }
    const patch = roomLightWrite(change);
    clearTimeout(commitTimer.current);
    commitTimer.current = setTimeout(() => { setRoomState(roomId, patch); }, 200);
  }

  // Room power is the ONE shared control plane: optimistic flips locally, then a
  // single room-state PUT — the server fans out to every member domain (with the
  // pure-power rule and per-room audio offsets).
  async function toggleAll() {
    if (!roomId) return;
    const next = !roomAnyOn;
    setBusy(true);
    for (const l of lights) onLightUpdate(l.id, { ...(l.last_state ?? { on: false }), on: next });
    for (const d of audio) onMediaPatch(d.id, { power: next });
    try {
      await setRoomState(roomId, { on: next });
      onChanged();
    } finally {
      setBusy(false);
    }
  }

  async function applyScene(sceneId: string) {
    if (!sceneId) return;
    setBusy(true);
    try {
      await activateScene(sceneId);
      onChanged();
    } finally {
      setBusy(false);
    }
  }

  async function saveAsScene(sceneName: string) {
    if (!roomId || !sceneName.trim()) return;
    setBusy(true);
    try {
      await createScene(sceneName.trim(), roomId);
      onScenesChanged();
    } catch (e) {
      await dialogs.alert({ title: "Couldn't save scene", message: String(e) });
    } finally {
      setBusy(false);
    }
  }

  async function deleteScene(sceneId: string) {
    setBusy(true);
    try {
      await removeScene(sceneId);
      onScenesChanged();
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      {/* The whole header is the room-editor button; the quick controls and
          power toggle opt out via stopPropagation. */}
      <header
        ref={headerRef}
        onClick={() => { if (tunable && interactive) setEditing((v) => !v); }}
        title={tunable ? "Set the whole room's color and brightness" : undefined}
        style={{
          position: "relative",
          display: "flex",
          alignItems: "center",
          gap: sz.headerGap,
          padding: sz.headerPad,
          borderBottom: page ? `1px solid ${T.hairline}` : "none",
          cursor: tunable && interactive ? "pointer" : "default",
        }}
      >
        {roomId && (
          <span
            aria-hidden
            style={{
              width: sz.dot,
              height: sz.dot,
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
          <span style={{ ...titleType, fontSize: sz.title, color: roomId ? T.text : T.faint, ...ELLIPSIS }}>
            {name}
          </span>
          <span style={{ fontSize: sz.sub, color: T.faint, ...ELLIPSIS }}>{subtitle}</span>
        </div>
        {(canPower || controls.length > 0) && (
          <div
            onClick={(e) => e.stopPropagation()}
            style={{ display: "flex", alignItems: "center", gap: sz.btnGap, flexShrink: 0 }}
          >
            {controls.map((c) => (
              <RoomControlButton
                key={c.id ?? `${c.kind}-${c.glyph}`}
                control={c}
                lights={lights}
                power={power}
                audio={audio}
                onLightUpdate={onLightUpdate}
                onPowerToggle={onPowerToggle}
                onMediaPatch={onMediaPatch}
                onChanged={onChanged}
                size={sz.btn}
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
                size={sz.btn}
              >
                <Glyph name="power" size={sz.btnGlyph} />
              </GlyphButton>
            )}
          </div>
        )}
      </header>

      {/* One glyph button per member device. The page card flows with its
          content; the board form fills its fixed plate and scrolls. */}
      {!page && total === 0 ? (
        <div style={{ position: "relative", flex: 1, minHeight: 0, display: "flex", alignItems: "center", justifyContent: "center" }}>
          <span style={{ fontSize: "0.78rem", color: T.faint }}>No devices</span>
        </div>
      ) : (
        <div
          {...(page ? {} : { "data-bf-scrollport": "" })}
          style={{
            position: "relative",
            display: "flex",
            flexWrap: "wrap",
            gap: sz.bodyGap,
            padding: sz.bodyPad,
            ...(page
              ? {}
              : { flex: 1, minHeight: 0, overflowY: "auto", alignContent: "flex-start", margin: -WIDGET_BLOOM_PAD }),
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
              <MediaButton
                key={`grp-${entry.coordinator.id}`}
                device={entry.coordinator}
                groupMembers={entry.members}
                onMediaPatch={onMediaPatch}
                onSetEnabled={onMediaSetEnabled}
              />
            ) : (
              <MediaButton
                key={entry.coordinator.id}
                device={entry.coordinator}
                onMediaPatch={onMediaPatch}
                onSetEnabled={onMediaSetEnabled}
              />
            ),
          )}
        </div>
      )}

      {editing && headerRef.current && (
        <LightEditor
          anchor={headerRef.current}
          title={name}
          initialHex={roomHex}
          initialBrightness={agg.brightness}
          initialMirek={agg.mirek}
          showColor={showColor}
          showWhite={showWhite}
          showBrightness={showBrightness}
          effects={agg.effects.length > 0 ? agg.effects : undefined}
          initialEffect={agg.commonEffect}
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
          scenes={scenes.filter((s) => s.room_id === roomId)}
          busy={busy}
          onApply={async (id) => { await applyScene(id); setScenesOpen(false); }}
          onSave={saveAsScene}
          onDelete={deleteScene}
          onClose={() => setScenesOpen(false)}
        />
      )}
    </>
  );
}
