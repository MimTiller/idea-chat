//! The live world: what one signed-in person sees, kept in step with the
//! server.
//!
//! A connection is served a [`State`] snapshot scoped to its viewer, then a
//! stream of [`Event`]s, and both sides fold events in with [`Live::apply`] —
//! the [`idea_state::Model`] this crate exists to share. The state holds the
//! shell: spaces, channels, people, presence, unread counts, who is in which
//! call. It does not hold messages; those are per-channel [`crate::timeline`]s
//! a client loads when it opens a channel, which the same events feed.
//!
//! # What apply may assume
//!
//! Nothing about order beyond "the server sent them in this order", and not
//! even that an event is new: a resnapshot can overlap events in flight, so
//! every arm is idempotent — applying an event twice leaves the state as
//! applying it once.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::perms::{self, Access, Permission};
use crate::{
    Category, Channel, ChannelKind, Device, Id, KeyRequest, Me, Member, Message, Millis, Presence, ReadState,
    Relation, Ring, Role, SealedKey, Space, Status, User, VoiceState,
};

/// Everything one viewer's client draws from, apart from message history.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct State {
    /// The viewer. Always present in a served snapshot; `None` only in the
    /// empty state a client starts from.
    pub me: Option<Me>,
    /// Everyone the viewer can see: fellow members, friends, conversation
    /// partners — and the viewer.
    pub users: BTreeMap<Id, User>,
    /// Others' status, for those not offline. Absent means offline.
    pub presence: BTreeMap<Id, Status>,
    pub spaces: BTreeMap<Id, Space>,
    /// The viewer's order for the space switcher.
    pub space_order: Vec<Id>,
    /// Every channel the viewer can see or see locked: space channels,
    /// threads, and their direct and group conversations.
    pub channels: BTreeMap<Id, Channel>,
    /// How far the viewer has read, by channel.
    pub reads: BTreeMap<Id, ReadState>,
    pub relations: BTreeMap<Id, Relation>,
    /// Who is in a call, by channel, then user.
    pub voice: BTreeMap<Id, BTreeMap<Id, VoiceState>>,
    /// Calls ringing or running in direct and group conversations, by channel.
    pub rings: BTreeMap<Id, Ring>,
    /// Spaces and channels the viewer has muted.
    pub muted: BTreeSet<Id>,
    /// The viewer's own devices.
    pub devices: Vec<Device>,
}

/// Something that happened. Never refused: by the time an event exists, it
/// is true.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "e", rename_all = "snake_case")]
pub enum Event {
    /// Someone's public profile changed, or someone became visible.
    UserUpdated { user: User },
    /// The viewer's own account changed (status, email).
    MeUpdated { me: Me },
    /// Someone's status, as others see it. `Offline` removes them.
    PresenceChanged { user: Id, status: Status },

    /// A space became visible to the viewer — created, joined — or was
    /// restated whole, with its channels and the people in it.
    SpaceUpserted { space: Space, channels: Vec<Channel>, users: Vec<User> },
    /// A space's name or icon changed.
    SpaceRenamed { space: Id, name: String, icon: Option<String> },
    /// The viewer is no longer in a space: left, removed, or it was deleted.
    SpaceRemoved { space: Id },
    MemberJoined { space: Id, user: User, member: Member },
    MemberRoles { space: Id, user: Id, roles: Vec<Id> },
    /// Ownership moved: the space now belongs to another member.
    OwnerChanged { space: Id, owner: Id },
    /// The space's whole role set restated, so every connection can see the
    /// new names and permissions.
    RolesChanged { space: Id, roles: Vec<Role> },
    MemberLeft { space: Id, user: Id },
    CategoriesChanged { space: Id, categories: Vec<Category> },
    SpaceOrder { spaces: Vec<Id> },

    /// A channel was created or changed.
    ChannelUpserted { channel: Channel },
    ChannelRemoved { channel: Id },

    MessageCreated { message: Message },
    MessageEdited { message: Message },
    MessageDeleted { channel: Id, message: Id },
    ReactionChanged { channel: Id, message: Id, emoji: String, user: Id, added: bool },
    /// The viewer read up to `last_read` — on this device or another.
    Read { channel: Id, last_read: Id },
    /// Someone is typing. Ephemeral: the state keeps nothing; clients show
    /// it for a few seconds.
    Typing { channel: Id, user: Id },

