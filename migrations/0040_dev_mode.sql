-- Developer mode: a runtime switch that exposes contributor/dev-only surfaces
-- (provider debug, missing-capability reports, dev API) which a live deploy has
-- no use for. Off by default; toggled from Settings → Developer.
ALTER TABLE config ADD COLUMN dev_mode INTEGER NOT NULL DEFAULT 0;
