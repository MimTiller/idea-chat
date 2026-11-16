-- Per-role allow and deny masks on a channel, applied to a member's base
-- set at that channel: widen by the allows, then cut by the denies. A row
-- follows its channel and its role, so deleting either leaves no orphan.

CREATE TABLE channel_overwrites (
    channel_id uuid NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
    space_id   uuid NOT NULL,
    role_id    uuid NOT NULL,
    allow      bigint NOT NULL DEFAULT 0,
    deny       bigint NOT NULL DEFAULT 0,
    PRIMARY KEY (channel_id, role_id)
);
CREATE INDEX channel_overwrites_channel ON channel_overwrites (channel_id);

ALTER TABLE channel_overwrites ADD FOREIGN KEY (space_id, role_id) REFERENCES space_roles (space_id, role_id) ON DELETE CASCADE;
