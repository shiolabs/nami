//! History: every step the apps went through (the computer's for admins,
//! yours for others), newest first, each to go back to (`nami undo`). Each opens to
//! what's different in its files from now, in words (`nami diff`), and Go back to this
//! shows that first; the raw lines are a click away.

use std::collections::{HashMap, HashSet};

use gpui::prelude::*;
use crate::look::Press;
use gpui::{AnyElement, Context, Div, ElementId, FontWeight, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled, div, rgb};
use crate::look::px;
use crate::look::text;

use crate::file_diff::{FileDiff, Mark, Row, Section};
use crate::nami::{Step, StepDiff};
use crate::ui::color;
use crate::Shell;

/// What's open on the page: the steps showing what's different, and their diffs.
#[derive(Default)]
pub struct View {
    pub open: HashSet<u64>,
    /// Each step's files against the step in use now (from now to it: what going back changes).
    pub diffs: HashMap<u64, Result<StepDiff, String>>,
    /// The step in use when they were asked for.
    pub against: Option<u64>,
    /// The step whose Go back to this is asking first.
    pub asking: Option<u64>,
    /// Steps showing their raw lines.
    pub raw: HashSet<u64>,
}

/// What a step changed, in a line.
pub fn summary(s: &Step, name: impl Fn(&str) -> String) -> String {
    let mut parts: Vec<String> = Vec::new();
    let names = |e: &[crate::nami::Entry]| e.iter().take(4).map(|x| name(&x.attr)).collect::<Vec<_>>().join(", ") + if e.len() > 4 { &"…"[..] } else { "" };
    if !s.added.is_empty() {
        parts.push(format!("added {}", names(&s.added)));
    }
    if !s.removed.is_empty() {
        parts.push(format!("uninstalled {}", names(&s.removed)));
    }
    if !s.changed.is_empty() {
        parts.push(format!("updated {}", names(&s.changed)));
    }
    if s.files > 0 {
        parts.push(format!("{} files", s.files));
    }
    if parts.is_empty() { "no app changes".into() } else { parts.join(" · ") }
}

/// `nami undo [n]`, in the history this page shows (the computer's, for an admin on namiOS).
fn undo_args(n: Option<u64>) -> Vec<String> {
    let mut args = vec!["undo".to_string()];
    args.extend(crate::nami::system_side().iter().map(|a| a.to_string()));
    args.extend(n.map(|n| n.to_string()));
    args
}

/// Ask nami what's different between step `n` and the one in use (once).
fn fetch(g: &mut Shell, n: u64, cx: &mut Context<Shell>) {
    let Some(now) = g.history_view.against else { return };
    if g.history_view.diffs.contains_key(&n) {
        return;
    }
    cx.spawn(async move |this, cx| {
        let d = cx.background_executor().spawn(async move { crate::nami::diff(now, n) }).await;
        let _ = this.update(cx, |g, cx| {
            if g.history_view.against == Some(now) {
                g.history_view.diffs.insert(n, d);
            }
            cx.notify();
        });
    })
    .detach();
}

/// A change in words, as Settings says it: `Frosted glass: Subtle → Medium`,
/// an app by its name.
pub fn row_text(g: &Shell, s: &Section, r: &Row) -> String {
    if s.key.first().is_some_and(|k| k == "packages") && r.item && r.path.len() == 2 {
        let name = g.app_name(&r.path[1]);
        return match (r.mark, &r.from, &r.to) {
            (Mark::Changed, Some(f), Some(t)) => format!("{name}: {} → {}", crate::file_diff::say(f), crate::file_diff::say(t)),
            _ => name,
        };
    }
    let words = |v: &str| <crate::os::Active as crate::os::Os>::in_words(&r.path, v).map(|(_, w)| w).unwrap_or_else(|| crate::file_diff::say(v));
    match <crate::os::Active as crate::os::Os>::in_words(&r.path, "") {
        Some((label, _)) => match (r.mark, &r.from, &r.to) {
            (Mark::Changed, Some(f), Some(t)) => format!("{label}: {} → {}", words(f), words(t)),
            (Mark::Added, _, Some(v)) | (Mark::Removed, Some(v), _) => format!("{label}: {}", words(v)),
            _ => label.to_string(),
        },
        None => crate::file_diff::row_words(&s.key, r),
    }
}

