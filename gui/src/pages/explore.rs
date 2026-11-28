//! Looking around the catalog (My apps and Apps, before anything's
//! searched): the most used apps, of each kind; and
//! an app's own page, with its screenshots, what it is, and its versions to
//! pick from. Each page that has them says where adding goes (`Explorer`).

use crate::look::Type as _;
use gpui::prelude::*;
use crate::look::Press;
use gpui::{AnyElement, Context, Div, ElementId, FontWeight, ObjectFit, ParentElement, Stateful, Styled, div, img, rgb};
use crate::look::px;
use crate::look::text;
use serde_json::{Value, json};

use super::{entry_name, packages};
use crate::nami::{self, Found, Version};
use crate::ui::{app_icon, color, pill};
use crate::Shell;

/// The kinds of app: `nami search --category` keys, and their names.
pub const KINDS: &[(&str, &str)] = &[
    ("office", "Office"),
    ("graphics", "Graphics and Photos"),
    ("audio-video", "Music and Video"),
    ("games", "Games"),
    ("internet", "Internet"),
    ("coding", "Coding"),
    ("learning", "Learning"),
    ("tools", "Tools"),
];

/// How many apps a kind shows at first, and more each time.
const PAGE: usize = 12;

/// A page apps can be added on, and looked around from.
pub trait Explorer: Sized + 'static {
    fn explore(&mut self) -> &mut Explore;
    /// How `attr` is listed where adding goes: None, not; Some(None), the
    /// newest; Some(Some(v)), kept at v.
    fn listed(&self, attr: &str) -> Option<Option<String>>;
    /// What the add button says for `f`, or (Err) why there's none.
    fn action(&self, f: &Found) -> Result<String, String>;
    /// The same, as short as a card has room for.
    fn card_action(&self, f: &Found) -> Result<String, String> {
        self.action(f)
    }
    /// What's happening to `f`, if anything: installing, waiting its
    /// turn, failed, just done, or changed in the files for Apply.
    fn doing(&self, _f: &Found) -> Option<Doing> {
        None
    }
    /// After it failed: again, or what nami said.
    fn retry(&mut self, _attr: &str, _cx: &mut Context<Self>) {}
    fn details(&mut self, _attr: &str, _cx: &mut Context<Self>) {}
    /// Remove asks first ("Remove GIMP?"): whose row asks now.
    fn asking_remove(&self, _attr: &str) -> bool {
        false
    }
    /// Ask on `attr`'s row (None: stop asking). Where nothing's asked, it's taken off.
    fn ask_remove(&mut self, attr: Option<&str>, cx: &mut Context<Self>) {
        if let Some(a) = attr {
            self.take_off(a, cx)
        }
    }
    /// What a listed app says it is ("Installed", "System app", "In music").
    fn listed_text(&self, attr: &str) -> String;
    /// List `attr` (kept at `version`; None: the newest), in place of
    /// what's listed for it.
    fn put(&mut self, attr: &str, version: Option<String>, cx: &mut Context<Self>);
    fn take_off(&mut self, attr: &str, cx: &mut Context<Self>);
    /// The page the app page goes back to.
    /// The menu entry that starts it, if it's on this computer to open
    /// (the window's own pages; not a computer being made).
    fn entry_of(&self, _f: &Found) -> Option<std::path::PathBuf> {
        None
    }
    fn open(&mut self, _entry: &std::path::Path, _cx: &mut Context<Self>) {}
    fn back_to(&self) -> String;
}

/// What's happening to an app, as its button and the line under it say.
#[derive(Clone, PartialEq, Debug)]
pub enum Doing {
    /// Changed in the files, for Apply: it'll be installed (or removed).
    Pending { install: bool },
    /// Its turn's next: "Waiting".
    Waiting,
    /// "Installing…", and "Downloading 120 MB…" while a big one comes down.
    Going { label: String, note: Option<String> },
    /// "Couldn't install GeoGebra".
    Failed { what: String },
    /// "GeoGebra is installed."
    Done { what: String },
}

#[derive(Default)]
pub struct Explore {
    /// The kind looked at (None: all apps, the most used first).
    pub kind: Option<&'static str>,
    pub list: Option<(Option<&'static str>, Result<Vec<Found>, String>)>,
    pub loading: bool,
    pub shown: usize,
    /// An app's own page, open.
    pub app: Option<AppView>,
    /// The app the front page shows big, with its screenshots.
    pub featured: Option<Found>,
    pub featuring: bool,
    /// The most used, all of them (See All), not only the first row.
    pub all: bool,
    /// An app's page: its Install for everyone, not only for you (an admin's choice).
    pub for_system: bool,
    /// An app's page: the release line its Install keeps it at (None: the newest).
    pub pick: Option<String>,
    /// Discover's row of kinds, as it's slid sideways in a narrow window.
    pub kinds_scroll: gpui::ScrollHandle,
}

#[derive(Clone)]
pub struct AppView {
    pub attr: String,
    /// What the search knew, until `nami info` answers.
    pub first: Option<Found>,
    pub info: Option<Result<Found, String>>,
    pub versions: Option<Result<Vec<Version>, String>>,
    /// Every version shown, not only each line's newest.
    pub all: bool,
    /// The screenshot shown big.
    pub shot: usize,
    /// Apps of its kind, the most used first.
    pub similar: Vec<Found>,
}

/// The apps of the kind picked, from nami, once.
pub fn load<V: Explorer>(g: &mut V, cx: &mut Context<V>) {
    let e = g.explore();
    let kind = e.kind;
    if e.loading || e.list.as_ref().is_some_and(|(k, _)| *k == kind) {
        return;
    }
    e.loading = true;
    cx.spawn(async move |this, cx| {
        let list = cx.background_executor().spawn(async move { nami::popular(kind) }).await;
        let _ = this.update(cx, |g, cx| {
            let e = g.explore();
            e.loading = false;
            e.list = Some((kind, list));
            e.shown = PAGE;
            if e.kind != kind {
                load(g, cx);
            }
            cx.notify();
        });
    })
    .detach();
}

/// Open an app's page: what it is and its versions, from nami.
pub fn open<V: Explorer>(g: &mut V, found: Option<Found>, attr: String, cx: &mut Context<V>) {
    g.explore().for_system = false;
    g.explore().pick = None;
    g.explore().app = Some(AppView { attr: attr.clone(), first: found, info: None, versions: None, all: false, shot: 0, similar: Vec::new() });
    let a = attr.clone();
    cx.spawn(async move |this, cx| {
        let info = cx.background_executor().spawn(async move { nami::info(&a) }).await;
        let kind = info.as_ref().ok().and_then(|f| f.categories.first().cloned());
        let _ = this.update(cx, |g, cx| {
            if let Some(v) = g.explore().app.as_mut().filter(|v| v.attr == attr) {
                v.info = Some(info);
                cx.notify();
            }
        });
        if let Some(kind) = kind {
            let similar = cx.background_executor().spawn(async move { nami::popular(Some(&kind)) }).await.unwrap_or_default();
            let _ = this.update(cx, |g, cx| {
                if let Some(v) = g.explore().app.as_mut().filter(|v| v.attr == attr) {
                    v.similar = similar.into_iter().filter(|f| f.attr != attr).take(4).collect();
                    cx.notify();
                }
            });
        }
        let a = attr.clone();
        let versions = cx.background_executor().spawn(async move { nami::versions(&a) }).await;
        let _ = this.update(cx, |g, cx| {
            if let Some(v) = g.explore().app.as_mut().filter(|v| v.attr == attr) {
                v.versions = Some(versions);
                cx.notify();
            }
        });
    })
    .detach();
    cx.notify();
}

/// Where adding an app goes on My apps and Apps, and what's there: the
/// page's file, or the own pick the search adds to (Apps).
#[derive(Clone)]
pub struct Target {
    pub file: Option<std::path::PathBuf>,
    /// The own pick's name, when it's that.
    pub pick: Option<String>,
    pub listed: Vec<Value>,
    /// system.nami, from Your Apps: the app's for everyone there,
    /// or sets up the computer (docker, steam), so it can only be.
    pub for_system: bool,
}

impl Target {
    pub fn of(g: &Shell, system: bool) -> Target {
        let dir = g.file(system).and_then(|f| f.parent().map(|p| p.to_path_buf()));
        if let Some(pick) = g.own_target.clone().filter(|_| system) {
            let file = dir.map(|d| d.join("../packages").join(&pick).join("package.nami"));
            let listed = file.as_ref().and_then(|f| nami::get(f).ok()).map(|v| packages(&v)).unwrap_or_default();
            return Target { file, pick: Some(pick), listed, for_system: false };
        }
        Target::file(g, system)
    }

