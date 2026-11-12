//! The web implementation: WebCodecs, the camera crate, and `<video>`.
//!
//! **The codec is the browser's**, through WebCodecs: hardware where there is
//! some, and nothing in the SDK wraps a video encoder yet. Audio stays on
//! `rusty-opus` in the page; there is no pure-Rust video encoder that keeps
//! up in wasm. The calls go through `web_glue::import!`, the framework's own
//! JS boundary, the way the `camera` crate's web backend does.
//!
//! **Frames are cut to fit datagrams** and put back by `call_proto::video`,
//! which also decides what may be decoded. A frame missing a piece is gone,
//! and so is everything after it until a keyframe: the receiver asks for one
//! over the control channel (`key #N`, which the relay forwards to everyone
//! as `#me: key #N`), and the sender named makes one.
//!
//! **Display is the SDK's `Video`**, which on the web plays a stream that
//! carries a browser `MediaStream`. The camera's own is used for the local
//! preview; for the far end, decoded frames are drawn onto a canvas and its
//! `captureStream()` is that stream.
//!
//! The capture timer is a `setInterval`, which a background tab slows to once
//! a second: hide the tab and the others see a slideshow. Audio is unaffected.
//!
//! # Lip sync
//!
//! `av_sync` decides it; this module carries it out. The receive loop reports
//! each audio track's clock ([`Video::set_audio_clock`]); decoded frames queue
//! in the decoder's JS, and a frame loop here shows whichever `av_sync::due`
//! says, waiting at most [`MAX_HOLD_MS`].

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use futures::channel::mpsc;
use idea_ui::IdeaThemeRef;
use media_stream::{FrameWriter, MediaStream};
use runtime_core::{stylesheet, Element, Length, StyleApplication};
use web_glue::{Closure, JsCast, JsValue};

use call_proto::relay::{FromRelay, KeyRequest};
use call_proto::video::{self as frames, Assembler, Fragment};
use call_proto::{TRACK_MAIN, TRACK_SCREEN};

/// What the camera is asked for. It answers with what it has, and the encoder
/// follows whatever size actually arrives.
const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;
const FPS: u32 = 24;
/// VP8 at 640×480, 24 fps: clear faces at 600 kb/s, comfortably under what a
/// home uplink carries next to the voice. Bitrate control may take it down to
/// the floor — blocky, but a face — on a link that cannot carry more.
const BITRATE: u32 = 600_000;
const BITRATE_MIN: u32 = 100_000;
/// A screen share: text wants pixels more than it wants motion, so fewer
/// frames at a higher rate, capped at 1080p (the browser is asked for no
/// more, and the encoder follows what arrives). Its floor is higher than the
/// camera's: below it, text smears.
const SCREEN_FPS: u32 = 15;
const SCREEN_BITRATE: u32 = 1_500_000;
const SCREEN_BITRATE_MIN: u32 = 250_000;
/// The least time between bitrate changes on one track. A change reconfigures
/// the encoder, which costs a keyframe.
const RATE_GAP_MS: f64 = 2_000.0;
/// Datagrams waiting for the video send task: a keyframe or two. Separate
/// from audio's queue, so a keyframe's burst never delays voice.
pub const SEND_QUEUE: usize = 96;
/// The fewest milliseconds between keyframes made on request — everyone who
/// lost the same frame asks at once.
const KEY_GAP_MS: f64 = 300.0;
/// How often a stuck receiver repeats its request, in case the request or the
/// keyframe was lost too.
const ASK_GAP_MS: f64 = 500.0;
/// The longest a decoded frame waits for the audio to catch up.
pub const MAX_HOLD_MS: f64 = 600.0;
/// A remote track that has sent nothing for this long has stopped — a camera
/// turned off, a share ended — and its tile goes. Senders produce frames at
/// a steady rate however still the picture, so silence this long is not a
/// static scene.
const STALE_MS: f64 = 2000.0;

