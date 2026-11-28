//! Apps from Flathub, through Flatpak: `- flathub:org.gnome.Calculator` in a
//! person's packages. They're each person's own (`flatpak --user`, in
//! ~/.local/share/flatpak), as the rest of their user.nami is: installing
//! one asks no one for a password, and nobody else gets it. `nami apply`
//! installs what's listed and takes off what nami put there that's no longer
//! listed (never what someone installed with flatpak themselves); each
//! change is a step of the profile's history like any other (an entry
//! `flathub:<id>` with no store paths, its version in its name), so
//! `nami undo` takes it back too.
//!
//! Also Flathub's apps for a search (its API, cached in
//! ~/.cache/nami/flathub), and what's newer there.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::profile::Entry;

/// How an entry from Flathub is written: `flathub:<app id>`.
pub const PREFIX: &str = "flathub:";
/// The remote's name, as Flatpak calls it.
pub const REMOTE: &str = "flathub";
const REPO: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";
const LOCAL_REPO: &str = "/etc/flatpak/remotes.d/flathub.flatpakrepo";
const API: &str = "https://flathub.org/api/v2";
/// GTK 3's look inside the sandbox, as namiOS has it outside (its desktop
/// sets gtk-theme to adw-gtk3, and adw-gtk3-dark in the dark): Flatpak's
/// GTK 3 apps find the theme of that name among these.
pub const THEMES: &[&str] = &["org.gtk.Gtk3theme.adw-gtk3", "org.gtk.Gtk3theme.adw-gtk3-dark"];
/// How long a search's answer is kept, and an app's page.
const SEARCH_FOR: Duration = Duration::from_secs(24 * 3600);
const PAGE_FOR: Duration = Duration::from_secs(7 * 24 * 3600);

/// The app id of a `flathub:` entry.
pub fn id(entry: &str) -> Option<&str> {
    entry.strip_prefix(PREFIX)
}

pub fn is_flathub(entry: &str) -> bool {
    entry.starts_with(PREFIX)
}

/// An app id as Flatpak takes one: three or more parts with dots between
/// (org.gnome.Calculator), each of letters, digits, _ and -, the first not
/// starting with a digit.
pub fn valid_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    id.len() <= 255
        && parts.len() >= 3
        && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        && !parts[0].starts_with(|c: char| c.is_ascii_digit())
}

/// The profile's entry for an app: its version in its name, when known
/// (flathub:org.gnome.Calculator-49.1), so a step that updated it says so.
pub fn entry(id: &str, version: Option<&str>) -> Entry {
    let attr = format!("{PREFIX}{id}");
    let name = match version.map(|v| v.split_whitespace().next().unwrap_or_default()).filter(|v| !v.is_empty()) {
        Some(v) => format!("{attr}-{v}"),
        None => attr.clone(),
    };
    Entry { attr, name, paths: Vec::new() }
}

/// The app ids among a profile's entries.
pub fn ids_of(entries: &[Entry]) -> Vec<String> {
    entries.iter().filter_map(|e| id(&e.attr)).map(String::from).collect()
}

/// Flatpak's program: the computer's (namiOS's flatpak package), else one
/// on PATH. `NAMI_FLATPAK` names another (tests).
pub fn program() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NAMI_FLATPAK").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let mut dirs = vec![PathBuf::from("/run/current-system/sw/bin"), crate::migrate::here().join("profiles/default/bin")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join(".local/state/nami/profile/bin"));
    }
    dirs.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    dirs.into_iter().map(|d| d.join("flatpak")).find(|p| p.is_file())
}

fn needs_flatpak() -> anyhow::Error {
    anyhow::anyhow!("Flathub's apps need Flatpak, and it isn't on this computer: on namiOS it comes with the flatpak package (`- flatpak` in system.nami); elsewhere, install the distribution's flatpak")
}

/// Flatpak, as one person's (`--user`).
pub struct Flatpak {
    bin: PathBuf,
}

impl Flatpak {
    pub fn find() -> Result<Flatpak> {
        program().map(|bin| Flatpak { bin }).ok_or_else(needs_flatpak)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(&self.bin);
        c.args(args);
        // (Its own messages, not the session's debugging.)
        c.env_remove("G_MESSAGES_DEBUG");
        c
    }

