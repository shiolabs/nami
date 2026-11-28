//! Storage, from Installed's Storage box: how full the disk is and how much of it nami uses,
//! in parts (`nami storage`: each store path counted once, in the first part that uses it),
//! the apps that take the most (each opening its page, where Remove is), and Clean Up.
//!
//! Clean Up frees what nothing needs: your older changes past the newest `cleanup:` keeps
//! (`nami gc --now --user`), then the computer's and the downloads nothing uses any more
//! (`nami gc --now`: through the daemon for an administrator; on a Mac as root, with an
//! administrator's password, as Uninstall nami… asks for it). It asks first, saying what goes
//! and that undoing back to those changes won't be possible, then shows how far it is, then
//! the numbers again. Someone who isn't an administrator on Linux has their own older changes
//! removed; an administrator frees the space they took.
//!
//! On namiOS, Settings has its own Storage page (the whole disk, the Trash, caches); this one
//! is nami's, wherever else it runs.

use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, ElementId, FontWeight, div, relative, rgb};

use crate::look::{Stop, px, text};
use crate::nami::{Freeing, Storage as Seen};
use crate::ui::{color, grey, main_button, page_title, quiet, sheet, sheet_row, tag};
use crate::{Page, Shell};

#[derive(Default)]
pub struct Storage {
    /// What nami said last (None: not asked yet).
    pub seen: Option<Result<Seen, String>>,
    /// Being read again (what's there stays shown until the new numbers come).
    pub reading: bool,
    pub cleaning: Cleaning,
    /// What the last Clean Up freed: nami's total before it, less after.
    pub freed: Option<u64>,
    /// nami's total as Clean Up started, to say what it freed.
    before: Option<u64>,
}

#[derive(Default, Debug, PartialEq)]
pub enum Cleaning {
    #[default]
    Idle,
    /// "Clean Up 2.3 GB?", over the window.
    Asking,
    /// Going: what nami's said, and the step (`Step`).
    Going { lines: Vec<String>, step: Step },
    /// It stopped: what was said.
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// Your own older changes (`nami gc --now --user`).
    Yours,
    /// The computer's, and what nothing uses (`nami gc --now`).
    Computers,
    /// A Mac's: the same as root, once the password's given.
    AsAdministrator,
}

impl Storage {
    pub fn cleaning(&self) -> bool {
        matches!(self.cleaning, Cleaning::Going { .. })
    }
}

/// Read what nami says, away from the window, and show it.
pub fn look(g: &mut Shell, cx: &mut Context<Shell>) {
    if g.storage.reading {
        return;
    }
    g.storage.reading = true;
    Shell::later(cx, crate::nami::storage, |g, seen, cx| {
        g.storage.reading = false;
        if let (Some(before), Ok(now)) = (g.storage.before.take(), &seen) {
            g.storage.freed = Some(before.saturating_sub(now.total));
        }
        if let Ok(s) = &seen {
            let attrs = s.largest.iter().map(|a| a.attr.clone()).collect();
            g.learn_names(attrs, cx);
        }
        g.storage.seen = Some(seen);
        cx.notify();
    });
}

/// "2.3 GB", "340 MB", "12 KB": as a Mac says sizes (1 GB is 1,000,000,000 bytes).
pub fn size(b: u64) -> String {
    let f = b as f64;
    if f >= 100e9 {
        format!("{:.0} GB", f / 1e9)
    } else if f >= 1e9 {
        format!("{:.1} GB", f / 1e9)
    } else if f >= 1e6 {
        format!("{:.0} MB", f / 1e6)
    } else if f >= 1e3 {
        format!("{:.0} KB", f / 1e3)
    } else if b > 0 {
        format!("{b} bytes")
    } else {
        "Zero KB".into()
    }
}

