// Voice feedback overlay — the dashboard's visual response to the wall-tablet's
// wake word. The native kiosk app (Vosk) detects the wake word and drives this
// via the WebView: `window.bifrostVoice.setState('listening')` fires the instant
// it's heard (no server round-trip), then `thinking` while the command is sent,
// `speaking` while the reply is read back, `error` on a mishear, and `idle` when
// done. `partial(text)` streams the live transcript. The overlay is an edge glow
// + a pulsing mic pill — non-blocking (pointer-events: none) so the dashboard
// stays usable underneath, and readable across a room on a wall-mounted tablet.

import { useEffect, useRef, useState } from "react";
import { color, alpha, glow, font } from "../theme";

export type VoiceState = "idle" | "listening" | "thinking" | "speaking" | "error";

declare global {
  interface Window {
    bifrostVoice?: {
      setState: (state: VoiceState) => void;
      partial: (text: string) => void;
    };
  }
}

const META: Record<Exclude<VoiceState, "idle">, { label: string; color: string }> = {
  listening: { label: "Listening…", color: color.cyan },
  thinking: { label: "Thinking…", color: color.violet },
  speaking: { label: "Speaking…", color: color.good },
  error: { label: "Didn't catch that", color: color.rose },
};

// Failsafe: if the kiosk app dies mid-utterance and never sends `idle`, the
// overlay clears itself rather than getting stuck on screen. `error` clears
// faster since the app normally moves off it quickly.
const AUTO_IDLE_MS: Record<Exclude<VoiceState, "idle">, number> = {
  listening: 15000,
  thinking: 20000,
  speaking: 30000,
  error: 4000,
};

export function VoiceFeedback() {
  const [state, setState] = useState<VoiceState>("idle");
  const [partial, setPartial] = useState("");
  const idleTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const clearIdle = () => {
      if (idleTimer.current) clearTimeout(idleTimer.current);
      idleTimer.current = null;
    };
    window.bifrostVoice = {
      setState: (next) => {
        clearIdle();
        setState(next);
        if (next === "idle") setPartial("");
        else {
          const ms = AUTO_IDLE_MS[next];
          idleTimer.current = setTimeout(() => {
            setState("idle");
            setPartial("");
          }, ms);
        }
      },
      partial: (text) => setPartial(text),
    };
    return () => {
      clearIdle();
      delete window.bifrostVoice;
    };
  }, []);

  if (state === "idle") return null;
  const { label, color: c } = META[state];

  return (
    <div
      aria-live="polite"
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 90,
        pointerEvents: "none", // never block the dashboard underneath
        display: "flex",
        alignItems: "flex-end",
        justifyContent: "center",
      }}
    >
      {/* Screen-edge glow — pulses (breathing) while active, tinted per state. */}
      <div
        className="bifrost-voice-edge"
        style={{
          position: "absolute",
          inset: 0,
          boxShadow: `inset 0 0 140px -24px ${c}, inset 0 0 0 2px ${alpha(c, 0.55)}`,
        }}
      />

      {/* Mic pill — sits above the mobile tab bar / home inset. */}
      <div
        style={{
          position: "relative",
          marginBottom: "calc(76px + env(safe-area-inset-bottom))",
          maxWidth: "min(80vw, 460px)",
          display: "flex",
          alignItems: "center",
          gap: "0.7rem",
          padding: "0.7rem 1.1rem",
          borderRadius: 999,
          background: alpha(color.void, 0.72),
          backdropFilter: "blur(10px)",
          WebkitBackdropFilter: "blur(10px)",
          border: `1px solid ${alpha(c, 0.5)}`,
          boxShadow: glow(c, 26),
          color: color.text,
        }}
      >
        <span
          className={state === "error" ? undefined : "bifrost-voice-pulse"}
          style={{ display: "grid", placeItems: "center", color: c, flexShrink: 0 }}
        >
          <MicGlyph error={state === "error"} />
        </span>
        <div style={{ minWidth: 0 }}>
          <div style={{ fontSize: "0.9rem", fontWeight: 600, fontFamily: font.display, letterSpacing: "0.03em", color: c }}>
            {label}
          </div>
          {partial && state !== "error" && (
            <div
              style={{
                fontSize: "0.82rem",
                color: color.dim,
                whiteSpace: "nowrap",
                overflow: "hidden",
                textOverflow: "ellipsis",
                maxWidth: "min(70vw, 400px)",
              }}
            >
              {partial}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function MicGlyph({ error }: { error: boolean }) {
  return (
    <svg width={22} height={22} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <rect x="9" y="2" width="6" height="11" rx="3" />
      <path d="M5 10a7 7 0 0 0 14 0" />
      <line x1="12" y1="17" x2="12" y2="21" />
      <line x1="8" y1="21" x2="16" y2="21" />
      {error && <line x1="3" y1="3" x2="21" y2="21" />}
    </svg>
  );
}