    /// What it prints, for what's only read.
    fn read(&self, args: &[&str]) -> Result<String> {
        let out = self.command(args).stdin(Stdio::null()).output().with_context(|| format!("running {}", self.bin.display()))?;
        if !out.status.success() {
            bail!("flatpak {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// A change: what it says goes to nami's output as it goes.
    fn change(&self, args: &[&str]) -> Result<()> {
        crate::say!("flatpak {}", args.join(" "));
        let status = self.command(args).stdin(Stdio::null()).stdout(Stdio::inherit()).stderr(Stdio::inherit()).status().with_context(|| format!("running {}", self.bin.display()))?;
        if !status.success() {
            bail!("flatpak {} didn't work (it says why above)", args.join(" "));
        }
        Ok(())
    }

    /// The person's apps (and runtimes, with `runtimes`): id -> version.
    pub fn installed(&self, runtimes: bool) -> Result<BTreeMap<String, String>> {
        let kind = if runtimes { "--runtime" } else { "--app" };
        let text = self.read(&["list", "--user", kind, "--columns=application,version"])?;
        Ok(columns(&text).into_iter().map(|mut c| (c.remove(0), c.into_iter().next().unwrap_or_default())).collect())
    }

    /// Flathub, for this person (once).
    fn ensure_remote(&self) -> Result<()> {
        let have = self.read(&["remotes", "--user", "--columns=name"]).unwrap_or_default();
        if have.lines().any(|l| l.trim() == REMOTE) {
            return Ok(());
        }
        // The computer's file of it (the flatpak package's), else Flathub's.
        let local = Path::new(LOCAL_REPO);
        let from = if local.is_file() { LOCAL_REPO } else { REPO };
        self.change(&["remote-add", "--user", "--if-not-exists", REMOTE, from])
    }

    pub fn install(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.ensure_remote()?;
        let mut args = vec!["install", "--user", "--noninteractive", "-y", REMOTE];
        args.extend(ids.iter().map(String::as_str));
        self.change(&args)?;
        // namiOS's GTK 3 look for them, once (small; a failure here isn't the app's).
        if crate::nami_os::here() {
            let have = self.installed(true).unwrap_or_default();
            let themes: Vec<&str> = THEMES.iter().copied().filter(|t| !have.contains_key(*t)).collect();
            if !themes.is_empty() {
                let mut args = vec!["install", "--user", "--noninteractive", "-y", REMOTE];
                args.extend(themes);
                if let Err(e) = self.change(&args) {
                    crate::say!("warning: GTK's look for Flatpak's apps: {e:#}");
                }
            }
        }
        Ok(())
    }

    /// Take apps off, and then the runtimes nothing uses now. Their data
    /// (~/.var/app/<id>) stays, as an app's settings do elsewhere.
    pub fn uninstall(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut args = vec!["uninstall", "--user", "--noninteractive", "-y"];
        args.extend(ids.iter().map(String::as_str));
        self.change(&args)?;
        if let Err(e) = self.change(&["uninstall", "--user", "--unused", "--noninteractive", "-y"]) {
            crate::say!("warning: {e:#}");
        }
        Ok(())
    }

    /// Newer versions on Flathub: id -> version there.
    pub fn newer(&self) -> Result<BTreeMap<String, String>> {
        let text = self.read(&["remote-ls", "--user", "--updates", "--app", "--columns=application,version"])?;
        Ok(columns(&text).into_iter().map(|mut c| (c.remove(0), c.into_iter().next().unwrap_or_default())).collect())
    }

    /// Everything of this person's to its newest: apps, runtimes, themes.
    pub fn update(&self) -> Result<()> {
        self.change(&["update", "--user", "--noninteractive", "-y"])
    }
}

/// The person's apps that start by D-Bus (most GNOME apps, Settings' Open and
/// the launcher's start them so): their services, linked where the session bus
/// looks (~/.local/share/dbus-1/services; Flatpak puts them in its own folder),
/// the ones gone unlinked; then the bus is told.
pub fn link_services() {
    let Some(data) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))) else { return };
    let (from, to) = (data.join("flatpak/exports/share/dbus-1/services"), data.join("dbus-1/services"));
    let mut changed = false;
    // Gone: links into Flatpak's folder with nothing there now.
    for e in std::fs::read_dir(&to).into_iter().flatten().flatten() {
        let p = e.path();
        if std::fs::read_link(&p).is_ok_and(|t| t.starts_with(&from)) && !p.exists() {
            changed |= std::fs::remove_file(&p).is_ok();
        }
    }
    for e in std::fs::read_dir(&from).into_iter().flatten().flatten() {
        let link = to.join(e.file_name());
        if link.symlink_metadata().is_err() && e.file_name().to_string_lossy().ends_with(".service") {
            let _ = std::fs::create_dir_all(&to);
            changed |= std::os::unix::fs::symlink(e.path(), &link).is_ok();
        }
    }
    if changed {
        let _ = Command::new("busctl").args(["--user", "call", "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ReloadConfig"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
}

/// Flatpak's tab-separated columns, a row a line (its header left out).
fn columns(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split('\t').map(|c| c.trim().to_string()).collect::<Vec<_>>())
        .filter(|c| !c[0].is_empty() && !c[0].eq_ignore_ascii_case("application id") && !c[0].eq_ignore_ascii_case("application"))
        .collect()
}

/// The apps listed, installed (what's missing comes now): the entries of
/// those that are there, with their versions, and why the others aren't
/// (no network, no such app), so the rest of a setup still comes up.
pub fn install(ids: &[String]) -> (Vec<Entry>, Vec<(String, String)>) {
    if ids.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let fp = match Flatpak::find() {
        Ok(f) => f,
        Err(e) => return (Vec::new(), ids.iter().map(|i| (format!("{PREFIX}{i}"), format!("{e:#}"))).collect()),
    };
    let have = fp.installed(false).unwrap_or_default();
    let missing: Vec<String> = ids.iter().filter(|i| !have.contains_key(*i)).cloned().collect();
    let mut why: BTreeMap<String, String> = BTreeMap::new();
    if !missing.is_empty() {
        crate::say!("from Flathub: {}", missing.join(", "));
        // All in one go; if that doesn't work, each alone, to tell which.
        if fp.install(&missing).is_err() && missing.len() > 1 {
            for i in &missing {
                if let Err(e) = fp.install(std::slice::from_ref(i)) {
                    why.insert(i.clone(), format!("{e:#}"));
                }
            }
        }
    }
    let have = fp.installed(false).unwrap_or_default();
    let (mut entries, mut left) = (Vec::new(), Vec::new());
    for i in ids {
        match have.get(i) {
            Some(v) => entries.push(entry(i, Some(v))),
            None => left.push((format!("{PREFIX}{i}"), why.remove(i).unwrap_or_else(|| "Flatpak couldn't install it (no connection to Flathub, or no app of that id: `nami search <word> --flathub` finds them)".into()))),
        }
    }
    link_services();
    (entries, left)
}

/// The entries the apps listed would have, without changing anything.
pub fn entries(ids: &[String]) -> Vec<Entry> {
    let have = Flatpak::find().and_then(|f| f.installed(false)).unwrap_or_default();
    ids.iter().map(|i| entry(i, have.get(i).map(String::as_str))).collect()
}

/// A profile went from `before` to `after`: what nami installed that's
/// gone now comes off, and what's listed but not there (going back to an
/// earlier step) goes on. Said, not failed: the rest of the step stands.
pub fn follow(before: &[Entry], after: &[Entry]) {
    let (was, now) = (ids_of(before), ids_of(after));
    if was.is_empty() && now.is_empty() {
        return;
    }
    let Ok(fp) = Flatpak::find() else {
        if now.iter().any(|i| !was.contains(i)) {
            crate::say!("warning: {}", needs_flatpak());
        }
        return;
    };
    let have = fp.installed(false).unwrap_or_default();
    let gone: Vec<String> = was.iter().filter(|i| !now.contains(i) && have.contains_key(*i)).cloned().collect();
    if let Err(e) = fp.uninstall(&gone) {
        crate::say!("warning: taking off {}: {e:#}", gone.join(", "));
    }
    let missing: Vec<String> = now.iter().filter(|i| !have.contains_key(*i)).cloned().collect();
    if let Err(e) = fp.install(&missing) {
        crate::say!("warning: putting back {}: {e:#}", missing.join(", "));
    }
    link_services();
}

/// The listed apps' newer versions: (entry, version now, version then).
pub fn updates(listed: &[Entry]) -> Vec<(String, String, String)> {
    let ids = ids_of(listed);
    if ids.is_empty() {
        return Vec::new();
    }
    let Ok(fp) = Flatpak::find() else { return Vec::new() };
    let (have, newer) = (fp.installed(false).unwrap_or_default(), fp.newer().unwrap_or_default());
    ids.iter()
        .filter_map(|i| {
            let to = newer.get(i)?;
            let from = have.get(i).cloned().unwrap_or_default();
            // (A new build of the same version: still an update.)
            let to = if to.is_empty() || *to == from { "newer build".to_string() } else { to.clone() };
            Some((format!("{PREFIX}{i}"), from, to))
        })
        .collect()
}

// ---------- Flathub's catalog: searching it, and an app's page ----------

/// An app as Flathub's search finds it (the fields used here).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Hit {
    pub app_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub project_license: Option<String>,
    #[serde(default)]
    pub is_free_license: Option<bool>,
    #[serde(default)]
    pub developer_name: Option<String>,
    #[serde(default)]
    pub main_categories: Option<serde_json::Value>,
    #[serde(default)]
    pub installs_last_month: Option<u64>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    /// The computers Flathub builds it for (x86_64, aarch64); empty when it doesn't say.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arches: Vec<String>,
    // An app's page only:
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub screenshots: Vec<String>,
}

fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("nami/flathub"))
}

