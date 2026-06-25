-- Colour palettes — named, light-agnostic colour sets imported from a provider's
-- stored scenes (today: Hue scenes). Unlike a Bifrost scene (per-light state
-- snapshot), a palette is just an ordered list of colours that can be
-- *distributed* across any room's lights, so it is reusable beyond the lights it
-- was authored against.
CREATE TABLE IF NOT EXISTS palettes (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    -- Where it came from, e.g. "hue". Pairs with source_id for idempotent re-import.
    source      TEXT NOT NULL,
    -- The provider-native id of the source scene (Hue scene rid); NULL for a
    -- palette not tied to a provider object.
    source_id   TEXT,
    -- JSON array of PaletteColor: { "xy":[x,y]?, "mirek":u16?, "brightness":f32? }.
    colors      TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- One row per source scene, so re-importing updates in place instead of duplicating.
CREATE UNIQUE INDEX IF NOT EXISTS idx_palettes_source
    ON palettes (source, source_id);
