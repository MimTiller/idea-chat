//! Settings, in one modal with its sections down the side: you (profile,
//! account, appearance, devices), the space you are in (overview, members,
//! invites) and, when one is chosen, a channel.

use chat_api::files::Purpose;
use chat_model::{ChannelKind, Id, Invite, Status};
use icons_lucide::{LAPTOP, LOG_OUT, SHIELD_CHECK, TRASH_2, X};
use idea_ui::{
    tone, typography_kind, variant, Alert, AvatarSize, Button, Divider, Field, Icon, IdeaThemeRef, Modal, SegmentOption,
    SegmentedControl, Select, SelectOption, Textarea, Typography,
};
use runtime_core::{
    component, memo, rx, signal, stylesheet, ui, AlignItems, Element, FlexDirection, FontWeight, JustifyContent,
    Length, Signal,
};

use crate::bits::{Action, NavRow, NavRowSelected, PersonAvatar, RowLabel};
use crate::forms::{cb, cb1, set, Form};
use crate::nav::{style, Tap};
use crate::store::{store, Settings, SpacePage};

stylesheet! {
    Shell<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            min_height: 480.0,
            max_height: 640.0,
        }
        breakpoint md(_t) { flex_direction: FlexDirection::Row }
    }
}

stylesheet! {
    Side<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            gap: 2.0,
            padding_bottom: 8.0,
            border_bottom_width: 1.0,
            border_color: t.color.border(),
            overflow: runtime_core::Overflow::Hidden,
        }
        breakpoint md(t) {
            flex_direction: FlexDirection::Column,
            width: 200.0,
            flex_shrink: 0.0,
            border_bottom_width: 0.0,
            border_right_width: 1.0,
            padding_right: 8.0,
            padding_bottom: 0.0,
        }
    }
}

stylesheet! {
    SideHead<()> {
        base(_t) {
            padding_left: 16.0,
            padding_top: 12.0,
            padding_bottom: 4.0,
        }
    }
}

stylesheet! {
    Page<()> {
        base(_t) {
            flex_grow: 1.0,
            flex_basis: 0.0,
            min_width: 0.0,
            min_height: 0.0,
        }
    }
}

stylesheet! {
    PageBody<()> {
        base(_t) {
            // Scroll content grows past the viewport rather than shrinking into it.
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            gap: 16.0,
            padding_left: 4.0,
            padding_right: 4.0,
            padding_top: 12.0,
            padding_bottom: 12.0,
        }
        breakpoint md(_t) { padding_left: 24.0, padding_right: 8.0 }
    }
}

stylesheet! {
    Head<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
        }
    }
}

stylesheet! {
    Card<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 14.0,
            padding_left: 16.0,
            padding_right: 16.0,
            padding_top: 14.0,
            padding_bottom: 14.0,
            border_radius: 12.0,
            background: t.color.background(),
        }
    }
}

stylesheet! {
    Buttons<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            gap: 8.0,
            justify_content: JustifyContent::FlexEnd,
        }
    }
}

stylesheet! {
    ListRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 12.0,
            min_height: 52.0,
            border_bottom_width: 1.0,
            border_color: t.color.border(),
        }
    }
}

