-- SQLite does not support ALTER TABLE DROP CONSTRAINT.
-- Recreate providers without the hard-coded IN ('hue','govee') check so that
-- new providers registered in Rust need no future schema migrations.
-- Validation is enforced in Rust via ProviderRegistry::is_known().

CREATE TABLE providers_new (
    id              TEXT PRIMARY KEY,
    provider_type   TEXT NOT NULL,
    name            TEXT NOT NULL,
    credentials     TEXT NOT NULL,
    enabled         INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO providers_new SELECT * FROM providers;
DROP TABLE providers;
ALTER TABLE providers_new RENAME TO providers;
