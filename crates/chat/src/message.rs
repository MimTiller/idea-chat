//! One message, as a row of a channel's stream: who and when, the text with
//! its formatting and mentions, what it replies to, its files, reactions and
//! thread — and, on hover, what can be done with it.

use std::rc::Rc;

use chat_model::text::{self, Block, Inline};
use chat_model::{Attachment, CallRecord, Id, Message, MessageKind};
use icons_lucide::{
    ARROW_RIGHT, CORNER_DOWN_RIGHT, DOWNLOAD, FILE, MESSAGES_SQUARE, PENCIL, PHONE, PHONE_MISSED, REPLY, SMILE,
    TRASH_2,
};
use idea_ui::{
    tone, typography_kind, variant, AnchorTarget, AvatarSize, Button, ElementAlign, Field, Icon, IdeaThemeRef, Image, Menu,
    MenuItem, Typography,
};
use runtime_core::{
    component, derived, memo, rx, signal, stylesheet, ui, AlignItems, Color, Element, FlexDirection, FlexWrap,
    FontFamily, FontWeight, JustifyContent, KeyOutcome, Length, Overflow, Position, PressableHandle, Ref, Signal,
    Tokenized,
};
use runtime_core::styled_text::{TextRun, TextRunStyle};

use crate::bits::{clock, duration, size, stamp, Action, PersonAvatar};
use crate::forms::{cb, set};
use crate::nav::{style, Tap};
use crate::platform;
use crate::store::{store, Dialog, Place};

/// The reactions offered with one press.
pub const QUICK: [&str; 6] = ["👍", "❤️", "😂", "🎉", "😮", "😢"];

stylesheet! {
    pub Row<IdeaThemeRef> {
        base(t) {
            position: Position::Relative,
            flex_direction: FlexDirection::Row,
            gap: 12.0,
            padding_left: 16.0,
            padding_right: 16.0,
            padding_top: 2.0,
            padding_bottom: 2.0,
        }
        state hovered(t) { background: t.color.background() }
        variant first {
            #[default]
            off(_t) {}
            on(_t) { padding_top: 10.0 }
        }
        variant mentioned {
            #[default]
            off(_t) {}
            on(t) { background: t.intent.danger.soft_bg() }
        }
        variant focused {
            #[default]
            off(_t) {}
            on(t) { background: t.intent.primary.soft_bg() }
        }
        transitions { background: 400ms EaseOut }
    }
}

stylesheet! {
    Gutter<()> {
        base(_t) {
            width: 40.0,
            flex_shrink: 0.0,
            align_items: AlignItems::Center,
            padding_top: 2.0,
        }
    }
}

stylesheet! {
    Content<()> {
        base(_t) {
            flex_grow: 1.0,
            flex_basis: 0.0,
            min_width: 0.0,
            flex_direction: FlexDirection::Column,
            gap: 4.0,
        }
    }
}

stylesheet! {
    Header<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
            flex_wrap: FlexWrap::Wrap,
        }
    }
}

stylesheet! {
    pub Paragraph<IdeaThemeRef> {
        base(t) {
            color: t.color.text(),
            font_size: 15.0,
            // Pixels, not a multiplier: 15px type on a 22px line.
            line_height: 22.0,
        }
    }
}

stylesheet! {
    Quote<IdeaThemeRef> {
        base(t) {
            border_left_width: 3.0,
            border_color: t.color.border_hover(),
            padding_left: 10.0,
        }
    }
}

stylesheet! {
    CodeBlock<IdeaThemeRef> {
        base(t) {
            background: t.color.background(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: 6.0,
            padding_left: 10.0,
            padding_right: 10.0,
            padding_top: 8.0,
            padding_bottom: 8.0,
            font_family: crate::forms::mono(),
            font_size: 13.0,
            color: t.color.text(),
            overflow: Overflow::Hidden,
        }
    }
}

stylesheet! {
    ReplyLine<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 6.0,
            color: t.color.text_muted(),
            cursor: runtime_core::Cursor::Pointer,
            min_width: 0.0,
        }
        state hovered(t) { color: t.color.text() }
    }
}

stylesheet! {
    Files<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            gap: 8.0,
            padding_top: 2.0,
        }
    }
}

