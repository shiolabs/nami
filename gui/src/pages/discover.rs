//! Discover: nami as a store. One search at the top, for apps and command-line tools in any
//! version; the kinds to look through; today's app and the most used, each to get. An app's
//! own page opens from any of them (explore.rs).

use gpui::prelude::*;
use crate::look::Press;
use gpui::{AnyElement, Context, Div, ElementId, FontWeight, ParentElement, Styled, div, rgb};
use crate::look::px;
use crate::look::text;

use super::explore::{self, KINDS};
use crate::nami::Found;
use crate::ui::{app_icon, app_row_with, color, filter, filters, ink, sheet};
use crate::Shell;

/// How many of the most used the front page shows, before See All.
const POPULAR: usize = 8;
/// Under a Mac's front page: what nami installs there, and where apps with a window go.
const MAC_WINDOWS: &str = "Apps with a window install too. nami adds them to Applications, in a folder named nami Apps, so you can open them from Spotlight, the Dock or Finder.";
/// Under a Mac's apps from Homebrew's casks, and over what a search finds of them.
pub const MAC_CASKS: &str = "From Homebrew's casks. nami downloads each app from its developer, checks it, and adds it to Applications, in nami Apps. macOS asks before you open it the first time.";
/// How many a kind (or See All) shows at first, and more each time.
const PAGE: usize = 24;

/// The search, as a store has it at its top: its glass, the field, and its shortcut.
pub fn search_field(g: &Shell, cx: &mut Context<Shell>) -> Div {
    let keys = if crate::look::MAC { "⌘F" } else if g.mac_keys() { "Super F" } else { "Ctrl F" };
    g.search.update(cx, |s, _| s.bare = true);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .h(px(40.))
        .px(px(14.))
        .rounded(px(11.))
        .bg(rgb(color::panel()))
        .border_1()
        .border_color(ink(0x24))
        .child(crate::icons::glyph("search", 16., color::muted()))
        .child(div().flex_1().min_w_0().child(g.search.clone()))
        .child(div().flex_none().px(px(7.)).py(px(1.)).rounded(px(6.)).border_1().border_color(ink(0x28)).text_size(px(text::META)).text_color(rgb(color::muted())).child(keys))
}

pub fn render(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    if g.explore.app.is_some() {
        return explore::app_page(g, cx);
    }
    let page = div().flex().flex_col().gap(px(22.)).pt(px(16.)).child(search_field(g, cx));
    if g.text(&g.search, cx).chars().count() >= 2 {
        return page.child(results(g, cx));
    }
    // (A Mac's catalog doesn't sort apps into kinds yet.)
    if crate::look::MAC {
        return page.child(browse(g, cx));
    }
    page.child(kinds(g, cx)).child(browse(g, cx))
}

/// The kinds to look through, as a row of filters: Discover (the front page) first.
fn kinds(g: &Shell, cx: &mut Context<Shell>) -> Div {
    let kind = g.explore.kind;
    let all = g.explore.all;
    let row = std::iter::once((None, "Discover")).chain(KINDS.iter().map(|(k, n)| (Some(*k), *n))).enumerate().map(|(i, (k, name))| {
        filter(ElementId::NamedInteger("kind".into(), i as u64), name, kind == k && !(k.is_none() && all), true).on_click(cx.listener(move |this, _, _, cx| {
            this.explore.kind = k;
            this.explore.all = false;
            this.explore.shown = PAGE;
            explore::load(this, cx);
            cx.notify();
        }))
    });
    // (One line, as a Mac's: in a narrow window it slides, faded where there's more.)
    let about = crate::fit::pills_about(std::iter::once("Discover").chain(KINDS.iter().map(|(_, n)| *n)));
    crate::fit::sliding("kinds", &g.explore.kinds_scroll, rgb(color::grouped()), about, div().flex().child(filters(true).children(row)))
}

/// An app's card in the grid: its icon, name and line, and Get (or Open, or Installed).
fn tile(g: &Shell, id: &str, n: usize, f: &Found, cx: &mut Context<Shell>) -> gpui::Stateful<Div> {
    let action = explore::action_of(g, ElementId::NamedInteger(format!("{id}-add").into(), n as u64), f, cx);
    let (found, attr) = (f.clone(), f.attr.clone());
    div()
        .id(ElementId::NamedInteger(id.to_string().into(), n as u64))
        .flex()
        .items_center()
        .gap(px(12.))
        .p(px(12.))
        .min_w_0()
        .rounded(px(13.))
        .bg(rgb(color::panel()))
        .border_1()
        .border_color(ink(0x14))
        .cursor_pointer()
        .press_row()
        .child(app_icon(f, 44.))
        .child(
            div().flex_1().min_w_0().flex().flex_col().gap(px(1.))
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(f.name()))
                .child(div().text_size(px(text::META)).text_color(rgb(color::muted())).truncate().child(explore::line_of_app(g, ElementId::NamedInteger(format!("{id}-line").into(), n as u64), f, false, cx))),
        )
        // (Its button keeps its size, at the tile's right: the name gives way, cut short.)
        .child(div().flex_none().child(action))
        .on_click(cx.listener(move |this, _, _, cx| explore::open(this, Some(found.clone()), attr.clone(), cx)))
}