web_glue::import! {
    // The encoder: a detached <video> on the camera's track, sampled by a
    // timer into a realtime VP8 encoder. Each encoded chunk is queued and
    // `wake` is called; Rust drains the queue with `js_enc_len`/`js_enc_take`.
    // A keyframe is made when asked (`key`) and every ten seconds regardless.
    //
    // In three temporal layers (L1T3) where the browser can, so a throttling
    // relay can hold layers back from a weak download: the encoder starts
    // without, and is reconfigured with them once `isConfigSupported` says
    // yes. Each chunk's layer comes from its metadata.
    #[catch]
    fn js_enc_open(s: u32, wake: u32, bitrate: u32, fps: u32) -> u32 =
        "(s, w, bitrate, fps) => { \
           if (typeof VideoEncoder === 'undefined') throw new Error('this browser has no WebCodecs VideoEncoder'); \
           const track = G.get(s).getVideoTracks()[0]; \
           if (!track) throw new Error('the camera stream has no video track'); \
           const wake = G.get(w); \
           const v = document.createElement('video'); v.muted = true; v.setAttribute('playsinline', ''); \
           v.srcObject = new MediaStream([track]); const p = v.play(); if (p) p.catch(() => {}); \
           const st = { q: [], key: true, done: false, failed: false, ended: false, w: 0, h: 0, lastKey: 0, v, cap: null, bitrate, svc: false }; \
           VideoEncoder.isConfigSupported({ codec: 'vp8', width: 640, height: 480, bitrate, framerate: fps, scalabilityMode: 'L1T3' }) \
             .then((r) => { if (r.supported && !st.done) { st.svc = true; st.w = 0; } }, () => {}); \
           track.addEventListener('ended', () => { st.ended = true; }); \
           if (v.requestVideoFrameCallback) { \
             const seen = (_, meta) => { if (st.done) return; \
               st.cap = meta.captureTime !== undefined ? meta.captureTime : null; \
               v.requestVideoFrameCallback(seen); }; \
             v.requestVideoFrameCallback(seen); } \
           st.enc = new VideoEncoder({ \
             output: (c, meta) => { if (st.done) return; const b = new Uint8Array(c.byteLength); c.copyTo(b); \
               const layer = meta && meta.svc ? meta.svc.temporalLayerId : 0; \
               st.q.push([b, c.type === 'key', c.timestamp / 1000, layer]); wake(); }, \
             error: (e) => { console.error('[call-video] video encoder', e); st.failed = true; }, \
           }); \
           st.timer = setInterval(() => { \
             const vw = v.videoWidth, vh = v.videoHeight; \
             if (st.done || !vw || !vh || st.enc.state === 'closed') return; \
             if (vw !== st.w || vh !== st.h) { \
               const cfg = { codec: 'vp8', width: vw, height: vh, bitrate: st.bitrate, framerate: fps, latencyMode: 'realtime' }; \
               if (st.svc) cfg.scalabilityMode = 'L1T3'; \
               st.enc.configure(cfg); \
               st.w = vw; st.h = vh; st.key = true; } \
             if (st.enc.encodeQueueSize > 2) return; \
             const now = performance.now(); \
             if (now - st.lastKey > 10000) st.key = true; \
             if (st.key) st.lastKey = now; \
             const f = new VideoFrame(v, { timestamp: Math.round((st.cap !== null ? st.cap : now) * 1000) }); \
             st.enc.encode(f, { keyFrame: st.key }); st.key = false; f.close(); \
           }, 1000 / fps); \
           return G.add(st); }";
    fn js_enc_len(s: u32) -> u32 =
        "(s) => { const q = G.get(s).q; return q.length ? q[0][0].length : 0; }";
    // Copy the oldest chunk to `out` (which has room for `js_enc_len` bytes).
    // 1 for a keyframe.
    fn js_enc_take(s: u32, out: usize) -> u32 =
        "(s, o) => { const [b, k] = G.get(s).q.shift(); G.u8().set(b, o >>> 0); return k ? 1 : 0; }";
    fn js_enc_key(s: u32) = "(s) => { G.get(s).key = true; }";
    // A new bitrate: the timer reconfigures at the next frame, as for a new
    // size, and that frame is a keyframe.
    fn js_enc_rate(s: u32, b: u32) = "(s, b) => { const t = G.get(s); if (t.bitrate !== b) { t.bitrate = b; t.w = 0; } }";
    // The oldest chunk's capture time, ms on performance.now() — the
    // VideoFrame's timestamp: the camera's own capture time for the frame on
    // show (`requestVideoFrameCallback`'s `captureTime`, which Chromium gives
    // for camera tracks), or failing that the moment the timer grabbed it.
    fn js_enc_capture(s: u32) -> f64 = "(s) => G.get(s).q[0][2]";
    // The oldest chunk's temporal layer: 0 without layers.
    fn js_enc_layer(s: u32) -> u32 = "(s) => G.get(s).q[0][3] >>> 0";
    fn js_enc_failed(s: u32) -> u32 = "(s) => G.get(s).failed ? 1 : 0";
    // Whether the source's track ended on its own — a screen share stopped
    // from the browser's own "Stop sharing" control.
    fn js_enc_ended(s: u32) -> u32 = "(s) => G.get(s).ended ? 1 : 0";

    // A screen share: getDisplayMedia, asking for the screen's (or tab's)
    // audio too, which the browser may or may not give. `done` is called once,
    // with the stream or with null (refused, or no browser support).
    fn js_screen_open(done: u32, fps: u32) =
        "(d, fps) => { const done = G.get(d); \
           if (!navigator.mediaDevices || !navigator.mediaDevices.getDisplayMedia) { done(null); return; } \
           navigator.mediaDevices.getDisplayMedia({ \
               video: { width: { max: 1920 }, height: { max: 1080 }, frameRate: { max: fps } }, audio: true }) \
             .then((s) => done(s), (e) => { console.warn('[call-video] screen share', e); done(null); }); }";
    fn js_stream_has_audio(s: u32) -> u32 = "(s) => G.get(s).getAudioTracks().length ? 1 : 0";
    fn js_stream_stop(s: u32) = "(s) => G.get(s).getTracks().forEach((t) => t.stop())";
    // What is being encoded, as width << 16 | height.
    fn js_enc_size(s: u32) -> u32 = "(s) => { const t = G.get(s); return (((t.w & 0xffff) << 16) | (t.h & 0xffff)) >>> 0; }";
    fn js_enc_close(s: u32) =
        "(s) => { const t = G.get(s); t.done = true; clearInterval(t.timer); \
           try { if (t.enc.state !== 'closed') t.enc.close(); } catch (_) {} \
           t.v.pause(); t.v.srcObject = null; }";

    // The decoder, drawing onto a canvas whose captureStream is what plays.
    // A decoder that errors closes itself; the next decode rebuilds it and
    // answers 0, which tells Rust a keyframe is needed.
    //
    // Decoded frames are not drawn on arrival: they queue, each with when it
    // came out of the decoder, and `js_dec_show` draws the one the frame loop
    // in Rust says is due (see the module docs on lip sync). A long queue —
    // nothing being shown — sheds its oldest.
    #[catch]
    fn js_dec_open() -> u32 =
        "() => { \
           if (typeof VideoDecoder === 'undefined') throw new Error('this browser has no WebCodecs VideoDecoder'); \
           const c = document.createElement('canvas'); c.width = 640; c.height = 480; \
           const x = c.getContext('2d'); \
           const st = { c, x, q: [], done: false }; \
           st.make = () => { \
             st.dec = new VideoDecoder({ \
               output: (f) => { \
                 if (st.done) { f.close(); return; } \
                 st.q.push([f, performance.now()]); \
                 while (st.q.length > 30) st.q.shift()[0].close(); }, \
               error: (e) => console.error('[call-video] video decoder', e), \
             }); \
             st.dec.configure({ codec: 'vp8', optimizeForLatency: true }); }; \
           st.make(); st.stream = c.captureStream(); \
           return G.add(st); }";
    fn js_dec_queued(s: u32) -> u32 = "(s) => G.get(s).q.length >>> 0";
    // Queued frame `i`: its capture time (ms, from the chunk's timestamp in µs)
    // and when it was decoded.
    fn js_dec_capture(s: u32, i: u32) -> f64 = "(s, i) => G.get(s).q[i][0].timestamp / 1000";
    fn js_dec_decoded(s: u32, i: u32) -> f64 = "(s, i) => G.get(s).q[i][1]";
    // Take `n` frames off the front and draw the last of them.
    fn js_dec_show(s: u32, n: u32) =
        "(s, n) => { const st = G.get(s); const taken = st.q.splice(0, n); const f = taken.pop()[0]; \
           taken.forEach(([g]) => g.close()); \
           if (st.c.width !== f.displayWidth) st.c.width = f.displayWidth; \
           if (st.c.height !== f.displayHeight) st.c.height = f.displayHeight; \
           st.x.drawImage(f, 0, 0); f.close(); }";
    fn js_dec_stream(s: u32) -> u32 = "(s) => G.add(G.get(s).stream)";
    fn js_dec_decode(s: u32, p: usize, l: usize, key: u32, ts: f64) -> u32 =
        "(s, p, l, k, ts) => { const t = G.get(s); \
           if (t.dec.state === 'closed') { t.make(); return 0; } \
           try { t.dec.decode(new EncodedVideoChunk({ type: k ? 'key' : 'delta', timestamp: ts, \
                   data: G.u8().slice(p >>> 0, (p >>> 0) + l) })); return 1; } \
           catch (e) { console.error('[call-video] video decode', e); \
             try { t.dec.close(); } catch (_) {} t.make(); return 0; } }";
    fn js_dec_close(s: u32) =
        "(s) => { const t = G.get(s); t.done = true; t.q.forEach(([f]) => f.close()); t.q = []; \
           try { if (t.dec.state !== 'closed') t.dec.close(); } catch (_) {} \
           t.stream.getTracks().forEach((k) => k.stop()); }";
}

