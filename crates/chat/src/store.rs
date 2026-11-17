//! The client's state: what the live connection keeps in step, the message
//! timelines of the channels opened, who is typing, and the UI's own state.
//!
//! # Reading it
//!
//! The live state and the timelines are plain values behind a `RefCell`,
//! each with a revision signal bumped on every change. Reading through
//! [`Store::read`] / [`Store::timeline`] subscribes to the revision, so a
//! `memo` or `rx!` over a read recomputes when the state moves — and, being
//! a memo, only notifies its own readers when its answer changed. That keeps
//! an event from cloning the whole world into a new signal value, while
//! every view still sees exactly what it reads.
//!
//! # Where it lives
//!
//! One [`Store`] per signed-in session, provided at the root
//! ([`Store::provide`]) and fetched anywhere below with [`store()`].

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use chat_api::live::{Down, Up};
use chat_api::messages::Window;
use chat_model::live::{Action, Event, State};
use chat_model::perms;
use chat_model::timeline::{Page, Timeline};
use chat_model::{Id, Message};
use idea_state::{Cursor, Step};
use runtime_core::{signal, Signal};

use crate::platform::now_ms;

/// How the live connection is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conn {
    /// First connection, before the snapshot.
    Connecting,
    Open,
    /// Lost; trying again. The state shown is as it was.
    Reconnecting,
}

/// Where the person is in the app — what the stage shows and the navigation
/// highlights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Place {
    #[default]
    Home,
    Direct(Id),
    Channel { space: Id, channel: Id },
}

impl Place {
    pub fn space(self) -> Option<Id> {
        match self {
            Place::Channel { space, .. } => Some(space),
            _ => None,
        }
    }

    pub fn channel(self) -> Option<Id> {
        match self {
            Place::Direct(c) | Place::Channel { channel: c, .. } => Some(c),
            Place::Home => None,
        }
    }
}

/// Which page of the settings modal is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settings {
    Profile,
    Account,
    Appearance,
    Devices,
    Space(Id, SpacePage),
    Channel(Id),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpacePage {
    Overview,
    Members,
    Invites,
    Channels,
}

/// The dialogs that sit over everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    CreateSpace,
    JoinSpace,
    Invite(Id),
    CreateChannel { space: Id, voice: bool, category: Option<Id> },
    NewMessage,
    AddFriend,
    StartThread { channel: Id, from: Option<Id> },
}

/// What the stage's side panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    /// A space's members, or a conversation's profile.
    People,
    Search,
}

/// The UI's own state. All signals, so it is `Copy`.
#[derive(Clone, Copy)]
pub struct Ui {
    pub place: Signal<Place>,
    /// The side panel, if open. Open by default on wide screens.
    pub panel: Signal<Option<Panel>>,
    /// The navigation drawer, below the width it pins at.
    pub drawer: Signal<bool>,
    pub settings: Signal<Option<Settings>>,
    pub dialog: Signal<Option<Dialog>>,
    /// The person whose profile card is open.
    pub profile: Signal<Option<Id>>,
    /// The message being replied to, by channel.
    pub reply: Signal<BTreeMap<Id, Id>>,
    /// The message being edited.
    pub editing: Signal<Option<Id>>,
    /// The message to bring into view in a channel (a search result, a jump
    /// to a reply's original).
    pub focus: Signal<Option<Id>>,
    pub search: Signal<String>,
    pub dark: Signal<bool>,
    /// An image being viewed full size.
    pub viewer: Signal<Option<chat_model::Attachment>>,
}

struct Inner {
    live: RefCell<State>,
    rev: Signal<u64>,
    conn: Signal<Conn>,
    cursor: RefCell<Cursor>,
    timelines: RefCell<HashMap<Id, Timeline>>,
    trev: Signal<u64>,
    /// Channels whose history is being fetched.
    loading: Signal<Vec<Id>>,
    /// Who is typing where, and when they last said so (local clock).
    typing: Signal<BTreeMap<Id, BTreeMap<Id, i64>>>,
    /// A coarse clock for anything drawn relative to now.
    now: Signal<i64>,
    ui: Ui,
    up: RefCell<Option<Rc<dyn Fn(Up) -> bool>>>,
    /// Ephemeral call-key events, for the call module.
    keys: RefCell<Vec<Rc<dyn Fn(&Event)>>>,
    reads_sent: RefCell<HashMap<Id, Id>>,
    /// A space to open once the live state has it.
    pending_space: std::cell::Cell<Option<Id>>,
}