/// A part of what nami uses: its name, and what it is.
pub fn part_words(part: &str) -> Option<(&'static str, &'static str)> {
    Some(match part {
        "yours" => ("Your Apps", "Apps and tools you installed, and what they need to run."),
        "everyone" => ("Apps for Everyone", "Apps installed for everyone on this computer."),
        "projects" => ("Projects' Tools", "Tools your projects use, kept while their folders exist."),
        "others" => ("Other People's Apps", "Apps other people on this computer installed."),
        "running" => ("In Use by Open Programs", "Kept until the programs using them quit."),
        "older" => ("Older Changes", "Earlier versions of your apps, kept so you can undo changes in History."),
        "unused" => ("Not Used by Anything", "Downloads no app or change uses any more."),
        "casks" => ("Apps from Homebrew's Casks", "Mac apps installed from Homebrew's casks, stored once."),
        "nami" => ("nami", "The catalog, app icons and nami's own files."),
        _ => return None,
    })
}

/// `4`, `4 and 6`, `4–7`: change numbers, in ranges.
pub fn ranges(numbers: &[u64]) -> String {
    let mut n = numbers.to_vec();
    n.sort_unstable();
    n.dedup();
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < n.len() {
        let mut j = i;
        while j + 1 < n.len() && n[j + 1] == n[j] + 1 {
            j += 1;
        }
        match j - i {
            0 => parts.push(n[i].to_string()),
            1 => parts.extend([n[i].to_string(), n[j].to_string()]),
            _ => parts.push(format!("{}–{}", n[i], n[j])),
        }
        i = j + 1;
    }
    match parts.len() {
        0 => String::new(),
        1 => parts.remove(0),
        _ => {
            let last = parts.pop().unwrap_or_default();
            format!("{} and {last}", parts.join(", "))
        }
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// What Clean Up frees, in a sentence ("Frees 2.3 GB: 4 older changes and downloads nothing
/// uses."). None: there's nothing to clean up. `unused`: what nothing uses already.
pub fn frees_words(c: &Freeing, unused: u64) -> Option<String> {
    let changes = c.yours.len() + c.computers.len();
    if changes == 0 && c.bytes == 0 {
        return None;
    }
    let older = plural(changes, "older change", "older changes");
    let (they, their) = if changes == 1 { ("It shares", "its") } else { ("They share", "their") };
    Some(match (changes, c.bytes, c.now) {
        (0, b, _) => format!("Frees {}: downloads nothing uses.", size(b)),
        (_, 0, _) => format!("Removes {older}. {they} all {their} files with what you use now, so no space is freed."),
        (_, b, false) => format!("Removes {older}. An administrator can then free the {} {} {}.", size(b), if changes == 1 { "it" } else { "they" }, if changes == 1 { "uses" } else { "use" }),
        (_, b, true) if unused > 0 => format!("Frees {}: {older} and downloads nothing uses.", size(b)),
        (_, b, true) => format!("Frees {}: {older}.", size(b)),
    })
}

/// What's said asking first: the title, what goes (and that undo to it won't be possible), and
/// what stays. `unused`: what nothing uses already (downloads left behind).
pub fn ask_words(c: &Freeing, unused: u64, mac: bool) -> (String, String, String) {
    let title = if c.bytes > 0 && c.now { format!("Clean Up {}?", size(c.bytes)) } else { "Clean Up?".to_string() };
    let of = |whose: &str, n: &[u64]| format!("{whose} {} {}", if n.len() == 1 { "change" } else { "changes" }, ranges(n));
    let mut goes: Vec<String> = [(!c.yours.is_empty()).then(|| of("your", &c.yours)), (!c.computers.is_empty()).then(|| of("the computer's", &c.computers))].into_iter().flatten().collect();
    let mut said = String::new();
    if !goes.is_empty() {
        // (Only where there are any, and they go now.)
        if c.now && c.bytes > 0 {
            goes.push(if c.yours.len() + c.computers.len() == 1 { "the files only it uses" } else { "the files only they use" }.into());
        }
        let last = goes.pop().unwrap_or_default();
        let list = if goes.is_empty() { last } else { format!("{} and {last}", goes.join(", ")) };
        said = format!("This removes {list}. You can't undo back to {} in History afterward.", if c.yours.len() + c.computers.len() == 1 { "it" } else { "them" });
    }
    if unused > 0 && c.now {
        said += if said.is_empty() { "This deletes downloads nothing uses any more." } else { " Downloads nothing uses any more are deleted too." };
    }
    let mut stays = format!("Your apps as they are now and your {} most recent changes stay.", c.keep);
    if !c.now {
        stays += if c.yours.len() + c.computers.len() == 1 { " An administrator can then free the space it uses." } else { " An administrator can then free the space they use." };
    }
    if mac && c.now {
        stays += " macOS asks for an administrator's password.";
    }
    (title, said, stays)
}

/// How far Clean Up is, in words, and how much is done (None: not known). nami says `removing
/// 480 paths (2300.0 MiB)` as the store's cleaned up, then each path as it goes.
pub fn progress(lines: &[String], step: Step) -> (String, Option<f32>) {
    match step {
        Step::Yours => ("Removing older changes…".into(), None),
        Step::AsAdministrator => ("Cleaning up…".into(), None),
        Step::Computers => {
            let Some(at) = lines.iter().rposition(|l| l.starts_with("removing ") && l.contains(" paths")) else { return ("Removing older changes…".into(), None) };
            let total: usize = lines[at]["removing ".len()..].split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
            let done = lines[at + 1..].iter().filter(|l| crate::progress::landed(l).is_some()).count().min(total);
            (format!("Removing {done} of {total} items…"), Some(done as f32 / total.max(1) as f32))
        }
    }
}

/// Clean Up asked for: it asks first.
pub fn ask(g: &mut Shell, cx: &mut Context<Shell>) {
    if !g.busy() && !g.storage.cleaning() {
        g.storage.cleaning = Cleaning::Asking;
        cx.notify();
    }
}

/// Esc: the question put away (not while it's cleaning up).
pub fn back(g: &mut Shell) -> bool {
    match g.storage.cleaning {
        Cleaning::Asking | Cleaning::Failed(_) => {
            g.storage.cleaning = Cleaning::Idle;
            true
        }
        _ => false,
    }
}

/// The steps Clean Up runs: yours, then (an administrator, through the daemon) the computer's;
/// on a Mac the computer's as root, with a password.
pub fn steps(c: &Freeing, mac: bool) -> Vec<Step> {
    let mut s = vec![Step::Yours];
    if c.now {
        s.push(if mac { Step::AsAdministrator } else { Step::Computers });
    }
    s
}

/// Clean Up, once asked: each step, away from the window, then the numbers again.
fn go(g: &mut Shell, cx: &mut Context<Shell>) {
    let Some(Ok(seen)) = g.storage.seen.clone() else { return };
    let mac = crate::look::MAC;
    let steps = steps(&seen.cleanup, mac);
    g.storage.before = Some(seen.total);
    g.storage.freed = None;
    g.storage.cleaning = Cleaning::Going { lines: Vec::new(), step: steps[0] };
    cx.notify();
    cx.spawn(async move |this, cx| {
        use futures::StreamExt;
        for step in steps {
            let _ = this.update(cx, |g, cx| {
                g.storage.cleaning = Cleaning::Going { lines: Vec::new(), step };
                cx.notify();
            });
            let failed: Option<String> = match step {
                Step::AsAdministrator => {
                    let (program, args) = as_administrator(&crate::pages::uninstall_nami::nami_path(true));
                    let out = cx.background_executor().spawn(async move { std::process::Command::new(&program).args(&args).stdin(std::process::Stdio::null()).output() }).await;
                    match out {
                        Ok(o) if o.status.success() => None,
                        Ok(o) => {
                            let said = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
                            // (The password not given: nothing more is done, and nothing's said.)
                            if crate::pages::uninstall_nami::cancelled(true, o.status.code(), &said) { Some(String::new()) } else { Some(said.trim().to_string()) }
                        }
                        Err(e) => Some(format!("Couldn't ask for an administrator's password: {e}")),
                    }
                }
                _ => {
                    let args: Vec<String> = if step == Step::Yours { vec!["gc".into(), "--now".into(), "--user".into()] } else { vec!["gc".into(), "--now".into()] };
                    let mut rx = crate::nami::run(args, None);
                    let mut end = None;
                    while let Some(said) = rx.next().await {
                        match said {
                            crate::nami::Said::Line(l) => {
                                let _ = this.update(cx, |g, cx| {
                                    if let Cleaning::Going { lines, .. } = &mut g.storage.cleaning {
                                        lines.push(l);
                                    }
                                    cx.notify();
                                });
                            }
                            crate::nami::Said::Finished(r) => {
                                end = Some(r);
                                break;
                            }
                        }
                    }
                    match end {
                        Some(Ok(())) => None,
                        Some(Err(e)) => Some(e),
                        None => Some(String::new()),
                    }
                }
            };
            if let Some(why) = failed {
                let _ = this.update(cx, |g, cx| {
                    g.storage.cleaning = if why.is_empty() { Cleaning::Idle } else { Cleaning::Failed(why) };
                    g.storage.before = None;
                    look(g, cx);
                    g.history = Some(crate::nami::history());
                    cx.notify();
                });
                return;
            }
        }
        let _ = this.update(cx, |g, cx| {
            g.storage.cleaning = Cleaning::Idle;
            look(g, cx);
            g.history = Some(crate::nami::history());
            cx.notify();
        });
    })
    .detach();
}

/// A Mac's root step: `nami gc --now` as root, macOS asking for an administrator's password.
pub fn as_administrator(nami: &str) -> (String, Vec<String>) {
    let run = format!("{nami} gc --now");
    let script = format!("do shell script \"{}\" with prompt \"nami wants to free up space on this Mac.\" with administrator privileges", run.replace('\\', "\\\\").replace('"', "\\\""));
    ("/usr/bin/osascript".into(), vec!["-e".into(), script])
}

/// What nothing uses already (the part of it), from what nami said.
fn bytes_of(s: &Seen, part: &str) -> u64 {
    s.parts.iter().filter(|p| p.part == part).map(|p| p.bytes).sum()
}

/// Installed's Storage box: how much nami uses, what's available, what Clean Up frees; it
/// opens the page. Not on namiOS (Settings has Storage).
pub fn summary(g: &Shell, cx: &mut Context<Shell>) -> Option<Div> {
    if !crate::pages::store_mode() || g.role.is_err() {
        return None;
    }
    let line = match &g.storage.seen {
        None => "Calculating…".to_string(),
        Some(Err(_)) => "Couldn't check how much space nami uses.".to_string(),
        Some(Ok(s)) => {
            let mut l = format!("nami uses {} · {} available", size(s.total), size(s.disk.free));
            if s.cleanup.bytes > 0 && s.cleanup.now {
                l += &format!(" · {} can be freed", size(s.cleanup.bytes));
            }
            l
        }
    };
    Some(
        sheet().child(
            sheet_row(true)
                .id("storage-box")
                .py(px(12.))
                .cursor_pointer()
                .hover(|s| s.bg(color::hover_pill()))
                .on_click(cx.listener(|g, _, w, cx| g.go(Page::Storage, w, cx)))
                .role(gpui::Role::Button)
                .aria_label("Storage")
                .child(crate::icons::glyph("disk", 22., color::muted()))
                .child(div().flex_1().min_w_0().child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child("Storage")).child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(line)))
                .child(crate::ui::chevron("")),
        ),
    )
}

