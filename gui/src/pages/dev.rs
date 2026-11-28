//! Developer mode: the files behind the pages, to edit as text. Yours
//! (user.nami), and for admins the computer's (system.nami), each
//! person's (<login>/user.nami) and the package folders it uses
//! (packages). `nami check` reads what you type as you type; Save
//! writes the file, and Apply and Discard work as on the pages.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::prelude::*;
use crate::look::Press;
use gpui::{Context, Div, Entity, FontWeight, ParentElement, Styled, div, rgb};
use crate::look::px;
use crate::look::text;

use crate::editor::lang::Mark;
use crate::editor::{Editor, SaveAsked};
use crate::nami::{self, Problem};
use crate::ui::{button, color, tick};
use crate::Shell;

pub struct Dev {
    pub on: bool,
    /// Vim's keys in the editors.
    pub vim: bool,
    /// The files: what the sidebar calls each, and where it is.
    pub files: Vec<(String, PathBuf)>,
    pub at: usize,
    pub editor: Entity<Editor>,
    /// The file's text as it is on disk.
    pub saved: String,
    /// Text not saved yet, of the files not open now.
    pub drafts: BTreeMap<PathBuf, String>,
    pub problems: Option<Result<Vec<Problem>, String>>,
    /// The text last sent to `nami check`.
    pub checked: String,
    /// Save was pressed with problems: asked whether to save anyway.
    pub asking: bool,
}

/// Where developer mode is remembered: ~/.config/nami/gui.json.
fn remembered() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config/nami/gui.json")
}

/// Remember a switch in gui.json, keeping the others.
pub fn remember(name: &str, on: bool) {
    let file = remembered();
    let mut v = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
    v[name] = serde_json::Value::Bool(on);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, v.to_string());
}

/// A switch remembered in gui.json ("developer", "vim").
pub fn setting(name: &str) -> bool {
    std::fs::read_to_string(remembered()).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v.get(name)?.as_bool()).unwrap_or(false)
}

impl Dev {
    pub fn new(cx: &mut Context<Shell>) -> Self {
        let editor = cx.new(|cx| Editor::new(cx, ""));
        cx.observe(&editor, |this, _, cx| check_soon(this, cx)).detach();
        cx.subscribe(&editor, |this, _, _: &SaveAsked, cx| save(this, cx)).detach();
        Dev { on: setting("developer"), vim: setting("vim"), files: Vec::new(), at: 0, editor, saved: String::new(), drafts: BTreeMap::new(), problems: None, checked: String::new(), asking: false }
    }

    pub fn remember(&self) {
        remember("developer", self.on);
        remember("vim", self.vim);
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.files.get(self.at).map(|(_, p)| p)
    }

    pub fn unsaved(&self, path: &Path, cx: &gpui::App) -> bool {
        self.drafts.contains_key(path) || (self.path().is_some_and(|p| p == path) && self.editor.read(cx).content != self.saved)
    }
}

/// The files there are to edit, for who you are.
pub fn files(g: &Shell) -> Vec<(String, PathBuf)> {
    let Ok(role) = &g.role else { return Vec::new() };
    let mut out = vec![("user.nami".to_string(), role.user.clone())];
    // (On another distribution, only yours, unless nami keeps a computer's file there too.)
    if !role.admin || (super::store_mode() && !role.system.is_file()) {
        return out;
    }
    out.push(("system.nami".into(), role.system.clone()));
    let Some(dir) = role.system.parent().and_then(|d| d.canonicalize().ok()) else { return out };
    let mut people: Vec<String> = std::fs::read_dir(&dir).into_iter().flatten().flatten().filter(|e| crate::nami::existing(e.path().join("user.nami")).is_file()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    people.sort();
    out.extend(people.into_iter().map(|p| (format!("{p}/user.nami"), crate::nami::existing(dir.join(&p).join("user.nami")))));
    if let Some(shared) = dir.parent().map(|t| t.join("packages")) {
        let mut pkgs: Vec<String> = std::fs::read_dir(&shared).into_iter().flatten().flatten().filter(|e| crate::nami::existing(e.path().join("package.nami")).is_file()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        pkgs.sort();
        out.extend(pkgs.into_iter().map(|p| (format!("packages/{p}"), crate::nami::existing(shared.join(&p).join("package.nami")))));
    }
    out
}

/// Open file `i` (what wasn't saved in the one open stays, as a draft).
pub fn open(g: &mut Shell, i: usize, cx: &mut Context<Shell>) {
    if let Some(p) = g.dev.path().cloned() {
        let text = g.dev.editor.read(cx).content.clone();
        if text != g.dev.saved {
            g.dev.drafts.insert(p, text);
        } else {
            g.dev.drafts.remove(&p);
        }
    }
    g.dev.files = files(g);
    g.dev.at = i.min(g.dev.files.len().saturating_sub(1));
    let Some(path) = g.dev.path().cloned() else { return };
    g.dev.saved = std::fs::read_to_string(&path).unwrap_or_default();
    let text = g.dev.drafts.remove(&path).unwrap_or_else(|| g.dev.saved.clone());
    // (What kind of file it is: what fits as you type, and what's said on hover.)
    let kind = path.file_name().and_then(|n| Editor::kind_of(&n.to_string_lossy()));
    g.dev.editor.update(cx, |e, cx| {
        e.set(cx, &text);
        e.kind = kind;
    });
    g.dev.problems = None;
    g.dev.checked = String::new();
    g.dev.asking = false;
    check_soon(g, cx);
}

/// What `nami check` makes of the text: once typing stops a moment.
fn check_soon(g: &mut Shell, cx: &mut Context<Shell>) {
    let text = g.dev.editor.read(cx).content.clone();
    if text == g.dev.checked {
        return;
    }
    g.dev.checked = text.clone();
    let Some(path) = g.dev.path().cloned() else { return };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let base = path.parent().and_then(|d| d.canonicalize().ok()).unwrap_or_default();
    cx.notify();
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(Duration::from_millis(400)).await;
        if !this.update(cx, |g, cx| g.dev.editor.read(cx).content == text).unwrap_or(false) {
            return;
        }
        let words = text.clone();
        let found = cx.background_executor().spawn(async move { nami::check(&words, &name, &base) }).await;
        let _ = this.update(cx, |g, cx| {
            if g.dev.editor.read(cx).content != text {
                return;
            }
            let marks: Vec<Mark> = found
                .as_ref()
                .map(|p| p.iter().filter(|p| p.line > 0 && p.severity != "note").map(|p| Mark { line: p.line, message: p.message.clone(), hint: p.hint.clone(), error: p.severity == "error" }).collect())
                .unwrap_or_default();
            g.dev.editor.update(cx, |e, cx| {
                e.problems = marks;
                cx.notify();
            });
            g.dev.problems = Some(found);
            cx.notify();
        });
    })
    .detach();
}

