//! `nami import /`: moving from an old computer. Run on it (Ubuntu or
//! Debian), or on Windows' disk from a nami USB drive, it writes a config
//! repo: system.nami (<hostname>/system.nami: the apps, Wi-Fi
//! networks, hostname, language, keyboard, you) and yours (<login>/user.nami in
//! it: your settings, dotfiles, shell, toolchains). Your files stay out of the repo:
//! `--home` says how to move them, or moves them.
//!
//! Nothing is applied, nothing on the old computer changes, and the repo
//! never gets a secret in plain text: Wi-Fi passwords and tokens are
//! sealed (`!secret`) before anything is written. Run again, it merges
//! (nothing you changed is replaced).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_yaml::{Mapping, Value};

use super::{check_packages, merge, Catalog, Found, Tier};
use crate::index::Index;

/// The repo's two files, in the computer's folder (named after it): the
/// computer's, and yours (in your folder in it).
fn system_file(moved: &Moved) -> String {
    format!("{}/{}", host_of(moved), crate::config::SYSTEM_FILE)
}

fn user_file(moved: &Moved) -> String {
    format!("{}/{}/{}", host_of(moved), moved.person.login, crate::config::USER_FILE)
}

fn host_of(moved: &Moved) -> String {
    moved.system.top.iter().find(|(k, _)| k.len() == 1 && k[0] == "hostname").and_then(|(_, v)| v.as_str().map(String::from)).unwrap_or_else(|| "computer".into())
}


// ---------- what a path is ----------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    /// Debian, Ubuntu and the distros made from them (dpkg).
    Debian,
    /// A Windows partition (C:), as a nami USB drive mounts it.
    Windows,
    /// Someone's files, copied onto a disk by `nami import / --home <it>`.
    Copied,
}

/// The file `--home` leaves in a copy on a disk, saying whose it is.
pub const COPIED: &str = ".nami-home";

/// Is `root` a whole computer: a Linux system's `/`, or Windows' C:?
pub fn what(root: &Path) -> Option<Source> {
    if root.join("var/lib/dpkg/status").is_file() && root.join("etc").is_dir() {
        Some(Source::Debian)
    } else if root.join("Windows/System32").is_dir() && root.join("Users").is_dir() {
        Some(Source::Windows)
    } else if root.join(COPIED).is_file() {
        Some(Source::Copied)
    } else {
        None
    }
}

// ---------- apps: their names on nami ----------

#[derive(Deserialize)]
struct Tables {
    deb: BTreeMap<String, Option<String>>,
    snap: BTreeMap<String, Option<String>>,
    flatpak: BTreeMap<String, Option<String>>,
    windows: Mapping,
}

/// What an app on the old computer is on nami.
#[derive(Debug, PartialEq)]
pub enum Match {
    /// These packages.
    Pkgs(Vec<String>),
    /// Part of the system: nothing to bring.
    Own,
    /// Can't be brought, and why.
    No(String),
    Unknown,
}

pub struct Apps<'a> {
    t: Tables,
    pub catalog: Option<Catalog<'a>>,
    /// A menu entry (`gimp.desktop`) or a program (`rg`) -> the package with it.
    desktops: HashMap<String, String>,
    programs: HashMap<String, String>,
    /// The windows table's phrases, normalized, longest first.
    phrases: Vec<(String, Option<String>)>,
}

/// The better of two attributes for the same thing: a top-level one, then the shortest.
fn better(a: &str, b: &str) -> bool {
    (a.contains('.'), a.len(), a) < (b.contains('.'), b.len(), b)
}