/// The disk, as a line: what nami uses (the accent; what Clean Up frees of it, the accent
/// lighter), everything else (grey), and what's free (the track). Each part at least a sliver.
pub fn shares(s: &Seen) -> [(u64, &'static str); 4] {
    let used = s.disk.used.max(1);
    let nami = s.total.min(used);
    let freeable = if s.cleanup.now { s.cleanup.bytes.min(nami) } else { 0 };
    [(nami - freeable, "nami"), (freeable, "freeable"), (used - nami, "other"), (s.disk.free, "free")]
}

fn tone(what: &str) -> gpui::Rgba {
    match what {
        "nami" => rgb(color::key_strong()),
        "freeable" => gpui::rgba((color::key_strong() << 8) | 0x8c),
        "other" => crate::ui::ink(0x38),
        _ => color::level_track(),
    }
}

fn disk_sheet(s: &Seen) -> Div {
    let total = (s.disk.used + s.disk.free).max(1) as f32;
    let shares = shares(s);
    let bar = div().flex().w_full().h(px(14.)).rounded(px(4.)).overflow_hidden().bg(color::level_track()).children(shares.iter().filter(|(b, w)| *b > 0 && *w != "free").map(|(b, w)| div().h_full().min_w(px(3.)).w(relative((*b as f32 / total).min(1.))).bg(tone(w))));
    let legend = [("nami", "nami", s.total.min(s.disk.used)), ("freeable", "Can be freed", shares[1].0), ("other", "Everything else", shares[2].0), ("free", "Available", s.disk.free)];
    let legend = div().flex().flex_wrap().gap_x(px(16.)).gap_y(px(6.)).children(legend.into_iter().filter(|(w, _, b)| *b > 0 || *w == "free").map(|(w, label, b)| {
        div().flex().items_center().gap(px(6.)).child(div().size(px(9.)).rounded_full().bg(tone(w))).child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child(format!("{label} {}", size(b))))
    }));
    let whose = if s.disk.container { "This Mac" } else { "This computer" };
    sheet().p(px(18.)).flex().flex_col().gap(px(12.))
        .child(
            div().flex().flex_wrap().items_baseline().justify_between().gap_x(px(12.))
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(format!("{whose}: {} of {} used", size(s.disk.used), size(s.disk.size))))
                .child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(format!("{} available", size(s.disk.free)))),
        )
        .child(bar)
        .child(legend)
}

