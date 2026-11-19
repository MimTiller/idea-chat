//! The context panel beside the stage: a space's members, the person a
//! conversation is with, or search. Beside the stage on wide screens, over it
//! on narrower ones.

use chat_api::messages::Scope;
use chat_model::{ChannelKind, Id, Message};
use icons_lucide::{MESSAGE_SQUARE, PHONE, SEARCH, X};
use idea_ui::{tone, typography_kind, variant, AvatarSize, Button, Divider, Field, IdeaThemeRef, Spinner, SpinnerSize, Typography};
use runtime_core::{
    component, memo, rx, signal, stylesheet, ui, AlignItems, Color, Element, FlexDirection, FontWeight,
    Position, Shadow, Signal, Tokenized,
};

use crate::bits::{presence_label, stamp, Action, PersonAvatar, RowLabel};
use crate::forms::{cb, on_enter, set};
use crate::nav::{style, Tap};
use crate::platform;
use crate::shell::go;
use crate::store::{store, Panel as Showing, Place};

stylesheet! {
    Panel<IdeaThemeRef> {
        base(t) {
            width: 264.0,
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            background: t.color.background(),
            border_left_width: 1.0,
            border_color: t.color.border(),
            min_height: 0.0,
        }
        variant float {
            #[default]
            off(_t) {}
            on(_t) {
                position: Position::Absolute,
                right: 0.0,
                top: 56.0,
                bottom: 0.0,
                shadow: Shadow { x: -8.0, y: 0.0, blur: 24.0, color: Tokenized::Literal(Color("rgba(0,0,0,0.12)".into())) },
            }
        }
    }
}

stylesheet! {
    PanelHead<IdeaThemeRef> {
        base(t) {
            height: 48.0,
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding_left: 16.0,
            padding_right: 8.0,
            border_bottom_width: 1.0,
            border_color: t.color.border(),
        }
    }
}

stylesheet! {
    PanelScroll<()> {
        base(_t) {
            flex_grow: 1.0,
            min_height: 0.0,
        }
    }
}

stylesheet! {
    PanelBody<()> {
        base(_t) {
            // Scroll content grows past the viewport rather than shrinking into it.
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            gap: 2.0,
            padding_top: 8.0,
            padding_bottom: 16.0,
        }
    }
}

stylesheet! {
    Group<()> {
        base(_t) {
            padding_left: 16.0,
            padding_right: 16.0,
            padding_top: 12.0,
            padding_bottom: 4.0,
        }
    }
}

stylesheet! {
    MemberRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 10.0,
            min_height: 42.0,
            padding_left: 8.0,
            padding_right: 8.0,
            margin_left: 8.0,
            margin_right: 8.0,
            border_radius: 6.0,
            cursor: runtime_core::Cursor::Pointer,
        }
        state hovered(t) { background: t.color.surface_alt() }
        variant away {
            #[default]
            off(_t) {}
            on(_t) { opacity: 0.5 }
        }
    }
}

#[component]
pub fn ContextPanel(#[prop(static)] channel: Id, inline: bool) -> Element {
    let s = store();
    let ui = s.ui();
    let kind = s.peek(|st| st.channels.get(&channel).map(|c| (c.kind, c.space, st.partner(c))));
    let title = memo(move || match ui.panel.get() {
        Some(Showing::Search) => "Search".to_string(),
        _ => match kind {
            Some((ChannelKind::Direct, _, _)) => "Profile".to_string(),
            _ => "Members".to_string(),
        },
    });
    let floating = inline.clone();
    ui! {
        view(style = Panel().float(runtime_core::derived(move || if floating.get() { PanelFloat::Off } else { PanelFloat::On }))) {
            view(style = PanelHead()) {
                view(style = RowLabel()) { Typography(content = rx!(title.get()), weight = Some(FontWeight::SemiBold)) }
                Action(icon = X, label = "Close panel".to_string(), size = idea_ui::IconButtonSize::Sm, on_click = cb(move || ui.panel.set(None)))
            }
            match ui.panel.get() {
                Some(Showing::Search) => { SearchPanel(channel = channel) }
                _ => {
                    match kind {
                        Some((ChannelKind::Direct, _, Some(partner))) => { ProfilePanel(user = partner) }
                        Some((_, Some(space), _)) => { MemberList(space = space) }
                        _ => { ConversationPeople(channel = channel) }
                    }
                }
            }
        }
    }
}

#[component]
fn MemberList(#[prop(static)] space: Id) -> Element {
    let s = store();
    let roster = memo(move || s.read(|st| st.roster(space)));
    let (admins, online, offline) = (memo(move || roster.get().admins), memo(move || roster.get().online), memo(move || roster.get().offline));
    ui! {
        scroll_view(style = PanelScroll()) {
            view(style = PanelBody()) {
                if !roster.get().admins.is_empty() {
                    view(style = Group()) { Typography(content = rx!(format!("ADMINS — {}", roster.get().admins.len())), kind = typography_kind::Overline, muted = true) }
                    for u in admins, key = format!("a{u}") { Member(user = u, away = false) }
                }
                if !roster.get().online.is_empty() {
                    view(style = Group()) { Typography(content = rx!(format!("ONLINE — {}", roster.get().online.len())), kind = typography_kind::Overline, muted = true) }
                    for u in online, key = format!("o{u}") { Member(user = u, away = false) }
                }
                if !roster.get().offline.is_empty() {
                    view(style = Group()) { Typography(content = rx!(format!("OFFLINE — {}", roster.get().offline.len())), kind = typography_kind::Overline, muted = true) }
                    for u in offline, key = format!("f{u}") { Member(user = u, away = true) }
                }
            }
        }
    }
}

