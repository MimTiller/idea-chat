# Named roles with permission bitsets replace the three fixed ranks

## Summary

The space's Owner/Admin/Member ladder is gone. A space now owns named roles —
`Role { id, name, permissions: u64 bitset, position }` — and each member holds
the ids of the roles they have. What someone may do is the union of their roles'
permission sets, on top of a space-wide "everyone" baseline, computed in one
shared module, `chat-model/src/perms.rs`, which both the server and the client
consult so a refused button is never drawn. Ownership is no longer a role: it is
a fact on the space (`spaces.owner_id`), the owner always holds every permission,
and no role or verb can demote or remove them. Channels gate with two permission
masks (`view`, `post`) instead of rank floors, and per-channel allow/deny
overwrites per role widen or cut a member's set at that channel only. 24 named
permissions (bits 0–23, plus an `IsAdmin` tag at bit 63 that `expand`s to all)
cover messages, voice, channels and space settings. This PR is the full change:
the server, the model, the db plumbing, the backfill migration, and the port of
the `chat` client crate are all in. The head being described is commit `eb5e988`
("Port the chat client to named roles and permission bitsets").

## Changes

### chat-model

- `src/perms.rs` is the single rulebook: the `Permission` enum (24 functional
  bits plus `IsAdmin`), `effective` (the owner short-circuits to ALL; otherwise
  the member's roles unioned over the space's `everyone` baseline, then
  `expand`ed), `channel_access` with the `Access` lattice (None < Locked <
  ReadOnly < Full), the per-channel overwrite algebra `at_channel`
  (`(base | allow) & !deny`, owner and admin-tag sets ignore overwrites), and
  the verb guards `can_moderate`, `can_ban`, `can_timeout`, `can_grant`,
  `can_set_roles`, `can_author_role`, `can_edit_role`,
  `can_delete_message`, `can_edit_message`, `can_react`, `can_attach`,
  `can_embed`, the four voice guards, `can_invite`. A `position` on each role
  makes the ladder: `reaches`/`reaches_role` let an act only go "at their own
  rank or below". `DEFAULT_EVERYONE` (bits 0,1,2,3,6,8,9,10,11,12,13) is the
  baseline new spaces seed.
- `src/domain.rs`: `Role` is the record above; `Space` gains
  `roles: BTreeMap<Id, Role>` and `everyone: PermissionSet`; `Member` holds
  `roles: Vec<Id>`; `Channel.view`/`post` become `PermissionSet` (u64) masks
  and `Channel.overwrites` carries per-role allow/deny; ownership is
  `Space.owner`. `src/live.rs::roster` sorts admin-tag holders first.
- Ban and timeout are permissions with verbs, not rank fields.

### chat-api

- New server verbs in `src/spaces.rs`: `create_role`, `update_role`,
  `delete_role`, `set_roles`, `space_roles`, `ban_member`, `unban_member`,
  `timeout_member`, each guarded by the perms rules; the escalation rule — a
  role cannot carry power its author lacks — is `can_author_role` and is
  re-checked per role inside `can_set_roles`, so a batch cannot smuggle in
  power the actor lacks. `create_invite`/`invites`/`revoke_invite` now gate on
  `ManageInvites` (any member could invite before); `remove_member` on
  `can_moderate`; the space-settings verbs on `ManageSpaceSettings`
  (`body::manager`).
- Existing verbs re-gated on bits: `send` drops the `@everyone` flag unless the
  member holds `MentionEveryone`, requires `AttachFile` for attachments and
  `EmbedLink` when `text::embeds` sees a link, and refuses a timed-out member
  (`timed_out`); `edit`/`delete` go through `can_edit_message`/`can_delete_message`
  — `DeleteOwnMessage` is now enforced for the author, not just
  `DeleteOthersMessage` for others; `react` on `AddReaction`; the channel
  masks in `update_channel` on `ManageChannelRoles`; `join_call` on
  `VoiceConnect` and the mute/camera/screen controls in `update_call` on
  `VoiceSpeak`/`VoiceVideo`/`VoiceShareScreen`.
- Delivery-side gating in `src/srv.rs`: a channel with a nonzero `view` mask
  rides its own topic; only viewers whose effective set covers the mask
  subscribe (`gated`, `topics_for`, `topics_of`), so a locked channel's
  messages never cross the wire.
- `crates/chat-api/tests/roles_verbs.rs`: 25 unit tests, one allow/refusal pair
  per new gate, asserting the `perms` call each `#[server]` verb makes before
  it touches the db.

### chat-db

- `migrations/0002_roles.sql` (below) creates `space_roles` and backfills
  before the old rank columns go — old admins keep their power, old locked
  channels keep their lock. Plus `0003_everyone.sql` (the baseline column),
  `0004_bans.sql` (`space_bans` + `space_members.timeout_ends`), and
  `0005_channel_overwrites.sql` (per-role allow/deny rows with CASCADE FKs to
  both the channel and the role, so neither deletion leaves orphans).
- The role plumbing in `src/spaces.rs` (`create_role`/`update_role`/
  `delete_role`/`set_role_ids`, `ban`/`unban`/`timed_out`/`set_timeout`, and
  the owner-exclusion predicates), `src/channels.rs` mapping
  `view_mask`/`post_mask` and aggregating `channel_overwrites`, and a db test
  round-tripping the verbs in `tests/db.rs`.
