import { useEffect, useRef, useState } from "react";
import {
  activateScene,
  createScene,
  getProviders,
  getRooms,
  getScenes,
  mergePatch,
  removeScene,
  rgbToXy,
  setLightState,
  setRoomState,
  type Light,
  type LightState,
  type LightStatePatch,
  type Provider,
  type Room,
  type Scene,
} from "../api";
import { S } from "../styles";

interface Props {
  lights: Light[];
  onRefresh: () => void;
  onNavigate: (page: "settings") => void;
}

export function DashboardPage({ lights, onRefresh, onNavigate }: Props) {
  // Local copy so SSE events can update individual lights without a full server round-trip.
  const [localLights, setLocalLights] = useState<Light[]>(lights);
  const [providers, setProviders] = useState<Provider[]>([]);

  // Keep in sync when the parent does a full refresh (authoritative server state wins).
  useEffect(() => { setLocalLights(lights); }, [lights]);
  useEffect(() => { getProviders().then(setProviders); }, []);

  // Real-time light state from Hue SSE → our SSE → browser.
  useEffect(() => {
    const es = new EventSource("/api/events");

    es.addEventListener("light_state", (raw) => {
      const { device_id, patch } = JSON.parse((raw as MessageEvent).data) as {
        device_id: string;
        patch: LightStatePatch;
      };
      setLocalLights((prev) =>
        prev.map((l) =>
          l.device_id === device_id ? { ...l, last_state: mergePatch(l.last_state, patch) } : l
        )
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
      {localLights.length > 0 && <SceneBar onActivated={onRefresh} />}
      {localLights.length > 0 && <RoomBar onChanged={onRefresh} />}
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
        <ProviderSections
          lights={localLights}
          providers={providers}
          onLocalUpdate={handleLocalUpdate}
          onChanged={onRefresh}
        />
      )}
    </div>
  );
}

/** Lights grouped under one section per provider. */
function ProviderSections({
  lights,
  providers,
  onLocalUpdate,
  onChanged,
}: {
  lights: Light[];
  providers: Provider[];
  onLocalUpdate: (id: string, state: LightState) => void;
  onChanged: () => void;
}) {
  const providerName = new Map(providers.map((p) => [p.id, p.name]));
  const sections = new Map<string, Light[]>();
  for (const l of lights) {
    sections.set(l.provider_id, [...(sections.get(l.provider_id) ?? []), l]);
  }
  const ordered = [...sections.entries()].sort((a, b) =>
    (providerName.get(a[0]) ?? "").localeCompare(providerName.get(b[0]) ?? ""),
  );

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "1.5rem" }}>
      {ordered.map(([providerId, sectionLights]) => (
        <section key={providerId}>
          <h2
            style={{
              margin: "0 0 0.6rem",
              fontSize: "0.8rem",
              fontWeight: 600,
              color: "#777",
              textTransform: "uppercase",
              letterSpacing: "0.08em",
            }}
          >
            {providerName.get(providerId) ?? "Other"}
            <span style={{ color: "#555", marginLeft: "0.5rem", textTransform: "none", letterSpacing: 0 }}>
              {sectionLights.length} light{sectionLights.length !== 1 ? "s" : ""}
            </span>
          </h2>
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fill, minmax(220px, 1fr))",
              gap: "1rem",
            }}
          >
            {sectionLights.map((light) => (
              <LightCard
                key={light.id}
                light={light}
                onLocalUpdate={onLocalUpdate}
                onChanged={onChanged}
              />
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}

function SceneBar({ onActivated }: { onActivated: () => void }) {
  const [scenes, setScenes] = useState<Scene[]>([]);
  const [busy, setBusy] = useState("");

  async function load() {
    setScenes(await getScenes());
  }

  useEffect(() => { load(); }, []);

  async function handleActivate(id: string) {
    setBusy(id);
    try {
      await activateScene(id);
      onActivated(); // refresh light states from the server
    } finally {
      setBusy("");
    }
  }

  async function handleSave() {
    const name = window.prompt("Scene name (saves the current state of all lights):");
    if (!name?.trim()) return;
    await createScene(name.trim());
    await load();
  }

  async function handleRemove(id: string, name: string) {
    if (!window.confirm(`Delete scene "${name}"?`)) return;
    await removeScene(id);
    await load();
  }

  return (
    <div style={{ display: "flex", flexWrap: "wrap", gap: "0.5rem", alignItems: "center", marginBottom: "1.25rem" }}>
      {scenes.map((s) => (
        <span key={s.id} style={{ display: "inline-flex" }}>
          <button
            onClick={() => handleActivate(s.id)}
            disabled={busy === s.id}
            title={`Apply "${s.name}" (${s.lights} light${s.lights !== 1 ? "s" : ""})`}
            style={{ ...S.buttonGhost, borderRadius: "6px 0 0 6px" }}
          >
            {busy === s.id ? "…" : s.name}
          </button>
          <button
            onClick={() => handleRemove(s.id, s.name)}
            title="Delete scene"
            style={{ ...S.buttonGhost, borderRadius: "0 6px 6px 0", borderLeft: "none", padding: "0.45rem 0.55rem", color: "#866" }}
          >
            ×
          </button>
        </span>
      ))}
      <button onClick={handleSave} style={S.buttonGhost} title="Save the current light states as a scene">
        + Save scene
      </button>
    </div>
  );
}

function RoomBar({ onChanged }: { onChanged: () => void }) {
  const [rooms, setRooms] = useState<Room[]>([]);
  const [busy, setBusy] = useState("");

  useEffect(() => {
    getRooms().then(setRooms);
  }, []);

  async function setAll(id: string, on: boolean) {
    setBusy(id);
    try {
      await setRoomState(id, { on });
      onChanged();
    } finally {
      setBusy("");
    }
  }

  if (rooms.length === 0) return null;

  return (
    <div style={{ display: "flex", flexWrap: "wrap", gap: "0.5rem", alignItems: "center", marginBottom: "1.25rem" }}>
      {rooms.map((r) => (
        <span
          key={r.id}
          style={{ display: "inline-flex", alignItems: "center", gap: "0.4rem", border: "1px solid #333", borderRadius: 6, padding: "0.3rem 0.3rem 0.3rem 0.7rem" }}
        >
          <span style={{ fontSize: "0.85rem", color: "#ccc" }}>{r.name}</span>
          <button
            onClick={() => setAll(r.id, true)}
            disabled={busy === r.id || r.light_ids.length === 0}
            style={{ ...S.buttonGhost, padding: "0.25rem 0.55rem", fontSize: "0.75rem" }}
          >
            On
          </button>
          <button
            onClick={() => setAll(r.id, false)}
            disabled={busy === r.id || r.light_ids.length === 0}
            style={{ ...S.buttonGhost, padding: "0.25rem 0.55rem", fontSize: "0.75rem" }}
          >
            Off
          </button>
        </span>
      ))}
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
  const [localHex, setLocalHex] = useState("#ffb84d");
  const commitTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const colorTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const isOn = light.last_state?.on ?? false;

  // Sync slider when a server update (refresh or SSE) changes brightness.
  useEffect(() => { setLocalBrightness(serverBrightness); }, [serverBrightness]);

  function handleColorChange(hex: string) {
    setLocalHex(hex);
    const r = parseInt(hex.slice(1, 3), 16);
    const g = parseInt(hex.slice(3, 5), 16);
    const b = parseInt(hex.slice(5, 7), 16);
    const color = rgbToXy(r, g, b);
    const next: LightState = { ...(light.last_state ?? { on: true }), on: true, color };
    onLocalUpdate(light.id, next);
    clearTimeout(colorTimer.current);
    colorTimer.current = setTimeout(() => { setLightState(light.id, next); }, 200);
  }

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
      {light.capabilities.color_rgb && (
        <div style={{ display: "flex", alignItems: "center", gap: "0.5rem", marginTop: "0.4rem" }}>
          <input
            type="color"
            value={localHex}
            disabled={!isOn}
            onChange={(e) => handleColorChange(e.target.value)}
            style={{
              width: 36,
              height: 24,
              padding: 0,
              border: "1px solid #444",
              borderRadius: 4,
              background: "none",
              cursor: isOn ? "pointer" : "default",
            }}
          />
          <span style={{ fontSize: "0.75rem", color: "#888" }}>Color</span>
        </div>
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
