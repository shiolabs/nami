//! nami file: its format, its documentation (`nami options`), a linter
//! (`nami check`) and the reverse direction (`nami export`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_yaml::Value;

use crate::home;
use crate::index::Index;
use crate::profile::Manifest;
use crate::secrets::Kind;
use crate::store::Cache;

/// A sealed secret for the examples (`nami secret seal` writes real ones, longer).
macro_rules! sealed_example {
    () => {
        "YWdlLWVuY3J5cHRpb24ub3JnL3YxCi0+IFgyNTUxOSB0cmF5LWV4YW1wbGUK"
    };
}

// (After sealed_example!, which options.rs and the tests use.)
mod check;
pub mod computer;
mod merge;
mod options;
mod packages;
mod read;

pub use check::*;
pub use merge::*;
pub use options::*;
pub use packages::*;
pub use read::*;

/// nami file. Unknown keys are errors, so typos don't silently do nothing.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub packages: Vec<String>,
    /// Extra binary caches (e.g. where `nami push` uploads), tried after
    /// cache.nixos.org, before building anything locally.
    #[serde(default)]
    pub caches: Vec<Cache>,
    /// After each apply, delete all but the newest N changes.
    #[serde(default, rename = "history")]
    pub history: Option<usize>,
    /// When old changes go by themselves (cleanup.rs): `30d`, `off`, or
    /// `{ older-than: 30d, keep: 3 }`. Unset: 30 days, the newest 3 kept.
    #[serde(default, deserialize_with = "crate::cleanup::deserialize")]
    pub cleanup: Option<crate::cleanup::Policy>,
    /// Dotfiles and program settings, linked into your home directory.
    #[serde(default)]
    pub home: Option<home::Home>,
    /// Desktop settings (GSettings/dconf): schema -> key -> value.
    #[serde(default)]
    pub settings: Option<crate::settings::Settings>,
    /// Allow software that isn't free (Steam, Spotify, ...). Yes unless
    /// this says `false`.
    #[serde(default)]
    pub unfree: Option<bool>,
    /// systemd units: name -> true (a package's unit) or settings (see services.rs).
    #[serde(default)]
    pub services: serde_yaml::Mapping,
    /// Files under /etc (system file only): path -> text, settings or `{ source: ... }`.
    #[serde(default)]
    pub files: serde_yaml::Mapping,
    /// The operating system itself, for `nami usb` (system file only).
    #[serde(default)]
    pub os: Option<computer::Os>,
    /// Libraries swapped in packages without building them again: the
    /// package -> the recipe (or catalog package) replacing it (replace.rs).
    #[serde(default)]
    pub replace: std::collections::BTreeMap<String, String>,
    /// The same in one package only, from `- firefox: { replace: ... }`.
    #[serde(default, rename = "replace-in")]
    pub replace_in: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
}

impl Config {
    pub fn allow_unfree(&self) -> bool {
        self.unfree.unwrap_or(true)
    }

    pub fn replace_plan(&self) -> crate::replace::Plan {
        crate::replace::Plan { everywhere: self.replace.clone(), only: self.replace_in.clone() }
    }
}

/// The names of nami files: yours (~/user.nami), a computer's
/// (/etc/nami/system.nami; <hostname>/system.nami in a config repo) and a
/// project's (project.nami). Sets of your own are any name.nami.
pub const USER_FILE: &str = "user.nami";
pub const SYSTEM_FILE: &str = "system.nami";
pub const PROJECT_FILE: &str = "project.nami";
/// In a config repo: what the computers share, as system.nami and
/// user.nami (and files/), each listed by the computers that have it.
pub const SHARED_DIR: &str = "shared";
/// The folder package folders are in, at the top of a config repo (and
/// nami's examples): a name alone in `packages:` is looked up there.
pub const PACKAGES_DIR: &str = "packages";

/// Whose folder `dir` is, when it's a person's in a computer's folder
/// (<computer>/<login>/, with their user.nami): their login. Theirs and the
/// admins' alone, on the computer.
pub fn person_of(dir: &std::path::Path) -> Option<String> {
    let name = dir.file_name()?.to_string_lossy().into_owned();
    let computer = dir.parent()?;
    (crate::oldnames::is_file(&dir.join(USER_FILE)) && crate::oldnames::is_file(&computer.join(SYSTEM_FILE)) && !name.starts_with('.') && name != SHARED_DIR).then_some(name)
}

