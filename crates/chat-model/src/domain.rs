//! The nouns: people, spaces, channels, messages and calls, as both sides see
//! them on the wire.
//!
//! These are view types, not rows. A [`Space`] carries its members and
//! categories because every client that can see a space needs both to draw
//! it; a [`Message`] carries its reactions and thread summary because a row
//! is drawn from one value. The database is free to store them however it
//! likes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::expressions::{Emoji, Sticker};

use crate::{Id, Millis};

/// The permission bitset [`crate::perms`] decides against: `Role`'s `permissions`
/// field and `Channel`'s `view` and `post` masks are all of these.
pub type PermissionSet = u64;

/// Someone with an account, as anyone who can see them sees them. Email and
/// everything else private lives on [`Me`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: Id,
    /// Unique, lowercase; what `@` mentions are typed with.
    pub handle: String,
    /// What people are shown as. Free text within [`crate::text::DISPLAY_NAME_MAX`].
    pub display_name: String,
    /// An uploaded picture's URL; `None` draws initials.
    pub avatar: Option<String>,
    pub bio: String,
    /// The id of a cosmetic the app draws; `None` is none.
    #[serde(default)]
    pub avatar_decoration: Option<String>,
    /// The id of a cosmetic the app draws; `None` is none.
    #[serde(default)]
    pub profile_effect: Option<String>,
}

impl User {
    /// The two letters an avatar without a picture shows.
    pub fn initials(&self) -> String {
        let mut words = self.display_name.split_whitespace();
        let first = words.next().and_then(|w| w.chars().next());
        let second = words.last().and_then(|w| w.chars().next());
        match (first, second) {
            (Some(a), Some(b)) => [a, b].iter().flat_map(|c| c.to_uppercase()).collect(),
            (Some(a), None) => a.to_uppercase().collect(),
            _ => self.handle.chars().take(2).flat_map(|c| c.to_uppercase()).collect(),
        }
    }
}

/// The signed-in account, with what only its owner sees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Me {
    pub user: User,
    pub email: String,
    /// What the account chose to show; [`Presence::Offline`] here means
    /// "appear offline", not "is offline".
    pub status: Status,
}

/// Whether someone is around, as others see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    Online,
    /// Connected, but nothing done for a while.
    Idle,
    /// Do not disturb: connected, notifications held.
    Busy,
    #[default]
    Offline,
}

/// The status someone picked: a presence to show while connected, and an
/// optional line of text.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Status {
    pub presence: Presence,
    pub text: Option<String>,
}

/// One named bundle of what people may do in a space. `permissions` is a bitset
/// over [`crate::perms::Permission`]; `position` orders the role list. The
/// owner is not a role: ownership is its own fact on the space and carries
/// every permission whatever roles the owner also holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Role {
    pub id: Id,
    pub name: String,
    pub permissions: u64,
    pub position: i32,
    /// A display colour, 0xRRGGBB; `None` draws the role in the plain role
    /// colour. Absent from older snapshots, hence the serde default.
    #[serde(default)]
    pub colour: Option<u32>,
    /// Whether `@RoleName` pings its holders. Absent from older snapshots,
    /// hence the serde default.
    #[serde(default)]
    pub mentionable: bool,
}

impl Role {
    pub fn label(self) -> String {
        self.name.clone()
    }
}

/// A person's membership in a space: the roles they hold. Their permission set
/// is the union of the roles' sets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub roles: Vec<Id>,
    pub joined_at: Millis,
}

/// A group of channels in a space's navigation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    pub id: Id,
    pub name: String,
    pub position: i32,
}

/// A community: its people, the roles it hands out, and how its channels are
/// grouped. The channels themselves are kept beside spaces in
/// [`crate::live::State::channels`], so a direct conversation and a space
/// channel are looked up the same way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    pub id: Id,
    pub name: String,
    pub icon: Option<String>,
    pub owner: Id,
    pub members: BTreeMap<Id, Member>,
    /// The space's roles by id; a member's `roles` point into this.
    pub roles: BTreeMap<Id, Role>,
    /// The space's baseline: what every member holds whatever their roles say.
    /// Seeded with [`DEFAULT_EVERYONE`] when the space is made, kept per space
    /// so an owner can change it.
    pub everyone: PermissionSet,
    pub categories: Vec<Category>,
    pub created_at: Millis,
    /// The space's custom emoji by id, and its stickers: metadata in the
    /// clear, so an older snapshot simply decodes with none.
    #[serde(default)]
    pub emoji: BTreeMap<Id, Emoji>,
    #[serde(default)]
    pub stickers: BTreeMap<Id, Sticker>,
}