/// What nami uses, part by part (those with nothing in them left out).
fn parts_sheet(s: &Seen) -> Div {
    let rows: Vec<Div> = s.parts.iter().filter(|p| p.bytes > 0).filter_map(|p| part_words(&p.part).map(|w| (w, p.bytes))).enumerate().map(|(n, ((name, about), b))| {
        sheet_row(n == 0)
            .child(div().flex_1().min_w_0().child(div().text_size(px(text::BODY)).child(name)).child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child(if s.disk.container { about.replace("this computer", "this Mac") } else { about.to_string() })))
            .child(div().flex_none().text_size(px(text::BODY)).text_color(rgb(color::muted())).child(size(b)))
    }).collect();
    sheet().flex().flex_col().children(rows)
}

/// Clean Up's own box: what it frees and the button, how far it is while it goes, or that
/// there's nothing to clean up.
fn clean_sheet(g: &Shell, s: &Seen, cx: &mut Context<Shell>) -> Div {
    let row = sheet_row(true).py(px(14.)).flex_wrap();
    let words = |title: String, line: String| div().flex_1().min_w(px(220.)).child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(title)).child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(line));
    let body = match &g.storage.cleaning {
        Cleaning::Going { lines, step } => {
            let (said, part) = progress(lines, *step);
            return sheet().child(row.child(words("Cleaning Up".into(), said))).child(div().px(px(18.)).pb(px(14.)).child(crate::motion::bar("storage-cleaning", part, 4., color::key_strong())));
        }
        _ => match frees_words(&s.cleanup, bytes_of(s, "unused")) {
            Some(line) => {
                let can = !g.busy();
                row.child(words("Clean Up".into(), line)).child(main_button("storage-clean-up", "Clean Up…", can).stop(cx).when(can, |b| b.on_click(cx.listener(|g, _, _, cx| ask(g, cx)))))
            }
            None => row.child(words("Nothing to Clean Up".into(), format!("Everything nami keeps is in use. Your apps as they are now and your {} most recent changes always stay.", s.cleanup.keep))),
        },
    };
    let mut out = sheet().flex().flex_col().child(body);
    if let Some(freed) = g.storage.freed.filter(|_| g.storage.cleaning == Cleaning::Idle) {
        let said = if freed > 0 { format!("Freed {}.", size(freed)) } else { "Older changes were removed.".to_string() };
        out = out.child(sheet_row(false).child(crate::icons::glyph("check", 14., color::good())).child(div().text_size(px(text::CALLOUT)).child(said)));
    }
    if let Cleaning::Failed(why) = &g.storage.cleaning {
        out = out.child(sheet_row(false).child(crate::pages::trouble("storage-failed", "Clean Up stopped before it finished.", why.clone(), cx)));
    }
    out
}

