//! The gate every role and member verb checks, over spaces built in the
//! shapes the server can hold. Each `#[server]` verb makes exactly one of
//! these `chat_model::perms` calls before it touches the db, so covering
//! the call — allow for the one who may, refusal for the one who may not —
//! covers the verb without reaching through the session wire.

use chat_model::{Channel, ChannelKind, Id, Member, Overwrite, Role, Space};
use chat_model::perms::{
    ALL,
    Access,
    channel_access,
    ADMIN,
    DEFAULT_EVERYONE,
    DEFAULT_MODERATOR,
    EVERYONE,
    Permission,
    at_channel,
    can,
    can_act,
    can_at,
    can_attach,
    can_author_role,
    can_ban,
    can_connect,
    can_delete_message,
    can_edit_message,
    can_edit_role,
    can_embed,
    can_grant,
    can_invite,
    can_moderate,
    can_react,
    can_set_roles,
    can_share,
    can_speak,
    can_timeout,
    can_video,
    can_transfer,
    covers,
    effective,
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
        ms
            .insert(
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
                        colour: None,
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

/// The same channel with one role's allow/deny pair set on it.
fn over(c: Channel, role: Id, allow: u64, deny: u64) -> Channel {
    Channel {
        overwrites: [(role, Overwrite {
            allow,
            deny,
        })].into_iter().collect(),
        ..c
    }
}

// -------------------------------------------------------------- create_role

#[test]
fn a_role_creator_authors_only_powers_they_hold() {
    const MOD: u64 = Permission::ManageRoles.bit();
    let s = space(
        &[(7u128, MOD, 0i32)],
        &[(2u128, vec![7u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(
        can_author_role(&s, id(2), MOD),
        "the boss authors a copy of their own bit"
    );
    assert!(
        !can_author_role(&s, id(2), MOD | Permission::ManageMembers.bit()),
        "a role may not carry power the creator does not hold"
    );
    assert!(
        !can_author_role(&s, id(4), MOD),
        "a plain member authors nothing"
    );
    assert!(can_author_role(&s, id(1), ALL), "the owner may author anything");
}

// -------------------------------------------------------------- update_role

#[test]
fn updating_a_role_needs_reach_and_the_new_powers_held() {
    const MOD: u64 = Permission::ManageRoles.bit();
    let s = space(
        &[(7u128, MOD, 0i32), (8u128, MOD, 1i32)],
        &[(2u128, vec![7u128]), (3u128, vec![8u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(can_edit_role(&s, id(2), id(8)), "a senior member edits the role below theirs");
    assert!(
        !can_edit_role(&s, id(3), id(8)),
        "nobody edits their own top role: it sits at their own rank, not below it"
    );
    assert!(
        !can_edit_role(&s, id(3), id(7)),
        "they cannot rewrite the senior role above them"
    );
    assert!(can_edit_role(&s, id(1), id(7)), "the owner is exempt from the ladder");
    assert!(
        !can_author_role(&s, id(2), MOD | Permission::ManageSpaceSettings.bit()),
        "re-powering smuggles nothing the boss lacks"
    );
}

// -------------------------------------------------------------- delete_role

#[test]
fn deleting_a_role_uses_the_same_reach_as_editing_it() {
    const MOD: u64 = Permission::ManageRoles.bit();
    let s = space(
        &[(7u128, MOD, 0i32), (8u128, MOD, 1i32)],
        &[(2u128, vec![7u128]), (3u128, vec![8u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(can_edit_role(&s, id(2), id(8)), "their subordinate role comes off");
    assert!(
        !can_edit_role(&s, id(2), id(7)),
        "the role at their own top is not below them, so it is not theirs to delete"
    );
    assert!(
        !can_edit_role(&s, id(4), id(8)),
        "a plain member deletes nobody's role"
    );
}

// -------------------------------------------------------------- set_roles

#[test]
fn handing_out_roles_needs_the_grant_bit_and_no_smuggled_power() {
    const POWERS: u64 = Permission::ManageRoles.bit() | Permission::ManageMembers.bit();
    let s = space(
        &[(7u128, POWERS, 0i32), (8u128, Permission::ManageMembers.bit(), 1i32)],
        &[(2u128, vec![7u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(
        can_set_roles(&s, id(2), id(4), &[id(8)]),
        "the boss hands a junior role down"
    );
    assert!(
        !can_set_roles(&s, id(2), id(4), &[id(7)]),
        "a role at the boss's own rank is not below them, so it is not theirs to hand out"
    );
    assert!(
        !can_set_roles(&s, id(4), id(2), &[id(7)]),
        "a plain member hands nothing out"
    );
    assert!(
        !can_set_roles(&s, id(2), id(1), &[]),
        "the owner's roles are not the boss's to set"
    );
    assert!(
        can_set_roles(&s, id(1), id(2), &[id(7)]),
        "the owner assigns anything they hold"
    );
}

#[test]
fn grant_refuses_roles_carrying_unheld_power() {
    const MOD: u64 = Permission::ManageRoles.bit() | Permission::ManageMembers.bit();
    const WIDE: u64 = MOD | Permission::ManageSpaceSettings.bit();
    // The boss holds role 9 at the top; roles 7 and 8 sit below it.
    let s = space(
        &[(9u128, MOD, 0i32), (7u128, MOD, 1i32), (8u128, WIDE, 1i32)],
        &[(2u128, vec![9u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(
        can_grant(&s, id(2), id(4), id(7)),
        "handing a junior role down is allowed when it carries only powers the boss holds"
    );
    assert!(
        !can_grant(&s, id(2), id(4), id(8)),
        "the fat role carries power the boss lacks"
    );
    assert!(
        !can_set_roles(&s, id(2), id(4), &[id(8)]),
        "a batch cannot smuggle it in"
    );
    assert!(
        !can_grant(&s, id(2), id(4), id(9)),
        "the boss's own top role is at their rank, not below it"
    );
}

// -------------------------------------------------------------- remove_member / escalate

#[test]
fn removing_members_needs_the_moderation_bit_at_rank() {
    const MEMBER_POWER: u64 = Permission::ManageMembers.bit();
    let s = space(
        &[(7u128, MEMBER_POWER, 0i32)],
        &[(2u128, vec![7u128]), (3u128, vec![]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(can_moderate(&s, id(2), id(4)), "a mod removes a plain member");
    assert!(
        !can_moderate(&s, id(4), id(2)),
        "a plain member removes nobody"
    );
    assert!(can_moderate(&s, id(1), id(2)), "the owner moderates everyone below");
    assert!(
        !can_moderate(&s, id(2), id(1)),
        "the owner is never a target"
    );
    // A mod above a mod: the senior reaches the junior, never the reverse.
    let ladder = space(
        &[(7u128, MEMBER_POWER, 0i32), (8u128, MEMBER_POWER, 1i32)],
        &[(2u128, vec![7u128]), (3u128, vec![8u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(can_moderate(&ladder, id(2), id(3)), "the top mod moderates their junior");
    assert!(
        !can_moderate(&ladder, id(3), id(2)),
        "the junior cannot touch their senior"
    );
    assert!(can_moderate(&ladder, id(3), id(4)), "the junior moderates plain members");
}

// -------------------------------------------------------------- ban / unban / timeout

#[test]
fn ban_timeout_and_unban_gate_on_their_new_bits_at_rank() {
    const POWERS: u64 = Permission::ManageMembers.bit()
        | Permission::BanMembers.bit()
        | Permission::TimeoutMembers.bit();
    let s = space(
        &[(7u128, POWERS, 0i32)],
        &[(2u128, vec![7u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(can_ban(&s, id(2), id(4)), "a mod bans a plain member");
    assert!(can_timeout(&s, id(2), id(4)), "a mod times one out");
    assert!(
        !can_ban(&s, id(4), id(2)),
        "a plain member bans nobody"
    );
    assert!(
        !can_timeout(&s, id(4), id(2)),
        "a plain member times nobody out"
    );
    assert!(
        can(&s, id(1), Permission::BanMembers),
        "the owner holds every bit, unbanning anyone"
    );
    assert!(
        !can(&s, id(4), Permission::BanMembers),
        "unban asks the same bit they never hold"
    );
    const TIMEOUT: u64 = Permission::TimeoutMembers.bit();
    assert!(
        covers(effective(&s, id(2)).unwrap_or(EVERYONE), TIMEOUT),
        "the timeout bit reaches through the role"
    );
}

#[test]
fn the_owner_is_never_a_ban_or_timeout_target() {
    let s = space(&[(7u128, ADMIN, 0i32)], &[(2u128, vec![7u128])], EVERYONE);
    assert!(!can_ban(&s, id(2), id(1)));
    assert!(!can_timeout(&s, id(2), id(1)));
    assert!(!can_grant(&s, id(2), id(1), id(7)));
    assert!(!can_moderate(&s, id(2), id(1)));
}

#[test]
fn transfer_needs_the_owner_alone() {
    let s = space(&[(7u128, ADMIN, 0i32)], &[(2u128, vec![7u128])], EVERYONE);
    assert!(can_transfer(&s, id(1), id(2)), "the owner hands over to a member");
    assert!(
        !can_transfer(&s, id(2), id(1)),
        "a member never owns the transfer"
    );
    assert!(
        !can_transfer(&s, id(1), id(1)),
        "a transfer to the owner is not a transfer"
    );
    assert!(
        !can_transfer(&s, id(1), id(99)),
        "the new owner must be in the space"
    );
}

// -------------------------------------------------------------- react

#[test]
fn reacting_needs_the_react_bit_at_the_channel() {
    let s = space(
        &[(7, Permission::AddReaction.bit(), 0)],
        &[(2u128, vec![7u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(can_react(&channel(), Some(&s), id(2)));
    assert!(
        !can_react(&channel(), Some(&s), id(4)),
        "a plain member reacts nowhere"
    );
    assert!(
        can_react(&channel(), None, id(4)),
        "a conversation outside a space admits its members"
    );
}

// -------------------------------------------------------------- attach

#[test]
fn attaching_a_file_needs_the_attach_bit() {
    let s = space(
        &[(7, Permission::AttachFile.bit(), 0)],
        &[(2u128, vec![7u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(can_attach(&channel(), Some(&s), id(2)));
    assert!(
        !can_attach(&channel(), Some(&s), id(4)),
        "a plain member attaches nothing"
    );
}

// -------------------------------------------------------------- embed

#[test]
fn posting_an_expandable_link_needs_the_embed_bit() {
    let s = space(
        &[(7, Permission::EmbedLink.bit(), 0)],
        &[(2u128, vec![7u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(can_embed(&channel(), Some(&s), id(2)));
    assert!(
        !can_embed(&channel(), Some(&s), id(4)),
        "a plain member posts no links"
    );
}

// -------------------------------------------------------------- voice

#[test]
fn joining_and_the_call_controls_gate_on_their_bits() {
    const VOICE: u64 = Permission::VoiceConnect.bit()
        | Permission::VoiceSpeak.bit()
        | Permission::VoiceVideo.bit()
        | Permission::VoiceShareScreen.bit();
    let s = space(&[(7u128, VOICE, 0i32)], &[(2u128, vec![7u128]), (4u128, vec![])], EVERYONE);
    assert!(can_connect(&channel(), Some(&s), id(2)));
    assert!(can_speak(&channel(), Some(&s), id(2)));
    assert!(can_video(&channel(), Some(&s), id(2)));
    assert!(can_share(&channel(), Some(&s), id(2)));
    assert!(
        !can_connect(&channel(), Some(&s), id(4)),
        "join refused"
    );
    assert!(
        !can_speak(&channel(), Some(&s), id(4)),
        "unmute refused"
    );
    assert!(
        !can_video(&channel(), Some(&s), id(4)),
        "camera refused"
    );
    assert!(
        !can_share(&channel(), Some(&s), id(4)),
        "screen share refused"
    );
}

#[test]
fn force_mute_and_move_have_no_site_to_check_yet() {
    // This stack offers no verb that mutes or moves another person, so the
    // bits gate nothing; a test asserting a refusal would be a fake gate.
    // Pin only that the bits are defined, distinct, and inside ALL.
    assert_eq!(
        Permission::VoiceForceMute.bit() & Permission::VoiceMoveMember.bit(),
        0
    );
    assert!(
        covers(ALL, Permission::VoiceForceMute.bit()),
        "ALL carries the voice-moderation bits"
    );
    assert!(covers(ALL, Permission::VoiceMoveMember.bit()));
}

// -------------------------------------------------------------- channel role masks

#[test]
fn setting_view_and_post_masks_needs_manage_channel_roles() {
    let s = space(
        &[(7, Permission::ManageChannelRoles.bit(), 0)],
        &[(2u128, vec![7u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(
        can_act(Some(&s), id(2), Permission::ManageChannelRoles),
        "the role buys it"
    );
    assert!(
        !can_act(Some(&s), id(4), Permission::ManageChannelRoles),
        "a plain member sets none"
    );
    assert!(
        !can_act(Some(&space(&[], &[], EVERYONE)), id(99), Permission::ManageChannelRoles),
        "a non-member holds nothing"
    );
    assert!(
        can_act(None, id(99), Permission::ManageChannelRoles),
        "outside a space the perms module has no role set to check; the api refuses"
    );
}

// -------------------------------------------------------------- invites

#[test]
fn making_seeing_and_revoking_invites_needs_the_invite_bit() {
    let s = space(
        &[(7, Permission::ManageInvites.bit(), 0)],
        &[(2u128, vec![7u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(can_invite(&s, id(2)));
    assert!(
        !can_invite(&s, id(4)),
        "a plain member manages no invites"
    );
    assert!(
        !can_invite(
            &space(&[], &[], EVERYONE),
            id(99)
        ),
        "a non-member gets nothing"
    );
}

// -------------------------------------------------------------- own message edit/delete

#[test]
fn editing_and_deleting_their_own_message_now_needs_their_bits() {
    const OWN: u64 = Permission::EditOwnMessage.bit() | Permission::DeleteOwnMessage.bit();
    let s = space(
        &[(7u128, OWN, 0i32)],
        &[(2u128, vec![7u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(can_edit_message(&channel(), Some(&s), id(2), id(2)));
    assert!(can_delete_message(&channel(), Some(&s), id(2), id(2)));
    assert!(
        !can_edit_message(&channel(), Some(&s), id(4), id(4)),
        "a plain member's own edits are refused"
    );
    assert!(
        !can_delete_message(&channel(), Some(&s), id(4), id(4)),
        "and their own deletes"
    );
    assert!(
        can_edit_message(&channel(), Some(&s), id(1), id(1)),
        "the owner holds everything"
    );
}

#[test]
fn deleting_another_persons_message_needs_the_delete_others_bit() {
    let s = space(
        &[(7, Permission::DeleteOthersMessage.bit(), 0)],
        &[(2u128, vec![7u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(
        can_delete_message(&channel(), Some(&s), id(2), id(4)),
        "the bit buys someone else's message"
    );
    assert!(
        !can_delete_message(&channel(), Some(&s), id(4), id(2)),
        "and not the plain member's"
    );
}

#[test]
fn own_message_edits_never_reach_another_author() {
    const OWN: u64 = Permission::EditOwnMessage.bit();
    let s = space(&[(7u128, OWN, 0i32)], &[(2u128, vec![7u128]), (4u128, vec![])], EVERYONE);
    assert!(can_edit_message(&channel(), Some(&s), id(2), id(2)), "the boss edits their own");
    assert!(
        !can_edit_message(&channel(), Some(&s), id(2), id(4)),
        "own-edit is the author's act only"
    );
    assert!(
        !can_edit_message(&channel(), Some(&s), id(4), id(4)),
        "a plain member cannot edit their own"
    );
}

#[test]
fn outside_a_space_the_author_may_edit_and_delete_their_own() {
    assert!(
        can_edit_message(&channel(), None, id(9), id(9)),
        "a conversation holds no roles to consult"
    );
    assert!(
        can_delete_message(&channel(), None, id(9), id(9)),
        "the author deletes their own there too"
    );
    assert!(
        !can_delete_message(&channel(), None, id(9), id(8)),
        "and never another author's"
    );
}

// -------------------------------------------------------------- baseline

#[test]
fn the_everyone_baseline_reaches_members_with_no_roles() {
    let s = space(&[], &[(4u128, vec![])], DEFAULT_EVERYONE);
    assert!(can(&s, id(4), Permission::SendMessage));
    assert!(can(&s, id(4), Permission::AddReaction));
    assert!(can(&s, id(4), Permission::VoiceShareScreen));
    assert!(
        can_edit_message(&channel(), Some(&s), id(4), id(4)),
        "the baseline carries own-edit"
    );
    assert!(
        !can(&s, id(4), Permission::ManageRoles),
        "the baseline stops at member powers"
    );
    assert!(
        !can_invite(&s, id(4)),
        "the baseline hands out no invites"
    );
}

#[test]
fn a_space_without_the_baseline_keeps_the_empty_set() {
    let s = space(&[], &[(4u128, vec![])], EVERYONE);
    assert!(
        !can(&s, id(4), Permission::SendMessage),
        "an owner who cleared it sends nothing"
    );
    assert!(
        can(&s, id(1), Permission::ViewChannel),
        "the owner still holds everything"
    );
    assert_eq!(effective(&s, id(4)), Some(EVERYONE), "no roles means the empty set");
}

// -------------------------------------------------------------- channel overwrites

#[test]
fn a_channel_deny_set_cuts_a_role_s_power_there_only() {
    const POWERS: u64 = Permission::ManageRoles.bit() | Permission::ManageMembers.bit();
    let s = space(
        &[(7u128, POWERS, 0i32)],
        &[(2u128, vec![7u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(can_moderate(&s, id(2), id(4)), "the mod moderates in the space");
    let here = over(channel(), id(7), 0, Permission::ManageMembers.bit());
    assert!(
        !can_at(&here, Some(&s), id(2), Permission::ManageMembers),
        "the deny cuts it at this channel"
    );
    assert!(
        can_at(&channel(), Some(&s), id(2), Permission::ManageMembers),
        "and only at this channel"
    );
}

#[test]
fn an_allow_set_widens_a_role_at_that_channel() {
    let s = space(
        &[(7, Permission::ManageRoles.bit(), 0)],
        &[(2u128, vec![7u128])],
        EVERYONE,
    );
    assert!(
        !can(&s, id(2), Permission::VoiceConnect),
        "the role has no voice bit"
    );
    let widened = over(channel(), id(7), Permission::VoiceConnect.bit(), 0);
    assert!(
        can_connect(&widened, Some(&s), id(2)),
        "the channel's allow grants it here"
    );
}

#[test]
fn the_owner_and_an_admin_ignore_the_overwrites() {
    let s = space(&[(7u128, ADMIN, 0i32)], &[(2u128, vec![7u128])], EVERYONE);
    let here = over(channel(), id(7), 0, Permission::ManageMembers.bit());
    assert!(
        covers(
            at_channel(id(2), &here, &s).unwrap_or(0),
            Permission::ManageMembers.bit()
        ),
        "an admin's reach survives the deny"
    );
    assert!(
        can_at(&here, Some(&s), id(1), Permission::ManageMembers),
        "and the owner's"
    );
}
// -------------------------------------------------------------- role colour

#[test]
fn a_colour_is_valid_up_to_six_hex_digits_and_refused_above() {
    use chat_model::perms::valid_colour;
    assert!(valid_colour(None), "no colour stands");
    assert!(valid_colour(Some(0xFFFFFF)), "the top of 0xRRGGBB is accepted");
    assert!(!valid_colour(Some(0x1_000000)), "one above is refused");
}

#[test]
fn the_colour_needs_no_permission_beyond_the_role_verb_gates() {
    // The colour is not a power: authoring and editing gate on the same role
    // rules whatever the colour. A plain member still cannot author even a
    // coloured role, and the boss still cannot author a power they lack.
    const MOD: u64 = Permission::ManageRoles.bit();
    let s = space(
        &[(7u128, MOD, 0i32)],
        &[(2u128, vec![7u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(can_author_role(&s, id(2), MOD), "the boss authors a plain-bit role");
    assert!(
        !can_author_role(&s, id(4), MOD),
        "a plain member authors nothing, coloured or not"
    );
    assert!(
        !can_author_role(&s, id(2), MOD | Permission::ManageMembers.bit()),
        "a colour smuggles no power: the gate is still the held powers"
    );
}

// -------------------------------------------------------------- the strict ladder

#[test]
fn peers_never_reach_each_other() {
    const POWERS: u64 = Permission::ManageMembers.bit()
        | Permission::BanMembers.bit()
        | Permission::TimeoutMembers.bit();
    let s = space(
        &[(7u128, POWERS, 0i32)],
        &[(2u128, vec![7u128]), (3u128, vec![7u128])],
        EVERYONE,
    );
    assert!(!can_moderate(&s, id(2), id(3)), "two members at one rank cannot remove each other");
    assert!(!can_ban(&s, id(2), id(3)), "nor ban");
    assert!(!can_timeout(&s, id(2), id(3)), "nor time out");
}

#[test]
fn a_role_senior_to_the_granter_cannot_be_handed_out_even_with_fewer_powers() {
    const MOD: u64 = Permission::ManageRoles.bit() | Permission::ManageMembers.bit();
    // Role 8 carries a subset of the boss's powers but sits above the boss's own top role.
    let s = space(
        &[(7u128, MOD, 1i32), (8u128, Permission::ManageMembers.bit(), 0i32)],
        &[(2u128, vec![7u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(!can_grant(&s, id(2), id(4), id(8)), "seniority is a power too: it is not theirs to give");
    assert!(!can_set_roles(&s, id(2), id(4), &[id(8)]), "nor in a batch");
    assert!(can_set_roles(&s, id(1), id(4), &[id(8)]), "the owner may assign any role");
}

#[test]
fn nobody_hands_themselves_a_role() {
    const MOD: u64 = Permission::ManageRoles.bit() | Permission::ManageMembers.bit();
    let s = space(
        &[(7u128, MOD, 1i32), (9u128, Permission::ManageMembers.bit(), 2i32)],
        &[(2u128, vec![7u128])],
        EVERYONE,
    );
    assert!(!can_set_roles(&s, id(2), id(2), &[id(9)]), "a member is never below themselves");
    assert!(!can_moderate(&s, id(2), id(2)), "and cannot remove themselves through the moderation rule");
}

#[test]
fn clearing_a_members_roles_needs_manage_roles() {
    let s = space(
        &[
            (7u128, Permission::ManageMembers.bit(), 0i32),
            (8u128, Permission::ManageRoles.bit() | Permission::ManageMembers.bit(), 0i32),
            (9u128, Permission::ManageMembers.bit(), 1i32),
        ],
        &[(2u128, vec![7u128]), (3u128, vec![8u128]), (4u128, vec![9u128])],
        EVERYONE,
    );
    assert!(
        !can_set_roles(&s, id(2), id(4), &[]),
        "ManageMembers alone cannot strip someone's roles"
    );
    assert!(can_set_roles(&s, id(3), id(4), &[]), "with ManageRoles it can");
}

#[test]
fn ban_and_timeout_stand_on_their_own_bits() {
    let s = space(
        &[
            (7u128, Permission::BanMembers.bit(), 0i32),
            (8u128, Permission::ManageMembers.bit(), 0i32),
            (9u128, Permission::TimeoutMembers.bit(), 0i32),
        ],
        &[(2u128, vec![7u128]), (3u128, vec![8u128]), (5u128, vec![9u128]), (4u128, vec![])],
        EVERYONE,
    );
    assert!(can_ban(&s, id(2), id(4)), "BanMembers alone bans");
    assert!(!can_ban(&s, id(3), id(4)), "ManageMembers alone does not ban");
    assert!(can_timeout(&s, id(5), id(4)), "TimeoutMembers alone times out");
    assert!(!can_timeout(&s, id(3), id(4)), "ManageMembers alone does not time out");
    assert!(can_moderate(&s, id(3), id(4)), "ManageMembers still removes");
    assert!(!can_moderate(&s, id(2), id(4)), "and BanMembers alone does not remove");
}

// -------------------------------------------------------------- view and send are real gates

#[test]
fn the_view_and_send_bits_gate_a_channel_whatever_its_masks() {
    let none = space(&[], &[(2u128, vec![])], EVERYONE);
    assert_eq!(
        channel_access(id(2), &channel(), Some(&none)),
        Access::Locked,
        "an open channel still asks the member for ViewChannel"
    );
    let view_only = space(&[], &[(2u128, vec![])], Permission::ViewChannel.bit());
    assert_eq!(
        channel_access(id(2), &channel(), Some(&view_only)),
        Access::ReadOnly,
        "seeing without SendMessage is read-only"
    );
    let baseline = space(&[], &[(2u128, vec![])], DEFAULT_EVERYONE);
    assert_eq!(
        channel_access(id(2), &channel(), Some(&baseline)),
        Access::Full,
        "the default baseline keeps ordinary members at full access"
    );
    assert_eq!(
        channel_access(id(1), &channel(), Some(&none)),
        Access::Full,
        "the owner holds every bit whatever the baseline"
    );
}

#[test]
fn a_channel_overwrite_can_mute_or_hide_a_channel_for_a_role() {
    let s = space(
        &[(7u128, 0, 0i32)],
        &[(2u128, vec![7u128]), (3u128, vec![])],
        DEFAULT_EVERYONE,
    );
    let muted = over(channel(), id(7), 0, Permission::SendMessage.bit());
    assert_eq!(channel_access(id(2), &muted, Some(&s)), Access::ReadOnly, "a deny on SendMessage mutes the role there");
    assert_eq!(channel_access(id(3), &muted, Some(&s)), Access::Full, "a member without the role still posts");
    let hidden = over(channel(), id(7), 0, Permission::ViewChannel.bit());
    assert_eq!(channel_access(id(2), &hidden, Some(&s)), Access::Locked, "a deny on ViewChannel hides it from the role");
    assert_eq!(channel_access(id(3), &hidden, Some(&s)), Access::Full, "and only from the role");
}

#[test]
fn an_allow_can_open_posting_to_one_role_in_a_read_only_space() {
    let s = space(
        &[(7u128, 0, 0i32)],
        &[(2u128, vec![7u128]), (3u128, vec![])],
        Permission::ViewChannel.bit(),
    );
    let announce = over(channel(), id(7), Permission::SendMessage.bit(), 0);
    assert_eq!(channel_access(id(2), &announce, Some(&s)), Access::Full, "the role posts there");
    assert_eq!(channel_access(id(3), &announce, Some(&s)), Access::ReadOnly, "the rest only read");
}

// -------------------------------------------------------------- the seeded roles

#[test]
fn the_seeded_moderator_bans_below_and_creates_and_edits_nothing() {
    // The two roles a new space seeds: Admin at the top (the tag alone),
    // Moderator below it. A member the owner hands Moderator to acts at
    // position 1.
    let s = space(
        &[(7u128, ADMIN, 0i32), (8u128, DEFAULT_MODERATOR, 1i32)],
        &[(2u128, vec![8u128]), (3u128, vec![7u128]), (4u128, vec![])],
        DEFAULT_EVERYONE,
    );
    assert!(
        can_set_roles(&s, id(1), id(2), &[id(8)]),
        "the owner hands the seeded Moderator out"
    );
    assert!(
        can_ban(&s, id(2), id(4)),
        "the Moderator bans a plain member below their rank"
    );
    assert!(
        !can_ban(&s, id(2), id(3)),
        "but not the Admin-holding member senior to them"
    );
    assert!(
        !can_author_role(&s, id(2), Permission::AddReaction.bit()),
        "Moderator carries no ManageRoles, so it creates no role"
    );
    assert!(
        !can_edit_role(&s, id(2), id(8)),
        "and edits no role, not even the one it holds"
    );
    assert!(
        !can_edit_role(&s, id(2), id(7)),
        "the seeded Admin sits above them, so it is not theirs to rewrite"
    );
}
