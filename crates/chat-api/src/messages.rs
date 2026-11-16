//! Messages: history, posting, editing, reactions, reading, search.

use chat_model::timeline::Page;
use chat_model::{Id, Message};
use serde::{Deserialize, Serialize};
use server::server;
use server_kit::Auth;

use crate::{Answer, Principal};

/// Which part of a channel's history to load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "at", rename_all = "snake_case")]
pub enum Window {
    /// The newest messages.
    Latest,
    /// Older than this one.
    Before { id: Id },
    /// Newer than this one.
    After { id: Id },
    /// Around this one, for a jump to it.
    Around { id: Id },
}

/// Where a search looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "in", rename_all = "snake_case")]
pub enum Scope {
    /// Every channel of a space the caller can read.
    Space { id: Id },
    /// One channel.
    Channel { id: Id },
    /// Every conversation the caller is in.
    Conversations,
}

/// The most emoji kinds on one message.
pub const REACTION_KINDS: i64 = 20;

/// A page of a channel's history.
#[server]
pub async fn history(channel: Id, window: Window, user: Auth<Principal>) -> Answer<Page> {
    use crate::channels::body::open;
    use crate::srv::{app, db, forbidden};
    let (_, c, access, _) = open(&user.0, channel).await?;
    if !access.can_read() {
        return Err(forbidden("You don't have access to this channel."));
    }
    if !c.kind.has_messages() {
        return Ok(Page { messages: vec![], older: false, present: true });
    }
    let db_ = &app().db;
    const PAGE: u32 = 50;
    match window {
        Window::Latest => db_.page(channel, None, None, PAGE).await,
        Window::Before { id } => db_.page(channel, Some(id), None, PAGE).await,
        Window::After { id } => db_.page(channel, None, Some(id), PAGE).await,
        Window::Around { id } => db_.around(channel, id, PAGE / 2).await,
    }
    .map_err(db)
}

/// One message, if the caller can read where it is.
#[server]
pub async fn message(id: Id, user: Auth<Principal>) -> Answer<Message> {
    use crate::channels::body::open;
    use crate::srv::{app, db, fail};
    use crate::ApiError;
    let m = app().db.message(id).await.map_err(db)?;
    let (_, _, access, _) = open(&user.0, m.channel).await?;
    if !access.can_read() {
        return Err(fail(ApiError::NotFound));
    }
    Ok(m)
}

/// Post a message. `@handle` mentions of people who can be mentioned here
/// become mention tokens; `attachments` are upload ids from `files`.
#[server(tags(limit = "120/min"))]
pub async fn send(
    channel: Id,
    body: String,
    reply_to: Option<Id>,
    attachments: Vec<Id>,
    user: Auth<Principal>,
) -> Answer<Message> {
    use chat_db::NewMessage;
    use chat_model::live::Event;
    use chat_model::{ChannelKind, MessageKind};

    use crate::channels::body::open;
    use crate::srv::{app, channel_topic, db, forbidden, invalid, resync};
    let (me, c, access, space) = open(&user.0, channel).await?;
    if !c.kind.has_messages() {
        return Err(forbidden("Voice channels have no messages."));
    }
    if !access.can_post() {
        return Err(forbidden(if c.archived { "This thread is archived." } else { "You can't post in this channel." }));
    }
    let a = app();
    if c.kind == ChannelKind::Direct {
        if let Some(other) = c.members.iter().find(|m| **m != me) {
            if a.db.blocked_between(me, *other).await.map_err(db)? {
                return Err(forbidden("You can't message them."));
            }
        }
    }
    if attachments.len() > 10 {
        return Err(invalid("attachments")("At most 10 files per message.".into()));
    }
    let text = chat_model::text::message(&body).map_err(invalid("body"))?;
    if text.is_empty() && attachments.is_empty() {
        return Err(invalid("body")("Write something first.".into()));
    }
    // Who can be mentioned: the space's members, or the conversation's.
    let audience: Vec<Id> = match &space {
        Some(s) => s.members.keys().copied().collect(),
        None => c.members.clone(),
    };
    let people = a.db.users(&audience).await.map_err(db)?;
    let mut resolved = chat_model::text::resolve_mentions(&text, |h| {
        people.iter().find(|u| u.handle.eq_ignore_ascii_case(h)).map(|u| u.id)
    });
    resolved.mentions.retain(|m| audience.contains(m));
    // `@everyone` needs the MentionEveryone permission.
    let everyone = resolved.everyone
        && match &space {
            Some(s) => chat_model::perms::can(&s, me, chat_model::perms::Permission::MentionEveryone),
            None => true,
        };
    if let Some(r) = reply_to {
        if a.db.message(r).await.map_err(db)?.channel != channel {
            return Err(invalid("reply_to")("You can only reply in the same channel.".into()));
        }
    }
    let posted = a
        .db
        .post(&NewMessage {
            channel,
            author: me,
            kind: MessageKind::Text,
            body: resolved.body,
            reply_to,
            mentions: resolved.mentions,
            everyone,
            attachments,
        })
        .await
        .map_err(db)?;
    // A conversation someone had closed comes back for them.
    if c.kind.is_private() {
        let reopened = a.db.unhide_conversation(channel).await.map_err(db)?;
        if !reopened.is_empty() {
            resync(&reopened).await;
        }
    }
    a.bus.publish(vec![channel_topic(&c).await?], Event::MessageCreated { message: posted.message.clone() }).await;
    Ok(posted.message)
}

