//! Settings and the files are the same thing, both ways. Each change Settings writes
//! says its line for a few seconds (`user.nami · namios.appearance.blur: wallpaper`,
//! with Show in file), and a row's right-click asks for it again. A file changed by
//! hand or by `nami` shows in an open window within a second; one with a mistake in it
//! says so calmly, and what it said before stays.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use gpui::prelude::*;
use crate::look::Press;
use gpui::{Context, Div, ElementId, FontWeight, div, rgb};

use crate::file_diff::{self, Mark, Row};
use crate::look::px;
use crate::look::text;
use crate::ui::color;
use crate::{Page, Shell};

/// How long a change's line shows.
pub const SHOWN: Duration = Duration::from_secs(6);
/// How often the files are looked at.
const EVERY: Duration = Duration::from_millis(700);

/// A change's line in a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    pub file: PathBuf,
    /// The file, as Developer mode calls it: user.nami, system.nami, ann/user.nami.
    pub name: String,
    /// Where it is (`namios.appearance.blur`).
    pub path: Vec<String>,
    /// What it says now: `namios.appearance.blur: wallpaper`.
    pub what: String,
    /// More changed with it.
    pub more: usize,
    pub line: Option<usize>,
    pub at: Instant,
    /// Asked for (a row's right-click): stays until another.
    pub asked: bool,
}

impl Said {
    pub fn fresh(&self) -> bool {
        self.asked || self.at.elapsed() < SHOWN
    }

    pub fn text(&self) -> String {
        let more = if self.more > 0 { format!("  (and {} more)", self.more) } else { String::new() };
        format!("{} · {}{more}", self.name, self.what)
    }
}

/// A file with a mistake in it, as the window last read it.
#[derive(Clone, Debug, PartialEq)]
pub struct Mistake {
    pub file: PathBuf,
    pub name: String,
    pub line: Option<usize>,
    pub message: String,
}

/// What a file is called here.
pub fn name_of(g: &Shell, file: &Path) -> String {
    let same = |p: &Path| p == file || p.canonicalize().ok().is_some_and(|c| file.canonicalize().ok().is_some_and(|f| f == c));
    if let Ok(role) = &g.role {
        if same(&role.user) {
            return "user.nami".into();
        }
        if same(&role.system) {
            return "system.nami".into();
        }
    }
    // (Someone's: their login and the file.)
    let parent = file.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match parent {
        Some(p) if name == "user.nami" || name == "package.nami" => format!("{p}/{name}"),
        _ => name,
    }
}

/// A value as a line says it: as written in the file.
fn shown_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "~".into(),
        other => other.to_string(),
    }
}

/// A row of what changed, as the line says it: `namios.appearance.blur: wallpaper`,
/// `packages: + gimp`.
pub fn what_of(r: &Row) -> String {
    let dotted = r.path.join(".");
    let parent = r.path[..r.path.len().saturating_sub(1)].join(".");
    let last = r.path.last().cloned().unwrap_or_default();
    match (r.mark, r.item) {
        (Mark::Added, true) => format!("{parent}: + {last}{}", r.to.as_ref().map(|v| format!(" {v}")).unwrap_or_default()),
        (Mark::Removed, true) => format!("{parent}: − {last}"),
        (Mark::Removed, false) => format!("{dotted}: reset to default"),
        _ => format!("{dotted}: {}", r.to.clone().unwrap_or_default()),
    }
}

