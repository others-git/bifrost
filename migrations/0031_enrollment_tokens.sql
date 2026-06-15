-- Device enrollment tokens. A short-lived, single-use token minted from an
-- authenticated session and carried to a headless device (e.g. the wall tablet)
-- inside a pairing QR. The device redeems it for a real API key, so no one ever
-- types a 68-char key on a touchscreen. Tokens are high-entropy opaque strings;
-- only their plaintext is needed (they're already unguessable + short-lived), so
-- unlike api_keys we store the value directly.
CREATE TABLE IF NOT EXISTS enrollment_tokens (
    token       TEXT PRIMARY KEY,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at  TEXT NOT NULL,
    -- Set when redeemed; NULL while the token is still pending. Single-use.
    used_at     TEXT
);
