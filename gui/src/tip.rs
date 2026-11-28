//! Tooltips, as a Mac's: a small card under (or over) what the pointer rests on, saying what it
//! is, and its key if it has one ("Settings  Super+,"). It comes after the pointer's rested
//! there a while (`DELAY`), and at once while moving from one to the next as one shows. It goes
//! as the pointer leaves, at a click, a key or a scroll. It's a popup of its own, so it shows
//! over a surface as slim as the bar or the dock, and the compositor keeps it on the screen.
//! It fades in; nothing moves.
// (Not every program has a tooltip of every kind.)
#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use gpui::popup::{PopupAnchor, PopupConstraintAdjustment, PopupGravity, PopupOptions};
use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt, AnyWindowHandle, App, Bounds, Context, DispatchPhase, Div, FontWeight, Hitbox, HitboxBehavior, MouseDownEvent, MouseExitEvent, MouseMoveEvent, Pixels, ScrollWheelEvent, SharedString, Stateful,
    Subscription, TextRun, Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, canvas, div, font, point, rgb, size,
};

use crate::look::{color, px};

/// How long the pointer rests before a tooltip shows, as on a Mac.
pub const DELAY: u64 = 600;
/// After one's gone, how soon the next shows at once (moving along the bar's icons).
const AGAIN: u64 = 450;
/// Room round the card, for its shadow.
const MARGIN: f32 = crate::look::POPUP_SHADOW_REACH;
const TEXT: f32 = 12.;
const PAD_X: f32 = 10.;
const HEIGHT: f32 = 26.;
/// Between what it's for and the card.
const GAP: f32 = 6.;
/// Between what it says and its key.
const KEY_GAP: f32 = 12.;
const WIDEST: f32 = 360.;
const RADIUS: f32 = crate::look::shape::ROW;

/// Which way it opens from what it's for. Below unless it can't: the compositor flips it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Side {
    Below,
    Above,
}

#[derive(Clone)]
struct Said {
    text: SharedString,
    keys: Option<SharedString>,
    side: Side,
}

#[derive(Default)]
struct Now {
    /// The tooltip showing, and what it's for.
    showing: Option<(AnyWindowHandle, String)>,
    /// What's waiting its turn, and the turn's number (a newer one wins).
    waiting: Option<String>,
    turn: u64,
    /// When the last one went: the next one soon after shows at once.
    gone_at: Option<Instant>,
    /// What was clicked, or had a key or a scroll while the pointer was on it: none for it till
    /// the pointer's left it.
    quiet: Option<String>,
    keys: Option<Subscription>,
}

thread_local! {
    static NOW: RefCell<Now> = RefCell::new(Now::default());
    /// Whether the pointer's on each, by its element's id, as last seen.
    static ON: RefCell<HashMap<String, Rc<Cell<bool>>>> = RefCell::new(HashMap::new());
}

/// Menus open now: no tooltip while one is (`pause`).
static MENUS: AtomicUsize = AtomicUsize::new(0);

/// A menu's opened: tooltips wait while any is open, and one showing goes.
pub fn menu_opened(cx: &mut App) {
    MENUS.fetch_add(1, Ordering::Relaxed);
    hide(cx);
}

/// A menu's gone.
pub fn menu_closed() {
    let _ = MENUS.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
}

/// What has a tooltip.
pub trait Tip: Sized {
    /// Saying `text`.
    fn tip(self, text: impl Into<SharedString>) -> Self {
        self.tip_with(text, None::<SharedString>, Side::Below)
    }
    /// Saying `text`, and its key (as `shortcuts::said` says it), if it has one.
    fn tip_keys(self, text: impl Into<SharedString>, keys: Option<impl Into<SharedString>>) -> Self {
        self.tip_with(text, keys, Side::Below)
    }
    /// Above what it's for (along the bottom of the screen).
    fn tip_above(self, text: impl Into<SharedString>) -> Self {
        self.tip_with(text, None::<SharedString>, Side::Above)
    }
    fn tip_with(self, text: impl Into<SharedString>, keys: Option<impl Into<SharedString>>, side: Side) -> Self;
}

