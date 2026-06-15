// Shared audio controls — the central control surface for one audio device,
// reused by the Audio page cards and the floor-plan fly-out so the two never
// drift (per CLAUDE.md). Renders now-playing, transport, volume/mute, and
// favorites; power lives in the surrounding chrome (card header / fly-out title)
// via the exported PowerButton.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  getAudioFavorites,
  getRemoteDevices,
  playAudioFavorite,
  setAudioState,
  type AudioCommand,
  type AudioDevice,
  type AudioFavorite,
  type RemoteDevice,
} from "../api";
import { DisableRow } from "./PowerFlyout";
import { BifrostRemote } from "./BifrostRemote";
import { useViewport } from "../useViewport";
import { sheetStyle } from "./sheet";

const ACCENT = "#a78bfa"; // violet — audio's accent
const T = { text: "#eae4d6", dim: "#97907e", faint: "#6b6557", cardBorder: "#2c2922" };

export const KIND_LABEL: Record<string, string> = {
  receiver: "Receiver",
  speaker: "Speaker",
  zone: "Zone",
};

/** The control body for one audio device. State is owned by the parent (live
 * via SSE/poll) and updated optimistically through `onLocalPatch`. */
export function AudioControls({
  device,
  onLocalPatch,
  compact = false,
  receiverName,
}: {
  device: AudioDevice;
  onLocalPatch: (id: string, patch: Partial<AudioDevice["state"]>) => void;
  /** Tighter spacing + smaller transport buttons, for cramped cards (phones). */
  compact?: boolean;
  /** When this source is bound to a receiver (M22), the receiver's name — shown
   * by the volume row, since volume/mute control the receiver, not this device. */
  receiverName?: string;
}) {
  const volumeTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const s = device.state;
  const offline = s.reachable === false;
  const np = s.now_playing;
  const cap = device.capabilities;

  const [favOpen, setFavOpen] = useState(false);
  const [favs, setFavs] = useState<AudioFavorite[] | null>(null);
  const [favBusy, setFavBusy] = useState(false);

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
  async function toggleFavorites() {
    const next = !favOpen;
    setFavOpen(next);
    if (next && favs === null) {
      setFavBusy(true);
      setFavs(await getAudioFavorites(device.id));
      setFavBusy(false);
    }
  }
  async function playFav(f: AudioFavorite) {
    onLocalPatch(device.id, { power: true });
    const err = await playAudioFavorite(device.id, f.id);
    if (err) console.warn("play favorite failed:", err);
  }

  const playing = np?.play_state === "playing";
  const trackTitle = np?.title;
  const trackSub = [np?.artist, np?.album].filter(Boolean).join(" · ");
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
    <div style={{ display: "flex", flexDirection: "column", gap: compact ? "0.55rem" : "0.85rem" }}>
      {/* Now playing */}
      <div style={{ minHeight: compact ? "1.5rem" : "2.4rem" }}>
        {trackTitle ? (
          <>
            <div style={{ fontSize: "0.92rem", color: T.text, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {trackTitle}
            </div>
            {trackSub && (
              <div style={{ fontSize: "0.78rem", color: T.dim, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
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
        <div style={{ display: "flex", justifyContent: "center", gap: compact ? "0.4rem" : "0.6rem", alignItems: "center" }}>
          <TransportButton glyph="⏮" title="Previous" compact={compact} onClick={() => send({ transport: "previous" })} />
          <TransportButton glyph={playing ? "⏸" : "▶"} title="Play / pause" big compact={compact} onClick={() => send({ transport: "toggle" })} />
          <TransportButton glyph="⏭" title="Next" compact={compact} onClick={() => send({ transport: "next" })} />
        </div>
      )}

      {/* Volume */}
      {!offline && (
        <div style={{ display: "flex", alignItems: "center", gap: "0.6rem" }}>
          <button
            onClick={toggleMute}
            title={s.mute ? "Unmute" : "Mute"}
            style={{ background: "none", border: "none", cursor: "pointer", fontSize: "1rem", padding: 0, opacity: s.mute ? 1 : 0.6 }}
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
          <span style={{ fontSize: "0.78rem", color: T.dim, width: 30, textAlign: "right" }}>{s.volume}</span>
        </div>
      )}
      {!offline && receiverName && (
        <div style={{ fontSize: "0.7rem", color: T.dim, marginTop: -2 }}>
          Volume → {receiverName}
        </div>
      )}

      {/* Source / app picker — receiver inputs, or a smart TV's apps. */}
      {cap.sources && !offline && s.source_list && s.source_list.length > 0 && (
        <div style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
          <span style={{ fontSize: "0.74rem", color: T.dim, flexShrink: 0 }}>Source</span>
          <select
            value={s.source ?? ""}
            onChange={(e) => send({ source: e.target.value })}
            title="Switch input / app"
            style={{ flex: 1, minWidth: 0, background: "rgba(255,255,255,0.04)", color: T.text, border: `1px solid ${T.cardBorder}`, borderRadius: 8, padding: "0.35rem 0.5rem", fontSize: "0.82rem", cursor: "pointer" }}
          >
            {!s.source && <option value="" disabled>Select…</option>}
            {/* The current source can be outside the list (unknown/legacy) — keep it visible. */}
            {s.source && !s.source_list.includes(s.source) && <option value={s.source}>{s.source}</option>}
            {s.source_list.map((src) => (
              <option key={src} value={src}>{src}</option>
            ))}
          </select>
        </div>
      )}

      {/* Favorites */}
      {cap.favorites && !offline && (
        <div style={{ borderTop: `1px solid ${T.cardBorder}`, paddingTop: "0.6rem" }}>
          <button
            onClick={toggleFavorites}
            style={{ background: "none", border: "none", color: T.dim, cursor: "pointer", fontSize: "0.74rem", letterSpacing: "0.08em", textTransform: "uppercase", padding: 0, display: "flex", alignItems: "center", gap: "0.35rem" }}
          >
            ♥ Favorites <span style={{ fontSize: "0.6rem" }}>{favOpen ? "▲" : "▼"}</span>
          </button>
          {favOpen && (
            <div style={{ marginTop: "0.5rem", display: "flex", flexDirection: "column", gap: "0.25rem" }}>
              {favBusy ? (
                <div style={{ fontSize: "0.8rem", color: T.faint }}>Loading…</div>
              ) : favs && favs.length > 0 ? (
                favs.map((f) => (
                  <button
                    key={f.id}
                    onClick={() => playFav(f)}
                    title={`Play "${f.title}"`}
                    style={{ textAlign: "left", background: "rgba(255,255,255,0.03)", border: `1px solid ${T.cardBorder}`, borderRadius: 8, color: T.text, cursor: "pointer", padding: "0.4rem 0.6rem", display: "flex", alignItems: "center", gap: "0.5rem" }}
                  >
                    <span style={{ color: ACCENT, fontSize: "0.8rem" }}>▶</span>
                    <span style={{ minWidth: 0, flex: 1 }}>
                      <span style={{ display: "block", fontSize: "0.85rem", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                        {f.title}
                      </span>
                      {f.subtitle && <span style={{ display: "block", fontSize: "0.72rem", color: T.faint }}>{f.subtitle}</span>}
                    </span>
                  </button>
                ))
              ) : (
                <div style={{ fontSize: "0.8rem", color: T.faint }}>No favorites saved. Add some in the Sonos app.</div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export function TransportButton({
  glyph,
  title,
  onClick,
  big,
  compact,
}: {
  glyph: string;
  title: string;
  onClick: () => void;
  big?: boolean;
  compact?: boolean;
}) {
  const size = big ? (compact ? 38 : 48) : compact ? 32 : 40;
  return (
    <button
      onClick={onClick}
      title={title}
      style={{
        width: size,
        height: size,
        borderRadius: "50%",
        border: `1px solid ${big ? ACCENT : T.cardBorder}`,
        background: big ? `${ACCENT}22` : "rgba(255,255,255,0.04)",
        color: big ? "#fff" : T.dim,
        cursor: "pointer",
        fontSize: big ? (compact ? "1rem" : "1.2rem") : compact ? "0.82rem" : "0.95rem",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        flexShrink: 0,
      }}
    >
      {glyph}
    </button>
  );
}

/** Power for receivers/zones: a glassy electric pill, matching the light toggles. */
export function PowerButton({ on, onToggle }: { on: boolean; onToggle: () => void }) {
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

/**
 * Anchored fly-out wrapping AudioControls — the floor-plan counterpart to
 * LightEditor. State is owned by the caller; `onLocalPatch` updates it
 * optimistically and `setAudioState` drives the device.
 */
export function AudioEditor({
  device,
  anchor,
  onLocalPatch,
  onSetEnabled,
  onClose,
  receiverName,
}: {
  device: AudioDevice;
  anchor: HTMLElement | { x: number; y: number };
  onLocalPatch: (id: string, patch: Partial<AudioDevice["state"]>) => void;
  /** Enable/disable the device. Disabling drops it from room control. */
  onSetEnabled?: (enabled: boolean) => void;
  onClose: () => void;
  /** M22: name of the receiver this source's volume routes to, if bound. */
  receiverName?: string;
}) {
  const { isCompact } = useViewport();
  const panelRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const cap = device.capabilities;
  const offline = device.state.reachable === false;

  // A TV may have a paired remote (M24) — surface a button into `BifrostRemote`.
  const [pairedRemote, setPairedRemote] = useState<RemoteDevice | null>(null);
  const [remoteOpen, setRemoteOpen] = useState(false);
  const remoteOpenRef = useRef(false);
  remoteOpenRef.current = remoteOpen;

  useEffect(() => {
    if (device.kind !== "tv") return;
    let alive = true;
    getRemoteDevices().then((rs) => {
      if (alive) setPairedRemote(rs.find((r) => r.paired_audio_id === device.id && r.enabled) ?? null);
    });
    return () => {
      alive = false;
    };
  }, [device.kind, device.id]);

  useLayoutEffect(() => {
    if (isCompact) return; // bottom sheet on phones — no anchor math
    const panel = panelRef.current;
    if (!panel) return;
    const rect =
      anchor instanceof HTMLElement ? anchor.getBoundingClientRect() : new DOMRect(anchor.x, anchor.y, 0, 0);
    const w = panel.offsetWidth;
    const h = panel.offsetHeight;
    let left = rect.right + 12;
    if (left + w > window.innerWidth - 8) left = rect.left - 12 - w;
    left = Math.max(8, Math.min(window.innerWidth - w - 8, left));
    let top = rect.top + rect.height / 2 - h / 2;
    top = Math.max(8, Math.min(window.innerHeight - h - 8, top));
    setPos({ left, top });
  }, [anchor, isCompact]);

  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      if (remoteOpenRef.current) return; // the remote overlay owns clicks while open
      if (panelRef.current && !panelRef.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !remoteOpenRef.current) onClose();
    };
    const t = setTimeout(() => document.addEventListener("pointerdown", onDown), 0);
    window.addEventListener("keydown", onKey);
    return () => {
      clearTimeout(t);
      document.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  function togglePower() {
    onLocalPatch(device.id, { power: !device.state.power });
    setAudioState(device.id, { power: !device.state.power });
  }

  return createPortal(
    <div
      ref={panelRef}
      style={
        isCompact
          ? sheetStyle
          : {
              position: "fixed",
              left: pos?.left ?? 0,
              top: pos?.top ?? 0,
              visibility: pos ? "visible" : "hidden",
              zIndex: 60,
              width: 260,
              background: "#1c1c20",
              border: "1px solid #333",
              borderRadius: 14,
              padding: "0.9rem",
              boxShadow: "0 8px 30px rgba(0,0,0,0.6)",
              display: "flex",
              flexDirection: "column",
              gap: "0.7rem",
            }
      }
    >
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: "0.8rem" }}>
        <div style={{ minWidth: 0 }}>
          <div style={{ fontWeight: 600, fontSize: "0.9rem", color: "#eee", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {device.name}
          </div>
          <div style={{ fontSize: "0.7rem", color: T.faint }}>{KIND_LABEL[device.kind] ?? device.kind}</div>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
          {cap.sources && !offline && <PowerButton on={device.state.power} onToggle={togglePower} />}
          <button
            onClick={onClose}
            aria-label="Close"
            style={{ background: "none", border: "none", color: "#777", cursor: "pointer", fontSize: "1.15rem", lineHeight: 1, padding: 0 }}
          >
            ×
          </button>
        </div>
      </div>
      {pairedRemote && !offline && (
        <button
          onClick={() => setRemoteOpen(true)}
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            gap: "0.5rem",
            width: "100%",
            padding: "0.55rem",
            borderRadius: 10,
            border: `1px solid ${ACCENT}55`,
            background: `${ACCENT}14`,
            color: T.text,
            cursor: "pointer",
            fontSize: "0.85rem",
          }}
        >
          <span style={{ fontSize: "1rem" }}>📺</span> Remote
        </button>
      )}
      {offline ? (
        <div style={{ fontSize: "0.8rem", color: "#c66" }}>Device offline.</div>
      ) : (
        <AudioControls device={device} onLocalPatch={onLocalPatch} receiverName={receiverName} />
      )}
      {remoteOpen && pairedRemote && (
        <BifrostRemote
          remoteId={pairedRemote.id}
          name={device.name}
          initialOn={device.state.power}
          onClose={() => setRemoteOpen(false)}
        />
      )}
      {onSetEnabled && (
        <DisableRow
          enabled={device.enabled !== false}
          onSetEnabled={(en) => { onSetEnabled(en); if (!en) onClose(); }}
        />
      )}
    </div>,
    document.body,
  );
}
