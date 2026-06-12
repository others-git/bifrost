-- Optional room → audio device link: surfaces volume/mute on the room's
-- controls (Floor Plan, and later the Dashboard). One device per room; a
-- device may serve several rooms (open-plan receiver zones).
CREATE TABLE room_audio (
    room_id         TEXT PRIMARY KEY REFERENCES rooms(id) ON DELETE CASCADE,
    audio_device_id TEXT NOT NULL REFERENCES audio_devices(id) ON DELETE CASCADE
);
