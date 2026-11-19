//! Channels: a space's text and voice channels, threads, and direct and
//! group conversations.

use chat_model::{Channel, ChannelKind, Id, PermissionSet};
use server::server;
use server_kit::Auth;

use crate::{Answer, Principal};

#[cfg(feature = "server")]
pub(crate) mod body {
    use chat_model::perms::{self, Access};
    use chat_model::{Channel, ChannelKind, Id, Space};

    use crate::srv::{app, db, fail, me};
    use crate::{ApiError, Failure, Principal};

    /// A channel, the caller's access to it, and its space (if any). A
    /// channel the caller may not know about is `NotFound`.
    pub async fn open(p: &Principal, channel: Id) -> Result<(Id, Channel, Access, Option<Space>), Failure> {
        let account = me(p).await?;
        let a = app();
        let c = a.db.channel(channel).await.map_err(db)?;
        let space = match c.space {
            Some(s) => Some(a.db.space(s).await.map_err(db)?),
            None => None,
        };
        let rules = match c.kind {
            ChannelKind::Thread => {
                let parent = a.db.channel(c.parent.ok_or_else(|| fail(ApiError::NotFound))?).await.map_err(db)?;
                Channel { archived: c.archived, ..parent }
            }
            _ => c.clone(),
        };
        let access = perms::channel_access(account.id(), &rules, space.as_ref());
        if access == Access::None {
            return Err(fail(ApiError::NotFound));
        }
        Ok((account.id(), c, access, space))
    }
}

/// Make a text or voice channel in a space.
#[server]
pub async fn create_channel(
    space: Id,
    kind: ChannelKind,
    name: String,
    category: Option<Id>,
    user: Auth<Principal>,
) -> Answer<Channel> {
    use chat_db::ChannelInit;
    use chat_model::live::Event;

    use crate::srv::{app, db, forbidden, invalid, me, space_topic};
    let account = me(&user.0).await?;
    let a = app();
    if !a.db.role_ids(space, account.id()).await.map_err(db)?.is_some_and(|_| true)
        || !chat_model::perms::can(&a.db.space(space).await.map_err(db)?, account.id(), chat_model::perms::Permission::ManageChannels) {
        return Err(forbidden("Only those who manage channels can make channels."));
    }
    if !matches!(kind, ChannelKind::Text | ChannelKind::Voice) {
        return Err(invalid("kind")("Make threads from a channel.".into()));
    }
    let name = chat_model::text::channel_name(&name).map_err(invalid("name"))?;
    let channel = a
        .db
        .create_channel(&ChannelInit {
            kind,
            space: Some(space),
            name,
            topic: String::new(),
            category,
            parent: None,
            starter: None,
            view: 0,
            post: 0,
            created_by: account.id(),
        })
        .await
        .map_err(db)?;
    a.bus.publish(vec![space_topic(space)], Event::ChannelUpserted { channel: channel.clone() }).await;
    Ok(channel)
}

/// Change a channel's name, topic, category and who may see and post in it.
#[server]
pub async fn update_channel(
    channel: Id,
    name: String,
    topic: String,
    category: Option<Id>,
    view: PermissionSet,
    post: PermissionSet,
    user: Auth<Principal>,
) -> Answer<Channel> {
    use chat_model::live::Event;

    use crate::srv::{app, db, forbidden, invalid, space_topic};
    let (me, c, _, space) = body::open(&user.0, channel).await?;
    let Some(space_id) = c.space.filter(|_| c.kind != ChannelKind::Thread) else {
        return Err(forbidden("That isn't a space channel."));
    };
    if !chat_model::perms::manages(&space, me) {
        return Err(forbidden("Only admins can change channels."));
    }
    // Setting who may see and post in a channel is what ManageChannelRoles
    // buys: only when the masks actually move, a name or topic change does
    // not need it.
    if (view, post) != (c.view, c.post)
        && !chat_model::perms::can_act(
            space.as_ref(),
            me,
            chat_model::perms::Permission::ManageChannelRoles,
        )
    {
        return Err(forbidden("Only those who manage channel roles can change that."));
    }
    let name = chat_model::text::channel_name(&name).map_err(invalid("name"))?;
    let topic = chat_model::text::about(&topic).map_err(invalid("topic"))?;
    let gated = (view, post) != (c.view, c.post);
    let updated = app().db.update_channel(channel, &name, &topic, category, view, post).await.map_err(db)?;
    // Every member sees a channel exists — locked or not — so its settings
    // go to everyone; its messages go only to who may read them.
    app().bus.publish(vec![space_topic(space_id)], Event::ChannelUpserted { channel: updated.clone() }).await;
    // When the masks move, who may read can change and so can which topic the
    // channel's own events ride: recompute every member's subscriptions.
    if gated {
        let members = app().db.member_ids(space_id).await.unwrap_or_default();
        crate::srv::resync(&members).await;
    }
    Ok(updated)
}

