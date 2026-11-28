//! Your Setup: your ~/user.nami besides its apps. Each settings file with the app it's for (a
//! package's `~/…` entries) or by itself (`files:`), shown as its settings or as text, secrets
//! as encrypted; what apps changed in the files they may edit, to keep in your file or undo
//! (`nami home keep` / `undo`); files in your home nami doesn't keep yet (`nami home adopt`);
//! your shell's variables and aliases.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, ElementId, Entity, FontWeight, ParentElement, Styled, div, rgb};
use crate::look::px;
use crate::look::text;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{entry_name, packages};
use crate::input::TextInput;
use crate::ui::{color, column_heading, filter, filters, grey, icon, ink, letters, list_row, main_button, page_title, quiet, sheet};
use crate::Shell;

/// A setting an app changed (`nami home diff --json`): its keys, what your file says, what
/// the file says now (None: not set, or removed).
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Change {
    pub path: Vec<String>,
    pub from: Option<Value>,
    pub to: Option<Value>,
}

/// A file apps may edit that no longer says what your file does.
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Drift {
    /// Relative to your home.
    pub file: String,
    #[serde(default)]
    pub changes: Vec<Change>,
    /// Changed, and it's text nami can't compare setting by setting.
    #[serde(default)]
    pub whole: bool,
}

/// A settings file your user.nami gives.
#[derive(Clone, Debug, PartialEq)]
pub struct SetupFile {
    /// As it's written: `~/.gitconfig`.
    pub path: String,
    /// The package it's under, if it's an app's.
    pub app: Option<String>,
    /// Where its entry is in your file, for `nami config json`: `["packages", "- git", "~/.gitconfig"]`.
    pub at: Vec<String>,
    /// Its settings (a map), or its text (a string), or a secret.
    pub content: Value,
    /// Its settings are under `content:` (it has options of its own: `mutable`, `mode`).
    pub wrapped: bool,
    /// An app may change it (`mutable: true`).
    pub mutable: bool,
    /// A file or folder from your repo (`source:`).
    pub source: Option<String>,
}

impl SetupFile {
    /// Settings to show as fields, not text.
    pub fn is_settings(&self) -> bool {
        self.content.is_object() && !is_secret(&self.content)
    }
}

/// Whether a value is a `!secret` (`{"!secret": "..."}`, as nami says it in JSON).
pub fn is_secret(v: &Value) -> bool {
    v.as_object().is_some_and(|m| m.len() == 1 && m.contains_key("!secret"))
}

/// The settings files your file gives: each app's (under its package), then `files:`'s.
pub fn files_of(user: &Value) -> Vec<SetupFile> {
    let mut out = Vec::new();
    let mut take = |path: &str, v: &Value, app: Option<String>, at: Vec<String>| {
        let wrapper = v.as_object().filter(|m| !is_secret(v) && (m.contains_key("content") || m.contains_key("source") || m.contains_key("mutable")));
        let (content, wrapped, mutable, source) = match wrapper {
            Some(m) => (m.get("content").cloned().unwrap_or(Value::Null), m.contains_key("content"), m.get("mutable").and_then(Value::as_bool).unwrap_or(false), m.get("source").and_then(Value::as_str).map(String::from)),
            None => (v.clone(), false, false, None),
        };
        out.push(SetupFile { path: path.to_string(), app, at, content, wrapped, mutable, source });
    };
    for e in packages(user) {
        let Some((name, setup)) = e.as_object().and_then(|m| m.iter().next()) else { continue };
        let Some(setup) = setup.as_object() else { continue };
        for (path, v) in setup.iter().filter(|(k, _)| k.starts_with("~/")) {
            take(path, v, Some(entry_name(&e)), vec!["packages".into(), format!("- {name}"), path.clone()]);
        }
    }
    if let Some(files) = user.get("files").and_then(Value::as_object) {
        for (path, v) in files.iter().filter(|(k, _)| k.starts_with("~/")) {
            take(path, v, None, vec!["files".into(), path.clone()]);
        }
    }
    out
}

/// A setting's line in the settings view: its keys from the file's top, its value.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// The section it's in (`user`, for gitconfig's `[user]`), empty at the top.
    pub section: String,
    /// Its keys from the file's top.
    pub keys: Vec<String>,
    pub value: Value,
}

