//! Channels: a space's text and voice channels, threads, and direct and
//! group conversations.

use chat_model::{Channel, ChannelKind, Id, Overwrite};
use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;

use crate::{kind_of, kind_text, millis, new_id, Db, Error, Result};

/// The most people in a group conversation.
pub const GROUP_MAX: usize = 10;

/// Everything a new channel starts with.
#[derive(Debug, Clone)]
pub struct ChannelInit {
    pub kind: ChannelKind,
    pub space: Option<Id>,
    pub name: String,
    pub topic: String,
    pub category: Option<Id>,
    pub parent: Option<Id>,
    pub starter: Option<Id>,
    pub view: u64,
    pub post: u64,
    pub created_by: Id,
}

#[derive(sqlx::FromRow)]
pub(crate) struct ChannelRow {
    id: Id,
    kind: String,
    space_id: Option<Id>,
    name: String,
    topic: String,
    category_id: Option<Id>,
    position: i32,
    parent_id: Option<Id>,
    starter_id: Option<Id>,
    view_mask: i64,
    post_mask: i64,
    archived: bool,
    last_message_id: Option<Id>,
    message_count: i32,
    created_by: Id,
    created_at: OffsetDateTime,
    members: Vec<Id>,
    overwrite_roles: Vec<Id>,
    overwrite_allows: Vec<i64>,
    overwrite_denies: Vec<i64>,
}

impl From<ChannelRow> for Channel {
    fn from(r: ChannelRow) -> Channel {
        Channel {
            id: r.id,
            kind: kind_of(&r.kind),
            space: r.space_id,
            name: r.name,
            topic: r.topic,
            category: r.category_id,
            position: r.position,
            parent: r.parent_id,
            starter: r.starter_id,
            view: r.view_mask as u64,
            post: r.post_mask as u64,
            members: r.members,
            archived: r.archived,
            last_message: r.last_message_id,
            message_count: r.message_count.max(0) as u32,
            created_by: r.created_by,
            created_at: millis(r.created_at),
            overwrites: r.overwrite_roles.iter()
                .zip(r.overwrite_allows.iter())
                .zip(r.overwrite_denies.iter())
                .map(|((role, allow), deny)| (
                    *role,
                    Overwrite {
                        allow: *allow as u64,
                        deny: *deny as u64,
                    },
                ))
                .collect(),
        }
    }
}

/// A channel's columns, with its conversation members aggregated in (empty
/// for space channels). `c` is the channel's alias.
pub(crate) const CHANNEL_SELECT: &str = "SELECT c.id, c.kind, c.space_id, c.name, c.topic, c.category_id, c.position, \
     c.parent_id, c.starter_id, c.view_mask, c.post_mask, c.archived, c.last_message_id, c.message_count, \
     c.created_by, c.created_at, \
     coalesce((SELECT array_agg(m.user_id ORDER BY m.joined_at) FROM channel_members m WHERE m.channel_id = c.id), \
              '{}') AS members, \
     coalesce((SELECT array_agg(o.role_id ORDER BY o.role_id) FROM channel_overwrites o WHERE o.channel_id = c.id), \
              '{}') AS overwrite_roles, \
     coalesce((SELECT array_agg(o.allow ORDER BY o.role_id) FROM channel_overwrites o WHERE o.channel_id = c.id), \
              '{}') AS overwrite_allows, \
     coalesce((SELECT array_agg(o.deny ORDER BY o.role_id) FROM channel_overwrites o WHERE o.channel_id = c.id), \
              '{}') AS overwrite_denies \
     FROM channels c";

