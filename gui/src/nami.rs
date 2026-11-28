//! What the window asks nami for: its examples, who this is, its files,
//! installing from the USB drive, all by running the nami command (`NAMI`, default
//! the `nami` on PATH) and reading its JSON.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use serde::Deserialize;

fn nami() -> Command {
    Command::new(program())
}

/// The nami command: `NAMI` (or tray's `TRAY`), else the `nami` on PATH, else
/// `tray` (a session started before nami came, its PATH tray's).
pub fn program() -> std::ffi::OsString {
    if let Some(p) = std::env::var_os("NAMI").or_else(|| std::env::var_os("TRAY")) {
        return p;
    }
    let on_path = |name: &str| std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()));
    if on_path("nami") || !on_path("tray") { "nami".into() } else { "tray".into() }
}

/// How many packages this computer can install from its catalog, from the head of nami's compact
/// form of it (index.idx: "NAMIIDX1", the count as a little-endian u32 at byte 80, and at 84 how
/// many of them this computer can install: a Mac only what's ready-built). Where 84 is 0 (made
/// by a nami before 0.22), the count on a computer that can install all of them; else None.
/// Read once: 88 bytes.
pub fn catalog_count() -> Option<u32> {
    static COUNT: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *COUNT.get_or_init(|| {
        use std::io::Read;
        let root = std::env::var_os("NAMI_ROOT").map(PathBuf::from).unwrap_or_else(|| "/".into());
        // nami's folder: /nami, or where it was (a computer whose `/` can't hold it), or the old one.
        ["nami/index.idx", "nix/var/nami/index.idx", "nix/var/tray/index.idx"].iter().find_map(|p| {
            let mut head = [0u8; 88];
            std::fs::File::open(root.join(p)).ok()?.read_exact(&mut head).ok()?;
            (&head[..8] == b"NAMIIDX1").then_some(())?;
            let at = |i: usize| u32::from_le_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
            match at(84) {
                0 if SOME_NOT_INSTALLABLE => None,
                0 => Some(at(80)),
                n => Some(n),
            }
        })
    })
}

/// A computer whose catalog has packages it can't install: a Mac (only what's ready-built) and
/// ARM Linux (what's ready-built or has a build definition).
const SOME_NOT_INSTALLABLE: bool = cfg!(target_os = "macos") || cfg!(all(target_os = "linux", target_arch = "aarch64"));

/// What's said where the catalog doesn't say: about what this kind of computer can install
/// (November 2026: 146,202 on x86_64 Linux, 87,731 on ARM Linux, 73,502 on an Apple silicon Mac,
/// 66,872 on an Intel one).
const SIZE_SAID_BEFORE: &str = if cfg!(target_os = "macos") {
    if cfg!(target_arch = "aarch64") { "over 70,000" } else { "over 60,000" }
} else if cfg!(target_arch = "aarch64") {
    "over 80,000"
} else {
    "over 140,000"
};

/// A catalog's size as said to people: "over 140,000" for 146,202, "over 90,000" for 96,811
/// (two figures past 100,000, one below), "140,000" when it's just that. None for one too small
/// to be the real catalog (a test's).
pub fn friendly_count(n: u32) -> Option<String> {
    if n < 1000 {
        return None;
    }
    let digits = n.ilog10() + 1;
    let step = 10u32.pow(digits - 2).max(10_000u32.min(10u32.pow(digits - 1)));
    let floor = n / step * step;
    let mut s = String::new();
    for (i, c) in floor.to_string().chars().enumerate() {
        if i > 0 && (digits as usize - i) % 3 == 0 {
            s.push(',');
        }
        s.push(c);
    }
    Some(if floor < n { format!("over {s}") } else { s })
}

/// "over 140,000", from the catalog this computer has (what it can install); about what this
/// kind of computer can, when it doesn't say.
pub fn catalog_size_said() -> String {
    catalog_count().and_then(friendly_count).unwrap_or_else(|| SIZE_SAID_BEFORE.into())
}

/// The search field's words: "Search over 140,000 apps and tools", and on a Mac its apps from
/// Homebrew's casks too.
pub fn search_said() -> String {
    let size = catalog_size_said();
    if crate::look::MAC { format!("Search {size} apps and tools, and Mac apps") } else { format!("Search {size} apps and tools") }
}

#[cfg(test)]
#[test]
fn a_catalogs_size_rounded_down() {
    assert_eq!(friendly_count(146_202).as_deref(), Some("over 140,000"));
    assert_eq!(friendly_count(96_811).as_deref(), Some("over 90,000"));
    assert_eq!(friendly_count(12_345).as_deref(), Some("over 10,000"));
    assert_eq!(friendly_count(5_432).as_deref(), Some("over 5,000"));
    assert_eq!(friendly_count(1_460_000).as_deref(), Some("over 1,400,000"));
    assert_eq!(friendly_count(140_000).as_deref(), Some("140,000"));
    assert_eq!(friendly_count(12), None);
}

/// Where the file `path` (named as nami names it, user.nami) is: by tray's
/// name (user.tray) when only that one's there.
pub fn existing(path: PathBuf) -> PathBuf {
    if path.symlink_metadata().is_ok() {
        return path;
    }
    // Its names before, newest first, as nami reads them (src/oldnames.rs): system.nami was
    // computer.nami until 0.19, and tray's computer.tray; any other x.nami was x.tray.
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let olds: Vec<String> = match name {
        "system.nami" => vec!["computer.nami".into(), "computer.tray".into()],
        _ => name.strip_suffix(".nami").map(|s| vec![format!("{s}.tray")]).unwrap_or_default(),
    };
    olds.into_iter().map(|o| path.with_file_name(o)).find(|o| o.symlink_metadata().is_ok()).unwrap_or(path)
}

/// A nami file's text as YAML, tray's `trayos:` read as `namios:` (what's in
/// both is `namios:`'s), as nami reads it.
pub fn yaml(text: &str) -> Result<serde_yaml::Value, serde_yaml::Error> {
    let mut v: serde_yaml::Value = serde_yaml::from_str(text)?;
    if let Some(m) = v.as_mapping_mut() {
        if let Some(old) = m.remove("trayos") {
            match m.get_mut("namios") {
                Some(new) => under(new, old),
                None => {
                    m.insert("namios".into(), old);
                }
            }
        }
    }
    Ok(v)
}

fn under(new: &mut serde_yaml::Value, old: serde_yaml::Value) {
    if let (Some(n), serde_yaml::Value::Mapping(o)) = (new.as_mapping_mut(), old) {
        for (k, v) in o {
            match n.get_mut(&k) {
                Some(have) => under(have, v),
                None => {
                    n.insert(k, v);
                }
            }
        }
    }
}

