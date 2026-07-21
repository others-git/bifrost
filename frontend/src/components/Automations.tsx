// Automation primitives shared by the Automations page and the Devices-page
// per-sensor modal: the data hook, the sentence row, and the editor. An
// automation's trigger subject is a sensor, a Room's occupancy, or a device's
// power (a TV turning on); the editor
// is one modal ("When [subject] [does X] … then […]") — never a nested box.

import { useEffect, useMemo, useState } from "react";
import {
  createAutomation,
  deleteAutomation,
  getAutomations,
  getLights,
  getMediaDevices,
  getPowerDevices,
  getRemoteApps,
  getRemoteDevices,
  getRooms,
  getScenes,
  getSensors,
  rgbToHex,
  rgbToXy,
  runAutomation,
  updateAutomation,
  xyToRgb,
  type ActionStep,
  type Automation,
  type AutomationBody,
  type AutomationTrigger,
  type Light,
  type LightState,
  type MediaDevice,
  type PowerDevice,
  type RemoteApp,
  type RemoteDevice,
  type Room,
  type RuleAction,
  type RuleCondition,
  type Scene,
  type SensorDevice,
  type SensorTrigger,
  type TriggerDeviceDomain,
} from "../api";
import { Button, Switch } from "./controls";
import { OptionCheckList, deviceSelectOptions } from "./deviceOptions";
import {
  pickableLights,
  pickableMedia,
  pickablePower,
  pickableRemotes,
  pickableSensors,
} from "../deviceSelectors";
import { Modal } from "./dialogs";
import { Flyout } from "./Flyout";
import { hexToRgb } from "./LightEditor";
import { Glyph } from "./glyphs";
import { Select } from "./Select";
import { S } from "../styles";
import { T, alpha, color } from "../theme";

// ── Subjects & phrasing ───────────────────────────────────────────────────────

/** What an automation listens to: a sensor's reading or a Room's occupancy. */
export type TriggerSubject =
  | { type: "sensor"; id: string; kind: SensorDevice["kind"] }
  | { type: "room"; id: string }
  | { type: "device"; domain: TriggerDeviceDomain; id: string }
  /** A macro — no event input; the rule runs only on demand. */
  | { type: "manual" };

function subjectOf(trigger: AutomationTrigger, sensors: SensorDevice[]): TriggerSubject {
  if (trigger.kind === "room") return { type: "room", id: trigger.room_id };
  if (trigger.kind === "device")
    return { type: "device", domain: trigger.domain, id: trigger.device_id };
  if (trigger.kind === "manual") return { type: "manual" };
  return {
    type: "sensor",
    id: trigger.sensor_id,
    kind: sensors.find((s) => s.id === trigger.sensor_id)?.kind ?? "generic",
  };
}

/** A stable grouping/select key for a subject. */
export function subjectKey(trigger: AutomationTrigger): string {
  if (trigger.kind === "room") return `room:${trigger.room_id}`;
  if (trigger.kind === "device") return `device:${trigger.domain}:${trigger.device_id}`;
  if (trigger.kind === "manual") return "manual";
  return `sensor:${trigger.sensor_id}`;
}

/** Whether a sensor kind carries a boolean reading (vs a numeric one). */
function isBoolKind(kind: SensorDevice["kind"]): boolean {
  return kind === "motion" || kind === "occupancy" || kind === "contact" || kind === "generic";
}

/** The event phrasing, tuned per subject so the sentence reads naturally. */
function eventOptions(subject: TriggerSubject): { value: string; label: string }[] {
  if (subject.type === "manual") return [];
  if (subject.type === "room") {
    return [
      { value: "became_true", label: "becomes occupied" },
      { value: "became_false", label: "becomes empty" },
      { value: "clear_for", label: "stays empty for…" },
      { value: "held_for", label: "stays occupied for…" },
    ];
  }
  if (subject.type === "device") {
    return [
      { value: "became_true", label: "turns on" },
      { value: "became_false", label: "turns off" },
      { value: "held_for", label: "stays on for…" },
      { value: "clear_for", label: "stays off for…" },
    ];
  }
  if (isBoolKind(subject.kind)) {
    const contact = subject.kind === "contact";
    return [
      { value: "became_true", label: contact ? "opens" : "detects motion" },
      { value: "became_false", label: contact ? "closes" : "clears" },
      { value: "clear_for", label: contact ? "stays closed for…" : "stays clear for…" },
      { value: "held_for", label: contact ? "stays open for…" : "stays detected for…" },
    ];
  }
  return [
    { value: "rose_above", label: "rises above…" },
    { value: "dropped_below", label: "drops below…" },
  ];
}

/** The rendered event phrase for a stored trigger. */
function triggerText(trigger: AutomationTrigger, sensors: SensorDevice[]): string {
  if (trigger.kind === "manual") return "run by hand (a button, voice, or the play icon)";
  const event = trigger.event;
  const label = (v: string) =>
    eventOptions(subjectOf(trigger, sensors)).find((o) => o.value === v)?.label ?? v;
  switch (event.kind) {
    case "became_true":
    case "became_false":
      return label(event.kind);
    case "clear_for":
    case "held_for":
      return `${label(event.kind).replace("…", "")} ${Math.round(event.secs / 60)} min`;
    case "rose_above":
      return `rises above ${event.value}`;
    case "dropped_below":
      return `drops below ${event.value}`;
  }
}

type NameMaps = {
  room: Map<string, string>;
  light: Map<string, string>;
  media: Map<string, string>;
  power: Map<string, string>;
  scene: Map<string, string>;
  remote: Map<string, string>;
};

/** "Office to 40% (colored)" — the compact clause-aware phrasing for list rows. */
function lightActionText(name: string, state: LightState): string {
  if (!state.on) return `${name} off`;
  const clauses = [
    ...(state.brightness != null ? [`to ${Math.round(state.brightness)}%`] : []),
    ...(state.color ? ["(colored)"] : []),
  ];
  return clauses.length > 0 ? `${name} ${clauses.join(" ")}` : `${name} on`;
}

function actionText(a: RuleAction, names: NameMaps): string {
  switch (a.kind) {
    case "room":
      return lightActionText(names.room.get(a.room_id) ?? "room", a.state);
    case "light":
      return lightActionText(names.light.get(a.light_id) ?? "light", a.state);
    case "power":
      return `${names.power.get(a.device_id) ?? "switch"} ${a.on ? "on" : "off"}`;
    case "scene":
      return `scene "${names.scene.get(a.scene_id) ?? "…"}"`;
    case "app":
      return `open ${a.app} on ${names.remote.get(a.remote_id) ?? "TV"}`;
    case "toggle": {
      const map = a.domain === "light" ? names.light : a.domain === "media" ? names.media : names.power;
      return `toggle ${map.get(a.device_id) ?? "device"}`;
    }
  }
}

const DAY_LABELS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

function conditionText(
  c: RuleCondition,
  names: NameMaps,
  sensorName: (id: string) => string,
): string {
  switch (c.kind) {
    case "time_window": {
      const days =
        c.days && c.days.length > 0 && c.days.length < 7
          ? ` (${c.days.map((d) => DAY_LABELS[d] ?? "?").join(", ")})`
          : "";
      return `between ${c.start} and ${c.end}${days}`;
    }
    case "sensor_above":
      return `${sensorName(c.sensor_id)} above ${c.value}`;
    case "sensor_below":
      return `${sensorName(c.sensor_id)} below ${c.value}`;
    case "sensor_is":
      return `${sensorName(c.sensor_id)} is ${c.on ? "on" : "off"}`;
    case "room_is":
      return `${names.room.get(c.room_id) ?? "room"} is ${c.occupied ? "occupied" : "empty"}`;
    case "device_is": {
      const name = names[c.domain].get(c.device_id) ?? "device";
      // on:false gates on the device being off — the "unless" reading.
      return c.on ? `only while ${name} is on` : `unless ${name} is on`;
    }
  }
}