/// Lower case, words only: `Microsoft Visual Studio Code (User)` -> `microsoft visual studio code user`.
fn words(s: &str) -> String {
    s.to_lowercase().split(|c: char| !(c.is_alphanumeric() || "+.-".contains(c))).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

impl<'a> Apps<'a> {
    pub fn new(index: Option<&'a Index>) -> Self {
        let t: Tables = serde_yaml::from_str(include_str!("apps.yaml")).expect("import/apps.yaml");
        let mut phrases: Vec<(String, Option<String>)> = t.windows.iter().filter_map(|(k, v)| Some((words(k.as_str()?), v.as_str().map(String::from)))).collect();
        phrases.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
        let (mut desktops, mut programs) = (HashMap::new(), HashMap::new());
        for (attr, pkg) in index.into_iter().flat_map(|i| i.iter()) {
            let put = |map: &mut HashMap<String, String>, key: &str| {
                let e = map.entry(key.to_string()).or_insert_with(|| attr.to_string());
                if better(attr, e) {
                    *e = attr.to_string();
                }
            };
            for d in pkg.apps.iter().flatten() {
                put(&mut desktops, d);
            }
            if let Some(p) = &pkg.main_program {
                put(&mut programs, p);
            }
        }
        Apps { t, catalog: index.map(Catalog::from_index), desktops, programs, phrases }
    }

    /// For tests: a catalog of these names only.
    #[cfg(test)]
    pub fn with_names(names: &'a [&'a str]) -> Self {
        let mut a = Apps::new(None);
        a.catalog = Some(Catalog::new(names.iter().copied()));
        a
    }

    /// For tests: a package's menu entry or program.
    #[cfg(test)]
    pub fn with_file(mut self, desktop_or_program: &str, attr: &str) -> Self {
        let map = if desktop_or_program.ends_with(".desktop") { &mut self.desktops } else { &mut self.programs };
        map.insert(desktop_or_program.into(), attr.into());
        self
    }

    fn entry(v: &Option<String>) -> Match {
        match v.as_deref().map(str::trim) {
            None | Some("") => Match::Own,
            Some(s) if s.starts_with('-') => Match::No(s.trim_start_matches('-').trim().to_string()),
            Some(s) => Match::Pkgs(s.split_whitespace().map(String::from).collect()),
        }
    }

    /// The catalog's name for `attr`, if it has it.
    fn found(&self, attr: &str) -> Option<String> {
        self.catalog.as_ref()?.resolve(attr)
    }

    fn by_name(&self, name: &str) -> Option<String> {
        let c = self.catalog.as_ref()?;
        self.found(name).or_else(|| c.attr_for_pname(name))
    }

    /// An apt package, with the menu entries and programs it put in /usr.
    pub fn deb(&self, name: &str, desktops: &[String], programs: &[String]) -> Match {
        if let Some(v) = self.t.deb.get(name) {
            return Self::entry(v);
        }
        // Ubuntu's renames for the 64-bit time change (libfoo1t64), and versions in names.
        let base = name.strip_suffix("t64").unwrap_or(name);
        let candidates = [name.to_string(), base.to_string(), base.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-').to_string()];
        if let Some(a) = candidates.iter().filter(|c| !c.is_empty()).find_map(|c| self.by_name(c)) {
            return Match::Pkgs(vec![a]);
        }
        if let Some(a) = desktops.iter().find_map(|d| self.desktops.get(d)).or_else(|| programs.iter().find_map(|p| self.programs.get(p))) {
            return Match::Pkgs(vec![a.clone()]);
        }
        Match::Unknown
    }

    pub fn snap(&self, name: &str) -> Match {
        if let Some(v) = self.t.snap.get(name) {
            return Self::entry(v);
        }
        // The snaps the system runs on.
        let own = ["core", "bare", "snapd", "gtk-common-themes", "gnome-", "kf5-", "kf6-", "mesa-", "gpu-"];
        if own.iter().any(|o| if o.ends_with('-') { name.starts_with(o) } else { name == *o || (name.starts_with(o) && name[o.len()..].chars().all(|c| c.is_ascii_digit())) }) {
            return Match::Own;
        }
        match self.by_name(name).or_else(|| self.desktops.get(&format!("{name}.desktop")).cloned()) {
            Some(a) => Match::Pkgs(vec![a]),
            None => Match::Unknown,
        }
    }

    pub fn flatpak(&self, id: &str) -> Match {
        if let Some(v) = self.t.flatpak.get(id) {
            return Self::entry(v);
        }
        if let Some(a) = self.desktops.get(&format!("{id}.desktop")) {
            return Match::Pkgs(vec![a.clone()]);
        }
        let last = id.rsplit('.').next().unwrap_or(id).to_lowercase();
        match self.found(&last).or_else(|| self.found(&format!("gnome-{last}"))) {
            Some(a) => Match::Pkgs(vec![a]),
            None => Match::Unknown,
        }
    }

    /// A Windows program by its name (`Google Chrome`, `Spotify`).
    pub fn windows(&self, name: &str) -> Match {
        let w = format!(" {} ", words(name));
        match self.phrases.iter().find(|(k, _)| w.contains(&format!(" {k} "))) {
            Some((_, v)) => Self::entry(v),
            None => Match::Unknown,
        }
    }

    /// The package with this menu entry.
    pub fn desktop(&self, file: &str) -> Option<&str> {
        self.desktops.get(file).map(String::as_str)
    }
}

// ---------- the person ----------

#[derive(Debug, Clone)]
pub struct Person {
    pub login: String,
    pub name: Option<String>,
    /// Their home, as this computer sees it (under the root).
    pub home: PathBuf,
    pub shell: Option<String>,
    pub groups: Vec<String>,
    pub admin: bool,
}

// ---------- what was found ----------

/// Everything brought over from the old computer, by file.
pub struct Moved {
    /// "Ubuntu 26.04 LTS", "Windows".
    pub from: String,
    pub person: Person,
    /// system.nami.
    pub system: Found,
    /// The person's file.
    pub user: Found,
    /// What came, a line each (for the summary).
    pub brought: Vec<String>,
    pub notes: Vec<String>,
    /// apt packages with nothing in the catalog: (name, its .deb in apt's cache, if there).
    pub unmatched_debs: Vec<(String, Option<PathBuf>)>,
    pub home: HomePlan,
}

impl Moved {
    pub fn new(from: String, person: Person, home: HomePlan) -> Self {
        Moved { from, person, system: Found::default(), user: Found::default(), brought: Vec::new(), notes: Vec::new(), unmatched_debs: Vec::new(), home }
    }

    pub fn note(&mut self, n: impl Into<String>) {
        let n = n.into();
        if !self.notes.contains(&n) {
            self.notes.push(n);
        }
    }

    /// A line for the summary: `what (n): a, b, c`.
    pub fn brought(&mut self, what: &str, items: &[String]) {
        if items.is_empty() {
            return;
        }
        let shown: Vec<&str> = items.iter().take(12).map(String::as_str).collect();
        let more = if items.len() > 12 { format!(", and {} more", items.len() - 12) } else { String::new() };
        self.brought.push(format!("{what} ({}): {}{more}", items.len(), shown.join(", ")));
    }
}

/// An `os:` key (or any other) for system.nami.
pub fn top(found: &mut Found, path: &[&str], v: Value) {
    found.top.push((path.iter().map(|s| s.to_string()).collect(), v));
}

pub fn secret(plain: &str) -> Value {
    Value::Tagged(Box::new(serde_yaml::value::TaggedValue { tag: serde_yaml::value::Tag::new("secret"), value: Value::String(plain.to_string()) }))
}

/// A name that's for a secret: `GITHUB_TOKEN`, `db_password`, `psk`.
pub fn secret_name(name: &str) -> bool {
    let n = name.to_uppercase();
    ["TOKEN", "SECRET", "PASSWORD", "PASSWD", "PASSPHRASE", "API_KEY", "APIKEY", "ACCESS_KEY", "PRIVATE_KEY", "AUTH", "CREDENTIAL", "PSK"].iter().any(|w| n.contains(w))
}

/// Text with what looks like a secret in it: a token's shape, a private
/// key, a password in a URL or setting.
pub fn secret_text(text: &str) -> bool {
    let prefixes = ["ghp_", "gho_", "ghu_", "ghs_", "github_pat_", "glpat-", "xoxb-", "xoxp-", "sk-", "sk_live_", "sk_test_", "AKIA", "AIza", "npm_", "pypi-", "hf_", "dop_v1_"];
    if text.contains("PRIVATE KEY") {
        return true;
    }
    for w in text.split(|c: char| c.is_whitespace() || "\"'[](),;<>".contains(c)) {
        if w.split(['=', ':']).any(|piece| prefixes.iter().any(|p| piece.starts_with(p) && piece.len() >= p.len() + 12)) {
            return true;
        }
        // user:password@host
        if let Some(rest) = w.split_once("://").map(|(_, r)| r) {
            if rest.split('/').next().is_some_and(|auth| auth.contains('@') && auth.split('@').next().is_some_and(|u| u.contains(':'))) {
                return true;
            }
        }
    }
    text.lines().any(|l| {
        let l = l.trim();
        let Some((k, v)) = l.split_once('=').or_else(|| l.split_once(':')) else { return false };
        let k = k.trim().trim_start_matches("export ").trim();
        let v = v.trim().trim_matches(['"', '\'']);
        secret_name(k) && v.len() >= 6 && !v.starts_with('$') && !v.contains(' ')
    })
}

/// A Wi-Fi network for system.nami, as `wifi:` has it:
/// `home: { password: !secret ... }`. Its password is sealed before
/// anything is written.
pub fn add_wifi(found: &mut Found, name: &str, entry: Mapping) {
    top(found, &["wifi", name], Value::Mapping(entry));
}

/// A `wifi:` entry's settings, from what's known about a network.
#[derive(Default)]
pub struct Wifi {
    pub ssid: Option<String>,
    pub security: Option<&'static str>,
    pub password: Option<String>,
    pub hidden: bool,
    pub autoconnect: bool,
    pub identity: Option<String>,
    pub eap: Option<String>,
    pub phase2: Option<String>,
}

impl Wifi {
    /// The entry, leaving out what `wifi:` takes as its defaults anyway.
    pub fn entry(&self, name: &str) -> Mapping {
        let mut m = Mapping::new();
        if let Some(s) = self.ssid.as_deref().filter(|s| *s != name) {
            m.insert("ssid".into(), s.into());
        }
        // wpa-psk is the default with a password; without one, it must be said.
        if let Some(sec) = self.security.filter(|s| !(*s == "wpa-psk" && self.password.is_some())) {
            m.insert("security".into(), sec.into());
        }
        if let Some(i) = &self.identity {
            m.insert("identity".into(), i.clone().into());
        }
        if let Some(e) = self.eap.as_deref().filter(|e| *e != "peap") {
            m.insert("eap".into(), e.into());
        }
        if let Some(p) = self.phase2.as_deref().filter(|p| *p != "mschapv2") {
            m.insert("phase2".into(), p.into());
        }
        if let Some(p) = &self.password {
            m.insert("password".into(), secret(p));
        }
        if self.hidden {
            m.insert("hidden".into(), true.into());
        }
        if !self.autoconnect {
            m.insert("autoconnect".into(), false.into());
        }
        m
    }
}

// ---------- your files ----------

/// How to move a person's files to the new computer.
#[derive(Debug, Default)]
pub struct HomePlan {
    /// Their home on the old computer (or Windows' C:\Users\<them>).
    pub from: PathBuf,
    pub windows: bool,
    /// Windows: the folders to copy, and where each goes in the new home.
    pub folders: Vec<(String, String)>,
    /// Linux: what stays behind (paths from the home: caches, and files the repo makes).
    pub exclude: Vec<String>,
    /// Data apps keep somewhere else on the old computer (a snap's), and where it goes.
    pub moves: Vec<(String, String)>,
}

/// Something of yours to bring: a folder, or an app's data.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Item {
    /// Its name for --only: `Documents`, `firefox`, `settings`.
    pub key: String,
    pub label: String,
    /// What it brings, for an app's (`bookmarks, history, saved passwords`).
    pub about: Option<String>,
    pub bytes: u64,
    #[serde(skip)]
    copies: Vec<CopyOf>,
}

