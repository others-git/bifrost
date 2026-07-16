-- Per-hour kiosk display plan: a 24-character string, one mode per local hour —
-- 'W' (awake: screen forced on), 'S' (asleep: screen forced off), 'A' (aware:
-- presence-controlled — wake on motion, screen off after the no-motion timer).
-- Replaces the two-policy combination of a sleep window + a presence toggle
-- with one paintable timeline; `schedule_enabled` becomes the plan's master
-- switch. NULL = no plan painted yet — the scheduler falls back to the legacy
-- sleep_at/wake_at + presence_enabled behaviour, so existing kiosks keep
-- working untouched until their plan is first saved.
ALTER TABLE kiosks ADD COLUMN hour_modes TEXT;