/// A file's settings as fields: its plain settings first, then each section's (a map in it);
/// what's nested deeper, by its keys joined with dots.
pub fn fields_of(settings: &Value) -> Vec<Field> {
    let Some(top) = settings.as_object() else { return Vec::new() };
    fn walk(section: &str, keys: &mut Vec<String>, v: &Value, out: &mut Vec<Field>) {
        match v.as_object() {
            Some(m) if !is_secret(v) => {
                for (k, v) in m {
                    keys.push(k.clone());
                    walk(section, keys, v, out);
                    keys.pop();
                }
            }
            _ => out.push(Field { section: section.to_string(), keys: keys.clone(), value: v.clone() }),
        }
    }
    let mut plain = Vec::new();
    let mut sections = Vec::new();
    for (k, v) in top {
        if v.is_object() && !is_secret(v) {
            walk(k, &mut vec![k.clone()], v, &mut sections);
        } else {
            plain.push(Field { section: String::new(), keys: vec![k.clone()], value: v.clone() });
        }
    }
    plain.extend(sections);
    plain
}

/// A value as it's shown in its field: text as it is, the rest as JSON (`14`, `true`).
pub fn shown(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// What's typed in a field, as the setting it was: a number stays a number, a yes-or-no a
/// yes-or-no; anything else is text.
pub fn typed(text: &str, was: &Value) -> Value {
    let t = text.trim();
    match was {
        Value::Number(_) => t.parse::<i64>().map(Value::from).or_else(|_| t.parse::<f64>().map(Value::from)).unwrap_or_else(|_| Value::String(text.to_string())),
        Value::Bool(_) if t == "true" || t == "false" => Value::Bool(t == "true"),
        _ => Value::String(text.to_string()),
    }
}

/// What an app changed, in a line: `editor.fontSize 14 → 16 · workbench.colorTheme set to Dark`.
pub fn change_line(d: &Drift) -> String {
    if d.whole {
        return "Its text changed.".into();
    }
    let one = |c: &Change| {
        let key = c.path.join(".");
        match (&c.from, &c.to) {
            (Some(a), Some(b)) => format!("{key} {} → {}", shown(a), shown(b)),
            (None, Some(b)) => format!("{key} set to {}", shown(b)),
            (Some(_), None) => format!("{key} removed"),
            (None, None) => key,
        }
    };
    let mut parts: Vec<String> = d.changes.iter().take(3).map(one).collect();
    if d.changes.len() > 3 {
        parts.push(format!("and {} more", d.changes.len() - 3));
    }
    parts.join(" · ")
}

/// Settings files people often have: those in your home that nami doesn't keep yet are
/// offered, to keep in your file.
const COMMON: &[&str] = &[
    ".bashrc",
    ".zshrc",
    ".gitconfig",
    ".config/git/config",
    ".vimrc",
    ".tmux.conf",
    ".inputrc",
    ".npmrc",
    ".wezterm.lua",
    ".ssh/config",
    ".config/starship.toml",
    ".config/nvim/init.lua",
    ".config/nvim/init.vim",
    ".config/helix/config.toml",
    ".config/Code/User/settings.json",
    ".config/zed/settings.json",
    ".config/ghostty/config",
    ".config/kitty/kitty.conf",
    ".config/alacritty/alacritty.toml",
    ".config/fish/config.fish",
];

/// Of `COMMON`, those that are in `home` as files of their own (not nami's links), and that
/// neither nami keeps nor your file gives.
pub fn found_in(home: &Path, kept: &[String], given: &[String]) -> Vec<String> {
    COMMON
        .iter()
        .filter(|rel| {
            let p = home.join(rel);
            p.symlink_metadata().is_ok_and(|m| m.file_type().is_file()) && !kept.iter().any(|k| k == *rel) && !given.iter().any(|g| g.trim_start_matches("~/") == **rel)
        })
        .map(|rel| rel.to_string())
        .collect()
}

/// What's picked in the list.
#[derive(Clone, PartialEq, Debug)]
pub enum Picked {
    File(String),
    Shell,
    Found(String),
}

pub struct YourSetup {
    pub drift: Option<Result<Vec<Drift>, String>>,
    /// The files nami keeps in your home (relative to it).
    pub kept: Vec<String>,
    /// Files in your home it could keep.
    pub found: Vec<String>,
    pub picked: Option<Picked>,
    /// The picked file shown as text, not its settings.
    pub as_text: bool,
    /// The settings' fields, by file, keys and what they said when made.
    pub fields: HashMap<String, Entity<TextInput>>,
    /// Add a File…: open, and the path typed.
    pub adding: bool,
    pub add_path: Entity<TextInput>,
}

impl YourSetup {
    pub fn new(cx: &mut Context<Shell>) -> Self {
        YourSetup { drift: None, kept: Vec::new(), found: Vec::new(), picked: None, as_text: false, fields: HashMap::new(), adding: false, add_path: cx.new(|cx| TextInput::new(cx, "~/.config/app/settings.toml", "", false)) }
    }

    /// How many files apps changed, for the sidebar.
    pub fn changed(&self) -> usize {
        self.drift.as_ref().and_then(|d| d.as_ref().ok()).map_or(0, Vec::len)
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// The files nami keeps in your home, as it wrote them down.
fn kept_files(home: &Path) -> Vec<String> {
    let state = home.join(".local/state/nami/home-files.json");
    std::fs::read(state).ok().and_then(|d| serde_json::from_slice::<serde_json::Map<String, Value>>(&d).ok()).map(|m| m.keys().cloned().collect()).unwrap_or_default()
}

/// Look again, in the background: what apps changed, what nami keeps, what's in your home.
pub fn look(cx: &mut Context<Shell>) {
    cx.spawn(async move |this, cx| {
        let given = this.update(cx, |g, _| files_of(&g.user).into_iter().map(|f| f.path).collect::<Vec<_>>()).unwrap_or_default();
        let (drift, kept, found) = cx
            .background_executor()
            .spawn(async move {
                let home = home();
                let kept = kept_files(&home);
                let found = found_in(&home, &kept, &given);
                (crate::nami::json::<Vec<Drift>>(&["home", "diff", "--json"]), kept, found)
            })
            .await;
        let _ = this.update(cx, |g, cx| {
            g.your_setup.drift = Some(drift);
            g.your_setup.kept = kept;
            g.your_setup.found = found;
            cx.notify();
        });
    })
    .detach();
}

/// A file's name as its line says it: its app's, else the file's own.
fn title_of(g: &Shell, f: &SetupFile) -> String {
    match &f.app {
        Some(a) => g.app_name(a),
        None => f.path.rsplit('/').next().unwrap_or(&f.path).to_string(),
    }
}

/// A file's picture: its app's icon, or a few letters.
fn picture(f: &SetupFile, size: f32) -> AnyElement {
    match &f.app {
        Some(a) if crate::ui::catalog_icon(a).is_some() || crate::icons::dev_mark(a).is_some() => icon(crate::ui::catalog_icon(a).as_deref(), a, size),
        Some(a) => letters(&a.chars().filter(|c| c.is_alphanumeric()).take(3).collect::<String>(), size).into_any_element(),
        None => letters(&f.path.rsplit('.').next().unwrap_or("").chars().take(3).collect::<String>(), size).into_any_element(),
    }
}

/// Run one of `nami home`'s, then look again.
fn home_cmd(g: &mut Shell, title: String, args: &[&str], cx: &mut Context<Shell>) {
    let args: Vec<String> = std::iter::once("home".to_string()).chain(args.iter().map(|a| a.to_string())).collect();
    g.start(&title, vec![(args, None)], cx);
}

/// What an app changed, in a box: Keep it in your file, or Undo it.
fn drift_box(g: &Shell, n: usize, d: &Drift, files: &[SetupFile], cx: &mut Context<Shell>) -> Div {
    let f = files.iter().find(|f| f.path.trim_start_matches("~/") == d.file);
    let who = f.map(|f| title_of(g, f)).unwrap_or_else(|| format!("~/{}", d.file));
    let said = if f.is_some_and(|f| f.app.is_some()) { format!("{who} changed its settings") } else { format!("{who} was changed") };
    let busy = g.busy();
    let path = format!("~/{}", d.file);
    let (keep, undo) = (path.clone(), path.clone());
    let keep_b = if n == 0 { main_button(ElementId::NamedInteger("keep".into(), n as u64), "Keep", !busy) } else { grey(ElementId::NamedInteger("keep".into(), n as u64), "Keep") };
    sheet()
        .flex()
        .items_center()
        .gap(px(14.))
        .px(px(16.))
        .py(px(12.))
        .child(f.map(|f| picture(f, 30.)).unwrap_or_else(|| letters("~", 30.).into_any_element()))
        .child(
            div().flex_1().min_w_0()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(said))
                .child(div().text_size(px(text::META)).font_family("Geist Mono").text_color(rgb(color::muted())).truncate().child(change_line(d))),
        )
        .child(grey(ElementId::NamedInteger("undo".into(), n as u64), "Undo").when(!busy, |b| b.on_click(cx.listener(move |g, _, _, cx| home_cmd(g, format!("Putting back {undo}"), &["undo", &undo], cx)))))
        .child(keep_b.when(!busy, |b| b.on_click(cx.listener(move |g, _, _, cx| home_cmd(g, format!("Keeping the changes to {keep}"), &["keep", &keep], cx)))))
}

/// The field for a setting: made once, with what it said then.
fn field(g: &mut Shell, key: String, value: &str, cx: &mut Context<Shell>) -> Entity<TextInput> {
    if let Some(f) = g.your_setup.fields.get(&key) {
        return f.clone();
    }
    let f = cx.new(|cx| {
        let mut t = TextInput::new(cx, "", value, false);
        t.bare = true;
        t
    });
    g.your_setup.fields.insert(key, f.clone());
    f
}

/// The key a setting's field is kept by.
fn field_key(file: &str, keys: &[String], was: &Value) -> String {
    format!("{file}\u{1f}{}\u{1f}{was}", keys.join("\u{1f}"))
}

/// Save: each field that's changed, set in your file (where its entry is), then applied.
fn save(g: &mut Shell, at: Vec<String>, wrapped: bool, file: String, fields: Vec<Field>, cx: &mut Context<Shell>) {
    let Some(user) = g.file(false) else { return };
    let mut ops = Vec::new();
    for f in &fields {
        let Some(entity) = g.your_setup.fields.get(&field_key(&file, &f.keys, &f.value)) else { continue };
        let now = entity.read(cx).content.to_string();
        if now == shown(&f.value) {
            continue;
        }
        let mut path = at.clone();
        if wrapped {
            path.push("content".into());
        }
        path.extend(f.keys.iter().cloned());
        ops.push(json!({ "set": { "path": path, "value": typed(&now, &f.value) } }));
    }
    if ops.is_empty() {
        return;
    }
    for op in ops {
        g.edit_file(user.clone(), op, cx);
    }
    g.apply_now(cx);
}

/// A file's settings as fields, under their sections' names.
fn settings_view(g: &mut Shell, f: &SetupFile, cx: &mut Context<Shell>) -> Div {
    let fields = fields_of(&f.content);
    let mut list = div().flex().flex_col().rounded(px(11.)).border_1().border_color(ink(0x18)).overflow_hidden();
    let mut section: Option<String> = None;
    let mut first = true;
    for fl in &fields {
        if !fl.section.is_empty() && section.as_deref() != Some(fl.section.as_str()) {
            section = Some(fl.section.clone());
            list = list.child(div().px(px(14.)).py(px(9.)).bg(rgb(color::grouped())).when(!first, |d| d.border_t_1().border_color(ink(0x10))).text_size(px(text::META)).font_weight(FontWeight::SEMIBOLD).font_family("Geist Mono").text_color(rgb(color::muted())).child(format!("[{}]", fl.section)));
            first = false;
        }
        let label = fl.keys[if fl.section.is_empty() { 0 } else { 1 }..].join(".");
        let well = |d: Div| d.flex_1().min_w_0().h(px(30.)).px(px(10.)).flex().items_center().rounded(px(7.)).bg(ink(0x0c));
        let value: AnyElement = if is_secret(&fl.value) {
            well(div()).gap(px(8.)).text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(crate::icons::glyph("lock", 13., color::muted())).child("Secret, encrypted in your file").into_any_element()
        } else if fl.value.is_array() || fl.value.is_object() {
            well(div()).text_size(px(text::CALLOUT)).font_family("Geist Mono").text_color(rgb(color::muted())).truncate().child(fl.value.to_string()).into_any_element()
        } else {
            let entity = field(g, field_key(&f.path, &fl.keys, &fl.value), &shown(&fl.value), cx);
            well(div()).font_family("Geist Mono").child(div().flex_1().min_w_0().child(entity)).into_any_element()
        };
        list = list.child(
            div().flex().items_center().gap(px(12.)).px(px(14.)).py(px(8.)).when(!first, |d| d.border_t_1().border_color(ink(0x10)))
                .child(div().w(px(150.)).flex_none().truncate().text_size(px(text::CALLOUT)).text_color(rgb(color::text())).child(label))
                .child(value),
        );
        first = false;
    }
    if fields.is_empty() {
        list = list.child(div().p(px(14.)).child(quiet("No settings yet.")));
    }
    let (at, wrapped, file) = (f.at.clone(), f.wrapped, f.path.clone());
    div().flex().flex_col().gap(px(12.)).child(list).child(div().flex().child(grey("setup-save", "Save").on_click(cx.listener(move |g, _, _, cx| save(g, at.clone(), wrapped, file.clone(), fields.clone(), cx)))))
}

/// A file as text: what your file says, or what's in your home now.
fn text_view(f: &SetupFile) -> Div {
    let text = match &f.content {
        Value::String(s) => s.clone(),
        v if is_secret(v) => "This file is a secret. It's encrypted in your file, and written into your home only for you.".to_string(),
        _ => std::fs::read_to_string(home().join(f.path.trim_start_matches("~/"))).unwrap_or_else(|_| "The file isn't in your home yet. It's written there the next time your setup is applied.".to_string()),
    };
    let text: String = text.lines().take(60).collect::<Vec<_>>().join("\n");
    div().p(px(14.)).rounded(px(11.)).bg(ink(0x0a)).font_family("Geist Mono").text_size(px(text::META)).line_height(px(19.)).child(text)
}

/// The picked file: its settings or its text, and what happens to it.
fn file_detail(g: &mut Shell, f: &SetupFile, cx: &mut Context<Shell>) -> Div {
    let settings = f.is_settings();
    let as_text = g.your_setup.as_text || !settings;
    let head = div().flex().items_center().gap(px(10.))
        .child(div().flex_1().min_w_0().child(div().text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).child(title_of(g, f))).child(div().text_size(px(text::META)).font_family("Geist Mono").text_color(rgb(color::muted())).child(f.path.clone())))
        .when(settings, |d| {
            d.child(
                filters(false)
                    .child(filter("show-settings", "Settings", !as_text, false).on_click(cx.listener(|g, _, _, cx| {
                        g.your_setup.as_text = false;
                        cx.notify();
                    })))
                    .child(filter("show-text", "As Text", as_text, false).on_click(cx.listener(|g, _, _, cx| {
                        g.your_setup.as_text = true;
                        cx.notify();
                    }))),
            )
        });
    let body = if let Some(src) = &f.source {
        let dir = g.file(false).and_then(|u| u.parent().map(Path::to_path_buf)).unwrap_or_else(home);
        let at = dir.join(src);
        div().flex().flex_col().gap(px(10.)).child(quiet(format!("This comes from your repo, at {src}. Change it there."))).child(div().flex().child(grey("open-source", "Show in Editor").on_click(cx.listener(move |g, _, _, cx| {
            g.problem = crate::launch::open_text_file(&at, None).err().map(crate::Problem::under("Couldn't open it."));
            cx.notify();
        }))))
    } else if as_text {
        text_view(f)
    } else {
        settings_view(g, f, cx)
    };
    let rel = f.path.trim_start_matches("~/");
    let backup = home().join(format!("{rel}.nami-backup")).exists();
    let name = f.path.rsplit('/').next().unwrap_or(rel).to_string();
    let foot = match (&f.app, backup) {
        (Some(a), true) => format!("Removing {} removes this file too. The file you had before nami is kept as {name}.nami-backup.", g.app_name(a)),
        (Some(a), false) => format!("Removing {} removes this file too.", g.app_name(a)),
        (None, true) => format!("The file you had before nami is kept as {name}.nami-backup."),
        (None, false) => String::new(),
    };
    let note = if f.mutable { "The app can change this file. What it changes is shown above, to keep or undo." } else { "" };
    sheet().flex_1().min_w_0().flex().flex_col().gap(px(14.)).px(px(20.)).py(px(18.))
        .child(head)
        .child(body)
        .when(!note.is_empty(), |d| d.child(quiet(note).text_size(px(text::META))))
        .when(!foot.is_empty(), |d| d.child(quiet(foot).text_size(px(text::META))))
}

/// Your shell's variables and aliases, as fields.
fn shell_detail(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    let user = g.user.clone();
    let mut fields = Vec::new();
    for key in ["env", "aliases"] {
        if let Some(m) = user.get(key).and_then(Value::as_object) {
            for (k, v) in m {
                fields.push(Field { section: key.to_string(), keys: vec![key.to_string(), k.clone()], value: v.clone() });
            }
        }
    }
    let mut list = div().flex().flex_col().gap(px(12.));
    for (key, title) in [("env", "Variables"), ("aliases", "Aliases")] {
        let these: Vec<&Field> = fields.iter().filter(|f| f.section == key).collect();
        let mut rows = div().flex().flex_col().rounded(px(11.)).border_1().border_color(ink(0x18)).overflow_hidden();
        for (i, fl) in these.iter().enumerate() {
            let value: AnyElement = if is_secret(&fl.value) {
                div().flex_1().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child("Secret, encrypted in your file").into_any_element()
            } else {
                let entity = field(g, field_key("shell", &fl.keys, &fl.value), &shown(&fl.value), cx);
                div().flex_1().min_w_0().h(px(30.)).px(px(10.)).flex().items_center().rounded(px(7.)).bg(ink(0x0c)).font_family("Geist Mono").child(div().flex_1().min_w_0().child(entity)).into_any_element()
            };
            rows = rows.child(div().flex().items_center().gap(px(12.)).px(px(14.)).py(px(8.)).when(i > 0, |d| d.border_t_1().border_color(ink(0x10))).child(div().w(px(150.)).flex_none().truncate().font_family("Geist Mono").text_size(px(text::CALLOUT)).child(fl.keys[1].clone())).child(value));
        }
        if these.is_empty() {
            rows = rows.child(div().p(px(14.)).child(quiet(if key == "env" { "No variables yet." } else { "No aliases yet." })));
        }
        list = list.child(div().flex().flex_col().gap(px(6.)).child(div().text_size(px(text::CALLOUT)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(color::muted())).child(title)).child(rows));
    }
    sheet().flex_1().min_w_0().flex().flex_col().gap(px(14.)).px(px(20.)).py(px(18.))
        .child(div().child(div().text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).child("Variables and Aliases")).child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child("In every terminal, and the desktop's variables in its apps too.")))
        .child(list)
        .child(div().flex().child(grey("shell-save", "Save").on_click(cx.listener(move |g, _, _, cx| save(g, Vec::new(), false, "shell".into(), fields.clone(), cx)))))
}