impl Tip for Stateful<Div> {
    fn tip_with(mut self, text: impl Into<SharedString>, keys: Option<impl Into<SharedString>>, side: Side) -> Self {
        let said = Said { text: text.into(), keys: keys.map(Into::into), side };
        if said.text.is_empty() {
            return self;
        }
        let key = format!("{:?}", self.interactivity().element_id);
        // (Something placed already keeps its place: an absolute one holds its overlay too.)
        let placed = self.style().position.is_some();
        let el = if placed { self } else { self.relative() };
        el.child(watch(key, said))
    }
}

/// Text that may be cut short (an ellipsis at its end), with its whole text as its tooltip, only
/// when it is cut.
pub fn cut(text: impl Into<SharedString>) -> Stateful<Div> {
    let text = text.into();
    let key = format!("cut:{text}");
    let said = Said { text: text.clone(), keys: None, side: Side::Below };
    div().id(gpui::ElementId::Name(key.clone().into())).min_w_0().truncate().relative().child(text).child(watch_if_cut(key, said))
}

/// What watches the pointer over what it's in: drawn over it, taking nothing from it.
fn watch(key: String, said: Said) -> impl IntoElement {
    watch_where(key, said, false)
}

fn watch_if_cut(key: String, said: Said) -> impl IntoElement {
    watch_where(key, said, true)
}

