//! Layers from git repos: `include: [github:alice/os, github:alice/os/gaming]`.
//!
//! A repo of nami files is a set of layers anyone can build on:
//! `github:owner/repo[/path][@ref]` or `git+<url>[@ref][#path]`, the one
//! form of git references (gitref.rs). The path
//! names `<path>.nami` in the repo (or `<path>/default.nami`); without one
//! it's the repo's `default.nami`. Inside a fetched layer, a name is the
//! repo's own `<name>.nami` first, then one of nami's (so a layer can say
//! `include: [desktop]`), and `./paths` stay inside the repo.
//!
//! A layer is as good as code on the computer (it can add services, files,
//! sudo), so it's pinned: the first time, the ref is looked up, what's
//! included is said (repo and commit), and the commit goes into nami.lock
//! next to the file (`layers:`). From then on that commit is used, also on
//! the computer `nami usb` made (the commits go onto it, so it rebuilds
//! offline). Only `nami layers --update` or `nami apply --upgrade` moves a
//! pin; the computer's automatic updates never do.
//!
//! A project's project.nami includes layers the same way (a team's tools,
//! tasks and services), pinned in the project's nami.lock; `nami shell
//! --upgrade` moves them too, and the cd hook ([`Mode::Offline`]) never
//! fetches: it uses what's pinned and here, else says how to fetch it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};

/// A layer's repo, pinned at a commit (in nami.lock's `layers:`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pin {
    pub url: String,
    /// The branch or tag it follows (none: the repo's default branch).
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    pub rev: String,
}

/// How pins are treated while files are read.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    /// Use pins; pin what's new (and say so).
    Pin = 0,
    /// Use pins; anything not pinned is an error (`--locked`, the
    /// computer's automatic updates).
    Locked = 1,
    /// Move every pin to the newest commit of its ref.
    Update = 2,
    /// Use pins and the files already here; never the network (`nami
    /// hook`, at every prompt). What's missing says how to fetch it.
    Offline = 3,
}

static MODE: AtomicU8 = AtomicU8::new(0);
/// Commits looked up in this run (an update moves each pin once).
static LOOKED_UP: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

pub fn set_mode(m: Mode) {
    MODE.store(m as u8, Ordering::Relaxed);
    *LOOKED_UP.lock().unwrap() = None;
}

fn mode() -> Mode {
    match MODE.load(Ordering::Relaxed) {
        1 => Mode::Locked,
        2 => Mode::Update,
        3 => Mode::Offline,
        _ => Mode::Pin,
    }
}

/// Is this `include:` entry a layer from a git repo?
pub fn is_remote(inc: &str) -> bool {
    inc.starts_with("github:") || inc.starts_with("git+")
}

/// An `include:` entry naming a layer in a git repo.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// The repo (and ref) as nami.lock names it: `github:alice/os`,
    /// `github:alice/os@v2`, `git+https://codeberg.org/bob/os@v2`.
    pub key: String,
    pub url: String,
    pub git_ref: Option<String>,
    /// The layer's path in the repo, without `.nami` ("": the default).
    pub layer: String,
}

/// The repo's own layer when the path names none.
pub const DEFAULT_LAYER: &str = "default";

const FORMS: &str = "github:owner/repo[/layer.nami][@ref], or git+<url>[@ref][#layer.nami]";

fn layer_path(path: &str) -> Result<String> {
    let path = path.trim_matches('/');
    let path = crate::oldnames::strip_extension(path).unwrap_or(path);
    ensure!(crate::gitref::path_ok(path), "{path:?} isn't a layer's path in the repo (like gaming, or layers/gaming)");
    Ok(path.to_string())
}

/// Say once in a run that `written` is an earlier spelling, and the new one.
pub fn say_old(r: &crate::gitref::GitRef, written: &str) {
    static SAID: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    if let Some(n) = r.notice(written) {
        if SAID.lock().unwrap().insert(written.to_string()) {
            crate::say!("{n}");
        }
    }
}

