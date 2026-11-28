//! The menus, the desktop's and the apps': one look and one way of going, wherever a menu opens (a right click on
//! the desktop, the Dock, a widget, the island, the bar; the app's name on the bar). As a Mac's:
//! rows the same height, lines between groups, the keys on the right, what can't be done now
//! greyed; one row lit, by the pointer or the arrows alike; a letter goes to the next row it
//! starts; Enter or Space does it, Escape closes. A row with more opens its menu beside it as the
//! pointer rests there, or at the right arrow, and the left one goes back; moving across to it
//! slantwise, over other rows on the way, doesn't close it. Pressed on what opens it, held and
//! dragged onto a row, let go: that row's done (`Held`); let go at once, it was a click and
//! the menu stays; dragged and let go anywhere else, it goes.
//! `List` is the rows and the keys, for what draws a menu its own way (the desktop's, the app
//! menu's panels); `open` is a native pop-up from the window it opens over, gone at a click
//! elsewhere. One open at a time, with the menus it opened from it.
// (Not every program opens every kind of menu.)
#![allow(dead_code)]

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::popup::{PopupAnchor, PopupConstraintAdjustment, PopupGravity, PopupOptions};
use gpui::prelude::*;
use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, FocusHandle, FontWeight, Global, KeyDownEvent, MouseButton, MouseMoveEvent, Pixels, Point, Window, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, div, point, rgb, size,
};

use crate::icons::glyph;
use crate::look::{Press, color, px};

pub const WIDTH: f32 = 260.;
/// A row: close, as a Mac's are.
pub const ROW: f32 = 26.;
pub const GAP: f32 = 9.;
pub const PADDING: f32 = 5.;
pub const TEXT: f32 = crate::look::text::CALLOUT;
/// A row's, round inside the menu's corners (`shape::MENU`, `PADDING` in).
pub const MENU_ROW_RADIUS: f32 = crate::look::shape::MENU - PADDING;
/// Room around a menu, for its shadow; clicks there are outside it.
const MARGIN: f32 = crate::look::POPUP_SHADOW_REACH;
/// How long the pointer rests on a row with more before its menu opens.
const SUBMENU_DELAY: u64 = 180;
/// How long a row's menu stays while the pointer crosses other rows towards it.
const CROSSING: u64 = 300;
/// Letters typed within this are one word, to go to.
const TYPED: u64 = 900;
/// A press let go within this, where it was, was a click (ms); held longer, or moved, a drag.
const CLICK: u64 = 300;
/// How far the pointer goes in a menu before the press that opened it is a drag.
const SLOP: f32 = 4.;

/// Which way it opens from what it's opened from.
#[derive(Clone, Copy)]
pub enum Place {
    Below,
    Above,
    Beside,
    RightOf,
    LeftOf,
}

#[derive(Clone, Default)]
pub struct Entry {
    pub label: String,
    pub enabled: bool,
    pub separator: bool,
    pub ticked: bool,
    pub children: Vec<Entry>,
    pub id: i32,
    /// Its key, as niri says it ("Super+H"), on the right.
    pub keys: Option<String>,
}

impl Entry {
    /// A row that does `id`.
    pub fn new(id: i32, label: impl Into<String>) -> Entry {
        Entry { label: label.into(), enabled: true, id, ..Default::default() }
    }
    /// A line between groups.
    pub fn line() -> Entry {
        Entry { separator: true, ..Default::default() }
    }
    /// Greyed, as what can't be done now (or what's only said: "Camera: Firefox").
    pub fn enabled(mut self, enabled: bool) -> Entry {
        self.enabled = enabled;
        self
    }
    pub fn ticked(mut self, ticked: bool) -> Entry {
        self.ticked = ticked;
        self
    }
    pub fn keys(mut self, keys: Option<String>) -> Entry {
        self.keys = keys;
        self
    }
    /// A menu of its own, beside it.
    pub fn more(mut self, children: Vec<Entry>) -> Entry {
        self.children = children;
        self
    }
    fn can_light(&self) -> bool {
        !self.separator && self.enabled
    }
}

/// Lines at the ends, and two together, go (apps leave lines where they've hidden what was below
/// or above them).
pub fn tidy(mut entries: Vec<Entry>) -> Vec<Entry> {
    while entries.last().is_some_and(|e| e.separator) {
        entries.pop();
    }
    while entries.first().is_some_and(|e| e.separator) {
        entries.remove(0);
    }
    entries.dedup_by(|a, b| a.separator && b.separator);
    entries
}

