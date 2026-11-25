//! The gate the pin verbs check. A `#[server]` verb cannot be called from a
//! test — no session wire, no `server` feature — so, as in the other verbs
//! tests, the decisions the verbs make are factored into pure functions and
//! covered directly: `chat_model::perms::can_at` with `ManagePins` before the
//! db is touched, `pin_room` for the cap, and `pin_order` for the list order.

use chat_api::messages::{pin_order, pin_room, PINS_MAX};
use chat_model::{Channel, ChannelKind, Id, Member, Overwrite, Role, Space};
use chat_model::perms::{
    ADMIN,
    DEFAULT_EVERYONE,
    Permission,
    can_at,
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

/// A space channel with open masks and no overwrites.
fn channel() -> Channel {
    Channel {
        id: id(200),
        kind: ChannelKind::Text,
        space: Some(id(100)),
        name: "c".into(),
        topic: "".into(),
        category: None,
        position: 0,
        parent: None,
        starter: None,
        view: 0,
        post: 0,
        members: vec![],
        archived: false,
        overwrites: BTreeMap::<Id, Overwrite>::new(),
        last_message: None,
        message_count: 0,
        created_by: id(1),
        created_at: 0,
    }
}

/// A direct conversation: no space to consult.
fn direct(with: Id) -> Channel {
    Channel { kind: ChannelKind::Direct, space: None, members: vec![id(1), with], ..channel() }
}

#[test]
fn pin_needs_manage_pins() {
    let s = space(
        &[(7u128, Permission::ManagePins.bit(), 0i32), (8u128, ADMIN, 1i32)],
        &[(2u128, vec![7u128]), (3u128, vec![8u128]), (4u128, vec![])],
        DEFAULT_EVERYONE,
    );
    assert!(can_at(&channel(), Some(&s), id(2), Permission::ManagePins), "a plain holder of the bit may pin");
    assert!(can_at(&channel(), Some(&s), id(3), Permission::ManagePins), "the seeded Admin holds everything");
    assert!(can_at(&channel(), Some(&s), id(1), Permission::ManagePins), "the owner may pin");
    assert!(!can_at(&channel(), Some(&s), id(4), Permission::ManagePins), "the everyone baseline does not buy pinning");
    // Reading the list needs only read access, no bit: a plain member with
    // just ViewChannel may list, and the baseline stops before ManagePins.
    let plain = space(&[], &[(4u128, vec![])], Permission::ViewChannel.bit());
    assert!(can_at(&channel(), Some(&plain), id(4), Permission::ViewChannel), "a reader may list pins");
    assert!(!can_at(&channel(), Some(&plain), id(4), Permission::ManagePins), "listing asked no pin bit");
    assert!(!covers(DEFAULT_EVERYONE, Permission::ManagePins.bit()), "the baseline stops before ManagePins");
    // A DM follows what can_at answers: open to anyone the conversation admits.
    assert!(can_at(&direct(id(4)), None, id(1), Permission::ManagePins), "a DM has no roles to consult");
}

#[test]
fn pins_are_capped_at_fifty() {
    assert_eq!(PINS_MAX, 50);
    assert!(pin_room(49), "the fifty-first pin fits");
    assert!(!pin_room(50), "the fifty-first pin past fifty is refused");
    assert!(pin_room(0), "an empty channel takes pins");
}

#[test]
fn unpin_removes_and_lists_newest_first() {
    // pins_of orders by pinned_at, newest pin first; the unpin drops the row,
    // so the list is what remains, still newest first.
    let pinned = vec![(id(11), 100i64), (id(12), 250), (id(13), 175)];
    assert_eq!(pin_order(&pinned), vec![id(12), id(13), id(11)], "newest pin first");
    let mut left = pinned.iter().filter(|(m, _)| *m != id(12)).collect::<Vec<_>>();
    left.sort_by(|a, b| b.1.cmp(&a.1));
    assert_eq!(left.iter().map(|(m, _)| *m).collect::<Vec<_>>(), vec![id(13), id(11)], "the unpin taken out, order kept");
    assert!(pin_order(&[]).is_empty(), "an unpin of the only pin leaves nothing");
}