- `redeem_invite` refuses a banned joiner ("That invite no longer works for
  you."), which is what gives `BanMembers` its effect.

### chat (client)

- The `chat` crate is ported in this PR: `settings.rs` and `nav.rs` gate on
  `perms::has(my_permissions(space), ManageSpaceSettings)` over
  `live.rs::my_permissions`, the member line shows role names read-only plus a
  picker filtered through `can_grant` calling `set_roles`, `message.rs` RowTools
  hides delete via `perms::can_delete_message` at the channel, and the channel
  page's WHO CAN SEE IT / WHO CAN POST offer the space's role masks plus
  Everyone. The old `my_role`/`can_manage_space`/`set_role` calls no longer
  exist anywhere in the crate.

## Migration

`0002_roles.sql` at `eb5e988` backfills before the old columns drop:

- Each space with any old `role = 'admin'` gets one `Admin` role whose id is
  the space's own id (cannot clash), permissions `-9223372036854775808`
  (INT64_MIN, bit 63, the `IsAdmin` tag — `expand` turns it into every
  permission), and `position 0`, so ex-admins stay senior to any custom role
  made later. Their `role_ids` are set to it; the `role` column is then
  dropped. Old `role = 'owner'` members get no role: their power comes from
  `spaces.owner_id`, which the migration never touches.
- Before `view_role`/`post_role` are dropped, each floor becomes a mask:
  `'member'` → 0 (asks nothing of ordinary members); anything else
  (`'admin'`, `'owner'`) → the admin-tag mask, so a channel that was locked or
  read-only for ordinary members keeps locking them out — the old owner-only
  floor ("nobody but the owner" was all it could mean) reads as the same admin
  lock, since the space's owner holds everything anyway.
- Nothing silently opens and no former admin comes back powerless; the earlier
  snapshot's drop-everything migration is superseded.

## Tested

- `cargo test -p chat-model -p chat-api -p chat-db` passes on the build host
  against this code: 108 tests across the three crates, 0 failures.
- `RUSTFLAGS=--cfg=web_sys_unstable_apis cargo check --target wasm32-unknown-unknown -p chat`
  passes — the client crate compiles against the new model.
- `cargo check --workspace --all-targets` run natively still fails in the `chat`
  crate with "missing `Auth<Principal>`" errors. The V100 worker reports these
  same errors also occur on `master` (88 there, 59 now) and come from cargo
  unifying chat-api's `server` feature across the workspace because chat-server
  is a member; the count on `master` has not been independently verified — it is
  the worker's report.
- End-to-end browser check (role created and handed out, a member below a
  channel mask shown Locked and hearing no messages): **not done** — needs the
  dev-server/browser run on the build host, not yet exercised.

## Notes and open questions

- **Do roles grow, or do the three ranks stay?** docs/data-model.md §8 still
  says "three roles, no capability bitfields… if a permission matrix is wanted
  later it is an additive" change — this branch is one answer to Nicho's open
  question: 24 bits with `IsAdmin` as the escape hatch so an admin tag *is* the
  old rank, and the backfill uses exactly that hatch. Is the granularity right?
  Three bits are still not enforced anywhere: `ManagePins` (no pin endpoint
  exists in this stack at all), `VoiceForceMute` and `VoiceMoveMember` (no verb
  acts on someone else's call; `calls.rs` carries an explicit comment naming
  them so the audit does not read the absence as a missing check).
- **Relationship to `crates/sigil/src/policy.rs`:** sigil has its own `Role`
  enum (Member/Admin/Owner) with its own rules (`may_add: Roles::at_least(Role::Admin)`
  style) — the same shape of idea as `can_grant`/`can_moderate`, and its doc
  says policy commits are sent in the clear, which is what lets the sigil
  server enforce `may_send` without reading message content: the pattern the
  chat side could borrow for the content-permission problem below. What does
  not overlap: sigil's lattice is a fixed three ranks evaluated on group-epoch
  commits inside a group context every member holds, while the chat model is
  named roles with u64 bitsets checked against database state; nothing in
  either crate references the other's `Role`, so today they are two parallel
  models a reviewer will read as one concept with two definitions. Worth
  deciding whether sigil's policy vocabulary should consume the chat bitsets
  or stay deliberately separate (group/E2EE layer vs space layer).
- **E2EE limit:** nothing in this stack is sealed today —
  `docs/e2ee-protocol.md` is a proposal and `chat-api` reads message bodies in
  cleartext, which is what lets `MentionEveryone` (parsed server-side in
  `send`) and `EmbedLink` (`text::embeds` over the body) be enforced at the
  act, and `search` read bodies. If sealed bodies ever land, the permissions
  that act on message content — `MentionEveryone`, `EmbedLink`, and the
  server-side `search` — cannot be checked by a server that cannot read the
  body: either the sealed format gains a cleartext side-channel the server can
  gate on (the sigil policy-commit trick above), or those permissions become
  client-only, which contradicts the rule that client checks never protect.
  Open question for Nicho, not solved at this commit.
- **The client:** this PR ports the existing screens only. There are no
  role-creation, role-editing, ban/timeout or per-channel-overwrite UIs —
  `create_role`, `update_role`, `delete_role`, `ban_member`,
  `unban_member`, `timeout_member` and the overwrite writes exist on the
  server with tests, but nothing in `crates/chat/src` calls them, and no API
  verb writes `channel_overwrites` rows yet. That screen set is a follow-up.
- **Hearth is untouched:** `crates/hearth` and `crates/hearth-db` keep their
  own role enum (owner|moderator|member) and their `moderation_actions`
  table; this branch does not migrate them.
- The `chat` crate's native `--workspace --all-targets` feature-unification
  breakage (above) is what a native workspace check shows today; the wasm
  build of `chat` passes, and the API crates build with their `server` feature
  wired through `chat-server`. If the reviewer wants a clean native workspace
  check, chat-server needs to be made a non-member or the features
  un-hard-merged — not touched here.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
https://claude.ai/code/session_01YUsp2gqWUmvQs1kqbewJL8