/// `github:owner/repo[/path][@ref]`, or `git+<url>[@ref][#path]`: one
/// grammar with shared tools and config repos (gitref.rs). The earlier
/// `git+<url>#path@ref` works too, with a notice.
pub fn parse(inc: &str) -> Result<Source> {
    ensure!(is_remote(inc), "write it as {FORMS}");
    let r = crate::gitref::parse(inc, crate::gitref::Use::Repo)?;
    say_old(&r, inc);
    let layer = match (r.path.as_str(), r.name.as_deref()) {
        (p, None) => p.to_string(),
        ("", Some(n)) => n.to_string(),
        (p, Some(n)) => format!("{p}/{n}"),
    };
    Ok(Source { key: r.repo_at(), url: r.url(), git_ref: r.git_ref.clone(), layer: layer_path(&layer)? })
}

/// Where fetched layers are kept: each commit's files, as
/// `<slug>-<commit>/`, and `git/` (the repos, to fetch from). Root's
/// (and so a computer's) are nami's own, /nami/layers; that's
/// where `nami usb` puts them on the USB drive.
pub fn own_dir() -> PathBuf {
    if let Ok(d) = std::env::var("NAMI_LAYERS_DIR") {
        return PathBuf::from(d);
    }
    if unsafe { libc::geteuid() } == 0 {
        return system_dir();
    }
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("nami/layers")
}

/// The computer's own layers (what the USB drive came with, and root's).
pub fn system_dir() -> PathBuf {
    crate::migrate::var_dir(&PathBuf::from(std::env::var_os("NAMI_ROOT").unwrap_or_else(|| "/".into()))).join("layers")
}

/// A repo's folder name: readable, and its own.
fn slug(url: &str) -> String {
    let u = url.split_once("://").map_or(url, |(_, r)| r).trim_end_matches('/').trim_end_matches(".git");
    let s: String = u.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect();
    s.trim_matches(|c| c == '_' || c == '.').to_string()
}

/// A commit's files' folder name.
pub fn dir_name(url: &str, rev: &str) -> String {
    format!("{}-{}", slug(url), &rev[..rev.len().min(12)])
}

fn is_commit(r: &str) -> bool {
    r.len() == 40 && r.bytes().all(|b| b.is_ascii_hexdigit())
}

fn git() -> Result<crate::machine::Git> {
    crate::machine::Git::get(&PathBuf::from(std::env::var_os("NAMI_ROOT").unwrap_or_else(|| "/".into())))
}

