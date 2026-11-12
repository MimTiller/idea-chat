//! A one-room video call over `datagram` and nothing else from the chat app:
//! a grid of everyone, a control bar, and every audio setting in a modal.
//! Light or dark, following the system until the bar's toggle says otherwise.
//!
//! ```text
//! cargo run -p call-relay -- 0.0.0.0:4433   # on the server
//! idealyst dev --web --local crates/quic-call              # the page, on :8080
//! ```
//!
//! `--local` matters: without it the dev server runs `app()` itself, and the
//! call code is wasm-only.
//!
//! DeepFilterNet runs inline in the page and needs a release build to keep up:
//! `idealyst build --web --release crates/quic-call` (which needs `wasm-opt`
//! from binaryen), then serve `dist/web` — `tools/quic-call-public.sh` does.
//! The dev server's unoptimized wasm falls far behind real time with it on;
//! the page's "dsp % of real time" says by how much. The model must be in
//! `assets/` first: `tools/fetch-model.sh quic-call`.
//!
//! The relay prints a link that fills in the URL and certificate hash. Open it
//! in two browsers (or on two machines), press Join in both, and talk. Use
//! headphones, or leave echo cancellation on.

#[cfg(target_arch = "wasm32")]
mod call;
// Platform-free, so their tests run natively. The connection grading is
// used only by the web build's frame loop.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod connection;
mod room;

use icons_lucide::{DICES, MIC, MIC_OFF, MONITOR_UP, MONITOR_X, MOON, PHONE_OFF, SETTINGS, SUN, VIDEO, VIDEO_OFF};
use idea_ui::{
    dark_theme, install_idea_theme_reactive, Adornment, light_theme, tone, typography_kind, variant, Button, Card, Field,
    IconButton, IconButtonSize, IdeaThemeRef, Modal, SegmentOption, SegmentedControl, Slider, Stack, StackAxis,
    StackGap, StackJustify, StackPadding, Switch, ToneRef, Tooltip, Typography,
};
use runtime_core::accessibility::Role;
use runtime_core::primitives::presence::{presence, PresenceAnim, PresenceState};
use runtime_core::{
    rx, signal, stylesheet, switch, ui, AlignItems, Color, Easing, Element, FlexDirection, IconData,
    JustifyContent, Length, Shadow, Signal, StyleApplication, Tokenized,
};
use std::rc::Rc;

/// What the room shows, as data: the tiles in order, and what each has. The
/// room is rebuilt when this changes — someone joins, a camera or a share
/// starts or stops, we mute — and not otherwise, so video keeps playing.
// Built by the web-only frame loop; the native build only sees the type.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
enum TileKey {
    /// Someone's screen share.
    Screen(u32),
    MyScreen,
    /// Us: whether our camera is on. Not whether we are muted — the tile
    /// shows that from the signal itself, so muting rebuilds nothing.
    Me { camera: bool },
    /// Someone else: whether their camera is on.
    Person { id: u32, camera: bool },
}

stylesheet! {
    /// The window, in the theme's colours — eased, so the dark-mode toggle
    /// fades rather than flashes.
    Shell<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            height: Length::pct(100.0),
            background: t.color.background(),
            color: t.color.text(),
        }
        transitions {
            background: 220ms EaseInOut,
            color: 220ms EaseInOut,
        }
    }
}

stylesheet! {
    /// The page in a call: room above, control bar below, filling the window.
    Page<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: 0.0,
            padding_left: 16.0,
            padding_right: 16.0,
            padding_top: 16.0,
            padding_bottom: 12.0,
            gap: 12.0,
        }
    }
}

stylesheet! {
    /// The control bar: what is going on at the left, the call's controls in
    /// the middle, the page's own at the right.
    Bar<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 12.0,
        }
    }
}

stylesheet! {
    /// Either end of the bar. Both grow alike, so the dock stays centred
    /// whatever the status line says.
    BarEnd<()> {
        base(_t) {
            flex_grow: 1.0,
            flex_basis: 0.0,
            min_width: 0.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
        }
        variant side {
            #[default]
            start(_t) {}
            end(_t) { justify_content: JustifyContent::FlexEnd }
        }
    }
}

stylesheet! {
    /// The call's buttons, together on a floating pill.
    Dock<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 10.0,
            padding_left: 10.0,
            padding_right: 10.0,
            padding_top: 8.0,
            padding_bottom: 8.0,
            border_radius: 999.0,
            border_width: 1.0,
            border_color: t.color.border(),
            background: t.color.surface(),
            shadow: Shadow { x: 0.0, y: 6.0, blur: 24.0, color: Tokenized::Literal(Color("rgba(0, 0, 0, 0.18)".into())) },
        }
        transitions {
            background: 220ms EaseInOut,
            border_color: 220ms EaseInOut,
        }
    }
}

stylesheet! {
    /// Before joining: the form, centred in the window.
    Lobby<()> {
        base(_t) {
            flex_grow: 1.0,
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding_left: 16.0,
            padding_right: 16.0,
            padding_top: 16.0,
            padding_bottom: 16.0,
        }
    }
}

stylesheet! {
    LobbyCard<()> {
        base(_t) {
            width: 460.0,
            max_width: Length::pct(100.0),
        }
    }
}

stylesheet! {
    /// The lobby card's title row: the name, and the theme toggle across.
    TitleRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
        }
    }
}

stylesheet! {
    /// Long settings scroll inside the modal rather than past the window.
    SettingsScroll<()> {
        base(_t) {
            max_height: 560.0,
        }
    }
}

/// `Video`, which shows the camera and everyone else's. Web only, like the call.
pub fn register_scene_extensions<H: idealyst::SceneHost>(registry: &mut runtime_scene::Registry<H>) {
    call_video::register_scene_extensions(registry);
}

pub fn scene_app() -> Element {
    app()
}

/// A typed callback for a prop, which `Rc::new` on a bare closure is not.
fn cb(f: impl Fn() + 'static) -> Rc<dyn Fn()> {
    Rc::new(f)
}

/// The same, for a callback that takes a value.
fn cb1<T>(f: impl Fn(T) + 'static) -> Rc<dyn Fn(T)> {
    Rc::new(f)
}

/// A round toolbar button: an idea-ui IconButton with a tooltip saying what
/// pressing it does. IconButton takes no accessible name, so its wrapper
/// carries one ("Mute, group" then the button) — which is also what the
/// browser tests press it by.
fn tool(label: &str, icon: IconData, tone: ToneRef, variant: idea_ui::IconButtonVariant, on_click: Rc<dyn Fn()>) -> Element {
    let button = ui! {
        IconButton(
            glyph = String::new(),
            icon = Some(icon),
            tone = tone,
            variant = variant,
            size = IconButtonSize::Lg,
            size_px = Some(46.0),
            on_click = on_click,
        )
    };
    let named: Element = runtime_core::view(vec![button]).a11y_role(Role::Group).a11y_label(label).into();
    ui! { Tooltip(text = label.to_string()) { named } }
}

