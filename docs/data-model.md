# Data model and authentication — design

Status: **the schema is built.** Migrations live in `crates/chat-db/migrations/`
(five files, applied by `sqlx::migrate!` at connect) and `crates/hearth-db/migrations/`
(two files, same runner). This document describes the schema those migrations
define, for both tiers; the auth-flow prose is still partly proposal. Open
questions are collected at the end.

Scope: identity, spaces, channels, messages, membership, read state, roles and
permissions, and every authentication flow, for both the Idea Chat tier and the
Hearth app. Out of scope: media storage internals, push notifications, and the
voice media path (which already exists — see §9 for how the two tiers meet).

---

## 1. What the framework already provides

The design below is deliberately thin, because idealyst carries most of the
mechanism already. Building a parallel stack would be waste.

| Need | What we use | What we still write |
| --- | --- | --- |
| RPC | `#[server]` async fns, isomorphic | the function bodies |
| Live events | `#[channel]` — WebSocket duplex, extractors resolved at upgrade | the protocol enum + fan-out |
| Cross-node fan-out | `pubsub` SDK (redis/postgres backend) | topic naming |
| Sessions | `server_kit::Sessions` — opaque CSPRNG token, httpOnly cookie for web, bearer for native, TTL, guard middleware | the principal type |
| Authorization | `Auth<P>`, `Role<M>`, `require("prefix/")`, `csrf_guard` | role marker types |
| Rate limits | `#[server(tags(limit = "5/min"))]` + `rate_limit().key_by(…)` | the limits themselves |
| Transactional email | `email` SDK | the templates |
| Native secret storage | `credentials` (Keychain/Keystore) | nothing |
| Offline send | `sync` SDK's outbox, or a simpler local queue | see §10 |

**No database opinion ships with the framework.** `server-kit`'s own worked
example is `State<Db>` over an `sqlx::PgPool`, and that's what §3 assumes.

Note a divergence to resolve: `voice-coord` (already in this repo) uses
`tokio-postgres` + `deadpool`, not `sqlx`. The chat tier uses **sqlx** —
compile-time-checked queries were argued for, but the shipped code went
runtime-checked (see §13). Two Postgres clients in one workspace is a real (if
small) cost; flagged in §12.

## 2. Constraints that actually drive the design

Three, and they do most of the work:

**Self-hosting has to be genuinely easy.** This is the stated point of the
project. Concretely it means: no mandatory external identity provider, no
mandatory SMTP, no default password anywhere, and a first-run that can't leave
an instance briefly open to the internet. §7 is shaped almost entirely by this.

**The product is anti-bloat, and the schema should be too.** The design is
emphatic that notifications are one setting rather than a per-channel matrix.
The same conviction applied to permissions at the start: three ranks, no
capability bitfields. The chat tier's migrations then went the other way on
that open question — `0002_roles.sql` through `0005_channel_overwrites.sql`
replace the three ranks with named roles carrying permission bitsets, a
space-wide everyone baseline, and per-channel allow/deny overwrites — while
the Hearth tier keeps the three-rank scheme (`space_members.role`). §4
describes both as the migrations leave them.

**A voice tier already exists and is not mine to redesign.** `voice-server` is
a binary WebSocket audio relay with its own coordination store. The chat tier
must authorise voice joins without coupling to it — §9.

## 3. Entity model