/// A person's file on a computer: <computer's folder>/<login>/user.nami.
pub fn person_file(computer: &std::path::Path, login: &str) -> std::path::PathBuf {
    crate::oldnames::find(&computer.join(login).join(USER_FILE))
}

/// The people with a file of their own in a computer's folder (their
/// logins: <login>/user.nami), sorted.
pub fn people(computer: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(computer)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| crate::oldnames::is_file(&e.path().join(USER_FILE)))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') && n != SHARED_DIR)
        .collect();
    names.sort();
    names
}

/// Whether `dir` is a computer's folder: its system.nami, or people's
/// files (<login>/user.nami).
pub fn is_computer_dir(dir: &std::path::Path) -> bool {
    crate::oldnames::is_file(&dir.join(SYSTEM_FILE)) || !people(dir).is_empty()
}

/// A config repo's computers: its folders with a system.nami or people's
/// files (<login>/user.nami), each named after it (its hostname).
/// What they share is in shared/, as system.nami and user.nami, listed by
/// each (`- ../../shared/user.nami` in a person's file).
pub fn computers(top: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(top)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| is_computer_dir(&e.path()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') && n != SHARED_DIR)
        .collect();
    names.sort();
    names
}

/// Whether `dir` is a folder at the top of a repo (a computer's, or an
/// OS's variant).
pub fn in_config_repo(dir: &std::path::Path) -> bool {
    let Ok(dir) = std::fs::canonicalize(dir) else { return false };
    dir.parent().is_some_and(|top| top.join(".git").exists())
}

/// This computer's name (its hostname).
pub fn hostname() -> Option<String> {
    if crate::index::MAC {
        return crate::sys::hostname();
    }
    let h = std::fs::read_to_string("/proc/sys/kernel/hostname").ok()?;
    Some(h.trim().to_string()).filter(|h| !h.is_empty())
}

/// Which of a config repo's computers: the one named, else its only one,
/// else the one named after `here` (this computer, when applying).
pub fn computer_dir(top: &std::path::Path, name: Option<&str>, here: Option<&str>) -> anyhow::Result<std::path::PathBuf> {
    let all = computers(top);
    let which = match name {
        Some(n) => all.iter().find(|a| *a == n).ok_or_else(|| anyhow::anyhow!("{}: no computer {n} there (a folder {n}/ with system.nami, or people's <login>/user.nami); its computers: {}", top.display(), list_or_none(&all)))?,
        // A repo that is one computer (or an OS): its files at the top.
        None if all.is_empty() && is_computer_dir(top) => return Ok(top.to_path_buf()),
        None if all.len() == 1 => &all[0],
        None => match here.and_then(|h| all.iter().find(|a| *a == h)) {
            Some(a) => a,
            None if all.is_empty() => anyhow::bail!("{}: no computer there (a folder named after it, with its system.nami and each person's <login>/user.nami)", top.display()),
            None => {
                let mine = here.map(|h| format!(" none is named after this one ({h}), so")).unwrap_or_default();
                anyhow::bail!("{} has several computers ({}):{mine} name which, as {}/{}", top.display(), all.join(", "), top.display(), all[0])
            }
        },
    };
    Ok(top.join(which))
}

fn list_or_none(all: &[String]) -> String {
    if all.is_empty() { "none".into() } else { all.join(", ") }
}

/// The system file, /etc/nami/system.nami: the system's apps, for everyone who uses the
/// computer (by its name before, computer.nami, until it's renamed).
pub fn system_file() -> std::path::PathBuf {
    system_file_in(&std::path::PathBuf::from(std::env::var_os("NAMI_ROOT").unwrap_or_else(|| "/".into())))
}

/// [`system_file`] under `root`.
pub fn system_file_in(root: &Path) -> std::path::PathBuf {
    crate::oldnames::find(&root.join("etc/nami").join(SYSTEM_FILE))
}

/// The user file, ~/user.nami: your shell, dotfiles and settings, and the
/// apps of someone who isn't an admin. It may be ~/.config/nami/user.nami
/// instead, for a home without files at the top; ~/user.nami wins when
/// both are there (and nami says so).
pub fn user_file() -> std::path::PathBuf {
    user_file_in(&std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default())
}

/// [`user_file`] in the home `home`.
pub fn user_file_in(home: &Path) -> std::path::PathBuf {
    let top = crate::oldnames::find(&home.join(USER_FILE));
    // (tray's ~/.config/tray, before nami has moved it.)
    let config = if home.join(".config/nami").exists() { home.join(".config/nami") } else { home.join(".config/tray") };
    let inside = crate::oldnames::find(&config.join(USER_FILE));
    let inside = if inside.symlink_metadata().is_ok() { inside } else { home.join(".config/nami").join(USER_FILE) };
    let there = |p: &Path| p.symlink_metadata().is_ok();
    // (~/user.nami linked to it is the one file: a config repo's checkout is there.)
    let same = || std::fs::canonicalize(&top).ok().is_some_and(|t| std::fs::canonicalize(&inside).ok() == Some(t));
    match (there(&top), there(&inside)) {
        (true, true) if same() => top,
        (true, true) => {
            deprecated("~/.config/nami/user.nami", "both it and ~/user.nami are there: nami uses ~/user.nami, so move what's in one into the other");
            top
        }
        (false, true) => inside,
        _ => top,
    }
}

/// Whether unfree packages may be installed: what the spec in charge says
/// (the one last applied, else ~/user.nami), yes by default.
pub fn unfree_allowed(managed: Option<&std::path::Path>) -> bool {
    let file = managed.map(std::path::Path::to_path_buf).unwrap_or_else(user_file);
    std::fs::read_to_string(file).ok()
        .and_then(|t| crate::yaml_text::from_str(&t).ok())
        .and_then(|v| v.get("unfree").and_then(serde_yaml::Value::as_bool))
        .unwrap_or(true)
}

/// The keys every nami file may have (the computer's and a person's have
/// theirs too: [`COMPUTER_KEYS`], [`USER_KEYS`]).
pub const TOP_KEYS: &[&str] = &["format", "packages", "caches", "history", "cleanup", "settings", "unfree", "services", "files", "replace", "replace-in"];

/// A file's folder as an absolute path ("" is the current one).
fn absolute(base: &Path) -> std::path::PathBuf {
    let base = if base.as_os_str().is_empty() { Path::new(".") } else { base };
    base.canonicalize().unwrap_or_else(|_| base.to_path_buf())
}


thread_local! {
    /// The package folders loaded so far (a folder that builds nothing):
    /// its name in `packages:`, and what it sets up.
    static FOLDERS: std::cell::RefCell<std::collections::BTreeMap<String, serde_yaml::Mapping>> = Default::default();
    /// Package folders the top file leaves out (`without: [plymouth]`):
    /// wherever they're listed, they and their setup aren't loaded.
    static LEFT_OUT: std::cell::RefCell<std::collections::BTreeSet<String>> = Default::default();
    /// Login screens the computer doesn't use, left out as `without:` does.
    static UNUSED: std::cell::RefCell<std::collections::BTreeSet<String>> = Default::default();
    /// The files that listed one of them: desktops still (niri, without GDM).
    static LISTED_UNUSED: std::cell::RefCell<std::collections::BTreeSet<String>> = Default::default();
}

/// Whether `name` is a package folder that was listed (by its path).
pub fn is_package_file(name: &str) -> bool {
    FOLDERS.with(|f| f.borrow().contains_key(crate::index::split_version(name).0))
}

/// What a listed package folder `name` sets up besides the program, in
/// words (its services, its group...): what a computer nami didn't make
/// goes without. Empty when it has no folder, or only lists packages.
pub fn package_setup(name: &str) -> Vec<String> {
    let name = crate::index::split_version(name).0;
    let Some(m) = FOLDERS.with(|f| f.borrow().get(name).cloned()) else { return Vec::new() };
    let count = |k: &str| m.get(k).map_or(0, |v| v.as_sequence().map(Vec::len).or(v.as_mapping().map(serde_yaml::Mapping::len)).unwrap_or(1));
    let mut out = Vec::new();
    let mut say = |k: &str, one: &str, many: &str| match count(k) {
        0 => {}
        1 => out.push(one.to_string()),
        _ => out.push(many.to_string()),
    };
    say("services", "its service", "its services");
    say("groups", "its group", "its groups");
    say("files", "its file", "its files");
    say("login", "its login rules", "its login rules");
    say("setuid", "a program that acts as root", "programs that act as root");
    say("name-lookup", "its name lookup", "its name lookup");
    say("drivers", "its drivers", "its drivers");
    say("libraries", "libraries for programs built for other distributions", "libraries for programs built for other distributions");
    say("boot", "its boot parts", "its boot parts");
    say("settings", "its desktop settings", "its desktop settings");
    say("env", "its environment", "its environment");
    say("packages", "what it brings along", "what it brings along");
    out
}

/// Whether `name` is a set (namios, niri, base): a package folder with
/// no program of its name (nor a package's output, like
/// shadow.su) in the catalog. Installing it is installing what it lists.
pub fn is_set(name: &str, index: &Index) -> bool {
    let program = index.get(name).is_ok() || name.rsplit_once('.').is_some_and(|(n, out)| index.get(n).is_ok_and(|p| p.outputs.contains_key(out)));
    is_package_file(name) && !program
}

/// Whether a `packages:` entry is a nami file of its own (a set): a path
/// or a repo's file ending in .nami (`./work.nami`,
/// `github:ada/me/user.nami`).
pub fn is_set_file(entry: &str) -> bool {
    if crate::layers::is_remote(entry) {
        return remote_set(entry).is_some();
    }
    crate::oldnames::ends_with_extension(entry) && (entry.starts_with("./") || entry.starts_with("../") || entry.starts_with('/'))
}

/// A repo's set, as written, for layers.rs to fetch: `github:alice/os/gaming.nami@v2`
/// (or `git+<url>@v2#gaming.nami`, gitref.rs) is gaming.nami of github:alice/os at v2.
fn remote_set(entry: &str) -> Option<String> {
    let at = entry.rfind(".nami").or_else(|| entry.rfind(".tray"))?;
    let rest = &entry[at + 5..];
    (rest.is_empty() || (rest.starts_with('@') && !rest.contains('/'))).then(|| entry.to_string())
}

/// Which file a nami file is: the computer's, a person's (user.nami), or
/// (read elsewhere) a project's. The same keys mean the same things in
/// each, for the computer or for one person.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Scope {
    System,
    User,
}

