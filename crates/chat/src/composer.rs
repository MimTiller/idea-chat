//! The message box: what you are replying to, the files you are sending, a
//! picker for the person you are `@`-mentioning, and the text — Enter sends,
//! Shift+Enter starts a new line.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use chat_api::files::Purpose;
use chat_model::{Attachment, Id};
use icons_lucide::{FILE, PAPERCLIP, SEND_HORIZONTAL, X};
use idea_ui::{tone, typography_kind, variant, AvatarSize, Icon, IdeaThemeRef, Spinner, SpinnerSize, Typography};
use runtime_core::{
    component, memo, rx, signal, stylesheet, ui, AlignItems, Element, FlexDirection, FlexWrap, FontWeight,
    JustifyContent, KeyOutcome, Position, Signal, TextAreaHandle,
};

use crate::bits::{Action, PersonAvatar, RowLabel};
use crate::forms::cb;
use crate::nav::{style, Tap};
use crate::platform::now_ms;
use crate::store::store;

thread_local! {
    /// Each channel's unsent text, kept while you look elsewhere.
    static DRAFTS: RefCell<HashMap<Id, String>> = RefCell::new(HashMap::new());
    /// The live text box per channel, to focus from elsewhere (Reply).
    static BOXES: RefCell<HashMap<Id, TextAreaHandle>> = RefCell::new(HashMap::new());
}

/// Put the cursor in a channel's message box.
pub fn focus(channel: Id) {
    BOXES.with(|b| {
        if let Some(h) = b.borrow().get(&channel) {
            h.focus();
        }
    });
}

stylesheet! {
    Dock<()> {
        base(_t) {
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            padding_left: 16.0,
            padding_right: 16.0,
            padding_bottom: 14.0,
            gap: 6.0,
            position: Position::Relative,
        }
    }
}

stylesheet! {
    BoxShape<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            border_radius: 12.0,
            background: t.color.background(),
            border_width: 1.0,
            border_color: t.color.border(),
        }
    }
}

stylesheet! {
    InputRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexEnd,
            gap: 4.0,
            padding_left: 6.0,
            padding_right: 6.0,
            padding_top: 6.0,
            padding_bottom: 6.0,
        }
    }
}

stylesheet! {
    Input<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            flex_basis: 0.0,
            min_width: 0.0,
            min_height: 36.0,
            padding_left: 6.0,
            padding_right: 6.0,
            padding_top: 8.0,
            padding_bottom: 8.0,
            font_size: 15.0,
            color: t.color.text(),
            background: runtime_core::Color("transparent".into()),
            border_width: 0.0,
        }
    }
}

stylesheet! {
    Banner<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
            padding_left: 12.0,
            padding_right: 6.0,
            padding_top: 4.0,
            padding_bottom: 4.0,
            border_bottom_width: 1.0,
            border_color: t.color.border(),
        }
    }
}

stylesheet! {
    Pending<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            gap: 8.0,
            padding_left: 10.0,
            padding_right: 10.0,
            padding_top: 10.0,
        }
    }
}

stylesheet! {
    PendingFile<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 6.0,
            padding_left: 10.0,
            padding_right: 4.0,
            height: 34.0,
            border_radius: 8.0,
            background: t.color.surface(),
            border_width: 1.0,
            border_color: t.color.border(),
            max_width: 240.0,
        }
    }
}

stylesheet! {
    Hint<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
            min_height: 18.0,
            padding_left: 4.0,
        }
    }
}

stylesheet! {
    /// The mention picker, floating over the box.
    Picker<IdeaThemeRef> {
        base(t) {
            position: Position::Absolute,
            left: 16.0,
            right: 16.0,
            bottom: 100.0,
            flex_direction: FlexDirection::Column,
            padding_top: 6.0,
            padding_bottom: 6.0,
            border_radius: 10.0,
            border_width: 1.0,
            border_color: t.color.border(),
            background: t.color.surface(),
            shadow: runtime_core::Shadow { x: 0.0, y: 8.0, blur: 24.0, color: runtime_core::Tokenized::Literal(runtime_core::Color("rgba(0,0,0,0.12)".into())) },
        }
    }
}

