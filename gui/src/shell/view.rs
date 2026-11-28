//! Settings' window: the sidebar and the pages it goes to, the toolbar, the status line,
//! and the keys of the whole window.

use std::path::Path;

use gpui::prelude::*;
use crate::look::Press;
use gpui::{App, Context, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled, Window, div, rgb};
use crate::look::{Stop, px, text};

use crate::ui::{button, color};
use crate::{FocusNext, FocusPrev, file_lines, fit, icons, jobs, look, motion, nami, pages, progress, ui};
use super::{MAKING, Page, Shell};
use crate::os::{Active, Os};

/// A line of the sidebar, in its order. The arrow keys walk those that go somewhere.
#[derive(Clone, Copy)]
pub enum Line {
    Heading(&'static str),
    /// Its id, icon and name, where it goes, and whether this person may.
    To(&'static str, &'static str, &'static str, Page, bool),
    /// Room, pushing what's after it to the sidebar's foot.
    Spacer,
    /// The system's own (a category of namiOS's Settings, its welcome, Install).
    Os(<Active as Os>::Line),
}

impl Shell {
    pub fn go(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        Active::entering(self, page, cx);
        self.found = None;
        self.explore.app = None;
        // What's done was said; what failed stays, to try again.
        self.jobs.retain(|j| j.state != jobs::State::Done);
        self.asking_remove = None;
        // (So were what the status line said of the page before, and its Undo.)
        if self.run.as_ref().is_some_and(|r| matches!(r.finished, Some(Ok(())))) {
            self.run = None;
        }
        if !self.busy() {
            self.undo = None;
        }
        self.search.update(cx, |s, cx| s.set(cx, ""));
        if page == Page::Projects {
            pages::projects::look(cx);
        }
        // Updates looks for them as it opens, as a Mac's Software Update does.
        if page == Page::Updates {
            pages::updates::check(self, false, cx);
        }
        if page == Page::Sync {
            self.sync = Default::default();
            pages::sync::look(cx);
        }
        if page == Page::MoveToNamios {
            pages::move_to::look(self, cx);
        }
        if page == Page::History && self.history.is_none() {
            self.history = Some(nami::history());
        }
        // (A Mac has no Sync yet.)
        if page == Page::History && !look::MAC {
            pages::sync::look_setup(cx);
        }
        // (Installed's Storage box, and its page: what nami says, read again.)
        if matches!(page, Page::Installed | Page::Storage) && pages::store_mode() {
            pages::storage::look(self, cx);
        }
        if page == Page::Installed {
            pages::install_file::reload(self);
            let attrs = self.installed_attrs();
            self.learn_names(attrs, cx);
        }
        if page == Page::YourSetup {
            pages::your_setup::look(cx);
        }
        if matches!(page, Page::Discover | Page::Apps) {
            window.focus(&gpui::Focusable::focus_handle(self.search.read(cx), cx), cx);
            // (A press goes on to the window, which would take the focus back.)
            window.prevent_default();
        }
        cx.notify();
    }

    /// What the sidebar lists.
    fn lines(&self) -> Vec<Line> {
        lines_for(self.side())
    }

    /// What the sidebar is drawn for.
    fn side(&self) -> Side {
        Side {
            have_nami: self.role.is_ok(),
            from_stick: self.from_stick,
            store: pages::store_mode(),
            requests: self.requests.waiting() > 0 || self.page == Page::Requests,
            move_to: pages::move_to::offered(self),
            namios: look::on_namios(),
            desktop: look::windows_switch().is_some(),
            mac: look::MAC,
        }
    }

    /// The page's title, in the toolbar above it as a Mac's are. (A file's editor has its own.)
    fn title(&self, cx: &App) -> Option<String> {
        Some(match self.page {
            Page::Os(page) => return Active::title(self, page, cx),
            // (Their own titles, in the page, as a store's: Discover has its search there.)
            Page::Discover | Page::Installed | Page::YourSetup => return None,
            Page::Projects | Page::History | Page::Storage => return None,
            Page::Apps => "System Apps".into(),
            Page::Report => "Report a Problem".into(),
            Page::Requests => "Requests".into(),
            Page::Updates => "Updates".into(),
            Page::Sync => "Sync".into(),
            Page::MoveToNamios => "Move to namiOS".into(),
            Page::InstallFile => "Install an App".into(),
            Page::File(_) => return None,
        })
    }

    /// Up or down the sidebar, to the next line that goes somewhere.
    fn step(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        let lines = self.lines();
        let stops: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| match l {
            Line::To(.., on) => *on,
            Line::Os(l) => Active::line_stop(*l),
            _ => false,
        }).map(|(i, _)| i).collect();
        let here = stops.iter().position(|i| match lines[*i] {
            Line::To(.., page, _) => page == self.page,
            Line::Os(l) => Active::line_lit(self, l),
            _ => false,
        });
        let to = match here {
            Some(at) if down => (at + 1).min(stops.len() - 1),
            Some(at) => at.saturating_sub(1),
            None => 0,
        };
        if here == Some(to) {
            return;
        }
        match lines[stops[to]] {
            Line::To(.., page, _) => self.go(page, window, cx),
            Line::Os(l) => Active::line_open(self, l, window, cx),
            _ => {}
        }
    }

    /// A screenful up or down the page.
    fn scroll_page(&self, down: bool, cx: &mut Context<Self>) {
        let (now, most) = (self.page_scroll.offset(), self.page_scroll.max_offset());
        let by = self.page_scroll.bounds().size.height * 0.9;
        let y = if down { now.y - by } else { now.y + by };
        self.page_scroll.set_offset(gpui::point(now.x, y.max(-most.y).min(px(0.))));
        cx.notify();
    }

    /// The keys of the whole window. Fields keep what they use: this sees what they leave.
    fn key(&mut self, e: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let plain = !(k.modifiers.control || k.modifiers.platform || k.modifiers.alt || k.modifiers.shift);
        // Ctrl+F finds a setting, from anywhere (with a Mac's keys Super+F, as Cmd+F is); Ctrl+W
        // closes the window, as elsewhere.
        if (k.modifiers.control || k.modifiers.platform) && !k.modifiers.shift && !k.modifiers.alt && k.key == "f" {
            // (As a package manager, on another distribution, or on Discover: the apps' search.)
            if self.role.is_ok() && !self.dev.on && (pages::store_mode() || self.page == Page::Discover) {
                if self.page != Page::Discover || self.explore.app.is_some() {
                    self.go(Page::Discover, window, cx);
                }
                window.focus(&gpui::Focusable::focus_handle(self.search.read(cx), cx), cx);
                cx.notify();
            } else if self.role.is_ok() && !self.dev.on {
                Active::find(self, window, cx);
            }
        } else if (k.modifiers.control || k.modifiers.platform) && (k.key == "w" || k.key == "q") {
            // (Not while it's changing the computer.)
            if self.run.as_ref().is_none_or(|r| r.finished.is_some()) {
                window.remove_window();
            }
        } else if !plain {
        } else if k.key == "escape" && self.side_shown {
            self.side_shown = false;
            cx.notify();
        } else if Active::key(self, e, window, cx) {
        } else if k.key == "escape" && !e.is_held && !self.dev.on && !Active::holds_keys(self) {
            // One step out, wherever it's from (not a held Esc's repeats: one press, one step).
            pages::back(self, cx);
        } else if k.key == "pagedown" || k.key == "pageup" {
            self.scroll_page(k.key == "pagedown", cx);
        } else if (k.key == "down" || k.key == "up") && !self.dev.on && !Active::holds_keys(self) {
            // Not out of a field being typed in; the searches, empty, are only passed through.
            let apps_search = gpui::Focusable::focus_handle(self.search.read(cx), cx);
            let free = self.root_focus.is_focused(window) || Active::arrows_free(self, window, cx) || (apps_search.is_focused(window) && self.search.read(cx).content.is_empty());
            if free {
                self.step(k.key == "down", window, cx);
            }
        }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // (On GNOME, KDE Plasma and the rest: framed as namiOS frames its windows.)
        crate::look::frame(window, |window| self.body(window, cx).into_any_element())
    }
}

impl Shell {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        crate::pictures::screen_scale(window.scale_factor());
        // In namiOS's desktop the sidebar lets the wallpaper through, blurred, as thick as
        // Frosted glass has it. Solid with it off, Reduce transparency or high contrast.
        let glass = crate::look::vibrant();
        if self.glass != glass {
            self.glass = glass;
            window.set_background_appearance(if glass { gpui::WindowBackgroundAppearance::Transparent } else { gpui::WindowBackgroundAppearance::Opaque });
        }
        // Whatever had the focus may be gone with its page: without it here, no key would arrive.
        // (Looked at after this frame is drawn: what was just focused may only be in that one.)
        if !self.root_focus.contains_focused(window, cx) {
            cx.defer_in(window, |this, window, cx| {
                if !this.root_focus.contains_focused(window, cx) {
                    window.focus(&this.root_focus, cx);
                }
            });
        }
        // What fits: the window's width, less its frame's room for a shadow (when it has one).
        let inset = match window.window_decorations() {
            gpui::Decorations::Client { .. } if !(window.is_maximized() || window.is_fullscreen()) => crate::look::FRAME_INSET,
            _ => 0.,
        };
        fit::set_window(window.viewport_size().width.as_f32() - 2. * inset);
        let folds = fit::sidebar_folds();
        if !folds {
            self.side_shown = false;
        }
        let showing = (self.page, Active::subpage(&self.os));
        if self.showing != showing {
            self.showing = showing;
            // (A page picked from the sidebar shown over the page: it folds away again.)
            self.side_shown = false;
            self.page_scroll.set_offset(gpui::point(px(0.), px(0.)));
            // However it got here (the keys, the launcher, the first page): its line in view.
            let line = self.lines().iter().position(|l| match l {
                Line::To(.., page, _) => *page == self.page,
                Line::Os(l) => Active::line_lit(self, *l),
                _ => false,
            });
            if let Some(line) = line {
                self.nav_scroll.scroll_to_item(line);
            }
        }
        let admin = self.admin();
        let have_nami = self.role.is_ok();
        let nav = self.nav_scroll.clone();
        // (Grey counts: the updates waiting, the settings files apps changed.)
        let (installed_n, setup_n) = (pages::installed::waiting(self), self.your_setup.changed());
        let store = pages::store_mode();
        let item = |id: &'static str, glyph: &str, label: &str, page: Page, here: Page, on: bool, cx: &mut Context<Self>| {
            let count = match page {
                Page::Installed => installed_n,
                Page::YourSetup => setup_n,
                _ => 0,
            };
            // (At the foot, on another distribution: quieter.)
            let foot = store && matches!(page, Page::MoveToNamios | Page::Report) && page != here;
            // (Said to the screen reader as the Settings parts' rows are.)
            crate::ui::side_row(id, page == here)
                .when(store && !foot, |d| d.h(px(34.)))
                .when(foot, |d| d.text_color(rgb(color::side_muted())).text_size(px(text::CALLOUT)))
                .when(foot && page == Page::Report, |d| d.h(px(26.)).text_size(px(text::META)))
                .role(gpui::Role::Button)
                .aria_label(label.to_string())
                .child(icons::glyph(glyph, 16., crate::ui::side_icon(page == here, on)))
                .when(on, |d| d.cursor_pointer().stop_in(&nav, cx).on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, window, cx| this.go(page, window, cx))).on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                    if e.is_keyboard() {
                        this.go(page, window, cx);
                    }
                })))
                .when(!on, |d| d.text_color(rgb(color::faint())))
                .child(label.to_string())
                .when(count > 0, |d| d.child(crate::ui::badge(count)))
        };
        let n = self.updates.count();
        let updates_label = if n > 0 { format!("Updates · {n}") } else { "Updates".to_string() };
        let waiting = self.requests.waiting();
        let requests_label = if waiting > 0 { format!("Requests · {waiting}") } else { "Requests".to_string() };
        let group = |t: &'static str| crate::look::side_heading(t);
        // (Storage is Installed's: Installed stays lit.)
        let here = if self.page == Page::Storage { Page::Installed } else { self.page };
        let sidebar_top = || div()
            .w(px(crate::look::shape::SIDEBAR))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .px(px(10.))
            .pt(px(14.)).pb(px(8.))
            // (No line beside it: the glass and the page are two surfaces, as on a Mac.)
            .bg(crate::look::sidebar_fill(glass))
            .rounded_l(crate::look::frame_round());
        // Until nami's said who this is (a moment): the window, its sidebar and page still empty.
        if !self.known {
            return div().size_full().flex().rounded(crate::look::frame_round()).when(!glass, |d| d.bg(rgb(color::grouped()))).when(!folds, |d| d.child(sidebar_top())).child(div().flex_1().h_full().rounded_r(crate::look::frame_round()).bg(rgb(color::grouped())));
        }
        let lines = self.lines().into_iter().map(|line| match line {
            Line::Heading(title) => group(title).into_any_element(),
            Line::To(id, glyph, label, page, on) => item(id, glyph, match page { Page::Updates => &updates_label, Page::Requests => &requests_label, _ => label }, page, here, on, cx).into_any_element(),
            Line::Os(l) => Active::line_row(self, l, cx),
            Line::Spacer => div().flex_1().min_h(px(16.)).into_any_element(),
        }).collect::<Vec<_>>();
        // (Its own buttons, and where it's moved from when it floats.)
        let (zoomed, active) = (window.is_maximized(), window.is_window_active());
        // (A Mac's are macOS's own, at `MAC_BUTTONS`: the row's middle where theirs is.)
        let buttons = || if crate::look::MAC {
            div().flex().flex_none().items_center().h(px(18.)).px(px(4.)).mb(px(10.)).child(crate::look::window_controls(zoomed, active))
        } else {
            div().flex().flex_none().items_center().h(px(22.)).px(px(4.)).mt(px(-4.)).mb(px(8.)).child(crate::look::window_buttons(zoomed, active))
        };
        let buttons = || crate::look::moves_window(buttons());
        // The search, at the sidebar's top as a Mac's System Settings has it, with its shortcut.
        // (The system's, at the sidebar's top: namiOS's Settings' search.)
        let search = (have_nami && !self.dev.on).then(|| Active::sidebar_top(self, cx)).flatten();
        let sidebar = sidebar_top()
            .child(buttons())
            .children(search)
            .child(div().relative().flex().flex_col().flex_1().min_h_0().child(div().id("nav").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.nav_scroll).children(lines)).children(crate::look::scroll_edge(&self.nav_scroll, crate::look::Edge::Bottom, crate::look::sidebar_fill(glass))));

        // Developer mode: the files instead of the pages.
        let sidebar = if self.dev.on && have_nami {
            let files = self.dev.files.clone();
            let file_item = |i: usize, label: String, unsaved: bool, here: Page, cx: &mut Context<Self>| {
                div()
                    .id(gpui::ElementId::NamedInteger("file".into(), i as u64))
                    .px(px(12.))
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .rounded(px(crate::look::shape::ROW))
                    .text_size(px(text::CALLOUT))
                    .font_family("Geist Mono")
                    .when(here == Page::File(i), |d| d.bg(crate::ui::ink(0x16)).font_weight(FontWeight::SEMIBOLD))
                    .cursor_pointer()
                    .press_row()
                    .when(here != Page::File(i), |d| d.hover(|s| s.bg(color::hover_pill())))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        pages::dev::open(this, i, cx);
                        this.page = Page::File(i);
                        cx.notify();
                    }))
                    .child(format!("{label}{}", if unsaved { " •" } else { "" }))
            };
            let mut list = div().flex().flex_col().child(group("ME"));
            let mut seen_packages = false;
            for (i, (label, path)) in files.into_iter().enumerate() {
                if i == 1 {
                    list = list.child(group("THIS COMPUTER"));
                }
                // A package folder: under PACKAGES, by its name.
                let label = match label.strip_prefix("packages/") {
                    Some(name) => {
                        if !seen_packages {
                            seen_packages = true;
                            list = list.child(group("PACKAGES"));
                        }
                        name.to_string()
                    }
                    None => label,
                };
                let unsaved = self.dev.unsaved(&path, cx);
                list = list.child(file_item(i, label, unsaved, here, cx));
            }
            // (On another distribution there's no computer's file to be kept from: only yours.)
            if !admin && !store {
                list = list.child(group("THIS COMPUTER")).child(div().px(px(12.)).text_size(px(text::META)).text_color(rgb(color::muted())).child("Only an administrator can change these settings."));
            }
            sidebar_top()
                .child(buttons())
                .child(div().id("files").flex_1().overflow_y_scroll().child(list))
                .child(div().h(px(12.)))
                // (On another distribution the updates are Installed's, and namiOS's own
                // settings, Terminal and Plugins, aren't there.)
                .when(!store, |s| s.child(item("updates", "updates", &updates_label, Page::Updates, here, true, cx)))
                .when(store, |s| s.child(item("installed", "apps", "Installed", Page::Installed, here, true, cx)))
                .child(item("history", "history", "History", Page::History, here, true, cx))
                .child(item("projects", "code", "Projects", Page::Projects, here, true, cx))
                .children(Active::dev_rows(self, cx))
        } else {
            sidebar
        };
        let dev_on = self.dev.on;
        // (Among the files, Welcome and Report a problem stay below them.)
        let report = dev_on.then(|| item("report", "report", "Report a Problem", Page::Report, here, true, cx));
        let welcome = dev_on.then(|| Active::dev_foot(self, cx)).flatten();
        // (What's above scrolls: its pages fade out at its foot, the files' under a line.)
        let foot = if self.dev.on && have_nami { div().flex_none().h(px(1.)).mx(px(8.)).mt(px(2.)).mb(px(4.)).bg(gpui::rgba((color::line() << 8) | 0x90)) } else { div().flex_none().h(px(7.)) };
        // (Developer mode on another distribution too: your own nami files, to edit and apply.)
        let sidebar = sidebar.child(foot).children(welcome).children(report).when(have_nami, |s| {
            s.child(
                div()
                    .id("developer")
                    .mt(px(if dev_on { 12. } else { 0. }))
                    .px(px(12.))
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(crate::look::shape::ROW))
                    .text_size(px(text::CALLOUT))
                    .cursor_pointer()
                    .press_row()
                    .hover(|s| s.bg(color::hover_pill()))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_dev(!dev_on, cx)))
                    .child(ui::tick("developer-tick".into(), dev_on))
                    .child("Developer Mode"),
            )
        });

        // How wide the page gets at most: a settings page's groups as a Mac's System Settings
        // has them, the store's grids and lists wider. (Before it's drawn: it lays out for it.)
        let widest = match self.page {
            Page::Updates | Page::Requests | Page::Sync | Page::Report | Page::MoveToNamios | Page::Storage => fit::SETTINGS_WIDEST,
            Page::Os(page) => Active::widest(page),
            _ => fit::PAGE_WIDEST,
        };
        fit::set_widest(widest);
        // Above the page, as a Mac's: its title, with the way back to General before it on a
        // page General lists (and where the window's moved from).
        let general = (!self.dev.on).then(|| Active::above(self, cx)).flatten();
        // In a narrow window the sidebar folds away: the toolbar has the window's buttons then,
        // and a button to show the sidebar over the page.
        let side_button = folds.then(|| {
            let shown = self.side_shown;
            div()
                .id("show-sidebar")
                .size(px(28.))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .when(shown, |d| d.bg(color::push()))
                .hover(|s| s.bg(color::push()))
                .press_row()
                .role(gpui::Role::Button)
                .aria_label(if shown { "Hide Sidebar" } else { "Show Sidebar" })
                // (Not the toolbar's: that moves the window.)
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.side_shown = !this.side_shown;
                    cx.notify();
                }))
                .child(icons::glyph("sidebar", 16., color::text()))
        });
        let title = self.title(cx);
        let header = (title.is_some() || folds).then(|| {
            crate::look::moves_window(div())
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.))
                .h(px(52.))
                .px(px(fit::gutter() - 6.))
                // (Its title over the page's left edge, the page held to its widest.)
                .pl(px(fit::gutter() - 6. + fit::spare()))
                .when(folds, |d| d.child(div().flex().flex_none().items_center().px(px(4.)).mr(px(6.)).child(crate::look::window_controls(zoomed, active))).children(side_button))
                .children(general)
                .children(title.map(|title| div().flex_1().min_w_0().pl(px(6.)).truncate().text_size(px(text::SECTION)).font_weight(FontWeight::BOLD).child(title)))
        });
        // Uninstall nami…'s dialog, over everything while it's up.
        let uninstall_over = pages::uninstall_nami::overlay(self, cx).or_else(|| pages::storage::overlay(self, cx));
        // The sidebar: beside the page, or (folded away) over it while it's shown.
        let (sidebar, side_over) = if folds { (None, self.side_shown.then_some(sidebar)) } else { (Some(sidebar), None) };
        let side_over = side_over.map(|s| {
            div()
                .absolute()
                .inset_0()
                .flex()
                .child(motion::slide_in_left("sidebar-over", s.bg(rgb(color::ground())).shadow(crate::look::popup_shadow())))
                .child(div().id("beside-sidebar").flex_1().h_full().on_click(cx.listener(|this, _, _, cx| {
                    this.side_shown = false;
                    cx.notify();
                })))
        });
        // What the window just wrote: its line, for a few seconds.
        file_lines::take_written(self, cx);
        let page = match self.page {
            Page::File(_) => pages::dev::render(self, cx),
            Page::Discover => pages::discover::render(self, cx),
            Page::Installed => pages::installed::render(self, cx),
            Page::YourSetup => pages::your_setup::render(self, cx),
            Page::Apps => pages::apps::render(self, true, cx),
            Page::Projects => pages::projects::render(self, cx),
            Page::History => pages::history::render(self, cx),
            Page::Report => pages::report::render(self, cx),
            Page::Requests => pages::requests::render(self, cx),
            Page::Storage => pages::storage::render(self, cx),
            Page::Updates => pages::updates::render(self, cx),
            Page::Sync => pages::sync::render(self, cx),
            Page::MoveToNamios => pages::move_to::render(self, cx),
            Page::InstallFile => pages::install_file::render(self, cx),
            Page::Os(page) => Active::render(self, page, cx),
        };
        // Its line: under the setting's row, or (another page) a small card at the bottom.
        let in_row = self.said_file.as_ref().is_some_and(|s| Active::has_row(self, &s.path, cx));
        let mut toast = file_lines::toast(self, in_row || matches!(self.page, Page::File(_)), cx);

        // Something going: said quietly in the footer, with a spinner. What
        // failed: what nami said, and what to do.
        let running = self.run.as_ref().is_some_and(|r| r.finished.is_none());
        let going: Vec<&jobs::Job> = self.jobs.iter().filter(|j| matches!(j.state, jobs::State::Going | jobs::State::Waiting)).collect();
        let job_going = !going.is_empty();
        let busy = running || job_going;
        let run_panel = self.run.as_ref().filter(|r| !(self.page == Page::Updates && r.title == pages::updates::UPDATING)).and_then(|r| match &r.finished {
            Some(Err(e)) => Some((r, e)),
            _ => None,
        }).map(|(r, e)| {
            // What went wrong, without the paths that landed before it.
            let all: Vec<&str> = if e.trim().is_empty() { r.lines.iter().map(String::as_str).collect() } else { e.lines().collect() };
            let said: Vec<&str> = all.into_iter().filter(|l| progress::landed(l).is_none()).collect();
            let shown = self.run_details;
            trouble(failed_title(&r.title), if shown { &said[said.len().saturating_sub(6)..] } else { &[] }).child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(button("retry", "Try Again", true, true).on_click(cx.listener(|this, _, _, cx| {
                        if let Some(r) = this.run.take() {
                            this.start(&r.title, r.cmds, cx);
                        }
                    })))
                    .child(button("tell", "Report This Problem", false, true).on_click(cx.listener(|this, _, w, cx| this.go(Page::Report, w, cx))))
                    .when(!said.is_empty(), |d| d.child(button("run-details", if shown { "Hide Details" } else { "Show Details" }, false, true).on_click(cx.listener(|this, _, _, cx| {
                        this.run_details = !this.run_details;
                        cx.notify();
                    }))))
                    .child(button("close-run", "Close", false, true).on_click(cx.listener(|this, _, _, cx| {
                        this.run = None;
                        cx.notify();
                    }))),
            )
        });
        // An app that couldn't go on, its Details asked for: what nami said.
        let details_panel = self.details.as_ref().and_then(|a| self.job(a)).and_then(|j| match &j.state {
            jobs::State::Failed(log) => Some((j, log)),
            _ => None,
        }).map(|(j, log)| {
            // Each thing said once (nami tries a download again and again).
            let mut said: Vec<&str> = Vec::new();
            for l in log.lines().filter(|l| !l.trim().is_empty()) {
                if !said.contains(&l) {
                    said.push(l);
                }
            }
            let attr = j.attr.clone();
            trouble(format!("Couldn't {} {}", j.kind.plain(), self.app_name(&j.attr)), &said[said.len().saturating_sub(8)..]).child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(button("job-retry", "Try Again", true, true).on_click(cx.listener(move |this, _, _, cx| this.retry(&attr, cx))))
                    .child(button("job-tell", "Report This Problem", false, true).on_click(cx.listener(|this, _, w, cx| this.go(Page::Report, w, cx))))
                    .child(button("job-close", "Close", false, true).on_click(cx.listener(|this, _, _, cx| {
                        this.details = None;
                        cx.notify();
                    }))),
            )
        });
        let applied = self.run.as_ref().is_some_and(|r| matches!(r.finished, Some(Ok(()))));
        let dev = self.dev.on;
        // Just applied: the apps it added, to open.
        let fresh: Vec<(String, String)> = if applied && self.problem.is_none() {
            self.history.as_ref().and_then(|h| h.as_ref().ok()).and_then(|h| h.first()).map(|s| s.added.iter().map(|e| e.attr.clone()).filter(|a| self.launchers.contains_key(a)).take(3).map(|a| (format!("Open {}", self.app_name(&a)), a)).collect()).unwrap_or_default()
        } else {
            Vec::new()
        };
        let open_fresh = |fresh: Vec<(String, String)>, cx: &mut Context<Self>| {
            div().flex().flex_none().gap(px(8.)).children(fresh.into_iter().enumerate().map(|(n, (label, attr))| {
                div().id(ElementId::NamedInteger("open-new".into(), n as u64)).child(button("open-new", label, false, true).on_click(cx.listener(move |this, _, _, cx| this.open_app(&attr, cx))))
            }))
        };
        // What went wrong, in a sentence on the line; what was said of it behind Show Details,
        // under it. (Hidden again for the next one.)
        if self.problem != self.said_problem {
            self.said_problem = self.problem.clone();
            pages::hide_details("status");
        }
        let detail = self.problem.as_ref().filter(|_| !running && !job_going).map(|p| p.detail.clone()).filter(|d| !d.is_empty());
        let shown = detail.is_some() && pages::showing_details("status");
        let details_pill = |cx: &mut Context<Self>| {
            detail.as_ref().map(|_| ui::pill(ElementId::Name("status-details".into()), if shown { "Hide Details" } else { "Show Details" }, false).stop(cx).on_click(|_, window, _| pages::flip_details("status", window)))
        };
        let details = detail.clone().filter(|_| shown).map(|d| {
            div().id("status-detail").flex_none().max_h(px(140.)).overflow_y_scroll().px(px(fit::gutter())).pb(px(14.)).text_size(px(text::META)).font_family("Geist Mono").text_color(rgb(color::muted())).child(d)
        });
        // Outside Developer mode each change is in place as it's made: no Apply, only a line that
        // says what's going ("Removing Cowsay…"), what went wrong, or what's done, with its Undo.
        if !dev {
            let said: Option<(String, bool)> = if running {
                self.run.as_ref().map(|r| (format!("{}…", r.title), false))
            } else if job_going {
                let now = going.iter().find(|j| j.state == jobs::State::Going).unwrap_or(&going[0]);
                let more = going.len() - 1;
                Some((format!("{} {}…{}", now.kind.verb(), self.app_name(&now.attr), if more > 0 { format!(" ({more} more queued)") } else { String::new() }), false))
            } else if let Some(p) = &self.problem {
                Some((p.said.clone(), true))
            } else if let Some(u) = self.undo.as_ref().filter(|u| u.ready) {
                Some((u.said.clone(), false))
            } else if applied && self.run.as_ref().is_some_and(|r| r.title != MAKING) || !fresh.is_empty() {
                Some((done_said(self.run.as_ref().map(|r| r.title.as_str()).unwrap_or_default(), false), false))
            } else {
                None
            };
            let can_undo = self.undo.as_ref().is_some_and(|u| u.ready) && !busy && self.problem.is_none();
            let shown = said.as_ref().map(|(s, _)| first_up(s)).unwrap_or_default();
            self.note_status(if said.is_some() { "status" } else { "none" }, &shown, can_undo, cx);
            let line = said.map(|(text, bad)| {
                let row = div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(12.))
                    .px(px(fit::gutter()))
                    .h(px(52.))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(text::CALLOUT)).text_color(rgb(if bad { color::bad() } else { color::muted() })).child(first_up(&text)))
                    .children(details_pill(cx))
                    .when(can_undo, |d| d.child(button("undo", "Undo", false, true).stop(cx).on_click(cx.listener(|this, _, _, cx| this.undo_last(cx)))))
                    .when(!fresh.is_empty() && !can_undo, |d| d.child(open_fresh(fresh.clone(), cx)));
                div().flex().flex_col().flex_none().border_t_1().border_color(color::hairline()).bg(rgb(color::grouped())).rounded_br(crate::look::frame_round()).child(row).children(details)
            });
            // (What's last at the foot is as round as the window's corner.)
            let round = crate::look::frame_round();
            let details_panel = details_panel.map(|p| p.when(line.is_none(), |p| p.rounded_br(round)));
            let run_panel = run_panel.map(|p| p.when(line.is_none() && details_panel.is_none(), |p| p.rounded_br(round)));
            return div()
                .key_context("Shell")
                .track_focus(&self.root_focus)
                .on_key_down(cx.listener(Self::key))
                .on_action(|_: &FocusNext, window, cx| window.focus_next(cx))
                .on_action(|_: &FocusPrev, window, cx| window.focus_prev(cx))
                .size_full()
                .flex()
                .rounded(round)
                .when(!glass, |d| d.bg(rgb(color::panel())))
                .text_color(rgb(color::text()))
                .font_family("Geist")
                .relative()
                .children(sidebar)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .when(folds, |d| d.rounded(round))
                        .when(!folds, |d| d.rounded_r(round))
                        .bg(rgb(color::grouped()))
                        .children(header).children(file_lines::banner(self, cx)).children(Active::banner(self, cx))
                        .child(div().relative().flex_1().min_h_0().flex().flex_col().child(div().id("page").flex_1().overflow_y_scroll().track_scroll(&self.page_scroll).px(px(fit::gutter())).pt(px(if have_nami { 4. } else { 24. })).pb(px(fit::gutter())).child(div().w_full().max_w(px(widest)).mx_auto().child(motion::enter(ElementId::Name(format!("page-{:?}-{}", self.page, Active::subpage(&self.os)).into()), page)))).children(crate::look::scroll_edge(&self.page_scroll, crate::look::Edge::Top, rgb(color::grouped()))).children(toast.take()))
                        .children(run_panel.map(|p| motion::enter("run-failed", p)))
                        .children(details_panel.map(|p| motion::enter("job-failed", p)))
                        .children(line.map(|l| motion::enter("status-line", l))),
                )
                .children(side_over)
                .children(uninstall_over);
        }
        // Developer mode: the files edited as text wait for Apply (or Discard).
        let changed = !self.before.is_empty();
        let apps_page = matches!(self.page, Page::Discover | Page::Installed | Page::YourSetup | Page::Apps);
        // Settings' own pages take effect as they're picked: nothing to apply there.
        let no_footer = !changed && !busy && self.problem.is_none() && Active::quiet_footer(self, cx);
        let said = match (&self.problem, changed, applied) {
            _ if running => format!("{}…", self.run.as_ref().map(|r| if dev && r.title == MAKING { "Applying" } else { r.title.as_str() }).unwrap_or_default()),
            _ if job_going => {
                let now = going.iter().find(|j| j.state == jobs::State::Going).unwrap_or(&going[0]);
                let more = going.len() - 1;
                format!("{} {}…{}", now.kind.verb(), self.app_name(&now.attr), if more > 0 { format!(" ({more} more queued)") } else { String::new() })
            }
            (Some(p), _, _) => p.said.clone(),
            (None, true, _) => self.changes_said().unwrap_or_else(|| if dev { "Not applied yet.".to_string() } else { "Your changes are saved. Click Apply to use them.".to_string() }),
            (None, false, true) => done_said(self.run.as_ref().map(|r| r.title.as_str()).unwrap_or_default(), dev),
            (None, false, false) => "No changes.".to_string(),
        };
        let footer = div()
            .flex()
            .justify_between()
            .items_center()
            .gap(px(16.))
            .px(px(fit::gutter()))
            .h(px(64.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .when(busy, |d| d.child(motion::spinner("footer-spin", 14., color::text(), true)))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(text::CALLOUT)).text_color(rgb(if self.problem.is_some() && !busy { color::bad() } else { color::muted() })).child(said.clone()))
                    .children(details_pill(cx)),
            )
            .when(!changed && !fresh.is_empty(), |d| d.child(open_fresh(fresh.clone(), cx)))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(px(8.))
                    .child(button("discard", "Discard", false, changed && !busy).when(changed && !busy, |b| b.stop(cx).on_click(cx.listener(|this, _, _, cx| this.discard(cx)))))
                    .child(button("apply", "Apply", true, changed && !busy).when(changed && !busy, |b| b.stop(cx).on_click(cx.listener(|this, _, _, cx| this.apply(cx))))),
            );
        // The footer: where there's something to say. On the apps' pages
        // their own buttons say how each app goes.
        // (Update Now's run: its page says how it's going, and how it went.)
        let updating_here = self.page == Page::Updates && self.run.as_ref().is_some_and(|r| r.title == pages::updates::UPDATING) && !changed && !job_going && self.problem.is_none();
        let show_footer = !no_footer
            && !updating_here
            && self.page != Page::History
            && if apps_page {
                changed || running || applied || self.problem.is_some()
            } else {
                !(matches!(self.page, Page::Projects | Page::InstallFile) || matches!(self.page, Page::Os(p) if Active::bare(p))) || changed || busy || applied || self.problem.is_some()
            };
        self.note_status(if show_footer { "apply" } else { "none" }, &said, false, cx);
        // (What's last at the foot is as round as the window's corner.)
        let round = crate::look::frame_round();
        let details_panel = details_panel.map(|p| p.when(!show_footer, |p| p.rounded_br(round)));
        let run_panel = run_panel.map(|p| p.when(!show_footer && details_panel.is_none(), |p| p.rounded_br(round)));

        div()
            .key_context("Shell")
            .track_focus(&self.root_focus)
            .on_key_down(cx.listener(Self::key))
            .on_action(|_: &FocusNext, window, cx| window.focus_next(cx))
            .on_action(|_: &FocusPrev, window, cx| window.focus_prev(cx))
            .size_full()
            .flex()
            .rounded(round)
            .when(!glass, |d| d.bg(rgb(color::panel())))
            .text_color(rgb(color::text()))
            .font_family("Geist")
            .relative()
            .children(sidebar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .when(folds, |d| d.rounded(round))
                    .when(!folds, |d| d.rounded_r(round))
                    .bg(rgb(color::grouped()))
                    .when(!matches!(self.page, Page::File(_)) || folds, |d| d.children(header))
                    .children(file_lines::banner(self, cx))
                    .children(Active::banner(self, cx))
                    .child(div().relative().flex_1().min_h_0().flex().flex_col().child(div().id("page").flex_1().overflow_y_scroll().track_scroll(&self.page_scroll).px(px(fit::gutter())).pt(px(if matches!(self.page, Page::File(_)) { 24. } else { 4. })).pb(px(24.)).child(div().w_full().max_w(px(widest)).mx_auto().child(motion::enter(ElementId::Name(format!("page-{:?}-{}", self.page, Active::subpage(&self.os)).into()), page)))).children(crate::look::scroll_edge(&self.page_scroll, crate::look::Edge::Top, rgb(color::grouped()))).children(toast.take()))
                    .children(run_panel.map(|p| motion::enter("run-failed", p)))
                    .children(details_panel.map(|p| motion::enter("job-failed", p)))
                    .when(show_footer, |d| d.child(div().flex().flex_col().flex_none().border_t_1().border_color(color::hairline()).bg(rgb(color::grouped())).rounded_br(round).child(footer).children(details))),
            )
            .children(side_over)
            .children(uninstall_over)
    }
}

