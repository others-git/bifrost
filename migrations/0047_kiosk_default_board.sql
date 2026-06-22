-- A kiosk can auto-launch a specific board, full-screen, on load. Configured
-- per-kiosk from a main (non-kiosk) client; null = no auto-launch. References a
-- dashboards.id; on board delete the value simply dangles (handled gracefully).
ALTER TABLE kiosks ADD COLUMN default_board_id TEXT;
