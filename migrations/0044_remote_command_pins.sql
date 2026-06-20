-- Pinned native ("Full remote") commands per remote — the user's favourites,
-- surfaced above the expanded command catalogue (the keypad's full IRCC / native
-- key grid). The catalogue itself is fetched live from the provider, so only the
-- opaque token is stored here; a row's presence *is* the pin.
CREATE TABLE remote_command_pins (
    remote_id TEXT NOT NULL REFERENCES remote_devices(id) ON DELETE CASCADE,
    token     TEXT NOT NULL,
    PRIMARY KEY (remote_id, token)
);
