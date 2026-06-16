-- A kiosk can be assigned a Bifrost Room (its "location"). This becomes the
-- voice context room for commands spoken to that tablet — e.g. "turn on the
-- lights" on the bedroom kiosk resolves to the Bedroom. NULL = unassigned.
ALTER TABLE kiosks ADD COLUMN room_id TEXT REFERENCES rooms(id) ON DELETE SET NULL;