impl Space {
    /// The ids of the roles `user` holds here, if they are a member at all.
    pub fn role_ids(&self, user: Id) -> Option<Vec<Id>> {
        self.members.get(&user).map(|m| m.roles.clone())
    }
}

/// What a channel is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    /// Messages, in a space.
    Text,
    /// A standing call in a space. No messages.
    Voice,
    /// A conversation branched from a text channel; messages. Threads are a
    /// channel kind rather than a feature of messages, so everything a text
    /// channel can do, one can.
    Thread,
    /// Two people, outside any space.
    Direct,
    /// A few people, outside any space.
    Group,
}

impl ChannelKind {
    /// Whether people post messages here.
    pub fn has_messages(self) -> bool {
        !matches!(self, ChannelKind::Voice)
    }

    /// Whether a call can happen here: a voice channel always, a direct or
    /// group conversation when someone starts one.
    pub fn has_calls(self) -> bool {
        matches!(self, ChannelKind::Voice | ChannelKind::Direct | ChannelKind::Group)
    }

    /// A direct or group conversation, which lives under Home.
    pub fn is_private(self) -> bool {
        matches!(self, ChannelKind::Direct | ChannelKind::Group)
    }
}

/// A place people talk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub id: Id,
    pub kind: ChannelKind,
    /// The space it belongs to; `None` for direct and group conversations.
    pub space: Option<Id>,
    /// Empty for a direct conversation, which is named after the other person.
    pub name: String,
    pub topic: String,
    pub category: Option<Id>,
    pub position: i32,
    /// A thread's channel.
    pub parent: Option<Id>,
    /// The message a thread grew from, shown at its top.
    pub starter: Option<Id>,
    /// The permission mask a member's set must cover to see it. `0` asks
    /// everyone in the space; [`crate::perms::ADMIN`] asks an admin.
    pub view: u64,
    /// The extra permission mask a set must cover to post. A thread ignores
    /// these; its parent's apply.
    pub post: u64,
    /// Who is in a direct or group conversation. Empty for space channels,
    /// whose audience is the space.
    pub members: Vec<Id>,
    /// A thread nobody can post in any more; it leaves the navigation.
    pub archived: bool,
    /// Per-role allow and deny sets on this channel, keyed by role: each
    /// widens and then narrows what a holder of that role may do here. The
    /// owner and anyone holding IsAdmin ignore them.
    pub overwrites: BTreeMap<Id, Overwrite>,
    /// The newest message, if any — what unread is measured against.
    pub last_message: Option<Id>,
    /// How many messages it holds. Shown for threads.
    pub message_count: u32,
    pub created_by: Id,
    pub created_at: Millis,
}

/// What one role switches on a channel: `allow` grants on top of the base
/// set, `deny` takes away afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overwrite {
    pub allow: PermissionSet,
    pub deny: PermissionSet,
}

/// How far someone has read in a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReadState {
    /// The newest message they have seen.
    pub last_read: Option<Id>,
    /// Messages since then that name them. In a direct conversation, every
    /// message counts.
    pub mentions: u32,
}

/// How two people relate, from the viewer's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    Friend,
    /// They asked; the viewer has not answered.
    Incoming,
    /// The viewer asked; they have not answered.
    Outgoing,
    /// The viewer blocked them. They are not told.
    Blocked,
}

/// A file attached to a message, or anything else uploaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: Id,
    pub name: String,
    pub size: u64,
    pub content_type: String,
    /// Where it is fetched from. Stable for the attachment's life.
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.content_type.starts_with("image/")
    }
}

/// One emoji's reactions on a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reaction {
    pub emoji: String,
    /// Who reacted with it, in the order they did.
    pub users: Vec<Id>,
}