/// One copy: from the old home, into the new one; rsync's own filters.
#[derive(Debug, Clone)]
struct CopyOf {
    src: String,
    to: String,
    filters: Vec<String>,
}

/// From Windows, beside the folders: apps' data that works on Linux too
/// (key, name, what it brings, where on Windows, where here, rsync's filters).
const WINDOWS_APPS: &[(&str, &str, &str, &str, &str, &[&str])] = &[
    ("firefox", "Firefox", "bookmarks, history, saved passwords", "AppData/Roaming/Mozilla/Firefox", ".mozilla/firefox", &[]),
    ("thunderbird", "Thunderbird", "mail and accounts", "AppData/Roaming/Thunderbird", ".thunderbird", &[]),
    // Their saved passwords are Windows' (DPAPI): they can't come.
    ("chrome", "Chrome", "bookmarks and history", "AppData/Local/Google/Chrome/User Data/Default", ".config/google-chrome/Default", &["--include=Bookmarks", "--include=History", "--include=Favicons", "--exclude=*"]),
    ("edge", "Edge", "bookmarks and history", "AppData/Local/Microsoft/Edge/User Data/Default", ".config/microsoft-edge/Default", &["--include=Bookmarks", "--include=History", "--include=Favicons", "--exclude=*"]),
    ("ssh", "SSH keys", "", ".ssh", ".ssh", &[]),
];

const RSYNC_LINUX: &str = "rsync -aHAX --info=progress2";
const RSYNC_WINDOWS: &str = "rsync -rt --info=progress2 --exclude=desktop.ini --exclude=Thumbs.db";

fn quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-~+:@=,".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

