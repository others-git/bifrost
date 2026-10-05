-- The hub's clock: an IANA time zone for every time-of-day feature (kiosk hour
-- plans, automation timers and time gates, board clocks) and the NTP servers
-- the in-process SNTP client corrects the wall clock against. An empty
-- `timezone` defers to the process environment (`TZ` / the container's
-- /etc/localtime); an empty `ntp_servers` means the built-in default pool.
ALTER TABLE config ADD COLUMN timezone TEXT NOT NULL DEFAULT '';
ALTER TABLE config ADD COLUMN ntp_enabled INTEGER NOT NULL DEFAULT 1;
ALTER TABLE config ADD COLUMN ntp_servers TEXT NOT NULL DEFAULT '';
