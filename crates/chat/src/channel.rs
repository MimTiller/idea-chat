//! A channel on the stage — a space's text channel, a thread, or a direct or
//! group conversation: its header, its stream, the box to write in, and the
//! context panel beside it. Voice channels hand their stage to `calls`.

use std::cell::Cell;
use std::rc::Rc;

use chat_model::perms::Access;
use chat_model::timeline::Row;
use chat_model::{ChannelKind, Id};
use icons_lucide::{
    ARROW_LEFT, HASH, LOCK_KEYHOLE, MENU, MESSAGES_SQUARE, PHONE, SEARCH, USERS, USER_PLUS, VIDEO, ARCHIVE,
};
use idea_ui::{tone, typography_kind, variant, AvatarSize, Button, Icon, IdeaThemeRef, Spinner, SpinnerSize, Typography};
use idea_ui_nav::sidebar_pinned;
use runtime_core::{
    component, memo, rx, stylesheet, ui, AlignItems, Element, FlexDirection, FontWeight,
    JustifyContent, Length, Position, Ref, ScrollViewHandle,
};

use crate::bits::{day_label, Action, Empty, PersonAvatar};
use crate::calls::VoiceStage;
use crate::composer::Composer;
use crate::context::ContextPanel;
use crate::forms::cb;
use crate::message::MessageRow;
use crate::platform;
use crate::shell::{go, panel_inline, toggle_panel, StageMain, StageRow, PIN_AT};
use crate::store::{store, Conn, Dialog, Panel, Place};

stylesheet! {
    pub StageHeader<IdeaThemeRef> {
        base(t) {
            height: 56.0,
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 10.0,
            padding_left: 16.0,
            padding_right: 12.0,
            border_bottom_width: 1.0,
            border_color: t.color.border(),
        }
    }
}

stylesheet! {
    pub HeaderTitle<()> {
        base(_t) {
            flex_grow: 1.0,
            flex_basis: 0.0,
            min_width: 0.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 10.0,
        }
    }
}

stylesheet! {
    pub HeaderActions<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 2.0,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    /// The topic beside the title (shown from tablet width up).
    Topic<IdeaThemeRef> {
        base(t) {
            min_width: 0.0,
            flex_shrink: 1.0,
            border_left_width: 1.0,
            border_color: t.color.border(),
            padding_left: 10.0,
        }
    }
}

stylesheet! {
    StreamScroll<()> {
        base(_t) {
            flex_grow: 1.0,
            min_height: 0.0,
        }
    }
}

stylesheet! {
    Stream<()> {
        base(_t) {
            // Scroll content grows past the viewport rather than shrinking into it.
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::FlexEnd,
            min_height: Length::pct(100.0),
            padding_top: 16.0,
            padding_bottom: 12.0,
        }
    }
}

stylesheet! {
    DayRule<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 10.0,
            padding_left: 16.0,
            padding_right: 16.0,
            padding_top: 14.0,
            padding_bottom: 6.0,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    Hairline<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            height: 1.0,
            background: t.color.border(),
        }
        variant warm {
            #[default]
            off(_t) {}
            on(t) { background: t.intent.danger.fg(), height: 2.0 }
        }
    }
}

stylesheet! {
    NewTag<IdeaThemeRef> {
        base(t) {
            padding_left: 8.0,
            padding_right: 8.0,
            padding_top: 1.0,
            padding_bottom: 1.0,
            border_radius: 999.0,
            background: t.intent.danger.solid_bg(),
            color: t.intent.danger.solid_text(),
        }
    }
}

stylesheet! {
    Older<()> {
        base(_t) {
            align_items: AlignItems::Center,
            padding_top: 8.0,
            padding_bottom: 8.0,
        }
    }
}

stylesheet! {
    /// The jump-to-present pill, over the stream's foot.
    Float<()> {
        base(_t) {
            position: Position::Absolute,
            left: 0.0,
            right: 0.0,
            bottom: 96.0,
            align_items: AlignItems::Center,
        }
    }
}