#[server]
pub async fn delete_channel(channel: Id, user: Auth<Principal>) -> Answer<()> {
    use chat_model::live::Event;

    use crate::srv::{app, db, forbidden, space_topic};
    let (me, c, _, space) = body::open(&user.0, channel).await?;
    let Some(space_id) = c.space else { return Err(forbidden("Close a conversation instead.")) };
    let may = chat_model::perms::manages(&space, me)
        || (c.kind == ChannelKind::Thread && c.created_by == me);
    if !may {
        return Err(forbidden("Only admins can delete channels."));
    }
    app().db.delete_channel(channel).await.map_err(db)?;
    app().bus.publish(vec![space_topic(space_id)], Event::ChannelRemoved { channel }).await;
    Ok(())
}

/// Set one role's allow and deny overwrites on a channel: an ordinary space
/// channel only (a thread follows its parent, a conversation has no roles),
/// for a role below the caller's own rank, and with an allow of powers the
/// caller holds. Both zero removes the overwrite.
#[server]
pub async fn set_overwrite(
    channel: Id,
    role: Id,
    allow: PermissionSet,
    deny: PermissionSet,
    user: Auth<Principal>,
) -> Answer<()> {
    use chat_model::live::Event;
    use chat_model::perms;

    use crate::srv::{app, db, forbidden, space_topic};
    let (me, c, _, space) = body::open(&user.0, channel).await?;
    let Some(space_id) = c.space else {
        return Err(forbidden("Only space channels take per-role overwrites."));
    };
    match space.as_ref() {
        Some(space) if perms::can_set_overwrite(space, me, &c, role, allow) => {}
        _ => return Err(forbidden("Only those who manage channel roles can change that, for a role below their own and with powers they have.")),
    }
    app().db.set_overwrite(channel, space_id, role, allow, deny).await.map_err(db)?;
    // The channel restates itself, so connections fold in its new overwrites.
    let updated = app().db.channel(channel).await.map_err(db)?;
    app().bus.publish(vec![space_topic(space_id)], Event::ChannelUpserted { channel: updated }).await;
    // A gated channel may ride a different topic now: everyone re-derives
    // their subscriptions, as a mask change does.
    crate::srv::resync(&app().db.member_ids(space_id).await.unwrap_or_default()).await;
    Ok(())
}

/// Drop one role's overwrite on a channel.
#[server]
pub async fn clear_overwrite(channel: Id, role: Id, user: Auth<Principal>) -> Answer<()> {
    use chat_model::live::Event;
    use chat_model::perms;

    use crate::srv::{app, db, forbidden, space_topic};
    let (me, c, _, space) = body::open(&user.0, channel).await?;
    let Some(space_id) = c.space else {
        return Err(forbidden("Only space channels take per-role overwrites."));
    };
    match space.as_ref() {
        Some(space) if perms::can_set_overwrite(space, me, &c, role, 0) => {}
        _ => return Err(forbidden("Only those who manage channel roles can change that, for a role below their own.")),
    }
    app().db.clear_overwrite(channel, role).await.map_err(db)?;
    let updated = app().db.channel(channel).await.map_err(db)?;
    app().bus.publish(vec![space_topic(space_id)], Event::ChannelUpserted { channel: updated }).await;
    crate::srv::resync(&app().db.member_ids(space_id).await.unwrap_or_default()).await;
    Ok(())
}

