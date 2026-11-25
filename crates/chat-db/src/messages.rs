//! Messages: posting, history, edits, reactions, read markers, search, and
//! the uploads messages carry.

use std::collections::{BTreeMap, HashMap};

use chat_model::timeline::Page;
use chat_model::{Attachment, CallRecord, Id, Message, MessageKind, Reaction, ReplyTo, ThreadLink};
use time::OffsetDateTime;

use crate::{millis, new_id, opt_millis, Db, Error, Result};

/// What posting needs, already validated and with mentions resolved.
#[derive(Debug, Clone)]
pub struct NewMessage {
    pub channel: Id,
    pub author: Id,
    pub kind: MessageKind,
    pub body: String,
    pub reply_to: Option<Id>,
    pub mentions: Vec<Id>,
    pub everyone: bool,
    /// Roles the body named, already honoured at the sender.
    pub mention_roles: Vec<Id>,
    /// Uploads by the author to attach, in order.
    pub attachments: Vec<Id>,
    /// A space sticker sent in place of a body; the API checks it belongs
    /// to the channel's space.
    pub sticker: Option<Id>,
}

/// A posted message, and whose unread mention counts it raised.
#[derive(Debug, Clone)]
pub struct Posted {
    pub message: Message,
    pub notified: Vec<Id>,
}

/// One search result.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub message: Message,
}

/// An upload slot: where the object goes, before a message claims it.
#[derive(Debug, Clone, PartialEq)]
pub struct Upload {
    pub id: Id,
    pub object_key: String,
    pub name: String,
    pub size: u64,
    pub content_type: String,
    pub uploaded: bool,
}

/// How long the start of a replied-to message is, in characters.
const EXCERPT: usize = 120;

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: Id,
    channel_id: Id,
    author_id: Id,
    kind: String,
    detail: Option<serde_json::Value>,
    body: String,
    reply_to: Option<Id>,
    mentions: Vec<Id>,
    mentions_everyone: bool,
    sticker: Option<Id>,
    mention_roles: Vec<Id>,
    thread_id: Option<Id>,
    created_at: OffsetDateTime,
    edited_at: Option<OffsetDateTime>,
}

const MESSAGE_COLS: &str = "id, channel_id, author_id, kind, detail, body, reply_to, mentions, mentions_everyone, \
                            sticker, mention_roles, thread_id, created_at, edited_at";

fn kind_parts(kind: &MessageKind) -> (&'static str, Option<serde_json::Value>) {
    match kind {
        MessageKind::Text => ("text", None),
        MessageKind::Call(record) => ("call", serde_json::to_value(record).ok()),
        MessageKind::Joined => ("joined", None),
        MessageKind::ThreadStarted { thread } => ("thread_started", Some(serde_json::json!({ "thread": thread }))),
    }
}

fn kind_from(kind: &str, detail: Option<serde_json::Value>) -> MessageKind {
    match kind {
        "call" => detail
            .and_then(|d| serde_json::from_value::<CallRecord>(d).ok())
            .map_or(MessageKind::Text, MessageKind::Call),
        "joined" => MessageKind::Joined,
        "thread_started" => detail
            .and_then(|d| d.get("thread").and_then(|t| serde_json::from_value::<Id>(t.clone()).ok()))
            .map_or(MessageKind::Text, |thread| MessageKind::ThreadStarted { thread }),
        _ => MessageKind::Text,
    }
}

/// Where a stored object is fetched from, relative to the server's origin.
pub fn file_url(object_key: &str) -> String {
    format!("/f/{object_key}")
}