#[component]
pub fn SettingsModal(#[prop(static, default = Settings::Profile)] start: Settings) -> Element {
    let s = store();
    let ui = s.ui();
    let page = signal(start);
    // The space whose sections show: the one chosen in the app.
    let space = match start {
        Settings::Space(sp, _) => Some(sp),
        Settings::Channel(c) => s.peek(|st| st.channels.get(&c).and_then(|c| c.space)),
        _ => ui.place.peek().space(),
    };
    let admin = space.is_some_and(|sp| s.manages_space(sp));
    let space_name = space.and_then(|sp| s.peek(|st| st.spaces.get(&sp).map(|s| s.name.clone()))).unwrap_or_default();
    let channel = match start {
        Settings::Channel(c) => Some(c),
        _ => None,
    };
    let close = cb(move || ui.settings.set(None));
    ui! {
        Modal(open = true, width = 820.0, on_dismiss = Some(close.clone()), content = move || {
            let space_name = space_name.clone();
            ui! {
            view(style = Shell()) {
                view(style = Side()) {
                    view(style = SideHead()) { Typography(content = "YOU".to_string(), kind = typography_kind::Overline, muted = true) }
                    PageLink(label = "Profile".to_string(), page = page, to = Settings::Profile)
                    PageLink(label = "Account".to_string(), page = page, to = Settings::Account)
                    PageLink(label = "Appearance".to_string(), page = page, to = Settings::Appearance)
                    PageLink(label = "Devices".to_string(), page = page, to = Settings::Devices)
                    if let Some(sp) = space.filter(|_| admin) {
                        view(style = SideHead()) { Typography(content = space_name.to_uppercase(), kind = typography_kind::Overline, muted = true, lines = Some(1)) }
                        PageLink(label = "Overview".to_string(), page = page, to = Settings::Space(sp, SpacePage::Overview))
                        PageLink(label = "Members".to_string(), page = page, to = Settings::Space(sp, SpacePage::Members))
                        PageLink(label = "Invites".to_string(), page = page, to = Settings::Space(sp, SpacePage::Invites))
                        PageLink(label = "Channels".to_string(), page = page, to = Settings::Space(sp, SpacePage::Channels))
                    }
                    if let Some(c) = channel {
                        view(style = SideHead()) { Typography(content = "CHANNEL".to_string(), kind = typography_kind::Overline, muted = true) }
                        PageLink(label = "Channel settings".to_string(), page = page, to = Settings::Channel(c))
                    }
                }
                view(style = Page()) {
                    scroll_view(style = crate::bits::Fill()) {
                        view(style = PageBody()) {
                            match page.get() {
                                Settings::Profile => { ProfilePage() }
                                Settings::Account => { AccountPage() }
                                Settings::Appearance => { AppearancePage() }
                                Settings::Devices => { DevicesPage() }
                                Settings::Space(sp, SpacePage::Overview) => { SpaceOverview(space = *sp) }
                                Settings::Space(sp, SpacePage::Members) => { SpaceMembers(space = *sp) }
                                Settings::Space(sp, SpacePage::Invites) => { SpaceInvites(space = *sp) }
                                Settings::Space(sp, SpacePage::Channels) => { SpaceChannels(space = *sp, page = page) }
                                Settings::Channel(c) => { ChannelPage(channel = *c) }
                            }
                        }
                    }
                }
            }
            }
        })
    }
}

#[component]
fn PageLink(#[prop(static)] label: String, #[prop(static, default = signal(Settings::Profile))] page: Signal<Settings>, #[prop(static, default = Settings::Profile)] to: Settings) -> Element {
    let on = memo(move || page.get() == to);
    ui! {
        Tap(on_press = cb(move || page.set(to)), label = label.clone(), style = style(NavRow().selected(runtime_core::derived(move || if on.get() { NavRowSelected::On } else { NavRowSelected::Off })))) {
            Typography(content = label.clone(), kind = typography_kind::BodySm, weight = Some(FontWeight::Medium), lines = Some(1))
        }
    }
}

#[component]
fn Title(#[prop(static)] text: String) -> Element {
    let ui = store().ui();
    ui! {
        view(style = Head()) {
            Typography(content = text, kind = typography_kind::H5)
            Action(icon = X, label = "Close settings".to_string(), on_click = cb(move || ui.settings.set(None)))
        }
    }
}