pub fn height(entries: &[Entry]) -> f32 {
    PADDING * 2. + entries.iter().map(|e| if e.separator { GAP } else { ROW }).sum::<f32>()
}

/// Where a row starts, down from the menu's top (its padding in it).
pub fn top_of(entries: &[Entry], i: usize) -> f32 {
    PADDING + entries[..i].iter().map(|e| if e.separator { GAP } else { ROW }).sum::<f32>()
}

/// Which row's under the pointer, `x` and `y` across and down from the menu's card's corner (its
/// padding in it): one that can be done, one with a menu of its own, or nothing to do (a line,
/// a greyed row, the card's edge, outside it).
pub fn under(entries: &[Entry], x: f32, y: f32) -> Under {
    if !(0. ..WIDTH).contains(&x) {
        return Under::Nothing;
    }
    let mut top = PADDING;
    for (i, entry) in entries.iter().enumerate() {
        let tall = if entry.separator { GAP } else { ROW };
        if (top..top + tall).contains(&y) {
            return match entry {
                e if !e.can_light() => Under::Nothing,
                e if !e.children.is_empty() => Under::More(i),
                _ => Under::Row(i),
            };
        }
        top += tall;
    }
    Under::Nothing
}

/// What's under the pointer as the press ends.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Under {
    Row(usize),
    /// A row with a menu of its own (opening beside it as the pointer rests there).
    More(usize),
    Nothing,
}

/// What letting go of the press that opened a menu does.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Release {
    Choose(usize),
    Stay,
    Close,
}

/// As on a Mac: a click (let go at once, where it was pressed) leaves the menu open, whatever's
/// under it then; a drag does the row it's let go on, and otherwise closes the menu (but on a
/// row with a menu of its own: that's where it's going).
pub fn release(click: bool, under: Under) -> Release {
    match under {
        Under::Row(i) if !click => Release::Choose(i),
        Under::More(_) => Release::Stay,
        _ if click => Release::Stay,
        _ => Release::Close,
    }
}

/// The press that opened a menu, while it's held: how long, and whether the pointer's moved
/// in the menu (where it is is all a menu's told of it, in its own window's terms).
#[derive(Clone, Copy, Debug)]
pub struct Held {
    since: Instant,
    first: Option<Point<Pixels>>,
    moved: bool,
}

impl Held {
    pub fn new() -> Held {
        Held { since: Instant::now(), first: None, moved: false }
    }

    /// The pointer's at `at`, in the menu.
    pub fn saw(&mut self, at: Point<Pixels>) {
        match self.first {
            None => self.first = Some(at),
            Some(first) => self.moved |= f32::from(at.x - first.x).abs() > SLOP || f32::from(at.y - first.y).abs() > SLOP,
        }
    }

    /// Seen in another menu than before (one opened from it): it's been moved there.
    pub fn elsewhere(&mut self) {
        self.moved = true;
        self.first = None;
    }

    /// Whether, let go now, it was a click.
    pub fn click(&self) -> bool {
        self.click_after(self.since.elapsed())
    }

    fn click_after(&self, held: Duration) -> bool {
        !self.moved && held < Duration::from_millis(CLICK)
    }

    /// Let go with the pointer at `under`: what it does.
    pub fn release(&self, under: Under) -> Release {
        release(self.click(), under)
    }
}

impl Default for Held {
    fn default() -> Held {
        Held::new()
    }
}

/// What a key in a menu does.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Act {
    /// Nothing for the menu: the key's not its.
    None,
    /// The lit row moved.
    Moved,
    /// Do this row's.
    Choose(usize),
    /// Open this row's menu, its first row lit.
    Open(usize),
    /// Back out of this menu, to the one it came from.
    Back,
    Close,
}

/// A menu's rows, which is lit, and what's typed to go to one.
pub struct List {
    pub entries: Vec<Entry>,
    pub lit: Option<usize>,
    typed: String,
    typed_at: Option<Instant>,
}

impl List {
    pub fn new(entries: Vec<Entry>) -> List {
        List { entries: tidy(entries), lit: None, typed: String::new(), typed_at: None }
    }

    /// The first row that can be done lit (opened by the keyboard).
    pub fn light_first(&mut self) {
        self.lit = self.entries.iter().position(Entry::can_light);
    }