/// Counters for the page. Cumulative unless noted.
#[derive(Debug, Default, Clone, Copy)]
pub struct VideoStats {
    pub frames_sent: u64,
    pub keys_sent: u64,
    /// Frames not sent whole: the send queue was full, or one was too big.
    pub frames_dropped: u64,
    pub datagrams_sent: u64,
    /// Across everyone we can see.
    pub frames_shown: u64,
    pub keys_asked: u64,
    /// How long decoded frames have been held for lip sync, on average across
    /// everyone we can see, in ms.
    pub sync_hold_ms: f64,
    /// What our screen share is being encoded at, right now. 0×0 when off.
    pub screen_width: u32,
    pub screen_height: u32,
    /// What our camera is being encoded at, right now. 0×0 when off.
    pub width: u32,
    pub height: u32,
}

/// One of our video tracks — the camera or a screen share — on its way out.
struct Sender {
    /// The encoder's state, a web-glue handle.
    enc: JsValue,
    /// What the preview plays. For the camera it also holds the device open.
    preview: MediaStream,
    /// A screen share's own browser stream: stopped on drop, which ends the
    /// share and the browser's "sharing" indicator. `None` for the camera,
    /// whose stream the `camera` crate owns.
    screen: Option<JsValue>,
    /// This track's frame counter; each track numbers its own frames.
    frame: Cell<u32>,
    /// The latest frame of each layer since the last keyframe — what the next
    /// frame of a layer needs is the latest of its own or below.
    latest: Cell<[Option<u32>; frames::LAYERS as usize]>,
    _wake: Closure,
    last_key: Cell<f64>,
    /// The range bitrate control may set, and what it is now, bps.
    min: u32,
    max: u32,
    bitrate: Cell<u32>,
    /// When the bitrate last changed.
    rated: Cell<f64>,
}

