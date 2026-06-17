-- Room-scoped scenes + removal of the lossy palette-scene system.
--
-- A scene is now the single snapshot model, scoped by room:
--   room_id IS NULL  → whole-home snapshot  (Home Scene, the existing behaviour)
--   room_id = <room> → that room's members  (Room Scene, new)
-- Both share scene_entries (per-light full LightState) + scene_power_entries and
-- the same capture/apply engine. The old `palette_scenes` "looks" (colours only,
-- no effects / temp / power / per-light fidelity) are dropped.
ALTER TABLE scenes ADD COLUMN room_id TEXT REFERENCES rooms(id) ON DELETE CASCADE;

-- room_controls.scene_id referenced palette_scenes; repoint it at the unified
-- `scenes` table. SQLite can't alter a foreign key in place, so recreate the
-- table. Any existing scene-kind control pointed at a (now-removed) palette
-- scene, so drop those rows first; power/volume/brightness controls are kept.
DELETE FROM room_controls WHERE kind = 'scene';

CREATE TABLE room_controls_new (
    id         TEXT PRIMARY KEY,
    room_id    TEXT NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,
    glyph      TEXT NOT NULL,
    label      TEXT,
    targets    TEXT NOT NULL DEFAULT '[]',
    scene_id   TEXT REFERENCES scenes(id) ON DELETE CASCADE, -- kind='scene'
    position   INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT INTO room_controls_new (id, room_id, kind, glyph, label, targets, scene_id, position, created_at)
    SELECT id, room_id, kind, glyph, label, targets, scene_id, position, created_at FROM room_controls;
DROP TABLE room_controls;
ALTER TABLE room_controls_new RENAME TO room_controls;
CREATE INDEX idx_room_controls_room ON room_controls(room_id, position);

DROP TABLE palette_scenes;
