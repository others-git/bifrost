-- Cross-provider device de-duplication. A physical device reachable both
-- natively (Hue, Govee, Sonos, Onkyo) and via an integration (Home Assistant)
-- imports as two rows; these columns let Bifrost recognise and collapse them.
--
--   hw_id        Normalized hardware identity (currently `mac:<hex>`), populated
--                by providers that expose one. NULL = unknown (never matched).
--   shadowed_by  The id of the *canonical* device this row defers to. A shadowed
--                device stays tracked but drops out of control surfaces and room
--                membership (like `enabled = 0`), and the Devices page collapses
--                it under the canonical one. NULL = visible.
--   shadow_auto  1 = the shadow was set automatically by hw_id reconciliation
--                (native wins over integration); 0 = a manual user link. The
--                reconciler only ever clears/sets `shadow_auto = 1` rows, so a
--                manual link is never clobbered.
ALTER TABLE lights        ADD COLUMN hw_id TEXT;
ALTER TABLE audio_devices ADD COLUMN hw_id TEXT;
ALTER TABLE power_devices ADD COLUMN hw_id TEXT;

ALTER TABLE lights        ADD COLUMN shadowed_by TEXT;
ALTER TABLE audio_devices ADD COLUMN shadowed_by TEXT;
ALTER TABLE power_devices ADD COLUMN shadowed_by TEXT;

ALTER TABLE lights        ADD COLUMN shadow_auto INTEGER NOT NULL DEFAULT 0;
ALTER TABLE audio_devices ADD COLUMN shadow_auto INTEGER NOT NULL DEFAULT 0;
ALTER TABLE power_devices ADD COLUMN shadow_auto INTEGER NOT NULL DEFAULT 0;
