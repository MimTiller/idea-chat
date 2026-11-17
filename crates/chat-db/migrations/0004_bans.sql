-- Ban and timeout. A ban keeps a member's seat closed: they are taken out
-- of the space but the ban is recorded, so redeeming an invite again is
-- refused until unban. A timeout sits on the member row and expires by
-- itself; a timed-out member cannot send or speak until it passes.

CREATE TABLE space_bans (
    space_id uuid NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    user_id  uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    banned_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (space_id, user_id)
);
CREATE INDEX space_bans_user ON space_bans (user_id);

ALTER TABLE space_members ADD COLUMN timeout_ends timestamptz NULL;