    fn step(&mut self, down: bool) {
        let n = self.entries.len();
        if n == 0 {
            return;
        }
        let mut at = match self.lit {
            Some(at) => at,
            None if down => n - 1,
            None => 0,
        };
        for _ in 0..n {
            at = if down { (at + 1) % n } else { (at + n - 1) % n };
            if self.entries[at].can_light() {
                self.lit = Some(at);
                return;
            }
        }
    }

    /// What `key` does (as gpui names it), `typed` the letter it types, if any.
    pub fn key(&mut self, key: &str, typed: Option<&str>) -> Act {
        match key {
            "down" => self.step(true),
            "up" => self.step(false),
            "home" | "pageup" => {
                self.lit = None;
                self.step(true);
            }
            "end" | "pagedown" => {
                self.lit = None;
                self.step(false);
            }
            "enter" | "space" | "kp_enter" => {
                return match self.lit {
                    Some(at) if !self.entries[at].children.is_empty() => Act::Open(at),
                    Some(at) => Act::Choose(at),
                    None => Act::None,
                };
            }
            "right" => {
                return match self.lit {
                    Some(at) if !self.entries[at].children.is_empty() => Act::Open(at),
                    _ => Act::None,
                };
            }
            "left" => return Act::Back,
            "escape" => return Act::Close,
            _ => {
                let Some(letter) = typed.filter(|t| t.chars().count() == 1 && t.chars().all(|c| !c.is_control())) else { return Act::None };
                let again = self.typed_at.is_some_and(|at| at.elapsed() < Duration::from_millis(TYPED));
                if !again {
                    self.typed.clear();
                }
                self.typed.push_str(&letter.to_lowercase());
                self.typed_at = Some(Instant::now());
                // (The same letter again goes on to the next row it starts; a word, to the row it starts.)
                let same = self.typed.chars().all(|c| Some(c) == self.typed.chars().next());
                let word = if same { self.typed.chars().next().map(String::from).unwrap_or_default() } else { self.typed.clone() };
                let n = self.entries.len();
                let from = match self.lit {
                    Some(at) if same => at + 1,
                    Some(at) => at,
                    None => 0,
                };
                let find = |from: usize, word: &str| (0..n).map(|k| (from + k) % n).find(|&i| self.entries[i].can_light() && self.entries[i].label.to_lowercase().starts_with(word));
                // (No row starts with the word: the letter alone, afresh.)
                let found = find(from, &word).or_else(|| {
                    let alone = letter.to_lowercase();
                    let found = find(self.lit.map_or(0, |at| at + 1), &alone);
                    if found.is_some() {
                        self.typed = alone;
                    }
                    found
                });
                match found {
                    Some(i) => self.lit = Some(i),
                    None => return Act::None,
                }
            }
        }
        Act::Moved
    }

    /// The rows, drawn: what `get` finds in the view; `chosen` at a click or Enter, `hovered` as the
    /// pointer comes onto a row.
    pub fn rows<V: 'static>(
        &self,
        cx: &mut Context<V>,
        get: fn(&mut V) -> &mut List,
        chosen: impl Fn(&mut V, usize, &mut Window, &mut Context<V>) + Clone + 'static,
        hovered: impl Fn(&mut V, usize, Point<Pixels>, &mut Window, &mut Context<V>) + Clone + 'static,
    ) -> Vec<AnyElement> {
        self.entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                // (A line between groups: a see-through hairline, a little in from the edges.)
                if entry.separator {
                    return div().flex_none().h(px(GAP)).flex().items_center().child(div().w_full().h(px(1.)).mx(px(8.)).bg(color::hairline())).into_any_element();
                }
                // The lit row on a soft grey, its words their own (slate is for what's on, not
                // what the pointer's on); the keys at the right quiet.
                let lit = self.lit == Some(i);
                let colour = if entry.enabled { color::text() } else { color::faint() };
                let quiet = if entry.enabled { color::muted() } else { color::faint() };
                let has_children = !entry.children.is_empty();
                let (chosen, hovered) = (chosen.clone(), hovered.clone());
                div()
                    .id(("menu-row", i))
                    .h(px(ROW))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(6.))
                    .pr(px(10.))
                    .rounded(px(MENU_ROW_RADIUS))
                    .text_color(rgb(colour))
                    .role(gpui::Role::MenuItem)
                    .aria_label(entry.label.clone())
                    .when(lit, |d| d.bg(color::row_picked()))
                    .when(entry.enabled, |d| d.cursor_pointer().press())
                    .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, window, cx| {
                        let list = get(this);
                        let can = list.entries.get(i).is_some_and(Entry::can_light);
                        if list.lit != Some(i) || !can {
                            hovered(this, i, e.position, window, cx);
                        }
                    }))
                    .child(div().w(px(14.)).flex_none().when(entry.ticked, |d| d.child(glyph("check", 14., colour))))
                    .child(div().flex_1().min_w_0().child(crate::tip::cut(entry.label.clone())))
                    .when_some(entry.keys.clone(), |d, keys| d.child(div().flex_none().text_size(px(crate::look::text::META)).text_color(rgb(quiet)).child(keys)))
                    .when(has_children, |d| d.child(glyph("chevron", 13., quiet)))
                    .when(entry.enabled, |d| {
                        d.on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            chosen(this, i, window, cx);
                        }))
                    })
                    .into_any_element()
            })
            .collect()
    }
}

