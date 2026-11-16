-- Custom roles: named permission sets per space, and members holding them.
-- The fixed per-space rank column and the per-channel rank floors of
-- 0001_init.sql do not survive; they go here as replacements, not edits.

ALTER TABLE space_members DROP COLUMN role;
ALTER TABLE space_members ADD COLUMN role_ids uuid[] NOT NULL DEFAULT '{}';

ALTER TABLE channels DROP COLUMN view_role;
ALTER TABLE channels DROP COLUMN post_role;
ALTER TABLE channels ADD COLUMN view_mask bigint NOT NULL DEFAULT 0;
ALTER TABLE channels ADD COLUMN post_mask bigint NOT NULL DEFAULT 0;

CREATE TABLE space_roles (
    space_id     uuid NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    role_id      uuid NOT NULL,
    name         text NOT NULL,
    permissions  bigint NOT NULL DEFAULT 0,
    position     integer NOT NULL DEFAULT 0,
    PRIMARY KEY (space_id, role_id)
);
CREATE INDEX space_roles_space ON space_roles (space_id);