/// Run nami with `args` and read what it prints as JSON.
pub fn json<T: for<'a> Deserialize<'a>>(args: &[&str]) -> Result<T, String> {
    let out = nami().args(args).output().map_err(|e| format!("can't run nami: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("nami {}: {e}", args.join(" ")))
}

#[derive(Deserialize, Clone)]
pub struct Example {
    pub name: String,
    /// It sets up the system (a service, files in /etc, drivers): only ever the
    /// system's, never one person's own. (`computer` as nami before 0.19 said it.)
    #[serde(default, alias = "computer")]
    pub system: bool,
}

/// What `nami new --json` lists: the computers to start from (namiOS and
/// its barebones edition), the packages to add to one, and a project's
/// starters and services.
#[derive(Deserialize, Clone)]
pub struct Examples {
    pub computers: Vec<Example>,
    pub packages: Vec<Example>,
    /// Starters for a project (node, rust...).
    #[serde(default)]
    pub projects: Vec<Example>,
    /// Services for a project (postgres, redis...).
    #[serde(default)]
    pub services: Vec<Example>,
}

impl Examples {
    /// Whether `name` is one of nami's examples, of any kind.
    pub fn has(&self, name: &str) -> bool {
        self.computers.iter().chain(&self.packages).any(|x| x.name == name)
    }

    /// Whether installing `name` sets up the computer (docker, steam), so
    /// it's only ever for everyone on it.
    pub fn sets_up_system(&self, name: &str) -> bool {
        self.computers.iter().chain(&self.packages).any(|x| x.name == name && x.system)
    }
}

pub fn examples() -> Result<Examples, String> {
    json(&["new", "--json"])
}

/// A package of the catalog, as `nami search` finds it.
#[derive(Deserialize, Clone)]
pub struct Found {
    pub attr: String,
    pub description: Option<String>,
    /// An app (it has a menu entry), not a command.
    #[serde(default)]
    pub app: bool,
    /// On this computer already (for everyone, or yours).
    #[serde(default)]
    pub installed: bool,
    pub icon: Option<PathBuf>,
    #[serde(default)]
    pub version: String,
    pub homepage: Option<String>,
    pub license: Option<String>,
    #[serde(default)]
    pub unfree: bool,
    /// Built on the computer: the binary cache hasn't it.
    #[serde(default)]
    pub builds_here: bool,
    /// The app's own name ("GIMP"), its line and a few sentences about it.
    pub title: Option<String>,
    pub summary: Option<String>,
    pub about: Option<String>,
    /// Its screenshots: files, from `info` (URLs from `search`).
    #[serde(default)]
    pub screenshots: Vec<String>,
    /// Its menu entries (.desktop files), to start it by.
    #[serde(default)]
    pub desktop: Vec<String>,
    /// Its kinds: `nami search --category` keys ("graphics").
    #[serde(default)]
    pub categories: Vec<String>,
    /// Where it's from when not the catalog: `flathub` (its attr is
    /// `flathub:<app id>`).
    #[serde(default)]
    pub source: Option<String>,
    /// Who makes it, when it's not nixpkgs' (Flathub's: its developer).
    #[serde(default)]
    pub publisher: Option<String>,
    /// Why nami can't install it here (one of Homebrew's casks that needs a .pkg, a newer
    /// macOS...): said where Install would be.
    #[serde(default)]
    pub unavailable: Option<String>,
}

impl Found {
    /// From Flathub (through Flatpak), not the catalog.
    pub fn flathub(&self) -> bool {
        self.source.as_deref() == Some("flathub") || self.attr.starts_with("flathub:")
    }

    /// From Homebrew's casks (a Mac's apps: `cask:<token>`), not the catalog.
    pub fn cask(&self) -> bool {
        self.source.as_deref() == Some("cask") || self.attr.starts_with("cask:")
    }

    /// What to call it: the app's own name, else the package's.
    pub fn name(&self) -> String {
        self.title.clone().unwrap_or_else(|| self.attr.clone())
    }

    /// Its line: the app's own, else nixpkgs'.
    pub fn line(&self) -> String {
        self.summary.clone().or_else(|| self.description.clone()).unwrap_or_default()
    }
}

/// The apps as the catalog knows them: their own names ("GIMP",
/// "LibreOffice"), short enough for a tile, and their menu entries.
pub fn names(attrs: &[String]) -> Vec<Found> {
    let mut args = vec!["info"];
    args.extend(attrs.iter().map(String::as_str));
    args.push("--json");
    let mut found: Vec<Found> = json(&args).unwrap_or_default();
    for f in &mut found {
        f.title = f.title.as_ref().map(|t| short_name(&f.attr, t));
    }
    found
}

/// A long name its package's name is the initials of, as those:
/// GNU Image Manipulation Program is GIMP.
fn short_name(attr: &str, title: &str) -> String {
    let initials: String = title.split_whitespace().filter_map(|w| w.chars().next()).collect();
    if title.len() > 18 && initials.eq_ignore_ascii_case(attr) { initials.to_uppercase() } else { title.to_string() }
}

/// The catalog's packages for `term`, apps first.
pub fn search(term: &str) -> Result<Vec<Found>, String> {
    json(&["search", term, "--json"])
}

/// Every package for `term`: apps, and commands too (the launcher's Install, Discover). (A
/// Mac's apps from Homebrew's casks come in a section of their own: `search_flathub`.)
pub fn search_all(term: &str) -> Result<Vec<Found>, String> {
    let found: Vec<Found> = json(&["search", term, "--all", "--json"])?;
    Ok(found.into_iter().filter(|f| !f.cask()).collect())
}

/// Flathub's apps for `term` (those the catalog has too are left out:
/// its build is the one to have). Slower than the catalog's: it asks
/// Flathub, the first time a day. On a Mac, Homebrew's casks instead (Flathub's apps are
/// Linux's): Mac apps from their developers.
pub fn search_flathub(term: &str) -> Result<Vec<Found>, String> {
    if crate::look::MAC {
        let found: Vec<Found> = json(&["search", term, "--json"])?;
        return Ok(found.into_iter().filter(Found::cask).collect());
    }
    let found: Vec<Found> = json(&["search", term, "--flathub", "--json"])?;
    Ok(found.into_iter().filter(Found::flathub).collect())
}

/// Tools for a project: every package for `term`, commands first.
pub fn search_tools(term: &str) -> Result<Vec<Found>, String> {
    let mut found: Vec<Found> = json(&["search", term, "--all", "--json"])?;
    found.sort_by_key(|f| f.app);
    Ok(found)
}

/// A package of nami's this computer's hardware is for (`nami doctor
/// --hardware --json`): `nvidia-legacy`, for an NVIDIA GeForce GTX 1070.
#[derive(Deserialize, Clone)]
pub struct Hardware {
    pub hardware: String,
    pub model: Option<String>,
    pub package: String,
    /// system.nami lists its package already.
    #[serde(default)]
    pub listed: bool,
}

impl Hardware {
    /// The card as people know it: its model, else what it is.
    pub fn name(&self) -> String {
        self.model.clone().unwrap_or_else(|| self.hardware.clone())
    }
}

/// This computer's NVIDIA graphics card, and the driver package for it.
pub fn nvidia() -> Option<Hardware> {
    let out = nami().args(["doctor", "--hardware", "--json"]).output().ok().filter(|o| o.status.success())?;
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| serde_json::from_str::<Hardware>(l).ok()).find(|h| h.package == "nvidia" || h.package == "nvidia-legacy")
}

/// What a Mac's Discover shows first: well-known tools, ready-built for Macs. (A Mac's catalog
/// doesn't know yet which packages are apps, their kinds or how much each is used.)
pub const MAC_POPULAR: [&str; 12] = ["git", "ripgrep", "nodejs", "python3", "jq", "gh", "neovim", "go", "fd", "htop", "ffmpeg", "wget"];

/// And the well-known apps with a window, from Homebrew's casks (among its most installed, each
/// an app nami installs: nami's src/casks.rs).
pub const MAC_POPULAR_APPS: [&str; 12] = [
    "cask:google-chrome", "cask:firefox", "cask:visual-studio-code", "cask:iterm2", "cask:slack", "cask:discord", "cask:spotify", "cask:obsidian", "cask:rectangle",
    "cask:maccy", "cask:stats", "cask:vlc",
];

/// The most used apps, of one kind (`nami search --category`) or of all. On a Mac, well-known
/// apps from Homebrew's casks and tools (`MAC_POPULAR_APPS`, `MAC_POPULAR`), and no kinds; an
/// app nami can't install on this Mac is left out.
pub fn popular(category: Option<&str>) -> Result<Vec<Found>, String> {
    if crate::look::MAC {
        let all: Vec<String> = if category.is_none() { MAC_POPULAR_APPS.iter().chain(&MAC_POPULAR).map(|t| t.to_string()).collect() } else { Vec::new() };
        return Ok(if all.is_empty() { Vec::new() } else { names(&all).into_iter().filter(|f| f.unavailable.is_none()).collect() });
    }
    let found: Vec<Found> = match category {
        Some(c) => json(&["search", "--category", c, "--json"])?,
        None => json(&["search", "--json"])?,
    };
    Ok(found.into_iter().map(shortened).collect())
}

/// An app with its name as short as it's known by: GIMP.
fn shortened(mut f: Found) -> Found {
    f.title = f.title.as_ref().map(|t| short_name(&f.attr, t));
    f
}

/// One app, with its screenshots downloaded.
pub fn info(attr: &str) -> Result<Found, String> {
    let found: Vec<Found> = json(&["info", attr, "--json", "--screenshots"])?;
    found.into_iter().next().map(shortened).ok_or_else(|| format!("{attr} isn't in the catalog"))
}

/// A version a package has had (`nami info --versions`), newest first;
/// `date` is when nixpkgs moved on from it (empty: the catalog's own).
#[derive(Deserialize, Clone)]
pub struct Version {
    pub version: String,
    pub date: String,
}

pub fn versions(attr: &str) -> Result<Vec<Version>, String> {
    let mut all: std::collections::BTreeMap<String, Vec<Version>> = json(&["info", attr, "--versions", "--json"])?;
    Ok(all.remove(attr).unwrap_or_default())
}

/// What `nami install-to --json` says as it goes.
pub enum Event {
    Step { step: String, percent: Option<u64>, message: String },
    /// It's over: what went wrong, if something did.
    Finished(Result<(), String>),
}

#[derive(Deserialize)]
struct Line {
    step: String,
    percent: Option<u64>,
    message: String,
}

/// Put this computer on `disk` (`nami install-to`): next to Windows with
/// `size` GB, or the whole disk; encrypted with `password`, if given.
pub fn install(disk: &str, next_to_windows: Option<u64>, password: Option<&str>) -> UnboundedReceiver<Event> {
    let mut cmd = nami();
    cmd.args(["install-to", disk, "--yes", "--json"]);
    match next_to_windows {
        Some(gb) => cmd.args(["--next-to-windows", "--size", &format!("{gb}G")]),
        None => cmd.arg("--whole-disk"),
    };
    if password.is_some() {
        cmd.arg("--password-stdin");
    }
    steps(cmd, password.map(|p| format!("{p}\n")).unwrap_or_default())
}

/// Install namiOS from its ISO onto `disk` (`nami install-to`, as root): next to Windows with
/// `next_to_windows` bytes, or the whole disk; `person` is who's first (`--person-stdin`'s
/// JSON), and `seed` a computer to install (a drop-in's folder).
pub fn install_namios(disk: &str, next_to_windows: Option<u64>, person: &serde_json::Value, seed: Option<&str>) -> UnboundedReceiver<Event> {
    let mut cmd = nami_as_root();
    cmd.args(["install-to", disk, "--yes", "--json", "--person-stdin"]);
    match next_to_windows {
        Some(bytes) => cmd.args(["--next-to-windows", "--size", &format!("{}M", bytes / 1_000_000)]),
        None => cmd.arg("--whole-disk"),
    };
    if let Some(s) = seed {
        cmd.arg("--seed").arg(s);
    }
    steps(cmd, format!("{person}\n"))
}

/// Run `cmd` (nami with --json), `input` on its stdin: its steps come on
/// the channel, `Finished` last.
fn steps(mut cmd: Command, input: String) -> UnboundedReceiver<Event> {
    let (tx, rx) = unbounded();
    std::thread::spawn(move || {
        let mut child = match cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.unbounded_send(Event::Finished(Err(format!("can't run nami: {e}"))));
                return;
            }
        };
        let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
        // What it says on stderr: the last lines tell what went wrong.
        let stderr = child.stderr.take().unwrap();
        let said = std::thread::spawn(move || BufReader::new(stderr).lines().map_while(Result::ok).collect::<Vec<_>>());
        for line in BufReader::new(child.stdout.take().unwrap()).lines().map_while(Result::ok) {
            if let Ok(l) = serde_json::from_str::<Line>(&line) {
                let _ = tx.unbounded_send(Event::Step { step: l.step, percent: l.percent, message: l.message });
            }
        }
        let said = said.join().unwrap_or_default();
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        let end = if ok {
            Ok(())
        } else {
            let said: Vec<_> = said.into_iter().filter(|l| !l.trim().is_empty()).collect();
            let tail = &said[said.len().saturating_sub(6)..];
            Err(if tail.is_empty() { "The process stopped unexpectedly.".to_string() } else { tail.join("\n") })
        };
        let _ = tx.unbounded_send(Event::Finished(end));
    });
    rx
}