/// Run git; its last words on failure.
fn run_git(g: &crate::machine::Git, dir: Option<&Path>, args: &[&str]) -> Result<String> {
    let out = g.cmd(dir).args(args).env("GIT_TERMINAL_PROMPT", "0").stdin(std::process::Stdio::null()).output().context("running git")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        // Its first complaint says what's wrong; the rest is advice.
        let why = err.lines().find(|l| l.starts_with("fatal: ") || l.starts_with("error: ")).or_else(|| err.lines().rev().find(|l| !l.trim().is_empty())).unwrap_or("git failed");
        bail!("{}", why.trim().trim_start_matches("fatal: ").trim_start_matches("error: "));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The commit the ref (or the repo's default branch) is at now.
fn look_up(src: &Source) -> Result<String> {
    if let Some(r) = src.git_ref.as_deref().filter(|r| is_commit(r)) {
        return Ok(r.to_lowercase());
    }
    let mut cache = LOOKED_UP.lock().unwrap();
    if let Some(rev) = cache.get_or_insert_with(HashMap::new).get(&src.key) {
        return Ok(rev.clone());
    }
    let want = src.git_ref.as_deref().unwrap_or("HEAD");
    let out = run_git(&git()?, None, &["ls-remote", "--", &src.url, want])
        .map_err(|e| anyhow!("can't reach {} ({e}): check the address, and that the repo is public (a private one needs your git credentials)", src.url))?;
    let refs: Vec<(&str, &str)> = out.lines().filter_map(|l| l.split_once('\t')).collect();
    // A tag's commit (`^{}`) first, then a branch, then the rest.
    let find = |name: String| refs.iter().find(|(_, n)| *n == name).map(|(h, _)| h.to_string());
    let rev = find(format!("refs/tags/{want}^{{}}"))
        .or_else(|| find(format!("refs/heads/{want}")))
        .or_else(|| find(format!("refs/tags/{want}")))
        .or_else(|| find(want.to_string()))
        .ok_or_else(|| if want == "HEAD" { anyhow!("{} has no commits yet", src.url) } else { anyhow!("{} has no branch or tag {want}", src.url) })?;
    ensure!(is_commit(&rev), "{} gave no commit for {want}", src.url);
    cache.as_mut().unwrap().insert(src.key.clone(), rev.clone());
    Ok(rev)
}

/// The commit's files, if they're here already.
fn files_here(src: &Source, rev: &str) -> Option<PathBuf> {
    let name = dir_name(&src.url, rev);
    [own_dir(), system_dir()].into_iter().map(|d| d.join(&name)).find(|d| d.is_dir())
}

/// The commit's files (fetched once, then kept).
fn files_at(src: &Source, rev: &str) -> Result<PathBuf> {
    if let Some(d) = files_here(src, rev) {
        return Ok(d);
    }
    let name = dir_name(&src.url, rev);
    let own = own_dir();
    std::fs::create_dir_all(own.join("git")).with_context(|| format!("making {}", own.display()))?;
    let g = git()?;
    let mirror = own.join("git").join(format!("{}.git", slug(&src.url)));
    let m = mirror.display().to_string();
    if !mirror.join("HEAD").is_file() {
        run_git(&g, None, &["init", "--quiet", "--bare", &m])?;
    }
    let have = |g: &crate::machine::Git| run_git(g, Some(&mirror), &["cat-file", "-e", &format!("{rev}^{{commit}}")]).is_ok();
    if !have(&g) {
        let fetched = run_git(&g, Some(&mirror), &["fetch", "--quiet", "--tags", "--force", "--", &src.url, "+refs/heads/*:refs/heads/*"]);
        if !have(&g) {
            // A commit no branch has any more: ask for it by itself.
            let by_rev = run_git(&g, Some(&mirror), &["fetch", "--quiet", "--", &src.url, rev]);
            if !have(&g) {
                let why = by_rev.err().or(fetched.err()).map(|e| e.to_string()).unwrap_or_else(|| "it isn't there".into());
                bail!("can't fetch {} at {} ({why})", src.url, &rev[..12]);
            }
        }
    }
    let tmp = own.join(format!(".{name}.tmp"));
    let _ = std::fs::remove_dir_all(&tmp);
    let t = tmp.display().to_string();
    run_git(&g, None, &["clone", "--quiet", "--shared", "--no-checkout", "--", &m, &t])?;
    run_git(&g, Some(&tmp), &["-c", "advice.detachedHead=false", "checkout", "--quiet", "--detach", rev])?;
    std::fs::remove_dir_all(tmp.join(".git"))?;
    let dest = own.join(&name);
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

/// The layer's file in a repo's files at `root`.
pub fn layer_file(root: &Path, layer: &str) -> Option<PathBuf> {
    let layer = if layer.is_empty() { DEFAULT_LAYER } else { layer };
    let f = crate::oldnames::find(&root.join(format!("{layer}.nami")));
    if f.is_file() {
        return Some(f);
    }
    Some(crate::oldnames::find(&root.join(layer).join(format!("{DEFAULT_LAYER}.nami")))).filter(|f| f.is_file())
}

/// The layers a repo has at its top (to say which there are).
fn layers_in(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root)
        .map(|d| d.flatten().filter_map(|e| crate::oldnames::strip_extension(e.file_name().to_str()?).map(String::from)).collect())
        .unwrap_or_default();
    names.sort();
    names
}

/// A layer in use: what was included, and where its repo's files are.
#[derive(Debug, Clone)]
pub struct Used {
    pub include: String,
    pub key: String,
    pub pin: Pin,
    pub root: PathBuf,
}

/// Reading one file with what it includes: the pins in its nami.lock,
/// and what changed.
pub struct Session {
    lock: Option<PathBuf>,
    /// A project's file (its layers bring tools, tasks and services, not
    /// the system's apps), for what's said.
    project: bool,
    pins: Option<BTreeMap<String, Pin>>,
    changed: bool,
    pub used: Vec<Used>,
}

impl Session {
    /// For the file in `base` (its nami.lock is there).
    pub fn new(base: &Path) -> Session {
        Session { lock: Some(crate::lock::path_for(&base.join("x"))), project: false, pins: None, changed: false, used: Vec::new() }
    }

    /// For a project's file in `base` (project.nami; its nami.lock is there).
    pub fn project(base: &Path) -> Session {
        Session { project: true, ..Session::new(base) }
    }

    fn pins(&mut self) -> Result<&mut BTreeMap<String, Pin>> {
        if self.pins.is_none() {
            let lock = self.lock.as_ref().map(|l| crate::lock::load(l)).transpose()?;
            self.pins = Some(lock.map(|l| l.layers).unwrap_or_default());
        }
        Ok(self.pins.as_mut().unwrap())
    }

    /// The layer `inc` names: its repo's files at the pinned commit (pinned
    /// now if it's new), and the layer's file in them.
    pub fn fetch(&mut self, inc: &str) -> Result<(PathBuf, PathBuf)> {
        let src = parse(inc)?;
        let lock_name = self.lock.as_ref().map(|l| l.display().to_string()).ok_or_else(|| anyhow!("nami's own files include no repos"))?;
        let pinned = self.pins()?.get(&src.key).cloned().filter(|p| p.url == src.url);
        let pin_it = if self.project { "`nami layers` (or `nami shell`)" } else { "`nami layers` (or `nami apply`)" };
        let rev = match (pinned, mode()) {
            (Some(p), Mode::Update) => {
                let rev = look_up(&src)?;
                if rev != p.rev {
                    eprintln!("{}", crate::style::err().line(&format!("~ {}: {} -> {} (pinned in {lock_name})", src.key, &p.rev[..12], &rev[..12])));
                }
                rev
            }
            (Some(p), Mode::Offline) => {
                ensure!(files_here(&src, &p.rev).is_some(), "{} is pinned at {} in {lock_name}, but isn't fetched here yet: {pin_it} in that folder fetches it", src.key, &p.rev[..12]);
                p.rev
            }
            (Some(p), _) => p.rev,
            (None, Mode::Locked) => bail!(
                "{} isn't pinned in {lock_name}; a layer is as good as code {}, so it's pinned first: {pin_it} without --locked says what it includes and pins it",
                src.key,
                if self.project { "in the project" } else { "on the computer" }
            ),
            (None, Mode::Offline) => bail!("{} isn't pinned in {lock_name} yet: {pin_it} in that folder says what it includes, and pins and fetches it", src.key),
            (None, _) => {
                let rev = look_up(&src)?;
                let what = src.git_ref.as_deref().map_or("its default branch".to_string(), |r| r.to_string());
                let can = if self.project {
                    "a layer runs code in the project (its tools, tasks and services); read it first"
                } else {
                    "a layer can change anything on the computer (apps, services, files, sudo); read it first"
                };
                eprintln!("including {} ({}, {what}) at commit {}: {can}. Pinned in {lock_name}: it stays at that commit until `nami layers --update`", src.key, src.url, &rev[..12]);
                rev
            }
        };
        let root = files_at(&src, &rev).map_err(|e| match self.pins.as_ref().and_then(|p| p.get(&src.key)) {
            Some(_) => anyhow!("{e}; it's pinned in {lock_name}, and isn't here yet: connect to the network and run this again"),
            None => e,
        })?;
        let file = layer_file(&root, &src.layer).ok_or_else(|| {
            let name = if src.layer.is_empty() { DEFAULT_LAYER } else { &src.layer };
            let has = layers_in(&root);
            anyhow!("{} has no {name}.nami (at {}); its layers: {}", src.key, &rev[..12], if has.is_empty() { "none (no .nami files at its top)".into() } else { has.join(", ") })
        })?;
        let pin = Pin { url: src.url.clone(), git_ref: src.git_ref.clone(), rev };
        let pins = self.pins()?;
        if pins.get(&src.key) != Some(&pin) {
            pins.insert(src.key.clone(), pin.clone());
            self.changed = true;
        }
        // (Once each: a computer is read again without its unused login screens.)
        if !self.used.iter().any(|u| u.include == inc && u.key == src.key) {
            self.used.push(Used { include: inc.to_string(), key: src.key, pin, root: root.clone() });
        }
        Ok((root, file))
    }

    /// Write the pins to nami.lock if they changed (an update keeps only
    /// the ones in use).
    pub fn finish(mut self) -> Vec<Used> {
        let Some(lock_path) = self.lock.clone() else { return self.used };
        let used: BTreeSet<String> = self.used.iter().map(|u| u.key.clone()).collect();
        if mode() == Mode::Update && !used.is_empty() {
            if let Ok(pins) = self.pins() {
                let before = pins.len();
                pins.retain(|k, _| used.contains(k));
                self.changed |= pins.len() != before;
            }
        }
        if self.changed {
            let saved = (|| -> Result<()> {
                let mut lock = crate::lock::load(&lock_path)?;
                lock.layers = self.pins.clone().unwrap_or_default();
                crate::lock::save(&lock_path, &lock)
            })();
            if let Err(e) = saved {
                eprintln!("{}", crate::style::err().line(&format!("warning: couldn't pin the layers in {}: {e:#}", lock_path.display())));
            }
        }
        self.used
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_forms() {
        let s = parse("github:alice/os").unwrap();
        assert_eq!((s.key.as_str(), s.url.as_str(), s.git_ref, s.layer.as_str()), ("github:alice/os", "https://github.com/alice/os.git", None, ""));
        let s = parse("github:alice/os/gaming@v2").unwrap();
        assert_eq!((s.key.as_str(), s.git_ref.as_deref(), s.layer.as_str()), ("github:alice/os@v2", Some("v2"), "gaming"));
        assert_eq!(parse("github:alice/os/layers/dev.nami").unwrap().layer, "layers/dev");
        let s = parse("git+https://codeberg.org/bob/os@v2#dev").unwrap();
        assert_eq!((s.key.as_str(), s.url.as_str(), s.git_ref.as_deref(), s.layer.as_str()), ("git+https://codeberg.org/bob/os@v2", "https://codeberg.org/bob/os", Some("v2"), "dev"));
        assert_eq!(parse("github:alice/os@v2#gaming.nami").unwrap(), parse("github:alice/os/gaming.nami@v2").unwrap());
        // The earlier spelling: the same layer.
        assert_eq!(parse("git+https://codeberg.org/bob/os#dev@v2").unwrap(), s);
        let s = parse("git+file:///srv/os.git#@main").unwrap();
        assert_eq!((s.layer.as_str(), s.git_ref.as_deref()), ("", Some("main")));
        assert_eq!(parse("git+ssh://git@host/os.git").unwrap().key, "git+ssh://git@host/os.git");
        for bad in ["github:alice", "github:../x", "github:alice/os/../etc", "github:alice/os@-x", "github:alice/os@", "git+/etc/os", "git+https://x/os#a@--upload-pack=x", "gitlab:a/b"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(is_remote("github:a/b") && is_remote("git+file:///x") && !is_remote("./x.nami") && !is_remote("ubuntu"));
    }

    #[test]
    fn layer_files() {
        let d = std::env::temp_dir().join(format!("nami-layers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("dev")).unwrap();
        std::fs::write(d.join("default.nami"), "").unwrap();
        std::fs::write(d.join("gaming.nami"), "").unwrap();
        std::fs::write(d.join("dev/default.nami"), "").unwrap();
        assert_eq!(layer_file(&d, ""), Some(d.join("default.nami")));
        assert_eq!(layer_file(&d, "gaming"), Some(d.join("gaming.nami")));
        assert_eq!(layer_file(&d, "dev"), Some(d.join("dev/default.nami")));
        assert_eq!(layer_file(&d, "nope"), None);
        assert_eq!(layers_in(&d), ["default", "gaming"]);
        assert_eq!(dir_name("https://github.com/alice/os.git", &"a".repeat(40)), "github.com_alice_os-aaaaaaaaaaaa");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
