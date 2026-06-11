-- Scenes: a named snapshot of light states, applied all at once.
CREATE TABLE IF NOT EXISTS scenes (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS scene_entries (
    scene_id    TEXT NOT NULL REFERENCES scenes(id) ON DELETE CASCADE,
    light_id    TEXT NOT NULL REFERENCES lights(id) ON DELETE CASCADE,
    -- LightState JSON captured when the scene was saved
    state       TEXT NOT NULL,
    PRIMARY KEY (scene_id, light_id)
);

-- Groups: a named set of lights controlled together.
CREATE TABLE IF NOT EXISTS groups (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS group_lights (
    group_id    TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    light_id    TEXT NOT NULL REFERENCES lights(id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, light_id)
);
