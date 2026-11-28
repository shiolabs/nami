//! The look nami's windows share (nami-gui's, namios-installer's): colors,
//! buttons, headings, cards to pick from, pills.

use gpui::prelude::*;
use gpui::{App, BorderStyle, Div, ElementId, FocusHandle, FontWeight, InteractiveElement, ParentElement, Pixels, ScrollHandle, SharedString, Stateful, Styled, canvas, div, outline, rgb};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

/// How much larger than usual everything's drawn: the text size picked (`namios:` in user.nami).
static SCALE: AtomicU32 = AtomicU32::new(0x3f80_0000);

pub fn scale() -> f32 {
    f32::from_bits(SCALE.load(Ordering::Relaxed))
}

#[allow(dead_code)]
pub fn set_scale(scale: f32) {
    SCALE.store(scale.to_bits(), Ordering::Relaxed);
}

/// Text size's word as how much larger.
#[allow(dead_code)]
pub fn scale_of(word: Option<&str>) -> f32 {
    match word {
        Some("small") => 0.9,
        Some("large") => 1.15,
        Some("larger") => 1.3,
        _ => 1.,
    }
}

/// Pixels at that size: all that's drawn goes through this.
pub fn px(n: f32) -> Pixels {
    gpui::px(n * scale())
}

thread_local! {
    /// Each thing Tab gets to, by its element's id: the same one from frame to frame.
    static STOPS: RefCell<HashMap<String, FocusHandle>> = RefCell::new(HashMap::new());
    /// What the page scrolls with, for what's reached by Tab below its edge.
    static SCROLLER: RefCell<Option<ScrollHandle>> = const { RefCell::new(None) };
    /// What was last brought into view: only as focus arrives, or scrolling by hand couldn't.
    static SHOWN: RefCell<Option<FocusHandle>> = const { RefCell::new(None) };
}

#[allow(dead_code)]
pub fn set_scroller(scroller: ScrollHandle) {
    SCROLLER.with(|s| *s.borrow_mut() = Some(scroller));
}

/// Something to press that the keyboard gets to as well: Tab comes to it (Shift+Tab back), its
/// `on_click` hears Enter and Space, and a ring shows it's the one, only when keys are in use.
#[allow(dead_code)]
pub trait Stop: Sized {
    fn stop(self, cx: &App) -> Self;
    /// In a part of its own that scrolls.
    fn stop_in(self, scroller: &ScrollHandle, cx: &App) -> Self;
}

fn stop_handle(id: &Option<ElementId>, cx: &App) -> FocusHandle {
    STOPS.with(|s| s.borrow_mut().entry(format!("{id:?}")).or_insert_with(|| cx.focus_handle().tab_stop(true)).clone())
}

/// What the keyboard's on to start with (a sheet's Cancel): the one by this id, given `stop`.
#[allow(dead_code)]
pub fn focus_stop(id: impl Into<ElementId>, window: &mut gpui::Window, cx: &mut App) {
    let focus = stop_handle(&Some(id.into()), cx);
    window.focus(&focus, cx);
}

