//! How people relate: friend requests, friendships, blocks.

use chat_model::{Id, NotificationSetting, Level, Relation, Scope};

use crate::{relation_of, relation_text, Db, Error, Result};

/// A relation change, from each side: what the actor now has, and what the
/// other person now has (`None`: nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelationChange {
    pub mine: Option<Relation>,
    pub theirs: Option<Relation>,
}

impl Db {
    /// How `user` relates to `other`.
    pub async fn relation(&self, user: Id, other: Id) -> Result<Option<Relation>> {
        let r: Option<String> = sqlx::query_scalar("SELECT kind FROM relations WHERE user_id = $1 AND other_id = $2")
            .bind(user)
            .bind(other)
            .fetch_optional(&self.pool)
            .await?;
        Ok(r.as_deref().map(relation_of))
    }

    pub async fn relations_of(&self, user: Id) -> Result<Vec<(Id, Relation)>> {
        let rows: Vec<(Id, String)> = sqlx::query_as("SELECT other_id, kind FROM relations WHERE user_id = $1")
            .bind(user)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(o, k)| (o, relation_of(&k))).collect())
    }

    async fn put(&self, tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, a: Id, b: Id, r: Option<Relation>) -> Result<()> {
        match r {
            Some(r) => {
                sqlx::query(
                    "INSERT INTO relations (user_id, other_id, kind) VALUES ($1, $2, $3) \
                     ON CONFLICT (user_id, other_id) DO UPDATE SET kind = EXCLUDED.kind, created_at = now()",
                )
                .bind(a)
                .bind(b)
                .bind(relation_text(r))
                .execute(&mut **tx)
                .await?;
            }
            None => {
                sqlx::query("DELETE FROM relations WHERE user_id = $1 AND other_id = $2")
                    .bind(a)
                    .bind(b)
                    .execute(&mut **tx)
                    .await?;
            }
        }
        Ok(())
    }

    /// Ask `other` to be friends — or, if they already asked, accept.
    /// Refused silently (as if sent) when they have blocked the asker, so a
    /// block is not revealed.
    pub async fn befriend(&self, user: Id, other: Id) -> Result<RelationChange> {
        if user == other {
            return Err(Error::Refused("You can't add yourself.".into()));
        }
        let mut tx = self.pool.begin().await?;
        let mine: Option<String> =
            sqlx::query_scalar("SELECT kind FROM relations WHERE user_id = $1 AND other_id = $2 FOR UPDATE")
                .bind(user)
                .bind(other)
                .fetch_optional(&mut *tx)
                .await?;
        let theirs: Option<String> =
            sqlx::query_scalar("SELECT kind FROM relations WHERE user_id = $1 AND other_id = $2 FOR UPDATE")
                .bind(other)
                .bind(user)
                .fetch_optional(&mut *tx)
                .await?;
        let change = match (mine.as_deref().map(relation_of), theirs.as_deref().map(relation_of)) {
            (Some(Relation::Friend), _) => RelationChange { mine: Some(Relation::Friend), theirs: Some(Relation::Friend) },
            (_, Some(Relation::Blocked)) => {
                self.put(&mut tx, user, other, Some(Relation::Outgoing)).await?;
                RelationChange { mine: Some(Relation::Outgoing), theirs: Some(Relation::Blocked) }
            }
            (Some(Relation::Incoming), _) => {
                self.put(&mut tx, user, other, Some(Relation::Friend)).await?;
                self.put(&mut tx, other, user, Some(Relation::Friend)).await?;
                RelationChange { mine: Some(Relation::Friend), theirs: Some(Relation::Friend) }
            }
            _ => {
                self.put(&mut tx, user, other, Some(Relation::Outgoing)).await?;
                self.put(&mut tx, other, user, Some(Relation::Incoming)).await?;
                RelationChange { mine: Some(Relation::Outgoing), theirs: Some(Relation::Incoming) }
            }
        };
        tx.commit().await?;
        Ok(change)
    }

    /// Remove a friend, decline a request or cancel one sent. A block the
    /// other side holds stays.
    pub async fn unfriend(&self, user: Id, other: Id) -> Result<RelationChange> {
        let mut tx = self.pool.begin().await?;
        let theirs: Option<String> =
            sqlx::query_scalar("SELECT kind FROM relations WHERE user_id = $1 AND other_id = $2 FOR UPDATE")
                .bind(other)
                .bind(user)
                .fetch_optional(&mut *tx)
                .await?;
        let theirs = theirs.as_deref().map(relation_of);
        self.put(&mut tx, user, other, None).await?;
        let theirs = if theirs == Some(Relation::Blocked) {
            theirs
        } else {
            self.put(&mut tx, other, user, None).await?;
            None
        };
        tx.commit().await?;
        Ok(RelationChange { mine: None, theirs })
    }

    /// Block `other`: any friendship or request ends, from both sides, and
    /// the block stays on the blocker's.
    pub async fn block(&self, user: Id, other: Id) -> Result<RelationChange> {
        if user == other {
            return Err(Error::Refused("You can't block yourself.".into()));
        }
        let mut tx = self.pool.begin().await?;
        let theirs: Option<String> =
            sqlx::query_scalar("SELECT kind FROM relations WHERE user_id = $1 AND other_id = $2 FOR UPDATE")
                .bind(other)
                .bind(user)
                .fetch_optional(&mut *tx)
                .await?;
        let theirs = theirs.as_deref().map(relation_of);
        self.put(&mut tx, user, other, Some(Relation::Blocked)).await?;
        let theirs = if theirs == Some(Relation::Blocked) {
            theirs
        } else {
            self.put(&mut tx, other, user, None).await?;
            None
        };
        tx.commit().await?;
        Ok(RelationChange { mine: Some(Relation::Blocked), theirs })
    }

    /// Whether either has blocked the other — which stops direct messages.
    pub async fn blocked_between(&self, a: Id, b: Id) -> Result<bool> {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM relations WHERE kind = 'blocked' \
             AND ((user_id = $1 AND other_id = $2) OR (user_id = $2 AND other_id = $1))",
        )
        .bind(a)
        .bind(b)
        .fetch_one(&self.pool)
        .await?;
        Ok(n > 0)
    }

    /// Mute or unmute a space or channel for `user`.
    pub async fn set_muted(&self, user: Id, target: Id, muted: bool) -> Result<()> {
        if muted {
            sqlx::query("INSERT INTO mutes (user_id, target_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        } else {
            sqlx::query("DELETE FROM mutes WHERE user_id = $1 AND target_id = $2")
        }
        .bind(user)
        .bind(target)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mutes_of(&self, user: Id) -> Result<Vec<Id>> {
        Ok(sqlx::query_scalar("SELECT target_id FROM mutes WHERE user_id = $1")
            .bind(user)
            .fetch_all(&self.pool)
            .await?)
    }

    /// The person's notification settings: all of them, or those of one
    /// space — its own row and its channels' rows.
    pub async fn notification_settings(&self, user: Id, space: Option<Id>) -> Result<Vec<NotificationSetting>> {
        const SELECT: &str = "SELECT scope, target_id, level, muted_until FROM notification_settings WHERE user_id = $1";
        let rows: Vec<(String, Id, Option<String>, Option<i64>)> = match space {
            None => sqlx::query_as(&format!("{SELECT} ORDER BY scope, target_id"))
                .bind(user)
                .fetch_all(&self.pool)
                .await?,
            Some(space) => sqlx::query_as(&format!(
                "{SELECT} AND (target_id = $2 OR target_id IN (SELECT id FROM channels WHERE space_id = $2)) \
                 ORDER BY scope, target_id",
            ))
            .bind(user)
            .bind(space)
            .fetch_all(&self.pool)
            .await?,
        };
        Ok(rows
            .into_iter()
            .filter_map(|(scope, target, level, until)| {
                Some(NotificationSetting {
                    scope: Scope::from_db(&scope)?,
                    target,
                    level: level.as_deref().and_then(Level::from_db),
                    muted_until: until,
                })
            })
            .collect())
    }

    /// Store (or, with no level and no mute, clear) one of the person's
    /// settings.
    pub async fn set_notification_setting(&self, user: Id, setting: &NotificationSetting) -> Result<()> {
        if setting.level.is_none() && setting.muted_until.is_none() {
            sqlx::query("DELETE FROM notification_settings WHERE user_id = $1 AND target_id = $2")
                .bind(user)
                .bind(setting.target)
                .execute(&self.pool)
                .await?;
        } else {
            sqlx::query(
                "INSERT INTO notification_settings (user_id, scope, target_id, level, muted_until) \
                 VALUES ($1, $2, $3, $4, $5) ON CONFLICT (user_id, target_id) \
                 DO UPDATE SET scope = EXCLUDED.scope, level = EXCLUDED.level, muted_until = EXCLUDED.muted_until",
            )
            .bind(user)
            .bind(setting.scope.db_str())
            .bind(setting.target)
            .bind(setting.level.map(Level::db_str))
            .bind(setting.muted_until)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }
}
