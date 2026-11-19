//! Idea Chat's shared vocabulary.
//!
//! Everything both halves of the app must agree on lives here, and nothing
//! here does IO:
//!
//! - [`domain`] — people, spaces, channels, messages, calls: the nouns.
//! - [`live`] — the live world a connection is kept in step with: one
//!   [`live::State`] per viewer, the [`live::Event`]s that change it, and the
//!   pure [`live::Live::apply`] that the server and every client run alike
//!   (an [`idea_state::Model`]).
//! - [`timeline`] — one channel's loaded window of messages, and how message
//!   events fold into it. Messages are not part of the live state: a client
//!   holds only the channels it has opened, and pages history on demand.
//! - [`text`] — what a message body may contain, how mentions are written
//!   down, and the rules for names.
//! - [`expressions`] — the space's custom emoji and stickers, and their rules.
//! - [`perms`] — who may see, post in and manage what.
//!
//! # Identifiers and time
//!
//! Ids are UUIDv7, minted by the server: they sort by creation time, so "is
//! this message newer than what I last read" is an id comparison, and a
//! channel's history pages by id. Time is milliseconds since the Unix epoch
//! ([`Millis`]) — the one representation every target has without a clock
//! crate.

pub mod domain;
pub mod expressions;
pub mod live;
pub mod perms;
pub mod text;
pub mod timeline;

pub use domain::*;

/// Every entity's id: a UUIDv7 the server assigned.
pub type Id = uuid::Uuid;

/// Milliseconds since the Unix epoch, UTC.
pub type Millis = i64;
