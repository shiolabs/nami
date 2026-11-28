//! The windows' motion: quick, quiet, and only where something changes. One set of
//! curves, lengths and springs for nami-gui and nami-desktop (which includes this file).
//! What appears fades in as it rises a little; values glide on a spring that never
//! overshoots; what's running breathes. What's opened grows from what opened it and goes back
//! there on a spring (`Opening`), from wherever it is when it's sent the other way; what's
//! dragged follows the hand and, let go, carries on at its speed to where it was thrown
//! (`Track`, `project`, `rubber_band`). With the person's Animations off, gpui leaves it all
//! still (`set_reduce_motion`, from their settings); with Reduce motion, what would move fades
//! instead (`reduced`). `NAMI_SLOW_MOTION=10` makes it ten times slower, to look at it closely.
// (A set of tokens: not every program uses each of them.)
#![allow(dead_code)]

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{Animation, AnimationExt, AnyElement, ElementId, Rgba, SpringAnimation, SpringConfig, Styled, div, pulsating_between, relative, rgb};
use crate::look::px;
use crate::look::text;

use crate::ui::color;

/// A spring as Apple gives one: how long it takes to get most of the way (`response`, in
/// seconds: not a length, it has none) and how much it overshoots (`damping`: 1 none at all,
/// 0.8 a little).
pub const fn response(response: f32, damping: f32) -> SpringConfig {
    let w = 2. * std::f32::consts::PI / response;
    SpringConfig::new(w * w, 2. * damping * w, 1.)
}

/// What's opened (a panel, the island's card, All Apps) growing from what opened it: critically
/// damped, most of the way in a quarter of a second.
pub const OPEN: SpringConfig = response(0.35, 1.);
/// The same going back where it came from: the same path, a little quicker.
pub const CLOSE: SpringConfig = response(0.28, 1.);
/// What's let go of while it moves (a notification flicked away, a page swiped, a slider
/// pulled past its end): it carries on at the hand's speed and settles with a little bounce.
pub const THROWN: SpringConfig = response(0.35, 0.8);
/// With Reduce motion: what moves fades instead, this long in, and out.
pub const FADE_IN: u64 = 180;
pub const FADE_OUT: u64 = 150;

/// Critically damped: settles in about 0.4 s, no wobble.
pub const GLIDE: SpringConfig = SpringConfig::new(170., 26., 1.);

/// Entering, leaving, and the default: fast at first, settling softly.
pub fn ease_out() -> impl Fn(f32) -> f32 {
    let [x1, y1, x2, y2] = EASE_OUT;
    cubic_bezier(x1, y1, x2, y2)
}
/// `ease_out`'s curve, as CSS's cubic-bezier (and niri's `curve "cubic-bezier"`) has it.
pub const EASE_OUT: [f32; 4] = [0.23, 1.0, 0.32, 1.0];
/// Moving or changing shape on screen.
pub fn ease_in_out() -> impl Fn(f32) -> f32 {
    cubic_bezier(0.77, 0.0, 0.175, 1.0)
}
/// Sliding in from an edge, as a drawer does.
pub fn ease_drawer() -> impl Fn(f32) -> f32 {
    cubic_bezier(0.32, 0.72, 0.0, 1.0)
}

/// CSS's cubic-bezier(x1, y1, x2, y2): time in, progress out.
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32) -> impl Fn(f32) -> f32 {
    let (cx, cy) = (3. * x1, 3. * y1);
    let (bx, by) = (3. * (x2 - x1) - cx, 3. * (y2 - y1) - cy);
    let (ax, ay) = (1. - cx - bx, 1. - cy - by);
    move |t: f32| {
        let t = t.clamp(0., 1.);
        if t == 0. || t == 1. {
            return t;
        }
        let x_at = |s: f32| ((ax * s + bx) * s + cx) * s;
        let slope = |s: f32| (3. * ax * s + 2. * bx) * s + cx;
        // Newton's way first; halving when the curve's too flat for it.
        let mut s = t;
        for _ in 0..8 {
            let off = x_at(s) - t;
            let d = slope(s);
            if off.abs() < 1e-5 || d.abs() < 1e-6 {
                break;
            }
            s -= off / d;
        }
        if !(0. ..=1.).contains(&s) || (x_at(s) - t).abs() >= 1e-5 {
            let (mut lo, mut hi) = (0f32, 1f32);
            s = t;
            for _ in 0..30 {
                let x = x_at(s);
                if (x - t).abs() < 1e-5 {
                    break;
                }
                if x < t { lo = s } else { hi = s }
                s = (lo + hi) / 2.;
            }
        }
        ((ay * s + by) * s + cy) * s
    }
}