stylesheet! {
    Picture<IdeaThemeRef> {
        base(t) {
            max_width: Length::pct(100.0),
            border_radius: 10.0,
            overflow: Overflow::Hidden,
            background: t.color.surface_alt(),
            cursor: runtime_core::Cursor::Pointer,
        }
    }
}

stylesheet! {
    FileCard<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 10.0,
            padding_left: 12.0,
            padding_right: 8.0,
            padding_top: 10.0,
            padding_bottom: 10.0,
            border_radius: 10.0,
            border_width: 1.0,
            border_color: t.color.border(),
            background: t.color.background(),
            max_width: 360.0,
            min_width: 0.0,
        }
    }
}

stylesheet! {
    Reactions<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            gap: 6.0,
            padding_top: 2.0,
        }
    }
}

stylesheet! {
    Chip<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 6.0,
            height: 26.0,
            padding_left: 8.0,
            padding_right: 8.0,
            border_radius: 13.0,
            border_width: 1.0,
            border_color: t.color.border(),
            background: t.color.background(),
            cursor: runtime_core::Cursor::Pointer,
        }
        state hovered(t) { border_color: t.color.border_strong() }
        variant mine {
            #[default]
            off(_t) {}
            on(t) { background: t.intent.primary.soft_bg(), border_color: t.intent.primary.border() }
        }
    }
}

stylesheet! {
    ThreadChip<IdeaThemeRef> {
        base(t) {
            align_self: runtime_core::AlignSelf::FlexStart,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
            padding_left: 10.0,
            padding_right: 12.0,
            padding_top: 6.0,
            padding_bottom: 6.0,
            border_radius: 8.0,
            background: t.color.background(),
            border_width: 1.0,
            border_color: t.color.border(),
            cursor: runtime_core::Cursor::Pointer,
        }
        state hovered(t) { border_color: t.intent.primary.border() }
    }
}

stylesheet! {
    /// The actions that float over a message's corner on hover.
    Tools<IdeaThemeRef> {
        base(t) {
            position: Position::Absolute,
            right: 16.0,
            top: -14.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 2.0,
            padding_left: 4.0,
            padding_right: 4.0,
            padding_top: 2.0,
            padding_bottom: 2.0,
            border_radius: 8.0,
            border_width: 1.0,
            border_color: t.color.border(),
            background: t.color.surface(),
            shadow: runtime_core::Shadow { x: 0.0, y: 4.0, blur: 14.0, color: Tokenized::Literal(Color("rgba(0,0,0,0.10)".into())) },
        }
    }
}

stylesheet! {
    SystemRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 12.0,
            padding_left: 16.0,
            padding_right: 16.0,
            padding_top: 6.0,
            padding_bottom: 6.0,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    SystemIcon<()> {
        base(_t) {
            width: 40.0,
            flex_shrink: 0.0,
            align_items: AlignItems::Center,
        }
    }
}

stylesheet! {
    EditBox<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 4.0,
        }
    }
}

// ---------------------------------------------------------------- text

fn tok_primary() -> Tokenized<Color> {
    idea_ui::tokens().intent.primary.fg()
}

fn tok_primary_soft() -> Tokenized<Color> {
    idea_ui::tokens().intent.primary.soft_bg()
}

