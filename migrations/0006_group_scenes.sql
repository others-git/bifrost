-- Hue-like scenes attached to a group (and therefore to planner rooms,
-- whose auto-managed groups they ride on). A scene is a colour palette plus
-- brightness; applying it distributes the palette across the group's lights.
CREATE TABLE IF NOT EXISTS group_scenes (
    id          TEXT PRIMARY KEY,
    group_id    TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    -- 1..100; NULL leaves each light's brightness unchanged
    brightness  REAL,
    -- JSON array of "#rrggbb" strings; empty = no colour change
    palette     TEXT NOT NULL DEFAULT '[]',
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
