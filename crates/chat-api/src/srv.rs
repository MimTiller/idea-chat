//! What the server bodies share. Server build only.
//!
//! `chat-server` builds an [`App`] at start and [`install`]s it; every body
//! reaches it through [`app`].
//!
//! # Topics and the bus
//!
//! Events are addressed to topics, and a live connection hears the topics
//! its viewer may: their own (`u:<id>`), each conversation they are in
//! (`c:<id>`), and each space they are a member of (`s:<id>`). A channel's
//! events go to the space topic where the channel is open to everyone
//! (its `view` mask is `0`), and to the channel's own topic (`c:<id>`)
//! where it is gated, so only viewers whose permissions cover the mask
//! subscribe to it — the fan-out, not the client, enforces reading.
//!
//! [`Bus::publish`] sends an envelope (topics + event) through the `pubsub`
//! backend on one shared channel; every server instance subscribes once and
//! routes envelopes to its own connections ([`Router`]). With Redis behind
//! `pubsub`, that is cross-instance fan-out; with the memory backend, one
//! instance.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use chat_db::{Account, Db};
use chat_model::live::Event;
use chat_model::perms::{channel_access, rides_own_topic};
use chat_model::{Channel, Id};
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use server::ServerError;
use server_kit::Sessions;
use sha2::Sha256;
use tokio::sync::mpsc;

use crate::{ApiError, Failure, Principal};

/// Everything the bodies need.
pub struct App {
    pub db: Db,
    pub sessions: Sessions,
    pub mail: Arc<dyn Mailer>,
    pub files: Files,
    pub calls: Calls,
    pub bus: Bus,
    /// Signs live-connection tickets.
    pub live_secret: Vec<u8>,
    /// Where people reach the app, for links in emails.
    pub public_url: String,
    /// This server instance's name, on its live connections' rows.
    pub instance: String,
}

static APP: OnceLock<App> = OnceLock::new();

/// Install the app. Once, at start.
pub fn install(app: App) {
    if APP.set(app).is_err() {
        panic!("chat_api::srv::install called twice");
    }
}

pub fn app() -> &'static App {
    APP.get().expect("chat_api::srv::install was not called before serving")
}

// ---------------------------------------------------------------- errors

pub fn fail(e: ApiError) -> Failure {
    ServerError::Failed(e)
}

/// A database error, as the person should hear it.
pub fn db(e: chat_db::Error) -> Failure {
    match e {
        chat_db::Error::NotFound => fail(ApiError::NotFound),
        chat_db::Error::Taken(what) => fail(ApiError::Taken { message: format!("{what} is already taken.") }),
        chat_db::Error::Refused(message) => fail(ApiError::Forbidden { message }),
        chat_db::Error::Sql(e) => {
            eprintln!("[chat] database error: {e}");
            fail(ApiError::Unavailable { message: "Something went wrong on our side. Try again.".into() })
        }
    }
}

/// Text from `chat_model::text` validation, as an input error.
pub fn invalid(field: &str) -> impl Fn(String) -> Failure + '_ {
    move |message| fail(ApiError::invalid(field, message))
}

pub fn forbidden(message: &str) -> Failure {
    fail(ApiError::forbidden(message))
}

/// The signed-in account behind a principal, if the session still stands:
/// the account exists, its email is verified, and it has not signed out
/// everywhere (or changed its password) since the session started.
pub async fn me(p: &Principal) -> Result<Account, Failure> {
    let account = app().db.account(p.user).await.map_err(|_| fail(ApiError::SignedOut))?;
    if account.epoch != p.epoch {
        return Err(fail(ApiError::SignedOut));
    }
    if !account.verified {
        return Err(fail(ApiError::Unverified { email: account.me.email.clone() }));
    }
    Ok(account)
}

// ---------------------------------------------------------------- mail

/// The emails the app sends.
#[derive(Debug, Clone)]
pub enum Mail {
    /// Confirm the address an account signed up with.
    Verify { to: String, name: String, code: String, link: String },
    /// Reset a forgotten password.
    Reset { to: String, name: String, code: String, link: String },
    /// The password was changed; if that was not them, here is what to do.
    PasswordChanged { to: String, name: String },
}

