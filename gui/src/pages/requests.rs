//! Requests: changes to this computer that an app asked for (`nami propose`: an AI agent,
//! say), waiting for someone to decide. Each shows who asks and why, the change in words and
//! the file's lines, and Apply (an administrator's password, then it's applied) or Don't
//! Allow. The notification "Claude wants to change your computer" opens this page.

use std::collections::HashSet;
use std::path::PathBuf;

use gpui::prelude::*;
use gpui::{Context, Div, ElementId, FontWeight, ParentElement, Styled, div, rgb};
use serde::Deserialize;

use crate::file_diff::{FileDiff, Mark};
use crate::look::{px, text};
use crate::ui::color;
use crate::Shell;

/// A problem `nami check` found in the file as the change would leave it.
#[derive(Deserialize, Clone)]
pub struct Problem {
    #[serde(default)]
    pub severity: String,
    pub message: String,
}

/// A request, as `nami proposals list --json` has it.
#[derive(Deserialize, Clone)]
pub struct Request {
    pub id: String,
    pub by: String,
    pub reason: String,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub status: String,
    /// What Apply checks it's still the request shown.
    pub fingerprint: String,
    /// The computer's file changed since it was asked for: it can't be applied.
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub diff: Option<FileDiff>,
    #[serde(default)]
    pub problems: Vec<Problem>,
}

impl Request {
    fn errors(&self) -> Vec<&Problem> {
        self.problems.iter().filter(|p| p.severity == "error").collect()
    }
}

#[derive(Default)]
pub struct Requests {
    /// Those waiting, oldest first (None: not looked yet).
    pub list: Option<Result<Vec<Request>, String>>,
    /// Requests whose lines are hidden.
    pub hidden: HashSet<String>,
    /// The one being answered no.
    pub declining: Option<String>,
    looking: bool,
}

impl Requests {
    /// How many are waiting.
    pub fn waiting(&self) -> usize {
        match &self.list {
            Some(Ok(l)) => l.len(),
            _ => 0,
        }
    }
}

/// Where nami keeps them: `$XDG_STATE_HOME/nami/proposals` (src/proposals.rs).
pub fn dir() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state"));
    state.join("nami/proposals")
}

/// When the requests last changed (one asked for, or decided): the folder's time.
pub fn changed_at() -> Option<std::time::SystemTime> {
    std::fs::metadata(dir()).ok()?.modified().ok()
}

/// Ask nami for the requests waiting, in the background.
pub fn look(cx: &mut Context<Shell>) {
    cx.spawn(async move |this, cx| {
        let busy = this.update(cx, |g, _| std::mem::replace(&mut g.requests.looking, true)).unwrap_or(true);
        if busy {
            return;
        }
        let found = cx.background_executor().spawn(async { crate::nami::json::<Vec<Request>>(&["proposals", "list", "--json"]) }).await;
        let _ = this.update(cx, |g, cx| {
            g.requests.looking = false;
            g.requests.list = Some(found.map(|l| l.into_iter().filter(|r| r.status == "pending").collect()));
            if g.requests.declining.as_ref().is_some_and(|id| !g.requests.list.as_ref().and_then(|l| l.as_ref().ok()).is_some_and(|l| l.iter().any(|r| &r.id == id))) {
                g.requests.declining = None;
            }
            cx.notify();
        });
    })
    .detach();
}

/// Don't Allow: discarded, and whoever asked is told.
fn decline(g: &mut Shell, id: String, cx: &mut Context<Shell>) {
    g.requests.declining = Some(id.clone());
    cx.notify();
    cx.spawn(async move |this, cx| {
        let done = cx.background_executor().spawn(async move { crate::nami::reject_request(&id) }).await;
        let _ = this.update(cx, |g, cx| {
            if let Err(e) = done {
                g.requests.declining = None;
                g.problem = Some(crate::Problem::of("Couldn't decline the request.", &e));
            }
            look(cx);
        });
    })
    .detach();
}

/// Apply: an administrator's password (the desktop asks, saying what changes), then the
/// computer's file is changed and applied, as the request was shown.
fn approve(g: &mut Shell, r: &Request, cx: &mut Context<Shell>) {
    let args = vec!["proposals".to_string(), "apply".into(), r.id.clone(), "--expect".into(), r.fingerprint.clone()];
    g.start(&format!("Applying {}'s change", r.by), vec![(args, None)], cx);
}

/// "Today 14:02", "Yesterday 09:15", "12 November 18:40".
fn when(at: u64) -> String {
    if at == 0 {
        return String::new();
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let day = super::history::day_label(super::history::Day::of(at), super::history::Day::of(now));
    let tm = crate::clock::local(at as libc::time_t);
    format!("{day} {:02}:{:02}", tm.tm_hour, tm.tm_min)
}

/// The change in words: by section, + for what's added and − for what goes.
fn in_words(g: &Shell, d: &FileDiff) -> Div {
    let mut out = div().flex().flex_col().gap(px(10.));
    for s in &d.sections {
        out = out.child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
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
                        .child(div().flex_1().min_w_0().text_size(px(text::CALLOUT)).when(r.mark == Mark::Removed, |d| d.text_color(rgb(color::muted()))).child(super::history::row_text(g, s, r)))
                })),
        );
    }
    if d.sections.is_empty() {
        out = out.child(crate::ui::quiet("Only comments in the file change."));
    }
    out
}