// ---------- this computer: its files, applying them, its history ----------

/// Who you are here: an admin (system.nami is yours to change) or
/// not, system.nami, and yours (~/user.nami, or where it links).
#[derive(Clone)]
pub struct Role {
    pub admin: bool,
    /// A child on the computer (its children group): an admin chooses their apps.
    pub child: bool,
    pub system: PathBuf,
    pub user: PathBuf,
}

fn text(args: &[&str]) -> Result<String, String> {
    let out = nami().args(args).output().map_err(|e| format!("can't run nami: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A request to change the computer (`nami propose`) declined: discarded, and whoever asked is told.
pub fn reject_request(id: &str) -> Result<(), String> {
    text(&["proposals", "reject", id]).map(|_| ())
}

pub fn role() -> Result<Role, String> {
    // system.nami (`nami config path --system`) and yours. (Both at once:
    // Settings' window waits for this.)
    let (file, user) = std::thread::scope(|s| {
        let user = s.spawn(|| text(&["config", "path", "--personal"]));
        (text(&["config", "path", "--system"]), user.join().unwrap_or_else(|_| Err("nami stopped".into())))
    });
    let (file, user) = (PathBuf::from(file?), PathBuf::from(user?));
    let admin = is_admin();
    let system = if admin { file } else { existing(PathBuf::from("/etc/nami/system.nami")) };
    let me = std::env::var("USER").unwrap_or_default();
    let child = std::fs::read_to_string("/etc/group").unwrap_or_default().lines().any(|l| {
        let f: Vec<&str> = l.split(':').collect();
        f.len() >= 4 && f[0] == "children" && f[3].split(',').any(|m| m == me)
    });
    Ok(Role { admin, child, system, user })
}

/// What a nami file says (its own lines), as JSON.
pub fn get(file: &Path) -> Result<serde_json::Value, String> {
    soon(nami().args(["config", "get", "--json", "--file"]).arg(file), None)
}

/// What the window itself waits for: nami's quick with these, and this stops it if it ever isn't.
fn soon(command: &mut Command, input: Option<&str>) -> Result<serde_json::Value, String> {
    let limit = std::time::Duration::from_secs(5);
    let out = match input {
        Some(input) => crate::bounded::told(command, input, limit),
        None => crate::bounded::output(command, limit),
    }
    .ok_or("nami didn't answer")?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("nami config: {e}"))
}

/// What a file's text says (as `get` of a file with it): how a changed
/// file was when it was applied.
pub fn get_text(text: &str) -> Result<serde_json::Value, String> {
    soon(nami().args(["config", "get", "--json", "--file", "-"]), Some(text))
}

/// Change a nami file, keeping its comments: `op` is `nami config json`'s
/// ({"add": {"path": ["packages"], "value": "gimp"}}, set, unset, remove).
/// The edit goes on stdin, so a secret in it never shows in a process list.
pub fn edit(file: &Path, op: serde_json::Value) -> Result<(), String> {
    let before = std::fs::read_to_string(file).ok();
    let out = crate::bounded::told(nami().args(["config", "json", "-", "--file"]).arg(file), &op.to_string(), std::time::Duration::from_secs(if writable(file) { 10 } else { 120 })).ok_or("nami didn't answer")?;
    if !out.status.success() {
        return Err(said(&out.stderr));
    }
    noted(file, before, Some(op));
    Ok(())
}

/// A file the window just wrote: which, how it was and is, and the edit (if it was one).
pub struct Written {
    pub file: PathBuf,
    pub before: Option<String>,
    pub after: Option<String>,
    pub op: Option<serde_json::Value>,
}

static WRITTEN: std::sync::Mutex<Vec<Written>> = std::sync::Mutex::new(Vec::new());

fn noted(file: &Path, before: Option<String>, op: Option<serde_json::Value>) {
    let after = std::fs::read_to_string(file).ok();
    if let Ok(mut w) = WRITTEN.lock() {
        // (Only the last few: what never asks, the desktop, keeps none to speak of.)
        if w.len() >= 8 {
            w.remove(0);
        }
        w.push(Written { file: file.to_path_buf(), before, after, op });
    }
}

/// What was written since asked last (Settings says each change's line).
pub fn written() -> Vec<Written> {
    WRITTEN.lock().map(|mut w| std::mem::take(&mut *w)).unwrap_or_default()
}

/// The same edits made to a file's text, not the file: the text after.
pub fn edit_text(text: &str, ops: &[serde_json::Value]) -> Result<String, String> {
    let mut text = text.to_string();
    for op in ops {
        let mut child = nami().args(["config", "json", &op.to_string(), "--file", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("can't run nami: {e}"))?;
        child.stdin.take().map(|mut i| i.write_all(text.as_bytes()));
        let out = child.wait_with_output().map_err(|e| format!("can't run nami: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        text = String::from_utf8_lossy(&out.stdout).into_owned();
    }
    Ok(text)
}

/// Write a nami file's whole text. system.nami isn't the admins' to
/// write themselves: nami writes it for them once they've given their
/// password (`nami config write`, through its daemon); any other is written here.
pub fn write(file: &Path, text: &str) -> Result<(), String> {
    let before = std::fs::read_to_string(file).ok();
    let done = write_only(file, text);
    if done.is_ok() {
        noted(file, before, None);
    }
    done
}

fn write_only(file: &Path, text: &str) -> Result<(), String> {
    let own =std::fs::OpenOptions::new().write(true).open(file).map(drop);
    match own {
        Ok(()) => std::fs::write(file, text).map_err(|e| format!("writing {}: {e}", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let mut child = nami().args(["config", "write", "--file"]).arg(file).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| format!("can't run nami: {e}"))?;
            child.stdin.take().map(|mut i| i.write_all(text.as_bytes()));
            let out = child.wait_with_output().map_err(|e| format!("can't run nami: {e}"))?;
            if out.status.success() { Ok(()) } else { Err(said(&out.stderr)) }
        }
        Err(_) => std::fs::write(file, text).map_err(|e| format!("writing {}: {e}", file.display())),
    }
}

/// Whether this person may write `file` themselves (system.nami is root's).
pub fn writable(file: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(file.as_os_str().as_encoded_bytes()) else { return false };
    // SAFETY: a valid C path.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// An admin's password, for changes to the computer's `file`: the desktop asks
/// (saying `about`), and the changes of the next few minutes don't ask again.
/// (Someone who isn't an admin picks an admin and types theirs: Settings' Unlock.)
/// Waits for them to answer, so away from the window.
pub fn unlock(file: &Path, about: &str) -> Result<(), String> {
    let out = nami().args(["config", "unlock", about, "--file"]).arg(file).stdin(Stdio::null()).output().map_err(|e| format!("can't run nami: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(said(&out.stderr)) }
}

/// Settings locked again: what polkit keeps of an admin's password given in this session goes
/// too, so nothing more is changed without it. (polkit's own tool; nothing if it isn't here.)
pub fn forget_password() {
    // (Never from the tests: they don't touch the computer they run on.)
    if cfg!(test) {
        return;
    }
    let tool = ["/run/current-system/sw/bin/pkcheck", "/usr/bin/pkcheck", "/bin/pkcheck"].into_iter().find(|p| Path::new(p).is_file());
    if let Some(pkcheck) = tool {
        let _ = Command::new(pkcheck).arg("--revoke-temp").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    }
}

/// A nami file's whole text: read here, or (the computer's, for someone who isn't an admin,
/// unlocked) through nami's daemon while the admin's password given a moment ago is kept.
pub fn read_text(file: &Path) -> std::io::Result<String> {
    match std::fs::read_to_string(file) {
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let out = crate::bounded::output(nami().args(["config", "get", "--text", "--file"]).arg(file), std::time::Duration::from_secs(10)).ok_or(e)?;
            if out.status.success() { Ok(String::from_utf8_lossy(&out.stdout).into_owned()) } else { Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, said(&out.stderr))) }
        }
        other => other,
    }
}

/// A new ~/user.nami, for one that isn't there yet: what `nami install` writes from the
/// command line (cmd/files.rs, ensure_user_config): what you have installed, from `nami export`,
/// under its first line. (Without it, the app's first install had no file to add the app to.)
pub fn new_user_text() -> String {
    let have = crate::bounded::output(nami().arg("export"), std::time::Duration::from_secs(10))
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    new_user_text_from(&have)
}

fn new_user_text_from(export: &str) -> String {
    let body: Vec<&str> = export.lines().filter(|l| !l.starts_with("# Generated")).collect();
    let body = if body.iter().all(|l| l.trim().is_empty()) { "packages:".to_string() } else { body.join("\n") };
    format!("# Your apps and settings; `nami apply` makes your profile match.\n{body}\n")
}

/// What nami said when it stopped, without its "error: ".
fn said(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default().trim();
    last.strip_prefix("error: ").unwrap_or(last).to_string()
}

/// Encrypt the file's plain `!secret`s (`nami secret seal`).
pub fn seal(file: &Path) -> Result<(), String> {
    // (system.nami: nami sealed it as it wrote it, for them.)
    if std::fs::OpenOptions::new().write(true).open(file).is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied) {
        return Ok(());
    }
    text(&["secret", "seal", &file.to_string_lossy()]).map(|_| ())
}

/// What a command says as it runs.
pub enum Said {
    Line(String),
    /// It's over: what went wrong (its last lines), if something did.
    Finished(Result<(), String>),
}

/// Run nami with `args` (and `input` on its stdin): what it says comes on
/// the channel a line at a time, `Finished` last.
pub fn run(args: Vec<String>, input: Option<String>) -> UnboundedReceiver<Said> {
    let (tx, rx) = unbounded();
    std::thread::spawn(move || {
        let mut cmd = nami();
        cmd.args(&args).stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.unbounded_send(Said::Finished(Err(format!("can't run nami: {e}"))));
                return;
            }
        };
        if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
            let _ = stdin.write_all(text.as_bytes());
        }
        let out = child.stdout.take().unwrap();
        let tx2 = tx.clone();
        let t = std::thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = tx2.unbounded_send(Said::Line(l));
            }
        });
        let mut last = Vec::new();
        for l in BufReader::new(child.stderr.take().unwrap()).lines().map_while(Result::ok) {
            if !l.trim().is_empty() {
                last.push(l.clone());
            }
            let _ = tx.unbounded_send(Said::Line(l));
        }
        let _ = t.join();
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        let end = if ok { Ok(()) } else { Err(last[last.len().saturating_sub(6)..].join("\n")) };
        let _ = tx.unbounded_send(Said::Finished(end));
    });
    rx
}

