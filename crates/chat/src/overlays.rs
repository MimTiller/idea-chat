//! What sits over everything: dialogs, a person's profile card, the media
//! viewer, settings, the incoming-call card, and the connection banner.

use std::collections::BTreeSet;

use chat_model::{ChannelKind, Id, Invite, Relation};
use icons_lucide::{CHECK, COPY, HASH, VOLUME_2, WIFI_OFF, X};
use idea_ui::{
    tone, typography_kind, variant, AvatarSize, Button, Field, Icon, IdeaThemeRef, Image, Modal, ModalPresentation, SegmentOption,
    SegmentedControl, Spinner, SpinnerSize, Typography,
};
use runtime_core::{
    component, memo, rx, signal, stylesheet, ui, AlignItems, Color, Element, FlexDirection, FlexWrap, FontWeight,
    JustifyContent, Length, Position, Signal,
};

use crate::bits::{Action, PersonAvatar, RowLabel};
use crate::calls::Ringing;
use crate::context::ProfilePanel;
use crate::settings::SettingsModal;
use crate::forms::{cb, cb1, on_enter, set, Form};
use crate::nav::{style, Tap};
use crate::platform;
use crate::shell::{go, open_space};
use crate::store::{store, Conn, Dialog, Place};

/// Open (or make) the conversation with someone and go there.
pub fn message_person(user: Id) {
    let s = store();
    if let Some(c) = s.peek(|st| st.direct_with(user).map(|c| c.id)) {
        s.ui().profile.set(None);
        go(Place::Direct(c));
        return;
    }
    runtime_core::spawn_then(chat_api::channels::open_direct(user), move |r| match r {
        Ok(c) => {
            store().ui().profile.set(None);
            go(Place::Direct(c.id));
        }
        Err(e) => crate::forms::toast(&e),
    });
}

/// Call someone: open the conversation and start its call.
pub fn call_person(user: Id) {
    runtime_core::spawn_then(chat_api::channels::open_direct(user), move |r| match r {
        Ok(c) => {
            store().ui().profile.set(None);
            go(Place::Direct(c.id));
            crate::calls::join(c.id);
        }
        Err(e) => crate::forms::toast(&e),
    });
}

stylesheet! {
    Banner<IdeaThemeRef> {
        base(t) {
            position: Position::Absolute,
            top: 10.0,
            left: 0.0,
            right: 0.0,
            align_items: AlignItems::Center,
            pointer_events: runtime_core::PointerEvents::None,
        }
    }
}

stylesheet! {
    BannerPill<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 10.0,
            padding_left: 14.0,
            padding_right: 16.0,
            padding_top: 8.0,
            padding_bottom: 8.0,
            border_radius: 999.0,
            background: t.intent.warning.soft_bg(),
            color: t.intent.warning.soft_text(),
            border_width: 1.0,
            border_color: t.intent.warning.border(),
        }
    }
}

#[component]
pub fn Overlays() -> Element {
    let s = store();
    let ui = s.ui();
    let conn = s.conn();
    ui! {
        match ui.dialog.get() {
            Some(d) => { DialogFor(dialog = *d) }
            None => { runtime_core::fragment(Vec::new()) }
        }
        match ui.profile.get() {
            Some(user) => { ProfileCard(user = *user) }
            None => { runtime_core::fragment(Vec::new()) }
        }
        match ui.viewer.get() {
            Some(file) => { Viewer(file = file.clone()) }
            None => { runtime_core::fragment(Vec::new()) }
        }
        match ui.settings.get() {
            Some(page) => { SettingsModal(start = *page) }
            None => { runtime_core::fragment(Vec::new()) }
        }
        Ringing()
        if conn.get() == Conn::Reconnecting {
            view(style = Banner()) {
                view(style = BannerPill()) {
                    Spinner(size = SpinnerSize::Small)
                    Icon(data = WIFI_OFF, size = 16.0)
                    Typography(content = "Reconnecting… messages will catch up".to_string(), kind = typography_kind::BodySm, weight = Some(FontWeight::Medium))
                }
            }
        }
    }
}

stylesheet! {
    Sheet<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 14.0,
        }
    }
}

stylesheet! {
    SheetHead<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            justify_content: JustifyContent::SpaceBetween,
            gap: 12.0,
        }
    }
}

