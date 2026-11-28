//! Where nami's window meets the system it's on. nami-gui is nami's window on any Linux, a
//! package manager first (Discover, Installed, Your Setup, Projects, History, Sync); on namiOS
//! it's the system's Settings as well, with namiOS's own pages, sidebar, keys and windows. All
//! that comes in through `Os`, picked when nami-gui is built: `Plain` (nami's own window, as the
//! public app is), or namiOS's (with the `namios` feature).
//!
//! nami's own code (src/) never names namiOS's: it asks `Active`, here. A page of namiOS's is
//! `Page::Os(..)`, a line of its sidebar `Line::Os(..)`, what it keeps `Shell::os`.

use gpui::{AnyElement, App, Context, Div, KeyDownEvent, Window};

use crate::shell::{Line, Side};
use crate::{Page, Shell};

/// What a system adds to nami's window. Every method has nami's own answer (nothing to add)
/// but the few that need a page of the system's to say anything.
pub trait Os {
    /// What it keeps in the window (`Shell::os`): its pages' forms, the category showing.
    type State;
    /// Its own pages (`Page::Os`).
    type Page: Copy + PartialEq + std::fmt::Debug;
    /// Its own lines in the sidebar (`Line::Os`).
    type Line: Copy;

    /// Whether nami-gui is the system's Settings here, not only nami's window
    /// (`pages::store_mode` is its opposite).
    fn on() -> bool {
        false
    }

    // The program, before any window.

    /// A command of the system's (`nami-gui privacy …`): whether it was one (it's done).
    fn command(_args: &[String]) -> bool {
        false
    }
    /// A program of its own, run instead of the window (the backups' timeline): whether it ran.
    fn run_apart() -> bool {
        false
    }
    /// Whether one of its own windows is asked for (namiOS's welcome, Install namiOS), not
    /// Settings' (which is one window, looked up first).
    fn own_window_asked() -> bool {
        false
    }
    /// That window, opened: whether it was.
    fn open_own_window(_cx: &mut App) -> bool {
        false
    }

    // What it keeps, and the page first shown.

    /// What it keeps, as the window's made (its own loops started too).
    fn state(cx: &mut Context<Shell>) -> Self::State;
    /// The page the window opens at, if it's the system's (else Discover).
    fn first_page() -> Option<Page> {
        None
    }
    /// Which part of a page is showing (Settings' category), for the page to start at its top
    /// when it changes.
    fn subpage(_os: &Self::State) -> usize {
        0
    }

    // Pages, by name and by the way there.