    /// system.nami or yours, as it is.
    pub fn file(g: &Shell, system: bool) -> Target {
        let file_v = if system { &g.system } else { &g.user };
        Target { file: g.file(system), pick: None, listed: packages(file_v), for_system: false }
    }

    /// Where `attr` is, or goes, from the page it's looked at on. On Apps
    /// (everyone's), system.nami. On Your Apps, yours, unless it's
    /// everyone's already or sets up the computer: then the computer's (for someone
    /// who isn't an admin, with an admin's name and password).
    pub fn for_app(g: &Shell, attr: &str) -> Target {
        if g.computer_page() {
            return Target::of(g, true);
        }
        let mine = Target::file(g, false);
        if mine.entry(attr).is_some() || attr.starts_with("flathub:") {
            return mine;
        }
        let for_system = Target::file(g, true);
        if for_system.entry(attr).is_some() || g.sets_up_system(attr) {
            return Target { for_system: true, ..for_system };
        }
        mine
    }

    /// Its entry for `attr`, if it lists it: `gimp`, or `gimp: "3.0"`. On a Mac, one of
    /// Homebrew's casks by either name: `cask:iterm2`, or `iterm2` (the cask, where nixpkgs has no
    /// ready-built Mac build of it).
    pub fn entry(&self, attr: &str) -> Option<Value> {
        let other = attr.strip_prefix("cask:").map(String::from).filter(|_| crate::look::MAC);
        self.listed.iter().find(|e| entry_name(e) == attr).or_else(|| other.and_then(|o| self.listed.iter().find(|e| entry_name(e) == o))).cloned()
    }

    /// What the add button says, or (Err) why there's none.
    pub fn action(&self, f: &Found) -> Result<String, String> {
        match (&self.pick, self.entry(&f.attr).is_some(), f.installed) {
            (Some(p), true, _) => Err(format!("In {p}")),
            (Some(p), false, _) => Ok(format!("Add to {p}")),
            (None, true, _) => Err("Installed".into()),
            (None, false, true) => Err("On this computer".into()),
            (None, false, false) => Ok("Install".into()),
        }
    }
}

/// List `value` (`gimp`, or `{gimp: "3.0"}`) where adding goes, in place
/// of what's listed for it: installed now, on its own (an own pick's
/// apps wait for Apply, with the rest of it).
pub fn put(g: &mut Shell, t: Target, attr: &str, value: Value, cx: &mut Context<Shell>) {
    let Some(file) = t.file.clone() else { return };
    let old = t.entry(attr);
    if old.as_ref() == Some(&value) {
        return;
    }
    if t.pick.is_none() {
        // For everyone, what sets up the computer comes with its setup (someone who isn't an
        // admin: an admin's password first, for copying it in).
        if g.file(true).as_ref() == Some(&file) && g.sets_up_system(attr) {
            let (again, a, v) = (t.clone(), attr.to_string(), value.clone());
            if g.unlock_first(&format!("{} is being installed on the system, for everyone on this computer.", g.app_name(attr)), move |g, cx| put(g, again, &a, v, cx), cx) {
                return;
            }
        }
        if g.file(true).as_ref() == Some(&file) && !copy_setup(g, attr, cx) {
            return;
        }
        return g.install(file, attr, value, old, cx);
    }
    if let Some(old) = old {
        g.edit_file(file.clone(), json!({ "remove": { "path": ["packages"], "value": old } }), cx);
    }
    g.edit_file(file, json!({ "add": { "path": ["packages"], "value": value } }), cx);
    g.apply_now(cx);
}

/// Nami's package that sets `attr` up on the computer (docker's service,
/// steam's drivers), copied next to the computer's folder (`nami new`), as
/// its add-on in Apps does, before it's listed there. Whether it's there.
pub fn copy_setup(g: &mut Shell, attr: &str, cx: &mut Context<Shell>) -> bool {
    let Some(dir) = g.file(true).and_then(|f| f.parent().map(|p| p.to_path_buf())) else { return false };
    if !g.sets_up_system(attr) || dir.join("../packages").join(attr).join("package.nami").exists() {
        return true;
    }
    let out = std::process::Command::new(std::env::var_os("NAMI").unwrap_or_else(|| "nami".into())).arg("new").arg(&dir).args(["--from", attr]).output();
    let why = match out {
        Ok(o) if o.status.success() => return true,
        Ok(o) => String::from_utf8_lossy(&o.stderr).to_string(),
        Err(e) => format!("nami: {e}"),
    };
    g.problem = Some(crate::Problem::of(format!("Couldn't install {}.", g.app_name(attr)), why));
    cx.notify();
    false
}

pub fn take_off(g: &mut Shell, t: Target, attr: &str, cx: &mut Context<Shell>) {
    let (Some(old), Some(file)) = (t.entry(attr), t.file) else { return };
    if t.pick.is_none() {
        return g.uninstall(file, attr, old, cx);
    }
    g.edit_file(file, json!({ "remove": { "path": ["packages"], "value": old } }), cx);
    g.apply_now(cx);
}

impl Explorer for Shell {
    fn explore(&mut self) -> &mut Explore {
        &mut self.explore
    }