    /// How the viewer relates to `user` changed; `None` means no relation.
    RelationChanged { user: User, relation: Option<Relation> },

    VoiceJoined { state: VoiceState },
    VoiceUpdated { state: VoiceState },
    VoiceLeft { channel: Id, user: Id },
    /// A call started ringing, or who it is ringing changed.
    RingUpdated { ring: Ring },
    RingEnded { channel: Id },
    /// A device joining a call is asking for its key. Ephemeral; for devices
    /// already in the call.
    KeyRequested { request: KeyRequest },
    /// A call key, sealed to one of the viewer's devices. Ephemeral.
    KeyDelivered { key: SealedKey },

    MutedChanged { target: Id, muted: bool },
    DevicesChanged { devices: Vec<Device> },

    /// Server-internal: the viewer's visible world changed shape (a channel's
    /// permissions, a membership) in a way best restated than explained. A
    /// connection that sees one takes a fresh snapshot; clients never do.
    Resync,
}

/// What a client may ask for over the live connection. Everything that
/// changes stored data goes through the API instead, so it has a reply; these
/// are the fire-and-forget signals that only make sense while connected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "a", rename_all = "snake_case")]
pub enum Action {
    /// "I am typing here."
    Typing { channel: Id },
    /// "I have been idle" / "I am back."
    Idle { idle: bool },
}

/// The model the server and every client fold events with.
pub struct Live;

impl idea_state::Model for Live {
    type State = State;
    type Event = Event;
    type Action = Action;

    fn apply(state: &mut State, event: &Event) {
        state.apply(event);
    }
}

impl State {
    /// Fold one event in. See the module docs for what this may assume.
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::UserUpdated { user } => {
                if let Some(me) = self.me.as_mut().filter(|m| m.user.id == user.id) {
                    me.user = user.clone();
                }
                self.users.insert(user.id, user.clone());
            }
            Event::MeUpdated { me } => {
                self.users.insert(me.user.id, me.user.clone());
                self.me = Some(me.clone());
            }
            Event::PresenceChanged { user, status } => {
                if status.presence == Presence::Offline {
                    self.presence.remove(user);
                } else {
                    self.presence.insert(*user, status.clone());
                }
            }

            Event::SpaceUpserted { space, channels, users } => {
                for u in users {
                    self.users.insert(u.id, u.clone());
                }
                // A restatement replaces the space's channels wholesale, so
                // one deleted meanwhile does not linger.
                self.channels.retain(|_, c| c.space != Some(space.id));
                for c in channels {
                    self.channels.insert(c.id, c.clone());
                }
                if !self.space_order.contains(&space.id) {
                    self.space_order.push(space.id);
                }
                self.spaces.insert(space.id, space.clone());
            }
            Event::SpaceRenamed { space, name, icon } => {
                if let Some(s) = self.spaces.get_mut(space) {
                    s.name = name.clone();
                    s.icon = icon.clone();
                }
            }
            Event::SpaceRemoved { space } => {
                self.spaces.remove(space);
                self.space_order.retain(|s| s != space);
                let gone: Vec<Id> =
                    self.channels.values().filter(|c| c.space == Some(*space)).map(|c| c.id).collect();
                for id in gone {
                    self.forget_channel(id);
                }
                self.muted.remove(space);
            }
            Event::MemberJoined { space, user, member } => {
                self.users.insert(user.id, user.clone());
                if let Some(s) = self.spaces.get_mut(space) {
                    s.members.insert(user.id, member.clone());
                }
            }
            Event::MemberRoles { space, user, roles } => {
                if let Some(m) = self.spaces.get_mut(space).and_then(|s| s.members.get_mut(user)) {
                    m.roles = roles.clone();
                }
            }
            Event::RolesChanged { space, roles } => {
                if let Some(s) = self.spaces.get_mut(space) {
                    s.roles = roles.iter().map(|r| (r.id, r.clone())).collect();
                }
            }
            Event::OwnerChanged { space, owner } => {
                if let Some(s) = self.spaces.get_mut(space) {
                    s.owner = *owner;
                }
            }
            Event::MemberLeft { space, user } => {
                if let Some(s) = self.spaces.get_mut(space) {
                    s.members.remove(user);
                }
            }
            Event::CategoriesChanged { space, categories } => {
                if let Some(s) = self.spaces.get_mut(space) {
                    s.categories = categories.clone();
                }
            }
            Event::SpaceOrder { spaces } => {
                // Keep only spaces we have, then any we have that the order
                // left out, so a space can never vanish from the switcher.
                let mut order: Vec<Id> = spaces.iter().copied().filter(|s| self.spaces.contains_key(s)).collect();
                for s in self.spaces.keys() {
                    if !order.contains(s) {
                        order.push(*s);
                    }
                }
                self.space_order = order;
            }

