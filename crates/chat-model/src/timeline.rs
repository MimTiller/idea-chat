//! One channel's loaded messages.
//!
//! A client keeps a [`Timeline`] per channel it has opened: a contiguous
//! window of history, in id (and so time) order, that grows backwards as the
//! person scrolls up and forwards as messages arrive. The live connection's
//! message events fold into every timeline they concern through
//! [`Timeline::apply`] — idempotently, like the live state.
//!
//! A window need not reach the present: jumping to a search result loads the
//! history around it. Such a window ignores new messages (they are not next
//! to anything it holds) until it is paged forward to the present again.

use serde::{Deserialize, Serialize};

use crate::live::Event;
use crate::{Channel, Id, Message, Millis, Reaction};

/// A page of history, as the server returns it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Page {
    /// Oldest first.
    pub messages: Vec<Message>,
    /// There is history before the first message.
    pub older: bool,
    /// The last message is the channel's newest.
    pub present: bool,
}

/// A loaded window of a channel's history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    pub channel: Id,
    /// Oldest first, no duplicates.
    pub messages: Vec<Message>,
    /// There is history before the window.
    pub older: bool,
    /// The window reaches the present, so new messages belong at its end.
    pub present: bool,
}

/// How many minutes apart two messages from the same person may be and
/// still be drawn as one group.
pub const GROUP_MINUTES: i64 = 7;

impl Timeline {
    pub fn new(channel: Id, page: Page) -> Timeline {
        let mut t = Timeline { channel, messages: Vec::new(), older: page.older, present: page.present };
        for m in page.messages {
            t.insert(m);
        }
        t
    }

    /// The oldest loaded message, to page back from.
    pub fn first(&self) -> Option<Id> {
        self.messages.first().map(|m| m.id)
    }

    /// The newest loaded message, to page forward from.
    pub fn last(&self) -> Option<Id> {
        self.messages.last().map(|m| m.id)
    }

    /// Add a page of older history in front.
    pub fn prepend(&mut self, page: Page) {
        self.older = page.older;
        for m in page.messages {
            self.insert(m);
        }
    }

    /// Add a page of newer history behind.
    pub fn append(&mut self, page: Page) {
        self.present = page.present;
        for m in page.messages {
            self.insert(m);
        }
    }

    pub fn get(&self, id: Id) -> Option<&Message> {
        self.index(id).map(|i| &self.messages[i])
    }

    fn index(&self, id: Id) -> Option<usize> {
        self.messages.binary_search_by(|m| m.id.cmp(&id)).ok()
    }

    /// Put a message in its place, replacing any copy already there.
    fn insert(&mut self, m: Message) {
        match self.messages.binary_search_by(|x| x.id.cmp(&m.id)) {
            Ok(i) => self.messages[i] = m,
            Err(i) => self.messages.insert(i, m),
        }
    }

    /// Fold one live event in. Returns whether the window changed.
    pub fn apply(&mut self, event: &Event) -> bool {
        match event {
            Event::MessageCreated { message } if message.channel == self.channel => {
                if !self.present || self.index(message.id).is_some() {
                    return false;
                }
                self.insert(message.clone());
                true
            }
            // A message in a thread grown from one of ours: the starter's
            // summary counts it.
            Event::MessageCreated { message } => {
                let Some(m) = self.messages.iter_mut().find(|m| m.thread.as_ref().is_some_and(|t| t.id == message.channel))
                else {
                    return false;
                };
                let link = m.thread.as_mut().expect("found by its thread");
                if link.last_message.is_some_and(|last| message.id <= last) {
                    return false;
                }
                link.message_count += 1;
                link.last_message = Some(message.id);
                link.last_at = Some(message.at);
                true
            }
            Event::MessageEdited { message } if message.channel == self.channel => {
                let Some(i) = self.index(message.id) else { return false };
                if self.messages[i] == *message {
                    return false;
                }
                self.messages[i] = message.clone();
                true
            }
            Event::MessageDeleted { channel, message } if *channel == self.channel => {
                let Some(i) = self.index(*message) else { return false };
                self.messages.remove(i);
                // Replies keep their place but lose the quote.
                for m in self.messages.iter_mut() {
                    if let Some(r) = m.reply_to.as_mut().filter(|r| r.id == *message) {
                        r.excerpt.clear();
                    }
                }
                true
            }
            Event::ReactionChanged { channel, message, emoji, user, added } if *channel == self.channel => {
                let Some(i) = self.index(*message) else { return false };
                react(&mut self.messages[i].reactions, emoji, *user, *added)
            }
            // A thread renamed: its starter's summary follows.
            Event::ChannelUpserted { channel } => self.thread_renamed(channel),
            _ => false,
        }
    }

