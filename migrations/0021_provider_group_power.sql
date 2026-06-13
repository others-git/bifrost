-- Power members of a provider group (an HA Area, mirrored). The power-domain
-- analog of provider_group_lights / provider_group_audio_devices, so a synced
-- Area that contains switches/plugs/fans carries them into the Bifrost Room it
-- links. Lets an Area with no lights still sync (the common HA case).
CREATE TABLE provider_group_power_devices (
    provider_group_id TEXT NOT NULL REFERENCES provider_groups(id) ON DELETE CASCADE,
    power_device_id   TEXT NOT NULL REFERENCES power_devices(id) ON DELETE CASCADE,
    PRIMARY KEY (provider_group_id, power_device_id)
);
