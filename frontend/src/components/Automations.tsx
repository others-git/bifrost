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
  runAutomation,
  updateAutomation,
  type Automation,
  type AutomationBody,
  type AutomationTrigger,
  type Light,
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
import { Modal } from "./dialogs";
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

function actionText(a: RuleAction, names: NameMaps): string {
  switch (a.kind) {
    case "room": {
      const room = names.room.get(a.room_id) ?? "room";
      if (!a.state.on) return `${room} off`;
      return a.state.brightness != null ? `${room} to ${a.state.brightness}%` : `${room} on`;
    }
    case "light": {
      const light = names.light.get(a.light_id) ?? "light";
      if (!a.state.on) return `${light} off`;
      return a.state.brightness != null ? `${light} to ${a.state.brightness}%` : `${light} on`;
    }
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

  // One grouped subject picker: Rooms (aggregate occupancy), Sensors, then
  // device power — TVs & speakers (surfaces only), lights, switches.
  const subjectOptions = [
    ...rooms
      .filter((r) => r.enabled)
      .map((r) => ({ value: `room:${r.id}`, label: r.name, group: "Rooms" })),
    ...sensors
      .filter((s) => s.enabled !== false && !s.shadowed_by)
      .map((s) => ({ value: `sensor:${s.id}`, label: s.name, group: "Sensors" })),
    ...media
      .filter((m) => m.enabled !== false && !m.shadowed_by && !m.companion_of)
      .map((m) => ({ value: `device:media:${m.id}`, label: m.name, group: "TVs & speakers" })),
    ...lights
      .filter((l) => l.enabled !== false)
      .map((l) => ({ value: `device:light:${l.id}`, label: l.name, group: "Lights (as trigger)" })),
    ...power
      .filter((d) => d.enabled !== false)
      .map((d) => ({ value: `device:power:${d.id}`, label: d.name, group: "Switches (as trigger)" })),
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
  onChange,
}: {
  conditions: RuleCondition[];
  gateSensors: SensorDevice[];
  gateRooms: Room[];
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

  // One grouped gate picker (Rooms' occupancy / Sensors' readings).
  const gateOptions = [
    ...gateRooms.map((r) => ({ value: `room:${r.id}`, label: r.name, group: "Rooms" })),
    ...gateSensors.map((s) => ({ value: `sensor:${s.id}`, label: s.name, group: "Sensors" })),
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
  const set = (i: number, a: RuleAction) => onChange(actions.map((x, j) => (j === i ? a : x)));
  const numInput: React.CSSProperties = { ...S.input, width: 72, padding: "0.35rem 0.5rem" };

  // One grouped target picker: rooms, then lights, switches, scenes.
  const targetOptions = [
    ...rooms.filter((r) => r.enabled).map((r) => ({ value: `room:${r.id}`, label: r.name, group: "Rooms" })),
    ...lights.filter((l) => l.enabled !== false).map((l) => ({ value: `light:${l.id}`, label: l.name, group: "Lights" })),
    ...power.filter((p) => p.enabled !== false).map((p) => ({ value: `power:${p.id}`, label: p.name, group: "Switches" })),
    ...scenes.map((s) => ({ value: `scene:${s.id}`, label: s.name, group: "Scenes" })),
  ];

  const targetOf = (a: RuleAction) =>
    a.kind === "room"
      ? `room:${a.room_id}`
      : a.kind === "light"
        ? `light:${a.light_id}`
        : a.kind === "power"
          ? `power:${a.device_id}`
          : `scene:${a.scene_id}`;

  const forTarget = (v: string, prev: RuleAction): RuleAction => {
    const [kind, id] = [v.slice(0, v.indexOf(":")), v.slice(v.indexOf(":") + 1)];
    const on =
      prev.kind === "power"
        ? prev.on
        : prev.kind === "room" || prev.kind === "light"
          ? prev.state.on
          : true;
    if (kind === "room") return { kind: "room", room_id: id, state: { on } };
    if (kind === "light") return { kind: "light", light_id: id, state: { on } };
    if (kind === "power") return { kind: "power", device_id: id, on };
    return { kind: "scene", scene_id: id };
  };

  /** The on/off/brightness verb for room/light/power actions. */
  const verbOf = (a: RuleAction) =>
    a.kind === "scene"
      ? null
      : a.kind === "power"
        ? a.on
          ? "on"
          : "off"
        : a.state.brightness != null
          ? "dim"
          : a.state.on
            ? "on"
            : "off";

  const setVerb = (i: number, a: RuleAction, verb: string) => {
    if (a.kind === "power") {
      set(i, { ...a, on: verb === "on" });
      return;
    }
    if (a.kind === "room" || a.kind === "light") {
      const state = verb === "dim" ? { on: true, brightness: 50 } : { on: verb === "on" };
      set(i, { ...a, state });
    }
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.45rem" }}>
      {actions.map((a, i) => (
        <div key={i} style={ROW}>
          <Select value={targetOf(a)} options={targetOptions} onChange={(v) => set(i, forTarget(v, a))} width={180} searchable />
          {verbOf(a) && (
            <Select
              value={verbOf(a)!}
              options={[
                { value: "on", label: "turn on" },
                { value: "off", label: "turn off" },
                ...(a.kind !== "power" ? [{ value: "dim", label: "set brightness…" }] : []),
              ]}
              onChange={(v) => setVerb(i, a, v)}
              width={150}
            />
          )}
          {(a.kind === "room" || a.kind === "light") && a.state.brightness != null && (
            <>
              <input
                type="number"
                min={1}
                max={100}
                style={numInput}
                value={a.state.brightness}
                onChange={(e) =>
                  set(i, { ...a, state: { on: true, brightness: Math.max(1, Math.min(100, Number(e.target.value) || 1)) } })
                }
              />
              <span style={{ color: T.dim, fontSize: "0.8rem" }}>%</span>
            </>
          )}
          <button onClick={() => onChange(actions.filter((_, j) => j !== i))} title="Remove action" style={ICON_BTN}>
            ✕
          </button>
        </div>
      ))}
      <div style={ROW}>
        <Button
          variant="ghost"
          disabled={targetOptions.length === 0}
          onClick={() => {
            const first = rooms.find((r) => r.enabled);
            onChange([
              ...actions,
              first
                ? { kind: "room", room_id: first.id, state: { on: true } }
                : { kind: "power", device_id: power[0]?.id ?? "", on: true },
            ]);
          }}
        >
          + Action
        </Button>
      </div>
    </div>
  );
}