/// A file of the cache, if it's younger than `fresh` (any age: `None`).
fn cached(file: &Path, fresh: Option<Duration>) -> Option<String> {
    let age = std::fs::metadata(file).ok()?.modified().ok()?.elapsed().unwrap_or_default();
    if fresh.is_some_and(|f| age > f) {
        return None;
    }
    std::fs::read_to_string(file).ok()
}

fn keep(file: &Path, text: &str) {
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let part = file.with_extension("part");
    if std::fs::write(&part, text).is_ok() {
        let _ = std::fs::rename(&part, file);
    }
}

fn http() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(20))
        .user_agent(concat!("nami/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// What Flathub answers, as bytes (at most `limit`). Asked once more if it didn't get there or
/// broke off on the way (a dropped connection, a name that didn't resolve, no answer in time):
/// a moment's trouble with the network shouldn't cost a search its answer, or an app its
/// picture. An answer that's an error (404, 500) isn't asked again. Its error says which it was:
/// "couldn't reach Flathub", or what Flathub answered.
fn fetch(go: impl Fn() -> Result<ureq::Response, ureq::Error>, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    // (Whether it's worth a second try, and why not.)
    let once = || -> std::result::Result<Vec<u8>, (bool, String)> {
        let answer = go().map_err(|e| match e {
            ureq::Error::Status(code, r) => (false, format!("Flathub answered {code} for {}", r.get_url())),
            ureq::Error::Transport(t) => (true, format!("couldn't reach Flathub ({t})")),
        })?;
        let mut data = Vec::new();
        answer.into_reader().take(limit).read_to_end(&mut data).map_err(|e| (true, format!("couldn't reach Flathub (its answer broke off: {e})")))?;
        Ok(data)
    };
    match once() {
        Ok(data) => Ok(data),
        Err((true, _)) => {
            std::thread::sleep(Duration::from_secs(1));
            once().map_err(|(_, e)| anyhow::anyhow!(e))
        }
        Err((false, e)) => Err(anyhow::anyhow!(e)),
    }
}

/// Flathub's apps for `q`: from the cache for a day, else asked (and on
/// no answer, as last asked, however old).
pub fn search(q: &str) -> Result<Vec<Hit>> {
    let q = q.trim().to_lowercase();
    if q.chars().count() < 2 {
        return Ok(Vec::new());
    }
    let file = cache_dir().map(|d| d.join("search").join(format!("{}.json", &crate::catalog::sha256_hex(q.as_bytes())[..24])));
    if let Some(text) = file.as_deref().and_then(|f| cached(f, Some(SEARCH_FOR))) {
        if let Ok(hits) = serde_json::from_str(&text) {
            return Ok(hits);
        }
    }
    let body = serde_json::json!({ "query": q, "hits_per_page": 30 }).to_string();
    let asked = fetch(|| http().post(&format!("{API}/search")).set("Content-Type", "application/json").send_string(&body), 16 << 20)
        .and_then(|data| Ok(serde_json::from_slice::<serde_json::Value>(&data)?));
    let answer = match asked {
        Ok(v) => v,
        Err(e) => match file.as_deref().and_then(|f| cached(f, None)).and_then(|t| serde_json::from_str(&t).ok()) {
            Some(old) => return Ok(old),
            None => return Err(e).context("asking Flathub"),
        },
    };
    let hits = read_hits(&answer);
    if let Some(f) = &file {
        keep(f, &serde_json::to_string(&hits)?);
    }
    Ok(hits)
}

/// The apps of a search's answer (not runtimes, nor what's no longer kept up; on an ARM
/// computer, not those Flathub has for x86_64 alone).
fn read_hits(answer: &serde_json::Value) -> Vec<Hit> {
    let list = answer.get("hits").and_then(|h| h.as_array()).cloned().unwrap_or_default();
    list.into_iter()
        .filter_map(|h| serde_json::from_value::<Hit>(h).ok())
        .filter(|h| valid_id(&h.app_id) && h.kind.as_deref().is_none_or(|k| k == "desktop-application" || k == "desktop"))
        .filter(|h| runs_here(h, crate::arch::ARM))
        .collect()
}

/// Whether Flathub has the app for this computer: on ARM, for aarch64 (an app that doesn't
/// say is kept). Every one on x86_64, as before.
fn runs_here(h: &Hit, arm: bool) -> bool {
    !arm || h.arches.is_empty() || h.arches.iter().any(|a| a == "aarch64")
}

/// One app's page on Flathub: its words, version, homepage and screenshots
/// (kept a week).
pub fn page(id: &str) -> Result<Hit> {
    anyhow::ensure!(valid_id(id), "{id} isn't an app id");
    let file = cache_dir().map(|d| d.join("apps").join(format!("{id}.json")));
    if let Some(text) = file.as_deref().and_then(|f| cached(f, Some(PAGE_FOR))) {
        if let Ok(hit) = serde_json::from_str(&text) {
            return Ok(hit);
        }
    }
    let asked = fetch(|| http().get(&format!("{API}/appstream/{id}")).call(), 16 << 20).and_then(|data| Ok(serde_json::from_slice::<serde_json::Value>(&data)?));
    let v = match asked {
        Ok(v) => v,
        Err(e) => match file.as_deref().and_then(|f| cached(f, None)).and_then(|t| serde_json::from_str(&t).ok()) {
            Some(old) => return Ok(old),
            None => return Err(e).context("asking Flathub"),
        },
    };
    let hit = read_page(id, &v);
    if let Some(f) = &file {
        keep(f, &serde_json::to_string(&hit)?);
    }
    Ok(hit)
}

fn read_page(id: &str, v: &serde_json::Value) -> Hit {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(String::from);
    // Each screenshot's largest picture.
    let screenshots = v.get("screenshots").and_then(|x| x.as_array()).into_iter().flatten().filter_map(|shot| {
        let sizes = shot.get("sizes")?.as_array()?;
        let width = |s: &serde_json::Value| s.get("width").and_then(|w| w.as_str().and_then(|w| w.parse::<u32>().ok()).or_else(|| w.as_u64().map(|w| w as u32))).unwrap_or(0);
        sizes.iter().filter(|s| width(s) <= 1600).max_by_key(|s| width(s)).and_then(|s| s.get("src")?.as_str()).map(String::from)
    }).take(4).collect();
    Hit {
        app_id: id.to_string(),
        name: s("name").unwrap_or_else(|| id.to_string()),
        summary: s("summary"),
        description: s("description").map(|d| plain(&d)),
        icon: s("icon"),
        project_license: s("project_license"),
        is_free_license: v.get("is_free_license").and_then(|x| x.as_bool()),
        developer_name: s("developer_name"),
        main_categories: v.get("categories").cloned(),
        installs_last_month: None,
        kind: s("type"),
        arches: Vec::new(),
        homepage: v.get("urls").and_then(|u| u.get("homepage")).and_then(|h| h.as_str()).map(String::from),
        version: v.get("releases").and_then(|r| r.as_array()).and_then(|r| r.first()).and_then(|r| r.get("version")).and_then(|x| x.as_str()).map(String::from),
        screenshots,
    }
}

/// AppStream's text (a little HTML: <p>, <li>) as plain sentences.
fn plain(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut tag = String::new();
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let t = tag.trim_start_matches('/').split_whitespace().next().unwrap_or("").to_lowercase();
                if matches!(t.as_str(), "p" | "br" | "ul" | "ol") && !out.ends_with("\n\n") && !out.is_empty() {
                    out.push_str("\n\n");
                } else if t == "li" && !out.ends_with('\n') && !out.is_empty() {
                    out.push('\n');
                }
                if t == "li" && !tag.starts_with('/') {
                    out.push_str("• ");
                }
            }
            _ if in_tag => tag.push(c),
            _ => out.push(c),
        }
    }
    let text = out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'");
    // Each paragraph's lines joined, as the words were wrapped in the source.
    text.split("\n\n").map(|p| p.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")).filter(|p| !p.is_empty()).map(|p| {
        p.lines().fold(String::new(), |mut acc, l| {
            if !acc.is_empty() {
                acc.push(if l.starts_with('•') { '\n' } else { ' ' });
            }
            acc.push_str(l);
            acc
        })
    }).collect::<Vec<_>>().join("\n\n")
}