#[component]
fn ProfilePage() -> Element {
    let s = store();
    let me = s.peek(|st| st.me.clone());
    let Some(me) = me else { return runtime_core::fragment(Vec::new()) };
    let id = me.user.id;
    let name = signal(me.user.display_name.clone());
    let handle = signal(me.user.handle.clone());
    let bio = signal(me.user.bio.clone());
    let status = signal(me.status.text.clone().unwrap_or_default());
    let form = Form::new();
    let saved = signal(false);
    let original_handle = me.user.handle.clone();
    let presence = me.status.presence;
    let save = cb(move || {
        saved.set(false);
        let (n, b, h, st) = (name.peek(), bio.peek(), handle.peek(), status.peek());
        let changed_handle = h.trim().trim_start_matches('@').to_lowercase() != original_handle;
        form.submit(
            async move {
                chat_api::auth::update_profile(Some(n), Some(b)).await?;
                if changed_handle {
                    chat_api::auth::set_handle(h).await?;
                }
                chat_api::auth::set_status(Status { presence, text: Some(st) }).await?;
                Ok(())
            },
            move |_| saved.set(true),
        );
    });
    let busy = signal(false);
    let pick = cb(move || {
        busy.set(true);
        runtime_core::spawn_then(
            async move {
                let files = crate::uploads::pick(true, false).await;
                let Some(file) = files.first() else { return Ok(None) };
                let a = crate::uploads::upload(file, Purpose::Avatar).await?;
                chat_api::files::set_avatar(Some(a.id)).await.map(Some).map_err(|e| chat_api::describe(&e))
            },
            move |r: Result<Option<chat_model::User>, String>| {
                busy.set(false);
                if let Err(e) = r {
                    idea_ui::push_toast(e, tone::Danger);
                }
            },
        );
    });
    let remove = cb(move || runtime_core::spawn_then(chat_api::files::set_avatar(None), |_| {}));
    ui! {
        view(style = crate::bits::Col()) {
            Title(text = "Profile".to_string())
            view(style = Card()) {
                PersonAvatar(user = id, size = AvatarSize::Xl)
                view(style = RowLabel()) {
                    Typography(content = "Your picture".to_string(), weight = Some(FontWeight::Medium))
                    Typography(content = "Square images look best. Up to 8 MB.".to_string(), kind = typography_kind::Caption, muted = true)
                }
                Button(label = "Upload".to_string(), size = idea_ui::size::Sm, on_click = pick, loading = rx!(busy.get()))
                Button(label = "Remove".to_string(), size = idea_ui::size::Sm, variant = variant::Ghost, tone = tone::Neutral, on_click = remove)
            }
            if form.general.get().is_some() {
                Alert(title = rx!(form.general.get().unwrap_or_default()), tone = tone::Danger, variant = variant::Soft)
            }
            if saved.get() {
                Alert(title = "Saved.".to_string(), tone = tone::Success, variant = variant::Soft)
            }
            Field(label = Some("Display name".to_string()), value = name, on_change = set(name), error = rx!(form.error("display_name")))
            Field(label = Some("Username".to_string()), value = handle, on_change = set(handle), help = Some("People @mention and add you by this.".to_string()), error = rx!(form.error("handle")))
            Field(label = Some("Status".to_string()), value = status, on_change = set(status), placeholder = Some("What are you up to?".to_string()), error = rx!(form.error("status")))
            Textarea(label = Some("About you".to_string()), value = bio, on_change = set(bio), rows = 3, max_rows = 8, error = rx!(form.error("bio")))
            view(style = Buttons()) {
                Button(label = "Save changes".to_string(), on_click = save, loading = rx!(form.busy.get()))
            }
        }
    }
}

