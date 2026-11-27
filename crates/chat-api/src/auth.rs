//! Accounts: signing up, verifying an email, signing in and out, passwords,
//! profiles, status and devices.
//!
//! # Flows
//!
//! ```text
//! sign_up ──▶ email with a code ──▶ verify_email ──▶ signed in
//! sign_in (unverified) ──▶ Unverified, code re-sent ──▶ verify_email
//! forgot_password ──▶ email with a code ──▶ reset_password ──▶ signed in, every other session out
//! ```
//!
//! Codes are six digits, live fifteen minutes, and survive five wrong
//! guesses. Resending is throttled to once a minute per account. Sign-in
//! takes as long for an unknown email as for a wrong password, and answers
//! both with [`crate::ApiError::BadCredentials`].

use chat_model::{Device, Id, Me, Status, User};
use serde::{Deserialize, Serialize};
use server::server;
use server_kit::Auth;

use crate::{Answer, Principal};

/// A session just started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// The bearer token, for native clients to keep in the OS keystore. Web
    /// clients get an httpOnly cookie instead and this is `None`.
    pub token: Option<String>,
    pub me: Me,
}

/// Sign-up went through; a code is on its way to `email`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignedUp {
    pub email: String,
}

#[cfg(feature = "server")]
mod body {
    use chat_db::{Account, CodeCheck, CodePurpose};
    use chat_model::live::Event;
    use time::Duration;

    use crate::srv::{app, db, fail, Mail};
    use crate::{ApiError, Failure, Principal};

    use super::Session;

    pub const CODE_TTL: Duration = Duration::minutes(15);
    pub const RESEND_AFTER: Duration = Duration::seconds(60);

    pub async fn start(account: &Account) -> Result<Session, Failure> {
        let principal = Principal { user: account.id(), epoch: account.epoch };
        let ticket = app().sessions.start(&principal).await.map_err(|e| {
            eprintln!("[chat] session store: {e:?}");
            fail(ApiError::Unavailable { message: "Couldn't sign you in right now. Try again.".into() })
        })?;
        Ok(Session { token: ticket.token, me: account.me.clone() })
    }

    /// Email `account` a code, unless one went out less than a minute ago.
    pub async fn send_code(account: &Account, purpose: CodePurpose) -> Result<(), Failure> {
        let a = app();
        if let Some(at) = a.db.code_issued_at(account.id(), purpose).await.map_err(db)? {
            let since = time::OffsetDateTime::now_utc() - at;
            if since < RESEND_AFTER {
                return Err(fail(ApiError::Slow { retry_secs: (RESEND_AFTER - since).whole_seconds().max(1) as u64 }));
            }
        }
        let code = a.db.issue_code(account.id(), purpose, CODE_TTL).await.map_err(db)?;
        let (to, name) = (account.me.email.clone(), account.me.user.display_name.clone());
        let email = urlencode(&to);
        let mail = match purpose {
            CodePurpose::Verify => {
                Mail::Verify { to, name, link: format!("{}/verify?email={email}&code={code}", a.public_url), code }
            }
            CodePurpose::Reset => {
                Mail::Reset { to, name, link: format!("{}/reset?email={email}&code={code}", a.public_url), code }
            }
        };
        a.mail.send(mail).await.map_err(|e| {
            eprintln!("[chat] mail: {e}");
            fail(ApiError::Unavailable { message: "We couldn't send the email. Try again shortly.".into() })
        })
    }

    pub fn code_failure(check: CodeCheck) -> Failure {
        fail(match check {
            CodeCheck::Wrong => ApiError::invalid("code", "That code isn't right."),
            CodeCheck::Expired => ApiError::invalid("code", "That code has expired. Ask for a new one."),
            CodeCheck::Locked => ApiError::invalid("code", "Too many wrong tries. Ask for a new code."),
            CodeCheck::Ok => unreachable!("not a failure"),
        })
    }