fn stop_with(mut el: Stateful<Div>, scroller: Option<ScrollHandle>, cx: &App) -> Stateful<Div> {
    let focus = stop_handle(&el.interactivity().element_id, cx);
    let radius = el.style().corner_radii.top_left.map_or(gpui::px(6.), |r| r.to_pixels(gpui::px(16.)));
    let (seen, ring) = (focus.clone(), focus.clone());
    let ring_id = format!("{:?}", el.interactivity().element_id);
    // (What scrolls cuts off what's outside it, and its rows go from edge to edge.)
    let out = if scroller.is_some() { gpui::px(-1.) } else { gpui::px(4.) };
    // (Placed already, it stays where it was put: relative would undo an absolute one.)
    if el.style().position.is_none() {
        el = el.relative();
    }
    el.track_focus(&focus).child(
        canvas(
            move |bounds, window, _| {
                if !seen.is_focused(window) || !window.last_input_was_keyboard() || SHOWN.with(|s| s.borrow().as_ref() == Some(&seen)) {
                    return;
                }
                SHOWN.with(|s| *s.borrow_mut() = Some(seen.clone()));
                let Some(scroller) = scroller.clone().or_else(|| SCROLLER.with(|s| s.borrow().clone())) else { return };
                let view = scroller.bounds();
                // (What's beside the part that scrolls isn't in it.)
                if bounds.left() < view.left() || bounds.left() > view.right() {
                    return;
                }
                let mut offset = scroller.offset();
                let room = gpui::px(12.);
                if bounds.bottom() + room > view.bottom() {
                    offset.y -= bounds.bottom() + room - view.bottom();
                } else if bounds.top() - room < view.top() {
                    offset.y += view.top() - bounds.top() + room;
                } else {
                    return;
                }
                scroller.set_offset(offset);
                // (This frame's already laid out.)
                window.on_next_frame(|window, _| window.refresh());
            },
            move |bounds, _, window, _| {
                if ring.is_focused(window) && window.last_input_was_keyboard() {
                    note_ring(&ring_id);
                    let ring_at = bounds.dilate(out);
                    // (A pill's "as round as can be" is a very large number.)
                    let round = (radius + out).min(ring_at.size.width.min(ring_at.size.height) / 2.);
                    window.paint_quad(outline(ring_at, color::ring(), BorderStyle::Solid).border_widths(gpui::px(2.)).corner_radii(round));
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full(),
    )
}

/// Which has the ring now, for tests/vm-quick.sh to read (`$XDG_RUNTIME_DIR/<program>-ring`):
/// written as it moves.
fn note_ring(id: &str) {
    thread_local! { static LAST: RefCell<String> = const { RefCell::new(String::new()) }; }
    if LAST.with(|l| *l.borrow() == id) {
        return;
    }
    LAST.with(|l| *l.borrow_mut() = id.to_string());
    let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") else { return };
    let program = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
    let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{program}-ring")), format!("ring={id}\n"));
}

impl Stop for Stateful<Div> {
    fn stop(self, cx: &App) -> Self {
        stop_with(self, None, cx)
    }

    fn stop_in(self, scroller: &ScrollHandle, cx: &App) -> Self {
        stop_with(self, Some(scroller.clone()), cx)
    }
}

/// Something to press that answers the moment it's pressed, not when it's let go: it dips while
/// the button's down. What it does still waits for the release over it (gpui's `on_click`), so
/// pressing, thinking better of it and moving off before letting go does nothing; moved off,
/// it no longer looks pressed, and back on, it does again (gpui's `.active`, as namiOS has it:
/// vendor/gpui-pre, `ElementClickedState::away`).
#[allow(dead_code)]
pub trait Press: Sized {
    /// What has a fill of its own (a button, a tile, an icon): a little dimmer while held.
    fn press(self) -> Self;
    /// A row, or a quiet button with no fill till the pointer's over it: a little of the text's
    /// colour behind it while held, a step deeper than its hover.
    fn press_row(self) -> Self;
}

impl Press for Stateful<Div> {
    fn press(self) -> Self {
        self.active(|s| s.opacity(0.7))
    }

    fn press_row(self) -> Self {
        self.active(|s| s.bg(color::pressed()))
    }
}

pub mod color {
    //! The window's colours, light or dark as the desktop is (`DARK`):
    //! namiOS's greys, its dark slate, and its terminal's muted colours.
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    pub static DARK: AtomicBool = AtomicBool::new(false);
    /// What this program draws is mostly on glass (nami-desktop's bar, Dock, panels, widgets):
    /// its quiet words a step stronger while it's frosted (`muted`).
    pub static ON_GLASS: AtomicBool = AtomicBool::new(false);

    /// A theme's colours in place of these (`themes`): none here means namiOS's own.
    const OWN: u32 = u32::MAX;
    #[allow(clippy::declare_interior_mutable_const)]
    const NOT_SET: AtomicU32 = AtomicU32::new(OWN);
    #[allow(clippy::declare_interior_mutable_const)]
    const ROW: [AtomicU32; 12] = [NOT_SET; 12];
    static THEME: [[AtomicU32; 12]; 2] = [ROW; 2];

    #[allow(dead_code)]
    pub fn set_slot(dark: bool, slot: usize, value: Option<u32>) {
        THEME[dark as usize][slot].store(value.unwrap_or(OWN), Ordering::Relaxed);
    }

    fn themed(slot: usize, light: u32, dark: u32) -> u32 {
        let is_dark = DARK.load(Ordering::Relaxed);
        match THEME[is_dark as usize][slot].load(Ordering::Relaxed) {
            OWN => if is_dark { dark } else { light },
            value => value,
        }
    }

    fn pick(light: u32, dark: u32) -> u32 {
        if DARK.load(Ordering::Relaxed) { dark } else { light }
    }

    pub fn text() -> u32 { themed(0, 0x18181b, 0xf2f3f6) }
    /// Quieter words. On glass (`ON_GLASS`, with Frosted glass on) darker in light and lighter
    /// in dark, about as a sidebar's (`side_muted`): a flat light grey goes flat over what's blurred.
    pub fn muted() -> u32 {
        if ON_GLASS.load(Ordering::Relaxed) { muted_on_glass() } else { themed(1, 0x71717a, 0x9aa2b2) }
    }
    /// The same, on what's frosted in any program (a tooltip): the stronger one while it's
    /// frosted.
    pub fn muted_on_glass() -> u32 {
        if super::vibrant() { themed(1, 0x52525b, 0xbcc2ce) } else { themed(1, 0x71717a, 0x9aa2b2) }
    }
    /// What can't be used now (an admin's page, for others).
    pub fn faint() -> u32 { themed(2, 0xa1a1aa, 0x5d6476) }
    pub fn line() -> u32 { themed(3, 0xe4e4e7, 0x2a3040) }
    pub fn field_line() -> u32 { themed(4, 0xd4d4d8, 0x3a4152) }
    pub fn soft() -> u32 { themed(5, 0xf4f4f5, 0x212634) }
    /// An app icon's ground: a shade off the page.
    pub fn tile() -> u32 { themed(6, 0xffffff, 0x1e2330) }
    pub fn ground() -> u32 { themed(7, 0xfafafa, 0x13161d) }
    /// Pages, fields, cards.
    pub fn panel() -> u32 { themed(8, 0xffffff, 0x191d26) }
    /// What's filled to stand out (the main button, what's picked): a
    /// softer ink than the text's, slate charcoal or slate grey.
    pub fn fill() -> u32 { themed(9, 0x33394a, 0xcdd3de) }
    /// Text on `fill()`.
    pub fn on_text() -> u32 { themed(10, 0xffffff, 0x13161d) }
    pub fn bad() -> u32 { pick(0xb3443c, 0xe08a8a) }
    pub fn bad_soft() -> u32 { pick(0xf8eeec, 0x3a2427) }
    /// Text on `bad()` (a button that takes something away).
    pub fn on_bad() -> u32 { pick(0xffffff, 0x13161d) }
    pub fn good() -> u32 { pick(0x4f7a5a, 0x9cc4a4) }
    pub fn good_soft() -> u32 { pick(0xeef4ef, 0x1c2b23) }
    /// namiOS's deep slate: what's listening now (the microphone), what's loudest; a shade
    /// apart from `key()`'s slate.
    #[allow(dead_code)]
    pub fn deep() -> u32 { pick(0x2a3650, 0xc2cfe4) }
    /// The editor's keys and comments.
    pub fn key() -> u32 { themed(11, 0x45577a, 0xa9bad6) }
    pub fn key_soft() -> u32 {
        // (Another accent: a little of it in the panel's colour, as the slate's is.)
        match THEME[0][11].load(Ordering::Relaxed) {
            OWN => pick(0xeef2f8, 0x1f2738),
            accent => mix(accent, panel(), if DARK.load(Ordering::Relaxed) { 0.22 } else { 0.1 }),
        }
    }

    /// `a` over `b`, `t` of it.
    fn mix(a: u32, b: u32, t: f32) -> u32 {
        let part = |shift: u32| {
            let (x, y) = (((a >> shift) & 0xff) as f32, ((b >> shift) & 0xff) as f32);
            ((x * t + y * (1. - t)).round() as u32) << shift
        };
        part(16) | part(8) | part(0)
    }
    /// The ring round what the keyboard's on (Tab, the arrows): a soft slate, as a Mac's is its
    /// accent's.
    pub fn ring() -> gpui::Rgba {
        gpui::rgba((key() << 8) | 0xb8)
    }
    pub fn comment() -> u32 { pick(0x8b8b93, 0x6f7686) }
    /// A nami file's parts, as the website colours them (site/src/data/hl.js): a key inside a
    /// value (`{ name: Ada }`) the secondary text's grey; a number or version, a quoted string
    /// and a `!secret` in the muted state colours, lighter tints of them in the dark. (Keys are
    /// the text's, a little heavier; paths the slate, `key()`.)
    #[allow(dead_code)]
    pub fn code_inner_key() -> u32 { pick(0x52525b, 0xb3bac7) }
    #[allow(dead_code)]
    pub fn code_number() -> u32 { pick(0x8a6a2e, 0xd8bd8a) }
    #[allow(dead_code)]
    pub fn code_string() -> u32 { pick(0x4f7a5a, 0x9cc4a4) }
    #[allow(dead_code)]
    pub fn code_secret() -> u32 { pick(0x6e5a8a, 0xc2acd6) }
    /// The editor's line with the cursor on it, and the guide down the block it's in: barely
    /// there, the text's colour a little.
    #[allow(dead_code)]
    pub fn current_line() -> gpui::Rgba {
        gpui::rgba((text() << 8) | pick(0x06, 0x07))
    }
    #[allow(dead_code)]
    pub fn indent_guide() -> gpui::Rgba {
        gpui::rgba((text() << 8) | pick(0x1c, 0x1c))
    }

    /// A sidebar over the wallpaper (vibrancy): the window's ground, as see-through as Frosted
    /// glass has it, and thick enough to be a scrim. Its text keeps WCAG AA over anything behind
    /// it, even black or white, and `side_muted()` 3:1 (look's tests).
    pub fn sidebar_glass() -> gpui::Rgba {
        super::frosted(ground(), super::Pane::Sidebar)
    }
    /// A sidebar's quiet text (its headings, counts): darker than `muted()`, for the glass.
    pub fn side_muted() -> u32 { pick(0x52525b, 0xb4bac7) }
    /// What's picked in a sidebar or a list: a soft white pill, a soft light grey in the dark.
    #[allow(dead_code)]
    pub fn picked_pill() -> gpui::Rgba {
        gpui::rgba(pick(0xffffffc8, 0xffffff1f))
    }
    /// Under the pointer, before it's picked: fainter than the picked pill.
    pub fn hover_pill() -> gpui::Rgba {
        gpui::rgba(pick(0x0000000c, 0xffffff10))
    }
    /// Under the pointer in a list or a menu on the desktop's panels, glass or solid: a soft
    /// grey, the text's colour a little (a soft white in dark), its words staying their own, as
    /// a Mac's are. No slate: that's for what's on (look's tests keep the words at AA on it).
    #[allow(dead_code)]
    pub fn row_hover() -> gpui::Rgba {
        gpui::rgba((text() << 8) | pick(0x0f, 0x0a))
    }
    /// What's picked in one (the launcher's line, a menu's lit row, the switcher's app, an app
    /// picked in All Apps): the same grey, a step deeper than the hover.
    #[allow(dead_code)]
    pub fn row_picked() -> gpui::Rgba {
        gpui::rgba((text() << 8) | pick(0x1a, 0x12))
    }
    /// Held down (`Press::press_row`): the text's colour, a little of it, in place of the row's
    /// own (its hover, the picked pill), and deeper than either.
    pub fn pressed() -> gpui::Rgba {
        gpui::rgba((text() << 8) | 0x24)
    }
    /// Behind a dialog over a window (Time Machine's Keep both): what's under it, dimmed with
    /// the darkest slate, light or dark.
    pub fn dim() -> gpui::Rgba {
        gpui::rgba(0x0e111966)
    }
    /// A card's hairline: fainter than `line()`, the shadow does the rest.
    pub fn card_line() -> u32 { pick(0xececef, 0x252a38) }

    /// Control Center's ground, under its modules (`Pane::Controls`): a cool light grey in light,
    /// so the modules on it can be whiter, as a Mac's are; a shade under the panel's in dark.
    #[allow(dead_code)]
    pub fn controls_ground() -> u32 { pick(0xe6e8ec, 0x161920) }
    /// A module in Control Center: a lighter, see-through layer on its ground, white at 72% in
    /// light, at 6% in dark (look's tests keep the text on it at AA over anything).
    #[allow(dead_code)]
    pub fn module() -> gpui::Rgba {
        gpui::rgba(pick(0xffffffb8, 0xffffff10))
    }
    /// The hairline round a module: what keeps it apart where nothing's frosted.
    #[allow(dead_code)]
    pub fn module_line() -> gpui::Rgba {
        gpui::rgba(pick(0x0f172a0d, 0xffffff0a))
    }
    /// Light catching a module's top edge, inside it, as a Mac's materials have it.
    #[allow(dead_code)]
    pub fn module_lit() -> gpui::Rgba {
        gpui::rgba(pick(0xffffffe6, 0xffffff1a))
    }
    /// What's on, filled solid (a switch's round in Control Center, a switch's track): the key's
    /// slate in light; in dark a deeper, more saturated slate than `key()`'s pale one, so white
    /// reads on it (`on_key_strong`, at AA: look's tests).
    #[allow(dead_code)]
    pub fn key_strong() -> u32 {
        // (An accent picked: in dark, its light look's own, which white reads on, as a Mac's
        // accent is the same in both; namiOS's slate its own deeper one.)
        match THEME[0][11].load(Ordering::Relaxed) {
            _ if !DARK.load(Ordering::Relaxed) => key(),
            OWN => 0x5a72a3,
            accent => accent,
        }
    }
    #[allow(dead_code)]
    pub fn on_key_strong() -> u32 { 0xffffff }
    /// A round button that's off, or only a button: the text's colour at 12%, see-through.
    #[allow(dead_code)]
    pub fn round_off() -> gpui::Rgba {
        gpui::rgba((text() << 8) | 0x1f)
    }
    /// Under the pointer in a list on glass: a wash of the key's slate (no more of it in dark
    /// than keeps the text on it at AA over the thinnest ground: look's tests).
    #[allow(dead_code)]
    pub fn key_wash() -> gpui::Rgba {
        gpui::rgba((key() << 8) | pick(0x1a, 0x1e))
    }
    /// Round what's picked where it's a picture (an app in the switcher, the launcher's apps used
    /// lately): a soft plate of the strong slate, see-through, deeper than the wash (in dark the
    /// strong one, not `key()`'s pale one, which would lift the panel under white words); its
    /// name, if it's on it, in the text's colour (look's tests keep it at AA on any panel).
    #[allow(dead_code)]
    pub fn key_plate() -> gpui::Rgba {
        gpui::rgba((key_strong() << 8) | pick(0x2e, 0x40))
    }
    /// A hairline on glass (above "… Settings" at a panel's foot): the text's colour, a little.
    #[allow(dead_code)]
    pub fn hairline() -> gpui::Rgba {
        gpui::rgba((text() << 8) | 0x1a)
    }
    /// A level (Control Center's sliders): its track, the text's colour a little, see-through;
    /// its fill the bright foreground, near white in dark, slate charcoal in light; the glyph on
    /// the fill (`level_ink`, which is always on it); its knob white, with a shadow.
    #[allow(dead_code)]
    pub fn level_track() -> gpui::Rgba {
        gpui::rgba((text() << 8) | pick(0x17, 0x24))
    }
    #[allow(dead_code)]
    pub fn level_fill() -> u32 { pick(0x323a4b, 0xe4e7ec) }
    #[allow(dead_code)]
    pub fn level_ink() -> u32 { pick(0xd4d8e0, 0x5d6370) }
    #[allow(dead_code)]
    pub fn level_knob() -> u32 { 0xffffff }
    /// A module made solid, for what's stacked on what's like it (the notifications' list, an
    /// app's folded under its top one: see-through, the one under would show): a module as it
    /// is on Control Center's ground where nothing's frosted (look's tests keep it so).
    #[allow(dead_code)]
    pub fn module_solid() -> u32 { pick(0xf8f9fa, 0x25272e) }

    /// Settings' page, under its groups (`look::group`), as a Mac's System Settings has it: a
    /// cool light grey, lighter than Control Center's ground (a whole window of it), so the
    /// groups on it are the whiter layer; in dark, Control Center's ground.
    #[allow(dead_code)]
    pub fn grouped() -> u32 { pick(0xf0f1f4, 0x161920) }
    /// Quiet words on that ground (a group's heading, the footnote under it): in light a step
    /// darker than `muted()`, which is under AA on the grey (look's tests).
    #[allow(dead_code)]
    pub fn grouped_muted() -> u32 {
        if DARK.load(Ordering::Relaxed) { muted() } else { 0x5f5f68 }
    }
    /// A segmented control's track (`look::segments`): the text's colour, a little,
    /// see-through.
    #[allow(dead_code)]
    pub fn segment_track() -> gpui::Rgba {
        gpui::rgba((text() << 8) | 0x12)
    }
    /// The picked segment, raised on the track with a knob's shadow: white, or in dark a lit
    /// slate grey (the text on it at AA: look's tests).
    #[allow(dead_code)]
    pub fn segment_on() -> u32 { pick(0xffffff, 0x50566a) }
    /// A push button that isn't the main one (`look::push_button`), a pop-up, a field's well in
    /// a group: the text's colour at 8%; a round button's 12% under the pointer.
    #[allow(dead_code)]
    pub fn push() -> gpui::Rgba {
        gpui::rgba((text() << 8) | 0x14)
    }
    /// A switch's track while it's off: the text's colour, see-through, a step deeper than a
    /// round button's.
    #[allow(dead_code)]
    pub fn switch_off() -> gpui::Rgba {
        gpui::rgba((text() << 8) | 0x2e)
    }

    /// The island hanging from the top edge, as the iPhone's Dynamic Island is ("should
    /// look like iOS", the one place namiOS looks like Apple): pure black, light or dark; iOS's
    /// dark fills for what's pressed in it; its text white, its quieter parts white at about 58%
    /// (as opaque greys: it's always on black), levels on iOS's dark track.
    #[allow(dead_code)]
    pub fn island() -> u32 { 0x000000 }
    #[allow(dead_code)]
    pub fn island_raised() -> u32 { 0x1c1c1e }
    /// A press on what's raised: iOS's next fill up.
    #[allow(dead_code)]
    pub fn island_pressed() -> u32 { 0x2c2c2e }
    #[allow(dead_code)]
    pub fn on_island() -> u32 { 0xffffff }
    #[allow(dead_code)]
    pub fn island_muted() -> u32 { 0x949494 }
    /// A level's track in the island, under its white fill.
    #[allow(dead_code)]
    pub fn island_track() -> u32 { 0x3a3a3c }
    /// Its faintest words (the app a song's from): white at about 38%.
    #[allow(dead_code)]
    pub fn island_faint() -> u32 { 0x616161 }
    /// What's charging, on its black (the AirPods' rings and their bolt): `good()`'s green,
    /// brighter, as it would be lit on black.
    #[allow(dead_code)]
    pub fn island_good() -> u32 { 0x7fcf92 }
    /// What's running low, on its black: `bad()`'s dark red.
    #[allow(dead_code)]
    pub fn island_low() -> u32 { 0xe08a8a }
    /// The microphone in use, on its black: the pale slate, as `deep()` is in dark (in light
    /// `deep()`'s dark slate would be lost on the island's black).
    #[allow(dead_code)]
    pub fn island_listening() -> u32 { 0xc2cfe4 }
}

/// One geometry for every window: a radius family, gutters, one soft shadow.
#[allow(dead_code)]
pub mod shape {
    /// Cards: whatever holds a group in a page.
    pub const CARD: f32 = 14.;
    /// What floats over the desktop or a window: the bar's panels, menus, popovers, a dialog.
    /// As round as a card, so a card in one sits in it as in a page.
    pub const PANEL: f32 = CARD;
    /// A panel that stands by itself, away from the bar, and holds controls a little in from
    /// its edge: the capture bar, the widget gallery.
    pub const FLOATING: f32 = 18.;
    /// Buttons, fields, the pickers in a card.
    pub const CONTROL: f32 = 10.;
    /// A line of a list or a sidebar, and its picked pill; the bar's items, a small button in a
    /// panel, a tooltip.
    pub const ROW: f32 = 8.;
    /// Ticks and the smallest marks.
    pub const SMALL: f32 = 5.;
    /// The page's side room, and above and below.
    pub const GUTTER: f32 = 28.;
    /// Between cards, and between a page's parts.
    pub const GAP: f32 = 16.;
    /// A sidebar's width, Settings' and the others'.
    pub const SIDEBAR: f32 = 220.;
    /// How tall a button is, and a field beside it.
    pub const BUTTON: f32 = 36.;
    /// A dialog's own buttons and fields, a whole row wide: setup's, the password dialog's.
    pub const BUTTON_LARGE: f32 = 40.;
    /// A module in Control Center: as round as a card.
    pub const MODULE: f32 = CARD;
    /// Control Center itself, round about its modules (nearly concentric with them, 10 in), and
    /// each of the bar's panels that holds modules (the calendar, the notifications, the meters).
    pub const CONTROLS: f32 = 22.;
    /// What's in a module, this far in from its edge.
    pub const MODULE_INSET: f32 = 12.;
    /// A menu, as a Mac's: a little less round than a panel, its rows close.
    pub const MENU: f32 = 12.;
    /// A line in a group of settings (`look::group_row`): at least this tall.
    pub const GROUP_ROW: f32 = 40.;
    /// A push button, a pop-up and a segmented control in a group's line.
    pub const PUSH: f32 = 28.;
}

/// The steps of type, and how each is set: hierarchy from weight, size and leading together.
/// Letters close up as text grows and open a touch when it's small (`tracking`, which the text
/// system applies to every size by itself); lines are tight on large text, looser on body
/// (`leading`, through `Type::type_size`).
#[allow(dead_code)]
pub mod text {
    /// The clock on the lock screen, a date or a reading set large: semibold, tight.
    pub const DISPLAY: f32 = 38.;
    /// A page's title: bold.
    pub const TITLE: f32 = 26.;
    /// A dialog's or a step's title, in a window that's a few steps (the installer, setup, the
    /// welcome) or a view of its own (Time Machine): bold. A small dialog over a window (a
    /// password asked for) has `HEADLINE`, semibold.
    pub const DIALOG: f32 = 20.;
    /// A panel's or a sheet's heading, the launcher's and All Apps' parts: semibold.
    pub const HEADLINE: f32 = 17.;
    /// A section in a page ("Recently").
    pub const SECTION: f32 = 15.;
    /// What's read: a row's label, a paragraph, a button's word.
    pub const BODY: f32 = 14.;
    /// The second voice: a menu's item, a pill, a tick, a field's words, the line under a title.
    pub const CALLOUT: f32 = 13.;
    /// What's said about something: a hint under a row, a time, a size.
    pub const META: f32 = 12.;
    /// The smallest words: a badge, a count, under an icon.
    pub const CAPTION: f32 = 11.;

    /// How far apart letters are, in ems, at `size` pixels as drawn (the Text size setting's
    /// already in it, as Dynamic Type's sizes are): +0.02 at 10, nothing for body (13 to 16),
    /// -0.01 at 20, -0.02 from 28, and no tighter than -0.022 however large. In thousandths, so
    /// sizes in between share their shaping.
    pub fn tracking(size: f32) -> f32 {
        const AT: [(f32, f32); 8] = [(10., 0.02), (11., 0.012), (12., 0.006), (13., 0.), (16., 0.), (20., -0.01), (28., -0.02), (72., -0.022)];
        let em = match AT.iter().position(|(s, _)| size < *s) {
            Some(0) => AT[0].1,
            None => AT[AT.len() - 1].1,
            Some(i) => {
                let ((s0, t0), (s1, t1)) = (AT[i - 1], AT[i]);
                t0 + (t1 - t0) * (size - s0) / (s1 - s0)
            }
        };
        (em * 1000.).round() / 1000.
    }

    /// How tall a line is, as a part of `size`: 1.1 for display, 1.15 a title, 1.3 a headline,
    /// 1.45 body, 1.35 the small words (Geist's own is 1.3).
    pub fn leading(size: f32) -> f32 {
        match size {
            s if s >= 32. => 1.1,
            s if s >= 24. => 1.15,
            s if s >= 20. => 1.2,
            s if s >= 17. => 1.3,
            s if s >= 13. => 1.45,
            _ => 1.35,
        }
    }
}

/// The text system's letter spacing (`text::tracking`), for all of this program's text: once,
/// before its windows. (On Linux, where gpui's text is namiOS's own: vendor/gpui-pre-wgpu.)
#[allow(dead_code)]
pub fn set_type() {
    #[cfg(target_os = "linux")]
    gpui_wgpu::set_tracking(text::tracking);
}

/// Type set by the scale: `type_size(n)` is `n` large with its leading (`text::leading`); its
/// tracking the text system gives it by itself. For text that's set on its own (a title, a
/// display, a paragraph), not a row's label that its row centres.
#[allow(dead_code)]
pub trait Type: Styled + Sized {
    fn type_size(self, size: f32) -> Self {
        self.text_size(px(size)).line_height(gpui::relative(text::leading(size)))
    }

    /// Display: large, semibold, tight.
    fn display(self, size: f32) -> Self {
        self.type_size(size).font_weight(FontWeight::SEMIBOLD)
    }
}

impl<E: Styled> Type for E {}

/// The one shadow: barely there, under a card.
#[allow(dead_code)]
pub fn soft_shadow() -> Vec<gpui::BoxShadow> {
    let dark = color::DARK.load(Ordering::Relaxed);
    let (near, far) = if dark { (0x00000038, 0x00000030) } else { (0x0f172a0a, 0x0f172a0c) };
    shadow_of(near, 2., far, 4., 14.)
}

/// A slider's knob, or a switch's: white, lifted off what it's on by a soft shadow and a faint
/// ring, so it shows on a white fill (Control Center's in dark) as on a light track.
#[allow(dead_code)]
pub fn knob_shadow() -> Vec<gpui::BoxShadow> {
    let one = |c: u32, down: f32, blur: f32, spread: f32| gpui::BoxShadow { color: gpui::rgba(c).into(), offset: gpui::point(gpui::px(0.), px(down)), blur_radius: px(blur), spread_radius: px(spread), inset: false };
    vec![one(0x0000001f, 0., 0., 0.5), one(0x00000038, 1., 3., 0.)]
}

/// A module's top edge catching the light (`color::module_lit`), inside it: solid or frosted.
#[allow(dead_code)]
pub fn module_edge() -> Vec<gpui::BoxShadow> {
    vec![gpui::BoxShadow { color: color::module_lit().into(), offset: gpui::point(gpui::px(0.), px(1.)), blur_radius: gpui::px(0.), spread_radius: gpui::px(0.), inset: true }]
}

/// A module, as Control Center's are (and the bar's other panels' since): a lighter,
/// see-through layer on the ground under it (`color::module`), round, a hairline round it and
/// light along its top edge inside, what's in it `shape::MODULE_INSET` in.
#[allow(dead_code)]
pub fn module() -> Div {
    div()
        .flex()
        .flex_col()
        .p(px(shape::MODULE_INSET))
        .rounded(px(shape::MODULE))
        .bg(color::module())
        .border_1()
        .border_color(color::module_line())
        .shadow(module_edge())
}

/// What a module's about, at its top: small and semibold, in the text's colour.
#[allow(dead_code)]
pub fn module_title(title: impl Into<SharedString>) -> Div {
    div().text_size(px(text::CALLOUT)).font_weight(FontWeight::SEMIBOLD).child(crate::tip::cut(title.into().to_string()))
}

/// A few words over something (a tooltip, the Dock's names): round as a row, a panel's fill and
/// line, frosted where it's `see_through` (where niri blurs behind it), with its lit edge and a
/// tight shadow, as a menu's material; solid otherwise. Its size and what's in it are the caller's.
#[allow(dead_code)]
pub fn chip(see_through: bool) -> Div {
    let fill = if see_through { frosted(color::panel(), Pane::Panel) } else { rgb(color::panel()) };
    div()
        .rounded(px(shape::ROW))
        .border_1()
        .border_color(rgb(color::line()))
        .bg(fill)
        .shadow(if see_through { material(Depth::Chip) } else { popup_shadow() })
}

/// A group of settings, as a Mac's System Settings has them: a module (`module()`) on the
/// page's grey (`color::grouped`), its lines (`group_row`) edge to edge in it, their own room in.
#[allow(dead_code)]
pub fn group() -> Div {
    module().p_0().overflow_hidden()
}

/// A line in a group: what it is at the left, how it's changed at the right, at least
/// `shape::GROUP_ROW` tall; under it, but for the `last`, a see-through hairline that starts
/// where its words do, as a Mac's. What's in it is the caller's.
#[allow(dead_code)]
pub fn group_row(last: bool) -> Div {
    div()
        .relative()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .min_h(px(shape::GROUP_ROW))
        .px(px(shape::MODULE_INSET))
        .py(px(6.))
        .when(!last, |d| d.child(div().absolute().bottom_0().left(px(shape::MODULE_INSET)).right_0().h(gpui::px(1.)).bg(color::hairline())))
}

/// A segmented control's track: its `segment`s side by side in it.
#[allow(dead_code)]
pub fn segments() -> Div {
    div().flex().flex_none().items_center().p(px(3.)).gap(px(2.)).rounded_full().bg(color::segment_track())
}

/// One of a few to pick from, in `segments`: a pill, the picked one raised, white (a lit grey
/// in dark) with a knob's shadow, as a Mac's; round like every button.
#[allow(dead_code)]
pub fn segment(id: impl Into<ElementId>, label: impl Into<SharedString>, picked: bool) -> Stateful<Div> {
    segment_base(id, picked).child(label.into())
}

/// A `segment` with its small icon before its name (quieter while it's not the one picked).
#[allow(dead_code)]
pub fn icon_segment(id: impl Into<ElementId>, icon: &str, label: impl Into<SharedString>, picked: bool) -> Stateful<Div> {
    segment_base(id, picked).gap(px(5.)).child(crate::icons::glyph(icon, 14., if picked { color::text() } else { color::muted() })).child(label.into())
}

#[allow(dead_code)]
fn segment_base(id: impl Into<ElementId>, picked: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(shape::PUSH - 4.))
        .px(px(10.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .text_size(px(text::CALLOUT))
        .cursor_pointer()
        .when(picked, |d| d.bg(rgb(color::segment_on())).shadow(knob_shadow()).font_weight(FontWeight::MEDIUM))
        .when(!picked, |d| d.hover(|s| s.bg(color::hover_pill())))
        .press_row()
}

/// A push button, as Settings' and the desktop's panels have them: round, the main one
/// (`primary`) in the key's strong slate with white words, the others the text's colour, a
/// little, under them.
#[allow(dead_code)]
pub fn push_button(id: impl Into<ElementId>, label: impl Into<SharedString>, primary: bool, enabled: bool) -> Stateful<Div> {
    let b = div()
        .id(id)
        .flex_none()
        .h(px(shape::PUSH))
        .px(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .text_size(px(text::CALLOUT))
        .font_weight(FontWeight::MEDIUM)
        .child(label.into());
    let b = if primary { b.bg(rgb(color::key_strong())).text_color(rgb(color::on_key_strong())) } else { b.bg(color::push()).text_color(rgb(color::text())) };
    match (enabled, primary) {
        (false, _) => b.opacity(0.4),
        (true, true) => b.cursor_pointer().hover(|s| s.opacity(0.88)).press(),
        (true, false) => b.cursor_pointer().hover(|s| s.bg(color::round_off())).press_row(),
    }
}

/// The same shadow, for what floats over others (a menu, a panel, a popover): a little deeper.
#[allow(dead_code)]
pub fn lifted_shadow() -> Vec<gpui::BoxShadow> {
    let dark = color::DARK.load(Ordering::Relaxed);
    let (near, far) = if dark { (0x00000050, 0x00000060) } else { (0x0f172a14, 0x0f172a1f) };
    shadow_of(near, 3., far, 10., 30.)
}

/// The same, tight enough for a pop-up of its own (a menu, a tooltip), which has only a little
/// room round it to draw it in: one cut off at that room's edge is a faint rectangle round it.
/// (gpui's blur radius is the shadow's sigma: it reaches about three of them.)
#[allow(dead_code)]
pub fn popup_shadow() -> Vec<gpui::BoxShadow> {
    let dark = color::DARK.load(Ordering::Relaxed);
    let (near, far) = if dark { (0x00000050, 0x00000060) } else { (0x0f172a14, 0x0f172a1f) };
    shadow_of(near, 2., far, 4., 4.)
}

/// How far `popup_shadow` reaches past what it's under, below it (to the sides, a little less).
#[allow(dead_code)]
pub const POPUP_SHADOW_REACH: f32 = 4. + 3. * 4.;

/// How thick a frosted surface is: the bigger, the deeper its shadow, as a Mac's materials.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code)]
pub enum Depth {
    /// A menu, a tooltip: tight (`popup_shadow`), there's little room round it.
    Chip,
    /// A panel, a widget, a notification (`lifted_shadow`).
    Panel,
    /// The biggest that float: the launcher, the notifications' list.
    Sheet,
}

/// A frosted surface's shadow, as thick as it is, and its lit edge (`lit_edge`).
#[allow(dead_code)]
pub fn material(depth: Depth) -> Vec<gpui::BoxShadow> {
    let mut shadows = match depth {
        Depth::Chip => popup_shadow(),
        Depth::Panel => lifted_shadow(),
        Depth::Sheet => {
            let dark = color::DARK.load(Ordering::Relaxed);
            let (near, far) = if dark { (0x00000058, 0x00000078) } else { (0x0f172a18, 0x0f172a2a) };
            shadow_of(near, 4., far, 18., 44.)
        }
    };
    shadows.extend(lit_edge());
    shadows
}

/// Light catching the glass: a faint bright line along a frosted surface's top, inside it, in
/// light and in dark. None where it's solid.
#[allow(dead_code)]
pub fn lit_edge() -> Vec<gpui::BoxShadow> {
    if !(glass().on() && in_namios()) {
        return Vec::new();
    }
    let dark = color::DARK.load(Ordering::Relaxed);
    let edge = |colour: u32, x: f32, y: f32| gpui::BoxShadow { color: gpui::rgba(colour).into(), offset: gpui::point(gpui::px(x), gpui::px(y)), blur_radius: gpui::px(0.), spread_radius: gpui::px(0.), inset: true };
    let lens = lens();
    if !lens.on() {
        return vec![edge(if dark { 0xffffff1c } else { 0xffffffd0 }, 0., 1.)];
    }
    // With Glass, the same one line along the top, a little brighter as the glass is stronger:
    // niri lights the rim under the tint, from the top left. (Lines on the other sides too
    // made a second outline round each card, over a dark wallpaper.)
    let s = lens.strength();
    let lit = if dark { 0x1c + (0x18 as f32 * s) as u32 } else { 0xd8 + (0x27 as f32 * s) as u32 };
    vec![edge(0xffffff00 | lit, 0., 1.)]
}

/// Glass (look::Lens) in this program now: Off until it's told (`set_lens`).
static LENS: AtomicU8 = AtomicU8::new(0);

#[allow(dead_code)]
pub fn set_lens(lens: Lens) {
    LENS.store(Lens::ALL.iter().position(|l| *l == lens).unwrap_or(0) as u8, Ordering::Relaxed);
}

#[allow(dead_code)]
pub fn lens() -> Lens {
    Lens::ALL.get(LENS.load(Ordering::Relaxed) as usize).copied().unwrap_or(Lens::Off)
}

/// A frosted surface `t` of the way there (0 to 1, coming or going), as a material arrives
/// rather than a plain fade: what's on it comes in as its shadow lifts it off what's behind.
/// For whatever moves it (a spring, a tween) to drive. (The blur behind is niri's, there at
/// once: so with glass it starts half there. gpui has no scale for an element.)
#[allow(dead_code)]
pub fn materialize<E: Styled>(el: E, t: f32, depth: Depth) -> E {
    let t = t.clamp(0., 1.);
    let frosted = glass().on() && in_namios();
    let shadows = material(depth)
        .into_iter()
        .map(|mut s| {
            if !s.inset {
                s.color.a *= t;
                s.offset.y = s.offset.y * t;
                s.blur_radius = s.blur_radius * (0.6 + 0.4 * t);
            }
            s
        })
        .collect();
    el.opacity(if frosted { 0.5 + 0.5 * t } else { t }).shadow(shadows)
}

/// Which edge of what scrolls has something floating over it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code)]
pub enum Edge {
    Top,
    Bottom,
}

/// How far a scroll edge's fade reaches.
#[allow(dead_code)]
pub const EDGE_FADE: f32 = 18.;

/// Where what scrolls goes under what floats at its `edge` (a search field, a heading, the
/// sidebar's foot): a soft fade into `fill`, what it's on, instead of a hard line, and only while
/// something's under there. In a `.relative()` round the part that scrolls, after it.
#[allow(dead_code)]
pub fn scroll_edge(scroller: &ScrollHandle, edge: Edge, fill: gpui::Rgba) -> Option<Div> {
    let (at, most) = (scroller.offset().y, scroller.max_offset().y);
    let under = match edge {
        Edge::Top => at < gpui::px(-0.5),
        Edge::Bottom => -at < most - gpui::px(0.5),
    };
    let clear = gpui::Rgba { a: 0., ..fill };
    let fade = div().absolute().left_0().right_0().h(px(EDGE_FADE));
    under.then(|| match edge {
        Edge::Top => fade.top_0().bg(gpui::linear_gradient(180., gpui::linear_color_stop(fill, 0.), gpui::linear_color_stop(clear, 1.))),
        Edge::Bottom => fade.bottom_0().bg(gpui::linear_gradient(0., gpui::linear_color_stop(fill, 0.), gpui::linear_color_stop(clear, 1.))),
    })
}

fn shadow_of(near: u32, near_blur: f32, far: u32, far_down: f32, far_blur: f32) -> Vec<gpui::BoxShadow> {
    let one = |c: u32, down: f32, blur: f32| gpui::BoxShadow { color: gpui::rgba(c).into(), offset: gpui::point(gpui::px(0.), px(down)), blur_radius: px(blur), spread_radius: gpui::px(0.), inset: false };
    vec![one(near, 1., near_blur), one(far, far_down, far_blur)]
}

/// Frosted glass (Settings, Appearance): how much of what's behind the desktop's surfaces and
/// the apps' sidebars shows through them, blurred, as a Mac's vibrancy. Each step is the blur
/// (niri's, desktop/settings.rs), how thick each surface's tint is over it, and how much the
/// colours behind are lifted. Every step keeps text() at WCAG AA over anything behind, black or
/// white (the tests below).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[allow(dead_code)]
pub enum Glass {
    /// Solid, all of it.
    Off,
    /// Mostly solid, a hint of the wallpaper.
    #[default]
    Subtle,
    Medium,
    /// Clearly see-through, still read easily.
    Strong,
}

/// What can be frosted, each its own thickness: thinnest the menu bar, thickest where a lot's read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code)]
pub enum Pane {
    /// The menu bar.
    Bar,
    /// The Dock, menus, notifications, the launcher, the panels.
    Panel,
    /// Control Center, on its own ground (`color::controls_ground`): a touch thinner than a
    /// panel, its modules a lighter layer on it.
    Controls,
    /// A window's sidebar (Settings', System Monitor's), in the window's ground.
    Sidebar,
    /// All Apps, over the whole screen.
    Scrim,
    /// A widget on the desktop, and one quietened under windows.
    Widget,
    QuietWidget,
}

#[allow(dead_code)]
impl Glass {
    pub const ALL: [Glass; 4] = [Glass::Off, Glass::Subtle, Glass::Medium, Glass::Strong];

    /// As the file has it: `glass` (off, subtle, medium, strong), or, from before there was
    /// that, `blur` (off, wallpaper, everything: Off, Medium, Strong); Subtle with neither.
    /// Solid whatever it says with Reduce transparency or high contrast.
    pub fn read(glass: Option<&str>, blur: Option<&str>, reduce: bool, high_contrast: bool) -> Glass {
        if reduce || high_contrast {
            return Glass::Off;
        }
        Glass::chosen(glass, blur)
    }

    /// What's picked, as Settings shows it (Reduce transparency and high contrast aside).
    pub fn chosen(glass: Option<&str>, blur: Option<&str>) -> Glass {
        match glass.and_then(Glass::of_word) {
            Some(g) => g,
            None => match blur {
                Some("off") => Glass::Off,
                Some("wallpaper") => Glass::Medium,
                Some("everything") => Glass::Strong,
                _ => Glass::Subtle,
            },
        }
    }

    pub fn of_word(word: &str) -> Option<Glass> {
        Glass::ALL.into_iter().find(|g| g.word() == word)
    }

    pub fn word(self) -> &'static str {
        match self {
            Glass::Off => "off",
            Glass::Subtle => "subtle",
            Glass::Medium => "medium",
            Glass::Strong => "strong",
        }
    }

    pub fn on(self) -> bool {
        self != Glass::Off
    }

    /// niri's blur: its passes, its offset (together how wide), and the colours' saturation.
    pub fn blur(self) -> (u8, f32, f32) {
        match self {
            Glass::Off => (3, 3., 1.5),
            Glass::Subtle => (3, 4., 1.6),
            Glass::Medium => (4, 4.5, 1.8),
            Glass::Strong => (4, 6., 2.),
        }
    }

    /// Over the wallpaper alone (niri's xray), or over all that's behind: all that's behind, at
    /// every step, as a Mac's. (Over the wallpaper alone, a window over dark windows looked solid.)
    pub fn xray(self) -> bool {
        false
    }

    /// How thick a pane's tint is (its alpha), in light and in dark.
    /// (Light's thicker: milky, as a Mac's, so it stays light over dark windows rather than going
    /// grey; niri's blur lifts the colours behind, not their lightness.)
    pub fn alpha(self, pane: Pane, dark: bool) -> u32 {
        let (light, darker) = match (self, pane) {
            (Glass::Off, Pane::Widget) => (0xf2, 0xec),
            (Glass::Off, Pane::QuietWidget) => (0xd8, 0xc8),
            (Glass::Off, Pane::Scrim) => (0xf2, 0xf2),
            (Glass::Off, _) => (0xff, 0xff),
            (Glass::Subtle, Pane::Bar) => (0xc8, 0xa8),
            (Glass::Subtle, Pane::Panel) => (0xd0, 0xb0),
            (Glass::Subtle, Pane::Controls) => (0xc0, 0xa8),
            (Glass::Subtle, Pane::Sidebar) => (0xd0, 0xb0),
            (Glass::Subtle, Pane::Scrim) => (0xb0, 0xa8),
            (Glass::Subtle, Pane::Widget) => (0xd0, 0xb0),
            (Glass::Subtle, Pane::QuietWidget) => (0xb0, 0x90),
            (Glass::Medium, Pane::Bar) => (0xb8, 0x98),
            (Glass::Medium, Pane::Panel) => (0xc0, 0xa0),
            (Glass::Medium, Pane::Controls) => (0xb0, 0x98),
            (Glass::Medium, Pane::Sidebar) => (0xc0, 0xa0),
            (Glass::Medium, Pane::Scrim) => (0xa0, 0x98),
            (Glass::Medium, Pane::Widget) => (0xc0, 0x98),
            (Glass::Medium, Pane::QuietWidget) => (0xa0, 0x78),
            (Glass::Strong, Pane::Bar) => (0xa8, 0x88),
            (Glass::Strong, Pane::Panel) => (0xb0, 0x90),
            (Glass::Strong, Pane::Controls) => (0xa0, 0x8c),
            (Glass::Strong, Pane::Sidebar) => (0xb0, 0x90),
            (Glass::Strong, Pane::Scrim) => (0x90, 0x88),
            (Glass::Strong, Pane::Widget) => (0xb0, 0x88),
            (Glass::Strong, Pane::QuietWidget) => (0x90, 0x60),
        };
        if dark { darker } else { light }
    }

    /// GTK's apps, as a percentage: their window, title bar (and away), sidebar (and away),
    /// and Files' bars and sidebar; in light and in dark, as `alpha`'s are. Thicker than ours:
    /// their quiet text is fainter. Light's milky, dark's more see-through, but never so thin
    /// that their text reads less well than ours does (the tests below).
    pub fn gtk(self, dark: bool) -> [u32; 7] {
        match (self, dark) {
            (Glass::Off, _) => [100; 7],
            (Glass::Subtle, false) => [86, 82, 78, 74, 70, 86, 76],
            (Glass::Subtle, true) => [74, 70, 66, 64, 60, 74, 64],
            (Glass::Medium, false) => [80, 76, 72, 66, 62, 80, 68],
            (Glass::Medium, true) => [66, 62, 58, 58, 54, 66, 58],
            (Glass::Strong, false) => [72, 68, 64, 58, 54, 72, 60],
            (Glass::Strong, true) => [58, 54, 50, 54, 50, 58, 54],
        }
    }

    /// The terminal's background opacity.
    pub fn terminal(self) -> Option<f32> {
        match self {
            Glass::Off => None,
            Glass::Subtle => Some(0.9),
            Glass::Medium => Some(0.82),
            Glass::Strong => Some(0.75),
        }
    }
}

/// Glass (Settings, Appearance), beside Frosted glass and apart from it: the edges of the
/// desktop's own surfaces (the menu bar, the Dock, menus, panels, notifications, the widgets)
/// bend what's behind them as thick glass does, with a rim that catches the light from the top
/// left (niri draws it: desktop/niri/23, docs/glass.md). Frosted glass is how much what's behind
/// is blurred; this is how much the edges bend it. Both: frosted glass with clear, bent edges;
/// this alone: clear glass, what's behind sharp, the surfaces' tint over it for the words.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[allow(dead_code)]
pub enum Lens {
    #[default]
    Off,
    Subtle,
    Medium,
    Strong,
}

#[allow(dead_code)]
impl Lens {
    pub const ALL: [Lens; 4] = [Lens::Off, Lens::Subtle, Lens::Medium, Lens::Strong];

    /// As the file has it (`lens`: off, subtle, medium, strong; Off with none). Off whatever
    /// it says with Reduce transparency or high contrast.
    pub fn read(lens: Option<&str>, reduce: bool, high_contrast: bool) -> Lens {
        if reduce || high_contrast {
            return Lens::Off;
        }
        lens.and_then(Lens::of_word).unwrap_or_default()
    }

    pub fn of_word(word: &str) -> Option<Lens> {
        Lens::ALL.into_iter().find(|l| l.word() == word)
    }

    pub fn word(self) -> &'static str {
        match self {
            Lens::Off => "off",
            Lens::Subtle => "subtle",
            Lens::Medium => "medium",
            Lens::Strong => "strong",
        }
    }

    pub fn on(self) -> bool {
        self != Lens::Off
    }

    /// How strongly niri bends the edges (its `glass`, 0 to 1): how wide the rounded rim is,
    /// how far in it draws from, how bright it's lit.
    pub fn strength(self) -> f32 {
        match self {
            Lens::Off => 0.,
            Lens::Subtle => 0.4,
            Lens::Medium => 0.7,
            Lens::Strong => 1.,
        }
    }

    /// niri's blur with Glass alone (its passes, offset and saturation): only softened, as a
    /// Mac's clear glass is, so the bent edges show and what's behind stays plain to see.
    pub fn blur(self) -> (u8, f32, f32) {
        (1, 1.0, 1.2)
    }

    /// How thick the desktop's surfaces are with this and Frosted glass as `frost` is: as
    /// Frosted glass has it; with that Off, as clear as this is strong, at Frosted glass's
    /// tint for the same step (whose text keeps its contrast over anything behind: the tests
    /// below), so the stronger the glass, the clearer.
    pub fn material(self, frost: Glass) -> Glass {
        match (frost, self) {
            (frost, _) if frost.on() => frost,
            (_, Lens::Off) => Glass::Off,
            (_, Lens::Subtle) => Glass::Subtle,
            (_, Lens::Medium) => Glass::Medium,
            (_, Lens::Strong) => Glass::Strong,
        }
    }
}

/// How frosted this program's surfaces are now: Off until it's told (`set_glass`), so what
/// never reads the file (the installer) stays solid.
static GLASS: AtomicU8 = AtomicU8::new(0);

#[allow(dead_code)]
pub fn set_glass(glass: Glass) {
    GLASS.store(Glass::ALL.iter().position(|g| *g == glass).unwrap_or(0) as u8, Ordering::Relaxed);
}

#[allow(dead_code)]
pub fn glass() -> Glass {
    Glass::ALL.get(GLASS.load(Ordering::Relaxed) as usize).copied().unwrap_or(Glass::Off)
}

/// Whether what's drawn here lets what's behind through, blurred (vibrancy): with Frosted glass
/// on, in namiOS's desktop, where niri blurs behind it (desktop/settings.rs). Otherwise solid.
#[allow(dead_code)]
pub fn vibrant() -> bool {
    glass().on() && in_namios()
}

fn in_namios() -> bool {
    static IN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *IN.get_or_init(|| std::env::var_os("NIRI_SOCKET").is_some() && on_namios())
}

/// The window as it is on another distribution (Ubuntu, Fedora...), on namiOS: NAMI_GUI_AS=other-linux
/// (`nami-gui --as-other-linux`, the menu's "nami (as on Other Distributions)"). To see it as it is
/// there: namiOS's own pages and look aren't, and what would only be done there (making a USB
/// drive) says it's a preview instead.
#[allow(dead_code)]
pub fn as_other_linux() -> bool {
    static AS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AS.get_or_init(|| as_other_linux_said(std::env::var("NAMI_GUI_AS").ok().as_deref(), std::env::args().any(|a| a == "--as-other-linux")))
}

fn as_other_linux_said(env: Option<&str>, flag: bool) -> bool {
    flag || env.is_some_and(|v| matches!(v.trim(), "other-linux" | "other"))
}

/// Whether this is namiOS (not as another Linux has it, for a preview).
#[allow(dead_code)]
pub fn on_namios() -> bool {
    !as_other_linux() && std::path::Path::new("/etc/namios").exists()
}

/// A pane's colour (`base`, one of `color`'s) as thick as Frosted glass has it now: solid
/// where nothing's blurred behind.
#[allow(dead_code)]
pub fn frosted(base: u32, pane: Pane) -> gpui::Rgba {
    let glass = if in_namios() { glass() } else { Glass::Off };
    gpui::rgba((base << 8) | glass.alpha(pane, color::DARK.load(Ordering::Relaxed)))
}

/// A sidebar's ground: the glass, or solid.
#[allow(dead_code)]
pub fn sidebar_fill(vibrant: bool) -> gpui::Rgba {
    if vibrant { color::sidebar_glass() } else { rgb(color::ground()) }
}

/// A rectangle with round corners, in strips, for a surface's input region: niri blurs a
/// namiOS surface only where it takes the pointer (desktop/niri/03), so the blur is as round
/// as what's drawn over it. `bounds` as laid out (in the text size's pixels already).
#[allow(dead_code)]
pub fn rounded_region(bounds: gpui::Bounds<Pixels>, radius: f32) -> Vec<gpui::Bounds<Pixels>> {
    use gpui::{point, px, size, Bounds};
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let r = (radius * scale()).min(w / 2.).min(h / 2.);
    let mut strips = vec![Bounds::new(point(px(x), px(y + r)), size(px(w), px((h - 2. * r).max(0.))))];
    for row in 0..r.ceil() as i32 {
        let across = r - row as f32 - 0.5;
        let inset = r - (r * r - across * across).max(0.).sqrt();
        let width = px((w - 2. * inset).max(0.));
        strips.push(Bounds::new(point(px(x + inset), px(y + row as f32)), size(width, px(1.))));
        strips.push(Bounds::new(point(px(x + inset), px(y + h - row as f32 - 1.)), size(width, px(1.))));
    }
    strips
}

/// A card: a plain panel, rounded, with its soft shadow and no more than a hairline.
#[allow(dead_code)]
pub fn card() -> Div {
    div().rounded(px(shape::CARD)).bg(rgb(color::panel())).border_1().border_color(rgb(color::card_line())).shadow(soft_shadow())
}

/// A sidebar's line: picked or not. Picked, as a Mac's is in its accent: the key's strong slate,
/// its words white (`on_key_strong`, and its icon, the caller's: `side_icon`). Its icon and label
/// are the caller's.
#[allow(dead_code)]
pub fn side_row(id: impl Into<ElementId>, picked: bool) -> Stateful<Div> {
    let row = div()
        .id(id)
        .flex_none()
        .h(px(28.))
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(10.))
        .rounded(px(shape::ROW))
        .text_size(px(text::BODY));
    if picked {
        row.bg(rgb(color::key_strong())).text_color(rgb(color::on_key_strong())).font_weight(FontWeight::MEDIUM).press()
    } else {
        row.hover(|s| s.bg(color::hover_pill())).press_row()
    }
}

/// The colour of a sidebar line's icon: white on the picked one, quiet otherwise, faint where
/// it can't be used (`on`).
#[allow(dead_code)]
pub fn side_icon(picked: bool, on: bool) -> u32 {
    if !on { color::faint() } else if picked { color::on_key_strong() } else { color::side_muted() }
}

/// A sidebar's heading, sentence case ("THIS COMPUTER" is "This computer"): small and grey.
#[allow(dead_code)]
pub fn side_heading(title: &str) -> Div {
    let low = title.to_lowercase();
    let said = match low.char_indices().nth(1) {
        Some((at, _)) => low[..at].to_uppercase() + &low[at..],
        None => low.to_uppercase(),
    };
    div().flex_none().px(px(10.)).pt(px(14.)).pb(px(4.)).text_size(px(text::META)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(color::side_muted())).child(said)
}

/// The heading over a group in a page (a group of settings, a list, a row of choices): sentence
/// case, small, semibold and quiet, in line with the group's words, so the group is what's read.
#[allow(dead_code)]
pub fn group_heading(title: &str) -> Div {
    div().px(px(shape::MODULE_INSET)).text_size(px(text::CALLOUT)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(color::grouped_muted())).child(title.to_string())
}

/// What's said under a group, as a Mac's footnotes are: small and quiet, in line with its words.
#[allow(dead_code)]
pub fn footnote(said: impl Into<SharedString>) -> Div {
    div().px(px(shape::MODULE_INSET)).text_size(px(text::META)).line_height(gpui::relative(text::leading(text::META))).text_color(rgb(color::grouped_muted())).child(said.into())
}

/// The search field at the top of a sidebar, as a Mac's System Settings has it: its glass, the
/// field, and its shortcut at the right, quiet, in a well of the text's colour (see-through on
/// the sidebar's glass).
#[allow(dead_code)]
pub fn search_pill(glass: impl IntoElement, field: impl IntoElement, shortcut: &str) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .w_full()
        .h(px(shape::PUSH))
        .pl(px(8.))
        .pr(px(8.))
        .rounded(px(shape::ROW))
        .bg(color::push())
        .child(glass)
        .child(div().flex_1().min_w_0().child(field))
        .child(div().flex_none().text_size(px(text::CAPTION)).text_color(rgb(color::side_muted())).child(shortcut.to_string()))
}

/// Soft pill tabs, for a page's secondary choices: the picked one filled softly.
#[allow(dead_code)]
pub fn tab(id: impl Into<ElementId>, label: impl Into<SharedString>, picked: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(30.))
        .px(px(12.))
        .flex()
        .items_center()
        .rounded_full()
        .text_size(px(text::CALLOUT))
        .cursor_pointer()
        .text_color(rgb(if picked { color::text() } else { color::muted() }))
        .when(picked, |d| d.bg(rgb(color::soft())).font_weight(FontWeight::MEDIUM))
        .when(!picked, |d| d.hover(|s| s.text_color(rgb(color::text()))))
        .press_row()
        .child(label.into())
}