stylesheet! {
    Buttons<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::FlexEnd,
            gap: 8.0,
            flex_wrap: FlexWrap::Wrap,
        }
    }
}

/// A dialog's frame: a title, what it is for, the close button, the body.
#[component(children)]
fn Frame(#[prop(static)] title: String, #[prop(static, default = String::new())] blurb: String, children: Vec<Element>) -> Element {
    let ui = store().ui();
    let has_blurb = !blurb.is_empty();
    ui! {
        view(style = Sheet()) {
            view(style = SheetHead()) {
                view(style = RowLabel()) {
                    Typography(content = title, kind = typography_kind::H5)
                    if has_blurb {
                        Typography(content = blurb.clone(), muted = true, kind = typography_kind::BodySm)
                    }
                }
                Action(icon = X, label = "Close".to_string(), on_click = cb(move || ui.dialog.set(None)))
            }
            children
        }
    }
}

#[component]
fn DialogFor(#[prop(static, default = Dialog::CreateSpace)] dialog: Dialog) -> Element {
    let ui = store().ui();
    let close = cb(move || ui.dialog.set(None));
    let width = match dialog {
        Dialog::NewMessage => 480.0,
        _ => 440.0,
    };
    ui! {
        Modal(open = true, width = width, on_dismiss = Some(close), content = move || match dialog {
            Dialog::CreateSpace => ui! { CreateSpace() },
            Dialog::JoinSpace => ui! { JoinSpace() },
            Dialog::Invite(space) => ui! { InviteDialog(space = space) },
            Dialog::CreateChannel { space, voice, category } => ui! { CreateChannel(space = space, voice = voice, category = category) },
            Dialog::NewMessage => ui! { NewMessage() },
            Dialog::AddFriend => ui! { AddFriend() },
            Dialog::StartThread { channel, from } => ui! { StartThread(channel = channel, from = from) },
        })
    }
}

#[component]
fn CreateSpace() -> Element {
    let ui = store().ui();
    let name = signal(String::new());
    let form = Form::new();
    let make = cb(move || form.submit(chat_api::spaces::create_space(name.peek()), move |space| {
        ui.dialog.set(None);
        open_space(space);
    }));
    ui! {
        Frame(title = "Create a space".to_string(), blurb = "A home for a group: channels to talk in, rooms to meet in.".to_string()) {
            Field(label = Some("Name".to_string()), value = name, on_change = set(name), autofocus = true, placeholder = Some("Book club".to_string()), error = rx!(form.error("name").or(form.general.get())), on_key_down = on_enter(make.clone()))
            view(style = Buttons()) {
                Button(label = "Join one instead".to_string(), variant = variant::Ghost, tone = tone::Neutral, on_click = cb(move || ui.dialog.set(Some(Dialog::JoinSpace))))
                Button(label = "Create".to_string(), on_click = make, loading = rx!(form.busy.get()))
            }
        }
    }
}

#[component]
fn JoinSpace() -> Element {
    let ui = store().ui();
    let code = signal(String::new());
    let form = Form::new();
    let join = cb(move || form.submit(chat_api::spaces::join_space(code.peek()), move |space| {
        ui.dialog.set(None);
        open_space(space);
    }));
    ui! {
        Frame(title = "Join a space".to_string(), blurb = "Paste an invite link or code.".to_string()) {
            Field(label = Some("Invite".to_string()), value = code, on_change = set(code), autofocus = true, placeholder = Some("https://…/invite/abc123 or abc123".to_string()), error = rx!(form.error("code").or(form.general.get())), on_key_down = on_enter(join.clone()))
            view(style = Buttons()) {
                Button(label = "Create one instead".to_string(), variant = variant::Ghost, tone = tone::Neutral, on_click = cb(move || ui.dialog.set(Some(Dialog::CreateSpace))))
                Button(label = "Join".to_string(), on_click = join, loading = rx!(form.busy.get()))
            }
        }
    }
}

stylesheet! {
    LinkBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
            padding_left: 12.0,
            padding_right: 6.0,
            padding_top: 6.0,
            padding_bottom: 6.0,
            border_radius: 10.0,
            background: t.color.background(),
            border_width: 1.0,
            border_color: t.color.border(),
        }
    }
}

