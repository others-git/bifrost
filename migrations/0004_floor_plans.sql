-- Floor plans: a 2D tile grid representing a floor of the house.
CREATE TABLE IF NOT EXISTS floor_plans (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    width       INTEGER NOT NULL CHECK (width  BETWEEN 1 AND 128),
    height      INTEGER NOT NULL CHECK (height BETWEEN 1 AND 128),
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Sparse floor tiles: a row's existence means "this square foot is floor".
CREATE TABLE IF NOT EXISTS plan_tiles (
    plan_id     TEXT NOT NULL REFERENCES floor_plans(id) ON DELETE CASCADE,
    x           INTEGER NOT NULL,
    y           INTEGER NOT NULL,
    PRIMARY KEY (plan_id, x, y)
);

-- Wall segments on tile boundaries (walls are edges, not tiles).
-- dir 'h': horizontal segment along the TOP edge of tile (x, y) — y may equal height
-- dir 'v': vertical segment along the LEFT edge of tile (x, y) — x may equal width
CREATE TABLE IF NOT EXISTS plan_walls (
    plan_id     TEXT NOT NULL REFERENCES floor_plans(id) ON DELETE CASCADE,
    x           INTEGER NOT NULL,
    y           INTEGER NOT NULL,
    dir         TEXT NOT NULL CHECK (dir IN ('h', 'v')),
    PRIMARY KEY (plan_id, x, y, dir)
);

-- Light placements: tile + mount point.
-- mount 'c' = tile centre (ceiling / floor lamp); n/s/e/w = wall-mounted on
-- that edge. Several lights may share the same (x, y, mount) — the UI renders
-- them as a cluster with a count badge.
CREATE TABLE IF NOT EXISTS plan_lights (
    plan_id     TEXT NOT NULL REFERENCES floor_plans(id) ON DELETE CASCADE,
    light_id    TEXT NOT NULL REFERENCES lights(id) ON DELETE CASCADE,
    x           INTEGER NOT NULL,
    y           INTEGER NOT NULL,
    mount       TEXT NOT NULL DEFAULT 'c' CHECK (mount IN ('c', 'n', 's', 'e', 'w')),
    PRIMARY KEY (plan_id, light_id)
);