#[component]
fn AccountPage() -> Element {
    let s = store();
    let email = s.peek(|st| st.me.as_ref().map(|m| m.email.clone())).unwrap_or_default();
    let current = signal(String::new());
    let next = signal(String::new());
    let form = Form::new();
    let done = signal(false);
    let change = cb(move || form.submit(chat_api::auth::change_password(current.peek(), next.peek()), move |session| {
        if let Some(t) = &session.token {
            crate::platform::set_secret("session", t);
        }
        current.set(String::new());
        next.set(String::new());
        done.set(true);
    }));
    let sign_out = cb(|| runtime_core::spawn_then(chat_api::auth::sign_out(), |_| crate::app::signed_out()));
    let everywhere = cb(|| runtime_core::spawn_then(chat_api::auth::sign_out_everywhere(), |_| crate::app::signed_out()));
    ui! {
        view(style = crate::bits::Col()) {
            Title(text = "Account".to_string())
            view(style = Card()) {
                view(style = RowLabel()) {
                    Typography(content = "EMAIL".to_string(), kind = typography_kind::Overline, muted = true)
                    Typography(content = email.clone(), weight = Some(FontWeight::Medium))
                }
                Icon(data = SHIELD_CHECK, size = 18.0, tone = Some(tone::Success.into()))
            }
            Typography(content = "Change password".to_string(), kind = typography_kind::H6)
            if done.get() {
                Alert(title = "Password changed. Your other sessions were signed out.".to_string(), tone = tone::Success, variant = variant::Soft)
            }
            if form.general.get().is_some() {
                Alert(title = rx!(form.general.get().unwrap_or_default()), tone = tone::Danger, variant = variant::Soft)
            }
            Field(label = Some("Current password".to_string()), value = current, on_change = set(current), secure = true, error = rx!(form.error("current")))
            Field(label = Some("New password".to_string()), value = next, on_change = set(next), secure = true, help = Some(format!("At least {} characters.", chat_model::text::PASSWORD_MIN)), error = rx!(form.error("password")))
            view(style = Buttons()) {
                Button(label = "Change password".to_string(), on_click = change, loading = rx!(form.busy.get()))
            }
            Divider()
            view(style = Buttons()) {
                Button(label = "Sign out everywhere".to_string(), variant = variant::Soft, tone = tone::Danger, on_click = everywhere)
                Button(label = "Sign out".to_string(), tone = tone::Danger, leading_icon = Some(LOG_OUT), on_click = sign_out)
            }
        }
    }
}

#[component]
fn AppearancePage() -> Element {
    let ui = store().ui();
    ui! {
        view(style = crate::bits::Col()) {
            Title(text = "Appearance".to_string())
            Typography(content = "THEME".to_string(), kind = typography_kind::Overline, muted = true)
            SegmentedControl(
                value = rx!(if ui.dark.get() { "dark".to_string() } else { "light".to_string() }),
                on_change = cb1(move |v: String| ui.dark.set(v == "dark")),
                options = vec![SegmentOption::new("light", "Light".to_string()), SegmentOption::new("dark", "Dark".to_string())],
            )
        }
    }
}

#[component]
fn DevicesPage() -> Element {
    let s = store();
    let devices = memo(move || s.read(|st| st.devices.clone()));
    let this = crate::device::current().map(|(id, _)| id);
    ui! {
        view(style = crate::bits::Col()) {
            Title(text = "Devices".to_string())
            Typography(content = "Calls are end-to-end encrypted to these devices. Compare a device's security code with the person you're calling to be sure nobody is in between.".to_string(), kind = typography_kind::BodySm, muted = true)
            for d in devices, key = d.id.to_string() {
                view(style = ListRow()) {
                    Icon(data = LAPTOP, size = 20.0)
                    view(style = RowLabel()) {
                        Typography(content = if Some(d.id) == this { format!("{} (this device)", d.name) } else { d.name.clone() }, weight = Some(FontWeight::Medium), lines = Some(1))
                        Typography(content = chat_keys::PublicKey::from_text(&d.public_key).map(|k| chat_keys::fingerprint(&k)).unwrap_or_default(), kind = typography_kind::Caption, muted = true, font = Some(crate::forms::mono()))
                    }
                    if Some(d.id) != this {
                        Action(icon = TRASH_2, label = "Remove device".to_string(), tone = tone::Danger, on_click = { let id = d.id; cb(move || runtime_core::spawn_then(chat_api::auth::remove_device(id), |_| {})) })
                    }
                }
            }
        }
    }
}

