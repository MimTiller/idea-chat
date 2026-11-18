# Permissions

How who-may-do-what works in Idea Chat's chat tier: named roles that carry
permission bits, an owner who sits outside them, a per-space baseline, a
rank ladder, per-channel overwrites, and the rule functions the server and
the client share. Every statement here names the function or file it comes
from, at commit `ca96ac0`.

## The shape of it

- A space has **named roles**, each a record `{ id, name, permissions, position }`
  (`chat-model/src/domain.rs::Role`). `permissions` is a `u64` bitset.
- A member holds **any number of roles**; what they may do is the union of the
  roles' sets, on top of the space's **everyone baseline**.
- The **owner** is not a role. Ownership is a fact on the space; the owner holds
  every permission whatever roles they carry, and nobody can demote or remove them.
- **Rank** decides who may act on whom: a lower `position` is more senior, and you
  only ever act on those strictly below you.
- A channel can **widen or cut** a role's powers just there (per-channel overwrites),
  and can still ask for permission bits through its `view` and `post` masks.
- All the rules live in one module, `chat-model/src/perms.rs`. The server decides
  with them; the client calls the same functions to decide what to offer, so a
  button the server would refuse is not drawn.

## The owner

`perms.rs::effective` returns every permission for `space.owner`, whatever roles
they hold. `can_moderate`, `can_ban` and `can_timeout` refuse the owner as a target
and `can_transfer` is the only way ownership moves (the owner alone, to a member).
No permission buys any of that.

## Roles and the ladder

A member's rank is the position of their most senior role (`perms.rs::top_position`);
a member with no roles sits below every role. `reaches(actor, target)` is true for
the owner, and otherwise only when the target is strictly junior to the actor
(`target_position > actor_position`, or the target holds no roles). So:

- two members at the same rank never act on each other, and nobody acts on themselves;
- a member with no roles reaches nobody;
- `reaches_role(actor, position)` is the same test for a role, so nobody grants,
  edits or deletes a role at or above their own top role.

A new role is created at the bottom of the ladder (`max(position) + 1`), so rank
never blocks making one. A new space seeds two roles itself, in its own creation
transaction: `Admin` (the `IsAdmin` tag alone, which expands to every permission)
at position 0, and `Moderator` (`perms::DEFAULT_MODERATOR`: delete others'
messages, manage, ban and time out members, mention everyone, manage pins) at
position 1. A role made after them still lands below both. The role the migration
backfills for old admins sits at position 0.

## The everyone baseline

Every member holds the space's `everyone` set (`Space.everyone`, column
`spaces.everyone`) before any role is added. A new space starts at
`perms::DEFAULT_EVERYONE`: view, send, edit and delete your own messages, react,
attach, embed links, and voice connect, speak, video and share screen (bits
0, 1, 2, 3, 6, 8, 9, 10, 11, 12, 13; the integer `16207`). It stops where acts on other
people and on the space begin. A space whose baseline lacks a bit gives that power
only through roles.

`set_everyone` changes it (below). The `IsAdmin` tag can never be in a baseline.

## Permissions

24 functional permissions, bits 0 to 23 (`perms.rs::Permission`), plus the
`IsAdmin` tag at bit 63, which expands to all of them (`perms::expand`; `perms::ALL`
is exactly the defined bits, asserted in tests). "Enforced at" lists the production
call sites outside `perms.rs`.

