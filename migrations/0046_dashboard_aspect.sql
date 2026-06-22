-- A board carries a fixed aspect ratio (e.g. "16:9") chosen at creation, so its
-- canvas fits a screen of that shape and a layout is device-independent. Existing
-- boards default to 16:9.
ALTER TABLE dashboards ADD COLUMN aspect TEXT NOT NULL DEFAULT '16:9';