/// A file's raw lines, as `diff -u` has them: what's added and taken off in slate.
pub fn raw_lines(raw: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .px(px(12.))
        .py(px(10.))
        .rounded(px(crate::look::shape::ROW))
        .bg(color::push())
        .font_family("Geist Mono")
        .text_size(px(text::META))
        .children(raw.lines().map(|l| {
            let c = match l.chars().next() {
                Some('+') | Some('-') if !l.starts_with("+++") && !l.starts_with("---") => color::key(),
                Some('@') => color::faint(),
                _ => color::muted(),
            };
            div().text_color(rgb(c)).when(l.starts_with('-') && !l.starts_with("---"), |d| d.opacity(0.75)).child(if l.is_empty() { " ".to_string() } else { l.to_string() })
        }))
}

/// What going back to step `n` changes in the files, by section.
fn changes(g: &Shell, n: u64, cx: &mut Context<Shell>) -> Div {
    let quiet = |what: String| div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(what);
    let files: Vec<FileDiff> = match g.history_view.diffs.get(&n) {
        None => return div().child(crate::motion::looking(ElementId::NamedInteger("diff-looking".into(), n), "Loading…")),
        Some(Err(why)) => return quiet(format!("Couldn't compare the changes. {why}")),
        Some(Ok(d)) => d.files.clone(),
    };
    let any_raw = files.iter().any(|f| !f.raw.is_empty());
    let showing_raw = g.history_view.raw.contains(&n);
    let mut out = div().flex().flex_col().gap(px(10.));
    if files.iter().all(|f| f.sections.is_empty()) {
        out = out.child(quiet(if any_raw { "The settings are the same as now. Only comments in the file differ.".into() } else { "The files are the same as now.".into() }));
    }
    for f in &files {
        if f.sections.is_empty() {
            continue;
        }
        // (On a computer nami made, going back brings back its apps; the rest stays.)
        let back = |s: &Section| f.restores.is_empty() || f.restores.iter().any(|r| *r == s.key.join("."));
        let (goes, stays): (Vec<&Section>, Vec<&Section>) = f.sections.iter().partition(|s| back(s));
        let section = |s: &Section, faded: bool| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .when(faded, |d| d.opacity(0.7))
                .child(div().text_size(px(text::META)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(color::muted())).child(s.title.clone()))
                .children(s.rows.iter().map(|r| {
                    let mark = match r.mark {
                        Mark::Added => "+",
                        Mark::Removed => "−",
                        Mark::Changed => "",
                    };
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(8.))
                        .child(div().w(px(10.)).flex_none().text_size(px(text::CALLOUT)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(color::key())).child(mark))
                        .child(div().flex_1().min_w_0().text_size(px(text::CALLOUT)).when(r.mark == Mark::Removed, |d| d.text_color(rgb(color::muted()))).child(row_text(g, s, r)))
                }))
        };
        out = out.child(div().font_family("Geist Mono").text_size(px(text::META)).text_color(rgb(color::faint())).child(f.file.clone()));
        out = out.children(goes.into_iter().map(|s| section(s, false)));
        if !stays.is_empty() {
            out = out.child(div().mt(px(4.)).text_size(px(text::META)).text_color(rgb(color::muted())).child("These differ, but won't change because they're set in system.nami:"));
            out = out.children(stays.into_iter().map(|s| section(s, true)));
        }
    }
    if any_raw {
        out = out.child(
            div()
                .id(ElementId::NamedInteger("raw".into(), n))
                .text_size(px(text::META))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(color::key()))
                .cursor_pointer()
                .press()
                .hover(|s| s.underline())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.history_view.raw.remove(&n) {
                        this.history_view.raw.insert(n);
                    }
                    cx.notify();
                }))
                .child(if showing_raw { "Hide Raw Diff" } else { "Show Raw Diff" }),
        );
        if showing_raw {
            out = out.children(files.iter().filter(|f| !f.raw.is_empty()).map(|f| raw_lines(&f.raw)));
        }
    }
    out
}

