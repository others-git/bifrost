-- Registered wall-tablet kiosks (the companion app). A kiosk is identified by
-- the `bfr_` API key it authenticates with; it "checks in" on a heartbeat, and
-- the server hands back a queued management command (sleep/wake/lock). De-auth
-- is a separate immediate key revocation (the app sees 401 → re-enrolls via QR),
-- so `api_key_id` goes NULL (ON DELETE SET NULL) but the row survives to show a
-- pending re-pair.
CREATE TABLE IF NOT EXISTS kiosks (
    id              TEXT PRIMARY KEY,
    -- The key this kiosk authenticates with; NULL once de-authed (key revoked).
    api_key_id      TEXT REFERENCES api_keys(id) ON DELETE SET NULL,
    name            TEXT NOT NULL,
    app_version     TEXT,
    -- Last reported display state (1 = on, 0 = off, NULL = unknown).
    screen_on       INTEGER,
    -- Last heartbeat; drives the online/offline derivation in the clients view.
    last_seen       TEXT,
    -- Queued command consumed on the next check-in: 'sleep' | 'wake' | 'lock'.
    pending_command TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

-- One kiosk row per key (the check-in upserts on this).
CREATE UNIQUE INDEX IF NOT EXISTS kiosks_api_key ON kiosks (api_key_id);