            Event::ChannelUpserted { channel } => {
                self.channels.insert(channel.id, channel.clone());
            }
            Event::ChannelRemoved { channel } => {
                let threads: Vec<Id> =
                    self.channels.values().filter(|c| c.parent == Some(*channel)).map(|c| c.id).collect();
                for t in threads {
                    self.forget_channel(t);
                }
                self.forget_channel(*channel);
            }

            Event::MessageCreated { message } => self.message_created(message),
            Event::MessageEdited { .. } | Event::ReactionChanged { .. } | Event::Typing { .. } => {}
            Event::MessageDeleted { channel, message } => {
                if let Some(c) = self.channels.get_mut(channel) {
                    c.message_count = c.message_count.saturating_sub(1);
                    let _ = message;
                }
            }
            Event::Read { channel, last_read } => {
                let r = self.reads.entry(*channel).or_default();
                // Reading is monotonic: a stale marker from a slower device
                // must not un-read what this one has read.
                if r.last_read.is_none_or(|l| *last_read >= l) {
                    r.last_read = Some(*last_read);
                    r.mentions = 0;
                }
            }

            Event::RelationChanged { user, relation } => {
                self.users.insert(user.id, user.clone());
                match relation {
                    Some(r) => self.relations.insert(user.id, *r),
                    None => self.relations.remove(&user.id),
                };
            }

            Event::VoiceJoined { state } | Event::VoiceUpdated { state } => {
                // One call at a time: joining one leaves any other.
                for (channel, people) in self.voice.iter_mut() {
                    if *channel != state.channel {
                        people.remove(&state.user);
                    }
                }
                self.voice.retain(|_, people| !people.is_empty());
                self.voice.entry(state.channel).or_default().insert(state.user, *state);
                if let Some(ring) = self.rings.get_mut(&state.channel) {
                    ring.ringing.retain(|u| *u != state.user);
                }
            }
            Event::VoiceLeft { channel, user } => {
                if let Some(people) = self.voice.get_mut(channel) {
                    people.remove(user);
                    if people.is_empty() {
                        self.voice.remove(channel);
                    }
                }
            }
            Event::RingUpdated { ring } => {
                self.rings.insert(ring.channel, ring.clone());
            }
            Event::RingEnded { channel } => {
                self.rings.remove(channel);
            }
            Event::KeyRequested { .. } | Event::KeyDelivered { .. } | Event::Resync => {}

