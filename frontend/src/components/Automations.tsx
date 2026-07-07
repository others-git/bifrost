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
  getRooms,
  getScenes,
  getSensors,
  rgbToHex,
  rgbToXy,
  runAutomation,
  updateAutomation,
  xyToRgb,
  type Automation,
  type AutomationBody,
  type AutomationTrigger,
  type Light,
  type LightState,
  type MediaDevice,
  type PowerDevice,
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
import { Modal } from "./dialogs";
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
  | { type: "device"; domain: TriggerDeviceDomain; id: string };

function subjectOf(trigger: AutomationTrigger, sensors: SensorDevice[]): TriggerSubject {
  if (trigger.kind === "room") return { type: "room", id: trigger.room_id };
  if (trigger.kind === "device")
    return { type: "device", domain: trigger.domain, id: trigger.device_id };
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
  return `sensor:${trigger.sensor_id}`;
}

/** Whether a sensor kind carries a boolean reading (vs a numeric one). */
function isBoolKind(kind: SensorDevice["kind"]): boolean {
  return kind === "motion" || kind === "occupancy" || kind === "contact" || kind === "generic";
}

/** The event phrasing, tuned per subject so the sentence reads naturally. */
function eventOptions(subject: TriggerSubject): { value: string; label: string }[] {
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
};