/// A block's runs, as styled text runs: formatting, mentions as names in
/// the accent, links underlined.
fn runs(inlines: &[Inline], name: &impl Fn(Id) -> String, channel: &impl Fn(Id) -> String) -> Vec<TextRun> {
    inlines
        .iter()
        .map(|run| match run {
            Inline::Text { text, style } => {
                let mut s = TextRunStyle::default();
                if style.bold {
                    s.font_weight = Some(FontWeight::Bold);
                }
                if style.italic {
                    s.font_style = Some(runtime_core::FontStyle::Italic);
                }
                if style.code {
                    s.font_family = Some(crate::forms::mono());
                    s.background = Some(idea_ui::tokens().color.surface_alt());
                }
                if style.strike {
                    // No strike-through across every backend's run styling:
                    // struck text is shown muted instead.
                    s.color = Some(idea_ui::tokens().color.text_muted());
                }
                if s == TextRunStyle::default() { TextRun::plain(text.clone()) } else { TextRun::styled(text.clone(), s) }
            }
            Inline::Link { url } => TextRun::styled(
                url.clone(),
                // Coloured, not underlined: run underlines are not on the
                // author surface at this framework version. The link is
                // also listed, pressable, under the message.
                TextRunStyle { color: Some(tok_primary()), ..Default::default() },
            ),
            Inline::User(id) => TextRun::styled(
                format!("@{}", name(*id)),
                TextRunStyle { color: Some(tok_primary()), background: Some(tok_primary_soft()), font_weight: Some(FontWeight::Medium), ..Default::default() },
            ),
            Inline::Role(id) => TextRun::styled(
                format!("@{}", name(*id)),
                TextRunStyle { color: Some(tok_primary()), font_weight: Some(FontWeight::Medium), ..Default::default() },
            ),
            Inline::Channel(id) => TextRun::styled(
                format!("#{}", channel(*id)),
                TextRunStyle { color: Some(tok_primary()), background: Some(tok_primary_soft()), font_weight: Some(FontWeight::Medium), ..Default::default() },
            ),
            Inline::Everyone => TextRun::styled(
                "@everyone".to_string(),
                TextRunStyle { color: Some(tok_primary()), background: Some(tok_primary_soft()), font_weight: Some(FontWeight::Medium), ..Default::default() },
            ),
        })
        .collect()
}

/// A paragraph of styled runs. Built with the styled-text builder: `ui!`
/// has no form for a run list, which is one text node rather than children.
fn paragraph(runs: Vec<TextRun>) -> Element {
    runtime_core::IntoElement::into_element(runtime_core::styled_text(runs).with_style(Paragraph()))
}

/// The links in a body, in order, each once.
fn links(blocks: &[Block]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for b in blocks {
        if let Block::Paragraph(r) | Block::Quote(r) = b {
            for run in r {
                if let Inline::Link { url } = run {
                    if !out.contains(url) {
                        out.push(url.clone());
                    }
                }
            }
        }
    }
    out
}

/// A message body, drawn.
#[component]
pub fn Body(#[prop(static)] body: String) -> Element {
    let s = store();
    let name = |id: Id| s.peek(|st| st.users.get(&id).map(|u| u.display_name.clone()).unwrap_or_else(|| { let label = st.role_label(id); if label.is_empty() { "someone".into() } else { label } }));
    let channel = |id: Id| s.peek(|st| st.channels.get(&id).map(|c| c.name.clone()).unwrap_or_else(|| "channel".into()));
    let blocks = text::parse(&body);
    let urls = links(&blocks);
    ui! {
        view(style = BodyCol()) {
            for block in blocks {
                match block {
                    Block::Paragraph(r) => { paragraph(runs(&r, &name, &channel)) }
                    Block::Quote(r) => { view(style = Quote()) { paragraph(runs(&r, &name, &channel)) } }
                    Block::Code(code) => { view(style = CodeBlock()) { text { code } } }
                }
            }
            for url in urls {
                link(external = url.clone(), style = LinkLine()) {
                    Typography(content = url.clone(), kind = typography_kind::Caption, tone = Some(tone::Primary.into()), lines = Some(1))
                }
            }
        }
    }
}

stylesheet! {
    LinkLine<()> {
        base(_t) {
            align_self: runtime_core::AlignSelf::FlexStart,
            max_width: Length::pct(100.0),
        }
    }
}

// ---------------------------------------------------------------- the row

