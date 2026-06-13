// Generic "Devices" inventory — the home for device classes that don't have a
// dedicated page. Today that's power devices (switches, plugs, fans, toggles)
// surfaced by integrations like Home Assistant: see them, their reachability,
// and toggle them. Lights and audio keep their own richer pages; this is the
// catch-all for everything else, and the place to eyeball what an integration
// actually imported.

import { useCallback, useEffect, useState } from "react";
import {
  getPowerDevices,
  getPowerDevice,
  setPowerState,
  type PowerDevice,
  type PowerKind,
} from "../api";
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

const KIND_LABEL: Record<PowerKind, string> = {
  switch: "Switch",
  outlet: "Outlet",
  fan: "Fan",
  toggle: "Toggle",
  generic: "Device",
};

/** A small monochrome glyph per power-device kind (currentColor stroke). */
function DeviceGlyph({ kind, size = 22 }: { kind: PowerKind; size?: number }) {
  const common = {
    width: size,
    height: size,
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.7,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
  };
  switch (kind) {
    case "outlet":
      return (
        <svg {...common}>
          <rect x="4" y="4" width="16" height="16" rx="3" />
          <line x1="10" y1="9" x2="10" y2="12" />
          <line x1="14" y1="9" x2="14" y2="12" />
          <circle cx="12" cy="15.5" r="0.6" fill="currentColor" stroke="none" />
        </svg>
      );
    case "fan":
      return (
        <svg {...common}>
          <circle cx="12" cy="12" r="1.6" />
          <path d="M12 10.4C12 7 13.5 4.5 16 5c1.6 .9 .7 4-4 5.4Z" />
          <path d="M13.6 12C17 12 19.5 13.5 19 16c-.9 1.6-4 .7-5.4-4Z" />
          <path d="M12 13.6C12 17 10.5 19.5 8 19c-1.6-.9-.7-4 4-5.4Z" />
        </svg>
      );
    case "toggle":
      return (
        <svg {...common}>
          <rect x="3" y="8" width="18" height="8" rx="4" />
          <circle cx="15" cy="12" r="2.4" fill="currentColor" stroke="none" />
        </svg>
      );
    case "switch":
      return (
        <svg {...common}>
          <rect x="6" y="3" width="12" height="18" rx="2.5" />
          <rect x="9.5" y="6.5" width="5" height="7" rx="1.2" />
        </svg>
      );
    default:
      return (
        <svg {...common}>
          <rect x="4" y="4" width="16" height="16" rx="3" />
          <circle cx="12" cy="12" r="2.2" fill="currentColor" stroke="none" />
        </svg>
      );
  }
}

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

function DeviceCard({
  device,
  onToggle,
}: {
  device: PowerDevice;
  onToggle: (next: boolean) => void;
}) {
  const offline = device.state.reachable === false;
  const on = device.state.on;
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
        opacity: offline ? 0.6 : 1,
      }}
    >
      <div
        style={{
          flexShrink: 0,
          width: 38,
          height: 38,
          borderRadius: 9,
          display: "grid",
          placeItems: "center",
          color: on ? ACCENT : T.dim,
          background: on ? "rgba(56,189,248,0.10)" : "rgba(255,255,255,0.03)",
        }}
      >
        <DeviceGlyph kind={device.kind} />
      </div>

      <div style={{ minWidth: 0, flex: 1 }}>
        <div
          style={{
            color: T.text,
            fontSize: "0.95rem",
            fontWeight: 600,
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {device.name}
        </div>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: "0.4rem",
            fontSize: "0.72rem",
            color: T.faint,
            marginTop: 2,
          }}
        >
          <span>{KIND_LABEL[device.kind]}</span>
          <span>·</span>
          <span
            title={device.device_id}
            style={{ whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}
          >
            {device.device_id}
          </span>
          {offline && (
            <>
              <span>·</span>
              <span style={{ color: T.bad }}>offline</span>
            </>
          )}
        </div>
      </div>

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
      <Toggle on={on} disabled={offline} onToggle={() => onToggle(!on)} />
    </div>
  );
}

export function DevicesPage() {
  const [devices, setDevices] = useState<PowerDevice[]>([]);
  const [loading, setLoading] = useState(true);
  const { isMobile } = useViewport();

  const refresh = useCallback(async (live: boolean) => {
    const list = await getPowerDevices();
    if (live && list.length > 0) {
      const fresh = await Promise.all(list.map((d) => getPowerDevice(d.id)));
      setDevices(fresh.flatMap((d, i) => (d ? [d] : [list[i]])));
    } else {
      setDevices(list);
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setInterval>;
    refresh(true).then(() => {
      if (cancelled) return;
      // Power devices have no push channel yet — poll for drift.
      timer = setInterval(() => refresh(true), 30000);
    });
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [refresh]);

  async function toggle(device: PowerDevice, next: boolean) {
    // Optimistic — reflect immediately, reconcile on error.
    setDevices((prev) =>
      prev.map((d) => (d.id === device.id ? { ...d, state: { ...d.state, on: next } } : d)),
    );
    const err = await setPowerState(device.id, next);
    if (err) {
      setDevices((prev) =>
        prev.map((d) => (d.id === device.id ? { ...d, state: { ...d.state, on: !next } } : d)),
      );
    }
  }

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
          onClick={() => refresh(true)}
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
        On/off devices — switches, plugs, fans — surfaced from integrations.
        Lights and audio have their own pages.
      </p>

      {loading ? (
        <div style={{ color: T.faint, fontSize: "0.9rem" }}>Loading…</div>
      ) : devices.length === 0 ? (
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
          No power devices yet. Add an integration (Settings → Add Provider →
          Integrations) and click <strong>Sync</strong> on it to import its
          switches, plugs, and fans.
        </div>
      ) : (
        <div
          style={{
            display: "grid",
            gridTemplateColumns: isMobile ? "1fr" : "repeat(auto-fill, minmax(280px, 1fr))",
            gap: "0.7rem",
          }}
        >
          {devices.map((d) => (
            <DeviceCard key={d.id} device={d} onToggle={(next) => toggle(d, next)} />
          ))}
        </div>
      )}
    </div>
  );
}
