//! Calls: joining and leaving, what each person sends, ringing in
//! conversations, and passing the end-to-end room key between devices.
//!
//! # Joining
//!
//! ```text
//! join_call(channel, device) ──▶ CallTicket { relay, grant, room, make_key }
//!   make_key: this device generates the room key (call_e2ee::RoomKey)
//!   else:     request_key(room) ──▶ KeyRequested to everyone in the room
//!             a device holding the key ──▶ deliver_key(sealed to us) ──▶ KeyDelivered
//! then dial the relay with the grant and the key.
//! ```
//!
//! The server only ever carries the key sealed (`chat-keys`), so neither it
//! nor the relay can hear the call. A room is one call instance: when the
//! last person leaves it ends, and the next call in the channel is a new
//! room with a new key.
//!
//! Joining needs a live connection from the device: a person is in a call
//! for as long as the connection that joined it lasts, so a closed tab or a
//! dropped network takes them out instead of leaving a ghost.

use chat_model::{CallTicket, Id, SealedKey};
use server::server;
use server_kit::Auth;

use crate::{Answer, Principal};

/// How long a conversation call rings before it counts as missed.
pub const RING_SECS: u64 = 45;

#[cfg(feature = "server")]
mod body {
    use chat_db::LeftCall;
    use chat_model::live::Event;
    use chat_model::{CallRecord, Id, MessageKind};

    use crate::srv::{app, channel_topic, now_ms};

    /// Tell everyone a person left a call, take them out at the relay, and
    /// if the call ended in a conversation, close its record.
    pub async fn left(l: LeftCall) {
        let a = app();
        a.calls.kick(l.room, l.user, now_ms() as u64);
        let Ok(channel) = a.db.channel(l.channel).await else { return };
        let Ok(topic) = channel_topic(&channel).await else { return };
        a.bus.publish(vec![topic.clone()], Event::VoiceLeft { channel: l.channel, user: l.user }).await;
        if l.ended && channel.kind.is_private() {
            a.bus.publish(vec![topic.clone()], Event::RingEnded { channel: l.channel }).await;
            if let Ok((_, by, started, people, ended, Some(msg))) = a.db.room(l.room).await {
                let record = CallRecord { started_by: by, started_at: started, ended_at: ended.or(Some(now_ms())), participants: people };
                if let Ok(message) = a.db.set_message_kind(msg, &MessageKind::Call(record)).await {
                    a.bus.publish(vec![topic], Event::MessageEdited { message }).await;
                }
            }
        }
    }

    /// Stop ringing a room after a while: nobody answered.
    pub fn ring_timeout(room: Id, channel: Id) {
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(super::RING_SECS)).await;
            let a = app();
            if a.db.stop_ringing(room, None).await.is_ok() {
                if let Ok(c) = a.db.channel(channel).await {
                    if let Ok(topic) = channel_topic(&c).await {
                        a.bus.publish(vec![topic], Event::RingEnded { channel }).await;
                    }
                }
            }
        });
    }
}

/// Take `user` out of whatever call they are in.
#[cfg(feature = "server")]
pub(crate) async fn leave_everywhere(user: Id) {
    if let Ok(Some(l)) = crate::srv::app().db.leave_call(user).await {
        body::left(l).await;
    }
}

/// Tell everyone about a call someone left without asking to: their
/// connection went away, or their server did.
#[cfg(feature = "server")]
pub async fn announce_left(left: chat_db::LeftCall) {
    body::left(left).await;
}

/// What a connection dropping meant for calls. Server-internal.
#[cfg(feature = "server")]
pub(crate) async fn connection_left(left: Option<chat_db::LeftCall>) {
    if let Some(l) = left {
        body::left(l).await;
    }
}

