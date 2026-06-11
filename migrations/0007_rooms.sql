-- Rooms architecture: Bifrost Rooms become an abstraction layer over synced
-- provider-group mirrors. Plain groups disappear; their data migrates here.

-- ── Provider-group mirrors (read-only; refreshed by "Sync rooms") ──────────
CREATE TABLE IF NOT EXISTS provider_groups (
    id                  TEXT PRIMARY KEY,
    provider_id         TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    -- The group's id in the provider's own namespace (e.g. Hue room UUID).
    provider_group_id   TEXT NOT NULL,
    name                TEXT NOT NULL,
    -- Native group-control handle (Hue grouped_light rid); NULL if none.
    grouped_ref         TEXT,
    UNIQUE (provider_id, provider_group_id)
);

CREATE TABLE IF NOT EXISTS provider_group_lights (
    provider_group_id   TEXT NOT NULL REFERENCES provider_groups(id) ON DELETE CASCADE,
    light_id            TEXT NOT NULL REFERENCES lights(id) ON DELETE CASCADE,
    PRIMARY KEY (provider_group_id, light_id)
);

-- ── Rooms (user-owned abstraction) ──────────────────────────────────────────
CREATE TABLE IF NOT EXISTS rooms (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    -- The name inherited from a linked provider group, for rename-follow:
    -- a sync renames the room only while rooms.name == inherited_name.
    inherited_name  TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Links to provider-group mirrors: membership flows through these on sync.
CREATE TABLE IF NOT EXISTS room_links (
    room_id             TEXT NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    provider_group_id   TEXT NOT NULL REFERENCES provider_groups(id) ON DELETE CASCADE,
    PRIMARY KEY (room_id, provider_group_id)
);

-- Direct members, for providers without a native grouping concept.
CREATE TABLE IF NOT EXISTS room_lights (
    room_id     TEXT NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    light_id    TEXT NOT NULL REFERENCES lights(id) ON DELETE CASCADE,
    PRIMARY KEY (room_id, light_id)
);

-- Palette scenes move from groups to rooms.
CREATE TABLE IF NOT EXISTS room_scenes (
    id          TEXT PRIMARY KEY,
    room_id     TEXT NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    brightness  REAL,
    palette     TEXT NOT NULL DEFAULT '[]',
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- ── Data migration ───────────────────────────────────────────────────────────
-- Every existing group becomes a Room with the same id; its members become
-- direct lights. (Which groups were provider imports isn't recorded, so all
-- start as direct — the first "Sync rooms" creates mirrors and links them by
-- name.)
INSERT INTO rooms (id, name)
    SELECT id, name FROM groups;
INSERT INTO room_lights (room_id, light_id)
    SELECT group_id, light_id FROM group_lights;
INSERT INTO room_scenes (id, room_id, name, brightness, palette, created_at)
    SELECT id, group_id, name, brightness, palette, created_at FROM group_scenes;

-- plan_rooms: group_id becomes room_id (room ids equal old group ids).
-- plan_room_tiles must be parked first: with foreign keys on, dropping
-- plan_rooms would cascade-delete every painted region.
CREATE TABLE plan_room_tiles_backup AS SELECT room_id, x, y FROM plan_room_tiles;
DROP TABLE plan_room_tiles;

CREATE TABLE plan_rooms_new (
    id          TEXT PRIMARY KEY,
    plan_id     TEXT NOT NULL REFERENCES floor_plans(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    room_id     TEXT REFERENCES rooms(id) ON DELETE SET NULL
);
INSERT INTO plan_rooms_new (id, plan_id, name, room_id)
    SELECT pr.id, pr.plan_id, pr.name,
           CASE WHEN EXISTS (SELECT 1 FROM rooms r WHERE r.id = pr.group_id)
                THEN pr.group_id ELSE NULL END
    FROM plan_rooms pr;
DROP TABLE plan_rooms;
ALTER TABLE plan_rooms_new RENAME TO plan_rooms;

CREATE TABLE plan_room_tiles (
    room_id     TEXT NOT NULL REFERENCES plan_rooms(id) ON DELETE CASCADE,
    x           INTEGER NOT NULL,
    y           INTEGER NOT NULL,
    PRIMARY KEY (room_id, x, y)
);
INSERT INTO plan_room_tiles (room_id, x, y)
    SELECT room_id, x, y FROM plan_room_tiles_backup
    WHERE room_id IN (SELECT id FROM plan_rooms);
DROP TABLE plan_room_tiles_backup;

-- Old group tables retire.
DROP TABLE group_scenes;
DROP TABLE group_lights;
DROP TABLE groups;