impl HomePlan {
    /// What there is to bring, with sizes: Windows' folders and apps' data;
    /// a Linux home's folders, and its settings and app data (dot files) as one.
    pub fn items(&self) -> Vec<Item> {
        let mut out: Vec<Item> = Vec::new();
        if self.windows {
            for (src, to) in &self.folders {
                let bytes = size_of(&self.from.join(src), &[]);
                let copy = CopyOf { src: src.clone(), to: to.clone(), filters: Vec::new() };
                match out.iter_mut().find(|i| i.key == *to) {
                    Some(i) => {
                        i.bytes += bytes;
                        i.copies.push(copy);
                    }
                    None => out.push(Item { key: to.clone(), label: to.clone(), about: None, bytes, copies: vec![copy] }),
                }
            }
            for (key, label, about, src, to, filters) in WINDOWS_APPS {
                let dir = self.from.join(src);
                if !dir.is_dir() {
                    continue;
                }
                let bytes = if filters.is_empty() {
                    size_of(&dir, &[])
                } else {
                    filters.iter().filter_map(|f| f.strip_prefix("--include=")).map(|f| size_of(&dir.join(f), &[])).sum()
                };
                let copy = CopyOf { src: src.to_string(), to: to.to_string(), filters: filters.iter().map(|f| f.to_string()).collect() };
                out.push(Item { key: key.to_string(), label: label.to_string(), about: Some(about.to_string()).filter(|a| !a.is_empty()), bytes, copies: vec![copy] });
            }
        } else {
            let mut names: Vec<(String, bool)> = std::fs::read_dir(&self.from)
                .map(|rd| rd.flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), e.file_type().is_ok_and(|t| t.is_dir()))).collect())
                .unwrap_or_default();
            names.sort();
            let (mut loose, mut dots) = (0, 0);
            for (name, dir) in &names {
                if self.exclude.contains(name) || name == COPIED {
                    continue;
                }
                let excluded: Vec<&str> = self.exclude.iter().filter_map(|x| x.strip_prefix(&format!("{name}/"))).collect();
                let n = size_of(&self.from.join(name), &excluded);
                if name.starts_with('.') {
                    dots += n;
                } else if *dir {
                    out.push(Item { key: name.clone(), label: name.clone(), about: None, bytes: n, copies: Vec::new() });
                } else {
                    loose += n;
                }
            }
            if loose > 0 {
                out.push(Item { key: "files".into(), label: "Files in your home".into(), about: None, bytes: loose, copies: Vec::new() });
            }
            let moved: u64 = self.moves.iter().map(|(src, _)| size_of(&self.from.join(src), &[])).sum();
            out.push(Item { key: "settings".into(), label: "Settings and app data".into(), about: Some("browsers, mail, keys, apps' own (the dot folders)".into()), bytes: dots + moved, copies: Vec::new() });
        }
        out
    }

    /// The commands that copy the files into `dest` (a folder, or host:folder).
    pub fn commands(&self, dest: &str) -> Vec<Vec<String>> {
        self.commands_of(dest, None)
    }

    /// The same, for the items in `only` (all: None).
    pub fn commands_of(&self, dest: &str, only: Option<&[String]>) -> Vec<Vec<String>> {
        let picked = |k: &str| only.is_none_or(|o| o.iter().any(|x| x == k));
        let dest = dest.trim_end_matches('/');
        let from = self.from.display().to_string();
        let split = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        if self.windows {
            let mut out = Vec::new();
            for item in self.items().into_iter().filter(|i| picked(&i.key)) {
                for c in &item.copies {
                    let mut cmd = split(RSYNC_WINDOWS);
                    cmd.extend(c.filters.iter().cloned());
                    cmd.push(format!("{from}/{}/", c.src));
                    cmd.push(format!("{dest}/{}/", c.to));
                    out.push(cmd);
                }
            }
            return out;
        }
        let mut out = Vec::new();
        let mut c = split(RSYNC_LINUX);
        c.extend(self.exclude.iter().map(|e| format!("--exclude=/{e}")));
        if self.from.join(COPIED).exists() {
            c.push(format!("--exclude=/{COPIED}"));
        }
        if only.is_some() {
            for item in self.items().iter().filter(|i| !picked(&i.key)) {
                match item.key.as_str() {
                    "settings" => c.push("--exclude=/.*".into()),
                    "files" => {
                        // The loose files: everything at the top that isn't a folder.
                        if let Ok(rd) = std::fs::read_dir(&self.from) {
                            for e in rd.flatten().filter(|e| e.file_type().is_ok_and(|t| !t.is_dir())) {
                                c.push(format!("--exclude=/{}", e.file_name().to_string_lossy()));
                            }
                        }
                    }
                    k => c.push(format!("--exclude=/{k}")),
                }
            }
        }
        c.push(format!("{from}/"));
        c.push(format!("{dest}/"));
        out.push(c);
        if picked("settings") {
            for (src, to) in &self.moves {
                let mut c = split(RSYNC_LINUX);
                c.push(format!("{from}/{src}/"));
                c.push(format!("{dest}/{to}/"));
                out.push(c);
            }
        }
        out
    }

    /// The plan as JSON, for the window.
    pub fn json(&self, kind: &str, person: Option<&str>, people: &[String]) -> serde_json::Value {
        serde_json::json!({ "from": self.from, "kind": kind, "person": person, "people": people, "items": self.items() })
    }

    /// After copying: an app's folder made at home here. Firefox's and
    /// Thunderbird's profiles were the Windows install's (they'd start a
    /// new, empty one); SSH wants its keys yours alone.
    fn settle(&self, dest: &Path, only: Option<&[String]>) {
        let picked = |k: &str| only.is_none_or(|o| o.iter().any(|x| x == k));
        if !self.windows {
            return;
        }
        for dir in [(picked("firefox"), ".mozilla/firefox"), (picked("thunderbird"), ".thunderbird")].into_iter().filter(|(p, _)| *p).map(|(_, d)| dest.join(d)) {
            let ini = dir.join("profiles.ini");
            if let Ok(text) = std::fs::read_to_string(&ini) {
                let mut keep = true;
                let kept: String = text.lines().filter(|l| {
                    if l.starts_with('[') {
                        keep = !l.starts_with("[Install");
                    }
                    keep
                }).map(|l| format!("{l}\n")).collect();
                let _ = std::fs::write(&ini, kept);
                let _ = std::fs::remove_file(dir.join("installs.ini"));
            }
        }
        let ssh = dest.join(".ssh");
        if picked("ssh") && ssh.is_dir() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700));
            for e in std::fs::read_dir(&ssh).into_iter().flatten().flatten() {
                let pubkey = e.file_name().to_string_lossy().ends_with(".pub");
                let _ = std::fs::set_permissions(e.path(), std::fs::Permissions::from_mode(if pubkey { 0o644 } else { 0o600 }));
            }
        }
    }

    /// The plan, to read: what there is, and the commands.
    pub fn describe(&self) -> String {
        let mut out = String::new();
        let mut sizes: Vec<(String, u64)> = Vec::new();
        if self.windows {
            for (src, _) in &self.folders {
                sizes.push((src.clone(), size_of(&self.from.join(src), &[])));
            }
        } else if let Ok(rd) = std::fs::read_dir(&self.from) {
            let mut dots = 0;
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if self.exclude.contains(&name) {
                    continue;
                }
                let excluded: Vec<&str> = self.exclude.iter().filter_map(|x| x.strip_prefix(&format!("{name}/"))).collect();
                let n = size_of(&e.path(), &excluded);
                if name.starts_with('.') {
                    dots += n;
                } else {
                    sizes.push((name, n));
                }
            }
            sizes.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
            sizes.push(("settings and app data (dot folders)".into(), dots));
        }
        let total: u64 = sizes.iter().map(|(_, n)| n).sum();
        out += &format!("Your files in {} ({}), not in the repo:\n", self.from.display(), human(total));
        for (name, n) in sizes.iter().filter(|(_, n)| *n > 0).take(14) {
            out += &format!("  {name}: {}\n", human(*n));
        }
        let cmd = |c: &Vec<String>| c.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ");
        if self.windows {
            out += "\nOn the new computer (from a nami USB drive, with Windows' disk mounted here), copy them into your home:\n";
            for c in self.commands("~") {
                out += &format!("  {}\n", cmd(&c));
            }
            out += &format!("\nOr let nami do it: nami import {} --home ~\n", self.from.parent().and_then(Path::parent).unwrap_or(Path::new("/")).display());
        } else {
            out += "\nCopy them onto a disk (mounted at /media/you/disk, say; leave out what's listed with --exclude):\n";
            for c in self.commands("/media/you/disk/home") {
                out += &format!("  {}\n", cmd(&c));
            }
            out += &format!("\nThen on the new computer, after your first login:\n  {RSYNC_LINUX} /media/you/disk/home/ ~/\n");
            out += "(or straight there over the network: give `you@new-computer:` instead of the disk)\n";
            out += "Or let nami do the first step: nami import / --home /media/you/disk/home\n";
            if !self.exclude.is_empty() {
                out += &format!("Left behind: {} (caches, and the files your repo makes).\n", self.exclude.join(", "));
            }
            out += "Big folders of what can be made again (node_modules, target, build) can stay behind too: add an --exclude=<name> for each.\n";
        }
        out
    }

    /// Copy the files into `dest`, with rsync (the items in `only`; all: None).
    /// Onto a disk (not your home), it leaves a note saying whose they
    /// are, so the new computer's `nami import <there> --home ~` knows them.
    pub fn copy(&self, dest: &str, only: Option<&[String]>, whose: Option<&str>) -> Result<()> {
        ensure!(Command::new("rsync").arg("--version").output().is_ok_and(|o| o.status.success()), "moving your files needs rsync: `nami install rsync`, or apt install rsync");
        if !dest.contains(':') {
            std::fs::create_dir_all(dest).with_context(|| format!("creating {dest}"))?;
        }
        for c in self.commands_of(dest, only) {
            let src = &c[c.len() - 2];
            if !Path::new(src).exists() {
                continue;
            }
            // (rsync makes only the last folder of where it copies to.)
            if !dest.contains(':') {
                std::fs::create_dir_all(&c[c.len() - 1]).with_context(|| format!("creating {}", c[c.len() - 1]))?;
            }
            eprintln!("{}", c.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" "));
            let st = Command::new(&c[0]).args(&c[1..]).status().with_context(|| format!("running {}", c[0]))?;
            // 23/24: some files couldn't be read, or vanished while copying: said above, the rest is there.
            ensure!(st.success() || matches!(st.code(), Some(23 | 24)), "rsync stopped ({st}); run it again to go on where it stopped");
        }
        if !dest.contains(':') {
            let here = Path::new(dest);
            self.settle(here, only);
            let home = std::env::var_os("HOME").map(PathBuf::from);
            if let Some(whose) = whose.filter(|_| home.as_deref().and_then(|h| h.canonicalize().ok()) != here.canonicalize().ok() && !self.windows) {
                std::fs::write(here.join(COPIED), format!("{whose}\n"))?;
            }
        }
        eprintln!("your files are in {dest}");
        Ok(())
    }
}

