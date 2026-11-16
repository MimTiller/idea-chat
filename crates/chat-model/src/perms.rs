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

use crate::{Channel, Id, Space};

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
    /// Kick, ban and time out members.
    ManageMembers,
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
            Permission::IsAdmin => 63,
        }
    }
}

/// Every permission: what the owner holds, and what a role tagged admin
/// expands to.
pub const ALL: u64 = (1u64 << 63) | ((1u64 << 22) - 1);

/// The admin tag alone: what a channel asks of its admins, and what a
/// permission set must cover to count as an admin (for the roster).
pub const ADMIN: u64 = Permission::IsAdmin.bit();

/// The empty mask: what a channel asks of everyone in the space.
pub const EVERYONE: u64 = 0;

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

/// What `user` may do in `space`: the union of the permissions of the roles
/// they hold. The owner holds everything whatever roles they carry, so no
/// role can take their control away. `None` for someone who is not in the
/// space.
pub fn effective(space: &Space, user: Id) -> Option<u64> {
    let Some(member) = space.members.get(&user) else { return None };
    if user == space.owner {
        return Some(ALL);
    }
    Some(member.roles.iter().fold(EVERYONE, |acc, role| union(acc, role_permissions(space, *role))))
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
    let Some(permissions) = space.and_then(|s| effective(s, user)) else {
        return Access::None;
    };
    if !covers(permissions, channel.view) {
        Access::Locked
    } else if !covers(permissions, channel.view | channel.post) || channel.archived {
        Access::ReadOnly
    } else {
        Access::Full
    }
}

/// Removing someone from a space: the [`Permission::ManageMembers`] bit buys
/// it (the owner always may). The target must be in the space, and the owner
/// is never a target — ownership is transferred, not revoked, so no role can
/// demote them.
pub fn can_moderate(space: &Space, actor: Id, target: Id) -> bool {
    space.members.contains_key(&target)
        && target != space.owner
        && (actor == space.owner || can(space, actor, Permission::ManageMembers))
}

/// Giving someone a role: [`Permission::ManageRoles`] buys it (the owner
/// always may), and only up to what the actor can already do — an actor
/// cannot hand out power they do not have. The target is the same one a
/// moderation act may touch: never the owner.
pub fn can_grant(space: &Space, actor: Id, target: Id, to: Id) -> bool {
    can_moderate(space, actor, target)
        && can(space, actor, Permission::ManageRoles)
        && covers(effective(space, actor).unwrap_or(0), role_permissions(space, to))
}

/// Setting several roles at once: as [`can_grant`] for each role handed out,
/// so a batch grant cannot smuggle in power the actor does not have.
pub fn can_set_roles(space: &Space, actor: Id, target: Id, roles: &[Id]) -> bool {
    roles.iter().all(|to| can_grant(space, actor, target, *to))
}

/// Whether `user` may manage a space's settings through an optional space.
pub fn manages(space: &Option<Space>, user: Id) -> bool {
    space.as_ref().is_some_and(|s| can(s, user, Permission::ManageSpaceSettings))
}

/// Whether `actor` may hand the space to `target`: the owner only, to a
/// member who is not already the owner. Ownership never comes from a role,
/// so no permission buys this.
pub fn can_transfer(space: &Space, actor: Id, target: Id) -> bool {
    actor == space.owner && target != space.owner && space.members.contains_key(&target)
}

/// Deleting a message: its author always (a role's
/// [`Permission::DeleteOwnMessage`] governs whether they may), and anyone
/// whose set covers [`Permission::DeleteOthersMessage`] for someone else's.
pub fn can_delete_message(space: Option<&Space>, user: Id, author: Id) -> bool {
    user == author
        || space.is_some_and(|s| can(s, user, Permission::DeleteOthersMessage))
}

/// Editing a message: only its author, and only with the permission.
pub fn can_edit_message(space: Option<&Space>, user: Id, author: Id) -> bool {
    user == author
        && space.is_some_and(|s| can(s, user, Permission::EditOwnMessage))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Channel, ChannelKind, Member, Role, Space};
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
            members: members
                .iter()
                .map(|(u, _, rs)| (id(*u), Member { roles: rs.iter().map(|&n| id(n)).collect(), joined_at: 0 }))
                .collect::<BTreeMap<_, _>>(),
            roles: roles
                .iter()
                .map(|&(r, p)| (id(r), Role { id: id(r), name: "r".into(), permissions: p, position: 0 }))
                .collect::<BTreeMap<_, _>>(),
            categories: vec![],
            created_at: 0,
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
            last_message: None,
            message_count: 0,
            created_by: id(1),
            created_at: 0,
        }
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
        let s = space(&[(7, ADMIN)], &[(1, true, vec![]), (2, false, vec![7])]);
        assert!(can_moderate(&s, id(2), id(2)), "an admin can manage members");
        assert!(!can_moderate(&s, id(3), id(2)), "a stranger manages nothing");
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
        assert!(can_delete_message(Some(&s), id(1), id(2)), "the admin bit buys delete-others");
        assert!(!can_delete_message(Some(&s), id(9), id(2)), "a stranger deletes nothing");
        assert!(can_delete_message(None, id(2), id(2)), "an author deletes their own even with no space");
    }
}
