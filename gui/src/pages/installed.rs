//! Installed: the updates waiting, with Update All; then everything installed, yours and
//! everyone's, apps and command-line tools, each with where it's from (Flathub, a download,
//! kept at a version) and who has it. A row opens the app's page, to remove it or keep it at
//! another version.

use std::path::{Path, PathBuf};

use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, ElementId, Entity, FontWeight, ParentElement, Styled, div, rgb};
use crate::look::px;
use crate::look::text;
use serde_json::Value;

use super::explore::{self, small_action, Explorer as _};
use super::{entry_name, is_folder, packages};
use crate::input::TextInput;
use crate::ui::{color, filter, filters, grey, icon, ink, letters, main_button, page_title, quiet, sheet, sheet_row, tag};
use crate::Shell;

/// How nami updates itself on a Mac: as root, in Terminal (there's no admin through the daemon).
/// A Mac's nami, updated from here: macOS's own box asks for an administrator's name and
/// password (as Uninstall nami… does), then `nami self-update` runs as root. A cancelled box
/// leaves it as it was, quietly.
#[derive(Default, Clone, PartialEq, Debug)]
pub enum NamiUpdate {
    #[default]
    Idle,
    Working,
    /// Updated: the app that's open is the one from before until it's opened again.
    Done,
    Failed(String),
}

/// What updates a Mac's nami: osascript, with macOS's box for an administrator's password.
pub fn mac_update_command(nami: &str) -> (String, Vec<String>) {
    let run = format!("{nami} self-update");
    let script = format!("do shell script \"{}\" with prompt \"nami wants to update nami.\" with administrator privileges", run.replace('\\', "\\\\").replace('"', "\\\""));
    ("/usr/bin/osascript".into(), vec!["-e".into(), script])
}

fn update_mac_nami(g: &mut Shell, cx: &mut Context<Shell>) {
    if g.nami_update == NamiUpdate::Working {
        return;
    }
    g.nami_update = NamiUpdate::Working;
    cx.notify();
    let (program, args) = mac_update_command(&crate::pages::uninstall_nami::nami_path(true));
    Shell::later(cx, move || std::process::Command::new(&program).args(&args).stdin(std::process::Stdio::null()).output(), move |g, out, cx| {
        g.nami_update = match out {
            Ok(o) if o.status.success() => NamiUpdate::Done,
            Ok(o) => {
                let said = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
                if crate::pages::uninstall_nami::cancelled(true, o.status.code(), &said) { NamiUpdate::Idle } else { NamiUpdate::Failed(said.trim().to_string()) }
            }
            Err(e) => NamiUpdate::Failed(format!("Couldn't ask for an administrator's password (osascript): {e}")),
        };
        if g.nami_update == NamiUpdate::Done {
            // What's waiting now, the new nami's own answer.
            g.updates = Default::default();
            crate::pages::updates::look(cx);
        }
        cx.notify();
    });
}


#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Show {
    All,
    Apps,
    Tools,
}

pub struct Installed {
    /// The words the list is filtered by.
    pub filter: Entity<TextInput>,
    pub show: Show,
}

impl Installed {
    pub fn new(cx: &mut Context<Shell>) -> Self {
        let filter = cx.new(|cx| {
            let mut t = TextInput::new(cx, "Filter", "", false);
            t.bare = true;
            t
        });
        cx.observe(&filter, |_, _, cx| cx.notify()).detach();
        Installed { filter, show: Show::All }
    }
}

/// Who has an installed app.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Whose {
    You,
    System,
}

impl Whose {
    pub fn said(self) -> &'static str {
        match self {
            Whose::You => "Just you",
            Whose::System => "System",
        }
    }
}

/// An installed app or tool, as its row says it.
#[derive(Clone, Debug)]
pub struct Item {
    pub attr: String,
    pub whose: Whose,
    /// The version it's kept at, if it's kept at one.
    pub kept: Option<String>,
}

/// The version a `packages:` entry keeps its app at: `python3: "3.12"`, or
/// `python3: { version: "3.12" }`.
pub fn kept_at(entry: &Value) -> Option<String> {
    let v = entry.as_object()?.values().next()?;
    v.as_str().map(String::from).or_else(|| v.get("version").and_then(Value::as_str).map(String::from))
}

