//! Who may do what. One copy of the rules, run by the server to decide and by
//! the client to decide what to offer — so a button the server would refuse
//! is never drawn.
//!
//! A space's people hold [`crate::Role`]s, and a role is a name plus a set of
//! [`Permission`]s. What someone may do in a space is the union of the
//! permissions of the roles they hold — the owner's being everything,
//! whatever roles they also carry. A channel gates itself with two permission
//! masks ([`crate::Channel`]'s `view` and `post`): reading is allowed where a
//! member's set covers the `view` mask, posting where it covers both.
//! The masks spell themselves with [`Permission::bit`], so a channel open to
//! everyone carries `0` and one open to admins carries [`ADMIN`].

use crate::{Channel, Id, PermissionSet, Space};

/// What someone may do in a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    /// Not theirs to know about: a conversation they are not in, or a space
    /// they are not a member of.
    None,
    /// They can see it exists, but not read it.
    Locked,
    /// They can read but not post.
    ReadOnly,
    /// Read and post.
    Full,
}

impl Access {
    pub fn can_read(self) -> bool {
        self >= Access::ReadOnly
    }

    pub fn can_post(self) -> bool {
        self == Access::Full
    }
}

/// One thing a role may allow in a space. A role's set of these is a bitset
/// (`Permission::bit` selects its bit); a member's effective set is the
/// union of the permissions of the roles they hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    // ---- messages ----
    /// Read a space channel at all.
    ViewChannel,
    /// Post in a channel.
    SendMessage,
    /// Change one of their own messages.
    EditOwnMessage,
    /// Delete one of their own messages.
    DeleteOwnMessage,
    /// Delete someone else's message.
    DeleteOthersMessage,
    /// Pin and unpin messages.
    ManagePins,
    /// React to messages.
    AddReaction,
    /// Address a message to the whole space.
    MentionEveryone,
    /// Upload an attachment alongside a message.
    AttachFile,
    /// Post a link that the reader expands.
    EmbedLink,
    // ---- voice ----
    /// Join a voice channel's call.
    VoiceConnect,
    /// Unmute their microphone.
    VoiceSpeak,
    /// Use a camera.
    VoiceVideo,
    /// Share a screen.
    VoiceShareScreen,
    /// Mute or deafen someone else for the length of the call.
    VoiceForceMute,
    /// Move someone between calls.
    VoiceMoveMember,
    // ---- channels ----
    /// Create, delete and arrange channels.
    ManageChannels,
    /// Set who may see and post in a channel.
    ManageChannelRoles,
    /// Create and revoke invites.
    ManageInvites,
    // ---- space ----
    /// Create, rename and delete roles.
    ManageRoles,
    /// Remove members from the space.
    ManageMembers,
    /// Ban members so they cannot rejoin until unbanned.
    BanMembers,
    /// Time members out so they cannot send or speak until it expires.
    TimeoutMembers,
    /// Change the space's own settings.
    ManageSpaceSettings,
    /// Every permission. A role with this bit is an admin whatever else it
    /// carries.
    IsAdmin,
}

impl Permission {
    /// Its bit in a role's permission set.
    pub const fn bit(self) -> u64 {
        1 << match self {
            Permission::ViewChannel => 0,
            Permission::SendMessage => 1,
            Permission::EditOwnMessage => 2,
            Permission::DeleteOwnMessage => 3,
            Permission::DeleteOthersMessage => 4,
            Permission::ManagePins => 5,
            Permission::AddReaction => 6,
            Permission::MentionEveryone => 7,
            Permission::AttachFile => 8,
            Permission::EmbedLink => 9,
            Permission::VoiceConnect => 10,
            Permission::VoiceSpeak => 11,
            Permission::VoiceVideo => 12,
            Permission::VoiceShareScreen => 13,
            Permission::VoiceForceMute => 14,
            Permission::VoiceMoveMember => 15,
            Permission::ManageChannels => 16,
            Permission::ManageChannelRoles => 17,
            Permission::ManageInvites => 18,
            Permission::ManageRoles => 19,
            Permission::ManageMembers => 20,
            Permission::ManageSpaceSettings => 21,
            Permission::BanMembers => 22,
            Permission::TimeoutMembers => 23,
            Permission::IsAdmin => 63,
        }
    }
}

