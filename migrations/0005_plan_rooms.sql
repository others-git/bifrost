-- Rooms: named tile regions on a floor plan, linked to an auto-managed group
-- so the room can be controlled (on/off, scenes) as a unit.
CREATE TABLE IF NOT EXISTS plan_rooms (
    id          TEXT PRIMARY KEY,
    plan_id     TEXT NOT NULL REFERENCES floor_plans(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    -- The group whose membership mirrors the lights placed inside this room.
    -- Created and maintained by the rooms API; deleted with the room.
    group_id    TEXT REFERENCES groups(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS plan_room_tiles (
    room_id     TEXT NOT NULL REFERENCES plan_rooms(id) ON DELETE CASCADE,
    x           INTEGER NOT NULL,
    y           INTEGER NOT NULL,
    PRIMARY KEY (room_id, x, y)
);