impl Scope {
    /// A file's scope by its name: user.nami is a person's; system.nami
    /// and package.nami are the computer's. A set of another name is what
    /// it says: a person's with a shell, aliases or files in ~/ (a set a
    /// file includes is that file's, whatever it says).
    pub fn of(name: &str, value: &Value) -> Scope {
        match Path::new(name).file_name().and_then(|f| f.to_str()).map(crate::oldnames::current).as_deref() {
            Some(USER_FILE) => Scope::User,
            Some(SYSTEM_FILE | "package.nami") => Scope::System,
            _ => {
                let personal = ["aliases", "shell", "login-shell"].iter().any(|k| value.get(k).is_some())
                    || value.get("files").and_then(Value::as_mapping).is_some_and(|f| f.keys().any(|k| k.as_str().is_some_and(|p| p.starts_with("~/"))));
                if personal { Scope::User } else { Scope::System }
            }
        }
    }
}

/// Keys only a computer has: who's on it, its name, its parts.
pub const COMPUTER_KEYS: &[&str] = &[
    "desktop", "login-screen", "hostname", "timezone", "language", "keyboard", "users", "without", "environment", "wrappers",
    "login", "login-services", "groups", "name-lookup", "units", "mask", "build", "build-tools", "early-boot", "initrd-programs", "initrd-modules",
    "kernel-modules", "firmware", "graphics", "graphics-32bit", "kernel-options", "checks", "secure-boot", "wifi", "kernel", "libraries", "updates",
];