    fn listed(&self, attr: &str) -> Option<Option<String>> {
        Some(match Target::for_app(self, attr).entry(attr)? {
            Value::Object(m) => m.values().next().and_then(|v| v.as_str().map(String::from).or_else(|| v.get("version").and_then(Value::as_str).map(String::from))),
            _ => None,
        })
    }

    fn action(&self, f: &Found) -> Result<String, String> {
        // A child's apps are the computer's, an admin's to choose.
        if self.role.as_ref().is_ok_and(|r| r.child) && !self.computer_page() && !f.installed {
            return Err("Only an admin can install this app".into());
        }
        // (What sets up the computer is only ever everyone's: anyone may install it, with an
        // admin's name and password.)
        let t = Target::for_app(self, &f.attr);
        // Just removed: the search's "on this computer" is from before.
        if self.job(&f.attr).is_some_and(|j| j.kind == crate::jobs::Kind::Remove && j.state == crate::jobs::State::Done) && t.entry(&f.attr).is_none() {
            return Ok("Install".into());
        }
        t.action(f)
    }

    /// On a card, as a store has it: Get, and Installed for what's on the computer already.
    fn card_action(&self, f: &Found) -> Result<String, String> {
        match self.action(f) {
            Ok(l) if l == "Install" => Ok("Get".into()),
            Err(why) if why == "On this computer" => Err("Installed".into()),
            other => other,
        }
    }

    fn doing(&self, f: &Found) -> Option<Doing> {
        if let Some(j) = self.job(&f.attr) {
            let name = f.name();
            return Some(match &j.state {
                crate::jobs::State::Waiting => Doing::Waiting,
                crate::jobs::State::Going => Doing::Going { label: format!("{}…", j.kind.verb()), note: j.note() },
                crate::jobs::State::Failed(_) => Doing::Failed {
                    what: match j.reason() {
                        Some(why) => format!("Couldn't {} {name}: {why}", j.kind.plain()),
                        None => format!("Couldn't {} {name}", j.kind.plain()),
                    },
                },
                crate::jobs::State::Done => Doing::Done { what: format!("{name} is {}.", j.kind.done()) },
            });
        }
        let t = Target::for_app(self, &f.attr);
        let file = t.file.as_ref().filter(|_| t.pick.is_none())?;
        let install = self.will_change(file, &f.attr, t.entry(&f.attr).is_some())?;
        Some(Doing::Pending { install })
    }

    fn retry(&mut self, attr: &str, cx: &mut Context<Self>) {
        Shell::retry(self, attr, cx)
    }

    fn details(&mut self, attr: &str, cx: &mut Context<Self>) {
        self.details = Some(attr.to_string());
        cx.notify();
    }

    fn asking_remove(&self, attr: &str) -> bool {
        self.asking_remove.as_deref() == Some(attr)
    }

    fn ask_remove(&mut self, attr: Option<&str>, cx: &mut Context<Self>) {
        self.asking_remove = attr.map(String::from);
        cx.notify();
    }

    fn listed_text(&self, attr: &str) -> String {
        let t = Target::for_app(self, attr);
        match t.pick {
            Some(p) => format!("In {p}"),
            None if t.for_system => "System app".into(),
            // (Where there's a choice, anyone's: their own.)
            None if !self.computer_page() => "Installed for you".into(),
            None => "Installed".into(),
        }
    }

    fn put(&mut self, attr: &str, version: Option<String>, cx: &mut Context<Self>) {
        let value = match version {
            Some(v) => json!({ attr: v }),
            None => json!(attr),
        };
        let t = Target::for_app(self, attr);
        put(self, t, attr, value, cx)
    }

    fn take_off(&mut self, attr: &str, cx: &mut Context<Self>) {
        let t = Target::for_app(self, attr);
        take_off(self, t, attr, cx)
    }

    fn back_to(&self) -> String {
        match self.page {
            crate::Page::Apps => "System Apps",
            crate::Page::Installed => "Installed",
            crate::Page::Storage => "Storage",
            _ => "Discover",
        }
        .into()
    }

    fn entry_of(&self, f: &Found) -> Option<std::path::PathBuf> {
        self.launchers.get(&f.attr).cloned().or_else(|| crate::launch::app_entry(&f.attr, &f.desktop))
    }

    fn open(&mut self, entry: &std::path::Path, cx: &mut Context<Self>) {
        self.problem = crate::launch::open_app(entry).err().map(crate::Problem::under("Couldn't open the app."));
        cx.notify();
    }
}

/// A small grey button on a row or a card: Get, Open, Update.
pub fn small_action(id: ElementId, text: String) -> Stateful<Div> {
    div().id(id).px(px(12.)).h(px(28.)).flex().flex_none().items_center().rounded_full().bg(color::push()).text_size(px(text::META)).font_weight(FontWeight::SEMIBOLD).cursor_pointer().press_row().hover(|s| s.bg(color::round_off())).child(text)
}

/// An app as a card to look at: its icon, name and line, and its add button.
pub fn card<V: Explorer>(g: &V, n: usize, f: &Found, cx: &mut Context<V>) -> Stateful<Div> {
    let action = action_of(g, ElementId::NamedInteger("card-add".into(), n as u64), f, cx);
    let (found, attr) = (f.clone(), f.attr.clone());
    div()
        .id(ElementId::NamedInteger("card".into(), n as u64))
        .flex()
        .items_center()
        .gap(px(12.))
        .p(px(12.))
        .min_w_0()
        .rounded(px(13.))
        .border_1()
        .border_color(crate::ui::ink(0x14))
        .bg(rgb(color::panel()))
        .cursor_pointer()
        .press_row()
        .child(app_icon(f, 44.))
        .child(
            div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(crate::tip::cut(f.name())))
                .child(div().text_size(px(text::META)).text_color(rgb(color::muted())).truncate().child(line_of_app(g, ElementId::NamedInteger("card-line".into(), n as u64), f, false, cx))),
        )
        .child(action)
        .on_click(cx.listener(move |this, _, _, cx| open(this, Some(found.clone()), attr.clone(), cx)))
}

/// The front page's big app: one of the most used with screenshots, a
/// different one each day, fetched with its screenshots once.
pub fn feature<V: Explorer>(g: &mut V, list: &[Found], cx: &mut Context<V>) {
    let e = g.explore();
    if e.featured.is_some() || e.featuring {
        return;
    }
    // (A cask has no words about it beyond its line: its App Store screenshots are enough.)
    let picks: Vec<&Found> = list.iter().take(40).filter(|f| !f.screenshots.is_empty() && (f.about.is_some() || f.cask())).collect();
    let day = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / 86400).unwrap_or(0) as usize;
    let Some(pick) = picks.get(day % picks.len().max(1)).map(|f| f.attr.clone()) else { return };
    e.featuring = true;
    cx.spawn(async move |this, cx| {
        let info = cx.background_executor().spawn(async move { nami::info(&pick) }).await;
        let _ = this.update(cx, |g, cx| {
            g.explore().featured = info.ok();
            cx.notify();
        });
    })
    .detach();
}

