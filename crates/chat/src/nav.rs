//! The sidebar: the space switcher, the navigation beside it, and the strips
//! at its foot — the call you are in, and you.

use std::collections::BTreeSet;
use std::rc::Rc;

use chat_model::perms::Access;
use chat_model::{ChannelKind, Id, Presence, Status};
use icons_lucide::{
    CHEVRON_DOWN, CHEVRON_RIGHT, COMPASS, HASH, LOCK, LOG_OUT, MESSAGES_SQUARE, MOON, PLUS, SEARCH, SETTINGS, SUN,
    USERS, USER_PLUS, VOLUME_2, BELL_OFF, FOLDER_PLUS, MIC_OFF, HEADPHONE_OFF,
};
use idea_ui::{
    tone, typography_kind, AnchorTarget, AvatarSize, ElementAlign, ElementSide, Icon, IdeaThemeRef, Menu, MenuItem,
    MenuLabel, MenuSeparator, Tooltip, Typography,
};
use runtime_core::{
    component, derived, memo, rx, signal, stylesheet, ui, AlignItems, ChildList, Element, FlexDirection,
    JustifyContent, Length, Overflow, Position, PressableHandle, Ref, Signal, StyleProp,
};

use crate::calls::CallStrip;
use crate::bits::{presence_label, Action, Count, NavRow, NavRowSelected, NavRowUnread, PersonAvatar, RowLabel, Section};
use crate::forms::cb;
use crate::shell::{go, open_space};
use crate::store::{store, Dialog, Place, Settings, SpacePage};

// ---------------------------------------------------------------- tap

/// Something to press, holding whatever it is given. The `pressable`
/// primitive has no `ui!` tag (idea-ui owns the `Pressable` name), so this
/// is the one place it is built by hand; everything else presses through
/// this.
#[component(children)]
pub fn Tap(
    #[prop(static, default = Rc::new(|| {}) as Rc<dyn Fn()>)] on_press: Rc<dyn Fn()>,
    #[prop(static, default = None)] style: Option<StyleProp>,
    #[prop(static, default = String::new())] label: String,
    #[prop(static, default = None)] bind: Option<Ref<PressableHandle>>,
    children: Vec<Element>,
) -> Element {
    let mut kids = Vec::with_capacity(children.len());
    for c in children {
        ChildList::append_to(c, &mut kids);
    }
    let mut b = runtime_core::pressable(kids, move || on_press());
    if let Some(s) = style {
        b = b.with_style(s);
    }
    if !label.is_empty() {
        b = b.a11y_label(label);
    }
    if let Some(r) = bind {
        b = b.bind(r);
    }
    runtime_core::IntoElement::into_element(b)
}

/// A style for [`Tap`], from a sheet builder.
pub fn style(s: impl runtime_core::IntoStyleProp) -> Option<StyleProp> {
    Some(s.into_style_prop())
}

// ---------------------------------------------------------------- layout

stylesheet! {
    SidebarRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            height: Length::pct(100.0),
            min_height: 0.0,
        }
    }
}

stylesheet! {
    Rail<IdeaThemeRef> {
        base(t) {
            width: 64.0,
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            background: t.color.surface_alt(),
            padding_top: 12.0,
            padding_bottom: 12.0,
            gap: 8.0,
        }
    }
}

stylesheet! {
    RailList<()> {
        base(_t) {
            flex_grow: 1.0,
            min_height: 0.0,
            width: Length::pct(100.0),
        }
    }
}

stylesheet! {
    RailInner<()> {
        base(_t) {
            // Scroll content grows past the viewport rather than shrinking into it.
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            gap: 8.0,
            padding_bottom: 8.0,
        }
    }
}