#[component]
fn SpaceOverview(#[prop(static)] space: Id) -> Element {
    let s = store();
    let ui = s.ui();
    let (name0, owner) = s.peek(|st| st.spaces.get(&space).map(|sp| (sp.name.clone(), Some(sp.owner) == st.me_id()))).unwrap_or_default();
    let name = signal(name0);
    let form = Form::new();
    let save = cb(move || form.submit(chat_api::spaces::rename_space(space, name.peek()), |_| {}));
    let busy = signal(false);
    let icon = cb(move || {
        busy.set(true);
        runtime_core::spawn_then(
            async move {
                let files = crate::uploads::pick(true, false).await;
                let Some(file) = files.first() else { return Ok(()) };
                let a = crate::uploads::upload(file, Purpose::Icon).await?;
                chat_api::files::set_space_icon(space, Some(a.id)).await.map_err(|e| chat_api::describe(&e))
            },
            move |r: Result<(), String>| {
                busy.set(false);
                if let Err(e) = r {
                    idea_ui::push_toast(e, tone::Danger);
                }
            },
        );
    });
    let delete = cb(move || runtime_core::spawn_then(chat_api::spaces::delete_space(space), move |r| match r {
        Ok(_) => {
            ui.settings.set(None);
            crate::shell::go(crate::store::Place::Home);
        }
        Err(e) => crate::forms::toast(&e),
    }));
    ui! {
        view(style = crate::bits::Col()) {
            Title(text = "Overview".to_string())
            Field(label = Some("Space name".to_string()), value = name, on_change = set(name), error = rx!(form.error("name").or(form.general.get())))
            view(style = Buttons()) {
                Button(label = "Change icon".to_string(), variant = variant::Soft, on_click = icon, loading = rx!(busy.get()))
                Button(label = "Save".to_string(), on_click = save, loading = rx!(form.busy.get()))
            }
            if owner {
                Divider()
                Typography(content = "Delete this space".to_string(), kind = typography_kind::H6)
                Typography(content = "Every channel and message in it goes too. This can't be undone.".to_string(), kind = typography_kind::BodySm, muted = true)
                view(style = Buttons()) {
                    Button(label = "Delete space".to_string(), tone = tone::Danger, leading_icon = Some(TRASH_2), on_click = delete)
                }
            }
        }
    }
}

#[component]
fn SpaceMembers(#[prop(static)] space: Id) -> Element {
    let s = store();
    let members = {
        let s = s.clone();
        memo(move || {
            s.read(|st| {
                let mut v: Vec<(Id, Id, bool)> = st.spaces.get(&space).map(|sp| sp.members.iter().map(|(u, m)| (*u, m.roles.first().copied().unwrap_or(Id::nil()), *u == sp.owner)).collect()).unwrap_or_default();
                v.sort_by_cached_key(|(u, _, owned)| (!owned, st.display_name(*u).to_lowercase()));
                v
            })
        })
    };
    ui! {
        view(style = crate::bits::Col()) {
            Title(text = "Members".to_string())
            // The line shows the member's first role name read-only; the old
            // Member/Admin picker has no named-role equivalent for renaming or
            // per-role permission editing, which a later task will add.
            for (user, role, owned) in members, key = user.to_string() {
                MemberLine(space = space, user = user, role = role, owned = owned)
            }
        }
    }
}