/** "5m ago" style readout for the UTC last-fired stamp. */
function firedAgo(utc: string): string {
  const then = Date.parse(`${utc.replace(" ", "T")}Z`);
  if (Number.isNaN(then)) return "";
  const mins = Math.max(0, Math.round((Date.now() - then) / 60000));
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const h = Math.round(mins / 60);
  return h < 48 ? `${h}h ago` : `${Math.round(h / 24)}d ago`;
}

const ICON_BTN: React.CSSProperties = {
  background: "none",
  border: "none",
  color: "inherit",
  cursor: "pointer",
  padding: "0.2rem",
  minWidth: 40,
  minHeight: 40,
  display: "grid",
  placeItems: "center",
  fontSize: "0.85rem",
  lineHeight: 1,
  opacity: 0.75,
};

const SECTION_LABEL: React.CSSProperties = {
  fontSize: "0.68rem",
  letterSpacing: "0.1em",
  textTransform: "uppercase",
  color: T.faint,
};

const ROW: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: "0.45rem",
  flexWrap: "wrap",
};

/** One labelled section of the editor: an engraved label sitting tight above
 * its content, so the eye groups each part of the sentence. Sections are
 * spaced apart by the editor's own column gap. */
function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.45rem" }}>
      <span style={SECTION_LABEL}>
        {label}
        {hint && (
          <span
            style={{
              color: T.faint,
              fontWeight: 400,
              letterSpacing: 0,
              textTransform: "none",
              fontSize: "0.72rem",
            }}
          >
            {" "}
            · {hint}
          </span>
        )}
      </span>
      {children}
    </div>
  );
}

// ── Shared data ───────────────────────────────────────────────────────────────

/** The device/room/scene context every automation surface needs: the lists
 * for pickers plus name lookups for rendering rule sentences. Shared by the
 * per-sensor modal and the Automations page — one fetch shape, no forks. */
export function useAutomationData() {
  const [rooms, setRooms] = useState<Room[]>([]);
  const [lights, setLights] = useState<Light[]>([]);
  const [media, setMedia] = useState<MediaDevice[]>([]);
  const [power, setPower] = useState<PowerDevice[]>([]);
  const [scenes, setScenes] = useState<Scene[]>([]);
  const [sensors, setSensors] = useState<SensorDevice[]>([]);
  const [remotes, setRemotes] = useState<RemoteDevice[]>([]);

  useEffect(() => {
    getRooms().then(setRooms);
    getLights().then((l) => setLights(l === "unauthorized" ? [] : l));
    getMediaDevices().then(setMedia);
    getPowerDevices().then(setPower);
    getScenes().then(setScenes);
    getSensors().then(setSensors);
    getRemoteDevices().then(setRemotes);
  }, []);

  const names: NameMaps = useMemo(
    () => ({
      room: new Map(rooms.map((r) => [r.id, r.name])),
      light: new Map(lights.map((l) => [l.id, l.name])),
      media: new Map(media.map((m) => [m.id, m.name])),
      power: new Map(power.map((p) => [p.id, p.name])),
      scene: new Map(scenes.map((s) => [s.id, s.name])),
      remote: new Map(remotes.map((r) => [r.id, r.name])),
    }),
    [rooms, lights, media, power, scenes, remotes],
  );
  const sensorName = (id: string) => sensors.find((s) => s.id === id)?.name ?? "sensor";
  return { rooms, lights, media, power, scenes, sensors, remotes, names, sensorName };
}

type AutomationData = ReturnType<typeof useAutomationData>;

// ── Rule row ──────────────────────────────────────────────────────────────────

/** One automation as a sentence row: bolt, name/summary, run, enable switch,
 * duplicate, edit, delete. */
export function AutomationRow({
  rule,
  data,
  onToggle,
  onEdit,
  onDelete,
  onRun,
  onDuplicate,
}: {
  rule: Automation;
  data: AutomationData;
  onToggle: () => void;
  onEdit: () => void;
  onDelete: () => void;
  onRun?: () => void;
  onDuplicate?: () => void;
}) {
  const { names, sensors, sensorName } = data;
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: "0.6rem",
        padding: "0.55rem 0.7rem",
        borderRadius: 10,
        border: `1px solid ${rule.enabled ? alpha(color.cyan, 0.25) : T.cardBorder}`,
        background: rule.enabled ? T.card : T.cardOff,
        opacity: rule.enabled ? 1 : 0.65,
      }}
    >
      <span style={{ color: rule.enabled ? color.cyan : T.dim, flexShrink: 0 }}>
        <Glyph name="bolt" size={16} />
      </span>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontSize: "0.85rem", color: T.text, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {rule.name || `When it ${triggerText(rule.trigger, sensors)}`}
        </div>
        <div style={{ fontSize: "0.72rem", color: T.faint, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {`${triggerText(rule.trigger, sensors)} → ${rule.steps
            .flatMap((s) => s.actions)
            .map((a) => actionText(a, names))
            .join(", ")}`}
          {rule.conditions.length > 0 &&
            ` · only ${rule.conditions.map((c) => conditionText(c, names, sensorName)).join(", ")}`}
          {rule.restore_secs != null &&
            ` · puts things back after ${Math.round(rule.restore_secs / 60)} min`}
          {rule.last_fired_at && ` · ran ${firedAgo(rule.last_fired_at)}`}
        </div>
      </div>
      {onRun && (
        <button onClick={onRun} title="Run the actions now" style={ICON_BTN}>
          <Glyph name="play" size={14} />
        </button>
      )}
      <Switch on={rule.enabled} onChange={onToggle} />
      {onDuplicate && (
        <button onClick={onDuplicate} title="Duplicate" style={ICON_BTN}>
          <Glyph name="copy" size={15} />
        </button>
      )}
      <button onClick={onEdit} title="Edit" style={ICON_BTN}>
        <Glyph name="gear" size={15} />
      </button>
      <button onClick={onDelete} title="Delete" style={ICON_BTN}>
        ✕
      </button>
    </div>
  );
}

/** Duplicate a rule server-side: same body, "(copy)" name, disabled so the
 * copy can be tuned before it starts firing. Returns the new rule, if saved. */
export async function duplicateAutomation(rule: Automation): Promise<Automation | null> {
  const result = await createAutomation({
    name: rule.name ? `${rule.name} (copy)` : "",
    enabled: false,
    trigger: rule.trigger,
    conditions: rule.conditions,
    steps: rule.steps,
    cooldown_secs: rule.cooldown_secs,
  });
  return typeof result === "string" ? null : result;
}

// ── The editor modal ──────────────────────────────────────────────────────────

/** One automation, one modal: the sentence builder as the whole surface. The
 * Automations page opens this directly — no picker step, no list detour. */
export function AutomationEditorModal({
  initial,
  initialSubject,
  data,
  onSaved,
  onClose,
}: {
  /** The automation being edited, or null for a new one. */
  initial: Automation | null;
  /** Pre-selected trigger subject (e.g. a group's + button); still changeable. */
  initialSubject?: TriggerSubject;
  data: AutomationData;
  onSaved: (rule: Automation) => void;
  onClose: () => void;
}) {
  const [error, setError] = useState<string | null>(null);

  async function save(body: AutomationBody) {
    const result = initial
      ? await updateAutomation(initial.id, body)
      : await createAutomation(body);
    if (typeof result === "string") {
      setError(result);
      return;
    }
    onSaved(result);
  }

  return (
    <Modal title={initial ? "Edit automation" : "New automation"} onClose={onClose} width={640}>
      <AutomationEditor
        initial={initial}
        initialSubject={initialSubject}
        data={data}
        error={error}
        onCancel={onClose}
        onSave={save}
      />
    </Modal>
  );
}