stylesheet! {
    RailRule<IdeaThemeRef> {
        base(t) {
            width: 24.0,
            height: 2.0,
            border_radius: 1.0,
            background: t.color.border_hover(),
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    NavColumn<IdeaThemeRef> {
        base(t) {
            width: 248.0,
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            background: t.color.background(),
            min_height: 0.0,
        }
    }
}

stylesheet! {
    NavHeader<IdeaThemeRef> {
        base(t) {
            height: 56.0,
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
            padding_left: 16.0,
            padding_right: 12.0,
            border_bottom_width: 1.0,
            border_color: t.color.border(),
            cursor: runtime_core::Cursor::Pointer,
        }
        state hovered(t) { background: t.color.surface_alt() }
    }
}

stylesheet! {
    /// Home's "find a conversation" box, a button dressed as a field.
    FindBox<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            height: 32.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
            padding_left: 10.0,
            border_radius: 8.0,
            background: t.color.surface_alt(),
            color: t.color.text_muted(),
            cursor: runtime_core::Cursor::Pointer,
        }
        state hovered(t) { color: t.color.text() }
    }
}

stylesheet! {
    NavScroll<()> {
        base(_t) {
            flex_grow: 1.0,
            min_height: 0.0,
        }
    }
}

stylesheet! {
    NavList<()> {
        base(_t) {
            // Scroll content grows past the viewport rather than shrinking into it.
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            gap: 1.0,
            padding_bottom: 12.0,
            padding_top: 4.0,
        }
    }
}

// ---------------------------------------------------------------- sidebar

#[component]
pub fn Sidebar() -> Element {
    let ui = store().ui();
    ui! {
        view(style = SidebarRow()) {
            Switcher()
            view(style = NavColumn()) {
                match ui.place.get().space() {
                    Some(space) => { SpaceNav(space = *space) }
                    None => { HomeNav() }
                }
                CallStrip()
                PresenceStrip()
            }
        }
    }
}

// ---------------------------------------------------------------- switcher

