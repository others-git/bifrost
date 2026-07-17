-- Board backgrounds: a per-board background spec (JSON, opaque to the backend
-- like the widget layout — preset id / scrim / speed / upload marker), plus the
-- uploaded media itself (image or short video loop) stored inline with its mime.
ALTER TABLE dashboards ADD COLUMN background TEXT;
ALTER TABLE dashboards ADD COLUMN bg_media BLOB;
ALTER TABLE dashboards ADD COLUMN bg_mime TEXT;