impl Drop for Sender {
    fn drop(&mut self) {
        // Stopped before `_wake` drops, so the encoder cannot call into a
        // closure that is gone.
        // SAFETY: live handles from `js_enc_open` / `js_screen_open`.
        unsafe { js_enc_close(self.enc.raw()) };
        if let Some(screen) = &self.screen {
            unsafe { js_stream_stop(screen.raw()) };
        }
    }
}

impl Sender {
    /// What frame `n` needs, and note it as its layer's latest.
    fn deps(&self, n: u32, key: bool, layer: u8) -> frames::Deps {
        let mut latest = self.latest.get();
        let back = if key {
            latest = [None; frames::LAYERS as usize];
            0
        } else {
            // The latest of its own layer or below. Without one since the last
            // keyframe — a stream without layers, or one just restarted — the
            // frame before.
            latest[..=layer as usize]
                .iter()
                .flatten()
                .map(|&f| n.wrapping_sub(f))
                .min()
                .unwrap_or(1)
                .clamp(1, u8::MAX as u32) as u8
        };
        latest[layer as usize] = Some(n);
        self.latest.set(latest);
        frames::Deps { key, layer, back }
    }

    fn key(&self) {
        let now = now_ms();
        if now - self.last_key.get() >= KEY_GAP_MS {
            self.last_key.set(now);
            // SAFETY: a live handle.
            unsafe { js_enc_key(self.enc.raw()) };
        }
    }
}

/// Someone we can see.
struct Remote {
    assembler: Assembler,
    dec: JsValue,
    stream: MediaStream,
    _writer: FrameWriter,
    asked: f64,
    /// When the last piece of this track arrived, ms.
    last: f64,
    /// Frames drawn, and a running mean of how long they waited (ms).
    shown: u64,
    hold: f64,
}

impl Drop for Remote {
    fn drop(&mut self) {
        // SAFETY: a live handle from `js_dec_open`.
        unsafe { js_dec_close(self.dec.raw()) };
    }
}

impl Remote {
    fn open() -> Result<Remote, String> {
        // SAFETY: fresh handles, owned by the `JsValue`s that wrap them.
        let dec = unsafe { js_dec_open() }
            .map(|h| unsafe { JsValue::from_raw(h) })
            .map_err(|e| e.to_string())?;
        let native = unsafe { JsValue::from_raw(js_dec_stream(dec.raw())) }
            .unchecked_into::<web_glue::dom::MediaStream>();
        // No CPU frames: the native stream is the whole of it, and `Video`
        // plays that.
        let (stream, writer) = MediaStream::new();
        stream.set_native_source(Rc::new(native));
        Ok(Remote {
            assembler: Assembler::default(),
            dec,
            stream,
            _writer: writer,
            asked: f64::NEG_INFINITY,
            last: now_ms(),
            shown: 0,
            hold: 0.0,
        })
    }