fn watch_where(key: String, said: Said, only_if_cut: bool) -> impl IntoElement {
    let on = ON.with(|on| on.borrow_mut().entry(key.clone()).or_default().clone());
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox: Hitbox, window, cx| {
            // (Cut short: the text's wider than where it's drawn.)
            if only_if_cut && drawn_width(&said.text, window) <= bounds.size.width + gpui::px(0.5) {
                if on.get() {
                    on.set(false);
                    left(&key, cx);
                }
                return;
            }
            // (Gone from the window, the pointer's last place in it is still its hit test's.)
            let now = hitbox.is_hovered(window) && window.is_window_hovered();
            if now != on.get() {
                on.set(now);
                if now {
                    entered(&key, &said, bounds, window, cx);
                } else {
                    left(&key, cx);
                }
            }
            let (moved, moved_key, moved_said, moved_hitbox) = (on.clone(), key.clone(), said.clone(), hitbox.clone());
            window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let now = moved_hitbox.is_hovered(window) && window.is_window_hovered();
                if now != moved.get() {
                    moved.set(now);
                    if now {
                        entered(&moved_key, &moved_said, bounds, window, cx);
                    } else {
                        left(&moved_key, cx);
                    }
                }
            });
            let (pressed_key, pressed_hitbox) = (key.clone(), hitbox.clone());
            window.on_mouse_event(move |_: &MouseDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture && pressed_hitbox.is_hovered(window) {
                    quieten(&pressed_key, cx);
                }
            });
            let (gone_key, gone_on) = (key.clone(), on.clone());
            window.on_mouse_event(move |_: &MouseExitEvent, _, _, cx| {
                if gone_on.replace(false) {
                    left(&gone_key, cx);
                }
            });
            let (scrolled_key, scrolled_hitbox) = (key.clone(), hitbox);
            window.on_mouse_event(move |_: &ScrollWheelEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture && scrolled_hitbox.is_hovered(window) {
                    quieten(&scrolled_key, cx);
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// How wide `text` is, as it's drawn where it is.
fn drawn_width(text: &SharedString, window: &Window) -> Pixels {
    let style = window.text_style();
    let size = style.font_size.to_pixels(window.rem_size());
    let run = style.to_run(text.len());
    window.text_system().shape_line(text.clone(), size, &[run], None).width
}

fn width_of(text: &SharedString, window: &Window) -> Pixels {
    let run = TextRun { len: text.len(), font: font("Geist"), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
    window.text_system().shape_line(text.clone(), px(TEXT), &[run], None).width
}

/// The pointer's come onto it.
fn entered(key: &str, said: &Said, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    if !cfg!(target_os = "linux") {
        return;
    }
    let (at_once, turn) = NOW.with(|now| {
        let mut now = now.borrow_mut();
        if now.quiet.as_deref() == Some(key) || now.showing.as_ref().is_some_and(|(_, k)| k == key) {
            return (false, None);
        }
        now.turn += 1;
        now.waiting = Some(key.to_string());
        let at_once = now.showing.is_some() || now.gone_at.is_some_and(|at| at.elapsed() < Duration::from_millis(AGAIN));
        (at_once, Some(now.turn))
    });
    let Some(turn) = turn else { return };
    watch_keys(cx);
    let parent = window.window_handle();
    let (key, said) = (key.to_string(), said.clone());
    let width = width_of(&said.text, window) + said.keys.as_ref().map_or(gpui::px(0.), |k| width_of(k, window) + px(KEY_GAP));
    let delay = if at_once { 0 } else { DELAY };
    cx.spawn(async move |cx| {
        if delay > 0 {
            cx.background_executor().timer(Duration::from_millis(delay)).await;
        }
        let _ = cx.update(|cx| {
            let still = NOW.with(|now| {
                let now = now.borrow();
                now.turn == turn && now.waiting.as_deref() == Some(key.as_str())
            });
            if still && MENUS.load(Ordering::Relaxed) == 0 {
                show(parent, bounds, key, said, width, cx);
            }
        });
    })
    .detach();
}

/// The pointer's gone from it.
fn left(key: &str, cx: &mut App) {
    let showing = NOW.with(|now| {
        let mut now = now.borrow_mut();
        if now.waiting.as_deref() == Some(key) {
            now.waiting = None;
        }
        if now.quiet.as_deref() == Some(key) {
            now.quiet = None;
        }
        now.showing.as_ref().is_some_and(|(_, k)| k == key)
    });
    if showing {
        close_showing(cx);
    }
}

/// Clicked, a key, scrolled: what shows goes, and none for this till the pointer leaves it.
fn quieten(key: &str, cx: &mut App) {
    NOW.with(|now| {
        let mut now = now.borrow_mut();
        now.quiet = Some(key.to_string());
        now.waiting = None;
    });
    hide(cx);
}

fn watch_keys(cx: &mut App) {
    if NOW.with(|now| now.borrow().keys.is_some()) {
        return;
    }
    let keys = cx.observe_keystrokes(|_, _, cx| {
        let on = NOW.with(|now| {
            let mut now = now.borrow_mut();
            now.waiting = None;
            now.showing.as_ref().map(|(_, k)| k.clone())
        });
        if let Some(key) = on {
            quieten(&key, cx);
        }
    });
    NOW.with(|now| now.borrow_mut().keys = Some(keys));
}

/// Whatever tooltip shows goes now, and none that's waiting comes.
pub fn hide(cx: &mut App) {
    NOW.with(|now| now.borrow_mut().waiting = None);
    close_showing(cx);
}

/// The one showing goes (one waiting for the next along still comes).
fn close_showing(cx: &mut App) {
    let gone = NOW.with(|now| {
        let mut now = now.borrow_mut();
        let gone = now.showing.take();
        if gone.is_some() {
            now.gone_at = Some(Instant::now());
        }
        gone
    });
    if let Some((window, _)) = gone {
        let _ = window.update(cx, |_, window, _| window.remove_window());
        note(None);
    }
}

/// What tooltip shows, for tests/vm-quick.sh to read (`$XDG_RUNTIME_DIR/<program>-tip`).
fn note(said: Option<&str>) {
    let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") else { return };
    let program = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
    let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{program}-tip")), format!("tip={}\n", said.unwrap_or("")));
}

struct Card {
    said: Said,
    /// How wide the card is (its surface has `MARGIN` round it).
    width: f32,
}

fn show(parent: AnyWindowHandle, bounds: Bounds<Pixels>, key: String, said: Said, text_width: Pixels, cx: &mut App) {
    // (One at a time: the one there goes before this one comes, so it's always the top one.)
    close_showing(cx);
    let scale = crate::look::scale();
    let card_w = (f32::from(text_width) / scale + 2. * PAD_X + 2.).min(WIDEST).ceil();
    let (anchor, gravity, offset) = match said.side {
        Side::Below => (PopupAnchor::Bottom, PopupGravity::Bottom, point(gpui::px(0.), px(GAP - MARGIN))),
        Side::Above => (PopupAnchor::Top, PopupGravity::Top, point(gpui::px(0.), px(MARGIN - GAP))),
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(card_w + 2. * MARGIN), px(HEIGHT + 2. * MARGIN))))),
        titlebar: None,
        focus: false,
        show: true,
        is_movable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        kind: WindowKind::AnchoredPopup(PopupOptions {
            parent,
            anchor_rect: bounds,
            anchor,
            gravity,
            constraint_adjustment: PopupConstraintAdjustment::all(),
            offset,
            grab: false,
        }),
        ..Default::default()
    };
    let text = said.text.to_string();
    let opened = cx.open_window(options, |_, cx| cx.new(|_| Card { said, width: card_w }));
    if let Ok(card) = opened {
        NOW.with(|now| {
            let mut now = now.borrow_mut();
            now.showing = Some((card.into(), key));
            now.waiting = None;
        });
        note(Some(&text));
        // (What it's for can go with no word of the pointer leaving it: it's looked at.)
        let window: AnyWindowHandle = card.into();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                let gone = cx.update(|cx| {
                    let mine = NOW.with(|now| now.borrow().showing.as_ref().is_some_and(|(w, _)| *w == window));
                    if !mine {
                        return true;
                    }
                    let away = parent.update(cx, |_, window, _| !window.is_window_hovered() || !bounds.dilate(gpui::px(1.)).contains(&window.mouse_position())).unwrap_or(true);
                    if away {
                        close_showing(cx);
                    }
                    away
                });
                if gone {
                    return;
                }
            }
        })
        .detach();
    }
}

