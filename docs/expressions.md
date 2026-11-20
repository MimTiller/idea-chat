# Expressions: per-space custom emoji and stickers

The model as it stands in the final code on the `expressions` branch. Every
fact is re-derived from that code; file and function names come with each
claim.

## Overview

Each space carries two metadata sets — custom emoji and stickers — that are
**not** part of the message body: a message that uses one stores only a token
or an id, and the sets themselves are plain metadata in the clear. The rules
mirror RWND's product limits: 50 custom emoji per space at 256 KiB per image,
30 stickers per space at 512 KiB per image, and a sticker description of at
most 100 characters.

The single source of the rules is `crates/chat-model/src/expressions.rs`
(its own module in this crate), and the permission bit is the one
`crates/chat-model/src/perms.rs::Permission::ManageExpressions` (bit 24).

## The model (`chat-model/src/expressions.rs`, `domain.rs`, `live.rs`)

- `Emoji` and `Sticker` are records defined in `expressions.rs`; a `Sticker`
  is the emoji fields plus a `description` string (free text, checked to at
  most `STICKER_DESCRIPTION_MAX` = 100 characters) — a custom emoji is a bare
  name, so the description is what tells you what a sticker shows. `emoji_name_ok`
  (name only, no description). A sticker may have a description.
- `Space` gained two maps in `chat-model/src/domain.rs` (~134–139): `emoji:
  BTreeMap<Id, Emoji>` and `stickers: BTreeMap<Id, Sticker>`. Because older
  snapshots must still decode, both are `#[serde(default)]`; an older snapshot
  decodes with empty maps.
- A `Message` carries an optional `sticker: Option<Id>` (in `Message` in
  `domain.rs`). The body for a sticker-only message may be empty; a message
  needs text, an attachment, or a sticker, not nothing.
- `expressions.rs` consts: `EMOJI_MAX` = 50, `EMOJI_MAX_BYTES` = 256×1024,
  `STICKER_MAX` = 30, `STICKER_MAX_BYTES` = 512×1024. Name rules:
  `emoji_name_ok` is 2–32 characters, ASCII letters/digits/underscore only;
  `sticker_name_ok` is 2–30 characters, no control characters, none of `<`
  `>` `:` (the message token delimiters), and not blank-padded. `sticker_description_ok` is free text ≤100 characters.
- `text.rs::token_at` recognises only `<@` and `<#`, so a custom emoji token
  of the form `<:name:id>` passes through untouched and is never a mention.
  The unit test in `text.rs` says so directly.

## Wire changes (what a client/server sees on the socket and in JSON)

- `Space` snapshots: `emoji` and `stickers` maps added, each `#[serde(default)]`
  so an older snapshot still decodes (empty maps). The id-keyed shape is the
  same as roles' `BTreeMap<Id, …>` idiom.
- `Message`: a new optional `sticker` field (an upload-backed sticker `Id`); a
  body may be empty when `sticker` or an `attachments` carry the message.
- Uploads: `begin_upload`'s `purpose` argument accepts `"emoji"` and `"sticker"`
  (snake-cased from `Purpose::Emoji`/`Purpose::Sticker` in `files.rs::Purpose`),
  with the per-image byte limits 256 KiB and 512 KiB enforced there.
- Live events: two new `Event` variants in `chat-model/src/live.rs`,
  serialised with the `e` tag, snake-cased (`EmojiChanged`, `StickersChanged`).

## The verbs (`chat-api/src/spaces.rs`)

Six verbs, mirroring the roles template (create/rename/delete in `spaces.rs`,
the `publish_emoji`/`publish_stickers` helpers in the same body module
republish the space snapshot so connected members resync):

| Verb | Args (wire order) | Needs |
| --- | --- | --- |
| `list_emoji` | `(space)` | membership only |
| `list_stickers` | `(space)` | membership only |
| `create_emoji` | `(space, name, upload)` | `ManageExpressions` + a finished image upload |
| `rename_emoji` | `(space, emoji, new_name)` | `ManageExpressions`, name follows `emoji_name_ok`, no clash |
| `delete_emoji` | `(space, emoji)` | `ManageExpressions` |
| `create_sticker` | `(space, name, description, upload)` | `ManageExpressions` + a finished image upload, name follows `sticker_name_ok`, description follows `sticker_description_ok` |
| `rename_sticker` | `(space, sticker, new_name)` | `ManageExpressions`, `sticker_name_ok`, no clash |
| `delete_sticker` | `(space, sticker)` | `ManageExpressions` |

Listing needs only membership; create/rename/delete need the bit. `create_emoji`
checks the name form first, then the upload (via `upload_image_ok`, which checks
the image content type and the 256 KiB cap), then the space's `emoji_count`
against `EMOJI_MAX`. Refusals are readable sentences, e.g. "Only those who
manage custom emoji can add one." (emoji) and "Only those who manage stickers
can add one." (sticker), "The space is at its emoji limit." / "The space is at
its sticker limit."

## The permission (`ManageExpressions`, bit 24)

