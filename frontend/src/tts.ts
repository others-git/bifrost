// Web text-to-speech playback. Synthesizes a reply on the server
// (`POST /api/voice/speak`) and plays it in the browser — the single in-app
// speech path, shared by push-to-talk talk-back and the Settings voice preview.
// The kiosk has its own native playback; this is for browser/web-UI access.

const OPT_OUT_KEY = "bifrostSpeakReplies";

/** Talk-back is on unless the user opts out. It also self-disables when no TTS
 * model is configured (the server answers `/speak` with 503). */
export function talkBackEnabled(): boolean {
  try {
    return localStorage.getItem(OPT_OUT_KEY) !== "off";
  } catch {
    return true;
  }
}

export function setTalkBackEnabled(on: boolean): void {
  try {
    localStorage.setItem(OPT_OUT_KEY, on ? "on" : "off");
  } catch {
    /* private mode / disabled storage — non-fatal */
  }
}

// The reply currently playing, so a new one can interrupt it instead of overlapping.
let current: HTMLAudioElement | null = null;

/** Stop any reply that's currently playing. */
export function stopSpeaking(): void {
  if (current) {
    current.pause();
    current.src = "";
    current = null;
  }
}

/** Synthesize `text` and play it, resolving once playback *starts*. `force`
 * bypasses the talk-back opt-out (for an explicit preview). Throws on a non-OK
 * response or blocked playback so callers that want to surface it can — the
 * fire-and-forget talk-back path should `.catch()` and ignore. */
export async function speak(
  text: string,
  opts: { force?: boolean; voice?: string; format?: string } = {},
): Promise<void> {
  if (!text.trim()) return;
  if (!opts.force && !talkBackEnabled()) return;

  const res = await fetch("/api/voice/speak", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      text,
      ...(opts.voice ? { voice: opts.voice } : {}),
      ...(opts.format ? { format: opts.format } : {}),
    }),
  });
  if (!res.ok) {
    throw new Error(
      res.status === 503
        ? "No text-to-speech model is configured (Settings → Voice & AI)."
        : `Speech request failed (HTTP ${res.status}).`,
    );
  }

  stopSpeaking(); // don't overlap with an in-flight reply
  const url = URL.createObjectURL(await res.blob());
  const audio = new Audio(url);
  current = audio;
  const cleanup = () => {
    URL.revokeObjectURL(url);
    if (current === audio) current = null;
  };
  audio.onended = cleanup;
  audio.onerror = cleanup;
  await audio.play().catch((e) => {
    cleanup();
    throw e;
  });
}
