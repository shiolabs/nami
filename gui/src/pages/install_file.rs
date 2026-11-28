//! An app from a download, to install: a .deb, an .rpm, an AppImage or a
//! tarball opened in Files (or from the browser's downloads) shows here,
//! as an app of the catalog does: its icon, name, maker, version and size,
//! what won't work, and Install. nami does the work (`nami package-info`,
//! `nami install <file>`, `nami remove <app>`); a notification says when
//! it's in, with Open. Those installed are listed in Your Apps, to open or
//! remove.

use crate::look::Type as _;
use std::path::{Path, PathBuf};

use futures::StreamExt;
use gpui::prelude::*;
use crate::look::Press;
use gpui::{AnyElement, Context, Div, ElementId, FontWeight, ParentElement, Styled, div, rgb};
use serde::Deserialize;

use crate::look::px;
use crate::look::text;
use crate::nami::{self, Said};
use crate::ui::{button, color, icon, pill};
use crate::{Page, Shell};

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Warning {
    pub title: String,
    pub text: String,
    /// Said only until it's installed (that it's a program from the internet).
    #[serde(default)]
    pub until_installed: bool,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Source {
    pub url: String,
}

/// What `nami package-info --json` says of a download.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Info {
    pub file: String,
    pub format: Option<String>,
    pub made_for: String,
    pub id: String,
    pub name: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub version: String,
    pub publisher: Option<String>,
    pub homepage: Option<String>,
    pub size: u64,
    pub download: u64,
    pub icon: Option<String>,
    pub app: bool,
    pub commands: Vec<String>,
    pub schemes: Vec<String>,
    pub warnings: Vec<Warning>,
    pub notes: Vec<String>,
    pub blocked: Option<String>,
    pub updates: Option<Source>,
    pub installed: Option<String>,
    /// A nami bundle's own (install_bundle.rs).
    pub bundle: Option<crate::pages::install_bundle::Extra>,
}

/// An app installed from a download (its app.json).
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct FileApp {
    pub id: String,
    pub name: String,
    pub version: String,
    pub format: String,
    pub publisher: Option<String>,
    pub summary: Option<String>,
    pub icon: Option<String>,
    pub main_entry: Option<String>,
    pub commands: Vec<String>,
    /// Packages added to the person's file for the libraries it needs.
    pub libraries: Vec<String>,
    /// A nami bundle's: where it is; and, read from it, its version and updates now.
    pub bundle: Option<crate::pages::install_bundle::Kept>,
    #[serde(skip)]
    pub live: Option<crate::pages::install_bundle::Live>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum State {
    #[default]
    Idle,
    /// Installing: what nami says it's doing.
    Installing(String),
    Installed,
    /// What went wrong.
    Failed(String),
    Removing,
}

#[derive(Default)]
pub struct InstallFile {
    pub path: Option<PathBuf>,
    /// None while it's looked at.
    pub info: Option<Result<Info, String>>,
    pub state: State,
    pub asking_remove: bool,
    /// Installed from downloads, for Your Apps; and which one is asking
    /// "Remove?", or being removed.
    pub apps: Vec<FileApp>,
    pub asking: Option<String>,
    pub removing: Option<String>,
    /// Where to go back to.
    pub back: Option<Page>,
    /// Install it once it's looked at (asked from outside: `install-file:`).
    pub install_when_known: bool,
}

impl InstallFile {
    /// Whether it's changing something (Settings stays until it's done).
    pub fn busy(&self) -> bool {
        matches!(self.state, State::Installing(_) | State::Removing) || self.removing.is_some()
    }
}

fn apps_dir() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    data.join("nami/apps")
}

/// The apps installed from downloads, by name.
pub fn installed() -> Vec<FileApp> {
    let mut out: Vec<FileApp> = std::fs::read_dir(apps_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| serde_json::from_str(&std::fs::read_to_string(e.path().join("app.json")).ok()?).ok())
        .map(|mut a: FileApp| {
            a.live = a.bundle.as_ref().and_then(crate::pages::install_bundle::live);
            // (A bundle updates itself: its version is the file's.)
            if let Some(v) = a.live.as_ref().map(|l| l.version.clone()).filter(|v| !v.is_empty()) {
                a.version = v;
            }
            a
        })
        .collect();
    out.sort_by_key(|a: &FileApp| a.name.to_lowercase());
    out
}