fn invite_link(code: &str) -> String {
    format!("{}/invite/{code}", platform::server_url().trim_end_matches('/'))
}

#[component]
fn InviteDialog(#[prop(static)] space: Id) -> Element {
    let invite: Signal<Option<Invite>> = signal(None);
    let expiry = signal("7d".to_string());
    let copied = signal(false);
    let error: Signal<Option<String>> = signal(None);
    let make = move |which: String| {
        let hours = match which.as_str() {
            "1d" => Some(24),
            "7d" => Some(24 * 7),
            _ => None,
        };
        copied.set(false);
        runtime_core::spawn_then(chat_api::spaces::create_invite(space, None, hours), move |r| match r {
            Ok(i) => invite.set(Some(i)),
            Err(e) => error.set(Some(chat_api::describe(&e))),
        });
    };
    make(expiry.peek());
    let on_expiry = cb1(move |v: String| {
        expiry.set(v.clone());
        make(v);
    });
    let copy = cb(move || {
        if let Some(i) = invite.peek() {
            let link = invite_link(&i.code);
            runtime_core::spawn_then(async move { clipboard::set_text(&link).await }, move |r| {
                if r.is_ok() {
                    copied.set(true);
                }
            });
        }
    });
    let name = store().peek(|st| st.spaces.get(&space).map(|s| s.name.clone()).unwrap_or_default());
    ui! {
        Frame(title = format!("Invite people to {name}"), blurb = "Anyone with the link can join.".to_string()) {
            view(style = LinkBox()) {
                view(style = RowLabel()) {
                    Typography(content = rx!(invite.get().map(|i| invite_link(&i.code)).unwrap_or_else(|| "Making a link…".to_string())), lines = Some(1), font = Some(crate::forms::mono()), kind = typography_kind::BodySm)
                }
                Button(label = rx!(if copied.get() { "Copied".to_string() } else { "Copy".to_string() }), size = idea_ui::size::Sm, leading_icon = rx!(Some(if copied.get() { CHECK } else { COPY })).get(), on_click = copy)
            }
            if error.get().is_some() {
                Typography(content = rx!(error.get().unwrap_or_default()), tone = Some(tone::Danger.into()), kind = typography_kind::BodySm)
            }
            Typography(content = "LINK EXPIRES".to_string(), kind = typography_kind::Overline, muted = true)
            SegmentedControl(value = rx!(expiry.get()), on_change = on_expiry, options = vec![
                SegmentOption::new("1d", "1 day".to_string()),
                SegmentOption::new("7d", "7 days".to_string()),
                SegmentOption::new("never", "Never".to_string()),
            ])
        }
    }
}

#[component]
fn CreateChannel(#[prop(static)] space: Id, #[prop(static)] voice: bool, #[prop(static, default = None)] category: Option<Id>) -> Element {
    let ui = store().ui();
    let kind = signal(if voice { "voice".to_string() } else { "text".to_string() });
    let name = signal(String::new());
    let form = Form::new();
    let make = cb(move || {
        let k = if kind.peek() == "voice" { ChannelKind::Voice } else { ChannelKind::Text };
        form.submit(chat_api::channels::create_channel(space, k, name.peek(), category), move |c| {
            ui.dialog.set(None);
            go(Place::Channel { space, channel: c.id });
        });
    });
    ui! {
        Frame(title = "Create a channel".to_string()) {
            SegmentedControl(value = rx!(kind.get()), on_change = cb1(move |v: String| kind.set(v)), block = true, options = vec![
                SegmentOption::new("text", "Text".to_string()).leading(idea_ui::Adornment::Icon(HASH)),
                SegmentOption::new("voice", "Voice".to_string()).leading(idea_ui::Adornment::Icon(VOLUME_2)),
            ])
            Typography(content = rx!(if kind.get() == "voice" { "Hang out together with voice, video and screen sharing.".to_string() } else { "Send messages, files and threads.".to_string() }), kind = typography_kind::BodySm, muted = true)
            Field(label = Some("Name".to_string()), value = name, on_change = set(name), autofocus = true, placeholder = Some("new-channel".to_string()), error = rx!(form.error("name").or(form.general.get())), on_key_down = on_enter(make.clone()))
            view(style = Buttons()) {
                Button(label = "Create channel".to_string(), on_click = make, loading = rx!(form.busy.get()))
            }
        }
    }
}

