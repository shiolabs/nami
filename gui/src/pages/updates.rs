//! Updates, as a Mac's Software Update: one status at the top (up to date, checking, what's
//! ready, how far an update is, restart to finish), What's New (nami's release notes, the apps
//! that change), one button that does all of it, and whether updates install by themselves.
//!
//! Update Now runs what's needed, in turn: `nami os-update` (the newest nami and catalog; on
//! namiOS a new version of the computer for its next start, on another distribution the
//! computer's apps), then `nami upgrade --user` for your own apps. Someone who isn't an admin
//! is asked for an admin's password for the computer's part (the daemon asks polkit). Opening
//! the page looks for updates: the newest catalog if it's over an hour old, then what's newer.

use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, ElementId, FontWeight, ParentElement, Styled, div, rgb};
use serde_json::json;

use crate::look::{Stop, px, text};
use crate::nami::{self, NamiVersion, Update};
use crate::ui::{catalog_icon, color, icon};
use crate::Shell;

/// The title of Update Now's run (and how the window knows it's this one going).
pub const UPDATING: &str = "Updating";

/// The timer that installs updates once a day (base's on namiOS; nami's own elsewhere).
pub const TIMER: &str = "nami-update.timer";

/// How many apps or lines of notes show before Show All.
const FEW: usize = 4;
const FEW_NOTES: usize = 6;

#[derive(Default)]
pub struct Updates {
    pub apps: Option<Result<Vec<Update>, String>>,
    pub nami: Option<Result<NamiVersion, String>>,
    pub catalog: Option<String>,
    /// This computer's version, and the one waiting for its next start.
    pub running: Option<u64>,
    pub waiting: Option<u64>,
    /// nami's version as the computer started, and the one that made what's waiting.
    pub started: Option<String>,
    pub ready: Option<String>,
    /// Getting the newest catalog, before looking.
    pub checking: bool,
    /// Show All, pressed: every app, all the notes.
    pub all_apps: bool,
    pub all_notes: bool,
}

impl Updates {
    /// How many there are, for the sidebar.
    pub fn count(&self) -> usize {
        let apps = self.apps.as_ref().and_then(|a| a.as_ref().ok()).map_or(0, Vec::len);
        apps + self.newer_nami().is_some() as usize + self.waiting.is_some() as usize
    }

    /// A newer nami, if there is one.
    fn newer_nami(&self) -> Option<&NamiVersion> {
        self.nami.as_ref().and_then(|t| t.as_ref().ok()).filter(|t| t.newer)
    }

    fn app_list(&self) -> &[Update] {
        self.apps.as_ref().and_then(|a| a.as_ref().ok()).map_or(&[], Vec::as_slice)
    }
}

/// Look again, in the background.
pub fn look(cx: &mut Context<Shell>) {
    cx.spawn(async move |this, cx| {
        let found = cx.background_executor().spawn(async move { found() }).await;
        let _ = this.update(cx, |g, cx| keep(g, found, cx));
    })
    .detach();
}

/// What's newer, found out.
fn found() -> Updates {
    let (running, waiting) = nami::os_versions();
    let (started, ready) = nami::os_nami_versions();
    Updates { apps: Some(nami::updates()), nami: Some(nami::nami_version()), catalog: nami::catalog_date(), running, waiting, started, ready, ..Default::default() }
}

fn keep(g: &mut Shell, found: Updates, cx: &mut Context<Shell>) {
    let attrs = found.apps.as_ref().and_then(|a| a.as_ref().ok()).map(|a| a.iter().map(|u| u.attr.clone()).collect()).unwrap_or_default();
    // (What was shown in full stays so.)
    let (all_apps, all_notes) = (g.updates.all_apps, g.updates.all_notes);
    g.updates = Updates { all_apps, all_notes, ..found };
    g.learn_names(attrs, cx);
    cx.notify();
}

