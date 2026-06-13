-- Power devices: strictly on/off endpoints (switches, plugs, fans, boolean
-- helpers) surfaced by integration providers (Home Assistant today).
--
-- Mirrors audio_devices, with one deliberate omission: there is no
-- `capabilities` column. A power device's whole surface is on/off, so it has no
-- capability set to store — that leanness is the point of the domain. `kind`
-- holds the PowerKind (switch | outlet | fan | toggle | generic) that drives the
-- UI glyph; `last_state` is the PowerState JSON snapshot ({"on":bool,...}).
CREATE TABLE power_devices (
    id          TEXT PRIMARY KEY,
    provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    device_id   TEXT NOT NULL,
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL DEFAULT 'generic',  -- switch | outlet | fan | toggle | generic
    last_state  TEXT,
    last_seen   TEXT,
    UNIQUE (provider_id, device_id)
);