Postgres 16+, `citext` extension enabled (one line in the first migration —
case-insensitive handles and emails are worth it, and folding case in the
application is the version of this that eventually gets forgotten somewhere).
All ids are **UUIDv7**, generated application-side, so id comparisons are
time comparisons and history pages by id. Enumerations are text with CHECK
constraints rather than Postgres enums, so adding a value is a migration that
cannot lock a table (`0001_init.sql`'s header).

The repo ships **two schemas**: the Idea Chat tier (`chat-db`, the full
design — DMs, threads, roles, calls, live connections) and the Hearth app
(`hearth-db`, the shipped product: spaces, channels, messages, invites,
voice presence). §3.1–§3.6 describe Hearth's tables; §3.7–§3.11 describe the
chat tier's.

### 3.1 Hearth: identity

Tables: `users`, `credentials`, `auth_events`.

```sql
CREATE TABLE users (
    id                uuid PRIMARY KEY,
    handle            citext UNIQUE NOT NULL,
    display_name      text NOT NULL,
    email             citext UNIQUE,               -- NULL: valid, see §7.6
    email_verified_at timestamptz,
    avatar_key        text,
    is_instance_admin boolean NOT NULL DEFAULT false,
    created_at        timestamptz NOT NULL DEFAULT now(),
    deleted_at        timestamptz,
    CONSTRAINT users_handle_shape
        CHECK (deleted_at IS NOT NULL OR handle ~ '^[a-z0-9_]{2,32}$')
);

CREATE TABLE credentials (
    id           uuid PRIMARY KEY,
    user_id      uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind         text NOT NULL,
    secret       text,          -- password: argon2id PHC string
    issuer       text,          -- oidc
    subject      text,          -- oidc
    confirmed_at timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT credentials_kind CHECK (kind IN ('password', 'oidc', 'totp')),
    CONSTRAINT credentials_oidc_identity UNIQUE (issuer, subject)
);
CREATE UNIQUE INDEX credentials_one_per_kind
    ON credentials (user_id, kind) WHERE kind IN ('password', 'totp');

CREATE TABLE auth_events (
    id         uuid PRIMARY KEY,
    user_id    uuid REFERENCES users(id) ON DELETE SET NULL,
    kind       text NOT NULL,
    ip         inet,
    user_agent text,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX auth_events_by_user ON auth_events (user_id, created_at DESC);
```

Deletion is a tombstone (`deleted_at`), not a delete: messages stay attributed
to an account that renders as "Deleted account". The handle-shape rule applies
to live accounts only — a tombstone's handle is generated, and holding it to
the same shape would make deletion impossible (this bug and its fix are in
§13).

**Sessions are not in Postgres.** `server_kit::Sessions` rides the `cache`
contract (Redis in production, in-memory in dev) — the right store for a
KV-with-TTL. The chat tier backs "sign out everywhere" in the schema instead:
`users.auth_epoch` is bumped on password change and on sign-out-everywhere,
and a session minted under an older epoch is refused (§3.7). It also keeps a
`devices` table so the session list has labels to show.

### 3.2 Hearth: spaces and membership

Tables: `spaces`, `space_members`.

```sql
CREATE TABLE spaces (
    id          uuid PRIMARY KEY,
    slug        citext UNIQUE NOT NULL,
    name        text NOT NULL,
    description text,
    icon_key    text,
    created_by  uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    archived_at timestamptz,
    CONSTRAINT spaces_slug_shape CHECK (slug ~ '^[a-z0-9-]{2,48}$')
);

CREATE TABLE space_members (
    space_id  uuid NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    user_id   uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role      text NOT NULL DEFAULT 'member',
    joined_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (space_id, user_id),
    CONSTRAINT space_members_role CHECK (role IN ('owner', 'moderator', 'member'))
);
CREATE INDEX space_members_by_user ON space_members (user_id);
```

### 3.3 Hearth: channels — one table for rooms and DMs

Tables: `channels`, `channel_members`.

The design draws text channels, voice rooms, video rooms, share rooms and
direct messages as five things. They are one thing with a `kind`, and DMs are
the same thing with no space:

```sql
CREATE TABLE channels (
    id              uuid PRIMARY KEY,
    space_id        uuid REFERENCES spaces(id) ON DELETE CASCADE,  -- NULL ⇒ DM
    kind            text NOT NULL,
    name            text,
    topic           text,
    position        int NOT NULL DEFAULT 0,
    last_message_id uuid,
    dm_key          text UNIQUE,
    created_at      timestamptz NOT NULL DEFAULT now(),
    archived_at     timestamptz,
    CONSTRAINT channels_kind CHECK (kind IN ('text', 'voice', 'video', 'share', 'dm')),
    CONSTRAINT channels_dm_shape CHECK ((space_id IS NULL) = (kind = 'dm')),
    CONSTRAINT channels_dm_key_shape CHECK ((dm_key IS NULL) = (kind <> 'dm')),
    CONSTRAINT channels_named CHECK (kind = 'dm' OR name IS NOT NULL)
);
CREATE INDEX channels_by_space ON channels (space_id, position);

CREATE TABLE channel_members (
    channel_id  uuid NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    user_id     uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (channel_id, user_id)
);
CREATE INDEX channel_members_by_user ON channel_members (user_id);
```

Only DMs are populated in `channel_members`; space channels derive membership
from `space_members` — there are no private channels yet, and adding them
means populating this table rather than reshaping it. `last_message_id` is
denormalised so "does this channel have unread" compares two ids rather than
aggregating over messages, and gets a foreign-key link to `messages` by
`ALTER TABLE channels ADD CONSTRAINT channels_last_message FOREIGN KEY …` in
the same migration.

### 3.4 Hearth: messages and threads

Tables: `messages`, `attachments`.

A thread is not an entity. A reply points at the message it hangs off
(`thread_root_id`), which is exactly what the design shows — a badge on a
message, opening a panel of its replies.

```sql
CREATE TABLE messages (
    id             uuid PRIMARY KEY,
    channel_id     uuid NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    author_id      uuid REFERENCES users(id) ON DELETE SET NULL,
    thread_root_id uuid REFERENCES messages(id) ON DELETE CASCADE,
    body           text,
    reply_count    int NOT NULL DEFAULT 0,
    created_at     timestamptz NOT NULL DEFAULT now(),
    edited_at      timestamptz,
    deleted_at     timestamptz,
    CONSTRAINT messages_body_or_tombstone CHECK (deleted_at IS NOT NULL OR body IS NOT NULL)
);
CREATE INDEX messages_channel_page ON messages (channel_id, id DESC)
    WHERE thread_root_id IS NULL;
CREATE INDEX messages_thread ON messages (thread_root_id, id)
    WHERE thread_root_id IS NOT NULL;

CREATE TABLE attachments (
    id           uuid PRIMARY KEY,
    message_id   uuid NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    filename     text NOT NULL,
    content_type text NOT NULL,
    byte_size    bigint NOT NULL,
    storage_key  text NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX attachments_by_message ON attachments (message_id);
```

Pagination is `WHERE channel_id = $1 AND id < $cursor ORDER BY id DESC LIMIT
50` — no offset, no ordering column, stable under concurrent inserts.

### 3.5 Hearth: read state, invites, moderation, settings

Tables: `read_state`, `invites`, `user_mutes`, `moderation_actions`,
`instance_settings`.

```sql
CREATE TABLE read_state (
    user_id              uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    channel_id           uuid NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    last_read_message_id uuid,
    mention_count        int NOT NULL DEFAULT 0,
    updated_at           timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, channel_id)
);

CREATE TABLE invites (
    code       text PRIMARY KEY,                -- 'quiet-porch' — human-readable
    space_id   uuid NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    created_by uuid REFERENCES users(id) ON DELETE SET NULL,
    expires_at timestamptz NOT NULL,            -- default now() + 7 days
    max_uses   int NOT NULL DEFAULT 1,          -- the design: "only work once"
    use_count  int NOT NULL DEFAULT 0,
    revoked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT invites_uses_sane CHECK (use_count >= 0 AND max_uses >= 1)
);
CREATE INDEX invites_by_space ON invites (space_id);

-- "Mute for me" — a personal filter, invisible to its subject.
CREATE TABLE user_mutes (
    user_id    uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    muted_id   uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, muted_id),
    CONSTRAINT user_mutes_not_self CHECK (user_id <> muted_id)
);

-- "Mute in rooms for an hour", "Remove from space" — audited, because they
-- are done TO someone.
CREATE TABLE moderation_actions (
    id         uuid PRIMARY KEY,
    space_id   uuid NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    target_id  uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    actor_id   uuid REFERENCES users(id) ON DELETE SET NULL,
    kind       text NOT NULL,
    expires_at timestamptz,
    reason     text,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT moderation_kind CHECK (kind IN ('timeout', 'remove', 'role_change'))
);
CREATE INDEX moderation_by_target ON moderation_actions (space_id, target_id, created_at DESC);

-- Instance settings — exactly one row, enforced.
CREATE TABLE instance_settings (
    id                integer PRIMARY KEY DEFAULT 1,
    registration_mode text NOT NULL DEFAULT 'invite_only',
    instance_name     text NOT NULL DEFAULT 'Hearth',
    created_at        timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT instance_settings_singleton CHECK (id = 1),
    CONSTRAINT registration_mode CHECK (registration_mode IN ('invite_only', 'open', 'closed'))
);
INSERT INTO instance_settings (id) VALUES (1);
```

`moderation_actions` audits Hearth's moderator verbs; it is a record of acts,
not the state itself — the timeout state lives on the member row. (The chat
tier has no such table; see §3.8.)

### 3.6 Hearth: voice presence

Table: `voice_presence`.

Where people are, in voice — owned by the chat server rather than derived
from the relay fleet: a relay forwards bytes, and presence read from the
fleet goes blank the moment the fleet is unreachable. One row per person
(`user_id` primary), so being in two calls at once is not a state the
database can hold. A row is only presence while `seen_at` is fresh; the worst
case is one stale entry for one heartbeat interval rather than a ghost until
someone notices.

```sql
CREATE TABLE voice_presence (
    user_id    uuid PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    channel_id uuid NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
    joined_at  timestamptz NOT NULL DEFAULT now(),
    seen_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX voice_presence_channel ON voice_presence (channel_id, joined_at);
```

### 3.7 Chat tier: accounts and people

Tables: `users`, `email_codes`, `devices`, `relations`.

The chat tier's `users` differs from Hearth's: it carries password, presence,
status text and the session epoch, and uniqueness goes through case-folded
indexes rather than `citext`:

```sql
CREATE TABLE users (
    id                uuid PRIMARY KEY,
    email             text NOT NULL,
    password_hash     text NOT NULL,
    handle            text NOT NULL,
    display_name      text NOT NULL,
    avatar            text,
    bio               text NOT NULL DEFAULT '',
    status_presence   text NOT NULL DEFAULT 'online'
                      CHECK (status_presence IN ('online', 'idle', 'busy', 'offline')),
    status_text       text,
    email_verified_at timestamptz,
    auth_epoch        integer NOT NULL DEFAULT 0,
    created_at        timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX users_email ON users (lower(email));
CREATE UNIQUE INDEX users_handle ON users (lower(handle));

CREATE TABLE email_codes (
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    purpose    text NOT NULL CHECK (purpose IN ('verify', 'reset')),
    code_hash  text NOT NULL,
    attempts   integer NOT NULL DEFAULT 0,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, purpose)
);

CREATE TABLE devices (
    id         uuid PRIMARY KEY,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name       text NOT NULL,
    public_key text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_seen  timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX devices_key ON devices (public_key);
```

Verification and reset codes are stored hashed, one live code per account and
purpose. Relations store both directions as rows — `(a, b, 'outgoing')` beside
`(b, a, 'incoming')`, `(a, b, 'friend')` twice — and a block is one-sided:

```sql
CREATE TABLE relations (
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    other_id   uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind       text NOT NULL CHECK (kind IN ('friend', 'incoming', 'outgoing', 'blocked')),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, other_id),
    CHECK (user_id <> other_id)
);
```

The `users.status_presence` column and the `connections` table (§3.11) both
describe presence; which one is authoritative is not decidable from the
migrations alone — flagged here rather than guessed.

### 3.8 Chat tier: spaces, roles, bans

Tables: `spaces`, `space_members`, `categories`, `invites`, `space_roles`,
`space_bans`, `channel_overwrites`.

```sql
CREATE TABLE spaces (
    id         uuid PRIMARY KEY,
    name       text NOT NULL,
    icon       text,
    owner_id   uuid NOT NULL REFERENCES users (id),
    created_at timestamptz NOT NULL DEFAULT now()
);
-- 0003_everyone.sql:
ALTER TABLE spaces ADD COLUMN everyone bigint NOT NULL DEFAULT 16207;

CREATE TABLE space_members (
    space_id  uuid NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    user_id   uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    position  integer NOT NULL DEFAULT 0,     -- the member's own space order
    joined_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (space_id, user_id)
);
-- as of 0002_roles.sql: DROP role, ADD role_ids; 0004_bans.sql: ADD timeout_ends
ALTER TABLE space_members ADD COLUMN role_ids uuid[] NOT NULL DEFAULT '{}';
ALTER TABLE space_members ADD COLUMN timeout_ends timestamptz NULL;

CREATE TABLE categories (
    id       uuid PRIMARY KEY,
    space_id uuid NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    name     text NOT NULL,
    position integer NOT NULL DEFAULT 0
);

CREATE TABLE invites (
    code       text PRIMARY KEY,
    space_id   uuid NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    created_by uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    uses       integer NOT NULL DEFAULT 0,
    max_uses   integer,
    expires_at timestamptz,
    revoked    boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now()
);
```

Ownership is a fact on the space — `spaces.owner_id` — not a role column.
Custom roles are named permission sets per space, and members hold role ids:

```sql
CREATE TABLE space_roles (
    space_id     uuid NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    role_id      uuid NOT NULL,
    name         text NOT NULL,
    permissions  bigint NOT NULL DEFAULT 0,
    position     integer NOT NULL DEFAULT 0,
    PRIMARY KEY (space_id, role_id)
);

CREATE TABLE space_bans (
    space_id   uuid NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    banned_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (space_id, user_id)
);

CREATE TABLE channel_overwrites (
    channel_id uuid NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
    space_id   uuid NOT NULL,
    role_id    uuid NOT NULL,
    allow      bigint NOT NULL DEFAULT 0,
    deny       bigint NOT NULL DEFAULT 0,
    PRIMARY KEY (channel_id, role_id)
);
ALTER TABLE channel_overwrites ADD FOREIGN KEY (space_id, role_id)
    REFERENCES space_roles (space_id, role_id) ON DELETE CASCADE;
```

A row in `channel_overwrites` follows both its channel and its role, so
deleting either leaves no orphan. A ban keeps a member's seat closed: they are
out of the space, but redeeming an invite again is refused until unban. A
timeout sits on the member row and expires by itself.

### 3.9 Chat tier: channels and conversations

Tables: `channels`, `channel_members`, `direct_pairs`.

The chat tier draws text, voice, thread, direct and group as kinds of one
thing; a thread has a parent, a conversation has no space:

```sql
CREATE TABLE channels (
    id              uuid PRIMARY KEY,
    kind            text NOT NULL CHECK (kind IN ('text', 'voice', 'thread', 'direct', 'group')),
    space_id        uuid REFERENCES spaces (id) ON DELETE CASCADE,
    name            text NOT NULL DEFAULT '',
    topic           text NOT NULL DEFAULT '',
    category_id     uuid REFERENCES categories (id) ON DELETE SET NULL,
    position        integer NOT NULL DEFAULT 0,
    parent_id       uuid REFERENCES channels (id) ON DELETE CASCADE,
    starter_id      uuid,
    archived        boolean NOT NULL DEFAULT false,
    last_message_id uuid,
    message_count   integer NOT NULL DEFAULT 0,
    created_by      uuid NOT NULL REFERENCES users (id),
    created_at      timestamptz NOT NULL DEFAULT now(),
    CHECK ((kind IN ('direct', 'group')) = (space_id IS NULL)),
    CHECK ((kind = 'thread') = (parent_id IS NOT NULL))
);
-- 0002_roles.sql replaces the rank floors with masks:
ALTER TABLE channels ADD COLUMN view_mask bigint NOT NULL DEFAULT 0;
ALTER TABLE channels ADD COLUMN post_mask bigint NOT NULL DEFAULT 0;

CREATE TABLE channel_members (
    channel_id uuid NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    joined_at  timestamptz NOT NULL DEFAULT now(),
    hidden     boolean NOT NULL DEFAULT false,
    PRIMARY KEY (channel_id, user_id)
);

CREATE TABLE direct_pairs (
    user_lo    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    user_hi    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    channel_id uuid NOT NULL UNIQUE REFERENCES channels (id) ON DELETE CASCADE,
    PRIMARY KEY (user_lo, user_hi),
    CHECK (user_lo < user_hi)
);
```

`starter_id` gets its foreign key by `ALTER TABLE channels ADD CONSTRAINT
channels_starter FOREIGN KEY (starter_id) REFERENCES messages (id)` once
messages exist. A closed direct conversation stays owned but `hidden` until a
new message reopens it; at most one direct exists per pair, found by
`(lower id, higher id)`.

### 3.10 Chat tier: messages, attachments, reactions, read state, mutes

Tables: `messages`, `attachments`, `reactions`, `read_states`, `mutes`.

```sql
CREATE TABLE messages (
    id                uuid PRIMARY KEY,
    channel_id        uuid NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
    author_id         uuid NOT NULL REFERENCES users (id),
    kind              text NOT NULL DEFAULT 'text'
                      CHECK (kind IN ('text', 'call', 'joined', 'thread_started')),
    detail            jsonb,
    body              text NOT NULL DEFAULT '',
    reply_to          uuid,
    mentions          uuid[] NOT NULL DEFAULT '{}',
    mentions_everyone boolean NOT NULL DEFAULT false,
    thread_id         uuid REFERENCES channels (id) ON DELETE SET NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),
    edited_at         timestamptz,
    search            tsvector GENERATED ALWAYS AS (to_tsvector('simple', body)) STORED
);
CREATE INDEX messages_channel ON messages (channel_id, id);
CREATE INDEX messages_search ON messages USING gin (search);
CREATE INDEX messages_mentions ON messages USING gin (mentions);

CREATE TABLE attachments (
    id           uuid PRIMARY KEY,
    uploader_id  uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    message_id   uuid REFERENCES messages (id) ON DELETE CASCADE,
    purpose      text NOT NULL DEFAULT 'message' CHECK (purpose IN ('message', 'avatar', 'icon')),
    object_key   text NOT NULL UNIQUE,
    name         text NOT NULL,
    size         bigint NOT NULL,
    content_type text NOT NULL,
    width        integer,
    height       integer,
    uploaded     boolean NOT NULL DEFAULT false,
    position     integer NOT NULL DEFAULT 0,
    created_at   timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE reactions (
    message_id uuid NOT NULL REFERENCES messages (id) ON DELETE CASCADE,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    emoji      text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (message_id, user_id, emoji)
);

CREATE TABLE read_states (
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    channel_id uuid NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
    last_read  uuid,
    mentions   integer NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, channel_id)
);

CREATE TABLE mutes (
    user_id   uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    target_id uuid NOT NULL,
    PRIMARY KEY (user_id, target_id)
);
```

Differences from Hearth worth knowing: the chat tier grows a thread as its own
channel (`thread_id`), not a `thread_root_id` on the reply; attachments are
uploaded before the message claims them, and an unclaimed upload is swept;
`mutes.target_id` carries no foreign key, so the migrations alone cannot
constrain whether it points at a space or a channel.

### 3.11 Chat tier: calls and live connections

Tables: `call_rooms`, `rings`, `connections`, `voice_states`.

```sql
CREATE TABLE call_rooms (
    id         uuid PRIMARY KEY,
    channel_id uuid NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
    started_by uuid NOT NULL REFERENCES users (id),
    started_at timestamptz NOT NULL DEFAULT now(),
    ended_at   timestamptz,
    message_id uuid REFERENCES messages (id) ON DELETE SET NULL,
    participants uuid[] NOT NULL DEFAULT '{}'
);
CREATE UNIQUE INDEX call_rooms_live ON call_rooms (channel_id) WHERE ended_at IS NULL;

CREATE TABLE rings (
    room_id    uuid PRIMARY KEY REFERENCES call_rooms (id) ON DELETE CASCADE,
    channel_id uuid NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
    started_by uuid NOT NULL REFERENCES users (id),
    started_at timestamptz NOT NULL DEFAULT now(),
    ringing    uuid[] NOT NULL DEFAULT '{}'
);

CREATE TABLE connections (
    id           uuid PRIMARY KEY,
    user_id      uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    device_id    uuid REFERENCES devices (id) ON DELETE SET NULL,
    instance     text NOT NULL,
    idle         boolean NOT NULL DEFAULT false,
    connected_at timestamptz NOT NULL DEFAULT now(),
    heartbeat_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE voice_states (
    user_id       uuid PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    channel_id    uuid NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
    room_id       uuid NOT NULL REFERENCES call_rooms (id) ON DELETE CASCADE,
    connection_id uuid NOT NULL REFERENCES connections (id) ON DELETE CASCADE,
    device_id     uuid NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
    muted         boolean NOT NULL DEFAULT false,
    deafened      boolean NOT NULL DEFAULT false,
    camera        boolean NOT NULL DEFAULT false,
    screen        boolean NOT NULL DEFAULT false,
    joined_at     timestamptz NOT NULL DEFAULT now()
);
```

A call instance is a relay room with its own end-to-end key: a new one starts
when someone joins a channel with nobody in it, and ends when the last person
leaves — the unique partial index says at most one live room per channel. A
server instance heartbeats its own `connections` rows; rows whose heartbeat
stops are swept, so a dead instance takes its people offline within a minute.
`voice_states` keys on `user_id`: one call per person, and a connection that
goes away takes its person out of the call.

### 3.12 What lives in Redis, not Postgres

Ephemeral by nature; writing it to disk would be churn for data that is wrong
the moment the process restarts.

| Key | Value | TTL |
| --- | --- | --- |
| `sess:{token}` | serialized `Principal` | session TTL (sliding) |
| `user:{id}:sessions` | set of tokens | — (pruned on revoke) |
| `typing:{channel_id}:{user_id}` | `1` | 6s |
| voice room state | *(existing `voice-coord`)* | — |

Presence is no longer Redis-only: the chat tier backs it in the schema with
`status_presence` on `users` plus live `connections` rows, and Hearth with
`voice_presence` (§3.6). There is no "set yourself invisible" in either
schema, so there is no column for it.

## 4. Roles — as the migrations leave them

Two schemes, one per tier.

**Hearth** keeps the three ranks: `space_members.role` is CHECK-constrained to
`owner`, `moderator`, `member` (§3.2), plus an instance-level `is_instance_admin`
boolean on `users`. No capability bitfields, no per-channel overrides —
`moderation_actions` records the acts moderators take.

| | member | moderator | owner |
| --- | --- | --- | --- |
| read, post, join rooms | ✓ | ✓ | ✓ |
| mute-in-rooms, remove member | | ✓ | ✓ |
| create/rename/archive channels | | ✓ | ✓ |
| promote/demote moderators | | | ✓ |
| delete the space, transfer ownership | | | ✓ |

**The chat tier replaced the ranks with named roles.** `0001_init.sql`
started from a per-member rank (member, admin, owner) and per-channel
rank floors (view_role, post_role); `0002_roles.sql` replaces both, back
to front: it creates `space_roles` (a role is a `role_id`, a displayable
`name`, a `permissions` bigint bitmask, a `position`), gives each space that
had admins one backfilled `Admin` role whose id is the space's own, sets each
member's `role_ids`, then drops the rank column and swaps the channel floors
for `view_mask`/`post_mask`. `0003_everyone.sql` adds the space-wide baseline
`everyone` mask every member holds whatever their roles say. `0005` adds
per-role `allow`/`deny` overwrites per channel. Ownership is not a role here
at all: it is `spaces.owner_id`, a column no migration ever moves into a role.

Role names in the schema as it stands: `owner`, `moderator`, `member` for
Hearth; `Admin` for the chat tier only as a backfilled named role — the old
chat ranks `admin` and `owner` as a `space_members.role` value exist in no
final migration (0002 drops that column).

The migration comments are the definition of the backfill's meaning: the
`Admin` role's permissions are the single INT64_MIN bit (the admin tag, which
expands to everything), and an old channel floor of anything but `member`
becomes the admin-tag mask — "the old owner-only floor (which could only mean
nobody but the owner) reads as the same admin lock", since the space's owner
holds everything anyway.