/// Check for updates, as the page opens (and Check Now): the newest catalog when the one here
/// is over an hour old (or `fresh`), then what's newer in it. What was found stays shown
/// meanwhile.
pub fn check(g: &mut Shell, fresh: bool, cx: &mut Context<Shell>) {
    if g.updates.checking || g.run.as_ref().is_some_and(|r| r.finished.is_none()) {
        return;
    }
    let stale = fresh || g.updates.catalog.as_deref().and_then(utc_secs).is_none_or(|t| now() - t > 3600);
    g.updates.checking = true;
    cx.notify();
    cx.spawn(async move |this, cx| {
        let found = cx.background_executor().spawn(async move {
            // (Offline: what's here is looked in, and the check says if nami's site couldn't be reached.)
            let refreshed = if stale { nami::refresh_catalog() } else { Ok(()) };
            let mut found = found();
            if let (Err(e), Some(Ok(_))) = (refreshed, &found.nami) {
                found.nami = Some(Err(e));
            }
            found
        }).await;
        let _ = this.update(cx, |g, cx| keep(g, found, cx));
    })
    .detach();
}

/// Seconds since 1970 of `2026-11-18 08:03` (UTC, as `nami status` says the catalog's time).
fn utc_secs(at: &str) -> Option<i64> {
    let (date, time) = at.split_once(' ')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let (h, min) = time.split_once(':')?;
    // Howard Hinnant's days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    Some(days * 86400 + h.parse::<i64>().ok()? * 3600 + min.get(..2)?.parse::<i64>().ok()? * 60)
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

const RESTART: &str = "/etc/nami-os/update/restart";

/// `script`, or where tray's packages put it (a config repo not updated yet).
fn script_at(script: &'static str) -> String {
    let old = script.replace("/etc/nami-os/", "/etc/tray-os/");
    if !std::path::Path::new(script).exists() && std::path::Path::new(&old).exists() { old } else { script.to_string() }
}

/// Whether the computer installs updates by itself: as its file says, for whoever may change
/// it (`updates: manual`, or the timer in `without:`); for anyone else, as it's set up now
/// (on a Mac, launchd's job: nami's src/auto_update.rs).
pub fn automatic(g: &Shell) -> bool {
    if g.may_change() {
        let without = super::list_at(&g.system, "without").iter().any(|w| w == TIMER);
        g.system["updates"].as_str() != Some("manual") && !without
    } else if cfg!(target_os = "macos") {
        std::path::Path::new("/Library/LaunchDaemons/dev.namios.update.plist").exists()
    } else {
        std::path::Path::new("/etc/systemd/system/timers.target.wants").join(TIMER).exists()
    }
}

/// Turn automatic updates on or off, in system.nami (`updates: manual`; on again, the
/// key goes, or the timer out of `without:` where it was written that way).
pub fn set_automatic(g: &mut Shell, on: bool, cx: &mut Context<Shell>) {
    let op = if !on {
        json!({ "set": { "path": ["updates"], "value": "manual" } })
    } else if super::list_at(&g.system, "without").iter().any(|w| w == TIMER) {
        json!({ "remove": { "path": ["without"], "value": TIMER } })
    } else {
        json!({ "unset": { "path": ["updates"] } })
    };
    g.edit_saying(true, op, if on { "Turning on automatic updates." } else { "Turning off automatic updates." }, cx);
}

/// What Update Now runs, each with what's said while it goes.
fn steps(g: &Shell) -> Vec<(String, Vec<String>)> {
    let u = &g.updates;
    let mut steps = Vec::new();
    let newer = u.newer_nami().map(|t| t.latest.clone());
    // The computer's part: on namiOS always (its apps come with it); elsewhere for whoever may
    // change it, or a newer nami (an admin's password asked for).
    if g.nami_os || g.may_change() || newer.is_some() {
        let said = match (&newer, g.nami_os) {
            (Some(v), true) => format!("Installing namiOS {v}"),
            (None, true) => "Updating namiOS".to_string(),
            (Some(v), false) => format!("Installing nami {v}"),
            (None, false) => "Updating the system's apps".to_string(),
        };
        steps.push((said, vec!["os-update".into()]));
    }
    if !u.app_list().is_empty() {
        steps.push(("Updating your apps".to_string(), vec!["upgrade".into(), "--user".into()]));
    }
    steps
}

/// Update Now: everything, in turn.
fn update_now(g: &mut Shell, cx: &mut Context<Shell>) {
    let cmds = steps(g).into_iter().map(|(_, args)| (args, None)).collect();
    super::hide_details("updates-failed");
    g.start(UPDATING, cmds, cx);
}

/// Where the page is: what's said at its top.
enum State {
    Checking,
    Updating,
    Failed(String),
    CouldntCheck(String),
    Restart,
    Available,
    UpToDate,
}

fn state(g: &Shell) -> State {
    let u = &g.updates;
    match g.run.as_ref().filter(|r| r.title == UPDATING) {
        Some(r) if r.finished.is_none() => return State::Updating,
        Some(r) => {
            if let Some(Err(e)) = &r.finished {
                return State::Failed(e.clone());
            }
        }
        None => {}
    }
    if u.waiting.is_some() {
        return State::Restart;
    }
    if u.checking || u.apps.is_none() || u.nami.is_none() {
        return State::Checking;
    }
    if u.newer_nami().is_some() || !u.app_list().is_empty() {
        return State::Available;
    }
    match (&u.nami, &u.apps) {
        (Some(Err(e)), _) | (_, Some(Err(e))) => State::CouldntCheck(e.clone()),
        _ => State::UpToDate,
    }
}

/// `24 MB`.
fn size_words(bytes: u64) -> String {
    let mb = bytes as f64 / 1e6;
    if mb >= 1000. { format!("{:.1} GB", mb / 1000.) } else { format!("{} MB", mb.ceil().max(1.) as u64) }
}

/// The product, as this computer has it: namiOS, or nami on another distribution.
fn product(g: &Shell) -> &'static str {
    if g.nami_os { "namiOS" } else { "nami" }
}

