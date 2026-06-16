-- Home Scenes: whole-home snapshot/restore of lights + power devices. Repurposes
-- the global `scenes` system (previously lights-only and UI-less) into a one-tap
-- "Restore Home" default — handy after a power outage resets bulbs/switches to
-- factory state. `is_default` marks the single preset the Restore-Home button
-- applies; the partial-unique index keeps at most one default.
ALTER TABLE scenes ADD COLUMN is_default INTEGER NOT NULL DEFAULT 0;

CREATE UNIQUE INDEX idx_scenes_one_default ON scenes(is_default) WHERE is_default = 1;

-- Captured on/off state of power devices, mirroring scene_entries for lights.
CREATE TABLE scene_power_entries (
    scene_id        TEXT NOT NULL REFERENCES scenes(id) ON DELETE CASCADE,
    power_device_id TEXT NOT NULL REFERENCES power_devices(id) ON DELETE CASCADE,
    on_state        INTEGER NOT NULL,
    PRIMARY KEY (scene_id, power_device_id)
);