These become `Role<Moderator>`-style marker types on the Hearth server fns;
the chat tier checks masks in `chat-model::perms` before its db calls
(`chat-db/src/lib.rs`'s own doc says authorization "is not here").

## 5. Authentication — the model

Two layers, kept apart:

- **Credential** — proves who you are, once. Password (argon2id), OIDC, or a
  one-time link. Rows in `credentials` (Hearth) or `users.password_hash` plus
  `email_codes` (chat tier).
- **Session** — carries that proof afterwards. `server_kit::Sessions`:
  32 bytes of CSPRNG, opaque, stored in Redis with a TTL. **Not a JWT** — a
  self-hosted admin needs "log this person out now" to actually work, and a
  stateless token cannot offer that without a revocation list, which is a
  session store wearing a disguise. The chat tier adds `users.auth_epoch` so
  revocation-by-epoch works across sessions minted under an older epoch, and
  `devices` gives the "your sessions" screen its labels.

Transport differs by client and the framework handles the branch: web gets an
httpOnly `Secure` `SameSite=Lax` cookie (the secret never reaches JS, so XSS
cannot read it), native announces `x-idealyst-client: native` and gets a
bearer token for the OS keystore via the `credentials` SDK. One `login` serves
both.

The principal we put in the session is deliberately small — it is read on
every request:

```rust
#[derive(Serialize, Deserialize, Clone)]
pub struct Principal {
    pub user_id: Uuid,
    pub handle: String,
    pub is_instance_admin: bool,
}
```

Space roles are **not** in the principal. They change while a session is live
(someone gets promoted, or removed from a space), and a session that cached
them would keep enforcing stale permissions until it expired. Hearth reads
them per request from `space_members`, a primary-key lookup; the chat tier
resolves `role_ids` against `space_roles` the same way.

## 6. Password handling

argon2id, `m=19456 KiB, t=2, p=1` (the OWASP-recommended floor), PHC-string
encoded so the parameters travel with the hash and can be raised later without
a migration — on next successful login, if the stored params are below current
policy, re-hash transparently. Hearth stores the hash in `credentials.secret`;
the chat tier in `users.password_hash`.

Login is constant-time in the "does this user exist" sense: if the handle is
unknown we still verify against a dummy hash before returning the same error
(`chat-db` exports `dummy_password_check` for exactly this), so response
timing doesn't enumerate accounts. The error is always "invalid handle or
password", never which half was wrong.

Rate limits, declared on the endpoints:

| endpoint | limit | keyed by |
| --- | --- | --- |
| `login` | 5/min, 20/hour | handle + IP |
| `request_password_reset` | 3/hour | email |
| `redeem_invite` | 10/hour | IP |
| `send_message` | 30/min | user |
| `create_space` | 3/hour | user |

## 7. Authentication flows

### 7.1 First run — instance bootstrap

The flow self-hosted apps most often get wrong, usually by shipping
`admin/admin` or by leaving registration open until someone claims the
instance.

On boot, if `users` is empty, the server mints a single-use claim token (32
CSPRNG bytes, 15-minute TTL, held in Redis) and prints it:

```
Hearth is running, and has no accounts yet.
Claim this instance:  http://localhost:8080/claim/3f9a…c21b
This link works once and expires in 15 minutes. Restart to issue a new one.
```

Visiting it creates the first account as `is_instance_admin`. For automated
deploys, `HEARTH_BOOTSTRAP_TOKEN` sets the token instead of generating one.

No default credentials exist at any point, and the window is 15 minutes rather
than "until someone notices".

### 7.2 Registration

An instance setting, defaulting to the conservative option — the
`registration_mode` CHECK on `instance_settings` names exactly these three:

- `invite_only` **(default)** — an account can only be created by redeeming a
  valid invite code. This matches the design, whose join dialog leads with
  "Join with a link".
- `open` — anyone can register. Requires email verification.
- `closed` — no new accounts; an admin creates them.

### 7.3 Sign up by invite

```
client                          server
  │  GET /j/quiet-porch
  ├──────────────────────────────▶  look up invites.code
  │                                 reject: unknown | expired | revoked
  │                                 | use_count >= max_uses
  │  ◀── space name, icon, inviter ─┤   (enough to show "Join The Cabin?")
  │
  │  register(code, handle, display_name, password, email?)
  ├──────────────────────────────▶  ── BEGIN ──
  │                                  insert users, credentials(password)
  │                                  insert space_members (Hearth: role='member')
  │                                  UPDATE invites SET use_count = use_count + 1
  │                                    WHERE code = $1 AND use_count < max_uses
  │                                    AND revoked_at IS NULL AND expires_at > now()
  │                                  ── COMMIT ──   (0 rows updated ⇒ roll back)
  │  ◀── session ticket ────────────┤
```

The conditional `UPDATE … WHERE use_count < max_uses` inside the transaction is
what makes "only work once" true under concurrency. Checking first and updating
after would let two simultaneous redemptions both pass.

An already-signed-in user redeeming a code just gets `space_members` inserted.
(The chat tier's equivalent uses `uses`/`max_uses` and a `revoked` boolean —
same transaction shape.)

### 7.4 Sign in

`login(handle_or_email, password)` → verify argon2id → `sessions.start(&principal)`
→ cookie or bearer per client kind → `auth_events` row either way.

### 7.5 Session lifetime and revocation

30-day TTL, sliding on use. `GET sessions` lists the user's live sessions with
device label, IP and last-seen. `revoke(session_id)` or `revoke_all_others()`
deletes from Redis, so the next request from that device 401s immediately;
the chat tier bumps `users.auth_epoch` as well, so a session minted under an
older epoch is refused.

Changing a password revokes every session except the current one.

### 7.6 Password reset — and the no-SMTP case

Self-hosted instances often have no working mail. Email is **optional on the
account** (`users.email` is nullable on Hearth; the chat tier's `email` is
NOT NULL but `email_verified_at` is not, and its reset codes live in
`email_codes` with purpose `reset`), and reset has two paths:

- **With email**: standard single-use token, 1-hour TTL, in Redis. The response
  is identical whether or not the address exists — no enumeration. Sent via the
  `email` SDK. The chat tier stores the code hashed (`code_hash`) and counts
  `attempts` on the row.
- **Without email**: an instance admin issues a one-time reset link from the
  admin surface and hands it over out of band. Same token mechanism, different
  delivery.

An instance with no SMTP configured hides the "email me a link" affordance
rather than showing a button that silently fails.

### 7.7 OIDC (optional)

Off by default. When configured, standard authorization-code + PKCE; on
callback we look up `credentials(kind='oidc', issuer, subject)` and either sign
in the linked account or create one. An instance can require OIDC
exclusively, in which case password credentials are refused at login even if
rows exist.

Linking OIDC to an existing account requires being signed in already —
otherwise "sign in with X" on an unverified email is an account-takeover.

### 7.8 Two-factor (designed now, shipped later)

`credentials(kind='totp')` holds the secret; the schema supports it today so
enabling it is not a migration. There is no recovery-codes table in any
migration, so recovery codes are a later, additive change, not a column set
we ship.

## 8. Live protocol

**One socket per client**, not one per channel — a user in six channels should
not hold six sockets.

```rust
#[channel]
async fn stream(mut s: Socket<ClientEvent, ServerEvent>, me: Auth<Principal>)
    -> Result<(), ServerError>
```

Auth is resolved at upgrade by the same middleware chain as every other
endpoint, so an unauthenticated socket never opens.

```rust
enum ClientEvent {
    Resume { cursors: Vec<(ChannelId, Option<MessageId>)> },
    Typing { channel: ChannelId },
    MarkRead { channel: ChannelId, message: MessageId },
    Heartbeat,
}

enum ServerEvent {
    Ready { user: UserSummary, spaces: Vec<SpaceSummary> },
    MessageCreated(Message),
    MessageEdited { id: MessageId, body: String, edited_at: Timestamp },
    MessageDeleted { id: MessageId, channel: ChannelId },
    TypingStarted { channel: ChannelId, user: UserId },
    PresenceChanged { user: UserId, status: Presence },
    VoiceStateChanged { channel: ChannelId, participants: Vec<UserId> },
    MemberChanged { space: SpaceId, user: UserId, role: Option<Role> },
    ChannelChanged(ChannelSummary),
    ReadStateChanged { channel: ChannelId, last_read: MessageId },
}
```

Fan-out uses the `pubsub` SDK over topics `space:{id}` and `channel:{id}`, so
an event produced on one instance reaches a client connected to another. That
is the SDK's headline use case and it is why this scales past one process
without us writing anything.

**Gap recovery.** A dropped socket must not silently lose messages. On
reconnect the client sends `Resume` with its last seen message id per channel;
the server replies with everything after each cursor (capped, with a
`truncated` flag that tells the client to refetch that channel cold). Because
message ids are UUIDv7 and therefore ordered, "everything after" is one indexed
range scan — no per-connection sequence numbers to maintain.

### Voice presence is a lease, not a socket

Who is in a voice room lives in `voice_presence` (Hearth, §3.6), keyed on
`user_id` so being in two calls at once is not a state the database can hold.
A client renews its row every 15 s over the live socket (`Action::StillHere`),
and a row older than `PRESENCE_TTL_SECS` is not presence — it is a ghost.

The chat tier stores the same facts differently: `connections` rows heartbeated
per instance, plus a `voice_states` row per person tied to a live
`connection_id` — a connection that goes away takes its person out of the
call. Same lease idea, different tables.

The subtlety is what happens when the **live socket** closes, which is a
different socket from the one carrying the call. It used to end the call
outright, on the reasoning that a closed tab says nothing on its way out. But
nothing at that moment can tell a closed tab from a tunnel that blinked, and
answering wrongly is loud: everyone in the room hears a leave tone and then a
join tone, and the person jumps to the bottom of the sidebar. On a tunnelled
connection that happened every few seconds.

So a closing socket **stops vouching** instead of evicting — `voice_fading`
shortens the lease to `PRESENCE_GRACE_SECS` rather than deleting the row — and
three things finish the job:

- the client re-asserts presence on its first snapshot after reconnecting, so a
  blip costs nothing and, because a heartbeat for a room you are already in is
  news to nobody, publishes nothing;
- a sweeper (`srv::sweep_presence`) reaps expired leases every 5 s and publishes
  one `VoiceLeft` for each, so a genuinely closed tab disappears for everyone
  rather than only for whoever reconnects next;
- `voice_fading` never *lengthens* a lease, so disconnecting cannot buy someone
  a fresh grace period.

The heartbeat's 15 s is safe against background-tab timer throttling because
Chrome exempts pages capturing or playing audio, which every page in a call is
doing by definition.

## 9. Where chat meets the existing voice tier

`voice-server` is a separate binary that must not grow a dependency on the chat
database, and clients must not be able to join a voice room they have no right
to be in. A short-lived capability token bridges the two:

1. Client calls `join_voice(channel_id)` on the API.
2. API checks membership for the channel's space, then mints
   `HMAC-SHA256(instance_secret, user_id | channel_id | exp)` with a 60-second
   expiry, and returns it with the assigned voice node's address (from the
   existing director).