/// Changed in the files, for Apply: a grey pill, with the slate's dot for what changed.
pub fn changing_pill(label: String) -> Div {
    div().px(px(12.)).h(px(28.)).flex().flex_none().items_center().gap(px(7.)).rounded_full().bg(color::push()).text_color(rgb(color::text())).text_size(px(text::META)).font_weight(FontWeight::MEDIUM)
        .child(div().size(px(6.)).rounded_full().bg(rgb(color::key())))
        .child(label)
}

/// Being installed (or waiting its turn): where its button was, a quiet
/// pill with a small spinner.
pub fn busy_pill(id: ElementId, label: &str, turning: bool) -> Div {
    div().px(px(12.)).h(px(28.)).flex().flex_none().items_center().gap(px(8.)).rounded_full().bg(color::push()).text_color(rgb(color::muted())).text_size(px(text::META)).font_weight(FontWeight::MEDIUM)
        .child(crate::motion::spinner(id, 13., color::text(), turning))
        .child(label.to_string())
}

/// An app's button while something's happening to it (None: nothing is,
/// or it's done and its usual button says so).
pub fn doing_button<V: Explorer>(id: ElementId, doing: &Doing, attr: &str, cx: &mut Context<V>) -> Option<AnyElement> {
    let spin = ElementId::Name(format!("{id}-spin").into());
    Some(match doing {
        Doing::Pending { install } => changing_pill(if *install { "Will install" } else { "Will remove" }.into()).into_any_element(),
        Doing::Waiting => busy_pill(spin, "Waiting", false).into_any_element(),
        Doing::Going { label, .. } => busy_pill(spin, label, true).into_any_element(),
        Doing::Failed { .. } => {
            let a = attr.to_string();
            small_action(id, "Try Again".into())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.retry(&a, cx)
                }))
                .into_any_element()
        }
        Doing::Done { .. } => return None,
    })
}

/// The line under an app's name: what it is; or, while something's
/// happening to it, how that goes ("Downloading 120 MB…", "Couldn't
/// install GIMP · Details", "GIMP is installed.").
pub fn line_of_app<V: Explorer>(g: &V, id: ElementId, f: &Found, stacked: bool, cx: &mut Context<V>) -> AnyElement {
    match g.doing(f) {
        Some(Doing::Going { note: Some(note), .. }) => div().child(crate::tip::cut(note)).into_any_element(),
        Some(Doing::Failed { what }) => {
            let a = f.attr.clone();
            div().flex().when(stacked, |d| d.flex_col()).when(!stacked, |d| d.items_center().gap(px(6.))).min_w_0()
                // (It gives way, cut short, so Details always shows: a long name pushed it out.)
                .child(div().min_w_0().text_color(rgb(color::bad())).child(crate::tip::cut(what)))
                .when(!stacked, |d| d.child(div().text_color(rgb(color::faint())).child("·")))
                .child(div().id(id).flex_none().text_color(rgb(color::text())).cursor_pointer().press().hover(|s| s.underline()).child("Details").on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.details(&a, cx)
                })))
                .into_any_element()
        }
        Some(Doing::Done { what }) => div().text_color(rgb(color::good())).child(crate::tip::cut(what)).into_any_element(),
        _ => div().child(crate::tip::cut(f.line())).into_any_element(),
    }
}

/// The add button of a card or tile (Open, once it's there), or why there's none.
pub fn action_of<V: Explorer>(g: &V, id: ElementId, f: &Found, cx: &mut Context<V>) -> AnyElement {
    if let Some(b) = g.doing(f).and_then(|d| doing_button(id.clone(), &d, &f.attr, cx)) {
        return b;
    }
    if let Some(entry) = g.listed(&f.attr).and_then(|_| g.entry_of(f)) {
        return small_action(id, "Open".into())
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.open(&entry, cx)
            }))
            .into_any_element();
    }
    match g.card_action(f) {
        Ok(label) => {
            let attr = f.attr.clone();
            small_action(id, label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.put(&attr, None, cx)
                }))
                .into_any_element()
        }
        Err(why) => div().h(px(28.)).flex().items_center().text_size(px(text::META)).text_color(rgb(color::muted())).child(why).into_any_element(),
    }
}

/// The featured app: what it is and its screenshot, side by side.
fn hero<V: Explorer>(g: &V, f: &Found, cx: &mut Context<V>) -> Stateful<Div> {
    let (found, attr) = (f.clone(), f.attr.clone());
    let shot = f.screenshots.first().map(|s| img(std::path::PathBuf::from(s)).size_full().object_fit(ObjectFit::Cover));
    // (Side by side where there's room; in a narrow window the picture above the words.)
    let stacked = crate::fit::narrower(600.);
    let words = div().when(!stacked, |d| d.w(px(300.)).flex_none()).p(px(22.)).flex().flex_col().gap(px(8.))
        .child(div().text_size(px(text::CAPTION)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(color::muted())).child("Today's app"))
        .child(app_icon(f, 48.))
        .child(div().type_size(text::DIALOG).font_weight(FontWeight::BOLD).line_clamp(2).text_ellipsis().child(f.name()))
        .child(match g.doing(f) {
            Some(Doing::Going { note: Some(_), .. } | Doing::Failed { .. } | Doing::Done { .. }) => div().h(px(38.)).text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(line_of_app(g, "hero-line".into(), f, true, cx)),
            _ => div().h(px(38.)).text_size(px(text::CALLOUT)).line_height(px(19.)).text_color(rgb(color::muted())).line_clamp(2).text_ellipsis().child(f.line()),
        })
        .when(!stacked, |d| d.child(div().flex_1()))
        .child(div().flex().child(action_of(g, "hero-add".into(), f, cx)));
    let picture = div().when(!stacked, |d| d.flex_1().min_w_0().h_full()).when(stacked, |d| d.w_full().h(px(180.))).overflow_hidden().children(shot);
    div()
        .id("hero")
        .flex()
        .when(stacked, |d| d.flex_col())
        .when(!stacked, |d| d.h(px(256.)))
        .rounded(px(crate::look::shape::CARD))
        .overflow_hidden()
        .border_1()
        .border_color(color::hairline())
        .bg(color::push())
        .cursor_pointer()
        .press()
        .children(if stacked { [picture, words] } else { [words, picture] })
        .on_click(cx.listener(move |this, _, _, cx| open(this, Some(found.clone()), attr.clone(), cx)))
}

