# Notification settings

Each person picks a notification level per space and per channel, with an
optional expiry. The model mirrors RWND's `discord-server/src/notification_settings.rs`;
the two line up on the names, the storage values, and the millisecond clock.

## The model

A `NotificationSetting` (`chat-model/src/domain.rs`) is one row for one
person:

- `scope`: `Space` or `Channel` — what kind of target the row names.
- `target`: the space's or channel's `Id`.
- `level`: `All`, `Mentions`, or `Nothing`; `None` inherits — a channel from
  its space's row, a space the default of showing everything.
- `muted_until`: `None` (not muted), `MUTED_UNTIL_TURNED_OFF` (-1, a mute with
  no end), or an epoch-ms deadline.

`Level` and `Scope` carry `db_str`/`from_db` with the same values as RWND's
module (`"all"`, `"mentions"`, `"nothing"`), so the two backends can share a
row format.

## The rule

One function decides everything: `State::notification_shows(target, now_ms)`
(`chat-model/src/live.rs`), returning `(unread dot, mention badge)` — the
clock is an argument so tests need no sleeping.

- The level is the target's own setting, else (for a channel) its space's,
  else `All`.
- A mute is in force while the plain `muted` flag is set OR the setting's
  `muted_until` is in the future (`is_muted`); an expired timed mute counts as
  not muted again.
- `Mentions`: no unread dot, the mention badge keeps. `Nothing`: neither.
  `All`: a mute in force hides the dot but keeps mentions.

Where it is used: `State::unread`, `State::mentions`, `space_badge` and
`home_badge` (which count through `unread`/`mentions`), and the client's
counters in `crates/chat` (`channel.rs`, `nav.rs`) pass `s.now()` as the
clock. The plain `muted` set stays for back-compat with `set_muted`.

## The verbs (`chat-api/src/notifications.rs`)

- `notification_settings(space: Option<Id>, user) -> Vec<NotificationSetting>`
  — the caller's own settings, all of them or one space's (its own row and its
  channels' rows); reading a space's needs membership.
- `set_notification_setting(setting, user) -> NotificationSetting` — stores
  one of the caller's own settings (a row with no level and no mute clears).
  The target must be readable: membership for a space, `channel_access(...).can_read()`
  for a channel. A `muted_until` that is not the marker or a positive deadline
  is refused ("A mute end must be in the future.").
- `set_muted` keeps working unchanged; `Level::All` with no mute is the same
  as not muted.

## The live event

`Event::NotificationSettingsChanged { settings }` restates the person's whole
setting set. `set_notification_setting` publishes it to the person's own topic
(`u:<id>`), so every connection of that person — their other sessions and
devices — follows the change. Settings ride the person's own topic; nothing
about them is broadcast to others. The snapshot (`chat-db/src/snapshot.rs`)
fills `state.settings` from the db so a fresh connection starts with them.

## The data (`chat-db`)

Migration `0009_notification_settings.sql` (0008 is taken on the `expressions`
branch, which will meet this one):

```sql
CREATE TABLE notification_settings (
    user_id     uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    scope       text NOT NULL CHECK (scope IN ('space', 'channel')),
    target_id   uuid NOT NULL,
    level       text CHECK (level IN ('all', 'mentions', 'nothing')),
    muted_until bigint,
    PRIMARY KEY (user_id, target_id)
);
```

The primary key is `(user_id, target_id)`: one row per person per target, as
the live state keeps `settings` as a map by target, so a target is not
duplicated. `target_id` has no single foreign key because it names either a
space or a channel; deleting a person still cascades. Queries live in
`chat-db/src/people.rs`: `notification_settings(user, space)` (all, or one
space's own and its channels' rows, ordered `scope, target_id`) and
`set_notification_setting(user, setting)` (insert-or-replace; an empty setting
deletes).

## Wire changes for RWND's backend

- `Scope::Server` becomes `Scope::Space` (value `"space"`); `db_str` for
  scopes is `"space"`/`"channel"`.
- `scope_id` (int64) becomes `target` (UUID `Id`).
- `NotificationSetting { scope, target, level, muted_until }` — same field
  names otherwise, same level names, `muted_until` the same epoch-ms integer,
  `MUTED_UNTIL_TURNED_OFF = -1`.
- A channel's `parent`/space lookup on the wire is the same shape as
  `State::channels`; the rule reads a channel's space from the live state.
- The restatement event is `Event::NotificationSettingsChanged` delivered to
  `u:<id>`, with settings as a list ordered `scope, target_id`.

## Tests

- `chat-model/src/live.rs`: the rule — each level, expiry at the exact
  boundary (in force just before, expired at the deadline), the channel
  overriding its space, the plain-mute template test
  (`a_muted_channel_shows_no_dot_but_keeps_its_mentions`).
- `chat-db/tests/db.rs`: store/replace/clear round trip, per-person isolation,
  the snapshot carrying settings, `set_muted` beside them.
- `chat-api/tests/notifications_verbs.rs`: the gates (member allow / stranger
  refuse; readable channel allow / locked channel refuse; conversation
  membership), the mute-end rule, and that the restatement reaches only the
  person's own topic.