stylesheet! {
    PickRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
            padding_left: 10.0,
            padding_right: 10.0,
            height: 36.0,
            cursor: runtime_core::Cursor::Pointer,
        }
        state hovered(t) { background: t.intent.primary.soft_bg() }
        variant chosen {
            #[default]
            off(_t) {}
            on(t) { background: t.intent.primary.soft_bg() }
        }
    }
}

/// A file on its way into a message.
#[derive(Debug, Clone, PartialEq)]
enum Upload {
    Sending { key: u64, name: String },
    Ready(Attachment),
    Failed { key: u64, name: String, why: String },
}

impl Upload {
    fn key(&self) -> String {
        match self {
            Upload::Sending { key, .. } | Upload::Failed { key, .. } => format!("u{key}"),
            Upload::Ready(a) => a.id.to_string(),
        }
    }
}

/// The message box for `channel`, with what it may say in its placeholder.
#[component]
pub fn Composer(#[prop(static)] channel: Id, #[prop(static)] placeholder: String) -> Element {
    let s = store();
    let ui = s.ui();
    let draft = signal(DRAFTS.with(|d| d.borrow().get(&channel).cloned().unwrap_or_default()));
    let uploads: Signal<Vec<Upload>> = signal(Vec::new());
    let sending = signal(false);
    let last_typing = Rc::new(std::cell::Cell::new(0i64));
    // The people who can be mentioned here, filtered by what follows `@`.
    let audience = {
        let s = s.clone();
        memo(move || {
            s.read(|st| {
                let Some(c) = st.channels.get(&channel) else { return Vec::new() };
                let me = st.me_id();
                let ids: Vec<Id> = match c.space.and_then(|sp| st.spaces.get(&sp)) {
                    Some(space) => space.members.keys().copied().collect(),
                    None => c.members.clone(),
                };
                ids.into_iter().filter(|u| Some(*u) != me).filter_map(|u| st.users.get(&u).cloned()).collect::<Vec<_>>()
            })
        })
    };
    let picks = memo(move || {
        let text = draft.get();
        let Some(q) = chat_model::text::mention_query(&text) else { return Vec::new() };
        let q = q.to_lowercase();
        let mut found: Vec<(Id, String, String)> = audience
            .get()
            .into_iter()
            .filter(|u| u.handle.starts_with(&q) || u.display_name.to_lowercase().contains(&q))
            .map(|u| (u.id, u.display_name, u.handle))
            .collect();
        found.sort_by(|a, b| (!a.2.starts_with(&q), a.2.len()).cmp(&(!b.2.starts_with(&q), b.2.len())));
        found.truncate(6);
        found
    });
    let chosen = signal(0usize);
    let handle: Rc<RefCell<Option<TextAreaHandle>>> = Rc::new(RefCell::new(None));

    let keep = move |v: String| {
        DRAFTS.with(|d| {
            d.borrow_mut().insert(channel, v);
        });
    };
    let complete: Rc<dyn Fn(String)> = {
        let handle = handle.clone();
        Rc::new(move |h: String| {
            let text = draft.peek();
            if let Some(at) = text.rfind('@') {
                let next = format!("{}@{h} ", &text[..at]);
                draft.set(next.clone());
                keep(next);
            }
            chosen.set(0);
            if let Some(b) = handle.borrow().as_ref() {
                b.focus();
            }
        })
    };

    let send = {
        let s = s.clone();
        cb(move || {
            if sending.peek() {
                return;
            }
            let ready: Vec<Id> = uploads.peek().iter().filter_map(|u| if let Upload::Ready(a) = u { Some(a.id) } else { None }).collect();
            let still = uploads.peek().iter().any(|u| matches!(u, Upload::Sending { .. }));
            let body = draft.peek();
            if still || (body.trim().is_empty() && ready.is_empty()) {
                return;
            }
            let reply = ui.reply.peek().get(&channel).copied();
            sending.set(true);
            let s = s.clone();
            runtime_core::spawn_then(chat_api::messages::send(channel, body, reply, ready, None), move |r| {
                sending.set(false);
                match r {
                    Ok(message) => {
                        s.posted(message);
                        draft.set(String::new());
                        keep(String::new());
                        uploads.set(Vec::new());
                        ui.reply.update(|m| {
                            let mut m = m.clone();
                            m.remove(&channel);
                            m
                        });
                    }
                    Err(e) => crate::forms::toast(&e),
                }
            });
        })
    };

    let on_change = {
        let s = s.clone();
        let last_typing = last_typing.clone();
        move |v: String| {
            if !v.trim().is_empty() && now_ms() - last_typing.get() > chat_api::live::TYPING_EVERY_MS {
                last_typing.set(now_ms());
                s.say_typing(channel);
            }
            chosen.set(0);
            keep(v.clone());
            draft.set(v);
        }
    };
    let on_key = {
        let send = send.clone();
        let complete = complete.clone();
        move |k: &runtime_core::KeyEvent| {
            let options = picks.peek();
            if !options.is_empty() {
                match k.key.as_str() {
                    "ArrowDown" => {
                        chosen.set((chosen.peek() + 1) % options.len());
                        return KeyOutcome::PreventDefault;
                    }
                    "ArrowUp" => {
                        chosen.set((chosen.peek() + options.len() - 1) % options.len());
                        return KeyOutcome::PreventDefault;
                    }
                    "Enter" | "Tab" => {
                        let pick = options.get(chosen.peek()).map(|p| p.2.clone()).unwrap_or_default();
                        complete(pick);
                        return KeyOutcome::PreventDefault;
                    }
                    _ => {}
                }
            }
            if k.key == "Enter" && !k.shift {
                send();
                return KeyOutcome::PreventDefault;
            }
            if k.key == "Escape" && ui.reply.peek().contains_key(&channel) {
                ui.reply.update(|m| {
                    let mut m = m.clone();
                    m.remove(&channel);
                    m
                });
                return KeyOutcome::PreventDefault;
            }
            KeyOutcome::Default
        }
    };
    // The raw text-area builder: idea-ui's Textarea takes no key handler, and
    // Enter-to-send needs one. Styled from the same tokens as everything else.
    let input: Element = {
        let handle = handle.clone();
        runtime_vocabulary::builders::text_area()
            .value(draft)
            .on_change(on_change)
            .on_key_down(on_key)
            .placeholder(placeholder.clone())
            .min_rows(1)
            .max_rows(10)
            .style(Input())
            .a11y(runtime_core::AccessibilityProps { label: Some(placeholder.clone()), ..Default::default() })
            .on_handle(move |h| {
                BOXES.with(|b| b.borrow_mut().insert(channel, h.clone()));
                *handle.borrow_mut() = Some(h);
            })
            .build()
    };

    let attach = cb(move || {
        runtime_core::spawn_then(crate::uploads::pick(false, true), move |files| {
            for file in files {
                let key = (now_ms() as u64).wrapping_mul(31).wrapping_add(file.name().len() as u64);
                let name = file.name().to_string();
                uploads.update(|u| {
                    let mut u = u.clone();
                    u.push(Upload::Sending { key, name: name.clone() });
                    u
                });
                runtime_core::spawn_then(async move { crate::uploads::upload(&file, Purpose::Message).await }, move |r| {
                    uploads.update(|u| {
                        u.iter()
                            .map(|x| match x {
                                Upload::Sending { key: k, name } if *k == key => match &r {
                                    Ok(a) => Upload::Ready(a.clone()),
                                    Err(why) => Upload::Failed { key, name: name.clone(), why: why.clone() },
                                },
                                other => other.clone(),
                            })
                            .collect()
                    });
                });
            }
        });
    });

    let replying = {
        let s = s.clone();
        memo(move || {
            let r = ui.reply.get().get(&channel).copied()?;
            Some(s.message(channel, r).map(|m| s.peek(|st| st.display_name(m.author))).unwrap_or_else(|| "a message".into()))
        })
    };
    let typing = {
        let s = s.clone();
        memo(move || {
            let people = s.typing_in(channel);
            s.peek(|st| match people.as_slice() {
                [] => String::new(),
                [a] => format!("{} is typing…", st.display_name(*a)),
                [a, b] => format!("{} and {} are typing…", st.display_name(*a), st.display_name(*b)),
                _ => "Several people are typing…".to_string(),
            })
        })
    };

    ui! {
        view(style = Dock()) {
            if !picks.get().is_empty() {
                MentionPicks(picks = rx!(picks.get()), chosen = chosen, complete = complete.clone())
            }
            view(style = BoxShape()) {
                match replying.get() {
                    Some(name) => {
                        view(style = Banner()) {
                            view(style = RowLabel()) {
                                Typography(content = format!("Replying to {name}"), kind = typography_kind::Caption, muted = true)
                            }
                            Action(icon = X, label = "Cancel reply".to_string(), size = idea_ui::IconButtonSize::Sm, on_click = cb(move || ui.reply.update(|m| { let mut m = m.clone(); m.remove(&channel); m })))
                        }
                    }
                    None => { runtime_core::fragment(Vec::new()) }
                }
                if !uploads.get().is_empty() {
                    view(style = Pending()) {
                        for u in uploads, key = u.key() {
                            PendingUpload(upload = u.clone(), uploads = uploads)
                        }
                    }
                }
                view(style = InputRow()) {
                    Action(icon = PAPERCLIP, label = "Attach files".to_string(), on_click = attach)
                    input
                    Action(icon = SEND_HORIZONTAL, label = "Send".to_string(), tone = tone::Primary, on_click = send)
                }
            }
            view(style = Hint()) {
                Typography(content = rx!(typing.get()), kind = typography_kind::Caption, muted = true, lines = Some(1))
            }
        }
    }
}