/// An app as a tile in a row: its icon, name and line, and its add button.
fn tile<V: Explorer>(g: &V, id: &str, n: usize, f: &Found, cx: &mut Context<V>) -> Stateful<Div> {
    let (found, attr) = (f.clone(), f.attr.clone());
    div()
        .id(ElementId::NamedInteger(id.to_string().into(), n as u64))
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(14.))
        .min_w_0()
        .rounded(px(crate::look::shape::CARD))
        .border_1()
        .border_color(color::hairline())
        .bg(rgb(color::panel()))
        .cursor_pointer()
        .press_row()
        .hover(|s| s.bg(color::hover_pill()))
        .child(app_icon(f, 48.))
        .child(
            div().flex().flex_col().gap(px(2.)).min_w_0()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(crate::tip::cut(f.name())))
                .child(match g.doing(f) {
                    Some(Doing::Going { note: Some(_), .. } | Doing::Failed { .. } | Doing::Done { .. }) => div().h(px(36.)).text_size(px(text::META)).line_height(px(18.)).text_color(rgb(color::muted())).child(line_of_app(g, ElementId::NamedInteger(format!("{id}-line").into(), n as u64), f, true, cx)),
                    _ => div().h(px(36.)).text_size(px(text::META)).line_height(px(18.)).text_color(rgb(color::muted())).line_clamp(2).text_ellipsis().child(f.line()),
                }),
        )
        .child(div().flex().child(action_of(g, ElementId::NamedInteger(format!("{id}-add").into(), n as u64), f, cx)))
        .on_click(cx.listener(move |this, _, _, cx| open(this, Some(found.clone()), attr.clone(), cx)))
}

/// A row's heading, with "See all" to its kind's page.
fn row_head<V: Explorer>(title: &str, kind: Option<&'static str>, n: usize, cx: &mut Context<V>) -> Div {
    div().flex().items_center().justify_between().pt(px(6.))
        .child(div().text_size(px(text::SECTION)).font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
        .child(
            div().id(ElementId::NamedInteger("see-all".into(), n as u64)).text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).cursor_pointer().press().hover(|s| s.text_color(rgb(color::text()))).child("See All  ›").on_click(cx.listener(move |this, _, _, cx| {
                this.explore().kind = kind;
                this.explore().shown = PAGE;
                load(this, cx);
                cx.notify();
            })),
        )
}

/// Looking around: the kinds to pick; with none picked, today's app and a
/// row of each kind's most used; with one, all of its apps.
pub fn browse<V: Explorer>(g: &mut V, cx: &mut Context<V>) -> Div {
    load(g, cx);
    let kind = g.explore().kind;
    let kinds = std::iter::once((None, "Discover")).chain(KINDS.iter().map(|(k, n)| (Some(*k), *n))).enumerate().map(|(i, (k, name))| {
        pill(ElementId::NamedInteger("kind".into(), i as u64), name, kind == k).h(px(28.)).text_size(px(text::META)).on_click(cx.listener(move |this, _, _, cx| {
            this.explore().kind = k;
            this.explore().shown = PAGE;
            load(this, cx);
            cx.notify();
        }))
    });
    let mut out = div().flex().flex_col().gap(px(12.)).child(div().flex().flex_wrap().gap(px(6.)).children(kinds));
    let (shown, list) = {
        let e = g.explore();
        (e.shown, e.list.as_ref().filter(|(k, _)| *k == kind).map(|(_, l)| l.clone()))
    };
    match list {
        Some(Ok(list)) if kind.is_none() => {
            feature(g, &list, cx);
            if let Some(f) = g.explore().featured.clone() {
                out = out.child(hero(g, &f, cx));
            }
            out = out.child(row_head("Most Used", None, 0, cx));
            let top: Vec<_> = list.iter().take(4).enumerate().map(|(n, f)| tile(g, "top", n, f, cx)).collect();
            out = out.child(crate::fit::grid(crate::fit::APP_CARD, 10., 4).children(top));
            // Each app once on the page: in the first row it's of.
            let mut shown: Vec<String> = list.iter().take(4).map(|f| f.attr.clone()).collect();
            for (r, (key, name)) in KINDS.iter().enumerate() {
                let apps: Vec<Found> = list.iter().filter(|f| !shown.contains(&f.attr) && f.categories.iter().any(|c| c == key)).take(4).cloned().collect();
                if apps.len() < 4 {
                    continue;
                }
                shown.extend(apps.iter().map(|f| f.attr.clone()));
                out = out.child(row_head(name, Some(*key), r + 1, cx));
                let tiles: Vec<_> = apps.iter().enumerate().map(|(n, f)| tile(g, key, n, f, cx)).collect();
                out = out.child(crate::fit::grid(crate::fit::APP_CARD, 10., 4).children(tiles));
            }
        }
        Some(Ok(list)) => {
            let shown = shown.min(list.len());
            let cards: Vec<_> = list[..shown].iter().enumerate().map(|(n, f)| card(g, n, f, cx)).collect();
            out = out.child(crate::fit::grid(crate::fit::APP_TILE, 8., 2).children(cards));
            if list.len() > shown {
                out = out.child(div().flex().child(pill("more-apps".into(), "Show More", false).on_click(cx.listener(|this, _, _, cx| {
                    this.explore().shown += PAGE * 2;
                    cx.notify();
                }))));
            }
        }
        Some(Err(why)) => out = out.child(super::failed("explore-again", "Couldn't load apps.", why, |this: &mut V, cx| {
            this.explore().list = None;
            load(this, cx);
        }, cx)),
        None => out = out.child(super::looking("explore-looking")),
    }
    out
}

/// `3.2.6` -> `3.2`: the release line a version is of.
fn line_of(v: &str) -> String {
    let mut parts = v.split(['.', '-']);
    match (parts.next(), parts.next()) {
        (Some(a), Some(b)) => format!("{a}.{b}"),
        _ => v.to_string(),
    }
}

/// `3.10.2` > `3.9`: numbers part by part.
fn newer(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |v: &str| v.split(['.', '-']).map(|p| (p.chars().take_while(char::is_ascii_digit).collect::<String>().parse::<u64>().unwrap_or(0), p.to_string())).collect::<Vec<_>>();
    key(a).cmp(&key(b))
}

/// Who an app's Install is for, and what's said of it under the switch: (for everyone, whether
/// the person may pick, the line).
pub fn install_for(g: &Shell, f: &Found) -> (bool, bool, String) {
    let (for_system, may_pick, said) = install_line(g, f);
    // On another distribution, what Everyone covers there.
    let more = if for_system { super::apps::everyone_here().map(|s| format!(" {s}")).unwrap_or_default() } else { String::new() };
    (for_system, may_pick, format!("{said}{more}"))
}