/// How many bytes are in a folder (not following links), leaving out
/// paths below it.
fn size_of(path: &Path, exclude: &[&str]) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(path) else { return 0 };
    if !m.is_dir() {
        return m.len();
    }
    let Ok(rd) = std::fs::read_dir(path) else { return 0 };
    rd.flatten()
        .filter(|e| !exclude.contains(&e.file_name().to_string_lossy().as_ref()))
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let deeper: Vec<&str> = exclude.iter().filter_map(|x| x.strip_prefix(&format!("{name}/"))).collect();
            size_of(&e.path(), &deeper)
        })
        .sum()
}

fn human(n: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1000.0 && u < units.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", units[u])
    }
}

// ---------- the command ----------

pub struct Options {
    /// `/`, an old disk mounted somewhere, or Windows' C:.
    pub root: PathBuf,
    /// The repo to write (default ~/nami-config).
    pub out: Option<PathBuf>,
    /// Whose settings, when there are several people.
    pub user: Option<String>,
    /// The repo's address, for `os.users.<you>.config`.
    pub repo: Option<String>,
    /// Write recipes for apt packages the catalog doesn't have.
    pub recipes: bool,
    /// Only the plan for your files (Some("")), or copy them there.
    pub home: Option<String>,
    /// Of your files, only these (an item's key: `Documents`, `firefox`).
    pub only: Option<Vec<String>>,
    /// The plan as JSON (with --home), for the window.
    pub json: bool,
    pub dry_run: bool,
}

/// Makes a recipe from an apt package (a name in Ubuntu's archive, or a
/// .deb file) in a folder: `nami recipe from-deb`.
pub type RecipeMaker<'a> = &'a dyn Fn(&str, &Path) -> Result<()>;

/// Who runs this: under sudo, the person who ran sudo (the repo and the
/// secrets' key are theirs).
struct Invoker {
    login: Option<String>,
    home: PathBuf,
    ids: Option<(u32, u32)>,
}

fn invoker() -> Invoker {
    let sudo = crate::profile::is_root().then(|| std::env::var("SUDO_USER").ok()).flatten().filter(|u| !u.is_empty() && u != "root");
    if let Some(u) = sudo {
        if let Some((home, uid, gid)) = passwd_entry(Path::new("/"), &u) {
            return Invoker { login: Some(u), home, ids: Some((uid, gid)) };
        }
    }
    Invoker { login: None, home: PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into())), ids: None }
}

/// A login's home, uid and gid, from the root's /etc/passwd.
pub fn passwd_entry(root: &Path, login: &str) -> Option<(PathBuf, u32, u32)> {
    let text = std::fs::read_to_string(root.join("etc/passwd")).ok()?;
    let f: Vec<String> = text.lines().map(|l| l.split(':').map(String::from).collect::<Vec<_>>()).find(|f| f.len() >= 7 && f[0] == login)?;
    Some((PathBuf::from(&f[5]), f[2].parse().ok()?, f[3].parse().ok()?))
}