/// Where an installed app's from, said after its version: Flathub, kept at a version, or
/// nothing (the catalog's, kept up to date).
pub fn source_of(item: &Item) -> Option<String> {
    if item.attr.starts_with("flathub:") {
        return Some("from Flathub".into());
    }
    if item.attr.starts_with("cask:") {
        return Some("from Homebrew's casks".into());
    }
    item.kept.as_ref().map(|v| format!("kept at {v}"))
}

/// What your file and the computer's list (not a computer's own package folders, nor the
/// libraries an app from a download needed: they go with it), yours first, each once.
pub fn items(user: &Value, system: &Value, computer_dir: Option<&Path>, for_downloads: &[String]) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::new();
    for e in packages(user) {
        let attr = entry_name(&e);
        if attr.is_empty() || for_downloads.contains(&attr) || out.iter().any(|i| i.attr == attr) {
            continue;
        }
        out.push(Item { attr, whose: Whose::You, kept: kept_at(&e) });
    }
    for e in packages(system) {
        let attr = entry_name(&e);
        if attr.is_empty() || is_folder(computer_dir, &e) || out.iter().any(|i| i.attr == attr) {
            continue;
        }
        out.push(Item { attr, whose: Whose::System, kept: kept_at(&e) });
    }
    out
}

/// Whether `words` find an installed app by its name or its package's.
pub fn matches(words: &str, name: &str, attr: &str) -> bool {
    let w = words.trim().to_lowercase();
    w.is_empty() || name.to_lowercase().contains(&w) || attr.to_lowercase().contains(&w)
}

/// The apps from downloads with a newer version waiting.
fn downloads_waiting(g: &Shell) -> Vec<(crate::pages::install_file::FileApp, String)> {
    g.install_file.apps.iter().filter_map(|a| {
        let newest = g.app_updates.known.get(&a.id)?.newest.clone()?;
        (newest != a.version).then(|| (a.clone(), newest))
    }).collect()
}

/// How many updates are waiting, for the sidebar: the apps', nami's, the downloads'.
pub fn waiting(g: &Shell) -> usize {
    let apps = g.updates.apps.as_ref().and_then(|a| a.as_ref().ok()).map_or(0, Vec::len);
    let nami = g.updates.nami.as_ref().and_then(|t| t.as_ref().ok()).is_some_and(|t| t.newer) as usize;
    apps + nami + downloads_waiting(g).len()
}

/// An app's icon, or for a command-line tool without one, its first letters.
pub(crate) fn mark(g: &Shell, attr: &str, size: f32) -> AnyElement {
    let f = g.catalog.get(attr);
    let path = crate::ui::catalog_icon(attr).or_else(|| f.and_then(|f| f.icon.clone()));
    let tool = f.is_some_and(|f| !f.app) && !g.launchers.contains_key(attr);
    if path.is_none() && tool && crate::icons::dev_mark(attr).is_none() {
        let short: String = attr.chars().filter(|c| c.is_alphanumeric()).take(2).collect();
        return letters(&short, size).into_any_element();
    }
    icon(path.as_deref(), attr, size)
}

/// Who an app's for, as the updates and the list say it.
fn whose_of(g: &Shell, attr: &str) -> Whose {
    if packages(&g.user).iter().any(|e| entry_name(e) == attr) { Whose::You } else { Whose::System }
}