/// Every permission: what the owner holds, and what a role tagged admin
/// expands to.
pub const ALL: u64 = (1u64 << 63) | ((1u64 << 24) - 1);

/// The admin tag alone: what a channel asks of its admins, and what a
/// permission set must cover to count as an admin (for the roster).
pub const ADMIN: u64 = Permission::IsAdmin.bit();

/// The empty mask: what a channel asks of everyone in the space.
pub const EVERYONE: u64 = 0;

/// The default everyone baseline a new space seeds: see, post, own-message
/// edits and deletes, react, attach, embed, and the self-service voice acts.
/// It stops where the acts on other people and on the space begin.
pub const DEFAULT_EVERYONE: u64 =
    Permission::ViewChannel.bit()
    | Permission::SendMessage.bit()
    | Permission::EditOwnMessage.bit()
    | Permission::DeleteOwnMessage.bit()
    | Permission::AddReaction.bit()
    | Permission::AttachFile.bit()
    | Permission::EmbedLink.bit()
    | Permission::VoiceConnect.bit()
    | Permission::VoiceSpeak.bit()
    | Permission::VoiceVideo.bit()
    | Permission::VoiceShareScreen.bit();

/// Expand a permission set: a set carrying the admin tag carries all.
pub fn expand(permissions: u64) -> u64 {
    if permissions & ADMIN != 0 {
        ALL
    } else {
        permissions
    }
}

/// The union of two permission sets, with the admin tag expanded.
pub fn union(a: u64, b: u64) -> u64 {
    expand(a | b)
}

/// Whether a permission set allows `what`. An admin tag allows everything.
pub fn has(permissions: u64, what: Permission) -> bool {
    covers(permissions, what.bit())
}

/// A set is allowed where it covers every bit of a required mask.
pub fn covers(permissions: u64, required: u64) -> bool {
    permissions & required == required
}

/// A space's role's permission set; empty when the member points at a role
/// the space no longer has.
pub fn role_permissions(space: &Space, role: Id) -> u64 {
    space.roles.get(&role).map(|r| r.permissions).unwrap_or(0)
}

/// What `user` may do in `space`: the space's baseline, then the union of the
/// permissions of the roles they hold. The owner holds everything whatever
/// roles they carry, so no role can take their control away. `None` for
/// someone who is not in the space.
pub fn effective(space: &Space, user: Id) -> Option<u64> {
    let Some(member) = space.members.get(&user) else { return None };
    if user == space.owner {
        return Some(ALL);
    }
    Some(member.roles.iter().fold(space.everyone, |acc, role| union(acc, role_permissions(space, *role))))
}

/// Whether `user` may do `what` in `space`.
pub fn can(space: &Space, user: Id, what: Permission) -> bool {
    effective(space, user).is_some_and(|permissions| has(permissions, what))
}

/// What `user` may do in `channel`, given the space it is in (if any).
///
/// A thread inherits its parent channel's rules — pass the parent's — except
/// that an archived thread is read-only to everyone.
pub fn channel_access(user: Id, channel: &Channel, space: Option<&Space>) -> Access {
    if channel.kind.is_private() {
        return if channel.members.contains(&user) { Access::Full } else { Access::None };
    }
    let Some(permissions) = space.and_then(|s| at_channel(user, channel, s)) else {
        return Access::None;
    };
    // The ViewChannel and SendMessage bits are gates in their own right, on top
    // of the channel's masks: a role, the space baseline or a channel overwrite
    // that takes them away locks or mutes the channel, whatever the masks say.
    let view = channel.view | Permission::ViewChannel.bit();
    let post = view | channel.post | Permission::SendMessage.bit();
    if !covers(permissions, view) {
        Access::Locked
    } else if !covers(permissions, post) || channel.archived {
        Access::ReadOnly
    } else {
        Access::Full
    }
}