/// A wrong password's shake, as the lock screen's: across to -10, 8, -5, 2 and back to 0 px,
/// each swing eased into its turn, 270 ms in all. Where it is `elapsed` after it started, or
/// None once it's still again. (With Animations off, don't start it: the red says it.)
pub fn shake(elapsed: Duration) -> Option<f32> {
    const SWINGS: [(f32, f32); 5] = [(50., -10.), (70., 8.), (60., -5.), (50., 2.), (40., 0.)];
    let mut at = elapsed.as_secs_f32() * 1000. / slowdown();
    let mut from = 0.;
    for (n, (length, to)) in SWINGS.iter().enumerate() {
        if at < *length {
            let t = at / length;
            // Out of the start and into the end (out-quad); in-out-sine for the swings between.
            let eased = if n == 0 || n == SWINGS.len() - 1 { 1. - (1. - t) * (1. - t) } else { (1. - (std::f32::consts::PI * t).cos()) / 2. };
            return Some(from + (to - from) * eased);
        }
        at -= length;
        from = *to;
    }
    None
}

/// A press, a small popover: milliseconds.
pub const QUICK: u64 = 150;
/// Something arriving: a panel, a page.
pub const ENTER: u64 = 200;
/// Sliding in from an edge.
pub const SLIDE: u64 = 300;
/// Going away: quicker than coming.
pub const LEAVE: u64 = 150;
/// Light and dark crossing over the whole screen (niri's screen transition), eased out: a
/// quarter of a second, between a panel's coming and a drawer's.
pub const CROSS_FADE: u64 = 250;

/// Critically damped, a quarter of a second: what follows the hand (switches, the dock).
pub const SNAP: SpringConfig = SpringConfig::new(631.65, 50.27, 1.);
/// Half a second, a little bounce (Apple's usual): what's pushed aside.
pub const SPRING: SpringConfig = SpringConfig::new(157.91, 20.11, 1.);
/// niri's own workspace spring (damping-ratio 1, stiffness 1000): the bar moves as the screen does.
pub const WORKSPACE: SpringConfig = SpringConfig::new(1000., 63.25, 1.);

/// The island around the clock widening and narrowing on the bar (and its card from one tab's
/// height to the next): critically damped, so it never overshoots, most of the way in a quarter
/// of a second; interrupted, it turns where it is, at the speed it had.
pub const ISLAND: SpringConfig = SpringConfig::new(400., 40., 1.);

/// A value on a spring, stepped a frame at a time by what draws it by hand (the island's
/// outline): turned where it is, at the speed it had, when it's sent somewhere else.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sprung {
    pub now: f32,
    pub speed: f32,
    pub to: f32,
}

impl Sprung {
    pub fn at(v: f32) -> Sprung {
        Sprung { now: v, speed: 0., to: v }
    }

    /// Where it's to go (from where it is, as fast as it's going).
    pub fn aim(&mut self, to: f32) {
        self.to = to;
    }

    /// There at once (Animations off, or the first time it's drawn).
    pub fn snap(&mut self) {
        self.now = self.to;
        self.speed = 0.;
    }

    pub fn settled(&self) -> bool {
        self.now == self.to && self.speed == 0.
    }

    /// Let go of at `now`, moving at `speed` (units a second), on its way to `to`: it goes on
    /// at that speed, the spring taking over from the hand with no seam.
    pub fn throw(&mut self, now: f32, speed: f32, to: f32) {
        (self.now, self.speed, self.to) = (now, speed, to);
    }

    /// Held at `v` (by the hand): there, and still, until it's let go.
    pub fn hold(&mut self, v: f32) {
        (self.now, self.speed, self.to) = (v, 0., v);
    }

    /// `dt` seconds on: whether it's still moving.
    pub fn step(&mut self, config: SpringConfig, dt: f32) -> bool {
        // (Near enough, and slow enough, it's there: a quarter of a pixel.)
        self.step_within(config, dt, 0.25)
    }

