-- Debug screenshots pulled from a kiosk's display: the controller sends the
-- "screenshot" command, the kiosk app captures its WebView and uploads the
-- image here (latest wins — this is a live debugging surface, not history).
ALTER TABLE kiosks ADD COLUMN screenshot BLOB;
ALTER TABLE kiosks ADD COLUMN screenshot_mime TEXT;
ALTER TABLE kiosks ADD COLUMN screenshot_at TEXT;
