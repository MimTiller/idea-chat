-- Profile cosmetics: ids the app draws on a user's avatar and profile card.
-- Nullable; no default means none.
ALTER TABLE users ADD COLUMN avatar_decoration TEXT, ADD COLUMN profile_effect TEXT;