            Event::MutedChanged { target, muted } => {
                if *muted {
                    self.muted.insert(*target);
                } else {
                    self.muted.remove(target);
                }
            }
            Event::DevicesChanged { devices } => self.devices = devices.clone(),
        }
    }

    fn forget_channel(&mut self, id: Id) {
        self.channels.remove(&id);
        self.reads.remove(&id);
        self.voice.remove(&id);
        self.rings.remove(&id);
        self.muted.remove(&id);
    }

    fn message_created(&mut self, message: &Message) {
        let Some(channel) = self.channels.get_mut(&message.channel) else { return };
        // A duplicate, or one older than what we know is newest: the counts
        // already include it.
        if channel.last_message.is_some_and(|last| message.id <= last) {
            return;
        }
        channel.last_message = Some(message.id);
        channel.message_count += 1;
        let private = channel.kind.is_private();
        let Some(me) = self.me_id() else { return };
        let read = self.reads.entry(message.channel).or_default();
        if message.author == me {
            // Posting is reading: everything up to your own message is seen.
            read.last_read = Some(message.id);
            read.mentions = 0;
        } else if read.last_read.is_none_or(|l| message.id > l) && (private || message.mentions_user(me)) {
            read.mentions += 1;
        }
    }

    // ---- reading the state ----

    /// The viewer's id.
    pub fn me_id(&self) -> Option<Id> {
        self.me.as_ref().map(|m| m.user.id)
    }

    /// The space a channel belongs to, through its parent for a thread.
    pub fn space_of(&self, channel: &Channel) -> Option<&Space> {
        channel.space.and_then(|s| self.spaces.get(&s))
    }

    /// What the viewer may do in a channel. A thread is judged by its
    /// parent's rules, and is read-only once archived.
    pub fn access(&self, channel: Id) -> Access {
        let (Some(me), Some(c)) = (self.me_id(), self.channels.get(&channel)) else { return Access::None };
        let rules = match c.kind {
            ChannelKind::Thread => {
                let Some(parent) = c.parent.and_then(|p| self.channels.get(&p)) else { return Access::None };
                let mut rules = parent.clone();
                rules.archived = c.archived;
                rules
            }
            _ => c.clone(),
        };
        perms::channel_access(me, &rules, self.space_of(c))
    }

    /// The viewer's permission set in a space — what they may do there.
    pub fn my_permissions(&self, space: Id) -> Option<u64> {
        let Some(me) = self.me_id() else { return None };
        self.spaces.get(&space).and_then(|s| perms::effective(s, me))
    }

    /// Whether a channel has messages the viewer has not read.
    pub fn unread(&self, channel: Id) -> bool {
        let Some(c) = self.channels.get(&channel) else { return false };
        if !c.kind.has_messages() || !self.access(channel).can_read() {
            return false;
        }
        let last_read = self.reads.get(&channel).and_then(|r| r.last_read);
        c.last_message.is_some_and(|last| last_read.is_none_or(|read| last > read))
    }

    /// Messages naming the viewer since they last read a channel.
    pub fn mentions(&self, channel: Id) -> u32 {
        if !self.access(channel).can_read() {
            return 0;
        }
        self.reads.get(&channel).map_or(0, |r| r.mentions)
    }

    /// A space's switcher badge: whether anything in it is unread (muted
    /// channels and a muted space aside), and how many mentions it holds
    /// (muting never hides a mention).
    pub fn space_badge(&self, space: Id) -> Badge {
        let space_muted = self.muted.contains(&space);
        let mut badge = Badge::default();
        for c in self.channels.values().filter(|c| c.space == Some(space)) {
            if !space_muted && !self.muted.contains(&c.id) && self.unread(c.id) {
                badge.unread = true;
            }
            badge.mentions += self.mentions(c.id);
        }
        badge
    }

    /// Home's badge: unread direct messages, plus friend requests waiting.
    pub fn home_badge(&self) -> Badge {
        let mut badge = Badge::default();
        for c in self.conversations() {
            badge.mentions += self.mentions(c.id);
        }
        badge.mentions += self.relations.values().filter(|r| **r == Relation::Incoming).count() as u32;
        badge.unread = badge.mentions > 0;
        badge
    }

    /// The viewer's spaces, in switcher order.
    pub fn ordered_spaces(&self) -> Vec<&Space> {
        self.space_order.iter().filter_map(|s| self.spaces.get(s)).collect()
    }

    /// A space's navigation: uncategorised channels first, then each category
    /// in order; within each, channels by position then name, every one
    /// followed by its open threads, most recently active first.
    pub fn sidebar(&self, space: Id) -> Vec<Section<'_>> {
        let Some(s) = self.spaces.get(&space) else { return Vec::new() };
        let mut tops: Vec<&Channel> = self
            .channels
            .values()
            .filter(|c| c.space == Some(space) && c.kind != ChannelKind::Thread && self.access(c.id) != Access::None)
            .collect();
        tops.sort_by(|a, b| (a.position, &a.name, a.id).cmp(&(b.position, &b.name, b.id)));
        let entry = |c: &'_ Channel| -> Entry<'_> {
            let mut threads: Vec<&Channel> = self
                .channels
                .values()
                .filter(|t| t.parent == Some(c.id) && !t.archived && self.access(t.id).can_read())
                .collect();
            threads.sort_by(|a, b| b.last_message.cmp(&a.last_message).then(b.id.cmp(&a.id)));
            Entry { channel: self.channels.get(&c.id).expect("listed channel exists"), threads }
        };
        let mut sections = vec![Section {
            category: None,
            entries: tops.iter().filter(|c| c.category.is_none()).map(|c| entry(c)).collect(),
        }];
        let mut categories: Vec<&Category> = s.categories.iter().collect();
        categories.sort_by(|a, b| (a.position, &a.name).cmp(&(b.position, &b.name)));
        for cat in categories {
            sections.push(Section {
                category: Some(cat),
                entries: tops.iter().filter(|c| c.category == Some(cat.id)).map(|c| entry(c)).collect(),
            });
        }
        // Channels whose category is gone fall back to the top.
        let known: BTreeSet<Id> = s.categories.iter().map(|c| c.id).collect();
        let orphans: Vec<Entry<'_>> =
            tops.iter().filter(|c| c.category.is_some_and(|k| !known.contains(&k))).map(|c| entry(c)).collect();
        sections[0].entries.extend(orphans);
        sections.retain(|s| s.category.is_some() || !s.entries.is_empty());
        sections
    }

    /// The viewer's direct and group conversations, most recent first.
    pub fn conversations(&self) -> Vec<&Channel> {
        let mut list: Vec<&Channel> = self.channels.values().filter(|c| c.kind.is_private()).collect();
        list.sort_by(|a, b| b.last_message.unwrap_or(b.id).cmp(&a.last_message.unwrap_or(a.id)));
        list
    }

    /// The other person in a direct conversation.
    pub fn partner(&self, channel: &Channel) -> Option<Id> {
        let me = self.me_id()?;
        (channel.kind == ChannelKind::Direct).then(|| channel.members.iter().copied().find(|m| *m != me)).flatten()
    }

    /// The direct conversation with `user`, if there is one.
    pub fn direct_with(&self, user: Id) -> Option<&Channel> {
        self.channels.values().find(|c| c.kind == ChannelKind::Direct && self.partner(c) == Some(user))
    }

    /// What a channel is called where it is listed: its name, the other
    /// person for a direct conversation, the people for an unnamed group.
    pub fn title(&self, channel: &Channel) -> String {
        match channel.kind {
            ChannelKind::Direct => {
                self.partner(channel).map(|u| self.display_name(u)).unwrap_or_else(|| "Direct message".into())
            }
            ChannelKind::Group if channel.name.is_empty() => {
                let me = self.me_id();
                let names: Vec<String> =
                    channel.members.iter().filter(|m| Some(**m) != me).map(|m| self.display_name(*m)).collect();
                if names.is_empty() { "Just you".into() } else { names.join(", ") }
            }
            _ => channel.name.clone(),
        }
    }

    /// Someone's name, or a stand-in for someone the viewer cannot see.
    pub fn display_name(&self, user: Id) -> String {
        self.users.get(&user).map_or_else(|| "Someone".to_string(), |u| u.display_name.clone())
    }

    /// Someone's presence as the viewer sees it. The viewer sees their own
    /// chosen status.
    pub fn presence_of(&self, user: Id) -> Presence {
        if Some(user) == self.me_id() {
            return self.me.as_ref().map_or(Presence::Offline, |m| m.status.presence);
        }
        self.presence.get(&user).map_or(Presence::Offline, |s| s.presence)
    }

    /// Someone's status line, if they set one and are not offline.
    pub fn status_text(&self, user: Id) -> Option<&str> {
        if Some(user) == self.me_id() {
            return self.me.as_ref().and_then(|m| m.status.text.as_deref());
        }
        self.presence.get(&user).and_then(|s| s.text.as_deref())
    }

    /// Who is in a channel's call, in the order they joined.
    pub fn in_call(&self, channel: Id) -> Vec<&VoiceState> {
        let mut people: Vec<&VoiceState> = self.voice.get(&channel).map(|p| p.values().collect()).unwrap_or_default();
        people.sort_by_key(|v| (v.joined_at, v.user));
        people
    }

    /// The call the viewer is in, if any.
    pub fn my_call(&self) -> Option<&VoiceState> {
        let me = self.me_id()?;
        self.voice.values().find_map(|people| people.get(&me))
    }

    /// A call ringing for the viewer — one they were rung for and have not
    /// answered, and are not already in.
    pub fn incoming_ring(&self) -> Option<&Ring> {
        let me = self.me_id()?;
        self.rings.values().find(|r| r.started_by != me && r.ringing.contains(&me))
    }

    /// The people in a space, grouped for its member list: admins (anyone
    /// whose permission set covers the admin tag) online first, then others
    /// online, then offline, each by name.
    pub fn roster(&self, space: Id) -> Roster {
        let mut roster = Roster::default();
        let Some(s) = self.spaces.get(&space) else { return roster };
        let mut members: Vec<(Id, bool)> = s.members.keys().map(|u| (*u, perms::can(s, *u, Permission::IsAdmin))).collect();
        members.sort_by_key(|(u, admin)| (std::cmp::Reverse(*admin), self.display_name(*u).to_lowercase()));
        for (user, _) in members {
            if self.presence_of(user) == Presence::Offline {
                roster.offline.push(user);
            } else if perms::can(s, user, Permission::IsAdmin) {
                roster.admins.push(user);
            } else {
                roster.online.push(user);
            }
        }
        roster
    }

    /// People by how the viewer relates to them, each list by name.
    pub fn people(&self, relation: Relation) -> Vec<&User> {
        let mut list: Vec<&User> =
            self.relations.iter().filter(|(_, r)| **r == relation).filter_map(|(u, _)| self.users.get(u)).collect();
        list.sort_by_cached_key(|u| u.display_name.to_lowercase());
        list
    }

    /// Spaces the viewer shares with `user`.
    pub fn shared_spaces(&self, user: Id) -> Vec<&Space> {
        self.ordered_spaces().into_iter().filter(|s| s.members.contains_key(&user)).collect()
    }

    /// When someone joined a space, if they are in it.
    pub fn joined_at(&self, space: Id, user: Id) -> Option<Millis> {
        self.spaces.get(&space)?.members.get(&user).map(|m| m.joined_at)
    }
}

