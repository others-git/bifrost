-- Audio device placements on a floor plan — the audio analog of plan_lights.
-- Speakers/receivers are point placements (no LED-strip `points`); a device
-- sits on one tile with a mount point, like a point light.
CREATE TABLE IF NOT EXISTS plan_audio_devices (
    plan_id         TEXT NOT NULL REFERENCES floor_plans(id)   ON DELETE CASCADE,
    audio_device_id TEXT NOT NULL REFERENCES audio_devices(id) ON DELETE CASCADE,
    x               INTEGER NOT NULL,
    y               INTEGER NOT NULL,
    mount           TEXT NOT NULL DEFAULT 'c' CHECK (mount IN ('c', 'n', 's', 'e', 'w')),
    PRIMARY KEY (plan_id, audio_device_id)
);