/// The client's state. Cheap to clone.
#[derive(Clone)]
pub struct Store(Rc<Inner>);

/// How long a typing signal shows for.
pub const TYPING_MS: i64 = 6_000;

thread_local! {
    static CURRENT: RefCell<Option<Store>> = const { RefCell::new(None) };
}

/// The signed-in session's store. Kept in a thread-local as well as the
/// world's context, because handlers, timers and async continuations run
/// outside the reactive world, where context injection is not available.
pub fn store() -> Store {
    CURRENT.with(|c| c.borrow().clone()).expect("store() is only called while signed in")
}

impl Store {
    pub fn new(dark: bool) -> Store {
        Store(Rc::new(Inner {
            live: RefCell::new(State::default()),
            rev: signal(0),
            conn: signal(Conn::Connecting),
            cursor: RefCell::new(Cursor::at(0)),
            timelines: RefCell::new(HashMap::new()),
            trev: signal(0),
            loading: signal(Vec::new()),
            typing: signal(BTreeMap::new()),
            now: signal(now_ms()),
            ui: Ui {
                place: signal(Place::Home),
                panel: signal(Some(Panel::People)),
                drawer: signal(false),
                settings: signal(None),
                dialog: signal(None),
                profile: signal(None),
                reply: signal(BTreeMap::new()),
                editing: signal(None),
                focus: signal(None),
                search: signal(String::new()),
                dark: signal(dark),
                viewer: signal(None),
            },
            up: RefCell::new(None),
            keys: RefCell::new(Vec::new()),
            reads_sent: RefCell::new(HashMap::new()),
            pending_space: std::cell::Cell::new(None),
        }))
    }

    /// Make this the store everything reads.
    pub fn provide(&self) {
        CURRENT.with(|c| *c.borrow_mut() = Some(self.clone()));
        runtime_core::provide(self.clone());
    }

    pub fn ui(&self) -> Ui {
        self.0.ui
    }

    /// Read the live state, subscribing to its changes.
    pub fn read<R>(&self, f: impl FnOnce(&State) -> R) -> R {
        self.0.rev.get();
        f(&self.0.live.borrow())
    }

    /// Read the live state without subscribing — in handlers.
    pub fn peek<R>(&self, f: impl FnOnce(&State) -> R) -> R {
        f(&self.0.live.borrow())
    }

    pub fn me(&self) -> Option<Id> {
        self.peek(|s| s.me_id())
    }

    /// Whether the viewer manages `space`'s settings, the check the settings
    /// modal and the space nav both ask for.
    pub fn manages_space(&self, space: Id) -> bool {
        self.read(|s| s.my_permissions(space).is_some_and(|p| perms::has(p, perms::Permission::ManageSpaceSettings)))
    }

    pub fn conn(&self) -> Signal<Conn> {
        self.0.conn
    }

    /// The coarse clock, ticking every few seconds.
    pub fn now(&self) -> i64 {
        self.0.now.get()
    }

    pub fn tick(&self) {
        self.0.now.set(now_ms());
    }

    /// Who is typing in a channel right now, but the viewer.
    pub fn typing_in(&self, channel: Id) -> Vec<Id> {
        let now = self.0.now.get();
        let me = self.me();
        self.0.typing.with(|t| {
            t.get(&channel)
                .map(|people| {
                    people.iter().filter(|(u, at)| Some(**u) != me && now - **at < TYPING_MS).map(|(u, _)| *u).collect()
                })
                .unwrap_or_default()
        })
    }

    // ---- the live connection ----

    /// The live connection is up and can be written to through `send`.
    pub fn attach(&self, send: Rc<dyn Fn(Up) -> bool>) {
        *self.0.up.borrow_mut() = Some(send);
    }