/// How many menus there are now: for tests/vm-quick.sh to read
/// (`$XDG_RUNTIME_DIR/nami-desktop-menus`: `menus=1 lit=Open`), however each went.
static SHOWN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn note(by: isize, lit: Option<&str>) {
    let now = SHOWN.fetch_add(by as usize, std::sync::atomic::Ordering::Relaxed).wrapping_add(by as usize);
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        let lit = lit.map_or(String::new(), |l| format!(" lit={}", l.replace(' ', "_")));
        let _ = std::fs::write(std::path::Path::new(&dir).join("nami-desktop-menus"), format!("menus={now}{lit}\n"));
    }
}

/// Whether a menu's open now.
pub fn any_open() -> bool {
    SHOWN.load(std::sync::atomic::Ordering::Relaxed) > 0
}

thread_local! {
    /// The press that opened the menus open now, while it's held, and the menu it was last seen
    /// in. (One pointer: one press, whichever of them it's in.)
    static HELD: std::cell::Cell<Option<(Held, Option<AnyWindowHandle>)>> = const { std::cell::Cell::new(None) };
}

/// Whether the app's been asked to tell the menus of every button let go.
struct Watching;
impl Global for Watching {}

/// A button let go anywhere but in a menu (the menus hear their own): the press that opened
/// them ended there, back on what opened them, say. A drag: they go; a click: they stay.
/// Over another program's window, niri gives the release to the menu (or what opened it) the
/// pointer was last over, outside it (nami's niri: desktop/niri/20): that's let go elsewhere too.
fn let_go_elsewhere(e: &gpui::MouseUpEvent, window: &mut Window, cx: &mut App) {
    let Some((held, _)) = HELD.take() else { return };
    let me = window.window_handle();
    if !held.click() && (outside(e.position, window.viewport_size()) || !cx.default_global::<Open>().0.contains(&me)) {
        cx.defer(close_all);
    }
}

/// Whether `at`, in a window's terms, is beyond its edges.
fn outside(at: Point<Pixels>, size: gpui::Size<Pixels>) -> bool {
    !Bounds::new(Point::default(), size).contains(&at)
}

pub struct Menu {
    list: List,
    chosen: Rc<dyn Fn(i32, &mut App)>,
    focus: FocusHandle,
    /// The row whose menu is open beside it, and the turn of the one waiting to open (a newer one wins).
    open_child: Option<usize>,
    waiting: u64,
    /// Where the pointer was last, for which way it's going.
    pointer: Option<Point<Pixels>>,
    /// Whether it was opened from a menu: Left goes back to it.
    nested: bool,
}

impl Drop for Menu {
    fn drop(&mut self) {
        note(-1, None);
        if !any_open() {
            HELD.set(None);
        }
        crate::tip::menu_closed();
    }
}

/// The menus open now, the one opened first first: each after the one it came from.
#[derive(Default)]
struct Open(Vec<AnyWindowHandle>);
impl Global for Open {}

/// Close these, the last opened first. niri lets go of a program that closes a menu while
/// one opened after it is still open (an xdg_popup that isn't the topmost): its windows all
/// go, the bar and the dock with them.
fn close(menus: &[AnyWindowHandle], cx: &mut App) {
    for menu in menus.iter().rev() {
        let _ = menu.update(cx, |_, window, _| window.remove_window());
    }
}

/// Close whatever menu's open, and those opened from it. (From within one of them, the one
/// it's in goes with `window.remove_window()` first, and the rest once that's done:
/// `cx.defer(menu::close_all)`.)
pub fn close_all(cx: &mut App) {
    let open = std::mem::take(&mut cx.default_global::<Open>().0);
    close(&open, cx);
}

