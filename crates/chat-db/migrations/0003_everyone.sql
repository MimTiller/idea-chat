-- The space's everyone baseline: what every member holds whatever their
-- roles say, so an owner can retune it per space. The default is
-- chat_model::perms::DEFAULT_EVERYONE spelled out: see, post, edit and
-- delete own, react, attach, embed, and voice connect/speak/video/share —
-- bits 0,1,2,3,6,8,9,10,11,12,13.

ALTER TABLE spaces ADD COLUMN everyone bigint NOT NULL DEFAULT 16207;