/// Whether the computer updates itself (`updates:`, `auto` unless it says
/// `manual`): nami-update.timer, once a day. On namiOS `manual` keeps the
/// timer off, as `without: [nami-update.timer]` does; on another
/// distribution, nami makes the timer or takes it away (src/auto_update.rs).
pub const UPDATES: &[&str] = &["auto", "manual"];

/// The timer that updates the computer once a day.
pub const UPDATE_TIMER: &str = "nami-update.timer";

/// A computer file's `updates:` as written: auto (the default) or manual.
pub fn updates_of(value: &Value) -> anyhow::Result<bool> {
    match value.get("updates") {
        None | Some(Value::Null) => Ok(true),
        Some(Value::String(s)) if s == "auto" => Ok(true),
        Some(Value::String(s)) if s == "manual" => Ok(false),
        Some(v) => anyhow::bail!("updates: auto or manual, not {}", serde_yaml::to_string(v).unwrap_or_default().trim()),
    }
}

/// Whether computer file `text` lets the computer update itself: `updates:`
/// isn't manual, and the timer isn't in `without:`. (A file that can't be
/// read leaves it as it is: on.)
pub fn updates_itself(text: &str) -> bool {
    let Ok(v) = crate::yaml_text::from_str(text) else { return true };
    let without = v.get("without").and_then(Value::as_sequence).into_iter().flatten().any(|w| w.as_str() == Some(UPDATE_TIMER));
    updates_of(&v).unwrap_or(true) && !without
}
/// The keys in computer file `text` that only a computer nami made acts on
/// (its name, people, everyone's env and desktop settings...), said as a
/// list: `hostname:, users: and env:`. None if it has none.
pub fn only_on_nami_os(text: &str) -> Option<String> {
    let keys: Vec<String> = only_on_nami_os_keys(text).iter().map(|k| format!("{k}:")).collect();
    match keys.split_last()? {
        (last, []) => Some(last.clone()),
        (last, rest) => Some(format!("{} and {last}", rest.join(", "))),
    }
}