pub fn run(opts: Options, index: Option<&Index>, recipe: Option<RecipeMaker>) -> Result<()> {
    let root = std::path::absolute(&opts.root)?;
    let source = what(&root).with_context(|| format!("{} isn't a computer nami can move from (a Debian or Ubuntu system's /, or Windows' C:)", root.display()))?;
    let apps = Apps::new(index);
    let me = invoker();
    // On this computer, under sudo: the person who ran it.
    let user = opts.user.as_deref().or(if root == Path::new("/") { me.login.as_deref() } else { None });
    // A copy of someone's files on a disk: only to bring here.
    if source == Source::Copied {
        let whose = std::fs::read_to_string(root.join(COPIED)).unwrap_or_default().trim().to_string();
        let plan = HomePlan { from: root.clone(), ..Default::default() };
        return match opts.home.as_deref() {
            None => anyhow::bail!("{} is {whose}'s files, copied: `nami import {} --home ~` brings them here", root.display(), root.display()),
            Some("") if opts.json => Ok(println!("{}", plan.json("copied", Some(&whose), std::slice::from_ref(&whose)))),
            Some("") => Ok(print!("{}", plan.describe())),
            Some(dest) => plan.copy(dest, opts.only.as_deref(), None),
        };
    }
    // The window asks before it knows whose: the people there, first.
    if opts.home.as_deref() == Some("") && opts.json && user.is_none() {
        let people = match source {
            Source::Windows => super::windows::people(&root)?,
            _ => super::debian::people(&root),
        };
        if people.len() != 1 {
            println!("{}", serde_json::json!({ "from": root, "kind": if source == Source::Windows { "windows" } else { "linux" }, "person": null, "people": people, "items": [] }));
            return Ok(());
        }
    }
    let mut moved = match source {
        Source::Debian => super::debian::read(&root, user, &apps)?,
        Source::Windows => super::windows::read(&root, user, &apps)?,
        Source::Copied => unreachable!(),
    };
    if let Some(dest) = &opts.home {
        let whose = moved.person.login.clone();
        if dest.is_empty() && opts.json {
            let people = match source {
                Source::Windows => super::windows::people(&root)?,
                _ => super::debian::people(&root),
            };
            let shown = if source == Source::Windows { moved.home.from.file_name().map(|n| n.to_string_lossy().into_owned()) } else { Some(whose) };
            println!("{}", moved.home.json(if source == Source::Windows { "windows" } else { "linux" }, shown.as_deref(), &people));
            return Ok(());
        }
        if dest.is_empty() {
            print!("{}", moved.home.describe());
            return Ok(());
        }
        return moved.home.copy(dest, opts.only.as_deref(), Some(&whose));
    }
    if apps.catalog.is_none() {
        moved.note("no catalog on this computer, so only nami's table of apps was used: `nami update` (or install nami), then import again, finds the rest");
    }
    let out = std::path::absolute(opts.out.clone().unwrap_or_else(|| me.home.join("nami-config")))?;
    finish(&mut moved, &root, &out, opts.repo.as_deref(), opts.recipes.then_some(recipe).flatten(), apps.catalog.as_ref(), opts.recipes);
    let (system_text, user_text) = texts(&moved, &out)?;
    if opts.dry_run {
        println!("# {}\n{}", out.join(system_file(&moved)).display(), crate::secrets::hide_plain(&system_text.0)?);
        println!("# {}\n{}", out.join(user_file(&moved)).display(), crate::secrets::hide_plain(&user_text.0)?);
        summary(&moved, &out, &system_text.1, &user_text.1, &root, true);
        return Ok(());
    }
    // Secrets are sealed (for whoever ran this, under sudo too) before anything's written.
    let system_file = system_file(&moved);
    std::fs::create_dir_all(out.join(host_of(&moved))).with_context(|| format!("creating {}", out.display()))?;
    let sealed = as_invoker(&me, || -> Result<(String, String, usize)> {
        // One .nami-recipients, at the repo's top, for both files.
        let plain = |t: &str| crate::secrets::find(t).map(|f| f.iter().any(|s| !s.sealed));
        if plain(&system_text.0)? || plain(&user_text.0)? {
            crate::secrets::recipients_for(&out)?;
        }
        let (s, n1) = crate::secrets::seal_new_text(&system_text.0, &out.join(&system_file))?;
        let (u, n2) = crate::secrets::seal_new_text(&user_text.0, &out.join(user_file(&moved)))?;
        Ok((s, u, n1 + n2))
    })?;
    write_new(&out.join(&system_file), &sealed.0)?;
    write_new(&out.join(user_file(&moved)), &sealed.1)?;
    crate::new::copy_packages(&out.join(host_of(&moved)), &moved.system.includes)?;
    crate::machine::ignore_own_files(&out);
    if let Some((uid, gid)) = me.ids {
        // Written as root: theirs, like the rest of their home.
        chown_tree(&out, uid, gid);
        let key_dir = me.home.join(".config/nami");
        for p in [key_dir.join("age.key"), key_dir.join(".gitignore"), key_dir.clone()] {
            if std::fs::symlink_metadata(&p).is_ok_and(|m| std::os::unix::fs::MetadataExt::uid(&m) == 0) {
                let _ = std::os::unix::fs::lchown(&p, Some(uid), Some(gid));
            }
        }
    }
    if sealed.2 > 0 {
        moved.brought.push(format!("secrets ({}): sealed, for the people in {}", sealed.2, out.join(crate::secrets::RECIPIENTS).display()));
    }
    summary(&moved, &out, &system_text.1, &user_text.1, &root, false);
    Ok(())
}

/// Run `f` with HOME and USER set to the person who ran sudo, so their key seals the secrets.
fn as_invoker<T>(me: &Invoker, f: impl FnOnce() -> T) -> T {
    let Some(login) = &me.login else { return f() };
    let saved: Vec<(&str, Option<std::ffi::OsString>)> = ["HOME", "USER", "XDG_CONFIG_HOME"].iter().map(|k| (*k, std::env::var_os(k))).collect();
    std::env::set_var("HOME", &me.home);
    std::env::set_var("USER", login);
    std::env::remove_var("XDG_CONFIG_HOME");
    let r = f();
    for (k, v) in saved {
        match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
    r
}

fn chown_tree(path: &Path, uid: u32, gid: u32) {
    let _ = std::os::unix::fs::lchown(path, Some(uid), Some(gid));
    if path.is_dir() && !path.is_symlink() {
        for e in std::fs::read_dir(path).into_iter().flatten().flatten() {
            chown_tree(&e.path(), uid, gid);
        }
    }
}

fn write_new(file: &Path, text: &str) -> Result<()> {
    if std::fs::read_to_string(file).ok().as_deref() == Some(text) {
        return Ok(());
    }
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d)?;
    }
    crate::store::write_atomic(file, text.as_bytes()).with_context(|| format!("writing {}", file.display()))
}

