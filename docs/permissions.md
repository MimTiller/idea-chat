# Permissions

The permission model of the `roles-permissions` branch, as it stands in this
snapshot (commit `3c6095b`). Every fact here is traceable to code at this
commit; file and function names are given with each claim. Where the code and
a doc comment disagree, or something is missing, that is called out rather
than smoothed over.

## Overview: what changed and why

The three fixed ranks (Owner / Admin / Member) are replaced with named roles
carrying permission bitsets. `git show 2cfc7bb` ("Replace the three fixed
ranks with named roles carrying permission bitsets") records the intent:
`Role` is now a record instead of an enum, ownership moves out of the role
lattice, and channels gate with permission masks instead of rank floors.

The single source of the rules is `crates/chat-model/src/perms.rs`, whose
header says the copy is shared: "run by the server to decide and by the client
to decide what to offer — so a button the server would refuse is never drawn."

At this snapshot the **server side** of the new model is in place
(`chat-api` calls `perms::…` helpers), but the **client UI crate `chat` still
speaks the old ranks and no longer compiles against this model**:
`crates/chat/src/settings.rs` (`MemberLine`, ~line 480) still offers a
member/admin `Select` calling `chat_api::spaces::set_role`, which does not
exist — the API verb is `set_roles` (`crates/chat-api/src/spaces.rs`). The
same file and `crates/chat/src/nav.rs` (`SpaceNav`, ~line 570),
`crates/chat/src/message.rs` (`RowTools`, ~line 646) and
`crates/chat/src/settings.rs` (~line 152) call
`chat_model::perms::can_manage_space` and a store method `my_role`, and
`settings.rs` constructs `Role::Owner`/`Role::Admin`/`Role::Member` — none of
which exist in this snapshot (`chat-model/src/domain.rs::Role` is a struct, not
an enum, and `chat/src/store.rs::Store` — the `chat` crate's own state type —
has no `my_role`; the only `my_role` anywhere in the repo is
`crates/hearth/src/model.rs`). `chat/src` is a
sibling crate to `chat-model`, so those references do not resolve; the client
is unfinished at this snapshot and the work to port it is listed as planned.

## The owner

Ownership is a fact on the space, not a role (`chat-model/src/domain.rs::Space.owner`,
`chat-db/migrations/0001_init.sql` `spaces.owner_id`).

- The owner holds every permission whatever roles they carry:
  `perms.rs::effective` short-circuits with `if user == space.owner { return Some(ALL) }`.
- No role can demote or remove the owner: `perms.rs::can_moderate` refuses
  targets equal to `space.owner`, and `chat-db/src/spaces.rs::remove_member`
  and `set_role_ids` exclude the owner in their SQL predicates
  (`user_id <> (SELECT owner_id FROM spaces …)`).
- Ownership moves by transfer only: `perms.rs::can_transfer` (owner → another
  member; no permission buys it) and `chat-api/src/spaces.rs::transfer_space`,
  whose db body `chat-db/src/spaces.rs::transfer_space` moves `owner_id`.

## Roles

The record (`chat-model/src/domain.rs::Role`): `id`, `name`, `permissions`
(a `u64` bitset — `domain.rs`'s `PermissionSet = u64`), `position` (orders the
role list; `chat-api/src/spaces.rs::space_roles` sorts by it). A space keeps its
roles in `Space.roles: BTreeMap<Id, Role>`; a membership
(`domain.rs::Member`) holds the ids of the roles the person has.

A member may hold several roles; what they may do is the **union** of the roles'
permission sets: `perms.rs::effective` folds `member.roles` with
`perms.rs::union`, which is `expand(a | b)` — and `expand` turns a set carrying
the `IsAdmin` bit (bit 63) into every permission. A member pointing at a role
the space no longer has gets nothing from it: `role_permissions` returns `0`
for an unknown role id.

## Permissions

All 23, from `chat-model/src/perms.rs::Permission` and its `bit()` numbering.
"Checked where" lists call sites **outside `perms.rs` itself**; a hit that is
only data (a db test creating a role with a bit, `chat-db/tests/db.rs`) is not
a check. "not enforced yet" means exactly that: nothing outside the module's
own definitions and tests asks the bit.

| Permission | Bit | Group | What it allows | Checked where |
| --- | --- | --- | --- | --- |
| ViewChannel | 0 | messages | Read a space channel at all. | Not checked on its own. It is the required bit of a channel's `view` mask, enforced through `perms::channel_access`: `chat-api/src/channels.rs::body::open` (every channel-bearing verb reads the resulting `Access`), `chat-api/src/srv.rs::topics_for` / `topics_of` (subscribe to a gated channel's topic only when the effective set covers `view`), `chat-api/src/messages.rs::search` (`Scope::Space` filters channels by `channel_access >= ReadOnly`). |
| SendMessage | 1 | messages | Post in a channel. | Same route as ViewChannel: `view | post` masks in `channel_access`, then `Access.can_post()` at `chat-api/src/channels.rs` (`create_thread` guard ~line 165), `chat-api/src/messages.rs` (`send` ~line 98, `edit` ~line 180), `chat-api/src/channels.rs::rename_channel` (~line 239). A channel whose `post` mask is 0 asks nothing of ordinary members. |
| EditOwnMessage | 2 | messages | Change one of their own messages. | `perms.rs::can_edit_message` → `chat-api/src/messages.rs::edit` (~line 172). |
| DeleteOwnMessage | 3 | messages | Delete one of their own messages. | **not enforced yet.** Nothing checks the bit; `perms.rs::can_delete_message` lets the author delete their own message regardless of it, while its own doc comment says the bit "governs whether they may". The code and the comment disagree; nothing at this snapshot enforces it. |
| DeleteOthersMessage | 4 | messages | Delete someone else's message. | `perms.rs::can_delete_message` → `chat-api/src/messages.rs::delete` (~line 207). |
| ManagePins | 5 | messages | Pin and unpin messages. | **not enforced yet** — no pin/unpin endpoint exists in `chat-api` at all; the bit appears only as data in `chat-db/tests/db.rs`. |
| AddReaction | 6 | messages | React to messages. | **not enforced yet.** `chat-api/src/messages.rs::react` (~line 220) gates on `access.can_read()` and not-archived only; no reaction permission check. |
| MentionEveryone | 7 | messages | Address a message to the whole space (`@everyone`). | `chat-api/src/messages.rs::send` (~line 128): the `everyone` flag is dropped unless `perms::can(…, MentionEveryone)` in a space (conversations allow it outright). `edit` (~line 197) keeps the original message's everyone flag rather than granting a new one — no permission call. |
| AttachFile | 8 | messages | Upload an attachment with a message. | **not enforced yet.** `send` caps attachments at 10 and validates uploads in `chat-api/src/files.rs`, but never asks the bit. |
| EmbedLink | 9 | messages | Post a link the reader expands. | **not enforced yet.** |
| VoiceConnect | 10 | voice | Join a voice channel's call. | **not enforced yet.** `chat-api/src/calls.rs` contains no `perms` reference at all. |
| VoiceSpeak | 11 | voice | Unmute their microphone. | **not enforced yet.** |
| VoiceVideo | 12 | voice | Use a camera. | **not enforced yet.** |
| VoiceShareScreen | 13 | voice | Share a screen. | **not enforced yet.** |
| VoiceForceMute | 14 | voice | Mute or deafen someone else for the call. | **not enforced yet.** |
| VoiceMoveMember | 15 | voice | Move someone between calls. | **not enforced yet.** |
| ManageChannels | 16 | channels | Create, delete and arrange channels. | `chat-api/src/channels.rs::create_channel` (~line 59). Note `delete_channel` (~line 117) and `order_channels` (~line 141) gate on ManageSpaceSettings instead — this bit buys only channel creation today. |
| ManageChannelRoles | 17 | channels | Set who may see and post in a channel. | **not enforced yet.** `channels.rs::update_channel` (~line 104) gates on ManageSpaceSettings via `perms::manages`. |
| ManageInvites | 18 | channels | Create and revoke invites. | **not enforced yet.** `spaces.rs::create_invite` (~line 228) lets any member invite (its comment says "managing invites is for admins" — unenforced); `spaces.rs::invites` / `revoke_invite` (~240/247) gate on ManageSpaceSettings through `body::manager`. |
| ManageRoles | 19 | space | Create, rename and delete roles. | Directly at `chat-api/src/spaces.rs::delete_role` (~line 202); through `perms.rs::can_author_role` at `create_role` (~167) and `update_role` (~186); through `can_grant`/`can_set_roles` at `set_roles` (~line 141). |
| ManageMembers | 20 | space | Kick, ban and time out members. | `perms.rs::can_moderate` → `chat-api/src/spaces.rs::remove_member` (~line 121); also underpins `set_roles`, `create_role`, `update_role`, `delete_role` (they all moderate the target). Ban/timeout themselves are unimplemented — see Planned. |
| ManageSpaceSettings | 21 | space | Change the space's own settings. | Directly: `chat-api/src/spaces.rs::body::manager` (~line 39) → `rename_space`, `create_category`, `rename_category`, `delete_category`, `invites`, `revoke_invite`; `chat-api/src/files.rs::set_space_icon` (~line 132); `chat-api/src/channels.rs::order_channels` (~line 141). Through `perms::manages` (~line 246 of perms.rs, used by `chat-api/src/channels.rs` at ~104, ~123, ~215, ~239): `update_channel`, `delete_channel`, `set_archived`, `rename_channel`. |
| IsAdmin | 63 | space | Every permission. | Enforced as expansion: `perms.rs::expand` inside `effective`, so the tag grants all 22 functional bits. Read as a presentation fact in `chat-model/src/live.rs::roster` (~lines 561/566), which sorts admins first. |