pub(crate) async fn insert_channel(tx: &mut Transaction<'_, Postgres>, init: &ChannelInit) -> Result<Id> {
    let id = new_id();
    let position: i32 = sqlx::query_scalar(
        "SELECT coalesce(max(position) + 1, 0) FROM channels \
         WHERE space_id IS NOT DISTINCT FROM $1 AND category_id IS NOT DISTINCT FROM $2 AND parent_id IS NULL",
    )
    .bind(init.space)
    .bind(init.category)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO channels (id, kind, space_id, name, topic, category_id, position, parent_id, starter_id, \
                               view_mask, post_mask, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(id)
    .bind(kind_text(init.kind))
    .bind(init.space)
    .bind(&init.name)
    .bind(&init.topic)
    .bind(init.category)
    .bind(position)
    .bind(init.parent)
    .bind(init.starter)
    .bind(init.view as i64)
    .bind(init.post as i64)
    .bind(init.created_by)
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

/// Who a channel's events go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Audience {
    /// A space channel: it rides the space's shared topic when open to every
    /// member who can see it, and its own topic when gated. `rules` is the
    /// channel whose masks and overwrites decide (the parent's, for a
    /// thread); `everyone` is the space's baseline permission set.
    Space { space: Id, rules: Channel, everyone: u64 },
    /// The people in a conversation.
    People(Vec<Id>),
}

impl Db {
    pub async fn channel(&self, id: Id) -> Result<Channel> {
        let row: Option<ChannelRow> = sqlx::query_as(&format!("{CHANNEL_SELECT} WHERE c.id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(Channel::from).ok_or(Error::NotFound)
    }

    /// Every channel and thread in a space.
    pub async fn space_channels(&self, space: Id) -> Result<Vec<Channel>> {
        let rows: Vec<ChannelRow> = sqlx::query_as(&format!("{CHANNEL_SELECT} WHERE c.space_id = $1"))
            .bind(space)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(Channel::from).collect())
    }

    /// `user`'s direct and group conversations, but those they closed.
    pub async fn conversations_of(&self, user: Id) -> Result<Vec<Channel>> {
        let rows: Vec<ChannelRow> = sqlx::query_as(&format!(
            "{CHANNEL_SELECT} JOIN channel_members cm ON cm.channel_id = c.id AND cm.user_id = $1 AND NOT cm.hidden"
        ))
        .bind(user)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Channel::from).collect())
    }

    /// Who should hear about a channel: the rules channel (the parent's for
    /// a thread) and the space's baseline, so the topic decision reads the
    /// same gates the viewer's access does.
    pub async fn audience(&self, channel: &Channel) -> Result<Audience> {
        if channel.kind.is_private() {
            return Ok(Audience::People(channel.members.clone()));
        }
        let space = channel.space.ok_or(Error::NotFound)?;
        let rules = match channel.parent {
            Some(parent) => self.channel(parent).await?,
            None => channel.clone(),
        };
        let s = self.space(space).await?;
        Ok(Audience::Space { space, rules, everyone: s.everyone })
    }

    pub async fn create_channel(&self, init: &ChannelInit) -> Result<Channel> {
        let mut tx = self.pool.begin().await?;
        let id = insert_channel(&mut tx, init).await?;
        tx.commit().await?;
        self.channel(id).await
    }

    /// Restate one role's allow/deny overwrite on a channel: the pair rewrites
    /// the row, and `(0, 0)` means no overwrite, so it deletes the row.
    pub async fn set_overwrite(&self, channel: Id, space: Id, role: Id, allow: u64, deny: u64) -> Result<()> {
        if allow == 0 && deny == 0 {
            sqlx::query("DELETE FROM channel_overwrites WHERE channel_id = $1 AND role_id = $2")
                .bind(channel)
                .bind(role)
                .execute(&self.pool)
                .await?;
        } else {
            sqlx::query(
                "INSERT INTO channel_overwrites (channel_id, space_id, role_id, allow, deny) \
                 VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT (channel_id, role_id) DO UPDATE SET allow = $4, deny = $5",
            )
                .bind(channel)
                .bind(space)
                .bind(role)
                .bind(allow as i64)
                .bind(deny as i64)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    /// Drop one role's overwrite on a channel, whatever it held.
    pub async fn clear_overwrite(&self, channel: Id, role: Id) -> Result<()> {
        sqlx::query("DELETE FROM channel_overwrites WHERE channel_id = $1 AND role_id = $2")
            .bind(channel)
            .bind(role)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Change a space channel's settings. Every field is set as given.
    pub async fn update_channel(
        &self,
        id: Id,
        name: &str,
        topic: &str,
        category: Option<Id>,
        view: u64,
        post: u64,
    ) -> Result<Channel> {
        sqlx::query(
            "UPDATE channels SET name = $2, topic = $3, category_id = $4, view_mask = $5, post_mask = $6 WHERE id = $1",
        )
        .bind(id)
        .bind(name)
        .bind(topic)
        .bind(category)
        .bind(view as i64)
        .bind(post as i64)
        .execute(&self.pool)
        .await?;
        self.channel(id).await
    }

    pub async fn rename_channel(&self, id: Id, name: &str) -> Result<Channel> {
        sqlx::query("UPDATE channels SET name = $2 WHERE id = $1").bind(id).bind(name).execute(&self.pool).await?;
        self.channel(id).await
    }

    pub async fn delete_channel(&self, id: Id) -> Result<()> {
        sqlx::query("DELETE FROM channels WHERE id = $1").bind(id).execute(&self.pool).await?;
        Ok(())
    }

    /// Place channels: each `(channel, category, position)` as given.
    pub async fn order_channels(&self, space: Id, places: &[(Id, Option<Id>, i32)]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (channel, category, position) in places {
            sqlx::query("UPDATE channels SET category_id = $3, position = $4 WHERE id = $2 AND space_id = $1")
                .bind(space)
                .bind(channel)
                .bind(category)
                .bind(position)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Start a thread in `parent`, optionally from one of its messages.
    pub async fn create_thread(&self, parent: &Channel, name: &str, starter: Option<Id>, by: Id) -> Result<Channel> {
        let mut tx = self.pool.begin().await?;
        if let Some(m) = starter {
            let taken: Option<Option<Id>> =
                sqlx::query_scalar("SELECT thread_id FROM messages WHERE id = $1 AND channel_id = $2 FOR UPDATE")
                    .bind(m)
                    .bind(parent.id)
                    .fetch_optional(&mut *tx)
                    .await?;
            match taken {
                None => return Err(Error::NotFound),
                Some(Some(_)) => return Err(Error::Refused("That message already has a thread.".into())),
                Some(None) => {}
            }
        }
        let id = insert_channel(
            &mut tx,
            &ChannelInit {
                kind: ChannelKind::Thread,
                space: parent.space,
                name: name.to_string(),
                topic: String::new(),
                category: None,
                parent: Some(parent.id),
                starter,
                view: parent.view,
                post: parent.post,
                created_by: by,
            },
        )
        .await?;
        if let Some(m) = starter {
            sqlx::query("UPDATE messages SET thread_id = $2 WHERE id = $1").bind(m).bind(id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        self.channel(id).await
    }

    pub async fn set_archived(&self, thread: Id, archived: bool) -> Result<Channel> {
        sqlx::query("UPDATE channels SET archived = $2 WHERE id = $1 AND kind = 'thread'")
            .bind(thread)
            .bind(archived)
            .execute(&self.pool)
            .await?;
        self.channel(thread).await
    }

    /// The direct conversation between two people, made if it does not
    /// exist, and shown again to `opener` if they had closed it.
    pub async fn open_direct(&self, opener: Id, other: Id) -> Result<(Channel, bool)> {
        if opener == other {
            return Err(Error::Refused("That's you.".into()));
        }
        let (lo, hi) = if opener < other { (opener, other) } else { (other, opener) };
        let mut tx = self.pool.begin().await?;
        let existing: Option<Id> =
            sqlx::query_scalar("SELECT channel_id FROM direct_pairs WHERE user_lo = $1 AND user_hi = $2 FOR UPDATE")
                .bind(lo)
                .bind(hi)
                .fetch_optional(&mut *tx)
                .await?;
        let (id, created) = match existing {
            Some(id) => (id, false),
            None => {
                let id = insert_channel(
                    &mut tx,
                    &ChannelInit {
                        kind: ChannelKind::Direct,
                        space: None,
                        name: String::new(),
                        topic: String::new(),
                        category: None,
                        parent: None,
                        starter: None,
                        view: 0,
                        post: 0,
                        created_by: opener,
                    },
                )
                .await?;
                sqlx::query("INSERT INTO direct_pairs (user_lo, user_hi, channel_id) VALUES ($1, $2, $3)")
                    .bind(lo)
                    .bind(hi)
                    .bind(id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| crate::taken(e, "direct_pairs_pkey", "That conversation"))?;
                // The other side does not see it until there is a message.
                for (user, hidden) in [(opener, false), (other, true)] {
                    sqlx::query("INSERT INTO channel_members (channel_id, user_id, hidden) VALUES ($1, $2, $3)")
                        .bind(id)
                        .bind(user)
                        .bind(hidden)
                        .execute(&mut *tx)
                        .await?;
                }
                (id, true)
            }
        };
        sqlx::query("UPDATE channel_members SET hidden = false WHERE channel_id = $1 AND user_id = $2")
            .bind(id)
            .bind(opener)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok((self.channel(id).await?, created))
    }

    /// A group conversation of `owner` and `others`.
    pub async fn create_group(&self, owner: Id, others: &[Id], name: &str) -> Result<Channel> {
        let mut people = vec![owner];
        for o in others {
            if !people.contains(o) {
                people.push(*o);
            }
        }
        if people.len() < 2 {
            return Err(Error::Refused("A group needs someone besides you.".into()));
        }
        if people.len() > GROUP_MAX {
            return Err(Error::Refused(format!("A group holds at most {GROUP_MAX} people.")));
        }
        let mut tx = self.pool.begin().await?;
        let id = insert_channel(
            &mut tx,
            &ChannelInit {
                kind: ChannelKind::Group,
                space: None,
                name: name.to_string(),
                topic: String::new(),
                category: None,
                parent: None,
                starter: None,
                view: 0,
                post: 0,
                created_by: owner,
            },
        )
        .await?;
        for user in &people {
            sqlx::query("INSERT INTO channel_members (channel_id, user_id) VALUES ($1, $2)")
                .bind(id)
                .bind(user)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        self.channel(id).await
    }

    pub async fn add_to_group(&self, group: Id, user: Id) -> Result<Channel> {
        let mut tx = self.pool.begin().await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_members WHERE channel_id = $1")
            .bind(group)
            .fetch_one(&mut *tx)
            .await?;
        if count as usize >= GROUP_MAX {
            return Err(Error::Refused(format!("A group holds at most {GROUP_MAX} people.")));
        }
        sqlx::query("INSERT INTO channel_members (channel_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(group)
            .bind(user)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.channel(group).await
    }

    /// Leave a group. The last one out deletes it.
    pub async fn leave_group(&self, group: Id, user: Id) -> Result<Option<Channel>> {
        sqlx::query("DELETE FROM channel_members WHERE channel_id = $1 AND user_id = $2")
            .bind(group)
            .bind(user)
            .execute(&self.pool)
            .await?;
        let channel = self.channel(group).await?;
        if channel.members.is_empty() {
            self.delete_channel(group).await?;
            return Ok(None);
        }
        Ok(Some(channel))
    }

    /// Take a conversation out of `user`'s list until a new message arrives.
    pub async fn hide_conversation(&self, channel: Id, user: Id) -> Result<()> {
        sqlx::query("UPDATE channel_members SET hidden = true WHERE channel_id = $1 AND user_id = $2")
            .bind(channel)
            .bind(user)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Show a conversation to everyone in it again; returns who had hidden it.
    pub async fn unhide_conversation(&self, channel: Id) -> Result<Vec<Id>> {
        Ok(sqlx::query_scalar(
            "UPDATE channel_members SET hidden = false WHERE channel_id = $1 AND hidden RETURNING user_id",
        )
        .bind(channel)
        .fetch_all(&self.pool)
        .await?)
    }
}