/// The apps that take the most, each opening its page.
fn largest_sheet(g: &Shell, s: &Seen, cx: &mut Context<Shell>) -> Option<Div> {
    if s.largest.is_empty() {
        return None;
    }
    let rows: Vec<AnyElement> = s.largest.iter().enumerate().map(|(n, a)| {
        let attr = a.attr.clone();
        let found = g.catalog.get(&attr).cloned();
        let name = g.app_name(&attr);
        let whose = if a.whose == "system" { "System" } else { "Just you" };
        sheet_row(n == 0)
            .id(ElementId::NamedInteger("storage-app".into(), n as u64))
            .cursor_pointer()
            .hover(|s| s.bg(color::hover_pill()))
            .on_click(cx.listener(move |g, _, _, cx| crate::pages::explore::open(g, found.clone(), attr.clone(), cx)))
            .child(crate::pages::installed::mark(g, &a.attr, 32.))
            .child(div().flex_1().min_w_0().truncate().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(name))
            .child(tag(whose))
            .child(div().flex_none().min_w(px(64.)).flex().justify_end().text_size(px(text::BODY)).text_color(rgb(color::muted())).child(size(a.adds)))
            .into_any_element()
    }).collect();
    Some(
        div().flex().flex_col().gap(px(8.))
            .child(div().text_size(px(text::SECTION)).font_weight(FontWeight::SEMIBOLD).child("Largest Apps"))
            .child(sheet().flex().flex_col().children(rows))
            .child(quiet("What removing each app frees, once its older changes are cleaned up.")),
    )
}