/// The set of what `user` may do inside `channel` of `space`: their space
/// base, with each of their roles' channel overwrites applied — the allow
/// sets widen, the deny sets cut afterwards. The owner and anyone holding
/// the admin tag ignore the overwrites: their reach is the space's whole
/// power, and a channel cannot demote them.
pub fn at_channel(user: Id, channel: &Channel, space: &Space) -> Option<u64> {
    let Some(member) = space.members.get(&user) else { return None };
    let base = effective(space, user).unwrap_or(EVERYONE);
    // The owner and an admin ignore the overwrites: an effective set
    // carrying the admin tag is ALL, so the test below catches both.
    if user == space.owner || covers(base, ADMIN) {
        return Some(base);
    }
    let allow = member.roles.iter().fold(EVERYONE, |acc, role| acc | channel.overwrites.get(&role).map(|o| o.allow).unwrap_or(0));
    let deny = member.roles.iter().fold(EVERYONE, |acc, role| acc | channel.overwrites.get(&role).map(|o| o.deny).unwrap_or(0));
    Some((base | allow) & !deny)
}

/// The rank a member acts at: the position of their most senior role — a
/// lower `position` is more senior — or `None` for a member holding no roles,
/// who sits at the bottom of the ladder.
pub fn top_position(space: &Space, user: Id) -> Option<i32> {
    space.members.get(&user).and_then(|m|
        m.roles
            .iter()
            .filter_map(|r| space.roles.get(r).map(|role| role.position))
            .min())
}

/// Removing someone from a space: the [`Permission::ManageMembers`] bit buys
/// it (the owner always may), and only below their own rank: a member
/// can only reach those under their top role. The target must be in the space,
/// and the owner is never a target — ownership is transferred, not revoked, so
/// no role can demote them.
pub fn can_moderate(space: &Space, actor: Id, target: Id) -> bool {
    space.members.contains_key(&target)
        && target != space.owner
        && (actor == space.owner
            || can(space, actor, Permission::ManageMembers) && reaches(space, actor, target))
}

/// Whether `actor` sits strictly above `target`, so the target is their
/// subordinate: the owner reaches everyone, a member with roles reaches those
/// whose top role is junior to their own and those with no roles, and a
/// member with no roles reaches nobody. Peers never reach each other.
pub fn reaches(space: &Space, actor: Id, target: Id) -> bool {
    actor == space.owner
        || match (top_position(space, actor), top_position(space, target)) {
            (Some(a), Some(t)) => t > a,
            (Some(_), None) => true,
            (None, _) => false,
        }
}

/// Giving someone a role: [`Permission::ManageRoles`] buys it (the owner
/// always may), and only up to what the actor can already do — an actor
/// cannot hand out power they do not have. The target is the same one a
/// moderation act may touch: never the owner.
pub fn can_grant(space: &Space, actor: Id, target: Id, to: Id) -> bool {
    can_moderate(space, actor, target)
        && can(space, actor, Permission::ManageRoles)
        && covers(effective(space, actor).unwrap_or(0), role_permissions(space, to))
        && space.roles.get(&to).is_some_and(|role| reaches_role(space, actor, role.position))
}

/// Setting several roles at once: the actor may moderate the target and
/// manage roles — clearing a member's roles is an act too — and every role
/// handed out passes [`can_grant`], so a batch cannot smuggle in power the
/// actor does not have.
pub fn can_set_roles(space: &Space, actor: Id, target: Id, roles: &[Id]) -> bool {
    can_moderate(space, actor, target)
        && can(space, actor, Permission::ManageRoles)
        && roles.iter().all(|to| can_grant(space, actor, target, *to))
}

/// Creating a role: [`Permission::ManageRoles`] buys it (the owner always
/// may), and the role cannot carry power the actor does not hold. A new role
/// sits at the bottom of the ladder, so hierarchy never blocks creating one.
pub fn can_author_role(space: &Space, actor: Id, permissions: PermissionSet) -> bool {
    can(space, actor, Permission::ManageRoles)
        && covers(effective(space, actor).unwrap_or(0), expand(permissions))
}