/** The per-sensor automations list, opened from a sensor's detail panel on the
 * Devices page. Add/edit swaps the whole modal body to the editor. */
export function AutomationsModal({
  sensor,
  onClose,
}: {
  sensor: { id: string; name: string; kind: SensorDevice["kind"] };
  onClose: () => void;
}) {
  const [rules, setRules] = useState<Automation[]>([]);
  const data = useAutomationData();
  const [editing, setEditing] = useState<Automation | "new" | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getAutomations().then((all) =>
      setRules(all.filter((r) => r.trigger.kind === "sensor" && r.trigger.sensor_id === sensor.id)),
    );
  }, [sensor.id]);

  async function save(body: AutomationBody, existing: Automation | "new") {
    const result =
      existing === "new" ? await createAutomation(body) : await updateAutomation(existing.id, body);
    if (typeof result === "string") {
      setError(result);
      return;
    }
    setError(null);
    setEditing(null);
    setRules((rs) =>
      existing === "new" ? [...rs, result] : rs.map((r) => (r.id === result.id ? result : r)),
    );
  }

  async function toggleEnabled(rule: Automation) {
    await save({ ...rule, enabled: !rule.enabled }, rule);
  }

  async function remove(rule: Automation) {
    await deleteAutomation(rule.id);
    setRules((rs) => rs.filter((r) => r.id !== rule.id));
  }

  // The editor takes over the whole modal — a sentence in one surface, never a
  // box nested inside the list.
  if (editing) {
    return (
      <Modal
        title={editing === "new" ? "New automation" : "Edit automation"}
        onClose={onClose}
        width={640}
      >
        <AutomationEditor
          initial={editing === "new" ? null : editing}
          initialSubject={{ type: "sensor", id: sensor.id, kind: sensor.kind }}
          data={data}
          error={error}
          onCancel={() => {
            setEditing(null);
            setError(null);
          }}
          onSave={(body) => save(body, editing)}
        />
      </Modal>
    );
  }

  return (
    <Modal title={`${sensor.name} — automations`} onClose={onClose} width={460}>
      <div style={{ display: "flex", flexDirection: "column", gap: "0.6rem" }}>
        {rules.length === 0 && (
          <p style={{ margin: 0, color: T.dim, fontSize: "0.85rem" }}>
            No automations yet. A rule runs actions when this sensor changes — for
            example, turn a room on when motion is detected, or off after it stays
            clear.
          </p>
        )}

        {rules.map((rule) => (
          <AutomationRow
            key={rule.id}
            rule={rule}
            data={data}
            onToggle={() => toggleEnabled(rule)}
            onEdit={() => setEditing(rule)}
            onDelete={() => remove(rule)}
            onRun={() => runAutomation(rule.id)}
            onDuplicate={async () => {
              const copy = await duplicateAutomation(rule);
              if (copy) setRules((rs) => [...rs, copy]);
            }}
          />
        ))}

        <Button variant="accent" onClick={() => setEditing("new")}>
          Add automation
        </Button>
      </div>
    </Modal>
  );
}

// ── The sentence builder ──────────────────────────────────────────────────────

/** The sentence builder for one automation. The trigger subject — a sensor or
 * a Room's occupancy — is the first word of the sentence: "When [subject]
 * [does X] … then […]", authored in one surface with no picker step before it. */
