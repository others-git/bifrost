// The "Other devices" surface on the Devices page — generic passthrough devices
// (climate, cover, lock, number, select, button, … from HA), rendered from their
// control primitives. One small widget per primitive covers the whole long tail.
// Devices are read live; a control write re-reads to reflect the new state.

import { useCallback, useEffect, useRef, useState } from "react";
import {
  getGenericDevices,
  setGenericControl,
  type GenericControl,
  type GenericDevice,
} from "../api";
import { Switch } from "./controls";
import { Select } from "./Select";
import { SectionLabel } from "./PageHeader";
import { T, ACCENT, alpha } from "../theme";

export function GenericDevicesSection() {
  const [devices, setDevices] = useState<GenericDevice[]>([]);
  const [loaded, setLoaded] = useState(false);

  const refresh = useCallback(async () => {
    setDevices(await getGenericDevices());
    setLoaded(true);
  }, []);
  useEffect(() => {
    refresh();
  }, [refresh]);

  // Hidden entirely until there's something to show (no HA / no long-tail devices).
  if (!loaded || devices.length === 0) return null;

  return (
    <section style={{ marginBottom: "2rem" }}>
      <SectionLabel style={{ fontSize: "0.95rem", color: T.text, marginBottom: "0.9rem" }}>
        Other devices
        <span style={{ color: T.faint, fontWeight: 400, letterSpacing: "0.08em" }}>
          {" "}
          · passthrough · {devices.length}
        </span>
      </SectionLabel>
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(auto-fill, minmax(300px, 1fr))",
          gap: "0.8rem",
          alignItems: "start",
        }}
      >
        {devices.map((d) => (
          <GenericCard key={`${d.provider_id}:${d.device_id}`} device={d} onChanged={refresh} />
        ))}
      </div>
    </section>
  );
}

function GenericCard({ device, onChanged }: { device: GenericDevice; onChanged: () => void }) {
  async function set(key: string, value: unknown) {
    await setGenericControl(device.provider_id, device.device_id, key, value);
    onChanged();
  }
  return (
    <div style={{ border: `1px solid ${T.cardBorder}`, borderRadius: 14, padding: "0.85rem 1rem", background: alpha(T.text, 0.02), display: "flex", flexDirection: "column", gap: "0.7rem" }}>
      <div style={{ display: "flex", alignItems: "baseline", gap: "0.5rem" }}>
        <span style={{ fontWeight: 600, color: T.text, fontSize: "0.92rem", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {device.name}
        </span>
        <span style={{ color: T.faint, fontSize: "0.66rem", letterSpacing: "0.07em", textTransform: "uppercase" }}>
          {device.kind}
        </span>
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: "0.6rem" }}>
        {device.controls.map((c) => (
          <ControlWidget key={c.key} control={c} onSet={(v) => set(c.key, v)} />
        ))}
      </div>
    </div>
  );
}

const rowStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: "0.6rem",
  fontSize: "0.82rem",
  color: T.dim,
};
const labelStyle: React.CSSProperties = { minWidth: 84, flexShrink: 0, color: T.dim };

function ControlWidget({ control, onSet }: { control: GenericControl; onSet: (v: unknown) => void }) {
  switch (control.type) {
    case "toggle":
      return (
        <div style={rowStyle}>
          <span style={labelStyle}>{control.label}</span>
          <Switch on={!!control.value} onChange={(v) => onSet(v)} />
        </div>
      );
    case "number":
      return <NumberWidget control={control} onSet={onSet} />;
    case "enum":
      return (
        <div style={rowStyle}>
          <span style={labelStyle}>{control.label}</span>
          <Select
            value={control.value != null ? String(control.value) : ""}
            onChange={(v) => onSet(v)}
            style={{ flex: 1 }}
            options={(control.options ?? []).map((o) => ({ value: o, label: o }))}
          />
        </div>
      );
    case "button":
      return (
        <button
          onClick={() => onSet(null)}
          style={{ alignSelf: "flex-start", padding: "0.35rem 0.9rem", borderRadius: 9, border: `1px solid ${alpha(ACCENT, 0.4)}`, background: alpha(ACCENT, 0.1), color: T.text, cursor: "pointer", fontSize: "0.82rem" }}
        >
          {control.label}
        </button>
      );
    case "readout":
      return (
        <div style={rowStyle}>
          <span style={labelStyle}>{control.label}</span>
          <span style={{ color: T.text }}>
            {String(control.value)}
            {control.unit ?? ""}
          </span>
        </div>
      );
  }
}

/** A range slider that shows the live drag value but only commits on release, so
 * a drag doesn't spam the device with intermediate values. */
function NumberWidget({ control, onSet }: { control: GenericControl; onSet: (v: unknown) => void }) {
  const initial = typeof control.value === "number" ? control.value : Number(control.value) || 0;
  const [val, setVal] = useState(initial);
  // Keep in sync when a refresh brings a new server value (and we're not dragging).
  const dragging = useRef(false);
  useEffect(() => {
    if (!dragging.current) setVal(initial);
  }, [initial]);

  const commit = () => {
    dragging.current = false;
    onSet(val);
  };
  return (
    <div style={rowStyle}>
      <span style={labelStyle}>{control.label}</span>
      <input
        type="range"
        min={control.min ?? 0}
        max={control.max ?? 100}
        step={control.step ?? 1}
        value={val}
        onChange={(e) => {
          dragging.current = true;
          setVal(Number(e.target.value));
        }}
        onPointerUp={commit}
        onKeyUp={commit}
        style={{ flex: 1, accentColor: ACCENT }}
      />
      <span style={{ width: 46, textAlign: "right", color: T.text, fontVariantNumeric: "tabular-nums" }}>
        {val}
        {control.unit ?? ""}
      </span>
    </div>
  );
}
