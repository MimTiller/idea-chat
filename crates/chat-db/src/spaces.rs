//! Spaces: creating them, who is in them, how their channels are grouped,
//! and invitations in.

use std::collections::BTreeMap;

use chat_model::{Category, Channel, ChannelKind, Id, Invite, InvitePreview, Member, Role, Space};
use chat_model::expressions::{Emoji, Sticker};
use rand::Rng;
use time::OffsetDateTime;

use crate::channels::ChannelInit;
use crate::{millis, new_id, opt_millis, Db, Error, Result};

/// What creating a space needs.
#[derive(Debug, Clone)]
pub struct NewSpace {
    pub name: String,
    pub owner: Id,
}

/// What redeeming an invite came to.
#[derive(Debug, Clone, PartialEq)]
pub enum InviteOutcome {
    /// Newly a member.
    Joined { space: Id, member: Member },
    /// Was already; nothing used up.
    AlreadyMember { space: Id },
}

#[derive(sqlx::FromRow)]
struct SpaceRow {
    id: Id,
    name: String,
    icon: Option<String>,
    owner_id: Id,
    everyone: i64,
    created_at: OffsetDateTime,
}

#[derive(sqlx::FromRow)]
struct InviteRow {
    code: String,
    space_id: Id,
    created_by: Id,
    uses: i32,
    max_uses: Option<i32>,
    expires_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
}

impl From<InviteRow> for Invite {
    fn from(r: InviteRow) -> Invite {
        Invite {
            code: r.code,
            space: r.space_id,
            created_by: r.created_by,
            uses: r.uses as u32,
            max_uses: r.max_uses.map(|m| m as u32),
            expires_at: opt_millis(r.expires_at),
            created_at: millis(r.created_at),
        }
    }
}

/// An invite code: ten characters from an alphabet without look-alikes
/// (no 0/O, 1/l/I), about 50 bits.
fn invite_code() -> String {
    const ALPHABET: &[u8] = b"23456789abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ";
    let mut rng = rand::thread_rng();
    (0..10).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect()
}