/// The light/dark toggle, showing what pressing it switches to.
fn theme_toggle(dark: Signal<bool>) -> Element {
    switch(
        move || dark.get(),
        move |&is_dark: &bool| {
            tool(
                if is_dark { "Light mode" } else { "Dark mode" },
                if is_dark { SUN } else { MOON },
                tone::Neutral.into(),
                variant::Ghost.into(),
                cb(move || dark.set(!is_dark)),
            )
        },
    )
}

/// What a `call-auth` send code allows, in words.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn describe_send(code: &str) -> String {
    let words: Vec<&str> = code
        .chars()
        .filter_map(|c| match c {
            'a' => Some("voice"),
            'v' => Some("camera"),
            's' => Some("screen"),
            _ => None,
        })
        .collect();
    words.join(", ")
}

/// A fixed-width bar, so the meter does not reflow the page every frame.
fn meter_bar(name: &str, level: f32) -> String {
    let filled = (level.clamp(0.0, 1.0) * 24.0).round() as usize;
    format!("{name} [{}{}]", "#".repeat(filled), "-".repeat(24 - filled))
}

/// The bitrate mode the settings describe; kb/s in, bps out.
fn rate_mode(auto: bool, audio_kbps: f32, video_kbps: f32) -> call_rate::Mode {
    if auto {
        call_rate::Mode::Auto
    } else {
        call_rate::Mode::Manual { audio_bps: (audio_kbps * 1000.0) as u32, video_bps: (video_kbps * 1000.0) as u32 }
    }
}