One caveat on "where": the `chat` client crate (nav menu, settings modal,
message row tools) makes show-hiding decisions with the old-rank calls
described in the Overview — they reference a `can_manage_space` that does not
exist yet, so no client gate at this snapshot resolves through the new model.

## Channel access

A `Channel` carries two permission masks (`chat-model/src/domain.rs::Channel.view`
and `.post`, `PermissionSet = u64`; persisted as `view_mask`/`post_mask`
bigints — `chat-db/src/channels.rs` maps them back to `view`/`post`).
`perms.rs::channel_access(user, channel, space)` computes the `Access`:

- private kinds (`ChannelKind::private` — Direct/Group): `Full` if the user is
  in `channel.members`, else `None`; masks don't apply.
- otherwise the user's effective set in the space; if it does not cover `view`
  → `Locked`; if it does not cover `view | post`, or the channel is archived
  → `ReadOnly`; else `Full`. Not being in the space at all → `None`.
- `Access::can_read()` is `>= ReadOnly`; `can_post()` is `== Full`.

Meanings (`perms.rs::Access` doc comments): **Full** — read and post;
**ReadOnly** — read but not post; **Locked** — the member knows the channel
exists but cannot read it; **None** — not theirs to know about (not in the
space, or a private channel they are not in). A thread is judged by its
**parent's** masks with the thread's own `archived` flag (`body::open` and
`live.rs::access` rebuild `Channel { archived: c.archived, ..parent }`).

