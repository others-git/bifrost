-- Kiosk microphone presence: the wall tablet's always-on mic becomes a room
-- occupancy sensor. The app computes sound LEVEL on-device (no audio leaves the
-- tablet) against an adaptive ambient baseline and reports elevated/quiet edges;
-- the hub surfaces each mic-enabled kiosk as a real sensor_devices row (kind
-- 'occupancy') under the internal 'kiosk' pseudo-provider.
ALTER TABLE kiosks ADD COLUMN mic_presence INTEGER NOT NULL DEFAULT 0;
ALTER TABLE kiosks ADD COLUMN mic_sensitivity TEXT;  -- low | medium | high (null = medium)
ALTER TABLE kiosks ADD COLUMN mic_level REAL;        -- last reported level (dBFS), telemetry
