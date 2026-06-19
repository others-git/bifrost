-- Rename the "audio" device domain to "media": the domain holds TVs and
-- streamers as well as speakers/receivers (it mirrors HA's media_player), so
-- "audio" undersold it. SQLite auto-updates foreign-key references in other
-- tables when a table or column is renamed (3.25+, legacy_alter_table off).
ALTER TABLE audio_devices RENAME TO media_devices;
ALTER TABLE room_audio_devices RENAME TO room_media_devices;
ALTER TABLE provider_group_audio_devices RENAME TO provider_group_media_devices;
ALTER TABLE plan_audio_devices RENAME TO plan_media_devices;

ALTER TABLE room_media_devices RENAME COLUMN audio_device_id TO media_device_id;
ALTER TABLE provider_group_media_devices RENAME COLUMN audio_device_id TO media_device_id;
ALTER TABLE plan_media_devices RENAME COLUMN audio_device_id TO media_device_id;
ALTER TABLE remote_devices RENAME COLUMN paired_audio_id TO paired_media_id;
