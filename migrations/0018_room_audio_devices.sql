-- A room can hold MANY audio devices (e.g. two Sonos in one office). Room
-- volume/mute fans out to all of them, and each carries a per-room volume
-- offset (signed %, added to the room volume then clamped 0–100) so a given
-- room level lands at the same loudness on speakers of differing output.
--
-- Replaces the single-device `room_audio` table. Existing links migrate over
-- with a zero offset.
CREATE TABLE room_audio_devices (
    room_id         TEXT NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    audio_device_id TEXT NOT NULL REFERENCES audio_devices(id) ON DELETE CASCADE,
    volume_offset   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (room_id, audio_device_id)
);

INSERT INTO room_audio_devices (room_id, audio_device_id, volume_offset)
    SELECT room_id, audio_device_id, 0 FROM room_audio;

DROP TABLE room_audio;