stylesheet! {
    PickList<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            max_height: 320.0,
        }
    }
}

stylesheet! {
    PickRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 10.0,
            min_height: 44.0,
            padding_left: 8.0,
            padding_right: 8.0,
            border_radius: 8.0,
            cursor: runtime_core::Cursor::Pointer,
        }
        state hovered(t) { background: t.color.surface_alt() }
        variant picked {
            #[default]
            off(_t) {}
            on(t) { background: t.intent.primary.soft_bg() }
        }
    }
}

/// Start a conversation: pick one person for a direct message, or several
/// for a group.
#[component]
fn NewMessage() -> Element {
    let s = store();
    let ui = s.ui();
    let query = signal(String::new());
    let picked: Signal<BTreeSet<Id>> = signal(BTreeSet::new());
    let people = {
        let s = s.clone();
        memo(move || {
            let q = query.get().to_lowercase();
            s.read(|st| {
                let me = st.me_id();
                let mut out: Vec<Id> = st
                    .users
                    .values()
                    .filter(|u| Some(u.id) != me && st.relations.get(&u.id) != Some(&Relation::Blocked))
                    .filter(|u| q.is_empty() || u.handle.contains(&q) || u.display_name.to_lowercase().contains(&q))
                    .map(|u| u.id)
                    .collect();
                // Friends first, then by name.
                out.sort_by_cached_key(|u| (st.relations.get(u) != Some(&Relation::Friend), st.display_name(*u).to_lowercase()));
                out.truncate(30);
                out
            })
        })
    };
    let form = Form::new();
    let start = cb(move || {
        let chosen: Vec<Id> = picked.peek().into_iter().collect();
        match chosen.as_slice() {
            [] => {}
            [one] => {
                ui.dialog.set(None);
                message_person(*one);
            }
            many => form.submit(chat_api::channels::create_group(many.to_vec(), String::new()), move |c| {
                ui.dialog.set(None);
                go(Place::Direct(c.id));
            }),
        }
    });
    let label = memo(move || match picked.get().len() {
        0 => "Pick someone".to_string(),
        1 => "Message".to_string(),
        n => format!("Start a group of {}", n + 1),
    });
    ui! {
        Frame(title = "New message".to_string(), blurb = "Pick one person, or a few for a group.".to_string()) {
            Field(value = query, on_change = set(query), autofocus = true, placeholder = Some("Search by name or username".to_string()), accessibility_label = Some("Search people".to_string()))
            scroll_view(style = PickList()) {
                for user in people, key = user.to_string() {
                    PersonPick(user = user, picked = picked)
                }
            }
            if form.general.get().is_some() {
                Typography(content = rx!(form.general.get().unwrap_or_default()), tone = Some(tone::Danger.into()), kind = typography_kind::BodySm)
            }
            view(style = Buttons()) {
                Button(label = rx!(label.get()), on_click = start, disabled = rx!(picked.get().is_empty()), loading = rx!(form.busy.get()))
            }
        }
    }
}

#[component]
fn PersonPick(#[prop(static)] user: Id, #[prop(static, default = signal(BTreeSet::new()))] picked: Signal<BTreeSet<Id>>) -> Element {
    let s = store();
    let (name, handle) = s.peek(|st| st.users.get(&user).map(|u| (u.display_name.clone(), u.handle.clone())).unwrap_or_default());
    let on = memo(move || picked.get().contains(&user));
    let toggle = cb(move || {
        let mut p = picked.peek();
        if !p.remove(&user) {
            p.insert(user);
        }
        picked.set(p);
    });
    ui! {
        Tap(on_press = toggle, label = name.clone(), style = style(PickRow().picked(runtime_core::derived(move || if on.get() { PickRowPicked::On } else { PickRowPicked::Off })))) {
            PersonAvatar(user = user, size = AvatarSize::Sm)
            view(style = RowLabel()) {
                Typography(content = name.clone(), weight = Some(FontWeight::Medium), lines = Some(1))
                Typography(content = format!("@{handle}"), kind = typography_kind::Caption, muted = true)
            }
            if on.get() {
                Icon(data = CHECK, size = 18.0, tone = Some(tone::Primary.into()))
            }
        }
    }
}

