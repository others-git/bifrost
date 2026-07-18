-- Kiosk "aware override": while any configured device is on, an Aware hour
-- treats the room as occupied regardless of actual presence — e.g. "don't let
-- the screen sleep from a no-motion timeout while the TV is playing". Stored
-- as the same {domain,id} shape as a room's quick-control targets.
ALTER TABLE kiosks ADD COLUMN aware_override_targets TEXT;