stylesheet! {
    TileWrap<()> {
        base(_t) {
            position: Position::Relative,
            width: 64.0,
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::Center,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    /// A space (or Home) in the switcher: round, squaring off when chosen.
    Tile<IdeaThemeRef> {
        base(t) {
            width: 44.0,
            height: 44.0,
            border_radius: 22.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            background: t.color.surface(),
            color: t.color.text(),
            overflow: Overflow::Hidden,
            cursor: runtime_core::Cursor::Pointer,
        }
        state hovered(t) { border_radius: 14.0, background: t.intent.primary.soft_bg() }
        variant selected {
            #[default]
            off(_t) {}
            on(t) { border_radius: 14.0, background: t.intent.primary.solid_bg(), color: t.intent.primary.solid_text() }
        }
        transitions { border_radius: 160ms EaseOut, background: 160ms EaseOut }
    }
}

stylesheet! {
    /// The bar at the switcher's edge: short for unread, tall for chosen.
    Pill<IdeaThemeRef> {
        base(t) {
            position: Position::Absolute,
            left: 0.0,
            top: 18.0,
            width: 4.0,
            height: 8.0,
            border_top_right_radius: 4.0,
            border_bottom_right_radius: 4.0,
            background: t.color.text(),
        }
        variant mark {
            #[default]
            none(_t) { opacity: 0.0 }
            unread(_t) {}
            selected(_t) { top: 4.0, height: 36.0 }
        }
        transitions { height: 160ms EaseOut, top: 160ms EaseOut, opacity: 160ms EaseOut }
    }
}

stylesheet! {
    Corner<()> {
        base(_t) {
            position: Position::Absolute,
            right: 6.0,
            bottom: -2.0,
        }
    }
}

stylesheet! {
    /// "In a call here": a dot on the space you are talking in.
    CallDot<IdeaThemeRef> {
        base(t) {
            position: Position::Absolute,
            right: 8.0,
            bottom: -1.0,
            width: 14.0,
            height: 14.0,
            border_radius: 999.0,
            border_width: 3.0,
            border_color: t.color.surface_alt(),
            background: t.intent.success.fg(),
        }
    }
}

stylesheet! {
    TileImage<()> {
        base(_t) {
            width: 44.0,
            height: 44.0,
        }
    }
}

/// The initials a space's tile shows without an icon.
fn initials(name: &str) -> String {
    let letters: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(3).collect();
    if letters.is_empty() { "?".into() } else { letters.to_uppercase() }
}

#[component]
fn Switcher() -> Element {
    let s = store();
    let spaces = {
        let s = s.clone();
        memo(move || s.read(|st| st.space_order.iter().copied().filter(|id| st.spaces.contains_key(id)).collect::<Vec<Id>>()))
    };
    let ui = s.ui();
    ui! {
        view(style = Rail()) {
            HomeTile()
            view(style = RailRule()) {}
            scroll_view(style = RailList()) {
                view(style = RailInner()) {
                    for space in spaces, key = space.to_string() {
                        SpaceTile(space = space)
                    }
                    view(style = TileWrap()) {
                        Action(icon = PLUS, label = "Add a space".to_string(), size_px = Some(44.0), tone = tone::Success, variant = idea_ui::variant::Soft, on_click = cb(move || ui.dialog.set(Some(Dialog::CreateSpace))))
                    }
                    view(style = TileWrap()) {
                        Action(icon = COMPASS, label = "Join with an invite".to_string(), size_px = Some(44.0), variant = idea_ui::variant::Soft, on_click = cb(move || ui.dialog.set(Some(Dialog::JoinSpace))))
                    }
                }
            }
        }
    }
}

#[component]
fn HomeTile() -> Element {
    let s = store();
    let ui = s.ui();
    let badge = memo(move || s.read(|st| st.home_badge(s.now())));
    let here = memo(move || ui.place.get().space().is_none());
    ui! {
        view(style = TileWrap()) {
            view(style = Pill().mark(derived(move || if here.get() { PillMark::Selected } else if badge.get().unread { PillMark::Unread } else { PillMark::None }))) {}
            Tooltip(text = "Home".to_string(), side = ElementSide::End) {
                Tap(on_press = cb(|| go(Place::Home)), label = "Home".to_string(), style = style(Tile().selected(derived(move || if here.get() { TileSelected::On } else { TileSelected::Off })))) {
                    Icon(data = MESSAGES_SQUARE, size = 22.0)
                }
            }
            view(style = Corner()) { Count(count = rx!(badge.get().mentions)) }
        }
    }
}

#[component]
fn SpaceTile(#[prop(static)] space: Id) -> Element {
    let s = store();
    let ui = s.ui();
    let info = {
        let s = s.clone();
        memo(move || s.read(|st| st.spaces.get(&space).map(|sp| (sp.name.clone(), sp.icon.clone()))))
    };
    let badge = {
        let s = s.clone();
        memo(move || s.read(|st| st.space_badge(space, s.now())))
    };
    // Whether the person's call is in this space: the switcher marks it, so
    // they can find their way back.
    let calling = memo(move || {
        s.read(|st| st.my_call().and_then(|v| st.channels.get(&v.channel)).is_some_and(|c| c.space == Some(space)))
    });
    let here = memo(move || ui.place.get().space() == Some(space));
    let name = memo(move || info.get().map(|i| i.0).unwrap_or_default());
    ui! {
        view(style = TileWrap()) {
            view(style = Pill().mark(derived(move || if here.get() { PillMark::Selected } else if badge.get().unread { PillMark::Unread } else { PillMark::None }))) {}
            Tooltip(text = rx!(name.get()), side = ElementSide::End) {
                Tap(on_press = cb(move || open_space(space)), label = name.peek(), style = style(Tile().selected(derived(move || if here.get() { TileSelected::On } else { TileSelected::Off })))) {
                    match info.get().and_then(|i| i.1) {
                        Some(icon) => { image(src = crate::platform::absolute(icon), alt = String::new(), style = TileImage()) }
                        None => { Typography(content = initials(&name.peek()), kind = typography_kind::BodySm, weight = Some(runtime_core::FontWeight::SemiBold)) }
                    }
                }
            }
            if calling.get() {
                view(style = CallDot()) {}
            }
            view(style = Corner()) { Count(count = rx!(badge.get().mentions)) }
        }
    }
}

// ---------------------------------------------------------------- home

#[component]
fn HomeNav() -> Element {
    let s = store();
    let ui = s.ui();
    let conversations = {
        let s = s.clone();
        memo(move || s.read(|st| st.conversations().iter().map(|c| c.id).collect::<Vec<Id>>()))
    };
    let requests = memo(move || s.read(|st| st.relations.values().filter(|r| **r == chat_model::Relation::Incoming).count() as u32));
    let on_friends = memo(move || ui.place.get() == Place::Home);
    ui! {
        view(style = crate::bits::Grow()) {
            view(style = NavHeader()) {
                Tap(on_press = cb(move || ui.dialog.set(Some(Dialog::NewMessage))), label = "Find or start a conversation".to_string(), style = style(FindBox())) {
                    Icon(data = SEARCH, size = 15.0)
                    Typography(content = "Find or start a conversation".to_string(), kind = typography_kind::BodySm, muted = true, lines = Some(1))
                }
            }
            scroll_view(style = NavScroll()) {
                view(style = NavList()) {
                    Tap(on_press = cb(|| go(Place::Home)), label = "Friends".to_string(), style = style(NavRow().tall(crate::bits::NavRowTall::On).selected(derived(move || if on_friends.get() { NavRowSelected::On } else { NavRowSelected::Off })))) {
                        Icon(data = USERS, size = 18.0)
                        view(style = RowLabel()) { Typography(content = "Friends".to_string(), weight = Some(runtime_core::FontWeight::Medium)) }
                        Count(count = rx!(requests.get()))
                    }
                    Section(label = "Direct messages".to_string(), action = Some(ui! {
                        Action(icon = PLUS, label = "New message".to_string(), size = idea_ui::IconButtonSize::Sm, on_click = cb(move || ui.dialog.set(Some(Dialog::NewMessage))))
                    }))
                    for channel in conversations, key = channel.to_string() {
                        ConversationRow(channel = channel)
                    }
                }
            }
        }
    }
}

stylesheet! {
    GroupMark<IdeaThemeRef> {
        base(t) {
            width: 32.0,
            height: 32.0,
            border_radius: 10.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            background: t.intent.primary.soft_bg(),
            color: t.intent.primary.soft_text(),
            flex_shrink: 0.0,
        }
    }
}

/// A conversation in Home's list: who it is with, their status, unread.
#[component]
fn ConversationRow(#[prop(static)] channel: Id) -> Element {
    let s = store();
    let ui = s.ui();
    let row = {
        let s = s.clone();
        memo(move || {
            s.read(|st| {
                let c = st.channels.get(&channel)?;
                let partner = st.partner(c);
                let sub = match partner {
                    Some(p) => st.status_text(p).map(str::to_string).unwrap_or_default(),
                    None => format!("{} members", c.members.len()),
                };
                Some((st.title(c), partner, sub, st.unread(channel, s.now()), st.mentions(channel, s.now())))
            })
        })
    };
    let here = memo(move || ui.place.get() == Place::Direct(channel));
    let unread = memo(move || row.get().is_some_and(|r| r.3));
    ui! {
        Tap(
            on_press = cb(move || go(Place::Direct(channel))),
            label = row.peek().map(|r| r.0).unwrap_or_default(),
            style = style(NavRow().tall(crate::bits::NavRowTall::On)
                .selected(derived(move || if here.get() { NavRowSelected::On } else { NavRowSelected::Off }))
                .unread(derived(move || if unread.get() { NavRowUnread::On } else { NavRowUnread::Off }))),
        ) {
            match row.get().and_then(|r| r.1) {
                Some(partner) => { PersonAvatar(user = *partner, size = AvatarSize::Sm) }
                None => { view(style = GroupMark()) { Icon(data = USERS, size = 16.0) } }
            }
            view(style = RowLabel()) {
                Typography(content = rx!(row.get().map(|r| r.0).unwrap_or_default()), lines = Some(1), weight = rx!(Some(if unread.get() { runtime_core::FontWeight::SemiBold } else { runtime_core::FontWeight::Normal })))
                if row.get().is_some_and(|r| !r.2.is_empty()) {
                    Typography(content = rx!(row.get().map(|r| r.2).unwrap_or_default()), kind = typography_kind::Caption, muted = true, lines = Some(1))
                }
            }
            Count(count = rx!(row.get().map_or(0, |r| r.4)))
        }
    }
}

// ---------------------------------------------------------------- a space

/// One line of a space's navigation, as data — what the list is keyed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Line {
    Category { id: Id },
    Channel { id: Id },
    Thread { id: Id },
    InCall { channel: Id, user: Id },
}

impl Line {
    fn key(&self) -> String {
        match self {
            Line::Category { id } => format!("cat:{id}"),
            Line::Channel { id } => format!("ch:{id}"),
            Line::Thread { id } => format!("th:{id}"),
            Line::InCall { channel, user } => format!("call:{channel}:{user}"),
        }
    }
}

#[component]
fn SpaceNav(#[prop(static)] space: Id) -> Element {
    let s = store();
    let ui = s.ui();
    // Categories folded shut, for this visit.
    let folded: Signal<BTreeSet<Id>> = signal(BTreeSet::new());
    let lines = {
        let s = s.clone();
        memo(move || {
            let shut = folded.get();
            let here = ui.place.get().channel();
            s.read(|st| {
                let mut out = Vec::new();
                for section in st.sidebar(space) {
                    if let Some(cat) = section.category {
                        out.push(Line::Category { id: cat.id });
                        if shut.contains(&cat.id) {
                            // A folded category still shows where you are.
                            for e in &section.entries {
                                if Some(e.channel.id) == here {
                                    out.push(Line::Channel { id: e.channel.id });
                                }
                            }
                            continue;
                        }
                    }
                    for e in &section.entries {
                        out.push(Line::Channel { id: e.channel.id });
                        for t in &e.threads {
                            out.push(Line::Thread { id: t.id });
                        }
                        for v in st.in_call(e.channel.id) {
                            out.push(Line::InCall { channel: e.channel.id, user: v.user });
                        }
                    }
                }
                out
            })
        })
    };
    let name = {
        let s = s.clone();
        memo(move || s.read(|st| st.spaces.get(&space).map(|sp| sp.name.clone()).unwrap_or_default()))
    };
    let admin = memo(move || s.manages_space(space));
    let menu_open = signal(false);
    let anchor: Ref<PressableHandle> = Ref::new();
    ui! {
        view(style = crate::bits::Grow()) {
            Tap(on_press = cb(move || menu_open.set(!menu_open.peek())), label = "Space menu".to_string(), bind = Some(anchor), style = style(NavHeader())) {
                view(style = RowLabel()) { Typography(content = rx!(name.get()), kind = typography_kind::BodyLg, weight = Some(runtime_core::FontWeight::SemiBold), lines = Some(1)) }
                Icon(data = CHEVRON_DOWN, size = 16.0)
            }
            if menu_open.get() {
                SpaceMenu(space = space, anchor = anchor, open = menu_open, admin = admin.peek())
            }
            scroll_view(style = NavScroll()) {
                view(style = NavList()) {
                    for line in lines, key = line.key() {
                        match line {
                            Line::Category { id } => { CategoryLine(space = space, category = id, folded = folded, admin = rx!(admin.get())) }
                            Line::Channel { id } => { ChannelLine(space = space, channel = id) }
                            Line::Thread { id } => { ThreadLine(space = space, channel = id) }
                            Line::InCall { channel, user } => { InCallLine(channel = channel, user = user) }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SpaceMenu(
    #[prop(static)] space: Id,
    #[prop(static, default = Ref::new())] anchor: Ref<PressableHandle>,
    #[prop(static, default = signal(false))] open: Signal<bool>,
    #[prop(static)] admin: bool,
) -> Element {
    let s = store();
    let ui = s.ui();
    let muted = s.peek(|st| st.muted.contains(&space));
    let close = move || open.set(false);
    let item = move |f: Rc<dyn Fn()>| -> Rc<dyn Fn()> {
        Rc::new(move || {
            close();
            f();
        })
    };
    let leave = item(cb(move || {
        runtime_core::spawn_then(chat_api::spaces::leave_space(space), move |r| {
            if let Err(e) = r {
                idea_ui::push_toast(chat_api::describe(&e), tone::Danger);
            } else {
                go(Place::Home);
            }
        })
    }));
    let mute = item(cb(move || runtime_core::spawn_then(chat_api::spaces::set_muted(space, !muted), |_| {})));
    ui! {
        Menu(target = Some(AnchorTarget::from(anchor)), on_dismiss = Some(cb(close)), align = ElementAlign::Start) {
            MenuItem(label = "Invite people".to_string(), leading = Some(ui! { Icon(data = USER_PLUS, size = 15.0) }), on_select = item(cb(move || ui.dialog.set(Some(Dialog::Invite(space))))))
            if admin {
                MenuItem(label = "Space settings".to_string(), leading = Some(ui! { Icon(data = SETTINGS, size = 15.0) }), on_select = item(cb(move || ui.settings.set(Some(Settings::Space(space, SpacePage::Overview))))))
                MenuItem(label = "Create a channel".to_string(), leading = Some(ui! { Icon(data = PLUS, size = 15.0) }), on_select = item(cb(move || ui.dialog.set(Some(Dialog::CreateChannel { space, voice: false, category: None })))))
                MenuItem(label = "Create a category".to_string(), leading = Some(ui! { Icon(data = FOLDER_PLUS, size = 15.0) }), on_select = item(cb(move || {
                    runtime_core::spawn_then(chat_api::spaces::create_category(space, "New category".into()), |_| {})
                })))
            }
            MenuSeparator()
            MenuItem(label = if muted { "Unmute space".to_string() } else { "Mute space".to_string() }, leading = Some(ui! { Icon(data = BELL_OFF, size = 15.0) }), on_select = mute)
            MenuItem(label = "Leave space".to_string(), leading = Some(ui! { Icon(data = LOG_OUT, size = 15.0, tone = Some(tone::Danger.into())) }), on_select = leave)
        }
    }
}

stylesheet! {
    CategoryRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 2.0,
            padding_left: 8.0,
            padding_right: 10.0,
            padding_top: 14.0,
            padding_bottom: 4.0,
            color: t.color.text_muted(),
            cursor: runtime_core::Cursor::Pointer,
        }
        state hovered(t) { color: t.color.text() }
    }
}

#[component]
fn CategoryLine(
    #[prop(static)] space: Id,
    #[prop(static)] category: Id,
    #[prop(static, default = signal(BTreeSet::new()))] folded: Signal<BTreeSet<Id>>,
    admin: bool,
) -> Element {
    let s = store();
    let ui = s.ui();
    let name = memo(move || {
        s.read(|st| {
            st.spaces.get(&space).and_then(|sp| sp.categories.iter().find(|c| c.id == category).map(|c| c.name.clone())).unwrap_or_default()
        })
    });
    let shut = memo(move || folded.get().contains(&category));
    let toggle = cb(move || {
        let mut f = folded.peek();
        if !f.remove(&category) {
            f.insert(category);
        }
        folded.set(f);
    });
    ui! {
        view(style = CategoryRow()) {
            Tap(on_press = toggle, label = rx!(name.get()).get(), style = style(RowLabel())) {
                view(style = InlineRow()) {
                    Icon(data = rx!(if shut.get() { CHEVRON_RIGHT } else { CHEVRON_DOWN }).get(), size = 12.0)
                    Typography(content = rx!(name.get().to_uppercase()), kind = typography_kind::Overline, muted = true, lines = Some(1))
                }
            }
            if admin.get() {
                Action(icon = PLUS, label = "Create a channel here".to_string(), size = idea_ui::IconButtonSize::Sm, on_click = cb(move || ui.dialog.set(Some(Dialog::CreateChannel { space, voice: false, category: Some(category) }))))
            }
        }
    }
}

stylesheet! {
    InlineRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 4.0,
            min_width: 0.0,
        }
    }
}

/// What a channel row shows, as data.
#[derive(Debug, Clone, PartialEq)]
struct ChannelView {
    name: String,
    kind: ChannelKind,
    access: Access,
    unread: bool,
    mentions: u32,
    muted: bool,
    in_call: usize,
}

#[component]
fn ChannelLine(#[prop(static)] space: Id, #[prop(static)] channel: Id) -> Element {
    let s = store();
    let ui = s.ui();
    let view = memo(move || {
        s.read(|st| {
            let c = st.channels.get(&channel)?;
            Some(ChannelView {
                name: c.name.clone(),
                kind: c.kind,
                access: st.access(channel),
                unread: st.unread(channel, s.now()),
                mentions: st.mentions(channel, s.now()),
                muted: st.muted.contains(&channel),
                in_call: st.in_call(channel).len(),
            })
        })
    });
    let here = memo(move || ui.place.get().channel() == Some(channel));
    let bright = memo(move || view.get().is_some_and(|v| v.unread && !v.muted));
    let icon = match view.peek().map(|v| (v.kind, v.access)) {
        Some((_, Access::Locked)) => LOCK,
        Some((ChannelKind::Voice, _)) => VOLUME_2,
        _ => HASH,
    };
    ui! {
        Tap(
            on_press = cb(move || go(Place::Channel { space, channel })),
            label = view.peek().map(|v| v.name).unwrap_or_default(),
            style = style(NavRow()
                .selected(derived(move || if here.get() { NavRowSelected::On } else { NavRowSelected::Off }))
                .unread(derived(move || if bright.get() { NavRowUnread::On } else { NavRowUnread::Off }))),
        ) {
            Icon(data = icon, size = 16.0)
            view(style = RowLabel()) {
                Typography(content = rx!(view.get().map(|v| v.name).unwrap_or_default()), lines = Some(1), weight = rx!(Some(if bright.get() { runtime_core::FontWeight::SemiBold } else { runtime_core::FontWeight::Normal })))
            }
            Count(count = rx!(view.get().map_or(0, |v| v.mentions)))
        }
    }
}

stylesheet! {
    /// A thread under its channel, joined to it by a line.
    ThreadIndent<IdeaThemeRef> {
        base(t) {
            margin_left: 24.0,
            border_left_width: 2.0,
            border_color: t.color.border(),
        }
    }
}

#[component]
fn ThreadLine(#[prop(static)] space: Id, #[prop(static)] channel: Id) -> Element {
    let s = store();
    let ui = s.ui();
    let view = memo(move || s.read(|st| st.channels.get(&channel).map(|c| (c.name.clone(), st.unread(channel, s.now()), st.mentions(channel, s.now())))));
    let here = memo(move || ui.place.get().channel() == Some(channel));
    let bright = memo(move || view.get().is_some_and(|v| v.1));
    ui! {
        view(style = ThreadIndent()) {
            Tap(
                on_press = cb(move || go(Place::Channel { space, channel })),
                label = view.peek().map(|v| v.0).unwrap_or_default(),
                style = style(NavRow()
                    .selected(derived(move || if here.get() { NavRowSelected::On } else { NavRowSelected::Off }))
                    .unread(derived(move || if bright.get() { NavRowUnread::On } else { NavRowUnread::Off }))),
            ) {
                view(style = RowLabel()) {
                    Typography(content = rx!(view.get().map(|v| v.0).unwrap_or_default()), kind = typography_kind::BodySm, lines = Some(1))
                }
                Count(count = rx!(view.get().map_or(0, |v| v.2)))
            }
        }
    }
}

stylesheet! {
    InCallRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
            min_height: 28.0,
            padding_left: 40.0,
            padding_right: 12.0,
            color: t.color.text_muted(),
        }
    }
}

/// Someone in a voice channel's call, listed under it.
#[component]
fn InCallLine(#[prop(static)] channel: Id, #[prop(static)] user: Id) -> Element {
    let s = store();
    let view = memo(move || {
        s.read(|st| st.voice.get(&channel).and_then(|p| p.get(&user)).map(|v| (st.display_name(user), v.muted, v.deafened)))
    });
    ui! {
        view(style = InCallRow()) {
            PersonAvatar(user = user, size = AvatarSize::Xs, presence = false, speaking = true)
            view(style = RowLabel()) {
                Typography(content = rx!(view.get().map(|v| v.0).unwrap_or_default()), kind = typography_kind::BodySm, lines = Some(1))
            }
            if view.get().is_some_and(|v| v.2) {
                Icon(data = HEADPHONE_OFF, size = 13.0)
            } else {
                if view.get().is_some_and(|v| v.1) {
                    Icon(data = MIC_OFF, size = 13.0)
                }
            }
        }
    }
}

// ---------------------------------------------------------------- you

stylesheet! {
    Strip<IdeaThemeRef> {
        base(t) {
            height: 60.0,
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
            padding_left: 10.0,
            padding_right: 8.0,
            background: t.color.surface_alt(),
        }
    }
}

stylesheet! {
    Me<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            flex_basis: 0.0,
            min_width: 0.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
            padding_left: 4.0,
            padding_right: 4.0,
            padding_top: 4.0,
            padding_bottom: 4.0,
            border_radius: 8.0,
            cursor: runtime_core::Cursor::Pointer,
        }
        state hovered(t) { background: t.color.border() }
    }
}

