-- Optional per-device glyph override. By default a device's glyph is derived
-- from its type/kind (a bulb for lights, a switch/plug/fan for power, a speaker
-- for audio); this lets the user pin a different one — e.g. a switch that drives
-- an LED strip can show the led_strip glyph. NULL = use the type default.
ALTER TABLE lights        ADD COLUMN glyph TEXT;
ALTER TABLE audio_devices ADD COLUMN glyph TEXT;
ALTER TABLE power_devices ADD COLUMN glyph TEXT;