/// Today's app, big: a dark band with its name and line, and its icon. Side by side where
/// there's room; in a narrow window the icon's band above the words, each the page's width.
fn hero(f: &Found, cx: &mut Context<Shell>) -> gpui::Stateful<Div> {
    let dark = color::DARK.load(std::sync::atomic::Ordering::Relaxed);
    let (ground, side) = if dark { (0x232a38, 0x2c3546) } else { (0x1f2633, 0x2a3344) };
    let (found, attr) = (f.clone(), f.attr.clone());
    let stacked = crate::fit::narrower(600.);
    let words = div()
        .flex_1()
        .min_w_0()
        .px(px(if stacked { 22. } else { 32. }))
        .py(px(if stacked { 20. } else { 28. }))
        .flex()
        .flex_col()
        .justify_end()
        .gap(px(6.))
        .child(div().text_size(px(text::META)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(0xaab4c8)).child("TODAY'S APP"))
        // (Its whole name, on two lines if it needs them: never cut short where it can wrap.)
        .child(div().text_size(px(if stacked { 26. } else { 30. })).line_height(px(if stacked { 30. } else { 34. })).font_weight(FontWeight::BOLD).text_color(rgb(0xffffff)).line_clamp(2).text_ellipsis().child(f.name()))
        .child(div().text_size(px(15.)).line_height(px(21.)).text_color(rgb(0xc9d0dc)).line_clamp(if stacked { 3 } else { 2 }).text_ellipsis().child(f.line()));
    let side = div().flex_none().bg(rgb(side)).flex().items_center().justify_center();
    let side = if stacked { side.w_full().h(px(148.)).child(app_icon(f, 88.)) } else { side.w(px((crate::fit::page() * 0.36).min(300.))).h_full().child(app_icon(f, 112.)) };
    div()
        .id("hero")
        .flex()
        .when(stacked, |d| d.flex_col())
        .when(!stacked, |d| d.h(px(216.)))
        .rounded(px(16.))
        .overflow_hidden()
        .bg(rgb(ground))
        .cursor_pointer()
        .press()
        .children(if stacked { [side, words] } else { [words, side] })
        .on_click(cx.listener(move |this, _, _, cx| explore::open(this, Some(found.clone()), attr.clone(), cx)))
}

/// A tool's name in a sentence: a link that searches for it.
fn tool_link(name: &'static str, after: &'static str, cx: &mut Context<Shell>) -> Div {
    div().flex().child(crate::ui::link(ElementId::Name(format!("try-{name}").into()), name).font_family("Geist Mono").on_click(cx.listener(move |this, _, window, cx| {
        this.search.update(cx, |s, cx| s.set(cx, name));
        window.focus(&gpui::Focusable::focus_handle(this.search.read(cx), cx), cx);
    }))).child(after)
}

