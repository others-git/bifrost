-- User-composable dashboards ("Boards"): a named board holding a free-form grid of
-- widgets, dragged and resized by the user. The whole widget layout is stored as one
-- JSON array (atomic save); each widget carries its grid box (x,y,w,h), a
-- frontend-defined `type`, and an opaque `config` — so new widget types need no
-- schema change. `position` orders the boards in the picker.
CREATE TABLE dashboards (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    position   INTEGER NOT NULL DEFAULT 0,
    layout     TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