/// Sends [`Mail`]. `chat-server` provides one — rendering the templates
/// and sending through SES, or printing to the log in development — so this
/// crate names neither.
pub trait Mailer: Send + Sync {
    fn send(&self, mail: Mail) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;
}

// ---------------------------------------------------------------- topics

pub fn user_topic(user: Id) -> String {
    format!("u:{user}")
}

pub fn conversation_topic(channel: Id) -> String {
    format!("c:{channel}")
}

pub fn space_topic(space: Id) -> String {
    format!("s:{space}")
}

/// Everything `viewer` hears, given their live state.
pub fn topics_for(state: &chat_model::live::State) -> Vec<String> {
    let Some(me) = state.me_id() else { return Vec::new() };
    let mut topics = vec![user_topic(me)];
    for space in state.spaces.values() {
        topics.push(space_topic(space.id));
    }
    for c in state.channels.values() {
        if c.kind.is_private() {
            topics.push(conversation_topic(c.id));
            continue;
        }
        let Some(s) = c.space.and_then(|id| state.spaces.get(&id)) else { continue };
        // A thread is judged by its parent's rules and overwrites.
        let rules = c.parent.and_then(|p| state.channels.get(&p)).unwrap_or(c);
        if rides_own_topic(rules, s.everyone) && channel_access(me, rules, Some(s)).can_read() {
            topics.push(conversation_topic(c.id));
        }
    }
    topics
}

/// Everything `user` hears, from the database — for registering a
/// connection before its snapshot is taken.
pub async fn topics_of(user: Id) -> Vec<String> {
    let db = &app().db;
    let mut topics = vec![user_topic(user)];
    for space in db.spaces_of(user).await.unwrap_or_default() {
        topics.push(space_topic(space));
        let Ok(s) = db.space(space).await else { continue };
        // The rules channel: a thread's parent supplies masks and overwrites.
        let channels = db.space_channels(space).await.unwrap_or_default();
        for c in &channels {
            let rules: Channel = match c.parent {
                Some(p) => channels.iter().find(|x| x.id == p).unwrap_or(c).clone(),
                None => c.clone(),
            };
            if rides_own_topic(&rules, s.everyone) && channel_access(user, &rules, Some(&s)).can_read() {
                topics.push(conversation_topic(c.id));
            }
        }
    }
    for c in db.conversations_of(user).await.unwrap_or_default() {
        topics.push(conversation_topic(c.id));
    }
    topics
}

/// The topic a channel's events go to, worked out from a viewer's state.
pub fn channel_topic_of(state: &chat_model::live::State, channel: Id) -> Option<String> {
    let c = state.channels.get(&channel)?;
    if c.kind.is_private() {
        return Some(conversation_topic(c.id));
    }
    let Some(s) = c.space.and_then(|id| state.spaces.get(&id)) else { return None };
    // A thread is heard as its parent is.
    let rules = c.parent.and_then(|p| state.channels.get(&p)).unwrap_or(c);
    Some(if rides_own_topic(rules, s.everyone) { conversation_topic(c.id) } else { space_topic(s.id) })
}

/// The topic a channel's events go to. A thread is heard as its parent is.
pub async fn channel_topic(channel: &Channel) -> Result<String, Failure> {
    Ok(match app().db.audience(channel).await.map_err(db)? {
        chat_db::Audience::Space { space, rules, everyone } =>
            if rides_own_topic(&rules, everyone) { conversation_topic(channel.id) } else { space_topic(space) },
        chat_db::Audience::People(_) => conversation_topic(channel.id),
    })
}