#[derive(Deserialize, Clone)]
pub struct Entry {
    pub attr: String,
}

/// A step of `nami history`: what changed from the one before.
#[derive(Deserialize, Clone)]
pub struct Step {
    pub generation: u64,
    pub date: String,
    /// When, in seconds (0: no date of its own); `date` is the same in UTC.
    #[serde(default)]
    pub created: u64,
    pub current: bool,
    pub added: Vec<Entry>,
    pub removed: Vec<Entry>,
    pub changed: Vec<Entry>,
    pub files: usize,
}

impl Step {
    /// When it was made, in this computer's time: `2026-11-07 08:19` (empty: no date of its own).
    pub fn when(&self) -> String {
        #[cfg(unix)]
        if self.created > 0 {
            let tm = crate::clock::local(self.created as libc::time_t);
            return format!("{}-{:02}-{:02} {:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min);
        }
        self.date.clone()
    }
}

pub fn history() -> Result<Vec<Step>, String> {
    serde_json::from_value(soon(nami().args(["history", "--json"]).args(system_side()), None)?).map_err(|e| format!("nami history: {e}"))
}

/// The history this window shows and goes back in: on namiOS an admin's is the computer's
/// steps (`--system`, as `sudo nami history` is), anyone else's their own.
pub fn system_side() -> &'static [&'static str] {
    static IT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *IT.get_or_init(|| crate::look::on_namios() && is_admin()) { &["--system"] } else { &[] }
}