Delivery matches the gate: `srv.rs::gated` is true when `view != 0`, and a
gated channel's messages ride their own topic, subscribed to only by viewers
whose effective set covers `view` (`topics_for`/`topics_of`), while ungated
channels ride the space topic. That is what makes a Locked member able to see
the channel exists but not read it.

## Role verbs

All in `chat-api/src/spaces.rs`, guarded by `chat-model/src/perms.rs` rules.

- **create_role** (~line 162): `can_author_role` — needs `ManageRoles`, and the
  new role's permission set, after `expand`, must be covered by the actor's own
  effective set. "You can't create a role with powers you do not have."
- **update_role** (~line 178): role must exist in the space; same `can_author_role`
  gate on the new set ("You can't grant powers you do not have.").
- **delete_role** (~line 197): plain `can(…, ManageRoles)`. Holders drop the
  role: `chat-db/src/spaces.rs::delete_role` also strips the id from every
  `space_members.role_ids` in one transaction; channel masks are untouched,
  "a mask a deleted role used simply becomes one nobody covers" (its doc comment).
- **set_roles** (~line 135): `can_set_roles` — the actor must moderate the
  target and hold `ManageRoles`, and every handed-out role must pass the actor's
  own coverage, so a batch cannot smuggle in power the actor lacks. Clears are
  acts too (`perms.rs` test "clearing a member's roles is an act…").