/// One message in a stream.
#[component]
pub fn MessageRow(#[prop(static)] channel: Id, #[prop(static)] id: Id, #[prop(static)] continued: bool) -> Element {
    let s = store();
    let ui = s.ui();
    let msg = {
        let s = s.clone();
        memo(move || s.message(channel, id))
    };
    let Some(first) = msg.peek() else { return runtime_core::fragment(Vec::new()) };
    match first.kind {
        MessageKind::Text => {}
        _ => return ui! { SystemLine(channel = channel, id = id) },
    }
    let me = s.me();
    let author = first.author;
    let mentioned = me.is_some_and(|m|
        first.author != m && first.mentions_user(m, &s.read(|st| st.my_roles(channel)))
    );
    let hovered = signal(false);
    let editing = memo(move || ui.editing.get() == Some(id));
    let focused = memo(move || ui.focus.get() == Some(id));
    let name = {
        let s = s.clone();
        memo(move || s.read(|st| st.display_name(author)))
    };
    let now = {
        let s = s.clone();
        memo(move || s.now() / 60_000)
    };
    let body = memo(move || msg.get().map(|m| m.body).unwrap_or_default());
    let edited = memo(move || msg.get().is_some_and(|m| m.edited_at.is_some()));
    let reply = memo(move || msg.get().and_then(|m| m.reply_to));
    let files = memo(move || msg.get().map(|m| m.attachments).unwrap_or_default());
    let reactions = memo(move || msg.get().map(|m| m.reactions).unwrap_or_default());
    let thread = memo(move || msg.get().and_then(|m| m.thread));
    let at = first.at;
    let row: Element = ui! {
        view(
            style = Row()
                .first(if continued { RowFirst::Off } else { RowFirst::On })
                .mentioned(if mentioned { RowMentioned::On } else { RowMentioned::Off })
                .focused(derived(move || if focused.get() { RowFocused::On } else { RowFocused::Off })),
            on_hover = move |h: bool| hovered.set(h),
        ) {
            view(style = Gutter()) {
                if !continued {
                    Tap(on_press = cb(move || ui.profile.set(Some(author))), label = "Open profile".to_string()) {
                        PersonAvatar(user = author, size = AvatarSize::Md, presence = false)
                    }
                } else {
                    if hovered.get() {
                        Typography(content = clock(at), kind = typography_kind::Caption, muted = true)
                    }
                }
            }
            view(style = Content()) {
                if !continued {
                    view(style = Header()) {
                        Tap(on_press = cb(move || ui.profile.set(Some(author))), label = "Open profile".to_string()) {
                            Typography(content = rx!(name.get()), weight = Some(FontWeight::SemiBold))
                        }
                        Typography(content = rx!({ now.get(); stamp(at, platform::now_ms()) }), kind = typography_kind::Caption, muted = true)
                    }
                }
                match reply.get() {
                    Some(r) => { ReplyQuote(channel = channel, reply = r.clone()) }
                    None => { runtime_core::fragment(Vec::new()) }
                }
                if editing.get() {
                    EditMessage(id = id, body = body.peek())
                } else {
                    match body.get() {
                        b if b.is_empty() => { runtime_core::fragment(Vec::new()) }
                        b => { Body(body = b.clone()) }
                    }
                    if edited.get() {
                        Typography(content = "(edited)".to_string(), kind = typography_kind::Caption, muted = true)
                    }
                }
                if !files.get().is_empty() {
                    view(style = Files()) {
                        for file in files, key = file.id.to_string() {
                            FileView(file = file.clone())
                        }
                    }
                }
                if !reactions.get().is_empty() {
                    view(style = Reactions()) {
                        for r in reactions, key = r.emoji.clone() {
                            ReactionChip(id = id, emoji = r.emoji.clone(), count = r.users.len() as u32, mine = me.is_some_and(|m| r.users.contains(&m)))
                        }
                    }
                }
                match thread.get() {
                    Some(t) => { ThreadLinkChip(channel = channel, thread = t.id, name = t.name.clone(), count = t.message_count) }
                    None => { runtime_core::fragment(Vec::new()) }
                }
            }
            if hovered.get() && !editing.get() {
                RowTools(channel = channel, id = id, author = author, hovered = hovered)
            }
        }
    };
    row
}

#[component]
fn ReplyQuote(
    #[prop(static)] channel: Id,
    #[prop(static, default = chat_model::ReplyTo { id: Id::nil(), author: Id::nil(), excerpt: String::new() })] reply: chat_model::ReplyTo,
) -> Element {
    let s = store();
    let gone = reply.excerpt.is_empty();
    let author = reply.author;
    let name = s.peek(|st| st.display_name(author));
    let excerpt = if gone {
        "Original message was deleted".to_string()
    } else {
        text::plain(&reply.excerpt, |u| s.peek(|st| st.display_name(u)), |c| s.peek(|st| st.channels.get(&c).map(|c| c.name.clone()).unwrap_or_default()), |r| s.peek(|st| st.role_label(r)), 90)
    };
    let target = reply.id;
    ui! {
        Tap(on_press = cb(move || if !gone { store().jump(channel, target) }), label = "Go to the message replied to".to_string(), style = style(ReplyLine())) {
            Icon(data = CORNER_DOWN_RIGHT, size = 14.0)
            if !gone {
                PersonAvatar(user = author, size = AvatarSize::Xs, presence = false)
                Typography(content = name.clone(), kind = typography_kind::Caption, weight = Some(FontWeight::SemiBold))
            }
            Typography(content = excerpt, kind = typography_kind::Caption, muted = true, lines = Some(1))
        }
    }
}