3. Client connects to the voice node presenting the token.
4. `voice-server` verifies the HMAC with the shared secret — **no database
   call, no chat dependency** — and admits the connection.

The chat tier stays the only thing that knows about membership; the voice tier
stays the only thing that knows about audio. The token is the entire contract.

### A relay room outlives a socket, not an id

The node keeps a broadcast channel per room and, until recently, a set of the
participant ids in it — dropping the whole room when that set emptied. The set
was the wrong unit, and a refresh is what exposes it.

A participant id is derived from an account, so it is stable across a reload.
The browser opens the new socket before the node has finished noticing the old
one is gone, which means one id is legitimately claimed **twice** for a moment.
A set cannot hold that. When the stale claims drained, the last of them emptied
the set and deleted the room — with both live sockets still in it.

Nothing looked wrong at that instant: the two who were already there still
shared their channel and could still hear each other. The damage landed on the
*next* attach, which found no room, built a fresh one under the same name with
a fresh channel, and was inaudible in both directions. One name, two rooms.

It presents as one person refreshing and coming back to a call where everybody
has gone quiet, while their own microphone still reports as sending — and it is
invisible to any test that joins fresh and leaves, which is every voice suite
except the one that refreshes mid-call.

The unit is a socket. `ids` is `id -> live socket count`: a stale socket
releases its own claim, a duplicate is still reported, and the room goes only
when nothing is attached to it at all.