/// A file in your home nami doesn't keep yet: what's in it, and Keep in nami.
fn found_detail(g: &Shell, rel: &str, cx: &mut Context<Shell>) -> Div {
    let text = std::fs::read_to_string(home().join(rel)).unwrap_or_default();
    let text: String = text.lines().take(40).collect::<Vec<_>>().join("\n");
    let path = format!("~/{rel}");
    let p = path.clone();
    sheet().flex_1().min_w_0().flex().flex_col().gap(px(14.)).px(px(20.)).py(px(18.))
        .child(div().flex().items_center().gap(px(10.)).child(div().flex_1().min_w_0().child(div().text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).child(path.clone())).child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child("Not kept by nami yet"))).child(grey("keep-found-detail", "Keep in nami").when(!g.busy(), |b| b.on_click(cx.listener(move |g, _, _, cx| home_cmd(g, format!("Keeping {p} in nami"), &["adopt", &p], cx))))))
        // (A Mac has no Sync yet.)
        .child(quiet(if crate::look::MAC { "Keep it in nami to have it in your user.nami: it's then kept with every change, and can be undone." } else { "Keep it in nami to have it in your user.nami: it's then kept with every change, can be undone, and comes with you to your other computers with Sync." }))
        .child(div().p(px(14.)).rounded(px(11.)).bg(ink(0x0a)).font_family("Geist Mono").text_size(px(text::META)).line_height(px(19.)).child(text))
}