#[component]
fn ConversationPeople(#[prop(static)] channel: Id) -> Element {
    let s = store();
    let people = memo(move || s.read(|st| st.channels.get(&channel).map(|c| c.members.clone()).unwrap_or_default()));
    ui! {
        scroll_view(style = PanelScroll()) {
            view(style = PanelBody()) {
                view(style = Group()) { Typography(content = rx!(format!("IN THIS GROUP — {}", people.get().len())), kind = typography_kind::Overline, muted = true) }
                for u in people, key = u.to_string() { Member(user = u, away = false) }
            }
        }
    }
}

/// A person in a list, opening their profile card on press.
#[component]
pub fn Member(#[prop(static)] user: Id, #[prop(static)] away: bool) -> Element {
    let s = store();
    let ui = s.ui();
    let line = memo(move || s.read(|st| (st.display_name(user), st.status_text(user).map(str::to_string))));
    ui! {
        Tap(on_press = cb(move || ui.profile.set(Some(user))), label = line.peek().0, style = style(MemberRow().away(if away { MemberRowAway::On } else { MemberRowAway::Off }))) {
            PersonAvatar(user = user, size = AvatarSize::Sm)
            view(style = RowLabel()) {
                Typography(content = rx!(line.get().0), weight = Some(FontWeight::Medium), lines = Some(1))
                if line.get().1.is_some() {
                    Typography(content = rx!(line.get().1.unwrap_or_default()), kind = typography_kind::Caption, muted = true, lines = Some(1))
                }
            }
        }
    }
}

stylesheet! {
    Profile<()> {
        base(_t) {
            // Scroll content grows past the viewport rather than shrinking into it.
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            gap: 6.0,
            padding_left: 20.0,
            padding_right: 20.0,
            padding_top: 28.0,
            padding_bottom: 20.0,
        }
    }
}

stylesheet! {
    ProfileText<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 8.0,
            padding_left: 20.0,
            padding_right: 20.0,
            padding_top: 12.0,
        }
    }
}

stylesheet! {
    ProfileActions<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            gap: 8.0,
            padding_top: 8.0,
        }
    }
}

/// Everything public about one person, and ways to reach them.
#[component]
pub fn ProfilePanel(#[prop(static)] user: Id) -> Element {
    let s = store();
    let info = {
        let s = s.clone();
        memo(move || {
            s.read(|st| {
                let u = st.users.get(&user)?;
                Some((
                    u.display_name.clone(),
                    u.handle.clone(),
                    u.bio.clone(),
                    presence_label(st.presence_of(user)).to_string(),
                    st.status_text(user).map(str::to_string),
                    st.shared_spaces(user).iter().map(|sp| sp.name.clone()).collect::<Vec<_>>(),
                ))
            })
        })
    };
    let me = s.me();
    let not_me = me != Some(user);
    let message = cb(move || crate::overlays::message_person(user));
    let call = cb(move || crate::overlays::call_person(user));
    ui! {
        scroll_view(style = PanelScroll()) {
            view(style = Profile()) {
                PersonAvatar(user = user, size = AvatarSize::Xl)
                Typography(content = rx!(info.get().map(|i| i.0).unwrap_or_default()), kind = typography_kind::H5)
                Typography(content = rx!(info.get().map(|i| format!("@{}", i.1)).unwrap_or_default()), muted = true)
                Typography(content = rx!(info.get().map(|i| i.4.clone().unwrap_or(i.3)).unwrap_or_default()), kind = typography_kind::BodySm, muted = true)
                if not_me {
                    view(style = ProfileActions()) {
                        Button(label = "Message".to_string(), size = idea_ui::size::Sm, leading_icon = Some(MESSAGE_SQUARE), on_click = message.clone())
                        Button(label = "Call".to_string(), size = idea_ui::size::Sm, variant = variant::Soft, leading_icon = Some(PHONE), on_click = call.clone())
                    }
                }
            }
            Divider()
            view(style = ProfileText()) {
                if info.get().is_some_and(|i| !i.2.is_empty()) {
                    Typography(content = "ABOUT".to_string(), kind = typography_kind::Overline, muted = true)
                    Typography(content = rx!(info.get().map(|i| i.2).unwrap_or_default()), kind = typography_kind::BodySm)
                }
                if info.get().is_some_and(|i| !i.5.is_empty()) {
                    Typography(content = "SPACES IN COMMON".to_string(), kind = typography_kind::Overline, muted = true)
                    Typography(content = rx!(info.get().map(|i| i.5.join(", ")).unwrap_or_default()), kind = typography_kind::BodySm)
                }
            }
        }
    }
}

