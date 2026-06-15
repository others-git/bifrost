-- Apps seen / pinned on a remote's TV. HA's Android TV Remote gives no installed-
-- app list (activity_list is empty), only the *current* foreground app, so
-- Bifrost builds the launchable-app set itself: every package observed via
-- current_activity is recorded as a "recent", and the user can pin favourites.
-- The BifrostRemote shows pinned ∪ recent as launch buttons.
--
-- `package` is the Play Store id (e.g. com.netflix.ninja). `name` is a friendly
-- label (from the package→name registry, falling back to the package).
CREATE TABLE remote_apps (
    remote_id  TEXT NOT NULL REFERENCES remote_devices(id) ON DELETE CASCADE,
    package    TEXT NOT NULL,
    name       TEXT NOT NULL,
    pinned     INTEGER NOT NULL DEFAULT 0,
    last_seen  TEXT,
    PRIMARY KEY (remote_id, package)
);