/// Put a space's channels in order: each `(channel, category, position)` as given.
#[server]
pub async fn order_channels(space: Id, places: Vec<(Id, Option<Id>, i32)>, user: Auth<Principal>) -> Answer<()> {
    use chat_model::live::Event;

    use crate::srv::{app, db, forbidden, me, space_topic};
    let account = me(&user.0).await?;
    let a = app();
    if !chat_model::perms::can(&a.db.space(space).await.map_err(db)?, account.id(), chat_model::perms::Permission::ManageSpaceSettings) {
        return Err(forbidden("Only admins can arrange channels."));
    }
    a.db.order_channels(space, &places).await.map_err(db)?;
    for c in a.db.space_channels(space).await.map_err(db)? {
        if places.iter().any(|(id, _, _)| *id == c.id) {
            a.bus.publish(vec![space_topic(space)], Event::ChannelUpserted { channel: c }).await;
        }
    }
    Ok(())
}

/// Start a thread in a text channel, optionally from one of its messages.
#[server]
pub async fn create_thread(parent: Id, name: String, starter: Option<Id>, user: Auth<Principal>) -> Answer<Channel> {
    use chat_db::NewMessage;
    use chat_model::live::Event;
    use chat_model::MessageKind;

    use crate::srv::{app, channel_topic, db, forbidden, invalid, space_topic};
    let (me, c, access, _) = body::open(&user.0, parent).await?;
    if c.kind != ChannelKind::Text {
        return Err(forbidden("Threads grow from text channels."));
    }
    if !access.can_post() {
        return Err(forbidden("You can't post here."));
    }
    let name = chat_model::text::name(&name).map_err(invalid("name"))?;
    let a = app();
    let thread = a.db.create_thread(&c, &name, starter, me).await.map_err(db)?;
    let space = c.space.expect("a text channel is in a space");
    a.bus.publish(vec![space_topic(space)], Event::ChannelUpserted { channel: thread.clone() }).await;
    let topic = channel_topic(&c).await?;
    match starter {
        // The starter now shows its thread.
        Some(m) => {
            if let Ok(message) = a.db.message(m).await {
                a.bus.publish(vec![topic], Event::MessageEdited { message }).await;
            }
        }
        // A thread from nothing is announced in its channel.
        None => {
            if let Ok(posted) = a
                .db
                .post(&NewMessage {
                    channel: parent,
                    author: me,
                    kind: MessageKind::ThreadStarted { thread: thread.id },
                    body: String::new(),
                    reply_to: None,
                    mentions: vec![],
                    everyone: false,
                    attachments: vec![],
                    sticker: None,
                })
                .await
            {
                a.bus.publish(vec![topic], Event::MessageCreated { message: posted.message }).await;
            }
        }
    }
    Ok(thread)
}

/// Archive a thread (it leaves the navigation and goes read-only), or bring
/// it back. Its starter or an admin.
#[server]
pub async fn set_archived(thread: Id, archived: bool, user: Auth<Principal>) -> Answer<Channel> {
    use chat_model::live::Event;

    use crate::srv::{app, db, forbidden, space_topic};
    let (me, c, _, space) = body::open(&user.0, thread).await?;
    if c.kind != ChannelKind::Thread {
        return Err(forbidden("Only threads are archived."));
    }
    if c.created_by != me && !chat_model::perms::manages(&space, me) {
        return Err(forbidden("Only whoever started it, or an admin, can do that."));
    }
    let updated = app().db.set_archived(thread, archived).await.map_err(db)?;
    let space = c.space.expect("a thread is in a space");
    app().bus.publish(vec![space_topic(space)], Event::ChannelUpserted { channel: updated.clone() }).await;
    Ok(updated)
}

/// Rename a thread or a group conversation.
#[server]
pub async fn rename_channel(channel: Id, name: String, user: Auth<Principal>) -> Answer<Channel> {
    use chat_model::live::Event;

    use crate::srv::{app, channel_topic, db, forbidden, invalid, space_topic};
    let (me, c, access, space) = body::open(&user.0, channel).await?;
    let a = app();
    match c.kind {
        ChannelKind::Group => {
            let name = chat_model::text::name(&name).map_err(invalid("name"))?;
            let updated = a.db.rename_channel(channel, &name).await.map_err(db)?;
            a.bus.publish(vec![channel_topic(&updated).await?], Event::ChannelUpserted { channel: updated.clone() }).await;
            Ok(updated)
        }
        ChannelKind::Thread if access.can_post() && (c.created_by == me || chat_model::perms::manages(&space, me)) => {
            let name = chat_model::text::name(&name).map_err(invalid("name"))?;
            let updated = a.db.rename_channel(channel, &name).await.map_err(db)?;
            let space = c.space.expect("a thread is in a space");
            a.bus.publish(vec![space_topic(space)], Event::ChannelUpserted { channel: updated.clone() }).await;
            Ok(updated)
        }
        _ => Err(forbidden("Rename channels in the space's settings.")),
    }
}

