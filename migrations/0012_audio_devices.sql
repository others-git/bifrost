-- Audio devices (receivers, speakers) discovered by audio providers.
-- Mirrors the lights table: provider-scoped device id, JSON capability and
-- state snapshots refreshed on discovery / reads.
CREATE TABLE audio_devices (
    id           TEXT PRIMARY KEY,
    provider_id  TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    device_id    TEXT NOT NULL,
    name         TEXT NOT NULL,
    kind         TEXT NOT NULL DEFAULT 'receiver',  -- receiver | speaker | zone
    capabilities TEXT NOT NULL DEFAULT '{}',
    last_state   TEXT,
    last_seen    TEXT,
    UNIQUE (provider_id, device_id)
);