/// The updates waiting, in a box: how many, Update All, and each one's versions.
fn updates_box(g: &Shell, cx: &mut Context<Shell>) -> Div {
    let busy = g.busy();
    let changed = !g.before.is_empty();
    let apps: Vec<crate::nami::Update> = g.updates.apps.as_ref().and_then(|a| a.as_ref().ok()).cloned().unwrap_or_default();
    let nami_new = g.updates.nami.as_ref().and_then(|t| t.as_ref().ok()).filter(|t| t.newer).cloned();
    let downloads = downloads_waiting(g);
    let n = apps.len() + nami_new.is_some() as usize + downloads.len();
    let looking = g.updates.apps.is_none();
    let failed = g.updates.apps.as_ref().and_then(|a| a.as_ref().err()).cloned();
    let (title, line) = match (n, looking, &failed) {
        (_, true, _) => ("Checking for updates…".to_string(), "This takes a moment.".to_string()),
        (_, _, Some(_)) => ("Couldn't check for updates".to_string(), "Check your connection, then try again.".to_string()),
        (0, _, _) => ("Everything is up to date".to_string(), match g.updates.catalog.as_ref().filter(|d| !d.starts_with("1970")) {
            Some(d) => format!("The catalog is from {}.", d.split(' ').next().unwrap_or(d)),
            None => "Updates are checked every day.".to_string(),
        }),
        (1, ..) => ("1 update".to_string(), "You can undo an update in History.".to_string()),
        (n, ..) => (format!("{n} updates"), "You can undo an update in History.".to_string()),
    };
    let can = !busy && !changed;
    // (A Mac's nami updates itself with sudo alone, in Terminal: only nami waiting, its command
    // to copy.)
    let mac = crate::look::MAC;
    let updating = g.nami_update == NamiUpdate::Working;
    let action: AnyElement = if mac && nami_new.is_some() && apps.is_empty() && downloads.is_empty() {
        main_button("update-nami-all", if updating { "Updating…" } else { "Update" }, can && !updating)
            .when(can && !updating, |b| b.on_click(cx.listener(|g, _, _, cx| update_mac_nami(g, cx))))
            .into_any_element()
    } else if n > 0 {
        main_button("update-all", "Update All", can)
            .when(can, |b| {
                b.on_click(cx.listener(move |g, _, _, cx| {
                    let mut cmds = Vec::new();
                    if g.updates.apps.as_ref().and_then(|a| a.as_ref().ok()).is_some_and(|a| !a.is_empty()) {
                        cmds.push((vec!["upgrade".into()], None));
                    }
                    if !crate::look::MAC && g.updates.nami.as_ref().and_then(|t| t.as_ref().ok()).is_some_and(|t| t.newer) {
                        cmds.push((vec!["self-update".into()], None));
                    }
                    if !cmds.is_empty() {
                        g.start("Updating your apps", cmds, cx);
                    }
                    for (a, _) in downloads_waiting(g) {
                        crate::pages::app_updates::update_now(g, &a.id, cx);
                    }
                }))
            })
            .into_any_element()
    } else {
        grey("check-updates", "Check for Updates")
            .when(!busy && !looking, |b| b.on_click(cx.listener(|g, _, _, cx| g.start("Getting the newest catalog", vec![(vec!["update".into(), "--refresh".into()], None)], cx))))
            .when(busy || looking, |b| b.opacity(0.45))
            .into_any_element()
    };
    let mut rows: Vec<Div> = Vec::new();
    let version = |from: &str, to: &str| div().text_size(px(text::META)).font_family("Geist Mono").text_color(rgb(color::muted())).child(format!("{from} → {to}"));
    if let Some(t) = &nami_new {
        let b = grey("update-nami", "Update").when(can, |b| b.on_click(cx.listener(|g, _, _, cx| g.start("Updating nami", vec![(vec!["self-update".into()], None)], cx))));
        // (A Mac's: macOS's box asks for an administrator, then it updates.)
        let b: AnyElement = if mac {
            grey("update-nami", if updating { "Updating…" } else { "Update" })
                .when(can && !updating, |b| b.on_click(cx.listener(|g, _, _, cx| update_mac_nami(g, cx))))
                .when(!can || updating, |b| b.opacity(0.45))
                .into_any_element()
        } else {
            b.when(!can, |b| b.opacity(0.45)).into_any_element()
        };
        rows.push(
            div().flex().items_center().gap(px(14.)).w_full()
                .child(crate::icons::glyph("mark", 30., color::text()))
                .child(div().flex_1().min_w_0().child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child("nami")).child(version(&t.current, &t.latest)))
                .child(tag("System"))
                .child(b),
        );
    }
    for u in &apps {
        let whose = whose_of(g, &u.attr);
        let name = g.app_name(&u.attr);
        let tool = g.catalog.get(&u.attr).is_some_and(|f| !f.app);
        rows.push(
            div().flex().items_center().gap(px(14.)).w_full()
                .child(mark(g, &u.attr, 36.))
                .child(div().flex_1().min_w_0().child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).when(tool, |d| d.font_family("Geist Mono")).truncate().child(name)).child(version(&u.from, &u.to)))
                .child(tag(whose.said())),
        );
    }
    for (n, (a, newest)) in downloads.iter().enumerate() {
        rows.push(
            div().flex().items_center().gap(px(14.)).w_full()
                .child(icon(a.icon.as_deref().map(Path::new), &a.name, 36.))
                .child(div().flex_1().min_w_0().child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(a.name.clone())).child(version(&a.version, newest)))
                .child(tag("Just you"))
                .child(crate::pages::app_updates::button(g, ElementId::NamedInteger("dl-update".into(), n as u64), a, cx)),
        );
    }
    let head = sheet_row(true).px(px(18.)).py(px(14.))
        .child(div().flex_1().min_w_0().child(div().text_size(px(text::SECTION)).font_weight(FontWeight::SEMIBOLD).child(title)).child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(line)))
        .when(changed && n > 0, |d| d.child(quiet("Apply or discard your other changes first.")))
        .child(action);
    let mut out = sheet().flex().flex_col().child(head);
    match &g.nami_update {
        NamiUpdate::Done => out = out.child(sheet_row(false).child(quiet("nami is updated. Quit nami and open it again to use its new version."))),
        NamiUpdate::Failed(why) => {
            out = out.child(sheet_row(false).child(super::failed("update-nami-again", "Couldn't update nami.", why.clone(), |g: &mut Shell, cx| update_mac_nami(g, cx), cx)))
        }
        _ => {}
    }
    if let Some(why) = failed {
        out = out.child(sheet_row(false).child(super::failed("updates-again", "Couldn't check for updates.", why, |g: &mut Shell, cx| {
            g.updates = Default::default();
            crate::pages::updates::look(cx);
        }, cx)));
    }
    out.children(rows.into_iter().map(|r| sheet_row(false).py(px(11.)).child(r)))
}