## 10. Client model, and what changes in the app

Today `crates/hearth/src/model.rs` is a presentation model over the design's
fixture — display-name keys, string timestamps, positional message identity.
The seam it does provide is that views read through accessors rather than the
tables, so this is where the real model lands.

The changes that reach the views:

- `Person` keyed by `UserId`, not display name — which also fixes the current
  bug where renaming someone would re-hash their avatar tint.
- `time: String` becomes a real `Timestamp`, formatted at render.
- `Line.key` (`room:index`) becomes the message's own id, which makes the keyed
  `for` loops correct rather than incidentally correct.

**Optimistic send.** The client generates the message UUIDv7, renders it
immediately, and sends. Because the id is the idempotency key, a retry after a
dropped connection cannot double-post. A failed send marks the existing row
rather than inserting a second one.

Whether to adopt the `sync` SDK for offline history is a judgement call I'd
defer: its outbox and merge policy are exactly right for queued sends, but it
assumes single-writer-per-device and is not real-time, so it composes with
`#[channel]` rather than replacing it. For v1 a local queue plus the live socket
is less machinery, and the SDK is a clean upgrade if offline turns out to
matter.

## 11. Suggested build order

Each step ends somewhere demonstrable, and nothing is written twice.

1. **Schema + migrations**, sqlx. Nothing else.
2. **Auth end to end** — bootstrap, register-by-invite, login, logout, session
   revocation. Verifiable with `curl` before any UI moves.