    /// Draw whatever is due now.
    fn show_due(&mut self, clock: Option<av_sync::AudioClock>, now: f64) {
        // SAFETY: a live handle throughout.
        let queue: Vec<av_sync::Queued> = (0..unsafe { js_dec_queued(self.dec.raw()) })
            .map(|i| unsafe {
                av_sync::Queued { capture: js_dec_capture(self.dec.raw(), i), decoded: js_dec_decoded(self.dec.raw(), i) }
            })
            .collect();
        let n = av_sync::due(&queue, clock, now, MAX_HOLD_MS);
        if n > 0 {
            unsafe { js_dec_show(self.dec.raw(), n as u32) };
            self.shown += 1;
            self.hold = self.hold * 0.9 + (now - queue[n - 1].decoded) * 0.1;
        }
    }
}

struct Shared {
    /// Key requests out, as control messages; see [`Video::new`].
    control: Rc<dyn Fn(String)>,
    /// Our id, from the relay's "you are #N". 0 until it says.
    me: Cell<u32>,
    /// Ours, by track.
    senders: RefCell<HashMap<u8, Sender>>,
    /// Everyone else's, by (sender, track).
    remotes: RefCell<HashMap<(u32, u8), Remote>>,
    /// The audio clocks reported for each (sender, audio track).
    clocks: RefCell<av_sync::Clocks>,
    /// Shows due frames, once a display frame.
    frames: RefCell<Option<runtime_core::RafLoop>>,
    queue: mpsc::Sender<Vec<u8>>,
    /// The biggest datagram we may send, header included.
    payload: usize,
    stats: RefCell<VideoStats>,
}

/// The call's video. Cheap to clone; the call holds one and so does its
/// receive loop.
#[derive(Clone)]
pub struct Video(Rc<Shared>);

impl Video {
    /// Video for a call. Datagrams go into `datagrams`, each at most
    /// `payload` bytes; keyframe requests go to `control`, for every other
    /// client to see. Whoever owns the connection carries both.
    pub fn new(datagrams: mpsc::Sender<Vec<u8>>, control: Rc<dyn Fn(String)>, payload: usize) -> Video {
        let video = Video(Rc::new(Shared {
            control,
            me: Cell::new(0),
            senders: RefCell::new(HashMap::new()),
            remotes: RefCell::new(HashMap::new()),
            clocks: RefCell::new(av_sync::Clocks::default()),
            frames: RefCell::new(None),
            queue: datagrams,
            payload,
            stats: RefCell::new(VideoStats::default()),
        }));
        let shared = Rc::downgrade(&video.0);
        let frames = runtime_core::raf_loop(move || {
            let Some(shared) = shared.upgrade() else { return };
            let now = now_ms();
            let clocks = shared.clocks.borrow();
            for (&(sender, track), remote) in shared.remotes.borrow_mut().iter_mut() {
                remote.show_due(clocks.for_video(sender, track), now);
            }
        });
        *video.0.frames.borrow_mut() = Some(frames);
        video
    }

    /// Whether the camera is on.
    pub fn camera_on(&self) -> bool {
        self.0.senders.borrow().contains_key(&TRACK_MAIN)
    }

    /// Whether we are sharing a screen.
    pub fn screen_on(&self) -> bool {
        self.0.senders.borrow().contains_key(&TRACK_SCREEN)
    }

    /// Turn the camera on or off.
    pub async fn set_camera(&self, on: bool) -> Result<(), String> {
        if !on {
            self.0.senders.borrow_mut().remove(&TRACK_MAIN);
            return Ok(());
        }
        if self.camera_on() {
            return Ok(());
        }
        let camera = camera::Camera::new()
            .open(camera::CameraConfig::new().with_resolution(WIDTH, HEIGHT).with_fps(FPS))
            .await
            .map_err(|e| format!("camera: {e}"))?;
        // Turned on twice while the first was opening: keep that one.
        if self.camera_on() {
            return Ok(());
        }
        let native = camera
            .native_source()
            .and_then(|n| n.downcast_ref::<web_glue::dom::MediaStream>().map(|s| s.as_js().raw()))
            .ok_or_else(|| "the camera did not give a browser stream".to_string())?;
        self.start(TRACK_MAIN, native, camera, None, (BITRATE_MIN, BITRATE), FPS)?;
        log("[call-video] camera on");
        Ok(())
    }