/// What syncing your setup brought from your other computers, each to undo (Sync has them too).
fn from_other_computers(g: &Shell, cx: &mut Context<Shell>) -> Option<Div> {
    let s = g.sync.setup.as_ref()?.as_ref().ok()?;
    if !s.on || s.entries.is_empty() {
        return None;
    }
    let entries: Vec<crate::nami::SetupEntry> = s.entries.iter().take(10).cloned().collect();
    Some(div().flex().flex_col().gap(px(8.)).child(crate::look::group_heading("From Your Other Computers")).child(super::sync::changes_list(g, &entries, "history-synced", cx)))
}

/// A day as it's in this computer's time: its year, its day of the year, its weekday (0:
/// Sunday), its day of the month and its month (0: January).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Day {
    pub year: i32,
    pub yday: i32,
    pub wday: i32,
    pub mday: i32,
    pub mon: i32,
}

impl Day {
    #[cfg(unix)]
    pub fn of(at: u64) -> Day {
        let tm = crate::clock::local(at as libc::time_t);
        Day { year: tm.tm_year + 1900, yday: tm.tm_yday, wday: tm.tm_wday, mday: tm.tm_mday, mon: tm.tm_mon }
    }

    /// How many days `self` is before `now` (about: a year's days are taken as 365).
    fn before(self, now: Day) -> i32 {
        (now.year - self.year) * 365 + now.yday - self.yday
    }
}

/// A day's heading: Today, Yesterday, a weekday this week, else its date.
pub fn day_label(day: Day, now: Day) -> String {
    const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    match day.before(now) {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => DAYS[day.wday.clamp(0, 6) as usize].into(),
        _ if day.year == now.year => format!("{} {}", day.mday, MONTHS[day.mon.clamp(0, 11) as usize]),
        _ => format!("{} {} {}", day.mday, MONTHS[day.mon.clamp(0, 11) as usize], day.year),
    }
}

/// "Krita", "Krita and VS Code", "Steam, VS Code, ripgrep and 9 more".
fn names_of(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [a, b, c] => format!("{a}, {b} and {c}"),
        [a, b, c, rest @ ..] => format!("{a}, {b}, {c} and {} more", rest.len()),
    }
}

/// What a step did, as its line says it, and what's under that: ("Installed Krita", ""),
/// ("Updated 12 apps", "Steam, VS Code, ripgrep and 9 more").
pub fn step_title(s: &Step, name: impl Fn(&str) -> String) -> (String, String) {
    let names = |e: &[crate::nami::Entry]| e.iter().map(|x| name(&x.attr)).collect::<Vec<_>>();
    let (added, removed, changed) = (names(&s.added), names(&s.removed), names(&s.changed));
    let apps = |n: usize| if n == 1 { "1 app".to_string() } else { format!("{n} apps") };
    let kinds = [!added.is_empty(), !removed.is_empty(), !changed.is_empty()].iter().filter(|k| **k).count();
    let files = if s.files == 1 { "1 settings file".to_string() } else { format!("{} settings files", s.files) };
    match kinds {
        0 if s.files > 0 => ("Changed your settings".into(), files),
        0 if s.date.is_empty() => ("Initial setup".into(), String::new()),
        0 => ("Changed your setup".into(), String::new()),
        1 => {
            let (verb, list) = if !added.is_empty() { ("Installed", &added) } else if !removed.is_empty() { ("Removed", &removed) } else { ("Updated", &changed) };
            let title = if list.len() <= 2 { format!("{verb} {}", names_of(list)) } else { format!("{verb} {}", apps(list.len())) };
            let under = if list.len() > 2 { names_of(list) } else if s.files > 0 { format!("And {files}") } else { String::new() };
            (title, under)
        }
        _ => {
            let mut parts = Vec::new();
            if !added.is_empty() {
                parts.push(format!("installed {}", names_of(&added)));
            }
            if !removed.is_empty() {
                parts.push(format!("removed {}", names_of(&removed)));
            }
            if !changed.is_empty() {
                parts.push(format!("updated {}", apps(changed.len())));
            }
            let mut under = parts.join(", ");
            if let Some(c) = under.get(..1) {
                under = c.to_uppercase() + &under[1..];
            }
            ("Changed your apps".into(), under)
        }
    }
}