/// The status at the top: a mark, what's so and a line under it, and its one main button.
fn status(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    let busy = g.run.as_ref().is_some_and(|r| r.finished.is_none()) || g.busy();
    let changed = !g.before.is_empty();
    let product = product(g);
    let current = g.updates.nami.as_ref().and_then(|n| n.as_ref().ok()).map_or_else(String::new, |n| n.current.clone());
    let mark = |turning: bool| {
        div().size(px(44.)).flex_none().rounded(px(12.)).bg(crate::ui::ink(0x10)).flex().items_center().justify_center().child(if turning {
            crate::motion::spinner("updates-spin", 22., color::muted(), true)
        } else {
            crate::icons::glyph("updates", 22., color::muted()).into_any_element()
        })
    };
    let words = |title: String, line: String| {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(div().text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).child(title))
            // (One line's room always: nothing under it jumps as the words change.)
            .child(div().min_h(px(18.)).text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(line))
    };
    let row = |turning: bool| div().flex().items_center().gap(px(14.)).child(mark(turning));
    let state = state(g);
    let top = match &state {
        State::Checking => row(true).child(words("Checking for updates…".into(), last_checked(g).map(|c| format!("Last checked {c}")).unwrap_or_default())),
        // (Checked as the page opened: Check Now, grey, for a second look.)
        State::UpToDate => {
            let checked = last_checked(g).map(|c| format!(" · Last checked {c}")).unwrap_or_default();
            row(false)
                .child(words("Your computer is up to date.".into(), format!("{product} {current}{checked}")))
                .child(crate::ui::grey("updates-check", "Check Now").stop(cx).on_click(cx.listener(|g, _, _, cx| check(g, true, cx))))
        }
        State::CouldntCheck(why) => {
            let why = why.clone();
            return div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    row(false)
                        .child(words("Couldn't check for updates.".into(), "Check your internet connection, then try again.".into()))
                        .child(crate::ui::main_button("updates-check-again", "Try Again", true).stop(cx).on_click(cx.listener(|g, _, _, cx| check(g, true, cx)))),
                )
                .child(div().pl(px(58.)).child(details("updates-check-failed", why)));
        }
        State::Restart => {
            let v = g.updates.ready.clone().unwrap_or(current.clone());
            let ready = if v.is_empty() { "An update for namiOS is ready.".to_string() } else { format!("namiOS {v} is ready.") };
            let can = std::path::Path::new(&script_at(RESTART)).exists();
            row(false)
                .child(words("Restart to finish updating.".into(), format!("{ready} Your work stays as it is until you restart.")))
                .when(can, |d| d.child(crate::ui::main_button("updates-restart", "Restart Now", true).stop(cx).on_click(|_, _, _| {
                    let _ = std::process::Command::new(script_at(RESTART)).spawn();
                })))
        }
        State::Available => {
            let u = &g.updates;
            let n = u.app_list().len();
            let apps = (n > 0).then(|| format!("{n} app update{}", if n == 1 { "" } else { "s" }));
            let (title, line) = match u.newer_nami() {
                Some(t) => (
                    format!("{product} {} is ready to install.", t.latest),
                    [Some(format!("{product} {}{}", t.latest, t.size.map(|s| format!(" ({})", size_words(s))).unwrap_or_default())), apps].into_iter().flatten().collect::<Vec<_>>().join(" and "),
                ),
                None => ("Updates are available.".to_string(), apps.unwrap_or_default()),
            };
            let can = !busy && !changed;
            row(false)
                .child(words(title, if changed { "Apply or discard your other changes first.".into() } else { line }))
                .child(crate::ui::main_button("updates-now", "Update Now", can).stop(cx).when(can, |b| b.on_click(cx.listener(|g, _, _, cx| update_now(g, cx)))))
        }
        State::Updating => {
            let (line, part) = g.run.as_ref().map(|r| going(g, r)).unwrap_or_default();
            return div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(row(true).child(words("Updating…".into(), line)))
                .child(div().pl(px(58.)).child(crate::motion::bar("updates-bar", part, 4., color::key())));
        }
        State::Failed(why) => {
            let why = why.clone();
            return div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    row(false)
                        .child(words("The update couldn't be installed.".into(), "Try again later. Your computer is as it was.".into()))
                        .child(crate::ui::main_button("updates-again", "Try Again", !busy).stop(cx).when(!busy, |b| b.on_click(cx.listener(|g, _, _, cx| update_now(g, cx))))),
                )
                .child(div().pl(px(58.)).child(details("updates-failed", why)));
        }
    };
    top
}

