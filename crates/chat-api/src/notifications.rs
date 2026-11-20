//! Notification settings: each person's level and mute expiry per space and
//! per channel, read and changed only by that person.

use chat_model::{Id, NotificationSetting, Scope};
use server::server;
use server_kit::Auth;

use crate::{Answer, Principal};

#[cfg(feature = "server")]
pub(crate) mod body {
    use chat_model::live::Event;
    use chat_model::Id;

    use crate::srv::app;

    /// Restate the person's whole setting set to their connections, so a
    /// change follows their other sessions: every connection subscribes to
    /// its viewer's own topic.
    pub async fn restate(me: Id) {
        let settings = app().db.notification_settings(me, None).await.unwrap_or_default();
        app().bus.to_user(me, Event::NotificationSettingsChanged { settings }).await;
    }
}

/// The caller's own settings: all of them, or those of one space — its own
/// row and its channels' rows. Being in the space is what buys the read.
#[server]
pub async fn notification_settings(space: Option<Id>, user: Auth<Principal>) -> Answer<Vec<NotificationSetting>> {
    use crate::srv::{app, db, me};

    let account = me(&user.0).await?;
    if let Some(space) = space {
        crate::spaces::body::member(&user.0, space).await?;
    }
    app().db.notification_settings(account.id(), space).await.map_err(db)
}

/// Change (or, with no level and no mute, clear) one of the caller's own
/// settings, for a target they can read: membership of the space, or reading
/// the channel.
#[server]
pub async fn set_notification_setting(setting: NotificationSetting, user: Auth<Principal>) -> Answer<NotificationSetting> {
    use crate::srv::{app, db, forbidden, invalid, me};

    let account = me(&user.0).await?;
    if !chat_model::mute_end_ok(setting.muted_until) {
        return Err(invalid("muted_until")("A mute end must be in the future.".to_string()));
    }
    match setting.scope {
        Scope::Space => {
            crate::spaces::body::member(&user.0, setting.target).await?;
        }
        Scope::Channel => {
            let (_, _, access, _) = crate::channels::body::open(&user.0, setting.target).await?;
            if !access.can_read() {
                return Err(forbidden("You can't read that channel."));
            }
        }
    }
    app().db.set_notification_setting(account.id(), &setting).await.map_err(db)?;
    body::restate(account.id()).await;
    Ok(setting)
}
