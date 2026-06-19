// Shared audio controls — the central control surface for one audio device,
// reused by the Audio page cards and the floor-plan fly-out so the two never
// drift (per CLAUDE.md). Renders now-playing, transport, volume/mute, and
// favorites; power lives in the surrounding chrome (card header / fly-out title)
// via the exported PowerButton.

import { useEffect, useRef, useState } from "react";
import {
  getMediaFavorites,
  getRemoteDevices,
  playMediaFavorite,
  setMediaState,
  type MediaCommand,
  type MediaDevice,
  type MediaFavorite,
  type RemoteDevice,
} from "../api";
import { DisableRow } from "./PowerFlyout";
import { BifrostRemote } from "./BifrostRemote";
import { PowerToggle } from "./controls";
import { Flyout, FlyoutHeader } from "./Flyout";
import { Select } from "./Select";
import { Glyph } from "./glyphs";
import { T, domain, color, alpha, labelType } from "../theme";

const ACCENT = domain.media; // violet — audio's accent

export const KIND_LABEL: Record<string, string> = {
  receiver: "Receiver",
  speaker: "Speaker",
  zone: "Zone",
};

/** The control body for one audio device. State is owned by the parent (live
 * via SSE/poll) and updated optimistically through `onLocalPatch`. */
export function MediaControls({
  device,
  onLocalPatch,
  compact = false,
  receiverName,
}: {
  device: MediaDevice;
  onLocalPatch: (id: string, patch: Partial<MediaDevice["state"]>) => void;
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
  const [favs, setFavs] = useState<MediaFavorite[] | null>(null);
  const [favBusy, setFavBusy] = useState(false);

  async function send(cmd: MediaCommand) {
    const err = await setMediaState(device.id, cmd);
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
      setFavs(await getMediaFavorites(device.id));
      setFavBusy(false);
    }
  }
  async function playFav(f: MediaFavorite) {
    onLocalPatch(device.id, { power: true });
    const err = await playMediaFavorite(device.id, f.id);
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
          <TransportButton glyph="prev" title="Previous" compact={compact} onClick={() => send({ transport: "previous" })} />
          <TransportButton glyph={playing ? "pause" : "play"} title="Play / pause" big compact={compact} onClick={() => send({ transport: "toggle" })} />
          <TransportButton glyph="next" title="Next" compact={compact} onClick={() => send({ transport: "next" })} />
        </div>
      )}

      {/* Volume */}
      {!offline && (
        <div style={{ display: "flex", alignItems: "center", gap: "0.6rem" }}>
          <button
            onClick={toggleMute}
            title={s.mute ? "Unmute" : "Mute"}
            style={{ background: "none", border: "none", cursor: "pointer", padding: 0, color: T.text, display: "grid", placeItems: "center", opacity: s.mute ? 1 : 0.6 }}
          >
            <Glyph name={s.mute ? "mute" : "volume"} size={18} />
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
          <Select
            value={s.source ?? undefined}
            onChange={(src) => send({ source: src })}
            title="Switch input / app"
            style={{ flex: 1, minWidth: 0 }}
            options={[
              // The current source can be outside the list (unknown/legacy) — keep it visible.
              ...(s.source && !s.source_list.includes(s.source) ? [{ value: s.source, label: s.source }] : []),
              ...s.source_list.map((src) => ({ value: src, label: src })),
            ]}
          />
        </div>
      )}

      {/* Favorites */}
      {cap.favorites && !offline && (
        <div style={{ borderTop: `1px solid ${color.hairline}`, paddingTop: "0.6rem" }}>
          <button
            onClick={toggleFavorites}
            style={{ ...labelType, background: "none", border: "none", color: T.dim, cursor: "pointer", fontSize: "0.62rem", padding: 0, display: "flex", alignItems: "center", gap: "0.4rem" }}
          >
            <Glyph name="favorite" size={13} /> Favorites
            <span style={{ display: "grid", transform: favOpen ? "rotate(180deg)" : "none" }}>
              <Glyph name="chevron" size={13} />
            </span>
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
                    <span style={{ color: ACCENT, display: "grid", placeItems: "center" }}><Glyph name="play" size={13} /></span>
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
        background: big ? `${alpha(ACCENT, 0.13)}` : "rgba(255,255,255,0.04)",
        color: big ? "#fff" : T.dim,
        cursor: "pointer",
        display: "grid",
        placeItems: "center",
        flexShrink: 0,
      }}
    >
      <Glyph name={glyph} size={big ? (compact ? 18 : 22) : compact ? 16 : 18} />
    </button>
  );
}

