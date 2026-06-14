-- Per-provider "prune stale devices" preference. When set, a discover removes
-- devices the provider no longer reports (e.g. HA entities filtered out as
-- config sub-controls), instead of leaving them as orphans. Off by default —
-- discovery stays additive unless the user opts in (or forces it via
-- Prune+Sync). Mirrors the existing `providers.enabled` flag.
ALTER TABLE providers ADD COLUMN prune INTEGER NOT NULL DEFAULT 0;