/// The menus opened from `parent` go; it stays.
fn close_after(parent: AnyWindowHandle, cx: &mut App) {
    let mut open_now = std::mem::take(&mut cx.default_global::<Open>().0);
    let keep = open_now.iter().position(|m| *m == parent).map_or(0, |at| at + 1);
    let going = open_now.split_off(keep);
    close(&going, cx);
    cx.default_global::<Open>().0 = open_now;
}

/// Open a menu over `parent`, from `anchor` there (a right click's: the pointer, 1 by 1). Must
/// come from a mouse-down or a key: that's what lets it know of clicks outside it. It flips and
/// slides to stay on the screen.
pub fn open(parent: AnyWindowHandle, anchor: Bounds<Pixels>, place: Place, entries: Vec<Entry>, chosen: Rc<dyn Fn(i32, &mut App)>, cx: &mut App) {
    open_with(parent, anchor, place, entries, chosen, false, cx);
}

/// The same, its first row lit: opened from the keyboard.
pub fn open_by_keys(parent: AnyWindowHandle, anchor: Bounds<Pixels>, place: Place, entries: Vec<Entry>, chosen: Rc<dyn Fn(i32, &mut App)>, cx: &mut App) {
    open_with(parent, anchor, place, entries, chosen, true, cx);
}

fn open_with(parent: AnyWindowHandle, anchor: Bounds<Pixels>, place: Place, entries: Vec<Entry>, chosen: Rc<dyn Fn(i32, &mut App)>, by_keys: bool, cx: &mut App) {
    let mut list = List::new(entries);
    if list.entries.is_empty() {
        return;
    }
    if by_keys {
        list.light_first();
    }
    // (A tooltip's popup goes first: a menu's popup is then the top one.)
    crate::tip::hide(cx);
    if !cx.has_global::<Watching>() {
        cx.set_global(Watching);
        cx.observe_mouse_ups(let_go_elsewhere);
    }
    // From a menu, a menu of its own: what's open beyond that one goes. Otherwise a new menu,
    // and what was open goes first, before this one's there.
    let mut open_now = std::mem::take(&mut cx.default_global::<Open>().0);
    let nested = open_now.contains(&parent);
    // (Opened by a press, which is held still: where it's let go decides. A menu opened from
    // this one shares it.)
    if !nested {
        HELD.set((!by_keys).then(|| (Held::new(), None)));
    }
    let keep = open_now.iter().position(|m| *m == parent).map_or(0, |at| at + 1);
    let going = open_now.split_off(keep);
    close(&going, cx);
    let (corner, gravity, offset) = match place {
        Place::Beside => (PopupAnchor::TopRight, PopupGravity::BottomRight, point(px(2. - MARGIN), px(-PADDING - MARGIN))),
        Place::Below => (PopupAnchor::BottomLeft, PopupGravity::BottomRight, point(px(-MARGIN), px(4. - MARGIN))),
        Place::Above => (PopupAnchor::TopLeft, PopupGravity::TopRight, point(px(-MARGIN), px(-8. + MARGIN))),
        Place::RightOf => (PopupAnchor::TopRight, PopupGravity::BottomRight, point(px(8. - MARGIN), px(-MARGIN))),
        Place::LeftOf => (PopupAnchor::TopLeft, PopupGravity::BottomLeft, point(px(-8. + MARGIN), px(-MARGIN))),
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(Point::default(), size(px(WIDTH + 2. * MARGIN), px(height(&list.entries) + 2. * MARGIN))))),
        titlebar: None,
        focus: true,
        show: true,
        is_movable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        kind: WindowKind::AnchoredPopup(PopupOptions {
            parent,
            anchor_rect: anchor,
            anchor: corner,
            gravity,
            constraint_adjustment: PopupConstraintAdjustment::all(),
            offset,
            grab: true,
        }),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        crate::menu_theme(window, cx);
        cx.new(|cx| {
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            note(1, list.lit.map(|i| list.entries[i].label.as_str()));
            crate::tip::menu_opened(cx);
            Menu { list, chosen, focus, open_child: None, waiting: 0, pointer: None, nested }
        })
    });
    if let Ok(menu) = opened {
        open_now.push(menu.into());
    }
    cx.default_global::<Open>().0 = open_now;
}

/// This menu and the rest open with it go: this one now, the others after it.
fn close_from(window: &mut Window, cx: &mut App) {
    window.remove_window();
    cx.defer(close_all);
}

impl Menu {
    fn note_lit(&self) {
        note(0, self.list.lit.map(|i| self.list.entries[i].label.as_str()));
    }