/// A step's picture: what it mostly did.
fn glyph_of(s: &Step) -> &'static str {
    if !s.added.is_empty() && s.removed.is_empty() {
        "plus"
    } else if !s.removed.is_empty() && s.added.is_empty() && s.changed.is_empty() {
        "minus"
    } else if !s.changed.is_empty() {
        "updates"
    } else if s.files > 0 {
        "settings"
    } else {
        "history"
    }
}

pub fn render(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    use crate::ui::{grey, ink, main_button, page_title, quiet, sheet};
    let running = g.run.as_ref().is_some_and(|r| r.finished.is_none());
    let can_undo = !running && g.history.as_ref().and_then(|h| h.as_ref().ok()).is_some_and(|s| s.iter().position(|s| s.current).is_some_and(|at| at + 1 < s.len()));
    let undo = grey("undo-last", "Undo Last Change").when(!can_undo, |b| b.opacity(0.45)).when(can_undo, |b| b.on_click(cx.listener(|this, _, _, cx| this.start("Undoing the last change", vec![(undo_args(None), None)], cx))));
    let whose = if g.admin() && !super::store_mode() { "Every change to this computer's apps is kept here." } else { "Every change you make is kept here." };
    let page = div().flex().flex_col().gap(px(18.)).pt(px(20.)).child(div().flex().items_start().gap(px(12.)).child(page_title("History", Some(div().child(format!("{whose} Go back to any of them.")).into_any_element()))).child(undo)).children(from_other_computers(g, cx));
    let steps = match &g.history {
        Some(Ok(s)) if s.is_empty() => return page.child(quiet("No changes yet. Each change you make appears here.")),
        Some(Ok(s)) => s.clone(),
        Some(Err(why)) => return page.child(super::failed("history-again", "Couldn't load the history.", why.clone(), |g, _| g.history = Some(crate::nami::history()), cx)),
        None => return page.child(super::looking("history-looking")),
    };
    // (Another step in use since: what's different is asked again.)
    let now = steps.iter().find(|s| s.current).map(|s| s.generation);
    if g.history_view.against != now {
        g.history_view = View { against: now, ..Default::default() };
    }
    let opened: Vec<u64> = g.history_view.open.iter().copied().collect();
    for n in opened {
        fetch(g, n, cx);
    }
    // The steps by day, newest first; each opens to what going back to it changes.
    let today = Day::of(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
    let mut days: Vec<(String, Vec<Step>)> = Vec::new();
    for s in steps {
        let label = if s.created > 0 { day_label(Day::of(s.created), today) } else { "Earlier".to_string() };
        match days.last_mut() {
            Some((l, list)) if *l == label => list.push(s),
            _ => days.push((label, vec![s])),
        }
    }
    let mut out = page;
    for (label, list) in days {
        let mut card = sheet().flex().flex_col();
        for (i, s) in list.iter().enumerate() {
            let n = s.generation;
            let (mut title, under) = step_title(s, |a| g.app_name(a));
            // (An admin's steps on namiOS are the computer's.)
            if title == "Changed your setup" && g.admin() && !super::store_mode() {
                title = "Changed this computer's setup".into();
            }
            let clock = if s.created > 0 { s.when().rsplit(' ').next().unwrap_or_default().to_string() } else { String::new() };
            let sub = [Some(clock).filter(|c| !c.is_empty()), s.current.then(|| "Now".to_string()), Some(under).filter(|u| !u.is_empty())].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            let open = g.history_view.open.contains(&n) && !s.current;
            let asking = g.history_view.asking == Some(n) && !s.current;
            let right: AnyElement = if s.current {
                div().px(px(9.)).h(px(22.)).flex().items_center().rounded_full().bg(ink(0x12)).text_size(px(text::META)).font_weight(FontWeight::SEMIBOLD).child("Current").into_any_element()
            } else {
                grey(ElementId::NamedInteger("back-to".into(), n), "Go Back to Here").when(running || asking, |b| b.opacity(0.45)).when(!running && !asking, |b| {
                    b.on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.history_view.open.insert(n);
                        this.history_view.asking = Some(n);
                        cx.notify();
                    }))
                }).into_any_element()
            };
            let row = div()
                .id(ElementId::NamedInteger("step".into(), n))
                .flex()
                .items_center()
                .gap(px(14.))
                .px(px(18.))
                .py(px(13.))
                .when(i > 0, |d| d.border_t_1().border_color(ink(0x10)))
                .when(!s.current, |d| d.cursor_pointer().hover(|s| s.bg(color::hover_pill())))
                .child(div().size(px(34.)).flex_none().rounded_full().bg(ink(0x0e)).flex().items_center().justify_center().child(crate::icons::glyph(glyph_of(s), 15., color::muted())))
                .child(div().flex_1().min_w_0().child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(title)).child(div().text_size(px(text::META)).text_color(rgb(color::muted())).truncate().child(sub)))
                .child(right)
                .when(!s.current, |d| {
                    d.on_click(cx.listener(move |this, _, _, cx| {
                        let v = &mut this.history_view;
                        if !v.open.remove(&n) {
                            v.open.insert(n);
                        } else if v.asking == Some(n) {
                            v.asking = None;
                        }
                        cx.notify();
                    }))
                });
            card = card.child(row);
            if open {
                card = card.child(
                    div().flex().flex_col().gap(px(10.)).px(px(18.)).pb(px(14.)).pl(px(66.))
                        .child(div().text_size(px(text::CALLOUT)).font_weight(FontWeight::MEDIUM).child(if asking { "Going back to here makes these changes. Continue?" } else { "Going back to here changes the following:" }))
                        .child(changes(g, n, cx))
                        .when(asking, |d| {
                            d.child(
                                div().flex().gap(px(8.))
                                    .child(grey(ElementId::NamedInteger("undo-no".into(), n), "Cancel").on_click(cx.listener(move |this, _, _, cx| {
                                        this.history_view.asking = None;
                                        cx.notify();
                                    })))
                                    .child(main_button(ElementId::NamedInteger("undo-yes".into(), n), "Go Back", !running).when(!running, |b| {
                                        b.on_click(cx.listener(move |this, _, _, cx| {
                                            this.history_view.asking = None;
                                            this.history_view.open.remove(&n);
                                            this.start("Going back", vec![(undo_args(Some(n)), None)], cx)
                                        }))
                                    })),
                            )
                        }),
                );
            }
        }
        out = out.child(div().flex().flex_col().gap(px(8.)).child(div().px(px(4.)).text_size(px(text::META)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(color::grouped_muted())).child(label.to_uppercase())).child(card));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nami::Entry;

    fn day(year: i32, yday: i32, wday: i32, mday: i32, mon: i32) -> Day {
        Day { year, yday, wday, mday, mon }
    }

    #[test]
    fn days_are_said_as_a_mac_says_them() {
        let now = day(2026, 318, 0, 15, 10);
        assert_eq!(day_label(now, now), "Today");
        assert_eq!(day_label(day(2026, 317, 6, 14, 10), now), "Yesterday");
        assert_eq!(day_label(day(2026, 313, 2, 10, 10), now), "Tuesday");
        assert_eq!(day_label(day(2026, 300, 2, 28, 9), now), "28 October");
        assert_eq!(day_label(day(2025, 300, 2, 28, 9), now), "28 October 2025");
    }

    fn step(added: &[&str], removed: &[&str], changed: &[&str], files: usize) -> Step {
        let e = |l: &[&str]| l.iter().map(|a| Entry { attr: a.to_string() }).collect();
        Step { generation: 2, date: "2026-11-15".into(), created: 1, current: false, added: e(added), removed: e(removed), changed: e(changed), files }
    }

    #[test]
    fn a_step_is_said_in_a_line() {
        let name = |a: &str| a.to_string();
        assert_eq!(step_title(&step(&["krita"], &[], &[], 0), name), ("Installed krita".into(), "".into()));
        assert_eq!(step_title(&step(&[], &["discord"], &[], 0), name), ("Removed discord".into(), "".into()));
        assert_eq!(step_title(&step(&[], &[], &["a", "b", "c", "d", "e"], 0), name), ("Updated 5 apps".into(), "a, b, c and 2 more".into()));
        assert_eq!(step_title(&step(&[], &[], &[], 2), name), ("Changed your settings".into(), "2 settings files".into()));
        assert_eq!(step_title(&step(&["x"], &["y"], &[], 0), name), ("Changed your apps".into(), "Installed x, removed y".into()));
    }
}