/// The keys in computer file `text` that only namiOS acts on, in order.
fn only_on_nami_os_keys(text: &str) -> Vec<String> {
    let Ok(v) = crate::yaml_text::from_str(text) else { return Vec::new() };
    let Some(m) = v.as_mapping() else { return Vec::new() };
    m.keys().filter_map(Value::as_str).filter(|k| (COMPUTER_KEYS.contains(k) && !["wifi", "updates"].contains(k)) || ["env", "settings"].contains(k)).map(String::from).collect()
}

/// A `packages:` entry that's a name alone: not a path, a repo's or Flathub's.
pub fn is_plain_entry(p: &str) -> bool {
    !(p.starts_with("./") || p.starts_with("../") || p.starts_with('/') || p.starts_with("github:") || crate::flathub::is_flathub(p) || crate::casks::is_cask(p) || crate::bundle_app::is_entry(p) || crate::python_env::parse(p).is_some())
}

/// Keys only a person has: their shell.
pub const USER_KEYS: &[&str] = &["env", "aliases", "shell", "login-shell", "namios"];

/// Keys only a package.nami has (a package folder's, like
/// packages/<name>/package.nami): the parts of Linux that many packages add to. What one
/// package owns is a file, in `files:`.
pub const PACKAGE_KEYS: &[&str] = &["login", "setuid", "groups", "name-lookup", "drivers", "boot", "secure-boot", "hardware", "libraries"];

/// Keys of the earlier format, and where what they said goes now.
pub(crate) const FORMER_KEYS: &[(&str, &str)] = &[
    ("units", "a file now: files: /etc/systemd/system/<name> (each person's: /etc/systemd/user/<name>)"),
    ("mask", "without: now (a service there is kept off)"),
    ("from", "gone: a name alone is looked up in the nearest packages/ folder above the file, then in the catalog"),
    ("environment", "env: now"),
    ("login-services", "under login: now (<program>: login, su or its rules)"),
    ("wrappers", "setuid: now, a path each (bin/sudo)"),
    ("build", "a file now, made by a script: files: <path>: { script: ..., tools: [...] }"),
    ("build-tools", "tools: of the file made by a script now (files: <path>: { script, tools })"),
    ("early-boot", "boot: microcode: now"),
    ("kernel", "under boot: (boot: kernel: linux_zen)"),
    ("initrd-programs", "boot: initrd-programs: now"),
    ("initrd-modules", "boot: initrd-modules: now"),
    ("kernel-modules", "drivers: kernel-modules: now"),
    ("firmware", "drivers: firmware: now"),
    ("graphics", "drivers: graphics: now"),
    ("graphics-32bit", "under drivers: (drivers: graphics-32bit: [...]), in a package.nami"),
    ("kernel-options", "under boot: (boot: kernel-options: [...]), in a package.nami"),
    ("checks", "in a checks.nami next to the package.nami now"),
];

pub use crate::format::{FORMAT, NEWER_NAMI, RENAMED_KEYS};

pub const HOME_KEYS: &[&str] = &["env", "aliases", "shell", "login-shell", "files"];

