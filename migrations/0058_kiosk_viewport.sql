-- The kiosk's own CSS viewport (window.innerWidth × innerHeight), reported by
-- the web client when it loads inside a kiosk WebView. Feeds the Boards
-- preview: designing a board on a desktop, you can lock the canvas to a real
-- device's exact pixel size instead of guessing from marketing specs.
ALTER TABLE kiosks ADD COLUMN viewport_w INTEGER;
ALTER TABLE kiosks ADD COLUMN viewport_h INTEGER;
