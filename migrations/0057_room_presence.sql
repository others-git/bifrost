-- Configurable room presence: which of a room's presence sensors count toward
-- its occupancy. The default stays "every enabled presence member counts" (zero
-- config keeps working, synced sensors participate automatically); this table
-- holds the OPT-OUTS — a sensor listed here is ignored by room_occupancy even
-- though it remains a room member (its readings still display; automations can
-- still target it directly). Exclusion-shaped rather than an allowlist so a
-- newly synced or added sensor can never silently not-count.
CREATE TABLE room_presence_excluded (
    room_id          TEXT NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    sensor_device_id TEXT NOT NULL REFERENCES sensor_devices(id) ON DELETE CASCADE,
    PRIMARY KEY (room_id, sensor_device_id)
);