/// The front page: today's app and the most used; a kind's page: its apps, the most used first.
fn browse(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    explore::load(g, cx);
    let kind = g.explore.kind;
    let shown = g.explore.shown.max(PAGE);
    let list = g.explore.list.as_ref().filter(|(k, _)| *k == kind).map(|(_, l)| l.clone());
    let mut out = div().flex().flex_col().gap(px(22.));
    let grid = |tiles: Vec<gpui::Stateful<Div>>| crate::fit::grid(crate::fit::APP_TILE, 12., 4).children(tiles);
    match list {
        Some(Ok(list)) if kind.is_none() && !g.explore.all => {
            explore::feature(g, &list, cx);
            if let Some(f) = g.explore.featured.clone() {
                out = out.child(hero(&f, cx));
            }
            // (Whole rows: six in three columns, eight in four, two or one.)
            let cols = crate::fit::columns(crate::fit::APP_TILE, 12., 4);
            let shown = POPULAR / cols * cols;
            let tiles: Vec<_> = list.iter().take(shown).enumerate().map(|(n, f)| tile(g, "top", n, f, cx)).collect();
            // (A Mac's: well-known apps from Homebrew's casks, then tools, all of them shown,
            // nami::MAC_POPULAR_APPS and MAC_POPULAR.)
            let mac = crate::look::MAC;
            if mac {
                let apps: Vec<_> = list.iter().filter(|f| f.cask()).enumerate().map(|(n, f)| tile(g, "top-app", n, f, cx)).collect();
                if !apps.is_empty() {
                    out = out.child(
                        div().flex().flex_col().gap(px(12.))
                            .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child("Popular Apps"))
                            .child(grid(apps))
                            .child(crate::ui::quiet(MAC_CASKS)),
                    );
                }
            }
            let tiles = if mac { list.iter().filter(|f| !f.cask()).enumerate().map(|(n, f)| tile(g, "top", n, f, cx)).collect() } else { tiles };
            out = out.child(
                div().flex().flex_col().gap(px(12.))
                    .child(
                        div().flex().items_center().justify_between()
                            .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(if mac { "Popular Tools" } else { "Popular" }))
                            .when(!mac, |d| d.child(crate::ui::link("see-all", "See All").text_size(px(text::CALLOUT)).on_click(cx.listener(|this, _, _, cx| {
                                this.explore.all = true;
                                this.explore.shown = PAGE;
                                cx.notify();
                            })))),
                    )
                    .child(grid(tiles)),
            );
            out = out.child(
                sheet().flex().items_center().gap(px(14.)).px(px(18.)).py(px(14.))
                    .child(crate::icons::glyph("terminal", 20., color::muted()))
                    .child(
                        div().flex_1().min_w_0().flex().flex_wrap().items_center().gap(px(4.)).text_size(px(text::BODY)).text_color(rgb(color::text()))
                            .child(if mac { "Search nixpkgs for the tools you use, like" } else { "Command-line tools too. Search for" })
                            .child(tool_link("ripgrep", ",", cx))
                            .child(tool_link("nodejs", "", cx))
                            .child("or")
                            .child(tool_link("python3", if mac { "." } else { "," }, cx))
                            .when(!mac, |d| d.child("in any version.")),
                    ),
            );
            if mac {
                out = out.child(crate::look::footnote(MAC_WINDOWS));
            }
        }
        Some(Ok(list)) => {
            let title = if kind.is_none() { "Popular" } else { KINDS.iter().find(|(k, _)| Some(*k) == kind).map_or("", |(_, n)| *n) };
            let n = shown.min(list.len());
            let tiles: Vec<_> = list[..n].iter().enumerate().map(|(i, f)| tile(g, "kind-app", i, f, cx)).collect();
            out = out.child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(title)).child(grid(tiles));
            if list.len() > n {
                out = out.child(div().flex().child(crate::ui::grey("more-apps", "Show More").on_click(cx.listener(|this, _, _, cx| {
                    this.explore.shown = this.explore.shown.max(PAGE) + PAGE;
                    cx.notify();
                }))));
            }
        }
        Some(Err(why)) => out = out.child(super::failed("explore-again", "Couldn't load the apps.", why, |this: &mut Shell, cx| {
            this.explore.list = None;
            explore::load(this, cx);
        }, cx)),
        None => out = out.child(super::looking("explore-looking")),
    }
    out
}

/// What the search found: apps and tools from the catalog, then Flathub's apps.
fn results(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    let words = g.text(&g.search, cx);
    let rows = |g: &Shell, id: &'static str, found: Vec<Found>, cx: &mut Context<Shell>| -> Div {
        let rows: Vec<AnyElement> = found.into_iter().enumerate().map(|(n, f)| {
            let action = explore::action_of(g, ElementId::NamedInteger(format!("{id}-add").into(), n as u64), &f, cx);
            let line = explore::line_of_app(g, ElementId::NamedInteger(format!("{id}-line").into(), n as u64), &f, false, cx);
            let (found, attr) = (f.clone(), f.attr.clone());
            app_row_with(ElementId::NamedInteger(id.into(), n as u64), &f, Some(line), action)
                .rounded(px(0.))
                .px(px(16.))
                .py(px(10.))
                .hover(|s| s.bg(color::hover_pill()))
                .when(n > 0, |r| r.border_t_1().border_color(ink(0x10)))
                .on_click(cx.listener(move |this, _, _, cx| explore::open(this, Some(found.clone()), attr.clone(), cx)))
                .into_any_element()
        }).collect();
        sheet().flex().flex_col().children(rows)
    };
    let mut out = div().flex().flex_col().gap(px(12.));
    match g.found.clone() {
        Some((q, Ok(found))) if found.is_empty() => out = out.child(crate::ui::quiet(format!("Nothing was found for “{q}”. Try other words, or check the spelling."))),
        Some((_, Ok(found))) => out = out.child(rows(g, "found", found.into_iter().take(16).collect(), cx)),
        Some((_, Err(why))) => {
            out = out.child(super::failed("search-again", "Couldn't search the catalog.", why, |g: &mut Shell, cx| {
                g.found = None;
                super::apps::search_changed(g, cx);
            }, cx))
        }
        None => out = out.child(super::looking("search-looking")),
    }
    // (On a Mac, Homebrew's casks: nami::search_flathub.)
    if let Some((_, flathub)) = g.found_flathub.clone().filter(|(w, f)| *w == words && !f.is_empty()) {
        let (title, said) = if crate::look::MAC {
            ("Mac Apps", MAC_CASKS)
        } else {
            ("From Flathub", "Apps from Flathub are published by their developers. Each runs in its own sandbox, and is installed only for you.")
        };
        out = out
            .child(div().pt(px(10.)).text_size(px(text::SECTION)).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(crate::ui::quiet(said))
            .child(rows(g, "flathub", flathub.into_iter().take(6).collect(), cx));
    }
    out
}