/// nami's kinds of app (query::CATEGORIES' keys) for Flathub's.
fn groups(cats: Option<&serde_json::Value>) -> Vec<String> {
    let names: Vec<String> = match cats {
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|c| c.as_str().map(String::from)).collect(),
        _ => Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    for n in names {
        let g = match n.to_lowercase().as_str() {
            "office" => "office",
            "graphics" | "photography" => "graphics",
            "audiovideo" | "audio" | "video" => "audio-video",
            "game" => "games",
            "network" => "internet",
            "development" => "coding",
            "education" | "science" => "learning",
            "utility" | "system" | "settings" => "tools",
            _ => continue,
        };
        if !out.iter().any(|o| o == g) {
            out.push(g.to_string());
        }
    }
    out
}

/// Where Flatpak puts the person's apps' menu entries and icons.
pub fn exports() -> Vec<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    data.map(|d| d.join("flatpak/exports/share")).into_iter().chain([PathBuf::from("/var/lib/flatpak/exports/share")]).collect()
}

/// An installed app's own icon, as Flatpak put it out.
fn exported_icon(id: &str) -> Option<PathBuf> {
    for share in exports() {
        let hicolor = share.join("icons/hicolor");
        let mut candidates = vec![hicolor.join(format!("scalable/apps/{id}.svg"))];
        for size in ["512x512", "256x256", "128x128", "96x96", "64x64", "48x48"] {
            candidates.push(hicolor.join(format!("{size}/apps/{id}.png")));
        }
        if let Some(c) = candidates.into_iter().find(|c| c.is_file()) {
            return Some(c);
        }
    }
    None
}