`crates/chat-model/src/perms.rs`: bits 0–23 were used and `IsAdmin` is bit 63;
the next free bit is 24, so `ManageExpressions` takes it (`Permission::ManageExpressions
=> 24`). `perms.rs::ALL` grows from `(1 << 63) | ((1 << 24) - 1)` to `(1 << 63)
| ((1 << 25) - 1)` so it covers exactly the defined bits; the `ALL` unit test
(`the_new_bits_stay_inside_all` in `perms.rs`) grows with it, and asserts
every defined bit is inside `ALL` and no invented bit slips in. The seeded
Admin role has everything (it carries `IsAdmin`, which expands to all bits, so
it already includes the new one). The seeded Moderator and the `@everyone`
baseline do not carry `ManageExpressions`. `docs/permissions.md`'s table gains
the `ManageExpressions` row (bit 24).

## The limits (`chat-model/src/expressions.rs`)

- `EMOJI_MAX` = 50 custom emoji per space; each emoji image ≤ 256 KiB (`EMOJI_MAX_BYTES`).
- `STICKER_MAX` = 30 stickers per space; each sticker image ≤ 512 KiB (`STICKER_MAX_BYTES`);
  a sticker's description is free text of at most 100 characters (`STICKER_DESCRIPTION_MAX`).

`begin_upload` (`chat-api/src/files.rs`) rejects a size of 0 or over the limit
with "Files can be up to 256 KiB." / "Files can be up to 512 KiB." for the emoji
and sticker purposes (the same limit branch that avatars/icons use, with the
smaller non-MB hint). Non-emoji/sticker purposes still go through the message
`MAX_UPLOAD` cap.

## Uploads and the sweep

An emoji's or sticker's image goes through the same upload path as an avatar or
a space icon: `Purpose` (`chat-api/src/files.rs`) gained `Emoji` and `Sticker`
(snake-cased on the wire); `begin_upload` picks the per-purpose byte limit,
requires an `image/*` content type for these purposes, and returns a `Slot`
whose `put_url` the client PUTs the bytes to; `finish_upload` confirms the
object landed and returns the `Attachment`. `chat-db`'s `sweep_uploads` deletes
**unclaimed** uploads; an upload referenced by a created emoji or sticker is
claimed by that record (`chat-db/src/spaces.rs` create_emoji/create_sticker),
so the sweep never deletes a referenced image. `chat-db/tests/db.rs` proves
the round-trip: create an emoji, sweep, the upload survives; create an emoji,
delete the emoji, sweep, the orphan upload is gone.

## Live events (`chat-model/src/live.rs`)

`Event` gained two variants (serde tag `e`, snake-cased):

- `EmojiChanged { space, emoji: Vec<Emoji> }` — the space's whole custom-emoji
  set restated.
- `StickersChanged { space, stickers: Vec<Sticker> }` — likewise for stickers.

`State` gained `emoji` and `stickers` on each `Space` (the same maps as the
model). The `State::apply` arms (~`live.rs` 235–243) replace the space's maps
when the restatement names them. A restatement (not a delta) is what a role
change already does: `set_roles`/`space_roles` republish the snapshot via the
`publish_roles` helper; `publish_emoji`/`publish_stickers` are the same shape
in `spaces.rs`. So when an emoji set changes, every connection that can see the
space takes a fresh snapshot (`SpaceUpserted`-style resync) and thus sees the
new set — exactly as connected members resync on a role change. The `delivery.rs`
test (`crates/chat-api/tests/delivery.rs`) asserts `EmojiChanged`/`StickersChanged`
reach a connected member and land on that member's space.

## Decisions (what the code says)

- **Deleting an emoji leaves old messages as plain `:name:` text.** The token in
  an already-posted message body is cleared text; deleting the emoji record does
  not rewrite posted bodies, so an old message's `<:name:id>` renders as the
  literal `:name:`. Deleting a sticker does not rewrite posted sticker ids.
- **Sending takes an optional sticker id that must belong to the space** —
  `send` (`chat-api/src/messages.rs`) refuses one that isn't the channel's space's
  with "That sticker isn't one of this space's."
- **A sticker-only message may have an empty body** — `send`'s empty-body check
  (`if text.is_empty() && attachments.is_empty() && sticker.is_none()`) lets a
  sticker carry an otherwise-empty body.
- **Listing needs only membership; create/rename/delete need `ManageExpressions`** —
  see the verbs table above.

## Sealed-message note

Expressions are **metadata in the clear** — the emoji/sticker sets and the
`sticker` id on a message are part of the snapshot and message envelope, not
the sealed body. None of the feature's choices depend on reading a sealed body,
so the feature survives sealed bodies: a `<:name:id>` token is left in the
message body's clear text (not a mention), the sticker id rides the message
metadata, and a reader who can open the channel already has the space's emoji
and sticker sets from the snapshot to draw the token/id with. A client that
cannot see the space (no `ViewChannel`) sees neither the set nor the message, so
nothing leaks.
