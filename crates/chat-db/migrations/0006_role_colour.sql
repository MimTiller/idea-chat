-- A role's optional display colour: 0xRRGGBB, NULL for none. Nullable so
-- older rows simply have no colour.
ALTER TABLE space_roles ADD COLUMN colour bigint;

-- The 0002 backfill's Admin and the roles q144-v1's create_space seeds get
-- standing colours: Admin red, Moderator green. Matched by name so this
-- holds for both the backfilled spaces and the newly seeded ones.
UPDATE space_roles SET colour = 38023100 WHERE name = 'Admin';    -- 0xE74C3C
UPDATE space_roles SET colour = 49075985 WHERE name = 'Moderator'; -- 0x2ECC71