stylesheet! {
    /// Read-only: where the box would be, a note saying why it is not.
    ReadOnly<IdeaThemeRef> {
        base(t) {
            flex_shrink: 0.0,
            margin_left: 16.0,
            margin_right: 16.0,
            margin_bottom: 16.0,
            height: 44.0,
            border_radius: 12.0,
            border_width: 2.0,
            border_color: t.color.border(),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
    }
}

/// What the stage header and body are drawn from, as data.
#[derive(Debug, Clone, PartialEq)]
struct ChannelHead {
    kind: ChannelKind,
    access: Access,
    title: String,
    topic: String,
    space: Option<Id>,
    partner: Option<Id>,
    parent: Option<(Id, String)>,
    archived: bool,
}

fn head(channel: Id) -> Option<ChannelHead> {
    store().read(|st| {
        let c = st.channels.get(&channel)?;
        Some(ChannelHead {
            kind: c.kind,
            access: st.access(channel),
            title: st.title(c),
            topic: match st.partner(c) {
                Some(p) => st.status_text(p).unwrap_or_default().to_string(),
                None => c.topic.clone(),
            },
            space: c.space,
            partner: st.partner(c),
            parent: c.parent.and_then(|p| st.channels.get(&p)).map(|p| (p.id, p.name.clone())),
            archived: c.archived,
        })
    })
}

#[component]
pub fn ChannelStage(#[prop(static)] channel: Id) -> Element {
    let s = store();
    let ui = s.ui();
    let info = memo(move || head(channel));
    // Which body, coarsely: rebuilt only when that changes.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Body {
        Waiting,
        Missing,
        Locked,
        Voice,
        Text,
    }
    let conn = s.conn();
    let body = memo(move || match info.get() {
        None if conn.get() == Conn::Connecting => Body::Waiting,
        None => Body::Missing,
        Some(h) if h.access == Access::Locked => Body::Locked,
        Some(h) if h.kind == ChannelKind::Voice => Body::Voice,
        Some(_) => Body::Text,
    });
    let panel_open = memo(move || ui.panel.get().is_some());
    ui! {
        view(style = StageRow()) {
            view(style = StageMain()) {
                Header(channel = channel)
                match body.get() {
                    Body::Waiting => { view(style = crate::bits::EmptyBox()) { Spinner(size = SpinnerSize::Large) } }
                    Body::Missing => {
                        Empty(icon = HASH, title = "This isn't here".to_string(), body = "It may have been deleted, or you may have left.".to_string(), action = Some(("Go home".to_string(), cb(|| go(Place::Home)))))
                    }
                    Body::Locked => {
                        Empty(icon = LOCK_KEYHOLE, title = "This channel is locked".to_string(), body = "Only some roles in this space can read it. Ask an admin if you need in.".to_string())
                    }
                    Body::Voice => { VoiceStage(channel = channel) }
                    Body::Text => { TextBody(channel = channel) }
                }
            }
            if panel_open.get() {
                ContextPanel(channel = channel, inline = rx!(panel_inline()))
            }
        }
    }
}

#[component]
fn Header(#[prop(static)] channel: Id) -> Element {
    let s = store();
    let ui = s.ui();
    let info = memo(move || head(channel));
    let kind = info.peek().map(|h| h.kind);
    let private = kind.is_some_and(|k| k.is_private());
    let calls = kind.is_some_and(|k| matches!(k, ChannelKind::Direct | ChannelKind::Group));
    let space = info.peek().and_then(|h| h.space);
    let people_label = if private { "Profile" } else { "Members" };
    ui! {
        view(style = StageHeader()) {
            if !sidebar_pinned(PIN_AT) {
                Action(icon = MENU, label = "Open navigation".to_string(), on_click = cb(move || ui.drawer.set(true)))
            }
            view(style = HeaderTitle()) {
                match info.get().map(|h| (h.kind, h.partner)) {
                    Some((ChannelKind::Direct, Some(p))) => { PersonAvatar(user = *p, size = AvatarSize::Sm) }
                    Some((ChannelKind::Group, _)) => { Icon(data = USERS, size = 20.0) }
                    Some((ChannelKind::Thread, _)) => { Icon(data = MESSAGES_SQUARE, size = 20.0) }
                    _ => { Icon(data = HASH, size = 20.0) }
                }
                Typography(content = rx!(info.get().map(|h| h.title).unwrap_or_default()), weight = Some(FontWeight::SemiBold), lines = Some(1), kind = typography_kind::BodyLg)
                match info.get().and_then(|h| h.parent) {
                    Some((parent, name)) => {
                        Button(label = format!("in #{name}"), variant = variant::Ghost, tone = tone::Neutral, size = idea_ui::size::Sm, leading_icon = Some(ARROW_LEFT), on_click = {
                            let (parent, space) = (*parent, space);
                            cb(move || if let Some(space) = space { go(Place::Channel { space, channel: parent }) })
                        })
                    }
                    None => { runtime_core::fragment(Vec::new()) }
                }
                if info.get().is_some_and(|h| !h.topic.is_empty()) && runtime_core::current_breakpoint().get().is_at_least(runtime_core::Breakpoint::Md) {
                    view(style = Topic()) {
                        Typography(content = rx!(info.get().map(|h| h.topic).unwrap_or_default()), kind = typography_kind::BodySm, muted = true, lines = Some(1))
                    }
                }
                if info.get().is_some_and(|h| h.archived) {
                    Icon(data = ARCHIVE, size = 16.0, tone = Some(tone::Neutral.into()))
                }
            }
            view(style = HeaderActions()) {
                if calls {
                    Action(icon = PHONE, label = "Start a voice call".to_string(), tone = tone::Primary, on_click = cb(move || crate::calls::join(channel)))
                    Action(icon = VIDEO, label = "Start a video call".to_string(), tone = tone::Primary, on_click = cb(move || crate::calls::join_with_camera(channel)))
                }
                if let Some(space) = space {
                    Action(icon = USER_PLUS, label = "Invite people".to_string(), on_click = cb(move || ui.dialog.set(Some(Dialog::Invite(space)))))
                }
                Action(icon = SEARCH, label = "Search".to_string(), selected = rx!(ui.panel.get() == Some(Panel::Search)), on_click = cb(|| toggle_panel(Panel::Search)))
                Action(icon = USERS, label = people_label.to_string(), selected = rx!(ui.panel.get() == Some(Panel::People)), on_click = cb(|| toggle_panel(Panel::People)))
            }
        }
    }
}