/// Who this is and where, as the sidebar is drawn for them.
#[derive(Clone, Copy, Default)]
pub struct Side {
    pub have_nami: bool,
    /// Running from namiOS's USB drive: Install on This Computer first.
    pub from_stick: bool,
    /// nami as a package manager first (another distribution, or as it's previewed there).
    pub store: bool,
    /// Changes apps asked for: waiting, or Requests is open.
    pub requests: bool,
    /// Move to namiOS offered (off namiOS).
    pub move_to: bool,
    pub namios: bool,
    /// namiOS's desktop's own categories (Appearance, the Dock...).
    pub desktop: bool,
    /// A Mac: no Sync (its timers are Linux's) and no Move to namiOS.
    pub mac: bool,
}

/// What the sidebar lists, in its order: the system's (namiOS's Settings), or nami's own.
fn lines_for(s: Side) -> Vec<Line> {
    Active::sidebar(s).unwrap_or_else(|| own_lines(s))
}

/// nami's own sidebar: on another distribution, nami is a package manager first, its pages,
/// and moving to namiOS at the foot.
pub(crate) fn own_lines(s: Side) -> Vec<Line> {
    let mut lines = own_pages(s.have_nami).to_vec();
    if s.mac {
        lines.retain(|l| !matches!(l, Line::To(_, _, _, Page::Sync, _)));
    }
    if s.requests {
        lines.push(Line::To("requests", "inbox", "Requests", Page::Requests, s.have_nami));
    }
    lines.push(Line::Spacer);
    if s.move_to && !s.mac {
        lines.push(Line::To("move-to-namios", "computer", "Move to namiOS", Page::MoveToNamios, true));
    }
    lines.push(Line::To("report", "report", "Report a Problem", Page::Report, true));
    lines
}