/// The message a reply answers, as much of it as the reply shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyTo {
    pub id: Id,
    pub author: Id,
    /// The start of its text, or empty if it is gone.
    pub excerpt: String,
}

/// A thread grown from a message, as the message shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadLink {
    pub id: Id,
    pub name: String,
    pub message_count: u32,
    /// The thread's newest message — which also makes counting a message
    /// into the summary idempotent.
    pub last_message: Option<Id>,
    pub last_at: Option<Millis>,
}

/// A call in a direct or group conversation, as its record in the stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallRecord {
    pub started_by: Id,
    pub started_at: Millis,
    /// `None` while it is still going.
    pub ended_at: Option<Millis>,
    /// Everyone who was in it at any point.
    pub participants: Vec<Id>,
}

impl CallRecord {
    /// Nobody but the caller ever joined.
    pub fn missed(&self) -> bool {
        self.ended_at.is_some() && self.participants.iter().all(|p| *p == self.started_by)
    }
}

/// What kind of entry a message is. Most are someone talking; the rest are
/// the stream noting something that happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum MessageKind {
    Text,
    /// A call happened (or is happening) here.
    Call(CallRecord),
    /// The author joined the space.
    Joined,
    /// The author started the named thread.
    ThreadStarted { thread: Id },
}

/// One entry in a channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: Id,
    pub channel: Id,
    pub author: Id,
    pub kind: MessageKind,
    /// The text, with mentions written as tokens; see [`crate::text`].
    pub body: String,
    pub attachments: Vec<Attachment>,
    pub reply_to: Option<ReplyTo>,
    pub reactions: Vec<Reaction>,
    pub thread: Option<ThreadLink>,
    /// People the body names.
    pub mentions: Vec<Id>,
    /// Whether it says `@everyone`.
    pub mentions_everyone: bool,
    /// A sticker of the space sent in place of a body; `None` is plain text.
    #[serde(default)]
    pub sticker: Option<Id>,

    /// Roles the body names. Absent from older snapshots, hence the serde
    /// default.
    #[serde(default)]
    pub mention_roles: Vec<Id>,
    pub at: Millis,
    pub edited_at: Option<Millis>,
}

impl Message {
    /// Whether the message names `user`: directly, through `@everyone`, or
    /// through a role in `roles` — the ids `user` holds at the time the
    /// message is read, so a role mention reaches whoever holds the role
    /// then. A conversation passes no roles; a deleted role's id matches
    /// nobody, which leaves its old mentions as plain text.
    pub fn mentions_user(&self, user: Id, roles: &[Id]) -> bool {
        self.mentions_everyone || self.mentions.contains(&user)
            || self.mention_roles.iter().any(|r| roles.contains(r))
    }
}

/// Someone in a call, and what they are sending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceState {
    pub user: Id,
    pub channel: Id,
    pub muted: bool,
    /// Hearing nothing (and so, by convention, also muted).
    pub deafened: bool,
    pub camera: bool,
    pub screen: bool,
    pub joined_at: Millis,
}

/// A call someone started in a direct or group conversation, while it rings
/// or runs. Voice channels have none: they are always open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ring {
    /// The call's instance; see [`CallTicket::room`].
    pub call: Id,
    pub channel: Id,
    pub started_by: Id,
    pub started_at: Millis,
    /// Who is still being rung: not yet joined, declined or timed out.
    pub ringing: Vec<Id>,
}

/// What one notification row's `scope` column says: the target is a space, or
/// a channel inside one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    Space,
    Channel,
}

impl Scope {
    pub fn db_str(self) -> &'static str {
        match self {
            Scope::Space => "space",
            Scope::Channel => "channel",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        [Scope::Space, Scope::Channel].into_iter().find(|l| l.db_str() == s)
    }
}

/// What one target's notification row says in the db: `all` shows everything,
/// `mentions` hides the unread dot but keeps the mention badge, `nothing`
/// hides both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Level {
    All,
    Mentions,
    Nothing,
}

impl Level {
    pub fn db_str(self) -> &'static str {
        match self {
            Level::All => "all",
            Level::Mentions => "mentions",
            Level::Nothing => "nothing",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        [Level::All, Level::Mentions, Level::Nothing].into_iter().find(|l| l.db_str() == s)
    }
}

