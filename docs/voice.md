# Voice & assistants

Bifrost can be driven hands-free two ways: **spoken voice** (a built-in pipeline)
and **AI assistants** (an embedded MCP server). Both go through the *same* shared
service layer as the web UI and REST API, so they can't drift from it. Everything
here is **optional and degrades gracefully**, and the models are **pluggable** —
they can run entirely on your own hardware, or not at all.

## How a spoken command flows

1. **Capture.** Hold the dashboard's **push-to-talk** mic (phone/desktop/tablet),
   or — on the wall tablet — just say the **wake word** ("bifrost").
2. **Transcribe (STT).** The audio becomes text: on-device on the kiosk (offline
   Vosk), or server-side via the configured *transcription* model
   (`POST /api/voice/listen`).
3. **Resolve & act — native first, then fall back:**
   - **Deterministic grammar** *(no model, always on)* — parses power, brightness,
     color, color-temperature, volume, mute, transport, scenes, and relative
     nudges, resolves targets by room/device name, and handles compound commands
     ("dim the office and pause the kitchen"). Fast and offline.
   - **TV content** — "open Netflix on the bedroom TV" / "play Bob's Burgers on the
     living room TV" resolves to an app launch on that TV (matched by name; a title
     with no matching app opens the TV's last-used app as the best guess).
   - **LLM fallback** — anything still unparsed is handed to the *chat* model
     (OpenAI-compatible tool-calling), which maps it to the **same** internal
     command and dispatches it the same way.
   - **Home Assistant Assist** — if HA is connected and the LLM didn't resolve it,
     the clause is delegated to HA Assist (the long tail, e.g. "play *Bob's
     Burgers* on the TV").
   - With no models configured, the grammar still handles what it can; the rest
     comes back as "I didn't understand."
4. **Talk-back (TTS).** The spoken reply is synthesized by the *tts* model and
   played back. Optional — text and command control never depend on it.

## AI model endpoints

Configure these under **Settings → Voice & AI**. There are three roles, each an
independent **OpenAI-compatible HTTP endpoint** (base URL + model + optional key):

| Role | What it does | Used by |
|---|---|---|
| **transcription** | speech-to-text | `POST /api/voice/listen` (clients that upload audio) |
| **chat** | the LLM fallback / natural-language understanding | any command the grammar can't parse |
| **tts** | text-to-speech | spoken replies (talk-back) |

Point them at whatever you run — a local **Ollama** / **faster-whisper** /
**Piper** / **Kokoro** / **vocals-mcp**, or any hosted API. The **Test** button
probes the endpoint's `/models`. Each role degrades on its own: unset just means
that capability is off (e.g. `/listen` returns `503` until a transcription model
is set), while the deterministic grammar always works over text. Models are never
mandated — Bifrost ships no weights and no runtimes.

## Talk-back (spoken replies)

`POST /api/voice/speak` turns a reply into audio via the *tts* endpoint and plays
it (in the browser and on the kiosk; opt out per client). Multi-sentence replies
**stream** — the first sentence starts playing while the rest synthesize, so a
long reply doesn't wait on the whole thing. Pair it with an OpenAI-compatible TTS
server that supports voice cloning (e.g. **vocals-mcp**) to have Bifrost answer in
a voice you choose.

## The wall-tablet kiosk

The **[bifrost-kiosk](https://github.com/others-git/bifrost-kiosk)** app makes a
mounted tablet a hands-free fixture: an always-on, **offline wake word** plus
push-to-talk, with replies played natively. It's the display dashboard and the
voice satellite in one device-owner app. See [Pair the wall
tablet](setup.md#6-pair-the-wall-tablet).

## AI assistants (MCP)

The embedded **[MCP server](mcp.md)** at `/mcp` exposes the whole home as
assistant tools — lights, media, power, rooms, scenes, remotes, casting —
resolved by **name**, not just id. It's the same shared service layer behind the
voice pipeline, gated by the same Bearer API keys as `/api/v1`. Point any MCP
client (e.g. a desktop assistant) at it for natural-language control.

## Privacy & where things run

Nothing leaves your network unless *you* point a model endpoint at a cloud
service — the deterministic grammar is offline, the kiosk's wake word is
on-device, and STT/chat/TTS can all be local. All voice endpoints are gated by
the same Bearer API keys as `/api/v1`.

## API reference

The HTTP endpoints (`/api/voice/command`, `/api/voice/listen`, `/api/voice/speak`,
`/api/voice/vocabulary`) and their request/response shapes are documented under
**[Public API → Voice control](api.md#voice-control-apivoice)**; the assistant
tools under **[MCP server](mcp.md)**.
