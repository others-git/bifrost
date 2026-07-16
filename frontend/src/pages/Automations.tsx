// The Automations page — every automation in one place, grouped by its
// trigger subject (a room's occupancy or a sensor). "New automation" opens the
// sentence-builder modal directly; a group's + pre-fills its subject. Rows,
// editor, and data all come from the shared components/Automations.tsx
// primitives; this page only arranges them.

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  deleteAutomation,
  getAutomations,
  runAutomation,
  updateAutomation,
  type Automation,
} from "../api";
import {
  AutomationEditorModal,
  AutomationRow,
  duplicateAutomation,
  subjectKey,
  useAutomationData,
  type TriggerSubject,
} from "../components/Automations";
import { Button } from "../components/controls";
import { Glyph } from "../components/glyphs";
import { PageHeader } from "../components/PageHeader";
import { useViewport } from "../useViewport";
import { pageShell, tileGrid } from "../styles";
import { T, labelType } from "../theme";

export function AutomationsPage() {
  const { isMobile, isCompact } = useViewport();
  const [automations, setAutomations] = useState<Automation[]>([]);
  const data = useAutomationData();
  const { rooms, sensors, names, sensorName } = data;
  // The editor modal: null = closed; otherwise the automation being edited
  // (null inside = new) and an optional pre-selected trigger subject.
  const [editor, setEditor] = useState<{
    initial: Automation | null;
    subject?: TriggerSubject;
  } | null>(null);

  const reload = useCallback(() => {
    getAutomations().then(setAutomations);
  }, []);
  useEffect(reload, [reload]);

  // Group by trigger subject, in subject-name order.
  const subjectName = useCallback(
    (key: string) => {
      if (key.startsWith("room:")) return names.room.get(key.slice(5)) ?? "room";
      if (key.startsWith("device:")) {
        const rest = key.slice(7);
        const sep = rest.indexOf(":");
        const map = { light: names.light, media: names.media, power: names.power }[
          rest.slice(0, sep) as "light" | "media" | "power"
        ];
        return map?.get(rest.slice(sep + 1)) ?? "device";
      }
      return sensorName(key.slice(7));
    },
    [names, sensorName],
  );
  const groups = useMemo(() => {
    const by = new Map<string, Automation[]>();
    for (const a of automations) {
      const key = subjectKey(a.trigger);
      by.set(key, [...(by.get(key) ?? []), a]);
    }
    return [...by.entries()].sort((x, y) => subjectName(x[0]).localeCompare(subjectName(y[0])));
  }, [automations, subjectName]);

  const subjectFor = (key: string): TriggerSubject => {
    if (key.startsWith("room:")) return { type: "room", id: key.slice(5) };
    if (key.startsWith("device:")) {
      const rest = key.slice(7);
      const sep = rest.indexOf(":");
      return {
        type: "device",
        domain: rest.slice(0, sep) as "light" | "media" | "power",
        id: rest.slice(sep + 1),
      };
    }
    return {
      type: "sensor",
      id: key.slice(7),
      kind: sensors.find((s) => s.id === key.slice(7))?.kind ?? "generic",
    };
  };

  const upsert = (rule: Automation) =>
    setAutomations((all) =>
      all.some((x) => x.id === rule.id) ? all.map((x) => (x.id === rule.id ? rule : x)) : [...all, rule],
    );

  async function toggle(a: Automation) {
    const result = await updateAutomation(a.id, { ...a, enabled: !a.enabled });
    if (typeof result !== "string") upsert(result);
  }

  async function remove(a: Automation) {
    await deleteAutomation(a.id);
    setAutomations((all) => all.filter((x) => x.id !== a.id));
  }

  const { lights, media, power } = data;
  const hasSubjects =
    rooms.some((r) => r.enabled) ||
    sensors.some((s) => s.enabled !== false && !s.shadowed_by) ||
    lights.length + media.length + power.length > 0;

  return (
    <div style={{ ...pageShell(isMobile), ...(isCompact && !isMobile ? { padding: "1.1rem 1rem" } : {}), color: T.text }}>
      <PageHeader
        title="Automations"
        status={automations.length > 0 ? `${automations.length} rule${automations.length !== 1 ? "s" : ""}` : undefined}
      />

      {automations.length === 0 && (
        <div style={{ textAlign: "center", padding: "3rem 1rem", color: T.faint }}>
          <div style={{ marginBottom: "0.6rem", color: T.dim }}>
            <Glyph name="bolt" size={30} />
          </div>
          <p style={{ margin: "0 0 0.4rem" }}>No automations yet.</p>
          <p style={{ margin: 0, fontSize: "0.85rem" }}>
            An automation runs actions when a room or sensor changes — motion turns
            a room on, fifteen empty minutes turn it off, a dark room brightens.
          </p>
        </div>
      )}

      {/* Subject groups tile across the width; each group's rules stay stacked. */}
      <div style={tileGrid(420, isMobile, "1.2rem 1.6rem")}>
        {groups.map(([key, rules]) => (
          <section key={key} style={{ minWidth: 0 }}>
            <div style={{ display: "flex", alignItems: "center", gap: "0.6rem", marginBottom: "0.5rem" }}>
              <span style={{ ...labelType, fontSize: "0.78rem", color: "#d8cfba", flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                {subjectName(key)}
              </span>
              <Button
                variant="ghost"
                onClick={() => setEditor({ initial: null, subject: subjectFor(key) })}
                title={`New automation on ${subjectName(key)}`}
              >
                + Add
              </Button>
            </div>
            <div style={{ display: "flex", flexDirection: "column", gap: "0.45rem" }}>
              {rules.map((rule) => (
                <AutomationRow
                  key={rule.id}
                  rule={rule}
                  data={data}
                  onToggle={() => toggle(rule)}
                  onEdit={() => setEditor({ initial: rule })}
                  onDelete={() => remove(rule)}
                  onRun={async () => {
                    await runAutomation(rule.id);
                    reload(); // pick up the fresh last-ran stamp
                  }}
                  onDuplicate={async () => {
                    const copy = await duplicateAutomation(rule);
                    if (copy) upsert(copy);
                  }}
                />
              ))}
            </div>
          </section>
        ))}
      </div>

      <div style={{ marginTop: "1.4rem" }}>
        <Button
          variant="accent"
          onClick={() => setEditor({ initial: null })}
          disabled={!hasSubjects}
          title={hasSubjects ? undefined : "Add a provider with sensors first"}
        >
          New automation
        </Button>
      </div>

      {editor && (
        <AutomationEditorModal
          initial={editor.initial}
          initialSubject={editor.subject}
          data={data}
          onSaved={(rule) => {
            upsert(rule);
            setEditor(null);
          }}
          onClose={() => setEditor(null)}
        />
      )}
    </div>
  );
}