/// A text channel's (or conversation's) stream and box.
#[component]
fn TextBody(#[prop(static)] channel: Id) -> Element {
    let s = store();
    let ui = s.ui();
    s.open(channel);
    let me = s.me();
    // Where unread began when the channel was opened: the "new" marker stays
    // put while you read, rather than vanishing the moment you are seen.
    let read_at_open = s.peek(|st| st.reads.get(&channel).and_then(|r| r.last_read));
    let had_unread = s.peek(|st| st.unread(channel, s.now()));
    let marker = if had_unread { read_at_open } else { None };
    let rows = {
        let s = s.clone();
        memo(move || s.timeline(channel, |t| t.map(|t| t.rows(marker, me, platform::utc_offset_min())).unwrap_or_default()))
    };
    let state = {
        let s = s.clone();
        memo(move || {
            let loading = s.is_loading(channel);
            s.timeline(channel, |t| match t {
                None => (true, false, true, 0usize),
                Some(t) => (loading, t.older, t.present, t.messages.len()),
            })
        })
    };
    let head_info = memo(move || head(channel));
    let can_post = memo(move || head_info.get().is_some_and(|h| h.access.can_post()));
    let placeholder = head_info
        .peek()
        .map(|h| match h.kind {
            ChannelKind::Direct | ChannelKind::Group => format!("Message {}", h.title),
            _ => format!("Message #{}", h.title),
        })
        .unwrap_or_else(|| "Message".into());
    let readonly_why = head_info.peek().map(|h| if h.archived { "This thread is archived." } else { "You can read this channel, but not post in it." }).unwrap_or_default();

    // Stick to the bottom: follow new messages while at the bottom; stop when
    // the person scrolls up to read.
    let scroller: Ref<ScrollViewHandle> = Ref::new();
    let at_bottom = Rc::new(Cell::new(true));
    let last_y = Rc::new(Cell::new(0f32));
    {
        let (s, at_bottom) = (s.clone(), at_bottom.clone());
        runtime_core::effect!({
            let _ = rows.get();
            if at_bottom.get() {
                runtime_core::after_ms_scoped(16, move || {
                    scroller.with(|h| h.scroll_to(0.0, 1.0e7));
                });
                if platform::visible() {
                    s.mark_read(channel);
                }
            }
        });
    }
    let on_scroll = {
        let (at_bottom, last_y, s) = (at_bottom.clone(), last_y.clone(), s.clone());
        move |_x: f32, y: f32| {
            if y + 4.0 < last_y.get() {
                at_bottom.set(false);
            }
            last_y.set(y);
            if y < 120.0 {
                s.older(channel);
            }
        }
    };
    let on_end = {
        let (at_bottom, s) = (at_bottom.clone(), s.clone());
        move || {
            at_bottom.set(true);
            s.mark_read(channel);
        }
    };
    let present = cb({
        let s = s.clone();
        move || {
            ui.focus.set(None);
            s.jump_to_present(channel);
        }
    });

    ui! {
        view(style = crate::bits::Grow()) {
        scroll_view(style = StreamScroll(), bind = scroller, on_scroll = on_scroll, on_end_reached = on_end, end_reached_threshold = 40.0) {
            view(style = Stream()) {
                if state.get().1 {
                    OlderPager(channel = channel)
                }
                if !state.get().1 && !state.get().0 {
                    ChannelWelcome(channel = channel)
                }
                if state.get().0 && state.get().3 == 0 {
                    view(style = Older()) { Spinner(size = SpinnerSize::Large) }
                }
                for row in rows, key = row_key(&row) {
                    match row {
                        Row::Day { day_start } => { DayDivider(day = day_start) }
                        Row::New => { NewDivider() }
                        Row::Message { id, continued } => { MessageRow(channel = channel, id = id, continued = continued) }
                    }
                }
            }
        }
        if !state.get().2 {
            view(style = Float()) {
                Button(label = "Jump to the present".to_string(), size = idea_ui::size::Sm, on_click = present.clone())
            }
        }
        if can_post.get() {
            Composer(channel = channel, placeholder = placeholder.clone())
        } else {
            view(style = ReadOnly()) {
                Typography(content = readonly_why.to_string(), kind = typography_kind::BodySm, muted = true)
            }
        }
        }
    }
}

