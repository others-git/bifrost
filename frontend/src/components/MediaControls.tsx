// Shared audio controls — the central control surface for one audio device,
// reused by the Audio page cards and the floor-plan fly-out so the two never
// drift (per CLAUDE.md). Renders now-playing, transport, volume/mute, and
// favorites; power lives in the surrounding chrome (card header / fly-out title)
// via the exported PowerButton.

import { useRef, useState } from "react";
import {
  getMediaFavorites,
  playMediaFavorite,
  setMediaState,
  type MediaCommand,
  type MediaDevice,
  type MediaFavorite,
} from "../api";
import { DisableRow } from "./PowerFlyout";
import { useRemote, RemotePad, RemoteApps } from "./BifrostRemote";
import { PowerToggle, Segmented } from "./controls";
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
  hideTransport = false,
}: {
  device: MediaDevice;
  onLocalPatch: (id: string, patch: Partial<MediaDevice["state"]>) => void;
  /** Tighter spacing + smaller transport buttons, for cramped cards (phones). */
  compact?: boolean;
  /** When this source is bound to a receiver (M22), the receiver's name — shown
   * by the volume row, since volume/mute control the receiver, not this device. */
  receiverName?: string;
  /** Hide the big transport buttons (the AIO TV "Watch" tab has them on the
   * Remote tab instead). */
  hideTransport?: boolean;
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
      {cap.transport && !offline && !hideTransport && (
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

  // A TV's paired remote is resolved server-side onto the effective device
  // (`remote_id`) — its presence turns the fly-out into the unified AIO TV
  // control (Remote / Watch / Apps) instead of the plain media body.
  const pairedRemoteId = device.kind === "tv" ? (device.remote_id ?? null) : null;

  function togglePower() {
    const next = !device.state.power;
    // Optimistic: powering a standby TV on also marks it reachable, so the UI
    // reflects the wake immediately (the composite read confirms it shortly).
    onLocalPatch(device.id, next ? { power: next, reachable: true } : { power: next });
    setMediaState(device.id, { power: next });
  }

  // A TV with a paired remote is a wake-capable composite: keep offering its
  // control (and power) even when the media_player is unreachable — the remote +
  // Wake-on-LAN can bring a cold box up, which is exactly when it's needed. (The
  // backend already reports the composite reachable whenever the remote is.)
  const isTv = !!pairedRemoteId;
  // Power for a TV-with-remote shows regardless of media reachability; for other
  // media it needs source switching and a reachable device.
  const showPower = isTv || (!offline && cap.sources);

  return (
    <Flyout anchor={anchor} onClose={onClose} width={isTv ? 320 : 260}>
      {/* Media is the violet domain — the shared header carries that accent. */}
      <FlyoutHeader
        title={device.name}
        subtitle={device.kind === "tv" ? "TV" : (KIND_LABEL[device.kind] ?? device.kind)}
        accent={color.violet}
        leading={showPower ? <PowerButton on={device.state.power} onToggle={togglePower} /> : undefined}
        onClose={onClose}
      />
      {pairedRemoteId ? (
        <TvAio device={device} remoteId={pairedRemoteId} onLocalPatch={onLocalPatch} receiverName={receiverName} />
      ) : offline ? (
        <div style={{ fontSize: "0.8rem", color: "#c66" }}>Device offline.</div>
      ) : (
        <MediaControls device={device} onLocalPatch={onLocalPatch} receiverName={receiverName} />
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

/** The unified "AIO TV Control" — composed from what the composite (the media
 * device ∪ its paired remote) actually exposes, not a fixed kitchen sink. The
 * keypad **Remote** tab is always present (a paired remote always has the
 * canonical keyset); **Watch** (now-playing, volume, source) appears only when
 * the media side reports any of those capabilities; **Apps** (the launchable
 * grid) appears when app launch is available (the LCD across remotes). When only
 * one surface qualifies, the tab bar is dropped entirely. */
function TvAio({
  device,
  remoteId,
  onLocalPatch,
  receiverName,
}: {
  device: MediaDevice;
  remoteId: string;
  onLocalPatch: (id: string, patch: Partial<MediaDevice["state"]>) => void;
  receiverName?: string;
}) {
  const remote = useRemote(remoteId);
  const [tab, setTab] = useState<"remote" | "apps">("remote");

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.9rem" }}>
      <Segmented
        value={tab}
        onChange={setTab}
        variant="outline"
        accent={color.violet}
        options={[
          { value: "remote", label: "Remote" },
          { value: "apps", label: "Apps" },
        ]}
      />
      {tab === "remote" ? (
        <div style={{ display: "flex", flexDirection: "column", gap: "0.9rem" }}>
          {/* What's on + volume sit above the keypad — the old "Watch" tab, folded in. */}
          <TvNowPlaying device={device} />
          <FancyVolume device={device} onLocalPatch={onLocalPatch} receiverName={receiverName} />
          <RemotePad press={remote.press} />
        </div>
      ) : (
        <RemoteApps
          apps={remote.apps}
          currentApp={remote.currentApp}
          onLaunch={(pkg) => remote.send({ launch_app: { activity: pkg } })}
          onPin={remote.togglePin}
        />
      )}
    </div>
  );
}

/** A compact "what's on the TV" line — the now-playing title (with a secondary
 * line) or the current source / home screen. Sits at the top of the Remote tab. */
function TvNowPlaying({ device }: { device: MediaDevice }) {
  const np = device.state.now_playing;
  const title = np?.title?.trim();
  const sub = np?.artist ?? np?.album;
  const source = device.state.source;
  const primary = title || (source ? `Source · ${source}` : "Home screen");
  return (
    <div style={{ display: "flex", alignItems: "center", gap: "0.6rem", minWidth: 0, padding: "0.1rem" }}>
      <span style={{ display: "grid", placeItems: "center", color: color.violet, flexShrink: 0 }}>
        <Glyph name="tv" size={16} />
      </span>
      <div style={{ minWidth: 0 }}>
        <div style={{ fontSize: "0.9rem", color: T.text, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {primary}
        </div>
        {title && sub && (
          <div style={{ fontSize: "0.74rem", color: T.dim, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {sub}
          </div>
        )}
      </div>
    </div>
  );
}

/** A glassy custom volume slider — a translucent track with a glowing violet fill
 * and a haloed thumb, click/drag to set. Replaces the native range input.
 * Volume/mute route to a bound receiver server-side, so this drives them directly. */
function FancyVolume({
  device,
  onLocalPatch,
  receiverName,
}: {
  device: MediaDevice;
  onLocalPatch: (id: string, patch: Partial<MediaDevice["state"]>) => void;
  receiverName?: string;
}) {
  const st = device.state;
  const volume = st.volume ?? 0;
  const muted = st.mute ?? false;
  const trackRef = useRef<HTMLDivElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);

  function commit(v: number) {
    const clamped = Math.max(0, Math.min(100, Math.round(v)));
    onLocalPatch(device.id, { volume: clamped });
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setMediaState(device.id, { volume: clamped }), 200);
  }
  function fromPointer(clientX: number) {
    const el = trackRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    commit(((clientX - r.left) / r.width) * 100);
  }
  function toggleMute() {
    onLocalPatch(device.id, { mute: !muted });
    setMediaState(device.id, { mute: !muted });
  }

  const fill = muted ? 0 : volume;
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "0.3rem" }}>
      <div style={{ display: "flex", alignItems: "center", gap: "0.7rem" }}>
        <button
          onClick={toggleMute}
          title={muted ? "Unmute" : "Mute"}
          style={{ background: "none", border: "none", cursor: "pointer", padding: 0, color: muted ? color.violet : T.text, display: "grid", placeItems: "center" }}
        >
          <Glyph name={muted ? "mute" : "volume"} size={18} />
        </button>
        <div
          ref={trackRef}
          onPointerDown={(e) => {
            e.currentTarget.setPointerCapture(e.pointerId);
            fromPointer(e.clientX);
          }}
          onPointerMove={(e) => {
            if (e.buttons === 1) fromPointer(e.clientX);
          }}
          style={{
            position: "relative",
            flex: 1,
            height: 10,
            borderRadius: 999,
            cursor: "pointer",
            background: alpha(color.text, 0.1),
            boxShadow: "inset 0 1px 2px rgba(0,0,0,0.45)",
            touchAction: "none",
          }}
        >
          <div
            style={{
              position: "absolute",
              left: 0,
              top: 0,
              bottom: 0,
              width: `${fill}%`,
              borderRadius: 999,
              background: `linear-gradient(90deg, ${alpha(color.violet, 0.6)}, ${color.violet})`,
              boxShadow: muted ? "none" : `0 0 12px ${alpha(color.violet, 0.55)}`,
              transition: "width 0.08s",
            }}
          />
          <div
            style={{
              position: "absolute",
              top: "50%",
              left: `${fill}%`,
              transform: "translate(-50%, -50%)",
              width: 18,
              height: 18,
              borderRadius: "50%",
              background: "#fff",
              boxShadow: `0 0 0 4px ${alpha(color.violet, 0.3)}, 0 2px 6px rgba(0,0,0,0.55)`,
              transition: "left 0.08s",
            }}
          />
        </div>
        <span style={{ fontSize: "0.8rem", color: T.dim, width: 28, textAlign: "right", fontVariantNumeric: "tabular-nums" }}>
          {volume}
        </span>
      </div>
      {receiverName && <div style={{ fontSize: "0.7rem", color: T.dim }}>Volume → {receiverName}</div>}
    </div>
  );
}