    fn thread_renamed(&mut self, channel: &Channel) -> bool {
        let Some(starter) = channel.starter else { return false };
        let Some(i) = self.index(starter) else { return false };
        match self.messages[i].thread.as_mut() {
            Some(link) if link.id == channel.id && link.name != channel.name => {
                link.name = channel.name.clone();
                true
            }
            _ => false,
        }
    }

    /// The window as rows to draw: a divider at each new day, a "new" marker
    /// before the first message after `last_read`, and each message marked as
    /// continuing the one above when the same person sent both within
    /// [`GROUP_MINUTES`] on the same day. Days are local to `utc_offset_min`.
    pub fn rows(&self, last_read: Option<Id>, me: Option<Id>, utc_offset_min: i32) -> Vec<Row> {
        let mut rows = Vec::with_capacity(self.messages.len() + 4);
        let mut prev: Option<&Message> = None;
        let mut marked = false;
        for m in &self.messages {
            let day = local_day(m.at, utc_offset_min);
            let new_day = prev.is_none_or(|p| local_day(p.at, utc_offset_min) != day);
            if new_day {
                rows.push(Row::Day { day_start: day * DAY_MS - offset_ms(utc_offset_min) });
            }
            let unread = !marked && last_read.is_some_and(|r| m.id > r) && Some(m.author) != me;
            if unread {
                rows.push(Row::New);
                marked = true;
            }
            let continued = !new_day
                && !unread
                && matches!(m.kind, crate::MessageKind::Text)
                && m.reply_to.is_none()
                && prev.is_some_and(|p| {
                    p.author == m.author
                        && matches!(p.kind, crate::MessageKind::Text)
                        && m.at - p.at <= GROUP_MINUTES * 60_000
                });
            rows.push(Row::Message { id: m.id, continued });
            prev = Some(m);
        }
        rows
    }
}

/// Add or take away `user`'s `emoji` reaction. Returns whether it changed.
pub fn react(reactions: &mut Vec<Reaction>, emoji: &str, user: Id, added: bool) -> bool {
    match reactions.iter().position(|r| r.emoji == emoji) {
        Some(i) => {
            let r = &mut reactions[i];
            let had = r.users.contains(&user);
            if added == had {
                return false;
            }
            if added {
                r.users.push(user);
            } else {
                r.users.retain(|u| *u != user);
                if r.users.is_empty() {
                    reactions.remove(i);
                }
            }
            true
        }
        None if added => {
            reactions.push(Reaction { emoji: emoji.to_string(), users: vec![user] });
            true
        }
        None => false,
    }
}

const DAY_MS: i64 = 86_400_000;

fn offset_ms(utc_offset_min: i32) -> i64 {
    utc_offset_min as i64 * 60_000
}

fn local_day(at: Millis, utc_offset_min: i32) -> i64 {
    (at + offset_ms(utc_offset_min)).div_euclid(DAY_MS)
}