- Escalation rule everywhere: a role cannot carry power its author lacks
  (`can_author_role`'s `covers(effective(actor), expand(permissions))`), and the
  `IsAdmin` tag therefore buys the tag only for an admin (test
  `a_role_manager_cannot_author_a_role_beyond_their_powers`).

## Database

`crates/chat-db/migrations/0002_roles.sql`:

- `space_members`: drops the single `role` text column (an enum
  `member|admin|owner` in `0001_init.sql` line ~81), adds `role_ids uuid[]`
  defaulting to `{}`.
- `channels`: drops `view_role`/`post_role` rank floors (~lines 121/122), adds
  `view_mask bigint`/`post_mask bigint` defaulting to 0.
- Creates `space_roles` (`space_id` FK `ON DELETE CASCADE`, `role_id`, `name`,
  `permissions bigint` default 0, `position` default 0), PK `(space_id, role_id)`,
  indexed by space.

What it drops for existing data, at this snapshot: every prior rank is lost.
Former admins and owners come back with **no roles and no permissions** — only
the owner keeps power, because ownership lives on `spaces.owner_id`, untouched
by the migration. And every channel that used to ask for admin `view_role`/`post_role`
resets to mask 0: channels that were locked or read-only for ordinary members
silently open to everyone in the space. No clause in the file recreates roles
or translates old ranks into masks. A preservation migration is listed as
planned below.

## Planned (in progress — NOT in this snapshot)

The V100 worker is still changing `roles-permissions`. The following are
planned, not present in this snapshot, and none of it has been verified here:

- Enforcing the permissions the table above marks "not enforced yet".
- A default "everyone" baseline role/permission set.
- Role hierarchy by `position`.
- `ban` and `timeout` permissions and verbs (at this snapshot `ManageMembers`
  buys only `remove_member`; ban/timeout appear nowhere except the doc
  comment "Kick, ban and time out members" and the hearth `moderation_actions`
  table, which belongs to the old model).
- Per-channel role overwrites (allow and deny sets).
- A migration that keeps existing admins and locked channels (the 0002 above
  does not preserve them).
- Porting the `chat` client crate to the new model (see the Overview caveat:
  `my_role`, `can_manage_space`, `set_role` do not exist at this snapshot).

## Limits from end-to-end encryption

Message content is E2EE: the server does not read message bodies
(`docs/e2ee-protocol.md`; the call-key flow in `chat-keys` states the server
"must not learn" the key). So a permission that is really about **content** —
`MentionEveryone` and `EmbedLink` — can only be enforced where the server or
the shared `perms` module can see the act. `MentionEveryone` works at this
snapshot because the `send` verb parses mentions server-side before sealing
(`resolve_mentions` in `chat-api/src/messages.rs::send`); a permission about
text the server never parses (e.g. link policy inside sealed bodies) cannot be
checked there. Whether the client should pre-decide `EmbedLink` display, or
whether the sealed format needs a side-channel for it, is an **open question —
not solved at this snapshot**.
