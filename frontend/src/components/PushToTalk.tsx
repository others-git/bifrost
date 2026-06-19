// Push-to-talk: a global hold-to-talk mic button. Press and hold to capture mic
// audio (MediaRecorder), release to upload it to `POST /api/voice/listen`, which
// transcribes server-side and runs the text through the same command seam as the
// wake word. It bypasses wake spotting entirely — handy on a phone/desktop and as
// a reliable fallback when on-device wake detection is flaky.
//
// On-screen feedback reuses the existing `VoiceFeedback` overlay via the shared
// `window.bifrostVoice` bridge (listening → thinking → idle/error), so the visual
// language matches the kiosk's wake-word flow.

import { useRef, useState } from "react";
import { color, alpha, glow } from "../theme";
import { useViewport } from "../useViewport";
import { speak } from "../tts";

declare global {
  interface Window {
    /** Injected by the kiosk app (addJavascriptInterface). Hold-to-talk: `start()`
     * on press begins native capture (skip wake word), `stop()` on release ends it
     * and dispatches the held command. */
    bifrostKioskPtt?: { start: () => void; stop: () => void };
  }
}

// Below this hold time a press is treated as an accidental tap and dropped, so a
// stray click never fires an empty transcription request.
const MIN_HOLD_MS = 300;

/// Pick a MediaRecorder container the browser supports; Whisper-style STT
/// endpoints accept webm/opus and mp4. `undefined` ⇒ let the browser default.
function pickMimeType(): string | undefined {
  if (typeof MediaRecorder === "undefined") return undefined;
  const candidates = ["audio/webm;codecs=opus", "audio/webm", "audio/mp4", "audio/ogg;codecs=opus"];
  return candidates.find((m) => MediaRecorder.isTypeSupported(m));
}

export function PushToTalk() {
  const { isCompact } = useViewport();
  const [active, setActive] = useState(false);
  const recRef = useRef<MediaRecorder | null>(null);
  const chunksRef = useRef<Blob[]>([]);
  const streamRef = useRef<MediaStream | null>(null);
  const startedAtRef = useRef(0);
  const sendingRef = useRef(false);

  const feedback = (state: "listening" | "thinking" | "idle" | "error", detail?: string) =>
    window.bifrostVoice?.setState(state, detail);

  function releaseStream() {
    streamRef.current?.getTracks().forEach((t) => t.stop());
    streamRef.current = null;
  }

  async function start(e: React.PointerEvent<HTMLButtonElement>) {
    e.preventDefault();
    if (recRef.current || sendingRef.current) return;

    // On the kiosk, hand off to the native voice pipeline (the WebView mic can't
    // run over plain HTTP). Hold-to-talk: native captures while held; release
    // (`stop`) dispatches. The native side drives the listening overlay.
    if (window.bifrostKioskPtt) {
      try {
        e.currentTarget.setPointerCapture(e.pointerId);
      } catch {
        /* not fatal */
      }
      window.bifrostKioskPtt.start();
      setActive(true);
      return;
    }

    // Keep receiving pointerup even if the finger/cursor drifts off the button.
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      /* not fatal — some browsers reject capture on synthetic events */
    }

    if (!navigator.mediaDevices?.getUserMedia || typeof MediaRecorder === "undefined") {
      feedback(
        "error",
        window.isSecureContext ? "Mic not supported here" : "Microphone needs HTTPS (or localhost)",
      );
      return;
    }

    let stream: MediaStream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch {
      feedback("error", "Microphone blocked — allow access");
      return;
    }
    streamRef.current = stream;

    const mime = pickMimeType();
    const rec = new MediaRecorder(stream, mime ? { mimeType: mime } : undefined);
    chunksRef.current = [];
    rec.ondataavailable = (ev) => {
      if (ev.data.size) chunksRef.current.push(ev.data);
    };
    rec.onstop = () => void finish(mime);
    recRef.current = rec;
    startedAtRef.current = Date.now();
    rec.start();
    setActive(true);
    feedback("listening");
  }

  function stop(e?: React.PointerEvent<HTMLButtonElement>) {
    e?.preventDefault();
    setActive(false);
    // Kiosk hold-to-talk: release ends native capture and dispatches.
    if (window.bifrostKioskPtt) {
      window.bifrostKioskPtt.stop();
      return;
    }
    const rec = recRef.current;
    if (rec && rec.state !== "inactive") rec.stop(); // → onstop → finish()
  }

  async function finish(mime?: string) {
    const heldMs = Date.now() - startedAtRef.current;
    const type = mime ?? "audio/webm";
    const blob = new Blob(chunksRef.current, { type });
    chunksRef.current = [];
    recRef.current = null;
    releaseStream();

    if (heldMs < MIN_HOLD_MS || blob.size === 0) {
      feedback("idle");
      return;
    }

    sendingRef.current = true;
    feedback("thinking");
    try {
      const ext = type.includes("mp4") ? "mp4" : type.includes("ogg") ? "ogg" : "webm";
      const fd = new FormData();
      fd.append("file", blob, `speech.${ext}`);
      const res = await fetch("/api/voice/listen", { method: "POST", body: fd });
      if (res.status === 503) {
        feedback("error", "No transcription model set (Settings → AI)");
      } else if (!res.ok) {
        feedback("error", "Voice request failed");
      } else {
        const data = await res.json().catch(() => null);
        const transcript = (data?.transcript ?? "").trim();
        if (transcript) window.bifrostVoice?.partial(transcript);
        // The device action itself is the confirmation; clear the overlay.
        feedback("idle");
        // Speak the reply back (single-turn talk-back). Best-effort and
        // non-blocking — it never affects whether the command itself ran.
        const said = (data?.said ?? "").trim();
        if (said) void speak(said).catch(() => {});
      }
    } catch {
      feedback("error", "Couldn't reach the server");
    } finally {
      sendingRef.current = false;
    }
  }

  return (
    <button
      aria-label="Hold to talk"
      title="Hold to talk"
      onPointerDown={start}
      onPointerUp={stop}
      onPointerCancel={() => stop()}
      onContextMenu={(e) => e.preventDefault()}
      style={{
        position: "fixed",
        right: 16,
        bottom: isCompact ? "calc(70px + env(safe-area-inset-bottom))" : 24,
        zIndex: 85,
        width: 56,
        height: 56,
        borderRadius: "50%",
        display: "grid",
        placeItems: "center",
        cursor: "pointer",
        color: active ? color.void : color.cyan,
        background: active ? color.cyan : alpha(color.void, 0.72),
        border: `1px solid ${alpha(color.cyan, active ? 0.95 : 0.45)}`,
        boxShadow: glow(color.cyan, active ? 30 : 16),
        backdropFilter: "blur(10px)",
        WebkitBackdropFilter: "blur(10px)",
        transform: active ? "scale(1.08)" : "scale(1)",
        transition: "transform .15s, background .15s, box-shadow .2s, color .15s",
        touchAction: "none",
        userSelect: "none",
      }}
    >
      <span className={active ? "bifrost-voice-pulse" : undefined} style={{ display: "grid", placeItems: "center" }}>
        <MicGlyph />
      </span>
    </button>
  );
}

function MicGlyph() {
  return (
    <svg
      width={24}
      height={24}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <rect x="9" y="2" width="6" height="11" rx="3" />
      <path d="M5 10a7 7 0 0 0 14 0" />
      <line x1="12" y1="17" x2="12" y2="21" />
      <line x1="8" y1="21" x2="16" y2="21" />
    </svg>
  );
}
