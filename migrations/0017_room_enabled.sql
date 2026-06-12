-- A room can be disabled to hide it from the UI (Dashboard, Floor Plan, …) and
-- the public API, without deleting it. Disabled rooms remain in Settings so they
-- can be re-enabled. Defaults to enabled.
ALTER TABLE rooms ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