    /// The same, for a value that isn't in pixels: there once it's within `near` of it.
    pub fn step_within(&mut self, config: SpringConfig, dt: f32, near: f32) -> bool {
        if self.settled() {
            return false;
        }
        let state = tuned(config).step(gpui::SpringState { position: self.now, velocity: self.speed }, self.to, dt);
        (self.now, self.speed) = (state.position, state.velocity);
        if (self.now - self.to).abs() < near && self.speed.abs() < near * 16. {
            self.snap();
            return false;
        }
        true
    }
}

/// Reduce motion (Settings, Accessibility): what would move or grow fades instead, briefly, and
/// nothing overshoots. (Animations off is gpui's `reduce_motion`: nothing moves or fades.)
static REDUCED: AtomicBool = AtomicBool::new(false);

pub fn set_reduced(on: bool) {
    REDUCED.store(on, Ordering::Relaxed);
}

pub fn reduced() -> bool {
    REDUCED.load(Ordering::Relaxed)
}

/// Whether things move (not with Animations off, nor with Reduce motion).
pub fn moves(cx: &gpui::App) -> bool {
    !cx.reduce_motion() && !reduced()
}

/// What's opened, how it comes and goes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Arrival {
    /// From what was clicked to open it, and back there, on a spring.
    Grows,
    /// Reduce motion: a short fade in and out, in place.
    Fades,
    /// Opened by a key, or with Animations off: there at once, and gone at once.
    AtOnce,
}

impl Arrival {
    /// As it's opened: by the pointer (`clicked`) or by a key, and as motion is now.
    pub fn of(clicked: bool, cx: &gpui::App) -> Arrival {
        if !clicked || cx.reduce_motion() {
            Arrival::AtOnce
        } else if reduced() {
            Arrival::Fades
        } else {
            Arrival::Grows
        }
    }
}

/// Something opened (a panel, a card), 0 (not there) to 1 (open), stepped as it's drawn. Sent
/// back while it's still coming (or back again as it goes), it turns where it is, at the
/// speed it had: it can be caught at any moment, and never jumps.
#[derive(Clone, Debug)]
pub struct Opening {
    there: Sprung,
    pub how: Arrival,
    leaving: bool,
    frame: Option<Instant>,
}

/// Where an opening is, a frame at a time.
#[derive(Clone, Copy, Debug)]
pub struct Shown {
    /// How far open, 0 to 1 (never past it).
    pub open: f32,
    /// Still on its way, in or out: draw it again next frame.
    pub moving: bool,
    /// All the way back: it can go.
    pub gone: bool,
}

impl Opening {
    pub fn new(how: Arrival) -> Opening {
        let mut there = Sprung::at(0.);
        there.aim(1.);
        if how == Arrival::AtOnce {
            there.snap();
        }
        Opening { there, how, leaving: false, frame: None }
    }

    /// Back to where it came from. (Whether it's gone at once: opened by a key, it goes as it came.)
    pub fn leave(&mut self) -> bool {
        self.leaving = true;
        self.there.aim(0.);
        self.frame = None;
        if self.how == Arrival::AtOnce {
            self.there.snap();
        }
        self.how == Arrival::AtOnce
    }

    /// Opened again as it goes: back out, from where it is.
    pub fn come_back(&mut self) {
        self.leaving = false;
        self.there.aim(1.);
        self.frame = None;
    }

    pub fn leaving(&self) -> bool {
        self.leaving
    }

    /// Where it is now (its first step counted from when it was last sent somewhere).
    pub fn step(&mut self) -> Shown {
        let t = Instant::now();
        let dt = self.frame.map_or(0., |f| (t - f).as_secs_f32().min(0.05));
        self.frame = Some(t);
        self.step_by(dt)
    }

    pub fn step_by(&mut self, dt: f32) -> Shown {
        let moving = match self.how {
            Arrival::AtOnce => {
                self.there.snap();
                false
            }
            // In place, at a steady rate either way: turned, it carries on from where it is.
            Arrival::Fades => {
                let length = ms(if self.leaving { FADE_OUT } else { FADE_IN }).as_secs_f32().max(1e-3);
                let by = dt / length;
                self.there.now = if self.leaving { (self.there.now - by).max(0.) } else { (self.there.now + by).min(1.) };
                self.there.speed = 0.;
                self.there.now != self.there.to
            }
            // (Going, it's gone once it's within 2% of it: the spring's tail, too small to see,
            // would only hold it there a moment, still, as if it had stuck.)
            Arrival::Grows => self.there.step_within(if self.leaving { CLOSE } else { OPEN }, dt, if self.leaving { 0.02 } else { 0.002 }),
        };
        let open = self.there.now.clamp(0., 1.);
        Shown { open, moving, gone: self.leaving && !moving && open <= 0. }
    }

