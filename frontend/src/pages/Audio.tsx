// Audio devices page: speakers, receivers, and zones with full media controls.
// Separate from Lights — these are their own device class. Live state arrives
// via the audio_state SSE stream (Onkyo push) with a slow poll as a fallback.

import { useEffect, useRef, useState } from "react";
import {
  getAudioDevices,
  getAudioDevice,
  setAudioState,
  type AudioCommand,
  type AudioDevice,
} from "../api";

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

const KIND_LABEL: Record<string, string> = {
  receiver: "Receiver",
  speaker: "Speaker",
  zone: "Zone",
};

export function AudioPage() {
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [loading, setLoading] = useState(true);

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
    <div style={{ padding: "2rem", maxWidth: 1020, margin: "0 auto", color: T.text }}>
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
            gridTemplateColumns: "repeat(auto-fill, minmax(320px, 1fr))",
            gap: "1rem",
          }}
        >
          {devices.map((d) => (
            <AudioDeviceCard key={d.id} device={d} onLocalPatch={patchLocal} />
          ))}
        </div>
      )}
    </div>
  );
}

function AudioDeviceCard({
  device,
  onLocalPatch,
}: {
  device: AudioDevice;
  onLocalPatch: (id: string, patch: Partial<AudioDevice["state"]>) => void;
}) {
  const volumeTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const s = device.state;
  const offline = s.reachable === false;
  const np = s.now_playing;
  const cap = device.capabilities;

  async function send(cmd: AudioCommand) {
    const err = await setAudioState(device.id, cmd);
    if (err) console.warn("audio command failed:", err);
  }

  function setVolume(v: number) {
    onLocalPatch(device.id, { volume: v });
    clearTimeout(volumeTimer.current);
    volumeTimer.current = setTimeout(() => send({ volume: v }), 250);
  }
  function toggleMute() {
    onLocalPatch(device.id, { mute: !s.mute });
    send({ mute: !s.mute });
  }
  function togglePower() {
    onLocalPatch(device.id, { power: !s.power });
    send({ power: !s.power });
  }

  const playing = np?.play_state === "playing";
  const trackTitle = np?.title;
  const trackSub = [np?.artist, np?.album].filter(Boolean).join(" · ");
  // What "now playing" reads as when there's no track metadata.
  const idleLine = offline
    ? "Offline"
    : !s.power && cap.sources
      ? "Standby"
      : np?.play_state === "stopped"
        ? "Stopped"
        : s.source
          ? `Source · ${s.source}`
          : "Idle";

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
          padding: "0.85rem 1rem 0.6rem",
          borderBottom: `1px solid ${T.cardBorder}`,
        }}
      >
        <div style={{ minWidth: 0, flex: 1 }}>
          <div
            style={{
              fontWeight: 600,
              fontSize: "0.98rem",
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

      <div style={{ padding: "0.9rem 1rem 1rem", display: "flex", flexDirection: "column", gap: "0.85rem" }}>
        {/* Now playing */}
        <div style={{ minHeight: "2.4rem" }}>
          {trackTitle ? (
            <>
              <div
                style={{
                  fontSize: "0.92rem",
                  color: T.text,
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}
              >
                {trackTitle}
              </div>
              {trackSub && (
                <div
                  style={{
                    fontSize: "0.78rem",
                    color: T.dim,
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                >
                  {trackSub}
                </div>
              )}
            </>
          ) : (
            <div style={{ fontSize: "0.85rem", color: T.dim }}>{idleLine}</div>
          )}
        </div>

        {/* Transport */}
        {cap.transport && !offline && (
          <div style={{ display: "flex", justifyContent: "center", gap: "0.6rem", alignItems: "center" }}>
            <TransportButton glyph="⏮" title="Previous" onClick={() => send({ transport: "previous" })} />
            <TransportButton
              glyph={playing ? "⏸" : "▶"}
              title="Play / pause"
              big
              onClick={() => send({ transport: "toggle" })}
            />
            <TransportButton glyph="⏭" title="Next" onClick={() => send({ transport: "next" })} />
          </div>
        )}

        {/* Volume */}
        {!offline && (
          <div style={{ display: "flex", alignItems: "center", gap: "0.6rem" }}>
            <button
              onClick={toggleMute}
              title={s.mute ? "Unmute" : "Mute"}
              style={{
                background: "none",
                border: "none",
                cursor: "pointer",
                fontSize: "1rem",
                padding: 0,
                opacity: s.mute ? 1 : 0.6,
              }}
            >
              {s.mute ? "🔇" : "🔊"}
            </button>
            <input
              type="range"
              min={0}
              max={100}
              value={s.volume}
              onChange={(e) => setVolume(Number(e.target.value))}
              style={{ flex: 1, accentColor: ACCENT }}
            />
            <span style={{ fontSize: "0.78rem", color: T.dim, width: 30, textAlign: "right" }}>
              {s.volume}
            </span>
          </div>
        )}
      </div>
    </section>
  );
}

function TransportButton({
  glyph,
  title,
  onClick,
  big,
}: {
  glyph: string;
  title: string;
  onClick: () => void;
  big?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      title={title}
      style={{
        width: big ? 48 : 40,
        height: big ? 48 : 40,
        borderRadius: "50%",
        border: `1px solid ${big ? ACCENT : T.cardBorder}`,
        background: big ? `${ACCENT}22` : "rgba(255,255,255,0.04)",
        color: big ? "#fff" : T.dim,
        cursor: "pointer",
        fontSize: big ? "1.2rem" : "0.95rem",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
      }}
    >
      {glyph}
    </button>
  );
}

/** Power for receivers/zones: a glassy electric pill, matching the light toggles. */
function PowerButton({ on, onToggle }: { on: boolean; onToggle: () => void }) {
  return (
    <button
      onClick={onToggle}
      aria-label={on ? "Turn off" : "Turn on"}
      title={on ? "Power off" : "Power on"}
      style={{
        flexShrink: 0,
        width: 44,
        height: 24,
        borderRadius: 12,
        border: `1px solid ${on ? "rgba(167,139,250,0.6)" : "rgba(255,255,255,0.12)"}`,
        cursor: "pointer",
        background: on
          ? "linear-gradient(90deg, rgba(167,139,250,0.5), rgba(56,189,248,0.12) 70%), rgba(20,16,30,0.55)"
          : "rgba(255,255,255,0.06)",
        boxShadow: on ? `0 0 14px -4px ${ACCENT}` : "inset 0 1px 0 rgba(255,255,255,0.06)",
        position: "relative",
        transition: "background 0.2s, box-shadow 0.2s, border-color 0.2s",
      }}
    >
      <span
        style={{
          position: "absolute",
          top: 2,
          left: on ? 22 : 2,
          width: 18,
          height: 18,
          borderRadius: "50%",
          background: on ? "linear-gradient(180deg, #ffffff, #e6dbff)" : "rgba(255,255,255,0.4)",
          boxShadow: on ? `0 0 8px ${ACCENT}` : "0 1px 2px rgba(0,0,0,0.35)",
          transition: "left 0.2s",
        }}
      />
    </button>
  );
}
