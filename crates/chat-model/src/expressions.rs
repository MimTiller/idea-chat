//! The space's custom emoji and stickers: what they are, and the rules for
//! what may be made. The acts that need a permission gate on those bits live
//! in the API; the rules here are pure, so both sides can check them.

use crate::{Id, Millis};

use serde::{Deserialize, Serialize};

/// A custom emoji: one image the space shows whenever its name is typed
/// between colons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Emoji {
    pub id: Id,
    /// Unique in the space, typed as `:name:`.
    pub name: String,
    /// The upload's id; its image is fetched at the upload's url for the
    /// emoji's life.
    pub upload: Id,
    pub created_by: Id,
    pub created_at: Millis,
}

/// A sticker: one image a member sends in place of a message body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sticker {
    pub id: Id,
    /// Unique in the space; free-ish text within [`sticker_name_ok`].
    pub name: String,
    /// What the space says it shows, shown beside the sticker; may be empty.
    pub description: String,
    pub upload: Id,
    pub created_by: Id,
    pub created_at: Millis,
}

/// Custom emoji per space.
pub const EMOJI_MAX: usize = 50;
/// An emoji image's bytes.
pub const EMOJI_MAX_BYTES: usize = 256 * 1024;
/// Stickers per space.
pub const STICKER_MAX: usize = 30;
/// A sticker image's bytes.
pub const STICKER_MAX_BYTES: usize = 512 * 1024;
/// A sticker description's characters.
pub const STICKER_DESCRIPTION_MAX: usize = 100;

/// Whether `name` is a legal custom emoji name: 2–32 characters of letters,
/// digits and `_`, so it round-trips as `:name:` in a message body.
pub fn emoji_name_ok(name: &str) -> bool {
    (2..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Whether `name` is a legal sticker name: 2–30 characters, no control
/// characters and none of `<`, `>` or `:` — those are the message token
/// delimiters — and not blank-padded.
pub fn sticker_name_ok(name: &str) -> bool {
    let chars = name.chars().count();
    (2..=30).contains(&chars)
        && name == name.trim()
        && name.chars().all(|c| !c.is_control() && !"<>:".contains(c))
}

/// Whether `description` is a legal sticker description: free text up to
/// 100 characters, as the space typed it.
pub fn sticker_description_ok(description: &str) -> bool {
    description.chars().count() <= STICKER_DESCRIPTION_MAX
}

/// How many emoji/stickers a space may still hold, and the size checks for a
/// new image: false once the space is at its limit or the bytes are over.
pub fn emoji_room(count: usize) -> bool {
    count < EMOJI_MAX
}

pub fn sticker_room(count: usize) -> bool {
    count < STICKER_MAX
}

pub fn emoji_bytes_ok(size: usize) -> bool {
    size > 0 && size <= EMOJI_MAX_BYTES
}

pub fn sticker_bytes_ok(size: usize) -> bool {
    size > 0 && size <= STICKER_MAX_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_and_sticker_rules_are_checked() {
        assert!(emoji_name_ok("hi"), "the shortest emoji name");
        assert!(emoji_name_ok("A_z9"), "letters, digits and underscores");
        assert!(!emoji_name_ok("h"), "one character is too short");
        assert!(!emoji_name_ok("has a space"), "spaces are not in the alphabet");
        assert!(!emoji_name_ok("has:colon"), "the token delimiter would break :name:");
        assert!(!emoji_name_ok("wayyyyyyyyyyyyyyyyyyyyyyyyyyyyyyytoolong"), "33 characters are too many");
        assert!(sticker_name_ok("Party Parrot"), "free-ish text stickers carry words");
        assert!(sticker_name_ok("hi"), "the shortest sticker name");
        assert!(!sticker_name_ok(" padded "), "blank padding is not kept");
        assert!(!sticker_name_ok("bad<name"), "a token delimiter would break the body");
        assert!(!sticker_name_ok("x"), "one character is too short");
        assert!(!sticker_name_ok("the name is thirty-one characters long?"), "31 characters are too many");
        assert!(sticker_description_ok(""), "a sticker may have no description");
        assert!(sticker_description_ok("🎉 a party!"), "free text within the limit");
        assert!(!sticker_description_ok(&"x".repeat(101)), "101 characters are one over");
        assert!(emoji_room(EMOJI_MAX - 1), "the 50th emoji fits");
        assert!(!emoji_room(EMOJI_MAX), "the 51st does not");
        assert!(sticker_room(STICKER_MAX - 1), "the 30th sticker fits");
        assert!(!sticker_room(STICKER_MAX), "the 31st does not");
        assert!(emoji_bytes_ok(EMOJI_MAX_BYTES), "256 KiB is the limit");
        assert!(!emoji_bytes_ok(EMOJI_MAX_BYTES + 1), "one byte over is refused");
        assert!(sticker_bytes_ok(STICKER_MAX_BYTES), "512 KiB is the sticker limit");
        assert!(!sticker_bytes_ok(STICKER_MAX_BYTES + 1), "one byte over is refused");
    }
}
