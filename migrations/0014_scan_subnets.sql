-- Expanded-LAN auto-detect: extra private /24 subnets the HTTP-sweep providers
-- (WLED, Tasmota, Shelly) should scan in addition to the container's own
-- subnet. Comma-separated, e.g. "192.168.1.0/24,10.0.0.0/24". Empty = local
-- subnet only (the default). Lets auto-detect reach devices on a different LAN
-- when running bridged (unicast routes across subnets; broadcast does not).
ALTER TABLE config ADD COLUMN scan_subnets TEXT NOT NULL DEFAULT '';