3. **Read paths** — spaces, channels, message pages — wired behind the existing
   accessors in `model.rs`. The UI should light up with real data and no view
   changes.
4. **Write paths** — send, edit, delete, mark-read, with optimistic ids.
5. **The live socket** — `#[channel]`, fan-out, `Resume`. Two browsers talking.
6. **Membership and moderation** — invites, roles, timeouts, removal.
7. **Voice capability token**, joining the existing tier to real identity.

Steps 1–2 are the ones worth reviewing hardest; everything after is mechanical
by comparison.

## 12. Open questions

**12.1 — End-to-end encryption for DMs.** The biggest fork. The design says
direct messages "stay between the two of you", which reads as a *policy*
promise, but if it is meant as a *cryptographic* one then the server cannot
index DM bodies, server-side search over DMs is gone, and multi-device requires
key exchange and per-device identity — a different project, not a later ticket.
**This model assumes transport encryption and server-side storage, not E2EE** —
and the chat schema is built that way: `messages.search` is a server-side
tsvector over the cleartext `body`. Please confirm, or say if E2EE is a goal.

**12.2 — Federation.** Assumed out of scope: accounts are instance-scoped and a
space belongs to one instance. Worth stating explicitly because it is
effectively irreversible.

**12.3 — SQLite for small instances.** Postgres is already in
`docker-compose.yml`, so I've assumed it — and the migrations lean hard on
Postgres: `GENERATED ALWAYS AS … STORED` tsvector, `gin` indexes, foreign-key
constraint syntax. Supporting SQLite too would mean a second schema, not a
flag.

