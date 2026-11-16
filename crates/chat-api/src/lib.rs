//! Idea Chat's API.
//!
//! Every function a client calls is a `#[server]` function in one of the
//! modules below; [`live::live`] is the one long-lived connection. Built
//! without the `server` feature (as the app builds it), each is an HTTP stub
//! with the same signature minus its extractors. Built with it (as
//! `chat-server` does), each is its real body over `chat-db`, and [`srv`] is
//! the machinery those bodies share.
//!
//! # Errors
//!
//! Every function returns `Result<T, Failure>`: transport trouble and
//! [`ApiError`]s arrive through the same `Result`, and [`describe`] turns
//! either into a sentence for the person.
//!
//! # Who is calling
//!
//! Signed-in functions take `Auth<Principal>`, which a session supplies
//! (cookie on the web, bearer token on native). A principal carries the
//! account's sign-out epoch, and each body checks it ([`srv::me`]), so
//! changing a password or "sign out everywhere" takes effect on the next
//! request.

// The client build strips extractor parameters (`Auth<Principal>`) from
// every stub, which leaves their imports unused there.
#![cfg_attr(not(feature = "server"), allow(unused_imports))]

pub mod auth;
pub mod calls;
pub mod channels;
pub mod files;
pub mod live;
pub mod messages;
pub mod people;
pub mod spaces;

#[cfg(feature = "server")]
pub mod srv;

pub use chat_model as model;

use chat_model::Id;
use serde::{Deserialize, Serialize};
use server::ServerError;



/// What a session stands for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    pub user: Id,
    /// The account's sign-out epoch when the session started.
    pub epoch: i32,
}

/// Why a request was refused, for the person who made it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "why", rename_all = "snake_case")]
pub enum ApiError {
    /// No session, or it was signed out.
    SignedOut,
    /// The account's email is not verified yet.
    Unverified { email: String },
    /// Wrong email or password. Deliberately one case: which was wrong is
    /// not said.
    BadCredentials,
    /// Something given was not acceptable; `field` names the input.
    Invalid { field: Option<String>, message: String },
    NotFound,
    /// Not allowed to do that.
    Forbidden { message: String },
    /// Already exists: an email, a username.
    Taken { message: String },
    /// Too many tries; try again after this long.
    Slow { retry_secs: u64 },
    /// The server could not do it right now.
    Unavailable { message: String },
}

impl ApiError {
    pub fn invalid(field: &str, message: impl Into<String>) -> ApiError {
        ApiError::Invalid { field: Some(field.to_string()), message: message.into() }
    }

    pub fn forbidden(message: impl Into<String>) -> ApiError {
        ApiError::Forbidden { message: message.into() }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::SignedOut => f.write_str("You're signed out."),
            ApiError::Unverified { .. } => f.write_str("Verify your email to continue."),
            ApiError::BadCredentials => f.write_str("That email and password don't match."),
            ApiError::Invalid { message, .. }
            | ApiError::Forbidden { message }
            | ApiError::Taken { message }
            | ApiError::Unavailable { message } => f.write_str(message),
            ApiError::NotFound => f.write_str("That isn't there any more."),
            ApiError::Slow { retry_secs } => write!(f, "Too many tries. Try again in {retry_secs} seconds."),
        }
    }
}

/// What every API function can fail with.
pub type Failure = ServerError<ApiError>;

/// What every API function returns.
pub type Answer<T> = Result<T, Failure>;

/// A failure as a sentence for the person.
pub fn describe(e: &Failure) -> String {
    match e {
        ServerError::Failed(api) => api.to_string(),
        ServerError::Network(_) => "Can't reach the server. Check your connection.".into(),
        ServerError::Server { status: 401, .. } => ApiError::SignedOut.to_string(),
        ServerError::Server { status: 429, .. } => "Slow down a little and try again.".into(),
        ServerError::IncompatibleVersion { .. } => "The app was updated. Reload to continue.".into(),
        ServerError::Cancelled => "Cancelled.".into(),
        other => format!("Something went wrong ({other})."),
    }
}

/// The input a failure is about, if it is about one.
pub fn failed_field(e: &Failure) -> Option<&str> {
    match e {
        ServerError::Failed(ApiError::Invalid { field: Some(f), .. }) => Some(f),
        _ => None,
    }
}

/// Whether a failure means the session is gone.
pub fn is_signed_out(e: &Failure) -> bool {
    matches!(e, ServerError::Failed(ApiError::SignedOut) | ServerError::Server { status: 401, .. })
}
