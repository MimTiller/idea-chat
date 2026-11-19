//! Who hears a channel's live events, decided the same way the server
//! subscribes them. History, search and sending already went through
//! [`chat_model::perms::channel_access`]; delivery used to ride the shared
//! space topic whenever `view` was `0`, so a `ViewChannel` deny overwrite or
//! a baseline without `ViewChannel` hid a channel's history yet still pushed
//! its messages, edits, reactions and typing to people who could not see it.
//! These tests read the decision directly - `rides_own_topic`, `channel_access`
//! and the shared [`chat_model::live::topics_for`] over a built [`State`] - so
//! they need no network and no `server` feature: what the server wires
//! (`resync` -> `topics_for`) is exactly what they assert.

use chat_model::{Channel, ChannelKind, Id, Me, Member, Overwrite, PermissionSet, Presence, Role, Space, Status, User};
use chat_model::live::{Event, State, topics_for};
use chat_model::perms::{
    ALL,
    ADMIN,
    Access,
    Permission,
    at_channel,
    channel_access,
    covers,
    rides_own_topic,
    DEFAULT_EVERYONE,
};
use std::collections::BTreeMap;

fn id(n: u128) -> Id {
    Id::from_u128(n)
}

fn user(n: u128, name: &str) -> User {
    User {
        id: id(n),
        handle: name.to_lowercase(),
        display_name: name.into(),
        avatar: None,
        bio: String::new(),
    }
}

fn me(n: u128) -> Me {
    Me {
        user: user(n, "Me"),
        email: "me@example.com".into(),
        status: Status { presence: Presence::Online, text: None },
    }
}

fn space(everyone: PermissionSet, roles: &[(u128, PermissionSet)], people: &[(u128, Vec<u128>)]) -> Space {
    let mut ms = people
        .iter()
        .map(|(u, rs)| (id(*u), Member { roles: rs.iter().map(|&n| id(n)).collect(), joined_at: 0 }))
        .collect::<BTreeMap<Id, Member>>();
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
            .map(|(r, p)| (id(*r), Role { id: id(*r), name: "r".into(), permissions: *p, position: 0, colour: None, mentionable: false }))
            .collect::<BTreeMap<Id, Role>>(),
        categories: vec![],
        created_at: 0,
        emoji: BTreeMap::new(),
        stickers: BTreeMap::new(),
    }
}