/// A button: the page's main one (`primary`), or a quieter one.
/// Whether windows float, as `/etc/namios/windows` set them.
#[allow(dead_code)]
pub fn windows_float() -> bool {
    windows_mode() == "floating"
}

/// Whether this program's window floats, as niri last said (`follow_floating`): 0 not said yet,
/// 1 no, 2 yes.
static FLOATING: AtomicU8 = AtomicU8::new(0);

/// Whether this program's window floats: niri's word, once it's said it. Till then, under niri,
/// it does (namios-look opens Settings and System Monitor floating, however windows go); and
/// on another desktop (GNOME, KDE Plasma) windows float, so it has all three buttons there.
#[allow(dead_code)]
pub fn window_floats() -> bool {
    FLOATING.load(Ordering::Relaxed) != 1
}

/// niri's windows (its `Windows` answer): whether this program's of `app_id` floats, kept for
/// its buttons. Whether that changed.
#[allow(dead_code)]
pub fn follow_floating(windows: &serde_json::Value, app_id: &str) -> bool {
    let me = std::process::id() as u64;
    let Some(window) = windows.as_array().into_iter().flatten().find(|w| w["pid"].as_u64() == Some(me) && w["app_id"] == app_id) else { return false };
    let now = if window["is_floating"] == true { 2 } else { 1 };
    FLOATING.swap(now, Ordering::Relaxed) != now
}