/// Change a message's text. Its author only.
#[server(tags(limit = "60/min"))]
pub async fn edit(id: Id, body: String, user: Auth<Principal>) -> Answer<Message> {
    use chat_model::live::Event;
    use chat_model::MessageKind;

    use crate::channels::body::open;
    use crate::srv::{app, channel_topic, db, forbidden, invalid};
    let a = app();
    let m = a.db.message(id).await.map_err(db)?;
    let (me, c, access, space) = open(&user.0, m.channel).await?;
    if !chat_model::perms::can_edit_message(space.as_ref(), me, m.author) || !matches!(m.kind, MessageKind::Text) {
        return Err(forbidden("You can only edit your own messages."));
    }
    if !access.can_post() {
        return Err(forbidden("You can't post in this channel."));
    }
    let text = chat_model::text::message(&body).map_err(invalid("body"))?;
    if text.is_empty() && m.attachments.is_empty() {
        return Err(invalid("body")("A message can't be empty. Delete it instead.".into()));
    }
    let audience: Vec<Id> = match &space {
        Some(s) => s.members.keys().copied().collect(),
        None => c.members.clone(),
    };
    let people = a.db.users(&audience).await.map_err(db)?;
    let resolved = chat_model::text::resolve_mentions(&text, |h| {
        people.iter().find(|u| u.handle.eq_ignore_ascii_case(h)).map(|u| u.id)
    });
    // An edit cannot newly ping everyone; it keeps what the original had.
    let everyone = resolved.everyone && m.mentions_everyone;
    let edited = a.db.edit_message(id, &resolved.body, &resolved.mentions, everyone).await.map_err(db)?;
    a.bus.publish(vec![channel_topic(&c).await?], Event::MessageEdited { message: edited.clone() }).await;
    Ok(edited)
}

/// Delete a message: its author, or a space's admins.
#[server]
pub async fn delete(id: Id, user: Auth<Principal>) -> Answer<()> {
    use chat_model::live::Event;

    use crate::channels::body::open;
    use crate::srv::{app, channel_topic, db, forbidden};
    let a = app();
    let m = a.db.message(id).await.map_err(db)?;
    let (me, c, _, space) = open(&user.0, m.channel).await?;
    if !chat_model::perms::can_delete_message(space.as_ref(), me, m.author) {
        return Err(forbidden("You can't delete that."));
    }
    let keys = a.db.delete_message(id).await.map_err(db)?;
    for k in keys {
        a.files.delete(&k).await;
    }
    a.bus.publish(vec![channel_topic(&c).await?], Event::MessageDeleted { channel: m.channel, message: id }).await;
    Ok(())
}