/// An app's icon from Flathub, downloaded once into the cache. (None without one to get; and
/// if it couldn't be had, it says why, as a search's own trouble is said.)
fn icon_file(id: &str, url: &str) -> Option<PathBuf> {
    let file = cache_dir()?.join("icons").join(format!("{id}.png"));
    if file.is_file() {
        return Some(file);
    }
    if !url.starts_with("https://") {
        return None;
    }
    let data = match fetch(|| http().get(url).call(), 4 << 20) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("Flathub: no picture of {id}: {e:#}");
            return None;
        }
    };
    let _ = std::fs::create_dir_all(file.parent()?);
    let part = file.with_extension("part");
    std::fs::write(&part, &data).ok()?;
    std::fs::rename(&part, &file).ok()?;
    Some(file)
}

/// Icons for these apps, downloaded side by side (the ones not had yet).
fn icons(hits: &[Hit]) -> Vec<Option<PathBuf>> {
    let threads: Vec<_> = hits
        .iter()
        .map(|h| {
            let (id, url) = (h.app_id.clone(), h.icon.clone().unwrap_or_default());
            std::thread::spawn(move || exported_icon(&id).or_else(|| icon_file(&id, &url)))
        })
        .collect();
    threads.into_iter().map(|t| t.join().ok().flatten()).collect()
}

