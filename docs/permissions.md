# Permissions

The permission model as it stands in the final code: commit `eb5e988`
("Port the chat client to named roles and permission bitsets"), merged into
this branch at `df3d8c2`. Every fact here is re-derived from that code; file
and function names are given with each claim. Where the code and a doc
comment disagree, or a write path is missing, that is called out rather than
smoothed over.

## Overview: what this is

The three fixed ranks (Owner / Admin / Member) are replaced with named roles
carrying permission bitsets. `Role` is a record
(`chat-model/src/domain.rs::Role` — `id`, `name`, `permissions`, `position`),
ownership is a fact on the space (`Space.owner`,
`chat-db/migrations/0001_init.sql::spaces.owner_id`), and channels gate with
permission masks instead of rank floors. What someone may do is the union of
their roles' permission sets, above a space-wide "everyone" baseline.

The single source of the rules is `crates/chat-model/src/perms.rs`, whose
header says the copy is shared: "run by the server to decide and by the
client to decide what to offer — so a button the server would refuse is
never drawn." At `eb5e988` both halves exist: `chat-api` calls the
`perms::…` guards at its server verbs, and the `chat` client crate was
ported to the model in `eb5e988` itself — `crates/chat/src/settings.rs`,
`nav.rs`, `message.rs` and `store.rs` now call `perms::effective`,
`can_grant`, `can_moderate`, `can_delete_message` and
`Permission::ManageSpaceSettings` instead of the old `my_role` /
`can_manage_space` / `set_role` calls that do not exist (the old helper
names survive nowhere in the `chat` crate; see the Client section).

`docs/e2ee-protocol.md` describes the sigil group protocol as a
"proposal; … Nothing uses it yet", so today's `chat-api` reads message
bodies in cleartext — several content-permission checks below depend on
that; see E2EE limits.

## The owner

Ownership is a fact on the space, not a role (`chat-model/src/domain.rs::Space.owner`).

- The owner holds every permission whatever roles they carry:
  `perms.rs::effective` short-circuits with `if user == space.owner { return Some(ALL) }`.
- No role can demote or remove the owner: `perms.rs::can_moderate`,
  `perms.rs::can_ban` (via `can_moderate`), `perms.rs::can_grant` (via
  `can_moderate`) and `perms.rs::can_timeout` refuse targets equal to
  `space.owner`, and `chat-db/src/spaces.rs::remove_member` (SQL at ~line
  212), `chat-db/src/spaces.rs::set_role_ids` (~236) and
  `chat-db/src/spaces.rs::ban` (~315) all carry the `user_id <> (SELECT
  owner_id …)` predicate.
- The owner ignores channel overwrites entirely: `perms.rs::at_channel`
  returns the base set unchanged for `user == space.owner` or a set
  covering the admin tag (~line 239).
- Ownership moves by transfer only: `chat-api/src/spaces.rs::transfer_space`
  (~line 268) checks `me != s.owner` directly (mirroring
  `perms.rs::can_transfer`, which the verb itself does not call); the db
  body `chat-db/src/spaces.rs::transfer_space` (~389) moves `owner_id`.
  `chat-api/src/spaces.rs::delete_space` (~line 83) is owner-only by the
  same direct check.

## Roles

The record (`chat-model/src/domain.rs::Role`): `id`, `name`, `permissions`
(a `u64` bitset — `domain.rs`'s `PermissionSet = u64`), `position` (orders
the role list; `chat-api/src/spaces.rs::space_roles` ~line 202 sorts by
`(position, id)`). A space keeps its roles in `Space.roles:
BTreeMap<Id, Role>`; a membership (`domain.rs::Member`) holds `roles:
Vec<Id>` — the ids of the roles the person has.

A member may hold several roles; what they may do is the **union** of the
roles' permission sets **on top of the space's everyone baseline**:
`perms.rs::effective` (~line 196) folds `member.roles` over
`space.everyone` with `perms.rs::union`, which is `expand(a | b)` — and
`expand` turns a set carrying the `IsAdmin` bit (bit 63) into every
permission. A member pointing at a role the space no longer has gets
nothing from it: `role_permissions` returns `0` for an unknown role id.

A new role is created at the bottom of the ladder (`max(position) + 1`), so rank
never blocks making one. A new space seeds two roles itself, in its own creation
transaction: `Admin` (the `IsAdmin` tag alone, which expands to every permission)
at position 0, and `Moderator` (`perms::DEFAULT_MODERATOR`: delete others'
messages, manage, ban and time out members, mention everyone, manage pins) at
position 1. A role made after them still lands below both. The role the migration
backfills for old admins sits at position 0.