#[component]
fn FileView(#[prop(static, default = Attachment {
    id: Id::nil(), name: String::new(), size: 0, content_type: String::new(), url: String::new(), width: None, height: None,
})] file: Attachment) -> Element {
    let ui = store().ui();
    let url = platform::absolute(&file.url);
    if file.is_image() {
        // Fit within 360×280, keeping the picture's shape.
        let (w, h) = match (file.width, file.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => {
                let scale = (360.0 / w as f32).min(280.0 / h as f32).min(1.0);
                (w as f32 * scale, h as f32 * scale)
            }
            _ => (280.0, 200.0),
        };
        let open = file.clone();
        return ui! {
            Tap(on_press = cb(move || ui.viewer.set(Some(open.clone()))), label = format!("Open {}", file.name), style = style(Picture())) {
                Image(src = url.clone(), alt = file.name.clone(), width = Some(w), height = Some(h))
            }
        };
    }
    ui! {
        view(style = FileCard()) {
            Icon(data = FILE, size = 22.0, tone = Some(tone::Primary.into()))
            view(style = crate::bits::RowLabel()) {
                Typography(content = file.name.clone(), weight = Some(FontWeight::Medium), lines = Some(1))
                Typography(content = size(file.size), kind = typography_kind::Caption, muted = true)
            }
            link(external = url.clone()) {
                Icon(data = DOWNLOAD, size = 18.0, tone = Some(tone::Neutral.into()))
            }
        }
    }
}

#[component]
fn ReactionChip(#[prop(static)] id: Id, #[prop(static)] emoji: String, #[prop(static)] count: u32, #[prop(static)] mine: bool) -> Element {
    let toggle = {
        let emoji = emoji.clone();
        cb(move || runtime_core::spawn_then(chat_api::messages::react(id, emoji.clone(), !mine), |r| {
            if let Err(e) = r {
                idea_ui::push_toast(chat_api::describe(&e), tone::Danger);
            }
        }))
    };
    ui! {
        Tap(on_press = toggle, label = format!("{emoji} {count}"), style = style(Chip().mine(if mine { ChipMine::On } else { ChipMine::Off }))) {
            text { emoji.clone() }
            Typography(content = count.to_string(), kind = typography_kind::Caption, weight = Some(FontWeight::SemiBold))
        }
    }
}

#[component]
fn ThreadLinkChip(#[prop(static)] channel: Id, #[prop(static)] thread: Id, #[prop(static)] name: String, #[prop(static)] count: u32) -> Element {
    let space = store().peek(|st| st.channels.get(&channel).and_then(|c| c.space));
    let open = cb(move || {
        if let Some(space) = space {
            crate::shell::go(Place::Channel { space, channel: thread });
        }
    });
    let replies = match count {
        0 => "No replies yet".to_string(),
        1 => "1 reply".to_string(),
        n => format!("{n} replies"),
    };
    ui! {
        Tap(on_press = open, label = format!("Open thread {name}"), style = style(ThreadChip())) {
            Icon(data = MESSAGES_SQUARE, size = 15.0, tone = Some(tone::Primary.into()))
            Typography(content = name.clone(), kind = typography_kind::BodySm, weight = Some(FontWeight::SemiBold), lines = Some(1))
            Typography(content = replies, kind = typography_kind::Caption, tone = Some(tone::Primary.into()))
            Icon(data = ARROW_RIGHT, size = 14.0)
        }
    }
}