fn errors(g: &Shell) -> usize {
    match &g.dev.problems {
        Some(Ok(p)) => p.iter().filter(|p| p.severity == "error").count(),
        _ => 0,
    }
}

/// Write the file (with problems, only once asked).
fn save(g: &mut Shell, cx: &mut Context<Shell>) {
    if errors(g) > 0 && !g.dev.asking {
        g.dev.asking = true;
        cx.notify();
        return;
    }
    let Some(path) = g.dev.path().cloned() else { return };
    let text = g.dev.editor.read(cx).content.clone();
    // (The computer's file: once an admin's password is given.)
    g.as_admin(&path.clone(), crate::CHANGING, move |g, cx| saved(g, path, text, cx), cx);
}

fn saved(g: &mut Shell, path: PathBuf, text: String, cx: &mut Context<Shell>) {
    if !g.before.iter().any(|(f, _)| *f == path) {
        g.before.push((path.clone(), std::fs::read_to_string(&path).ok()));
    }
    g.problem = crate::nami::write(&path, &text).err().map(crate::Problem::under("Couldn't save the file."));
    if g.problem.is_none() {
        g.dev.saved = text;
    }
    g.dev.asking = false;
    g.reload(cx);
    cx.notify();
}

/// How a path is shown: ~/ for home.
fn shown(path: &Path) -> String {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    match path.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

pub fn render(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    let Some(path) = g.dev.path().cloned() else {
        return div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child("No nami files found.");
    };
    let unsaved = g.dev.editor.read(cx).content != g.dev.saved;
    let n = errors(g);
    let mut page = div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(div().flex_1().min_w_0().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).font_family("Geist Mono").child(format!("{}{}", shown(&path), if unsaved { "  •" } else { "" })))
                .child(
                    div()
                        .id("vim")
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(text::CALLOUT))
                        .cursor_pointer()
                        .press()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.dev.vim = !this.dev.vim;
                            this.dev.remember();
                            let on = this.dev.vim;
                            this.dev.editor.update(cx, |e, cx| {
                                e.vim = on;
                                e.vi = Default::default();
                                cx.notify();
                            });
                            cx.notify();
                        }))
                        .child(tick("vim-tick".into(), g.dev.vim))
                        .child("Vim keys"),
                )
                .when(g.dev.asking, |d| d.child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::bad())).child(format!("nami check found {n} problem{}. Save anyway?", if n == 1 { "" } else { "s" }))))
                .child(button("save", if g.dev.asking { "Save Anyway" } else { "Save" }, true, unsaved).when(unsaved, |b| b.on_click(cx.listener(|this, _, _, cx| save(this, cx))))),
        )
        .child(g.dev.editor.clone());
    page = page.child(match &g.dev.problems {
        None => div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child("nami check…"),
        Some(Ok(p)) if p.is_empty() => div().text_size(px(text::CALLOUT)).text_color(rgb(color::good())).child("✓ nami check found no problems"),
        // (A line's problem is said under the editor, with the others' lines to go to: here, only
        // what's about the whole file.)
        Some(Ok(p)) => div().flex().flex_col().gap(px(4.)).children(p.iter().filter(|p| p.line == 0 && p.severity != "note").map(|p| {
            let bad = p.severity == "error";
            div().text_size(px(text::CALLOUT)).child(crate::editor::said(&p.said(), if bad { color::bad() } else { color::text() }))
        })),
        Some(Err(why)) => div().text_size(px(text::CALLOUT)).text_color(rgb(color::bad())).child(format!("Couldn't run nami check: {why}")),
    });
    page
}
