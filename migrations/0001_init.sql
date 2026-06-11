-- App config: single row, set on first-run setup
CREATE TABLE IF NOT EXISTS config (
    id      INTEGER PRIMARY KEY CHECK (id = 1),
    password_hash   TEXT NOT NULL,
    setup_complete  INTEGER NOT NULL DEFAULT 0
);

-- Provider instances (one row per configured bridge/account)
CREATE TABLE IF NOT EXISTS providers (
    id              TEXT PRIMARY KEY,
    provider_type   TEXT NOT NULL CHECK (provider_type IN ('hue', 'govee')),
    name            TEXT NOT NULL,
    -- AES-256-GCM: hex(12-byte nonce || ciphertext || 16-byte tag)
    credentials     TEXT NOT NULL,
    enabled         INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Lights: cached from last discovery/event
CREATE TABLE IF NOT EXISTS lights (
    id              TEXT PRIMARY KEY,
    provider_id     TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    device_id       TEXT NOT NULL,
    name            TEXT NOT NULL,
    capabilities    TEXT NOT NULL DEFAULT '{}',
    last_state      TEXT,
    last_seen       TEXT,
    UNIQUE (provider_id, device_id)
);

-- Web sessions (single-password auth)
CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT PRIMARY KEY,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at  TEXT NOT NULL,
    last_used   TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS sessions_expires ON sessions (expires_at);
