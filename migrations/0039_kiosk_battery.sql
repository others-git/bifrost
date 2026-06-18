-- Battery / power telemetry reported by the kiosk heartbeat (M29+). All nullable:
-- older app versions don't send them, and a desktop "kiosk" has no battery.
ALTER TABLE kiosks ADD COLUMN battery_level      INTEGER; -- 0-100 (%)
ALTER TABLE kiosks ADD COLUMN battery_charging   INTEGER; -- 0/1 (on a charger)
ALTER TABLE kiosks ADD COLUMN battery_voltage_mv INTEGER; -- millivolts
ALTER TABLE kiosks ADD COLUMN battery_current_ua INTEGER; -- microamps, signed (+ = into battery)
ALTER TABLE kiosks ADD COLUMN battery_temp_dc    INTEGER; -- deci-celsius (245 = 24.5°C)
ALTER TABLE kiosks ADD COLUMN power_source       TEXT;    -- ac | usb | wireless | none