    fn choose(&mut self, i: usize, by_keys: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.list.entries.get(i).cloned().filter(|e| e.enabled && !e.separator) else { return };
        if entry.children.is_empty() {
            let chosen = self.chosen.clone();
            close_from(window, cx);
            chosen(entry.id, cx);
        } else {
            self.open_child(i, by_keys, window, cx);
        }
    }

    fn open_child(&mut self, i: usize, by_keys: bool, window: &mut Window, cx: &mut Context<Self>) {
        let entry = &self.list.entries[i];
        self.list.lit = Some(i);
        self.open_child = Some(i);
        let anchor = Bounds::new(point(px(MARGIN), px(MARGIN + top_of(&self.list.entries, i))), size(px(WIDTH), px(ROW)));
        let (children, chosen, me) = (entry.children.clone(), self.chosen.clone(), window.window_handle());
        cx.defer(move |cx| open_with(me, anchor, Place::Beside, children, chosen, by_keys, cx));
        cx.notify();
    }

    fn close_child(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_child.take().is_some() {
            let me = window.window_handle();
            cx.defer(move |cx| close_after(me, cx));
        }
    }

    /// Whether the pointer, from where it was to `now`, is heading for the open row's menu: in the
    /// triangle from where it was to that menu's near corners (it opens to the right).
    fn heading_for_child(&self, now: Point<Pixels>) -> bool {
        let (Some(open), Some(was)) = (self.open_child, self.pointer) else { return false };
        let top = px(MARGIN + top_of(&self.list.entries, open) - PADDING);
        let child = height(&self.list.entries[open].children);
        let edge = px(MARGIN + WIDTH);
        if now.x <= was.x {
            return false;
        }
        in_triangle(now, was, point(edge, top), point(edge, top + px(child)))
    }

    fn hovered(&mut self, i: usize, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let heading = self.heading_for_child(at);
        self.pointer = Some(at);
        if self.open_child.is_some_and(|open| open != i) && heading {
            // (On its way across to the open menu: that stays a moment, and this row's lit only if
            // the pointer stays.)
            self.waiting += 1;
            let turn = self.waiting;
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(crate::motion::ms(CROSSING)).await;
                let _ = this.update_in(cx, |this, window, cx| {
                    if this.waiting == turn && this.pointer == Some(at) {
                        this.light(i, window, cx);
                    }
                });
            })
            .detach();
            return;
        }
        self.light(i, window, cx);
    }

    /// The pointer's on row `i`: it's lit, a menu open beside another goes, its own opens soon.
    fn light(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let can = self.list.entries.get(i).is_some_and(Entry::can_light);
        if self.open_child.is_some_and(|open| open != i) {
            self.close_child(window, cx);
        }
        self.list.lit = can.then_some(i);
        self.note_lit();
        cx.notify();
        if can && !self.list.entries[i].children.is_empty() && self.open_child != Some(i) {
            self.waiting += 1;
            let turn = self.waiting;
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(crate::motion::ms(SUBMENU_DELAY)).await;
                let _ = this.update_in(cx, |this, window, cx| {
                    if this.waiting == turn && this.list.lit == Some(i) && this.open_child.is_none() {
                        this.open_child(i, false, window, cx);
                    }
                });
            })
            .detach();
        }
    }

    /// The pointer's at `at` here: if the press that opened it is held, it's seen here.
    fn seen(&mut self, at: Point<Pixels>, window: &mut Window) {
        let me = window.window_handle();
        HELD.set(HELD.get().map(|(mut held, was_in)| {
            if was_in.is_some_and(|w| w != me) {
                held.elsewhere();
            }
            held.saw(at);
            (held, Some(me))
        }));
    }

    /// A button let go in this menu: if it ends the press that opened it, as `Held` says.
    fn let_go(&mut self, e: &gpui::MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let scale = crate::look::scale();
        // (The card's in the room for its shadow, in a fine line.)
        let under = under(&self.list.entries, f32::from(e.position.x) / scale - MARGIN - 1., f32::from(e.position.y) / scale - MARGIN - 1.);
        let done = match HELD.take() {
            Some((held, _)) => held.release(under),
            // (A press that began in it, on a line or its edge, dragged onto a row: that row.)
            None => match under {
                Under::Row(i) => Release::Choose(i),
                _ => Release::Stay,
            },
        };
        match done {
            Release::Choose(i) => self.choose(i, false, window, cx),
            Release::Close => close_from(window, cx),
            Release::Stay => {}
        }
    }

    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let typed = e.keystroke.key_char.as_deref().filter(|_| !e.keystroke.modifiers.control && !e.keystroke.modifiers.platform && !e.keystroke.modifiers.alt);
        match self.list.key(&e.keystroke.key, typed) {
            Act::None => return,
            Act::Moved => {
                self.close_child(window, cx);
                self.note_lit();
            }
            Act::Choose(i) => self.choose(i, true, window, cx),
            Act::Open(i) => self.open_child(i, true, window, cx),
            // (Left in the first menu: nothing to go back to.)
            Act::Back if self.nested => window.remove_window(),
            Act::Back => return,
            Act::Close => close_from(window, cx),
        }
        cx.stop_propagation();
        cx.notify();
    }
}