/// Whether `actor` may touch a role at `position`: the owner is exempt, and a
/// member reaches only roles strictly junior to their own top role.
pub fn reaches_role(space: &Space, actor: Id, position: i32) -> bool {
    actor == space.owner
        || top_position(space, actor).is_some_and(|top| position > top)
}

/// Editing or deleting the role `role`: [`Permission::ManageRoles`] buys it
/// (the owner always may), and the role sits at or below the actor's own rank.
pub fn can_edit_role(space: &Space, actor: Id, role: Id) -> bool {
    can(space, actor, Permission::ManageRoles)
        && reaches_role(space, actor, space.roles.get(&role).map(|r| r.position).unwrap_or(0))
}

/// Banning someone: their space role's [`Permission::BanMembers`] bit buys
/// it (the owner always may) and only below their own rank, like any
/// other act on a member. Never the owner — they are moved by transfer.
pub fn can_ban(space: &Space, actor: Id, target: Id) -> bool {
    space.members.contains_key(&target)
        && target != space.owner
        && (actor == space.owner
            || (can(space, actor, Permission::BanMembers) && reaches(space, actor, target)))
}

/// Timing someone out: their space role's [`Permission::TimeoutMembers`] bit
/// buys it (the owner always may), below their own rank. The target
/// must be a member, and never the owner.
pub fn can_timeout(space: &Space, actor: Id, target: Id) -> bool {
    space.members.contains_key(&target)
        && target != space.owner
        && (actor == space.owner
            || (can(space, actor, Permission::TimeoutMembers) && reaches(space, actor, target)))
}

/// Whether `user` may manage a space's settings through an optional space.
pub fn manages(space: &Option<Space>, user: Id) -> bool {
    space.as_ref().is_some_and(|s| can(s, user, Permission::ManageSpaceSettings))
}

/// Whether `user` may set who sees and posts in a channel, through an
/// optional space: [`Permission::ManageChannelRoles`] buys it.
pub fn manages_channel(space: &Option<Space>, user: Id) -> bool {
    space.as_ref().is_some_and(|s| can(s, user, Permission::ManageChannelRoles))
}

// ---- per-act rules, one copy each: the server decides, the client hides ----

/// Whether `user` may do `what` in `space`, ignoring channels. Space-wide
/// acts (managing settings, invites, transfers) use this; acts taken at a
/// channel use [`can_at`] so the channel's overwrites apply.
pub fn can_act(space: Option<&Space>, user: Id, what: Permission) -> bool {
    space.is_none_or(|s| can(s, user, what))
}

/// Whether `user` may do `what` at `channel`: over the set the channel's
/// per-role overwrites adjust, so a role can be widened or cut here without
/// moving anywhere else in the space. Outside a space — a direct or group
/// conversation — there are no roles to consult, and the act is open to
/// anyone the conversation itself admits.
pub fn can_at(channel: &Channel, space: Option<&Space>, user: Id, what: Permission) -> bool {
    match space {
        Some(s) => at_channel(user, channel, s).is_some_and(|permissions| has(permissions, what)),
        None => true,
    }
}

/// Reacting to a message at a channel: [`Permission::AddReaction`] buys it.
pub fn can_react(channel: &Channel, space: Option<&Space>, user: Id) -> bool {
    can_at(channel, space, user, Permission::AddReaction)
}

/// Attaching a file to a message: [`Permission::AttachFile`] buys it. The
/// upload itself is not the act — a file is only attached when the message
/// that claims it is posted — so the server checks where the message lands.
pub fn can_attach(channel: &Channel, space: Option<&Space>, user: Id) -> bool {
    can_at(channel, space, user, Permission::AttachFile)
}

/// Posting a link a reader expands: [`Permission::EmbedLink`] buys it. The
/// server sees the act because it reads the message bodies it is handed
/// (`send` and `edit` resolve mentions in the cleartext they were given), so
/// this is checked where the act is, not guessed from encrypted content.
pub fn can_embed(channel: &Channel, space: Option<&Space>, user: Id) -> bool {
    can_at(channel, space, user, Permission::EmbedLink)
}

