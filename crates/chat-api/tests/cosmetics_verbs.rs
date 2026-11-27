//! What `set_cosmetics` decides before it touches the db. As in the other
//! verbs tests, a `#[server]` verb cannot be called from a test — no session
//! wire, no `server` feature — so its per-argument decision is factored into
//! the pure [`chat_model::text::cosmetic_update`] and covered directly: `None`
//! keeps a field, an empty string clears it, anything else is the id after
//! validation. What the db does with those decisions, and what a second
//! person sees when they read the user, is covered in `chat-db/tests/db.rs`.

use chat_model::text;

fn some(s: &str) -> Option<&str> {
    Some(s)
}

#[test]
fn an_argument_keeps_clears_or_sets() {
    // Left out: the field stays as stored.
    assert_eq!(text::cosmetic_update(None).unwrap(), None);
    // Empty string: clear it.
    assert_eq!(text::cosmetic_update(some("")).unwrap(), Some(None));
    // Anything else: the id.
    assert_eq!(text::cosmetic_update(some("sparkle")).unwrap(), Some(Some("sparkle".to_string())));
    // Setting one argument says nothing about the other, so the other is kept.
    let dec = text::cosmetic_update(some("sparkle")).unwrap();
    let eff = text::cosmetic_update(None).unwrap();
    assert_eq!((dec, eff), (Some(Some("sparkle".to_string())), None));
}

#[test]
fn a_bad_cosmetic_id_is_refused() {
    assert!(text::cosmetic_update(some("Sparkle")).is_err(), "an uppercase letter");
    assert!(text::cosmetic_update(some("dark glow")).is_err(), "a space");
    assert!(text::cosmetic_update(some(&"a".repeat(41))).is_err(), "41 characters");
    assert!(text::cosmetic_update(some("sparkle!")).is_err(), "punctuation");
    // The bounds and the alphabet as the verb states them.
    assert!(text::cosmetic_update(some("a")).is_ok(), "one character is a cosmetic id");
    assert!(text::cosmetic_update(some(&"a".repeat(40))).is_ok(), "40 characters is a cosmetic id");
    assert_eq!(text::cosmetic_id("wave-2_snake").unwrap(), "wave-2_snake");
}
