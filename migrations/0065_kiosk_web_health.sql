-- The kiosk's WEB layer reporting on itself.
--
-- A wall tablet fails in three layers and, until now, the hub could only see
-- two of them: the native app's check-in proves the DEVICE is alive on the
-- network, and the `/api/events` subscriber registry proves the STREAM is. The
-- layer between — is the WebView's page still running, and does it believe its
-- own stream is healthy? — was visible only as a badge on the tablet's screen,
-- which means walking to the tablet, and a reload to read it destroys the
-- evidence.
--
-- These columns are that middle layer, posted by the page itself. They are
-- deliberately reachable with only the `bfr_key` cookie, so the report survives
-- the failure it reports: a lapsed session is precisely when the page most
-- needs to be able to say so.
--
-- `web_seen_at` ages on its own if the page stops posting (a frozen or crashed
-- WebView), which distinguishes "the page is gone" from "the page is running
-- and its stream is refused" — the two cases that look identical from the hub.
ALTER TABLE kiosks ADD COLUMN web_seen_at TEXT;
-- ms since the page last received a device/inventory event, and a heartbeat.
ALTER TABLE kiosks ADD COLUMN web_since_event_ms INTEGER;
ALTER TABLE kiosks ADD COLUMN web_since_beat_ms INTEGER;
-- EventSource (re)connection ATTEMPTS since the page loaded. Climbing with no
-- events = the stream keeps being refused or dropped.
ALTER TABLE kiosks ADD COLUMN web_reconnects INTEGER;
-- EventSource.readyState: 0 connecting, 1 open, 2 closed, -1 no object.
ALTER TABLE kiosks ADD COLUMN web_ready_state INTEGER;
-- How long this page has been loaded. A WebView that has been up for weeks is
-- the shape of every failure that only a reload has ever fixed.
ALTER TABLE kiosks ADD COLUMN web_page_age_ms INTEGER;