    /// Whether it grows and moves as it opens (or only fades).
    pub fn grows(&self) -> bool {
        self.how == Arrival::Grows
    }
}

/// How far something thrown at `speed` (units a second) carries on before it stops, as a
/// scrolled list does (Apple's: `deceleration` 0.998 a millisecond for the usual feel, 0.99 for
/// a snappier one).
pub fn project(speed: f32, deceleration: f32) -> f32 {
    speed / 1000. * deceleration / (1. - deceleration)
}

/// The usual deceleration for `project`.
pub const DECELERATION: f32 = 0.998;

/// Pulled `over` past an edge, of something `size` big: how far it goes, less and less the
/// further it's pulled (never as far as `size` itself).
pub fn rubber_band(over: f32, size: f32) -> f32 {
    const C: f32 = 0.55;
    if size <= 0. {
        return 0.;
    }
    over * size * C / (size + C * over.abs())
}

/// How much of the hand's speed what's rubber-banded `over` past an edge has (a little over half
/// at the edge, less and less beyond): for its speed when it's let go, so it springs back with
/// no seam.
pub fn rubber_band_slope(over: f32, size: f32) -> f32 {
    const C: f32 = 0.55;
    if size <= 0. {
        return 0.;
    }
    let d = size + C * over.abs();
    size * size * C / (d * d)
}

/// `v`, softly kept within `lo` and `hi`: past them, rubber-banded (`size`: how big it is).
pub fn banded(v: f32, lo: f32, hi: f32, size: f32) -> f32 {
    if v < lo {
        lo + rubber_band(v - lo, size)
    } else if v > hi {
        hi + rubber_band(v - hi, size)
    } else {
        v
    }
}

/// Of the places `stops`, the one nearest where something at `at`, going at `speed`, would stop.
pub fn landing(at: f32, speed: f32, stops: &[f32]) -> f32 {
    let goes_to = at + project(speed, DECELERATION);
    stops.iter().copied().min_by(|a, b| (a - goes_to).abs().total_cmp(&(b - goes_to).abs())).unwrap_or(at)
}

/// Let go `at` (from where it rests, towards where it would go away), moving at `speed`:
/// whether it goes, `far` being far enough. Where it's going decides it, and a hand moving back
/// keeps it, however far it got.
pub fn goes(at: f32, speed: f32, far: f32) -> bool {
    let back = speed * far.signum() < -50.;
    !back && (at + project(speed, DECELERATION)) * far.signum() >= far.abs()
}

/// How far the pointer goes before a drag says which way it's going (and starts to move what's
/// under it): no further, so a press that wobbles is still a press.
pub const HYSTERESIS: f32 = 10.;

/// Which way a drag went, once it's said.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Way {
    /// Along what it moves (it's this drag's).
    Along,
    /// Across it: not this drag's (a scroll, a press that slipped).
    Across,
}

/// Something dragged by the pointer along one line: from where it was grabbed, at the hand's
/// pace (wherever on it it was grabbed stays under the pointer), once the pointer's gone far
/// enough to say it's a drag; how fast it's going, from its last moments, for when it's let go.
#[derive(Clone, Debug)]
pub struct Track {
    /// Where the pointer went down (along, across), and where what's dragged was.
    from: (f32, f32),
    grabbed_at: f32,
    pub way: Option<Way>,
    /// Where it's been lately, and when.
    lately: Vec<(f32, Instant)>,
}

impl Track {
    /// The pointer down at (`along`, `across`) on what's at `at`.
    pub fn new(along: f32, across: f32, at: f32) -> Track {
        Track { from: (along, across), grabbed_at: at, way: None, lately: Vec::new() }
    }

    /// The same, said to be this drag's from the start (a slider's knob, a scroll's swipe).
    pub fn along(along: f32, at: f32) -> Track {
        Track { from: (along, 0.), grabbed_at: at, way: Some(Way::Along), lately: Vec::new() }
    }

    /// The pointer at (`along`, `across`): where what's dragged is now, once it's said it's
    /// this drag's (None before that, or if it's gone across).
    pub fn moved(&mut self, along: f32, across: f32) -> Option<f32> {
        self.moved_at(along, across, Instant::now())
    }