/// nami's own pages, in the sidebar's order (namiOS's Settings has them under YOU).
pub(crate) fn own_pages(have_nami: bool) -> [Line; 6] {
    [
        Line::To("discover", "compass", "Discover", Page::Discover, have_nami),
        Line::To("installed", "apps", "Installed", Page::Installed, have_nami),
        Line::To("your-setup", "file", "Your Setup", Page::YourSetup, have_nami),
        Line::To("projects", "folder", "Projects", Page::Projects, have_nami),
        Line::To("history", "history", "History", Page::History, have_nami),
        Line::To("sync", "sync", "Sync", Page::Sync, have_nami),
    ]
}

/// What a run that failed is called: "Couldn't make your changes".
fn failed_title(title: &str) -> String {
    match title {
        MAKING => "Couldn't apply your changes".into(),
        "Copying your files" => "Couldn't copy your files".into(),
        t if t.starts_with("Applying ") && t.ends_with("'s change") => format!("Couldn't apply {}", &t["Applying ".len()..]),
        t => format!("{t} failed"),
    }
}

/// What the footer says once a run worked.
/// A sentence as it's shown: its first letter a capital ("cowsay is removed." is "Cowsay is removed.").
fn first_up(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

impl Shell {
    /// What the window's bottom line is and says (`bar`: "status", Developer mode's "apply", or
    /// "none"), and whether it offers Undo: in `$XDG_RUNTIME_DIR/nami-gui-status`, for
    /// tests/vm-quick.sh to read. Written when it changes.
    fn note_status(&mut self, bar: &str, said: &str, undo: bool, cx: &App) {
        // (And whether what a page reads from the computer, and the Overview's tiles, are read.)
        // (And the sidebar: its headings, and the lines under each.)
        let sidebar = self.lines().into_iter().filter_map(|l| match l {
            Line::Heading(h) => Some(format!("|{h}:")),
            Line::To(_, _, label, ..) => Some(format!("{label},")),
            Line::Os(l) => Active::line_label(l).map(|label| format!("{label},")),
            _ => None,
        }).collect::<String>();
        let (read, glance, wifi_password) = Active::status(self, cx);
        let now = format!(
            "bar={bar}\nsaid={}\nundo={}\npage={:?}\nread={}\nglance={}\nsidebar={sidebar}\nwifi_password={}\n",
            if bar == "none" { "" } else { said },
            undo as u8,
            self.page,
            read as u8,
            glance as u8,
            // (The Wi-Fi password: asked, and dots or shown as it's typed. Never what it is.)
            wifi_password,
        );
        if now == self.told_status {
            return;
        }
        self.told_status = now;
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            let _ = std::fs::write(Path::new(&dir).join("nami-gui-status"), &self.told_status);
        }
    }
}