/// A Flathub app as nami lists packages (`nami search --json`, `nami info`).
pub fn item(hit: &Hit, installed: Option<&str>, icon: Option<PathBuf>) -> crate::query::Item {
    crate::query::Item {
        attr: format!("{PREFIX}{}", hit.app_id),
        name: entry(&hit.app_id, installed.or(hit.version.as_deref())).name,
        version: installed.or(hit.version.as_deref()).unwrap_or_default().to_string(),
        description: hit.summary.clone(),
        homepage: hit.homepage.clone(),
        license: hit.project_license.clone(),
        app: true,
        installed: installed.is_some(),
        program: None,
        icon: icon.or_else(|| exported_icon(&hit.app_id)).map(|p| p.display().to_string()),
        unfree: hit.is_free_license == Some(false),
        builds_here: false,
        publisher: hit.developer_name.clone(),
        title: Some(hit.name.clone()).filter(|n| !n.is_empty()),
        summary: hit.summary.clone(),
        about: hit.description.as_deref().map(plain).filter(|d| !d.is_empty()),
        categories: groups(hit.main_categories.as_ref()),
        screenshots: hit.screenshots.clone(),
        popularity: None,
        desktop: vec![format!("{}.desktop", hit.app_id)],
        source: Some(REMOTE.to_string()),
        unavailable: None,
    }
}

/// Whether a catalog app and a Flathub one are the same app: the same menu
/// entry, or the same name with the package's name in Flathub's id
/// (spotify, com.spotify.Client) or the id's last part in the package's
/// (gnome-calculator, org.gnome.Calculator).
pub fn same_app(catalog: &crate::query::Item, hit: &Hit) -> bool {
    let entry = format!("{}.desktop", hit.app_id).to_lowercase();
    if catalog.desktop.iter().any(|d| d.to_lowercase() == entry) {
        return true;
    }
    let (attr, id) = (catalog.attr.to_lowercase(), hit.app_id.to_lowercase());
    let last = id.rsplit('.').next().unwrap_or(&id).to_string();
    let named_alike = catalog.title.as_deref().is_some_and(|t| t.eq_ignore_ascii_case(hit.name.trim()));
    named_alike && (id.split('.').any(|p| p == attr) || attr.split(['-', '_']).any(|p| p == last))
}

/// Flathub's apps for a search, after the catalog's (`items`): the ones the
/// catalog has too are left out, as the catalog's build is the one to
/// have, unless it'd be built here (which takes a while: Flathub's is
/// ready). With no Flatpak on the computer, none.
pub fn add_to_search(items: &mut Vec<crate::query::Item>, q: &str, limit: usize) {
    if program().is_none() && std::env::var_os("NAMI_FLATHUB_SEARCH").is_none() {
        return;
    }
    let hits = match search(q) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Flathub: {e:#}");
            return;
        }
    };
    let mut keep: Vec<Hit> = hits.into_iter().filter(|h| !items.iter().any(|i| same_app(i, h) && !i.builds_here)).collect();
    // Most used first, among those that match as well (Flathub's order, by tens).
    let ranked: Vec<(usize, Hit)> = {
        let mut r: Vec<(usize, Hit)> = keep.drain(..).enumerate().collect();
        r.sort_by_key(|(i, h)| (i / 10, std::cmp::Reverse(h.installs_last_month.unwrap_or(0))));
        r
    };
    let hits: Vec<Hit> = ranked.into_iter().map(|(_, h)| h).take(limit).collect();
    let installed = Flatpak::find().and_then(|f| f.installed(false)).unwrap_or_default();
    let pictures = icons(&hits);
    items.extend(hits.iter().zip(pictures).map(|(h, icon)| item(h, installed.get(&h.app_id).map(String::as_str), icon)));
}

/// `nami info flathub:<id>`: its page (or, offline and never seen, what
/// its installed menu entry says).
pub fn info(id: &str) -> Option<crate::query::Item> {
    let installed = Flatpak::find().and_then(|f| f.installed(false)).unwrap_or_default();
    let hit = match page(id) {
        Ok(h) => h,
        Err(e) => {
            let (name, summary) = entry_text(id)?;
            eprintln!("Flathub: {e:#}");
            Hit { app_id: id.to_string(), name, summary, ..Default::default() }
        }
    };
    let icon = exported_icon(id).or_else(|| hit.icon.as_deref().and_then(|u| icon_file(id, u)));
    Some(item(&hit, installed.get(id).map(String::as_str), icon))
}

/// An installed app's name and line, from its menu entry.
fn entry_text(id: &str) -> Option<(String, Option<String>)> {
    let text = exports().into_iter().find_map(|d| std::fs::read_to_string(d.join(format!("applications/{id}.desktop"))).ok())?;
    let field = |k: &str| text.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix('=')).map(|v| v.trim().to_string());
    Some((field("Name")?, field("Comment")))
}

/// What `nami list` says of an installed one.
pub fn list_item(e: &Entry) -> Option<crate::query::Item> {
    let id = id(&e.attr)?;
    let version = crate::query::version_of(&e.name);
    let hit = page_cached(id).unwrap_or_else(|| {
        let (name, summary) = entry_text(id).unwrap_or((id.to_string(), None));
        Hit { app_id: id.to_string(), name, summary, ..Default::default() }
    });
    Some(item(&hit, Some(&version), None))
}