/// How windows go, as `/etc/namios/windows` set them: "scrolling" (the start), "tiling" or
/// "floating". Its file includes Tiling's or Floating's; there's none for Scrolling.
#[allow(dead_code)]
pub fn windows_mode() -> &'static str {
    let Some(home) = std::env::var_os("HOME") else { return "scrolling" };
    windows_mode_of(std::fs::read_to_string(std::path::Path::new(&home).join(".local/state/nami/windows.kdl")).ok().as_deref())
}

/// The way windows go, by the file `/etc/namios/windows` keeps it in (None: there's none).
#[allow(dead_code)]
fn windows_mode_of(file: Option<&str>) -> &'static str {
    match file {
        None => "scrolling",
        Some(file) if file.contains("windows-tiling") => "tiling",
        Some(_) => "floating",
    }
}

/// The three ways windows can go, as `/etc/namios/windows` takes them: its word, what it's
/// called, and its icon. (Settings' choice and Control Center's are these.)
#[allow(dead_code)]
pub const WINDOWS_MODES: [(&str, &str, &str); 3] = [("scrolling", "Scrolling", "windows-scrolling"), ("tiling", "Tiling", "windows-tiling"), ("floating", "Floating", "windows-floating")];

/// namiOS's switch for how windows go (namios-windows), when it's there.
#[allow(dead_code)]
pub fn windows_switch() -> Option<String> {
    if as_other_linux() {
        return None;
    }
    Some(std::env::var("NAMI_WINDOWS").unwrap_or_else(|_| "/etc/namios/windows".into())).filter(|f| std::path::Path::new(f).is_file())
}