/// Joining a channel's call: [`Permission::VoiceConnect`] buys it.
pub fn can_connect(channel: &Channel, space: Option<&Space>, user: Id) -> bool {
    can_at(channel, space, user, Permission::VoiceConnect)
}

/// Unmuting their microphone in a call: [`Permission::VoiceSpeak`] buys it.
pub fn can_speak(channel: &Channel, space: Option<&Space>, user: Id) -> bool {
    can_at(channel, space, user, Permission::VoiceSpeak)
}

/// Using a camera in a call: [`Permission::VoiceVideo`] buys it.
pub fn can_video(channel: &Channel, space: Option<&Space>, user: Id) -> bool {
    can_at(channel, space, user, Permission::VoiceVideo)
}

/// Sharing a screen in a call: [`Permission::VoiceShareScreen`] buys it.
pub fn can_share(channel: &Channel, space: Option<&Space>, user: Id) -> bool {
    can_at(channel, space, user, Permission::VoiceShareScreen)
}

/// [`Permission::VoiceForceMute`] and [`Permission::VoiceMoveMember`] act on
/// other people's calls, and this stack has no verb that does: muting in
/// `update_call` is self-service, and the relay only takes a person out when
/// their own connection drops. There is no server site to check the bits
/// against, so no check is added — a gate over a control that does not exist
/// would be a fake one. The bits stay defined for the day those controls
/// arrive, where they will gate through `can`.

/// Making an invite: [`Permission::ManageInvites`] buys it (the owner always
/// may, holding everything). Revoking one is the same act. Invites belong to
/// the space, not a channel, so no overwrites apply.
pub fn can_invite(space: &Space, user: Id) -> bool {
    can(space, user, Permission::ManageInvites)
}

/// Whether `actor` may hand the space to `target`: the owner only, to a
/// member who is not already the owner. Ownership never comes from a role,
/// so no permission buys this.
pub fn can_transfer(space: &Space, actor: Id, target: Id) -> bool {
    actor == space.owner && target != space.owner && space.members.contains_key(&target)
}

/// Deleting a message: its author, where the channel's set for
/// [`Permission::DeleteOwnMessage`] allows it (in a conversation, outside any
/// space, there are no roles to consult, so the author may), and anyone whose
/// space set covers [`Permission::DeleteOthersMessage`] for someone else's.
pub fn can_delete_message(channel: &Channel, space: Option<&Space>, user: Id, author: Id) -> bool {
    (user == author && can_at(channel, space, user, Permission::DeleteOwnMessage))
        || space.is_some_and(|s| can(s, user, Permission::DeleteOthersMessage))
}