/// An unread dot and a mention count, for a switcher icon or Home.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Badge {
    pub unread: bool,
    pub mentions: u32,
}

/// One heading of a space's navigation, and what is under it.
#[derive(Debug, Clone, PartialEq)]
pub struct Section<'a> {
    pub category: Option<&'a Category>,
    pub entries: Vec<Entry<'a>>,
}

/// A channel in the navigation, with its open threads.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry<'a> {
    pub channel: &'a Channel,
    pub threads: Vec<&'a Channel>,
}

/// A space's member list, grouped.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Roster {
    pub admins: Vec<Id>,
    pub online: Vec<Id>,
    pub offline: Vec<Id>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MessageKind;

    fn id(n: u128) -> Id {
        Id::from_u128(n)
    }

    fn user(n: u128, name: &str) -> User {
        User { id: id(n), handle: name.to_lowercase(), display_name: name.into(), avatar: None, bio: String::new() }
    }

    fn me(n: u128) -> Me {
        Me { user: user(n, "Me"), email: "me@example.com".into(), status: Status { presence: Presence::Online, text: None } }
    }

    fn space(n: u128, members: &[(u128, Vec<u128>)]) -> Space {
        Space {
            id: id(n),
            name: "Space".into(),
            icon: None,
            owner: id(1),
            everyone: 0,
            members: members
                .iter()
                .map(|(u, rs)| (id(*u), Member { roles: rs.iter().map(|&n| id(n)).collect(), joined_at: 0 }))
                .collect(),
            roles: BTreeMap::new(),
            categories: vec![],
            created_at: 0,
        }
    }

    fn channel(n: u128, space: Option<u128>, kind: ChannelKind) -> Channel {
        Channel {
            id: id(n),
            kind,
            space: space.map(id),
            name: format!("c{n}"),
            topic: String::new(),
            category: None,
            position: 0,
            parent: None,
            starter: None,
            view: 0,
            post: 0,
            members: vec![],
            archived: false,
            last_message: None,
            message_count: 0,
            created_by: id(1),
            created_at: 0,
        }
    }

    fn message(n: u128, channel: u128, author: u128) -> Message {
        Message {
            id: id(n),
            channel: id(channel),
            author: id(author),
            kind: MessageKind::Text,
            body: "hi".into(),
            attachments: vec![],
            reply_to: None,
            reactions: vec![],
            thread: None,
            mentions: vec![],
            mentions_everyone: false,
            at: 0,
            edited_at: None,
        }
    }

    /// Me (1) in space 100 with a text channel 200, and a direct
    /// conversation 300 with user 2.
    fn world() -> State {
        let mut s = State { me: Some(me(1)), ..Default::default() };
        s.apply(&Event::SpaceUpserted {
            space: space(100, &[(1, vec![]), (2, vec![])]),
            channels: vec![channel(200, Some(100), ChannelKind::Text)],
            users: vec![user(1, "Me"), user(2, "Ana")],
        });
        let mut dm = channel(300, None, ChannelKind::Direct);
        dm.members = vec![id(1), id(2)];
        s.apply(&Event::ChannelUpserted { channel: dm });
        s
    }

    #[test]
    fn a_message_from_someone_else_is_unread_until_read() {
        let mut s = world();
        s.apply(&Event::MessageCreated { message: message(1000, 200, 2) });
        assert!(s.unread(id(200)));
        assert_eq!(s.mentions(id(200)), 0, "no mention, no count");
        s.apply(&Event::Read { channel: id(200), last_read: id(1000) });
        assert!(!s.unread(id(200)));
    }

    #[test]
    fn a_mention_counts_and_reading_clears_it() {
        let mut s = world();
        let mut m = message(1000, 200, 2);
        m.mentions = vec![id(1)];
        s.apply(&Event::MessageCreated { message: m });
        assert_eq!(s.mentions(id(200)), 1);
        assert_eq!(s.space_badge(id(100)), Badge { unread: true, mentions: 1 });
        s.apply(&Event::Read { channel: id(200), last_read: id(1000) });
        assert_eq!(s.space_badge(id(100)), Badge::default());
    }

    #[test]
    fn every_direct_message_counts_as_a_mention() {
        let mut s = world();
        s.apply(&Event::MessageCreated { message: message(1000, 300, 2) });
        s.apply(&Event::MessageCreated { message: message(1001, 300, 2) });
        assert_eq!(s.mentions(id(300)), 2);
        assert_eq!(s.home_badge().mentions, 2);
    }

    /// The case the idempotence rule exists for: a resnapshot overlapping an
    /// event in flight must not count it twice.
    #[test]
    fn applying_a_message_twice_counts_it_once() {
        let mut s = world();
        let m = message(1000, 300, 2);
        s.apply(&Event::MessageCreated { message: m.clone() });
        s.apply(&Event::MessageCreated { message: m });
        assert_eq!(s.mentions(id(300)), 1);
        assert_eq!(s.channels[&id(300)].message_count, 1);
    }

    #[test]
    fn posting_marks_the_channel_read() {
        let mut s = world();
        s.apply(&Event::MessageCreated { message: message(1000, 300, 2) });
        s.apply(&Event::MessageCreated { message: message(1001, 300, 1) });
        assert!(!s.unread(id(300)));
        assert_eq!(s.mentions(id(300)), 0);
    }

    /// Two devices: the slower one's older read marker must not mark unread
    /// what the faster one already read.
    #[test]
    fn a_stale_read_marker_does_not_unread() {
        let mut s = world();
        s.apply(&Event::MessageCreated { message: message(1000, 200, 2) });
        s.apply(&Event::MessageCreated { message: message(1001, 200, 2) });
        s.apply(&Event::Read { channel: id(200), last_read: id(1001) });
        s.apply(&Event::Read { channel: id(200), last_read: id(1000) });
        assert!(!s.unread(id(200)));
    }

    #[test]
    fn a_muted_channel_shows_no_dot_but_keeps_its_mentions() {
        let mut s = world();
        s.apply(&Event::MutedChanged { target: id(200), muted: true });
        let mut m = message(1000, 200, 2);
        s.apply(&Event::MessageCreated { message: m.clone() });
        assert!(!s.space_badge(id(100)).unread);
        m.id = id(1001);
        m.mentions = vec![id(1)];
        s.apply(&Event::MessageCreated { message: m });
        assert_eq!(s.space_badge(id(100)).mentions, 1);
    }

    #[test]
    fn a_locked_channel_is_never_unread() {
        let mut s = State { me: Some(me(2)), ..Default::default() };
        let mut locked = channel(200, Some(100), ChannelKind::Text);
        locked.view = perms::ADMIN;
        s.apply(&Event::SpaceUpserted {
            space: space(100, &[(1, vec![]), (2, vec![])]),
            channels: vec![locked],
            users: vec![],
        });
        s.apply(&Event::MessageCreated { message: message(1000, 200, 1) });
        assert_eq!(s.access(id(200)), Access::Locked);
        assert!(!s.unread(id(200)));
    }

    #[test]
    fn leaving_a_space_forgets_its_channels() {
        let mut s = world();
        s.apply(&Event::SpaceRemoved { space: id(100) });
        assert!(s.spaces.is_empty());
        assert!(!s.channels.contains_key(&id(200)));
        assert!(s.channels.contains_key(&id(300)), "conversations are not the space's");
        assert!(s.space_order.is_empty());
    }

    #[test]
    fn restating_a_space_drops_channels_deleted_meanwhile() {
        let mut s = world();
        s.apply(&Event::SpaceUpserted {
            space: space(100, &[(1, vec![])]),
            channels: vec![channel(201, Some(100), ChannelKind::Text)],
            users: vec![],
        });
        assert!(!s.channels.contains_key(&id(200)));
        assert!(s.channels.contains_key(&id(201)));
        assert_eq!(s.space_order, vec![id(100)], "restating does not reorder");
    }

    #[test]
    fn joining_a_call_leaves_the_last_one() {
        let mut s = world();
        let v = |channel: u128| VoiceState {
            user: id(1),
            channel: id(channel),
            muted: false,
            deafened: false,
            camera: false,
            screen: false,
            joined_at: 0,
        };
        s.apply(&Event::VoiceJoined { state: v(200) });
        s.apply(&Event::VoiceJoined { state: v(300) });
        assert!(s.in_call(id(200)).is_empty());
        assert_eq!(s.my_call().map(|v| v.channel), Some(id(300)));
        s.apply(&Event::VoiceLeft { channel: id(300), user: id(1) });
        assert!(s.my_call().is_none());
        assert!(s.voice.is_empty());
    }

    #[test]
    fn joining_a_ringing_call_stops_it_ringing_you() {
        let mut s = State { me: Some(me(2)), ..world() };
        s.apply(&Event::RingUpdated {
            ring: Ring { call: id(9), channel: id(300), started_by: id(1), started_at: 0, ringing: vec![id(2)] },
        });
        assert!(s.incoming_ring().is_some());
        s.apply(&Event::VoiceJoined {
            state: VoiceState {
                user: id(2),
                channel: id(300),
                muted: false,
                deafened: false,
                camera: false,
                screen: false,
                joined_at: 0,
            },
        });
        assert!(s.incoming_ring().is_none());
    }

    #[test]
    fn the_sidebar_groups_by_category_and_nests_threads() {
        let mut s = world();
        let cat = Category { id: id(50), name: "Voice".into(), position: 0 };
        s.apply(&Event::CategoriesChanged { space: id(100), categories: vec![cat] });
        let mut voice = channel(201, Some(100), ChannelKind::Voice);
        voice.category = Some(id(50));
        s.apply(&Event::ChannelUpserted { channel: voice });
        let mut thread = channel(202, Some(100), ChannelKind::Thread);
        thread.parent = Some(id(200));
        s.apply(&Event::ChannelUpserted { channel: thread });

        let sections = s.sidebar(id(100));
        assert_eq!(sections.len(), 2);
        assert!(sections[0].category.is_none());
        assert_eq!(sections[0].entries[0].channel.id, id(200));
        assert_eq!(sections[0].entries[0].threads[0].id, id(202));
        assert_eq!(sections[1].entries[0].channel.id, id(201));
    }

    #[test]
    fn a_channel_in_a_deleted_category_falls_to_the_top() {
        let mut s = world();
        let mut c = channel(201, Some(100), ChannelKind::Text);
        c.category = Some(id(77));
        s.apply(&Event::ChannelUpserted { channel: c });
        let top: Vec<Id> = s.sidebar(id(100))[0].entries.iter().map(|e| e.channel.id).collect();
        assert!(top.contains(&id(201)));
    }

    #[test]
    fn conversations_sort_by_latest_message_and_name_the_partner() {
        let mut s = world();
        let mut group = channel(301, None, ChannelKind::Group);
        group.name = String::new();
        group.members = vec![id(1), id(2)];
        s.apply(&Event::ChannelUpserted { channel: group });
        s.apply(&Event::MessageCreated { message: message(2000, 300, 2) });
        let list: Vec<Id> = s.conversations().iter().map(|c| c.id).collect();
        assert_eq!(list, vec![id(300), id(301)]);
        assert_eq!(s.title(&s.channels[&id(300)]), "Ana");
        assert_eq!(s.title(&s.channels[&id(301)]), "Ana");
    }

    #[test]
    fn the_space_order_never_loses_a_space() {
        let mut s = world();
        s.apply(&Event::SpaceUpserted { space: space(101, &[(1, vec![])]), channels: vec![], users: vec![] });
        s.apply(&Event::SpaceOrder { spaces: vec![id(101), id(999)] });
        assert_eq!(s.space_order, vec![id(101), id(100)]);
    }

    #[test]
    fn the_wire_form_round_trips() {
        let s = world();
        let e = Event::MessageCreated { message: message(1000, 200, 2) };
        let back: Event = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back, e);
        let back: State = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}