-- M22: bind a source audio device (a TV / streamer / console) to an AV receiver.
-- Real AV: N sources feed audio *through* a receiver, which is the actual volume
-- authority. These columns record that binding, stored on the *source* device
-- (many sources may point at one receiver — a many-to-one relationship):
--
--   receiver_id      The audio_devices.id of the receiver this source's volume/
--                    mute is routed to. NULL = unbound (the device controls its
--                    own volume, as before). A dangling id (receiver deleted) is
--                    treated as unbound at control time.
--   receiver_source  The receiver input to select when this source becomes active
--                    (e.g. "BD/DVD", "Game"). NULL = don't switch the input.
ALTER TABLE audio_devices ADD COLUMN receiver_id TEXT;
ALTER TABLE audio_devices ADD COLUMN receiver_source TEXT;