New roles get their position from the db:
`chat-db/src/spaces.rs::create_role` (~line 247) assigns
`max(position) + 1`, so a fresh role sits at the bottom of the ladder.

### The everyone baseline

Every space carries an `everyone: PermissionSet`
(`domain.rs::Space.everyone` ~line 129, column `spaces.everyone` —
`chat-db/migrations/0003_everyone.sql`, `DEFAULT 16207`), and
`effective` folds the member's roles over it, so the baseline holds
whatever their roles say. The default is
`perms.rs::DEFAULT_EVERYONE` (~line 149): see, post, edit and delete own,
react, attach, embed, and voice connect/speak/video/share — bits 0, 1, 2,
3, 6, 8, 9, 10, 11, 12, 13 (the 16207 in the migration is that value;
the migration comment spells the bits out). It stops where the acts on
other people and on the space begin: `MentionEveryone`, all the channel
management, and `ManageMembers` are **not** in the baseline (tests
`the_baseline_reaches_a_member_with_no_roles` and
`a_space_can_raise_its_baseline` in `perms.rs`, and
`the_everyone_baseline_reaches_members_with_no_roles` /
`a_space_without_the_baseline_keeps_the_empty_set` in
`crates/chat-api/tests/roles_verbs.rs`). New spaces seed the default:
`chat-db/src/spaces.rs::create_space` (~line 82) binds
`chat_model::perms::DEFAULT_EVERYONE`. There is **no verb that changes a
space's baseline** after creation — the 0003 comment ("an owner can retune
it per space") is intent, not an endpoint.

### Hierarchy by position

A lower `position` is more senior. `perms.rs::top_position` (~line 250)
is the position of the member's most senior role, `None` for a member
holding no roles, who sits at the bottom. The ladder gates who an actor
may touch:

- `perms.rs::reaches` (~273): the owner reaches everyone; otherwise the
  target's top position must be ≥ the actor's (a member with no roles
  reaches only members with no roles).
- `perms.rs::reaches_role` (~312): a member may touch a role at or below
  their own top role; the owner is exempt.
- `can_moderate`, `can_ban`, `can_timeout`, `can_grant`, `can_set_roles`,
  `can_edit_role` all fold the bit check and the reach together, so a
  moderation and role act only goes "at their own rank or below".
- The backfilled Admin role gets `position 0` (0002 below), keeping ex-admins
  senior to any custom role made later.

## Permissions

All 25 functional permissions plus the `IsAdmin` tag, counted from
`chat-model/src/perms.rs::Permission` and its `bit()` numbering (bits 0–24,
plus 63; `perms.rs::ALL` = `(1 << 63) | ((1 << 25) - 1)` covers exactly the
defined bits, asserted by `the_new_bits_stay_inside_all` in `perms.rs` and
again in `roles_verbs.rs`). "Checked where" lists call sites **outside
`perms.rs` itself**; a hit that is only data (a test creating a role with a
bit) is not a check. Channel-borne acts consult the **overwritten** set via
`can_at`/`at_channel` (~lines 367/234), so per-channel overwrites widen or
cut them; space-wide acts consult the plain effective set via `can`/`can_act`.

