-- Role mentions: a role knows whether `@RoleName` pings its holders, and a
-- message keeps the role ids its body named, beside its person mentions.
-- Nullable-with-default so older rows simply have none, and decode as such.
ALTER TABLE space_roles ADD COLUMN mentionable boolean NOT NULL DEFAULT false;

-- The seeded Moderator answers a ping, the seeded Admin does not. Matched by
-- name like the 0006 colours, so this holds for backfilled and new spaces.
UPDATE space_roles SET mentionable = true WHERE name = 'Moderator';

ALTER TABLE messages ADD COLUMN mention_roles uuid[] NOT NULL DEFAULT '{}';
CREATE INDEX messages_mention_roles ON messages USING gin (mention_roles);
