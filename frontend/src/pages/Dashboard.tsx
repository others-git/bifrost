import { useEffect, useRef, useState } from "react";
import { setLightState, type Light, type LightState } from "../api";
import { S } from "../styles";

interface Props {
  lights: Light[];
  onRefresh: () => void;
  onNavigate: (page: "settings") => void;
}

export function DashboardPage({ lights, onRefresh, onNavigate }: Props) {
  // Local copy so SSE events can update individual lights without a full server round-trip.
  const [localLights, setLocalLights] = useState<Light[]>(lights);

  // Keep in sync when the parent does a full refresh (authoritative server state wins).
  useEffect(() => { setLocalLights(lights); }, [lights]);

  // Real-time light state from Hue SSE → our SSE → browser.
  useEffect(() => {
    const es = new EventSource("/api/events");

    es.addEventListener("light_state", (raw) => {
      const { device_id, state } = JSON.parse((raw as MessageEvent).data) as {
        device_id: string;
        state: LightState;
      };
      setLocalLights((prev) =>
        prev.map((l) => (l.device_id === device_id ? { ...l, last_state: state } : l))
      );
    });

    // Browser reconnects automatically on error; nothing to do here.
    es.onerror = () => {};

    return () => es.close();
  }, []); // open once per mount — reconnect is handled by the browser

  function handleLocalUpdate(id: string, state: LightState) {
    setLocalLights((prev) =>
      prev.map((l) => (l.id === id ? { ...l, last_state: state } : l))
    );
  }

  return (
    <div style={{ padding: "2rem", maxWidth: 960, margin: "0 auto" }}>
      {localLights.length === 0 ? (
        <div style={{ textAlign: "center", padding: "4rem 0", color: "#666" }}>
          <p style={{ margin: "0 0 0.75rem" }}>No lights found.</p>
          <p style={{ margin: 0, fontSize: "0.875rem" }}>
            Add a provider in{" "}
            <button
              onClick={() => onNavigate("settings")}
              style={{
                background: "none",
                border: "none",
                color: "#f90",
                cursor: "pointer",
                fontSize: "0.875rem",
                padding: 0,
              }}
            >
              Settings
            </button>{" "}
            and run discovery.
          </p>
        </div>
      ) : (
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "repeat(auto-fill, minmax(220px, 1fr))",
            gap: "1rem",
          }}
        >
          {localLights.map((light) => (
            <LightCard
              key={light.id}
              light={light}
              onLocalUpdate={handleLocalUpdate}
              onChanged={onRefresh}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function LightCard({
  light,
  onLocalUpdate,
  onChanged,
}: {
  light: Light;
  onLocalUpdate: (id: string, state: LightState) => void;
  onChanged: () => void;
}) {
  const serverBrightness = light.last_state?.brightness ?? 100;
  const [localBrightness, setLocalBrightness] = useState(serverBrightness);
  const commitTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const isOn = light.last_state?.on ?? false;

  // Sync slider when a server update (refresh or SSE) changes brightness.
  useEffect(() => { setLocalBrightness(serverBrightness); }, [serverBrightness]);

  async function toggle() {
    const next: LightState = { ...(light.last_state ?? { on: false }), on: !isOn };
    onLocalUpdate(light.id, next);   // optimistic update
    await setLightState(light.id, next);
    onChanged();                      // fallback full refresh (catches Govee etc.)
  }

  function handleBrightnessChange(value: number) {
    setLocalBrightness(value);
    onLocalUpdate(light.id, { ...(light.last_state ?? { on: true }), on: true, brightness: value });
    clearTimeout(commitTimer.current);
    commitTimer.current = setTimeout(async () => {
      await setLightState(light.id, {
        ...(light.last_state ?? { on: true }),
        on: true,
        brightness: value,
      });
    }, 200);
  }

  return (
    <div style={{ ...S.card, opacity: isOn ? 1 : 0.6, transition: "opacity 0.2s" }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <span
          style={{
            fontWeight: 600,
            fontSize: "0.95rem",
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {light.name}
        </span>
        <Toggle on={isOn} onToggle={toggle} />
      </div>
      {light.capabilities.dimmable && (
        <input
          type="range"
          min={1}
          max={100}
          value={localBrightness}
          disabled={!isOn}
          onChange={(e) => handleBrightnessChange(Number(e.target.value))}
          style={{
            width: "100%",
            marginTop: "0.25rem",
            accentColor: "#f90",
            cursor: isOn ? "pointer" : "default",
          }}
        />
      )}
    </div>
  );
}

function Toggle({ on, onToggle }: { on: boolean; onToggle: () => void }) {
  return (
    <button
      onClick={onToggle}
      style={{
        flexShrink: 0,
        width: 44,
        height: 24,
        borderRadius: 12,
        border: "none",
        cursor: "pointer",
        background: on ? "#f90" : "#444",
        position: "relative",
        transition: "background 0.2s",
      }}
    >
      <span
        style={{
          position: "absolute",
          top: 3,
          left: on ? 23 : 3,
          width: 18,
          height: 18,
          borderRadius: "50%",
          background: "#fff",
          transition: "left 0.2s",
        }}
      />
    </button>
  );
}
