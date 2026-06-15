-- M26: composite (merged) devices.
--
-- One physical device can surface as several Bifrost audio entities with
-- *complementary* capabilities — e.g. a Sony BRAVIA's two HA `media_player`s:
-- one carries now-playing + the receiver binding, the other the apps/remote +
-- the right kind. Shadow-dedup (`shadowed_by`) is wrong here: it *hides* the
-- secondary, losing its capabilities. A **companion** link instead MERGES the
-- secondary into a PRIMARY — the companion is hidden as its own control card,
-- but its state and controls are routed/overlaid onto the primary, so nothing
-- is lost (the union of capabilities lives on one device).
--
--   companion_of  The id of the PRIMARY audio device this entity merges into.
--                 NULL = a standalone device. The primary is a normal row that
--                 other rows point at; it carries no marker itself.
--
-- Distinct from `shadowed_by` (lossy hide, native-vs-integration equivalents).
-- A device is never both shadowed and a companion.

ALTER TABLE audio_devices ADD COLUMN companion_of TEXT;