/// You, at the sidebar's foot: your presence and status, and your settings.
#[component]
fn PresenceStrip() -> Element {
    let s = store();
    let ui = s.ui();
    let me = {
        let s = s.clone();
        memo(move || s.read(|st| st.me.as_ref().map(|m| (m.user.id, m.user.display_name.clone(), m.status.clone()))))
    };
    let menu_open = signal(false);
    let anchor: Ref<PressableHandle> = Ref::new();
    ui! {
        view(style = Strip()) {
            Tap(on_press = cb(move || menu_open.set(!menu_open.peek())), label = "Set your status".to_string(), bind = Some(anchor), style = style(Me())) {
                match me.get().map(|m| m.0) {
                    Some(id) => { PersonAvatar(user = *id, size = AvatarSize::Sm, speaking = true) }
                    None => { runtime_core::fragment(Vec::new()) }
                }
                view(style = RowLabel()) {
                    Typography(content = rx!(me.get().map(|m| m.1).unwrap_or_default()), kind = typography_kind::BodySm, weight = Some(runtime_core::FontWeight::SemiBold), lines = Some(1))
                    Typography(content = rx!(me.get().map(|m| m.2.text.clone().unwrap_or_else(|| presence_label(m.2.presence).to_string())).unwrap_or_default()), kind = typography_kind::Caption, muted = true, lines = Some(1))
                }
            }
            Action(icon = rx!(if ui.dark.get() { SUN } else { MOON }).get(), label = "Switch theme".to_string(), on_click = cb(move || ui.dark.set(!ui.dark.peek())))
            Action(icon = SETTINGS, label = "Settings".to_string(), on_click = cb(move || ui.settings.set(Some(Settings::Profile))))
        }
        if menu_open.get() {
            StatusMenu(anchor = anchor, open = menu_open)
        }
    }
}

