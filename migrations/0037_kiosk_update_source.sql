-- Where the hub pulls kiosk APKs from, configurable from the dashboard (the
-- "master" admin client) rather than server env. Defaults to the canonical repo
-- + the model-bundled release asset; `api::kiosk_update` reads these per request.
ALTER TABLE config ADD COLUMN kiosk_update_repo TEXT NOT NULL DEFAULT 'others-git/bifrost-kiosk';
ALTER TABLE config ADD COLUMN kiosk_update_asset TEXT NOT NULL DEFAULT 'app-release.apk';