/// Windows to go `mode`'s way, as the keyboard shortcut switches them: by namiOS's switch
/// (`script`, `windows_switch`). What it said, if it couldn't. (It waits for the switch, which
/// moves the windows there are: away from a window that has to keep drawing.)
#[allow(dead_code)]
pub fn switch_windows(script: &str, mode: &str) -> Result<(), String> {
    match std::process::Command::new(script).arg(mode).output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).into_owned()),
        Err(e) => Err(format!("{script}: {e}")),
    }
}

/// A window's own buttons, as GTK's are in namiOS (namios-windows): small circles in slate,
/// darker on hover, fainter while the window isn't `active` (the one in front). Minimize and
/// zoom too when the window floats, or is `zoomed` (maximized: zoom puts it back); tiled, only
/// close.
/// With the pointer over any of them, as on a Mac, each shows its mark (×, −, +) and they're all
/// solid: the one under the pointer in its hover colour, the others a light slate. (The marks
/// are dark slate on the light slate, light on the darker slate.)
/// A window button's mark, in `colour` while `group` is hovered: over the whole circle (the mark's
/// 14 across too, icons::WINDOW_MARKS), so it's on the circle's pixels and in its middle.
fn mark(id: &'static str, group: &'static str, colour: u32) -> gpui::Svg {
    gpui::svg()
        .path(format!("icons/{id}.svg"))
        .absolute()
        .top_0()
        .left_0()
        .size(px(14.))
        .text_color(gpui::rgba(0))
        .group_hover(group, move |s| s.text_color(rgb(colour)))
}

#[allow(dead_code)]
pub fn window_buttons(zoomed: bool, active: bool) -> Div {
    use crate::tip::Tip;
    const GROUP: &str = "window-buttons";
    let rest_bg = if active { 0x64708a8c } else { 0x64708a40 };
    let circle = |id: &'static str, hover: u32| {
        let light_mark = hover == 0x45577a;
        div()
            .id(id)
            .group(id)
            .size(px(14.))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(gpui::rgba(rest_bg))
            .group_hover(GROUP, |s| s.bg(rgb(0x9aa3b5)))
            .hover(move |s| s.bg(rgb(hover)))
            .active(|s| s.bg(rgb(0x2a3650)))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // (An element answers to one group: a dark mark that shows with the buttons', and
            // for minimize and zoom a light one over it with their own.)
            .relative()
            .child(mark(id, GROUP, 0x1d2330))
            .when(light_mark, |d| d.child(mark(id, id, 0xf4f5f8)))
    };
    let floats = window_floats() || zoomed;
    // (Alone, a plain circle would look like something missing: close shows its mark at rest.)
    let rest = (!floats).then(|| {
        let dark = color::DARK.load(std::sync::atomic::Ordering::Relaxed);
        gpui::svg()
            .path("icons/window-close.svg")
            .absolute()
            .top_0()
            .left_0()
            .size(px(14.))
            .text_color(gpui::rgba(if dark { 0xe6e9efd0 } else { 0x1d2330c0 }))
            .group_hover(GROUP, |s| s.text_color(gpui::rgba(0)))
    });
    div()
        .group(GROUP)
        .flex()
        .flex_none()
        .gap(px(6.))
        .child(circle("window-close", 0x45577a).children(rest).tip("Close").on_click(|_, window, _| window.remove_window()))
        .when(floats, |d| {
            d.child(circle("window-minimize", 0x45577a).tip("Minimize").on_click(|_, window, _| window.minimize_window()))
                .child(circle("window-zoom", 0x45577a).tip(if zoomed { "Restore" } else { "Zoom" }).on_click(|_, window, _| window.zoom_window()))
        })
}