    pub fn moved_at(&mut self, along: f32, across: f32, when: Instant) -> Option<f32> {
        let (d, x) = (along - self.from.0, across - self.from.1);
        if self.way.is_none() && d.hypot(x) >= HYSTERESIS {
            self.way = Some(if d.abs() >= x.abs() { Way::Along } else { Way::Across });
            // (From here, so it doesn't jump the way it's come.)
            self.from.0 += HYSTERESIS.min(d.abs()) * d.signum();
        }
        if self.way != Some(Way::Along) {
            return None;
        }
        let at = self.grabbed_at + along - self.from.0;
        self.lately.push((at, when));
        // (The last tenth of a second says how fast.)
        self.lately.retain(|(_, t)| when.saturating_duration_since(*t) <= Duration::from_millis(100));
        Some(at)
    }

    /// Whether it's this drag's (it's gone far enough along).
    pub fn dragging(&self) -> bool {
        self.way == Some(Way::Along)
    }

    /// How fast it's going now, units a second (0 if it's been still a moment).
    pub fn speed(&self) -> f32 {
        self.speed_at(Instant::now())
    }

    pub fn speed_at(&self, now: Instant) -> f32 {
        let (Some(first), Some(last)) = (self.lately.first(), self.lately.last()) else { return 0. };
        if now.saturating_duration_since(last.1) > Duration::from_millis(60) {
            return 0.;
        }
        let dt = last.1.saturating_duration_since(first.1).as_secs_f32();
        if dt < 0.008 { 0. } else { (last.0 - first.0) / dt }
    }
}

/// What comes into the island as it changes: in once what was there has mostly gone (the
/// first third of the time), on the ease-out after.
pub fn cross_in() -> impl Fn(f32) -> f32 {
    let out = ease_out();
    move |t: f32| out(((t - 0.33) / 0.67).clamp(0., 1.))
}

/// `NAMI_SLOW_MOTION=10`: all of it ten times slower, to look at it closely.
fn slowdown() -> f32 {
    static SLOW: OnceLock<f32> = OnceLock::new();
    *SLOW.get_or_init(|| std::env::var("NAMI_SLOW_MOTION").ok().and_then(|v| v.parse::<f32>().ok()).filter(|f| *f >= 1.).unwrap_or(1.))
}

/// `NAMI_FRAME_TIMES=1`: what draws by hand says on stderr, a line a frame while it moves, how
/// long since the last frame and how long it took to make and draw this one.
pub fn frame_times() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("NAMI_FRAME_TIMES").is_ok_and(|v| v == "1"))
}

/// `n` milliseconds, as long as motion takes now.
pub fn ms(n: u64) -> Duration {
    Duration::from_secs_f32(n as f32 / 1000. * slowdown())
}

/// A spring, as slow as motion is now.
pub fn tuned(config: SpringConfig) -> SpringConfig {
    let f = slowdown();
    SpringConfig::new(config.stiffness / (f * f), config.damping / f, config.mass)
}

/// A spring to move something on. (With Animations off, gpui puts it where it's going; with
/// Reduce motion it's there quickly, with no overshoot.)
pub fn spring(config: SpringConfig) -> SpringAnimation {
    SpringAnimation::new(tuned(if reduced() { QUIET } else { config }))
}

/// Reduce motion's spring: there in well under a fifth of a second, no overshoot.
pub const QUIET: SpringConfig = response(0.15, 1.);

/// What comes and goes by itself, small (the sound's level): an ease-out with no seam, within 1%
/// in about 160 ms; sent back as it goes, it turns where it is.
pub const BRIEF: SpringConfig = response(0.15, 1.);
/// How long after it starts going on `BRIEF` it's gone: milliseconds.
pub const GONE_AFTER: u64 = 180;

/// Between two colours (0xrrggbb), `t` of the way.
pub fn mix(from: u32, to: u32, t: f32) -> Rgba {
    let (a, b, t) = (rgb(from), rgb(to), t.clamp(0., 1.));
    Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t }
}

/// `el` hanging from the bar: coming down 6 px as it fades in, once, when it first shows.
pub fn drop_in<E: Styled + IntoElement + 'static>(id: impl Into<ElementId>, el: E) -> AnyElement {
    if reduced() {
        return fade_in(id, el);
    }
    el.relative()
        .with_animation(id, Animation::new(ms(ENTER)).with_easing(ease_out()), |el, t| el.opacity(t).top(px(-6. * (1. - t))))
        .into_any_element()
}

