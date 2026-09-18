-- Kiosk display-policy telemetry, reported on the 10s check-in.
--
-- A wall tablet that wakes to "swipe to unlock" is stuck behind the KEYGUARD,
-- and nothing above the app could see it: the hub knew the device checked in,
-- that its page was healthy and that its stream was live, while the panel sat
-- on a lock screen no one was standing in front of. These four say whether the
-- kiosk still holds the device-owner powers that suppress that lock screen.
--
-- All nullable: an app build older than the one that reports them never will,
-- and "unknown" must not read as "false".
ALTER TABLE kiosks ADD COLUMN device_owner INTEGER;       -- app is device owner
ALTER TABLE kiosks ADD COLUMN lock_task INTEGER;          -- lock-task (pinned) active
ALTER TABLE kiosks ADD COLUMN keyguard_disabled INTEGER;  -- setKeyguardDisabled(true) took
ALTER TABLE kiosks ADD COLUMN keyguard_locked INTEGER;    -- a keyguard is showing right now
