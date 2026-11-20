-- Each person's notification level and mute expiry per space and per channel,
-- beside the plain mutes rows. level NULL = inherit (a channel from its space);
-- muted_until NULL = not muted, -1 = until turned off, else an epoch-ms
-- deadline (see chat-model's notification rule). 0008 is reserved on the
-- `expressions` branch, so this is 0009.
-- A row's target is a space or a channel, so it cannot carry a single
-- foreign key; deleting a person still takes their rows with it.
CREATE TABLE notification_settings (
    user_id     uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    scope       text NOT NULL CHECK (scope IN ('space', 'channel')),
    target_id   uuid NOT NULL,
    level       text CHECK (level IN ('all', 'mentions', 'nothing')),
    muted_until bigint,
    PRIMARY KEY (user_id, target_id)
);