**12.4 — Two Postgres clients.** `voice-coord` uses `tokio-postgres`; the chat
and Hearth tiers use `sqlx`. Live with the duplication, or converge one?

**12.5 — Retention.** Does an instance need message retention limits (age or
count) for disk, or is history kept forever until someone deletes it? No
migration implements a limit; chat attachments speak of an unclaimed upload
being "swept", nothing does for messages.

**12.6 — Handles.** Immutable `@handle` plus a mutable display name, as above?
Or display name only, Discord-style, with a discriminator? Both tiers ship the
former shape for now; neither has a discriminator column.

---

## 13. Implementation notes

Written while building, and correcting the plan where it turned out to be
wrong.

**Steps 1, 2 and most of 4 are done.** `crates/hearth-db` holds the schema and
migrations; `crates/hearth/src/api` holds the `#[server]` functions for auth,
spaces, invites, messages, threads and read state; `crates/hearth/src/bin/server.rs`
serves them beside the wasm bundle. The chat tier's schema and role plumbing
live in `crates/chat-db` with the rules in `chat-model::perms`. Verified by 41
database tests and 51 HTTP assertions (`tools/auth-flows.sh`,
`tools/message-flows.sh`).

**Queries are runtime-checked, not `query!`-checked.** §1 argued for sqlx
partly on compile-time checking. Using it needs either a live database at
build time or a committed `.sqlx` regenerated on every query edit, and
requiring Postgres to `cargo build` is a poor trade for a project whose pitch
is easy self-hosting. The integration suite covers it against a real Postgres
instead, and catches what a macro cannot — constraint behaviour, transaction
semantics, the invite race.

