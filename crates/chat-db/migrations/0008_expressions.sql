-- Per-space custom emoji and stickers: named images a space shows for :name:
-- typed in a body, and images a member sends in place of a message body.
-- 0006 and 0007 belong to the roles-defaults branch, so this takes 0008.

CREATE TABLE space_emoji (
    space_id    uuid NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    emoji_id    uuid NOT NULL UNIQUE,
    name        text NOT NULL,
    -- The image the emoji shows; deleting the emoji releases it.
    upload_id   uuid NOT NULL REFERENCES attachments (id) ON DELETE CASCADE,
    created_by  uuid NOT NULL REFERENCES users (id),
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (space_id, emoji_id),
    -- An emoji name is unique in a space, so :name: means one picture.
    CONSTRAINT space_emoji_name UNIQUE (space_id, name)
);
CREATE INDEX space_emoji_space ON space_emoji (space_id);

CREATE TABLE space_stickers (
    space_id        uuid NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    sticker_id      uuid NOT NULL UNIQUE,
    name            text NOT NULL,
    description     text NOT NULL DEFAULT '',
    -- The image the sticker shows; deleting the sticker releases it.
    upload_id       uuid NOT NULL REFERENCES attachments (id) ON DELETE CASCADE,
    created_by      uuid NOT NULL REFERENCES users (id),
    created_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (space_id, sticker_id),
    CONSTRAINT space_stickers_name UNIQUE (space_id, name)
);
CREATE INDEX space_stickers_space ON space_stickers (space_id);

-- The uploads an emoji or sticker claims are of their own purposes, so the
-- sweep — which only takes unclaimed message uploads — never reaches them.
ALTER TABLE attachments DROP CONSTRAINT attachments_purpose_check;
ALTER TABLE attachments ADD CONSTRAINT attachments_purpose CHECK (purpose IN ('message', 'avatar', 'icon', 'emoji', 'sticker'));

-- A message may carry one of its space's stickers in place of a body;
-- deleting the sticker leaves the message with its sticker dropped (SET
-- NULL: the message stays, the picture is gone).
ALTER TABLE messages ADD COLUMN sticker uuid NULL REFERENCES space_stickers (sticker_id) ON DELETE SET NULL;