/// What nami said of what went wrong, behind Show Details (quiet words, a disclosure as a
/// Mac's): without the paths that landed before it.
fn details(id: &'static str, said: String) -> Div {
    let said: Vec<&str> = said.lines().filter(|l| !l.trim().is_empty() && crate::progress::landed(l).is_none()).collect();
    let said = said[said.len().saturating_sub(6)..].join("\n");
    let shown = super::showing_details(id);
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.))
                .text_size(px(text::CALLOUT))
                .text_color(rgb(color::muted()))
                .cursor_pointer()
                .hover(|s| s.text_color(rgb(color::text())))
                .child(if shown { "Hide Details" } else { "Show Details" })
                .child(crate::icons::glyph("chevron", 11., color::muted()))
                .on_click(move |_, window, _| super::flip_details(id, window)),
        )
        .when(shown && !said.is_empty(), |d| d.child(div().text_size(px(text::META)).font_family("Geist Mono").text_color(rgb(color::muted())).child(said)))
}

/// How far Update Now is: what it's doing, and how much is done (none: not known yet).
fn going(g: &Shell, r: &crate::Run) -> (String, Option<f32>) {
    let steps = steps_of(g, r);
    let n = r.cmds.len().max(1);
    let doing = steps.get(r.step).cloned().unwrap_or_else(|| "Updating".to_string());
    let p = crate::progress::Progress::read(&r.lines[r.step_at.min(r.lines.len())..]);
    let within = (p.total > 0).then(|| p.done as f32 / p.total as f32);
    let part = within.map(|w| (r.step as f32 + w) / n as f32).or_else(|| (r.step > 0).then(|| r.step as f32 / n as f32));
    let secs = r.since.elapsed().as_secs_f32();
    let left = part.filter(|p| *p > 0.15 && secs > 10.).map(|p| secs * (1. - p) / p);
    let line = match (left, p.total > 0 && p.done < p.total) {
        (Some(s), _) if s < 60. => format!("{doing}… Less than a minute remaining."),
        (Some(s), _) => format!("{doing}… About {} minutes remaining.", (s / 60.).ceil() as u64),
        (None, true) => format!("{doing}… Downloading {}.", size_words((p.mib * 1.048576e6) as u64)),
        (None, false) => format!("{doing}…"),
    };
    (line, part)
}