    fn urlencode(s: &str) -> String {
        s.bytes()
            .map(|b| match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect()
    }

    /// Tell everyone who can see `user` that their profile changed.
    pub async fn announce_profile(user: chat_model::User) {
        let topics = crate::srv::seen_by(user.id).await;
        app().bus.publish(topics, Event::UserUpdated { user }).await;
    }
}

/// Make an account and email a code to verify it.
#[server(tags(limit = "5/min"))]
pub async fn sign_up(email: String, password: String, handle: String, display_name: String) -> Answer<SignedUp> {
    use chat_db::{CodePurpose, NewAccount};
    use chat_model::text;

    use crate::srv::{app, db, invalid};
    let email = text::email(&email).map_err(invalid("email"))?;
    text::password(&password).map_err(invalid("password"))?;
    let handle = text::handle(&handle).map_err(invalid("handle"))?;
    let display_name = text::display_name(&display_name).map_err(invalid("display_name"))?;
    let account = app()
        .db
        .create_account(&NewAccount { email: email.clone(), password, handle, display_name })
        .await
        .map_err(|e| match e {
            chat_db::Error::Taken("That email") => {
                crate::srv::fail(crate::ApiError::invalid("email", "That email already has an account. Sign in instead."))
            }
            chat_db::Error::Taken(_) => crate::srv::fail(crate::ApiError::invalid("handle", "That username is taken.")),
            e => db(e),
        })?;
    body::send_code(&account, CodePurpose::Verify).await?;
    Ok(SignedUp { email })
}

/// Confirm an email with the code sent to it, and sign in.
#[server(tags(limit = "20/min"))]
pub async fn verify_email(email: String, code: String) -> Answer<Session> {
    use chat_db::{CodeCheck, CodePurpose};

    use crate::srv::{app, db, fail};
    use crate::ApiError;
    let a = app();
    let Some(account) = a.db.account_by_email(email.trim()).await.map_err(db)? else {
        return Err(fail(ApiError::invalid("code", "That code isn't right.")));
    };
    if !account.verified {
        match a.db.check_code(account.id(), CodePurpose::Verify, &code).await.map_err(db)? {
            CodeCheck::Ok => a.db.mark_verified(account.id()).await.map_err(db)?,
            other => return Err(body::code_failure(other)),
        }
    }
    let account = a.db.account(account.id()).await.map_err(db)?;
    body::start(&account).await
}

/// Send another verification code.
#[server(tags(limit = "5/min"))]
pub async fn resend_code(email: String) -> Answer<()> {
    use chat_db::CodePurpose;

    use crate::srv::{app, db};
    // Silent for unknown and already-verified addresses: this must not say
    // which emails have accounts.
    if let Some(account) = app().db.account_by_email(email.trim()).await.map_err(db)? {
        if !account.verified {
            body::send_code(&account, CodePurpose::Verify).await?;
        }
    }
    Ok(())
}

/// Sign in with email and password.
#[server(tags(limit = "10/min"))]
pub async fn sign_in(email: String, password: String) -> Answer<Session> {
    use chat_db::CodePurpose;

    use crate::srv::{app, db, fail};
    use crate::ApiError;
    let Some(account) = app().db.account_by_email(email.trim()).await.map_err(db)? else {
        chat_db::dummy_password_check(&password);
        return Err(fail(ApiError::BadCredentials));
    };
    if !account.check_password(&password) {
        return Err(fail(ApiError::BadCredentials));
    }
    if !account.verified {
        // A throttled resend still leaves the code they have working.
        let _ = body::send_code(&account, CodePurpose::Verify).await;
        return Err(fail(ApiError::Unverified { email: account.me.email.clone() }));
    }
    body::start(&account).await
}

/// Email a code to reset a forgotten password. Always answers the same, so
/// it cannot be used to find out who has an account.
#[server(tags(limit = "5/min"))]
pub async fn forgot_password(email: String) -> Answer<()> {
    use chat_db::CodePurpose;

    use crate::srv::{app, db};
    if let Some(account) = app().db.account_by_email(email.trim()).await.map_err(db)? {
        match body::send_code(&account, CodePurpose::Reset).await {
            Err(e) if matches!(e, server::ServerError::Failed(crate::ApiError::Unavailable { .. })) => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// Set a new password with an emailed code. Signs out every other session,
/// and verifies the email if it was not (the code proved it).
#[server(tags(limit = "10/min"))]
pub async fn reset_password(email: String, code: String, password: String) -> Answer<Session> {
    use chat_db::{CodeCheck, CodePurpose};
    use chat_model::text;

    use crate::srv::{app, db, fail, invalid, Mail};
    use crate::ApiError;
    text::password(&password).map_err(invalid("password"))?;
    let a = app();
    let Some(account) = a.db.account_by_email(email.trim()).await.map_err(db)? else {
        return Err(fail(ApiError::invalid("code", "That code isn't right.")));
    };
    match a.db.check_code(account.id(), CodePurpose::Reset, &code).await.map_err(db)? {
        CodeCheck::Ok => {}
        other => return Err(body::code_failure(other)),
    }
    a.db.set_password(account.id(), &password).await.map_err(db)?;
    a.db.mark_verified(account.id()).await.map_err(db)?;
    let _ = a.mail.send(Mail::PasswordChanged { to: account.me.email.clone(), name: account.me.user.display_name.clone() }).await;
    let account = a.db.account(account.id()).await.map_err(db)?;
    body::start(&account).await
}

/// Who is signed in. Fails with `SignedOut` (or a 401) when nobody is.
#[server]
pub async fn whoami(user: Auth<Principal>) -> Answer<Me> {
    Ok(crate::srv::me(&user.0).await?.me)
}

/// End this session.
#[server]
pub async fn sign_out(user: Auth<Principal>) -> Answer<()> {
    let _ = user;
    let _ = crate::srv::app().sessions.end().await;
    Ok(())
}

/// End every session of this account, this one included.
#[server]
pub async fn sign_out_everywhere(user: Auth<Principal>) -> Answer<()> {
    use crate::srv::{app, db};
    app().db.bump_epoch(user.0.user).await.map_err(db)?;
    let _ = app().sessions.end().await;
    Ok(())
}

/// Change the password, knowing the current one. Every other session is
/// signed out; this one continues with a fresh session.
#[server(tags(limit = "10/min"))]
pub async fn change_password(current: String, password: String, user: Auth<Principal>) -> Answer<Session> {
    use chat_model::text;

    use crate::srv::{app, db, fail, invalid, me, Mail};
    use crate::ApiError;
    let account = me(&user.0).await?;
    if !account.check_password(&current) {
        return Err(fail(ApiError::invalid("current", "That isn't your current password.")));
    }
    text::password(&password).map_err(invalid("password"))?;
    app().db.set_password(account.id(), &password).await.map_err(db)?;
    let _ = app().sessions.end().await;
    let _ = app()
        .mail
        .send(Mail::PasswordChanged { to: account.me.email.clone(), name: account.me.user.display_name.clone() })
        .await;
    let account = app().db.account(account.id()).await.map_err(db)?;
    body::start(&account).await
}

/// Whether a username is free.
#[server(tags(limit = "60/min"))]
pub async fn handle_available(handle: String) -> Answer<bool> {
    use crate::srv::{app, db, invalid};
    let handle = chat_model::text::handle(&handle).map_err(invalid("handle"))?;
    app().db.handle_free(&handle).await.map_err(db)
}

/// Change what others see: name and bio. `None` keeps a field.
#[server]
pub async fn update_profile(display_name: Option<String>, bio: Option<String>, user: Auth<Principal>) -> Answer<User> {
    use chat_model::text;

    use crate::srv::{app, db, invalid, me};
    let account = me(&user.0).await?;
    let name = display_name.as_deref().map(text::display_name).transpose().map_err(invalid("display_name"))?;
    let bio = bio.as_deref().map(text::about).transpose().map_err(invalid("bio"))?;
    let updated = app().db.update_profile(account.id(), name.as_deref(), bio.as_deref(), None).await.map_err(db)?;
    body::announce_profile(updated.clone()).await;
    Ok(updated)
}

/// Change the cosmetic ids others see on a profile: an avatar decoration and
/// a profile effect. Per argument, `None` keeps the field, `""` clears it,
/// anything else sets the id.
#[server]
pub async fn set_cosmetics(avatar_decoration: Option<String>, profile_effect: Option<String>, user: Auth<Principal>) -> Answer<User> {
    use chat_model::text;

    use crate::srv::{app, db, invalid, me};
    let account = me(&user.0).await?;
    let dec = text::cosmetic_update(avatar_decoration.as_deref()).map_err(invalid("avatar_decoration"))?;
    let eff = text::cosmetic_update(profile_effect.as_deref()).map_err(invalid("profile_effect"))?;
    let updated = app().db.set_cosmetics(account.id(), dec.as_ref().map(|o| o.as_deref()), eff.as_ref().map(|o| o.as_deref())).await.map_err(db)?;
    body::announce_profile(updated.clone()).await;
    Ok(updated)
}

/// Change the username.
#[server(tags(limit = "10/min"))]
pub async fn set_handle(handle: String, user: Auth<Principal>) -> Answer<User> {
    use crate::srv::{app, db, fail, invalid, me};
    use crate::ApiError;
    let account = me(&user.0).await?;
    let handle = chat_model::text::handle(&handle).map_err(invalid("handle"))?;
    let updated = app().db.set_handle(account.id(), &handle).await.map_err(|e| match e {
        chat_db::Error::Taken(_) => fail(ApiError::invalid("handle", "That username is taken.")),
        e => db(e),
    })?;
    body::announce_profile(updated.clone()).await;
    Ok(updated)
}

/// Set the status others see.
#[server]
pub async fn set_status(status: Status, user: Auth<Principal>) -> Answer<Me> {
    use chat_model::live::Event;

    use crate::srv::{app, db, invalid, me};
    let account = me(&user.0).await?;
    let text = match status.text.as_deref() {
        Some(t) => chat_model::text::status_text(t).map_err(invalid("status"))?,
        None => None,
    };
    let status = Status { presence: status.presence, text };
    let a = app();
    a.db.set_status(account.id(), &status).await.map_err(db)?;
    let account = a.db.account(account.id()).await.map_err(db)?;
    a.bus.to_user(account.id(), Event::MeUpdated { me: account.me.clone() }).await;
    crate::srv::announce_presence(account.id()).await;
    Ok(account.me)
}

/// Publish this device's public key, so calls can be sealed to it. Returns
/// the device, existing or new.
#[server]
pub async fn register_device(name: String, public_key: String, user: Auth<Principal>) -> Answer<Device> {
    use chat_model::live::Event;

    use crate::srv::{app, db, invalid, me};
    let account = me(&user.0).await?;
    let name = chat_model::text::name(&name).map_err(invalid("name"))?;
    if public_key.len() != 43 || !public_key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(invalid("public_key")("That isn't a device key.".into()));
    }
    let a = app();
    let device = a.db.register_device(account.id(), &name, &public_key).await.map_err(db)?;
    let devices = a.db.devices(account.id()).await.map_err(db)?;
    a.bus.to_user(account.id(), Event::DevicesChanged { devices }).await;
    Ok(device)
}

/// Forget a device: it can no longer receive call keys.
#[server]
pub async fn remove_device(device: Id, user: Auth<Principal>) -> Answer<()> {
    use chat_model::live::Event;

    use crate::srv::{app, db, me};
    let account = me(&user.0).await?;
    let a = app();
    a.db.remove_device(account.id(), device).await.map_err(db)?;
    let devices = a.db.devices(account.id()).await.map_err(db)?;
    a.bus.to_user(account.id(), Event::DevicesChanged { devices }).await;
    Ok(())
}

/// A ticket to open the live connection with: a minute long, naming this
/// account and (if it has one) this device.
#[server]
pub async fn live_ticket(device: Option<Id>, user: Auth<Principal>) -> Answer<String> {
    use crate::srv::{app, db, forbidden, me, mint_ticket, now_ms};
    let account = me(&user.0).await?;
    if let Some(d) = device {
        let (owner, _) = app().db.device(d).await.map_err(db)?;
        if owner != account.id() {
            return Err(forbidden("That isn't your device."));
        }
    }
    Ok(mint_ticket(account.id(), device, now_ms()))
}