fn done_said(title: &str, dev: bool) -> String {
    match title {
        MAKING if dev => "The computer now matches its configuration files.".into(),
        MAKING => "Your changes were applied.".into(),
        "Copying your files" => "Your files were copied to your home folder.".into(),
        "Updating the apps" => "Your apps are up to date.".into(),
        "Updating nami" => "nami is up to date.".into(),
        "Getting the newest catalog" => "The catalog is up to date.".into(),
        "Freeing up space" => "Disk space was freed up.".into(),
        t if t.starts_with("Applying ") && t.ends_with("'s change") => "The change was applied.".into(),
        t if t.starts_with("Going back to change ") => format!("Restored change {}.", &t["Going back to change ".len()..]),
        // (As nami said it before: a step.)
        t if t.starts_with("Going back to step ") => format!("Restored change {}.", &t["Going back to step ".len()..]),
        _ => "Done.".into(),
    }
}

/// A panel for what went wrong: its title, and what nami said.
fn trouble(title: String, said: &[&str]) -> gpui::Div {
    div()
        .px(px(fit::gutter()))
        .py(px(14.))
        .border_t_1()
        .border_color(color::hairline())
        .bg(rgb(color::bad_soft()))
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(div().flex().items_center().gap(px(8.)).child(icons::glyph("report", 16., color::bad())).child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(color::bad())).child(title)))
        .when(!said.is_empty(), |d| d.child(div().text_size(px(text::META)).font_family("Geist Mono").text_color(rgb(color::text())).child(said.join("\n"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sidebar's lines, by what each is: a page's id, a |HEADING.
    fn ids(s: Side) -> Vec<String> {
        lines_for(s)
            .iter()
            .map(|l| match l {
                Line::Heading(h) => format!("|{h}"),
                Line::To(id, ..) => id.to_string(),
                Line::Spacer => "-".into(),
                // (The system's own: none here.)
                Line::Os(_) => "os".into(),
            })
            .collect()
    }

    /// Off namiOS (another distribution, or previewed as there): nami's own pages, and nothing
    /// of namiOS's Settings, whatever the desktop is.
    #[test]
    fn the_sidebar_off_namios_is_namis_own() {
        let plain = Side { have_nami: true, store: true, ..Default::default() };
        let own = ["discover", "installed", "your-setup", "projects", "history", "sync"];
        assert_eq!(ids(plain), [&own[..], &["-", "report"]].concat());
        assert_eq!(ids(Side { requests: true, move_to: true, ..plain }), [&own[..], &["requests", "-", "move-to-namios", "report"]].concat());
        assert_eq!(ids(Side { desktop: true, namios: true, ..plain }), ids(plain));
    }

    /// On a Mac: what nami does there, without Sync and Move to namiOS.
    #[test]
    fn the_sidebar_on_a_mac_has_what_works_there() {
        let mac = Side { have_nami: true, store: true, mac: true, move_to: true, ..Default::default() };
        assert_eq!(ids(mac), ["discover", "installed", "your-setup", "projects", "history", "-", "report"]);
    }
}