/// What can be done to a message, floating over its corner.
#[component]
fn RowTools(#[prop(static)] channel: Id, #[prop(static)] id: Id, #[prop(static)] author: Id, #[prop(static, default = signal(false))] hovered: Signal<bool>) -> Element {
    let s = store();
    let ui = s.ui();
    let me = s.me();
    let (in_space, can_delete, can_post, is_thread) = s.peek(|st| {
        let c = st.channels.get(&channel);
        let space = c.and_then(|c| c.space);
        (
            space.is_some(),
            c.is_some_and(|c| chat_model::perms::can_delete_message(c, st.space_of(c), st.me_id().unwrap_or_default(), author)),
            st.access(channel).can_post(),
            c.is_some_and(|c| c.kind == chat_model::ChannelKind::Thread),
        )
    });
    let mine = me == Some(author);
    let picker = signal(false);
    let anchor: Ref<PressableHandle> = Ref::new();
    let react = move |emoji: &'static str| -> Rc<dyn Fn()> {
        Rc::new(move || {
            picker.set(false);
            runtime_core::spawn_then(chat_api::messages::react(id, emoji.to_string(), true), |_| {});
        })
    };
    let reply = cb(move || {
        ui.reply.update(|r| {
            let mut r = r.clone();
            r.insert(channel, id);
            r
        });
        crate::composer::focus(channel);
    });
    let thread = cb(move || ui.dialog.set(Some(Dialog::StartThread { channel, from: Some(id) })));
    let edit = cb(move || ui.editing.set(Some(id)));
    let delete = cb(move || {
        runtime_core::spawn_then(chat_api::messages::delete(id), |r| {
            if let Err(e) = r {
                idea_ui::push_toast(chat_api::describe(&e), tone::Danger);
            }
        })
    });
    ui! {
        view(style = Tools()) {
            Tap(on_press = cb(move || picker.set(!picker.peek())), label = "Add a reaction".to_string(), bind = Some(anchor)) {
                Action(icon = SMILE, label = "Add a reaction".to_string(), size = idea_ui::IconButtonSize::Sm)
            }
            if can_post {
                Action(icon = REPLY, label = "Reply".to_string(), size = idea_ui::IconButtonSize::Sm, on_click = reply.clone())
            }
            if can_post && in_space && !is_thread {
                Action(icon = MESSAGES_SQUARE, label = "Start a thread".to_string(), size = idea_ui::IconButtonSize::Sm, on_click = thread.clone())
            }
            if mine && can_post {
                Action(icon = PENCIL, label = "Edit".to_string(), size = idea_ui::IconButtonSize::Sm, on_click = edit.clone())
            }
            if can_delete {
                Action(icon = TRASH_2, label = "Delete".to_string(), size = idea_ui::IconButtonSize::Sm, tone = tone::Danger, on_click = delete.clone())
            }
        }
        if picker.get() {
            Menu(target = Some(AnchorTarget::from(anchor)), on_dismiss = Some(cb(move || { picker.set(false); hovered.set(false); })), align = ElementAlign::End) {
                for emoji in QUICK.to_vec() {
                    MenuItem(label = emoji.to_string(), on_select = react(emoji))
                }
            }
        }
    }
}

/// Editing a message in place: Enter saves, Escape cancels.
#[component]
fn EditMessage(#[prop(static)] id: Id, #[prop(static)] body: String) -> Element {
    let ui = store().ui();
    let s = store();
    // Edit what was typed, not the tokens: mentions back to @handles.
    let shown = text::plain_editable(&body, |u| s.peek(|st| st.users.get(&u).map(|u| u.handle.clone()).unwrap_or_default()), |r| s.peek(|st| st.role_label(r)));
    let draft = signal(shown);
    let busy = signal(false);
    let save = cb(move || {
        if busy.peek() {
            return;
        }
        busy.set(true);
        runtime_core::spawn_then(chat_api::messages::edit(id, draft.peek()), move |r| {
            busy.set(false);
            match r {
                Ok(_) => ui.editing.set(None),
                Err(e) => crate::forms::toast(&e),
            }
        });
    });
    let keys = {
        let save = save.clone();
        Some(Rc::new(move |k: &runtime_core::KeyEvent| {
            if k.key == "Escape" {
                ui.editing.set(None);
                KeyOutcome::PreventDefault
            } else if k.key == "Enter" && !k.shift {
                save();
                KeyOutcome::PreventDefault
            } else {
                KeyOutcome::Default
            }
        }) as Rc<dyn Fn(&runtime_core::KeyEvent) -> KeyOutcome>)
    };
    ui! {
        view(style = EditBox()) {
            Field(value = draft, on_change = set(draft), autofocus = true, on_key_down = keys, accessibility_label = Some("Edit message".to_string()))
            view(style = Header()) {
                Typography(content = "Escape to cancel · Enter to save".to_string(), kind = typography_kind::Caption, muted = true)
                Button(label = "Cancel".to_string(), variant = variant::Ghost, tone = tone::Neutral, size = idea_ui::size::Sm, on_click = cb(move || ui.editing.set(None)))
                Button(label = "Save".to_string(), size = idea_ui::size::Sm, loading = rx!(busy.get()), on_click = save)
            }
        }
    }
}