/// One space's or channel's notification setting for one person. `level` of
/// `None` inherits: a channel from its space, a space the default of showing
/// everything. `muted_until` is `None` (not muted), [`MUTED_UNTIL_TURNED_OFF`]
/// (a mute with no end), or an epoch-ms deadline.
pub const MUTED_UNTIL_TURNED_OFF: i64 = -1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationSetting {
    /// Whether `target` is a space or a channel, as the db's `scope` column.
    pub scope: Scope,
    pub target: Id,
    pub level: Option<Level>,
    pub muted_until: Option<i64>,
}

/// Whether a mute is in force at `now_ms`.
pub fn is_muted(muted_until: Option<i64>, now_ms: Millis) -> bool {
    match muted_until {
        Some(MUTED_UNTIL_TURNED_OFF) => true,
        Some(until) => until > now_ms,
        None => false,
    }
}

/// A storable mute end: `None`, the no-end marker, or a positive deadline. A
/// zero or negative one would never read as muted, so the verb refuses it.
pub fn mute_end_ok(muted_until: Option<i64>) -> bool {
    !matches!(muted_until, Some(t) if t != MUTED_UNTIL_TURNED_OFF && t <= 0)
}

/// One of an account's devices, and the key calls are sealed to it with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: Id,
    pub name: String,
    /// The device's X25519 public key, base64url without padding.
    pub public_key: String,
    pub created_at: Millis,
    pub last_seen: Millis,
}

/// An invitation into a space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub code: String,
    pub space: Id,
    pub created_by: Id,
    pub uses: u32,
    pub max_uses: Option<u32>,
    pub expires_at: Option<Millis>,
    pub created_at: Millis,
}

/// What someone holding an invite code may see before accepting it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvitePreview {
    pub code: String,
    pub space: Id,
    pub space_name: String,
    pub space_icon: Option<String>,
    pub members: u32,
    pub inviter: Option<User>,
    /// Already a member: accepting just opens the space.
    pub joined: bool,
}

/// What the server hands a client to join a call: where the relay is, the
/// grant it checks, and how to get the room's end-to-end key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallTicket {
    /// The channel the call is in.
    pub channel: Id,
    /// This call's instance — the relay room. A new one each time a call
    /// starts in an empty channel, which is also when its key is new.
    pub room: Id,
    /// The relay, as a WebTransport URL.
    pub relay_url: String,
    /// The relay certificate's SHA-256, hex; empty when it has a CA-signed
    /// certificate.
    pub cert_hash: String,
    /// A `call-auth` grant for the room.
    pub grant: String,
    /// Whether this device must make the room key: it is the first in. If
    /// not, it asks the devices already there for it.
    pub make_key: bool,
}

/// A sealed copy of a call's room key, for one device, from another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedKey {
    pub room: Id,
    pub from_device: Id,
    /// The sender device's public key, so the receiver can open it — and
    /// check it against the directory.
    pub from_key: String,
    pub to_device: Id,
    /// `chat_keys::Sealed`, encoded.
    pub sealed: String,
}

/// A device asking for a call's room key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRequest {
    pub room: Id,
    pub user: Id,
    pub device: Id,
    pub public_key: String,
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_json_without_the_cosmetics_decodes_as_none() {
        let u: User = serde_json::from_str(
            r#"{"id":"00000000-0000-0000-0000-000000000001","handle":"ana","display_name":"Ana","avatar":null,"bio":"hi"}"#,
        )
        .unwrap();
        assert_eq!(u.avatar_decoration, None);
        assert_eq!(u.profile_effect, None);
    }

    #[test]
    fn the_cosmetics_round_trip() {
        let u = User {
            id: Id::from_u128(1),
            handle: "ana".into(),
            display_name: "Ana".into(),
            avatar: None,
            bio: String::new(),
            avatar_decoration: Some("sparkle".into()),
            profile_effect: Some("glow".into()),
        };
        let back: User = serde_json::from_str(&serde_json::to_string(&u).unwrap()).unwrap();
        assert_eq!(back, u);
    }
}