/// A nami file with what it includes: `include:` names nami files that come
/// with nami (`namios`) or paths relative to the file. Included files come
/// first and the file itself last; where both set something, the file wins.
/// Lists are joined (the file's own entries first), maps merged key by key.
pub fn load(file: &Path) -> anyhow::Result<Config> {
    let text = std::fs::read_to_string(file).map_err(|e| crate::errors::coded(if e.kind() == std::io::ErrorKind::NotFound { crate::errors::Code::NotFound } else { crate::errors::Code::Failed }, format!("reading {}: {e}", file.display())))?;
    // A linked file (~/user.nami into a config repo): its paths are the repo's.
    let real = std::fs::read_link(file).ok().map(|t| file.parent().unwrap_or(Path::new(".")).join(t));
    load_text(&text, &file.display().to_string(), real.as_deref().unwrap_or(file).parent().unwrap_or(Path::new(".")))
}

/// [`load`] for a file's text; `base` is its folder.
pub fn load_text(text: &str, name: &str, base: &Path) -> anyhow::Result<Config> {
    Ok(load_with_layers(text, name, base)?.0)
}

/// [`load_text`], and the layers from repos it includes (their pins are in
/// nami.lock in `base`; new ones are pinned there).
pub fn load_with_layers(text: &str, name: &str, base: &Path) -> anyhow::Result<(Config, Vec<crate::layers::Used>)> {
    let (value, mine, used) = load_whole(text, name, base)?;
    Ok((serde_yaml::from_value(inside(value, &mine, name)?).map_err(|e| crate::errors::coded(crate::errors::Code::InvalidFile, format!("{name}: {e}")))?, used))
}

/// The whole computer in one file (`nami show --flat`, and
/// /etc/nami/computer.flat.nami): the system file merged with all it
/// includes, as YAML, each value's line ending in the file it came from
/// (paths next to the file relative to it; a repo's layer at its commit). Secrets stay sealed.
pub fn flat(file: &Path) -> anyhow::Result<String> {
    let text = std::fs::read_to_string(file).map_err(|e| anyhow::anyhow!("reading {}: {e}", file.display()))?;
    flat_text(&text, &file.display().to_string(), file.parent().unwrap_or(Path::new(".")))
}

/// The part of a config repo a computer uses: its own folder, and each
/// folder of a file of yours it lists (../packages/x), each whole,
/// where they are under `top`. On the computer they're /etc/nami-config,
/// as in the repo, and /etc/nami is its own folder there, so ../shared/...
/// is there too.
pub struct Used {
    pub top: PathBuf,
    /// Folders under `top`; none inside another.
    pub folders: Vec<PathBuf>,
    /// The computer's own folder, under `top`.
    pub computer: PathBuf,
}

impl Used {
    /// Where `dir` goes under a copy of `top`.
    pub fn rel<'a>(&self, dir: &'a Path) -> &'a Path {
        dir.strip_prefix(&self.top).unwrap_or(Path::new(""))
    }
}

/// What of its config repo the computer `file` uses (see [`Used`]).
pub fn used(file: &Path) -> anyhow::Result<Used> {
    let computer = file.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")).canonicalize()?;
    let (loaded, mut sources) = crate::flat::recorded(|| load(file));
    loaded?;
    // And what each person's file lists (../../shared/user.nami): they're
    // applied on the computer from its copy.
    for p in people(&computer) {
        let (loaded, theirs) = crate::flat::recorded(|| load(&person_file(&computer, &p)));
        loaded?;
        sources.extend(theirs);
    }
    let mut folders: Vec<PathBuf> = sources
        .iter()
        .filter(|(s, _)| s.starts_with('/'))
        .filter_map(|(s, _)| Path::new(s).parent()?.canonicalize().ok())
        .chain([computer.clone()])
        .collect();
    folders.sort();
    folders.dedup();
    let all = folders.clone();
    folders.retain(|f| !all.iter().any(|a| a != f && f.starts_with(a)));
    // (The computer's folder is always one down: ../shared is in the copy too.)
    let above = computer.parent().unwrap_or(Path::new("/")).to_path_buf();
    let top = folders.iter().fold(above, |top, f| top.ancestors().find(|a| f.starts_with(a)).unwrap_or(Path::new("/")).to_path_buf());
    Ok(Used { top, folders, computer })
}

