-- Remote devices: a virtual smart-remote for a TV / streamer (Android TV Remote
-- via Home Assistant today). Its own domain because its control surface — D-pad,
-- nav/media keys, app launch — is neither audio state nor on/off.
--
-- Mirrors power_devices, plus the de-dup/enable/glyph columns the other domains
-- carry from later migrations (added inline here since this table is new), and
-- `paired_audio_id`: the TV's audio device (media_player) that is the *same*
-- physical box, so the UI can offer the remote from the TV's control fly-out.
-- `last_state` is the RemoteState JSON ({"on":bool,"current_app":...}).
CREATE TABLE remote_devices (
    id              TEXT PRIMARY KEY,
    provider_id     TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    device_id       TEXT NOT NULL,
    name            TEXT NOT NULL,
    last_state      TEXT,
    last_seen       TEXT,
    enabled         INTEGER NOT NULL DEFAULT 1,
    glyph           TEXT,
    hw_id           TEXT,
    -- The paired TV audio device (audio_devices.id), set when a media_player
    -- shares this remote's hardware. Cleared if that device goes away.
    paired_audio_id TEXT REFERENCES audio_devices(id) ON DELETE SET NULL,
    UNIQUE (provider_id, device_id)
);

CREATE INDEX idx_remote_devices_hw_id ON remote_devices(hw_id);
