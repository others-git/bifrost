-- Per-kiosk scheduled quiet hours (display power saving). When enabled, the
-- kiosk scheduler puts the display to sleep at `sleep_at` and wakes it at
-- `wake_at`, using the **server's local time**. Times are "HH:MM" (24h).
--
-- The window wraps past midnight (sleep_at > wake_at is the common case, e.g.
-- sleep 23:00 → wake 07:00). The scheduler is **edge-triggered**: it emits the
-- existing sleep/wake commands only when the desired state changes, so a manual
-- wake during the quiet window is respected until the next boundary.
ALTER TABLE kiosks ADD COLUMN schedule_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE kiosks ADD COLUMN sleep_at TEXT; -- "HH:MM" local, display off
ALTER TABLE kiosks ADD COLUMN wake_at  TEXT; -- "HH:MM" local, display on
