//! The gates the notification verbs check, over spaces built in the shapes
//! the server can hold. `set_notification_setting` makes exactly one of these
//! `chat_model::perms` calls before it touches the db — `effective` for a
//! space target, `channel_access(...).can_read()` for a channel one — so
//! covering the call, allow for the one who may and refusal for the one who
//! may not, covers the verb without reaching through the session wire. A
//! person only ever changes their own settings. The caller is a non-owner
//! member (user 2): the owner's reach is the whole space, so it is never
//! refused a locked channel, and only a plain member shows the refusal. The
//! mute-end rule and the restatement's delivery ride along.

use chat_model::perms::{channel_access, effective, Access, Permission, DEFAULT_EVERYONE};
use chat_model::{Channel, ChannelKind, Id, Member, Overwrite, Space};
use std::collections::BTreeMap;

fn id(n: u128) -> Id {
    Id::from_u128(n)
}

/// A space owned by user 1; the settings caller is user 2, a plain member.
fn space(people: &[(u128, Vec<u128>)]) -> Space {
    Space {
        id: id(100),
        name: "s".into(),
        icon: None,
        owner: id(1),
        everyone: DEFAULT_EVERYONE,
        members: people
            .iter()
            .map(|(u, rs)| (id(*u), Member { roles: rs.iter().map(|&n| id(n)).collect(), joined_at: 0 }))
            .collect(),
        roles: BTreeMap::new(),
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

fn over(c: Channel, role: Id, allow: u64, deny: u64) -> Channel {
    Channel {
        overwrites: [(role, Overwrite { allow, deny })].into_iter().collect(),
        ..c
    }
}

#[test]
fn a_member_may_set_their_space_setting_and_a_stranger_may_not() {
    let sp = space(&[(1, vec![]), (2, vec![])]);
    assert!(effective(&sp, id(2)).is_some(), "the verb's gate lets the member caller through");
    assert!(effective(&sp, id(3)).is_none(), "someone outside the space is refused");
}

#[test]
fn a_reader_may_set_a_channel_setting_and_a_locked_channel_is_refused() {
    let vc = Permission::ViewChannel.bit();

    // An open channel: readable to a plain member, so the setting is allowed.
    let sp = space(&[(1, vec![]), (2, vec![])]);
    assert!(channel_access(id(2), &channel(200), Some(&sp)).can_read(), "an open channel is readable");

    // A channel the member sees but cannot read: a mask asking for admins.
    // (The owner reads it whatever the mask asks, so the judge here is the
    // member, not user 1.)
    let mut locked = channel(300);
    locked.view = Permission::IsAdmin.bit();
    assert_eq!(channel_access(id(2), &locked, Some(&sp)), Access::Locked, "seen but not readable");
    assert!(!channel_access(id(2), &locked, Some(&sp)).can_read(), "and the verb refuses a setting on it");
    assert_eq!(channel_access(id(1), &locked, Some(&sp)), Access::Full, "the owner is never locked out, which is why the caller is a plain member");

    // A deny overwrite that takes reading away from a role the member holds.
    let held = space(&[(1, vec![]), (2, vec![50])]);
    assert_eq!(channel_access(id(2), &over(channel(400), id(50), 0, vc), Some(&held)), Access::Locked, "the deny cuts the member's reading");

    // A direct conversation the person is in is theirs to set; one they are
    // not in is `None`, not theirs to know about.
    let mut dm = channel(500);
    dm.kind = ChannelKind::Direct;
    dm.members = vec![id(1), id(2)];
    assert!(channel_access(id(2), &dm, None).can_read(), "the member caller is a party to the conversation");
    assert_eq!(channel_access(id(3), &dm, None), Access::None, "a stranger is not in it");
}

#[test]
fn a_mute_end_must_be_the_no_end_marker_or_a_positive_deadline() {
    assert!(chat_model::mute_end_ok(None), "no mute at all");
    assert!(chat_model::mute_end_ok(Some(chat_model::MUTED_UNTIL_TURNED_OFF)), "a mute with no end");
    assert!(chat_model::mute_end_ok(Some(1000)), "a deadline in the future");
    assert!(!chat_model::mute_end_ok(Some(0)), "zero would never read as muted");
    assert!(!chat_model::mute_end_ok(Some(-5)), "nor a negative one");
}

#[test]
fn the_restatement_reaches_the_persons_own_connections_only() {
    use chat_model::live::{Event, State, topics_for, user_topic};

    let mut s = State {
        me: Some(chat_model::Me {
            user: chat_model::User { id: id(1), handle: "me".into(), display_name: "Me".into(), avatar: None, bio: String::new() },
            email: "me@example.com".into(),
            status: chat_model::Status { presence: chat_model::Presence::Online, text: None },
        }),
        ..Default::default()
    };
    s.apply(&Event::SpaceUpserted { space: space(&[(1, vec![])]), channels: vec![channel(200)], users: vec![] });

    // Every connection of the person subscribes to their own topic, so a
    // restatement there follows their other sessions.
    assert!(topics_for(&s).contains(&user_topic(id(1))), "the person's own topic");
    assert!(!topics_for(&s).contains(&user_topic(id(2))), "a setting change is not announced to others");

    // And the event lands on the state, so the counters use the new level.
    s.apply(&Event::NotificationSettingsChanged {
        settings: vec![chat_model::NotificationSetting { scope: chat_model::Scope::Space, target: id(100), level: Some(chat_model::Level::Mentions), muted_until: None }],
    });
    assert_eq!(s.settings.get(&id(100)).and_then(|x| x.level), Some(chat_model::Level::Mentions));
}