/// One row of a drawn timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Row {
    /// A day divider; `day_start` is that local midnight, as UTC millis.
    Day { day_start: Millis },
    /// Where unread messages begin.
    New,
    Message { id: Id, continued: bool },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MessageKind, ThreadLink};

    fn id(n: u128) -> Id {
        Id::from_u128(n)
    }

    fn msg(n: u128, author: u128, at: Millis) -> Message {
        Message {
            id: id(n),
            channel: id(1),
            author: id(author),
            kind: MessageKind::Text,
            body: format!("m{n}"),
            attachments: vec![],
            reply_to: None,
            reactions: vec![],
            thread: None,
            mentions: vec![],
            mentions_everyone: false,
            sticker: None,
            at,
            edited_at: None,
        }
    }

    fn live(messages: Vec<Message>) -> Timeline {
        Timeline::new(id(1), Page { messages, older: true, present: true })
    }

    #[test]
    fn pages_merge_in_order_without_duplicates() {
        let mut t = live(vec![msg(5, 1, 0), msg(6, 1, 0)]);
        t.prepend(Page { messages: vec![msg(3, 1, 0), msg(5, 1, 0)], older: false, present: false });
        let ids: Vec<Id> = t.messages.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![id(3), id(5), id(6)]);
        assert!(!t.older);
    }

    #[test]
    fn a_new_message_appends_once() {
        let mut t = live(vec![msg(5, 1, 0)]);
        let e = Event::MessageCreated { message: msg(6, 2, 0) };
        assert!(t.apply(&e));
        assert!(!t.apply(&e));
        assert_eq!(t.messages.len(), 2);
    }

    #[test]
    fn a_window_away_from_the_present_ignores_new_messages() {
        let mut t = Timeline::new(id(1), Page { messages: vec![msg(5, 1, 0)], older: true, present: false });
        assert!(!t.apply(&Event::MessageCreated { message: msg(9, 2, 0) }));
    }

    #[test]
    fn edits_replace_and_deletes_clear_reply_quotes() {
        let mut t = live(vec![msg(5, 1, 0), msg(6, 2, 0)]);
        t.messages[1].reply_to = Some(crate::ReplyTo { id: id(5), author: id(1), excerpt: "m5".into() });
        let mut edited = msg(5, 1, 0);
        edited.body = "changed".into();
        assert!(t.apply(&Event::MessageEdited { message: edited }));
        assert_eq!(t.messages[0].body, "changed");
        assert!(t.apply(&Event::MessageDeleted { channel: id(1), message: id(5) }));
        assert_eq!(t.messages.len(), 1);
        assert_eq!(t.messages[0].reply_to.as_ref().unwrap().excerpt, "");
    }

    #[test]
    fn reactions_are_idempotent_and_empty_ones_disappear() {
        let mut t = live(vec![msg(5, 1, 0)]);
        let react = |added| Event::ReactionChanged { channel: id(1), message: id(5), emoji: "👍".into(), user: id(2), added };
        assert!(t.apply(&react(true)));
        assert!(!t.apply(&react(true)));
        assert_eq!(t.messages[0].reactions[0].users, vec![id(2)]);
        assert!(t.apply(&react(false)));
        assert!(t.messages[0].reactions.is_empty());
    }

    #[test]
    fn a_thread_message_updates_its_starter_once() {
        let mut starter = msg(5, 1, 0);
        starter.thread =
            Some(ThreadLink { id: id(77), name: "t".into(), message_count: 0, last_message: None, last_at: None });
        let mut t = live(vec![starter]);
        let mut reply = msg(80, 2, 1234);
        reply.channel = id(77);
        let e = Event::MessageCreated { message: reply };
        assert!(t.apply(&e));
        assert!(!t.apply(&e));
        let link = t.messages[0].thread.as_ref().unwrap();
        assert_eq!((link.message_count, link.last_at), (1, Some(1234)));
    }

    #[test]
    fn rows_group_by_author_and_time_and_mark_days_and_new() {
        let min = 60_000;
        let t = live(vec![
            msg(1, 1, 0),
            msg(2, 1, 2 * min),               // same author, close: continued
            msg(3, 1, 20 * min),              // same author, too late: new group
            msg(4, 2, 21 * min),              // other author
            msg(5, 2, DAY_MS + 22 * min),     // next day
        ]);
        let rows = t.rows(Some(id(3)), Some(id(1)), 0);
        assert_eq!(
            rows,
            vec![
                Row::Day { day_start: 0 },
                Row::Message { id: id(1), continued: false },
                Row::Message { id: id(2), continued: true },
                Row::Message { id: id(3), continued: false },
                Row::New,
                Row::Message { id: id(4), continued: false },
                Row::Day { day_start: DAY_MS },
                Row::Message { id: id(5), continued: false },
            ]
        );
    }

    #[test]
    fn your_own_messages_never_start_the_new_marker() {
        let t = live(vec![msg(1, 1, 0), msg(2, 2, 0)]);
        let rows = t.rows(Some(id(0)), Some(id(1)), 0);
        assert_eq!(rows[1], Row::Message { id: id(1), continued: false });
        assert_eq!(rows[2], Row::New);
    }

    #[test]
    fn days_follow_the_local_offset() {
        // 23:30 UTC is the next day at UTC+1.
        let at = DAY_MS - 30 * 60_000;
        let t = live(vec![msg(1, 1, at)]);
        assert_eq!(t.rows(None, None, 60)[0], Row::Day { day_start: DAY_MS - 3_600_000 });
    }
}