pub fn render(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    let files = files_of(&g.user);
    let line = div().child(if crate::look::MAC { "Your settings files, shell and apps, all in ~/user.nami. Every change can be undone." } else { "Your settings files, shell and apps, all in ~/user.nami. Every change can be undone, and Sync brings it to your other computers." });
    let user = g.file(false);
    let show = grey("show-user-nami", "Show user.nami").on_click(cx.listener(move |g, _, _, cx| {
        if let Some(u) = user.clone() {
            g.problem = crate::launch::open_text_file(&u, None).err().map(crate::Problem::under("Couldn't open user.nami."));
            cx.notify();
        }
    }));
    let add = grey("add-file", "Add a File…").on_click(cx.listener(|g, _, window, cx| {
        g.your_setup.adding = !g.your_setup.adding;
        window.focus(&gpui::Focusable::focus_handle(g.your_setup.add_path.read(cx), cx), cx);
        cx.notify();
    }));
    let mut page = div().flex().flex_col().gap(px(16.)).pt(px(20.)).child(div().flex().items_start().gap(px(10.)).child(page_title("Your Setup", Some(line.into_any_element()))).child(show).child(add));

    // Add a File…: a file in your home, kept in nami from now on.
    if g.your_setup.adding {
        let busy = g.busy();
        page = page.child(
            sheet().flex().items_center().gap(px(12.)).px(px(16.)).py(px(12.))
                .child(div().flex_none().text_size(px(text::CALLOUT)).font_weight(FontWeight::SEMIBOLD).child("A file in your home"))
                .child(div().flex_1().min_w_0().child(g.your_setup.add_path.clone()))
                .child(grey("add-cancel", "Cancel").on_click(cx.listener(|g, _, _, cx| {
                    g.your_setup.adding = false;
                    cx.notify();
                })))
                .child(grey("add-keep", "Keep in nami").when(!busy, |b| b.on_click(cx.listener(|g, _, _, cx| {
                    let path = g.text(&g.your_setup.add_path, cx);
                    if path.is_empty() {
                        return;
                    }
                    let path = if path.starts_with("~/") || path.starts_with('/') { path } else { format!("~/{path}") };
                    g.your_setup.adding = false;
                    g.your_setup.add_path.update(cx, |t, cx| t.set(cx, ""));
                    home_cmd(g, format!("Keeping {path} in nami"), &["adopt", &path], cx);
                })))),
        );
    }

    // What apps changed.
    match g.your_setup.drift.clone() {
        Some(Ok(drift)) => {
            for (n, d) in drift.iter().enumerate() {
                page = page.child(drift_box(g, n, d, &files, cx));
            }
        }
        Some(Err(why)) => page = page.child(super::trouble("home-diff-said", "Couldn't find out what apps changed in their settings.", why, cx)),
        None => {}
    }
    let changed: Vec<String> = g.your_setup.drift.as_ref().and_then(|d| d.as_ref().ok()).map(|d| d.iter().map(|d| format!("~/{}", d.file)).collect()).unwrap_or_default();

    // The list at the left, the picked one at the right.
    let picked = g.your_setup.picked.clone().filter(|p| match p {
        Picked::File(path) => files.iter().any(|f| f.path == *path),
        Picked::Found(rel) => g.your_setup.found.contains(rel),
        Picked::Shell => true,
    });
    let picked = picked.or_else(|| files.first().map(|f| Picked::File(f.path.clone()))).unwrap_or(Picked::Shell);
    // (The list at the left, the file at the right; in a narrow window the list above it.)
    let narrow = crate::fit::narrower(640.);
    let mut list = div().when(!narrow, |d| d.w(px(320.)).flex_none()).when(narrow, |d| d.w_full()).flex().flex_col().gap(px(4.));
    let row_of = |g: &Shell, id: ElementId, f: &SetupFile, on: bool, cx: &mut Context<Shell>| {
        let path = f.path.clone();
        let sub = match &f.source {
            Some(_) => format!("{} · from your repo", f.path),
            None => f.path.clone(),
        };
        list_row(id, on)
            .child(picture(f, 28.))
            .child(div().flex_1().min_w_0().child(div().text_size(px(text::CALLOUT)).font_weight(FontWeight::SEMIBOLD).truncate().child(title_of(g, f))).child(div().text_size(px(text::META)).font_family("Geist Mono").text_color(rgb(color::muted())).truncate().child(sub)))
            .when(changed.contains(&f.path), |d| d.child(crate::ui::dot(color::key())))
            .on_click(cx.listener(move |g, _, _, cx| {
                g.your_setup.picked = Some(Picked::File(path.clone()));
                g.your_setup.as_text = false;
                cx.notify();
            }))
    };
    let (theirs, others): (Vec<&SetupFile>, Vec<&SetupFile>) = files.iter().partition(|f| f.app.is_some());
    if !theirs.is_empty() {
        list = list.child(column_heading("With their apps"));
        for (i, f) in theirs.iter().enumerate() {
            list = list.child(row_of(g, ElementId::NamedInteger("app-file".into(), i as u64), f, picked == Picked::File(f.path.clone()), cx));
        }
    }
    if !others.is_empty() {
        list = list.child(column_heading("Other files"));
        for (i, f) in others.iter().enumerate() {
            list = list.child(row_of(g, ElementId::NamedInteger("other-file".into(), i as u64), f, picked == Picked::File(f.path.clone()), cx));
        }
    }
    if files.is_empty() {
        list = list.child(div().px(px(6.)).pt(px(6.)).child(quiet("No settings files yet. Keep one of yours in nami, or add a file.")));
    }
    let n_env = g.user.get("env").and_then(Value::as_object).map_or(0, |m| m.len());
    let n_alias = g.user.get("aliases").and_then(Value::as_object).map_or(0, |m| m.len());
    let count = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
    list = list.child(column_heading("Shell")).child(
        list_row("shell-row", picked == Picked::Shell)
            .child(letters("$", 28.))
            .child(div().flex_1().min_w_0().child(div().text_size(px(text::CALLOUT)).font_weight(FontWeight::SEMIBOLD).child("Variables and aliases")).child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child(format!("{} · {}", count(n_env, "variable", "variables"), count(n_alias, "alias", "aliases")))))
            .on_click(cx.listener(|g, _, _, cx| {
                g.your_setup.picked = Some(Picked::Shell);
                cx.notify();
            })),
    );
    let found = g.your_setup.found.clone();
    if !found.is_empty() {
        list = list.child(column_heading("Found in your home"));
        for (i, rel) in found.iter().enumerate() {
            let (r, p) = (rel.clone(), format!("~/{rel}"));
            let busy = g.busy();
            list = list.child(
                list_row(ElementId::NamedInteger("found".into(), i as u64), picked == Picked::Found(rel.clone()))
                    .border_1()
                    .border_color(ink(0x18))
                    .child(div().flex_1().min_w_0().child(div().text_size(px(text::CALLOUT)).font_family("Geist Mono").truncate().child(format!("~/{rel}"))).child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child("Not kept by nami yet")))
                    .child(grey(ElementId::NamedInteger("keep-found".into(), i as u64), "Keep in nami").h(px(26.)).text_size(px(text::META)).when(!busy, |b| b.on_click(cx.listener(move |g, _, _, cx| {
                        cx.stop_propagation();
                        home_cmd(g, format!("Keeping {p} in nami"), &["adopt", &p], cx);
                    }))))
                    .on_click(cx.listener(move |g, _, _, cx| {
                        g.your_setup.picked = Some(Picked::Found(r.clone()));
                        cx.notify();
                    })),
            );
        }
    }
    let detail = match &picked {
        Picked::File(path) => match files.iter().find(|f| f.path == *path) {
            Some(f) => file_detail(g, f, cx),
            None => shell_detail(g, cx),
        },
        Picked::Shell => shell_detail(g, cx),
        Picked::Found(rel) => found_detail(g, rel, cx),
    };
    page.child(div().flex().when(narrow, |d| d.flex_col()).when(!narrow, |d| d.items_start()).gap(px(16.)).child(list).child(detail))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> Value {
        json!({
            "packages": [
                "ripgrep",
                { "python3": "3.12" },
                { "git": { "~/.gitconfig": { "user": { "name": "Ada", "email": "me@example.org" }, "github": { "token": { "!secret": "YWdl" } } } } },
                { "vscode": { "~/.config/Code/User/settings.json": { "mutable": true, "content": { "editor.fontSize": 14 } } } },
                { "neovim": { "~/.config/nvim": { "source": "./files/nvim" } } }
            ],
            "files": { "~/.ssh/config": "Host work\n", "~/.netrc": { "!secret": "YWdl" } },
            "env": { "EDITOR": "nvim" }
        })
    }

    #[test]
    fn files_are_found_with_their_apps_then_by_themselves() {
        let files = files_of(&user());
        let said: Vec<(&str, Option<&str>)> = files.iter().map(|f| (f.path.as_str(), f.app.as_deref())).collect();
        assert_eq!(said, vec![
            ("~/.gitconfig", Some("git")),
            ("~/.config/Code/User/settings.json", Some("vscode")),
            ("~/.config/nvim", Some("neovim")),
            ("~/.ssh/config", None),
            ("~/.netrc", None),
        ]);
        let code = &files[1];
        assert!(code.mutable && code.wrapped && code.is_settings());
        assert_eq!(code.at, vec!["packages", "- vscode", "~/.config/Code/User/settings.json"]);
        assert_eq!(files[2].source.as_deref(), Some("./files/nvim"));
        assert!(!files[3].is_settings());
        assert!(is_secret(&files[4].content) && !files[4].is_settings());
        assert_eq!(files[3].at, vec!["files", "~/.ssh/config"]);
    }

    #[test]
    fn settings_become_fields_under_their_sections() {
        let files = files_of(&user());
        let fields = fields_of(&files[0].content);
        let said: Vec<(String, String)> = fields.iter().map(|f| (f.section.clone(), f.keys.join("."))).collect();
        assert_eq!(said, vec![("user".into(), "user.name".into()), ("user".into(), "user.email".into()), ("github".into(), "github.token".into())]);
        assert!(is_secret(&fields[2].value));
        let flat = fields_of(&json!({ "add_newline": false, "directory": { "truncation_length": 3, "style": { "bold": true } } }));
        assert_eq!(flat.iter().map(|f| f.keys.join(".")).collect::<Vec<_>>(), vec!["add_newline", "directory.truncation_length", "directory.style.bold"]);
    }

    #[test]
    fn a_field_keeps_its_settings_kind() {
        assert_eq!(typed("16", &json!(14)), json!(16));
        assert_eq!(typed("1.5", &json!(1)), json!(1.5));
        assert_eq!(typed("big", &json!(14)), json!("big"));
        assert_eq!(typed("false", &json!(true)), json!(false));
        assert_eq!(typed("main", &json!("master")), json!("main"));
        assert_eq!(shown(&json!(14)), "14");
        assert_eq!(shown(&json!("Ada")), "Ada");
    }

    #[test]
    fn what_an_app_changed_is_said_in_a_line() {
        let d: Drift = serde_json::from_value(json!({ "file": ".config/Code/User/settings.json", "changes": [
            { "path": ["editor.fontSize"], "from": 14, "to": 16 },
            { "path": ["workbench.colorTheme"], "from": null, "to": "Default Dark Modern" }
        ], "whole": false })).unwrap();
        assert_eq!(change_line(&d), "editor.fontSize 14 → 16 · workbench.colorTheme set to Default Dark Modern");
        let whole = Drift { file: ".bashrc".into(), changes: vec![], whole: true };
        assert_eq!(change_line(&whole), "Its text changed.");
    }

    #[test]
    fn only_files_nami_doesnt_keep_are_offered() {
        let home = std::env::temp_dir().join(format!("nami-gui-found-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".config")).unwrap();
        std::fs::write(home.join(".bashrc"), "x").unwrap();
        std::fs::write(home.join(".gitconfig"), "x").unwrap();
        std::fs::write(home.join(".vimrc"), "x").unwrap();
        std::os::unix::fs::symlink(home.join(".vimrc"), home.join(".tmux.conf")).unwrap();
        let found = found_in(&home, &[".gitconfig".to_string()], &["~/.vimrc".to_string()]);
        assert_eq!(found, vec![".bashrc".to_string()]);
        let _ = std::fs::remove_dir_all(&home);
    }
}