/// Whether whoever runs this is an admin, as nami counts them (src/profile.rs:
/// `is_admin_user`): their name in wheel, sudo or admin in /etc/group, or one of those
/// their primary group (from /etc/passwd). Root isn't: it needs no daemon. Nobody is on a Mac,
/// as nami has it there: the computer's apps are changed with sudo (there's no polkit to ask).
pub fn is_admin() -> bool {
    if crate::look::MAC {
        return false;
    }
    #[cfg(unix)]
    {
        let uid = unsafe { libc::getuid() };
        if uid == 0 {
            return false;
        }
        let Some((name, gid)) = std::fs::read_to_string("/etc/passwd").ok().and_then(|p| {
            p.lines().find_map(|l| {
                let f: Vec<&str> = l.split(':').collect();
                (f.len() >= 4 && f[2].parse::<u32>().ok() == Some(uid)).then(|| Some((f[0].to_string(), f[3].parse::<u32>().ok()?))).flatten()
            })
        }) else { return false };
        std::fs::read_to_string("/etc/group").unwrap_or_default().lines().any(|l| {
            let f: Vec<&str> = l.split(':').collect();
            f.len() >= 4 && ["wheel", "sudo", "admin"].contains(&f[0]) && (f[3].split(',').any(|m| m == name) || f[2].parse::<u32>().ok() == Some(gid))
        })
    }
    #[cfg(not(unix))]
    false
}

/// What's different between two steps' files (`nami diff --json`).
#[derive(Deserialize, Clone)]
pub struct StepDiff {
    #[serde(default)]
    pub files: Vec<crate::file_diff::FileDiff>,
}

/// What going from step `from` to step `to` changes in the files, in words and raw.
pub fn diff(from: u64, to: u64) -> Result<StepDiff, String> {
    serde_json::from_value(soon(nami().args(["diff", &from.to_string(), &to.to_string(), "--json"]).args(system_side()), None)?).map_err(|e| format!("nami diff: {e}"))
}

/// A problem `nami check` finds, on a line.
#[derive(Deserialize, Clone)]
pub struct Problem {
    pub line: usize,
    pub message: String,
    pub severity: String,
    /// What to do about it, when nami knows ("did you mean `ripgrep`?").
    #[serde(default)]
    pub hint: Option<String>,
}

impl Problem {
    /// As it's said: what's wrong, and what to do in brackets.
    pub fn said(&self) -> String {
        match self.hint.as_deref().filter(|h| !h.is_empty()) {
            Some(h) => format!("{} ({h})", self.message),
            None => self.message.clone(),
        }
    }
}