/// Bytes as people say them.
fn size_words(n: u64) -> String {
    let mb = n as f64 / 1e6;
    match mb {
        m if m >= 1000. => format!("{:.1} GB", m / 1000.),
        m if m >= 10. => format!("{} MB", m.round()),
        m if m >= 1. => format!("{m:.1} MB"),
        _ => format!("{} kB", (n as f64 / 1e3).round().max(1.)),
    }
}

/// Show `path` and install it, as Install would (asked from the desktop).
pub fn open_and_install(g: &mut Shell, path: PathBuf, cx: &mut Context<Shell>) {
    open(g, path, cx);
    let f = &mut g.install_file;
    match &f.info {
        Some(Ok(i)) if !f.busy() && i.blocked.is_none() => install(g, cx),
        None => f.install_when_known = true,
        _ => {}
    }
}

/// Show `path` to install: looked at in the background.
pub fn open(g: &mut Shell, path: PathBuf, cx: &mut Context<Shell>) {
    if g.page != Page::InstallFile {
        g.install_file.back = Some(g.page);
    }
    g.page = Page::InstallFile;
    let f = &mut g.install_file;
    // (The same one, being installed: as it is.)
    if f.path.as_ref() == Some(&path) && f.busy() {
        cx.notify();
        return;
    }
    f.path = Some(path.clone());
    f.info = None;
    f.state = State::Idle;
    f.asking_remove = false;
    look(path, cx);
    cx.notify();
}

fn look(path: PathBuf, cx: &mut Context<Shell>) {
    cx.spawn(async move |this, cx| {
        let p = path.clone();
        let info = cx.background_executor().spawn(async move { package_info(&p) }).await;
        let _ = this.update(cx, |g, cx| {
            if g.install_file.path.as_ref() == Some(&path) {
                let ready = info.as_ref().is_ok_and(|i| i.blocked.is_none() && i.installed.as_deref() != Some(i.version.as_str()));
                g.install_file.info = Some(info);
                if std::mem::take(&mut g.install_file.install_when_known) && ready {
                    install(g, cx);
                }
                cx.notify();
            }
        });
    })
    .detach();
}

fn package_info(path: &Path) -> Result<Info, String> {
    let out = std::process::Command::new(std::env::var_os("NAMI").unwrap_or_else(|| "nami".into())).arg("package-info").arg(path).arg("--json").output().map_err(|e| format!("can't run nami: {e}"))?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr).trim().trim_start_matches("error: ").to_string();
        return Err(said);
    }
    serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())
}

/// Back where it was opened from (its ‹ pill, and Esc).
pub fn go_back(g: &mut Shell) {
    use crate::os::{Active, Os};
    let back = g.install_file.back.take();
    g.page = back.filter(|p| match p {
        Page::Projects | Page::Updates => true,
        // (The system's, where it's said by name: namiOS's Settings, at its category.)
        Page::Os(page) => Active::back_label(g, *page).is_some(),
        _ => false,
    }).unwrap_or(Page::Discover);
    reload(g);
}

/// The apps installed from downloads, read again.
pub fn reload(g: &mut Shell) {
    g.install_file.apps = installed();
    super::app_updates::reload(g);
}