/// The same, for a panel with blur behind it: the blur's there at once, so the panel's half
/// there from the first frame too, not a frosted pane ahead of it.
pub fn drop_in_glass<E: Styled + IntoElement + 'static>(id: impl Into<ElementId>, el: E) -> AnyElement {
    if reduced() {
        return el.with_animation(id, Animation::new(ms(FADE_IN)), |el, t| el.opacity(0.5 + 0.5 * t)).into_any_element();
    }
    el.relative()
        .with_animation(id, Animation::new(ms(ENTER)).with_easing(ease_out()), |el, t| el.opacity(0.5 + 0.5 * t).top(px(-6. * (1. - t))))
        .into_any_element()
}

/// `el` fading in and rising 6 px, once, when it first shows under `id`.
pub fn enter<E: Styled + IntoElement + 'static>(id: impl Into<ElementId>, el: E) -> AnyElement {
    if reduced() {
        return fade_in(id, el);
    }
    el.relative()
        .with_animation(id, Animation::new(ms(110)).with_easing(ease_out()), |el, t| el.opacity(t).top(px(6. * (1. - t))))
        .into_any_element()
}

/// `el` coming in from the left, once (a sidebar shown over the page): a short slide and a fade.
pub fn slide_in_left<E: Styled + IntoElement + 'static>(id: impl Into<ElementId>, el: E) -> AnyElement {
    if reduced() {
        return fade_in(id, el);
    }
    el.relative()
        .with_animation(id, Animation::new(ms(ENTER)).with_easing(ease_out()), |el, t| el.opacity(t).left(px(-16. * (1. - t))))
        .into_any_element()
}

/// `el` fading in where it is, once: what Reduce motion has instead of a slide or a rise.
pub fn fade_in<E: Styled + IntoElement + 'static>(id: impl Into<ElementId>, el: E) -> AnyElement {
    el.with_animation(id, Animation::new(ms(FADE_IN)), |el, t| el.opacity(t)).into_any_element()
}

/// Something being looked up, said only if it takes a while: what's quick never shows this.
pub fn looking(id: impl Into<ElementId>, text: &'static str) -> AnyElement {
    div()
        .text_size(px(text::BODY))
        .text_color(rgb(color::muted()))
        .child(text)
        .with_animation(id, Animation::new(ms(600)).with_easing(|t| ((t - 0.5) * 2.).clamp(0., 1.)), |el, t| el.opacity(t))
        .into_any_element()
}

/// A progress bar `height` high: gliding to `part` (0 to 1), or, with
/// none yet, a segment going across and across.
pub fn bar(id: &str, part: Option<f32>, height: f32, tone: u32) -> AnyElement {
    let track = div().h(px(height)).w_full().rounded_full().bg(rgb(color::soft())).relative().overflow_hidden();
    match part {
        Some(p) => track.child(fill(ElementId::Name(format!("{id}-fill").into()), p, div().h_full().rounded_full().bg(rgb(tone)))).into_any_element(),
        None => track
            .child(
                div()
                    .absolute()
                    .top_0()
                    .h_full()
                    .w(relative(0.3))
                    .rounded_full()
                    .bg(rgb(tone))
                    .with_animation(ElementId::Name(format!("{id}-wait").into()), Animation::new(ms(1400)).repeat(), |el, t| el.left(relative(-0.3 + 1.3 * t))),
            )
            .into_any_element(),
    }
}

/// A meter's fill (a level, how full, how busy): `el`, `part` of the way across what it's in (0
/// to 1), gliding to each new reading from where it is, so a sample moves it and never jumps it.
/// New, it's there at once (a panel opening doesn't fill up from empty). Its look is the caller's.
pub fn fill<E: Styled + IntoElement + 'static>(id: impl Into<ElementId>, part: f32, el: E) -> AnyElement {
    el.with_spring(id, spring(GLIDE).to(part.clamp(0., 1.)), |el, v| el.w(relative(v.clamp(0., 1.)))).into_any_element()
}