/// What `nami check` says of `text`, as the file `name` (user.nami,
/// system.nami, package.nami: what it may say) in the folder `base`.
pub fn check(text: &str, name: &str, base: &Path) -> Result<Vec<Problem>, String> {
    let dir = std::env::temp_dir().join(format!("nami-gui-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(name);
    std::fs::write(&file, text).map_err(|e| e.to_string())?;
    let out = nami().args(["check", &file.to_string_lossy(), "--base", &base.to_string_lossy(), "--json"]).output().map_err(|e| format!("can't run nami: {e}"))?;
    // (Problems found: it says so, and fails.)
    serde_json::from_slice(&out.stdout).map_err(|_| String::from_utf8_lossy(&out.stderr).trim().to_string())
}

/// What `nami report` would send, as it shows it.
pub fn report_preview(message: &str, files: bool) -> Result<String, String> {
    let mut args = vec!["report", "--show", "--message", message];
    if !files {
        args.push("--no-files");
    }
    text(&args)
}

/// Send a report: its number.
pub fn report_send(message: &str, files: bool) -> Result<String, String> {
    #[derive(Deserialize)]
    struct Sent {
        id: String,
    }
    let mut args = vec!["report", "--yes", "--json", "--message", message];
    if !files {
        args.push("--no-files");
    }
    json::<Sent>(&args).map(|s| s.id)
}

/// Nami files changed since they were applied (`nami status`): each, and
/// its text as applied.
pub fn pending() -> Vec<(PathBuf, String)> {
    #[derive(Deserialize)]
    struct Pending {
        path: PathBuf,
        applied: String,
    }
    #[derive(Deserialize)]
    struct Status {
        #[serde(default)]
        pending: Vec<Pending>,
    }
    json::<Status>(&["status", "--json"]).map(|s| s.pending.into_iter().map(|p| (p.path, p.applied)).collect()).unwrap_or_default()
}

/// A newer version of an installed app, in the catalog.
#[derive(Deserialize, Clone)]
pub struct Update {
    pub attr: String,
    pub from: String,
    pub to: String,
}

/// The apps with newer versions in the catalog (`nami upgrade --dry-run`).
pub fn updates() -> Result<Vec<Update>, String> {
    json(&["upgrade", "--dry-run", "--json"])
}

/// Whether there's a newer nami (`nami self-update --check`): what's new in the latest (its
/// release notes, also once it's this one), and how big its download is.
#[derive(Deserialize, Clone)]
pub struct NamiVersion {
    pub current: String,
    pub latest: String,
    pub newer: bool,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

pub fn nami_version() -> Result<NamiVersion, String> {
    json(&["self-update", "--check", "--json"])
}

/// The newest catalog, fetched (`nami update --refresh`; through the daemon for anyone who
/// isn't root): what Updates looks for updates in.
pub fn refresh_catalog() -> Result<(), String> {
    let out = nami().args(["update", "--refresh"]).stdin(Stdio::null()).output().map_err(|e| format!("can't run nami: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

/// When the catalog on this computer was fetched.
pub fn catalog_date() -> Option<String> {
    let v: serde_json::Value = json(&["status", "--json"]).ok()?;
    v["catalog"]["updated"].as_str().map(String::from)
}

/// The disks this computer could go onto (`nami install-to --list`).
#[derive(Deserialize, Clone)]
pub struct InstallDisk {
    pub device: String,
    pub model: String,
    pub bytes: u64,
    /// "none", "windows" (on a GPT disk: it can stay), "old" (MBR: it can't).
    pub windows: String,
    pub fits: bool,
    /// With Windows on it, asked as root (`install_disks_as_root`): the room next to it.
    #[serde(default)]
    pub next_to_windows: Option<Room>,
}

/// How much of a disk with Windows on it namiOS can have: up to `most` bytes, `default`
/// offered first; Windows' C: is `windows` bytes now. Or why none (`problem`: BitLocker,
/// Windows asleep, no room).
#[derive(Deserialize, Clone, Default)]
pub struct Room {
    #[serde(default)]
    pub most: u64,
    #[serde(default)]
    pub default: u64,
    #[serde(default)]
    pub windows: u64,
    pub problem: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct InstallDisks {
    pub needs: u64,
    pub disks: Vec<InstallDisk>,
}

pub fn install_disks() -> Result<InstallDisks, String> {
    json(&["install-to", "--list", "--json"])
}

/// nami, as root: through pkexec (namiOS's session from its ISO may, without a password).
fn nami_as_root() -> Command {
    let nami = ["/nami/bin/nami", "/nix/var/nami/bin/nami", "/run/current-system/sw/bin/nami"].into_iter().find(|p| Path::new(p).is_file()).map(std::ffi::OsString::from).unwrap_or_else(program);
    let mut c = Command::new("pkexec");
    c.arg(nami);
    c
}

/// The disks, as Install namiOS sees them (from the ISO): as root, so that how much room each
/// has next to Windows is said too (read-only).
pub fn install_disks_as_root() -> Result<InstallDisks, String> {
    let out = nami_as_root().args(["install-to", "--list", "--json"]).stdin(Stdio::null()).output().map_err(|e| format!("can't run nami: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("nami install-to --list: {e}"))
}

/// Running from a USB drive nami made, and a computer nami made (its kernel and the like are
/// nami's): `nami status`, asked once for both.
pub fn stick_and_nami_os() -> (bool, bool) {
    let v = json::<serde_json::Value>(&["status", "--json"]).unwrap_or_default();
    (v["from_stick"].as_bool().unwrap_or(false), v["nami_os"].as_bool().unwrap_or(false))
}

/// This computer's version, and the one waiting for its next start (update-notify's
/// `state` says, as the computer starts and after each update).
pub fn os_versions() -> (Option<u64>, Option<u64>) {
    let version = |p: &str| std::fs::read_link(p).ok()?.to_str()?.rsplit("system-").next()?.parse().ok();
    let now: Option<u64> = version("/run/current-system");
    let new = std::fs::read_to_string("/run/nami-update/pending").ok().and_then(|t| t.trim().parse().ok());
    (now, new.filter(|n| now.is_none_or(|c| *n > c)))
}

/// nami's version as this computer started, and the one that made the version waiting for its
/// next start (update-notify's `state` writes both): What's New, after an update, before the
/// restart.
pub fn os_nami_versions() -> (Option<String>, Option<String>) {
    let read = |f: &str| std::fs::read_to_string(format!("/run/nami-update/{f}")).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    (read("started"), read("version"))
}

/// Running from a USB drive nami made (`nami status`): it can go on a disk.
pub fn from_stick() -> bool {
    json::<serde_json::Value>(&["status", "--json"]).ok().and_then(|v| v["from_stick"].as_bool()).unwrap_or(false)
}

/// A project you've used (`nami project list`): its folder, tools and tasks.
#[derive(Deserialize, Clone)]
pub struct Project {
    pub dir: String,
    /// Its project.nami.
    #[serde(default)]
    pub file: String,
    pub name: String,
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default)]
    pub tasks: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub services: Vec<String>,
}

pub fn projects() -> Result<Vec<Project>, String> {
    json(&["project", "list", "--json"])
}

/// A project's service and how it is (`nami project services`, in its folder).
#[derive(Deserialize, Clone)]
pub struct Service {
    pub name: String,
    pub state: String,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub ports: Vec<u16>,
}

pub fn services(dir: &str) -> Vec<Service> {
    // (A Mac's nami doesn't run a project's services yet: none is running.)
    if crate::look::MAC {
        return Vec::new();
    }
    nami().args(["project", "services", "--json"]).current_dir(dir).output().ok().filter(|o| o.status.success()).and_then(|o| serde_json::from_slice(&o.stdout).ok()).unwrap_or_default()
}

/// A project's task running by itself (`nami run`), in a process group of
/// its own so Stop ends what it started too: what it says, and its group.
pub fn run_task(dir: &str, task: &str) -> (UnboundedReceiver<Said>, Option<u32>) {
    grouped(&["run", "--project", dir, task], dir)
}

/// A project's service's output, followed (`nami logs`) until it's stopped: what it says, and
/// its group.
pub fn follow_logs(dir: &str, service: &str) -> (UnboundedReceiver<Said>, Option<u32>) {
    grouped(&["logs", service, "--project", dir], dir)
}

/// `nami` with `args` in `dir`, in a process group of its own: what it says, and its group.
fn grouped(args: &[&str], dir: &str) -> (UnboundedReceiver<Said>, Option<u32>) {
    use std::os::unix::process::CommandExt;
    let (tx, rx) = unbounded();
    let mut cmd = nami();
    cmd.args(args).current_dir(dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.unbounded_send(Said::Finished(Err(format!("can't run nami: {e}"))));
            return (rx, None);
        }
    };
    let pid = child.id();
    let (out, err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let tx2 = tx.clone();
    std::thread::spawn(move || {
        for l in BufReader::new(out).lines().map_while(Result::ok) {
            let _ = tx2.unbounded_send(Said::Line(l));
        }
    });
    std::thread::spawn(move || {
        for l in BufReader::new(err).lines().map_while(Result::ok) {
            let _ = tx.unbounded_send(Said::Line(l));
        }
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        let _ = tx.unbounded_send(Said::Finished(if ok { Ok(()) } else { Err(String::new()) }));
    });
    (rx, Some(pid))
}

/// Stop a task: its whole process group.
pub fn stop_task(group: u32) {
    // (A Mac's kill takes no `--`: the group is told by itself.)
    #[cfg(target_os = "macos")]
    {
        // SAFETY: a signal to a process group of ours.
        unsafe { libc::kill(-(group as libc::pid_t), libc::SIGTERM) };
        return;
    }
    #[allow(unreachable_code)]
    let _ = Command::new("kill").args(["-TERM", "--", &format!("-{group}")]).status();
}

/// What cleaning up now would free (`nami gc --now --dry-run`: by `cleanup:`'s rules,
/// without waiting): old steps, and downloads nothing uses any more.
#[derive(Deserialize, Clone, Default, Debug)]
pub struct Cleanup {
    pub steps: Vec<u64>,
    pub paths: usize,
    pub bytes: u64,
    /// How many of the newest steps stay (`cleanup:`'s keep).
    #[serde(default = "three")]
    pub keep: usize,
}

fn three() -> usize {
    3
}

pub fn cleanup() -> Result<Cleanup, String> {
    json(&["gc", "--now", "--dry-run", "--json"])
}

/// Clean Up Now: what `cleanup()` said would go.
pub fn clean_up_now() -> Vec<(Vec<String>, Option<String>)> {
    vec![(vec!["gc".into(), "--now".into()], None)]
}

/// What `nami storage --json` says: the disk nami's store is on, what nami takes of it in parts
/// (each path counted once), the apps that take the most, and what Clean Up frees.
#[derive(Deserialize, Clone, Default, Debug, PartialEq)]
pub struct Storage {
    pub disk: Disk,
    pub total: u64,
    pub parts: Vec<StoragePart>,
    #[serde(default)]
    pub largest: Vec<BigApp>,
    pub cleanup: Freeing,
}

#[derive(Deserialize, Clone, Default, Debug, PartialEq)]
pub struct Disk {
    pub size: u64,
    pub used: u64,
    pub free: u64,
    /// A Mac's: the numbers are its APFS container's, which the Nix Store volume shares.
    #[serde(default)]
    pub container: bool,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct StoragePart {
    /// yours, everyone, projects, others, running, older, unused, casks, nami.
    pub part: String,
    pub bytes: u64,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct BigApp {
    pub attr: String,
    /// "you" or "system".
    pub whose: String,
    /// What removing it frees (once its older changes go too).
    pub adds: u64,
}

/// What Clean Up frees: your old changes and (where it's freed now) the computer's, past the
/// newest `cleanup:` keeps, and what nothing uses then.
#[derive(Deserialize, Clone, Default, Debug, PartialEq)]
pub struct Freeing {
    #[serde(default)]
    pub yours: Vec<u64>,
    #[serde(default)]
    pub computers: Vec<u64>,
    pub paths: usize,
    pub bytes: u64,
    #[serde(default = "three")]
    pub keep: usize,
    /// Whether Clean Up frees the space itself (an administrator; on a Mac, with one's password).
    #[serde(default)]
    pub now: bool,
}

pub fn storage() -> Result<Storage, String> {
    json(&["storage", "--json"])
}

// ---------- backups (the backup package's nami-backup) ----------

/// What `nami-backup status` says: where to, the last backup, the days.
#[derive(Deserialize, Clone, Default)]
pub struct Backups {
    /// None: not set up.
    pub to: Option<String>,
    #[serde(default)]
    pub when: String,
    /// When the last backup finished, and the last try, in seconds.
    pub done: Option<u64>,
    pub tried: Option<u64>,
    /// The first try on this computer.
    pub since: Option<u64>,
    /// The newest backup from before this computer was made (again).
    pub before: Option<BackupDay>,
    #[serde(default)]
    pub said: String,
    /// Newest first.
    #[serde(default)]
    pub days: Vec<BackupDay>,
}

#[derive(Deserialize, Clone)]
pub struct BackupDay {
    pub id: String,
    /// `2026-10-26T11:31:39.15+01:00`
    pub time: String,
    pub files: Option<u64>,
    pub bytes: Option<u64>,
}

fn nami_backup() -> Command {
    Command::new(std::env::var_os("NAMI_BACKUP").unwrap_or_else(|| "nami-backup".into()))
}

/// The computer's backups; Ok(default) when it has no nami-backup.
pub fn backups() -> Result<Backups, String> {
    let out = match nami_backup().arg("status").output() {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Backups::default()),
        Err(e) => return Err(format!("can't run nami-backup: {e}")),
    };
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("nami-backup status: {e}"))
}

/// A USB disk plugged in, with a label to back up to.
#[derive(Clone)]
pub struct UsbDisk {
    pub label: String,
    pub bytes: u64,
}

pub fn usb_disks() -> Vec<UsbDisk> {
    let Ok(out) = Command::new("lsblk").args(["-J", "-b", "-o", "LABEL,SIZE,HOTPLUG,TYPE"]).output() else { return Vec::new() };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else { return Vec::new() };
    fn walk(v: &serde_json::Value, hot: bool, out: &mut Vec<UsbDisk>) {
        for d in v.as_array().into_iter().flatten() {
            let hot = hot || d["hotplug"].as_bool() == Some(true) || d["hotplug"].as_str() == Some("1");
            if let Some(label) = d["label"].as_str().filter(|l| hot && !l.is_empty()) {
                out.push(UsbDisk { label: label.to_string(), bytes: d["size"].as_u64().or_else(|| d["size"].as_str().and_then(|s| s.parse().ok())).unwrap_or(0) });
            }
            walk(&d["children"], hot, out);
        }
    }
    let mut disks = Vec::new();
    walk(&v["blockdevices"], false, &mut disks);
    disks
}

/// Back up now (`systemctl start nami-backup-now`: an admin's password, through polkit).
pub fn back_up_now() -> Result<(), String> {
    let out = Command::new("systemctl").args(["start", "nami-backup-now.service"]).output().map_err(|e| format!("can't run systemctl: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

/// Each person's own (`nami backup`, src/backup.rs): on, where to, the last backup, the space,
/// what's left out (`nami backup status --json`).
pub fn my_backups() -> Result<serde_json::Value, String> {
    json(&["backup", "status", "--json"])
}

/// A backup of the person's home now (as them: no admin), and what it said.
pub fn my_backup_now() -> Result<String, String> {
    let out = nami().args(["backup", "now"]).output().map_err(|e| format!("can't run nami: {e}"))?;
    if out.status.success() { Ok(String::from_utf8_lossy(&out.stdout).trim().to_string()) } else { Err(said(&out.stderr)) }
}

/// Bring `paths` back as backup `id` has them (as root, through pkexec).
pub fn restore(id: &str, paths: &[String]) -> Result<(), String> {
    let bin = std::env::var("NAMI_BACKUP").unwrap_or_else(|_| "/run/current-system/sw/bin/nami-backup".into());
    let out = Command::new("pkexec").arg(bin).arg("restore").arg(id).args(paths).output().map_err(|e| format!("can't run pkexec: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().lines().last().unwrap_or("The restore failed.").to_string()) }
}

// ---------- bringing your files (nami import <disk> --home) ----------

/// A disk with an old computer on it, or with files copied for nami.
#[derive(Clone)]
pub struct OldDisk {
    pub device: String,
    pub label: Option<String>,
    pub bytes: u64,
    pub usb: bool,
    /// Where it's open (None: not yet: Open first).
    pub root: Option<String>,
    /// windows, linux or copied; None while it isn't open.
    pub kind: Option<String>,
    /// `Ubuntu 22.04.4 LTS`, `Windows`, whose files.
    pub name: Option<String>,
}

impl OldDisk {
    pub fn title(&self) -> String {
        match (self.kind.as_deref(), &self.name) {
            (Some("copied"), Some(who)) => format!("Copy of {who}'s files"),
            (_, Some(n)) => n.clone(),
            _ => self.label.clone().unwrap_or_else(|| "Disk".into()),
        }
    }

    pub fn line(&self) -> String {
        let size = if self.bytes >= 1_000_000_000_000 { format!("{:.1} TB", self.bytes as f64 / 1e12) } else { format!("{} GB", self.bytes / 1_000_000_000) };
        let label = self.label.clone().filter(|l| Some(l) != self.name.as_ref());
        let what = match (&label, self.usb) {
            (Some(l), true) => format!("USB disk “{l}”"),
            (None, true) => "USB disk".into(),
            (Some(l), false) => format!("“{l}” on this computer"),
            (None, false) => "On this computer".into(),
        };
        let open = if self.root.is_none() { " · Not mounted" } else { "" };
        format!("{what} · {size}{open}")
    }
}

/// What a folder is: an old computer, or a copy of someone's files.
fn kind_of(root: &Path) -> Option<(String, String, String)> {
    let at = root.display().to_string();
    if root.join("Windows/System32").is_dir() && root.join("Users").is_dir() {
        return Some(("windows".into(), "Windows".into(), at));
    }
    if root.join("var/lib/dpkg/status").is_file() {
        let release = std::fs::read_to_string(root.join("etc/os-release")).unwrap_or_default();
        let name = release.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).map(|n| n.trim_matches('"').to_string()).unwrap_or_else(|| "Linux".into());
        return Some(("linux".into(), name, at));
    }
    // A copy: at the top, or one folder down (`--home /media/disk/home`).
    let copied = |dir: &Path| std::fs::read_to_string(dir.join(".nami-home")).ok().map(|w| ("copied".to_string(), w.trim().to_string(), dir.display().to_string()));
    copied(root).or_else(|| std::fs::read_dir(root).ok()?.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).find_map(|e| copied(&e.path())))
}

/// The disks with an old computer (or files copied for nami) on them,
/// and the ones not open yet that could have one.
pub fn old_disks() -> Vec<OldDisk> {
    let Ok(out) = Command::new("lsblk").args(["-J", "-b", "-o", "PATH,LABEL,FSTYPE,SIZE,MOUNTPOINT,TYPE,HOTPLUG"]).output() else { return Vec::new() };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else { return Vec::new() };
    let mut found = Vec::new();
    fn walk(v: &serde_json::Value, usb: bool, out: &mut Vec<OldDisk>) {
        for d in v.as_array().into_iter().flatten() {
            let usb = usb || d["hotplug"].as_bool() == Some(true);
            let s = |k: &str| d[k].as_str().filter(|x| !x.is_empty()).map(String::from);
            let bytes = d["size"].as_u64().or_else(|| d["size"].as_str().and_then(|x| x.parse().ok())).unwrap_or(0);
            let fs = s("fstype").unwrap_or_default();
            let fits = ["ntfs", "ext4", "ext3", "btrfs", "xfs", "exfat", "f2fs"].contains(&fs.as_str()) || (fs == "vfat" && bytes > 2_000_000_000);
            match s("mountpoint") {
                // This computer's own.
                Some(m) if m == "/" || m.starts_with("/boot") || m.starts_with("/nix") || m == "/home" || m.starts_with('[') => {}
                Some(m) => {
                    if let Some((kind, name, root)) = kind_of(Path::new(&m)) {
                        out.push(OldDisk { device: s("path").unwrap_or_default(), label: s("label"), bytes, usb, root: Some(root), kind: Some(kind), name: Some(name) });
                    }
                }
                None if fits => out.push(OldDisk { device: s("path").unwrap_or_default(), label: s("label"), bytes, usb, root: None, kind: None, name: None }),
                None => {}
            }
            walk(&d["children"], usb, out);
        }
    }
    walk(&v["blockdevices"], false, &mut found);
    found
}

/// Open a disk, as Files does (udisks: no password for a USB disk).
pub fn open_disk(device: &str) -> Result<(), String> {
    let out = Command::new("udisksctl").args(["mount", "--no-user-interaction", "-b", device]).output().map_err(|e| format!("can't run udisksctl: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

/// What's on a disk to bring (`nami import <root> --home --json`).
#[derive(Deserialize, Clone)]
pub struct HomePlan {
    pub kind: String,
    pub person: Option<String>,
    #[serde(default)]
    pub people: Vec<String>,
    #[serde(default)]
    pub items: Vec<HomeItem>,
}

#[derive(Deserialize, Clone)]
pub struct HomeItem {
    pub key: String,
    pub label: String,
    pub about: Option<String>,
    pub bytes: u64,
}

pub fn home_plan(root: &str, person: Option<&str>) -> Result<HomePlan, String> {
    let mut args = vec!["import", root, "--home", "--json"];
    if let Some(p) = person {
        args.extend(["--user", p]);
    }
    json(&args)
}

// ---------- your synced folders (the sync package's nami-sync) ----------

#[derive(Deserialize, Clone, Default)]
pub struct SyncStatus {
    pub id: String,
    #[serde(default)]
    pub folders: Vec<SyncFolder>,
    #[serde(default)]
    pub computers: Vec<SyncComputer>,
}

#[derive(Deserialize, Clone)]
pub struct SyncFolder {
    pub name: String,
    pub path: String,
    /// Syncthing's: idle, syncing, scanning, unknown...
    pub state: String,
    /// Bytes still to come, and in all.
    pub need: u64,
    pub bytes: u64,
}

#[derive(Deserialize, Clone)]
pub struct SyncComputer {
    pub name: String,
    pub id: String,
    pub online: bool,
}

// ---------- your setup on all your computers (`nami sync`) ----------

/// `nami sync status --json`: whether it's on, where, the computers and what came in.
#[derive(Deserialize, Clone, Default)]
pub struct SetupSync {
    pub on: bool,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub computer: Option<String>,
    #[serde(default)]
    pub last: SetupLast,
    #[serde(default)]
    pub computers: Vec<SetupComputer>,
    #[serde(default)]
    pub entries: Vec<SetupEntry>,
    #[serde(default)]
    pub github_cli: bool,
}

#[derive(Deserialize, Clone, Default)]
pub struct SetupLast {
    #[serde(default)]
    pub tried: u64,
    #[serde(default)]
    pub ok: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub apply_error: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct SetupComputer {
    pub name: String,
    #[serde(default)]
    pub system: String,
    #[serde(default)]
    pub last: u64,
    #[serde(default)]
    pub this: bool,
}

/// One thing a sync changed here: "From laptop: added gimp", to undo.
#[derive(Deserialize, Clone)]
pub struct SetupEntry {
    pub id: u64,
    pub when: u64,
    #[serde(default)]
    pub from: Vec<String>,
    pub summary: String,
    #[serde(default)]
    pub clashes: Vec<SetupClash>,
    #[serde(default)]
    pub undone: bool,
}

#[derive(Deserialize, Clone)]
pub struct SetupClash {
    pub path: String,
    #[serde(default)]
    pub ours: Option<String>,
    #[serde(default)]
    pub theirs: Option<String>,
    pub took_theirs: bool,
}

impl SetupEntry {
    /// In a line: `From laptop: added gimp`.
    pub fn said(&self) -> String {
        let what = {
            let mut c = self.summary.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        };
        match self.from.as_slice() {
            [] => what,
            [one] => format!("From {one}: {}", self.summary),
            many => format!("From {}: {}", many.join(" and "), self.summary),
        }
    }
}

pub fn setup_sync() -> Result<SetupSync, String> {
    json(&["sync", "status", "--json"])
}

/// `nami sync on` (`repo`: else GitHub's, signed in to in the browser): its steps, the
/// code to enter on GitHub among them (step `code`).
pub fn setup_sync_on(repo: Option<String>) -> UnboundedReceiver<Event> {
    let mut cmd = nami();
    cmd.args(["sync", "on", "--json"]);
    if let Some(r) = repo {
        cmd.arg(r);
    }
    steps(cmd, String::new())
}

/// `nami sync <args>` (now, off, undo <n>), to its end.
pub fn setup_sync_run(args: &[&str]) -> Result<(), String> {
    let out = nami().arg("sync").args(args).stdin(Stdio::null()).output().map_err(|e| format!("can't run nami: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(())
}

/// `nami-sync status`; None when it isn't here (not set up, or not applied).
pub fn sync_status() -> Option<Result<SyncStatus, String>> {
    let out = match Command::new(std::env::var_os("NAMI_SYNC").unwrap_or_else(|| "nami-sync".into())).arg("status").output() {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => return Some(Err(format!("can't run nami-sync: {e}"))),
    };
    if !out.status.success() {
        return Some(Err(String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    Some(serde_json::from_slice(&out.stdout).map_err(|e| format!("nami-sync status: {e}")))
}

// ---------- this computer (nami doctor --about, --hardware, the checks) ----------

/// `nami doctor --about --json`: what the computer is, as nami says it.
pub fn about() -> Result<serde_json::Value, String> {
    let out = nami().args(["doctor", "--about", "--json"]).output().map_err(|e| format!("can't run nami: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("nami doctor --about: {e}"))
}

/// The packages this computer's hardware is for (`nami doctor --hardware --json`).
pub fn suggestions() -> Vec<Hardware> {
    nami().args(["doctor", "--hardware", "--json"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect()).unwrap_or_default()
}

/// One of the computer's checks (`nami doctor --json`), as it finishes.
#[derive(Deserialize, Clone)]
pub struct Checked {
    pub check: String,
    pub ok: bool,
    pub saw: Option<String>,
}

/// The computer's checks, each sent as it finishes (a line of `nami doctor --json`).
pub fn checks() -> UnboundedReceiver<Checked> {
    let (tx, rx) = unbounded();
    std::thread::spawn(move || {
        // (Without the display: its checks "in the desktop" open windows and switch how they're
        // laid out, which isn't for a page that only looks. `nami doctor` in a terminal has them.)
        let Ok(mut child) = nami().args(["doctor", "--json"]).env_remove("WAYLAND_DISPLAY").env_remove("DISPLAY").stdout(Stdio::piped()).stderr(Stdio::null()).spawn() else { return };
        let Some(out) = child.stdout.take() else { return };
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if let Ok(c) = serde_json::from_str::<Checked>(&line) {
                if tx.unbounded_send(c).is_err() {
                    break;
                }
            }
        }
        let _ = child.wait();
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::Examples;

    #[test]
    fn a_new_user_file_is_what_nami_install_writes() {
        // Nothing installed yet (or no nami to ask): an empty list.
        assert_eq!(super::new_user_text_from(""), "# Your apps and settings; `nami apply` makes your profile match.\npackages:\n");
        assert_eq!(super::new_user_text_from("packages:\n"), "# Your apps and settings; `nami apply` makes your profile match.\npackages:\n");
        // What's installed stays in it, so applying it doesn't take anything away.
        let t = super::new_user_text_from("# Generated by nami\npackages:\n  - ripgrep\n  - cask:rectangle\n");
        assert_eq!(t, "# Your apps and settings; `nami apply` makes your profile match.\npackages:\n  - ripgrep\n  - cask:rectangle\n");
    }

    #[test]
    fn what_sets_up_the_system_is_known_by_its_package() {
        let e: Examples = serde_json::from_value(serde_json::json!({
            "computers": [{ "name": "namios" }],
            "packages": [{ "name": "docker", "computer": true }, { "name": "hello", "computer": false }, { "name": "old" }],
        }))
        .unwrap();
        assert!(e.sets_up_system("docker"));
        // (A nami from before says nothing: one person's, as before.)
        assert!(!e.sets_up_system("hello") && !e.sets_up_system("old") && !e.sets_up_system("gimp"));
    }
}

/// Your file: ~/user.nami, or ~/.config/nami/user.nami when that's where
/// it is (as nami finds it: ~/user.nami wins when both are there).
pub fn user_file() -> std::path::PathBuf {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    let (top, inside) = (existing(home.join("user.nami")), existing(home.join(".config/nami/user.nami")));
    if top.symlink_metadata().is_err() && inside.symlink_metadata().is_ok() { inside } else { top }
}
