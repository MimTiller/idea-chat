//! The small pieces every screen shares: a person's avatar with their
//! presence, counts, navigation rows, labelled icon buttons, section labels,
//! empty states, and how times are written.

use std::rc::Rc;

use chat_model::{Id, Presence};
use idea_ui::{
    tone, typography_kind, variant, Avatar, AvatarSize, Tag, Button, Icon, IconButton, IconButtonSize, IdeaThemeRef,
    IconButtonVariant, ToneRef, Tooltip, Typography,
};
use runtime_core::{
    component, memo, rx, stylesheet, ui, AlignItems, Element, FlexDirection, IconData, JustifyContent, Position,
};

use crate::platform;
use crate::store::store;

// ---------------------------------------------------------------- avatars

stylesheet! {
    pub AvatarWrap<()> {
        base(_t) {
            position: Position::Relative,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    /// Who is talking in our call: a ring just outside the avatar. Laid over
    /// it rather than around it, so lighting up moves nothing.
    pub SpeakingRing<IdeaThemeRef> {
        base(_t) {
            position: Position::Absolute,
            top: -3.0,
            left: -3.0,
            right: -3.0,
            bottom: -3.0,
            border_radius: 999.0,
            border_width: 0.0,
        }
        variant speaking {
            #[default]
            off(_t) {}
            on(t) { border_width: 2.0, border_color: t.intent.success.fg() }
        }
    }
}

stylesheet! {
    /// The presence dot on an avatar's corner, ringed in the plane behind it
    /// so it reads on any background.
    pub PresenceDot<IdeaThemeRef> {
        base(t) {
            position: Position::Absolute,
            right: -1.0,
            bottom: -1.0,
            width: 12.0,
            height: 12.0,
            border_radius: 999.0,
            border_width: 2.5,
            border_color: t.color.background(),
            background: t.intent.success.fg(),
        }
        variant presence {
            #[default]
            online(t) { background: t.intent.success.fg() }
            idle(t) { background: t.intent.warning.fg() }
            busy(t) { background: t.intent.danger.fg() }
            offline(t) { background: t.color.border_strong() }
        }
        variant size {
            #[default]
            small(_t) {}
            large(_t) { width: 16.0, height: 16.0, border_width: 3.0 }
        }
    }
}

pub fn presence_key(p: Presence) -> &'static str {
    match p {
        Presence::Online => "online",
        Presence::Idle => "idle",
        Presence::Busy => "busy",
        Presence::Offline => "offline",
    }
}

pub fn presence_label(p: Presence) -> &'static str {
    match p {
        Presence::Online => "Online",
        Presence::Idle => "Away",
        Presence::Busy => "Do not disturb",
        Presence::Offline => "Offline",
    }
}

/// A person's picture (or initials, in a colour of their own), with their
/// presence on its corner.
#[component]
pub fn PersonAvatar(
    #[prop(static)] user: Id,
    #[prop(static, default = AvatarSize::Md)] size: AvatarSize,
    #[prop(static, default = true)] presence: bool,
    /// Ring the avatar while this person talks in our call.
    #[prop(static, default = false)] speaking: bool,
) -> Element {
    let s = store();
    let face = {
        let s = s.clone();
        memo(move || s.read(|st| st.users.get(&user).map(|u| (u.initials(), u.avatar.clone()))))
    };
    let shown = memo(move || s.read(|st| st.presence_of(user)));
    let large = matches!(size, AvatarSize::Lg | AvatarSize::Xl);
    ui! {
        view(style = AvatarWrap()) {
            Avatar(
                initials = rx!(face.get().map(|f| f.0).unwrap_or_default()),
                src = rx!(face.get().and_then(|f| f.1).map(|a| platform::absolute(&a).into())),
                tone = tone::palette_from_seed(&user.to_string()),
                size = size,
            )
            if speaking {
                view(style = SpeakingRing().speaking(runtime_core::derived(move || {
                    if crate::media::speaking().get().contains(&user) { SpeakingRingSpeaking::On } else { SpeakingRingSpeaking::Off }
                })))
            }
            if presence {
                view(style = PresenceDot()
                    .presence(runtime_core::derived(move || PresenceDotPresence::from_key(presence_key(shown.get()))))
                    .size(if large { PresenceDotSize::Large } else { PresenceDotSize::Small }))
            }
        }
    }
}

impl PresenceDotPresence {
    fn from_key(k: &str) -> PresenceDotPresence {
        match k {
            "idle" => PresenceDotPresence::Idle,
            "busy" => PresenceDotPresence::Busy,
            "offline" => PresenceDotPresence::Offline,
            _ => PresenceDotPresence::Online,
        }
    }
}