/// Whether `p` is in the triangle `a b c` (either way round).
fn in_triangle(p: Point<Pixels>, a: Point<Pixels>, b: Point<Pixels>, c: Point<Pixels>) -> bool {
    let side = |p: Point<Pixels>, q: Point<Pixels>, r: Point<Pixels>| f32::from(q.x - p.x) * f32::from(r.y - p.y) - f32::from(q.y - p.y) * f32::from(r.x - p.x);
    let (d1, d2, d3) = (side(a, b, p), side(b, c, p), side(c, a, p));
    let neg = d1 < 0. || d2 < 0. || d3 < 0.;
    let pos = d1 > 0. || d2 > 0. || d3 > 0.;
    !(neg && pos)
}

impl Render for Menu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // (Only the menu takes the pointer, to its round corners: the room around it is
        // outside. With Frosted glass niri blurs just that much, desktop/settings.rs.)
        window.set_input_region(Some(&crate::look::rounded_region(Bounds::new(point(px(MARGIN), px(MARGIN)), size(px(WIDTH), px(height(&self.list.entries)))), crate::look::shape::MENU)));
        let rows = self.list.rows(cx, |m| &mut m.list, |this, i, window, cx| this.choose(i, false, window, cx), |this, i, at, window, cx| this.hovered(i, at, window, cx));
        div()
            .size_full()
            .p(px(MARGIN))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            // (Pressed in it, on what's not a row: a press of its own, not the one that opened it.)
            .on_any_mouse_down(|_, _, _| HELD.set(None))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, _| this.seen(e.position, window)))
            .capture_any_mouse_up(cx.listener(Self::let_go))
            .child(card().id("menu").size_full().role(gpui::Role::Menu).children(rows))
    }
}