/// The direct conversation with someone, opened (made if need be).
#[server]
pub async fn open_direct(with: Id, user: Auth<Principal>) -> Answer<Channel> {
    use crate::srv::{app, db, forbidden, me, resync};
    let account = me(&user.0).await?;
    let a = app();
    if a.db.blocked_between(account.id(), with).await.map_err(db)? {
        return Err(forbidden("You can't message them."));
    }
    a.db.user(with).await.map_err(db)?;
    let (channel, _) = a.db.open_direct(account.id(), with).await.map_err(db)?;
    // Hear the conversation's topic from now on.
    resync(&[account.id()]).await;
    Ok(channel)
}

/// A group conversation with some people.
#[server]
pub async fn create_group(with: Vec<Id>, name: String, user: Auth<Principal>) -> Answer<Channel> {
    use crate::srv::{app, db, forbidden, invalid, me, resync};
    let account = me(&user.0).await?;
    let a = app();
    for other in &with {
        if a.db.blocked_between(account.id(), *other).await.map_err(db)? {
            return Err(forbidden("You can't add someone you've blocked, or who blocked you."));
        }
    }
    let name = if name.trim().is_empty() { String::new() } else { chat_model::text::name(&name).map_err(invalid("name"))? };
    let channel = a.db.create_group(account.id(), &with, &name).await.map_err(db)?;
    resync(&channel.members).await;
    Ok(channel)
}

/// Add someone to a group conversation.
#[server]
pub async fn add_to_group(group: Id, add: Id, user: Auth<Principal>) -> Answer<Channel> {
    use chat_model::live::Event;

    use crate::srv::{app, channel_topic, db, forbidden, resync};
    let (me, c, _, _) = body::open(&user.0, group).await?;
    if c.kind != ChannelKind::Group {
        return Err(forbidden("Only group conversations take more people."));
    }
    let a = app();
    if a.db.blocked_between(me, add).await.map_err(db)? {
        return Err(forbidden("You can't add them."));
    }
    let updated = a.db.add_to_group(group, add).await.map_err(db)?;
    a.bus.publish(vec![channel_topic(&updated).await?], Event::ChannelUpserted { channel: updated.clone() }).await;
    resync(&[add]).await;
    Ok(updated)
}

/// Leave a group conversation.
#[server]
pub async fn leave_group(group: Id, user: Auth<Principal>) -> Answer<()> {
    use chat_model::live::Event;

    use crate::srv::{app, channel_topic, db, forbidden, resync};
    let (me, c, _, _) = body::open(&user.0, group).await?;
    if c.kind != ChannelKind::Group {
        return Err(forbidden("Close a direct conversation instead."));
    }
    let a = app();
    if let Some(updated) = a.db.leave_group(group, me).await.map_err(db)? {
        a.bus.publish(vec![channel_topic(&updated).await?], Event::ChannelUpserted { channel: updated }).await;
    }
    a.bus.to_user(me, Event::ChannelRemoved { channel: group }).await;
    resync(&[me]).await;
    Ok(())
}

/// Take a conversation out of the caller's list until someone writes in it.
#[server]
pub async fn close_conversation(channel: Id, user: Auth<Principal>) -> Answer<()> {
    use chat_model::live::Event;

    use crate::srv::{app, db, forbidden};
    let (me, c, _, _) = body::open(&user.0, channel).await?;
    if !c.kind.is_private() {
        return Err(forbidden("That isn't a conversation."));
    }
    app().db.hide_conversation(channel, me).await.map_err(db)?;
    app().bus.to_user(me, Event::ChannelRemoved { channel }).await;
    Ok(())
}