    /// Start or stop sharing a screen. Starting asks the browser, which shows
    /// its own picker; refusing it is an `Err`. The screen's (or tab's) audio,
    /// when the browser gives it, is [`screen_audio`](Self::screen_audio)'s to
    /// send.
    pub async fn set_screen(&self, on: bool) -> Result<(), String> {
        if !on {
            self.0.senders.borrow_mut().remove(&TRACK_SCREEN);
            return Ok(());
        }
        if self.screen_on() {
            return Ok(());
        }
        let (tx, rx) = futures::channel::oneshot::channel::<JsValue>();
        let tx = RefCell::new(Some(tx));
        let done = Closure::new(move |stream: JsValue| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(stream);
            }
        });
        // SAFETY: a live closure handle, kept alive across the await below.
        unsafe { js_screen_open(done.as_js().raw(), SCREEN_FPS) };
        let stream = rx.await.map_err(|_| "screen share: the browser never answered".to_string())?;
        drop(done);
        if stream.is_null() || stream.is_undefined() {
            return Err("screen share: not shared (refused, or this browser cannot)".to_string());
        }
        if self.screen_on() {
            // SAFETY: a live handle.
            unsafe { js_stream_stop(stream.raw()) };
            return Ok(());
        }
        let native = stream.clone().unchecked_into::<web_glue::dom::MediaStream>();
        let (preview, writer) = MediaStream::new();
        preview.set_native_source(Rc::new(native));
        drop(writer);
        self.start(TRACK_SCREEN, stream.raw(), preview, Some(stream), (SCREEN_BITRATE_MIN, SCREEN_BITRATE), SCREEN_FPS)?;
        log("[call-video] screen share on");
        Ok(())
    }

    /// The screen share's browser stream, when it carries audio, for the
    /// audio path to send as [`TRACK_SCREEN`]'s audio.
    pub fn screen_audio(&self) -> Option<web_sys::MediaStream> {
        let senders = self.0.senders.borrow();
        let screen = senders.get(&TRACK_SCREEN)?.screen.as_ref()?;
        // SAFETY: a live handle.
        if unsafe { js_stream_has_audio(screen.raw()) } == 0 {
            return None;
        }
        js_sys::wasm_bindgen::JsCast::dyn_into::<web_sys::MediaStream>(web_glue::bridge::to_bindgen(screen)).ok()
    }

    /// Whether the screen share was stopped from the browser's own control.
    /// The caller turns its switch off; `set_screen(false)` cleans up.
    pub fn screen_ended(&self) -> bool {
        // SAFETY: a live handle.
        self.0.senders.borrow().get(&TRACK_SCREEN).is_some_and(|t| unsafe { js_enc_ended(t.enc.raw()) } == 1)
    }

    /// Open an encoder on `native` (a web-glue stream handle) as `track`.
    fn start(
        &self,
        track: u8,
        native: u32,
        preview: MediaStream,
        screen: Option<JsValue>,
        (min, max): (u32, u32),
        fps: u32,
    ) -> Result<(), String> {
        // A new track starts at its ceiling; bitrate control brings it down
        // if the link says so.
        let bitrate = max;
        let handle = Rc::new(Cell::new(0u32));
        let wake = {
            let (handle, shared) = (handle.clone(), Rc::downgrade(&self.0));
            Closure::new(move |_: JsValue| drain(&shared, handle.get(), track))
        };
        // SAFETY: live handles; the result is a fresh one, owned below.
        let enc = unsafe { js_enc_open(native, wake.as_js().raw(), bitrate, fps) }
            .map(|h| unsafe { JsValue::from_raw(h) })
            .map_err(|e| format!("video encoder: {e}"))?;
        handle.set(enc.raw());
        self.0.senders.borrow_mut().insert(
            track,
            Sender {
                enc,
                preview,
                screen,
                frame: Cell::new(0),
                latest: Cell::new([None; frames::LAYERS as usize]),
                _wake: wake,
                last_key: Cell::new(f64::NEG_INFINITY),
                min,
                max,
                bitrate: Cell::new(bitrate),
                rated: Cell::new(now_ms()),
            },
        );
        Ok(())
    }

    /// The least and most our video tracks can be sent at, together, bps.
    /// Both 0 with none on.
    pub fn bitrate_range(&self) -> (u32, u32) {
        let senders = self.0.senders.borrow();
        (senders.values().map(|s| s.min).sum(), senders.values().map(|s| s.max).sum())
    }

    /// What our video tracks are being sent at now, together, bps.
    pub fn bitrate(&self) -> u32 {
        self.0.senders.borrow().values().map(|s| s.bitrate.get()).sum()
    }

    /// Send all our video at `total` bps together — each track the same share
    /// of the way from its floor to its ceiling. A track changes at most every
    /// [`RATE_GAP_MS`], and not for less than a tenth, since each change costs
    /// a keyframe.
    pub fn set_bitrate(&self, total: u32) {
        let (min, max) = self.bitrate_range();
        if max == 0 {
            return;
        }
        let share = if max > min { (total.saturating_sub(min) as f64 / (max - min) as f64).clamp(0.0, 1.0) } else { 1.0 };
        let now = now_ms();
        for sender in self.0.senders.borrow().values() {
            let target = sender.min + ((sender.max - sender.min) as f64 * share) as u32;
            let current = sender.bitrate.get();
            let small = target.abs_diff(current) < current / 10 && target != sender.min && target != sender.max;
            if target == current || small || now - sender.rated.get() < RATE_GAP_MS {
                continue;
            }
            sender.bitrate.set(target);
            sender.rated.set(now);
            // SAFETY: a live handle.
            unsafe { js_enc_rate(sender.enc.raw(), target) };
        }
    }

    /// What one of our tracks shows, for the preview.
    pub fn local(&self, track: u8) -> Option<MediaStream> {
        self.0.senders.borrow().get(&track).map(|s| s.preview.clone())
    }

    /// Every (sender, track) whose video is arriving, in order. Tracks that
    /// have gone quiet for [`STALE_MS`] are dropped here.
    pub fn remotes(&self) -> Vec<(u32, u8)> {
        let now = now_ms();
        self.0.remotes.borrow_mut().retain(|_, r| now - r.last < STALE_MS);
        let mut ids: Vec<(u32, u8)> = self.0.remotes.borrow().keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    pub fn remote(&self, id: u32, track: u8) -> Option<MediaStream> {
        self.0.remotes.borrow().get(&(id, track)).map(|r| r.stream.clone())
    }

    pub fn stats(&self) -> VideoStats {
        let mut s = *self.0.stats.borrow();
        // SAFETY: live handles throughout.
        let remotes = self.0.remotes.borrow();
        s.frames_shown = remotes.values().map(|r| r.shown).sum();
        s.sync_hold_ms = if remotes.is_empty() {
            0.0
        } else {
            remotes.values().map(|r| r.hold).sum::<f64>() / remotes.len() as f64
        };
        drop(remotes);
        let size = |track: u8| {
            self.0.senders.borrow().get(&track).map(|t| unsafe { js_enc_size(t.enc.raw()) }).unwrap_or(0)
        };
        let (camera, screen) = (size(TRACK_MAIN), size(TRACK_SCREEN));
        (s.width, s.height) = (camera >> 16, camera & 0xffff);
        (s.screen_width, s.screen_height) = (screen >> 16, screen & 0xffff);
        s
    }

    /// Whether the encoder has given up — it reports that once, by erroring.
    pub fn camera_failed(&self) -> bool {
        // SAFETY: a live handle.
        self.0.senders.borrow().values().any(|t| unsafe { js_enc_failed(t.enc.raw()) } == 1)
    }

    /// One piece of a video frame from `from`.
    pub fn on_fragment(&self, from: u32, fragment: Fragment<'_>) {
        let key = (from, fragment.track);
        let mut remotes = self.0.remotes.borrow_mut();
        if !remotes.contains_key(&key) {
            match Remote::open() {
                Ok(r) => {
                    remotes.insert(key, r);
                }
                Err(e) => {
                    log(&format!("[call-video] cannot show #{from}/{}: {e}", fragment.track));
                    return;
                }
            }
        }
        let remote = remotes.get_mut(&key).expect("opened above");
        remote.last = now_ms();
        for frame in remote.assembler.push(fragment) {
            // The capture time, in µs: it comes back out on the decoded
            // VideoFrame, which is how the frame loop knows when to show it.
            let ts = frame.capture as f64 * 1000.0;
            // SAFETY: a live handle and a live slice for the call's duration.
            let ok = unsafe {
                js_dec_decode(remote.dec.raw(), frame.data.as_ptr() as usize, frame.data.len(), frame.key as u32, ts)
            };
            if ok == 0 {
                remote.assembler.reset();
                break;
            }
        }
        if remote.assembler.needs_key() {
            let now = now_ms();
            if now - remote.asked >= ASK_GAP_MS {
                remote.asked = now;
                self.0.stats.borrow_mut().keys_asked += 1;
                (self.0.control)(KeyRequest { target: from, track: key.1 }.to_text());
            }
        }
    }

    /// The audio clock for `id`'s `audio_track`: which capture time is coming
    /// out of our speakers. Their frames are held until the audio reaches
    /// them — see `av_sync::Clocks::for_video` for which clock a video track
    /// follows.
    pub fn set_audio_clock(&self, id: u32, audio_track: u8, clock: av_sync::AudioClock) {
        self.0.clocks.borrow_mut().set(id, audio_track, clock);
    }

    /// A control message. True when it was video's and nobody else needs it.
    pub fn on_control(&self, text: &str) -> bool {
        let said = match FromRelay::parse(text) {
            FromRelay::You(id) => {
                self.0.me.set(id);
                return false;
            }
            FromRelay::Said { text, .. } => text,
            _ => return false,
        };
        // Someone lost part of `target`'s `track`.
        let Some(KeyRequest { target, track }) = KeyRequest::parse(&said) else { return false };
        if target == self.0.me.get() {
            if let Some(sender) = self.0.senders.borrow().get(&track) {
                sender.key();
            }
        }
        true
    }

    /// Someone hung up.
    pub fn left(&self, id: u32) {
        self.0.remotes.borrow_mut().retain(|&(sender, _), _| sender != id);
        self.0.clocks.borrow_mut().forget(id);
    }
}