pub fn app() -> Element {
    // The system's preference to start with; the toggle overrides it for the
    // page's life. Every idea-ui component and every sheet here reads theme
    // tokens, so flipping it re-themes the lot.
    let dark: Signal<bool> = signal(matches!(runtime_core::color_scheme(), runtime_core::ColorScheme::Dark));
    install_idea_theme_reactive(move || if dark.get() { dark_theme() } else { light_theme() });

    #[cfg(target_arch = "wasm32")]
    let (initial_url, initial_hash) = (call::query("url").or_else(call::default_relay), call::query("hash"));
    #[cfg(not(target_arch = "wasm32"))]
    let (initial_url, initial_hash): (Option<String>, Option<String>) = (None, None);

    let url: Signal<String> = signal(initial_url.unwrap_or_else(|| "https://127.0.0.1:4433/".to_string()));
    let hash: Signal<String> = signal(initial_hash.unwrap_or_default());
    // Which room, and the grant that lets us in — for a relay that checks
    // them (see `call-auth`). `?renew=` is where fresh grants come from.
    #[cfg(target_arch = "wasm32")]
    let (initial_room, initial_grant, renew_url) = (call::query("room"), call::query("grant"), call::query("renew"));
    #[cfg(not(target_arch = "wasm32"))]
    let (initial_room, initial_grant): (Option<String>, Option<String>) = (None, None);
    let room_id: Signal<String> = signal(initial_room.unwrap_or_default());
    let grant: Signal<String> = signal(initial_grant.unwrap_or_default());
    // The room's end-to-end key, from the address's `#key=` — which never
    // reaches a server. Empty: not encrypted end to end.
    #[cfg(target_arch = "wasm32")]
    let initial_key = call::key_from_address();
    #[cfg(not(target_arch = "wasm32"))]
    let initial_key: Option<String> = None;
    let e2ee_key: Signal<String> = signal(initial_key.unwrap_or_default());
    // Relay numbers whose media does not open with our key.
    let unreadable: Signal<std::collections::BTreeSet<u32>> = signal(Default::default());
    #[cfg(not(target_arch = "wasm32"))]
    let _ = unreadable;
    // Everyone's name by relay number, from their grants. Empty on a relay
    // that takes none, where people are their numbers.
    let names: Signal<std::collections::BTreeMap<u32, String>> = signal(Default::default());
    // Who has their mic off, by relay number, as they announce it.
    let mic_off: Signal<std::collections::BTreeSet<u32>> = signal(Default::default());
    #[cfg(not(target_arch = "wasm32"))]
    let _ = mic_off;
    let echo_cancellation: Signal<bool> = signal(true);
    // DeepFilterNet by default: it is the one that does something about the
    // noise people actually notice. Its 8 MB model is fetched on first join.
    let suppression: Signal<String> = signal("neural".to_string());
    // The browser's own gain on the track. Off: one fewer stage moving the
    // level before the model and the leveller see it.
    let auto_gain: Signal<bool> = signal(false);
    // DeepFilterNet's tuning, libDF's knobs. Defaults are upstream's, with the
    // attenuation unlimited (100 dB is libDF's "no limit").
    let max_reduction: Signal<f32> = signal(100.0);
    let min_thresh: Signal<f32> = signal(-10.0);
    let erb_thresh: Signal<f32> = signal(30.0);
    let df_thresh: Signal<f32> = signal(20.0);
    let post_filter: Signal<bool> = signal(false);
    // The voice_dsp layer, a switch per stage. Live, no reopen. Defaults from
    // `call::VoiceStages` — mirrored here because the call module is
    // wasm-only.
    let dehum: Signal<bool> = signal(true);
    let declick: Signal<bool> = signal(false);
    let level: Signal<bool> = signal(true);
    let expand: Signal<bool> = signal(true);
    // Bitrate: automatic by default (see `call-rate`), or fixed by hand — the
    // sliders, in kb/s; moving one switches to manual.
    let rate_auto: Signal<bool> = signal(true);
    let manual_audio: Signal<f32> = signal(40.0);
    let manual_video: Signal<f32> = signal(600.0);
    // What we send at and why, refreshed once a second.
    let rate_line: Signal<String> = signal("Not in a call".to_string());
    let joined: Signal<bool> = signal(false);
    let status: Signal<String> = signal("Not connected".to_string());
    let me: Signal<String> = signal(String::new());
    #[cfg(not(target_arch = "wasm32"))]
    let _ = me;
    let counters: Signal<String> = signal(String::new());
    let mic: Signal<String> = signal(meter_bar("mic ", 0.0));
    let sent_level: Signal<String> = signal(meter_bar("sent", 0.0));

    #[cfg(target_arch = "wasm32")]
    let active: Rc<std::cell::RefCell<Option<call::Call>>> = Rc::new(std::cell::RefCell::new(None));

    // Copy the call's counters into signals once a frame, only when they move.
    #[cfg(target_arch = "wasm32")]
    {
        let active = active.clone();
        // The DSP load over the last second of audio, not since joining, so a
        // suppression change shows up in it straight away.
        let window = std::cell::Cell::new((0.0f64, 0.0f64, 0.0f64));
        let raf = runtime_core::raf_loop(move || {
            let Some(call) = active.borrow().as_ref().map(|c| c.stats.clone()) else { return };
            let mut s = call.borrow_mut();
            let (dsp0, audio0, mut load) = window.get();
            if s.audio_ms < audio0 {
                window.set((s.dsp_ms, s.audio_ms, load));
            } else if s.audio_ms - audio0 >= 1000.0 {
                load = (s.dsp_ms - dsp0) / (s.audio_ms - audio0);
                window.set((s.dsp_ms, s.audio_ms, load));
            }
            let line = format!(
                "sent {} · received {} · late {} · concealed {} · underruns {} · buffer {:.0} ms · \
                 peers {} · dsp {:.0}% of real time · hum notches {} · {} Hz · max datagram {}{}{}",
                s.sent,
                s.received,
                s.late,
                s.concealed,
                s.underruns,
                s.playout_ms,
                s.peers,
                load * 100.0,
                s.hum_notches,
                s.rate,
                s.max_datagram,
                if s.unsent > 0 { format!(" · unsent {}", s.unsent) } else { String::new() },
                if s.codec_failed { " · OPUS CANNOT RUN AT THIS RATE" } else { "" },
            );
            if line != counters.get() {
                counters.set(line);
            }
            let bar = meter_bar("mic ", s.mic_peak);
            if bar != mic.get() {
                mic.set(bar);
            }
            let bar = meter_bar("sent", s.out_peak);
            if bar != sent_level.get() {
                sent_level.set(bar);
            }
            s.mic_peak = 0.0;
            s.out_peak = 0.0;
        });
        std::mem::forget(raf);
    }

    // Video, sharing and the room. `camera` and `screen` are what the buttons
    // ask for; the loop below makes the call match them — on joining, on
    // toggling, after a failure, and when a share is stopped from the
    // browser's own control — so the join path needs to know nothing of them.
    let camera: Signal<bool> = signal(false);
    let screen: Signal<bool> = signal(false);
    let muted: Signal<bool> = signal(false);
    let settings_open: Signal<bool> = signal(false);
    let tiles: Signal<Vec<TileKey>> = signal(Vec::new());
    // The tile clicked to fill the main area, by its room key.
    let focus: Signal<Option<String>> = signal(None);
    // Our volume on each remote tile's audio, as slider positions by room
    // key (1.0 = 100%), and the tile whose volume menu is open.
    let volumes: Signal<std::collections::BTreeMap<String, f32>> = signal(Default::default());
    let volume_menu: Signal<Option<String>> = signal(None);
    // Tiles we have muted for ourselves; their volumes are kept.
    let silenced: Signal<std::collections::BTreeSet<String>> = signal(Default::default());
    // Tiles that have faded in; see `room::Controls::entered`.
    let entered: Signal<std::collections::BTreeSet<String>> = signal(Default::default());
    // Ids heard speaking in the last moment, `room::ME` for us.
    let speaking: Signal<Vec<u32>> = signal(Vec::new());
    let video_counters: Signal<String> = signal(String::new());
    // The bar's connection indicator, refreshed once a second; `conn_note`
    // overrides its word when the relay has said something final (removed,
    // not authorised, gone).
    let health: Signal<connection::Health> =
        signal(connection::Health { label: "Not connected".to_string(), ..Default::default() });
    let conn_note: Signal<Option<(String, connection::Quality)>> = signal(None);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = conn_note;
    #[cfg(target_arch = "wasm32")]
    {
        let active = active.clone();
        let pending = Rc::new(std::cell::Cell::new(false));
        // The previous second's counters, for rates and loss: when, link
        // traffic, audio frames received and concealed.
        let window: Rc<std::cell::RefCell<Option<(f64, call::Traffic, u64, u64)>>> = Rc::default();
        let heard: Rc<std::cell::RefCell<std::collections::HashMap<u32, f64>>> = Rc::default();
        let raf = runtime_core::raf_loop(move || {
            let Some((v, people, stats, resync, traffic, roster, encrypted, rate)) = active.borrow().as_ref().map(|c| {
                let (v, people, stats) = (c.video.clone(), c.people.clone(), c.stats.clone());
                let (resync, rate) = (c.screen_audio_out_of_sync(), c.rate_status());
                (v, people, stats, resync, c.traffic.clone(), c.roster.clone(), c.encrypted(), rate)
            }) else {
                if !tiles.get().is_empty() {
                    tiles.set(Vec::new());
                }
                *window.borrow_mut() = None;
                let idle = if joined.get() {
                    connection::Health { label: "Connecting…".to_string(), quality: connection::Quality::Fair, ..Default::default() }
                } else {
                    connection::Health { label: "Not connected".to_string(), ..Default::default() }
                };
                if idle != health.get() {
                    health.set(idle);
                }
                return;
            };
            if resync {
                let active = active.clone();
                runtime_core::driver::spawn_async(async move { call::sync_screen_audio(&active).await });
            }
            if v.camera_failed() {
                camera.set(false);
                screen.set(false);
                status.set("The video encoder stopped — see the console".to_string());
            }
            if v.screen_ended() {
                screen.set(false);
            }
            // One change at a time, so a camera and a share asked for at once
            // do not open two prompts over each other.
            if !pending.get() {
                let want_camera = camera.get() && !v.camera_failed();
                let want_screen = screen.get() && !v.screen_ended();
                let change = if want_camera != v.camera_on() {
                    Some((true, want_camera))
                } else if want_screen != v.screen_on() {
                    Some((false, want_screen))
                } else {
                    None
                };
                if let Some((is_camera, on)) = change {
                    pending.set(true);
                    let (pending, v) = (pending.clone(), v.clone());
                    runtime_core::driver::spawn_async(async move {
                        let done = if is_camera { v.set_camera(on).await } else { v.set_screen(on).await };
                        if let Err(e) = done {
                            if is_camera { camera.set(false) } else { screen.set(false) }
                            status.set(e);
                        }
                        pending.set(false);
                    });
                }
            }

            // Who is here: everyone we hear, anyone we only see, and anyone
            // whose media we cannot open — they are here, just not with us.
            let remotes = v.remotes();
            let locked_out: std::collections::BTreeSet<u32> = roster.borrow().unreadable.iter().copied().collect();
            if locked_out != unreadable.get() {
                unreadable.set(locked_out.clone());
            }
            let mut ids: Vec<u32> = people.borrow().keys().copied().collect();
            ids.extend(remotes.iter().map(|&(id, _)| id));
            ids.extend(locked_out.iter().copied());
            ids.sort_unstable();
            ids.dedup();
            let mut keys: Vec<TileKey> = remotes
                .iter()
                .filter(|&&(_, t)| t == call_proto::TRACK_SCREEN)
                .map(|&(id, _)| TileKey::Screen(id))
                .collect();
            if v.screen_on() {
                keys.push(TileKey::MyScreen);
            }
            keys.push(TileKey::Me { camera: v.camera_on() });
            keys.extend(ids.iter().map(|&id| TileKey::Person {
                id,
                camera: remotes.contains(&(id, call_proto::TRACK_MAIN)),
            }));
            if keys != tiles.get() {
                tiles.set(keys);
            }

            // Speaking: a level above the floor within the last 300 ms, so the
            // ring does not flicker between syllables.
            let now = js_sys::Date::now();
            {
                let mut heard = heard.borrow_mut();
                for (&id, &peak) in people.borrow().iter() {
                    if peak > 0.03 {
                        heard.insert(id, now);
                    }
                }
                if !muted.get() && stats.borrow().out_peak > 0.03 {
                    heard.insert(room::ME, now);
                }
                heard.retain(|_, &mut t| now - t < 300.0);
                let mut now_speaking: Vec<u32> = heard.keys().copied().collect();
                now_speaking.sort_unstable();
                if now_speaking != speaking.get() {
                    speaking.set(now_speaking);
                }
            }

            let line = call_video::summary(v.stats());
            if line != video_counters.get() {
                video_counters.set(line);
            }

            // The connection, once a second.
            let now = call_audio::now_ms();
            let t = *traffic.borrow();
            let a = *stats.borrow();
            let mut w = window.borrow_mut();
            let Some((then, before, received, concealed)) = *w else {
                *w = Some((now, t, a.received, a.concealed));
                return;
            };
            if now - then < 1000.0 {
                return;
            }
            *w = Some((now, t, a.received, a.concealed));
            drop(w);
            let secs = (now - then) / 1000.0;
            let (got, lost) = (a.received.saturating_sub(received), a.concealed.saturating_sub(concealed));
            let loss = if got + lost > 0 { lost as f64 / (got + lost) as f64 } else { 0.0 };
            let up = |x: u64, y: u64| connection::rate(connection::kbps(x.saturating_sub(y), secs));
            let ms = |x: f64| format!("{x:.0} ms");
            let (label, quality) = match conn_note.get() {
                Some(note) => note,
                None if !t.open => ("Disconnected".to_string(), connection::Quality::Off),
                None if t.rtt_ms.is_some() && now - t.last_pong_ms > 6000.0 => {
                    ("Relay not responding".to_string(), connection::Quality::Poor)
                }
                None => {
                    let q = connection::grade(t.rtt_ms, loss);
                    let word = match q {
                        connection::Quality::Good if t.held_back > 0 => "Limited download",
                        connection::Quality::Good => "Connected",
                        connection::Quality::Fair => "Fair connection",
                        _ => "Poor connection",
                    };
                    let q = if t.held_back > 0 && q == connection::Quality::Good { connection::Quality::Fair } else { q };
                    (word.to_string(), q)
                }
            };
            let mut rows = vec![(
                "Round trip to the relay".to_string(),
                t.rtt_ms.map(ms).unwrap_or_else(|| "measuring…".to_string()),
            )];
            // Mouth to ear: their side before sending (ours stands in for it),
            // both legs of the trip (our round trip stands in for theirs and
            // ours), and our playout. An estimate, and labelled as one.
            match t.rtt_ms.filter(|_| a.listen_delay_ms > 0.0) {
                Some(rtt) => rows.push((
                    "Estimated latency".to_string(),
                    format!("≈ {} mouth to ear", ms(a.send_delay_ms + rtt + a.listen_delay_ms)),
                )),
                None => rows.push(("Estimated latency".to_string(), "— nobody to hear yet".to_string())),
            }
            rows.push(("   your sound, before sending".to_string(), ms(a.send_delay_ms)));
            rows.push(("   playout, before you hear".to_string(), ms(a.listen_delay_ms)));
            rows.push(("Audio".to_string(), format!("↑ {}   ↓ {}", up(t.audio_up, before.audio_up), up(t.audio_down, before.audio_down))));
            rows.push(("Video".to_string(), format!("↑ {}   ↓ {}", up(t.video_up, before.video_up), up(t.video_down, before.video_down))));
            rows.push(("Audio lost".to_string(), format!("{:.1}%", loss * 100.0)));
            let (sending, why) = connection::describe_rate(
                rate.audio.bps,
                rate.audio.fullband,
                rate.video_bps,
                rate.plan.map(|p| p.why),
            );
            let line = format!("Sending {sending} — {why}");
            if line != rate_line.get() {
                rate_line.set(line);
            }
            rows.push(("Sending".to_string(), sending));
            if let Some(held) = connection::describe_held_back(t.held_back) {
                rows.push(("Download".to_string(), format!("limited by the relay: {held}")));
            }
            rows.push(("Bitrate".to_string(), why));
            if let Some(l) = rate.loss {
                rows.push(("   others lose of you".to_string(), format!("{:.1}%", l * 100.0)));
            }
            rows.push((
                "Encryption".to_string(),
                if encrypted { "End to end".to_string() } else { "To the relay only".to_string() },
            ));
            if !unreadable.get().is_empty() {
                let n = unreadable.get().len();
                rows.push(("   cannot decrypt".to_string(), format!("{n} {}", if n == 1 { "person" } else { "people" })));
            }
            if !me.get().is_empty() {
                rows.push(("You are".to_string(), me.get()));
            }
            let next = connection::Health { label, quality, rows, encrypted };
            if next != health.get() {
                health.set(next);
            }
        });
        std::mem::forget(raf);
    }

    // Reopen the mic of a live call with new processing. Takes the new values
    // rather than re-reading the signals, because `Signal::set` stages: read
    // straight after, it still answers the value being replaced.
    let reprocess: Rc<dyn Fn(bool, String, bool)> = {
        #[cfg(target_arch = "wasm32")]
        let active = active.clone();
        Rc::new(move |echo: bool, noise: String, agc: bool| {
            #[cfg(not(target_arch = "wasm32"))]
            let _ = (echo, noise, agc);
            #[cfg(target_arch = "wasm32")]
            {
                if !joined.get() {
                    return;
                }
                let active = active.clone();
                let processing = call::Processing {
                    echo_cancellation: echo,
                    suppression: call::Suppression::parse(&noise),
                    auto_gain_control: agc,
                };
                if processing.suppression == call::Suppression::Neural {
                    status.set("Starting DeepFilterNet…".to_string());
                }
                runtime_core::driver::spawn_async(async move {
                    match call::reprocess(&active, processing).await {
                        Ok(()) => status.set(format!(
                            "Live — echo cancellation {}, browser auto gain {}, noise suppression {}",
                            if echo { "on" } else { "off" },
                            if agc { "on" } else { "off" },
                            processing.suppression.id(),
                        )),
                        Err(e) => status.set(e),
                    }
                });
            }
        })
    };
    let on_echo = {
        let reprocess = reprocess.clone();
        cb1(move |on: bool| {
            echo_cancellation.set(on);
            reprocess(on, suppression.get(), auto_gain.get());
        })
    };
    let on_agc = {
        let reprocess = reprocess.clone();
        cb1(move |on: bool| {
            auto_gain.set(on);
            reprocess(echo_cancellation.get(), suppression.get(), on);
        })
    };
    let on_noise = cb1(move |id: String| {
        if id == suppression.get() {
            return;
        }
        suppression.set(id.clone());
        reprocess(echo_cancellation.get(), id, auto_gain.get());
    });

    // DeepFilterNet's tuning. Each control passes its NEW value through, for
    // the same staging reason as above, and a live call is retuned — which
    // rebuilds the model, coalesced so a dragged slider builds once at a time.
    // Fields, by index, for the handlers below: 0 max reduction, 1 speech
    // gate, 2 clean-speech threshold, 3 deep-filter threshold, 4 post-filter
    // (0.0 or 1.0). One handler shape for all five, and nothing here names a
    // wasm-only type.
    let tuning = move || {
        [
            max_reduction.get(),
            min_thresh.get(),
            erb_thresh.get(),
            df_thresh.get(),
            if post_filter.get() { 1.0 } else { 0.0 },
        ]
    };
    #[cfg(target_arch = "wasm32")]
    let model_of = |t: [f32; 5]| call::ModelSettings {
        max_reduction_db: t[0],
        min_db_thresh: t[1],
        max_db_erb_thresh: t[2],
        max_db_df_thresh: t[3],
        post_filter: t[4] > 0.5,
    };
    let retune: Rc<dyn Fn(usize, f32)> = {
        #[cfg(target_arch = "wasm32")]
        let active = active.clone();
        Rc::new(move |which: usize, value: f32| {
            let mut t = tuning();
            t[which] = value;
            #[cfg(target_arch = "wasm32")]
            {
                if !joined.get() {
                    return;
                }
                let active = active.clone();
                let model = model_of(t);
                runtime_core::driver::spawn_async(async move {
                    if let Err(e) = call::retune(&active, model).await {
                        status.set(e);
                    }
                });
            }
            #[cfg(not(target_arch = "wasm32"))]
            let _ = t;
        })
    };
    let slider = |signal: Signal<f32>, which: usize| -> Rc<dyn Fn(f32)> {
        let retune = retune.clone();
        cb1(move |v: f32| {
            signal.set(v);
            retune(which, v);
        })
    };
    let on_reduction = slider(max_reduction, 0);
    let on_min = slider(min_thresh, 1);
    let on_erb = slider(erb_thresh, 2);
    let on_df = slider(df_thresh, 3);
    let on_post = {
        let retune = retune.clone();
        cb1(move |on: bool| {
            post_filter.set(on);
            retune(4, if on { 1.0 } else { 0.0 });
        })
    };

    // One handler per stage switch. Each passes its NEW value through rather
    // than re-reading its own signal, because `Signal::set` stages: read
    // straight after, it still answers the value being replaced.
    let on_stage = |which: usize| -> Rc<dyn Fn(bool)> {
        #[cfg(target_arch = "wasm32")]
        let active = active.clone();
        cb1(move |on: bool| {
            let mut v = [dehum.get(), declick.get(), level.get(), expand.get()];
            v[which] = on;
            [dehum, declick, level, expand][which].set(on);
            #[cfg(target_arch = "wasm32")]
            call::set_voice_processing(
                &active,
                call::VoiceStages { dehum: v[0], declick: v[1], level: v[2], expand: v[3] },
            );
            #[cfg(not(target_arch = "wasm32"))]
            let _ = v;
        })
    };
    let (on_dehum, on_declick, on_level, on_expand) = (on_stage(0), on_stage(1), on_stage(2), on_stage(3));
    // Bitrate control's mode, from the values given — `Signal::set` stages,
    // so they are passed rather than re-read.
    let apply_rate: Rc<dyn Fn(bool, f32, f32)> = {
        #[cfg(target_arch = "wasm32")]
        let active = active.clone();
        Rc::new(move |auto: bool, audio: f32, video: f32| {
            let mode = rate_mode(auto, audio, video);
            #[cfg(target_arch = "wasm32")]
            if let Some(call) = active.borrow().as_ref() {
                call.set_rate_mode(mode);
            }
            #[cfg(not(target_arch = "wasm32"))]
            let _ = mode;
        })
    };
    let on_rate_mode: Rc<dyn Fn(String)> = {
        let apply_rate = apply_rate.clone();
        cb1(move |id: String| {
            let auto = id == "auto";
            rate_auto.set(auto);
            apply_rate(auto, manual_audio.get(), manual_video.get());
        })
    };
    let on_audio_rate: Rc<dyn Fn(f32)> = {
        let apply_rate = apply_rate.clone();
        cb1(move |v: f32| {
            manual_audio.set(v);
            rate_auto.set(false);
            apply_rate(false, v, manual_video.get());
        })
    };
    let on_video_rate: Rc<dyn Fn(f32)> = {
        let apply_rate = apply_rate.clone();
        cb1(move |v: f32| {
            manual_video.set(v);
            rate_auto.set(false);
            apply_rate(false, manual_audio.get(), v);
        })
    };

    let on_mute: Rc<dyn Fn()> = {
        #[cfg(target_arch = "wasm32")]
        let active = active.clone();
        Rc::new(move || {
            let on = !muted.get();
            muted.set(on);
            #[cfg(target_arch = "wasm32")]
            call::set_muted(&active, on);
        })
    };

    let on_join: Rc<dyn Fn()> = {
        #[cfg(target_arch = "wasm32")]
        let active = active.clone();
        Rc::new(move || {
            #[cfg(not(target_arch = "wasm32"))]
            {
                status.set("This app runs on the web target only".to_string());
            }
            #[cfg(target_arch = "wasm32")]
            {
                if joined.get() {
                    *active.borrow_mut() = None;
                    joined.set(false);
                    screen.set(false);
                    focus.set(None);
                    // Ids are the relay's, per call: a volume set on #3 means
                    // nothing next time.
                    volumes.set(Default::default());
                    silenced.set(Default::default());
                    volume_menu.set(None);
                    names.set(Default::default());
                    mic_off.set(Default::default());
                    unreadable.set(Default::default());
                    me.set(String::new());
                    mic.set(meter_bar("mic ", 0.0));
                    sent_level.set(meter_bar("sent", 0.0));
                    status.set("Left".to_string());
                    return;
                }
                joined.set(true);
                conn_note.set(None);
                status.set("Connecting…".to_string());
                let access = call::Access {
                    room: room_id.get().trim().to_string(),
                    grant: Some(grant.get().trim().to_string()).filter(|g| !g.is_empty()),
                    renew: renew_url.clone().map(call::Renew::Url),
                    key: call_e2ee::RoomKey::from_text(&e2ee_key.get()),
                };
                // The address bar becomes the link to this call, key and all.
                call::show_key_in_address(&e2ee_key.get());
                let (u, h, aec, voice) = (
                    url.get(),
                    hash.get(),
                    call::Processing {
                        echo_cancellation: echo_cancellation.get(),
                        suppression: call::Suppression::parse(&suppression.get()),
                        auto_gain_control: auto_gain.get(),
                    },
                    call::VoiceStages {
                        dehum: dehum.get(),
                        declick: declick.get(),
                        level: level.get(),
                        expand: expand.get(),
                    },
                );
                let active = active.clone();
                runtime_core::driver::spawn_async(async move {
                    let my_id = Rc::new(std::cell::Cell::new(0u32));
                    // Once removed, the link closing after is not the news.
                    let removed = Rc::new(std::cell::Cell::new(false));
                    let on_control = move |text: String| {
                        use call_proto::relay::FromRelay;
                        call::log(&format!("[quic-call] control: {text}"));
                        match FromRelay::parse(&text) {
                            FromRelay::You(id) => {
                                my_id.set(id);
                                me.set(format!("#{id}"));
                            }
                            FromRelay::Named { id, user } => {
                                if id == my_id.get() {
                                    me.set(user.clone());
                                }
                                let mut all = names.get();
                                all.insert(id, user);
                                names.set(all);
                            }
                            FromRelay::Said { from, text } if call_proto::relay::Announce::parse(&text).is_some() => {
                                if let Some(call_proto::relay::Announce::Mic(on)) = call_proto::relay::Announce::parse(&text) {
                                    let mut off = mic_off.get();
                                    if on { off.remove(&from) } else { off.insert(from) };
                                    mic_off.set(off);
                                }
                            }
                            FromRelay::Joined(_) => {}
                            FromRelay::Left(id) => {
                                let mut off = mic_off.get();
                                off.remove(&id);
                                mic_off.set(off);
                                let mut all = names.get();
                                let who = all.remove(&id).unwrap_or_else(|| format!("#{id}"));
                                names.set(all);
                                status.set(format!("{who} left"));
                            }
                            FromRelay::Expires { secs: 0 } => {
                                conn_note.set(Some(("Not authorised".to_string(), connection::Quality::Poor)));
                                status.set("Your grant ran out — you cannot be heard until it is renewed".to_string())
                            }
                            // A renewal landed: authorised again.
                            FromRelay::Expires { .. } => {
                                if conn_note.get().is_some_and(|(l, _)| l == "Not authorised") {
                                    conn_note.set(None);
                                }
                            }
                            FromRelay::Revoked => {
                                removed.set(true);
                                conn_note.set(Some(("Removed from the call".to_string(), connection::Quality::Off)));
                                status.set("You were removed from this call".to_string())
                            }
                            FromRelay::Other(_) if removed.get() => {}
                            FromRelay::MaySend(code) => status.set(match code.as_str() {
                                "-" => "You can listen, but no longer send".to_string(),
                                code => format!("You may now send only: {}", describe_send(code)),
                            }),
                            FromRelay::Refused(why) => status.set(format!("Grant not renewed: {why}")),
                            // The connection card says so; not news for the status line.
                            FromRelay::Throttled(_) => {}
                            _ => status.set(text),
                        }
                    };
                    let rate = rate_mode(rate_auto.get(), manual_audio.get(), manual_video.get());
                    let model = model_of(tuning());
                    match call::join(&u, &h, access, aec, voice, model, on_control).await {
                        Ok(c) => {
                            *active.borrow_mut() = Some(c);
                            if let Some(call) = active.borrow().as_ref() {
                                call.set_rate_mode(rate);
                            }
                            call::set_muted(&active, muted.get());
                            status.set("Live — talk".to_string());
                        }
                        Err(e) => {
                            joined.set(false);
                            status.set(e);
                        }
                    }
                });
            }
        })
    };

    // Everything that tunes the call, in one modal: the same controls the
    // page used to show inline, plus the diagnostics.
    let settings_body = {
        let (on_echo, on_agc, on_noise) = (on_echo.clone(), on_agc.clone(), on_noise.clone());
        let (on_rate_mode, on_audio_rate, on_video_rate) = (on_rate_mode.clone(), on_audio_rate.clone(), on_video_rate.clone());
        let (on_dehum, on_declick, on_level, on_expand) =
            (on_dehum.clone(), on_declick.clone(), on_level.clone(), on_expand.clone());
        let (on_reduction, on_min, on_erb, on_df, on_post) =
            (on_reduction.clone(), on_min.clone(), on_erb.clone(), on_df.clone(), on_post.clone());
        move || -> Element {
            let (on_echo, on_agc, on_noise) = (on_echo.clone(), on_agc.clone(), on_noise.clone());
            let (on_rate_mode, on_audio_rate, on_video_rate) =
                (on_rate_mode.clone(), on_audio_rate.clone(), on_video_rate.clone());
            let (on_dehum, on_declick, on_level, on_expand) =
                (on_dehum.clone(), on_declick.clone(), on_level.clone(), on_expand.clone());
            let (on_reduction, on_min, on_erb, on_df, on_post) =
                (on_reduction.clone(), on_min.clone(), on_erb.clone(), on_df.clone(), on_post.clone());
            ui! {
                Stack(gap = StackGap::Md, padding = StackPadding::Lg) {
                    Typography(content = "Settings".to_string(), kind = typography_kind::H2)
                    scroll_view(style = || StyleApplication::new(SettingsScroll::sheet())) {
                        Stack(gap = StackGap::Md) {
                            Typography(content = "Microphone".to_string(), kind = typography_kind::H3)
                            Switch(
                                label = Some("Echo cancellation (browser)".to_string()),
                                value = echo_cancellation,
                                on_change = on_echo,
                            )
                            Switch(
                                label = Some("Auto gain (browser — stacks with Level)".to_string()),
                                value = auto_gain,
                                on_change = on_agc,
                            )
                            Typography(content = "Noise suppression".to_string(), muted = true)
                            SegmentedControl(
                                value = rx!(suppression.get()),
                                on_change = on_noise,
                                options = vec![
                                    SegmentOption::new("off", "Off".to_string()),
                                    SegmentOption::new("browser", "Browser".to_string()),
                                    SegmentOption::new("neural", "DeepFilterNet".to_string()),
                                ],
                            )
                            Typography(content = "DeepFilterNet tuning".to_string(), muted = true)
                            Typography(content = rx!(if max_reduction.get() >= 100.0 {
                                "Max noise reduction: unlimited".to_string()
                            } else {
                                format!("Max noise reduction: {:.0} dB (keeps a little room underneath)", max_reduction.get())
                            }))
                            Slider(value = rx!(max_reduction.get()), min = 6.0, max = 100.0, step = 1.0, on_change = on_reduction)
                            Typography(content = rx!(format!(
                                "Speech gate: frames under {:.0} dB local SNR are zeroed (lower lets quiet speech through)",
                                min_thresh.get(),
                            )))
                            Slider(value = rx!(min_thresh.get()), min = -30.0, max = 0.0, step = 1.0, on_change = on_min)
                            Typography(content = rx!(format!(
                                "Clean-speech threshold: over {:.0} dB is left untouched",
                                erb_thresh.get(),
                            )))
                            Slider(value = rx!(erb_thresh.get()), min = 10.0, max = 40.0, step = 1.0, on_change = on_erb)
                            Typography(content = rx!(format!(
                                "Deep-filter threshold: over {:.0} dB skips the second stage",
                                df_thresh.get(),
                            )))
                            Slider(value = rx!(df_thresh.get()), min = 5.0, max = 35.0, step = 1.0, on_change = on_df)
                            Switch(
                                label = Some("Post-filter (crisper, a little more attenuation)".to_string()),
                                value = post_filter,
                                on_change = on_post,
                            )
                            Typography(content = "Voice processing".to_string(), kind = typography_kind::H3)
                            Switch(
                                label = Some("De-hum (rumble, and mains hum where detected)".to_string()),
                                value = dehum,
                                on_change = on_dehum,
                            )
                            Switch(
                                label = Some("Declick (keystrokes — dulls sibilance a little)".to_string()),
                                value = declick,
                                on_change = on_declick,
                            )
                            Switch(
                                label = Some("Level (auto gain + peak limit)".to_string()),
                                value = level,
                                on_change = on_level,
                            )
                            Switch(
                                label = Some("Expand (quieter between words — with a model)".to_string()),
                                value = expand,
                                on_change = on_expand,
                            )
                            Typography(content = "Bitrate".to_string(), kind = typography_kind::H3)
                            Typography(content = rx!(rate_line.get()), muted = true)
                            SegmentedControl(
                                value = rx!(if rate_auto.get() { "auto" } else { "manual" }.to_string()),
                                on_change = on_rate_mode,
                                options = vec![
                                    SegmentOption::new("auto", "Auto — follows the link".to_string()),
                                    SegmentOption::new("manual", "Manual".to_string()),
                                ],
                            )
                            Typography(content = rx!(format!(
                                "Manual audio: {:.0} kb/s ({})",
                                manual_audio.get(),
                                if manual_audio.get() >= 28.0 { "20 kHz" } else { "8 kHz" },
                            )))
                            Slider(value = rx!(manual_audio.get()), min = 12.0, max = 64.0, step = 2.0, on_change = on_audio_rate)
                            Typography(content = rx!(format!(
                                "Manual video: {:.0} kb/s, camera and screen together",
                                manual_video.get(),
                            )))
                            Slider(value = rx!(manual_video.get()), min = 100.0, max = 2000.0, step = 50.0, on_change = on_video_rate)
                            Typography(content = "Diagnostics".to_string(), kind = typography_kind::H3)
                            Typography(content = rx!(status.get()))
                            Typography(content = rx!(mic.get()))
                            Typography(content = rx!(sent_level.get()))
                            Typography(content = rx!(counters.get()), muted = true)
                            Typography(content = rx!(video_counters.get()), muted = true)
                        }
                    }
                    Stack(axis = StackAxis::Row, justify = StackJustify::End) {
                        Button(label = "Done".to_string(), on_click = cb(move || settings_open.set(false)))
                    }
                }
            }
        }
    };

    let open_settings: Rc<dyn Fn()> = Rc::new(move || settings_open.set(true));

    // Before joining: where to, and Join — a card that rises into place.
    let join_view = {
        let (on_join, open_settings) = (on_join.clone(), open_settings.clone());
        move || -> Element {
            let (on_join, open_settings) = (on_join.clone(), open_settings.clone());
            let card = presence(move || {
                let (on_join, open_settings) = (on_join.clone(), open_settings.clone());
                let toggle = theme_toggle(dark);
                // A fresh random key, in place of whatever is typed.
                let new_key = Adornment::Button {
                    icon: DICES,
                    label: Some("New random key".to_string()),
                    on_press: Rc::new(move || {
                        #[cfg(target_arch = "wasm32")]
                        if let Some(key) = call::new_key() {
                            e2ee_key.set(key);
                        }
                    }),
                };
                ui! {
                    view(style = || StyleApplication::new(LobbyCard::sheet())) {
                        Card() {
                        Stack(gap = StackGap::Md) {
                            view(style = || StyleApplication::new(TitleRow::sheet())) {
                                Typography(content = "quic-call".to_string(), kind = typography_kind::H1)
                                toggle
                            }
                            Typography(
                                content = "One room, straight over the datagram crate: voice, cameras and \
                                    screen shares in QUIC datagrams."
                                    .to_string(),
                                muted = true,
                            )
                            Field(
                                label = Some("Relay URL".to_string()),
                                value = url,
                                on_change = cb1(move |v: String| url.set(v)),
                                placeholder = Some("https://your-server:4433/".to_string()),
                            )
                            Field(
                                label = Some("Room".to_string()),
                                value = room_id,
                                on_change = cb1(move |v: String| room_id.set(v)),
                                placeholder = Some("empty for a relay without rooms".to_string()),
                            )
                            Field(
                                label = Some("Grant".to_string()),
                                value = grant,
                                on_change = cb1(move |v: String| grant.set(v)),
                                placeholder = Some("empty for an open relay".to_string()),
                                help = Some("From whoever runs the room: `call-relay mint` prints one.".to_string()),
                            )
                            Field(
                                label = Some("End-to-end key".to_string()),
                                value = e2ee_key,
                                on_change = cb1(move |v: String| e2ee_key.set(v)),
                                placeholder = Some("empty to not encrypt end to end".to_string()),
                                help = Some(
                                    "Everyone needs the same key; the relay never sees it. Joining puts it in this \
                                     page's address — share that link."
                                        .to_string(),
                                ),
                                trailing = new_key,
                            )
                            Field(
                                label = Some("Certificate hash".to_string()),
                                value = hash,
                                on_change = cb1(move |v: String| hash.set(v)),
                                placeholder = Some("empty for a CA certificate".to_string()),
                                help = Some("Only for a self-signed relay, which prints a new one each start.".to_string()),
                            )
                            Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
                                Button(label = "Join".to_string(), on_click = on_join, leading_icon = Some(VIDEO))
                                Button(
                                    label = "Settings".to_string(),
                                    leading_icon = Some(SETTINGS),
                                    variant = variant::Ghost,
                                    on_click = open_settings,
                                )
                            }
                            Typography(content = rx!(status.get()), muted = true, kind = typography_kind::Caption)
                        }
                    }
                    }
                }
            })
            .enter(PresenceAnim::new(PresenceState::default().opacity(0.0).translate_y(16.0), 320, Easing::EaseOut));
            ui! {
                view(style = || StyleApplication::new(Lobby::sheet())) { card }
            }
        }
    };

    // In a call: the room, and the control bar under it.
    let room_view = {
        #[cfg(target_arch = "wasm32")]
        let active = active.clone();
        let (on_join, open_settings, on_mute) = (on_join.clone(), open_settings.clone(), on_mute.clone());
        move || -> Element {
            #[cfg(target_arch = "wasm32")]
            let active = active.clone();
            let (on_leave, open_settings, on_mute) = (on_join.clone(), open_settings.clone(), on_mute.clone());
            // A tile key is "person:N" or "screen:N": that person's voice, or
            // their screen's audio.
            let on_volume: Rc<dyn Fn(String, f32)> = {
                #[cfg(target_arch = "wasm32")]
                let active = active.clone();
                Rc::new(move |key: String, position: f32| {
                    let parsed = key.split_once(':').and_then(|(kind, id)| {
                        let track = match kind {
                            "person" => call_proto::TRACK_MAIN,
                            "screen" => call_proto::TRACK_SCREEN,
                            _ => return None,
                        };
                        Some((id.parse::<u32>().ok()?, track))
                    });
                    #[cfg(target_arch = "wasm32")]
                    if let Some((id, track)) = parsed {
                        call::set_volume(&active, id, track, call_audio::taper(position));
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    let _ = (parsed, position);
                })
            };
            let ctl = room::Controls {
                speaking,
                focus,
                volumes,
                silenced,
                menu: volume_menu,
                on_volume,
                entered,
                last_focus: Rc::default(),
            };
            // Names are in the key: they arrive as people join, which rebuilds
            // the room anyway.
            let room = switch(
                move || (tiles.get(), focus.get(), names.get()),
                move |(keys, _, names): &(Vec<TileKey>, Option<String>, std::collections::BTreeMap<u32, String>)| {
                    let name = |id: u32| names.get(&id).cloned().unwrap_or_else(|| format!("#{id}"));
                    #[cfg(target_arch = "wasm32")]
                    {
                        let slot = active.borrow();
                        let Some(v) = slot.as_ref().map(|c| &c.video) else {
                            return room::room(Vec::new(), &ctl);
                        };
                        let me_label = if me.get().is_empty() { "You".to_string() } else { format!("You ({})", me.get()) };
                        let built = keys
                            .iter()
                            .map(|k| match *k {
                                TileKey::Screen(id) => room::Tile {
                                    key: format!("screen:{id}"),
                                    id,
                                    label: format!("{}'s screen", name(id)),
                                    video: v.remote(id, call_proto::TRACK_SCREEN).map(|s| call_video::player(s, true)),
                                    screen: true,
                                    muted: None,
                                    unreadable: Some(Rc::new(move || unreadable.get().contains(&id))),
                                    volume: true,
                                },
                                TileKey::MyScreen => room::Tile {
                                    key: "screen:me".to_string(),
                                    id: room::ME,
                                    label: "Your screen".to_string(),
                                    video: v.local(call_proto::TRACK_SCREEN).map(|s| call_video::player(s, true)),
                                    screen: true,
                                    muted: None,
                                    unreadable: None,
                                    volume: false,
                                },
                                TileKey::Me { camera } => room::Tile {
                                    key: "me".to_string(),
                                    id: room::ME,
                                    label: me_label.clone(),
                                    video: if camera {
                                        v.local(call_proto::TRACK_MAIN).map(|s| call_video::player(s, false))
                                    } else {
                                        None
                                    },
                                    screen: false,
                                    muted: Some(Rc::new(move || muted.get())),
                                    unreadable: None,
                                    volume: false,
                                },
                                TileKey::Person { id, camera } => room::Tile {
                                    key: format!("person:{id}"),
                                    id,
                                    label: name(id),
                                    video: if camera {
                                        v.remote(id, call_proto::TRACK_MAIN).map(|s| call_video::player(s, false))
                                    } else {
                                        None
                                    },
                                    screen: false,
                                    muted: Some(Rc::new(move || mic_off.get().contains(&id))),
                                    unreadable: Some(Rc::new(move || unreadable.get().contains(&id))),
                                    volume: true,
                                },
                            })
                            .collect();
                        room::room(built, &ctl)
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let _ = (keys, name(0));
                        room::room(Vec::new(), &ctl)
                    }
                },
            );
            // The dock is rebuilt when a toggle flips: four buttons, cheaply,
            // and nothing in the room with them.
            let dock = switch(
                move || (muted.get(), camera.get(), screen.get()),
                move |&(is_muted, cam_on, sharing): &(bool, bool, bool)| {
                    let (on_leave, on_mute) = (on_leave.clone(), on_mute.clone());
                    // Muted is the state worth noticing, so it is red; the camera
                    // starts off, which is normal here, so off is merely quieter.
                    // A share in progress is the accent.
                    let mic = if is_muted {
                        tool("Unmute", MIC_OFF, tone::Danger.into(), variant::Filled.into(), on_mute)
                    } else {
                        tool("Mute", MIC, tone::Neutral.into(), variant::Soft.into(), on_mute)
                    };
                    let cam = if cam_on {
                        tool("Turn camera off", VIDEO, tone::Neutral.into(), variant::Soft.into(), cb(move || camera.set(false)))
                    } else {
                        tool("Turn camera on", VIDEO_OFF, tone::Neutral.into(), variant::Ghost.into(), cb(move || camera.set(true)))
                    };
                    let share = if sharing {
                        tool("Stop sharing", MONITOR_X, tone::Primary.into(), variant::Filled.into(), cb(move || screen.set(false)))
                    } else {
                        tool("Share screen", MONITOR_UP, tone::Neutral.into(), variant::Soft.into(), cb(move || screen.set(true)))
                    };
                    let leave = tool("Leave", PHONE_OFF, tone::Danger.into(), variant::Filled.into(), on_leave);
                    ui! {
                        view(style = || StyleApplication::new(Dock::sheet())) {
                            mic
                            cam
                            share
                            leave
                        }
                    }
                },
            );
            let settings_button =
                tool("Settings", SETTINGS, tone::Neutral.into(), variant::Ghost.into(), open_settings.clone());
            let toggle = theme_toggle(dark);
            let indicator = connection::indicator(health);
            ui! {
                view(style = || StyleApplication::new(Page::sheet())) {
                    room
                    view(style = || StyleApplication::new(Bar::sheet())) {
                        view(style = || StyleApplication::new(BarEnd::sheet())) {
                            indicator
                        }
                        dock
                        view(style = || StyleApplication::new(BarEnd::sheet()).with("side", "end")) {
                            toggle
                            settings_button
                        }
                    }
                }
            }
        }
    };

    let body = switch(move || joined.get(), move |&in_call: &bool| if in_call { room_view() } else { join_view() });
    let settings = settings_body.clone();
    ui! {
        view(style = || StyleApplication::new(Shell::sheet())) {
            body
            Modal(
                open = settings_open,
                width = 640.0,
                on_dismiss = Some(Rc::new(move || settings_open.set(false)) as Rc<dyn Fn()>),
                content = move || settings(),
            )
        }
    }
}
