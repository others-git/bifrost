-- Flat "logical device" groups: one cross-domain `group_id` replaces the two
-- directional composite links — media `companion_of` (a companion → its primary)
-- and remote `paired_media_id` (a remote → its media device). Members that share
-- a `group_id` are one composite; the representative ("surface") is *derived* at
-- read time (highest authority / kind), never stored, so it can't drift. De-dup
-- (`shadowed_by`) and the shared-receiver binding (`receiver_id`) are unchanged —
-- they're different relationships and stay as they are.

ALTER TABLE media_devices ADD COLUMN group_id TEXT;
ALTER TABLE remote_devices ADD COLUMN group_id TEXT;

-- Backfill media: every member of an existing companion cluster shares one group
-- id (the cluster root — the former primary's id). Only rows actually in a
-- cluster (a companion exists) get a group; true standalones stay NULL.
UPDATE media_devices
   SET group_id = COALESCE(companion_of, id)
 WHERE companion_of IS NOT NULL
    OR id IN (SELECT companion_of FROM media_devices WHERE companion_of IS NOT NULL);

-- A paired remote's media device might be a standalone (no cluster): give it a
-- singleton group (its own id) so the remote has a group to join.
UPDATE media_devices
   SET group_id = id
 WHERE group_id IS NULL
   AND id IN (SELECT paired_media_id FROM remote_devices WHERE paired_media_id IS NOT NULL);

-- Backfill remotes: a paired remote joins its media device's group.
UPDATE remote_devices
   SET group_id = (
       SELECT m.group_id FROM media_devices m WHERE m.id = remote_devices.paired_media_id
   )
 WHERE paired_media_id IS NOT NULL;

ALTER TABLE media_devices DROP COLUMN companion_of;
ALTER TABLE remote_devices DROP COLUMN paired_media_id;