function AutomationEditor({
  initial,
  initialSubject,
  data,
  error,
  onCancel,
  onSave,
}: {
  initial: Automation | null;
  initialSubject?: TriggerSubject;
  data: AutomationData;
  error: string | null;
  onCancel: () => void;
  onSave: (body: AutomationBody) => void;
}) {
  const { rooms, lights, media, power, sensors } = data;
  const initialEvent =
    initial && initial.trigger.kind !== "manual" ? initial.trigger.event : undefined;
  const [subject, setSubject] = useState<TriggerSubject | undefined>(
    initial ? subjectOf(initial.trigger, sensors) : initialSubject,
  );
  // The sensor list loads async; re-resolve a sensor subject's kind once it's in.
  useEffect(() => {
    if (initial) setSubject(subjectOf(initial.trigger, sensors));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sensors.length]);

  const [name, setName] = useState(initial?.name ?? "");
  const [eventKind, setEventKind] = useState<string>(
    initialEvent?.kind ??
      (initialSubject && initialSubject.type === "sensor" && !isBoolKind(initialSubject.kind)
        ? "rose_above"
        : "became_true"),
  );
  const [stayMins, setStayMins] = useState(
    initialEvent?.kind === "clear_for" || initialEvent?.kind === "held_for"
      ? Math.max(1, Math.round(initialEvent.secs / 60))
      : 10,
  );
  const [threshold, setThreshold] = useState(
    initialEvent?.kind === "rose_above" || initialEvent?.kind === "dropped_below"
      ? initialEvent.value
      : 0,
  );
  const [conditions, setConditions] = useState<RuleCondition[]>(initial?.conditions ?? []);
  // The "then" is a list of steps, each `{conditions, actions}`. A new rule
  // starts with one empty, unconditional step.
  const [steps, setSteps] = useState<ActionStep[]>(
    initial?.steps?.length ? initial.steps : [{ conditions: [], actions: [] }],
  );
  const totalActions = steps.reduce((n, s) => n + s.actions.length, 0);
  const setStep = (i: number, patch: Partial<ActionStep>) =>
    setSteps((cur) => cur.map((s, j) => (j === i ? { ...s, ...patch } : s)));
  const [advancedOpen, setAdvancedOpen] = useState(
    !!(initial && (initial.cooldown_secs > 0 || initial.restore_secs)),
  );
  const [cooldownMins, setCooldownMins] = useState(
    initial ? Math.round(initial.cooldown_secs / 60) : 0,
  );
  // Timed hold: 0 = off (changes stick), N = put things back after N minutes.
  const [restoreMins, setRestoreMins] = useState(
    initial?.restore_secs ? Math.max(1, Math.round(initial.restore_secs / 60)) : 0,
  );

  /** Switching the subject can change its reading type; keep the event legal. */
  function pickSubject(key: string) {
    let next: TriggerSubject;
    if (key === "manual") {
      next = { type: "manual" };
    } else if (key.startsWith("room:")) {
      next = { type: "room", id: key.slice(5) };
    } else if (key.startsWith("device:")) {
      const rest = key.slice(7);
      const sep = rest.indexOf(":");
      next = {
        type: "device",
        domain: rest.slice(0, sep) as TriggerDeviceDomain,
        id: rest.slice(sep + 1),
      };
    } else {
      next = {
        type: "sensor",
        id: key.slice(7),
        kind: sensors.find((s) => s.id === key.slice(7))?.kind ?? "generic",
      };
    }
    setSubject(next);
    const opts = eventOptions(next);
    if (opts.length > 0 && !opts.some((o) => o.value === eventKind)) {
      setEventKind(opts[0].value);
    }
  }

  function builtEvent(): SensorTrigger {
    if (eventKind === "clear_for") return { kind: "clear_for", secs: stayMins * 60 };
    if (eventKind === "held_for") return { kind: "held_for", secs: stayMins * 60 };
    if (eventKind === "rose_above") return { kind: "rose_above", value: threshold };
    if (eventKind === "dropped_below") return { kind: "dropped_below", value: threshold };
    return { kind: eventKind as "became_true" | "became_false" };
  }

  function builtTrigger(): AutomationTrigger | null {
    if (!subject) return null;
    if (subject.type === "manual") return { kind: "manual" };
    if (subject.type === "room") return { kind: "room", room_id: subject.id, event: builtEvent() };
    if (subject.type === "device")
      return { kind: "device", domain: subject.domain, device_id: subject.id, event: builtEvent() };
    return { kind: "sensor", sensor_id: subject.id, event: builtEvent() };
  }

  const numInput: React.CSSProperties = { ...S.input, width: 72, padding: "0.35rem 0.5rem" };
  const isStay = eventKind === "clear_for" || eventKind === "held_for";

  // One grouped subject picker: Rooms (aggregate occupancy) first, then every
  // watchable subject — sensors, TVs & speakers (surfaces only), lights,
  // switches — under its room's header (the shared room-grouped pattern; ids
  // pre-prefixed to carry the subject kind).
  const subjectOptions = [
    // The macro subject: no event input — the rule is a named action list an
    // AIO board button (or voice / the play icon) runs on demand.
    { value: "manual", label: "I press a button (macro)", group: "Manual" },
    ...rooms
      .filter((r) => r.enabled)
      .map((r) => ({ value: `room:${r.id}`, label: r.name, group: "Rooms" })),
    ...deviceSelectOptions(
      [
        ...pickableSensors(sensors).map((s) => ({
          ...s,
          id: `sensor:${s.id}`,
          // No reading = it can't fire (disabled at its bridge/hub, or has
          // never reported) — say so where the rule gets bound.
          name: s.state.reading == null ? `${s.name} — no signal` : s.name,
        })),
        ...pickableMedia(media).map((m) => ({ ...m, id: `device:media:${m.id}` })),
        ...pickableLights(lights).map((l) => ({ ...l, id: `device:light:${l.id}` })),
        ...pickablePower(power).map((d) => ({ ...d, id: `device:power:${d.id}` })),
      ],
      rooms,
    ),
  ];
  const subjectValue = subject
    ? subject.type === "manual"
      ? "manual"
      : subject.type === "room"
        ? `room:${subject.id}`
        : subject.type === "device"
          ? `device:${subject.domain}:${subject.id}`
          : `sensor:${subject.id}`
    : undefined;

  // Gate subjects offered as conditions: anything except the trigger itself.
  const gateSensors = pickableSensors(sensors).filter(
    (s) => !(subject?.type === "sensor" && s.id === subject.id),
  );
  const gateRooms = rooms.filter(
    (r) => r.enabled && !(subject?.type === "room" && r.id === subject.id),
  );

  const hairline: React.CSSProperties = {
    border: "none",
    borderTop: `1px solid ${T.cardBorder}`,
    margin: 0,
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "1.15rem" }}>
      <input
        style={S.input}
        placeholder="Name this automation (optional)"
        value={name}
        onChange={(e) => setName(e.target.value)}
      />

      <Field label="When">
        <div style={ROW}>
          <Select
            value={subjectValue}
            options={subjectOptions}
            onChange={pickSubject}
            placeholder="Pick a trigger"
            width={200}
            searchable
            empty="No sensors yet — add a Hue or Home Assistant provider first"
          />
          {subject?.type !== "manual" && (
            <Select
              value={eventKind}
              options={subject ? eventOptions(subject) : []}
              onChange={setEventKind}
              width={180}
              disabled={!subject}
            />
          )}
          {isStay && (
            <>
              <input
                type="number"
                min={1}
                max={1440}
                style={numInput}
                value={stayMins}
                onChange={(e) => setStayMins(Math.max(1, Number(e.target.value) || 1))}
              />
              <span style={{ color: T.dim, fontSize: "0.8rem" }}>minutes</span>
            </>
          )}
          {(eventKind === "rose_above" || eventKind === "dropped_below") && (
            <input
              type="number"
              style={numInput}
              value={threshold}
              onChange={(e) => setThreshold(Number(e.target.value) || 0)}
            />
          )}
        </div>
      </Field>

      <Field label="Only if" hint="optional">
        <ConditionList
          conditions={conditions}
          gateSensors={gateSensors}
          gateRooms={gateRooms}
          allRooms={rooms}
          lights={lights}
          media={media}
          power={power}
          onChange={setConditions}
        />
      </Field>

      <Field label="Then">
        <div style={{ display: "flex", flexDirection: "column", gap: steps.length > 1 ? "0.9rem" : "0.5rem" }}>
          {steps.map((step, i) => (
            <StepCard
              key={i}
              step={step}
              index={i}
              grouped={steps.length > 1}
              data={data}
              gateSensors={gateSensors}
              gateRooms={gateRooms}
              onChange={(patch) => setStep(i, patch)}
              onRemove={
                steps.length > 1
                  ? () => setSteps((cur) => cur.filter((_, j) => j !== i))
                  : undefined
              }
            />
          ))}
          <div>
            <Button
              variant="ghost"
              onClick={() => setSteps((cur) => [...cur, { conditions: [], actions: [] }])}
              title="Add a second group of actions that runs only when its own condition is met"
              style={{ fontSize: "0.76rem", padding: "0.2rem 0.6rem" }}
            >
              + Add a conditional step
            </Button>
          </div>
        </div>
      </Field>

      <hr style={hairline} />

      {/* Advanced settings — the rare knobs, collapsed so the common rule stays
          short. */}
      <div style={{ display: "flex", flexDirection: "column", gap: "0.5rem" }}>
        <button
          onClick={() => setAdvancedOpen((v) => !v)}
          style={{
            ...SECTION_LABEL,
            background: "none",
            border: "none",
            padding: 0,
            cursor: "pointer",
            textAlign: "left",
            display: "inline-flex",
            alignItems: "center",
            gap: "0.3rem",
          }}
        >
          {advancedOpen ? "▾" : "▸"} Advanced
        </button>
        {advancedOpen && (
          <>
            <div style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
              <span style={{ color: T.dim, fontSize: "0.8rem" }}>Don't re-run within</span>
              <input
                type="number"
                min={0}
                style={numInput}
                value={cooldownMins}
                onChange={(e) => setCooldownMins(Math.max(0, Number(e.target.value) || 0))}
              />
              <span style={{ color: T.dim, fontSize: "0.8rem" }}>minutes</span>
            </div>
            <div style={{ display: "flex", alignItems: "center", gap: "0.5rem", flexWrap: "wrap" }}>
              <Switch on={restoreMins > 0} onChange={(v) => setRestoreMins(v ? 10 : 0)} />
              <span style={{ color: restoreMins > 0 ? T.text : T.dim, fontSize: "0.8rem" }}>
                Put things back after
              </span>
              <input
                type="number"
                min={1}
                max={1440}
                disabled={restoreMins === 0}
                style={{ ...numInput, opacity: restoreMins === 0 ? 0.5 : 1 }}
                value={restoreMins || 10}
                onChange={(e) => setRestoreMins(Math.max(1, Number(e.target.value) || 1))}
              />
              <span style={{ color: restoreMins > 0 ? T.text : T.dim, fontSize: "0.8rem" }}>
                minutes — everything this rule changes returns to how it was
              </span>
            </div>
          </>
        )}
      </div>

      {error && <span style={{ color: T.bad, fontSize: "0.8rem" }}>{error}</span>}
      <div style={{ display: "flex", gap: "0.5rem", justifyContent: "flex-end", marginTop: "0.25rem" }}>
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button
          variant="primary"
          disabled={!subject || totalActions === 0}
          onClick={() => {
            const trigger = builtTrigger();
            if (!trigger) return;
            onSave({
              name,
              enabled: initial?.enabled ?? true,
              trigger,
              conditions,
              // Drop any empty step the user added but never filled.
              steps: steps.filter((s) => s.actions.length > 0),
              cooldown_secs: cooldownMins * 60,
              restore_secs: restoreMins > 0 ? restoreMins * 60 : null,
            });
          }}
        >
          Save automation
        </Button>
      </div>
    </div>
  );
}

// ── Conditions ────────────────────────────────────────────────────────────────