    pub fn detach(&self) {
        *self.0.up.borrow_mut() = None;
        if self.0.conn.peek() == Conn::Open {
            self.0.conn.set(Conn::Reconnecting);
        }
    }

    fn up(&self, up: Up) {
        if let Some(send) = self.0.up.borrow().as_ref() {
            send(up);
        }
    }

    pub fn ping(&self) {
        self.up(Up::Ping);
    }

    pub fn say_typing(&self, channel: Id) {
        self.up(Up::Action(Action::Typing { channel }));
    }

    pub fn say_idle(&self, idle: bool) {
        self.up(Up::Action(Action::Idle { idle }));
    }

    /// Call `f` with every call-key event that arrives.
    pub fn on_key_event(&self, f: Rc<dyn Fn(&Event)>) {
        self.0.keys.borrow_mut().push(f);
    }

    /// Fold in one frame from the server. Returns `false` when the stream
    /// can no longer be trusted (a gap) and the connection should restart.
    pub fn receive(&self, down: Down) -> bool {
        match down {
            Down::Snapshot { seq, state } => {
                *self.0.cursor.borrow_mut() = Cursor::at(seq);
                *self.0.live.borrow_mut() = state;
                self.0.rev.update(|r| r + 1);
                self.0.conn.set(Conn::Open);
                // Timelines may have missed messages while disconnected:
                // bring each open one up to the present.
                self.catch_up();
                self.open_pending();
                true
            }
            Down::Event { seq, event } => match self.0.cursor.borrow_mut().step(seq) {
                Step::Duplicate => true,
                Step::Gap => false,
                Step::Apply => {
                    self.apply(&event);
                    true
                }
            },
            Down::Refused { .. } | Down::Pong => true,
        }
    }

    fn apply(&self, event: &Event) {
        match event {
            Event::Typing { channel, user } => {
                let (channel, user) = (*channel, *user);
                self.0.typing.update(|t| {
                    let mut t = t.clone();
                    t.entry(channel).or_default().insert(user, now_ms());
                    t
                });
                self.tick();
                return;
            }
            Event::KeyRequested { .. } | Event::KeyDelivered { .. } => {
                for f in self.0.keys.borrow().iter() {
                    f(event);
                }
                return;
            }
            Event::MessageCreated { message } => {
                let (channel, author) = (message.channel, message.author);
                if self.0.typing.peek().get(&channel).is_some_and(|p| p.contains_key(&author)) {
                    self.0.typing.update(|t| {
                        let mut t = t.clone();
                        if let Some(p) = t.get_mut(&channel) {
                            p.remove(&author);
                        }
                        t
                    });
                }
            }
            _ => {}
        }
        self.0.live.borrow_mut().apply(event);
        self.0.rev.update(|r| r + 1);
        let mut changed = false;
        for t in self.0.timelines.borrow_mut().values_mut() {
            changed |= t.apply(event);
        }
        if changed {
            self.0.trev.update(|r| r + 1);
        }
        if matches!(event, Event::SpaceUpserted { .. }) {
            self.open_pending();
        }
        // A channel the viewer can no longer see is not worth keeping.
        if let Event::ChannelRemoved { channel } | Event::SpaceRemoved { space: channel } = event {
            let live = self.0.live.borrow();
            self.0.timelines.borrow_mut().retain(|c, _| live.channels.contains_key(c));
            let _ = channel;
        }
    }

    /// Open `space` as soon as the live state has it.
    pub fn open_when_ready(&self, space: Id) {
        self.0.pending_space.set(Some(space));
    }

    fn open_pending(&self) {
        if let Some(space) = self.0.pending_space.get() {
            if self.peek(|st| st.spaces.contains_key(&space)) {
                self.0.pending_space.set(None);
                crate::shell::open_space(space);
            }
        }
    }

    // ---- timelines ----

    /// Read a channel's loaded timeline, subscribing to its changes.
    pub fn timeline<R>(&self, channel: Id, f: impl FnOnce(Option<&Timeline>) -> R) -> R {
        self.0.trev.get();
        f(self.0.timelines.borrow().get(&channel))
    }

