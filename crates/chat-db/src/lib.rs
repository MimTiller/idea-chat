//! Idea Chat's data layer.
//!
//! Owns the schema (`migrations/`) and every query the API makes, returning
//! `chat-model` types so the API never sees a row. Server-only.
//!
//! # Runtime-checked queries
//!
//! Queries use `query_as` with `FromRow` structs rather than the `query!`
//! macros, so building the server does not need a live database or a
//! committed query cache. What that gives up is caught by `tests/`, which run
//! every query against a real Postgres and also cover what a macro cannot:
//! constraints, transactions, and races such as two people redeeming the
//! last use of an invite.
//!
//! ```sh
//! CHAT_TEST_DATABASE_URL=postgres://postgres:dev@localhost:55433/postgres \
//!   cargo test -p chat-db
//! ```
//!
//! Each test makes its own database from that server, so they run in
//! parallel and leave nothing behind but the databases' names.
//!
//! # Authorization is not here
//!
//! These functions do what they are told. Whether the caller may — is a
//! member, holds the role, can see the channel — is the API's question,
//! answered with `chat_model::perms` before it calls in. The one exception is
//! visibility in [`Db::snapshot`], which is what decides what a viewer is
//! shown at all.

mod accounts;
mod calls;
mod channels;
mod messages;
mod people;
mod snapshot;
mod spaces;

pub use accounts::{dummy_password_check, Account, CodeCheck, CodePurpose, NewAccount};
pub use calls::{JoinedCall, LeftCall};
pub use channels::{Audience, ChannelInit, GROUP_MAX};
pub use messages::{file_url, NewMessage, Posted, SearchHit, Upload};
pub use people::RelationChange;
pub use spaces::{InviteOutcome, NewSpace};

use chat_model::{ChannelKind, Id, Millis, Presence, Relation};
use sqlx::postgres::{PgPool, PgPoolOptions};
use time::OffsetDateTime;

/// What went wrong, in terms the API maps to a response.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found")]
    NotFound,
    /// A uniqueness rule: the email, the handle, the friendship, the name.
    #[error("{0} is already taken")]
    Taken(&'static str),
    /// A rule of the data itself: an invite used up, a conversation full.
    #[error("{0}")]
    Refused(String),
    #[error(transparent)]
    Sql(#[from] sqlx::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The database. Cheap to clone; a pool inside.
#[derive(Clone, Debug)]
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// Connect and bring the schema up to date.
    pub async fn connect(url: &str) -> Result<Db> {
        let pool = PgPoolOptions::new().max_connections(20).connect(url).await?;
        sqlx::migrate!("./migrations").run(&pool).await.map_err(|e| Error::Sql(e.into()))?;
        Ok(Db { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

/// A fresh time-ordered id.
pub fn new_id() -> Id {
    uuid::Uuid::now_v7()
}

pub(crate) fn millis(t: OffsetDateTime) -> Millis {
    (t.unix_timestamp_nanos() / 1_000_000) as Millis
}

pub(crate) fn opt_millis(t: Option<OffsetDateTime>) -> Option<Millis> {
    t.map(millis)
}

pub(crate) fn kind_text(k: ChannelKind) -> &'static str {
    match k {
        ChannelKind::Text => "text",
        ChannelKind::Voice => "voice",
        ChannelKind::Thread => "thread",
        ChannelKind::Direct => "direct",
        ChannelKind::Group => "group",
    }
}

pub(crate) fn kind_of(s: &str) -> ChannelKind {
    match s {
        "voice" => ChannelKind::Voice,
        "thread" => ChannelKind::Thread,
        "direct" => ChannelKind::Direct,
        "group" => ChannelKind::Group,
        _ => ChannelKind::Text,
    }
}

pub(crate) fn presence_text(p: Presence) -> &'static str {
    match p {
        Presence::Online => "online",
        Presence::Idle => "idle",
        Presence::Busy => "busy",
        Presence::Offline => "offline",
    }
}

pub(crate) fn presence_of(s: &str) -> Presence {
    match s {
        "idle" => Presence::Idle,
        "busy" => Presence::Busy,
        "offline" => Presence::Offline,
        _ => Presence::Online,
    }
}

pub(crate) fn relation_text(r: Relation) -> &'static str {
    match r {
        Relation::Friend => "friend",
        Relation::Incoming => "incoming",
        Relation::Outgoing => "outgoing",
        Relation::Blocked => "blocked",
    }
}

pub(crate) fn relation_of(s: &str) -> Relation {
    match s {
        "friend" => Relation::Friend,
        "incoming" => Relation::Incoming,
        "outgoing" => Relation::Outgoing,
        _ => Relation::Blocked,
    }
}

/// Map a unique-violation on `constraint` to [`Error::Taken`].
pub(crate) fn taken(e: sqlx::Error, constraint: &str, what: &'static str) -> Error {
    match &e {
        sqlx::Error::Database(d) if d.constraint() == Some(constraint) => Error::Taken(what),
        _ => Error::Sql(e),
    }
}