fn install_line(g: &Shell, f: &Found) -> (bool, bool, &'static str) {
    if f.flathub() {
        return (false, false, "Apps from Flathub are installed only for you. No password is needed.");
    }
    // (A Mac's nami installs for everyone with sudo alone, in Terminal.)
    if crate::look::MAC {
        return (false, false, "It's installed only for you. No password is needed.");
    }
    if g.sets_up_system(&f.attr) {
        return (true, false, "This app sets up the computer, so it's installed for everyone. An administrator's password is needed.");
    }
    if g.explore.for_system {
        (true, true, "Everyone on this computer gets it. An administrator's password is needed.")
    } else {
        (false, true, "It's installed only for you. No password is needed.")
    }
}

/// A path as it's said: `~/user.nami`.
fn shown_path(p: &std::path::Path) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match p.strip_prefix(&home) {
        Ok(rest) if !home.is_empty() => format!("~/{}", rest.display()),
        _ => p.display().to_string(),
    }
}

/// The lines an app's install writes in its file, as they read there (`- gimp`, or kept at a
/// version, `- gimp: "3.0"`).
pub fn file_lines(file: &str, attr: &str, version: Option<&str>) -> String {
    let entry = match version {
        Some(v) => format!("  - {attr}: \"{v}\""),
        None => format!("  - {attr}"),
    };
    format!("{file}\npackages:\n{entry}")
}

/// The command that does the same in a terminal.
pub fn command_for(attr: &str, version: Option<&str>, for_system: bool) -> String {
    let what = match version {
        Some(v) => format!("{attr}@{v}"),
        None => attr.to_string(),
    };
    if for_system { format!("nami install --system {what}") } else { format!("nami install {what}") }
}

/// The release lines of an app's versions to pick from, newest first: (what's listed to keep
/// it, the newest version of it). The first is the newest.
pub fn release_lines(list: &[Version], newest: &str) -> Vec<(String, String)> {
    let mut lines: Vec<(String, String)> = Vec::new();
    for v in list {
        let l = line_of(&v.version);
        match lines.iter_mut().find(|(k, _)| *k == l) {
            Some((_, best)) if newer(&v.version, best).is_gt() => *best = v.version.clone(),
            Some(_) => {}
            None => lines.push((l, v.version.clone())),
        }
    }
    if !newest.is_empty() && !lines.iter().any(|(l, _)| *l == line_of(newest)) {
        lines.push((line_of(newest), newest.to_string()));
    }
    lines.sort_by(|a, b| newer(&b.1, &a.1));
    lines
}

