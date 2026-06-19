-- Persisted fingerprint of the credential-encryption key, compared on startup to
-- detect a changed BIFROST_SECRET (which silently orphans every stored
-- credential). The stored value is a ONE-WAY SHA-256 of the derived AES key —
-- never the secret or the key itself — so it is safe to keep in plaintext: it
-- reveals nothing and only serves as an identity check across restarts.
CREATE TABLE IF NOT EXISTS credential_key_check (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    key_fp     TEXT NOT NULL,
    first_seen TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