fn channel(n: u128) -> Channel {
    Channel {
        id: id(n),
        kind: ChannelKind::Text,
        space: Some(id(100)),
        name: "c".into(),
        topic: String::new(),
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

fn viewing(c: Channel, view: PermissionSet) -> Channel {
    Channel { view, ..c }
}

fn over(c: Channel, role: Id, allow: PermissionSet, deny: PermissionSet) -> Channel {
    Channel {
        overwrites: [(role, Overwrite { allow, deny })].into_iter().collect(),
        ..c
    }
}

fn world(sp: Space, channels: &[Channel], people: &[(u128, &str)]) -> State {
    let mut s = State { me: Some(me(1)), ..Default::default() };
    s.apply(&Event::SpaceUpserted {
        space: sp.clone(),
        channels: channels.iter().cloned().collect(),
        users: people.iter().map(|(u, name)| user(*u, name)).collect(),
    });
    s
}

fn seen_by(s: State, n: u128) -> State {
    State { me: Some(me(n)), ..s }
}

fn space_topic(n: u128) -> String {
    format!("s:{}", id(n))
}

fn channel_topic(n: u128) -> String {
    format!("c:{}", id(n))
}

#[test]
fn a_baseline_without_view_channel_hides_the_channel_from_plain_members() {
    let vc = Permission::ViewChannel.bit();
    let hidden = space(DEFAULT_EVERYONE & !vc, &[], &[(2, vec![])]);
    let open = space(DEFAULT_EVERYONE, &[], &[(2, vec![])]);
    assert!(!rides_own_topic(&channel(200), open.everyone), "an open baseline gates nothing");
    assert!(rides_own_topic(&channel(200), hidden.everyone), "a baseline without ViewChannel gates everything");

    let topics = topics_for(&world(open, &[channel(200)], &[(1, "Me"), (2, "Ana")]));
    assert!(topics.contains(&space_topic(100)), "the open channel rides the shared space topic");
    assert!(!topics.contains(&channel_topic(200)));

    let ana = seen_by(world(hidden.clone(), &[channel(200)], &[(1, "Me"), (2, "Ana")]), 2);
    assert!(!topics_for(&ana).contains(&channel_topic(200)), "a plain member is not subscribed");

    let owner = world(hidden.clone(), &[channel(200)], &[(1, "Me"), (2, "Ana")]);
    assert!(topics_for(&owner).contains(&channel_topic(200)), "the owner hears it anyway");
}

#[test]
fn a_role_denied_view_channel_by_an_overwrite_is_not_heard() {
    let vc = Permission::ViewChannel.bit();
    let sp = space(DEFAULT_EVERYONE, &[(50, DEFAULT_EVERYONE)], &[(2, vec![50])]);
    let ch = over(channel(200), id(50), 0, vc);
    assert!(rides_own_topic(&ch, sp.everyone), "the overwrite makes the channel gated");
    assert!(!covers(at_channel(id(2), &ch, &sp).unwrap_or(0), vc), "the deny cuts the role's reading");

    let ana = seen_by(world(sp.clone(), &[ch.clone()], &[(1, "Me"), (2, "Ana")]), 2);
    assert!(!topics_for(&ana).contains(&channel_topic(200)));
}

#[test]
fn a_role_allowed_in_by_an_overwrite_reaches_the_channel() {
    let vc = Permission::ViewChannel.bit();
    let sp = space(DEFAULT_EVERYONE & !vc, &[], &[(2, vec![50])]);
    let ch = over(channel(200), id(50), vc, 0);
    assert!(rides_own_topic(&ch, sp.everyone));
    assert!(covers(at_channel(id(2), &ch, &sp).unwrap_or(0), vc), "the allow admits the role");

    let ana = seen_by(world(sp.clone(), &[ch.clone()], &[(1, "Me"), (2, "Ana")]), 2);
    assert!(topics_for(&ana).contains(&channel_topic(200)));
}

#[test]
fn a_thread_inherits_its_parents_gate() {
    let parent = viewing(channel(200), ADMIN);
    let mut thread = channel(300);
    thread.kind = ChannelKind::Thread;
    thread.parent = Some(id(200));
    let sp = space(DEFAULT_EVERYONE, &[], &[(2, vec![])]);
    assert!(rides_own_topic(&parent, sp.everyone), "the parent gates");
    assert!(!rides_own_topic(&thread, sp.everyone), "the bare thread would not");
    assert_eq!(channel_access(id(2), &parent, Some(&sp)), Access::Locked, "the thread is judged by the parent's rules");

    let ana = seen_by(world(sp.clone(), &[parent.clone(), thread.clone()], &[(1, "Me"), (2, "Ana")]), 2);
    assert!(!topics_for(&ana).contains(&channel_topic(300)), "Ana is not subscribed to the thread");

    let owner = world(sp, &[parent, thread.clone()], &[(1, "Me"), (2, "Ana")]);
    assert!(topics_for(&owner).contains(&channel_topic(300)));
}

#[test]
fn the_owner_and_an_admin_hear_everything() {
    let vc = Permission::ViewChannel.bit();
    let sp = space(DEFAULT_EVERYONE, &[(50, DEFAULT_EVERYONE | ADMIN)], &[(2, vec![50])]);
    let ch = over(viewing(channel(200), vc), id(50), 0, vc);
    assert_eq!(at_channel(id(2), &ch, &sp), Some(ALL), "an admin ignores the overwrites");

    let ana = seen_by(world(sp.clone(), &[ch.clone()], &[(1, "Me"), (2, "Ana")]), 2);
    assert!(topics_for(&ana).contains(&channel_topic(200)));
    let owner = world(sp.clone(), &[ch.clone()], &[(1, "Me"), (2, "Ana")]);
    assert_eq!(channel_access(id(1), &ch.clone(), Some(&sp.clone())), Access::Full, "the owner reads and posts");
    assert!(topics_for(&owner).contains(&channel_topic(200)));
}

#[test]
fn an_open_channel_stays_on_the_shared_space_topic() {
    let sp = space(DEFAULT_EVERYONE, &[], &[(2, vec![])]);
    assert!(!rides_own_topic(&channel(200), sp.everyone));
    let topics = topics_for(&world(sp, &[channel(200)], &[(1, "Me"), (2, "Ana")]));
    assert!(topics.contains(&space_topic(100)));
    assert!(!topics.contains(&channel_topic(200)));
}

#[test]
fn a_role_change_moves_the_subscription() {
    let vc = Permission::ViewChannel.bit();
    let sp = space(DEFAULT_EVERYONE & !vc, &[(50, vc)], &[(2, vec![])]);
    let mut s = seen_by(world(sp, &[channel(200)], &[(1, "Me"), (2, "Ana")]), 2);
    let before = topics_for(&s);
    assert!(!before.contains(&channel_topic(200)));

    s.apply(&Event::MemberRoles { space: id(100), user: id(2), roles: [id(50)].into_iter().collect() });
    let with = topics_for(&s);
    assert!(with.contains(&channel_topic(200)), "the role grants reading");

    s.apply(&Event::MemberRoles { space: id(100), user: id(2), roles: vec![] });
    assert_eq!(topics_for(&s), before, "the subscription goes back");
}

// ------------------------------------------------- expression restatements

#[test]
fn an_emoji_change_reaches_the_spaces_members() {
    use chat_model::expressions;

    // An open channel keeps its members on the shared space topic, which is
    // where the verbs publish EmojiChanged / StickersChanged.
    let sp = space(DEFAULT_EVERYONE, &[], &[(2, vec![])]);
    let topics = topics_for(&world(sp.clone(), &[channel(200)], &[(1, "Me"), (2, "Ana")]));
    assert!(topics.contains(&space_topic(100)), "a member of an open channel hears the space topic");

    // The restatement lands on the connected member's world.
    let mut s = seen_by(world(sp.clone(), &[channel(200)], &[(1, "Me"), (2, "Ana")]), 2);
    s.apply(&Event::EmojiChanged {
        space: id(100),
        emoji: vec![expressions::Emoji { id: id(900), name: "wave".into(), upload: id(901), created_by: id(1), created_at: 0 }],
    });
    assert_eq!(s.spaces[&id(100)].emoji[&id(900)].name, "wave", "the new emoji appears on their space");
    s.apply(&Event::EmojiChanged { space: id(100), emoji: vec![] });
    assert!(s.spaces[&id(100)].emoji.is_empty(), "a removal restates to empty");

    let mut t = seen_by(world(sp.clone(), &[channel(200)], &[(1, "Me"), (2, "Ana")]), 2);
    t.apply(&Event::StickersChanged {
        space: id(100),
        stickers: vec![expressions::Sticker { id: id(910), name: "cat".into(), description: "a cat".into(), upload: id(911), created_by: id(1), created_at: 0 }],
    });
    assert_eq!(t.spaces[&id(100)].stickers[&id(910)].description, "a cat", "the new sticker appears on their space");
}