/** "Office to 40% (colored)" — the compact clause-aware phrasing for list rows. */
function lightActionText(name: string, state: LightState): string {
  if (!state.on) return `${name} off`;
  const clauses = [
    ...(state.brightness != null ? [`to ${state.brightness}%`] : []),
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

  useEffect(() => {
    getRooms().then(setRooms);
    getLights().then((l) => setLights(l === "unauthorized" ? [] : l));
    getMediaDevices().then(setMedia);
    getPowerDevices().then(setPower);
    getScenes().then(setScenes);
    getSensors().then(setSensors);
  }, []);

  const names: NameMaps = useMemo(
    () => ({
      room: new Map(rooms.map((r) => [r.id, r.name])),
      light: new Map(lights.map((l) => [l.id, l.name])),
      media: new Map(media.map((m) => [m.id, m.name])),
      power: new Map(power.map((p) => [p.id, p.name])),
      scene: new Map(scenes.map((s) => [s.id, s.name])),
    }),
    [rooms, lights, media, power, scenes],
  );
  const sensorName = (id: string) => sensors.find((s) => s.id === id)?.name ?? "sensor";
  return { rooms, lights, media, power, scenes, sensors, names, sensorName };
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
          {`${triggerText(rule.trigger, sensors)} → ${rule.actions.map((a) => actionText(a, names)).join(", ")}`}
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
    actions: rule.actions,
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
    <Modal title={initial ? "Edit automation" : "New automation"} onClose={onClose} width={480}>
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
        width={480}
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
  const { rooms, lights, media, power, scenes, sensors } = data;
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
    initial?.trigger.event.kind ??
      (initialSubject && initialSubject.type === "sensor" && !isBoolKind(initialSubject.kind)
        ? "rose_above"
        : "became_true"),
  );
  const [stayMins, setStayMins] = useState(
    initial?.trigger.event.kind === "clear_for" || initial?.trigger.event.kind === "held_for"
      ? Math.max(1, Math.round(initial.trigger.event.secs / 60))
      : 10,
  );
  const [threshold, setThreshold] = useState(
    initial?.trigger.event.kind === "rose_above" || initial?.trigger.event.kind === "dropped_below"
      ? initial.trigger.event.value
      : 0,
  );
  const [conditions, setConditions] = useState<RuleCondition[]>(initial?.conditions ?? []);
  const [actions, setActions] = useState<RuleAction[]>(initial?.actions ?? []);
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
    if (key.startsWith("room:")) {
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
    if (!eventOptions(next).some((o) => o.value === eventKind)) {
      setEventKind(eventOptions(next)[0].value);
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
    ...rooms
      .filter((r) => r.enabled)
      .map((r) => ({ value: `room:${r.id}`, label: r.name, group: "Rooms" })),
    ...deviceSelectOptions(
      [
        ...sensors
          .filter((s) => s.enabled !== false && !s.shadowed_by)
          .map((s) => ({
            ...s,
            id: `sensor:${s.id}`,
            // No reading = it can't fire (disabled at its bridge/hub, or has
            // never reported) — say so where the rule gets bound.
            name: s.state.reading == null ? `${s.name} — no signal` : s.name,
          })),
        ...media
          .filter((m) => m.enabled !== false && !m.shadowed_by && !m.companion_of)
          .map((m) => ({ ...m, id: `device:media:${m.id}` })),
        ...lights
          .filter((l) => l.enabled !== false)
          .map((l) => ({ ...l, id: `device:light:${l.id}` })),
        ...power
          .filter((d) => d.enabled !== false)
          .map((d) => ({ ...d, id: `device:power:${d.id}` })),
      ],
      rooms,
    ),
  ];
  const subjectValue = subject
    ? subject.type === "room"
      ? `room:${subject.id}`
      : subject.type === "device"
        ? `device:${subject.domain}:${subject.id}`
        : `sensor:${subject.id}`
    : undefined;

  // Gate subjects offered as conditions: anything except the trigger itself.
  const gateSensors = sensors.filter(
    (s) => s.enabled !== false && !(subject?.type === "sensor" && s.id === subject.id),
  );
  const gateRooms = rooms.filter(
    (r) => r.enabled && !(subject?.type === "room" && r.id === subject.id),
  );

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.7rem" }}>
      <input
        style={S.input}
        placeholder="Name (optional)"
        value={name}
        onChange={(e) => setName(e.target.value)}
      />

      <span style={SECTION_LABEL}>When</span>
      <div style={ROW}>
        <Select
          value={subjectValue}
          options={subjectOptions}
          onChange={pickSubject}
          placeholder="Pick a room or sensor"
          width={200}
          searchable
          empty="No sensors yet — add a Hue or Home Assistant provider first"
        />
        <Select
          value={eventKind}
          options={subject ? eventOptions(subject) : []}
          onChange={setEventKind}
          width={180}
          disabled={!subject}
        />
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

      <span style={SECTION_LABEL}>Only if (optional)</span>
      <ConditionList
        conditions={conditions}
        gateSensors={gateSensors}
        gateRooms={gateRooms}
        allRooms={rooms}
        onChange={setConditions}
      />

      <span style={SECTION_LABEL}>Then</span>
      <ActionList
        actions={actions}
        rooms={rooms}
        lights={lights}
        power={power}
        scenes={scenes}
        onChange={setActions}
      />

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

      {error && <span style={{ color: T.bad, fontSize: "0.8rem" }}>{error}</span>}
      <div style={{ display: "flex", gap: "0.5rem", justifyContent: "flex-end" }}>
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button
          variant="primary"
          disabled={!subject || actions.length === 0}
          onClick={() => {
            const trigger = builtTrigger();
            if (!trigger) return;
            onSave({
              name,
              enabled: initial?.enabled ?? true,
              trigger,
              conditions,
              actions,
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
  onChange,
}: {
  conditions: RuleCondition[];
  gateSensors: SensorDevice[];
  /** Rooms offered as occupancy gates (the trigger's own room excluded). */
  gateRooms: Room[];
  /** Every room — for grouping sensors under their room header. */
  allRooms: Room[];
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
  ];
  const numericGate = (id: string) => {
    const k = gateSensors.find((s) => s.id === id)?.kind;
    return k === "illuminance" || k === "temperature" || k === "humidity";
  };
  /** The stored condition for a newly picked gate subject, keeping what carries over. */
  const forGate = (key: string, prev: RuleCondition): RuleCondition => {
    if (key.startsWith("room:")) return { kind: "room_is", room_id: key.slice(5), occupied: true };
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
      <div style={ROW}>
        <Button
          variant="ghost"
          onClick={() =>
            onChange([...conditions, { kind: "time_window", start: "21:00", end: "06:00" }])
          }
        >
          + Time window
        </Button>
        {gateOptions.length > 0 && (
          <Button
            variant="ghost"
            onClick={() =>
              onChange([
                ...conditions,
                forGate(gateOptions[0].value, { kind: "time_window", start: "", end: "" }),
              ])
            }
          >
            + Condition
          </Button>
        )}
      </div>
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
  verb: "on" | "off" | "scene";
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
    if (a.kind === "scene") {
      verb = "scene";
      target = `scene:${a.scene_id}`;
    } else if (a.kind === "power") {
      verb = a.on ? "on" : "off";
      target = `power:${a.device_id}`;
    } else {
      verb = a.state.on ? "on" : "off";
      brightness = a.state.brightness ?? null;
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

function ActionList({
  actions,
  rooms,
  lights,
  power,
  scenes,
  onChange,
}: {
  actions: RuleAction[];
  rooms: Room[];
  lights: Light[];
  power: PowerDevice[];
  scenes: Scene[];
  onChange: (a: RuleAction[]) => void;
}) {
  const rows = rowsFromActions(actions);
  // A new step with no targets yet can't exist in `actions`; it lives here
  // until its first target is picked, then materializes as a real row.
  const [draft, setDraft] = useState<Omit<ActionRow, "targets"> | null>(null);
  const [openPicker, setOpenPicker] = useState<number | null>(null);
  const numInput: React.CSSProperties = { ...S.input, width: 64, padding: "0.35rem 0.5rem" };
  const conj: React.CSSProperties = { color: T.dim, fontSize: "0.8rem", whiteSpace: "nowrap" };
  const swatch: React.CSSProperties = {
    width: 40,
    height: 30,
    padding: 2,
    cursor: "pointer",
    border: `1px solid ${alpha(color.text, 0.2)}`,
    borderRadius: 6,
    background: "rgba(0,0,0,0.25)",
  };

  // Verb-first grammar: the verb decides what it can act on. Power steps offer
  // rooms + devices under their room headers (the shared room-grouped pattern);
  // "apply scene" offers only scenes — scenes are a verb's object, never a
  // pseudo-device. Clauses only shape the light command, so switches can share
  // a "turn on and set brightness" step — they simply turn on.
  const targetOptionsFor = (verb: ActionRow["verb"]) => {
    if (verb === "scene") {
      return scenes.map((s) => ({ value: `scene:${s.id}`, label: s.name, group: "Scenes" }));
    }
    return [
      ...rooms
        .filter((r) => r.enabled)
        .map((r) => ({ value: `room:${r.id}`, label: r.name, group: "Rooms" })),
      ...deviceSelectOptions(
        [
          ...lights.filter((l) => l.enabled !== false).map((l) => ({ ...l, id: `light:${l.id}` })),
          ...power.filter((p) => p.enabled !== false).map((p) => ({ ...p, id: `power:${p.id}` })),
        ],
        rooms,
      ),
    ];
  };
  // Summaries resolve against the superset, whatever the row's current verb.
  const labelOf = new Map(
    (["on", "scene"] as const).flatMap((v) => targetOptionsFor(v)).map((o) => [o.value, o.label]),
  );

  const emit = (next: ActionRow[]) => onChange(actionsFromRows(next));
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

  /** The draft's first pick creates the real row, and the picker follows it. */
  const draftPick = (v: string) => {
    if (!draft) return;
    const next = [...rows, { targets: [v], ...draft }];
    const merged = rowsFromActions(actionsFromRows(next));
    setDraft(null);
    setOpenPicker(merged.findIndex((r) => rowSig(r) === rowSig(draft)));
    emit(next);
  };

  const summary = (targets: string[], verb: ActionRow["verb"]) =>
    targets.length === 0
      ? verb === "scene"
        ? "Pick scenes…"
        : "Pick rooms or devices…"
      : targets.length === 1
        ? (labelOf.get(targets[0]) ?? "1 target")
        : `${labelOf.get(targets[0]) ?? "…"} + ${targets.length - 1} more`;

  const targetBtn: React.CSSProperties = {
    ...S.input,
    width: 200,
    textAlign: "left",
    cursor: "pointer",
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  };
  const pickerBox: React.CSSProperties = {
    border: `1px solid ${alpha(color.cyan, 0.25)}`,
    borderRadius: 8,
    padding: "0.5rem 0.6rem",
    background: "rgba(0,0,0,0.25)",
  };

  const verbSelect = (value: ActionRow["verb"], onPick: (v: ActionRow["verb"]) => void) => (
    <Select
      value={value}
      options={[
        { value: "on", label: "turn on" },
        { value: "off", label: "turn off" },
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
      {rows.map((r, i) => (
        <div key={i} style={{ display: "flex", flexDirection: "column", gap: "0.35rem" }}>
          <div style={ROW}>
            {verbSelect(r.verb, (v) => setRow(i, { verb: v }))}
            {clauseChain(r, (patch) => setRow(i, patch))}
            <button
              onClick={() => setOpenPicker(openPicker === i ? null : i)}
              title={
                r.verb === "scene"
                  ? "Choose which scenes to apply"
                  : "Choose which rooms and devices this step drives"
              }
              style={targetBtn}
            >
              {summary(r.targets, r.verb)}
            </button>
            <button
              onClick={() => emit(rows.filter((_, j) => j !== i))}
              title="Remove this step"
              style={ICON_BTN}
            >
              ✕
            </button>
          </div>
          {openPicker === i && (
            <div style={pickerBox}>
              <OptionCheckList
                options={targetOptionsFor(r.verb)}
                selected={r.targets}
                onToggle={(v) => toggleTarget(i, v)}
              />
            </div>
          )}
        </div>
      ))}

      {draft && (
        <div style={{ display: "flex", flexDirection: "column", gap: "0.35rem" }}>
          <div style={ROW}>
            {verbSelect(draft.verb, (v) =>
              setDraft({ ...draft, verb: v, ...(v !== "on" ? { brightness: null, colorHex: null } : {}) }),
            )}
            {clauseChain(draft, (patch) => setDraft({ ...draft, ...patch }))}
            <span style={{ ...targetBtn, color: T.faint }}>
              {draft.verb === "scene" ? "Pick scenes…" : "Pick rooms or devices…"}
            </span>
            <button onClick={() => setDraft(null)} title="Discard this step" style={ICON_BTN}>
              ✕
            </button>
          </div>
          <div style={pickerBox}>
            <OptionCheckList options={targetOptionsFor(draft.verb)} selected={[]} onToggle={draftPick} />
          </div>
        </div>
      )}

      {!draft && (
        <div style={ROW}>
          <Button
            variant="ghost"
            disabled={targetOptionsFor("on").length + scenes.length === 0}
            onClick={() => {
              setDraft({ verb: "on", brightness: null, colorHex: null });
              setOpenPicker(null);
            }}
          >
            + Action
          </Button>
        </div>
      )}
    </div>
  );
}