// ---------------------------------------------------------------- system entries

/// A message that is the stream noting something: someone joined, a thread
/// started, a call happened.
#[component]
fn SystemLine(#[prop(static)] channel: Id, #[prop(static)] id: Id) -> Element {
    let s = store();
    let msg = {
        let s = s.clone();
        memo(move || s.message(channel, id))
    };
    let now = {
        let s = s.clone();
        memo(move || s.now() / 60_000)
    };
    let Some(m) = msg.peek() else { return runtime_core::fragment(Vec::new()) };
    let name = s.peek(|st| st.display_name(m.author));
    let at = m.at;
    ui! {
        view(style = SystemRow()) {
            match msg.get().map(|m| m.kind) {
                Some(MessageKind::Call(record)) => { CallLine(channel = channel, record = record.clone(), name = name.clone()) }
                Some(MessageKind::ThreadStarted { thread }) => {
                    view(style = CallRow()) {
                        view(style = SystemIcon()) { Icon(data = MESSAGES_SQUARE, size = 16.0) }
                        Typography(content = format!("{name} started a thread."), kind = typography_kind::BodySm, muted = true)
                        ThreadName(channel = channel, thread = *thread)
                    }
                }
                _ => {
                    view(style = CallRow()) {
                        view(style = SystemIcon()) { Icon(data = ARROW_RIGHT, size = 16.0, tone = Some(tone::Success.into())) }
                        Typography(content = format!("{name} joined the space. Say hi!"), kind = typography_kind::BodySm, muted = true)
                    }
                }
            }
            Typography(content = rx!({ now.get(); stamp(at, platform::now_ms()) }), kind = typography_kind::Caption, muted = true)
        }
    }
}

#[component]
fn ThreadName(#[prop(static)] channel: Id, #[prop(static)] thread: Id) -> Element {
    let s = store();
    let info = memo(move || s.read(|st| st.channels.get(&thread).map(|t| (t.name.clone(), t.message_count))));
    let space = store().peek(|st| st.channels.get(&channel).and_then(|c| c.space));
    ui! {
        if info.get().is_some() {
            Button(
                label = rx!(info.get().map(|i| format!("{} · {}", i.0, match i.1 { 1 => "1 message".to_string(), n => format!("{n} messages") })).unwrap_or_default()),
                variant = variant::Soft,
                size = idea_ui::size::Sm,
                on_click = cb(move || if let Some(space) = space { crate::shell::go(Place::Channel { space, channel: thread }) }),
            )
        }
    }
}

#[component]
fn CallLine(
    #[prop(static)] channel: Id,
    #[prop(static, default = CallRecord { started_by: Id::nil(), started_at: 0, ended_at: None, participants: vec![] })] record: CallRecord,
    #[prop(static)] name: String,
) -> Element {
    let (icon, line, live) = match record.ended_at {
        None => (PHONE, format!("{name} started a call."), true),
        Some(_) if record.missed() => (PHONE_MISSED, format!("Missed call from {name}."), false),
        Some(end) => (PHONE, format!("{name} started a call that lasted {}.", duration(end - record.started_at)), false),
    };
    let join = cb(move || crate::calls::join(channel));
    ui! {
        view(style = CallRow()) {
            view(style = SystemIcon()) { Icon(data = icon, size = 16.0, tone = Some(if live { tone::Success.into() } else { tone::Neutral.into() })) }
            Typography(content = line, kind = typography_kind::BodySm, muted = true)
            if live {
                Button(label = "Join".to_string(), tone = tone::Success, size = idea_ui::size::Sm, on_click = join.clone())
            }
        }
    }
}

stylesheet! {
    BodyCol<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 4.0,
        }
    }
}

stylesheet! {
    CallRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_wrap: FlexWrap::Wrap,
            gap: 12.0,
        }
    }
}
