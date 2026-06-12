-- Palette scenes become global. Previously each scene was bound to one room
-- (room_scenes.room_id); now a scene is a standalone Hue-style preset — a name,
-- an optional brightness, and a color palette — that can be applied to any room.
-- Rooms pick which scene to apply; a room's current colors can be saved as a
-- new global scene.
CREATE TABLE IF NOT EXISTS palette_scenes (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    -- 1..100; NULL leaves each light's brightness unchanged
    brightness  REAL,
    -- JSON array of "#rrggbb" strings; empty = no colour change
    palette     TEXT NOT NULL DEFAULT '[]',
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Carry existing room-scoped scenes over as global scenes (room binding dropped).
-- Two rooms that each had a "Relax" scene become two global "Relax" scenes; the
-- user can prune duplicates.
INSERT INTO palette_scenes (id, name, brightness, palette, created_at)
    SELECT id, name, brightness, palette, created_at FROM room_scenes;

DROP TABLE room_scenes;