/// Move whatever the encoder has produced onto the send queue.
fn drain(shared: &Weak<Shared>, enc: u32, track: u8) {
    let Some(shared) = shared.upgrade() else { return };
    if enc == 0 {
        return;
    }
    loop {
        // SAFETY: a live handle; `buf` has room for the chunk it is told of.
        let len = unsafe { js_enc_len(enc) } as usize;
        if len == 0 {
            break;
        }
        let mut buf = vec![0u8; len];
        // On the wire as whole milliseconds, wrapping every ~49 days.
        let capture = unsafe { js_enc_capture(enc) }.max(0.0) as u64 as u32;
        let layer = (unsafe { js_enc_layer(enc) } as u8).min(frames::LAYERS - 1);
        let key = unsafe { js_enc_take(enc, buf.as_mut_ptr() as usize) } == 1;
        send_frame(&shared, track, key, layer, capture, &buf);
    }
}

fn send_frame(shared: &Shared, track: u8, key: bool, layer: u8, capture: u32, data: &[u8]) {
    let Some((frame, deps)) = shared.senders.borrow().get(&track).map(|s| {
        let n = s.frame.get();
        s.frame.set(n.wrapping_add(1));
        (n, s.deps(n, key, layer))
    }) else {
        return;
    };
    let mut stats = shared.stats.borrow_mut();
    let whole = frames::fragment_layered(track, frame, capture, deps, data, shared.payload).is_some_and(|pieces| {
        let mut queue = shared.queue.clone();
        let count = pieces.len() as u64;
        let all = pieces.into_iter().all(|p| queue.try_send(p).is_ok());
        if all {
            stats.datagrams_sent += count;
        }
        all
    });
    if whole {
        stats.frames_sent += 1;
        stats.keys_sent += key as u64;
    } else {
        // The far ends will lose this frame and everything after it, so make
        // the next one something they can start again from.
        stats.frames_dropped += 1;
        drop(stats);
        if let Some(sender) = shared.senders.borrow().get(&track) {
            sender.key();
        }
    }
}

