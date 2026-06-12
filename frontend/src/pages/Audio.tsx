// Audio devices page: speakers, receivers, and zones with full media controls.
// Separate from Lights — these are their own device class. Live state arrives
// via the audio_state SSE stream (Onkyo push) with a slow poll as a fallback.

import { useEffect, useState } from "react";
import { getAudioDevices, getAudioDevice, setAudioState, type AudioDevice } from "../api";
import { AudioControls, KIND_LABEL, PowerButton } from "../components/AudioControls";
import { useViewport } from "../useViewport";

const ACCENT = "#a78bfa"; // violet — audio's counterpart to the lamps' warm glow

const T = {
  text: "#eae4d6",
  dim: "#97907e",
  faint: "#6b6557",
  panel: "linear-gradient(176deg, #1a1916 0%, #141311 100%)",
  panelBorder: "#2b2822",
  card: "#1d1c18",
  cardOff: "#171613",
  cardBorder: "#2c2922",
};

export function AudioPage() {
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [loading, setLoading] = useState(true);
  const { isMobile } = useViewport();

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;

    async function refresh(live: boolean) {
      const list = await getAudioDevices();
      if (cancelled) return;
      if (live && list.length > 0) {
        const fresh = await Promise.all(list.map((d) => getAudioDevice(d.id)));
        if (cancelled) return;
        setDevices(fresh.flatMap((d, i) => (d ? [d] : [list[i]])));
      } else {
        setDevices(list);
      }
      setLoading(false);
      timer = setTimeout(() => refresh(true), 30000);
    }

    refresh(true);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, []);

  // Receiver-initiated changes (volume knob, track changes) arrive as full
  // snapshots — replace, don't merge.
  useEffect(() => {
    const es = new EventSource("/api/events");
    es.addEventListener("audio_state", (raw) => {
      const ev = JSON.parse((raw as MessageEvent).data) as {
        provider_id: string;
        device_id: string;
        state: AudioDevice["state"];
      };
      setDevices((prev) =>
        prev.map((d) =>
          d.provider_id === ev.provider_id && d.device_id === ev.device_id
            ? { ...d, state: ev.state }
            : d,
        ),
      );
    });
    es.onerror = () => {};
    return () => es.close();
  }, []);

  function patchLocal(id: string, patch: Partial<AudioDevice["state"]>) {
    setDevices((prev) =>
      prev.map((d) => (d.id === id ? { ...d, state: { ...d.state, ...patch } } : d)),
    );
  }

  return (
    <div style={{ padding: isMobile ? "1rem 0.85rem" : "2rem", maxWidth: 1020, margin: "0 auto", color: T.text }}>
      <header style={{ marginBottom: "1.4rem" }}>
        <div style={{ display: "flex", alignItems: "baseline", gap: "0.9rem" }}>
          <h1
            style={{
              margin: 0,
              fontSize: "1rem",
              textTransform: "uppercase",
              letterSpacing: "0.22em",
              fontWeight: 700,
            }}
          >
            Audio
          </h1>
          {devices.length > 0 && (
            <span style={{ fontSize: "0.78rem", color: T.dim }}>
              {devices.length} device{devices.length !== 1 ? "s" : ""}
            </span>
          )}
        </div>
        <div
          aria-hidden
          style={{
            marginTop: "0.7rem",
            height: 1,
            background:
              "linear-gradient(90deg, rgba(167,139,250,0.6), rgba(56,189,248,0.25) 55%, transparent)",
          }}
        />
      </header>

      {loading ? (
        <p style={{ color: T.faint }}>Loading…</p>
      ) : devices.length === 0 ? (
        <div style={{ textAlign: "center", padding: "4rem 0", color: T.faint }}>
          <p style={{ margin: "0 0 0.5rem", fontSize: "1.4rem" }}>🔇</p>
          <p style={{ margin: "0 0 0.4rem" }}>No audio devices yet.</p>
          <p style={{ margin: 0, fontSize: "0.875rem" }}>
            Add a Sonos or Onkyo provider in Settings, then run Discover.
          </p>
        </div>
      ) : (
        <div
          style={{
            display: "grid",
            gridTemplateColumns: isMobile
              ? "repeat(2, minmax(0, 1fr))"
              : "repeat(auto-fill, minmax(320px, 1fr))",
            gap: isMobile ? "0.6rem" : "1rem",
          }}
        >
          {devices.map((d) => (
            <AudioDeviceCard key={d.id} device={d} onLocalPatch={patchLocal} compact={isMobile} />
          ))}
        </div>
      )}
    </div>
  );
}

function AudioDeviceCard({
  device,
  onLocalPatch,
  compact = false,
}: {
  device: AudioDevice;
  onLocalPatch: (id: string, patch: Partial<AudioDevice["state"]>) => void;
  compact?: boolean;
}) {
  const s = device.state;
  const offline = s.reachable === false;
  const playing = s.now_playing?.play_state === "playing";
  const cap = device.capabilities;

  function togglePower() {
    onLocalPatch(device.id, { power: !s.power });
    setAudioState(device.id, { power: !s.power });
  }

  return (
    <section
      style={{
        background: s.power || playing ? T.panel : T.cardOff,
        border: `1px solid ${s.power || playing ? `${ACCENT}55` : T.cardBorder}`,
        borderRadius: 14,
        overflow: "hidden",
        opacity: offline ? 0.5 : 1,
        boxShadow:
          s.power || playing ? `0 0 28px -14px ${ACCENT}` : "inset 0 1px 0 rgba(255,255,255,0.03)",
      }}
    >
      <header
        style={{
          display: "flex",
          alignItems: "center",
          gap: "0.6rem",
          padding: compact ? "0.6rem 0.7rem 0.5rem" : "0.85rem 1rem 0.6rem",
          borderBottom: `1px solid ${T.cardBorder}`,
        }}
      >
        <div style={{ minWidth: 0, flex: 1 }}>
          <div
            style={{
              fontWeight: 600,
              fontSize: compact ? "0.86rem" : "0.98rem",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {device.name}
          </div>
          <div style={{ fontSize: "0.7rem", color: T.faint, marginTop: "0.1rem" }}>
            {KIND_LABEL[device.kind] ?? device.kind}
          </div>
        </div>
        {/* Receivers/zones have a real power state; speakers don't (power=play). */}
        {cap.sources && !offline && <PowerButton on={s.power} onToggle={togglePower} />}
        {offline && (
          <span
            style={{
              fontSize: "0.7rem",
              color: "#c66",
              border: "1px solid #533",
              borderRadius: 4,
              padding: "0.1rem 0.4rem",
            }}
          >
            offline
          </span>
        )}
      </header>

      <div style={{ padding: compact ? "0.6rem 0.7rem 0.7rem" : "0.9rem 1rem 1rem" }}>
        <AudioControls device={device} onLocalPatch={onLocalPatch} compact={compact} />
      </div>
    </section>
  );
}
