-- Audio analog of provider_group_lights: members of a provider's room/zone
-- mirror that are audio devices (e.g. a Sonos room's speaker). Lets audio
-- providers' rooms/zones wrap into Bifrost Rooms via the same provider_groups
-- + room_links machinery used for lights.
CREATE TABLE IF NOT EXISTS provider_group_audio_devices (
    provider_group_id TEXT NOT NULL REFERENCES provider_groups(id) ON DELETE CASCADE,
    audio_device_id   TEXT NOT NULL REFERENCES audio_devices(id)   ON DELETE CASCADE,
    PRIMARY KEY (provider_group_id, audio_device_id)
);
