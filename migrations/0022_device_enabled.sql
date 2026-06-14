-- A device can be "disabled": still tracked, still a member of its Bifrost
-- Room, but excluded from all control — no commands are sent to it and it
-- drops out of room control and the Control page. Re-enabling restores it.
--
-- Mirrors the existing `providers.enabled` / `rooms.enabled` pattern, one flag
-- per device domain.
ALTER TABLE lights        ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
ALTER TABLE audio_devices ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
ALTER TABLE power_devices ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