/// An app's own page: what it is, its Install (for you or everyone), its versions, the line it
/// is in your file, its facts; then its screenshots and more like it.
pub fn app_page(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    use crate::ui::{filter, filters, grey, main_button, quiet, sheet};
    let Some(view) = g.explore.app.clone() else { return div() };
    let attr = view.attr.clone();
    let f = match (&view.info, &view.first) {
        (Some(Ok(f)), _) => f.clone(),
        (_, Some(f)) => f.clone(),
        _ => serde_json::from_value(json!({ "attr": attr })).expect("a Found needs only its attr"),
    };
    let listed = g.listed(&attr);
    let pinned: Option<String> = listed.clone().flatten();
    let back_to = g.back_to();
    let mut page = div().flex().flex_col().gap(px(22.)).pt(px(14.)).child(
        div().flex().child(crate::ui::link("app-back", format!("‹ {back_to}")).text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).on_click(cx.listener(|this, _, _, cx| {
            this.explore.app = None;
            cx.notify();
        }))),
    );

    // The install card: Install (or Open), and for whom.
    let doing = g.doing(&f);
    let busy = matches!(doing, Some(Doing::Waiting | Doing::Going { .. }));
    let asking = listed.is_some() && g.asking_remove(&attr);
    let open = g.entry_of(&f).filter(|_| listed.is_some() && !busy && !asking);
    let (for_system, may_pick, said) = install_for(g, &f);
    // (Beside its name where there's room; in a narrow window under it, the page's width.)
    let narrow = crate::fit::narrower(640.);
    let mut install = sheet().when(!narrow, |d| d.w(px(300.)).flex_none()).when(narrow, |d| d.w_full()).p(px(16.)).flex().flex_col().gap(px(10.));
    let full = |b: gpui::Stateful<Div>| b.w_full().h(px(38.)).text_size(px(text::BODY));
    match (&listed, g.action(&f)) {
        _ if busy => {
            let (label, turning) = match &doing {
                Some(Doing::Going { label, .. }) => (label.as_str(), true),
                _ => ("Waiting", false),
            };
            install = install.child(busy_pill("app-spin".into(), label, turning).h(px(38.)).justify_center().text_size(px(text::BODY)));
        }
        _ if matches!(doing, Some(Doing::Failed { .. })) => {
            let a = attr.clone();
            install = install.child(full(main_button("app-retry", "Try Again", true)).on_click(cx.listener(move |this, _, _, cx| this.retry(&a, cx))));
            install = install.child(div().text_size(px(text::META)).child(line_of_app(g, "app-failed".into(), &f, true, cx)));
        }
        _ if matches!(doing, Some(Doing::Pending { .. })) => {
            let adding = matches!(doing, Some(Doing::Pending { install: true }));
            let a = attr.clone();
            install = install.child(changing_pill(if adding { "Will install" } else { "Will remove" }.into()).h(px(38.)).justify_center()).child(full(grey("app-undo", "Cancel")).on_click(cx.listener(move |this, _, _, cx| {
                if adding {
                    this.take_off(&a, cx)
                } else {
                    this.put(&a, None, cx)
                }
            })));
        }
        // Remove asks first, where the buttons were.
        _ if asking => {
            let a = attr.clone();
            install = install
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(format!("Remove {}?", f.name())))
                .child(quiet("You can install it again any time, or undo this in History."))
                .child(
                    div().flex().gap(px(8.))
                        .child(grey("app-keep", "Cancel").flex_1().h(px(32.)).on_click(cx.listener(|this, _, _, cx| this.ask_remove(None, cx))))
                        .child(crate::look::destructive_button("app-off", "Remove").flex_1().h(px(32.)).rounded_full().justify_center().text_size(px(text::CALLOUT)).on_click(cx.listener(move |this, _, _, cx| {
                            this.ask_remove(None, cx);
                            this.take_off(&a, cx)
                        }))),
                );
        }
        (Some(_), _) => {
            let a = attr.clone();
            let text = match &pinned {
                Some(v) => format!("{}, kept at version {v}.", g.listed_text(&attr)),
                None => format!("{}.", g.listed_text(&attr)),
            };
            if let Some(entry) = open.clone() {
                install = install.child(full(main_button("app-open", "Open", true)).on_click(cx.listener(move |this, _, _, cx| this.open(&entry, cx))));
            }
            install = install.child(full(grey("app-ask-off", "Remove")).on_click(cx.listener(move |this, _, _, cx| this.ask_remove(Some(&a), cx)))).child(quiet(text).text_size(px(text::META)));
        }
        // One of Homebrew's casks nami can't install on this Mac: why, in place of Install.
        (None, Ok(_)) if f.unavailable.is_some() => install = install.child(quiet(f.unavailable.clone().unwrap_or_default())),
        (None, Ok(label)) => {
            let a = attr.clone();
            let version = g.explore.pick.clone();
            install = install.child(full(main_button("app-add", label, true)).on_click(cx.listener(move |this, _, _, cx| {
                let for_system = install_for(this, &this.explore.app.as_ref().and_then(|v| v.info.clone().and_then(Result::ok).or(v.first.clone())).unwrap_or_else(|| serde_json::from_value(json!({ "attr": a })).expect("a Found needs only its attr"))).0;
                let value = match &version {
                    Some(v) => json!({ a.clone(): v }),
                    None => json!(a),
                };
                if for_system && !this.sets_up_system(&a) {
                    let t = Target { for_system: true, ..Target::file(this, true) };
                    put(this, t, &a, value, cx)
                } else {
                    this.put(&a, version.clone(), cx)
                }
            })));
            // For whom: a switch under it, as a Mac's.
            let off = |b: gpui::Stateful<Div>, on: bool| if on { b } else { b.opacity(0.45).cursor_default() };
            let me = off(filter("app-for-me", "Just me", !for_system, false).flex_1(), may_pick || !for_system).when(may_pick, |b| b.on_click(cx.listener(|this, _, _, cx| {
                this.explore.for_system = false;
                cx.notify();
            })));
            let all = off(filter("app-for-everyone", "System", for_system, false).flex_1(), may_pick || for_system).when(may_pick, |b| b.on_click(cx.listener(|this, _, _, cx| {
                this.explore.for_system = true;
                cx.notify();
            })));
            if !f.flathub() && !crate::look::MAC {
                install = install.child(filters(false).w_full().child(me).child(all));
            }
            install = install.child(div().text_size(px(text::META)).line_height(px(18.)).text_color(rgb(color::muted())).child(said));
        }
        (None, Err(why)) => install = install.child(quiet(why)),
    }

    // Its icon, name, line and facts in a line; the card at its right.
    let licence = if f.unfree { "Not free software" } else { "Free software" };
    let from = if f.flathub() {
        format!("From Flathub{}", f.publisher.as_ref().map(|p| format!(", by {p}")).unwrap_or_default())
    } else if f.cask() {
        "From Homebrew's casks".to_string()
    } else {
        "From nixpkgs".to_string()
    };
    // (A cask doesn't say its licence.)
    let meta = [(!f.version.is_empty()).then(|| format!("Version {}", f.version)), (!f.cask()).then(|| licence.to_string()), Some(from)].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    let head = div().flex().items_start().gap(px(if narrow { 16. } else { 24. }))
        .child(app_icon(&f, if narrow { 72. } else { 104. }))
        .child(
            div().flex_1().min_w_0().pt(px(if narrow { 2. } else { 6. })).flex().flex_col().gap(px(6.))
                .child(div().text_size(px(if narrow { 24. } else { 30. })).line_height(px(if narrow { 29. } else { 36. })).font_weight(FontWeight::BOLD).child(f.name()))
                .child(div().text_size(px(15.)).text_color(rgb(color::text())).opacity(0.8).child(line_of_app(g, "app-line".into(), &f, false, cx)))
                .child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(meta)),
        );
    page = if narrow { page.child(head).child(install) } else { page.child(head.child(install)) };

    // Its version, and the line it is in your file.
    let file = if for_system { g.file(true) } else { g.file(false) }.map(|p| shown_path(&p)).unwrap_or_else(|| "~/user.nami".into());
    let kept = if listed.is_some() { pinned.clone() } else { g.explore.pick.clone() };
    let mut versions = sheet().when(!narrow, |d| d.flex_1()).min_w_0().p(px(18.)).flex().flex_col().gap(px(10.)).child(div().text_size(px(text::SECTION)).font_weight(FontWeight::SEMIBOLD).child("Version"));
    // (A Mac's catalog has each package's newest version alone, so far.)
    let mac_newest_only = crate::look::MAC && match &view.versions {
        Some(Err(_)) => true,
        Some(Ok(list)) => release_lines(list, &f.version).len() <= 1,
        None => false,
    };
    match &view.versions {
        // Homebrew's casks have each app's newest alone.
        _ if f.cask() => {
            let now = if f.version.is_empty() { String::new() } else { format!("Version {}. ", f.version) };
            versions = versions.child(quiet(format!("{now}Homebrew's casks have only the newest version of an app.")));
        }
        Some(_) if mac_newest_only => {
            let now = if f.version.is_empty() { String::new() } else { format!("Version {}. ", f.version) };
            versions = versions.child(quiet(format!("{now}Earlier versions aren't available on a Mac yet.")));
        }
        None => versions = versions.child(quiet("Loading versions…")),
        Some(Err(_)) => versions = versions.child(quiet("Couldn't load the versions. Try again later.")),
        Some(Ok(list)) => {
            let newest = list.iter().find(|v| v.date.is_empty()).map(|v| v.version.clone()).unwrap_or_else(|| f.version.clone());
            let lines = release_lines(list, &newest);
            let shown = if view.all { lines.len() } else { lines.len().min(6) };
            let chips = lines.iter().take(shown).enumerate().map(|(i, (line, best))| {
                let is_newest = i == 0;
                let on = match &kept {
                    Some(k) => *k == *line || *k == *best,
                    None => is_newest,
                };
                let label = if is_newest { format!("{best} · newest") } else { best.clone() };
                let (a, pick) = (attr.clone(), (!is_newest).then(|| line.clone()));
                let installed = listed.is_some();
                div()
                    .id(ElementId::NamedInteger("version".into(), i as u64))
                    .px(px(11.))
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .rounded(px(crate::look::shape::ROW))
                    .font_family("Geist Mono")
                    .text_size(px(text::CALLOUT))
                    .cursor_pointer()
                    .when(on, |d| d.bg(crate::ui::ink(0x17)).font_weight(FontWeight::SEMIBOLD))
                    .when(!on, |d| d.border_1().border_color(crate::ui::ink(0x24)).text_color(rgb(color::muted())).hover(|s| s.bg(color::hover_pill())))
                    .press_row()
                    .child(label)
                    .when(!on, |d| {
                        d.on_click(cx.listener(move |this, _, _, cx| {
                            if installed {
                                this.put(&a, pick.clone(), cx)
                            } else {
                                this.explore.pick = pick.clone();
                                cx.notify();
                            }
                        }))
                    })
            });
            versions = versions.child(div().flex().flex_wrap().gap(px(6.)).children(chips));
            versions = versions.child(quiet("Pick an older version to keep it. It won't be updated past that version."));
            if lines.len() > 6 {
                let all = view.all;
                versions = versions.child(crate::ui::link("all-versions", if all { "Show Fewer".to_string() } else { format!("Show All {} Versions", lines.len()) }).text_size(px(text::CALLOUT)).on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(v) = this.explore.app.as_mut() {
                        v.all = !all;
                    }
                    cx.notify();
                })));
            }
        }
    }
    let mono = |t: String| div().font_family("Geist Mono").text_size(px(text::META)).child(t);
    let in_file = sheet().when(!narrow, |d| d.flex_1()).min_w_0().p(px(18.)).flex().flex_col().gap(px(10.))
        .child(div().text_size(px(text::SECTION)).font_weight(FontWeight::SEMIBOLD).child("In Your File"))
        .child(crate::ui::code_block(file_lines(&file, &attr, kept.as_deref())))
        .child(div().flex().flex_wrap().items_center().gap(px(5.)).text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child("The same as").child(mono(command_for(&attr, kept.as_deref(), for_system && !f.flathub()))).child("in a terminal."));
    page = page.child(div().flex().when(narrow, |d| d.flex_col()).gap(px(16.)).child(versions).child(in_file));

    // Its facts, in a line.
    let fact = |label: &str, value: AnyElement| div().flex().items_center().gap(px(6.)).child(div().font_weight(FontWeight::SEMIBOLD).text_color(rgb(color::text())).child(label.to_string())).child(value);
    let text_el = |t: String| div().child(crate::tip::cut(t)).into_any_element();
    let mut facts = Vec::new();
    if !f.cask() {
        facts.push(fact("Licence", text_el(f.license.clone().unwrap_or_else(|| licence.to_string()))));
    }
    if let Some(url) = f.homepage.clone() {
        let shown = url.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("www.").trim_end_matches('/').to_string();
        facts.push(fact("Website", crate::ui::link("app-site", shown).on_click(move |_, _, cx| cx.open_url(&url)).into_any_element()));
    }
    if f.builds_here {
        facts.push(fact("Built", text_el("On your computer, as it's installed".into())));
    }
    if !f.flathub() {
        facts.push(fact("Package", div().font_family("Geist Mono").child(attr.clone()).into_any_element()));
    }
    page = page.child(div().flex().flex_wrap().gap_x(px(28.)).gap_y(px(8.)).text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).children(facts));
    // (Where it's found after installing it, on a Mac: Applications, in nami Apps.)
    if crate::look::MAC {
        page = page.child(crate::look::footnote(if f.cask() { super::discover::MAC_CASKS } else { super::MAC_APPS }));
    }

    // Screenshots: one big, the others small to pick.
    let shots: Vec<String> = match &view.info {
        Some(Ok(f)) => f.screenshots.clone(),
        _ => Vec::new(),
    };
    let shot = view.shot.min(shots.len().saturating_sub(1));
    if !shots.is_empty() {
        let big = img(std::path::PathBuf::from(&shots[shot])).size_full().object_fit(ObjectFit::Contain);
        let n = shots.len();
        // (Less tall in a narrow window: the picture keeps its shape.)
        let shot_h = (crate::fit::page() * 0.62).clamp(220., 380.);
        let arrow = |id: &'static str, glyph: &'static str, to: usize| {
            div().id(id).absolute().top(px(shot_h / 2. - 18.)).size(px(36.)).rounded_full().bg(rgb(color::panel())).border_1().border_color(color::hairline()).flex().items_center().justify_center().text_size(px(18.)).cursor_pointer().press_row().hover(|s| s.bg(color::hover_pill())).child(glyph).on_click(cx.listener(move |this, _, _, cx| {
                if let Some(v) = this.explore.app.as_mut() {
                    v.shot = to;
                }
                cx.notify();
            }))
        };
        let mut frame = div().relative().h(px(shot_h)).w_full().overflow_hidden().rounded(px(crate::look::shape::CARD)).bg(color::push()).p(px(10.)).child(big);
        if n > 1 {
            frame = frame.child(arrow("shot-back", "‹", (shot + n - 1) % n).left(px(12.))).child(arrow("shot-next", "›", (shot + 1) % n).right(px(12.)));
        }
        let mut gallery = div().flex().flex_col().gap(px(8.)).child(frame);
        if shots.len() > 1 {
            gallery = gallery.child(div().flex().flex_wrap().gap(px(8.)).children(shots.iter().enumerate().map(|(i, s)| {
                div().id(ElementId::NamedInteger("shot".into(), i as u64)).w(px(120.)).h(px(72.)).overflow_hidden().rounded(px(6.)).bg(color::push()).p(px(3.)).border_2()
                    .border_color(if i == shot { crate::ui::ink(0x60) } else { crate::ui::ink(0x00) })
                    .cursor_pointer()
                    .press()
                    .child(img(std::path::PathBuf::from(s)).size_full().object_fit(ObjectFit::Contain))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(v) = this.explore.app.as_mut() {
                            v.shot = i;
                        }
                        cx.notify();
                    }))
            })));
        }
        page = page.child(gallery);
    }
    if let Some(about) = f.about.clone().or(f.description.clone().filter(|d| Some(d) != f.summary.as_ref())) {
        page = page.child(div().text_size(px(text::BODY)).line_height(px(22.)).max_w(px(680.)).child(about));
    }

    // More of its kind, the most used first.
    if !view.similar.is_empty() {
        let tiles: Vec<_> = view.similar.iter().enumerate().map(|(n, f)| card(g, n, f, cx)).collect();
        page = page.child(div().flex().flex_col().gap(px(12.)).child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child("More Like This")).child(crate::fit::grid(crate::fit::APP_TILE, 12., 4).children(tiles)));
    }
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(version: &str, date: &str) -> Version {
        serde_json::from_value(json!({ "version": version, "date": date })).unwrap()
    }

    #[test]
    fn release_lines_newest_first_each_its_newest() {
        let list = [v("3.0.6", ""), v("3.0.4", "2026-08-01"), v("2.10.38", "2025-01-01"), v("2.10.36", "2024-06-01"), v("3.0.2", "2026-05-01")];
        let lines = release_lines(&list, "3.0.6");
        assert_eq!(lines, vec![("3.0".to_string(), "3.0.6".to_string()), ("2.10".to_string(), "2.10.38".to_string())]);
    }

    #[test]
    fn release_lines_have_the_newest_even_when_unlisted() {
        let lines = release_lines(&[v("1.2.0", "2025-01-01")], "1.3.1");
        assert_eq!(lines[0], ("1.3".to_string(), "1.3.1".to_string()));
    }

    #[test]
    fn the_file_and_the_command_say_the_same() {
        assert_eq!(file_lines("~/user.nami", "gimp", None), "~/user.nami\npackages:\n  - gimp");
        assert_eq!(file_lines("/etc/nami/system.nami", "gimp", Some("2.10")), "/etc/nami/system.nami\npackages:\n  - gimp: \"2.10\"");
        assert_eq!(command_for("gimp", None, false), "nami install gimp");
        assert_eq!(command_for("gimp", Some("2.10"), true), "nami install --system gimp@2.10");
    }
}