/// Add or take back a reaction.
#[server(tags(limit = "120/min"))]
pub async fn react(id: Id, emoji: String, add: bool, user: Auth<Principal>) -> Answer<()> {
    use chat_model::live::Event;

    use crate::channels::body::open;
    use crate::srv::{app, channel_topic, db, forbidden, invalid};
    let emoji = emoji.trim().to_string();
    if emoji.is_empty() || emoji.chars().count() > 16 || emoji.chars().any(|c| c.is_whitespace() || c.is_ascii_alphanumeric()) {
        return Err(invalid("emoji")("That isn't an emoji.".into()));
    }
    let a = app();
    let m = a.db.message(id).await.map_err(db)?;
    let (me, c, access, _) = open(&user.0, m.channel).await?;
    if !access.can_read() || c.archived {
        return Err(forbidden("You can't react here."));
    }
    if add && !m.reactions.iter().any(|r| r.emoji == emoji) && a.db.reaction_kinds(id).await.map_err(db)? >= REACTION_KINDS {
        return Err(forbidden("That message has all the reactions it can take."));
    }
    if a.db.react(id, me, &emoji, add).await.map_err(db)? {
        a.bus
            .publish(
                vec![channel_topic(&c).await?],
                Event::ReactionChanged { channel: m.channel, message: id, emoji, user: me, added: add },
            )
            .await;
    }
    Ok(())
}

/// Mark a channel read up to a message.
#[server(tags(limit = "240/min"))]
pub async fn mark_read(channel: Id, last_read: Id, user: Auth<Principal>) -> Answer<()> {
    use chat_model::live::Event;

    use crate::channels::body::open;
    use crate::srv::{app, db};
    let (me, _, access, _) = open(&user.0, channel).await?;
    if !access.can_read() {
        return Ok(());
    }
    let at = app().db.mark_read(me, channel, last_read).await.map_err(db)?;
    // Every device of the reader follows.
    app().bus.to_user(me, Event::Read { channel, last_read: at }).await;
    Ok(())
}

/// Search messages the caller can read, newest first.
#[server(tags(limit = "30/min"))]
pub async fn search(scope: Scope, query: String, user: Auth<Principal>) -> Answer<Vec<Message>> {
    use chat_model::perms::{self, Access};
    use chat_model::{Channel, ChannelKind};

    use crate::channels::body::open;
    use crate::srv::{app, db, fail, me};
    use crate::ApiError;
    let a = app();
    let channels: Vec<Id> = match scope {
        Scope::Channel { id } => {
            let (_, _, access, _) = open(&user.0, id).await?;
            if access.can_read() { vec![id] } else { vec![] }
        }
        Scope::Space { id } => {
            let account = me(&user.0).await?;
            let space = a.db.space(id).await.map_err(db)?;
            if !space.members.contains_key(&account.id()) {
                return Err(fail(ApiError::NotFound));
            }
            let all = a.db.space_channels(id).await.map_err(db)?;
            let rules_of = |c: &Channel| -> Channel {
                match c.parent.and_then(|p| all.iter().find(|x| x.id == p)) {
                    Some(parent) => Channel { archived: c.archived, ..parent.clone() },
                    None => c.clone(),
                }
            };
            all.iter()
                .filter(|c| c.kind != ChannelKind::Voice)
                .filter(|c| perms::channel_access(account.id(), &rules_of(c), Some(&space)) >= Access::ReadOnly)
                .map(|c| c.id)
                .collect()
        }
        Scope::Conversations => {
            let account = me(&user.0).await?;
            a.db.conversations_of(account.id()).await.map_err(db)?.into_iter().map(|c| c.id).collect()
        }
    };
    Ok(a.db.search(&channels, &query, None, 25).await.map_err(db)?.into_iter().map(|h| h.message).collect())
}