impl Render for Card {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // With Frosted glass it's as frosted as a menu: niri blurs a popup only where it takes
        // the pointer (desktop/settings.rs), so it takes it on the card, to its round corners.
        // The pointer only gets there leaving what it's for, and it goes as that's left.
        // Otherwise it takes nothing: the pointer's for what's under it.
        let frosted = crate::look::vibrant();
        if frosted {
            window.set_input_region(Some(&crate::look::rounded_region(Bounds::new(point(px(MARGIN), px(MARGIN)), size(px(self.width), px(HEIGHT))), RADIUS)));
        } else {
            window.set_input_region(Some(&[]));
        }
        // (A menu's material, round as a row: `look::chip`.)
        let card = crate::look::chip(frosted)
            .h(px(HEIGHT))
            .w(px(self.width))
            .px(px(PAD_X))
            .flex()
            .items_center()
            .gap(px(KEY_GAP))
            .font_family("Geist")
            .text_size(px(TEXT))
            .font_weight(FontWeight::NORMAL)
            .text_color(rgb(color::text()))
            .child(div().min_w_0().truncate().child(self.said.text.clone()))
            .when_some(self.said.keys.clone(), |d, keys| d.child(div().flex_none().text_color(rgb(color::muted_on_glass())).child(keys)));
        div()
            .size_full()
            .p(px(MARGIN))
            .flex()
            .justify_center()
            .child(card.with_animation("tip-in", Animation::new(Duration::from_millis(120)).with_easing(gpui::ease_out_quint()), |el, t| el.opacity(t)))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn waits_as_a_macs_do() {
        assert!((500..=700).contains(&super::DELAY));
        assert!(super::AGAIN < super::DELAY);
    }
}