/// What a write said, from the file as it was and is (or the edit, if it can't be read).
pub fn from_written(name: String, file: &Path, before: Option<&str>, after: Option<&str>, op: Option<&serde_json::Value>) -> Option<Said> {
    let rows: Option<Vec<Row>> = match (before, after) {
        (Some(b), Some(a)) => file_diff::changes(b, a).ok().map(|s| s.into_iter().flat_map(|s| s.rows).collect()),
        _ => None,
    };
    let (path, what, more) = match rows.as_deref() {
        // (Nothing different: nothing to say.)
        Some([]) => return None,
        Some(rows) => (rows[0].path.clone(), what_of(&rows[0]), rows.len() - 1),
        None => {
            let op = op?;
            let (kind, body) = op.as_object()?.iter().next()?;
            let path: Vec<String> = body["path"].as_array()?.iter().map(shown_value).collect();
            let value = &body["value"];
            let dotted = path.join(".");
            let what = match kind.as_str() {
                "add" => format!("{dotted}: + {}", shown_value(value)),
                "remove" => format!("{dotted}: − {}", shown_value(value)),
                "unset" => format!("{dotted}: reset to default"),
                _ => format!("{dotted}: {}", shown_value(value)),
            };
            (path, what, 0)
        }
    };
    let refs: Vec<&str> = path.iter().map(String::as_str).collect();
    let line = after.and_then(|t| file_diff::line_of(t, &refs)).map(|(l, _)| l);
    Some(Said { file: file.to_path_buf(), name, path, what, more, line, at: Instant::now(), asked: false })
}

/// A setting's line as the file has it now (asked for: a row's right-click).
pub fn for_setting(g: &Shell, path: &[&str]) -> Option<Said> {
    let file = g.file(false)?;
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let found = file_diff::line_of(&text, path);
    let whole = found.is_some_and(|(_, d)| d == path.len());
    let value = path.iter().fold(&g.user, |v, k| &v[*k]);
    let dotted = path.join(".");
    let what = if whole { format!("{dotted}: {}", shown_value(value)) } else { format!("{dotted}: not set, using default") };
    Some(Said { name: name_of(g, &file), file, path: path.iter().map(|s| s.to_string()).collect(), what, more: 0, line: found.map(|(l, _)| l), at: Instant::now(), asked: true })
}

/// What the window wrote since it last looked: the last one's line, shown.
pub fn take_written(g: &mut Shell, cx: &mut Context<Shell>) {
    let written = crate::nami::written();
    let Some(said) = written.iter().rev().find_map(|w| from_written(name_of(g, &w.file), &w.file, w.before.as_deref(), w.after.as_deref(), w.op.as_ref())) else { return };
    g.said_file = Some(said);
    // Gone again in a while.
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(SHOWN + Duration::from_millis(50)).await;
        let _ = this.update(cx, |_, cx| cx.notify());
    })
    .detach();
}

/// The line, quiet: the file and what it says, and Show in file.
pub fn line(said: &Said, id: &str, cx: &mut Context<Shell>) -> Div {
    let (file, at) = (said.file.clone(), said.line);
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(div().flex_1().min_w_0().truncate().font_family("Geist Mono").text_size(px(text::META)).text_color(rgb(color::muted())).child(said.text()))
        .child(
            div()
                .id(ElementId::Name(format!("show-in-file-{id}").into()))
                .flex_none()
                .text_size(px(text::META))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(color::key()))
                .cursor_pointer()
                .press()
                .hover(|s| s.underline())
                .role(gpui::Role::Link)
                .aria_label("Show in File")
                .on_click(cx.listener(move |this, _, _, cx| show_in_file(this, &file, at, cx)))
                .child("Show in File"),
        )
}

/// Under a setting's row: its line, while it's fresh, or asked for.
pub fn under_row(g: &Shell, path: &[&str], cx: &mut Context<Shell>) -> Option<gpui::AnyElement> {
    let said = g.said_file.as_ref().filter(|s| s.fresh() && s.path.iter().map(String::as_str).eq(path.iter().copied()))?;
    Some(crate::motion::enter(ElementId::Name(format!("file-line-{}", path.join(".")).into()), line(said, &path.join("."), cx).px(px(14.)).pb(px(8.)).mt(px(-4.))))
}