#[component]
fn MemberLine(#[prop(static)] space: Id, #[prop(static)] user: Id, #[prop(static, default = Id::nil())] role: Id, #[prop(static)] owned: bool) -> Element {
    let s = store();
    let name = s.peek(|st| st.display_name(user));
    let label = s.peek(|st|
        if owned {
            "Owner".to_string()
        } else {
            st.spaces.get(&space).and_then(|sp| sp.roles.get(&role)).map(|r| r.name.clone()).unwrap_or_else(|| "Member".to_string())
        }
    );
    let grants = s.peek(|st| {
        let Some(sp) = st.spaces.get(&space) else { return Vec::<SelectOption>::new() };
        let Some(me) = st.me_id() else { return Vec::<SelectOption>::new() };
        vec![SelectOption::new(Id::nil().to_string(), "Member".to_string())].into_iter()
            .chain(sp.roles.values().filter(|r| chat_model::perms::can_grant(sp, me, user, r.id)).map(|r| SelectOption::new(r.id.to_string(), r.name.clone())))
            .collect()
    });
    let can_remove = s.peek(|st| st.spaces.get(&space).is_some_and(|sp| chat_model::perms::can_moderate(sp, st.me_id().unwrap_or_default(), user)));
    let value = signal(role.to_string());
    let on_role = cb1(move |v: String| {
        let to = v.parse::<Id>().ok().filter(|r| *r != Id::nil()).map(|r| vec![r]).unwrap_or_default();
        value.set(v);
        runtime_core::spawn_then(chat_api::spaces::set_roles(space, user, to), |r| {
            if let Err(e) = r {
                idea_ui::push_toast(chat_api::describe(&e), tone::Danger);
            }
        });
    });
    let remove = cb(move || runtime_core::spawn_then(chat_api::spaces::remove_member(space, user), |r| {
        if let Err(e) = r {
            idea_ui::push_toast(chat_api::describe(&e), tone::Danger);
        }
    }));
    let can_grant = grants.len() > 1;
    ui! {
        view(style = ListRow()) {
            PersonAvatar(user = user, size = AvatarSize::Sm)
            view(style = RowLabel()) {
                Typography(content = name.clone(), weight = Some(FontWeight::Medium), lines = Some(1))
                Typography(content = label, kind = typography_kind::Caption, muted = true)
            }
            if can_grant {
                Select(value = value, on_change = on_role.clone(), options = grants.clone(), size = idea_ui::SelectSize::Sm)
            }
            if can_remove {
                Action(icon = X, label = format!("Remove {name}"), tone = tone::Danger, on_click = remove.clone())
            }
        }
    }
}

#[component]
fn SpaceInvites(#[prop(static)] space: Id) -> Element {
    let list: Signal<Vec<Invite>> = signal(Vec::new());
    let load = move || runtime_core::spawn_then(chat_api::spaces::invites(space), move |r| {
        if let Ok(v) = r {
            list.set(v);
        }
    });
    load();
    let ui = store().ui();
    ui! {
        view(style = crate::bits::Col()) {
            Title(text = "Invites".to_string())
            view(style = Buttons()) {
                Button(label = "New invite".to_string(), on_click = cb(move || { ui.settings.set(None); ui.dialog.set(Some(crate::store::Dialog::Invite(space))); }))
            }
            if list.get().is_empty() {
                Typography(content = "No invites are active.".to_string(), muted = true, kind = typography_kind::BodySm)
            }
            for i in list, key = i.code.clone() {
                view(style = ListRow()) {
                    view(style = RowLabel()) {
                        Typography(content = i.code.clone(), font = Some(crate::forms::mono()))
                        Typography(content = format!("{} uses{}", i.uses, i.max_uses.map(|m| format!(" of {m}")).unwrap_or_default()), kind = typography_kind::Caption, muted = true)
                    }
                    Button(label = "Revoke".to_string(), size = idea_ui::size::Sm, variant = variant::Soft, tone = tone::Danger, on_click = {
                        let code = i.code.clone();
                        cb(move || runtime_core::spawn_then(chat_api::spaces::revoke_invite(space, code.clone()), move |_| load()))
                    })
                }
            }
        }
    }
}

#[component]
fn SpaceChannels(#[prop(static)] space: Id, #[prop(static, default = signal(Settings::Profile))] page: Signal<Settings>) -> Element {
    let s = store();
    let channels = memo(move || {
        s.read(|st| {
            st.sidebar(space).iter().flat_map(|sec| sec.entries.iter().map(|e| (e.channel.id, e.channel.name.clone(), e.channel.kind))).collect::<Vec<_>>()
        })
    });
    ui! {
        view(style = crate::bits::Col()) {
            Title(text = "Channels".to_string())
            for (id, name, kind) in channels, key = id.to_string() {
                view(style = ListRow()) {
                    Icon(data = if kind == ChannelKind::Voice { icons_lucide::VOLUME_2 } else { icons_lucide::HASH }, size = 16.0)
                    view(style = RowLabel()) { Typography(content = name.clone(), weight = Some(FontWeight::Medium)) }
                    Button(label = "Edit".to_string(), size = idea_ui::size::Sm, variant = variant::Soft, on_click = cb(move || page.set(Settings::Channel(id))))
                }
            }
        }
    }
}