/// Whether this program's windows are framed by it, not by the desktop: everywhere but under
/// niri (on GNOME, KDE Plasma and the rest), where a window is drawn as namiOS draws it, round,
/// with its shadow and its own buttons, and the desktop's title bar would be one too many.
/// Never on a Mac: macOS frames it, with its own buttons (`MAC_BUTTONS`).
#[allow(dead_code)]
pub fn own_frame() -> bool {
    !MAC && std::env::var_os("NIRI_SOCKET").is_none()
}

/// nami on a Mac: macOS's menu bar, window buttons and keys (mac.rs), and only what nami does
/// on a Mac (the command line's packages, user.nami and projects: docs/macos.md).
#[allow(dead_code)]
pub const MAC: bool = cfg!(target_os = "macos");

/// Where macOS's window buttons are, from the window's top left: on the sidebar's first row,
/// where nami's own are elsewhere (`window_controls`).
#[allow(dead_code)]
pub const MAC_BUTTONS: (f32, f32) = (16., 16.);

/// The window's buttons, at the sidebar's top (or the toolbar's, with the sidebar folded): nami's
/// own; on a Mac, the room for macOS's, which macOS draws there (at `MAC_BUTTONS`).
#[allow(dead_code)]
pub fn window_controls(zoomed: bool, active: bool) -> Div {
    if MAC {
        // (Three buttons of 14 with 6 between, and a little room after them.)
        return div().flex_none().w(px(58.)).h(px(14.));
    }
    window_buttons(zoomed, active)
}

/// The decorations to ask for: the window's own, where it frames itself.
#[allow(dead_code)]
pub fn decorations() -> Option<gpui::WindowDecorations> {
    own_frame().then_some(gpui::WindowDecorations::Client)
}

/// The window's app id and title: Settings in namiOS (niri's rules know it by them); elsewhere
/// nami, next to the desktop's own Settings (nami.desktop has its name and picture).
#[allow(dead_code)]
pub fn app_id() -> &'static str {
    if own_frame() { "nami" } else { "nami-gui" }
}

/// A framed window's corners, as round as niri rounds namiOS's (namios-look: 12).
pub const FRAME_ROUND: f32 = 12.;
/// The room around a framed window for its shadow, and to take hold of its edges.
pub const FRAME_INSET: f32 = 26.;

thread_local! {
    /// How round the window drawn now is at its corners (0 where it's square: the desktop's
    /// frame, or tiled, or zoomed), for what's drawn at its edges (`frame_round`).
    static ROUND: std::cell::Cell<f32> = const { std::cell::Cell::new(0.) };
    /// Pressed where the window's moved from, not moved yet: a double-click zooms instead.
    static MOVE_FROM: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// How round the window drawn now is at its corners: what touches them is rounded as much
/// (a framed window's sidebar and page), since nothing clips them to its shape.
#[allow(dead_code)]
pub fn frame_round() -> Pixels {
    gpui::px(ROUND.with(|r| r.get()))
}

/// Where the window's moved from, as a Mac's title bar: dragged, it moves; double-clicked, it
/// zooms. (The move starts once the pointer goes, so the second click of two is still heard.)
#[allow(dead_code)]
pub fn moves_window<E: InteractiveElement>(e: E) -> E {
    e.on_mouse_down(gpui::MouseButton::Left, |ev, window, _| {
        if ev.click_count == 2 {
            MOVE_FROM.with(|m| m.set(false));
            // (A Mac's title bar does as its settings say: zoom, minimize, or nothing.)
            if MAC { window.titlebar_double_click() } else { window.zoom_window() }
        } else {
            MOVE_FROM.with(|m| m.set(true));
        }
    })
    .on_mouse_move(|ev, window, _| {
        if ev.pressed_button == Some(gpui::MouseButton::Left) && MOVE_FROM.with(|m| m.replace(false)) {
            window.start_window_move();
        }
    })
    .on_mouse_up(gpui::MouseButton::Left, |_, _, _| MOVE_FROM.with(|m| m.set(false)))
}

/// Which edge or corner of a window `size` big the pointer at `at` is on, `inset` from it.
fn resize_edge(at: gpui::Point<Pixels>, inset: Pixels, size: gpui::Size<Pixels>, tiling: gpui::Tiling) -> Option<gpui::ResizeEdge> {
    use gpui::ResizeEdge::*;
    // (Corners take a little more room, as a desktop's frame gives them.)
    let corner = inset + gpui::px(10.);
    let (left, right) = (!tiling.left && at.x < inset, !tiling.right && at.x > size.width - inset);
    let (top, bottom) = (!tiling.top && at.y < inset, !tiling.bottom && at.y > size.height - inset);
    let (near_left, near_right) = (at.x < corner, at.x > size.width - corner);
    let (near_top, near_bottom) = (at.y < corner, at.y > size.height - corner);
    Some(match () {
        _ if (top && near_left) || (left && near_top) => TopLeft,
        _ if (top && near_right) || (right && near_top) => TopRight,
        _ if (bottom && near_left) || (left && near_bottom) => BottomLeft,
        _ if (bottom && near_right) || (right && near_bottom) => BottomRight,
        _ if top => Top,
        _ if bottom => Bottom,
        _ if left => Left,
        _ if right => Right,
        _ => return None,
    })
}

/// A window as namiOS draws one, where it frames itself (`own_frame`): `content` rounded, on
/// its shadow, with a hairline round it, and room about it to take hold of its edges and
/// corners. Square, without the room, where the desktop's tiled or zoomed it; as it is where
/// the desktop frames it (niri). `content` is rounded where it touches the corners already
/// (`frame_round`, which this sets first: call it before drawing `content`).
#[allow(dead_code)]
pub fn frame(window: &mut gpui::Window, content: impl FnOnce(&mut gpui::Window) -> gpui::AnyElement) -> gpui::AnyElement {
    let gpui::Decorations::Client { tiling } = window.window_decorations() else {
        ROUND.with(|r| r.set(0.));
        return content(window);
    };
    let zoomed = window.is_maximized() || window.is_fullscreen();
    let alone = !tiling.is_tiled() && !zoomed;
    let inset = if zoomed { gpui::px(0.) } else { gpui::px(FRAME_INSET) };
    window.set_client_inset(inset);
    let round = if alone { FRAME_ROUND } else { 0. };
    ROUND.with(|r| r.set(round));
    let content = content(window);
    let r = gpui::px(round);
    let active = window.is_window_active();
    let dark = color::DARK.load(Ordering::Relaxed);
    // (As niri's for namiOS's windows: soft, a little lower than the window; lighter for one
    // that's behind. A hairline keeps its edge where the shadow's lost in a dark desktop.)
    let shadow = gpui::BoxShadow {
        color: gpui::hsla(0., 0., 0., match (active, dark) {
            (true, false) => 0.30,
            (false, false) => 0.16,
            (true, true) => 0.55,
            (false, true) => 0.35,
        }),
        offset: gpui::point(gpui::px(0.), gpui::px(6.)),
        blur_radius: gpui::px(22.),
        spread_radius: gpui::px(0.),
        inset: false,
    };
    let edge = if dark { gpui::rgba(0xffffff1f) } else { gpui::rgba(0x0000002e) };
    let side = |tiled: bool| if tiled || zoomed { gpui::px(0.) } else { inset };
    div()
        .id("window-frame")
        .size_full()
        .pt(side(tiling.top))
        .pb(side(tiling.bottom))
        .pl(side(tiling.left))
        .pr(side(tiling.right))
        // The pointer's look over the edges: a resize arrow.
        .child(
            canvas(
                |_, window, _| {
                    let size = window.window_bounds().get_bounds().size;
                    window.insert_hitbox(gpui::Bounds::new(gpui::point(gpui::px(0.), gpui::px(0.)), size), gpui::HitboxBehavior::Normal)
                },
                move |_, hitbox, window, _| {
                    let size = window.window_bounds().get_bounds().size;
                    let Some(edge) = resize_edge(window.mouse_position(), inset, size, tiling) else { return };
                    use gpui::ResizeEdge::*;
                    window.set_cursor_style(
                        match edge {
                            Top | Bottom => gpui::CursorStyle::ResizeUpDown,
                            Left | Right => gpui::CursorStyle::ResizeLeftRight,
                            TopLeft | BottomRight => gpui::CursorStyle::ResizeUpLeftDownRight,
                            TopRight | BottomLeft => gpui::CursorStyle::ResizeUpRightDownLeft,
                        },
                        &hitbox,
                    );
                },
            )
            .size_full()
            .absolute(),
        )
        .on_mouse_move(|_, window, _| window.refresh())
        .on_mouse_down(gpui::MouseButton::Left, move |e, window, _| {
            let size = window.window_bounds().get_bounds().size;
            if let Some(edge) = resize_edge(e.position, inset, size, tiling) {
                window.start_window_resize(edge);
            }
        })
        .child(
            div()
                .size_full()
                .rounded(r)
                .when(alone, |d| d.shadow(vec![shadow]))
                .child(div().size_full().rounded(r).overflow_hidden().child(content))
                // (Over what's in it, so a page's own ground doesn't hide it.)
                .when(!zoomed, |d| d.child(div().absolute().inset_0().rounded(r).border_1().border_color(edge))),
        )
        .into_any_element()
}

pub fn button(id: &'static str, label: impl Into<SharedString>, primary: bool, enabled: bool) -> Stateful<Div> {
    let b = div()
        .id(id)
        .px(px(16.))
        .h(px(shape::BUTTON))
        .flex()
        .items_center()
        .rounded(px(shape::CONTROL))
        .text_size(px(text::BODY))
        .font_weight(FontWeight::MEDIUM)
        .child(label.into());
    let b = if primary {
        b.bg(rgb(color::fill())).text_color(rgb(color::on_text()))
    } else {
        b.border_1().border_color(rgb(color::line())).bg(rgb(color::panel())).text_color(rgb(color::text()))
    };
    if enabled { b.cursor_pointer().hover(|s| s.opacity(0.85)).press() } else { b.opacity(0.4) }
}

/// A button that takes something away for good (Erase): `bad()`'s red, as a Mac's destructive
/// one, next to a Cancel that has the keyboard.
#[allow(dead_code)]
pub fn destructive_button(id: &'static str, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(16.))
        .h(px(shape::BUTTON))
        .flex()
        .items_center()
        .rounded(px(shape::CONTROL))
        .text_size(px(text::BODY))
        .font_weight(FontWeight::MEDIUM)
        .bg(rgb(color::bad()))
        .text_color(rgb(color::on_bad()))
        .cursor_pointer()
        .hover(|s| s.opacity(0.85))
        .press()
        .child(label.into())
}

/// One of a few to pick from: a title, a line about it, and whether it's picked.
#[allow(dead_code)]
pub fn choice(id: impl Into<gpui::ElementId>, title: impl Into<SharedString>, about: impl Into<SharedString>, picked: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .gap(px(12.))
        .items_start()
        .p(px(12.))
        .rounded(px(crate::look::shape::CONTROL))
        .border_1()
        .border_color(rgb(if picked { color::text() } else { color::line() }))
        .bg(if picked { rgb(color::soft()) } else { rgb(color::panel()) })
        .cursor_pointer()
        .hover(|s| s.bg(rgb(color::soft())))
        .press_row()
        .child(
            div()
                .mt(px(2.))
                .size(px(16.))
                .flex_none()
                .rounded_full()
                .border_1()
                .border_color(rgb(if picked { color::text() } else { color::faint() }))
                .flex()
                .items_center()
                .justify_center()
                .when(picked, |d| d.child(div().size(px(8.)).rounded_full().bg(rgb(color::text())))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .flex_1()
                .min_w_0()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(title.into()))
                .child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(about.into())),
        )
}

/// A step's heading and the line under it.
pub fn heading(title: &str, line: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(div().type_size(text::TITLE).font_weight(FontWeight::BOLD).child(title.to_string()))
        .child(div().text_size(px(text::BODY)).text_color(rgb(color::muted())).child(line.to_string()))
}

/// A small pill: picked or not.
pub fn pill(id: ElementId, text: impl Into<SharedString>, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(12.))
        .h(px(30.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded_full()
        .border_1()
        .border_color(rgb(if on { color::fill() } else { color::line() }))
        .bg(if on { rgb(color::fill()) } else { rgb(color::panel()) })
        .text_color(if on { rgb(color::on_text()) } else { rgb(color::text()) })
        .text_size(px(text::CALLOUT))
        .cursor_pointer()
        .press()
        .child(text.into())
}


/// A box that's ticked or not.
#[allow(dead_code)]
pub fn tick(id: ElementId, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(20.))
        .flex_none()
        .rounded(px(crate::look::shape::SMALL))
        .border_1()
        .border_color(rgb(if on { color::fill() } else { color::faint() }))
        .bg(if on { rgb(color::fill()) } else { rgb(color::panel()) })
        .text_color(rgb(color::on_text()))
        .text_size(px(text::CALLOUT))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .press()
        .child(if on { "✓" } else { "" })
}

thread_local! {
    /// Where each control `placed` names was last drawn: its middle, its size, and how tall its
    /// window is.
    static PLACES: RefCell<std::collections::BTreeMap<&'static str, [i32; 5]>> = RefCell::new(Default::default());
}