#[component]
fn StatusMenu(#[prop(static, default = Ref::new())] anchor: Ref<PressableHandle>, #[prop(static, default = signal(false))] open: Signal<bool>) -> Element {
    let s = store();
    let current = s.peek(|st| st.me.as_ref().map(|m| m.status.clone())).unwrap_or_default();
    let status_text = current.text.clone();
    let set = move |presence: Presence| -> Rc<dyn Fn()> {
        let text = status_text.clone();
        Rc::new(move || {
            open.set(false);
            runtime_core::spawn_then(chat_api::auth::set_status(Status { presence, text: text.clone() }), |_| {});
        })
    };
    let clear_text = {
        let presence = current.presence;
        cb(move || {
            open.set(false);
            runtime_core::spawn_then(chat_api::auth::set_status(Status { presence, text: None }), |_| {});
        })
    };
    let ui = s.ui();
    ui! {
        Menu(target = Some(AnchorTarget::from(anchor)), on_dismiss = Some(cb(move || open.set(false))), side = ElementSide::Above, align = ElementAlign::Start) {
            MenuLabel(text = "Show me as".to_string())
            MenuItem(label = "Online".to_string(), active = current.presence == Presence::Online, on_select = set(Presence::Online))
            MenuItem(label = "Away".to_string(), active = current.presence == Presence::Idle, on_select = set(Presence::Idle))
            MenuItem(label = "Do not disturb".to_string(), active = current.presence == Presence::Busy, on_select = set(Presence::Busy))
            MenuItem(label = "Invisible".to_string(), active = current.presence == Presence::Offline, on_select = set(Presence::Offline))
            MenuSeparator()
            MenuItem(label = "Set a status message…".to_string(), on_select = cb(move || { open.set(false); ui.settings.set(Some(Settings::Profile)); }))
            if current.text.is_some() {
                MenuItem(label = "Clear status message".to_string(), on_select = clear_text.clone())
            }
        }
    }
}
