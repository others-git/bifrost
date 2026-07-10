-- The app catalog pulled from the TV (appControl.getApplicationList) carries a
-- vendor launch URI per app (package + activity). Store it so launching an app
-- that has never been foreground works with the exact token the TV expects.
ALTER TABLE remote_apps ADD COLUMN activity TEXT;