| Permission | Bit | Group | What it allows | Checked where |
| --- | --- | --- | --- | --- |
| ViewChannel | 0 | messages | Read a space channel at all. | Never checked alone; it is the required bit of a channel's `view` mask, enforced through `perms::channel_access`: `chat-api/src/channels.rs::body::open` (~line 35, every channel-bearing verb reads the resulting `Access`), `chat-api/src/srv.rs::topics_for` / `topics_of` (~155/176, gated channels — `srv.rs::gated`, `view != 0` at ~150 — subscribe only viewers whose `effective` set covers `view`), `chat-api/src/messages.rs::search` (~318, `Scope::Space` filters channels by `channel_access >= ReadOnly`), and `Access::can_post()` gates below. |
| SendMessage | 1 | messages | Post in a channel. | Same route as ViewChannel: the `view | post` mask in `channel_access`, then `Access.can_post()` at `chat-api/src/messages.rs::send` (~97), `edit` (~189), `chat-api/src/channels.rs::create_thread` (~177), `rename_channel`'s thread arm (~251) and `chat-api/src/calls.rs::join_call` (~113). A channel whose `post` mask is 0 asks nothing of ordinary members. |
| EditOwnMessage | 2 | messages | Change one of their own messages. | `perms.rs::can_edit_message` (~448) → `chat-api/src/messages.rs::edit` (~186). The bit is also in the `DEFAULT_EVERYONE` baseline. Test: `own_message_edits_never_reach_another_author` (`roles_verbs.rs`). |
| DeleteOwnMessage | 3 | messages | Delete one of their own messages. | **Enforced now.** `perms.rs::can_delete_message` (~440) checks `can_at(…, DeleteOwnMessage)` when the user is the author — the old snapshot's disagreement between code and the bit's doc comment is gone: the code does what the comment says. Server site `chat-api/src/messages.rs::delete` (~226); client site `crates/chat/src/message.rs` (~646, RowTools) hides the delete button by the same call; test `editing_and_deleting_their_own_message_now_needs_their_bits` (`roles_verbs.rs`). |
| DeleteOthersMessage | 4 | messages | Delete someone else's message. | `perms.rs::can_delete_message` second clause (`can(s, user, DeleteOthersMessage)`) → `chat-api/src/messages.rs::delete` (~226); test `deleting_another_persons_message_needs_the_delete_others_bit`. |
| ManagePins | 5 | messages | Pin and unpin messages. | **not enforced yet** — no pin/unpin endpoint exists in `chat-api` at all (the word "pin" appears there only as `std::pin` and `Up::Ping`); the bit appears as data in `chat-db/tests/db.rs` (~172, a role carrying it). |
| AddReaction | 6 | messages | React to messages. | `perms.rs::can_react` (~376) — actually enforced directly at `chat-api/src/messages.rs::react` (~254: `can_at(…, AddReaction)` after read/not-archived). Test `reacting_needs_the_react_bit_at_the_channel`. |
| MentionEveryone | 7 | messages | Address a message to the whole space (`@everyone`). | `chat-api/src/messages.rs::send` (~142): the `everyone` flag is dropped unless `perms::can(…, MentionEveryone)` in a space (conversations allow it outright). `edit` (~210) keeps the original message's everyone flag rather than granting a new one — no permission call. Not in the baseline. |
| AttachFile | 8 | messages | Upload an attachment with a message. | `chat-api/src/messages.rs::send` (~117): non-empty attachments need `can_at(…, AttachFile)`; test `attaching_a_file_needs_the_attach_bit`. `perms.rs::can_attach` (~382) is the shared copy; the upload itself is not the act — the check runs where the message lands. |
| EmbedLink | 9 | messages | Post a link the reader expands. | `chat-api/src/messages.rs::send` (~126) and `edit` (~198): when `chat_model::text::embeds(&text)` (text.rs ~299), the act needs `can_at(…, EmbedLink)`. The server sees the body in cleartext (nothing E2EE is wired yet), so this is enforced where the act is — see E2EE limits for what changes if sealed bodies arrive. |
| VoiceConnect | 10 | voice | Join a voice channel's call. | `chat-api/src/calls.rs::join_call` (~116): `can_at(…, VoiceConnect)` after `can_post`. Test `joining_and_the_call_controls_gate_on_their_bits`. |
| VoiceSpeak | 11 | voice | Unmute their microphone. | `chat-api/src/calls.rs::update_call` (~218): unmuting (`!muted && !deafened`) needs `can_at(…, VoiceSpeak)`. |
| VoiceVideo | 12 | voice | Use a camera. | `chat-api/src/calls.rs::update_call` (~229). |
| VoiceShareScreen | 13 | voice | Share a screen. | `chat-api/src/calls.rs::update_call` (~232). |
| VoiceForceMute | 14 | voice | Mute or deafen someone else for the call. | **not enforced yet — cannot be.** `chat-api/src/calls.rs` (~191–199) carries an explicit note: no verb in this stack acts on someone else's call — `update_call` is self-service, the relay only removes a person when their own connection drops — so there is no server site to check the bit against, and a gate over a control that does not exist would be a fake one. The bit stays defined for when such controls arrive; test `force_mute_and_move_have_no_site_to_check_yet`. |
| VoiceMoveMember | 15 | voice | Move someone between calls. | Same as above: no server site; enforced the day such controls arrive, gating through `can_at`. |
| ManageChannels | 16 | channels | Create, delete and arrange channels. | `chat-api/src/channels.rs::create_channel` (~59). Note: `delete_channel` (~135) and `order_channels` (~153) gate on ManageSpaceSettings instead (plus, for `delete_channel`, the thread's own creator), so this bit buys only channel creation today. |
| ManageChannelRoles | 17 | channels | Set who may see and post in a channel. | `chat-api/src/channels.rs::update_channel` (~111): when the `view`/`post` masks actually move, the act needs `can_act(…, ManageChannelRoles)` — a pure name or topic change does not. The `perms.rs::manages_channel` helper (~349) exists beside it but has no call site outside the module; the test is `setting_view_and_post_masks_needs_manage_channel_roles` (`roles_verbs.rs`). |
| ManageInvites | 18 | channels | Create and revoke invites. | `chat-api/src/spaces.rs::create_invite` (~288), `invites` (~301), `revoke_invite` (~312) — all three gate directly on `can(…, ManageInvites)` (the old snapshot's "any member can invite" is corrected). `perms.rs::can_invite` (~425) is the shared copy; test `making_seeing_and_revoking_invites_needs_the_invite_bit`. Invites belong to the space, so no overwrites apply. |
| ManageRoles | 19 | space | Create, rename and delete roles. | Through `perms.rs::can_author_role` (~305) at `chat-api/src/spaces.rs::create_role` (~216) and `update_role` (~240); through `perms.rs::can_edit_role` (~319, which is `ManageRoles` + `reaches_role`) at `update_role` (~237) and `delete_role` (~258); through `can_set_roles`/`can_grant` at `set_roles` (~190); client side `crates/chat/src/settings.rs` (~488) filters the role picker with `can_grant`. |
| ManageMembers | 20 | space | Remove members from the space. | `perms.rs::can_moderate` (~263: the bit + `reaches`, never the owner) → `chat-api/src/spaces.rs::remove_member` (~121); also underpins `ban_member` (via `can_ban`), `set_roles`, `create_role` and `update_role` targets. Client: `crates/chat/src/settings.rs` ~491 hides the Remove action without `can_moderate`. Test `removing_members_needs_the_moderation_bit_at_rank`. |
| ManageSpaceSettings | 21 | space | Change the space's own settings. | Directly: `chat-api/src/spaces.rs::body::manager` (~39) → `rename_space` (~66), `create_category` (~383), `rename_category` (~396), `delete_category` (~410); `chat-api/src/files.rs::set_space_icon` (~132); `chat-api/src/channels.rs::order_channels` (~153). Through `perms.rs::manages` (~245): `update_channel` (~104), `delete_channel` (~135), `set_archived` (~227), `rename_channel` thread arm (~251). Client: `crates/chat/src/store.rs::manages_space` (~234) over `live.rs::my_permissions` (~383), used by the settings modal (`settings.rs` ~152) and space nav (`nav.rs` ~570). |
| BanMembers | 22 | space | Ban members so they cannot rejoin until unbanned; lift a ban. | `perms.rs::can_ban` (~327, `can_moderate` + the bit) → `chat-api/src/spaces.rs::ban_member` (~139). Unban needs only the bit, not reach — the banned person is out of the space so rank cannot be compared (`spaces.rs::unban_member` ~158 checks `can(…, BanMembers)` directly; the comment says why). The ban's effect is enforced in the db: `chat-db/src/spaces.rs::redeem_invite` (~564) refuses a banned joiner ("That invite no longer works for you."). Test `ban_timeout_and_unban_gate_on_their_new_bits_at_rank` + `the_owner_is_never_a_ban_or_timeout_target`. |
| TimeoutMembers | 23 | space | Time members out so they cannot send or speak until it expires. | `perms.rs::can_timeout` (~335, member + not owner + bit + reach) → `chat-api/src/spaces.rs::timeout_member` (~173). The effect is enforced where the acts are: `chat-api/src/messages.rs::send` (~103, `chat-db/src/spaces.rs::timed_out` ~352) and `chat-api/src/calls.rs::update_call` (~222). `set_timeout` (~363) stores `timeout_ends`; expiry is by comparison with `now()`. |
| ManageExpressions | 24 | space | Create, rename and delete the space's custom emoji and stickers. | `perms.rs::can` with `Permission::ManageExpressions` at `chat-api/src/spaces.rs::create_emoji` (~350), `rename_emoji` (~377), `delete_emoji`, `create_sticker` (~427), `rename_sticker`, `delete_sticker` — all six gate directly. Listing (`list_emoji`, `list_stickers`) needs only membership. The seeded Admin gets it via `expand(IsAdmin)`; the seeded Moderator and the `@everyone` baseline do not. Tests `the_new_bits_stay_inside_all` (perms.rs) and the verb matrix in `roles_verbs.rs`. |
| IsAdmin | 63 | space | Every permission. | Enforced as expansion: `perms.rs::expand` inside `effective` and `union`, so the tag grants all 24 functional bits; `at_channel` (~239) exempts any set covering `ADMIN` from channel overwrites ("a channel cannot demote them"). Read as a presentation fact in `chat-model/src/live.rs::roster` (~561/566), which sorts members whose set covers the tag first. |

One caveat on "where": topic **delivery** consults the space-wide
`effective` set (`srv.rs::topics_for`/`topics_of`), while channel **access**
consults the overwritten set (`channel_access` → `at_channel`). A per-role
`allow` on `ViewChannel` at a gated channel would let an API fetch read it
but would not subscribe the member to that channel's live messages. Nothing
sets overwrites through the API today (see Not done yet), so this gap is
latent.

## Channel access

A `Channel` carries two permission masks (`chat-model/src/domain.rs::Channel.view`
and `.post`, `PermissionSet = u64`; persisted as `view_mask`/`post_mask`
bigints — `chat-db/src/channels.rs` maps them back at ~65/66). `perms.rs::channel_access`
(~213) computes the `Access`:

- private kinds (`ChannelKind::private` — Direct/Group): `Full` if the
  user is in `channel.members`, else `None`; masks don't apply.
- otherwise the user's set **at the channel** — `perms.rs::at_channel`
  (~234): their space `effective` set, widened by the `allow` masks and then
  cut by the `deny` masks of every role they hold on that channel
  (`(base | allow) & !deny`), owner/admin-tag holders ignoring the
  overwrites. If the set does not cover `view` → `Locked`; if it does not
  cover `view | post`, or the channel is archived → `ReadOnly`; else `Full`.
  Not being in the space at all → `None`.
- `Access::can_read()` is `>= ReadOnly`; `can_post()` is `== Full`.

Meanings (`perms.rs::Access` doc comments): **Full** — read and post;
**ReadOnly** — read but not post; **Locked** — the member knows the channel
exists but cannot read it; **None** — not theirs to know about. A thread is
judged by its **parent's** masks with the thread's own `archived` flag
(`body::open` ~28–35 and `live.rs::access` ~379 rebuild
`Channel { archived: c.archived, ..parent }`).

Per-channel overwrites combine with the base as widen-then-cut:
allows union across the member's roles and OR into the base; denies union
and cut afterwards, so a deny wins over an allow on the same bit
(`a_channel_overwrite_cuts_a_role_s_power_there_only`,
`an_allow_set_widens_a_role_at_that_channel`, `the_owner_shrugs_the_overwrites_off` —
`perms.rs` tests and `roles_verbs.rs` twins). They belong only to acts taken
at a channel (`can_at`); space-wide acts (`can_act`: `manage_channel` masks,
invites) and delivery gating use the plain effective set.

Delivery matches the gate: `srv.rs::gated` is true when `view != 0`, a
gated channel's messages ride their own topic, subscribed to only by
viewers whose effective set covers `view` (`topics_for`/`topics_of`), while
ungated channels ride the space topic. `update_channel`'s comment
(channels.rs ~122) states the contract: settings go to everyone who knows
the channel; messages only to who may read them.

## Role and moderation verbs

All in `chat-api/src/spaces.rs`, guarded by `perms.rs` rules.

- **remove_member** (~115): `can_moderate`.
- **ban_member** (~133) / **unban_member** (~151): `can_ban` /
  `can(…, BanMembers)` (see above).
- **timeout_member** (~168): `can_timeout` (minutes `None` clears).
- **set_roles** (~184): `can_set_roles` (~297) — the actor must moderate
  the target and every role handed out passes `can_grant`, so a batch
  cannot smuggle in power the actor lacks; clearing a member's roles is an
  act too (`can_set_roles` with an empty list — test
  `clearing_a_member_s_roles_is_an_act_the_role_manager_makes`).
- **space_roles** (~202): lists the space's roles sorted by `(position, id)`.
- **create_role** (~211): `can_author_role` — needs `ManageRoles`, and the
  new role's set, after `expand`, must be covered by the actor's own
  effective set ("You can't create a role with powers you do not have.").
  A new role sits at the bottom of the ladder (db `max(position) + 1`), so
  hierarchy never blocks creating one (`can_author_role` comment ~304).
- **update_role** (~227): role must exist in the space;
  `can_edit_role` (hierarchy: "You can't edit a role above your own.") and
  again `can_author_role` on the new set ("You can't grant powers you do not have.").
- **delete_role** (~251): `can_edit_role` — the same reach as editing
  (test `deleting_a_role_uses_the_same_reach_as_editing_it`). Holders drop
  the role: `chat-db/src/spaces.rs::delete_role` (~283) strips the id from
  every `space_members.role_ids` in one transaction; channel masks are
  untouched, "a mask a deleted role used simply becomes one nobody covers"
  (its doc comment). `channel_overwrites` rows follow their role by FK
  (0005 below), so deleting a role deletes its per-channel overwrites.
- Escalation rule everywhere: a role cannot carry power its author lacks
  (`covers(effective(actor), expand(permissions))`), and the `IsAdmin`
  tag therefore buys the tag only for an admin (test
  `a_role_manager_cannot_author_a_role_beyond_their_powers`).

Every one of these gates is covered in `crates/chat-api/tests/roles_verbs.rs`:
25 tests asserting the `perms` call each `#[server]` verb makes — its
header states "Each `#[server]` verb makes exactly one of these
`chat_model::perms` calls before it touches the db", so the allow/refusal
pair covers the verb without going through the session wire.

## Database

`crates/chat-db/migrations/0002_roles.sql` (the old doc claimed this
dropped everything; it does **not** — the backfills run before the old
columns go):

- `space_roles` table: PK `(space_id, role_id)`, FK to `spaces` `ON DELETE
  CASCADE`, `permissions bigint DEFAULT 0`, `position integer DEFAULT 0`.
- **Backfill 1 (admins):** for each space that has any member with the old
  `role = 'admin'`, one `Admin` role is inserted whose id **is the space's
  own id** (one per space, cannot clash), permissions
  `-9223372036854775808` — that is `INT64_MIN` = bit 63 = the `IsAdmin`
  tag, which `expand` turns into every permission, so an ex-admin keeps the
  power they had — and `position 0`, senior to anything made later. Then
  `role_ids uuid[] DEFAULT '{}'` is added and set to that role's id for
  each old admin, and the `role` column is dropped. Members with the old
  `role = 'owner'` get no role: their power comes from `spaces.owner_id`,
  which the migration never touches.
- **Backfill 2 (channel floors):** before `view_role`/`post_role` are
  dropped, each floor becomes a mask: `'member'` → 0 (asks nothing);
  anything else (`'admin'`, `'owner'`) → `INT64_MIN`, the admin-tag mask —
  a channel locked to admins keeps locking out plain members
  (`covers` demands bit 63, which plain sets lack), and the old
  owner-only floor ("nobody but the owner" was all it could mean) reads as
  the same admin lock, since the space's owner holds everything.

`0003_everyone.sql`: `ALTER TABLE spaces ADD COLUMN everyone bigint NOT NULL DEFAULT 16207`
— `DEFAULT_EVERYONE` spelled out (comment lists bits 0,1,2,3,6,8,9,10,11,12,13).

`0004_bans.sql`: `space_bans` table (FKs to `spaces` and `users` `ON DELETE
CASCADE`, PK `(space_id, user_id)`), and `timeout_ends timestamptz NULL` on
`space_members`. A ban keeps the seat closed — the member is deleted from
`space_members` (db `ban`) but the row in `space_bans` makes `redeem_invite`
refuse them until `unban`.

`0005_channel_overwrites.sql`: `channel_overwrites` table —
`channel_id` FK to `channels` CASCADE, PK `(channel_id, role_id)`, and a FK
`(space_id, role_id)` → `space_roles` CASCADE, so deleting either the
channel or the role leaves no orphan rows.

The old doc's claim that "former admins and owners come back with no
roles" and "locked channels silently open to everyone" is corrected: neither
happens at `eb5e988`.

A new space is created with the default baseline, the owner as its only member,
and **the two seeded roles** (`Admin` at 0, `Moderator` at 1, in
`chat-db::create_space`): nobody holds either, and the owner hands them out with
`set_roles`. They are ordinary roles afterwards, editable, deletable, renamable.
The `chat` client still has no screen for creating or editing roles, so out of
the box an owner in that client cannot make a *new* role, but the seeded pair
gives them something to hand out.

## Client

The `chat` crate is ported in `eb5e988` itself (the old snapshot said the
client was unfinished and did not compile against the model — false now;
the wasm build below passes):

- `crates/chat/src/store.rs::manages_space` (~234): the check the settings
  modal (`settings.rs` ~152) and space nav (`nav.rs` ~570) share —
  `perms::has(my_permissions(space), ManageSpaceSettings)` over
  `chat-model/src/live.rs::my_permissions` (~383, `perms::effective`).
- `crates/chat/src/settings.rs` member line (~488): shows the member's
  first role name read-only; the picker offers `Member` plus the space's
  roles filtered through `can_grant`, and calls `chat_api::spaces::set_roles`
  with a role-id list (~496); the Remove action renders only under
  `can_moderate` (~491).
- `crates/chat/src/message.rs` RowTools (~646): the delete action asks
  `perms::can_delete_message` at the channel.
- `nav.rs`/`channel.rs` import `perms::Access` for the Locked/read-only
  display; `live.rs::roster` sorts the admin-tag holders first.
- The channel settings page offers the space's role masks plus Everyone
  for WHO CAN SEE IT / WHO CAN POST (commit message for `eb5e988`),
  matching the server's `PermissionSet` columns.

Still only existing screens: there are **no role-creation, role-editing or
ban/timeout UI screens** in the client — the old member/admin picker has no
named-role equivalent yet; that is a follow-up (see Not done yet).

## Not done yet

- **ManagePins cannot be enforced:** no pin/unpin endpoint exists anywhere
  in `chat-api`, so there is no act to gate (the bit is only ever data,
  e.g. the role in `chat-db/tests/db.rs` ~172).
- **VoiceForceMute and VoiceMoveMember** have no server site: no verb in
  this stack forces a mute on someone else or moves a member between calls.
  The bits are defined, deliberately unchecked, and documented as such
  (`calls.rs` ~191–199, `perms.rs` ~414–420) so the audit does not read the
  absence as a missing check.
- **Channel overwrites are enforced but unwritable:** `at_channel`,
  `can_at`, `Channel.overwrites`, the db read path
  (`chat-db/src/channels.rs` aggregating `channel_overwrites`) and the 0005
  table are all in place, but **no API verb inserts into
  `channel_overwrites`** — nothing in `chat-api` mentions the type at all.
  Today the shapes only exist in tests (`roles_verbs.rs` `over()` helper,
  `perms.rs` `with_overwrites`). A "set the per-role allow/deny here" verb is
  the missing half.
- **No verb to retune a space's `everyone` baseline** after creation
  (0003's comment promises owner retuning; no endpoint).
- **No client screens for roles management:** creation, renaming, per-role
  permission editing, and the ban/timeout/mask-setting verbs
  (`ban_member`, `unban_member`, `timeout_member`, `create_role`,
  `update_role`, `delete_role`) have no UI; the server half is done.
- `perms.rs::manages_channel` has no call site outside the module
  (`update_channel` uses `can_act` directly) — a kept-but-unused helper.
- The delivery-vs-overwrite gap noted above (`topics_for`/`topics_of` gate
  on the plain effective set).

- Pins, force-mute and move-member have nothing to gate (above).
- There is no overwrite for the everyone baseline itself, only per role: "only Moderators post
  in #announcements" works in a space whose baseline lacks `SendMessage`, not in an ordinary one.
- No client screens for roles, the baseline, overwrites, ban or timeout; the
  seeded Admin and Moderator cover delegating without them.
- `perms::manages_channel` has no call site outside the module.

## E2EE limits

`docs/e2ee-protocol.md` is a proposal the stack does not use yet ("Nothing
uses it yet"), so today the server reads message bodies in cleartext —
`send` and `edit` resolve mentions and check `chat_model::text::embeds`
server-side. That is what lets `MentionEveryone` (~142) and `EmbedLink`
(~126/198) be enforced at the act. If sealed bodies ever land, the
permissions about **content** — `MentionEveryone`, `EmbedLink`
(and the server-side `search`) — would need either a cleartext side-channel
(the sigil policy-commit trick: policy sent in the clear so a server can
enforce `may_send` without reading) or become client-only, which
contradicts the rule that client checks never protect. That is an open
question for Nicho, not solved at this commit.