impl Db {
    /// Make a space with its owner, two categories and a channel in each —
    /// a text channel to talk in and a voice channel to meet in.
    pub async fn create_space(&self, new: &NewSpace) -> Result<(Space, Vec<Channel>)> {
        let mut tx = self.pool.begin().await?;
        let id = new_id();
        sqlx::query("INSERT INTO spaces (id, name, owner_id, everyone) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(&new.name)
            .bind(new.owner)
            .bind(chat_model::perms::DEFAULT_EVERYONE as i64)
            .execute(&mut *tx)
            .await?;
        let position: i32 = sqlx::query_scalar("SELECT coalesce(max(position) + 1, 0) FROM space_members WHERE user_id = $1")
            .bind(new.owner)
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO space_members (space_id, user_id, role_ids, position) VALUES ($1, $2, '{}', $3)")
            .bind(id)
            .bind(new.owner)
            .bind(position)
            .execute(&mut *tx)
            .await?;
        // A new space seeds its role ladder: Admin (the tag alone, which
        // expands to every permission) at the top, then Moderator with the
        // acts on other people and on the space that fall short of it. They
        // are ordinary roles afterwards — editable, deletable, renamable —
        // and nobody holds either until the owner hands one out.
        for (name, permissions, position, colour, mentionable) in [
            ("Admin", chat_model::perms::ADMIN, 0, Some(0xE74C3Cu32), false),
            ("Moderator", chat_model::perms::DEFAULT_MODERATOR, 1, Some(0x2ECC71u32), true),
        ] {
            sqlx::query(
                "INSERT INTO space_roles (space_id, role_id, name, permissions, position, colour, mentionable) VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
                .bind(id)
                .bind(new_id())
                .bind(name)
                .bind(permissions as i64)
                .bind(position)
                .bind(colour.map(|c| c as i64))
                .bind(mentionable)
                .execute(&mut *tx)
                .await?;
        }
        let (text_cat, voice_cat) = (new_id(), new_id());
        for (cat, name, pos) in [(text_cat, "Text channels", 0), (voice_cat, "Voice channels", 1)] {
            sqlx::query("INSERT INTO categories (id, space_id, name, position) VALUES ($1, $2, $3, $4)")
                .bind(cat)
                .bind(id)
                .bind(name)
                .bind(pos)
                .execute(&mut *tx)
                .await?;
        }
        for (kind, name, cat) in [(ChannelKind::Text, "general", text_cat), (ChannelKind::Voice, "lounge", voice_cat)] {
            crate::channels::insert_channel(
                &mut tx,
                &ChannelInit {
                    kind,
                    space: Some(id),
                    name: name.into(),
                    topic: String::new(),
                    category: Some(cat),
                    parent: None,
                    starter: None,
                    view: 0,
                    post: 0,
                    created_by: new.owner,
                },
            )
            .await?;
        }
        tx.commit().await?;
        let space = self.space(id).await?;
        let channels = self.space_channels(id).await?;
        Ok((space, channels))
    }

    /// A space, with its members and categories.
    pub async fn space(&self, id: Id) -> Result<Space> {
        let row: SpaceRow = sqlx::query_as("SELECT id, name, icon, owner_id, everyone, created_at FROM spaces WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)?;
        let members: Vec<(Id, Vec<Id>, OffsetDateTime)> =
            sqlx::query_as("SELECT user_id, role_ids, joined_at FROM space_members WHERE space_id = $1")
                .bind(id)
                .fetch_all(&self.pool)
                .await?;
        let categories: Vec<(Id, String, i32)> =
            sqlx::query_as("SELECT id, name, position FROM categories WHERE space_id = $1 ORDER BY position, name")
                .bind(id)
                .fetch_all(&self.pool)
                .await?;
        let roles: Vec<(Id, String, i64, i32, Option<i64>, bool)> =
            sqlx::query_as("SELECT role_id, name, permissions, position, colour, mentionable FROM space_roles WHERE space_id = $1")
                .bind(id)
                .fetch_all(&self.pool)
                .await?;
        let emoji: Vec<(Id, String, Id, Id, OffsetDateTime)> =
            sqlx::query_as("SELECT emoji_id, name, upload_id, created_by, created_at FROM space_emoji WHERE space_id = $1")
                .bind(id)
                .fetch_all(&self.pool)
                .await?;
        let stickers: Vec<(Id, String, String, Id, Id, OffsetDateTime)> =
            sqlx::query_as("SELECT sticker_id, name, description, upload_id, created_by, created_at FROM space_stickers WHERE space_id = $1")
                .bind(id)
                .fetch_all(&self.pool)
                .await?;
        Ok(Space {
            id: row.id,
            name: row.name,
            icon: row.icon,
            owner: row.owner_id,
            everyone: row.everyone as u64,
            members: members
                .into_iter()
                .map(|(u, rs, at)| (u, Member { roles: rs, joined_at: millis(at) }))
                .collect::<BTreeMap<_, _>>(),
            roles: roles.into_iter().map(|(role_id, name, permissions, position, colour, mentionable)|
                    (role_id, Role { id: role_id, name, permissions: permissions as u64, position, colour: colour.map(|c| c as u32), mentionable })).collect::<BTreeMap<_, _>>(),
            categories: categories.into_iter().map(|(id, name, position)| Category { id, name, position }).collect(),
            created_at: millis(row.created_at),
            emoji: emoji.into_iter().map(|(id, name, upload, created_by, at)|
                    (id, Emoji { id, name, upload, created_by, created_at: millis(at) })).collect::<BTreeMap<_, _>>(),
            stickers: stickers.into_iter().map(|(id, name, description, upload, created_by, at)|
                    (id, Sticker { id, name, description, upload, created_by, created_at: millis(at) })).collect::<BTreeMap<_, _>>(),
        })
    }

    /// The spaces `user` is in, in their switcher order.
    pub async fn spaces_of(&self, user: Id) -> Result<Vec<Id>> {
        Ok(sqlx::query_scalar("SELECT space_id FROM space_members WHERE user_id = $1 ORDER BY position, joined_at")
            .bind(user)
            .fetch_all(&self.pool)
            .await?)
    }

    /// `user`'s role ids in `space`, or `None` if they are not in it.
    pub async fn role_ids(&self, space: Id, user: Id) -> Result<Option<Vec<Id>>> {
        let r: Option<Vec<Id>> = sqlx::query_scalar("SELECT role_ids FROM space_members WHERE space_id = $1 AND user_id = $2")
            .bind(space)
            .bind(user)
            .fetch_optional(&self.pool)
            .await?;
        Ok(r)
    }

    pub async fn member_ids(&self, space: Id) -> Result<Vec<Id>> {
        Ok(sqlx::query_scalar("SELECT user_id FROM space_members WHERE space_id = $1")
            .bind(space)
            .fetch_all(&self.pool)
            .await?)
    }

    pub async fn rename_space(&self, space: Id, name: &str, icon: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE spaces SET name = $2, icon = $3 WHERE id = $1")
            .bind(space)
            .bind(name)
            .bind(icon)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete_space(&self, space: Id) -> Result<()> {
        sqlx::query("DELETE FROM spaces WHERE id = $1").bind(space).execute(&self.pool).await?;
        Ok(())
    }

    /// Take `user` out of `space`. The owner cannot leave; they transfer the
    /// space or delete it.
    pub async fn remove_member(&self, space: Id, user: Id) -> Result<()> {
        let done = sqlx::query(
            "DELETE FROM space_members WHERE space_id = $1 AND user_id = $2 AND user_id <> (SELECT owner_id FROM spaces WHERE id = $1)",
        )
            .bind(space)
            .bind(user)
            .execute(&self.pool)
            .await?;
        if done.rows_affected() == 0 {
            return Err(Error::Refused("The owner can't leave their space. Transfer it first.".into()));
        }
        // Their read markers and mutes in it go with them.
        sqlx::query(
            "DELETE FROM read_states WHERE user_id = $1 AND channel_id IN (SELECT id FROM channels WHERE space_id = $2)",
        )
        .bind(user)
        .bind(space)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Restate the space's everyone baseline: what every member holds
    /// whatever their roles say.
    pub async fn set_everyone(&self, space: Id, everyone: u64) -> Result<()> {
        sqlx::query("UPDATE spaces SET everyone = $2 WHERE id = $1")
            .bind(space)
            .bind(everyone as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Restate the role ids `user` holds. The owner holds none by choice:
    /// their control comes from ownership, so a grant never reaches them.
    pub async fn set_role_ids(&self, space: Id, user: Id, roles: &[Id]) -> Result<()> {
        sqlx::query(
            "UPDATE space_members SET role_ids = $3 WHERE space_id = $1 AND user_id = $2 AND user_id <> (SELECT owner_id FROM spaces WHERE id = $1)",
        )
            .bind(space)
            .bind(user)
            .bind(roles)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Create a space role; `permissions` is the bitset its holders get.
    pub async fn create_role(&self, space: Id, name: &str, permissions: u64, colour: Option<u32>, mentionable: bool) -> Result<Role> {
        let position: i32 = sqlx::query_scalar("SELECT coalesce(max(position) + 1, 0) FROM space_roles WHERE space_id = $1")
            .bind(space)
            .fetch_one(&self.pool)
            .await?;
        let id = new_id();
        sqlx::query(
            "INSERT INTO space_roles (space_id, role_id, name, permissions, position, colour, mentionable) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
            .bind(space)
            .bind(id)
            .bind(name)
            .bind(permissions as i64)
            .bind(position)
            .bind(colour.map(|c| c as i64))
            .bind(mentionable)
            .execute(&self.pool)
            .await?;
        Ok(Role { id, name: name.to_string(), permissions, position, colour, mentionable })
    }

    /// Rename a role and restate its permissions, in place.
    pub async fn update_role(&self, space: Id, role: Id, name: &str, permissions: u64, colour: Option<u32>, mentionable: bool) -> Result<()> {
        sqlx::query(
            "UPDATE space_roles SET name = $3, permissions = $4, colour = $5, mentionable = $6 WHERE space_id = $1 AND role_id = $2",
        )
            .bind(space)
            .bind(role)
            .bind(name)
            .bind(permissions as i64)
            .bind(colour.map(|c| c as i64))
            .bind(mentionable)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete a role: every member holding it drops it. Channel masks are
    /// permission bits, not role names, so they are untouched — a mask a
    /// deleted role used simply becomes one nobody covers.
    pub async fn delete_role(&self, space: Id, role: Id) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM space_roles WHERE space_id = $1 AND role_id = $2")
            .bind(space)
            .bind(role)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE space_members SET role_ids = array_remove(role_ids, $2::uuid) WHERE space_id = $1 AND $2::uuid = ANY (role_ids)",
        )
            .bind(space)
            .bind(role)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    // ---- custom emoji and stickers ----

    /// The space's custom emoji, as its members see them. Listing needs only
    /// membership, which the API checks before calling.
    pub async fn emoji(&self, space: Id) -> Result<Vec<Emoji>> {
        let rows: Vec<(Id, String, Id, Id, OffsetDateTime)> =
            sqlx::query_as("SELECT emoji_id, name, upload_id, created_by, created_at FROM space_emoji WHERE space_id = $1")
                .bind(space)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(id, name, upload, created_by, at)|
                Emoji { id, name, upload, created_by, created_at: millis(at) }).collect())
    }

    /// Create a custom emoji from a finished image upload. A name already in
    /// the space is refused; the count limit is the API's question, so this
    /// counts what is there.
    pub async fn create_emoji(&self, space: Id, name: &str, upload: Id, created_by: Id) -> Result<Emoji> {
        let id = new_id();
        let at: OffsetDateTime = sqlx::query_scalar(
            "INSERT INTO space_emoji (space_id, emoji_id, name, upload_id, created_by) VALUES ($1, $2, $3, $4, $5) RETURNING created_at",
        )
            .bind(space)
            .bind(id)
            .bind(name)
            .bind(upload)
            .bind(created_by)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| crate::taken(e, "space_emoji_name", "That emoji name"))?;
        Ok(Emoji { id, name: name.to_string(), upload, created_by, created_at: millis(at) })
    }

    pub async fn rename_emoji(&self, space: Id, emoji: Id, name: &str) -> Result<()> {
        sqlx::query("UPDATE space_emoji SET name = $3 WHERE space_id = $1 AND emoji_id = $2")
            .bind(space)
            .bind(emoji)
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| crate::taken(e, "space_emoji_name", "That emoji name"))?;
        Ok(())
    }

    /// Delete a custom emoji: its row, and through the FK its upload row.
    pub async fn delete_emoji(&self, space: Id, emoji: Id) -> Result<()> {
        sqlx::query("DELETE FROM space_emoji WHERE space_id = $1 AND emoji_id = $2")
            .bind(space)
            .bind(emoji)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The space's stickers, as its members see them.
    pub async fn stickers(&self, space: Id) -> Result<Vec<Sticker>> {
        let rows: Vec<(Id, String, String, Id, Id, OffsetDateTime)> =
            sqlx::query_as("SELECT sticker_id, name, description, upload_id, created_by, created_at FROM space_stickers WHERE space_id = $1")
                .bind(space)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(id, name, description, upload, created_by, at)|
                Sticker { id, name, description, upload, created_by, created_at: millis(at) }).collect())
    }

    pub async fn create_sticker(&self, space: Id, name: &str, description: &str, upload: Id, created_by: Id) -> Result<Sticker> {
        let id = new_id();
        let at: OffsetDateTime = sqlx::query_scalar(
            "INSERT INTO space_stickers (space_id, sticker_id, name, description, upload_id, created_by) VALUES ($1, $2, $3, $4, $5, $6) RETURNING created_at",
        )
            .bind(space)
            .bind(id)
            .bind(name)
            .bind(description)
            .bind(upload)
            .bind(created_by)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| crate::taken(e, "space_stickers_name", "That sticker name"))?;
        Ok(Sticker { id, name: name.to_string(), description: description.to_string(), upload, created_by, created_at: millis(at) })
    }

    pub async fn rename_sticker(&self, space: Id, sticker: Id, name: &str) -> Result<()> {
        sqlx::query("UPDATE space_stickers SET name = $3 WHERE space_id = $1 AND sticker_id = $2")
            .bind(space)
            .bind(sticker)
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| crate::taken(e, "space_stickers_name", "That sticker name"))?;
        Ok(())
    }

    /// Delete a sticker: its row, and through the FK its upload row.
    pub async fn delete_sticker(&self, space: Id, sticker: Id) -> Result<()> {
        sqlx::query("DELETE FROM space_stickers WHERE space_id = $1 AND sticker_id = $2")
            .bind(space)
            .bind(sticker)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// How many custom emoji the space holds — what the count limit checks.
    pub async fn emoji_count(&self, space: Id) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM space_emoji WHERE space_id = $1")
            .bind(space)
            .fetch_one(&self.pool)
            .await?)
    }

    pub async fn sticker_count(&self, space: Id) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM space_stickers WHERE space_id = $1")
            .bind(space)
            .fetch_one(&self.pool)
            .await?)
    }

    /// Ban `user`: record the ban and take them out of the space now. The
    /// ban is what keeps their seat closed while they are out of it.
    pub async fn ban(&self, space: Id, user: Id) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO space_bans (space_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
            .bind(space)
            .bind(user)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "DELETE FROM space_members WHERE space_id = $1 AND user_id = $2 AND user_id <> (SELECT owner_id FROM spaces WHERE id = $1)",
        )
            .bind(space)
            .bind(user)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Lift a ban. The member does not come back by itself; an invite or a
    /// space join does.
    pub async fn unban(&self, space: Id, user: Id) -> Result<()> {
        sqlx::query("DELETE FROM space_bans WHERE space_id = $1 AND user_id = $2")
            .bind(space)
            .bind(user)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Whether `user` is banned in `space`.
    pub async fn banned(&self, space: Id, user: Id) -> Result<bool> {
        let hit: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM space_bans WHERE space_id = $1 AND user_id = $2",
        )
            .bind(space)
            .bind(user)
            .fetch_optional(&self.pool)
            .await?;
        Ok(hit.is_some())
    }

    /// Whether `user` is timed out in `space` right now: their seat stays
    /// open, but their sends and speech are held back until the row passes.
    pub async fn timed_out(&self, space: Id, user: Id) -> Result<bool> {
        let hit: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM space_members WHERE space_id = $1 AND user_id = $2 AND timeout_ends > now()",
        )
            .bind(space)
            .bind(user)
            .fetch_optional(&self.pool)
            .await?;
        Ok(hit.is_some())
    }

    /// Time `user` out in `space` until `until` (a fresh `now()` plus
    /// `minutes`), or clear it with `None`.
    pub async fn set_timeout(&self, space: Id, user: Id, minutes: Option<u32>) -> Result<()> {
        match minutes {
            Some(m) => {
                sqlx::query(
                    "UPDATE space_members SET timeout_ends = $3::interval * $4 WHERE space_id = $1 AND user_id = $2",
                )
                    .bind(space)
                    .bind(user)
                    .bind("minutes")
                    .bind(m as i32)
                    .execute(&self.pool)
                    .await?;
            }
            None => {
                sqlx::query(
                    "UPDATE space_members SET timeout_ends = NULL WHERE space_id = $1 AND user_id = $2",
                )
                    .bind(space)
                    .bind(user)
                    .execute(&self.pool)
                    .await?;
            }
        }
        Ok(())
    }

    /// Make `to` the owner. The old owner keeps whatever roles they hold;
    /// ownership is the space's own fact, so it simply moves.
    pub async fn transfer_space(&self, space: Id, _from: Id, to: Id) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let is_member: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM space_members WHERE space_id = $1 AND user_id = $2")
                .bind(space)
                .bind(to)
                .fetch_optional(&mut *tx)
                .await?;
        if is_member.is_none() {
            return Err(Error::NotFound);
        }
        sqlx::query("UPDATE spaces SET owner_id = $2 WHERE id = $1").bind(space).bind(to).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Save `user`'s switcher order. Spaces not listed keep their place after.
    pub async fn set_space_order(&self, user: Id, order: &[Id]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (i, space) in order.iter().enumerate() {
            sqlx::query("UPDATE space_members SET position = $3 WHERE space_id = $1 AND user_id = $2")
                .bind(space)
                .bind(user)
                .bind(i as i32)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    // ---- categories ----

    pub async fn create_category(&self, space: Id, name: &str) -> Result<Category> {
        let position: i32 = sqlx::query_scalar("SELECT coalesce(max(position) + 1, 0) FROM categories WHERE space_id = $1")
            .bind(space)
            .fetch_one(&self.pool)
            .await?;
        let id = new_id();
        sqlx::query("INSERT INTO categories (id, space_id, name, position) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(space)
            .bind(name)
            .bind(position)
            .execute(&self.pool)
            .await?;
        Ok(Category { id, name: name.to_string(), position })
    }

    pub async fn rename_category(&self, space: Id, category: Id, name: &str) -> Result<()> {
        sqlx::query("UPDATE categories SET name = $3 WHERE id = $2 AND space_id = $1")
            .bind(space)
            .bind(category)
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete a category. Its channels move to the top of the space.
    pub async fn delete_category(&self, space: Id, category: Id) -> Result<()> {
        sqlx::query("DELETE FROM categories WHERE id = $2 AND space_id = $1")
            .bind(space)
            .bind(category)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Put a space's categories in the given order.
    pub async fn order_categories(&self, space: Id, order: &[Id]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (i, cat) in order.iter().enumerate() {
            sqlx::query("UPDATE categories SET position = $3 WHERE id = $2 AND space_id = $1")
                .bind(space)
                .bind(cat)
                .bind(i as i32)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    // ---- invites ----

    pub async fn create_invite(
        &self,
        space: Id,
        by: Id,
        max_uses: Option<u32>,
        expires_at: Option<OffsetDateTime>,
    ) -> Result<Invite> {
        let row: InviteRow = sqlx::query_as(
            "INSERT INTO invites (code, space_id, created_by, max_uses, expires_at) VALUES ($1, $2, $3, $4, $5) \
             RETURNING code, space_id, created_by, uses, max_uses, expires_at, created_at",
        )
        .bind(invite_code())
        .bind(space)
        .bind(by)
        .bind(max_uses.map(|m| m as i32))
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.into())
    }

    /// A space's invites still worth listing: not revoked, not used up, not
    /// expired.
    pub async fn invites(&self, space: Id) -> Result<Vec<Invite>> {
        let rows: Vec<InviteRow> = sqlx::query_as(
            "SELECT code, space_id, created_by, uses, max_uses, expires_at, created_at FROM invites \
             WHERE space_id = $1 AND NOT revoked AND (max_uses IS NULL OR uses < max_uses) \
             AND (expires_at IS NULL OR expires_at > now()) ORDER BY created_at DESC",
        )
        .bind(space)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Invite::from).collect())
    }

    pub async fn revoke_invite(&self, space: Id, code: &str) -> Result<()> {
        sqlx::query("UPDATE invites SET revoked = true WHERE code = $1 AND space_id = $2")
            .bind(code)
            .bind(space)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// What an invite leads to, if it still works.
    pub async fn invite_preview(&self, code: &str, viewer: Option<Id>) -> Result<InvitePreview> {
        let row: Option<(Id, String, Option<String>, Id, i64, bool)> = sqlx::query_as(
            "SELECT s.id, s.name, s.icon, i.created_by, \
                    (SELECT count(*) FROM space_members m WHERE m.space_id = s.id), \
                    EXISTS (SELECT 1 FROM space_members m WHERE m.space_id = s.id AND m.user_id = $2) \
             FROM invites i JOIN spaces s ON s.id = i.space_id \
             WHERE i.code = $1 AND NOT i.revoked AND (i.max_uses IS NULL OR i.uses < i.max_uses) \
             AND (i.expires_at IS NULL OR i.expires_at > now())",
        )
        .bind(code)
        .bind(viewer)
        .fetch_optional(&self.pool)
        .await?;
        let (space, name, icon, inviter, members, joined) = row.ok_or(Error::NotFound)?;
        Ok(InvitePreview {
            code: code.to_string(),
            space,
            space_name: name,
            space_icon: icon,
            members: members as u32,
            inviter: self.user(inviter).await.ok(),
            joined,
        })
    }

    /// Join a space by invite. Using one up is atomic: two people redeeming
    /// the last use of a one-use invite get one join between them.
    pub async fn redeem_invite(&self, code: &str, user: Id) -> Result<InviteOutcome> {
        let mut tx = self.pool.begin().await?;
        let space: Option<Id> = sqlx::query_scalar(
            "SELECT space_id FROM invites WHERE code = $1 AND NOT revoked \
             AND (max_uses IS NULL OR uses < max_uses) AND (expires_at IS NULL OR expires_at > now()) FOR UPDATE",
        )
        .bind(code)
        .fetch_optional(&mut *tx)
        .await?;
        let space = space.ok_or(Error::NotFound)?;
        let banned: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM space_bans WHERE space_id = $1 AND user_id = $2",
        )
            .bind(space)
            .bind(user)
            .fetch_one(&mut *tx)
            .await?;
        if banned > 0 {
            tx.rollback().await?;
            return Err(Error::Refused("That invite no longer works for you.".into()));
        }
        let position: i32 = sqlx::query_scalar("SELECT coalesce(max(position) + 1, 0) FROM space_members WHERE user_id = $1")
            .bind(user)
            .fetch_one(&mut *tx)
            .await?;
        let joined: Option<OffsetDateTime> = sqlx::query_scalar(
            "INSERT INTO space_members (space_id, user_id, role_ids, position) VALUES ($1, $2, '{}', $3) \
             ON CONFLICT DO NOTHING RETURNING joined_at",
        )
        .bind(space)
        .bind(user)
        .bind(position)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(joined_at) = joined else {
            return Ok(InviteOutcome::AlreadyMember { space });
        };
        sqlx::query("UPDATE invites SET uses = uses + 1 WHERE code = $1").bind(code).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(InviteOutcome::Joined { space, member: Member { roles: vec![], joined_at: millis(joined_at) } })
    }
}