// ---------------------------------------------------------------- counts

/// A mention count, in the warm accent; nothing for zero.
#[component]
pub fn Count(count: u32) -> Element {
    let shown = count.clone();
    // A memo is Copy, so the branch (rebuilt as it toggles) can read it.
    let label = memo(move || match shown.get() {
        n if n > 99 => "99+".to_string(),
        n => n.to_string(),
    });
    ui! {
        if count.get() > 0 {
            Tag(label = rx!(label.get()), tone = tone::Danger, variant = variant::Filled)
        }
    }
}

// ---------------------------------------------------------------- rows

stylesheet! {
    /// A row in a navigation list: a channel, a conversation, a person.
    pub NavRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
            min_height: 34.0,
            padding_left: 8.0,
            padding_right: 8.0,
            margin_left: 8.0,
            margin_right: 8.0,
            border_radius: 6.0,
            color: t.color.text_muted(),
            cursor: runtime_core::Cursor::Pointer,
        }
        state hovered(t) { background: t.color.surface_alt(), color: t.color.text() }
        variant selected {
            #[default]
            off(_t) {}
            on(t) { background: t.intent.primary.soft_bg(), color: t.intent.primary.soft_text() }
        }
        variant unread {
            #[default]
            off(_t) {}
            on(t) { color: t.color.text() }
        }
        variant tall {
            #[default]
            off(_t) {}
            on(_t) { min_height: 42.0 }
        }
        transitions { background: 120ms EaseOut }
    }
}

stylesheet! {
    /// A row's label: takes the space and truncates.
    pub RowLabel<()> {
        base(_t) {
            flex_grow: 1.0,
            flex_basis: 0.0,
            min_width: 0.0,
            flex_direction: FlexDirection::Column,
        }
    }
}

// ---------------------------------------------------------------- actions

/// A round icon button with a name: shown as a tooltip, and given to
/// assistive tech (IconButton itself takes none, so a wrapper carries it).
#[component]
pub fn Action(
    #[prop(static, default = icons_lucide::CIRCLE)] icon: IconData,
    #[prop(static)] label: String,
    #[prop(static, default = Rc::new(|| {}) as Rc<dyn Fn()>)] on_click: Rc<dyn Fn()>,
    #[prop(static, default = ToneRef::from(tone::Neutral))] tone: ToneRef,
    #[prop(static, default = IconButtonVariant::from(variant::Ghost))] variant: IconButtonVariant,
    #[prop(static, default = IconButtonSize::Md)] size: IconButtonSize,
    #[prop(default = false)] selected: bool,
    #[prop(static, default = None)] size_px: Option<f32>,
) -> Element {
    // A chosen toggle reads as a soft wash of the accent, which sits better
    // in a toolbar than idea-ui's filled "selected" button.
    let (base_tone, base_variant, on) = (tone.clone(), variant.clone(), selected.clone());
    let button = ui! {
        IconButton(
            glyph = String::new(),
            icon = Some(icon),
            tone = rx!(if selected.get() { ToneRef::from(tone::Primary) } else { base_tone.clone() }),
            variant = rx!(if on.get() { IconButtonVariant::from(variant::Soft) } else { base_variant.clone() }),
            size = size,
            size_px = size_px,
            on_click = on_click,
        )
    };
    let named: Element = runtime_core::view(vec![button]).a11y_role(runtime_core::Role::Group).a11y_label(label.clone()).into();
    ui! { Tooltip(text = label) { named } }
}

// ---------------------------------------------------------------- sections

stylesheet! {
    pub SectionRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            padding_left: 16.0,
            padding_right: 10.0,
            padding_top: 14.0,
            padding_bottom: 4.0,
        }
    }
}

/// A small heading over a list, with an optional action at its end.
#[component]
pub fn Section(#[prop(static)] label: String, #[prop(static, default = None)] action: Option<Element>) -> Element {
    ui! {
        view(style = SectionRow()) {
            Typography(content = label.to_uppercase(), kind = typography_kind::Overline, muted = true)
            if let Some(a) = action { a }
        }
    }
}

// ---------------------------------------------------------------- empty states

stylesheet! {
    pub EmptyBox<()> {
        base(_t) {
            flex_grow: 1.0,
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            gap: 10.0,
            padding_left: 24.0,
            padding_right: 24.0,
            padding_top: 32.0,
            padding_bottom: 32.0,
        }
    }
}

stylesheet! {
    pub EmptyMark<IdeaThemeRef> {
        base(t) {
            width: 56.0,
            height: 56.0,
            border_radius: 999.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            background: t.intent.primary.soft_bg(),
            margin_bottom: 4.0,
        }
    }
}

