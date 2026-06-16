-- User-configured quick-control buttons on a room's Control-page card. Each is a
-- single glyph button that performs ONE action against a chosen set of the
-- room's devices:
--   power      → toggle the target devices on/off
--   volume     → open a volume control scoped to the target audio devices
--   brightness → open a brightness control scoped to the target lights
--   scene      → apply a palette scene to the room
-- `targets` is a JSON array of {domain, id} (domain ∈ light|audio|power); it's
-- empty for kind='scene', which uses `scene_id` instead. Buttons render left of
-- the room's power button, ordered by `position`.
CREATE TABLE room_controls (
    id         TEXT PRIMARY KEY,
    room_id    TEXT NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,                       -- power | volume | brightness | scene
    glyph      TEXT NOT NULL,                       -- a Glyph registry name
    label      TEXT,                                -- optional caption (tooltip / aria)
    targets    TEXT NOT NULL DEFAULT '[]',          -- JSON [{domain,id}, …]
    scene_id   TEXT REFERENCES palette_scenes(id) ON DELETE CASCADE, -- kind='scene'
    position   INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_room_controls_room ON room_controls(room_id, position);