stylesheet! {
    SearchBox<()> {
        base(_t) {
            padding_left: 12.0,
            padding_right: 12.0,
            padding_top: 12.0,
            padding_bottom: 4.0,
        }
    }
}

stylesheet! {
    Hit<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: 4.0,
            margin_left: 10.0,
            margin_right: 10.0,
            padding_left: 12.0,
            padding_right: 12.0,
            padding_top: 10.0,
            padding_bottom: 10.0,
            border_radius: 10.0,
            background: t.color.surface(),
            border_width: 1.0,
            border_color: t.color.border(),
            cursor: runtime_core::Cursor::Pointer,
        }
        state hovered(t) { border_color: t.intent.primary.border() }
    }
}

stylesheet! {
    HitHead<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 6.0,
        }
    }
}

/// Search the space this channel is in (or every conversation).
#[component]
fn SearchPanel(#[prop(static)] channel: Id) -> Element {
    let s = store();
    let ui = s.ui();
    let scope = s.peek(|st| match st.channels.get(&channel).and_then(|c| c.space) {
        Some(space) => Scope::Space { id: space },
        None => Scope::Conversations,
    });
    let query = ui.search;
    let results: Signal<Option<Vec<Message>>> = signal(None);
    let busy = signal(false);
    let run = cb(move || {
        let q = query.peek();
        if q.trim().is_empty() {
            results.set(None);
            return;
        }
        busy.set(true);
        runtime_core::spawn_then(chat_api::messages::search(scope, q), move |r| {
            busy.set(false);
            match r {
                Ok(found) => results.set(Some(found)),
                Err(e) => crate::forms::toast(&e),
            }
        });
    });
    let count = memo(move || results.get().map(|r| r.len()));
    let found = memo(move || results.get().unwrap_or_default());
    ui! {
        view(style = crate::bits::Grow()) {
        view(style = SearchBox()) {
            Field(
                value = query,
                on_change = set(query),
                placeholder = Some(if matches!(scope, Scope::Space { .. }) { "Search this space".to_string() } else { "Search your conversations".to_string() }),
                leading = idea_ui::Adornment::Icon(SEARCH),
                autofocus = true,
                on_key_down = on_enter(run.clone()),
                accessibility_label = Some("Search".to_string()),
            )
        }
        scroll_view(style = PanelScroll()) {
            view(style = PanelBody()) {
                if busy.get() {
                    view(style = Group()) { Spinner(size = SpinnerSize::Small) }
                }
                match count.get() {
                    Some(0) => { view(style = Group()) { Typography(content = "Nothing found.".to_string(), muted = true, kind = typography_kind::BodySm) } }
                    Some(n) => { view(style = Group()) { Typography(content = format!("{n} RESULTS"), kind = typography_kind::Overline, muted = true) } }
                    None => { view(style = Group()) { Typography(content = "Press Enter to search.".to_string(), muted = true, kind = typography_kind::BodySm) } }
                }
                for m in found, key = m.id.to_string() {
                    SearchHit(message = m.clone())
                }
            }
        }
        }
    }
}

#[component]
fn SearchHit(#[prop(static, default = empty_message())] message: Message) -> Element {
    let s = store();
    let (who, place, excerpt) = s.peek(|st| {
        let c = st.channels.get(&message.channel);
        let place = c.map(|c| match c.space {
            Some(_) => format!("#{}", st.title(c)),
            None => st.title(c),
        });
        let excerpt = chat_model::text::plain(
            &message.body,
            |u| st.display_name(u),
            |c| st.channels.get(&c).map(|c| c.name.clone()).unwrap_or_default(),
            |r| st.role_label(r),
            160,
        );
        (st.display_name(message.author), place.unwrap_or_default(), excerpt)
    });
    let (channel, id) = (message.channel, message.id);
    let space = s.peek(|st| st.channels.get(&channel).and_then(|c| c.space));
    let open = cb(move || {
        match space {
            Some(space) => go(Place::Channel { space, channel }),
            None => go(Place::Direct(channel)),
        }
        store().jump(channel, id);
    });
    let at = message.at;
    ui! {
        Tap(on_press = open, label = format!("Message from {who}"), style = style(Hit())) {
            view(style = HitHead()) {
                PersonAvatar(user = message.author, size = AvatarSize::Xs, presence = false)
                Typography(content = who.clone(), kind = typography_kind::Caption, weight = Some(FontWeight::SemiBold))
                Typography(content = format!("{place} · {}", stamp(at, platform::now_ms())), kind = typography_kind::Caption, muted = true, lines = Some(1))
            }
            Typography(content = excerpt, kind = typography_kind::BodySm)
        }
    }
}

fn empty_message() -> Message {
    Message {
        id: Id::nil(),
        channel: Id::nil(),
        author: Id::nil(),
        kind: chat_model::MessageKind::Text,
        body: String::new(),
        attachments: vec![],
        reply_to: None,
        reactions: vec![],
        thread: None,
        mentions: vec![],
        mentions_everyone: false,
        sticker: None,

        mention_roles: vec![],
        at: 0,
        edited_at: None,
    }
}
