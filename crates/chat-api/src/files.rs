//! Uploads: message attachments, avatars and space icons.
//!
//! ```text
//! begin_upload(name, size, type) ──▶ Slot { id, put_url }
//! PUT the bytes to put_url (straight to the bucket)
//! finish_upload(id, dims) ──▶ Attachment          — then send(…, attachments: [id])
//!                         or set_avatar(id) / set_space_icon(space, id)
//! ```
//!
//! Files are fetched at `Attachment::url` (`/f/<key>`, relative to the
//! server), which redirects to a short-lived signed URL. The key is random:
//! it is the file's capability, as it is in every chat app that serves
//! attachments to clients that cannot send credentials with an image fetch.

use chat_model::{Attachment, Id, User};
use serde::{Deserialize, Serialize};
use server::server;
use server_kit::Auth;

use crate::{Answer, Principal};

/// What an upload is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Message,
    Avatar,
    Icon,
}

/// Where to put an upload's bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    pub id: Id,
    /// A presigned PUT, good for ten minutes. Send `Content-Type` as given.
    pub put_url: String,
    pub content_type: String,
}

/// Reserve an upload.
#[server(tags(limit = "60/min"))]
pub async fn begin_upload(name: String, size: u64, content_type: String, purpose: Purpose, user: Auth<Principal>) -> Answer<Slot> {
    use crate::srv::{app, db, fail, invalid, me, MAX_IMAGE, MAX_UPLOAD};
    use crate::ApiError;
    let account = me(&user.0).await?;
    let name: String = name.trim().chars().filter(|c| !c.is_control() && *c != '/' && *c != '\\').take(200).collect();
    let name = if name.is_empty() { "file".to_string() } else { name };
    let content_type = content_type.trim().to_lowercase();
    let content_type = if content_type.is_empty() || content_type.len() > 100 || !content_type.contains('/') {
        "application/octet-stream".to_string()
    } else {
        content_type
    };
    let (limit, kind) = match purpose {
        Purpose::Message => (MAX_UPLOAD, "message"),
        Purpose::Avatar => (MAX_IMAGE, "avatar"),
        Purpose::Icon => (MAX_IMAGE, "icon"),
    };
    if size == 0 || size > limit {
        return Err(invalid("size")(format!("Files can be up to {} MB.", limit / 1024 / 1024)));
    }
    if purpose != Purpose::Message && !content_type.starts_with("image/") {
        return Err(invalid("content_type")("That needs to be an image.".into()));
    }
    let a = app();
    let upload = a.db.create_upload(account.id(), kind, &name, size, &content_type).await.map_err(db)?;
    let put_url = a.files.put_url(&upload.object_key, &content_type, size).await.map_err(|e| {
        eprintln!("[chat] presign: {e}");
        fail(ApiError::Unavailable { message: "Uploads aren't working right now.".into() })
    })?;
    Ok(Slot { id: upload.id, put_url, content_type })
}

/// Confirm an upload landed. `width`/`height` are an image's, as the client
/// measured it.
#[server]
pub async fn finish_upload(id: Id, width: Option<u32>, height: Option<u32>, user: Auth<Principal>) -> Answer<Attachment> {
    use crate::srv::{app, db, invalid, me};
    let account = me(&user.0).await?;
    let a = app();
    let upload = a.db.upload(id, account.id()).await.map_err(db)?;
    let Some(size) = a.files.size(&upload.object_key).await else {
        return Err(invalid("id")("That upload didn't arrive. Try again.".into()));
    };
    let dims = match (width, height) {
        (Some(w), Some(h)) if upload.content_type.starts_with("image/") && w > 0 && h > 0 => Some((w.min(65_535), h.min(65_535))),
        _ => None,
    };
    a.db.confirm_upload(id, size, dims).await.map_err(db)?;
    Ok(Attachment {
        id,
        name: upload.name,
        size,
        content_type: upload.content_type,
        url: chat_db::file_url(&upload.object_key),
        width: dims.map(|d| d.0),
        height: dims.map(|d| d.1),
    })
}

/// Use a finished image upload as the caller's avatar; `None` removes it.
#[server]
pub async fn set_avatar(upload: Option<Id>, user: Auth<Principal>) -> Answer<User> {
    use crate::srv::{app, db, invalid, me};
    let account = me(&user.0).await?;
    let a = app();
    let url = match upload {
        Some(id) => {
            let u = a.db.upload(id, account.id()).await.map_err(db)?;
            if !u.uploaded || !u.content_type.starts_with("image/") {
                return Err(invalid("upload")("Upload an image first.".into()));
            }
            Some(chat_db::file_url(&u.object_key))
        }
        None => None,
    };
    let updated = a.db.update_profile(account.id(), None, None, Some(url.as_deref())).await.map_err(db)?;
    let topics = crate::srv::seen_by(account.id()).await;
    a.bus.publish(topics, chat_model::live::Event::UserUpdated { user: updated.clone() }).await;
    Ok(updated)
}

/// Use a finished image upload as a space's icon; `None` removes it.
#[server]
pub async fn set_space_icon(space: Id, upload: Option<Id>, user: Auth<Principal>) -> Answer<()> {
    use chat_model::live::Event;

    use crate::srv::{app, db, forbidden, invalid, me, space_topic};
    let account = me(&user.0).await?;
    let a = app();
    let s = a.db.space(space).await.map_err(db)?;
    if !chat_model::perms::can(&s, account.id(), chat_model::perms::Permission::ManageSpaceSettings) {
        return Err(forbidden("Only admins can change the icon."));
    }
    let icon = match upload {
        Some(id) => {
            let u = a.db.upload(id, account.id()).await.map_err(db)?;
            if !u.uploaded || !u.content_type.starts_with("image/") {
                return Err(invalid("upload")("Upload an image first.".into()));
            }
            Some(chat_db::file_url(&u.object_key))
        }
        None => None,
    };
    a.db.rename_space(space, &s.name, icon.as_deref()).await.map_err(db)?;
    a.bus.publish(vec![space_topic(space)], Event::SpaceRenamed { space, name: s.name, icon }).await;
    Ok(())
}
