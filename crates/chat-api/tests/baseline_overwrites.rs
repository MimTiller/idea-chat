//! The gate every baseline and overwrite verb checks, over spaces built in
//! the shapes the server can hold. Each `#[server]` verb makes exactly one
//! of these `chat_model::perms` calls before it touches the db, so covering
//! the call — allow for the one who may, refusal for the one who may not —
//! covers the verb without reaching through the session wire.

use chat_model::perms::{
    at_channel, can_set_everyone, can_set_overwrite, covers, effective, takes_overwrites,
    Permission, ADMIN, ALL, EVERYONE,
};
use chat_model::{Channel, ChannelKind, Id, Member, Overwrite, Role, Space};
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
        .map(|(u, rs)| {
            (
                id(*u),
                Member {
                    roles: rs.iter().map(|&n| id(n)).collect(),
                    joined_at: 0,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    if !ms.contains_key(&id(1)) {
        ms.insert(
            id(1),
            Member {
                roles: vec![],
                joined_at: 0,
            },
        );
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
            .map(|(r, p, pos)| {
                (
                    id(*r),
                    Role {
                        id: id(*r),
                        name: "r".into(),
                        permissions: *p,
                        position: *pos,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>(),
        categories: vec![],
        created_at: 0,
        emoji: BTreeMap::new(),
        stickers: BTreeMap::new(),
    }
}

/// An ordinary space channel with open masks and no overwrites.
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
        view: EVERYONE,
        post: EVERYONE,
        members: vec![],
        archived: false,
        overwrites: BTreeMap::<Id, Overwrite>::new(),
        last_message: None,
        message_count: 0,
        created_by: id(1),
        created_at: 0,
    }
}

/// The same channel as a thread off the space's text channel: a thread
/// follows its parent, so it takes no per-role overwrite of its own.
fn thread() -> Channel {
    Channel {
        kind: ChannelKind::Thread,
        parent: Some(id(200)),
        ..channel()
    }
}

/// The same channel as a direct conversation: it lives outside the space,
/// so it has no roles to overwrite.
fn direct() -> Channel {
    Channel {
        kind: ChannelKind::Direct,
        space: None,
        ..channel()
    }
}

/// The same channel as a standing voice channel: it takes overwrites like a
/// text channel does.
fn voice() -> Channel {
    Channel {
        kind: ChannelKind::Voice,
        ..channel()
    }
}

/// The same channel with one role's allow/deny pair set on it.
fn over(c: Channel, role: Id, allow: u64, deny: u64) -> Channel {
    Channel {
        overwrites: [(role, Overwrite { allow, deny })].into_iter().collect(),
        ..c
    }
}

// -------------------------------------------------------------- set_everyone

#[test]
fn a_role_manager_restates_the_baseline_with_only_powers_they_hold() {
    const MOD: u64 = Permission::ManageRoles.bit();
    const VC: u64 = Permission::ViewChannel.bit();
    const SEND: u64 = Permission::SendMessage.bit();
    // The boss (user 2) manages roles and can read; the owner (user 1) holds all.
    let s = space(
        &[(7u128, MOD | VC, 0i32)],
        &[(2u128, vec![7u128])],
        EVERYONE,
    );
    assert!(
        can_set_everyone(&s, id(1), SEND),
        "the owner restates anything they hold"
    );
    assert!(
        can_set_everyone(&s, id(2), VC),
        "a role manager restates a power they hold"
    );
    assert!(
        !can_set_everyone(&s, id(2), VC | SEND),
        "the baseline may not smuggle in a power the manager lacks"
    );
    assert!(
        !can_set_everyone(&s, id(4), VC),
        "a plain member holds nothing and restates nothing"
    );
}

#[test]
fn the_admin_tag_never_lands_in_the_baseline() {
    const MOD: u64 = Permission::ManageRoles.bit();
    let s = space(
        &[(7u128, MOD | ADMIN, 0i32)],
        &[(2u128, vec![7u128])],
        EVERYONE,
    );
    assert!(
        !can_set_everyone(&s, id(1), ADMIN),
        "even the owner, who holds everything, cannot tag the baseline admin"
    );
    assert!(
        !can_set_everyone(&s, id(2), ADMIN),
        "the manager cannot tag the baseline admin either"
    );
    assert!(
        can_set_everyone(&s, id(1), MOD),
        "a plain power in the baseline is fine"
    );
}

#[test]
fn an_outsider_restates_nothing_and_denies_nothing() {
    const MOD: u64 = Permission::ManageRoles.bit();
    let s = space(&[(7u128, MOD, 0i32)], &[(2u128, vec![7u128])], EVERYONE);
    // user 9 is not in the space at all: neither verb may land through them.
    assert!(
        !can_set_everyone(&s, id(9), EVERYONE),
        "even an empty baseline is not an act for an outsider"
    );
    assert!(
        !can_set_overwrite(&s, id(9), &channel(), id(7), EVERYONE),
        "nor is writing a role's overwrite"
    );
    assert_eq!(
        effective(&s, id(9)),
        None,
        "an outsider reads no permission set at all"
    );
}

// -------------------------------------------------------------- set_overwrite

#[test]
fn an_overwrite_needs_the_channel_reach_and_the_allow_held() {
    const CH: u64 = Permission::ManageChannelRoles.bit();
    const SEND: u64 = Permission::SendMessage.bit();
    const KICK: u64 = Permission::BanMembers.bit();
    // user 2 holds ManageChannelRoles and SendMessage at the senior slot;
    // the junior role 8 sits below them.
    let s = space(
        &[(7u128, CH | SEND, 0i32), (8u128, SEND, 1i32)],
        &[(2u128, vec![7u128]), (3u128, vec![8u128])],
        EVERYONE,
    );
    let c = channel();
    assert!(
        can_set_overwrite(&s, id(1), &c, id(8), SEND),
        "the owner sets an overwrite on a role"
    );
    assert!(
        can_set_overwrite(&s, id(2), &c, id(8), SEND),
        "a channel manager sets an overwrite for a role below their own rank"
    );
    assert!(
        !can_set_overwrite(&s, id(2), &c, id(7), SEND),
        "a manager cannot overwrite the role at their own rank, only below it"
    );
    assert!(
        !can_set_overwrite(&s, id(3), &c, id(8), SEND),
        "a plain member sets nobody's overwrite"
    );
    assert!(
        !can_set_overwrite(&s, id(2), &c, id(8), SEND | KICK),
        "the allow may not smuggle in a power the manager lacks"
    );
    assert!(
        can_set_overwrite(&s, id(2), &c, id(8), EVERYONE),
        "the allow may be empty when the manager only wants to deny"
    );
}

#[test]
fn deny_is_unchecked_but_the_role_and_channel_must_fit() {
    const CH: u64 = Permission::ManageChannelRoles.bit();
    const SEND: u64 = Permission::SendMessage.bit();
    let s = space(
        &[(7u128, CH, 0i32), (8u128, SEND, 1i32)],
        &[(2u128, vec![7u128]), (3u128, vec![8u128])],
        EVERYONE,
    );
    let c = channel();
    // A denial carries whatever it likes: cutting a power one holds borrows
    // nobody else's.
    assert!(
        can_set_overwrite(&s, id(2), &c, id(8), EVERYONE),
        "an empty allow with a deny still lands once the role sits below rank"
    );
    assert!(
        !takes_overwrites(&thread(), &s),
        "a thread follows its parent and takes no overwrite of its own"
    );
    assert!(
        !takes_overwrites(&direct(), &s),
        "a direct conversation lives outside the space, so it takes none"
    );
    assert!(
        !can_set_overwrite(&s, id(2), &thread(), id(7), EVERYONE),
        "so the verb refuses a thread outright"
    );
    // The role must belong to the channel's space.
    assert!(
        !can_set_overwrite(&s, id(2), &c, id(999), EVERYONE),
        "a role the space does not have is nobody's overwrite"
    );
}

#[test]
fn a_voice_channel_takes_overwrites_and_a_thread_does_not() {
    const CH: u64 = Permission::ManageChannelRoles.bit();
    const SEND: u64 = Permission::SendMessage.bit();
    let s = space(
        &[(7u128, CH, 0i32), (8u128, SEND, 1i32)],
        &[(2u128, vec![7u128])],
        EVERYONE,
    );
    assert!(
        takes_overwrites(&voice(), &s),
        "a standing voice channel is an ordinary space channel, so it takes them"
    );
    assert!(
        can_set_overwrite(&s, id(2), &voice(), id(8), EVERYONE),
        "and the verb lands on one at a role below rank"
    );
    assert!(
        !takes_overwrites(&thread(), &s),
        "a thread reads its parent's rules instead"
    );
}

#[test]
fn setting_both_zero_is_the_clear_that_deletes_the_row() {
    // The rule reads a (0, 0) allow/deny the same as any other pair; the db
    // is what drops the row. Here the verb simply allows the act, so the
    // caller can reach the clear.
    const CH: u64 = Permission::ManageChannelRoles.bit();
    const SEND: u64 = Permission::SendMessage.bit();
    let s = space(
        &[(7u128, CH | SEND, 0i32), (8u128, SEND, 1i32)],
        &[(2u128, vec![7u128]), (3u128, vec![8u128])],
        EVERYONE,
    );
    assert!(
        can_set_overwrite(&s, id(2), &channel(), id(8), EVERYONE),
        "the manager may write the (0, 0) pair that clears the row"
    );
    // And the effective permission the client reads through at_channel agrees
    // with what the server applied, so the reducer keeps them in step.
    let gated = over(channel(), id(8), EVERYONE, SEND);
    assert_eq!(
        at_channel(id(3), &gated, &s).unwrap_or(EVERYONE) & SEND,
        0u64,
        "the junior viewer's posting is denied by the overwrite"
    );
    let widened = over(channel(), id(8), SEND, EVERYONE);
    assert_eq!(
        at_channel(id(3), &widened, &s).unwrap_or(EVERYONE) & SEND,
        SEND,
        "an allow set puts the posting back"
    );
}

#[test]
fn a_manager_at_the_bottom_of_the_ladder_sets_no_overwrite() {
    const CH: u64 = Permission::ManageChannelRoles.bit();
    // user 3 holds the most junior manager role, so nobody sits below them.
    let s = space(
        &[(7u128, CH, 0i32), (8u128, CH, 1i32)],
        &[(2u128, vec![7u128]), (3u128, vec![8u128])],
        EVERYONE,
    );
    let c = channel();
    assert!(
        !can_set_overwrite(&s, id(3), &c, id(7), EVERYONE),
        "a manager cannot rewrite the role senior to their own rank"
    );
    assert!(
        !can_set_overwrite(&s, id(3), &c, id(8), EVERYONE),
        "not even their own, since the rule is strictly below"
    );
    assert!(
        covers(ALL, effective(&s, id(1)).unwrap_or(EVERYONE)),
        "the owner's reach is the whole power, which the ladder never trims"
    );
}
