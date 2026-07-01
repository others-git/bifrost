-- Presence-driven display (kiosk power saving): when enabled, the kiosk
-- scheduler blanks the display while the kiosk's assigned Room is unoccupied and
-- wakes it when a presence sensor (motion/occupancy) in that room detects
-- someone. Occupancy is resolved provider-agnostically from the sensor domain
-- (rooms::room_occupancy), so a Hue motion sensor or an HA binary_sensor drive it
-- identically. Uses the kiosk's existing room_id — no separate binding.
--
-- `presence_timeout_secs` is the no-motion grace before sleeping: the display
-- stays awake for this long after the last detection, so a brief still moment
-- doesn't blank the screen mid-use.
ALTER TABLE kiosks ADD COLUMN presence_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE kiosks ADD COLUMN presence_timeout_secs INTEGER NOT NULL DEFAULT 600;
