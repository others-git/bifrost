// BifrostRemote — a provider-agnostic on-screen smart remote for a TV / streamer.
// Opened from the TV's Control / Floor-Plan audio fly-out; renders the canonical
// key set (a circular D-pad, nav, volume, transport) plus dynamic app-launch
// tiles, all driven through the shared remote API (session → the same service
// layer as v1/MCP). Full-width bottom sheet on compact viewports, centred modal
// on desktop.

import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import {
  getRemoteApps,
  getRemoteState,
  sendRemoteCommand,
  setRemoteAppPin,
  type RemoteApp,
  type RemoteCommand,
  type RemoteKey,
  type RemoteState,
} from "../api";
import { useViewport } from "../useViewport";
import { T, ACCENT, alpha } from "../theme";

export function BifrostRemote({
  remoteId,
  name,
  initialOn,
  onClose,
  onVolume,
}: {
  remoteId: string;
  name: string;
  /** Power state from the opening fly-out, shown until the live read lands. */
  initialOn?: boolean;
  onClose: () => void;
  /** When the paired device routes volume elsewhere (e.g. a receiver it's bound
   * to, M22), the volume/mute keys call this instead of sending TV remote keys —
   * so the buttons drive the device that actually owns the volume. */
  onVolume?: (key: "volume_up" | "volume_down" | "mute") => void;
}) {
  const { isCompact } = useViewport();
  const [state, setState] = useState<RemoteState | null>(null);
  const [apps, setApps] = useState<RemoteApp[]>([]);

  // Live state + apps on open; keep power / current-app fresh on a short poll.
  useEffect(() => {
    let alive = true;
    const load = () =>
      getRemoteState(remoteId).then((s) => {
        if (alive && s) setState(s);
      });
    load();
    getRemoteApps(remoteId).then((a) => {
      if (alive) setApps(a);
    });
    const t = setInterval(load, 4000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [remoteId]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const on = state?.on ?? initialOn ?? false;
  const currentApp = state?.current_app;
  const currentAppName = currentApp
    ? (apps.find((a) => a.package === currentApp)?.name ?? currentApp.split(".").pop() ?? currentApp)
    : undefined;

  function send(cmd: RemoteCommand) {
    sendRemoteCommand(remoteId, cmd).then((err) => {
      if (err) console.warn("remote command failed:", err);
    });
  }
  const press = (k: RemoteKey) => () => send({ key: { key: k } });
  function togglePower() {
    setState((s) => ({ ...(s ?? { on }), on: !on }));
    send({ power: { on: !on } });
  }
  async function togglePin(app: RemoteApp) {
    await setRemoteAppPin(remoteId, app.package, !app.pinned);
    setApps(await getRemoteApps(remoteId));
  }

  const panel = (
    <div
      onPointerDown={(e) => e.stopPropagation()}
      style={{
        width: isCompact ? "100%" : 300,
        maxHeight: isCompact ? "92vh" : "90vh",
        overflowY: "auto",
        background: "linear-gradient(180deg, #25252c 0%, #18181d 100%)",
        border: `1px solid ${T.border}`,
        borderRadius: isCompact ? "20px 20px 0 0" : 20,
        padding: isCompact ? "1.1rem 1rem calc(1.2rem + env(safe-area-inset-bottom))" : "1.1rem 1rem 1.3rem",
        boxShadow: "0 -10px 40px rgba(0,0,0,0.65), inset 0 1px 0 rgba(255,255,255,0.05)",
        display: "flex",
        flexDirection: "column",
        gap: "1rem",
      }}
    >
      {/* Header */}
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: "0.6rem" }}>
        <div style={{ minWidth: 0, display: "flex", alignItems: "center", gap: "0.55rem" }}>
          <span
            style={{
              flexShrink: 0,
              width: 30,
              height: 30,
              borderRadius: 8,
              display: "grid",
              placeItems: "center",
              fontSize: "0.95rem",
              background: on ? `${alpha(ACCENT, 0.12)}` : "rgba(255,255,255,0.05)",
              color: on ? ACCENT : T.dim,
            }}
          >
            📺
          </span>
          <div style={{ minWidth: 0 }}>
            <div style={{ fontWeight: 600, fontSize: "0.95rem", color: T.text, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {name}
            </div>
            <div style={{ fontSize: "0.7rem", color: on && currentAppName ? ACCENT : T.faint, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {on ? (currentAppName ? `▶ ${currentAppName}` : "On") : "Standby"}
            </div>
          </div>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: "0.4rem", flexShrink: 0 }}>
          <Key glyph="⏻" label="Power" active={on} onClick={togglePower} />
          <button
            onClick={onClose}
            aria-label="Close"
            style={{ background: "none", border: "none", color: "#888", cursor: "pointer", fontSize: "1.35rem", lineHeight: 1, padding: "0 0.15rem" }}
          >
            ×
          </button>
        </div>
      </div>

      {/* Circular D-pad */}
      <div
        style={{
          position: "relative",
          width: 196,
          height: 196,
          alignSelf: "center",
          borderRadius: "50%",
          background: "radial-gradient(circle at 50% 38%, rgba(255,255,255,0.06), rgba(255,255,255,0.015) 70%)",
          border: `1px solid ${T.border}`,
          boxShadow: "inset 0 1px 0 rgba(255,255,255,0.06), 0 8px 20px rgba(0,0,0,0.45)",
        }}
      >
        <DpadBtn dir="up" onClick={press("up")} />
        <DpadBtn dir="left" onClick={press("left")} />
        <DpadBtn dir="right" onClick={press("right")} />
        <DpadBtn dir="down" onClick={press("down")} />
        <button
          onClick={press("select")}
          title="OK / Select"
          aria-label="OK / Select"
          style={{
            position: "absolute",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            width: 74,
            height: 74,
            borderRadius: "50%",
            border: `1px solid ${alpha(ACCENT, 0.40)}`,
            background: "radial-gradient(circle at 50% 35%, rgba(56,189,248,0.22), rgba(56,189,248,0.05))",
            color: "#fff",
            fontWeight: 700,
            fontSize: "0.92rem",
            letterSpacing: "0.04em",
            cursor: "pointer",
            boxShadow: "0 2px 12px rgba(0,0,0,0.45), inset 0 1px 0 rgba(255,255,255,0.14)",
          }}
        >
          OK
        </button>
      </div>

      {/* Nav */}
      <Row>
        <Key glyph="↩" label="Back" onClick={press("back")} />
        <Key glyph="⌂" label="Home" onClick={press("home")} />
        <Key glyph="☰" label="Menu" onClick={press("menu")} />
      </Row>

      {/* Volume — routes to the receiver when the device is bound (onVolume). */}
      <Row>
        <Key glyph="🔉" label="Volume down" mono onClick={onVolume ? () => onVolume("volume_down") : press("volume_down")} />
        <Key glyph="🔇" label="Mute" mono onClick={onVolume ? () => onVolume("mute") : press("mute")} />
        <Key glyph="🔊" label="Volume up" mono onClick={onVolume ? () => onVolume("volume_up") : press("volume_up")} />
      </Row>

      {/* Transport */}
      <Row>
        <Key glyph="⏮" label="Previous" mono onClick={press("previous")} />
        <Key glyph="⏯" label="Play / pause" mono onClick={press("play_pause")} />
        <Key glyph="⏭" label="Next" mono onClick={press("next")} />
      </Row>

      {/* Apps */}
      <div style={{ borderTop: `1px solid ${T.border}`, paddingTop: "0.8rem" }}>
        <div style={{ fontSize: "0.7rem", color: T.dim, letterSpacing: "0.1em", textTransform: "uppercase", marginBottom: "0.55rem" }}>
          Apps
        </div>
        {apps.length === 0 ? (
          <div style={{ fontSize: "0.8rem", color: T.faint }}>No apps yet — they appear here as you open them on the TV.</div>
        ) : (
          <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(72px, 1fr))", gap: "0.5rem" }}>
            {apps.map((a) => {
              const active = a.package === currentApp;
              return (
                <div key={a.package} style={{ position: "relative" }}>
                  <button
                    onClick={() => send({ launch_app: { activity: a.package } })}
                    title={`Launch ${a.name}`}
                    style={{
                      width: "100%",
                      minHeight: 66,
                      borderRadius: 12,
                      border: `1px solid ${active ? ACCENT : T.border}`,
                      background: active ? `${alpha(ACCENT, 0.10)}` : T.surface,
                      color: T.text,
                      cursor: "pointer",
                      display: "flex",
                      flexDirection: "column",
                      alignItems: "center",
                      gap: "0.35rem",
                      padding: "0.55rem 0.35rem 0.45rem",
                    }}
                  >
                    <span
                      style={{
                        width: 28,
                        height: 28,
                        borderRadius: "50%",
                        display: "grid",
                        placeItems: "center",
                        fontSize: "0.85rem",
                        fontWeight: 700,
                        background: active ? ACCENT : "rgba(255,255,255,0.08)",
                        color: active ? "#0b1220" : T.text,
                      }}
                    >
                      {a.name.charAt(0).toUpperCase()}
                    </span>
                    <span style={{ fontSize: "0.72rem", maxWidth: "100%", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                      {a.name}
                    </span>
                  </button>
                  <button
                    onClick={() => togglePin(a)}
                    title={a.pinned ? "Unpin" : "Pin"}
                    aria-label={a.pinned ? "Unpin" : "Pin"}
                    style={{ position: "absolute", top: 3, right: 3, background: "none", border: "none", cursor: "pointer", fontSize: "0.72rem", color: a.pinned ? ACCENT : T.faint, padding: 2, lineHeight: 1 }}
                  >
                    {a.pinned ? "★" : "☆"}
                  </button>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );

  return createPortal(
    <div
      onPointerDown={onClose}
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 70,
        background: "rgba(0,0,0,0.6)",
        backdropFilter: "blur(2px)",
        display: "flex",
        alignItems: isCompact ? "flex-end" : "center",
        justifyContent: "center",
      }}
    >
      {panel}
    </div>,
    document.body,
  );
}

/** A centred row of remote keys. */
function Row({ children }: { children: React.ReactNode }) {
  return <div style={{ display: "flex", justifyContent: "center", gap: "0.7rem" }}>{children}</div>;
}

/** One directional button on the circular D-pad (transparent, blends into the pad). */
function DpadBtn({ dir, onClick }: { dir: "up" | "down" | "left" | "right"; onClick: () => void }) {
  const arrow = { up: "▲", down: "▼", left: "◀", right: "▶" }[dir];
  const label = { up: "Up", down: "Down", left: "Left", right: "Right" }[dir];
  const pos: React.CSSProperties =
    dir === "up"
      ? { top: 8, left: "50%", transform: "translateX(-50%)" }
      : dir === "down"
        ? { bottom: 8, left: "50%", transform: "translateX(-50%)" }
        : dir === "left"
          ? { left: 10, top: "50%", transform: "translateY(-50%)" }
          : { right: 10, top: "50%", transform: "translateY(-50%)" };
  return (
    <button
      onClick={onClick}
      title={label}
      aria-label={label}
      style={{
        position: "absolute",
        ...pos,
        width: 46,
        height: 46,
        borderRadius: 14,
        border: "none",
        background: "transparent",
        color: T.dim,
        fontSize: "1.05rem",
        cursor: "pointer",
        display: "grid",
        placeItems: "center",
      }}
    >
      {arrow}
    </button>
  );
}

function Key({
  glyph,
  label,
  onClick,
  active,
  mono,
}: {
  glyph: string;
  label: string;
  onClick: () => void;
  active?: boolean;
  mono?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      title={label}
      aria-label={label}
      style={{
        width: 46,
        height: 46,
        borderRadius: 13,
        border: `1px solid ${active ? ACCENT : T.border}`,
        background: active ? `${alpha(ACCENT, 0.12)}` : T.surface,
        color: active ? "#fff" : T.text,
        cursor: "pointer",
        fontSize: "1.1rem",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        flexShrink: 0,
        boxShadow: "inset 0 1px 0 rgba(255,255,255,0.05)",
        // Force emoji glyphs (volume/transport) to a muted monochrome so they
        // match the line icons instead of rendering as bright coloured emoji.
        filter: mono ? "grayscale(1) brightness(1.5) opacity(0.85)" : undefined,
      }}
    >
      {glyph}
    </button>
  );
}