function ConditionList({
  conditions,
  gateSensors,
  gateRooms,
  allRooms,
  lights,
  media,
  power,
  onChange,
}: {
  conditions: RuleCondition[];
  gateSensors: SensorDevice[];
  /** Rooms offered as occupancy gates (the trigger's own room excluded). */
  gateRooms: Room[];
  /** Every room — for grouping sensors under their room header. */
  allRooms: Room[];
  /** Devices offered as power gates ("…unless the TV is on"). */
  lights: Light[];
  media: MediaDevice[];
  power: PowerDevice[];
  onChange: (c: RuleCondition[]) => void;
}) {
  const set = (i: number, c: RuleCondition) => onChange(conditions.map((x, j) => (j === i ? c : x)));
  const numInput: React.CSSProperties = { ...S.input, width: 72, padding: "0.35rem 0.5rem" };
  // The same native time element the kiosk quiet-hours schedule uses — the
  // browser guarantees HH:MM, so a typo can't silently disable the window.
  const timeInput: React.CSSProperties = {
    ...S.input,
    width: 96,
    padding: "0.3rem 0.45rem",
    colorScheme: "dark",
  };

  // One grouped gate picker: Rooms' occupancy first, then sensors under their
  // room headers (the shared room-grouped pattern).
  const gateOptions = [
    ...gateRooms.map((r) => ({ value: `room:${r.id}`, label: r.name, group: "Rooms" })),
    ...deviceSelectOptions(
      gateSensors.map((s) => ({ ...s, id: `sensor:${s.id}` })),
      allRooms,
    ),
    // Device power gates ("…unless the TV is on") — grouped after the sensors,
    // with a suffixed header so a room's sensors and devices don't interleave.
    ...deviceSelectOptions(
      [
        ...media.filter((d) => d.enabled !== false).map((d) => ({ ...d, id: `device:media:${d.id}` })),
        ...power.filter((d) => d.enabled !== false).map((d) => ({ ...d, id: `device:power:${d.id}` })),
        ...lights.filter((l) => l.enabled !== false).map((l) => ({ ...l, id: `device:light:${l.id}` })),
      ],
      allRooms,
    ).map((o) => ({ ...o, group: `${o.group} · devices` })),
  ];
  const numericGate = (id: string) => {
    const k = gateSensors.find((s) => s.id === id)?.kind;
    return k === "illuminance" || k === "temperature" || k === "humidity";
  };
  /** The stored condition for a newly picked gate subject, keeping what carries over. */
  const forGate = (key: string, prev: RuleCondition): RuleCondition => {
    if (key.startsWith("room:")) return { kind: "room_is", room_id: key.slice(5), occupied: true };
    if (key.startsWith("device:")) {
      const [, domain, id] = key.split(":");
      // "unless it's on" is the headline use — the default polarity.
      const on = prev.kind === "device_is" ? prev.on : false;
      return { kind: "device_is", domain: domain as TriggerDeviceDomain, device_id: id, on };
    }
    const id = key.slice(7);
    if (numericGate(id)) {
      const value = prev.kind === "sensor_above" || prev.kind === "sensor_below" ? prev.value : 20;
      return {
        kind: prev.kind === "sensor_above" ? "sensor_above" : "sensor_below",
        sensor_id: id,
        value,
      };
    }
    return { kind: "sensor_is", sensor_id: id, on: true };
  };
  const gateValue = (c: RuleCondition) =>
    c.kind === "room_is"
      ? `room:${c.room_id}`
      : c.kind === "device_is"
        ? `device:${c.domain}:${c.device_id}`
        : c.kind === "time_window"
          ? ""
          : `sensor:${c.sensor_id}`;

  /** Weekday chips: no selection = every day; toggling curates the list. */
  const toggleDay = (c: Extract<RuleCondition, { kind: "time_window" }>, i: number, d: number) => {
    const current = c.days && c.days.length > 0 ? c.days : [0, 1, 2, 3, 4, 5, 6];
    const next = current.includes(d) ? current.filter((x) => x !== d) : [...current, d].sort();
    // Back to "every day" collapses to no filter; an empty pick is meaningless.
    set(i, { ...c, days: next.length === 0 || next.length === 7 ? undefined : next });
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.45rem" }}>
      {conditions.map((c, i) => (
        <div key={i} style={ROW}>
          {c.kind === "time_window" ? (
            <>
              <span style={{ color: T.dim, fontSize: "0.8rem" }}>between</span>
              <input
                type="time"
                style={timeInput}
                value={c.start}
                onChange={(e) => set(i, { ...c, start: e.target.value || c.start })}
              />
              <span style={{ color: T.dim, fontSize: "0.8rem" }}>and</span>
              <input
                type="time"
                style={timeInput}
                value={c.end}
                onChange={(e) => set(i, { ...c, end: e.target.value || c.end })}
              />
              <span style={{ display: "inline-flex", gap: 3 }}>
                {DAY_LABELS.map((label, d) => {
                  const active = !c.days || c.days.length === 0 || c.days.includes(d);
                  return (
                    <button
                      key={label}
                      onClick={() => toggleDay(c, i, d)}
                      title={label}
                      style={{
                        border: `1px solid ${active ? alpha(color.cyan, 0.5) : T.cardBorder}`,
                        background: active ? alpha(color.cyan, 0.14) : "transparent",
                        color: active ? T.text : T.faint,
                        borderRadius: 5,
                        padding: "0.18rem 0.3rem",
                        fontSize: "0.62rem",
                        cursor: "pointer",
                        lineHeight: 1,
                      }}
                    >
                      {label[0]}
                    </button>
                  );
                })}
              </span>
            </>
          ) : (
            <>
              <Select
                value={gateValue(c)}
                options={gateOptions}
                onChange={(key) => set(i, forGate(key, c))}
                width={170}
                searchable
              />
              {(c.kind === "sensor_above" || c.kind === "sensor_below") && (
                <>
                  <Select
                    value={c.kind}
                    options={[
                      { value: "sensor_below", label: "below" },
                      { value: "sensor_above", label: "above" },
                    ]}
                    onChange={(k) => set(i, { ...c, kind: k as "sensor_above" | "sensor_below" })}
                    width={100}
                  />
                  <input
                    type="number"
                    style={numInput}
                    value={c.value}
                    onChange={(e) => set(i, { ...c, value: Number(e.target.value) || 0 })}
                  />
                </>
              )}
              {c.kind === "sensor_is" && (
                <Select
                  value={c.on ? "on" : "off"}
                  options={[
                    { value: "on", label: "is on / detecting" },
                    { value: "off", label: "is off / clear" },
                  ]}
                  onChange={(v) => set(i, { ...c, on: v === "on" })}
                  width={150}
                />
              )}
              {c.kind === "room_is" && (
                <Select
                  value={c.occupied ? "occupied" : "empty"}
                  options={[
                    { value: "occupied", label: "is occupied" },
                    { value: "empty", label: "is empty" },
                  ]}
                  onChange={(v) => set(i, { ...c, occupied: v === "occupied" })}
                  width={140}
                />
              )}
              {c.kind === "device_is" && (
                <Select
                  value={c.on ? "only_on" : "unless_on"}
                  options={[
                    // on:false gates on the device being off — "unless it's on".
                    { value: "unless_on", label: "unless it's on" },
                    { value: "only_on", label: "only while it's on" },
                  ]}
                  onChange={(v) => set(i, { ...c, on: v === "only_on" })}
                  width={170}
                />
              )}
            </>
          )}
          <button
            onClick={() => onChange(conditions.filter((_, j) => j !== i))}
            title="Remove condition"
            style={ICON_BTN}
          >
            ✕
          </button>
        </div>
      ))}
      {/* One add affordance — a small menu, not two competing buttons. */}
      <Select
        value=""
        options={[
          { value: "time", label: "Time window" },
          ...(gateOptions.length > 0
            ? [{ value: "gate", label: "A sensor, room, or device" }]
            : []),
        ]}
        onChange={(v) =>
          onChange(
            v === "time"
              ? [...conditions, { kind: "time_window", start: "21:00", end: "06:00" }]
              : [...conditions, forGate(gateOptions[0].value, { kind: "time_window", start: "", end: "" })],
          )
        }
        placeholder="+ Add condition"
        width={160}
      />
    </div>
  );
}