#[component]
fn ChannelPage(#[prop(static)] channel: Id) -> Element {
    let s = store();
    let ui = s.ui();
    let Some(c) = s.peek(|st| st.channels.get(&channel).cloned()) else { return runtime_core::fragment(Vec::new()) };
    let space = c.space;
    let name = signal(c.name.clone());
    let topic = signal(c.topic.clone());
    let view_role = signal(c.view.to_string());
    let post_role = signal(c.post.to_string());
    let category = signal(c.category.map(|k| k.to_string()).unwrap_or_else(|| "none".to_string()));
    let categories: Vec<SelectOption> = std::iter::once(SelectOption::new("none", "No category".to_string()))
        .chain(
            space
                .and_then(|sp| s.peek(|st| st.spaces.get(&sp).map(|sp| sp.categories.clone())))
                .unwrap_or_default()
                .into_iter()
                .map(|k| SelectOption::new(k.id.to_string(), k.name)),
        )
        .collect();
    let form = Form::new();
    let saved = signal(false);
    let save = cb(move || {
        saved.set(false);
        let cat = category.peek().parse::<Id>().ok();
        form.submit(
            chat_api::channels::update_channel(channel, name.peek(), topic.peek(), cat, view_role.peek().parse::<u64>().unwrap_or(0), post_role.peek().parse::<u64>().unwrap_or(0)),
            move |_| saved.set(true),
        );
    });
    let delete = cb(move || runtime_core::spawn_then(chat_api::channels::delete_channel(channel), move |r| match r {
        Ok(_) => {
            ui.settings.set(None);
            if let Some(sp) = space {
                crate::shell::open_space(sp);
            }
        }
        Err(e) => crate::forms::toast(&e),
    }));
    // The old "everyone/admins/owner" floors map onto permission masks, not
    // role ids: each option offers a role's permission set as WHO CAN SEE/POST.
    // Two roles with the same set collapse into one option; editing roles
    // themselves is a later task.
    let masks: std::collections::BTreeMap<u64, String> = space
        .and_then(|sp| s.peek(|st| st.spaces.get(&sp).map(|sp| sp.roles.values().map(|r| (r.permissions, r.name.clone())).collect())))
        .unwrap_or_default();
    let roles: Vec<SelectOption> = std::iter::once(SelectOption::new("0", "Everyone".to_string()))
        .chain(masks.iter().map(|(mask, name)| SelectOption::new(mask.to_string(), name.clone())))
        .collect();
    ui! {
        view(style = crate::bits::Col()) {
            Title(text = format!("#{}", c.name))
            if saved.get() {
                Alert(title = "Saved.".to_string(), tone = tone::Success, variant = variant::Soft)
            }
            if form.general.get().is_some() {
                Alert(title = rx!(form.general.get().unwrap_or_default()), tone = tone::Danger, variant = variant::Soft)
            }
            Field(label = Some("Name".to_string()), value = name, on_change = set(name), error = rx!(form.error("name")))
            Field(label = Some("Topic".to_string()), value = topic, on_change = set(topic), placeholder = Some("What this channel is for".to_string()), error = rx!(form.error("topic")))
            Typography(content = "CATEGORY".to_string(), kind = typography_kind::Overline, muted = true)
            Select(value = category, on_change = cb1(move |v: String| category.set(v)), options = categories)
            Typography(content = "WHO CAN SEE IT".to_string(), kind = typography_kind::Overline, muted = true)
            Select(value = view_role, on_change = cb1(move |v: String| view_role.set(v)), options = roles.clone())
            Typography(content = "WHO CAN POST".to_string(), kind = typography_kind::Overline, muted = true)
            Select(value = post_role, on_change = cb1(move |v: String| post_role.set(v)), options = roles.clone())
            view(style = Buttons()) {
                Button(label = "Delete channel".to_string(), variant = variant::Soft, tone = tone::Danger, on_click = delete)
                Button(label = "Save".to_string(), on_click = save, loading = rx!(form.busy.get()))
            }
        }
    }
}