/// Package names checked against the catalog, recipes for the rest, the
/// person's entry in system.nami.
fn finish(moved: &mut Moved, _root: &Path, out: &Path, repo: Option<&str>, recipe: Option<RecipeMaker>, catalog: Option<&Catalog>, asked_recipes: bool) {
    check_packages(&mut moved.system, catalog);
    check_packages(&mut moved.user, catalog);
    // The computer's folder is named after it, so it has a name.
    if !moved.system.top.iter().any(|(k, _)| k.len() == 1 && k[0] == "hostname") {
        top(&mut moved.system, &["hostname"], "computer".into());
    }
    let host = host_of(moved);
    // apt packages with no match: a recipe from Ubuntu's package, if asked.
    let mut made = Vec::new();
    for (name, deb) in std::mem::take(&mut moved.unmatched_debs) {
        let dir = out.join(&host).join("packages").join(&name);
        let rel = format!("./packages/{name}");
        if crate::oldnames::is_file(&dir.join("package.nami")) {
            moved.system.package(&rel, "your recipe");
            continue;
        }
        let Some(make) = recipe else {
            let hint = if asked_recipes { "" } else { " (`--recipes` writes a recipe for it from Ubuntu's package)" };
            moved.system.skip(format!("apt package {name}"), format!("not in the catalog{hint}"));
            continue;
        };
        let what = deb.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| name.clone());
        match make(&what, &dir) {
            Ok(()) => {
                moved.system.package(&rel, "a recipe from its .deb");
                made.push(name);
            }
            Err(e) => moved.system.skip(format!("apt package {name}"), format!("not in the catalog, and no recipe could be made: {e:#}")),
        }
    }
    if !made.is_empty() {
        moved.brought(&format!("recipes from .deb packages, in {}", out.join(&host).join("packages").display()), &made);
        moved.note(&format!("check the recipes (NOT FOUND marks what they still need), then `nami build {host}/packages/<name>`"));
    }
    // You, on the new computer: your config repo applied at your first login.
    let repo = repo.map(String::from).or_else(|| git_remote(out)).filter(|r| crate::machine::is_repo(r));
    let p = moved.person.clone();
    let mut me = Mapping::new();
    me.insert("admin".into(), Value::Bool(p.admin));
    if let Some(n) = &p.name {
        me.insert("name".into(), n.clone().into());
    }
    // The USB drive is made from the repo: each person's <login>/user.nami comes at their first login.
    moved.note(match &repo {
        Some(r) => format!("after you push the repo, `nami usb {r} --to /dev/sdX` makes the new computer; your {}/user.nami comes at your first login", p.login),
        None => format!("after you push the repo, `nami usb <its address> --to /dev/sdX` (or this folder, once it's a clone of it) makes the new computer; your {}/user.nami comes at your first login", p.login),
    });
    top(&mut moved.system, &["users", &p.login], Value::Mapping(me));
    // Docker and the like: their nami file, its people and settings.
    super::features(&mut moved.system, true);
}

fn git_remote(dir: &Path) -> Option<String> {
    let o = Command::new("git").arg("-C").arg(dir).args(["remote", "get-url", "origin"]).output().ok()?;
    let url = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (o.status.success() && !url.is_empty()).then_some(url)
}

/// The two files' new text (merged into what's there), and what was added to each.
type Text = (String, Vec<String>);

fn texts(moved: &Moved, out: &Path) -> Result<(Text, Text)> {
    let from = moved.from.clone();
    let old = |f: &str| std::fs::read_to_string(out.join(f)).unwrap_or_default();
    let (s, s_added, _) = merge(&old(&system_file(moved)), &moved.system, Tier::System, &from)?;
    let (u, u_added, _) = merge(&old(&user_file(moved)), &moved.user, Tier::User, &from)?;
    Ok(((s, s_added), (u, u_added)))
}