/// Editing a message: only its author, and only with the
/// [`Permission::EditOwnMessage`] permission at the channel — outside a space
/// the act is open to the author.
pub fn can_edit_message(channel: &Channel, space: Option<&Space>, user: Id, author: Id) -> bool {
    user == author && can_at(channel, space, user, Permission::EditOwnMessage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Channel, ChannelKind, Member, Overwrite, Role, Space};
    use std::collections::BTreeMap;

    fn id(n: u128) -> Id {
        Id::from_u128(n)
    }

    const ADMIN: u64 = Permission::IsAdmin.bit();
    const EVERYONE: u64 = 0;
    const POSTING: u64 = Permission::ViewChannel.bit() | Permission::SendMessage.bit();

    /// A space with roles `(id, permissions)` and members
    /// `(user, is_owner, [role ids])`; user 1 is the owner.
    fn space(roles: &[(u128, u64)], members: &[(u128, bool, Vec<u128>)]) -> Space {
        Space {
            id: id(100),
            name: "s".into(),
            icon: None,
            owner: id(1),
            everyone: 0,
            members: members
                .iter()
                .map(|(u, _, rs)| (id(*u), Member { roles: rs.iter().map(|&n| id(n)).collect(), joined_at: 0 }))
                .collect::<BTreeMap<_, _>>(),
            roles: roles
                .iter()
                .map(|&(r, p)| (id(r), Role { id: id(r), name: "r".into(), permissions: p, position: r as i32 }))
                .collect::<BTreeMap<_, _>>(),
            categories: vec![],
            created_at: 0,
        }
    }

    /// A channel with one role's allow/deny pair set on it.
    fn with_overwrites(c: Channel, role: Id, allow: u64, deny: u64) -> Channel {
        Channel {
            overwrites: [(role, Overwrite { allow, deny })].into_iter().collect(),
            ..c
        }
    }

    fn channel(kind: ChannelKind, view: u64, post: u64) -> Channel {
        Channel {
            id: id(200),
            kind,
            space: Some(id(100)),
            name: "c".into(),
            topic: String::new(),
            category: None,
            position: 0,
            parent: None,
            starter: None,
            view,
            post,
            members: vec![],
            archived: false,
            overwrites: BTreeMap::new(),
            last_message: None,
            message_count: 0,
            created_by: id(1),
            created_at: 0,
        }
    }

    #[test]
    fn a_channel_overwrite_lets_one_role_post_where_the_post_mask_forbids() {
        let s = space(&[(8, Permission::ViewChannel.bit())], &[(1, true, vec![]), (2, false, vec![8])]);
        let mut c = channel(ChannelKind::Text, EVERYONE, Permission::SendMessage.bit());
        c = with_overwrites(c, id(8), Permission::SendMessage.bit() | Permission::AddReaction.bit(), EVERYONE);
        assert_eq!(channel_access(id(2), &c, Some(&s)), Access::Full);
        assert!(can_react(&c, Some(&s), id(2)), "and the widened set reaches the act checks");
    }

    #[test]
    fn a_channel_overwrite_cuts_a_permission_the_role_holds_wide() {
        let s = space(&[(7, POSTING | Permission::EmbedLink.bit())], &[(1, true, vec![]), (2, false, vec![7])]);
        let mut c = channel(ChannelKind::Text, EVERYONE, Permission::SendMessage.bit());
        c = with_overwrites(c, id(7), EVERYONE, Permission::EmbedLink.bit());
        assert!(can(&s, id(2), Permission::EmbedLink), "space-wide the role holds the bit");
        assert!(!can_embed(&c, Some(&s), id(2)), "but not at this channel");
    }

    #[test]
    fn the_owner_shrugs_the_overwrites_off() {
        let s = space(&[(7, POSTING)], &[(1, true, vec![7])]);
        let mut c = channel(ChannelKind::Text, EVERYONE, Permission::SendMessage.bit());
        c = with_overwrites(c, id(7), EVERYONE, Permission::SendMessage.bit());
        assert_eq!(channel_access(id(1), &c, Some(&s)), Access::Full, "even a deny on their own role");
    }

    #[test]
    fn a_member_below_the_view_mask_sees_the_channel_locked() {
        let s = space(&[(7, POSTING)], &[(1, true, vec![]), (2, false, vec![7])]);
        let c = channel(ChannelKind::Text, ADMIN, ADMIN);
        assert_eq!(channel_access(id(2), &c, Some(&s)), Access::Locked);
        assert_eq!(channel_access(id(1), &c, Some(&s)), Access::Full);
    }

    #[test]
    fn a_member_below_the_post_mask_reads_only() {
        let s = space(&[(7, POSTING), (8, ADMIN)], &[(2, false, vec![7]), (3, false, vec![8])]);
        let c = channel(ChannelKind::Text, EVERYONE, ADMIN);
        assert_eq!(channel_access(id(2), &c, Some(&s)), Access::ReadOnly);
        assert_eq!(channel_access(id(3), &c, Some(&s)), Access::Full);
    }

    #[test]
    fn a_stranger_has_no_access_to_a_space_channel() {
        let s = space(&[], &[(2, false, vec![])]);
        let c = channel(ChannelKind::Text, EVERYONE, EVERYONE);
        assert_eq!(channel_access(id(9), &c, Some(&s)), Access::None);
    }

    #[test]
    fn an_archived_thread_is_read_only_even_to_its_owner() {
        let s = space(&[], &[(1, true, vec![])]);
        let mut c = channel(ChannelKind::Thread, EVERYONE, EVERYONE);
        c.archived = true;
        assert_eq!(channel_access(id(1), &c, Some(&s)), Access::ReadOnly);
    }

    #[test]
    fn a_direct_conversation_is_open_only_to_its_members() {
        let mut c = channel(ChannelKind::Direct, EVERYONE, EVERYONE);
        c.space = None;
        c.members = vec![id(1), id(2)];
        assert_eq!(channel_access(id(1), &c, None), Access::Full);
        assert_eq!(channel_access(id(3), &c, None), Access::None);
    }

    #[test]
    fn a_role_without_send_cannot_post_where_one_with_it_can() {
        let s = space(&[(7, Permission::ViewChannel.bit()), (8, POSTING)], &[(2, false, vec![7]), (3, false, vec![8])]);
        let c = channel(ChannelKind::Text, Permission::ViewChannel.bit(), Permission::SendMessage.bit());
        assert_eq!(channel_access(id(2), &c, Some(&s)), Access::ReadOnly);
        assert_eq!(channel_access(id(3), &c, Some(&s)), Access::Full);
    }

    #[test]
    fn the_baseline_reaches_a_member_with_no_roles() {
        let mut s = space(&[], &[(2, false, vec![])]);
        s.everyone = DEFAULT_EVERYONE;
        assert!(
            has(effective(&s, id(2)).unwrap_or(EVERYONE), Permission::ViewChannel),
            "the space's baseline holds for a member with no roles",
        );
        assert!(
            !has(effective(&s, id(2)).unwrap_or(EVERYONE), Permission::ManageMembers),
            "but the baseline stops where the acts on others begin",
        );
    }

    #[test]
    fn a_space_can_raise_its_baseline() {
        let base = space(&[], &[(2, false, vec![])]);
        let mut raised = space(&[], &[(2, false, vec![])]);
        raised.everyone = DEFAULT_EVERYONE | Permission::ManageMembers.bit();
        assert!(!has(effective(&base, id(2)).unwrap_or(EVERYONE), Permission::ManageMembers));
        assert!(has(effective(&raised, id(2)).unwrap_or(EVERYONE), Permission::ManageMembers));
    }

    #[test]
    fn the_new_bits_stay_inside_all() {
        assert_eq!(
            Permission::BanMembers.bit() | Permission::TimeoutMembers.bit() | Permission::IsAdmin.bit() | (1u64 << 22) - 1,
            ALL,
            "the ban and timeout bits sit in the space and ALL covers exactly the defined bits",
        );
        for p in [Permission::BanMembers, Permission::TimeoutMembers] {
            assert!((ALL & p.bit()) == p.bit(), "ALL holds every defined bit");
        }
        for unused in 24..63 {
            assert_eq!(ALL & (1u64 << unused), 0, "no invented bit slipped into ALL");
        }
    }

    #[test]
    fn two_roles_add_up() {
        let s = space(&[(7, Permission::ViewChannel.bit()), (8, Permission::SendMessage.bit())], &[(2, false, vec![7, 8])]);
        assert_eq!(effective(&s, id(2)), Some(POSTING));
    }

    #[test]
    fn an_admin_role_carries_every_permission() {
        let s = space(&[(7, ADMIN)], &[(2, false, vec![7])]);
        assert_eq!(effective(&s, id(2)), Some(ALL));
    }

    #[test]
    fn the_owner_holds_everything_even_with_no_roles() {
        let s = space(&[(7, Permission::ViewChannel.bit())], &[(1, true, vec![])]);
        assert_eq!(effective(&s, id(1)), Some(ALL));
        assert!(
            has(effective(&s, id(1)).unwrap_or(EVERYONE), Permission::ManageMembers),
            "the owner moderates whatever their roles say",
        );
    }

    #[test]
    fn a_role_cannot_demote_the_owner() {
        let s = space(&[(7, Permission::ViewChannel.bit())], &[(1, true, vec![7]), (2, false, vec![])]);
        assert!(can_moderate(&s, id(1), id(2)), "the owner moderates even a non-admin role");
        assert!(!can_moderate(&s, id(1), id(1)), "and never reaches the owner themselves");
    }

    #[test]
    fn moderation_never_reaches_the_owner_themselves() {
        let s = space(&[(7, ADMIN)], &[(1, true, vec![]), (2, false, vec![7]), (3, false, vec![])]);
        assert!(can_moderate(&s, id(2), id(3)), "an admin can manage members below them");
        assert!(!can_moderate(&s, id(2), id(1)), "but never the owner");
        assert!(!can_moderate(&s, id(2), id(2)), "and not themselves: nobody is below their own rank");
        assert!(!can_moderate(&s, id(9), id(2)), "a stranger manages nothing");
    }

    #[test]
    fn only_role_managers_hand_out_roles() {
        let s = space(&[(7, ADMIN), (8, Permission::ManageChannels.bit())], &[(2, false, vec![8]), (1, true, vec![])]);
        assert!(can_grant(&s, id(1), id(2), id(7)), "the owner always can");
        assert!(!can_grant(&s, id(2), id(2), id(7)), "a channel manager without ManageRoles cannot");
    }

    #[test]
    fn transferring_a_space_needs_its_owner_and_a_member() {
        let s = space(&[], &[(1, true, vec![]), (2, false, vec![])]);
        assert!(can_transfer(&s, id(1), id(2)));
        assert!(!can_transfer(&s, id(2), id(1)), "an admin cannot hand it over");
        assert!(!can_transfer(&s, id(1), id(1)), "the owner already holds it");
    }

    #[test]
    fn deleting_another_s_message_needs_the_permission() {
        let s = space(&[(7, ADMIN)], &[(1, true, vec![]), (2, false, vec![7])]);
        let c = channel(ChannelKind::Text, EVERYONE, EVERYONE);
        assert!(
            can_delete_message(&c, Some(&s), id(1), id(2)),
            "the admin bit buys delete-others",
        );
        assert!(
            !can_delete_message(&c, Some(&s), id(9), id(2)),
            "a stranger deletes nothing",
        );
        assert!(
            can_delete_message(&c, None, id(2), id(2)),
            "an author deletes their own even with no space",
        );
    }

    #[test]
    fn clearing_a_member_s_roles_is_an_act_the_role_manager_makes() {
        let s = space(
            &[(7, ADMIN), (8, POSTING)],
            &[(1, true, vec![]), (2, false, vec![7]), (3, false, vec![8]), (4, false, vec![])],
        );
        assert!(can_set_roles(&s, id(1), id(2), &vec![]), "the owner strips roles as well as hands them out");
        assert!(can_set_roles(&s, id(2), id(3), &vec![]), "an admin strips the roles of someone below them");
        assert!(can_set_roles(&s, id(2), id(4), &vec![id(8)]), "and hands out a role junior to their own");
        assert!(!can_set_roles(&s, id(2), id(4), &vec![id(7)]), "but not a role at their own rank");
        assert!(!can_set_roles(&s, id(3), id(2), &vec![]), "a poster strips nothing");
        assert!(!can_set_roles(&s, id(1), id(1), &vec![id(7)]), "not even the owner strips their own");
    }

    #[test]
    fn a_role_manager_cannot_author_a_role_beyond_their_powers() {
        let s = space(&[(7, Permission::ManageRoles.bit() | Permission::SendMessage.bit()), (8, ADMIN)],
                      &[(1, true, vec![]), (2, false, vec![7])]);
        assert!(can_author_role(&s, id(1), ADMIN), "the owner authors anything");
        assert!(can_author_role(&s, id(2), Permission::ManageRoles.bit()), "a role manager authors their own bit");
        assert!(!can_author_role(&s, id(2), Permission::ManageRoles.bit() | Permission::DeleteOthersMessage.bit()),
                "but not a power they do not hold");
        assert!(!can_author_role(&s, id(2), ADMIN), "and not the admin tag");
    }
}