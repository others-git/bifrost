-- Friendly (user-overridable) device names. `name` is the display name shown
-- everywhere (rooms, scenes, voice, dashboard); `provider_name` keeps the raw
-- name the provider reported at discovery. A re-sync only overwrites `name` when
-- the user hasn't customised it (`name = provider_name`), so a friendly name
-- sticks; clearing the override reverts `name` back to `provider_name`.
ALTER TABLE lights ADD COLUMN provider_name TEXT;
ALTER TABLE media_devices ADD COLUMN provider_name TEXT;
ALTER TABLE power_devices ADD COLUMN provider_name TEXT;

UPDATE lights SET provider_name = name WHERE provider_name IS NULL;
UPDATE media_devices SET provider_name = name WHERE provider_name IS NULL;
UPDATE power_devices SET provider_name = name WHERE provider_name IS NULL;