    /// One of its pages, by the name it's asked for by (`--page`, `nami desktop settings`).
    fn page_named(_name: &str) -> Option<Page> {
        None
    }
    /// A name the desktop asked for that isn't one of nami's own pages: gone to, if it's the
    /// system's (whether it was).
    fn route(_g: &mut Shell, _name: &str, _cx: &mut Context<Shell>) -> bool {
        false
    }
    /// The same, as the window first opens (whether it took it).
    fn first_route(_g: &mut Shell, _name: &str, _cx: &mut Context<Shell>) -> bool {
        false
    }
    /// After the first look: the page it opened at, or the one asked for that's nobody's
    /// (a category of Settings, say), opened.
    fn first_part(_g: &mut Shell, _asked: Option<String>, _cx: &mut Context<Shell>) {}
    /// A page gone to (any page: what it had open is put away).
    fn entering(_g: &mut Shell, _page: Page, _cx: &mut Context<Shell>) {}
    /// Its page's title, in the toolbar (None: the page has its own).
    fn title(g: &Shell, page: Self::Page, cx: &App) -> Option<String>;
    /// Its page.
    fn render(g: &mut Shell, page: Self::Page, cx: &mut Context<Shell>) -> Div;
    /// How wide its page gets at most.
    fn widest(page: Self::Page) -> f32;
    /// Whether its page has no footer unless something's going (the Overview).
    fn bare(_page: Self::Page) -> bool {
        false
    }
    /// Whether Developer mode, turned on at its page, starts at the computer's file.
    fn on_system_file(_page: Self::Page) -> bool {
        false
    }
    /// What "‹" says, going back from an app in a download to its page (None: not back there).
    fn back_label(_g: &Shell, _page: Self::Page) -> Option<&'static str> {
        None
    }

    // The sidebar.

    /// What the sidebar lists, if it's the system's list (None: nami's own).
    fn sidebar(_s: Side) -> Option<Vec<Line>> {
        None
    }
    /// What's above the sidebar's lines (Settings' search).
    fn sidebar_top(_g: &Shell, _cx: &mut Context<Shell>) -> Option<AnyElement> {
        None
    }
    /// Its line, drawn.
    fn line_row(g: &Shell, line: Self::Line, cx: &mut Context<Shell>) -> AnyElement;
    /// Whether its line is where the window is.
    fn line_lit(_g: &Shell, _line: Self::Line) -> bool {
        false
    }
    /// Whether the arrow keys stop at its line.
    fn line_stop(_line: Self::Line) -> bool {
        false
    }
    /// Its line, gone to (from the arrow keys).
    fn line_open(_g: &mut Shell, _line: Self::Line, _window: &mut Window, _cx: &mut Context<Shell>) {}
    /// What its line says, in `nami-gui-status`' sidebar (None: nothing).
    fn line_label(_line: Self::Line) -> Option<String> {
        None
    }
    /// In Developer mode's sidebar, under the files: its own lines.
    fn dev_rows(_g: &Shell, _cx: &mut Context<Shell>) -> Vec<AnyElement> {
        Vec::new()
    }
    /// And at its foot.
    fn dev_foot(_g: &Shell, _cx: &mut Context<Shell>) -> Option<AnyElement> {
        None
    }

    // Keys.

    /// Ctrl+F, where it isn't the apps' search: its own.
    fn find(_g: &mut Shell, _window: &mut Window, _cx: &mut Context<Shell>) {}
    /// A plain key the window has, before its own (Esc, Enter): whether it took it.
    fn key(_g: &mut Shell, _e: &KeyDownEvent, _window: &mut Window, _cx: &mut Context<Shell>) -> bool {
        false
    }
    /// Whether it's holding the keys for itself (a shortcut being recorded): no Esc or arrows.
    fn holds_keys(_g: &Shell) -> bool {
        false
    }
    /// Whether the arrow keys walk the sidebar from what has the focus now (its own fields).
    fn arrows_free(_g: &Shell, _window: &Window, _cx: &App) -> bool {
        false
    }
    /// Esc: what it's asking put away (whether there was something), before nami's own steps.
    fn back_asking(_g: &mut Shell, _cx: &mut Context<Shell>) -> bool {
        false
    }
    /// Esc, once nami's own steps are taken: one of its own (whether there was one).
    fn back_last(_g: &mut Shell, _cx: &mut Context<Shell>) -> bool {
        false
    }

    // Around the page.

    /// Above the page, before its title (the way back to General).
    fn above(_g: &Shell, _cx: &mut Context<Shell>) -> Option<AnyElement> {
        None
    }
    /// Over the page, under the toolbar (Keep these display settings?).
    fn banner(_g: &Shell, _cx: &mut Context<Shell>) -> Option<AnyElement> {
        None
    }
    /// Whether the page showing has a row for the setting at `path` (its line goes under it).
    fn has_row(_g: &Shell, _path: &[String], _cx: &App) -> bool {
        false
    }
    /// Developer mode: whether the page showing takes effect as it's changed (no footer).
    fn quiet_footer(_g: &Shell, _cx: &App) -> bool {
        false
    }
    /// For `nami-gui-status`: whether what its page reads from the computer is read, whether the
    /// Overview's tiles are, and its Wi-Fi password field ("none", "dots", "shown").
    fn status(_g: &Shell, _cx: &App) -> (bool, bool, &'static str) {
        (false, false, "none")
    }

    // Changes.

    /// What it's writing to the person's file now (a reload waits for it).
    fn writing() -> bool {
        false
    }
    /// The files, read again.
    fn reloaded(_g: &mut Shell, _cx: &mut Context<Shell>) {}
    /// Locked again: what was for everyone, put away.
    fn locked(_g: &mut Shell) {}
    /// A run went well (Apply, an update): what it changed, looked at again.
    fn after_run(_g: &mut Shell, _cx: &mut Context<Shell>) {}
    /// An app went on or off.
    fn after_job(_g: &mut Shell) {}
    /// A setting at `path` and its value, as its row says them (History).
    fn in_words(_path: &[String], _value: &str) -> Option<(&'static str, String)> {
        None
    }
}

/// Nothing: the page or line nami's own window doesn't have.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Nothing {}

/// nami's own window, as on any Linux: nothing added.
pub struct Plain;

impl Os for Plain {
    type State = ();
    type Page = Nothing;
    type Line = Nothing;

    fn state(_: &mut Context<Shell>) {}
    fn title(_: &Shell, page: Nothing, _: &App) -> Option<String> {
        match page {}
    }
    fn render(_: &mut Shell, page: Nothing, _: &mut Context<Shell>) -> Div {
        match page {}
    }
    fn widest(page: Nothing) -> f32 {
        match page {}
    }
    fn line_row(_: &Shell, line: Nothing, _: &mut Context<Shell>) -> AnyElement {
        match line {}
    }
}

/// The system nami-gui is built for: nami's own window.
pub type Active = Plain;

/// One of the system's pages, by name (None: it has none of that name).
pub fn page_named(name: &str) -> Option<Page> {
    Active::page_named(name)
}