fn summary(moved: &Moved, out: &Path, system_added: &[String], user_added: &[String], root: &Path, dry_run: bool) {
    let mut o = String::new();
    let verb = if dry_run { "would write" } else { "wrote" };
    o += &format!("Moving from {} ({}) for {}:\n", moved.from, root.display(), moved.person.login);
    if system_added.is_empty() && user_added.is_empty() {
        o += &format!("nothing new for {}: it has all of it already\n", out.display());
    }
    if !moved.brought.is_empty() {
        o += "brought over:\n";
        for b in &moved.brought {
            o += &format!("  + {b}\n");
        }
    }
    let not: Vec<_> = moved.system.unconverted.iter().chain(&moved.user.unconverted).collect();
    if !not.is_empty() {
        o += &format!("not brought over ({}, also listed at the end of the files):\n", not.len());
        for u in not {
            o += &format!("  - {}: {}\n", u.what, u.why);
        }
    }
    for n in moved.notes.iter().chain(&moved.system.warnings).chain(&moved.user.warnings) {
        o += &format!("note: {n}\n");
    }
    let (s, u) = (out.join(system_file(moved)), out.join(user_file(moved)));
    o += &format!("{verb} {} ({} new) and {} ({} new)\n", s.display(), system_added.len(), u.display(), user_added.len());
    let here = if root == Path::new("/") { "/".to_string() } else { root.display().to_string() };
    o += "next:\n";
    o += &format!("  1. read the two files; change what you like (`nami check {}` finds mistakes)\n", s.display());
    o += &format!("  2. make it a repo: cd {} && git init && git add -A && git commit -m \"my computer\", and push it (secrets stay sealed)\n", out.display());
    o += &format!("  3. a USB drive for the new computer: nami usb {} --to /dev/sdX (or `nami apply` it on a computer that has nami)\n", out.display());
    o += &format!("  4. your files: `nami import {here} --home` shows how to move them\n");
    if dry_run {
        eprint!("{o}");
    } else {
        print!("{o}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_by_table_name_menu_entry_and_program() {
        let names = ["vscode", "gimp", "ripgrep", "btop", "qemu", "google-chrome", "gnome-boxes", "spotify", "python313Packages.pip"];
        let apps = Apps::with_names(&names);
        assert_eq!(apps.deb("code", &[], &[]), Match::Pkgs(vec!["vscode".into()]));
        assert_eq!(apps.deb("btop", &[], &[]), Match::Pkgs(vec!["btop".into()]));
        assert_eq!(apps.deb("snapd", &[], &[]), Match::Own);
        assert!(matches!(apps.deb("openssh-server", &[], &[]), Match::No(_)));
        assert_eq!(apps.deb("python3-pip", &[], &[]), Match::Pkgs(vec!["python3Packages.pip".into()]));
        assert_eq!(apps.deb("mytool", &[], &[]), Match::Unknown);
        assert_eq!(apps.snap("core22"), Match::Own);
        assert_eq!(apps.snap("gnome-42-2204"), Match::Own);
        assert_eq!(apps.snap("spotify"), Match::Pkgs(vec!["spotify".into()]));
        assert_eq!(apps.snap("code"), Match::Pkgs(vec!["vscode".into()]));
        assert_eq!(apps.flatpak("com.spotify.Client"), Match::Pkgs(vec!["spotify".into()]));
        assert_eq!(apps.flatpak("org.gnome.Boxes"), Match::Pkgs(vec!["gnome-boxes".into()]));
        assert_eq!(apps.windows("Google Chrome"), Match::Pkgs(vec!["google-chrome".into()]));
        assert_eq!(apps.windows("Microsoft Visual Studio Code (User)"), Match::Pkgs(vec!["vscode".into()]));
        assert_eq!(apps.windows("Microsoft Edge"), Match::Pkgs(vec!["microsoft-edge".into()]));
        assert_eq!(apps.windows("Microsoft Teams classic"), Match::Pkgs(vec!["teams-for-linux".into()]));
        assert_eq!(apps.windows("GitHub Desktop"), Match::Pkgs(vec!["github-desktop".into()]));
        assert_eq!(apps.windows("Microsoft Update Health Tools"), Match::Own);
        assert!(matches!(apps.windows("Rhino 8"), Match::No(_)));
        assert_eq!(apps.windows("Some Tool 3"), Match::Unknown);
    }

    #[test]
    fn secrets_are_spotted() {
        assert!(secret_name("GITHUB_TOKEN") && secret_name("db_password") && !secret_name("EDITOR"));
        assert!(secret_text("export OPENAI_API_KEY=sk-000000000000000000000000"));
        assert!(secret_text("[url \"https://me:hunter22@example.org/\"]"));
        assert!(secret_text("token = ghp_000000000000000000000000000000"));
        assert!(secret_text("//registry.npmjs.org/:_authToken=npm_0000000000000000"));
        assert!(!secret_text("[user]\n  name = Alice\n  email = alice@example.org\n[credential]\n  helper = store\n"));
        assert!(!secret_text("export PATH=\"$HOME/.local/bin:$PATH\""));
    }

    #[test]
    fn home_commands() {
        let p = HomePlan { from: "/home/alice".into(), exclude: vec![".cache".into(), ".bashrc".into()], moves: vec![("snap/firefox/common/.mozilla".into(), ".mozilla".into())], ..Default::default() };
        let c = p.commands("/media/d/home/");
        assert_eq!(c[0].join(" "), "rsync -aHAX --info=progress2 --exclude=/.cache --exclude=/.bashrc /home/alice/ /media/d/home/");
        assert_eq!(c[1].last().unwrap(), "/media/d/home/.mozilla/");
        let w = HomePlan { from: "/mnt/c/Users/adab".into(), windows: true, folders: vec![("Documents".into(), "Documents".into())], ..Default::default() };
        assert_eq!(w.commands("~")[0].join(" "), "rsync -rt --info=progress2 --exclude=desktop.ini --exclude=Thumbs.db /mnt/c/Users/adab/Documents/ ~/Documents/");
    }

    /// Your files, item by item: Windows' folders (OneDrive's with them)
    /// and apps' data; --only copies some; Firefox's profile comes unlocked
    /// from Windows' install, SSH keys yours alone.
    #[test]
    fn your_files_item_by_item() {
        let t = std::env::temp_dir().join(format!("nami-home-items-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let u = t.join("Users/ada");
        for d in ["Documents", "OneDrive/Documents", "Pictures", "AppData/Roaming/Mozilla/Firefox", "AppData/Local/Google/Chrome/User Data/Default/Cache", ".ssh"] {
            std::fs::create_dir_all(u.join(d)).unwrap();
        }
        std::fs::write(u.join("Documents/a"), "aa").unwrap();
        std::fs::write(u.join("OneDrive/Documents/b"), "bbb").unwrap();
        std::fs::write(u.join("AppData/Local/Google/Chrome/User Data/Default/Bookmarks"), "{}").unwrap();
        std::fs::write(u.join("AppData/Local/Google/Chrome/User Data/Default/Cache/big"), "x".repeat(1000)).unwrap();
        std::fs::write(u.join("AppData/Roaming/Mozilla/Firefox/profiles.ini"), "[Install123]\nDefault=Profiles/x\nLocked=1\n\n[Profile0]\nPath=Profiles/x\n").unwrap();
        std::fs::write(u.join(".ssh/id_ed25519"), "k").unwrap();
        let folders = [("Documents", "Documents"), ("OneDrive/Documents", "Documents"), ("Pictures", "Pictures")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let w = HomePlan { from: u.clone(), windows: true, folders, ..Default::default() };
        let items: Vec<(String, u64)> = w.items().into_iter().map(|i| (i.key, i.bytes)).collect();
        assert_eq!(items, [("Documents".into(), 5), ("Pictures".into(), 0), ("firefox".into(), 69), ("chrome".into(), 2), ("ssh".into(), 1)]);
        let only = ["Documents".to_string(), "chrome".to_string()];
        let cmds: Vec<String> = w.commands_of("/new", Some(&only)).iter().map(|c| c.join(" ")).collect();
        assert_eq!(cmds.len(), 3, "{cmds:?}");
        assert!(cmds[1].ends_with("/OneDrive/Documents/ /new/Documents/"), "{cmds:?}");
        assert!(cmds[2].contains("--include=Bookmarks") && cmds[2].ends_with("/Default/ /new/.config/google-chrome/Default/"), "{cmds:?}");
        // As copied: Firefox's profile no longer the Windows install's.
        let new = t.join("new");
        std::fs::create_dir_all(new.join(".mozilla/firefox")).unwrap();
        std::fs::create_dir_all(new.join(".ssh")).unwrap();
        std::fs::copy(u.join("AppData/Roaming/Mozilla/Firefox/profiles.ini"), new.join(".mozilla/firefox/profiles.ini")).unwrap();
        std::fs::write(new.join(".ssh/id_ed25519"), "k").unwrap();
        w.settle(&new, None);
        assert_eq!(std::fs::read_to_string(new.join(".mozilla/firefox/profiles.ini")).unwrap(), "[Profile0]\nPath=Profiles/x\n");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(new.join(".ssh/id_ed25519")).unwrap().permissions().mode() & 0o777, 0o600);
        // A Linux home: its folders, loose files, and the dot ones as one; --only leaves the others out.
        let h = t.join("home/alice");
        std::fs::create_dir_all(h.join("Music")).unwrap();
        std::fs::create_dir_all(h.join(".config")).unwrap();
        std::fs::write(h.join("notes"), "n").unwrap();
        let l = HomePlan { from: h, ..Default::default() };
        let keys: Vec<String> = l.items().into_iter().map(|i| i.key).collect();
        assert_eq!(keys, ["Music", "files", "settings"]);
        let c = l.commands_of("/new", Some(&["Music".to_string()])).remove(0).join(" ");
        assert!(c.contains("--exclude=/notes") && c.contains("--exclude=/.*") && !c.contains("--exclude=/Music"), "{c}");
        std::fs::remove_dir_all(&t).unwrap();
    }
}