pub fn render(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    if g.explore.app.is_some() {
        return explore::app_page(g, cx);
    }
    let field = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .w(px(240.))
        .h(px(34.))
        .px(px(12.))
        .rounded(px(9.))
        .bg(rgb(color::panel()))
        .border_1()
        .border_color(ink(0x24))
        .child(crate::icons::glyph("search", 14., color::muted()))
        .child(div().flex_1().min_w_0().child(g.installed.filter.clone()));
    let mut page = div().flex().flex_col().gap(px(20.)).pt(px(20.)).child(div().flex().items_center().gap(px(12.)).child(page_title("Installed", None)).child(field)).child(updates_box(g, cx)).children(super::storage::summary(g, cx));

    // Everything installed.
    let dir = g.file(true).and_then(|f| f.parent().map(Path::to_path_buf));
    let for_downloads: Vec<String> = g.install_file.apps.iter().flat_map(|a| a.libraries.clone()).collect();
    let words = g.text(&g.installed.filter, cx);
    let show = g.installed.show;
    let all = items(&g.user, &g.system, dir.as_deref(), &for_downloads);
    let is_tool = |g: &Shell, attr: &str| g.catalog.get(attr).is_some_and(|f| !f.app) && !g.launchers.contains_key(attr);
    let shown: Vec<Item> = all.into_iter().filter(|i| matches(&words, &g.app_name(&i.attr), &i.attr)).filter(|i| match show {
        Show::All => true,
        Show::Apps => !is_tool(g, &i.attr),
        Show::Tools => is_tool(g, &i.attr),
    }).collect();
    let downloads: Vec<crate::pages::install_file::FileApp> = g.install_file.apps.iter().filter(|a| show != Show::Tools && matches(&words, &a.name, &a.id)).cloned().collect();
    let picks = [(Show::All, "All"), (Show::Apps, "Apps"), (Show::Tools, "Tools")].map(|(s, label)| {
        filter(ElementId::Name(format!("show-{label}").into()), label, show == s, true).on_click(cx.listener(move |g, _, _, cx| {
            g.installed.show = s;
            cx.notify();
        }))
    });
    page = page.child(
        div().flex().items_center().gap(px(8.)).mt(px(4.))
            .child(div().flex_1().text_size(px(text::SECTION)).font_weight(FontWeight::SEMIBOLD).child("All Installed"))
            .child(filters(true).children(picks)),
    );
    let mut rows: Vec<AnyElement> = Vec::new();
    for (n, item) in shown.iter().enumerate() {
        let f = g.catalog.get(&item.attr).cloned();
        let name = g.app_name(&item.attr);
        let tool = is_tool(g, &item.attr);
        let version = item.kept.clone().or_else(|| f.as_ref().map(|f| f.version.clone()).filter(|v| !v.is_empty()));
        let open = g.launchers.get(&item.attr).cloned();
        let doing = f.as_ref().and_then(|f| g.doing(f));
        let id = |what: &str| ElementId::NamedInteger(format!("inst-{what}").into(), n as u64);
        let action: AnyElement = match (doing.as_ref().zip(f.as_ref()).and_then(|(d, f)| explore::doing_button(id("doing"), d, &f.attr, cx)), open) {
            (Some(b), _) => b,
            (None, Some(entry)) => small_action(id("open"), "Open".into())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.problem = crate::launch::open_app(&entry).err().map(crate::Problem::under("Couldn't open the app."));
                    cx.notify();
                }))
                .into_any_element(),
            (None, None) => crate::ui::round_button(id("more"), "chevron", format!("More for {name}")).size(px(28.)).into_any_element(),
        };
        let (found, attr) = (f.clone(), item.attr.clone());
        rows.push(
            sheet_row(n == 0)
                .id(id("row"))
                .cursor_pointer()
                .hover(|s| s.bg(color::hover_pill()))
                .child(mark(g, &item.attr, 32.))
                .child(
                    div().flex_1().min_w_0().flex().items_baseline().gap(px(8.))
                        .child(div().min_w_0().truncate().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).when(tool, |d| d.font_family("Geist Mono")).child(name))
                        .children(version.map(|v| div().flex_none().text_size(px(text::META)).font_family("Geist Mono").text_color(rgb(color::muted())).child(v)))
                        .children(source_of(item).map(|s| div().flex_none().text_size(px(text::META)).text_color(rgb(color::muted())).child(s))),
                )
                .child(tag(item.whose.said()))
                .child(action)
                .on_click(cx.listener(move |this, _, _, cx| explore::open(this, found.clone(), attr.clone(), cx)))
                .into_any_element(),
        );
    }
    let start = rows.len();
    for (n, a) in downloads.iter().enumerate() {
        let id = |what: &str| ElementId::NamedInteger(format!("dl-{what}").into(), n as u64);
        let entry = a.main_entry.clone().map(PathBuf::from).filter(|p| p.exists());
        let auto = crate::pages::app_updates::automatic(g, &a.id);
        let a_id = a.id.clone();
        let action: AnyElement = if g.install_file.removing.as_deref() == Some(a.id.as_str()) {
            explore::busy_pill(id("spin"), "Removing", true).into_any_element()
        } else if g.install_file.asking.as_deref() == Some(a.id.as_str()) {
            div().flex().flex_none().items_center().gap(px(8.))
                .child(div().text_size(px(text::CALLOUT)).font_weight(FontWeight::MEDIUM).child(format!("Remove {}?", a.name)))
                .child(small_action(id("keep"), "Cancel".into()).on_click(cx.listener(|this, _, _, cx| {
                    this.install_file.asking = None;
                    cx.notify();
                })))
                .child(small_action(id("off"), "Remove".into()).bg(rgb(color::bad())).text_color(rgb(color::on_bad())).on_click(cx.listener(move |this, _, _, cx| crate::pages::install_file::remove(this, &a_id, cx))))
                .into_any_element()
        } else {
            let ask = a.id.clone();
            div().flex().flex_none().items_center().gap(px(6.))
                .child(small_action(id("ask-off"), "Remove".into()).on_click(cx.listener(move |this, _, _, cx| {
                    this.install_file.asking = Some(ask.clone());
                    cx.notify();
                })))
                .children(entry.map(|e| {
                    small_action(id("open"), "Open".into()).on_click(cx.listener(move |this, _, _, cx| {
                        this.problem = crate::launch::open_app(&e).err().map(crate::Problem::under("Couldn't open the app."));
                        cx.notify();
                    }))
                }))
                .into_any_element()
        };
        let source = if auto { "from a download, kept up to date" } else { "from a download" };
        let short = match a.format.as_str() {
            "deb" => ".deb",
            "rpm" => ".rpm",
            "appimage" => "App",
            _ => "",
        };
        let picture = if a.icon.is_some() || short.is_empty() { icon(a.icon.as_deref().map(Path::new), &a.name, 32.) } else { letters(short, 32.).into_any_element() };
        rows.push(
            sheet_row(start + n == 0)
                .child(picture)
                .child(
                    div().flex_1().min_w_0().flex().items_baseline().gap(px(8.))
                        .child(div().min_w_0().truncate().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(a.name.clone()))
                        .when(!a.version.is_empty(), |d| d.child(div().flex_none().text_size(px(text::META)).font_family("Geist Mono").text_color(rgb(color::muted())).child(a.version.clone())))
                        .child(div().flex_none().text_size(px(text::META)).text_color(rgb(color::muted())).child(source)),
                )
                .child(tag("Just you"))
                .child(action)
                .into_any_element(),
        );
    }
    if rows.is_empty() {
        let said = if !words.is_empty() {
            format!("Nothing installed matches “{words}”.")
        } else if show == Show::Tools {
            "No command-line tools are installed. Find them in Discover.".to_string()
        } else {
            "Nothing is installed yet. Find apps and tools in Discover.".to_string()
        };
        page = page.child(quiet(said));
    } else {
        page = page.child(sheet().flex().flex_col().children(rows));
        // What the tag means, once.
        if shown.iter().any(|i| i.whose == Whose::System) {
            let more = super::apps::everyone_here().map(|s| format!(" {s}")).unwrap_or_default();
            page = page.child(quiet(format!("System: for all people on this computer. Changing it needs an administrator's password.{more}")));
        }
    }
    // Under everything installed, nami itself: Uninstall nami… (not on namiOS).
    if words.is_empty() {
        page = page.children(super::uninstall_nami::row(g, cx));
    }
    page
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn yours_first_then_everyones_each_once() {
        let user = json!({ "packages": ["krita", { "python3": "3.12" }, "flathub:md.obsidian.Obsidian", "libfoo"] });
        let system = json!({ "packages": ["steam", "krita", { "firefox": { "version": "150" } }] });
        let items = items(&user, &system, None, &["libfoo".to_string()]);
        let said: Vec<(&str, Whose, Option<&str>)> = items.iter().map(|i| (i.attr.as_str(), i.whose, i.kept.as_deref())).collect();
        assert_eq!(said, vec![
            ("krita", Whose::You, None),
            ("python3", Whose::You, Some("3.12")),
            ("flathub:md.obsidian.Obsidian", Whose::You, None),
            ("steam", Whose::System, None),
            ("firefox", Whose::System, Some("150")),
        ]);
    }

    #[test]
    fn where_its_from_is_said() {
        let item = |attr: &str, kept: Option<&str>| Item { attr: attr.into(), whose: Whose::You, kept: kept.map(String::from) };
        assert_eq!(source_of(&item("flathub:org.x", None)).as_deref(), Some("from Flathub"));
        assert_eq!(source_of(&item("cask:rectangle", None)).as_deref(), Some("from Homebrew's casks"));
        assert_eq!(source_of(&item("python3", Some("3.12"))).as_deref(), Some("kept at 3.12"));
        assert_eq!(source_of(&item("gimp", None)), None);
    }

    #[test]
    fn the_filter_finds_by_name_or_package() {
        assert!(matches("", "GIMP", "gimp"));
        assert!(matches("gi", "GIMP", "gimp"));
        assert!(matches("code", "VS Code", "vscode"));
        assert!(!matches("zed", "GIMP", "gimp"));
    }
}

#[cfg(test)]
mod nami_update_tests {
    #[test]
    fn a_macs_nami_updates_through_macos_box() {
        let (p, args) = super::mac_update_command("/nix/var/nami/bin/nami");
        assert_eq!(p, "/usr/bin/osascript");
        assert_eq!(args, vec!["-e".to_string(), "do shell script \"/nix/var/nami/bin/nami self-update\" with prompt \"nami wants to update nami.\" with administrator privileges".to_string()]);
    }
}