/// What each of a run's commands is said as (it was planned with what was known then).
fn steps_of(g: &Shell, r: &crate::Run) -> Vec<String> {
    let planned = steps(g);
    r.cmds
        .iter()
        .map(|(args, _)| planned.iter().find(|(_, a)| a == args).map(|(s, _)| s.clone()).unwrap_or_else(|| if args.first().is_some_and(|a| a == "upgrade") { "Updating your apps".into() } else { "Updating".into() }))
        .collect()
}

/// nami's release notes, as Updates shows them: a paragraph, `## ` a heading, `- ` a point.
fn notes(said: &str, all: bool) -> (Div, bool) {
    let lines: Vec<&str> = said.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let more = lines.len() > FEW_NOTES && !all;
    let shown = if more { &lines[..FEW_NOTES] } else { &lines[..] };
    let body = div().flex().flex_col().gap(px(5.)).children(shown.iter().enumerate().map(|(i, l)| {
        let plain = |t: &str| t.replace("**", "");
        if let Some(h) = l.strip_prefix("## ") {
            div().when(i > 0, |d| d.pt(px(6.))).text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(plain(h))
        } else if let Some(point) = l.strip_prefix("- ") {
            div().flex().gap(px(8.)).text_size(px(text::BODY)).child(div().flex_none().text_color(rgb(color::muted())).child("•")).child(div().flex_1().min_w_0().child(plain(point)))
        } else {
            div().text_size(px(text::BODY)).child(plain(l))
        }
    }));
    (body, more)
}

/// Show All, at a list's end: the rest of it.
fn show_all(id: &'static str, label: String, cx: &mut Context<Shell>, open: impl Fn(&mut Shell) + 'static) -> AnyElement {
    crate::ui::link(id, label)
        .text_size(px(text::CALLOUT))
        .on_click(cx.listener(move |g, _, _, cx| {
            open(g);
            cx.notify();
        }))
        .into_any_element()
}

/// What's New: nami's notes (of the version that's ready, or about to be installed), and the
/// apps that change.
fn whats_new(g: &mut Shell, cx: &mut Context<Shell>) -> Vec<Div> {
    let u = &g.updates;
    let mut out = Vec::new();
    // The notes of what's coming, or, waiting for a restart, of the nami that made it (when
    // that's another than the computer started with).
    let notes_of = match u.newer_nami() {
        Some(t) => t.notes.clone().map(|n| (t.latest.clone(), n)),
        None if u.waiting.is_some() => u.nami.as_ref().and_then(|t| t.as_ref().ok()).filter(|t| u.started.as_ref().is_some_and(|s| *s != t.current)).and_then(|t| t.notes.clone().map(|n| (t.current.clone(), n))),
        None => None,
    };
    if let Some((v, said)) = notes_of {
        let (body, more) = notes(&said, u.all_notes);
        let mut card = crate::look::group().px(px(crate::look::shape::MODULE_INSET + 4.)).py(px(14.)).child(body);
        if more {
            card = card.child(div().pt(px(8.)).child(show_all("updates-notes-all", "Show All".into(), cx, |g| g.updates.all_notes = true)));
        }
        out.push(div().flex().flex_col().gap(px(6.)).child(crate::look::group_heading(&format!("What's New in {} {v}", product(g)))).child(card));
    }
    let apps = g.updates.app_list().to_vec();
    if !apps.is_empty() {
        let n = apps.len();
        let shown = if g.updates.all_apps || n <= FEW + 1 { n } else { FEW };
        let rows: Vec<Div> = apps
            .iter()
            .take(shown)
            .enumerate()
            .map(|(i, u)| {
                crate::look::group_row(i + 1 == shown && shown == n)
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(px(10.))
                            .child(icon(catalog_icon(&u.attr).or_else(|| g.catalog.get(&u.attr).and_then(|f| f.icon.clone())).as_deref(), &u.attr, 28.))
                            .child(div().min_w_0().truncate().text_size(px(text::BODY)).child(g.app_name(&u.attr)))
                            .when(u.attr.starts_with("flathub:"), |d| d.child(crate::ui::flathub_label())),
                    )
                    .child(div().flex_none().text_size(px(text::CALLOUT)).font_family("Geist Mono").text_color(rgb(color::muted())).child(format!("{} → {}", u.from, u.to)))
            })
            .collect();
        let mut card = crate::look::group().children(rows);
        if shown < n {
            card = card.child(crate::look::group_row(true).child(show_all("updates-apps-all", format!("Show All {n}"), cx, |g| g.updates.all_apps = true)));
        }
        out.push(div().flex().flex_col().gap(px(6.)).child(crate::look::group_heading("App Updates")).child(card));
    }
    out
}

