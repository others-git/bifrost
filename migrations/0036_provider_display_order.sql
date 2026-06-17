-- User-controlled ordering of provider groups on the Devices page. Lower sorts
-- first; ties fall back to creation order (so existing installs keep their
-- current order until the user reorders). `PUT /api/providers/order` assigns
-- sequential values; a newly-added provider is given MAX+1 so it lands at the end.
ALTER TABLE providers ADD COLUMN display_order INTEGER NOT NULL DEFAULT 0;
