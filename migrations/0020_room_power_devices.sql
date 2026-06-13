-- A room can hold power devices (switches, plugs, fans) as members, the same
-- way it holds direct lights. Simple membership — power devices have no per-room
-- calibration (unlike audio's volume offset), so this mirrors room_lights.
CREATE TABLE room_power_devices (
    room_id         TEXT NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    power_device_id TEXT NOT NULL REFERENCES power_devices(id) ON DELETE CASCADE,
    PRIMARY KEY (room_id, power_device_id)
);