/// Install updates automatically: the computer's, an admin's to change (greyed, with Unlock
/// over it, for anyone else).
fn automatic_card(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    use crate::ui::{said_switch, setting_card as card, setting_row as row, switch};
    let may = g.may_change();
    let on = automatic(g);
    let label = "Install updates automatically";
    let hint = match (on, g.nami_os) {
        (true, true) => "Updates download in the background. namiOS finishes updating the next time you restart.",
        (true, false) => "nami and the system's apps update once a day, in the background.",
        (false, _) => "Updates install only when you click Update Now.",
    };
    let toggle = said_switch(switch(ElementId::Name("updates-automatic".into()), on), label, on);
    let toggle = if may { toggle.stop(cx).on_click(cx.listener(move |g, _, _, cx| set_automatic(g, !on, cx))).into_any_element() } else { toggle.into_any_element() };
    div().flex().flex_col().gap(px(10.)).children(super::lock_row(g, "updates-unlock", cx)).child(super::locked(!may, card("", vec![row(label, hint, toggle, true)])))
}

pub fn render(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    let top = status(g, cx);
    let mut page = div().flex().flex_col().gap(px(18.)).child(crate::look::group().px(px(crate::look::shape::MODULE_INSET + 4.)).py(px(16.)).child(top));
    // (While updating or failed, what's coming stays listed under it.)
    page = page.children(whats_new(g, cx));
    page = page.child(automatic_card(g, cx));
    if flathub_apps(g) {
        page = page.child(crate::look::footnote("Flathub apps update here, and by themselves once a day."));
    }
    // Apps from downloads (a .deb, .rpm, AppImage): their own updates.
    if !g.install_file.apps.is_empty() {
        page = page.child(crate::pages::app_updates::section(g, cx));
    }
    page
}

/// When it last looked (the catalog's time; none for one that came with the system, 1970's).
fn last_checked(g: &Shell) -> Option<String> {
    g.updates.catalog.as_deref().and_then(utc_secs).filter(|t| *t > 365 * 86400).map(|t| super::app_updates::when(t as u64))
}

/// Whether the person has apps from Flathub (in their user.nami).
fn flathub_apps(g: &Shell) -> bool {
    crate::pages::packages(&g.user).iter().any(|e| crate::pages::entry_name(e).starts_with("flathub:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_catalogs_time() {
        assert_eq!(utc_secs("1970-01-01 00:00"), Some(0));
        assert_eq!(utc_secs("2026-11-18 08:03"), Some(1_794_988_980));
        assert_eq!(utc_secs("2024-02-29 23:59 UTC"), Some(1_709_251_140));
        assert_eq!(utc_secs("yesterday"), None);
    }

    #[test]
    fn sizes_in_words() {
        assert_eq!(size_words(24_300_000), "25 MB");
        assert_eq!(size_words(1_200), "1 MB");
        assert_eq!(size_words(1_450_000_000), "1.4 GB");
    }

    #[test]
    fn notes_fold_after_a_few_lines() {
        let said = "Intro.\n\n## Updates\n- One\n- Two\n- Three\n## Desktop\n- Four\n- Five\n- Six\n- Seven";
        assert!(notes(said, false).1);
        assert!(!notes(said, true).1);
        assert!(!notes("- Only this.", false).1);
    }
}