/// An app's page as last fetched, without asking.
fn page_cached(id: &str) -> Option<Hit> {
    let file = cache_dir()?.join("apps").join(format!("{id}.json"));
    serde_json::from_str(&cached(&file, None)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_flatpaks() {
        assert!(valid_id("org.gnome.Calculator"));
        assert!(valid_id("com.spotify.Client"));
        assert!(valid_id("io.github.some_one.App-2"));
        assert!(!valid_id("spotify"));
        assert!(!valid_id("org.gnome"));
        assert!(!valid_id("org..Calculator"));
        assert!(!valid_id("1org.gnome.Calculator"));
        assert!(!valid_id("org.gnome.Calc ulator"));
        assert_eq!(id("flathub:org.gnome.Calculator"), Some("org.gnome.Calculator"));
        assert_eq!(id("gnome-calculator"), None);
    }

    #[test]
    fn entries_carry_the_version() {
        let e = entry("org.gnome.Calculator", Some("49.1"));
        assert_eq!((e.attr.as_str(), e.name.as_str()), ("flathub:org.gnome.Calculator", "flathub:org.gnome.Calculator-49.1"));
        assert_eq!(crate::query::version_of(&e.name), "49.1");
        assert_eq!(entry("com.spotify.Client", Some("")).name, "flathub:com.spotify.Client");
        assert_eq!(entry("org.x.Y", Some("2.0 (beta)")).name, "flathub:org.x.Y-2.0");
        assert!(entry("org.x.Y", None).paths.is_empty());
        let listed = vec![entry("org.x.Y", None), Entry { attr: "gimp".into(), name: "gimp-3.0".into(), paths: vec!["/nix/store/x-gimp".into()] }];
        assert_eq!(ids_of(&listed), vec!["org.x.Y".to_string()]);
    }

    #[test]
    fn reads_flatpaks_columns() {
        let rows = columns("org.gnome.Calculator\t49.1\nApplication ID\tVersion\ncom.spotify.Client\t\n\n");
        assert_eq!(rows, vec![vec!["org.gnome.Calculator".to_string(), "49.1".into()], vec!["com.spotify.Client".into(), "".into()]]);
    }

    #[test]
    fn reads_a_search_and_a_page() {
        let answer = serde_json::json!({ "hits": [
            { "app_id": "org.gnome.Calculator", "name": "Calculator", "summary": "Perform calculations", "type": "desktop-application", "main_categories": "utility", "is_free_license": true, "icon": "https://dl.flathub.org/x.png" },
            { "app_id": "org.gnome.Platform", "name": "GNOME", "type": "runtime" },
            { "app_id": "com.spotify.Client", "name": "Spotify", "type": "desktop-application", "is_free_license": false, "main_categories": "audiovideo", "developer_name": "Spotify" },
        ]});
        let hits = read_hits(&answer);
        assert_eq!(hits.iter().map(|h| h.app_id.as_str()).collect::<Vec<_>>(), vec!["org.gnome.Calculator", "com.spotify.Client"]);
        let spotify = item(&hits[1], None, None);
        assert_eq!(spotify.attr, "flathub:com.spotify.Client");
        assert!(spotify.unfree && spotify.app && !spotify.installed);
        assert_eq!(spotify.categories, vec!["audio-video".to_string()]);
        assert_eq!(spotify.source.as_deref(), Some("flathub"));
        assert_eq!(spotify.desktop, vec!["com.spotify.Client.desktop".to_string()]);
        let calc = item(&hits[0], Some("49.1"), None);
        assert!(calc.installed && calc.version == "49.1" && calc.categories == vec!["tools".to_string()]);

        let page = read_page("org.gnome.Calculator", &serde_json::json!({
            "name": "Calculator", "summary": "Sums", "description": "<p>\n  Calculator solves\n  equations.\n</p><ul><li>Basic</li><li>Advanced</li></ul>",
            "urls": { "homepage": "https://apps.gnome.org/Calculator" }, "releases": [{ "version": "49.1" }, { "version": "49.0" }],
            "screenshots": [{ "sizes": [{ "width": "224", "src": "https://s/small.png" }, { "width": "1200", "src": "https://s/big.png" }] }],
        }));
        assert_eq!(page.version.as_deref(), Some("49.1"));
        assert_eq!(page.homepage.as_deref(), Some("https://apps.gnome.org/Calculator"));
        assert_eq!(page.screenshots, vec!["https://s/big.png".to_string()]);
        assert_eq!(page.description.as_deref(), Some("Calculator solves equations.\n\n• Basic\n• Advanced"));
    }

    #[test]
    fn an_arm_computer_finds_what_flathub_has_for_it() {
        let hit = |arches: &[&str]| Hit { app_id: "org.x.Y".into(), arches: arches.iter().map(|a| a.to_string()).collect(), ..Default::default() };
        assert!(runs_here(&hit(&["aarch64", "x86_64"]), true) && runs_here(&hit(&[]), true));
        assert!(!runs_here(&hit(&["x86_64"]), true));
        assert!(runs_here(&hit(&["x86_64"]), false) && runs_here(&hit(&["aarch64"]), false));
    }

    fn catalog(attr: &str, title: &str, desktop: &[&str]) -> crate::query::Item {
        crate::query::Item {
            attr: attr.into(), name: attr.into(), version: String::new(), description: None, homepage: None, license: None, app: true, installed: false,
            program: None, icon: None, unfree: false, builds_here: false, publisher: None, title: Some(title.into()), summary: None, about: None,
            categories: Vec::new(), screenshots: Vec::new(), popularity: None, desktop: desktop.iter().map(|d| d.to_string()).collect(), source: None, unavailable: None,
        }
    }

    #[test]
    fn the_same_app_in_both() {
        let hit = |id: &str, name: &str| Hit { app_id: id.into(), name: name.into(), ..Default::default() };
        // The same menu entry.
        assert!(same_app(&catalog("gnome-calculator", "Calculator", &["org.gnome.Calculator.desktop"]), &hit("org.gnome.Calculator", "Calculator")));
        // The same name, and the package's name in the id.
        assert!(same_app(&catalog("spotify", "Spotify", &["spotify.desktop"]), &hit("com.spotify.Client", "Spotify")));
        assert!(same_app(&catalog("gnome-boxes", "Boxes", &[]), &hit("org.gnome.Boxes", "Boxes")));
        // Another app of the same name.
        assert!(!same_app(&catalog("kcalc", "Calculator", &["org.kde.kcalc.desktop"]), &hit("io.github.someone.Calculator2", "Calculator")));
        assert!(!same_app(&catalog("gimp", "GIMP", &["gimp.desktop"]), &hit("org.inkscape.Inkscape", "Inkscape")));
    }

    #[test]
    fn a_step_takes_off_and_puts_back() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("nami-flathub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (log, fake) = (dir.join("log"), dir.join("flatpak"));
        // Flatpak as far as this goes: what's installed, and what it was asked.
        std::fs::write(&fake, format!("#!/bin/sh\necho \"$*\" >> {log}\ncase \"$1\" in list) printf 'org.x.Gone\\t1.0\\norg.x.Kept\\t2.0\\norg.x.Mine\\t3.0\\n' ;; esac\n", log = log.display())).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("NAMI_FLATPAK", &fake);
        std::env::set_var("XDG_DATA_HOME", dir.join("data"));
        let before = vec![entry("org.x.Gone", Some("1.0")), entry("org.x.Kept", Some("2.0"))];
        let after = vec![entry("org.x.Kept", Some("2.0")), entry("org.x.New", None)];
        // An app that starts by D-Bus: its service, where the session bus looks.
        let exported = dir.join("data/flatpak/exports/share/dbus-1/services");
        std::fs::create_dir_all(&exported).unwrap();
        std::fs::write(exported.join("org.x.Kept.service"), "[D-BUS Service]\n").unwrap();
        std::os::unix::fs::symlink(exported.join("org.x.Gone.service"), dir.join("stale")).unwrap();
        std::fs::create_dir_all(dir.join("data/dbus-1/services")).unwrap();
        std::fs::rename(dir.join("stale"), dir.join("data/dbus-1/services/org.x.Gone.service")).unwrap();
        follow(&before, &after);
        let services = dir.join("data/dbus-1/services");
        assert_eq!(std::fs::read_link(services.join("org.x.Kept.service")).unwrap(), exported.join("org.x.Kept.service"));
        assert!(services.join("org.x.Gone.service").symlink_metadata().is_err());
        let asked = std::fs::read_to_string(&log).unwrap();
        assert!(asked.contains("uninstall --user --noninteractive -y org.x.Gone\n"), "{asked}");
        assert!(asked.contains("remote-add --user --if-not-exists flathub "), "{asked}");
        assert!(asked.contains("install --user --noninteractive -y flathub org.x.New\n"), "{asked}");
        // Only what nami put there: not what's kept, nor what the person installed themselves.
        assert!(!asked.contains("org.x.Kept\n") && !asked.contains("org.x.Mine"), "{asked}");
        // No step that had any: Flatpak isn't asked.
        std::fs::remove_file(&log).unwrap();
        follow(&[], &[Entry { attr: "htop".into(), name: "htop-3".into(), paths: vec![] }]);
        assert!(!log.exists());
        // What nami lists them as, from Flatpak's versions.
        assert_eq!(entries(&["org.x.Kept".into(), "org.x.New".into()]).iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["flathub:org.x.Kept-2.0", "flathub:org.x.New"]);
        std::env::remove_var("NAMI_FLATPAK");
        std::env::remove_var("XDG_DATA_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kinds_of_app() {
        assert_eq!(groups(Some(&serde_json::json!(["Utility", "Calculator", "System"]))), vec!["tools".to_string()]);
        assert_eq!(groups(Some(&serde_json::json!("game"))), vec!["games".to_string()]);
        assert!(groups(None).is_empty());
    }
}