/// Elsewhere (Apps, People, a page of its own): a small card above the bottom of the page.
pub fn toast(g: &Shell, in_row: bool, cx: &mut Context<Shell>) -> Option<Div> {
    let said = g.said_file.as_ref().filter(|s| s.fresh() && !in_row)?;
    Some(
        div()
            .absolute()
            .bottom(px(16.))
            .right(px(crate::look::shape::GUTTER))
            .max_w(px(520.))
            .px(px(14.))
            .py(px(9.))
            .rounded(px(crate::look::shape::CONTROL))
            .border_1()
            .border_color(color::hairline())
            .bg(rgb(color::panel()))
            .shadow(crate::look::soft_shadow())
            .child(line(said, "toast", cx)),
    )
}

/// The file, at the line: in Developer mode, its own view of it; else in Text Editor.
pub fn show_in_file(g: &mut Shell, file: &Path, line: Option<usize>, cx: &mut Context<Shell>) {
    if g.dev.on {
        let files = crate::pages::dev::files(g);
        let same = |p: &Path| p == file || p.canonicalize().ok().is_some_and(|c| file.canonicalize().ok().is_some_and(|f| f == c));
        if let Some(i) = files.iter().position(|(_, p)| same(p)) {
            crate::pages::dev::open(g, i, cx);
            g.page = Page::File(i);
            if let Some(n) = line {
                g.dev.editor.update(cx, |e, cx| e.go_to_line(n, cx));
                scroll_to_line(n, cx);
            }
            cx.notify();
            return;
        }
    }
    g.problem = crate::launch::open_text_file(file, line).err().map(crate::Problem::under("The file didn't open."));
    cx.notify();
}

/// Developer mode's page scrolled so the editor's line `n` is in view, a third of the way
/// down (once the page with it is drawn).
fn scroll_to_line(n: usize, cx: &mut Context<Shell>) {
    cx.spawn(async move |this, cx| {
        for wait in [60, 200] {
            cx.background_executor().timer(Duration::from_millis(wait)).await;
            let _ = this.update(cx, |g, cx| {
                let Some(top) = g.dev.editor.read(cx).line_top(n) else { return };
                let view = g.page_scroll.bounds();
                let at = g.page_scroll.offset();
                let y = (at.y - (top - view.top() - view.size.height / 3.)).min(gpui::px(0.));
                g.page_scroll.set_offset(gpui::point(at.x, y));
                cx.notify();
            });
        }
    })
    .detach();
}

/// When each file was last changed, and its size: changed by hand, or by `nami`.
fn stamp(file: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(file).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// Look at the files every little while: what changed outside the window shows in it.
pub fn watch(cx: &mut Context<Shell>) {
    cx.spawn(async move |this, cx| {
        let mut seen: Vec<(PathBuf, Option<(SystemTime, u64)>)> = Vec::new();
        loop {
            cx.background_executor().timer(EVERY).await;
            let Ok(files) = this.update(cx, |g, _| g.role.as_ref().ok().map(|r| if r.admin { vec![r.user.clone(), r.system.clone()] } else { vec![r.user.clone()] }).unwrap_or_default()) else { return };
            let now: Vec<(PathBuf, Option<(SystemTime, u64)>)> = files.iter().map(|f| (f.clone(), stamp(f))).collect();
            if seen.is_empty() || now.len() != seen.len() {
                seen = now;
                continue;
            }
            // (Not taken while a change goes in place: looked at again next time.)
            if now != seen && this.update(cx, |g, cx| g.files_changed(cx)).unwrap_or(false) {
                seen = now;
            }
        }
    })
    .detach();
}

/// A file that couldn't be read: its line and what's wrong, from what nami said
/// (`... at line 12 column 3`).
pub fn mistake_in(name: String, file: &Path, said: &str) -> Mistake {
    let line = said.split(" line ").nth(1).and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next()).and_then(|n| n.parse().ok());
    let message = said.split(" at line ").next().unwrap_or(said).trim();
    // (Without the file's path in front: the banner says which.)
    let message = message.rsplit_once(".nami: ").map_or(message, |(_, m)| m).trim();
    Mistake { file: file.to_path_buf(), name, line, message: message.to_string() }
}