/// Everyone who can see `user`: their own devices, the spaces they are in,
/// the people they relate to (not those they block), and the conversations
/// they are in.
pub async fn seen_by(user: Id) -> Vec<String> {
    let db = &app().db;
    let mut topics = vec![user_topic(user)];
    for space in db.spaces_of(user).await.unwrap_or_default() {
        topics.push(space_topic(space));
    }
    for (other, relation) in db.relations_of(user).await.unwrap_or_default() {
        if relation != chat_model::Relation::Blocked {
            topics.push(user_topic(other));
        }
    }
    for c in db.conversations_of(user).await.unwrap_or_default() {
        topics.push(conversation_topic(c.id));
    }
    topics
}

/// Tell everyone who can see `user` how they now appear.
pub async fn announce_presence(user: Id) {
    let status = app().db.presence_one(user).await.unwrap_or_default();
    let topics = seen_by(user).await;
    app().bus.publish(topics, Event::PresenceChanged { user, status }).await;
}

/// Have `user`'s connections take a fresh snapshot: what they hear changed.
pub async fn resync(users: &[Id]) {
    app().bus.to_users(users, Event::Resync).await;
}

// ---------------------------------------------------------------- the bus

/// One event and where it goes.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Envelope {
    topics: Vec<String>,
    event: Event,
}

const BUS: &str = "idea-chat:events";

/// Publishes events to topics, and routes what arrives to this instance's
/// connections.
pub struct Bus {
    router: Arc<Router>,
}