fn install(g: &mut Shell, cx: &mut Context<Shell>) {
    let Some(path) = g.install_file.path.clone() else { return };
    let info = g.install_file.info.clone().and_then(Result::ok).unwrap_or_default();
    g.install_file.state = State::Installing(String::new());
    cx.notify();
    cx.spawn(async move |this, cx| {
        let mut rx = nami::run(vec!["install".into(), path.display().to_string()], None);
        while let Some(said) = rx.next().await {
            let gone = this
                .update(cx, |g, cx| {
                    match said {
                        Said::Line(l) => {
                            if let State::Installing(now) = &mut g.install_file.state {
                                if !l.trim().is_empty() {
                                    *now = l.trim().to_string();
                                }
                            }
                        }
                        Said::Finished(Ok(())) => {
                            g.install_file.state = State::Installed;
                            reload(g);
                            let app = g.install_file.apps.iter().find(|a| a.id == info.id).cloned();
                            tell_installed(&info, app.as_ref());
                            if let Some(i) = g.install_file.info.as_mut().and_then(|i| i.as_mut().ok()) {
                                i.installed = Some(i.version.clone());
                            }
                        }
                        Said::Finished(Err(e)) => {
                            g.install_file.state = State::Failed(e.lines().filter(|l| !l.trim().is_empty()).map(|l| l.trim_start_matches("error: ")).collect::<Vec<_>>().join("\n"));
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

/// Remove an app installed from a download (asked first, where it shows).
pub fn remove(g: &mut Shell, id: &str, cx: &mut Context<Shell>) {
    let id = id.to_string();
    g.install_file.asking = None;
    g.install_file.asking_remove = false;
    g.install_file.removing = Some(id.clone());
    let here = g.install_file.info.as_ref().and_then(|i| i.as_ref().ok()).is_some_and(|i| i.id == id);
    if here {
        g.install_file.state = State::Removing;
    }
    cx.notify();
    cx.spawn(async move |this, cx| {
        let mut rx = nami::run(vec!["remove".into(), id.clone()], None);
        let mut end = Ok(());
        while let Some(said) = rx.next().await {
            if let Said::Finished(r) = said {
                end = r;
            }
        }
        let _ = this.update(cx, |g, cx| {
            g.install_file.removing = None;
            reload(g);
            if here {
                g.install_file.state = match &end {
                    Ok(()) => State::Idle,
                    Err(e) => State::Failed(e.clone()),
                };
                if let (Ok(()), Some(Ok(i))) = (&end, g.install_file.info.as_mut()) {
                    i.installed = None;
                }
            } else if let Err(e) = end {
                g.problem = Some(crate::Problem::of("Couldn't remove the app.", e));
            }
            cx.notify();
        });
    })
    .detach();
}

/// "Visual Studio Code is installed", with Open: as an app from the launcher's is told.
fn tell_installed(info: &Info, app: Option<&FileApp>) {
    let name = info.name.clone();
    let icon = app.and_then(|a| a.icon.clone()).or_else(|| info.icon.clone());
    let entry = app.and_then(|a| a.main_entry.clone());
    let commands = app.map(|a| a.commands.clone()).unwrap_or_default();
    std::thread::spawn(move || {
        let mut cmd = std::process::Command::new("notify-send");
        cmd.args(["--app-name=Apps", "--wait"]);
        if let Some(i) = &icon {
            cmd.arg(format!("--icon={i}"));
        }
        if entry.is_some() {
            cmd.args(["--action=open=Open", "--action=default=Open"]);
        }
        cmd.arg(format!("{name} is installed"));
        if entry.is_none() && !commands.is_empty() {
            cmd.arg(format!("Run it in a terminal: {}.", commands.join(", ")));
        }
        let Ok(out) = cmd.stderr(std::process::Stdio::null()).output() else { return };
        let picked = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if let (Some(e), "open" | "default") = (&entry, picked.as_str()) {
            let _ = crate::launch::open_app(Path::new(e));
        }
    });
}

/// The page: the app in the download, and Install.
pub fn render(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    let f = &g.install_file;
    // (Back to Your Apps, where it is once installed; or the page it was opened over, by name.)
    let back_label = match f.back {
        Some(Page::Os(page)) => <crate::os::Active as crate::os::Os>::back_label(g, page).unwrap_or("Your Apps"),
        Some(Page::Projects) => "Projects",
        Some(Page::Updates) => "Updates",
        _ => "Your Apps",
    };
    let mut page = div().flex().flex_col().gap(px(18.)).child(div().flex().child(pill("file-back".into(), format!("‹  {back_label}"), false).h(px(28.)).text_size(px(text::META)).on_click(cx.listener(|this, _, _, cx| {
        go_back(this);
        cx.notify();
    }))));
    let file_name = f.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let info = match &f.info {
        None => {
            return page.child(
                div().flex().items_start().gap(px(16.))
                    .child(icon(None, &file_name, 72.))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(6.))
                        .child(div().type_size(text::TITLE).font_weight(FontWeight::BOLD).child(crate::tip::cut(file_name)))
                        .child(div().flex().items_center().gap(px(8.)).text_size(px(text::BODY)).text_color(rgb(color::muted())).child(crate::motion::spinner("file-looking", 13., color::muted(), true)).child("Checking the file…"))),
            );
        }
        Some(Err(e)) => {
            return page.child(
                div().flex().items_start().gap(px(16.))
                    .child(icon(None, &file_name, 72.))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(6.))
                        .child(div().type_size(text::TITLE).font_weight(FontWeight::BOLD).child(crate::tip::cut(file_name)))
                        .child(div().text_size(px(text::BODY)).text_color(rgb(color::bad())).child(format!("This file can't be installed: {}", first_upper(e))))),
            );
        }
        Some(Ok(i)) => i.clone(),
    };
    let entry = f.apps.iter().find(|a| a.id == info.id).and_then(|a| a.main_entry.clone()).map(PathBuf::from).filter(|p| p.exists());
    let same = info.installed.as_deref() == Some(info.version.as_str());
    let state = f.state.clone();
    let asking = f.asking_remove;

    // The line under its name: what it is; or how installing goes.
    let line: AnyElement = match &state {
        State::Installing(now) if !now.is_empty() => div().child(crate::tip::cut(now.trim_end_matches('…').to_string() + "…")).into_any_element(),
        State::Installing(_) => div().child("Installing…").into_any_element(),
        State::Installed => div().text_color(rgb(color::good())).child(format!("{} is installed.", info.name)).into_any_element(),
        State::Failed(_) => div().text_color(rgb(color::bad())).child(format!("Couldn't install {}", info.name)).into_any_element(),
        State::Removing => div().child(format!("Removing {}…", info.name)).into_any_element(),
        State::Idle => match (&info.installed, same) {
            (Some(_), true) => div().child(format!("{} is already installed.", info.name)).into_any_element(),
            (Some(v), false) => div().child(format!("Version {v} is installed. This download is version {}.", info.version)).into_any_element(),
            _ => div().child(crate::tip::cut(info.summary.clone().unwrap_or_else(|| match info.format.as_deref() {
                Some("appimage") => "AppImage for any distribution".to_string(),
                Some("bundle") => "nami bundle, a single-file app for any distribution".to_string(),
                Some("tar") | Some("tarball") => "Program for any distribution".to_string(),
                _ => format!("App made for {}", info.made_for),
            }))).into_any_element(),
        },
    };
    let id = info.id.clone();
    let main: AnyElement = match &state {
        State::Installing(_) | State::Removing => crate::pages::explore::busy_pill("file-spin".into(), if state == State::Removing { "Removing" } else { "Installing" }, true).h(px(36.)).px(px(16.)).text_size(px(text::BODY)).into_any_element(),
        _ if info.blocked.is_some() => div().into_any_element(),
        _ if asking => {
            let id = id.clone();
            div().flex().items_center().gap(px(8.))
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).mr(px(4.)).child(format!("Remove {}?", info.name)))
                .child(button("file-keep", "Cancel", false, true).on_click(cx.listener(|this, _, _, cx| {
                    this.install_file.asking_remove = false;
                    cx.notify();
                })))
                .child(crate::look::destructive_button("file-off", "Remove").on_click(cx.listener(move |this, _, _, cx| remove(this, &id, cx))))
                .into_any_element()
        }
        State::Failed(_) => button("file-retry", "Try Again", true, true).on_click(cx.listener(|this, _, _, cx| install(this, cx))).into_any_element(),
        _ if info.installed.is_some() && same || state == State::Installed => div().flex().items_center().gap(px(10.))
            .child(button("file-ask-off", "Remove", false, true).on_click(cx.listener(|this, _, _, cx| {
                this.install_file.asking_remove = true;
                cx.notify();
            })))
            .into_any_element(),
        _ if info.installed.is_some() => button("file-add", "Install This Version", true, true).on_click(cx.listener(|this, _, _, cx| install(this, cx))).into_any_element(),
        _ => button("file-add", "Install", true, true).on_click(cx.listener(|this, _, _, cx| install(this, cx))).into_any_element(),
    };
    let open = entry.filter(|_| !asking && !matches!(state, State::Installing(_) | State::Removing) && (info.installed.is_some() || state == State::Installed));
    page = page.child(
        div().flex().items_start().gap(px(16.))
            .child(icon(info.icon.as_deref().map(Path::new), &info.name, 72.))
            .child(
                div().flex_1().min_w_0().flex().flex_col().gap(px(6.))
                    .child(div().type_size(text::TITLE).font_weight(FontWeight::BOLD).child(info.name.clone()))
                    .child(div().text_size(px(text::BODY)).text_color(rgb(color::muted())).child(line)),
            )
            .child(div().flex_none().flex().items_center().gap(px(10.)).child(main).children(open.map(|e| {
                button("file-open", "Open", true, true).on_click(cx.listener(move |this, _, _, cx| {
                    this.problem = crate::launch::open_app(&e).err().map(crate::Problem::under("Couldn't open the app."));
                    cx.notify();
                }))
            }))),
    );

    // Why it can't be installed at all.
    if let Some(b) = &info.blocked {
        page = page.child(note_box("file-blocked", "This app can't be installed", b, true));
    }

    // What it is, in a strip: version, maker, size, what it's made for.
    let fact = |label: &str, value: String, note: Option<String>| {
        div().flex_1().min_w_0().px(px(14.)).py(px(10.)).flex().flex_col().gap(px(3.))
            .child(div().text_size(px(text::CAPTION)).text_color(rgb(color::muted())).child(label.to_string()))
            .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child(crate::tip::cut(value)))
            .children(note.map(|n| div().text_size(px(text::CAPTION)).text_color(rgb(color::muted())).child(crate::tip::cut(n))))
    };
    // (A bundle's developer's key, if it signs its updates; and how it updates, for what it's made for.)
    let key = info.bundle.as_ref().and_then(crate::pages::install_bundle::key_note);
    let last = match &info.bundle {
        Some(b) => {
            let (value, note) = crate::pages::install_bundle::updates_fact(b);
            fact("Updates", value, note)
        }
        None => fact("Made for", info.made_for.clone(), Some(file_name.clone())),
    };
    // Who it's from: its maker (and the key that signs a bundle's updates, else its website), else its website, else the key; or nothing said.
    let site = info.homepage.as_ref().map(|h| h.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_string());
    let from = match (info.publisher.clone(), site, key) {
        (Some(p), site, key) => Some(fact("From", p, key.or(site))),
        (None, Some(site), _) => Some(fact("From", site, None)),
        (None, None, Some(key)) => Some(fact("Signed", "By the developer".into(), Some(key))),
        (None, None, None) => None,
    };
    // Its size: a bundle's parts unpacked from its file, less what this computer has
    // already (other bundles share them); a download's, installed.
    let installed_now = info.installed.is_some() && same || state == State::Installed;
    let size = match &info.bundle {
        Some(b) if b.here > 0 && !installed_now => {
            let new = info.size.saturating_sub(b.here);
            if new == 0 {
                fact("Size", "Nothing new".into(), Some(format!("All {} already on disk", size_words(info.size))))
            } else {
                fact("Size", format!("{} new", size_words(new)), Some(format!("+ {} already on disk", size_words(b.here.min(info.size)))))
            }
        }
        Some(_) => fact("Size", size_words(info.size), Some(format!("Unpacked from {}", size_words(info.download)))),
        None => fact("Size", size_words(info.size), Some(format!("{} download", size_words(info.download)))),
    };
    let facts: Vec<_> = [
        (!info.version.is_empty()).then(|| fact("Version", info.version.clone(), None)),
        from,
        Some(size),
        Some(last),
    ].into_iter().flatten().collect();
    page = page.child(div().flex().rounded(px(crate::look::shape::CONTROL)).border_1().border_color(color::hairline()).children(facts.into_iter().enumerate().map(|(i, d)| d.when(i > 0, |d| d.border_l_1().border_color(color::hairline())))));

    // What went wrong installing it.
    if let State::Failed(e) = &state {
        page = page.child(note_box("file-failed", &format!("Couldn't install {}", info.name), e, true));
    }
    // What won't work, before it's installed (and what it is, only until it is).
    for (n, w) in to_show(&info.warnings, installed_now).into_iter().enumerate() {
        page = page.child(note_box(ElementId::NamedInteger("file-warn".into(), n as u64), &w.title, &w.text, false));
    }
    // What it'll be once it's in.
    let mut what: Vec<String> = Vec::new();
    if info.app {
        what.push("Installs for you only and appears in the launcher. No admin password needed.".into());
    } else if !info.commands.is_empty() {
        what.push(format!("Adds the terminal {} {} for you only. No admin password needed.", if info.commands.len() == 1 { "command" } else { "commands" }, info.commands.join(", ")));
    }
    if !info.schemes.is_empty() {
        let mine: Vec<String> = info.schemes.iter().filter(|s| !["http", "https", "ftp", "mailto"].contains(&s.as_str())).map(|s| format!("{s}://")).collect();
        if !mine.is_empty() {
            what.push(format!("Opens {} links.", mine.join(" and ")));
        }
    }
    // (A bundle made without updates: its facts say so already.)
    let by_hand = info.bundle.as_ref().is_some_and(|b| b.updates == "none");
    what.extend(info.notes.iter().filter(|n| !(by_hand && n.starts_with("It doesn't update itself"))).cloned());
    if !what.is_empty() {
        page = page.child(div().flex().flex_col().gap(px(6.)).children(what.into_iter().map(|w| div().flex().items_start().gap(px(8.)).text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(div().mt(px(7.)).size(px(4.)).flex_none().rounded_full().bg(rgb(color::faint()))).child(div().flex_1().min_w_0().child(w)))));
    }
    if let Some(about) = info.description.clone() {
        page = page.child(div().text_size(px(text::BODY)).line_height(px(22.)).max_w(px(680.)).child(about));
    }
    page
}

/// The warnings to show: those about what it is only until it's `installed`.
fn to_show(warnings: &[Warning], installed: bool) -> Vec<&Warning> {
    warnings.iter().filter(|w| !(installed && w.until_installed)).collect()
}

fn first_upper(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// A note in a soft box: what won't work (slate), or what went wrong (red).
fn note_box(id: impl Into<ElementId>, title: &str, text: &str, bad: bool) -> gpui::Stateful<Div> {
    let (bg, fg) = if bad { (color::bad_soft(), color::bad()) } else { (color::soft(), color::deep()) };
    div()
        .id(id.into())
        .flex()
        .gap(px(12.))
        .px(px(14.))
        .py(px(12.))
        .rounded(px(crate::look::shape::CONTROL))
        .bg(rgb(bg))
        .child(div().mt(px(5.)).size(px(8.)).flex_none().rounded_full().bg(rgb(fg)))
        .child(
            div().flex_1().min_w_0().flex().flex_col().gap(px(3.))
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
                .child(div().text_size(px(text::CALLOUT)).line_height(px(19.)).text_color(rgb(color::text())).child(text.to_string())),
        )
}

/// Your Apps' rows for the apps installed from downloads: Open and Remove.
pub fn rows(g: &Shell, cx: &mut Context<Shell>) -> Vec<gpui::Stateful<Div>> {
    let f = &g.install_file;
    f.apps.iter().enumerate().map(|(n, a)| {
        let idn = |what: &str| ElementId::NamedInteger(format!("dl-{what}").into(), n as u64);
        let id = a.id.clone();
        let action: AnyElement = if f.removing.as_deref() == Some(a.id.as_str()) {
            crate::pages::explore::busy_pill(idn("spin"), "Removing", true).into_any_element()
        } else if f.asking.as_deref() == Some(a.id.as_str()) {
            div().flex().flex_none().items_center().gap(px(8.))
                .child(div().text_size(px(text::CALLOUT)).font_weight(FontWeight::MEDIUM).mr(px(2.)).child(format!("Remove {}?", a.name)))
                .child(crate::pages::explore::small_action(idn("keep"), "Cancel".into()).on_click(cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.install_file.asking = None;
                    cx.notify();
                })))
                .child(crate::pages::explore::small_action(idn("off"), "Remove".into()).bg(rgb(color::bad())).border_color(rgb(color::bad())).text_color(rgb(color::on_bad())).on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    remove(this, &id, cx);
                })))
                .into_any_element()
        } else {
            let entry = a.main_entry.clone().map(PathBuf::from).filter(|p| p.exists());
            div().flex().flex_none().items_center().gap(px(6.))
                .child(div().id(idn("ask-off")).px(px(10.)).h(px(28.)).flex().items_center().rounded_full().text_size(px(text::META)).text_color(rgb(color::muted())).cursor_pointer().press_row().hover(|s| s.text_color(rgb(color::text())).bg(color::push())).child("Remove").on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.install_file.asking = Some(id.clone());
                    cx.notify();
                })))
                .children(entry.map(|e| {
                    crate::pages::explore::small_action(idn("open"), "Open".into()).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.problem = crate::launch::open_app(&e).err().map(crate::Problem::under("Couldn't open the app."));
                        cx.notify();
                    }))
                }))
                .into_any_element()
        };
        let line = match a.format.as_str() {
            "deb" => "Debian or Ubuntu package",
            "rpm" => "Fedora package",
            "appimage" => "AppImage",
            _ => "Downloaded app",
        };
        let line = if a.format == "bundle" {
            crate::pages::install_bundle::row_line(&a.version, a.live.as_ref())
        } else if a.version.is_empty() {
            line.to_string()
        } else {
            format!("{line} · {}", a.version)
        };
        let (update_line, bad) = super::app_updates::line_of(g, a);
        div()
            .id(idn("row"))
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .py(px(10.))
            .child(icon(a.icon.as_deref().map(Path::new), &a.name, 36.))
            .child(
                div().flex_1().min_w_0().flex().flex_col()
                    .child(div().flex().items_center().gap(px(8.)).child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(a.name.clone())).when(a.main_entry.is_none() && !a.commands.is_empty(), |d| d.child(div().text_size(px(text::CAPTION)).px(px(6.)).rounded(px(4.)).bg(color::push()).text_color(rgb(color::muted())).child("command"))))
                    .child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(crate::tip::cut(line)))
                    // Its updates: how it's going, Update now, its switch.
                    .child(div().text_size(px(text::META)).text_color(rgb(if bad { color::bad() } else { color::muted() })).child(update_line))
                    // (Not for a bundle made without updates: there are none to have.)
                    .when(a.format != "bundle" || a.bundle.as_ref().is_some_and(|b| b.feed.is_some()), |d| d.child(div().mt(px(6.)).flex().items_center().gap(px(14.)).child(super::app_updates::button(g, idn("update"), a, cx)).child(super::app_updates::switch_row(g, idn("auto"), a, cx)))),
            )
            .child(action)
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_nami_says_of_a_download_is_read() {
        let info: Info = serde_json::from_str(r#"{"file":"/d/code.deb","format":"deb","made_for":"Debian and Ubuntu","id":"code","package":"code","name":"Visual Studio Code","version":"1.145.0","publisher":"Microsoft Corporation","size":1072162000,"download":263019110,"app":true,"commands":["code"],"schemes":["vscode"],"warnings":[{"title":"A","text":"B"}],"notes":[],"blocked":null,"updates":{"kind":"apt","url":"https://packages.microsoft.com/repos/code","suite":"stable","components":"main"},"installed":null,"needs":["gtk3"]}"#).unwrap();
        assert_eq!(info.name, "Visual Studio Code");
        assert_eq!(info.warnings.len(), 1);
        assert_eq!(size_words(info.size), "1.1 GB");
        assert_eq!(size_words(263019110), "263 MB");
        assert_eq!(size_words(1_700_000), "1.7 MB");
    }

    #[test]
    fn a_program_from_the_internet_is_said_until_its_installed() {
        let warnings: Vec<Warning> = serde_json::from_str(r#"[{"title":"A program from the internet","text":"B","until_installed":true},{"title":"Its driver won't work","text":"C"}]"#).unwrap();
        assert_eq!(to_show(&warnings, false).len(), 2);
        let shown: Vec<&str> = to_show(&warnings, true).iter().map(|w| w.title.as_str()).collect();
        assert_eq!(shown, ["Its driver won't work"]);
    }
}
