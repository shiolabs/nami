//! Apps from downloads, keeping themselves up to date: what nami knows of
//! each one's updates (~/.local/share/nami/app-updates.json, written by
//! `nami file-apps --update`), when it last looked, Update now, and each
//! one's own switch (`nami file-apps --automatic on|off`). Shown on the
//! Updates page and in Your Apps' "From downloads".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use futures::StreamExt;
use gpui::prelude::*;
use crate::look::Press;
use gpui::{AnyElement, Context, Div, ElementId, FontWeight, ParentElement, Stateful, Styled, div, rgb};
use serde::Deserialize;

use super::install_file::FileApp;
use crate::look::px;
use crate::look::text;
use crate::nami::{self, Said};
use crate::ui::{color, icon};
use crate::Shell;

/// What nami knows of one app's updates.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct AppStatus {
    pub automatic: bool,
    pub checked: u64,
    pub state: String,
    pub said: String,
    pub newest: Option<String>,
    pub updated: u64,
    pub before: Option<String>,
}

impl Default for AppStatus {
    fn default() -> Self {
        AppStatus { automatic: true, checked: 0, state: String::new(), said: String::new(), newest: None, updated: 0, before: None }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Status {
    apps: BTreeMap<String, AppStatus>,
}

#[derive(Default)]
pub struct AppUpdates {
    pub known: BTreeMap<String, AppStatus>,
    /// The app being updated now, and what nami says it's doing.
    pub updating: Option<(String, String)>,
}

fn status_file() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    data.join("nami/app-updates.json")
}

/// When what's known last changed (nami writes it after each app it looks at).
pub fn changed_at() -> Option<std::time::SystemTime> {
    std::fs::metadata(status_file()).ok()?.modified().ok()
}

/// What's known, read.
pub fn read() -> BTreeMap<String, AppStatus> {
    std::fs::read_to_string(status_file()).ok().and_then(|t| serde_json::from_str::<Status>(&t).ok()).map(|s| s.apps).unwrap_or_default()
}

/// Read again (the daily run may have changed it).
pub fn reload(g: &mut Shell) {
    g.app_updates.known = read();
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `today at 09:14`, `yesterday at 18:02`, `on 3 Nov`: in this computer's time.
pub(crate) fn when(t: u64) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let local = |t: u64| -> (i32, i32, i32, i32, i32) {
        #[cfg(unix)]
        {
            let tm = crate::clock::local(t as libc::time_t);
            (tm.tm_year, tm.tm_yday, tm.tm_mon, tm.tm_mday, tm.tm_hour * 60 + tm.tm_min)
        }
        #[cfg(not(unix))]
        {
            let d = (t / 86400) as i32;
            (0, d, 0, d, ((t % 86400) / 60) as i32)
        }
    };
    let (y, yd, m, md, min) = local(t);
    let (ny, nyd, ..) = local(now());
    let clock = format!("{:02}:{:02}", min / 60, min % 60);
    if (y, yd) == (ny, nyd) {
        format!("today at {clock}")
    } else if (y == ny && yd + 1 == nyd) || (y + 1 == ny && nyd == 0 && yd >= 364) {
        format!("yesterday at {clock}")
    } else {
        format!("on {md} {}", MONTHS.get(m as usize).copied().unwrap_or(""))
    }
}

/// How it's going with one app's updates: the words, and whether it's a problem.
pub fn line_of(g: &Shell, app: &FileApp) -> (String, bool) {
    if let Some((id, now)) = &g.app_updates.updating {
        if *id == app.id {
            return (if now.is_empty() { "Checking for updates…".to_string() } else { now.trim_end_matches('…').to_string() + "…" }, false);
        }
    }
    // A nami bundle updates itself as it starts too: its own last look (install_bundle.rs).
    if app.format == "bundle" {
        if let Some(said) = super::install_bundle::update_line(g.app_updates.known.get(&app.id), app.live.as_ref(), app.bundle.as_ref().is_some_and(|b| b.feed.is_some())) {
            return said;
        }
    }
    let Some(s) = g.app_updates.known.get(&app.id) else {
        return ("Not checked yet. Updates are checked once a day.".into(), false);
    };
    let checked = if s.checked > 0 { format!(" Checked {}.", when(s.checked)) } else { String::new() };
    match s.state.as_str() {
        "updated" if s.updated > 0 => (format!("Updated {} to {}{}.", when(s.updated), app.version, s.before.as_ref().map(|b| format!(", from {b}")).unwrap_or_default()), false),
        "up-to-date" => (format!("Up to date.{checked}"), false),
        "waiting" => (s.said.clone(), false),
        "refused" | "key-changed" | "failed" => (s.said.clone(), true),
        "manual" => (s.said.clone(), false),
        _ if s.checked > 0 => (format!("Up to date.{checked}"), false),
        _ => ("Not checked yet. Updates are checked once a day.".into(), false),
    }
}

/// Whether it updates by itself.
pub fn automatic(g: &Shell, id: &str) -> bool {
    g.app_updates.known.get(id).is_none_or(|s| s.automatic)
}

/// Update now: `nami file-apps --update <id>`, what it says shown as it goes.
pub fn update_now(g: &mut Shell, id: &str, cx: &mut Context<Shell>) {
    if g.app_updates.updating.is_some() {
        return;
    }
    let id = id.to_string();
    g.app_updates.updating = Some((id.clone(), String::new()));
    cx.notify();
    cx.spawn(async move |this, cx| {
        let mut rx = nami::run(vec!["file-apps".into(), "--update".into(), id.clone()], None);
        while let Some(said) = rx.next().await {
            let gone = this
                .update(cx, |g, cx| {
                    match said {
                        Said::Line(l) => {
                            if let Some((_, now)) = g.app_updates.updating.as_mut() {
                                if !l.trim().is_empty() && !l.contains("Waiting for the other") {
                                    *now = l.trim().to_string();
                                }
                            }
                        }
                        Said::Finished(r) => {
                            g.app_updates.updating = None;
                            super::install_file::reload(g);
                            reload(g);
                            if let Err(e) = r {
                                // (What went wrong is in its status too, when nami got that far.)
                                if g.app_updates.known.get(&id).is_none_or(|s| s.said.is_empty()) {
                                    g.problem = Some(crate::Problem::of("Couldn't update the app.", e));
                                }
                            }
                        }
                    }
                    cx.notify();
                })
                .is_err();
            if gone {
                return;
            }
        }
    })
    .detach();
}

/// Its switch: on or off (`nami file-apps --automatic`).
pub fn set_automatic(g: &mut Shell, id: &str, on: bool, cx: &mut Context<Shell>) {
    g.app_updates.known.entry(id.to_string()).or_default().automatic = on;
    cx.notify();
    let id = id.to_string();
    cx.spawn(async move |this, cx| {
        let mut rx = nami::run(vec!["file-apps".into(), "--automatic".into(), if on { "on" } else { "off" }.into(), id], None);
        let mut end = Ok(());
        while let Some(said) = rx.next().await {
            if let Said::Finished(r) = said {
                end = r;
            }
        }
        let _ = this.update(cx, |g, cx| {
            reload(g);
            if let Err(e) = end {
                g.problem = Some(crate::Problem::of("Couldn't change the setting.", e));
            }
            cx.notify();
        });
    })
    .detach();
}

/// Update now, or how it's going.
pub fn button(g: &Shell, id: ElementId, app: &FileApp, cx: &mut Context<Shell>) -> AnyElement {
    let busy = g.app_updates.updating.as_ref().is_some_and(|(i, _)| *i == app.id);
    if busy {
        return crate::pages::explore::busy_pill(ElementId::Name(format!("{id}-spin").into()), "Updating", true).into_any_element();
    }
    let a = app.id.clone();
    let other = g.app_updates.updating.is_some();
    crate::pages::explore::small_action(id, "Update Now".into())
        .when(other, |b| b.opacity(0.5).cursor_default())
        .when(!other, |b| {
            b.on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                update_now(this, &a, cx);
            }))
        })
        .into_any_element()
}