impl Db {
    /// Turn rows into messages, with their attachments, reactions, reply
    /// quotes and thread summaries, in the rows' order.
    async fn hydrate(&self, rows: Vec<MessageRow>) -> Result<Vec<Message>> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<Id> = rows.iter().map(|r| r.id).collect();
        let mut attachments: HashMap<Id, Vec<Attachment>> = HashMap::new();
        let att: Vec<(Id, Id, String, i64, String, String, Option<i32>, Option<i32>)> = sqlx::query_as(
            "SELECT message_id, id, name, size, content_type, object_key, width, height FROM attachments \
             WHERE message_id = ANY($1) ORDER BY position",
        )
        .bind(&ids)
        .fetch_all(&self.pool)
        .await?;
        for (message, id, name, size, content_type, key, width, height) in att {
            attachments.entry(message).or_default().push(Attachment {
                id,
                name,
                size: size.max(0) as u64,
                content_type,
                url: file_url(&key),
                width: width.map(|w| w as u32),
                height: height.map(|h| h as u32),
            });
        }
        let mut reactions: HashMap<Id, Vec<Reaction>> = HashMap::new();
        let rx: Vec<(Id, String, Vec<Id>)> = sqlx::query_as(
            "SELECT message_id, emoji, array_agg(user_id ORDER BY created_at) FROM reactions \
             WHERE message_id = ANY($1) GROUP BY message_id, emoji ORDER BY min(created_at)",
        )
        .bind(&ids)
        .fetch_all(&self.pool)
        .await?;
        for (message, emoji, users) in rx {
            reactions.entry(message).or_default().push(Reaction { emoji, users });
        }
        let reply_ids: Vec<Id> = rows.iter().filter_map(|r| r.reply_to).collect();
        let replies: HashMap<Id, (Id, String)> = if reply_ids.is_empty() {
            HashMap::new()
        } else {
            sqlx::query_as::<_, (Id, Id, String)>("SELECT id, author_id, body FROM messages WHERE id = ANY($1)")
                .bind(&reply_ids)
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .map(|(id, author, body)| (id, (author, body)))
                .collect()
        };
        let thread_ids: Vec<Id> = rows.iter().filter_map(|r| r.thread_id).collect();
        let threads: HashMap<Id, ThreadLink> = if thread_ids.is_empty() {
            HashMap::new()
        } else {
            let t: Vec<(Id, String, i32, Option<Id>, Option<OffsetDateTime>)> = sqlx::query_as(
                "SELECT c.id, c.name, c.message_count, c.last_message_id, \
                        (SELECT m.created_at FROM messages m WHERE m.id = c.last_message_id) \
                 FROM channels c WHERE c.id = ANY($1)",
            )
            .bind(&thread_ids)
            .fetch_all(&self.pool)
            .await?;
            t.into_iter()
                .map(|(id, name, count, last, at)| {
                    (id, ThreadLink { id, name, message_count: count.max(0) as u32, last_message: last, last_at: opt_millis(at) })
                })
                .collect()
        };
        Ok(rows
            .into_iter()
            .map(|r| Message {
                id: r.id,
                channel: r.channel_id,
                author: r.author_id,
                kind: kind_from(&r.kind, r.detail),
                attachments: attachments.remove(&r.id).unwrap_or_default(),
                reactions: reactions.remove(&r.id).unwrap_or_default(),
                reply_to: r.reply_to.map(|id| match replies.get(&id) {
                    Some((author, body)) => ReplyTo { id, author: *author, excerpt: body.chars().take(EXCERPT).collect() },
                    // Deleted since: the reply keeps its place, without a quote.
                    None => ReplyTo { id, author: Id::nil(), excerpt: String::new() },
                }),
                thread: r.thread_id.and_then(|t| threads.get(&t).cloned()),
                body: r.body,
                mentions: r.mentions,
                mentions_everyone: r.mentions_everyone,
                sticker: r.sticker,
                mention_roles: r.mention_roles,
                at: millis(r.created_at),
                edited_at: opt_millis(r.edited_at),
            })
            .collect())
    }

    pub async fn message(&self, id: Id) -> Result<Message> {
        let rows: Vec<MessageRow> = sqlx::query_as(&format!("SELECT {MESSAGE_COLS} FROM messages WHERE id = $1"))
            .bind(id)
            .fetch_all(&self.pool)
            .await?;
        self.hydrate(rows).await?.pop().ok_or(Error::NotFound)
    }

    /// Post a message. In one transaction: the row, its attachments, the
    /// channel's newest-message marker and count, the author's read marker,
    /// and the unread-mention count of everyone it notifies — the people it
    /// names (`@everyone`: every member who can see the channel), or
    /// everyone else in a direct or group conversation.
    pub async fn post(&self, new: &NewMessage) -> Result<Posted> {
        let mut tx = self.pool.begin().await?;
        let id = crate::new_id();
        let (kind, detail) = kind_parts(&new.kind);
        sqlx::query(
            "INSERT INTO messages (id, channel_id, author_id, kind, detail, body, reply_to, mentions, mentions_everyone, \
             sticker, mention_roles) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(id)
        .bind(new.channel)
        .bind(new.author)
        .bind(kind)
        .bind(detail)
        .bind(&new.body)
        .bind(new.reply_to)
        .bind(&new.mentions)
        .bind(new.everyone)
        .bind(new.sticker)
        .bind(&new.mention_roles)
        .execute(&mut *tx)
        .await?;
        for (position, upload) in new.attachments.iter().enumerate() {
            let claimed = sqlx::query(
                "UPDATE attachments SET message_id = $1, position = $4 \
                 WHERE id = $2 AND uploader_id = $3 AND message_id IS NULL AND uploaded AND purpose = 'message'",
            )
            .bind(id)
            .bind(upload)
            .bind(new.author)
            .bind(position as i32)
            .execute(&mut *tx)
            .await?;
            if claimed.rows_affected() == 0 {
                return Err(Error::Refused("An attachment wasn't uploaded, or is already used.".into()));
            }
        }
        let (kind, space, parent): (String, Option<Id>, Option<Id>) = sqlx::query_as(
            "UPDATE channels SET last_message_id = $2, message_count = message_count + 1 WHERE id = $1 \
             RETURNING kind, space_id, parent_id",
        )
        .bind(new.channel)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO read_states (user_id, channel_id, last_read, mentions) VALUES ($1, $2, $3, 0) \
             ON CONFLICT (user_id, channel_id) DO UPDATE SET last_read = EXCLUDED.last_read, mentions = 0",
        )
        .bind(new.author)
        .bind(new.channel)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        let notified: Vec<Id> = if kind == "direct" || kind == "group" {
            sqlx::query_scalar("SELECT user_id FROM channel_members WHERE channel_id = $1 AND user_id <> $2")
                .bind(new.channel)
                .bind(new.author)
                .fetch_all(&mut *tx)
                .await?
        } else if new.everyone {
            // Everyone who can see it: a thread is seen as its parent is.
            let view: i64 = sqlx::query_scalar("SELECT view_mask FROM channels WHERE id = $1")
                .bind(parent.unwrap_or(new.channel))
                .fetch_one(&mut *tx)
                .await?;
            // The members whose roles together cover the channel's view mask,
            // each member's permissions being the union of the roles they hold.
            let role_rows: Vec<(Id, i64)> = sqlx::query_as(
                "SELECT role_id, permissions FROM space_roles WHERE space_id = $1",
            )
            .bind(space)
            .fetch_all(&mut *tx)
            .await?;
            let role_bits: HashMap<Id, i64> = role_rows.into_iter().collect();
            let holders: Vec<(Id, bool, Vec<Id>)> = sqlx::query_as(
                "SELECT user_id, user_id = (SELECT owner_id FROM spaces WHERE id = $1), role_ids \
                 FROM space_members WHERE space_id = $1 AND user_id <> $2",
            )
            .bind(space)
            .bind(new.author)
            .fetch_all(&mut *tx)
            .await?;
            holders.iter()
                .filter(|(_, owner, role_ids)|
                    *owner || role_ids.iter().fold(0i64, |acc, role| acc | role_bits.get(role).copied().unwrap_or(0)) & view == view)
                .map(|(user, _, _)| *user)
                .collect::<Vec<Id>>()
        } else {
            // Named people, if they are in the space.
            let named: Vec<Id> = sqlx::query_scalar(
                "SELECT user_id FROM space_members WHERE space_id = $1 AND user_id = ANY($2) AND user_id <> $3",
            )
            .bind(space)
            .bind(&new.mentions)
            .bind(new.author)
            .fetch_all(&mut *tx)
            .await?
            ;
            // Holders of a honoured role, if the message named one: a role
            // mention reaches whoever holds the role when they read it.
            let holders: Vec<Id> = if new.mention_roles.is_empty() {
                Vec::new()
            } else {
                sqlx::query_scalar(
                    "SELECT user_id FROM space_members WHERE space_id = $1 AND user_id <> $2 \
                     AND $3::uuid[] && role_ids",
                )
                .bind(space)
                .bind(new.author)
                .bind(&new.mention_roles)
                .fetch_all(&mut *tx)
                .await?
            };
            let mut all: Vec<Id> = named.iter().chain(holders.iter()).map(|u| *u).collect();
            all.sort();
            all.dedup();
            all
        };
        if !notified.is_empty() {
            sqlx::query(
                "INSERT INTO read_states (user_id, channel_id, last_read, mentions) \
                 SELECT u, $2, NULL, 1 FROM unnest($1::uuid[]) AS u \
                 ON CONFLICT (user_id, channel_id) DO UPDATE SET mentions = read_states.mentions + 1",
            )
            .bind(&notified)
            .bind(new.channel)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(Posted { message: self.message(id).await?, notified })
    }

    /// A page of history: the newest `limit` before `before` (or the newest
    /// of all), or the oldest `limit` after `after`.
    pub async fn page(&self, channel: Id, before: Option<Id>, after: Option<Id>, limit: u32) -> Result<Page> {
        let limit = limit.clamp(1, 100) as i64;
        let (mut rows, forward): (Vec<MessageRow>, bool) = match after {
            Some(after) => (
                sqlx::query_as(&format!(
                    "SELECT {MESSAGE_COLS} FROM messages WHERE channel_id = $1 AND id > $2 ORDER BY id LIMIT $3"
                ))
                .bind(channel)
                .bind(after)
                .bind(limit + 1)
                .fetch_all(&self.pool)
                .await?,
                true,
            ),
            None => (
                sqlx::query_as(&format!(
                    "SELECT {MESSAGE_COLS} FROM messages WHERE channel_id = $1 AND ($2::uuid IS NULL OR id < $2) \
                     ORDER BY id DESC LIMIT $3"
                ))
                .bind(channel)
                .bind(before)
                .bind(limit + 1)
                .fetch_all(&self.pool)
                .await?,
                false,
            ),
        };
        // The extra row says whether there is more beyond the page.
        let more = rows.len() as i64 > limit;
        rows.truncate(limit as usize);
        if !forward {
            rows.reverse();
        }
        let messages = self.hydrate(rows).await?;
        Ok(if forward {
            Page { messages, older: true, present: !more }
        } else {
            Page { messages, older: more, present: before.is_none() || self.is_newest_after(channel, before).await? }
        })
    }

    async fn is_newest_after(&self, channel: Id, before: Option<Id>) -> Result<bool> {
        let Some(before) = before else { return Ok(true) };
        let newer: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM messages WHERE channel_id = $1 AND id >= $2 LIMIT 1")
                .bind(channel)
                .bind(before)
                .fetch_optional(&self.pool)
                .await?;
        Ok(newer.is_none())
    }

    /// History around one message — for jumping to a search result or a
    /// reply's original: about `limit` on each side.
    pub async fn around(&self, channel: Id, message: Id, limit: u32) -> Result<Page> {
        let mut before = self.page(channel, Some(message), None, limit).await?;
        let after = self.page(channel, None, Some(message), limit).await?;
        if let Ok(m) = self.message(message).await {
            before.messages.push(m);
        }
        before.messages.extend(after.messages);
        Ok(Page { messages: before.messages, older: before.older, present: after.present })
    }

    pub async fn edit_message(&self, id: Id, body: &str, mentions: &[Id], everyone: bool, mention_roles: &[Id]) -> Result<Message> {
        sqlx::query(
            "UPDATE messages SET body = $2, mentions = $3, mentions_everyone = $4, mention_roles = $5, edited_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(body)
        .bind(mentions)
        .bind(everyone)
        .bind(mention_roles)
        .execute(&self.pool)
        .await?;
        self.message(id).await
    }

    /// Delete a message; its attachments go with it. Returns the object keys
    /// to remove from storage.
    pub async fn delete_message(&self, id: Id) -> Result<Vec<String>> {
        let mut tx = self.pool.begin().await?;
        let keys: Vec<String> = sqlx::query_scalar("SELECT object_key FROM attachments WHERE message_id = $1")
            .bind(id)
            .fetch_all(&mut *tx)
            .await?;
        let channel: Option<Id> = sqlx::query_scalar("DELETE FROM messages WHERE id = $1 RETURNING channel_id")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        let channel = channel.ok_or(Error::NotFound)?;
        // The newest-message marker falls back to what is now newest, so a
        // deleted last message stops making the channel unread.
        sqlx::query(
            "UPDATE channels SET message_count = greatest(message_count - 1, 0), \
             last_message_id = (SELECT id FROM messages WHERE channel_id = $1 ORDER BY id DESC LIMIT 1) WHERE id = $1",
        )
        .bind(channel)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(keys)
    }

    /// Add or take back a reaction. Returns whether anything changed.
    pub async fn react(&self, message: Id, user: Id, emoji: &str, add: bool) -> Result<bool> {
        let done = if add {
            sqlx::query("INSERT INTO reactions (message_id, user_id, emoji) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
        } else {
            sqlx::query("DELETE FROM reactions WHERE message_id = $1 AND user_id = $2 AND emoji = $3")
        }
        .bind(message)
        .bind(user)
        .bind(emoji)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// How many different emoji a message has, to cap them.
    pub async fn reaction_kinds(&self, message: Id) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT count(DISTINCT emoji) FROM reactions WHERE message_id = $1")
            .bind(message)
            .fetch_one(&self.pool)
            .await?)
    }

    /// Pin a message, for `by`. Returns whether the pin was newly added.
    pub async fn pin(&self, message: Id, by: Id) -> Result<bool> {
        let channel = sqlx::query_scalar::<_, Id>("SELECT channel_id FROM messages WHERE id = $1")
            .bind(message)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)?;
        let done = sqlx::query(
            "INSERT INTO pins (message_id, channel_id, pinned_by) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(message)
        .bind(channel)
        .bind(by)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Take a pin back. Returns whether anything changed.
    pub async fn unpin(&self, message: Id) -> Result<bool> {
        Ok(sqlx::query("DELETE FROM pins WHERE message_id = $1")
            .bind(message)
            .execute(&self.pool)
            .await?
            .rows_affected()
            > 0)
    }

    /// The pinned messages of a channel, newest pin first.
    pub async fn pins_of(&self, channel: Id) -> Result<Vec<Message>> {
        let rows: Vec<MessageRow> = sqlx::query_as(&format!(
            "SELECT {MESSAGE_COLS} FROM messages WHERE channel_id = $1 AND id IN (SELECT message_id FROM pins WHERE channel_id = $1) ORDER BY (SELECT pinned_at FROM pins WHERE message_id = messages.id) DESC"
        ))
        .bind(channel)
        .fetch_all(&self.pool)
        .await?;
        self.hydrate(rows).await
    }

    /// How many pins a channel has, to cap them.
    pub async fn pin_count(&self, channel: Id) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM pins WHERE channel_id = $1")
            .bind(channel)
            .fetch_one(&self.pool)
            .await?)
    }

    /// Mark a channel read up to `last_read`. Never moves backwards; returns
    /// where the marker now is.
    pub async fn mark_read(&self, user: Id, channel: Id, last_read: Id) -> Result<Id> {
        Ok(sqlx::query_scalar(
            "INSERT INTO read_states (user_id, channel_id, last_read, mentions) VALUES ($1, $2, $3, 0) \
             ON CONFLICT (user_id, channel_id) DO UPDATE SET \
               last_read = CASE WHEN read_states.last_read IS NULL OR EXCLUDED.last_read >= read_states.last_read \
                           THEN EXCLUDED.last_read ELSE read_states.last_read END, \
               mentions = CASE WHEN read_states.last_read IS NULL OR EXCLUDED.last_read >= read_states.last_read \
                          THEN 0 ELSE read_states.mentions END \
             RETURNING last_read",
        )
        .bind(user)
        .bind(channel)
        .bind(last_read)
        .fetch_one(&self.pool)
        .await?)
    }

    /// Full-text search over the given channels, newest first.
    pub async fn search(&self, channels: &[Id], query: &str, author: Option<Id>, limit: u32) -> Result<Vec<SearchHit>> {
        if channels.is_empty() || query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let rows: Vec<MessageRow> = sqlx::query_as(&format!(
            "SELECT {MESSAGE_COLS} FROM messages WHERE channel_id = ANY($1) AND kind = 'text' \
             AND search @@ websearch_to_tsquery('simple', $2) AND ($3::uuid IS NULL OR author_id = $3) \
             ORDER BY id DESC LIMIT $4"
        ))
        .bind(channels)
        .bind(query)
        .bind(author)
        .bind(limit.clamp(1, 50) as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(self.hydrate(rows).await?.into_iter().map(|message| SearchHit { message }).collect())
    }

    /// The call record message for a room, to update when the call ends.
    pub async fn set_message_kind(&self, id: Id, kind: &MessageKind) -> Result<Message> {
        let (k, detail) = kind_parts(kind);
        sqlx::query("UPDATE messages SET kind = $2, detail = $3 WHERE id = $1")
            .bind(id)
            .bind(k)
            .bind(detail)
            .execute(&self.pool)
            .await?;
        self.message(id).await
    }

    // ---- uploads ----

    /// Reserve an upload. The object is put straight into the bucket by the
    /// client, then confirmed with [`Db::confirm_upload`].
    pub async fn create_upload(
        &self,
        uploader: Id,
        purpose: &str,
        name: &str,
        size: u64,
        content_type: &str,
    ) -> Result<Upload> {
        let id = new_id();
        // Unguessable on its own: the key is the file's capability.
        let object_key = format!("{}/{}", id.simple(), uuid::Uuid::new_v4().simple());
        sqlx::query(
            "INSERT INTO attachments (id, uploader_id, purpose, object_key, name, size, content_type) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(id)
        .bind(uploader)
        .bind(purpose)
        .bind(&object_key)
        .bind(name)
        .bind(size as i64)
        .bind(content_type)
        .execute(&self.pool)
        .await?;
        Ok(Upload { id, object_key, name: name.into(), size, content_type: content_type.into(), uploaded: false })
    }

    pub async fn upload(&self, id: Id, uploader: Id) -> Result<Upload> {
        let row: Option<(String, String, i64, String, bool)> = sqlx::query_as(
            "SELECT object_key, name, size, content_type, uploaded FROM attachments WHERE id = $1 AND uploader_id = $2",
        )
        .bind(id)
        .bind(uploader)
        .fetch_optional(&self.pool)
        .await?;
        let (object_key, name, size, content_type, uploaded) = row.ok_or(Error::NotFound)?;
        Ok(Upload { id, object_key, name, size: size.max(0) as u64, content_type, uploaded })
    }

    /// Note that an upload's object is in the bucket, at its real size and,
    /// for an image, dimensions.
    pub async fn confirm_upload(&self, id: Id, size: u64, dims: Option<(u32, u32)>) -> Result<()> {
        sqlx::query("UPDATE attachments SET uploaded = true, size = $2, width = $3, height = $4 WHERE id = $1")
            .bind(id)
            .bind(size as i64)
            .bind(dims.map(|d| d.0 as i32))
            .bind(dims.map(|d| d.1 as i32))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Whether an object key belongs to a confirmed upload — what the file
    /// route checks before redirecting to storage.
    pub async fn object_exists(&self, key: &str) -> Result<Option<(String, String)>> {
        Ok(sqlx::query_as("SELECT name, content_type FROM attachments WHERE object_key = $1 AND uploaded")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Unclaimed message uploads older than a day: their object keys, after
    /// deleting the rows. Avatars and icons are claimed on upload.
    pub async fn sweep_uploads(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "DELETE FROM attachments WHERE message_id IS NULL AND purpose = 'message' \
             AND created_at < now() - interval '1 day' RETURNING object_key",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Read markers for `user`, by channel.
    pub async fn reads_of(&self, user: Id) -> Result<BTreeMap<Id, chat_model::ReadState>> {
        let rows: Vec<(Id, Option<Id>, i32)> =
            sqlx::query_as("SELECT channel_id, last_read, mentions FROM read_states WHERE user_id = $1")
                .bind(user)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .map(|(c, last_read, mentions)| (c, chat_model::ReadState { last_read, mentions: mentions.max(0) as u32 }))
            .collect())
    }
}