    /// A loaded message, wherever it is.
    pub fn message(&self, channel: Id, id: Id) -> Option<Message> {
        self.timeline(channel, |t| t.and_then(|t| t.get(id).cloned()))
    }

    pub fn is_loading(&self, channel: Id) -> bool {
        self.0.loading.get().contains(&channel)
    }

    fn set_loading(&self, channel: Id, on: bool) {
        self.0.loading.update(|l| {
            let mut l = l.clone();
            l.retain(|c| *c != channel);
            if on {
                l.push(channel);
            }
            l
        });
    }

    /// Load a channel's latest history if it is not loaded yet.
    pub fn open(&self, channel: Id) {
        if self.0.timelines.borrow().contains_key(&channel) || self.0.loading.peek().contains(&channel) {
            return;
        }
        self.fetch(channel, Window::Latest);
    }

    /// Load the page before what is loaded.
    pub fn older(&self, channel: Id) {
        let first = self.0.timelines.borrow().get(&channel).filter(|t| t.older).and_then(|t| t.first());
        if let Some(id) = first {
            if !self.0.loading.peek().contains(&channel) {
                self.fetch(channel, Window::Before { id });
            }
        }
    }

    /// Load the history around a message and show it.
    pub fn jump(&self, channel: Id, message: Id) {
        if self.0.timelines.borrow().get(&channel).is_some_and(|t| t.get(message).is_some()) {
            self.0.ui.focus.set(Some(message));
            return;
        }
        self.fetch(channel, Window::Around { id: message });
        self.0.ui.focus.set(Some(message));
    }

    /// Drop a window away from the present and load the latest instead.
    pub fn jump_to_present(&self, channel: Id) {
        self.0.timelines.borrow_mut().remove(&channel);
        self.0.trev.update(|r| r + 1);
        self.fetch(channel, Window::Latest);
    }

    /// Bring every loaded timeline back up to the present.
    fn catch_up(&self) {
        let open: Vec<(Id, Option<Id>, bool)> =
            self.0.timelines.borrow().iter().map(|(c, t)| (*c, t.last(), t.present)).collect();
        for (channel, last, present) in open {
            match last {
                Some(id) if present => self.fetch(channel, Window::After { id }),
                _ => {
                    self.0.timelines.borrow_mut().remove(&channel);
                    self.fetch(channel, Window::Latest);
                }
            }
        }
    }

    fn fetch(&self, channel: Id, window: Window) {
        self.set_loading(channel, true);
        let this = self.clone();
        runtime_core::spawn_then(chat_api::messages::history(channel, window), move |page| {
            this.set_loading(channel, false);
            let Ok(page) = page else { return };
            this.merge(channel, window, page);
        });
    }

    fn merge(&self, channel: Id, window: Window, page: Page) {
        {
            let mut timelines = self.0.timelines.borrow_mut();
            match (timelines.get_mut(&channel), window) {
                (Some(t), Window::Before { .. }) => t.prepend(page),
                (Some(t), Window::After { .. }) => t.append(page),
                (_, _) => {
                    timelines.insert(channel, Timeline::new(channel, page));
                }
            }
        }
        self.0.trev.update(|r| r + 1);
    }

    /// Mark a channel read up to its newest loaded message, if that is news.
    pub fn mark_read(&self, channel: Id) {
        let newest = self.0.timelines.borrow().get(&channel).filter(|t| t.present).and_then(|t| t.last());
        let Some(newest) = newest else { return };
        let read = self.peek(|s| s.reads.get(&channel).and_then(|r| r.last_read));
        if read.is_some_and(|r| r >= newest) {
            return;
        }
        if self.0.reads_sent.borrow().get(&channel).is_some_and(|sent| *sent >= newest) {
            return;
        }
        self.0.reads_sent.borrow_mut().insert(channel, newest);
        runtime_core::spawn_then(chat_api::messages::mark_read(channel, newest), |_| {});
    }

    /// Put a message the server just accepted straight into its timeline, so
    /// the sender sees it before the live echo arrives (which then folds in
    /// as a duplicate).
    pub fn posted(&self, message: Message) {
        self.apply(&Event::MessageCreated { message });
    }
}