/// The calm banner: the file has a mistake, what's shown is what it said before.
pub fn banner(g: &Shell, cx: &mut Context<Shell>) -> Option<Div> {
    let m = g.mistake.as_ref()?;
    let (file, at) = (m.file.clone(), m.line);
    let where_ = m.line.map(|l| format!(" on line {l}")).unwrap_or_default();
    Some(
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .mx(px(crate::look::shape::GUTTER))
            // (In Developer mode nothing's above it: clear of the window's top.)
            .when(g.dev.on, |d| d.mt(px(16.)))
            .mb(px(8.))
            .px(px(14.))
            .py(px(10.))
            .rounded(px(crate::look::shape::CONTROL))
            .bg(rgb(color::key_soft()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_size(px(text::CALLOUT)).font_weight(FontWeight::MEDIUM).child(format!("{} has an error{where_}: {}", m.name, m.message)))
                    .child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child("Settings show the last working version until the error is fixed.")),
            )
            .child(
                crate::ui::button("mistake-show", "Show in File", false, true).on_click(cx.listener(move |this, _, _, cx| show_in_file(this, &file, at, cx))),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_says_its_line() {
        let before = "namios:\n  appearance:\n    blur: off\n";
        let after = "namios:\n  appearance:\n    blur: wallpaper\n";
        let s = from_written("user.nami".into(), Path::new("/h/user.nami"), Some(before), Some(after), None).unwrap();
        assert_eq!(s.text(), "user.nami · namios.appearance.blur: wallpaper");
        assert_eq!(s.line, Some(3));
    }

    #[test]
    fn an_app_added_and_taken_off() {
        let before = "packages:\n  - firefox\n";
        let after = "packages:\n  - firefox\n  - gimp\n";
        let s = from_written("system.nami".into(), Path::new("/etc/nami/system.nami"), Some(before), Some(after), None).unwrap();
        assert_eq!(s.what, "packages: + gimp");
        assert_eq!(s.line, Some(3));
        let s = from_written("system.nami".into(), Path::new("/etc/nami/system.nami"), Some(after), Some(before), None).unwrap();
        assert_eq!(s.what, "packages: − gimp");
        assert_eq!(s.line, Some(1));
    }

    #[test]
    fn the_same_text_says_nothing() {
        assert!(from_written("user.nami".into(), Path::new("/u"), Some("a: 1\n"), Some("a: 1  # x\n"), None).is_none());
    }

    #[test]
    fn unread_it_says_the_edit() {
        let op = serde_json::json!({ "set": { "path": ["timezone"], "value": "Europe/Copenhagen" } });
        let s = from_written("system.nami".into(), Path::new("/c"), None, None, Some(&op)).unwrap();
        assert_eq!(s.what, "timezone: Europe/Copenhagen");
        assert_eq!(s.line, None);
    }

    #[test]
    fn a_person_added_says_how_many_more() {
        let before = "users:\n  ann:\n    admin: true\n";
        let after = "users:\n  ann:\n    admin: true\n  bo:\n    name: Bo\n    child: true\n";
        let s = from_written("system.nami".into(), Path::new("/c"), Some(before), Some(after), None).unwrap();
        assert_eq!(s.what, "users.bo.name: Bo");
        assert_eq!(s.more, 1);
        assert_eq!(s.line, Some(5));
    }

    #[test]
    fn a_mistakes_line() {
        let m = mistake_in("user.nami".into(), Path::new("/u"), "/home/ada/user.nami: did not find expected key at line 12 column 3, while parsing a block mapping");
        assert_eq!(m.line, Some(12));
        assert_eq!(m.message, "did not find expected key");
        let m = mistake_in("user.nami".into(), Path::new("/u"), "unknown key `namioss`");
        assert_eq!(m.line, None);
    }
}