/// [`flat`] for a file's text; `base` is its folder.
pub fn flat_text(text: &str, name: &str, base: &Path) -> anyhow::Result<String> {
    let (value, mut sources) = crate::flat::recorded(|| load_whole(text, name, base));
    // Files of yours by where they are from this one (../packages/x).
    let top = absolute(base);
    for (s, _) in &mut sources {
        if !s.starts_with('/') {
            continue;
        }
        let common = top.ancestors().find(|a| Path::new(s.as_str()).starts_with(a)).unwrap_or(Path::new("/"));
        if common != Path::new("/") {
            let up = top.strip_prefix(common).map_or(0, |r| r.components().count());
            let rest = Path::new(s.as_str()).strip_prefix(common).unwrap_or(Path::new(s.as_str()));
            *s = format!("{}{}", "../".repeat(up), rest.display());
        }
    }
    Ok(crate::flat::text(&written_keys(value?.0, &sources), &sources))
}

/// A nami file's text as YAML, its secrets marked (see [`crate::secrets::mark`]).
pub fn parse(text: &str, name: &str, kind: crate::secrets::Kind) -> anyhow::Result<Value> {
    let mut v: Value = crate::yaml_text::from_str(text).map_err(|e| crate::errors::coded(crate::errors::Code::InvalidFile, format!("{name}: {e}")))?;
    modern(&mut v, name)?;
    package_map(&mut v, name)?;
    crate::secrets::mark(&mut v, name, kind)?;
    Ok(v)
}

/// A nami file's own text (without what it includes), read as a [`Config`],
/// its packages as written (its names not looked up).
pub fn parse_config(text: &str, name: &str) -> anyhow::Result<Config> {
    parse_config_in(text, name, None)
}

/// [`parse_config`], with names looked up in the nearest packages/ folder
/// when `base`, the file's folder, is given.
pub fn parse_config_in(text: &str, name: &str, base: Option<&Path>) -> anyhow::Result<Config> {
    let mut v = parse(text, name, crate::secrets::Kind::Nami)?;
    if let Some(base) = base {
        resolve_names(&mut v, base);
    }
    if v.is_null() {
        return Ok(Config::default());
    }
    let v = crate::wifi::expand(v, name)?;
    serde_yaml::from_value(inside(v.clone(), &v, name)?).map_err(|e| crate::errors::coded(crate::errors::Code::InvalidFile, format!("{name}: {e}")))
}

/// A project's file (project.nami) with what it includes: paths next to
/// it, and layers from repos (`github:acme/dev/node-service`), pinned in
/// the project's nami.lock. Merged as any nami file's includes are: lists
/// joined, maps key by key, the project's own file winning. Returns its
/// value (without `include:`) and the layers it used.
pub fn load_project(text: &str, name: &str, base: &Path) -> anyhow::Result<(Value, Vec<crate::layers::Used>)> {
    let base = if base.as_os_str().is_empty() { Path::new(".") } else { base };
    let mut layers = crate::layers::Session::project(base);
    let value = load_in(text, name, &top_source(name, base), base, 0, &mut layers, None, Kind::Project, None);
    let used = layers.finish();
    Ok((value?.0, used))
}

/// A nami file reproducing a generation's packages and caches.
pub fn export(m: &Manifest) -> String {
    let mut out = format!("# Generated by `nami export` from generation {} ({} UTC).\n", m.generation, crate::profile::format_time(m.created));
    if m.packages.is_empty() {
        out += "packages: []\n";
    } else {
        out += "packages:\n";
        for e in &m.packages {
            out += &match crate::index::split_version(&e.attr) {
                (name, Some(v)) => format!("  - {name}: {v}\n"),
                (name, None) => format!("  - {name}\n"),
            };
        }
    }
    if !m.caches.is_empty() {
        out += "caches:\n";
        for c in &m.caches {
            out += &format!("  - url: {}\n    key: {}\n", c.url, c.key);
        }
    }
    if m.services.is_some() || m.etc.is_some() {
        out += "# services:/files: this generation has some; copy them from the nami file you applied.\n";
    }
    if m.home.is_some() {
        out += "# home: this generation has dotfiles, but their settings can't be read\n# back from the rendered files; copy them from the nami file you applied.\n";
    }
    out
}

#[cfg(test)]
pub(crate) mod tests;