/** Power for receivers/zones — the shared power button, lit violet (audio domain),
 * so every device type powers on/off identically. */
export function PowerButton({ on, onToggle }: { on: boolean; onToggle: () => void }) {
  return <PowerToggle on={on} accent={domain.media} onToggle={onToggle} />;
}

/**
 * Anchored fly-out wrapping MediaControls — the floor-plan counterpart to
 * LightEditor. State is owned by the caller; `onLocalPatch` updates it
 * optimistically and `setMediaState` drives the device.
 */
export function MediaEditor({
  device,
  anchor,
  onLocalPatch,
  onSetEnabled,
  onClose,
  receiverName,
}: {
  device: MediaDevice;
  anchor: HTMLElement | { x: number; y: number };
  onLocalPatch: (id: string, patch: Partial<MediaDevice["state"]>) => void;
  /** Enable/disable the device. Disabling drops it from room control. */
  onSetEnabled?: (enabled: boolean) => void;
  onClose: () => void;
  /** M22: name of the receiver this source's volume routes to, if bound. */
  receiverName?: string;
}) {
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
      if (alive) setPairedRemote(rs.find((r) => r.paired_media_id === device.id && r.enabled) ?? null);
    });
    return () => {
      alive = false;
    };
  }, [device.kind, device.id]);

  function togglePower() {
    onLocalPatch(device.id, { power: !device.state.power });
    setMediaState(device.id, { power: !device.state.power });
  }

  return (
    <Flyout anchor={anchor} onClose={onClose} width={260} closeGuard={() => remoteOpenRef.current}>
      {/* Audio is the violet domain — the shared header carries that accent. */}
      <FlyoutHeader
        title={device.name}
        subtitle={KIND_LABEL[device.kind] ?? device.kind}
        accent={color.violet}
        leading={cap.sources && !offline ? <PowerButton on={device.state.power} onToggle={togglePower} /> : undefined}
        onClose={onClose}
      />
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
            border: `1px solid ${alpha(ACCENT, 0.33)}`,
            background: `${alpha(ACCENT, 0.08)}`,
            color: T.text,
            cursor: "pointer",
            fontSize: "0.85rem",
          }}
        >
          <Glyph name="tv" size={16} /> Remote
        </button>
      )}
      {offline ? (
        <div style={{ fontSize: "0.8rem", color: "#c66" }}>Device offline.</div>
      ) : (
        <MediaControls device={device} onLocalPatch={onLocalPatch} receiverName={receiverName} />
      )}
      {remoteOpen && pairedRemote && (
        <BifrostRemote
          remoteId={pairedRemote.id}
          name={device.name}
          initialOn={device.state.power}
          anchor={anchor}
          onClose={() => setRemoteOpen(false)}
          // Bound to a receiver (M22): the remote's volume/mute drive the device,
          // which the backend routes to the receiver — not the TV's own volume.
          onVolume={
            device.receiver_id
              ? (k) => {
                  const st = device.state;
                  if (k === "mute") {
                    onLocalPatch(device.id, { mute: !st.mute });
                    setMediaState(device.id, { mute: !st.mute });
                  } else {
                    const v = Math.max(0, Math.min(100, (st.volume ?? 0) + (k === "volume_up" ? 2 : -2)));
                    onLocalPatch(device.id, { volume: v });
                    setMediaState(device.id, { volume: v });
                  }
                }
              : undefined
          }
        />
      )}
      {onSetEnabled && (
        <DisableRow
          enabled={device.enabled !== false}
          onSetEnabled={(en) => { onSetEnabled(en); if (!en) onClose(); }}
        />
      )}
    </Flyout>
  );
}