/// Where a control is drawn, for tests/vm-quick.sh to press it by its name and not by a place
/// a redesign moves: `$XDG_RUNTIME_DIR/<program>-places`, a line each, `<name> <x> <y> <w> <h>
/// <window's height>`: its middle, in points from its window's corner (where a window is, niri
/// says; a layer of the desktop's not as tall as the screen is from under the bar), and
/// its size (a slider's quarter along is `x - w/4`). A child of what it names, which is
/// `relative`: as big as that, it draws nothing.
#[allow(dead_code)]
pub fn placed(name: &'static str) -> impl IntoElement {
    canvas(
        move |bounds, window, _| {
            let at = [bounds.center().x, bounds.center().y, bounds.size.width, bounds.size.height, window.viewport_size().height].map(|n| f32::from(n).round() as i32);
            PLACES.with(|p| {
                let mut p = p.borrow_mut();
                if p.insert(name, at) == Some(at) {
                    return;
                }
                let text: String = p.iter().map(|(n, [x, y, w, h, tall])| format!("{n} {x} {y} {w} {h} {tall}\n")).collect();
                let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") else { return };
                let program = std::env::current_exe().ok().and_then(|e| e.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
                let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{program}-places")), text);
            });
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_other_linux_only_when_asked() {
        assert!(as_other_linux_said(Some("other-linux"), false));
        assert!(as_other_linux_said(Some(" other "), false));
        assert!(as_other_linux_said(None, true));
        assert!(!as_other_linux_said(None, false));
        assert!(!as_other_linux_said(Some(""), false));
        assert!(!as_other_linux_said(Some("namios"), false));
    }

    #[test]
    fn the_windows_way_as_its_file_keeps_it() {
        // No file: Scrolling, where a computer starts; Tiling's includes its own; any other, Floating's.
        assert_eq!(windows_mode_of(None), "scrolling");
        assert_eq!(windows_mode_of(Some("include \"/etc/namios/windows-tiling.kdl\"\n")), "tiling");
        assert_eq!(windows_mode_of(Some("include \"/etc/namios/windows-floating.kdl\"\n")), "floating");
        // Each way's word is one the switch takes, once. (Their icons: icons.rs.)
        let words: Vec<_> = WINDOWS_MODES.iter().map(|(w, _, _)| *w).collect();
        assert_eq!(words, ["scrolling", "tiling", "floating"]);
    }

    /// WCAG's contrast ratio of a colour and one as floats.
    fn ratio(a: u32, b: [f64; 3]) -> f64 {
        let channel = |v: f64| {
            let v = v / 255.;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        let lum = |c: [f64; 3]| 0.2126 * channel(c[0]) + 0.7152 * channel(c[1]) + 0.0722 * channel(c[2]);
        let a = lum([f64::from(a >> 16), f64::from((a >> 8) & 255), f64::from(a & 255)]);
        let b = lum(b);
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// `colour` at `alpha` over a grey `behind`.
    fn over(colour: u32, alpha: u32, behind: f64) -> [f64; 3] {
        let a = f64::from(alpha) / 255.;
        [colour >> 16, (colour >> 8) & 255, colour & 255].map(|c| f64::from(c) * a + behind * (1. - a))
    }

    #[test]
    fn frosted_text_reads_over_anything() {
        // namiOS's own colours, light's and dark's: panel, ground, text, muted, side_muted.
        for (dark, panel, ground, text, muted, side_muted) in [(false, 0xffffff, 0xfafafa, 0x18181b, 0x71717a, 0x52525b), (true, 0x191d26, 0x13161d, 0xf2f3f6, 0x9aa2b2, 0xb4bac7)] {
            for glass in Glass::ALL {
                // (Light panes are worst over black, dark ones over white. Blurred, what's behind
                // is rarely either: words read at 4.5:1 over anything from dark grey to light
                // grey, and at 3:1 at the very least over black or white.)
                for behind in [0., 64., 128., 192., 255.] {
                    let least = if behind == 0. || behind == 255. { 3. } else { 4.5 };
                    for pane in [Pane::Bar, Pane::Panel, Pane::Scrim, Pane::Widget] {
                        let r = ratio(text, over(panel, glass.alpha(pane, dark), behind));
                        assert!(r >= least, "{glass:?} {pane:?} dark={dark} over {behind}: {r:.1}");
                    }
                    let side = over(ground, glass.alpha(Pane::Sidebar, dark), behind);
                    assert!(ratio(text, side) >= least, "{glass:?} sidebar dark={dark} over {behind}");
                }
                // The quiet words over a middle grey, but for Strong's (see-through, as it says).
                if glass != Glass::Strong {
                    let r = ratio(muted, over(panel, glass.alpha(Pane::Panel, dark), 128.));
                    assert!(r >= 3., "{glass:?} muted dark={dark}: {r:.1}");
                    assert!(ratio(side_muted, over(ground, glass.alpha(Pane::Sidebar, dark), 128.)) >= 3., "{glass:?} sidebar muted dark={dark}");
                }
            }
        }
    }

    #[test]
    fn glass_is_read_on_its_own_and_combines_with_frost() {
        assert_eq!(Lens::read(None, false, false), Lens::Off);
        assert_eq!(Lens::read(Some("strong"), false, false), Lens::Strong);
        assert_eq!(Lens::read(Some("thick"), false, false), Lens::Off);
        // Solid surfaces have no glass.
        assert_eq!(Lens::read(Some("strong"), true, false), Lens::Off);
        assert_eq!(Lens::read(Some("strong"), false, true), Lens::Off);
        for lens in Lens::ALL {
            assert_eq!(Lens::of_word(lens.word()), Some(lens));
        }
        // Each step bends more than the one before.
        assert!(Lens::ALL.windows(2).all(|w| w[0].strength() < w[1].strength()));
        assert!(Lens::ALL.iter().all(|l| (0. ..=1.).contains(&l.strength())));
        // With frost, frost's tint; without, the same step's; neither, solid.
        for frost in Glass::ALL.into_iter().filter(|g| g.on()) {
            for lens in Lens::ALL {
                assert_eq!(lens.material(frost), frost);
            }
        }
        assert_eq!(Lens::Off.material(Glass::Off), Glass::Off);
        assert_eq!(Lens::Subtle.material(Glass::Off), Glass::Subtle);
        assert_eq!(Lens::Strong.material(Glass::Off), Glass::Strong);
    }

    #[test]
    fn quiet_words_on_glass_read_at_every_step() {
        // (muted() on glass, light and dark, over a panel or a widget: 3:1 at every step, Strong
        // too, where the solid one isn't.)
        for (dark, panel, glass_muted) in [(false, 0xffffff, 0x52525b), (true, 0x191d26, 0xbcc2ce)] {
            for glass in Glass::ALL {
                for pane in [Pane::Bar, Pane::Panel, Pane::Widget] {
                    for behind in [64., 128., 192.] {
                        let r = ratio(glass_muted, over(panel, glass.alpha(pane, dark), behind));
                        assert!(r >= 3., "{glass:?} {pane:?} dark={dark} over {behind}: {r:.1}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_type_scale_goes_down_a_step_at_a_time() {
        use text::*;
        let steps = [DISPLAY, TITLE, DIALOG, HEADLINE, SECTION, BODY, CALLOUT, META, CAPTION];
        assert!(steps.windows(2).all(|w| w[0] > w[1]), "{steps:?}");
        // (Below a headline, a pixel apart: a step that's half one is a size between two.)
        assert!(steps[3..].windows(2).all(|w| w[0] - w[1] >= 1.), "{steps:?}");
    }

    #[test]
    fn what_floats_is_rounder_than_what_it_holds() {
        use shape::*;
        assert!(FLOATING > PANEL && PANEL >= CARD && CARD > CONTROL && CONTROL > ROW && ROW > SMALL);
        assert!(BUTTON < BUTTON_LARGE);
        // (Control Center round about its modules, as a Mac's is.)
        assert!(CONTROLS > MODULE && CONTROLS > FLOATING);
    }

    #[test]
    fn control_centers_modules_read_over_anything() {
        // Its ground, a module on it (white, 72% or 6%), and the text and its quieter words on
        // that, light's and dark's, as `frosted_text_reads_over_anything` has it for a panel.
        for (dark, ground, module, text, muted) in [(false, 0xe6e8ec, 0xb8, 0x18181b, 0x52525b), (true, 0x161920, 0x10, 0xf2f3f6, 0xbcc2ce)] {
            for glass in Glass::ALL {
                for behind in [0., 64., 128., 192., 255.] {
                    let least = if behind == 0. || behind == 255. { 3. } else { 4.5 };
                    let panel = over(ground, glass.alpha(Pane::Controls, dark), behind);
                    let a = f64::from(module) / 255.;
                    let on = |c: [f64; 3]| c.map(|v| 255. * a + v * (1. - a));
                    let r = (ratio(text, panel), ratio(text, on(panel)));
                    assert!(r.0 >= least && r.1 >= least, "{glass:?} dark={dark} over {behind}: {r:.1?}");
                    if behind == 128. {
                        assert!(ratio(muted, on(panel)) >= 3., "{glass:?} muted dark={dark}");
                    }
                }
            }
        }
        // White on what's on, light and dark; the level's glyph on its fill.
        for key_strong in [0x45577a, 0x5a72a3] {
            assert!(ratio(0xffffff, over(key_strong, 255, 0.)) >= 4.5, "{key_strong:06x}");
        }
        for (ink, fill) in [(0xd4d8e0, 0x323a4b), (0x5d6370, 0xe4e7ec)] {
            assert!(ratio(ink, over(fill, 255, 0.)) >= 4.5, "{ink:06x} on {fill:06x}");
        }
    }

    #[test]
    fn the_bar_panels_read_over_anything() {
        // The calendar, the notifications and the meters are on Control Center's ground, the bar's
        // menus on a panel's: what's said on the ground itself (a date, a heading, a foot's way
        // on) and a row under the pointer (`key_wash`) on either, a notification's button on a
        // banner, at every step, light and dark; as `control_centers_modules_read_over_anything` has it.
        // Light's and dark's: ground, panel, text, muted on glass, the key, the solid module.
        for (dark, ground, panel, text, muted, key, solid) in [(false, 0xe6e8ec, 0xffffff, 0x18181b, 0x52525b, 0x45577a, 0xf8f9fa), (true, 0x161920, 0x191d26, 0xf2f3f6, 0xbcc2ce, 0xa9bad6, 0x25272e)] {
            let wash = if dark { 0x1e } else { 0x1a };
            let tint = |under: [f64; 3], colour: u32, alpha: u32| {
                let a = f64::from(alpha) / 255.;
                let c = [colour >> 16, (colour >> 8) & 255, colour & 255].map(f64::from);
                [0, 1, 2].map(|i| c[i] * a + under[i] * (1. - a))
            };
            for glass in Glass::ALL {
                for behind in [0., 64., 128., 192., 255.] {
                    let least = if behind == 0. || behind == 255. { 3. } else { 4.5 };
                    for (name, under) in [("ground", over(ground, glass.alpha(Pane::Controls, dark), behind)), ("panel", over(panel, glass.alpha(Pane::Panel, dark), behind))] {
                        // (A notification's buttons are on a banner, a panel: the text's colour at 8%.)
                        let button = if name == "panel" { ratio(text, tint(under, text, 0x14)) } else { least };
                        let r = [ratio(text, under), ratio(text, tint(under, key, wash)), button];
                        assert!(r.iter().all(|r| *r >= least), "{glass:?} {name} dark={dark} over {behind}: {r:.1?}");
                        if behind == 128. && glass != Glass::Strong {
                            assert!(ratio(muted, under) >= 3., "{glass:?} {name} muted dark={dark}");
                        }
                    }
                }
            }
            // A notification in the list: a module made solid, as it is on the ground where
            // nothing's frosted; its words, its buttons' and the wash's on it.
            let module = if dark { 0x10 } else { 0xb8 };
            let made = tint(over(ground, 255, 0.), 0xffffff, module);
            let c = [solid >> 16, (solid >> 8) & 255, solid & 255].map(f64::from);
            assert!((0..3).all(|i| (c[i] - made[i]).abs() <= 1.5), "{solid:06x} is {made:?}");
            let on = over(solid, 255, 0.);
            assert!(ratio(text, on) >= 4.5 && ratio(text, tint(on, text, 0x14)) >= 4.5 && ratio(muted, on) >= 4.5, "dark={dark}");
        }
    }

    #[test]
    fn the_launcher_switcher_and_level_read_over_anything() {
        // On a panel, at every step, light and dark, over black, greys and white: an app's name
        // on the slate's plate (the launcher's apps used lately, the switcher's), at AA over
        // greys and 3:1 over black or white; the level's fill against its track and the panel
        // under that, 3:1 over greys (it's a mark, not words). The launcher's picked line is white on the
        // strong slate (`control_centers_modules_read_over_anything`).
        // Light's and dark's: panel, text, the strong slate, the plate's alpha, the level's fill.
        for (dark, panel, text, key, plate, fill) in [(false, 0xffffff, 0x18181b, 0x45577a, 0x2e, 0x323a4b), (true, 0x191d26, 0xf2f3f6, 0x5a72a3, 0x40, 0xe4e7ec)] {
            let track = if dark { 0x24 } else { 0x17 };
            let tint = |under: [f64; 3], colour: u32, alpha: u32| {
                let a = f64::from(alpha) / 255.;
                let c = [colour >> 16, (colour >> 8) & 255, colour & 255].map(f64::from);
                [0, 1, 2].map(|i| c[i] * a + under[i] * (1. - a))
            };
            for glass in Glass::ALL {
                for behind in [0., 64., 128., 192., 255.] {
                    let least = if behind == 0. || behind == 255. { 3. } else { 4.5 };
                    let under = over(panel, glass.alpha(Pane::Panel, dark), behind);
                    let r = ratio(text, tint(under, key, plate));
                    assert!(r >= least, "plate {glass:?} dark={dark} over {behind}: {r:.1}");
                    // (Over black or white, a mark on Strong glass is as see-through as it says,
                    // as Control Center's sliders are.)
                    let r = (ratio(fill, tint(under, text, track)), ratio(fill, under));
                    if least > 3. {
                        assert!(r.0 >= 3. && r.1 >= 3., "level {glass:?} dark={dark} over {behind}: {r:.1?}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_grey_hover_and_pick_read_over_anything() {
        // The desktop's lists and menus picked and hovered in grey (`row_hover`, `row_picked`),
        // not slate: their words, in the text's colour, on a panel and on Control Center's ground
        // at every step, light and dark, AA over greys and 3:1 over black or white; the pick a
        // step deeper than the hover, so it's plain which is which.
        // Light's and dark's: text, panel, ground, the hover's alpha, the pick's.
        for (dark, text, panel, ground, hover, picked) in [(false, 0x18181b, 0xffffff, 0xe6e8ec, 0x0f, 0x1a), (true, 0xf2f3f6, 0x191d26, 0x161920, 0x0a, 0x12)] {
            assert!(picked > hover);
            for glass in Glass::ALL {
                for behind in [0., 64., 128., 192., 255.] {
                    let least = if behind == 0. || behind == 255. { 3. } else { 4.5 };
                    for under in [over(panel, glass.alpha(Pane::Panel, dark), behind), over(ground, glass.alpha(Pane::Controls, dark), behind)] {
                        for wash in [hover, picked] {
                            let a = f64::from(wash) / 255.;
                            let c = [text >> 16, (text >> 8) & 255, text & 255].map(f64::from);
                            let on = [0, 1, 2].map(|i| c[i] * a + under[i] * (1. - a));
                            let r = ratio(text, on);
                            assert!(r >= least, "{glass:?} dark={dark} over {behind}: {r:.1}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn settings_groups_read_light_and_dark() {
        // Settings' page is solid (only its sidebar's frosted): its grey, a group on it (white,
        // 72% or 6%), light's and dark's. On the group: the text, a hint, a push button's and a
        // segment's words, and the line between rows a hairline that shows. On the grey: a
        // group's heading and the footnote under it at AA. The sidebar's picked line, white on
        // the strong slate.
        let tint = |under: [f64; 3], colour: u32, alpha: u32| {
            let a = f64::from(alpha) / 255.;
            let c = [colour >> 16, (colour >> 8) & 255, colour & 255].map(f64::from);
            [0, 1, 2].map(|i| c[i] * a + under[i] * (1. - a))
        };
        // (As `color` has them, light's and dark's: the ground, a group's white, the text, muted,
        // grouped_muted, segment_on, key_strong.)
        for (dark, grouped, module, text, muted, heading, segment, strong) in [(false, 0xf0f1f4, 0xb8, 0x18181b, 0x71717a, 0x5f5f68, 0xffffff, 0x45577a), (true, 0x161920, 0x10, 0xf2f3f6, 0x9aa2b2, 0x9aa2b2, 0x50566a, 0x5a72a3)] {
            let ground = over(grouped, 255, 0.);
            let group = tint(ground, 0xffffff, module);
            assert!(ratio(text, ground) >= 7. && ratio(text, group) >= 7., "dark={dark}");
            assert!(ratio(muted, group) >= 4.5, "hint dark={dark}: {:.2}", ratio(muted, group));
            assert!(ratio(heading, ground) >= 4.5, "heading dark={dark}: {:.2}", ratio(heading, ground));
            // (muted() on the grey is why there's grouped_muted(): under AA in light.)
            assert!(dark || ratio(muted, ground) < 4.5);
            assert!(ratio(text, tint(group, text, 0x14)) >= 4.5 && ratio(text, tint(group, text, 0x1f)) >= 4.5, "push dark={dark}");
            let track = tint(group, text, 0x12);
            assert!(ratio(text, over(segment, 255, 0.)) >= 4.5 && ratio(text, track) >= 4.5, "segments dark={dark}");
            // (The picked segment stands off its track.)
            assert!(ratio(segment, track) >= 1.15, "segment dark={dark}: {:.2}", ratio(segment, track));
            assert!(ratio(0xffffff, over(strong, 255, 0.)) >= 4.5, "picked dark={dark}");
            // (A group stands off the page, if only a little: its hairline and lit edge do the rest.)
            assert!(group.iter().zip(ground.iter()).all(|(g, p)| g > p), "dark={dark}");
        }
        // The sidebar's search field, a well of the text's colour on its glass: what's typed in
        // it as the sidebar's words are, at every step; its shortcut as the sidebar's quiet ones.
        for (dark, ground, text, side_muted) in [(false, 0xfafafa, 0x18181b, 0x52525b), (true, 0x13161d, 0xf2f3f6, 0xb4bac7)] {
            for glass in Glass::ALL {
                for behind in [0., 64., 128., 192., 255.] {
                    let least = if behind == 0. || behind == 255. { 3. } else { 4.5 };
                    let field = tint(over(ground, glass.alpha(Pane::Sidebar, dark), behind), text, 0x14);
                    assert!(ratio(text, field) >= least, "{glass:?} search dark={dark} over {behind}: {:.2}", ratio(text, field));
                    if behind == 128. && glass != Glass::Strong {
                        assert!(ratio(side_muted, field) >= 3., "{glass:?} search's keys dark={dark}");
                    }
                }
            }
        }
    }

    #[test]
    fn each_step_is_more_see_through_and_more_blurred() {
        for pane in [Pane::Bar, Pane::Panel, Pane::Controls, Pane::Sidebar, Pane::Scrim, Pane::Widget, Pane::QuietWidget] {
            for dark in [false, true] {
                let alphas = Glass::ALL.map(|g| g.alpha(pane, dark));
                assert!(alphas.windows(2).all(|w| w[0] > w[1]), "{pane:?} {alphas:?}");
            }
        }
        let wide = |g: Glass| {
            let (passes, offset, _) = g.blur();
            offset * f32::from(1u8 << passes)
        };
        assert!(wide(Glass::Subtle) < wide(Glass::Medium) && wide(Glass::Medium) < wide(Glass::Strong));
    }

    #[test]
    fn erase_reads_on_its_red() {
        // (bad() and on_bad(), light's and dark's.)
        for (bad, on_bad) in [(0xb3443c, 0xffffff), (0xe08a8a, 0x13161d)] {
            let r = ratio(on_bad, over(bad, 255, 0.));
            assert!(r >= 4.5, "{bad:06x}: {r:.1}");
        }
    }

    #[test]
    fn gtk_is_milky_in_light_and_clearer_in_dark() {
        for glass in Glass::ALL {
            let (light, dark) = (glass.gtk(false), glass.gtk(true));
            if glass.on() {
                assert!(light.iter().zip(dark).all(|(l, d)| *l > d), "{glass:?} {light:?} {dark:?}");
            } else {
                assert_eq!((light, dark), ([100; 7], [100; 7]));
            }
        }
        // (Each part as see-through or more at each step, and clearly more from Subtle to Strong.)
        for dark in [false, true] {
            for part in 0..7 {
                let steps = Glass::ALL.map(|g| g.gtk(dark)[part]);
                assert!(steps.windows(2).all(|w| w[0] >= w[1]) && steps[1] > steps[3], "part {part} dark={dark} {steps:?}");
            }
        }
    }

    #[test]
    fn gtk_text_reads_over_anything() {
        // Each part's colour (themes::gtk_css): the ground, the panel or the soft one; namiOS's
        // own, light's and dark's: ground, panel, soft, text. (As ours: 4.5:1 over greys, 3:1
        // over black or white.)
        for (dark, ground, panel, soft, text) in [(false, 0xfafafa, 0xffffff, 0xf4f4f5, 0x18181b), (true, 0x13161d, 0x191d26, 0x212634, 0xf2f3f6)] {
            let under = [ground, panel, ground, soft, ground, ground, soft];
            for glass in Glass::ALL {
                for (part, (percent, colour)) in glass.gtk(dark).into_iter().zip(under).enumerate() {
                    for behind in [0., 64., 128., 192., 255.] {
                        let least = if behind == 0. || behind == 255. { 3. } else { 4.5 };
                        let r = ratio(text, over(colour, percent * 255 / 100, behind));
                        assert!(r >= least, "{glass:?} part {part} dark={dark} over {behind}: {r:.1}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_old_blur_setting_still_reads() {
        assert_eq!(Glass::read(None, None, false, false), Glass::Subtle);
        assert_eq!(Glass::read(None, Some("off"), false, false), Glass::Off);
        assert_eq!(Glass::read(None, Some("wallpaper"), false, false), Glass::Medium);
        assert_eq!(Glass::read(None, Some("everything"), false, false), Glass::Strong);
        // (The new one wins; a word it doesn't know is as if it weren't there.)
        assert_eq!(Glass::read(Some("subtle"), Some("everything"), false, false), Glass::Subtle);
        assert_eq!(Glass::read(Some("very"), Some("everything"), false, false), Glass::Strong);
        assert_eq!(Glass::read(Some("strong"), None, true, false), Glass::Off);
        assert_eq!(Glass::read(Some("strong"), None, false, true), Glass::Off);
    }

    #[test]
    fn letters_close_up_as_type_grows() {
        let sizes = [9., 10., 11., 12., 12.5, 13., 14., 16., 17., 20., 26., 28., 38., 104., 200.];
        let t = sizes.map(text::tracking);
        assert!(t.windows(2).all(|w| w[0] >= w[1]), "{t:?}");
        // (Body's left as Geist has it; small words a touch apart; display about -0.02em.)
        assert_eq!(text::tracking(text::BODY), 0.);
        assert!(text::tracking(text::CAPTION) > 0. && text::tracking(text::CAPTION) <= 0.015);
        assert!((text::tracking(text::TITLE) + 0.0175).abs() < 0.002);
        assert!(text::tracking(104.) >= -0.022);
        let l = sizes.map(text::leading);
        assert!(l[5..].windows(2).all(|w| w[0] >= w[1]), "{l:?}");
        assert!(text::leading(text::DISPLAY) < text::leading(text::BODY));
    }

    /// The text system spaces Geist's letters as `text::tracking` has it, and not Geist Mono's.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_text_system_tracks_by_size() {
        use gpui::{FontRun, PlatformTextSystem};
        let system = gpui_wgpu::CosmicTextSystem::new_without_system_fonts("Geist");
        system.add_fonts(vec![include_bytes!("../fonts/Geist-SemiBold.ttf").as_slice().into(), include_bytes!("../fonts/GeistMono-Regular.otf").as_slice().into()]).unwrap();
        let width = |family: &str, size: f32| {
            let font = system.font_id(&gpui::font(family.to_string())).unwrap();
            f32::from(system.layout_line("Wednesday 12", gpui::px(size), &[FontRun { len: 12, font_id: font }]).width)
        };
        let (big, small, mono) = (width("Geist", 60.), width("Geist", 10.), width("Geist Mono", 60.));
        set_type();
        // (12 letters: each one's spacing on the width.)
        assert!((width("Geist", 60.) - (big + 12. * 60. * text::tracking(60.))).abs() < 0.5);
        assert!((width("Geist", 10.) - (small + 12. * 10. * text::tracking(10.))).abs() < 0.1);
        assert_eq!(width("Geist Mono", 60.), mono);
    }

    #[test]
    fn bigger_surfaces_are_thicker() {
        let far = |d: Depth| material(d).iter().filter(|s| !s.inset).map(|s| (f32::from(s.blur_radius), s.color.a)).fold((0f32, 0f32), |a, b| (a.0.max(b.0), a.1.max(b.1)));
        let (chip, panel, sheet) = (far(Depth::Chip), far(Depth::Panel), far(Depth::Sheet));
        assert!(chip.0 < panel.0 && panel.0 < sheet.0 && panel.1 < sheet.1, "{chip:?} {panel:?} {sheet:?}");
        // (Arriving, the shadow lifts it: none at the start.)
        let mut start = materialize(div(), 0., Depth::Sheet);
        assert!(start.style().box_shadow.as_ref().is_some_and(|s| s.iter().all(|s| s.inset || s.color.a == 0.)));
    }

    #[test]
    fn a_round_region_is_inside_its_bounds() {
        let b = gpui::Bounds::new(gpui::point(gpui::px(10.), gpui::px(20.)), gpui::size(gpui::px(100.), gpui::px(40.)));
        let strips = rounded_region(b, 14.);
        assert!(strips.iter().all(|s| s.origin.x >= b.origin.x && s.origin.y >= b.origin.y && s.origin.x + s.size.width <= b.origin.x + b.size.width && s.origin.y + s.size.height <= b.origin.y + b.size.height));
        // (A corner's first row is shorter than the middle.)
        assert!(strips[1].size.width < strips[0].size.width);
    }
}