/// Join a channel's call (a voice channel, or a call in a conversation —
/// starting one rings everyone else in it). Leaves any other call.
#[server]
pub async fn join_call(channel: Id, device: Id, user: Auth<Principal>) -> Answer<CallTicket> {
    use chat_db::NewMessage;
    use chat_model::live::Event;
    use chat_model::{CallRecord, MessageKind};

    use crate::channels::body::open;
    use crate::srv::{app, channel_topic, db, fail, forbidden, now_ms};
    use crate::ApiError;
    let (me, c, access, space) = open(&user.0, channel).await?;
    if !c.kind.has_calls() {
        return Err(forbidden("There are no calls in this channel."));
    }
    if !access.can_post() {
        return Err(forbidden("You can't join this call."));
    }
    if !chat_model::perms::can_connect(&c, space.as_ref(), me) {
        return Err(forbidden("You can't join this call."));
    }
    let a = app();
    let (owner, _) = a.db.device(device).await.map_err(db)?;
    if owner != me {
        return Err(forbidden("That isn't your device."));
    }
    let Some(connection) = a.db.connection_for(me, device).await.map_err(db)? else {
        return Err(fail(ApiError::Unavailable { message: "Still connecting. Try again in a moment.".into() }));
    };
    let joined = a.db.join_call(me, channel, connection, device).await.map_err(db)?;
    if let Some(l) = joined.left.clone() {
        body::left(l).await;
    }
    let topic = channel_topic(&c).await?;
    a.bus.publish(vec![topic.clone()], Event::VoiceJoined { state: joined.state }).await;
    if c.kind.is_private() {
        if joined.started {
            // A new call in a conversation: ring everyone else, and put its
            // record in the stream.
            let others: Vec<Id> = c.members.iter().copied().filter(|m| *m != me).collect();
            if let Ok(ring) = a.db.start_ring(joined.room, channel, me, &others).await {
                a.bus.publish(vec![topic.clone()], Event::RingUpdated { ring }).await;
                body::ring_timeout(joined.room, channel);
            }
            let record = CallRecord { started_by: me, started_at: now_ms(), ended_at: None, participants: vec![me] };
            if let Ok(posted) = a
                .db
                .post(&NewMessage {
                    channel,
                    author: me,
                    kind: MessageKind::Call(record),
                    body: String::new(),
                    reply_to: None,
                    mentions: vec![],
                    everyone: false,
                    attachments: vec![],
                })
                .await
            {
                let _ = a.db.set_room_message(joined.room, posted.message.id).await;
                a.bus.publish(vec![topic.clone()], Event::MessageCreated { message: posted.message }).await;
            }
        } else if let Ok(Some(ring)) = a.db.stop_ringing(joined.room, Some(me)).await {
            a.bus.publish(vec![topic.clone()], Event::RingUpdated { ring }).await;
        } else {
            a.bus.publish(vec![topic.clone()], Event::RingEnded { channel }).await;
        }
        // The record lists everyone who joined.
        if let Ok((_, by, started, people, _, Some(msg))) = a.db.room(joined.room).await {
            let record = CallRecord { started_by: by, started_at: started, ended_at: None, participants: people };
            if let Ok(message) = a.db.set_message_kind(msg, &MessageKind::Call(record)).await {
                a.bus.publish(vec![topic], Event::MessageEdited { message }).await;
            }
        }
    }
    Ok(CallTicket {
        channel,
        room: joined.room,
        relay_url: a.calls.relay_url.clone(),
        cert_hash: a.calls.cert_hash.clone(),
        grant: a.calls.grant(joined.room, me, now_ms() as u64),
        make_key: joined.make_key,
    })
}

/// Leave the call the caller is in.
#[server]
pub async fn leave_call(user: Auth<Principal>) -> Answer<()> {
    let account = crate::srv::me(&user.0).await?;
    leave_everywhere(account.id()).await;
    Ok(())
}

/// What the caller is sending in their call.
#[server(tags(limit = "120/min"))]
pub async fn update_call(muted: bool, deafened: bool, camera: bool, screen: bool, user: Auth<Principal>) -> Answer<()> {
    use chat_model::live::Event;
    use chat_model::perms;

    use crate::srv::{app, channel_topic, db, forbidden, me};
    let account = me(&user.0).await?;
    let a = app();
    // What the caller sends is gated by their roles: speaking (unmuting),
    // video and screen share each need their permission. The channel they are
    // in decides which space's roles apply.
    if let Some((_, channel, _)) = a.db.my_room(account.id()).await.map_err(db)? {
        let channel_row = a.db.channel(channel).await.map_err(db)?;
        let space = match channel_row.space {
            Some(space_id) => a.db.space(space_id).await.ok(),
            None => None,
        };
        if !muted && !deafened && !perms::can_speak(&channel_row, space.as_ref(), account.id()) {
            return Err(forbidden("You can't speak in this call."));
        }
        // A timed-out member is held out of speaking until it expires.
        if !muted && !deafened && space.is_some() {
            if let Some(space_id) = channel_row.space {
                if a.db.timed_out(space_id, account.id()).await.map_err(db)? {
                    return Err(forbidden("You are timed out in this space."));
                }
            }
        }
        if camera && !perms::can_video(&channel_row, space.as_ref(), account.id()) {
            return Err(forbidden("You can't use a camera here."));
        }
        if screen && !perms::can_share(&channel_row, space.as_ref(), account.id()) {
            return Err(forbidden("You can't share your screen here."));
        }
    }
    let state = a.db.update_voice(account.id(), muted, deafened, camera, screen).await.map_err(db)?;
    let c = a.db.channel(state.channel).await.map_err(db)?;
    a.bus.publish(vec![channel_topic(&c).await?], Event::VoiceUpdated { state }).await;
    Ok(())
}

