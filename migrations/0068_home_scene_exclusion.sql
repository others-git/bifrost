-- Per-device opt-out from Home Scenes (whole-home snapshots, incl. the "Restore
-- Home" default). An excluded light/power device is skipped when a Home Scene is
-- captured AND when one is applied, so flipping the flag takes effect on existing
-- scenes without a recapture. Room Scenes are unaffected.
ALTER TABLE lights        ADD COLUMN home_scene_excluded INTEGER NOT NULL DEFAULT 0;
ALTER TABLE power_devices ADD COLUMN home_scene_excluded INTEGER NOT NULL DEFAULT 0;