// ── Actions ───────────────────────────────────────────────────────────────────

/** One authored action step, read as a sentence: **[verb] (and clause…) to
 * [several targets]** — "turn on *and set brightness to 40%* — Office, Desk
 * lamp". Storage stays one action per target (the backend model is untouched):
 * rows group stored actions by their verb signature and flatten back on every
 * edit. `brightness`/`colorHex` are the optional **"and…" clauses** on "turn
 * on" — clauses compose into ONE command per target (`{on:true, brightness,
 * color}`), never separate writes, and only "turn on" carries them: a light
 * won't change color while off, so "set color" without the power verb would
 * silently do nothing — the sentence always says "turn on and…". */
type ActionRow = {
  targets: string[];
  verb: "on" | "off" | "toggle" | "scene";
  /** The "and set brightness to N%" clause; only meaningful on `on`. */
  brightness: number | null;
  /** The "and set color to ▮" clause (hex); only meaningful on `on`. */
  colorHex: string | null;
};

const rowSig = (r: Pick<ActionRow, "verb" | "brightness" | "colorHex">) =>
  r.verb === "on" ? `on:${r.brightness ?? ""}:${r.colorHex ?? ""}` : r.verb;

function rowsFromActions(actions: RuleAction[]): ActionRow[] {
  const rows: ActionRow[] = [];
  for (const a of actions) {
    let verb: ActionRow["verb"];
    let brightness: number | null = null;
    let colorHex: string | null = null;
    let target: string;
    // App actions are handled by their own list (AppActionRow), not the
    // verb-grouped rows — skip any that slip in.
    if (a.kind === "app") continue;
    if (a.kind === "toggle") {
      verb = "toggle";
      target = `${a.domain}:${a.device_id}`;
    } else if (a.kind === "scene") {
      verb = "scene";
      target = `scene:${a.scene_id}`;
    } else if (a.kind === "power") {
      verb = a.on ? "on" : "off";
      target = `power:${a.device_id}`;
    } else {
      verb = a.state.on ? "on" : "off";
      // Whole numbers only — an API-authored rule may carry a fractional value.
      brightness = a.state.brightness != null ? Math.round(a.state.brightness) : null;
      const c = a.state.color;
      colorHex = c ? rgbToHex(...xyToRgb(c.x, c.y, c.brightness)) : null;
      target = a.kind === "room" ? `room:${a.room_id}` : `light:${a.light_id}`;
    }
    const next = { verb, brightness, colorHex };
    const row = rows.find((r) => rowSig(r) === rowSig(next));
    if (row) row.targets.push(target);
    else rows.push({ targets: [target], ...next });
  }
  return rows;
}

function actionsFromRows(rows: ActionRow[]): RuleAction[] {
  return rows.flatMap((r) =>
    r.targets.map((t): RuleAction => {
      const sep = t.indexOf(":");
      const kind = t.slice(0, sep);
      const id = t.slice(sep + 1);
      if (kind === "scene") return { kind: "scene", scene_id: id };
      if (r.verb === "toggle") {
        // Toggle targets carry their own domain in the prefix (light/media/power).
        return { kind: "toggle", domain: kind as TriggerDeviceDomain, device_id: id };
      }
      const on = r.verb !== "off";
      if (kind === "power") return { kind: "power", device_id: id, on };
      const state: LightState = { on };
      if (r.verb === "on" && r.brightness != null) state.brightness = r.brightness;
      if (r.verb === "on" && r.colorHex) state.color = rgbToXy(...hexToRgb(r.colorHex));
      return kind === "room"
        ? { kind: "room", room_id: id, state }
        : { kind: "light", light_id: id, state };
    }),
  );
}

/** One "open <app> on <TV>" action row. The app list is the remote's own
 * catalog (pinned ∪ recents), fetched when the remote changes — the same
 * source the remote UI's launcher uses, so a rule offers exactly what's
 * installed. */
function AppActionRow({
  action,
  remotes,
  onChange,
  onRemove,
}: {
  action: Extract<RuleAction, { kind: "app" }>;
  remotes: RemoteDevice[];
  onChange: (patch: Partial<Extract<RuleAction, { kind: "app" }>>) => void;
  onRemove: () => void;
}) {
  const [apps, setApps] = useState<RemoteApp[]>([]);
  useEffect(() => {
    if (!action.remote_id) {
      setApps([]);
      return;
    }
    let alive = true;
    getRemoteApps(action.remote_id).then((a) => {
      if (alive) setApps(a);
    });
    return () => {
      alive = false;
    };
  }, [action.remote_id]);

  // Launch with the vendor URI when the catalog has one, else the bare package
  // (the provider wraps it) — the same value the remote's own launcher sends.
  const appValueOf = (a: RemoteApp) => a.activity ?? a.package;

  return (
    <div style={ROW}>
      {/* Read left-to-right in pick order: choose the TV first (it populates
          the app list), then the app. */}
      <span style={{ color: T.dim, fontSize: "0.8rem", whiteSpace: "nowrap" }}>on</span>
      <Select
        value={action.remote_id || undefined}
        options={remotes.map((r) => ({ value: r.id, label: r.name }))}
        onChange={(v) => onChange({ remote_id: v, app: "" })}
        placeholder="Pick a TV"
        width={168}
      />
      <span style={{ color: T.dim, fontSize: "0.8rem", whiteSpace: "nowrap" }}>open</span>
      <Select
        value={apps.some((a) => appValueOf(a) === action.app) ? action.app : undefined}
        options={apps.map((a) => ({ value: appValueOf(a), label: a.name }))}
        onChange={(v) => onChange({ app: v })}
        placeholder={action.remote_id ? "Pick an app" : "Pick a TV first"}
        width={168}
        searchable
        disabled={!action.remote_id}
        empty="No apps found — open one on the TV so it's learned"
      />
      <button onClick={onRemove} title="Remove this step" style={ICON_BTN}>
        ✕
      </button>
    </div>
  );
}

/** One step of the "then". A single-step rule shows its actions *flat* — no
 * chrome, no per-step condition (the rule-level "Only if" already gates it).
 * Once a rule has more than one step (`grouped`), each becomes a light,
 * accent-barred group with a "Step N" label and its own "only when" gate — so
 * branching (do X, but only-if…; else do Y) reads as an ordered list without a
 * heavy nested box. */