stylesheet! {
    pub EmptyText<()> {
        base(_t) {
            max_width: 360.0,
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            gap: 4.0,
        }
    }
}

/// A screen with nothing in it yet, saying why and what to do.
#[component]
pub fn Empty(
    #[prop(static, default = icons_lucide::CIRCLE)] icon: IconData,
    #[prop(static)] title: String,
    #[prop(static, default = String::new())] body: String,
    #[prop(static, default = None)] action: Option<(String, Rc<dyn Fn()>)>,
) -> Element {
    let has_body = !body.is_empty();
    ui! {
        view(style = EmptyBox()) {
            view(style = EmptyMark()) { Icon(data = icon, size = 24.0, tone = Some(tone::Primary.into())) }
            view(style = EmptyText()) {
                Typography(content = title, kind = typography_kind::H6, align = runtime_core::TextAlign::Center)
                if has_body {
                    Typography(content = body.clone(), muted = true, align = runtime_core::TextAlign::Center)
                }
            }
            if let Some((label, on_click)) = action.clone() {
                Button(label = label, on_click = on_click, variant = variant::Soft)
            }
        }
    }
}

// ---------------------------------------------------------------- time

const MINUTE: i64 = 60_000;
const DAY: i64 = 86_400_000;

fn local(ms: i64) -> i64 {
    ms + platform::utc_offset_min() as i64 * MINUTE
}

/// `14:05`, local.
pub fn clock(ms: i64) -> String {
    let m = local(ms).rem_euclid(DAY) / MINUTE;
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// The civil date of a day number (days since 1970-01-01).
fn civil(days: i64) -> (i64, u32, u32) {
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEKDAYS: [&str; 7] = ["Thursday", "Friday", "Saturday", "Sunday", "Monday", "Tuesday", "Wednesday"];

/// "Today", "Yesterday", "Monday", or "12 Oct 2026" for a day further back.
pub fn day_label(ms: i64, now: i64) -> String {
    let (day, today) = (local(ms).div_euclid(DAY), local(now).div_euclid(DAY));
    match today - day {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => WEEKDAYS[day.rem_euclid(7) as usize].into(),
        _ => {
            let (y, m, d) = civil(day);
            let (ty, _, _) = civil(today);
            if y == ty { format!("{d} {}", MONTHS[m as usize - 1]) } else { format!("{d} {} {y}", MONTHS[m as usize - 1]) }
        }
    }
}

/// When a message was sent, as its row shows it: the time today, the day
/// and time before.
pub fn stamp(ms: i64, now: i64) -> String {
    match day_label(ms, now).as_str() {
        "Today" => clock(ms),
        day => format!("{day} {}", clock(ms)),
    }
}

/// A duration, for a call record: "4 min", "1 h 12 min", "under a minute".
pub fn duration(ms: i64) -> String {
    let mins = ms / MINUTE;
    match mins {
        0 => "under a minute".into(),
        1..=59 => format!("{mins} min"),
        _ => format!("{} h {} min", mins / 60, mins % 60),
    }
}

/// Bytes, as people read them.
pub fn size(bytes: u64) -> String {
    match bytes {
        0..=999 => format!("{bytes} B"),
        1_000..=999_999 => format!("{:.0} KB", bytes as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1} MB", bytes as f64 / 1e6),
        _ => format!("{:.1} GB", bytes as f64 / 1e9),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_are_right() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_750), (2026, 10, 24));
    }

    #[test]
    fn day_labels_count_back() {
        let now = 20_750 * DAY + 12 * 3_600_000;
        assert_eq!(day_label(now, now), "Today");
        assert_eq!(day_label(now - DAY, now), "Yesterday");
        assert_eq!(day_label(now - 30 * DAY, now), "24 Sep");
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(30_000), "under a minute");
        assert_eq!(duration(4 * MINUTE), "4 min");
        assert_eq!(duration(72 * MINUTE), "1 h 12 min");
    }
}

stylesheet! {
    /// A scroll area that takes the rest of its column.
    pub Fill<()> {
        base(_t) {
            flex_grow: 1.0,
            min_height: 0.0,
        }
    }
}

stylesheet! {
    /// A column of form rows: what a screen's body sits in.
    pub Col<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 16.0,
        }
    }
}

stylesheet! {
    /// A column that takes the rest of its parent: for a component with
    /// several parts (a header, a scroll area, a footer) that must fill.
    pub Grow<()> {
        base(_t) {
            flex_grow: 1.0,
            flex_basis: 0.0,
            min_height: 0.0,
            min_width: 0.0,
            flex_direction: FlexDirection::Column,
            position: Position::Relative,
        }
    }
}