pub fn render(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    if g.explore.app.is_some() {
        return crate::pages::explore::app_page(g, cx);
    }
    let back = div().flex().child(crate::ui::link("storage-back", "‹ Installed").text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).on_click(cx.listener(|g, _, w, cx| g.go(Page::Installed, w, cx))));
    let page = div().flex().flex_col().gap(px(20.)).pt(px(14.)).child(back);
    let s = match g.storage.seen.clone() {
        None => return page.child(page_title("Storage", None)).child(crate::pages::looking("storage-looking")),
        Some(Err(why)) => {
            return page.child(page_title("Storage", None)).child(crate::pages::failed("storage-again", "Couldn't check how much space nami uses.", why, |g: &mut Shell, cx| {
                g.storage.seen = None;
                look(g, cx);
            }, cx));
        }
        Some(Ok(s)) => s,
    };
    let line = format!("nami uses {} on this {}.", size(s.total), if s.disk.container { "Mac" } else { "computer" });
    page.child(page_title("Storage", Some(div().child(line).into_any_element())))
        .child(disk_sheet(&s))
        .child(clean_sheet(g, &s, cx))
        .child(div().flex().flex_col().gap(px(8.)).child(div().text_size(px(text::SECTION)).font_weight(FontWeight::SEMIBOLD).child("What nami Uses")).child(parts_sheet(&s)))
        .children(largest_sheet(g, &s, cx))
}