#[component]
fn AddFriend() -> Element {
    let handle = signal(String::new());
    let form = Form::new();
    let sent: Signal<Option<String>> = signal(None);
    let add = cb(move || {
        let h = handle.peek();
        form.submit(chat_api::people::add_friend(h.clone()), move |r| {
            sent.set(Some(match r {
                Relation::Friend => format!("You and @{} are now friends.", h.trim_start_matches('@')),
                _ => format!("Request sent to @{}.", h.trim_start_matches('@')),
            }));
            handle.set(String::new());
        });
    });
    ui! {
        Frame(title = "Add a friend".to_string(), blurb = "Add someone by their username.".to_string()) {
            Field(label = Some("Username".to_string()), value = handle, on_change = set(handle), autofocus = true, placeholder = Some("username".to_string()), leading = idea_ui::Adornment::Element(std::rc::Rc::new(|| ui! { Typography(content = "@".to_string(), muted = true) })), error = rx!(form.error("handle").or(form.general.get())), on_key_down = on_enter(add.clone()))
            if sent.get().is_some() {
                Typography(content = rx!(sent.get().unwrap_or_default()), tone = Some(tone::Success.into()), kind = typography_kind::BodySm)
            }
            view(style = Buttons()) {
                Button(label = "Send request".to_string(), on_click = add, loading = rx!(form.busy.get()))
            }
        }
    }
}

#[component]
fn StartThread(#[prop(static)] channel: Id, #[prop(static, default = None)] from: Option<Id>) -> Element {
    let s = store();
    let ui = s.ui();
    let suggested = from
        .and_then(|m| s.message(channel, m))
        .map(|m| chat_model::text::plain(&m.body, |u| s.peek(|st| st.display_name(u)), |_| String::new(), |r| s.peek(|st| st.role_label(r)), 40))
        .unwrap_or_default();
    let name = signal(suggested);
    let form = Form::new();
    let space = s.peek(|st| st.channels.get(&channel).and_then(|c| c.space));
    let make = cb(move || form.submit(chat_api::channels::create_thread(channel, name.peek(), from), move |t| {
        ui.dialog.set(None);
        if let Some(space) = space {
            go(Place::Channel { space, channel: t.id });
        }
    }));
    ui! {
        Frame(title = "Start a thread".to_string(), blurb = "A side conversation, listed under its channel.".to_string()) {
            Field(label = Some("Thread name".to_string()), value = name, on_change = set(name), autofocus = true, error = rx!(form.error("name").or(form.general.get())), on_key_down = on_enter(make.clone()))
            view(style = Buttons()) {
                Button(label = "Start thread".to_string(), on_click = make, loading = rx!(form.busy.get()))
            }
        }
    }
}

/// Someone's profile, over whatever is on screen.
#[component]
fn ProfileCard(#[prop(static)] user: Id) -> Element {
    let ui = store().ui();
    ui! {
        Modal(open = true, width = 360.0, on_dismiss = Some(cb(move || ui.profile.set(None))), content = move || ui! { ProfilePanel(user = user) })
    }
}

stylesheet! {
    ViewerBox<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            gap: 10.0,
        }
    }
}

stylesheet! {
    ViewerBar<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
            width: Length::pct(100.0),
        }
    }
}

/// An image, as large as the window allows.
#[component]
fn Viewer(#[prop(static, default = chat_model::Attachment { id: Id::nil(), name: String::new(), size: 0, content_type: String::new(), url: String::new(), width: None, height: None })] file: chat_model::Attachment) -> Element {
    let ui = store().ui();
    let url = platform::absolute(&file.url);
    let name = file.name.clone();
    ui! {
        Modal(open = true, width = 960.0, on_dismiss = Some(cb(move || ui.viewer.set(None))), content = move || ui! {
            view(style = ViewerBox()) {
                view(style = ViewerBar()) {
                    view(style = RowLabel()) { Typography(content = name.clone(), weight = Some(FontWeight::Medium), lines = Some(1)) }
                    link(external = url.clone()) { Typography(content = "Open original".to_string(), tone = Some(tone::Primary.into()), kind = typography_kind::BodySm) }
                    Action(icon = X, label = "Close".to_string(), on_click = cb(move || ui.viewer.set(None)))
                }
                Image(src = url.clone(), alt = name.clone(), width = Some(900.0), height = Some(600.0))
            }
        })
    }
}