/// One line of counters for the page.
pub fn summary(s: VideoStats) -> String {
    let camera = if s.width == 0 { "camera off".to_string() } else { format!("camera {}×{}", s.width, s.height) };
    let screen = if s.screen_width == 0 {
        String::new()
    } else {
        format!(" · screen {}×{}", s.screen_width, s.screen_height)
    };
    format!(
        "{camera}{screen} · frames sent {} (keys {}) · dropped {} · video datagrams {} · frames shown {} · keyframes asked for {} · lip-sync hold {:.0} ms",
        s.frames_sent, s.keys_sent, s.frames_dropped, s.datagrams_sent, s.frames_shown, s.keys_asked, s.sync_hold_ms,
    )
}

stylesheet! {
    /// A player fills whatever cell the layout gives it.
    pub Player<IdeaThemeRef> {
        base(t) {
            width: Length::pct(100.0),
            height: Length::pct(100.0),
            border_radius: t.radius.md(),
            background: t.color.text(),
        }
    }
}

/// A `<video>` playing `stream`, filling its cell. Cameras `cover` the cell;
/// a screen share is `contain`ed, because cropping someone's slides loses
/// what they are showing.
pub fn player(stream: MediaStream, screen: bool) -> Element {
    video_sdk::Video(video_sdk::VideoProps {
        source: video_sdk::stream(stream),
        autoplay: true,
        // Sound comes through the voice path; a captured canvas has none, and
        // the local preview must not play our own mic back.
        muted: true,
        controls: false,
        loop_playback: false,
        object_fit: if screen { video_sdk::ObjectFit::Contain } else { video_sdk::ObjectFit::Cover },
        on_first_frame: None,
    })
    .with_style(|| StyleApplication::new(Player::sheet()))
    .into()
}

/// The page's `performance.now()` — the clock capture times, the audio clock
/// and the decoder's queue are all on. Not `Date.now()`, which is a different
/// clock altogether.
fn now_ms() -> f64 {
    thread_local! {
        static CLOCK: Option<web_sys::Performance> = web_sys::window().and_then(|w| w.performance());
    }
    CLOCK.with(|c| c.as_ref().map(|p| p.now()).unwrap_or(0.0))
}

fn log(message: &str) {
    web_sys::console::log_1(&message.into());
}