**Step 3 is bigger than this document implied.** It claimed the client would
"light up with real data and no view changes". That is not true: `model.rs`
keys people by display name, holds `&'static str` throughout, and gives
messages positional identity. Moving to server data changes those types, and
every view that reads through the accessors sees the change. It is a real
refactor of the client, not a wiring step, and it should be planned as one.

**Two bugs worth recording**, both found by driving the real thing rather than
by unit tests:

- The handle-shape constraint rejected its own tombstone, so account deletion
  was impossible. The rule now applies to live accounts only.
- Case-insensitive login silently didn't work. sqlx binds `&str` as `text`, so
  `citext = $1` resolves to a case-*sensitive* comparison — while the unique
  index still folded case on write. `ANA` could not register while `ana`
  existed, and could not sign in either. Any test that only checked uniqueness
  would have passed.

A third, caught by the HTTP harness: the first account was created, then
promoted to instance admin, and its session was built from the pre-promotion
value — so the first admin was an admin in the database and a plain member in
their own session until they signed out and back in.

## Source

Read from: `0001_init.sql`, `0002_roles.sql`, `0003_everyone.sql`,
`0004_bans.sql`, `0005_channel_overwrites.sql` (under `crates/chat-db/migrations/`),
and `0001_initial.sql`, `0002_voice_presence.sql` (under
`crates/hearth-db/migrations/`).
