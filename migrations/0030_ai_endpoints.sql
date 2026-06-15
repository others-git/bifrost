-- M23 P2: pluggable AI model-role endpoints for the native voice pipeline.
--
-- Each row configures one **OpenAI-compatible** HTTP endpoint for a model
-- *role*, so STT / chat (NLU + LLM fallback) / TTS are each "just another
-- endpoint" the user points at whatever they run locally (Ollama, llama.cpp,
-- faster-whisper, LocalAI, their own server). Bifrost ships **zero weights and
-- zero runtimes** — this table is the thin-client config, mirroring how
-- providers are configured (a row per role, with an encrypted optional key).
--
--   role      'transcription' | 'chat' | 'tts' — one row per role (PRIMARY KEY).
--   base_url  OpenAI-compatible API base, e.g. http://localhost:8080/v1
--   model     The model name to request from that endpoint.
--   api_key   Optional AES-256-GCM-encrypted bearer key (most local servers
--             need none); NULL = no auth header sent. Never stored in plaintext.
--   enabled   A disabled row is kept (so config isn't lost) but the pipeline
--             treats the role as unconfigured — core grammar control still works.
CREATE TABLE ai_endpoints (
    role       TEXT PRIMARY KEY,
    base_url   TEXT NOT NULL,
    model      TEXT NOT NULL,
    api_key    TEXT,
    enabled    INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