/// The same, filling up from the bottom (a core's bar).
pub fn fill_up<E: Styled + IntoElement + 'static>(id: impl Into<ElementId>, part: f32, el: E) -> AnyElement {
    el.with_spring(id, spring(GLIDE).to(part.clamp(0., 1.)), |el, v| el.h(relative(v.clamp(0., 1.)))).into_any_element()
}

/// Something going on, with no measure of how far: a small ring, an arc
/// turning on it; still (the ring alone) while it waits its turn.
pub fn spinner(id: impl Into<ElementId>, size: f32, tone: u32, turning: bool) -> AnyElement {
    let ring = gpui::svg().path("icons/ring.svg").absolute().top_0().left_0().size(px(size)).text_color(rgb(if turning { color::line() } else { color::faint() }));
    let d = div().size(px(size)).flex_none().relative().child(ring);
    if !turning {
        return d.into_any_element();
    }
    let arc = gpui::svg()
        .path("icons/arc.svg")
        .absolute()
        .top_0()
        .left_0()
        .size(px(size))
        .text_color(rgb(tone))
        .with_animation(id, Animation::new(ms(850)).repeat(), |s, t| s.with_transformation(gpui::Transformation::rotate(gpui::percentage(t))));
    d.child(arc).into_any_element()
}

/// A dot saying something runs: breathing while `on`, still and faint when not.
pub fn dot(id: impl Into<ElementId>, on: bool, tone: u32) -> AnyElement {
    let d = div().size(px(8.)).flex_none().rounded_full().bg(rgb(if on { tone } else { color::faint() }));
    if on {
        d.with_animation(id, Animation::new(ms(2000)).repeat().with_easing(pulsating_between(0.45, 1.)), |el, t| el.opacity(t)).into_any_element()
    } else {
        d.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn curves_start_and_end_where_they_should() {
        let out = super::ease_out();
        assert_eq!(out(0.), 0.);
        assert_eq!(out(1.), 1.);
        // (CSS's own values for this curve: 0.398 at a tenth, 0.966 at half.)
        assert!((out(0.1) - 0.398).abs() < 0.002);
        assert!((0.95..0.98).contains(&out(0.5)));
        let drawer = super::ease_drawer();
        assert!(drawer(0.25) > 0.5);
    }

    #[test]
    fn springs_as_apple_gives_them() {
        // Critically damped at 1, a little under at 0.8; quicker for a shorter response.
        let (w, z) = super::OPEN.canonical();
        assert!((z - 1.).abs() < 1e-4);
        assert!((w - 2. * std::f32::consts::PI / 0.35).abs() < 1e-3);
        assert!((super::THROWN.canonical().1 - 0.8).abs() < 1e-4);
        assert!(super::CLOSE.canonical().0 > w);
    }

    #[test]
    fn an_opening_turns_where_it_is() {
        use super::{Arrival, Opening};
        let mut o = Opening::new(Arrival::Grows);
        let mut last = 0.;
        // Out, never past open, never back on the way.
        for _ in 0..6 {
            let s = o.step_by(1. / 60.);
            assert!(s.open >= last && s.open <= 1.);
            last = s.open;
        }
        assert!(last > 0.2 && last < 1.);
        // Sent back mid-way: from there, with no jump (it's still going out a moment).
        o.leave();
        let mut s = o.step_by(1. / 60.);
        assert!((s.open - last).abs() < 0.08);
        for _ in 0..120 {
            s = o.step_by(1. / 60.);
        }
        assert!(s.gone && s.open == 0.);
        // By a key: there at once, gone at once.
        let mut keyed = Opening::new(Arrival::AtOnce);
        assert_eq!(keyed.step_by(0.).open, 1.);
        assert!(keyed.leave());
        // Reduce motion: a steady fade, turned where it is.
        let mut faded = Opening::new(Arrival::Fades);
        let a = faded.step_by(0.09).open;
        assert!((a - 0.5).abs() < 0.01);
        faded.leave();
        assert!((faded.step_by(0.03).open - 0.3).abs() < 0.01);
    }

    #[test]
    fn a_throw_carries_on_as_apples_do() {
        // 1000 px/s at 0.998 a millisecond carries on 499 px; at 0.99, 99.
        assert!((super::project(1000., 0.998) - 499.).abs() < 0.5);
        assert!((super::project(1000., 0.99) - 99.).abs() < 0.1);
        assert_eq!(super::project(0., 0.998), 0.);
        assert!(super::project(-500., 0.998) < 0.);
        // The nearest page to where it's going, not to where it was let go.
        let stops = [0., -800., -1600.];
        assert_eq!(super::landing(-300., 0., &stops), 0.);
        assert_eq!(super::landing(-300., -1000., &stops), -800.);
        assert_eq!(super::landing(-100., -3000., &stops), -1600.);
        // Gone: thrown far enough; kept: the hand going back, however far it got.
        assert!(super::goes(40., 800., 190.));
        assert!(!super::goes(40., 0., 190.));
        assert!(super::goes(200., 0., 190.));
        assert!(!super::goes(300., -200., 190.));
        assert!(super::goes(-40., -800., -190.));
    }

    #[test]
    fn past_an_edge_less_and_less() {
        use super::{banded, rubber_band};
        assert_eq!(rubber_band(0., 300.), 0.);
        let (a, b, c) = (rubber_band(50., 300.), rubber_band(100., 300.), rubber_band(1000., 300.));
        assert!(a > 0. && a < 50. && b > a && c > b && c < 300.);
        assert!(b - a < a);
        assert_eq!(rubber_band(-50., 300.), -a);
        assert_eq!(banded(150., 0., 300., 300.), 150.);
        assert_eq!(banded(350., 0., 300., 300.), 300. + a);
        assert_eq!(banded(-50., 0., 300., 300.), -a);
        // Its speed: what the band's slope says, less the further it's pulled, the same either way.
        use super::rubber_band_slope as slope;
        let at = |x: f32| (rubber_band(x + 0.01, 300.) - rubber_band(x - 0.01, 300.)) / 0.02;
        assert!((slope(0., 300.) - 0.55).abs() < 1e-4);
        assert!((slope(80., 300.) - at(80.)).abs() < 1e-2);
        assert!(slope(200., 300.) < slope(80., 300.));
        assert_eq!(slope(-80., 300.), slope(80., 300.));
        assert_eq!(slope(10., 0.), 0.);
    }

    #[test]
    fn a_drag_says_its_way_then_follows_the_hand() {
        use super::{Track, Way};
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        // Grabbed 30 px into something at 100: nothing moves for the first few px.
        let mut drag = Track::new(130., 0., 100.);
        assert_eq!(drag.moved_at(136., 2., at(8)), None);
        assert!(!drag.dragging());
        // Past the threshold, along: from there, a px for a px, no jump.
        let first = drag.moved_at(141., 2., at(16)).unwrap();
        assert!((first - 101.).abs() < 0.01);
        assert_eq!(drag.way, Some(Way::Along));
        assert!((drag.moved_at(171., 2., at(32)).unwrap() - 131.).abs() < 0.01);
        // 30 px in 16 ms: about 1900 px a second; still a moment, none.
        let speed = drag.speed_at(at(32));
        assert!((speed - 1875.).abs() < 1.);
        assert_eq!(drag.speed_at(at(200)), 0.);
        // Across first: not this drag's.
        let mut across = Track::new(0., 0., 0.);
        assert_eq!(across.moved_at(3., 12., at(10)), None);
        assert_eq!(across.way, Some(Way::Across));
        assert_eq!(across.moved_at(40., 12., at(20)), None);
    }

    #[test]
    fn what_comes_and_goes_is_gone_in_time() {
        // `BRIEF` from shown to gone: past halfway in a third of its time, within 1% by
        // `GONE_AFTER`, and never below nothing on the way.
        let mut s = super::Sprung::at(1.);
        s.aim(0.);
        let mut lowest = 1f32;
        for n in 1..=super::GONE_AFTER {
            s.step_within(super::BRIEF, 0.001, 1e-4);
            lowest = lowest.min(s.now);
            if n == super::GONE_AFTER / 3 {
                assert!(s.now < 0.5);
            }
        }
        assert!(s.now < 0.01 && lowest >= 0.);
    }

    #[test]
    fn a_shake_swings_and_settles() {
        use std::time::Duration;
        let at = |ms: u64| super::shake(Duration::from_millis(ms));
        assert_eq!(at(0), Some(0.));
        assert!((at(50).unwrap() + 10.).abs() < 0.01);
        assert!((at(120).unwrap() - 8.).abs() < 0.01);
        assert!(at(265).unwrap().abs() < 1.);
        assert_eq!(at(270), None);
        assert!((0..270).all(|ms| at(ms).unwrap().abs() <= 10.));
    }
}