/// A menu's card: every menu's the same, as frosted as the panels, a little less round
/// (`shape::MENU`).
pub fn card() -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .p(px(PADDING))
        .rounded(px(crate::look::shape::MENU))
        .border_1()
        .border_color(rgb(color::line()))
        .bg(crate::look::frosted(color::panel(), crate::look::Pane::Panel))
        .shadow(crate::look::material(crate::look::Depth::Chip))
        .text_size(px(TEXT))
        .text_color(rgb(color::text()))
        .font_weight(FontWeight::NORMAL)
        .font_family("Geist")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> List {
        List::new(vec![Entry::line(), Entry::new(1, "Open"), Entry::new(2, "Options").more(vec![Entry::new(3, "Keep in Dock")]), Entry::line(), Entry::line(), Entry::new(4, "Hide").enabled(false), Entry::new(5, "Quit"), Entry::line()])
    }

    #[test]
    fn lines_at_the_ends_and_together_go() {
        let l = list();
        assert_eq!(l.entries.iter().map(|e| if e.separator { "-".to_string() } else { e.label.clone() }).collect::<Vec<_>>(), ["Open", "Options", "-", "Hide", "Quit"]);
    }

    #[test]
    fn the_arrows_go_past_lines_and_what_cant_be_done() {
        let mut l = list();
        assert_eq!(l.key("down", None), Act::Moved);
        assert_eq!(l.lit, Some(0));
        l.key("down", None);
        l.key("down", None);
        // (Past the line and Hide, greyed.)
        assert_eq!(l.lit, Some(4));
        l.key("down", None);
        assert_eq!(l.lit, Some(0));
        l.key("up", None);
        assert_eq!(l.lit, Some(4));
        assert_eq!(l.key("enter", None), Act::Choose(4));
        assert_eq!(l.key("escape", None), Act::Close);
        assert_eq!(l.key("left", None), Act::Back);
    }

    #[test]
    fn right_and_enter_open_a_rows_menu() {
        let mut l = list();
        l.key("down", None);
        l.key("down", None);
        assert_eq!(l.key("right", None), Act::Open(1));
        assert_eq!(l.key("space", None), Act::Open(1));
    }

    #[test]
    fn a_letter_goes_to_the_next_row_it_starts() {
        let mut l = List::new(vec![Entry::new(1, "Open"), Entry::new(2, "Options"), Entry::new(3, "Quit"), Entry::new(4, "Other").enabled(false)]);
        l.key("o", Some("o"));
        assert_eq!(l.lit, Some(0));
        l.key("o", Some("o"));
        assert_eq!(l.lit, Some(1));
        l.key("o", Some("o"));
        // (Other's greyed: round to Open.)
        assert_eq!(l.lit, Some(0));
        assert_eq!(l.key("x", Some("x")), Act::None);
        l.key("q", Some("q"));
        assert_eq!(l.lit, Some(2));
    }

    #[test]
    fn whats_under_the_pointer() {
        let l = list();
        // Open, Options (more), a line, Hide (greyed), Quit; 5 px of padding first.
        assert_eq!(under(&l.entries, 10., 2.), Under::Nothing);
        assert_eq!(under(&l.entries, 10., PADDING + 1.), Under::Row(0));
        assert_eq!(under(&l.entries, 10., PADDING + ROW + 1.), Under::More(1));
        assert_eq!(under(&l.entries, 10., PADDING + 2. * ROW + 1.), Under::Nothing);
        assert_eq!(under(&l.entries, 10., PADDING + 2. * ROW + GAP + 1.), Under::Nothing);
        assert_eq!(under(&l.entries, 10., PADDING + 3. * ROW + GAP + 1.), Under::Row(4));
        assert_eq!(under(&l.entries, 10., height(&l.entries)), Under::Nothing);
        // Beside the menu, at a row's height.
        assert_eq!(under(&l.entries, -3., PADDING + 1.), Under::Nothing);
        assert_eq!(under(&l.entries, WIDTH + 3., PADDING + 1.), Under::Nothing);
    }

    #[test]
    fn let_go_beyond_a_windows_edges_is_outside_it() {
        let size = gpui::size(px(200.), px(100.));
        assert!(!outside(point(px(0.), px(0.)), size) && !outside(point(px(199.), px(99.)), size));
        // (Over another program's window, a held press is still said to the menu, out here.)
        for at in [point(px(-1.), px(50.)), point(px(200.), px(50.)), point(px(50.), px(-30.)), point(px(50.), px(400.))] {
            assert!(outside(at, size), "{at:?}");
        }
    }

    #[test]
    fn let_go_a_click_stays_a_drag_chooses_or_closes() {
        // Pressed on what opens it and let go at once: open, whatever's under it.
        assert_eq!(release(true, Under::Nothing), Release::Stay);
        assert_eq!(release(true, Under::Row(2)), Release::Stay);
        // Held and dragged: the row it's let go on, or closed; a row's own menu, open.
        assert_eq!(release(false, Under::Row(2)), Release::Choose(2));
        assert_eq!(release(false, Under::Nothing), Release::Close);
        assert_eq!(release(false, Under::More(1)), Release::Stay);
    }

    #[test]
    fn a_press_held_long_or_moved_is_a_drag() {
        let p = |x: f32, y: f32| point(gpui::px(x), gpui::px(y));
        let mut held = Held::new();
        assert!(held.click_after(Duration::from_millis(120)));
        assert!(!held.click_after(Duration::from_millis(CLICK + 1)));
        // Into the menu, where it opened under the pointer, and a little: still a click.
        held.saw(p(40., 20.));
        held.saw(p(42., 23.));
        assert!(held.click_after(Duration::from_millis(120)));
        held.saw(p(40., 40.));
        assert!(!held.click_after(Duration::from_millis(120)));
        // Seen in a menu opened from it: moved there.
        let mut held = Held::new();
        held.elsewhere();
        assert!(!held.click_after(Duration::ZERO));
    }

    #[test]
    fn slantwise_to_a_rows_menu_is_towards_it() {
        let p = |x: f32, y: f32| point(gpui::px(x), gpui::px(y));
        // From the row's middle, towards the menu at the right edge, from 20 down to 120.
        assert!(in_triangle(p(200., 40.), p(150., 30.), p(260., 20.), p(260., 120.)));
        assert!(!in_triangle(p(200., 5.), p(150., 30.), p(260., 20.), p(260., 120.)));
    }
}