function StepCard({
  step,
  index,
  grouped,
  data,
  gateSensors,
  gateRooms,
  onChange,
  onRemove,
}: {
  step: ActionStep;
  index: number;
  /** More than one step exists — show the group chrome + per-step gate. */
  grouped: boolean;
  data: AutomationData;
  gateSensors: SensorDevice[];
  gateRooms: Room[];
  onChange: (patch: Partial<ActionStep>) => void;
  onRemove?: () => void;
}) {
  const { rooms, lights, media, power, scenes, remotes } = data;
  const [showConditions, setShowConditions] = useState(step.conditions.length > 0);

  const actions = (
    <ActionList
      actions={step.actions}
      rooms={rooms}
      lights={lights}
      media={media}
      power={power}
      scenes={scenes}
      remotes={remotes}
      onChange={(actions) => onChange({ actions })}
    />
  );

  // Single step: flat, no chrome — the common case stays clean.
  if (!grouped) return actions;

  // Grouped: a quiet left accent bar + label, not a boxed card.
  return (
    <div style={{ borderLeft: `2px solid ${alpha(color.gold, 0.4)}`, paddingLeft: "0.7rem", display: "flex", flexDirection: "column", gap: "0.5rem" }}>
      <div style={{ display: "flex", alignItems: "center" }}>
        <span style={{ ...SECTION_LABEL, fontSize: "0.62rem" }}>Step {index + 1}</span>
        <span style={{ flex: 1 }} />
        {onRemove && (
          <button onClick={onRemove} title="Remove this step" style={{ ...ICON_BTN }}>
            ✕
          </button>
        )}
      </div>
      {actions}
      {showConditions ? (
        <div style={{ display: "flex", flexDirection: "column", gap: "0.35rem" }}>
          <span style={{ ...SECTION_LABEL, fontSize: "0.62rem" }}>Only when</span>
          <ConditionList
            conditions={step.conditions}
            gateSensors={gateSensors}
            gateRooms={gateRooms}
            allRooms={rooms}
            lights={lights}
            media={media}
            power={power}
            onChange={(conditions) => onChange({ conditions })}
          />
        </div>
      ) : (
        <div>
          <Button
            variant="ghost"
            onClick={() => setShowConditions(true)}
            title="Run this step only when a condition holds"
            style={{ fontSize: "0.76rem", padding: "0.2rem 0.6rem" }}
          >
            + only when…
          </Button>
        </div>
      )}
    </div>
  );
}