#[component]
fn MentionPicks(
    #[prop(reactive)] picks: Vec<(Id, String, String)>,
    #[prop(static, default = signal(0))] chosen: Signal<usize>,
    #[prop(static, default = Rc::new(|_| {}) as Rc<dyn Fn(String)>)] complete: Rc<dyn Fn(String)>,
) -> Element {
    let list = memo(move || picks.get().into_iter().enumerate().collect::<Vec<_>>());
    ui! {
        view(style = Picker()) {
            for (i, pick) in list, key = pick.2.clone() {
                MentionPick(user = pick.0, name = pick.1.clone(), handle = pick.2.clone(), index = i, chosen = chosen, complete = complete.clone())
            }
        }
    }
}

#[component]
fn MentionPick(
    #[prop(static)] user: Id,
    #[prop(static)] name: String,
    #[prop(static)] handle: String,
    #[prop(static)] index: usize,
    #[prop(static, default = signal(0))] chosen: Signal<usize>,
    #[prop(static, default = Rc::new(|_| {}) as Rc<dyn Fn(String)>)] complete: Rc<dyn Fn(String)>,
) -> Element {
    let pick = {
        let handle = handle.clone();
        cb(move || complete(handle.clone()))
    };
    ui! {
        Tap(on_press = pick, label = format!("Mention {name}"), style = style(PickRow().chosen(runtime_core::derived(move || if chosen.get() == index { PickRowChosen::On } else { PickRowChosen::Off })))) {
            PersonAvatar(user = user, size = AvatarSize::Xs)
            Typography(content = name.clone(), weight = Some(FontWeight::Medium))
            Typography(content = format!("@{handle}"), kind = typography_kind::Caption, muted = true)
        }
    }
}

#[component]
fn PendingUpload(
    #[prop(static, default = Upload::Sending { key: 0, name: String::new() })] upload: Upload,
    #[prop(static, default = signal(Vec::new()))] uploads: Signal<Vec<Upload>>,
) -> Element {
    let key = upload.key();
    let remove = cb(move || uploads.update(|u| u.iter().filter(|x| x.key() != key).cloned().collect()));
    let (name, state) = match &upload {
        Upload::Sending { name, .. } => (name.clone(), 0),
        Upload::Ready(a) => (a.name.clone(), 1),
        Upload::Failed { name, why, .. } => (format!("{name} — {why}"), 2),
    };
    ui! {
        view(style = PendingFile()) {
            if state == 0 {
                Spinner(size = SpinnerSize::Small)
            } else {
                Icon(data = FILE, size = 16.0, tone = Some(if state == 2 { tone::Danger.into() } else { tone::Primary.into() }))
            }
            view(style = RowLabel()) {
                Typography(content = name, kind = typography_kind::Caption, lines = Some(1))
            }
            Action(icon = X, label = "Remove".to_string(), size = idea_ui::IconButtonSize::Sm, on_click = remove)
        }
    }
}
