-- Custom roles: named permission sets per space, and members holding them.
-- The fixed per-space rank column and the per-channel rank floors of
-- 0001_init.sql do not survive; they go here as replacements, with their
-- information backfilled before the old columns go.

CREATE TABLE space_roles (
    space_id     uuid NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    role_id      uuid NOT NULL,
    name         text NOT NULL,
    permissions  bigint NOT NULL DEFAULT 0,
    position     integer NOT NULL DEFAULT 0,
    PRIMARY KEY (space_id, role_id)
);
CREATE INDEX space_roles_space ON space_roles (space_id);

-- Backfill 1: each space that has admins gets one Admin role, whose id is
-- the space's own (one per space, so it cannot clash). Its permissions are
-- the admin tag, which expands to everything, so a backfilled admin keeps
-- the power they had under the old rank; position 0 keeps them senior to
-- any custom role made later.
INSERT INTO space_roles
    SELECT d.space_id, d.space_id, 'Admin', -9223372036854775808, 0
    FROM (SELECT DISTINCT space_id AS space_id FROM space_members WHERE role = 'admin') AS d;

ALTER TABLE space_members ADD COLUMN role_ids uuid[] NOT NULL DEFAULT '{}';

UPDATE space_members AS m
    SET role_ids = ARRAY[r.role_id]
    FROM space_roles AS r
    WHERE m.role = 'admin' AND r.space_id = m.space_id;

ALTER TABLE space_members DROP COLUMN role;

-- Backfill 2: each channel's old view/post floor becomes a mask. member
-- asks nothing; admin asks the admin tag, which perms expands to every
-- bit, so a channel locked to admins keeps locking out plain members;
-- owner has no bit of its own and the space's owner holds everything, so
-- the old owner-only floor (which could only mean "nobody but the owner")
-- reads as the same admin lock.
ALTER TABLE channels ADD COLUMN view_mask bigint NOT NULL DEFAULT 0;
ALTER TABLE channels ADD COLUMN post_mask bigint NOT NULL DEFAULT 0;

UPDATE channels
    SET view_mask = CASE view_role WHEN 'member' THEN 0 ELSE -9223372036854775808 END;
UPDATE channels
    SET post_mask = CASE post_role WHEN 'member' THEN 0 ELSE -9223372036854775808 END;

ALTER TABLE channels DROP COLUMN view_role;
ALTER TABLE channels DROP COLUMN post_role;