function ActionList({
  actions,
  rooms,
  lights,
  media,
  power,
  scenes,
  remotes,
  onChange,
}: {
  actions: RuleAction[];
  rooms: Room[];
  lights: Light[];
  media: MediaDevice[];
  power: PowerDevice[];
  scenes: Scene[];
  remotes: RemoteDevice[];
  onChange: (a: RuleAction[]) => void;
}) {
  // App-launch actions have a fundamentally different shape (one remote + one
  // app, un-mergeable) from the verb-grouped device/scene actions, so they
  // live in their own list below rather than distorting the verb machinery.
  const appActions = actions.filter((a): a is Extract<RuleAction, { kind: "app" }> => a.kind === "app");
  const verbActions = actions.filter((a) => a.kind !== "app");
  const rows = rowsFromActions(verbActions);
  const tvRemotes = pickableRemotes(remotes);
  // A new action being built: a row with no targets can't live in `actions`
  // (it round-trips to nothing), so it accumulates its target selections here
  // and commits to `actions` when its picker closes (or is discarded empty).
  const [draft, setDraft] = useState<ActionRow | null>(null);
  // The target picker opens as an anchored fly-out (never inline — an inline
  // checklist shoved the rest of the editor down and lost your place). One at
  // a time: which row (or the draft) plus the button it's anchored to.
  const [picker, setPicker] = useState<{ which: number | "draft"; anchor: HTMLElement } | null>(
    null,
  );
  const numInput: React.CSSProperties = { ...S.input, width: 64, padding: "0.35rem 0.5rem", minHeight: 38 };
  const conj: React.CSSProperties = { color: T.dim, fontSize: "0.8rem", whiteSpace: "nowrap" };
  const swatch: React.CSSProperties = {
    width: 48,
    height: 38,
    padding: 2,
    cursor: "pointer",
    border: `1px solid ${alpha(color.text, 0.2)}`,
    borderRadius: 6,
    background: "rgba(0,0,0,0.25)",
  };

  // Verb-first grammar: the verb decides what it can act on. On/off/power steps
  // offer rooms + devices under their room headers (the shared room-grouped
  // pattern); "apply scene" offers only scenes — scenes are a verb's object,
  // never a pseudo-device. "Power toggle" is per-device (a room has no single
  // togglable state), so it offers individual lights, media, and switches —
  // and it's the one verb that can touch a TV/speaker's power. Clauses only
  // shape the light command, so switches can share a "turn on and set
  // brightness" step — they simply turn on.
  const targetOptionsFor = (verb: ActionRow["verb"]) => {
    if (verb === "scene") {
      return scenes.map((s) => ({ value: `scene:${s.id}`, label: s.name, group: "Scenes" }));
    }
    if (verb === "toggle") {
      return deviceSelectOptions(
        [
          ...pickableLights(lights).map((l) => ({ ...l, id: `light:${l.id}` })),
          ...pickableMedia(media).map((m) => ({ ...m, id: `media:${m.id}` })),
          ...pickablePower(power).map((p) => ({ ...p, id: `power:${p.id}` })),
        ],
        rooms,
      );
    }
    return [
      ...rooms
        .filter((r) => r.enabled)
        .map((r) => ({ value: `room:${r.id}`, label: r.name, group: "Rooms" })),
      ...deviceSelectOptions(
        [
          ...pickableLights(lights).map((l) => ({ ...l, id: `light:${l.id}` })),
          ...pickablePower(power).map((p) => ({ ...p, id: `power:${p.id}` })),
        ],
        rooms,
      ),
    ];
  };
  // Summaries resolve against the superset, whatever the row's current verb.
  const labelOf = new Map(
    (["on", "toggle", "scene"] as const)
      .flatMap((v) => targetOptionsFor(v))
      .map((o) => [o.value, o.label]),
  );

  // Verb rows and app actions round-trip together — one always preserves the
  // other so editing devices never drops the TV-app steps and vice versa.
  const emit = (next: ActionRow[]) => onChange([...actionsFromRows(next), ...appActions]);
  const emitApps = (next: RuleAction[]) => onChange([...actionsFromRows(rows), ...next]);
  const setAppAction = (i: number, patch: Partial<Extract<RuleAction, { kind: "app" }>>) =>
    emitApps(appActions.map((a, j) => (j === i ? { ...a, ...patch } : a)));
  const setRow = (i: number, patch: Partial<ActionRow>) => {
    if (patch.verb && patch.verb !== rows[i].verb) {
      // A verb change keeps only the targets it can still act on (a scene step
      // can't drive lamps), and clauses belong to "turn on" alone.
      const valid = new Set(targetOptionsFor(patch.verb).map((o) => o.value));
      patch = { ...patch, targets: rows[i].targets.filter((t) => valid.has(t)) };
      if (patch.verb !== "on") patch = { ...patch, brightness: null, colorHex: null };
    }
    emit(rows.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  };
  const toggleTarget = (i: number, v: string) =>
    setRow(i, {
      targets: rows[i].targets.includes(v)
        ? rows[i].targets.filter((x) => x !== v)
        : [...rows[i].targets, v],
    });

  /** Toggle a target into the draft — accumulating, so the fly-out stays open
   * for multi-select just like an existing row's picker. */
  const draftToggle = (v: string) =>
    setDraft((d) =>
      d
        ? {
            ...d,
            targets: d.targets.includes(v)
              ? d.targets.filter((x) => x !== v)
              : [...d.targets, v],
          }
        : d,
    );

  /** Close the picker. A draft that gained targets commits to a real row; an
   * empty draft stays visible (closing the picker isn't the same as discarding
   * the action — that's the row's own ✕). Used by both the fly-out's own close
   * and the target button's toggle-closed. */
  const closePicker = () => {
    if (picker?.which === "draft" && draft && draft.targets.length > 0) {
      emit([...rows, draft]);
      setDraft(null);
    }
    setPicker(null);
  };

  const summary = (targets: string[], verb: ActionRow["verb"]) =>
    targets.length === 0
      ? verb === "scene"
        ? "Pick scenes…"
        : verb === "toggle"
          ? "Pick devices…"
          : "Pick rooms or devices…"
      : targets.length === 1
        ? (labelOf.get(targets[0]) ?? "1 target")
        : `${labelOf.get(targets[0]) ?? "…"} + ${targets.length - 1} more`;

  const targetBtn: React.CSSProperties = {
    ...S.input,
    width: 200,
    textAlign: "left",
    cursor: "pointer",
    display: "flex",
    alignItems: "center",
    gap: "0.4rem",
  };
  // While its picker is open, the target button reads as the active one.
  const targetBtnOpen: React.CSSProperties = {
    borderColor: color.cyan,
    boxShadow: `0 0 0 1px ${alpha(color.cyan, 0.4)}`,
  };
  // The summary text (ellipsised) + a chevron, so the button reads as a
  // picker (matching `Select`'s affordance) rather than a text field.
  const targetInner = (text: string, muted: boolean) => (
    <>
      <span
        style={{
          flex: 1,
          minWidth: 0,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
          color: muted ? T.faint : T.text,
        }}
      >
        {text}
      </span>
      <span style={{ color: T.dim, flexShrink: 0, fontSize: "0.7rem" }}>▾</span>
    </>
  );

  const verbSelect = (value: ActionRow["verb"], onPick: (v: ActionRow["verb"]) => void) => (
    <Select
      value={value}
      options={[
        { value: "on", label: "turn on" },
        { value: "off", label: "turn off" },
        { value: "toggle", label: "power toggle" },
        { value: "scene", label: "apply scene" },
      ]}
      onChange={(v) => onPick(v as ActionRow["verb"])}
      width={124}
    />
  );

  /** The "and…" clause chain after "turn on": each active clause reads as a
   * sentence fragment with its own quiet remove, and the trailing "and…" menu
   * chains the next one. Shared verbatim between real rows and the draft. */
  const clauseChain = (
    r: Omit<ActionRow, "targets">,
    set: (patch: Partial<Omit<ActionRow, "targets">>) => void,
  ) => {
    if (r.verb !== "on") return null;
    const clauseX = (clear: () => void) => (
      <button onClick={clear} title="Remove this clause" style={{ ...ICON_BTN, opacity: 0.45 }}>
        ✕
      </button>
    );
    return (
      <>
        {r.brightness != null && (
          <>
            <span style={conj}>and set brightness to</span>
            <input
              type="number"
              min={1}
              max={100}
              style={numInput}
              value={r.brightness}
              onChange={(e) =>
                set({ brightness: Math.max(1, Math.min(100, Number(e.target.value) || 1)) })
              }
            />
            <span style={conj}>%</span>
            {clauseX(() => set({ brightness: null }))}
          </>
        )}
        {r.colorHex != null && (
          <>
            <span style={conj}>and set color to</span>
            <input
              type="color"
              value={r.colorHex}
              onChange={(e) => set({ colorHex: e.target.value })}
              title="Pick the color"
              style={swatch}
            />
            {clauseX(() => set({ colorHex: null }))}
          </>
        )}
        {(r.brightness == null || r.colorHex == null) && (
          <Select
            options={[
              ...(r.brightness == null
                ? [{ value: "brightness", label: "set brightness to…" }]
                : []),
              ...(r.colorHex == null ? [{ value: "color", label: "set color to…" }] : []),
            ]}
            onChange={(v) => set(v === "brightness" ? { brightness: 50 } : { colorHex: "#ffb84d" })}
            placeholder="and…"
            width={86}
            title="Chain another clause onto this step"
          />
        )}
      </>
    );
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.45rem" }}>
      {[
        ...rows.map((r, i) => (
        <div key={`s:${rowSig(r)}`} style={{ display: "flex", flexDirection: "column", gap: "0.35rem" }}>
          <div style={ROW}>
            {verbSelect(r.verb, (v) => setRow(i, { verb: v }))}
            <button
              onClick={(e) =>
                picker?.which === i
                  ? closePicker()
                  : setPicker({ which: i, anchor: e.currentTarget })
              }
              title={
                r.verb === "scene"
                  ? "Choose which scenes to apply"
                  : "Choose which rooms and devices this action drives"
              }
              style={{ ...targetBtn, ...(picker?.which === i ? targetBtnOpen : {}) }}
            >
              {targetInner(summary(r.targets, r.verb), r.targets.length === 0)}
            </button>
            {clauseChain(r, (patch) => setRow(i, patch))}
            <button
              onClick={() => emit(rows.filter((_, j) => j !== i))}
              title="Remove this action"
              style={ICON_BTN}
            >
              ✕
            </button>
          </div>
        </div>
        )),
        ...(draft
          ? [
              <div
                key="s:draft"
                style={{ display: "flex", flexDirection: "column", gap: "0.35rem" }}
              >
                <div style={ROW}>
                  {verbSelect(draft.verb, (v) => {
                    // Keep only targets the new verb can still act on; clauses
                    // belong to "turn on" alone.
                    const valid = new Set(targetOptionsFor(v).map((o) => o.value));
                    setDraft({
                      ...draft,
                      verb: v,
                      targets: draft.targets.filter((t) => valid.has(t)),
                      ...(v !== "on" ? { brightness: null, colorHex: null } : {}),
                    });
                  })}
                  <button
                    onClick={(e) =>
                      picker?.which === "draft"
                        ? closePicker()
                        : setPicker({ which: "draft", anchor: e.currentTarget })
                    }
                    style={{
                      ...targetBtn,
                      ...(picker?.which === "draft" ? targetBtnOpen : {}),
                    }}
                    title="Choose what this action drives"
                  >
                    {targetInner(summary(draft.targets, draft.verb), draft.targets.length === 0)}
                  </button>
                  {clauseChain(draft, (patch) => setDraft({ ...draft, ...patch }))}
                  <button
                    onClick={() => {
                      setDraft(null);
                      setPicker(null);
                    }}
                    title="Discard this action"
                    style={ICON_BTN}
                  >
                    ✕
                  </button>
                </div>
              </div>,
            ]
          : []),
      ]}

      {appActions.map((a, i) => (
        <AppActionRow
          key={`app:${i}`}
          action={a}
          remotes={tvRemotes}
          onChange={(patch) => setAppAction(i, patch)}
          onRemove={() => emitApps(appActions.filter((_, j) => j !== i))}
        />
      ))}

      {!draft && (
        <div style={ROW}>
          <Button
            variant="ghost"
            disabled={targetOptionsFor("on").length + scenes.length === 0}
            onClick={() => setDraft({ verb: "on", targets: [], brightness: null, colorHex: null })}
            style={{ fontSize: "0.76rem", padding: "0.2rem 0.6rem" }}
          >
            + Control devices
          </Button>
          {tvRemotes.length > 0 && (
            <Button
              variant="ghost"
              onClick={() => emitApps([...appActions, { kind: "app", remote_id: "", app: "" }])}
              title="Launch a streaming app on a TV as part of this automation"
              style={{ fontSize: "0.76rem", padding: "0.2rem 0.6rem" }}
            >
              + Open a TV app
            </Button>
          )}
        </div>
      )}

      {/* The one anchored target picker — never inline. Resolves its options /
          selection / toggle from whichever row (or the draft) opened it, and
          stays open for multi-select; it commits the draft on close. */}
      {picker &&
        (() => {
          const rowIdx = picker.which === "draft" ? null : picker.which;
          const verb = rowIdx === null ? draft?.verb : rows[rowIdx]?.verb;
          if (!verb) return null;
          const selected = rowIdx === null ? (draft?.targets ?? []) : rows[rowIdx].targets;
          const onToggle =
            rowIdx === null ? draftToggle : (v: string) => toggleTarget(rowIdx, v);
          return (
            <Flyout anchor={picker.anchor} onClose={closePicker} width={240}>
              <div style={{ padding: "0.5rem 0.55rem" }}>
                <OptionCheckList
                  options={targetOptionsFor(verb)}
                  selected={selected}
                  onToggle={onToggle}
                  columns={1}
                  maxHeight={220}
                />
              </div>
            </Flyout>
          );
        })()}
    </div>
  );
}