| Permission | Bit | Allows | Enforced at |
| --- | --- | --- | --- |
| ViewChannel | 0 | See a channel at all | `perms::channel_access` (a channel's set must cover its `view` mask **and** this bit, else `Locked`); through it `chat-api` `channels.rs::body::open`, `messages.rs::search`, history, and live delivery (below) |
| SendMessage | 1 | Post in a channel | `channel_access` again (the set must also cover `post` **and** this bit, else `ReadOnly`); `Access::can_post()` at `send`, `edit`, `create_thread`, `join_call` |
| EditOwnMessage | 2 | Edit your own message | `perms::can_edit_message` at `messages.rs::edit` |
| DeleteOwnMessage | 3 | Delete your own message | `perms::can_delete_message` at `messages.rs::delete`; the client hides the control with the same call |
| DeleteOthersMessage | 4 | Delete someone else's | the second clause of `can_delete_message` |
| ManagePins | 5 | Pin and unpin | **not enforced: idea-chat has no pin endpoint to gate** |
| AddReaction | 6 | React | `messages.rs::react` via `can_at` |
| MentionEveryone | 7 | `@everyone` | `messages.rs::send` drops the flag unless held |
| AttachFile | 8 | Attach a file | `messages.rs::send` via `can_at` |
| EmbedLink | 9 | Post a link that expands | `messages.rs::send` and `edit` via `can_at` when the text holds a link |
| VoiceConnect | 10 | Join a call | `calls.rs::join_call` via `can_at` |
| VoiceSpeak | 11 | Unmute | `calls.rs::update_call` via `can_at` |
| VoiceVideo | 12 | Use a camera | `calls.rs::update_call` |
| VoiceShareScreen | 13 | Share a screen | `calls.rs::update_call` |
| VoiceForceMute | 14 | Mute or deafen someone else | **not enforced: no verb acts on another person's call** (a note in `calls.rs` says so) |
| VoiceMoveMember | 15 | Move someone between calls | **not enforced, for the same reason** |
| ManageChannels | 16 | Create channels | `channels.rs::create_channel` |
| ManageChannelRoles | 17 | Set who sees and posts in a channel; set its overwrites | `channels.rs::update_channel` when the masks move; `perms::can_set_overwrite` at `set_overwrite` and `clear_overwrite` |
| ManageInvites | 18 | Make, list and revoke invites | `spaces.rs::create_invite`, `invites`, `revoke_invite` |
| ManageRoles | 19 | Create, edit and delete roles; hand roles out; set the baseline | `perms::can_author_role`, `can_edit_role`, `can_grant`, `can_set_roles`, `can_set_everyone` |
| ManageMembers | 20 | Remove members | `perms::can_moderate` at `spaces.rs::remove_member`; also the first half of `can_grant` and `can_set_roles` |
| ManageSpaceSettings | 21 | Rename the space, its icon, categories; channel settings and ordering | `spaces.rs::body::manager`, `files.rs::set_space_icon`, `channels.rs` through `perms::manages` |
| BanMembers | 22 | Ban, and lift a ban | `perms::can_ban` at `ban_member`; `unban_member` needs only the bit |
| TimeoutMembers | 23 | Time a member out | `perms::can_timeout` at `timeout_member`; the effect is enforced in `send` and `update_call` |
| IsAdmin | 63 | Everything | `perms::expand`; exempt from channel overwrites; the roster lists tag holders first |

Three bits are defined and deliberately unchecked: `ManagePins`, `VoiceForceMute`
and `VoiceMoveMember`. The server has no act to put a check on, and a gate on a
control that does not exist would be a fake one.

## Channel access

A channel carries two masks, `view` and `post` (`Channel.view`, `Channel.post`;
columns `view_mask`, `post_mask`), and a set of per-role overwrites
(`Channel.overwrites`). `perms::channel_access` returns:

- `None` for a direct or group conversation you are not in, or a space you are not in
  (a conversation you are in is `Full`; masks do not apply to it);
- otherwise it takes your set **at the channel** (`perms::at_channel`): your space set,
  widened by the `allow` of every overwrite for a role you hold, then cut by their `deny`
  (`(base | allow) & !deny`, so a deny wins). The owner and anyone whose set carries the
  admin tag ignore overwrites;
- `Locked` if that set does not cover `view | ViewChannel`; `ReadOnly` if it does not cover
  `view | post | ViewChannel | SendMessage`, or the channel is archived; else `Full`.

A thread is judged by its parent's masks and overwrites, with the thread's own archived flag.
`Access::can_read()` is `ReadOnly` or better; `can_post()` is `Full`.

Acts taken at a channel (react, attach, embed, join a call, speak, video, share) go through
`perms::can_at`, which uses the set at the channel; space-wide acts (invites, roles, settings)
use the plain `effective` set.

## Live delivery

A channel's events normally ride the space's shared topic. A channel rides **its own topic**,
which only people allowed to read it subscribe to, whenever anyone in the space might not be:
its `view` mask is not empty, or it has any overwrite, or the space baseline lacks `ViewChannel`
(`perms::rides_own_topic`). `chat-model::live::topics_for` and the server's `topics_of`,
`channel_topic_of` and `channel_topic` all use that one rule and `channel_access`, so a person
who cannot read a channel does not receive its live messages.

When anything that changes who may see what changes (a role's powers, a role removed, the
baseline, a channel's masks or overwrites, a member's roles), the server calls `srv::resync`
for the affected members: their connection takes a fresh snapshot and recomputes its topics.
The same fresh snapshot is what a client gets after creating or joining a space, so a client
must apply a snapshot and refresh its screens from it, not only react to events.

## Verbs

In `chat-api/src/spaces.rs` and `channels.rs`. Each makes one call into `perms.rs` before it
touches the database.

| Verb | Rule | Needs |
| --- | --- | --- |
| `remove_member` | `can_moderate` | `ManageMembers`, target strictly below you, not the owner |
| `ban_member`, `unban_member` | `can_ban`; unban is the bit alone (the person is no longer a member, so rank cannot be compared) | `BanMembers`; for a ban, target strictly below you |
| `timeout_member(minutes)` | `can_timeout`; `None` clears | `TimeoutMembers`, target strictly below you |
| `set_roles(member, roles)` | `can_set_roles` | `ManageMembers` and `ManageRoles`; target below you; every role handed out below you and within your own powers; this includes clearing a member's roles |
| `create_role` | `can_author_role` | `ManageRoles`; the role may not carry power you lack (the admin tag only for an admin) |
| `update_role`, `delete_role` | `can_edit_role` (+ `can_author_role` for the new set) | `ManageRoles`; the role strictly below your top role |
| `set_everyone(permissions)` | `can_set_everyone` | `ManageRoles` (or owner); no admin tag; only powers you hold |
| `set_overwrite(channel, role, allow, deny)` | `can_set_overwrite` | `ManageChannelRoles` (or owner); an ordinary space channel only (not a thread or a conversation); the role strictly below you; `allow` only powers you hold; `deny` may be anything; both zero removes it |
| `clear_overwrite(channel, role)` | `can_set_overwrite` with an empty allow | same |
| `update_channel` | masks need `ManageChannelRoles`; other settings need `ManageSpaceSettings` | |

Deleting a role removes it from every holder and deletes its channel overwrites (foreign key).

## Database

`chat-db/migrations`:

- `0002_roles.sql` creates `space_roles`; then **backfills before dropping the old columns**:
  each space with old admins gets one `Admin` role (its id is the space's own id, permissions
  the admin tag, position 0) held by each of them, so they keep their power; the old
  per-channel `view_role`/`post_role` floors become masks (member means 0; admin or owner
  means the admin tag), so a locked channel stays locked. Old owners need no role, ownership
  is `spaces.owner_id`. Then `space_members.role` and the two floor columns are dropped.
- `0003_everyone.sql` adds `spaces.everyone`, default 16207.
- `0004_bans.sql` adds `space_bans` and `space_members.timeout_ends`.
- `0005_channel_overwrites.sql` adds `channel_overwrites` (foreign keys to the channel and to
  the role, both cascading).

A new space is created with the default baseline, the owner as its only member,
and **the two seeded roles** (`Admin` at 0, `Moderator` at 1, in
`chat-db::create_space`): nobody holds either, and the owner hands them out with
`set_roles`. They are ordinary roles afterwards, editable, deletable, renamable.
The `chat` client still has no screen for creating or editing roles, so out of
the box an owner in that client cannot make a *new* role, but the seeded pair
gives them something to hand out.

## Client

`crates/chat` is ported to the model (existing screens only): `Store::manages_space` backs the
settings modal and the space menu; a member line shows the member's first role name and a picker
of the roles the viewer may hand out (`can_grant`), calling `set_roles`; Remove shows under
`can_moderate`; the message delete control asks `can_delete_message`; a channel's "who can see
it" and "who can post" offer the role masks. There are **no screens to create or edit roles,
the baseline or overwrites, or to ban or time out**.

## Tests

- `chat-model`: unit tests in `perms.rs` for every rule.
- `chat-api/tests/roles_verbs.rs`, `baseline_overwrites.rs`, `delivery.rs`: the rules and the
  delivery decisions over spaces built in memory. They check the rules, not the HTTP handlers.
- `chat-db/tests/db.rs`: every query and migration against a real Postgres. These **skip, and
  still report green, unless `CHAT_TEST_DATABASE_URL` is set** to a server URL (each test makes
  its own database). `tools/check-all.sh` sets it.
- `tools/chat-roles-flows.js`: drives the real server functions over HTTP (79 assertions): the
  default baseline, the seeded Admin and Moderator and what their powers reach, rank and peers,
  ban and timeout, the baseline, overwrites, and an announcements channel only one role can post
  in. It found a bug the other tests missed (timeouts could never be set).
- `RUSTFLAGS=--cfg=web_sys_unstable_apis cargo check --target wasm32-unknown-unknown -p chat`
  checks the client. A native `cargo check --workspace` still fails in `chat`: the workspace
  turns on `chat-api`'s `server` feature, and the client cannot compile against the server's
  signatures.

## Not done

- Pins, force-mute and move-member have nothing to gate (above).
- There is no overwrite for the everyone baseline itself, only per role: "only Moderators post
  in #announcements" works in a space whose baseline lacks `SendMessage`, not in an ordinary one.
- No client screens for roles, the baseline, overwrites, ban or timeout; the
  seeded Admin and Moderator cover delegating without them.
- `perms::manages_channel` has no call site outside the module.

## Encryption

Text messages are **not** end-to-end encrypted today: `messages.body` is stored and indexed in
the clear, and the server reads message text when it checks mentions and links. That is what
lets `MentionEveryone` and `EmbedLink` be enforced at the act. `docs/e2ee-protocol.md` (Sigil) is
a proposal that "nothing uses yet", and it plans to replace the plaintext column and the
server-side search with sealed records. If that lands, a permission about message content could
no longer be checked by the server and would have to be enforced by clients, or by a clear-text
side channel such as Sigil's policy commits. That is an open question for the maintainers; it is not
solved here. Call media and the call room key are already end-to-end encrypted.