/// "Clean Up 2.3 GB?", over the window, while it asks.
pub fn overlay(g: &Shell, cx: &mut Context<Shell>) -> Option<AnyElement> {
    if g.storage.cleaning != Cleaning::Asking {
        return None;
    }
    let Some(Ok(s)) = &g.storage.seen else { return None };
    let (title, goes, stays) = ask_words(&s.cleanup, bytes_of(s, "unused"), crate::look::MAC);
    let body = |t: String| div().text_size(px(text::BODY)).text_color(rgb(color::muted())).child(t);
    let card = div()
        .child(div().text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).child(title))
        .when(!goes.is_empty(), |d| d.child(body(goes)))
        .child(body(stays))
        .child(
            div().flex().justify_end().gap(px(8.)).mt(px(8.))
                .child(grey("storage-cancel", "Cancel").stop(cx).on_click(cx.listener(|g, _, _, cx| {
                    g.storage.cleaning = Cleaning::Idle;
                    cx.notify();
                })))
                .child(main_button("storage-go", "Clean Up", true).stop(cx).on_click(cx.listener(|g, _, _, cx| go(g, cx)))),
        );
    Some(crate::pages::uninstall_nami::dialog("storage-ask", "Clean Up", card, cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn freeing(yours: &[u64], computers: &[u64], bytes: u64, now: bool) -> Freeing {
        Freeing { yours: yours.to_vec(), computers: computers.to_vec(), paths: 12, bytes, keep: 3, now }
    }

    #[test]
    fn sizes_are_said_as_a_mac_says_them() {
        assert_eq!(size(2_345_000_000), "2.3 GB");
        assert_eq!(size(340_400_000), "340 MB");
        assert_eq!(size(512_000_000_000), "512 GB");
        assert_eq!(size(12_300), "12 KB");
        assert_eq!(size(0), "Zero KB");
    }

    #[test]
    fn what_clean_up_frees_is_said_plainly() {
        assert_eq!(frees_words(&freeing(&[4, 5, 6, 7], &[], 2_300_000_000, true), 1).as_deref(), Some("Frees 2.3 GB: 4 older changes and downloads nothing uses."));
        assert_eq!(frees_words(&freeing(&[4], &[], 2_300_000_000, true), 0).as_deref(), Some("Frees 2.3 GB: 1 older change."));
        assert_eq!(frees_words(&freeing(&[], &[], 80_000_000, true), 80_000_000).as_deref(), Some("Frees 80 MB: downloads nothing uses."));
        assert_eq!(frees_words(&freeing(&[2, 3], &[], 0, true), 0).as_deref(), Some("Removes 2 older changes. They share all their files with what you use now, so no space is freed."));
        assert_eq!(frees_words(&freeing(&[2, 3], &[], 1_000_000_000, false), 0).as_deref(), Some("Removes 2 older changes. An administrator can then free the 1.0 GB they use."));
        assert_eq!(frees_words(&freeing(&[1], &[], 0, true), 0).as_deref(), Some("Removes 1 older change. It shares all its files with what you use now, so no space is freed."));
        // Nothing: said so, and no button.
        assert_eq!(frees_words(&freeing(&[], &[], 0, true), 0), None);
    }

    #[test]
    fn it_asks_what_goes_and_that_undo_wont_reach_it() {
        let (title, goes, stays) = ask_words(&freeing(&[4, 5, 6, 7], &[2], 2_300_000_000, true), 80_000_000, false);
        assert_eq!(title, "Clean Up 2.3 GB?");
        assert_eq!(goes, "This removes your changes 4–7, the computer's change 2 and the files only they use. You can't undo back to them in History afterward. Downloads nothing uses any more are deleted too.");
        assert_eq!(stays, "Your apps as they are now and your 3 most recent changes stay.");
        let (_, goes, _) = ask_words(&freeing(&[1, 2, 3], &[], 445_000_000, true), 0, false);
        assert_eq!(goes, "This removes your changes 1–3 and the files only they use. You can't undo back to them in History afterward.");
        let (_, goes, stays) = ask_words(&freeing(&[], &[], 50_000_000, true), 50_000_000, true);
        assert_eq!(goes, "This deletes downloads nothing uses any more.");
        assert!(stays.ends_with("macOS asks for an administrator's password."), "{stays}");
        // Someone who isn't an administrator: their changes go, the space waits for one.
        let (title, goes, stays) = ask_words(&freeing(&[5], &[], 9, false), 0, false);
        assert_eq!((title.as_str(), goes.as_str()), ("Clean Up?", "This removes your change 5. You can't undo back to it in History afterward."));
        assert!(stays.ends_with("An administrator can then free the space it uses."), "{stays}");
        // One change that shares all its files: nothing's freed, and it's said so.
        let (title, goes, _) = ask_words(&freeing(&[1], &[], 0, true), 0, true);
        assert_eq!((title.as_str(), goes.as_str()), ("Clean Up?", "This removes your change 1. You can't undo back to it in History afterward."));
        let (a, b, c) = ask_words(&freeing(&[1], &[1], 1, true), 1, true);
        assert!(!format!("{a}{b}{c}").to_lowercase().contains("tray"));
    }

    #[test]
    fn change_numbers_are_ranges() {
        assert_eq!(ranges(&[4, 5, 6, 7]), "4–7");
        assert_eq!(ranges(&[3, 5]), "3 and 5");
        assert_eq!(ranges(&[1, 2, 4, 6, 7, 8]), "1, 2, 4 and 6–8");
        assert_eq!(ranges(&[9]), "9");
    }

    #[test]
    fn clean_up_runs_yours_then_the_computers() {
        let c = freeing(&[1], &[], 10, true);
        assert_eq!(steps(&c, false), vec![Step::Yours, Step::Computers]);
        assert_eq!(steps(&c, true), vec![Step::Yours, Step::AsAdministrator]);
        assert_eq!(steps(&freeing(&[1], &[], 10, false), false), vec![Step::Yours]);
        let (p, args) = as_administrator("/nix/var/nami/bin/nami");
        assert_eq!(p, "/usr/bin/osascript");
        assert_eq!(args[1], "do shell script \"/nix/var/nami/bin/nami gc --now\" with prompt \"nami wants to free up space on this Mac.\" with administrator privileges");
    }

    #[test]
    fn how_far_it_is_is_counted_from_what_nami_says() {
        let lines: Vec<String> = ["✓ Changes 1, 2 removed", "removing 4 paths (12.0 MiB)", "  0c3ssd2rjqakfggl1xcgndrxq3ll8bqz-libreoffice-26.2.3", "  1d8kd9s0q7n3c2mqcz9wq9a1b6xv0jrf-vlc-3.0.24"].map(String::from).to_vec();
        assert_eq!(progress(&lines, Step::Computers), ("Removing 2 of 4 items…".to_string(), Some(0.5)));
        assert_eq!(progress(&lines[..1], Step::Computers).1, None);
        assert_eq!(progress(&[], Step::Yours).0, "Removing older changes…");
    }

    #[test]
    fn every_part_has_its_words_and_the_bar_adds_up() {
        for p in ["yours", "everyone", "projects", "others", "running", "older", "unused", "casks", "nami"] {
            let (name, about) = part_words(p).unwrap();
            assert!(!name.is_empty() && about.ends_with('.'), "{p}");
            assert!(!format!("{name}{about}").to_lowercase().contains("tray"));
        }
        let s = Seen {
            disk: crate::nami::Disk { size: 500, used: 300, free: 200, container: false },
            total: 50,
            parts: Vec::new(),
            largest: Vec::new(),
            cleanup: freeing(&[1], &[], 20, true),
        };
        let shares = shares(&s);
        assert_eq!(shares.map(|(b, _)| b), [30, 20, 250, 200]);
        assert_eq!(shares.iter().map(|(b, _)| b).sum::<u64>(), s.disk.used + s.disk.free);
    }
}
