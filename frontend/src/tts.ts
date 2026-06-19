// Web text-to-speech playback. Synthesizes a reply on the server
// (`POST /api/voice/speak`) and plays it in the browser — the single in-app
// speech path, shared by push-to-talk talk-back and the Settings voice preview.
// The kiosk has its own native playback; this is for browser/web-UI access.
//
// Streaming: a multi-sentence reply is split into sentence chunks and synthesized
// one at a time, so playback starts on the first sentence while the rest render
// (the server serializes synthesis, so prefetching the next chunk overlaps render
// with playback). A single-sentence reply is just one synth+play, as before.

const OPT_OUT_KEY = "bifrostSpeakReplies";

type SpeakOpts = { force?: boolean; voice?: string; format?: string };

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

// The reply currently playing, plus a generation token so a new speak() (or
// stopSpeaking()) cancels any in-flight streaming loop.
let current: HTMLMediaElement | null = null;
let generation = 0;

/** Stop any reply that's currently playing and cancel a streaming loop. */
export function stopSpeaking(): void {
  generation++;
  if (current) {
    current.pause();
    current.src = "";
    current = null;
  }
}

/** Split a reply into sentence-ish chunks so playback can start on the first one
 * while the rest synthesize. Splits on sentence-ending punctuation (keeping it),
 * and drops empties — good enough for short status/assistant replies. */
export function splitSentences(text: string): string[] {
  const chunks = (text.match(/[^.!?\n]+[.!?]*\s*/g) ?? [text])
    .map((s) => s.trim())
    .filter(Boolean);
  return chunks.length ? chunks : [text.trim()].filter(Boolean);
}

/** Synthesize one chunk → object URL. Throws on a non-OK response. */
async function synthChunk(text: string, opts: SpeakOpts): Promise<string> {
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
  return URL.createObjectURL(await res.blob());
}

/** Play a chunk to completion (resolves on ended/error). Skips if cancelled. */
function playChunk(url: string, gen: number): Promise<void> {
  return new Promise((resolve) => {
    if (gen !== generation) {
      URL.revokeObjectURL(url);
      resolve();
      return;
    }
    const audio = new Audio(url);
    current = audio;
    const done = () => {
      URL.revokeObjectURL(url);
      if (current === audio) current = null;
      resolve();
    };
    audio.onended = done;
    audio.onerror = done;
    audio.play().catch(done);
  });
}

/** Play the remaining chunks in order, prefetching the next while one plays. */
async function streamRest(
  chunks: string[],
  firstUrl: string,
  gen: number,
  opts: SpeakOpts,
): Promise<void> {
  // null = synth failed/none; we stop silently (best-effort after the first chunk).
  let next: Promise<string | null> | null = Promise.resolve(firstUrl);
  for (let i = 0; i < chunks.length; i++) {
    const url = await next;
    if (url == null || gen !== generation) {
      if (url) URL.revokeObjectURL(url);
      return;
    }
    // Kick off the next synth before playing this chunk so they overlap.
    next =
      i + 1 < chunks.length ? synthChunk(chunks[i + 1], opts).then((u) => u, () => null) : null;
    await playChunk(url, gen);
    if (gen !== generation) return;
  }
}

/** Synthesize `text` and play it. Resolves once the first sentence has been
 * synthesized (the rest stream in the background); `force` bypasses the talk-back
 * opt-out (for an explicit preview). Throws if the *first* chunk fails, so callers
 * that want to surface it can — the fire-and-forget talk-back path should
 * `.catch()` and ignore. */
export async function speak(text: string, opts: SpeakOpts = {}): Promise<void> {
  const trimmed = text.trim();
  if (!trimmed) return;
  if (!opts.force && !talkBackEnabled()) return;

  const chunks = splitSentences(trimmed);
  if (chunks.length === 0) return;
  stopSpeaking(); // interrupt any prior reply (also bumps the generation token)
  const gen = generation;

  // Synthesize the first chunk up front so its errors (e.g. 503) reach the caller
  // and playback can begin as soon as it's ready; stream the rest in the background.
  const firstUrl = await synthChunk(chunks[0], opts);
  if (gen !== generation) {
    URL.revokeObjectURL(firstUrl);
    return;
  }
  void streamRest(chunks, firstUrl, gen, opts);
}
