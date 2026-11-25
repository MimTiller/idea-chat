-- Pins: a per-message toggle, like a reaction. One pin per message; the
-- pin vanishes when the message is deleted.
CREATE TABLE pins (
    message_id  uuid NOT NULL PRIMARY KEY REFERENCES messages (id) ON DELETE CASCADE,
    channel_id  uuid NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
    pinned_by   uuid NOT NULL REFERENCES users (id),
    pinned_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX pins_channel ON pins (channel_id, pinned_at DESC);
