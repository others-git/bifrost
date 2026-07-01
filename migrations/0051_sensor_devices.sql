-- Sensor devices: read-only environmental / presence inputs (motion, occupancy,
-- contact, illuminance/lux, temperature, humidity, …) surfaced by any provider.
-- The sixth device domain, and the simplest: a sensor has NO control writes, so
-- there is no command vocabulary — only a reading and a reachability flag.
--
-- Aggregated into Bifrost Rooms (Room.occupied = OR of member presence sensors),
-- so presence-driven behaviour (kiosk display sleep/wake, motion scenes) reads a
-- provider-agnostic room property. Hue motion sensors feed in over the existing
-- bridge SSE, HA binary_sensor/sensor over the existing WebSocket push.
--
-- Mirrors power_devices' final shape (name/provider_name split for the rename
-- guard, hw_id + shadowed_by/shadow_auto for cross-provider de-dup, enabled +
-- glyph for the Devices page). `kind` holds the SensorKind (drives the glyph +
-- whether it counts as presence); `last_state` is the SensorState JSON snapshot;
-- `unit` is the reading's display unit (°C, lx, %) when the provider reports one.
CREATE TABLE sensor_devices (
    id            TEXT PRIMARY KEY,
    provider_id   TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    device_id     TEXT NOT NULL,
    name          TEXT NOT NULL,
    -- The provider-reported name, tracked separately so a user rename survives a
    -- re-discovery (only overwrite `name` when it still equals `provider_name`).
    provider_name TEXT,
    kind          TEXT NOT NULL DEFAULT 'generic', -- motion|occupancy|contact|illuminance|temperature|humidity|generic
    unit          TEXT,                            -- reading unit (°C, lx, %) when known
    last_state    TEXT,                            -- SensorState JSON snapshot
    last_seen     TEXT,
    enabled       INTEGER NOT NULL DEFAULT 1,
    glyph         TEXT,                            -- glyph override; null = derive from kind
    hw_id         TEXT,                            -- normalized hardware id for de-dup
    shadowed_by   TEXT,                            -- when set, a duplicate hidden under this id
    shadow_auto   INTEGER NOT NULL DEFAULT 0,      -- 1 = shadow set automatically by hw_id
    UNIQUE (provider_id, device_id)
);

-- Direct room membership (Devices-page assignment), mirroring room_power_devices.
CREATE TABLE room_sensor_devices (
    room_id          TEXT NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    sensor_device_id TEXT NOT NULL REFERENCES sensor_devices(id) ON DELETE CASCADE,
    PRIMARY KEY (room_id, sensor_device_id)
);

-- Sensor members of a provider group (a synced Hue room / HA Area), so a synced
-- group carries its motion sensors into the Bifrost Room it links.
CREATE TABLE provider_group_sensor_devices (
    provider_group_id TEXT NOT NULL REFERENCES provider_groups(id) ON DELETE CASCADE,
    sensor_device_id  TEXT NOT NULL REFERENCES sensor_devices(id) ON DELETE CASCADE,
    PRIMARY KEY (provider_group_id, sensor_device_id)
);
