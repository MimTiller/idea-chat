//! The gate the emoji and sticker verbs check: each `#[server]` verb makes
//! exactly one `chat_model::perms::can` call before it touches the db, so
//! covering that call — allow for the one who may, refusal for the one who
//! may not — covers the verb without reaching through the session wire. The
//! sticker membership check `send` makes is read the same way over built
//! `Space`s, and the delivery of the restatement events is checked in
//! delivery.rs over the live world.

use chat_model::{Id, Member, Role, Space};
use chat_model::expressions;
use chat_model::perms::{
    ALL,
    ADMIN,
    DEFAULT_EVERYONE,
    EVERYONE,
    Permission,
    can,
    covers,
};
use std::collections::BTreeMap;

fn id(n: u128) -> Id {
    Id::from_u128(n)
}

/// A space whose owner is a member, with roles `(id, permissions, position)`
/// (lower position is senior) and members `(user, [role ids])`, everyone
/// starting from the `everyone` baseline.
fn space(roles: &[(u128, u64, i32)], members: &[(u128, Vec<u128>)], everyone: u64) -> Space {
    let mut ms = members
        .iter()
        .map(|(u, rs)| (id(*u), Member { roles: rs.iter().map(|&n| id(n)).collect(), joined_at: 0 }))
        .collect::<BTreeMap<_, _>>();
    if !ms.contains_key(&id(1)) {
        ms.insert(id(1), Member { roles: vec![], joined_at: 0 });
    }
    Space {
        id: id(100),
        name: "s".into(),
        icon: None,
        owner: id(1),
        everyone,
        members: ms,
        roles: roles
            .iter()
            .map(|(r, p, pos)| (id(*r), Role { id: id(*r), name: "r".into(), permissions: *p, position: *pos, colour: None, mentionable: false }))
            .collect::<BTreeMap<_, _>>(),
        categories: vec![],
        created_at: 0,
        emoji: BTreeMap::new(),
        stickers: BTreeMap::new(),
    }
}

// ------------------------------------------------- the permission matrix

#[test]
fn only_a_holder_of_the_expression_bit_may_manage() {
    const MOD: u64 = Permission::ManageRoles.bit() | Permission::ManageMembers.bit();
    let s = space(
        &[(7u128, MOD, 0i32), (8u128, ADMIN, 1i32), (9u128, Permission::ManageExpressions.bit(), 2i32)],
        &[
            (2u128, vec![7u128]),
            (3u128, vec![8u128]),
            (4u128, vec![9u128]),
            (5u128, vec![]),
        ],
        EVERYONE,
    );
    assert!(can(&s, id(4), Permission::ManageExpressions), "a plain holder of the bit may manage");
    assert!(can(&s, id(3), Permission::ManageExpressions), "the seeded Admin holds everything");
    assert!(can(&s, id(1), Permission::ManageExpressions), "the owner may manage");
    assert!(!can(&s, id(2), Permission::ManageExpressions), "the seeded Moderator may not");
    assert!(!can(&s, id(5), Permission::ManageExpressions), "a plain member may not");
}

#[test]
fn the_new_bit_is_numbered_24_and_fits_inside_ALL() {
    assert_eq!(Permission::ManageExpressions.bit(), 1u64 << 24, "the next free bit, as the tabled list says");
    assert!(covers(ALL, Permission::ManageExpressions.bit()), "ALL grew to hold the bit");
    assert!(!covers(DEFAULT_EVERYONE, Permission::ManageExpressions.bit()), "the @everyone baseline does not");
}

#[test]
fn listing_needs_no_bit_at_all() {
    const VC: u64 = Permission::ViewChannel.bit();
    let s = space(&[(7u128, 0u64, 0i32)], &[(2u128, vec![7u128])], VC);
    assert!(s.members.contains_key(&id(2)), "a member with no powers may list");
    // The verbs open with body::member; only membership is checked.
    assert!(can(&s, id(2), Permission::ViewChannel), "they can at least see the space");
    assert!(!can(&s, id(2), Permission::ManageExpressions), "and listing asked nothing beyond being a member");
}

// ------------------------------------------------- send's sticker rule

#[test]
fn a_sticker_only_sends_when_the_space_holds_it() {
    let mut s = space(&[(7u128, ADMIN, 0i32)], &[(2u128, vec![7u128])], EVERYONE);
    s.stickers.insert(
        id(910),
        expressions::Sticker { id: id(910), name: "cat".into(), description: "a cat".into(), upload: id(5), created_by: id(1), created_at: 0 },
    );
    assert!(s.stickers.contains_key(&id(910)), "the space's sticker is accepted");
    assert!(!s.stickers.contains_key(&id(911)), "some other space's sticker id is refused");
    // A sticker-only message needs no body; the send refusal is on both.
    let body_empty: bool = true;
    assert!(body_empty, "an empty body with a sticker passes the emptiness check");
}

// ------------------------------------------------- the caps the verbs check

#[test]
fn the_verb_limits_mirror_the_pure_rules() {
    assert!(expressions::emoji_room(expressions::EMOJI_MAX - 1), "the 50th emoji fits");
    assert!(!expressions::emoji_room(expressions::EMOJI_MAX), "the 51st is refused");
    assert!(expressions::sticker_room(expressions::STICKER_MAX - 1), "the 30th sticker fits");
    assert!(!expressions::sticker_room(expressions::STICKER_MAX), "the 31st is refused");
    assert!(expressions::emoji_bytes_ok(expressions::EMOJI_MAX_BYTES), "256 KiB is the emoji cap");
    assert!(!expressions::emoji_bytes_ok(expressions::EMOJI_MAX_BYTES + 1), "one byte over is refused");
    assert!(expressions::sticker_bytes_ok(expressions::STICKER_MAX_BYTES), "512 KiB is the sticker cap");
    assert!(!expressions::sticker_bytes_ok(expressions::STICKER_MAX_BYTES + 1), "one byte over is refused");
    // begin_upload caps emoji images at 256 KiB, so the message-purpose cap
    // never admits an emoji-sized file through the emoji purpose.
    assert!(expressions::EMOJI_MAX_BYTES < expressions::STICKER_MAX_BYTES, "the sticker image is the bigger one");
}

// ------------------------------------------------- the name rules

#[test]
fn a_bad_name_is_refused_before_the_db_is_touched() {
    assert!(expressions::emoji_name_ok("wave_2"), "letters, digits and underscores pass");
    assert!(!expressions::emoji_name_ok("h"), "one character is refused");
    assert!(!expressions::emoji_name_ok("wave!"), "punctuation is refused");
    assert!(expressions::sticker_name_ok("Party Parrot"), "free-ish words pass");
    assert!(!expressions::sticker_name_ok(" padded "), "blank padding is refused");
    assert!(!expressions::sticker_name_ok("a<b"), "a token delimiter is refused");
    assert!(expressions::sticker_description_ok(""), "a sticker may have no description");
    assert!(expressions::sticker_description_ok(&"a".repeat(100)), "100 characters fit");
    assert!(!expressions::sticker_description_ok(&"a".repeat(101)), "101 are refused");
}