impl Bus {
    /// Start routing. Call once the `pubsub` backend is configured; spawns
    /// the one subscription this instance holds.
    pub fn start() -> Bus {
        let router = Arc::new(Router::default());
        let r = router.clone();
        tokio::spawn(async move {
            loop {
                let mut stream = Box::pin(pubsub::subscribe::<Envelope>(BUS));
                while let Some(env) = stream.next().await {
                    r.route(&env.topics, Arc::new(env.event));
                }
                // The backend dropped us (a Redis restart): resubscribe.
                eprintln!("[chat] event bus subscription ended; resubscribing");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
        Bus { router }
    }

    pub fn router(&self) -> &Arc<Router> {
        &self.router
    }

    /// Send `event` to everyone hearing any of `topics`. Best effort: a
    /// failed publish is logged, and clients converge on their next
    /// snapshot.
    pub async fn publish(&self, topics: Vec<String>, event: Event) {
        if topics.is_empty() {
            return;
        }
        if let Err(e) = pubsub::publish(BUS, &Envelope { topics, event }).await {
            eprintln!("[chat] publish failed: {e}");
        }
    }

    pub async fn to_user(&self, user: Id, event: Event) {
        self.publish(vec![user_topic(user)], event).await;
    }

    pub async fn to_users(&self, users: &[Id], event: Event) {
        self.publish(users.iter().map(|u| user_topic(*u)).collect(), event).await;
    }
}

/// This instance's live connections, by the topics they hear.
#[derive(Default)]
pub struct Router {
    inner: Mutex<RouterInner>,
}

#[derive(Default)]
struct RouterInner {
    by_topic: HashMap<String, HashSet<Id>>,
    conns: HashMap<Id, (Vec<String>, mpsc::UnboundedSender<Arc<Event>>)>,
}

impl Router {
    /// Register a connection hearing `topics`; events arrive on the receiver.
    pub fn register(&self, conn: Id, topics: Vec<String>) -> mpsc::UnboundedReceiver<Arc<Event>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut inner = self.inner.lock().unwrap();
        for t in &topics {
            inner.by_topic.entry(t.clone()).or_default().insert(conn);
        }
        inner.conns.insert(conn, (topics, tx));
        rx
    }

    /// Change what a connection hears, keeping its receiver.
    pub fn retopic(&self, conn: Id, topics: Vec<String>) {
        let mut inner = self.inner.lock().unwrap();
        let Some((old, tx)) = inner.conns.remove(&conn) else { return };
        for t in &old {
            if let Some(set) = inner.by_topic.get_mut(t) {
                set.remove(&conn);
                if set.is_empty() {
                    inner.by_topic.remove(t);
                }
            }
        }
        for t in &topics {
            inner.by_topic.entry(t.clone()).or_default().insert(conn);
        }
        inner.conns.insert(conn, (topics, tx));
    }

    pub fn unregister(&self, conn: Id) {
        self.retopic(conn, Vec::new());
        self.inner.lock().unwrap().conns.remove(&conn);
    }

    /// Deliver to every connection hearing any of `topics` — once each,
    /// however many of the topics it hears.
    fn route(&self, topics: &[String], event: Arc<Event>) {
        let inner = self.inner.lock().unwrap();
        let mut sent: HashSet<Id> = HashSet::new();
        for t in topics {
            for conn in inner.by_topic.get(t).into_iter().flatten() {
                if sent.insert(*conn) {
                    if let Some((_, tx)) = inner.conns.get(conn) {
                        let _ = tx.send(event.clone());
                    }
                }
            }
        }
    }

    pub fn connections(&self) -> usize {
        self.inner.lock().unwrap().conns.len()
    }
}

// ---------------------------------------------------------------- tickets

type HmacSha256 = Hmac<Sha256>;

/// A live-connection ticket: proof that a session asked to connect, short
/// enough to sit in a WebSocket URL. `t1.<user>.<device|->.<expires ms>.<mac>`.
pub fn mint_ticket(user: Id, device: Option<Id>, now_ms: i64) -> String {
    let body = format!(
        "t1.{user}.{}.{}",
        device.map_or_else(|| "-".to_string(), |d| d.to_string()),
        now_ms + 60_000
    );
    format!("{body}.{}", sign(&app().live_secret, &body))
}

/// The user and device a ticket names, if it is genuine and current.
pub fn check_ticket(secret: &[u8], ticket: &str, now_ms: i64) -> Option<(Id, Option<Id>)> {
    let (body, mac) = ticket.rsplit_once('.')?;
    let expected = sign(secret, body);
    if mac.len() != expected.len() || mac.bytes().zip(expected.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) != 0 {
        return None;
    }
    let mut parts = body.split('.');
    if parts.next()? != "t1" {
        return None;
    }
    let user = Id::parse_str(parts.next()?).ok()?;
    let device = match parts.next()? {
        "-" => None,
        d => Some(Id::parse_str(d).ok()?),
    };
    let expires: i64 = parts.next()?.parse().ok()?;
    (expires > now_ms).then_some((user, device))
}

fn sign(secret: &[u8], body: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC takes any key length");
    mac.update(body.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// ---------------------------------------------------------------- files

/// Object storage: an S3 bucket (MinIO in development). Clients put objects
/// straight into it with a presigned URL, and fetch them through the app's
/// `/f/<key>` route, which redirects to a presigned GET.
pub struct Files {
    pub client: aws_sdk_s3::Client,
    /// The same bucket as clients reach it, when that differs from how the
    /// server does (a container network): presigned URLs are signed for it.
    pub public_client: aws_sdk_s3::Client,
    pub bucket: String,
}

/// The largest attachment accepted.
pub const MAX_UPLOAD: u64 = 50 * 1024 * 1024;
/// The largest avatar or space icon.
pub const MAX_IMAGE: u64 = 8 * 1024 * 1024;

impl Files {
    /// Make the bucket if it is missing — so a fresh MinIO works with no setup.
    pub async fn ensure_bucket(&self) -> Result<(), String> {
        if self.client.head_bucket().bucket(&self.bucket).send().await.is_ok() {
            return Ok(());
        }
        self.client
            .create_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map(|_| ())
            .map_err(|e| format!("could not create bucket {}: {e:?}", self.bucket))
    }

    pub async fn put_url(&self, key: &str, content_type: &str, size: u64) -> Result<String, String> {
        let cfg = aws_sdk_s3::presigning::PresigningConfig::expires_in(Duration::from_secs(600)).map_err(|e| e.to_string())?;
        self.public_client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .content_length(size as i64)
            .presigned(cfg)
            .await
            .map(|p| p.uri().to_string())
            .map_err(|e| e.to_string())
    }

    pub async fn get_url(&self, key: &str, name: &str, inline: bool) -> Result<String, String> {
        let cfg = aws_sdk_s3::presigning::PresigningConfig::expires_in(Duration::from_secs(3600)).map_err(|e| e.to_string())?;
        let disposition = format!(
            "{}; filename=\"{}\"",
            if inline { "inline" } else { "attachment" },
            name.replace(['"', '\\', '\r', '\n'], "_")
        );
        self.public_client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .response_content_disposition(disposition)
            .presigned(cfg)
            .await
            .map(|p| p.uri().to_string())
            .map_err(|e| e.to_string())
    }

    /// The object's size, if it is there.
    pub async fn size(&self, key: &str) -> Option<u64> {
        let head = self.client.head_object().bucket(&self.bucket).key(key).send().await.ok()?;
        head.content_length().map(|n| n.max(0) as u64)
    }

    pub async fn delete(&self, key: &str) {
        let _ = self.client.delete_object().bucket(&self.bucket).key(key).send().await;
    }
}

// ---------------------------------------------------------------- calls

/// The call relay this server issues grants for.
pub struct Calls {
    pub secret: Vec<u8>,
    /// The relay's WebTransport URL, as clients dial it.
    pub relay_url: String,
    /// Its certificate's SHA-256, hex, when self-signed; empty for a CA one.
    pub cert_hash: String,
    /// Hands a revocation token to the in-process relay. A function, so this
    /// crate does not depend on the relay `chat-server` runs.
    pub revoke: Option<Arc<dyn Fn(String) + Send + Sync>>,
}

/// Grants last this long and are renewed at half-life.
pub const GRANT_TTL_MS: u64 = 10 * 60 * 1000;

impl Calls {
    pub fn grant(&self, room: Id, user: Id, now_ms: u64) -> String {
        call_auth::Grant::mint(
            &self.secret,
            &room.simple().to_string(),
            &user.simple().to_string(),
            call_auth::Send::ALL,
            now_ms,
            GRANT_TTL_MS,
        )
    }

    /// Take `user` out of `room` at the relay, now.
    pub fn kick(&self, room: Id, user: Id, now_ms: u64) {
        if let Some(revoke) = &self.revoke {
            let token = call_auth::Revocation::mint(
                &self.secret,
                Some(&room.simple().to_string()),
                &user.simple().to_string(),
                call_auth::Action::Kick,
                now_ms,
            );
            revoke(token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_checks_out_until_it_expires() {
        let secret = b"0123456789abcdef0123456789abcdef";
        let user = Id::from_u128(7);
        let body = format!("t1.{user}.-.{}", 1_000 + 60_000);
        let ticket = format!("{body}.{}", sign(secret, &body));
        assert_eq!(check_ticket(secret, &ticket, 1_000), Some((user, None)));
        assert_eq!(check_ticket(secret, &ticket, 61_001), None, "expired");
        assert_eq!(check_ticket(b"another secret entirely, 32 byt", &ticket, 1_000), None, "wrong key");
        let forged = ticket.replace(&user.to_string(), &Id::from_u128(8).to_string());
        assert_eq!(check_ticket(secret, &forged, 1_000), None, "altered");
    }

    #[test]
    fn a_space_has_one_topic_and_a_gated_channel_its_own() {
        let (a, b) = (Id::from_u128(1), Id::from_u128(2));
        assert_ne!(space_topic(a), space_topic(b));
        assert_ne!(space_topic(a), conversation_topic(b));
    }

    #[test]
    fn the_router_delivers_once_per_connection() {
        let router = Router::default();
        let conn = Id::from_u128(1);
        let mut rx = router.register(conn, vec!["a".into(), "b".into()]);
        router.route(&["a".into(), "b".into()], Arc::new(Event::Resync));
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err(), "heard on two topics, delivered once");
        router.retopic(conn, vec!["c".into()]);
        router.route(&["a".into()], Arc::new(Event::Resync));
        assert!(rx.try_recv().is_err());
        router.route(&["c".into()], Arc::new(Event::Resync));
        assert!(rx.try_recv().is_ok());
    }
}