/// Its switch, with what it means.
pub fn switch_row(g: &Shell, id: ElementId, app: &FileApp, cx: &mut Context<Shell>) -> Stateful<Div> {
    let on = automatic(g, &app.id);
    let a = app.id.clone();
    let label = "Update automatically";
    div()
        .id(id.clone())
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .press()
        .child(crate::ui::said_switch(crate::ui::switch(ElementId::Name(format!("{id}-knob").into()), on), label, on))
        .child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child(label))
        .on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            set_automatic(this, &a, !on, cx);
        }))
}

/// The Updates page's part: each app from a download, its version, when
/// it was looked at, Update now.
pub fn section(g: &Shell, cx: &mut Context<Shell>) -> Div {
    let apps = g.install_file.apps.clone();
    let mut list = div().flex().flex_col();
    for (n, a) in apps.iter().enumerate() {
        let (line, bad) = line_of(g, a);
        let off = !automatic(g, &a.id);
        let line = if off && !bad { format!("{line} Automatic updates are off.") } else { line };
        list = list.child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .py(px(8.))
                .border_b_1()
                .border_color(rgb(color::soft()))
                .child(icon(a.icon.as_deref().map(Path::new), &a.name, 32.))
                .child(
                    div().flex_1().min_w_0().flex().flex_col().gap(px(1.))
                        .child(div().flex().items_center().gap(px(8.)).child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child(a.name.clone())).child(div().text_size(px(text::META)).font_family("Geist Mono").text_color(rgb(color::muted())).child(a.version.clone())))
                        .child(div().text_size(px(text::META)).text_color(rgb(if bad { color::bad() } else { color::muted() })).child(line)),
                )
                // (Not for a nami bundle made without updates: there are none.)
                .when(a.format != "bundle" || a.bundle.as_ref().is_some_and(|b| b.feed.is_some()), |d| d.child(button(g, ElementId::NamedInteger("dl-update-now".into(), n as u64), a, cx))),
        );
    }
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(div().text_size(px(text::SECTION)).font_weight(FontWeight::SEMIBOLD).mt(px(6.)).child("Downloaded Apps"))
        .child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child("These apps update automatically once a day from their developers. Each update is checked against the developer's signature first. An app that's open updates after you quit it."))
        .child(list)
}
