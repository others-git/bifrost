-- Client API keys for the public /api/v1 surface. No RBAC: a valid key grants
-- full access to lights and rooms. Keys are high-entropy random strings; we
-- store only their SHA-256 hash (hex) and a short display prefix, and reveal
-- the full key exactly once at creation.
CREATE TABLE IF NOT EXISTS api_keys (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    -- Hex SHA-256 of the full key; unique so lookups are an indexed equality.
    key_hash    TEXT NOT NULL UNIQUE,
    -- First few characters of the key (e.g. "bfr_a1b2c3"), for display only.
    prefix      TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    last_used   TEXT
);