/// The top of a window with more before it: a way to load it, or a
/// spinner while it loads.
#[component]
fn OlderPager(#[prop(static)] channel: Id) -> Element {
    let s = store();
    let loading = {
        let s = s.clone();
        memo(move || s.is_loading(channel))
    };
    let older = cb(move || s.older(channel));
    ui! {
        view(style = Older()) {
            if loading.get() {
                Spinner(size = SpinnerSize::Small)
            } else {
                Button(label = "Load earlier messages".to_string(), variant = variant::Ghost, size = idea_ui::size::Sm, on_click = older.clone())
            }
        }
    }
}

fn row_key(row: &Row) -> String {
    match row {
        Row::Day { day_start } => format!("d{day_start}"),
        Row::New => "new".to_string(),
        Row::Message { id, .. } => id.to_string(),
    }
}

#[component]
fn DayDivider(#[prop(static)] day: i64) -> Element {
    let s = store();
    let label = memo(move || day_label(day + 12 * 3_600_000, s.now()));
    ui! {
        view(style = DayRule()) {
            view(style = Hairline()) {}
            Typography(content = rx!(label.get()), kind = typography_kind::Caption, weight = Some(FontWeight::SemiBold), muted = true)
            view(style = Hairline()) {}
        }
    }
}

#[component]
fn NewDivider() -> Element {
    ui! {
        view(style = DayRule()) {
            view(style = Hairline().warm(HairlineWarm::On)) {}
            view(style = NewTag()) { Typography(content = "NEW".to_string(), kind = typography_kind::Overline, weight = Some(FontWeight::Bold)) }
        }
    }
}

stylesheet! {
    Welcome<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 6.0,
            padding_left: 16.0,
            padding_right: 16.0,
            padding_top: 24.0,
            padding_bottom: 16.0,
        }
    }
}

stylesheet! {
    WelcomeMark<IdeaThemeRef> {
        base(t) {
            width: 64.0,
            height: 64.0,
            border_radius: 999.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            background: t.intent.primary.soft_bg(),
            margin_bottom: 6.0,
        }
    }
}

/// The very top of a channel's history: what it is, and how it began.
#[component]
fn ChannelWelcome(#[prop(static)] channel: Id) -> Element {
    let info = head(channel);
    let Some(h) = info else { return runtime_core::fragment(Vec::new()) };
    let (title, body) = match h.kind {
        ChannelKind::Direct => (h.title.clone(), format!("This is the beginning of your conversation with {}.", h.title)),
        ChannelKind::Group => (h.title.clone(), "This is the beginning of this group.".to_string()),
        ChannelKind::Thread => (h.title.clone(), "This is the beginning of this thread.".to_string()),
        _ => (format!("Welcome to #{}", h.title), format!("This is the start of #{}. {}", h.title, h.topic)),
    };
    ui! {
        view(style = Welcome()) {
            match h.partner {
                Some(p) => { PersonAvatar(user = p, size = AvatarSize::Xl) }
                None => { view(style = WelcomeMark()) { Icon(data = if h.kind == ChannelKind::Thread { MESSAGES_SQUARE } else { HASH }, size = 30.0, tone = Some(tone::Primary.into())) } }
            }
            Typography(content = title, kind = typography_kind::H4)
            Typography(content = body, muted = true)
        }
    }
}