fn card(g: &Shell, r: &Request, running: bool, cx: &mut Context<Shell>) -> Div {
    use crate::ui::{grey, main_button, sheet};
    let id = r.id.clone();
    let declining = g.requests.declining.as_deref() == Some(r.id.as_str());
    let errors = r.errors();
    let can_apply = !running && !declining && !r.stale && errors.is_empty() && r.diff.is_some();
    let showing_lines = !g.requests.hidden.contains(&r.id);
    let heading = div()
        .flex()
        .items_center()
        .gap(px(14.))
        .child(div().size(px(38.)).flex_none().rounded_full().bg(crate::ui::ink(0x0e)).flex().items_center().justify_center().child(crate::icons::glyph("settings", 17., color::muted())))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(format!("{} wants to change your computer", r.by)))
                .child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child(when(r.created))),
        );
    let reason = div().flex().flex_col().gap(px(3.)).child(div().text_size(px(text::META)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(color::muted())).child("Reason")).child(div().text_size(px(text::BODY)).child(r.reason.clone()));
    let mut body = div().flex().flex_col().gap(px(14.)).px(px(18.)).py(px(16.)).child(heading).child(reason);
    if r.stale {
        body = body.child(crate::ui::quiet(format!("This computer's settings changed after {} asked, so this request can't be applied. Ask {} to make the request again.", r.by, r.by)));
    } else if let Some(d) = &r.diff {
        body = body.child(div().flex().flex_col().gap(px(6.)).child(div().text_size(px(text::META)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(color::muted())).child("Changes")).child(in_words(g, d)));
        if !d.raw.is_empty() {
            let toggle = div()
                .id(ElementId::Name(format!("request-lines-{id}").into()))
                .text_size(px(text::META))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(color::key()))
                .cursor_pointer()
                .hover(|s| s.underline())
                .on_click(cx.listener({
                    let id = id.clone();
                    move |g, _, _, cx| {
                        if !g.requests.hidden.remove(&id) {
                            g.requests.hidden.insert(id.clone());
                        }
                        cx.notify();
                    }
                }))
                .child(if showing_lines { "Hide Lines in system.nami" } else { "Show Lines in system.nami" });
            body = body.child(toggle);
            if showing_lines {
                // (The lines themselves: not diff's headers, which say nothing to a person here.)
                let lines: Vec<&str> = d.raw.lines().filter(|l| !l.starts_with("---") && !l.starts_with("+++") && !l.starts_with("@@")).collect();
                body = body.child(super::history::raw_lines(&lines.join("\n")));
            }
        }
    }
    for p in &errors {
        body = body.child(div().text_size(px(text::CALLOUT)).child(crate::editor::said(&format!("This change can't be applied: {}", p.message), color::bad())));
    }
    for p in r.problems.iter().filter(|p| p.severity == "warning") {
        body = body.child(div().text_size(px(text::CALLOUT)).child(crate::editor::said(&p.message, color::muted())));
    }
    let buttons = div()
        .flex()
        .justify_end()
        .gap(px(8.))
        .child(grey(ElementId::Name(format!("request-no-{id}").into()), if declining { "Declining…" } else { "Don't Allow" }).when(running || declining, |b| b.opacity(0.45)).when(!running && !declining, |b| {
            let id = id.clone();
            b.on_click(cx.listener(move |g, _, _, cx| decline(g, id.clone(), cx)))
        }))
        .child(main_button(ElementId::Name(format!("request-yes-{id}").into()), "Apply", can_apply).when(can_apply, |b| {
            let r = r.clone();
            b.on_click(cx.listener(move |g, _, _, cx| approve(g, &r, cx)))
        }));
    sheet().flex().flex_col().child(body.child(buttons))
}

pub fn render(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    let running = g.run.as_ref().is_some_and(|r| r.finished.is_none());
    let page = div().flex().flex_col().gap(px(16.)).child(crate::ui::intro("Apps can ask to change this computer's settings. Nothing changes until an administrator applies the request."));
    let list = match &g.requests.list {
        None => return page.child(super::looking("requests-looking")),
        Some(Err(why)) => return page.child(super::failed("requests-again", "Couldn't load the requests.", why.clone(), |_, cx| look(cx), cx)),
        Some(Ok(l)) => l.clone(),
    };
    if list.is_empty() {
        return page.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .pt(px(60.))
                .child(crate::icons::glyph("inbox", 28., color::faint()))
                .child(div().text_size(px(text::SECTION)).font_weight(FontWeight::SEMIBOLD).child("No Requests"))
                .child(crate::ui::quiet("When an app asks to change this computer, you can review it here.")),
        );
    }
    page.children(list.iter().map(|r| card(g, r, running, cx)))
}