/// Stop a ringing call ringing for the caller.
#[server]
pub async fn decline_call(channel: Id, user: Auth<Principal>) -> Answer<()> {
    use chat_model::live::Event;

    use crate::channels::body::open;
    use crate::srv::{app, channel_topic, db};
    let (me, c, _, _) = open(&user.0, channel).await?;
    let a = app();
    let Some(ring) = a.db.rings_in(&[channel]).await.map_err(db)?.pop() else { return Ok(()) };
    let topic = channel_topic(&c).await?;
    match a.db.stop_ringing(ring.call, Some(me)).await.map_err(db)? {
        Some(ring) => a.bus.publish(vec![topic], Event::RingUpdated { ring }).await,
        None => a.bus.publish(vec![topic], Event::RingEnded { channel }).await,
    }
    Ok(())
}

/// A fresh grant for the caller's call, before the one they hold runs out.
#[server(tags(limit = "60/min"))]
pub async fn renew_grant(user: Auth<Principal>) -> Answer<String> {
    use crate::srv::{app, db, fail, me, now_ms};
    use crate::ApiError;
    let account = me(&user.0).await?;
    let a = app();
    let Some((room, _, _)) = a.db.my_room(account.id()).await.map_err(db)? else {
        return Err(fail(ApiError::forbidden("You aren't in a call.")));
    };
    Ok(a.calls.grant(room, account.id(), now_ms() as u64))
}

/// Ask the devices in the caller's call for its room key, sealed to the
/// device the caller joined from.
#[server(tags(limit = "60/min"))]
pub async fn request_key(room: Id, user: Auth<Principal>) -> Answer<()> {
    use chat_model::live::Event;
    use chat_model::KeyRequest;

    use crate::srv::{app, db, forbidden, me};
    let account = me(&user.0).await?;
    let a = app();
    let Some((mine, _, device)) = a.db.my_room(account.id()).await.map_err(db)? else {
        return Err(forbidden("You aren't in that call."));
    };
    if mine != room {
        return Err(forbidden("You aren't in that call."));
    }
    let (_, d) = a.db.device(device).await.map_err(db)?;
    let others: Vec<Id> =
        a.db.room_devices(room).await.map_err(db)?.into_iter().map(|(u, _, _)| u).filter(|u| *u != account.id()).collect();
    a.bus
        .to_users(&others, Event::KeyRequested { request: KeyRequest { room, user: account.id(), device, public_key: d.public_key } })
        .await;
    Ok(())
}

/// Hand a sealed room key to another device in the same call. Both devices
/// must be in the room, as the devices they joined from; the server checks
/// that and carries the sealed key, which it cannot open.
#[server(tags(limit = "120/min"))]
pub async fn deliver_key(key: SealedKey, user: Auth<Principal>) -> Answer<()> {
    use chat_model::live::Event;

    use crate::srv::{app, db, forbidden, me};
    let account = me(&user.0).await?;
    let a = app();
    let devices = a.db.room_devices(key.room).await.map_err(db)?;
    let sender = devices.iter().find(|(u, d, _)| *u == account.id() && *d == key.from_device);
    let Some((_, _, from_key)) = sender else {
        return Err(forbidden("You aren't in that call from that device."));
    };
    let Some((to_user, _, _)) = devices.iter().find(|(_, d, _)| *d == key.to_device) else {
        return Err(forbidden("They aren't in that call."));
    };
    // The sender's key is the directory's, not what the client claimed.
    let key = SealedKey { from_key: from_key.clone(), ..key };
    a.bus.to_user(*to_user, Event::KeyDelivered { key }).await;
    Ok(())
}