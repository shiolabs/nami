//! Your setup on all your computers (`nami sync`): your ~/user.nami kept the same on each,
//! across Linux systems (Ubuntu with nami, namiOS, ...), through a private git repo of your
//! own: GitHub's (signed in once with GitHub's CLI, `gh`) or any git server's. Nothing of
//! ours in between, and no password in any file: git's own sign-in (gh's, SSH keys, a
//! credential helper) does that.
//!
//! The repo holds your user.nami, as you'd write it (`nami apply ./user.nami` works from a
//! clone), and a line per computer (computers/<id>.yaml: its name, system, last sync).
//! Each computer keeps a clone, the version they last agreed on (the base) and a log of
//! what came in, under ~/.local/state/nami/setup-sync.
//!
//! A sync is a three-way merge, key by key, of the base, this computer's file and the
//! repo's: what one side changed is taken; what both changed differently goes to the one
//! that changed it last (the repo's commit time against the file's), and what the other
//! said is in the log, to undo. Lists (apps) merge item by item: an app added on one and
//! another on the other are both kept. What's this computer's alone (its displays, its
//! graphics card, its backup drive, its sound) is never taken from another, nor is it a
//! reason to sync. What doesn't apply on this system (`namios:` on Ubuntu) is kept in the
//! file, and nami leaves it be there.
//!
//! A user timer syncs every 15 minutes and at login, and a path unit as soon as the file
//! changes (Settings, an app added). What came in is applied (`nami apply`) and logged
//! ("From laptop: added gimp"), each entry undoable (`nami sync undo <n>`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};

use crate::config_edit::Op;

// ---------- the merge: values only, no files ----------

/// What's this computer's alone: never taken from another one, never a reason to sync.
pub const LOCAL: &[&[&str]] = &[
    &["namios", "displays"],
    &["namios", "nvidia"],
    &["namios", "backups"],
    &["namios", "sound"],
    &["namios", "airpods"],
];

/// Where the two sides changed one thing differently, and which was kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clash {
    /// `namios.dock.size`, `packages.firefox`.
    pub path: String,
    /// What this computer had (None: it took it out).
    pub ours: Option<String>,
    /// What the other had.
    pub theirs: Option<String>,
    /// Whether the other's was kept.
    pub took_theirs: bool,
}

#[derive(Debug)]
pub struct Merged {
    pub value: Value,
    pub clashes: Vec<Clash>,
}

fn said(v: &Option<&Value>) -> Option<String> {
    v.map(|v| match v {
        Value::String(s) => s.clone(),
        Value::Tagged(t) if t.tag == "!secret" => "(a secret)".into(),
        other => serde_yaml::to_string(other).unwrap_or_default().trim().replace('\n', " "),
    })
}

/// An empty file's value is an empty map, like a file of comments.
pub fn doc(text: &str) -> Result<Value> {
    Ok(match serde_yaml::from_str::<Value>(text)? {
        Value::Null => Value::Mapping(Mapping::new()),
        v @ Value::Mapping(_) => v,
        _ => bail!("a nami file is a list of settings (key: value)"),
    })
}

/// A list's item, by what names it: `- firefox` and `- git: {...}` by their names; anything
/// else by itself.
fn item_key(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Mapping(m) if m.len() == 1 && m.keys().next().is_some_and(Value::is_string) => m.keys().next().and_then(Value::as_str).unwrap_or_default().to_string(),
        other => format!("\u{0}{}", serde_yaml::to_string(other).unwrap_or_default()),
    }
}

/// A list's items by their keys, in order; None if two have the same key (then the list is
/// one value, not a set).
fn keyed(s: &[Value]) -> Option<Vec<(String, &Value)>> {
    let mut out: Vec<(String, &Value)> = Vec::with_capacity(s.len());
    for v in s {
        let k = item_key(v);
        if out.iter().any(|(o, _)| *o == k) {
            return None;
        }
        out.push((k, v));
    }
    Some(out)
}

/// The value with its apps in one order: which comes first in `packages:` means nothing,
/// so a file whose apps are only in another order is the same file.
pub fn norm(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(Value::Sequence(s)) = v.as_mapping_mut().and_then(|m| m.get_mut("packages")) {
        s.sort_by_cached_key(item_key);
    }
    v
}

/// Whether two files say the same.
pub fn same(a: &Value, b: &Value) -> bool {
    norm(a) == norm(b)
}

/// Three-way merge of a nami file's values: `base` (as both last agreed; None: they never
/// did, so nothing's taken out), `ours` and `theirs`. `prefer_theirs`: who wins where both
/// changed one thing differently. This computer's own settings ([`LOCAL`]) stay ours.
pub fn merge(base: Option<&Value>, ours: &Value, theirs: &Value, prefer_theirs: bool) -> Merged {
    let (b, o, t) = (base.map(|b| norm(&strip_local(b))), norm(&strip_local(ours)), norm(&strip_local(theirs)));
    let mut clashes = Vec::new();
    let mut value = merge_at(&mut Vec::new(), b.as_ref(), Some(&o), Some(&t), prefer_theirs, base.is_none(), &mut clashes).unwrap_or_else(|| Value::Mapping(Mapping::new()));
    put_local(&mut value, ours);
    Merged { value, clashes }
}

fn merge_at(path: &mut Vec<String>, base: Option<&Value>, ours: Option<&Value>, theirs: Option<&Value>, prefer_theirs: bool, joining: bool, clashes: &mut Vec<Clash>) -> Option<Value> {
    if ours == theirs {
        return ours.cloned();
    }
    // Never agreed on anything (a computer joining): what either has is kept.
    if joining {
        match (ours, theirs) {
            (Some(o), None) => return Some(o.clone()),
            (None, Some(t)) => return Some(t.clone()),
            _ => {}
        }
    } else {
        if base == ours {
            return theirs.cloned();
        }
        if base == theirs {
            return ours.cloned();
        }
    }
    match (ours, theirs) {
        (Some(Value::Mapping(o)), Some(Value::Mapping(t))) => {
            let bm = base.and_then(Value::as_mapping);
            let mut out = Mapping::new();
            let keys: Vec<&Value> = o.keys().chain(t.keys().filter(|k| !o.contains_key(*k))).collect();
            for k in keys {
                path.push(key_words(k));
                if let Some(v) = merge_at(path, bm.and_then(|m| m.get(k)), o.get(k), t.get(k), prefer_theirs, joining, clashes) {
                    out.insert(k.clone(), v);
                }
                path.pop();
            }
            Some(Value::Mapping(out))
        }
        (Some(Value::Sequence(o)), Some(Value::Sequence(t))) if keyed(o).is_some() && keyed(t).is_some() && base.and_then(Value::as_sequence).is_none_or(|b| keyed(b).is_some()) => {
            let (ok, tk) = (keyed(o).unwrap(), keyed(t).unwrap());
            let bk = base.and_then(Value::as_sequence).map(|b| keyed(b).unwrap()).unwrap_or_default();
            fn find<'a>(list: &[(String, &'a Value)], k: &str) -> Option<&'a Value> {
                list.iter().find(|(key, _)| key == k).map(|(_, v)| *v)
            }
            let mut out = Vec::new();
            let mut add = |k: &str, path: &mut Vec<String>, clashes: &mut Vec<Clash>| {
                path.push(k.trim_start_matches('\u{0}').to_string());
                // (A list's base: an item only one side has was added there, unless the base had it.)
                let base_item = if base.is_some() { find(&bk, k) } else { None };
                if let Some(v) = merge_at(path, base_item, find(&ok, k), find(&tk, k), prefer_theirs, joining || base.is_none(), clashes) {
                    out.push(v);
                }
                path.pop();
            };
            for (k, _) in &ok {
                add(k, path, clashes);
            }
            for (k, _) in tk.iter().filter(|(k, _)| find(&ok, k).is_none()) {
                add(k, path, clashes);
            }
            Some(Value::Sequence(out))
        }
        _ => {
            clashes.push(Clash { path: path.join("."), ours: said(&ours), theirs: said(&theirs), took_theirs: prefer_theirs });
            if prefer_theirs { theirs.cloned() } else { ours.cloned() }
        }
    }
}

fn key_words(k: &Value) -> String {
    match k {
        Value::String(s) => s.clone(),
        other => serde_yaml::to_string(other).unwrap_or_default().trim().to_string(),
    }
}

fn get_path<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(v, |v, k| v.as_mapping()?.get(*k))
}

/// The value without this computer's own settings.
pub fn strip_local(v: &Value) -> Value {
    fn remove(v: &mut Value, path: &[&str]) {
        let Some(m) = v.as_mapping_mut() else { return };
        match path {
            [last] => {
                m.remove(*last);
            }
            [first, rest @ ..] => {
                if let Some(next) = m.get_mut(*first) {
                    remove(next, rest);
                }
            }
            [] => {}
        }
    }
    let mut v = v.clone();
    for path in LOCAL {
        remove(&mut v, path);
    }
    v
}

/// This computer's own settings, from `ours`, put in `v` (where `ours` has them).
fn put_local(v: &mut Value, ours: &Value) {
    for path in LOCAL {
        let Some(val) = get_path(ours, path) else { continue };
        let (last, parents) = path.split_last().unwrap();
        let mut at = &mut *v;
        for p in parents {
            let Some(m) = at.as_mapping_mut() else { return };
            if !m.get(*p).is_some_and(Value::is_mapping) {
                m.insert(Value::from(*p), Value::Mapping(Mapping::new()));
            }
            at = m.get_mut(*p).unwrap();
        }
        if let Some(m) = at.as_mapping_mut() {
            m.insert(Value::from(*last), val.clone());
        }
    }
}

/// `text` (a nami file) changed to say `to`, its comments and layout kept where they can be
/// (each change an edit of its lines, as `nami config` makes them); else the keys that
/// differ written again, else the whole file.
pub fn rewrite(text: &str, to: &Value) -> String {
    let Ok(from) = doc(text) else { return render(text, to) };
    if same(&from, to) {
        return text.to_string();
    }
    let mut ops = Vec::new();
    if diff_ops(&from, to, &mut Vec::new(), &mut ops) {
        let mut t = text.to_string();
        let ok = ops.iter().all(|op| match crate::config_edit::apply(&t, op) {
            Ok(n) => {
                t = n;
                true
            }
            Err(_) => false,
        });
        if ok && doc(&t).is_ok_and(|d| same(&d, to)) {
            return t;
        }
    }
    // Each top key that differs, whole.
    if let (Some(f), Some(m)) = (from.as_mapping(), to.as_mapping()) {
        let mut t = text.to_string();
        let mut ok = true;
        for k in f.keys().filter(|k| !m.contains_key(*k)) {
            ok &= k.as_str().and_then(|k| crate::config_edit::apply(&t, &Op::Unset { path: vec![k.into()] }).ok()).map(|n| t = n).is_some();
        }
        for (k, v) in m.iter().filter(|(k, v)| f.get(*k) != Some(*v)) {
            ok &= k.as_str().and_then(|k| crate::config_edit::apply(&t, &Op::Set { path: vec![k.into()], value: v.clone() }).ok()).map(|n| t = n).is_some();
        }
        if ok && doc(&t).is_ok_and(|d| same(&d, to)) {
            return t;
        }
    }
    render(text, to)
}

/// The whole file written again: its first comment lines, then `to`.
fn render(text: &str, to: &Value) -> String {
    let head: String = text.lines().take_while(|l| l.trim_start().starts_with('#') || l.trim().is_empty()).filter(|l| !l.trim().is_empty()).map(|l| format!("{l}\n")).collect();
    let body = if to.as_mapping().is_some_and(Mapping::is_empty) { String::new() } else { serde_yaml::to_string(to).unwrap_or_default() };
    head + &body
}

/// The edits from `from` to `to`, as `nami config` makes them. False: they can't be said
/// that way (a key that isn't text).
fn diff_ops(from: &Value, to: &Value, path: &mut Vec<String>, ops: &mut Vec<Op>) -> bool {
    match (from, to) {
        (Value::Mapping(a), Value::Mapping(b)) => {
            for k in a.keys().filter(|k| !b.contains_key(*k)) {
                let Some(k) = k.as_str().filter(|k| !k.starts_with("- ")) else { return false };
                ops.push(Op::Unset { path: path.iter().cloned().chain([k.to_string()]).collect() });
            }
            for (k, v) in b {
                let Some(ks) = k.as_str().filter(|k| !k.starts_with("- ")) else { return false };
                match a.get(k) {
                    None => ops.push(Op::Set { path: path.iter().cloned().chain([ks.to_string()]).collect(), value: v.clone() }),
                    Some(old) if old != v => {
                        path.push(ks.to_string());
                        let ok = diff_ops(old, v, path, ops);
                        path.pop();
                        if !ok {
                            return false;
                        }
                    }
                    _ => {}
                }
            }
            true
        }
        // The apps: each added or taken out (a changed one, both).
        (Value::Sequence(a), Value::Sequence(b)) if path.len() == 1 && path[0] == "packages" && keyed(a).is_some() && keyed(b).is_some() => {
            for old in a.iter().filter(|o| !b.contains(o)) {
                ops.push(Op::Remove { path: path.clone(), value: old.clone() });
            }
            for new in b.iter().filter(|n| !a.contains(n)) {
                ops.push(Op::Add { path: path.clone(), value: new.clone() });
            }
            true
        }
        _ => {
            ops.push(Op::Set { path: path.clone(), value: to.clone() });
            true
        }
    }
}

/// What changed from `before` to `after`, in a few words: `added gimp, removed vlc`,
/// `changed Dock and Appearance`.
pub fn summary(before: &str, after: &str) -> String {
    let Ok(sections) = crate::file_diff::changes(before, after) else { return "changed your settings".into() };
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    for s in &sections {
        if s.key == ["packages"] {
            for r in s.rows.iter().filter(|r| r.item && r.path.len() == 2) {
                match r.mark {
                    crate::file_diff::Mark::Added => added.push(r.path[1].clone()),
                    crate::file_diff::Mark::Removed => removed.push(r.path[1].clone()),
                    crate::file_diff::Mark::Changed => changed.push(r.path[1].clone()),
                }
            }
            if s.rows.iter().any(|r| !(r.item && r.path.len() == 2)) && !s.rows.iter().any(|r| r.item && r.path.len() == 2) {
                changed.push("Apps".into());
            }
        } else if !changed.contains(&s.title) {
            changed.push(s.title.clone());
        }
    }
    let list = |v: &[String]| match v {
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [a, b, c] => format!("{a}, {b} and {c}"),
        [a, b, rest @ ..] => format!("{a}, {b} and {} more", rest.len()),
        [] => String::new(),
    };
    let mut parts = Vec::new();
    if !added.is_empty() {
        parts.push(format!("added {}", list(&added)));
    }
    if !removed.is_empty() {
        parts.push(format!("removed {}", list(&removed)));
    }
    if !changed.is_empty() {
        parts.push(format!("changed {}", list(&changed)));
    }
    if parts.is_empty() { "changed comments in your file".into() } else { parts.join(", ") }
}

// ---------- where things are ----------

/// A person's sync: their file and the folder nami keeps its state in.
#[derive(Clone, Debug)]
pub struct Place {
    pub user_file: PathBuf,
    pub state: PathBuf,
}

impl Place {
    /// The person running this: ~/user.nami, ~/.local/state/nami/setup-sync.
    pub fn mine() -> Place {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".local/state"));
        Place { user_file: crate::config::user_file(), state: state.join("nami/setup-sync") }
    }
    fn settings(&self) -> PathBuf {
        self.state.join("settings.json")
    }
    fn clone_dir(&self) -> PathBuf {
        self.state.join("repo")
    }
    fn base(&self) -> PathBuf {
        self.state.join("base.nami")
    }
    fn log(&self) -> PathBuf {
        self.state.join("log.json")
    }
    fn last(&self) -> PathBuf {
        self.state.join("last.json")
    }
}

/// What `nami sync on` said: where, and who this computer is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// As given: `github:ada/nami-setup`, a git URL.
    pub repo: String,
    pub url: String,
    pub branch: String,
    /// This computer's name in the repo (its hostname) and its file's (its machine ID).
    pub computer: String,
    pub id: String,
    /// The last commit synced with (whose computers are "new" since).
    #[serde(default)]
    pub synced: Option<String>,
}

/// One thing a sync did to this computer's file, to see and to undo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    pub when: u64,
    /// The computers it came from (none: only this one's own, kept over another's).
    pub from: Vec<String>,
    /// `added gimp`.
    pub summary: String,
    #[serde(default)]
    pub clashes: Vec<Clash>,
    pub before: String,
    pub after: String,
    #[serde(default)]
    pub undone: bool,
}

/// How the last sync went.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Last {
    pub tried: u64,
    #[serde(default)]
    pub ok: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
    /// What applying what came in said, if it failed.
    #[serde(default)]
    pub apply_error: Option<String>,
}

/// One computer, as the repo lists it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Computer {
    pub name: String,
    #[serde(default)]
    pub system: String,
    #[serde(default)]
    pub last: u64,
    #[serde(default)]
    pub this: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Status {
    pub on: bool,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub computer: Option<String>,
    #[serde(default)]
    pub last: Last,
    #[serde(default)]
    pub computers: Vec<Computer>,
    /// The latest of what came in, newest first.
    #[serde(default)]
    pub entries: Vec<Entry>,
    /// GitHub's CLI is here (to sign in with).
    #[serde(default)]
    pub github_cli: bool,
}

fn read_json<T: for<'a> Deserialize<'a>>(p: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

fn write_json<T: Serialize>(p: &Path, v: &T) -> Result<()> {
    write_file(p, serde_json::to_string_pretty(v)?.as_bytes())
}

/// Written whole or not at all (a link: where it points).
fn write_file(p: &Path, bytes: &[u8]) -> Result<()> {
    let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_file_name(format!(".{}.sync-{}", p.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    std::fs::write(&tmp, bytes)?;
    if let Ok(m) = std::fs::metadata(&p) {
        let _ = std::fs::set_permissions(&tmp, m.permissions());
    }
    std::fs::rename(&tmp, &p).with_context(|| format!("writing {}", p.display()))
}

fn epoch() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// This computer: its name, a stable ID for its line in the repo, and its system in words.
pub fn this_computer() -> (String, String, String) {
    // (Tests: two homes on one computer, as two computers.)
    if let Some(n) = std::env::var("NAMI_SYNC_COMPUTER").ok().filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')) {
        return (n.clone(), n, std::env::var("NAMI_SYNC_SYSTEM").unwrap_or_else(|_| "Linux".into()));
    }
    let name = crate::config::hostname().unwrap_or_else(|| "computer".into());
    let id = std::fs::read_to_string("/etc/machine-id").ok().map(|s| s.trim().chars().take(12).collect::<String>()).filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric())).unwrap_or_else(|| name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect());
    let system = if crate::nami_os::here() || Path::new("/etc/namios").exists() {
        "namiOS".to_string()
    } else {
        std::fs::read_to_string("/etc/os-release").ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_string()))).unwrap_or_else(|| "Linux".into())
    };
    (name, id, system)
}

// ---------- git ----------

/// git, never asking on a terminal (a timer runs it), signed in to GitHub through gh's
/// sign-in when it's here.
pub struct Git {
    git: crate::machine::Git,
    gh: Option<PathBuf>,
}

impl Git {
    pub fn new(git: crate::machine::Git, gh: Option<PathBuf>) -> Git {
        Git { git, gh }
    }

    /// git (in `dir`), never asking anything, with gh's sign-in for GitHub.
    fn cmd_in(&self, dir: Option<&Path>) -> Command {
        let mut c = self.git.cmd(dir);
        c.env("GIT_TERMINAL_PROMPT", "0").env("GIT_ASKPASS", "true").env("SSH_ASKPASS", "true");
        c.env("GIT_SSH_COMMAND", std::env::var("GIT_SSH_COMMAND").unwrap_or_else(|_| "ssh -o BatchMode=yes".into()));
        if let Some(gh) = &self.gh {
            c.arg("-c").arg("credential.https://github.com.helper=");
            c.arg("-c").arg(format!("credential.https://github.com.helper=!'{}' auth git-credential", gh.display()));
        }
        c
    }

    fn cmd(&self, dir: &Path) -> Command {
        self.cmd_in(Some(dir))
    }

    fn run(&self, dir: &Path, args: &[&str]) -> Result<()> {
        let out = self.cmd(dir).args(args).stdin(Stdio::null()).output().context("running git")?;
        ensure!(out.status.success(), "git {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim());
        Ok(())
    }

    fn out(&self, dir: &Path, args: &[&str]) -> Option<String> {
        let o = self.cmd(dir).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
    }
}

/// `github:owner/repo` as a git URL; anything else as it is.
pub fn url_of(repo: &str) -> Result<String> {
    if let Some(rest) = repo.strip_prefix("github:") {
        let ok = |s: &str| !s.is_empty() && !s.starts_with('.') && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
        let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
        ensure!(parts.len() == 2 && parts.iter().all(|p| ok(p)), "write it as github:you/repo");
        return Ok(format!("https://github.com/{}/{}.git", parts[0], parts[1]));
    }
    ensure!(crate::machine::is_repo(repo) || repo.starts_with('/'), "{repo}: a git repo's address (github:you/repo, https://..., git@host:path, or a folder)");
    Ok(repo.to_string())
}

/// The branch the repo's on (an empty one: main).
fn remote_branch(git: &Git, dir: &Path) -> String {
    if let Some(h) = git.out(dir, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) {
        if let Some(b) = h.trim().strip_prefix("origin/") {
            return b.to_string();
        }
    }
    let branches: Vec<String> = git.out(dir, &["branch", "-r", "--format=%(refname:short)"]).unwrap_or_default().lines().filter_map(|l| l.trim().strip_prefix("origin/").map(String::from)).filter(|b| b != "HEAD").collect();
    if branches.iter().any(|b| b == "main") || branches.is_empty() {
        return "main".into();
    }
    branches[0].clone()
}

// ---------- syncing ----------

/// What a sync did.
#[derive(Debug, Default, Serialize)]
pub struct Done {
    /// This computer's file changed (what came in): its log entry.
    pub came_in: Option<u64>,
    /// This computer's changes went to the repo.
    pub sent: bool,
    pub clashes: Vec<Clash>,
}

/// Held while a sync runs: one at a time.
struct Lock(#[allow(dead_code)] std::fs::File);

fn lock(place: &Place) -> Result<Lock> {
    use std::os::fd::AsRawFd;
    std::fs::create_dir_all(&place.state)?;
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(place.state.join("lock"))?;
    // SAFETY: flock on a file we own.
    let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) };
    ensure!(rc == 0, "can't lock {}", place.state.display());
    Ok(Lock(f))
}

pub fn settings(place: &Place) -> Option<Settings> {
    read_json(&place.settings())
}

/// Turn it on: the repo cloned, and the first sync. An empty repo gets this computer's
/// file; one with a file already (another computer's) is joined, nothing either has left
/// out, and where they differ the repo's is kept (this computer's is in the log, to undo).
pub fn on(place: &Place, git: &Git, repo: &str, computer: (&str, &str, &str), apply: &dyn Fn(&Path) -> Result<()>) -> Result<Done> {
    let url = url_of(repo)?;
    let _l = lock(place)?;
    let dir = place.clone_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&place.state)?;
    let _ = std::fs::remove_file(place.base());
    let out = git.cmd_in(None).arg("clone").arg("--quiet").arg(&url).arg(&dir).stdin(Stdio::null()).output().context("running git")?;
    if !out.status.success() {
        bail!("couldn't reach {repo}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let branch = remote_branch(git, &dir);
    let s = Settings { repo: repo.to_string(), url, branch, computer: computer.0.to_string(), id: computer.1.to_string(), synced: None };
    write_json(&place.settings(), &s)?;
    sync_locked(place, git, computer.2, apply)
}

/// Turn it off: this computer stops syncing (the repo and the other computers stay as
/// they are; its own file too).
pub fn off(place: &Place) -> Result<()> {
    let _l = lock(place)?;
    for f in [place.settings(), place.base(), place.last()] {
        let _ = std::fs::remove_file(f);
    }
    let _ = std::fs::remove_dir_all(place.clone_dir());
    Ok(())
}

/// A sync, now.
pub fn now(place: &Place, git: &Git, system: &str, apply: &dyn Fn(&Path) -> Result<()>) -> Result<Done> {
    let _l = lock(place)?;
    sync_locked(place, git, system, apply)
}

fn sync_locked(place: &Place, git: &Git, system: &str, apply: &dyn Fn(&Path) -> Result<()>) -> Result<Done> {
    let mut last: Last = read_json(&place.last()).unwrap_or_default();
    last.tried = epoch();
    // Another computer sent something between fetching and sending: again, from it.
    let mut result = once(place, git, system, apply);
    for _ in 0..3 {
        match &result {
            Err(e) if ["[rejected]", "fetch first", "non-fast-forward"].iter().any(|r| format!("{e:#}").contains(r)) => result = once(place, git, system, apply),
            _ => break,
        }
    }
    match &result {
        Ok((_, apply_error)) => {
            last.ok = Some(last.tried);
            last.error = None;
            last.apply_error = apply_error.clone();
        }
        Err(e) => last.error = Some(format!("{e:#}")),
    }
    write_json(&place.last(), &last)?;
    result.map(|(d, _)| d)
}

/// One go: fetch, merge, write here, send there. Returns what applying said, if it failed.
fn once(place: &Place, git: &Git, system: &str, apply: &dyn Fn(&Path) -> Result<()>) -> Result<(Done, Option<String>)> {
    let mut s: Settings = settings(place).context("syncing your setup isn't on here: `nami sync on <repo>` turns it on")?;
    let dir = place.clone_dir();
    ensure!(dir.join(".git").exists(), "nami's copy of {} is gone: turn it on again (`nami sync on {}`)", s.repo, s.repo);
    git.run(&dir, &["fetch", "--quiet", "origin"]).with_context(|| format!("couldn't reach {}", s.repo))?;
    let remote = format!("origin/{}", s.branch);
    let head = git.out(&dir, &["rev-parse", "--verify", "--quiet", &format!("{remote}^{{commit}}")]).map(|h| h.trim().to_string());

    let ours_text = match std::fs::read(&place.user_file) {
        Ok(b) => String::from_utf8(b).context("your user.nami isn't text")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).context("reading your user.nami"),
    };
    let ours = doc(&ours_text).with_context(|| format!("{} can't be read, so it isn't synced until it's fixed", place.user_file.display()))?;
    // (A repo a computer with tray made has user.tray: it keeps that name,
    // for the computers that still have tray.)
    let theirs_text = head.as_ref().and_then(|_| git.out(&dir, &["show", &format!("{remote}:user.nami")]).or_else(|| git.out(&dir, &["show", &format!("{remote}:user.tray")])));
    let base_text = std::fs::read_to_string(place.base()).ok();
    let base = base_text.as_deref().and_then(|t| doc(t).ok());

    let mut done = Done::default();
    let mut apply_error = None;
    let merged_text = match &theirs_text {
        None => ours_text.clone(),
        Some(tt) => {
            let theirs = doc(tt).context("the repo's user.nami can't be read")?;
            let ours_time = std::fs::metadata(&place.user_file).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            let theirs_time: u64 = git.out(&dir, &["log", "-1", "--format=%ct", &remote]).and_then(|t| t.trim().parse().ok()).unwrap_or(0);
            // Joining: the setup you've been keeping wins where they differ.
            let prefer_theirs = base.is_none() || theirs_time >= ours_time;
            let m = merge(base.as_ref(), &ours, &theirs, prefer_theirs);
            done.clashes = m.clashes.clone();
            if same(&m.value, &ours) {
                ours_text.clone()
            } else {
                // The repo's file, comments and all, when nothing changed here (or this
                // computer's joining); else this one's, with what came in edited into it.
                let unchanged_here = base_text.as_deref().is_none_or(|b| doc(b).is_ok_and(|b| same(&strip_local(&b), &strip_local(&ours))));
                let text = if unchanged_here { rewrite(tt, &m.value) } else { rewrite(&ours_text, &m.value) };
                if let Ok(found) = crate::secrets::find(tt) {
                    ensure!(found.iter().all(|f| f.sealed), "a secret in the repo's user.nami isn't sealed, so nothing was taken from it: seal it there (`nami secret seal`)");
                }
                write_file(&place.user_file, text.as_bytes())?;
                let from = computers_since(git, &dir, s.synced.as_deref(), &remote, &s.computer);
                let entry = log_entry(place, Entry { id: 0, when: epoch(), from, summary: summary(&ours_text, &text), clashes: m.clashes.clone(), before: ours_text.clone(), after: text.clone(), undone: false })?;
                done.came_in = Some(entry);
                if let Err(e) = apply(&place.user_file) {
                    apply_error = Some(format!("{e:#}"));
                }
                text
            }
        }
    };

    // Send what the repo doesn't have yet (this computer's own settings aren't a reason).
    let merged = doc(&merged_text)?;
    let theirs_now = theirs_text.as_deref().map(doc).transpose()?;
    let heartbeat_due = computer_line(git, &dir, &remote, &s.id).is_none_or(|c| c.last + 24 * 3600 < epoch() || c.name != s.computer || c.system != system);
    let differs = theirs_now.as_ref().is_none_or(|t| !same(&strip_local(t), &strip_local(&merged)));
    // (What the repo gets: this computer's own settings left out, as they're no one else's.)
    let shared_text = rewrite(&merged_text, &strip_local(&merged));
    if differs || heartbeat_due {
        if differs {
            let found = crate::secrets::find(&merged_text).unwrap_or_default();
            if let Some(f) = found.iter().find(|f| !f.sealed) {
                bail!("{} line {}: {} is a secret that isn't sealed, so your file isn't sent anywhere until it is (`nami secret seal {}`)", place.user_file.display(), f.line, f.path, place.user_file.display());
            }
        }
        match &head {
            Some(h) => git.run(&dir, &["checkout", "--quiet", "-B", &s.branch, h])?,
            None => {
                // An empty repo: a branch of its own, from nothing.
                let _ = git.run(&dir, &["checkout", "--quiet", "--orphan", &s.branch]);
                let _ = git.run(&dir, &["rm", "-rfq", "--cached", "--ignore-unmatch", "."]);
            }
        }
        git.run(&dir, &["reset", "--quiet", "--hard"]).ok();
        if differs {
            std::fs::write(crate::oldnames::find(&dir.join("user.nami")), &shared_text)?;
        }
        if !dir.join("README.md").exists() {
            std::fs::write(dir.join("README.md"), README)?;
        }
        let line = Computer { name: s.computer.clone(), system: system.to_string(), last: epoch(), this: false };
        std::fs::create_dir_all(dir.join("computers"))?;
        std::fs::write(dir.join("computers").join(format!("{}.yaml", s.id)), format!("name: {}\nsystem: {}\nlast: {}\n", yaml_word(&line.name), yaml_word(&line.system), line.last))?;
        git.run(&dir, &["add", "-A"])?;
        let what = if differs { theirs_text.as_deref().map(|t| summary(t, &shared_text)).unwrap_or_else(|| "first copy of your setup".into()) } else { "still here".into() };
        let message = format!("{}: {what}\n\nComputer: {}\n", s.computer, s.computer);
        let ident = format!("user.name={}", s.computer);
        let mail = format!("user.email=nami@{}.invalid", s.id);
        git.run(&dir, &["-c", &ident, "-c", &mail, "-c", "commit.gpgsign=false", "commit", "--quiet", "--allow-empty", "-m", &message])?;
        git.run(&dir, &["push", "--quiet", "origin", &format!("HEAD:refs/heads/{}", s.branch)])?;
        done.sent = differs;
    }
    let synced = git.out(&dir, &["rev-parse", "HEAD"]).map(|h| h.trim().to_string()).or(head);
    if synced != s.synced {
        s.synced = synced;
        write_json(&place.settings(), &s)?;
    }
    write_file(&place.base(), merged_text.as_bytes())?;
    Ok((done, apply_error))
}

const README: &str = "# Your setup\n\nYour apps and settings (user.nami), kept the same on each of your computers by nami\n(Settings → Sync → Keep This Setup on All Your Computers, or `nami sync`).\nKeep this repository private. Secrets in it are sealed (`nami secret seal`).\n";

/// A word as YAML writes it on one line.
fn yaml_word(s: &str) -> String {
    serde_yaml::to_string(&Value::from(s)).unwrap_or_default().trim().to_string()
}

/// The computers that sent what's new since `since` (all of it, without one).
fn computers_since(git: &Git, dir: &Path, since: Option<&str>, remote: &str, me: &str) -> Vec<String> {
    let range = match since {
        Some(s) if git.out(dir, &["cat-file", "-e", &format!("{s}^{{commit}}")]).is_some() => format!("{s}..{remote}"),
        _ => remote.to_string(),
    };
    let said = git.out(dir, &["log", "--format=%(trailers:key=Computer,valueonly)", &range]).unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    for name in said.lines().map(str::trim).filter(|n| !n.is_empty() && *n != me) {
        if !out.iter().any(|o| o == name) {
            out.push(name.to_string());
        }
    }
    out
}

fn computer_line(git: &Git, dir: &Path, remote: &str, id: &str) -> Option<Computer> {
    let text = git.out(dir, &["show", &format!("{remote}:computers/{id}.yaml")])?;
    serde_yaml::from_str(&text).ok()
}

fn log_entry(place: &Place, mut e: Entry) -> Result<u64> {
    let mut log: Vec<Entry> = read_json(&place.log()).unwrap_or_default();
    e.id = log.iter().map(|e| e.id).max().unwrap_or(0) + 1;
    let id = e.id;
    log.push(e);
    let keep = log.len().saturating_sub(200);
    log.drain(..keep);
    write_json(&place.log(), &log)?;
    Ok(id)
}

pub fn entries(place: &Place) -> Vec<Entry> {
    read_json(&place.log()).unwrap_or_default()
}

/// Undo what entry `id` did to this computer's file: each thing it changed goes back,
/// unless it's been changed again since (that stays). The file's then synced as any
/// change is.
pub fn undo(place: &Place, id: u64) -> Result<String> {
    let _l = lock(place)?;
    let mut log = entries(place);
    let e = log.iter_mut().find(|e| e.id == id).with_context(|| format!("there's no change {id} to undo (`nami sync status` lists them)"))?;
    ensure!(!e.undone, "change {id} is undone already");
    let current_text = std::fs::read_to_string(&place.user_file).unwrap_or_default();
    let (after, before, current) = (doc(&e.after)?, doc(&e.before)?, doc(&current_text)?);
    // What it changed, reversed, on the file as it is now.
    let back = merge(Some(&after), &current, &before, false).value;
    let text = rewrite(&current_text, &back);
    write_file(&place.user_file, text.as_bytes())?;
    e.undone = true;
    let said = e.summary.clone();
    write_json(&place.log(), &log)?;
    Ok(said)
}

/// Where things are, for Settings and `nami sync status`.
pub fn status(place: &Place, git: Option<&Git>) -> Status {
    let s = settings(place);
    let mut st = Status { on: s.is_some(), last: read_json(&place.last()).unwrap_or_default(), ..Default::default() };
    let mut entries = entries(place);
    entries.reverse();
    entries.truncate(30);
    st.entries = entries;
    if let Some(s) = s {
        st.repo = Some(s.repo.clone());
        st.computer = Some(s.computer.clone());
        let dir = place.clone_dir();
        let remote = format!("origin/{}", s.branch);
        if let Some(git) = git {
            let names = git.out(&dir, &["ls-tree", "--name-only", &format!("{remote}:computers")]).unwrap_or_default();
            for n in names.lines().filter(|n| n.ends_with(".yaml")) {
                let id = n.trim_end_matches(".yaml");
                if let Some(mut c) = computer_line(git, &dir, &remote, id) {
                    c.this = id == s.id;
                    st.computers.push(c);
                }
            }
        }
        st.computers.sort_by(|a, b| b.this.cmp(&a.this).then(a.name.cmp(&b.name)));
    }
    st
}

// ---------- GitHub: signing in, and a private repo of your own ----------

/// GitHub's CLI: the computer's, else the catalog's (fetched once, when `fetch`).
pub fn gh(root: &Path, fetch: bool) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NAMI_GH").map(PathBuf::from) {
        return Some(p).filter(|p| p.is_file());
    }
    if let Some(bin) = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).map(|d| d.join("gh")).find(|p| p.is_file()) {
        return Some(bin);
    }
    let store = crate::store::Store::open(root).ok()?;
    let index = crate::index::Index::load(&store).ok()?;
    let out = index.get("gh").ok()?.outputs.get("out").cloned().flatten()?;
    if !store.is_valid(crate::store::base_name(&out)) {
        if !fetch {
            return None;
        }
        crate::say!("fetching GitHub's command-line tool (once)");
        store.realise(std::slice::from_ref(&out)).ok()?;
    }
    Some(store.phys(&out).join("bin/gh")).filter(|p| p.is_file())
}

/// Who's signed in to GitHub with gh, if anyone.
pub fn github_user(gh: &Path) -> Option<String> {
    let o = Command::new(gh).args(["api", "user", "--jq", ".login"]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    Some(String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|u| o.status.success() && !u.is_empty())
}

/// The one-time code in what `gh auth login --web` says: `! First copy your one-time code: 1A2B-3C4D`.
pub fn device_code(line: &str) -> Option<String> {
    let rest = line.split("code:").nth(1)?.trim();
    let code: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    (code.len() >= 8 && code.contains('-')).then_some(code)
}

/// Sign in to GitHub in the browser (GitHub's device sign-in, through gh): `told` hears the
/// code to enter there. Returns who's signed in.
pub fn sign_in(gh: &Path, told: &mut dyn FnMut(&str)) -> Result<String> {
    if let Some(u) = github_user(gh) {
        return Ok(u);
    }
    let mut child = Command::new(gh)
        .args(["auth", "login", "--hostname", "github.com", "--git-protocol", "https", "--web", "--scopes", "repo"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running {}", gh.display()))?;
    let mut stdin = child.stdin.take();
    let stderr = child.stderr.take().unwrap();
    let mut said = Vec::new();
    // gh says the code, then waits for Enter to open the browser.
    let mut reader = std::io::BufReader::new(stderr);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = std::io::BufRead::read_until(&mut reader, b'\n', &mut buf)?;
        if n == 0 {
            break;
        }
        let line = String::from_utf8_lossy(&buf).to_string();
        if let Some(code) = device_code(&line) {
            told(&code);
            if let Some(mut i) = stdin.take() {
                let _ = i.write_all(b"\n");
            }
        }
        said.push(line.trim().to_string());
    }
    let ok = child.wait()?.success();
    ensure!(ok, "GitHub sign-in didn't finish: {}", said.iter().rev().find(|l| !l.is_empty()).cloned().unwrap_or_default());
    github_user(gh).context("signed in, but GitHub doesn't say who")
}

/// Your private repo for this, on GitHub: `<you>/nami-setup`, made if it isn't there.
pub fn github_repo(gh: &Path, user: &str) -> Result<String> {
    let name = format!("{user}/nami-setup");
    let there = Command::new(gh).args(["repo", "view", &name, "--json", "name"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status()?.success();
    if !there {
        let o = Command::new(gh).args(["repo", "create", &name, "--private", "--description", "My apps and settings, kept the same on my computers by nami"]).stdin(Stdio::null()).output()?;
        ensure!(o.status.success(), "couldn't make the private repository {name} on GitHub: {}", String::from_utf8_lossy(&o.stderr).trim());
    }
    Ok(format!("github:{name}"))
}

// ---------- the user timer ----------

const UNIT: &str = "nami-setup-sync";
/// tray's (they run `tray sync now`, which is nami): replaced by nami's when it turns syncing on or off.
const OLD_UNIT: &str = "tray-setup-sync";

fn old_units_off() {
    if !units_dir().join(format!("{OLD_UNIT}.timer")).exists() {
        return;
    }
    if std::env::var_os("NAMI_SYNC_NO_UNITS").is_none() {
        let _ = Command::new("systemctl").args(["--user", "disable", "--now", &format!("{OLD_UNIT}.timer"), &format!("{OLD_UNIT}.path")]).stderr(Stdio::null()).status();
    }
    for kind in ["service", "timer", "path"] {
        let _ = std::fs::remove_file(units_dir().join(format!("{OLD_UNIT}.{kind}")));
    }
}

fn units_dir() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    config.join("systemd/user")
}

/// The timer (at login, every 15 minutes) and the path unit (as soon as your file
/// changes), started. Where there's no user systemd, said; syncing by hand still works.
pub fn units_on(nami: &Path) -> Result<()> {
    old_units_off();
    let dir = units_dir();
    std::fs::create_dir_all(&dir)?;
    let t = nami.display();
    std::fs::write(dir.join(format!("{UNIT}.service")), format!("[Unit]\nDescription=Keep your setup the same on your computers (nami sync)\nAfter=network-online.target\n\n[Service]\nType=oneshot\nExecStart={t} sync now --quiet --settle 5\nNice=10\n"))?;
    std::fs::write(dir.join(format!("{UNIT}.timer")), format!("[Unit]\nDescription=Keep your setup the same on your computers, every 15 minutes and at login\n\n[Timer]\nOnStartupSec=1min\nOnUnitActiveSec=15min\nRandomizedDelaySec=1min\n\n[Install]\nWantedBy=timers.target\n"))?;
    std::fs::write(dir.join(format!("{UNIT}.path")), format!("[Unit]\nDescription=Keep your setup the same on your computers, as soon as it changes\n\n[Path]\nPathChanged=%h/{}\nUnit={UNIT}.service\nTriggerLimitIntervalSec=60\nTriggerLimitBurst=6\n\n[Install]\nWantedBy=default.target\n", crate::oldnames::find(&std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(crate::config::USER_FILE)).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| crate::config::USER_FILE.into())))?;
    if std::env::var_os("NAMI_SYNC_NO_UNITS").is_some() {
        return Ok(());
    }
    let ok = Command::new("systemctl").args(["--user", "daemon-reload"]).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
        && Command::new("systemctl").args(["--user", "enable", "--now", &format!("{UNIT}.timer"), &format!("{UNIT}.path")]).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    if !ok {
        eprintln!("this computer has no user services running, so it syncs when you run `nami sync now`");
    }
    Ok(())
}

pub fn units_off() {
    old_units_off();
    if std::env::var_os("NAMI_SYNC_NO_UNITS").is_none() {
        let _ = Command::new("systemctl").args(["--user", "disable", "--now", &format!("{UNIT}.timer"), &format!("{UNIT}.path")]).stderr(Stdio::null()).status();
    }
    for kind in ["service", "timer", "path"] {
        let _ = std::fs::remove_file(units_dir().join(format!("{UNIT}.{kind}")));
    }
    if std::env::var_os("NAMI_SYNC_NO_UNITS").is_none() {
        let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).stderr(Stdio::null()).status();
    }
}

// ---------- nami sync ----------

#[derive(clap::Subcommand)]
pub enum SyncCommand {
    /// Turn it on: your user.nami kept in a private git repo of yours, and the same on each
    /// computer you turn it on on. No repo: your GitHub account's nami-setup (made private,
    /// signed in to in your browser, if need be)
    On {
        /// github:you/repo, a git URL (https://..., git@host:path), or a folder
        repo: Option<String>,
        /// Don't apply what comes in (tests)
        #[arg(long, hide = true)]
        no_apply: bool,
        /// Each step as a JSON line (for Settings)
        #[arg(long)]
        json: bool,
    },
    /// Sync now: what changed here goes to the repo, what changed on your other computers
    /// comes here (and is applied)
    Now {
        /// Say nothing unless something went wrong (the timer)
        #[arg(long)]
        quiet: bool,
        /// Wait this many seconds first, for a file still being written to settle
        #[arg(long, default_value_t = 0)]
        settle: u64,
        #[arg(long, hide = true)]
        no_apply: bool,
        #[arg(long)]
        json: bool,
    },
    /// Whether it's on, the last sync, your computers, and what came in
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Undo what one sync changed here (`nami sync status` numbers them); the undoing is synced too
    Undo {
        id: u64,
        #[arg(long, hide = true)]
        no_apply: bool,
    },
    /// Sign in to GitHub in your browser (through GitHub's own tool, gh)
    SignIn {
        #[arg(long)]
        json: bool,
    },
    /// Turn it off on this computer (the repo, your file and your other computers stay as they are)
    Off,
}

fn step(json: bool, step: &str, message: &str) {
    if json {
        println!("{}", serde_json::json!({ "step": step, "percent": null, "message": message }));
    } else if step != "done" {
        eprintln!("{message}");
    }
}

fn nami_exe() -> PathBuf {
    let fixed = crate::migrate::here().join("bin/nami");
    if fixed.is_file() {
        return fixed;
    }
    std::env::current_exe().unwrap_or(fixed)
}

/// Applying what came in: `nami apply <file>`, as you.
fn applier(skip: bool) -> impl Fn(&Path) -> Result<()> {
    move |file: &Path| {
        if skip {
            return Ok(());
        }
        let exe = std::env::current_exe()?;
        let o = Command::new(exe).arg("apply").arg(file).stdin(Stdio::null()).output()?;
        ensure!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr).lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("nami apply failed"));
        Ok(())
    }
}

fn git_here(root: &Path, fetch_gh: bool) -> Result<Git> {
    Ok(Git::new(crate::machine::Git::get(root)?, gh(root, fetch_gh)))
}

pub fn cli(command: SyncCommand) -> Result<()> {
    ensure!(!crate::profile::is_root(), "syncing a setup is each person's own: run it as yourself, not root");
    let root = PathBuf::from(std::env::var_os("NAMI_ROOT").unwrap_or_else(|| "/".into()));
    let place = Place::mine();
    let (name, id, system) = this_computer();
    match command {
        SyncCommand::On { repo, no_apply, json } => {
            let repo = match repo {
                Some(r) => r,
                None => {
                    step(json, "signing-in", "Signing in to GitHub…");
                    let gh = gh(&root, true).context("GitHub's command-line tool (gh) isn't here, and the catalog doesn't have it: name a git repo instead (`nami sync on git@host:you/setup.git`)")?;
                    let user = sign_in(&gh, &mut |code| {
                        if json {
                            println!("{}", serde_json::json!({ "step": "code", "percent": null, "message": code }));
                        } else {
                            eprintln!("Enter this code at https://github.com/login/device: {code}");
                        }
                    })?;
                    step(json, "repo", &format!("Signed in as {user}. Finding your private repository…"));
                    github_repo(&gh, &user)?
                }
            };
            step(json, "syncing", &format!("Bringing your setup together with {repo}…"));
            let git = git_here(&root, repo.starts_with("github:") || repo.contains("github.com"))?;
            let done = on(&place, &git, &repo, (&name, &id, &system), &applier(no_apply))?;
            units_on(&nami_exe())?;
            let said = match (&done.came_in, done.sent) {
                (Some(_), _) => format!("On. Your setup from {repo} is on this computer now; it's kept the same from here on."),
                (None, true) => format!("On. Your setup is in {repo}; turn it on on your other computers too."),
                (None, false) => "On. This computer already had the same setup.".to_string(),
            };
            step(json, "done", &said);
            if !json {
                eprintln!("{said}");
            }
            Ok(())
        }
        SyncCommand::Now { quiet, settle, no_apply, json } => {
            std::thread::sleep(std::time::Duration::from_secs(settle.min(60)));
            let git = git_here(&root, false)?;
            if settings(&place).is_none() {
                ensure!(quiet, "syncing your setup isn't on here: `nami sync on <repo>` turns it on");
                return Ok(());
            }
            let done = now(&place, &git, &system, &applier(no_apply))?;
            if json {
                println!("{}", serde_json::to_string(&done)?);
            } else if !quiet {
                let log = entries(&place);
                match done.came_in.and_then(|id| log.iter().find(|e| e.id == id)) {
                    Some(e) if !e.from.is_empty() => eprintln!("From {}: {}", e.from.join(" and "), e.summary),
                    Some(e) => eprintln!("{}", e.summary),
                    None if done.sent => eprintln!("Sent this computer's changes."),
                    None => eprintln!("Up to date."),
                }
                for c in &done.clashes {
                    eprintln!("  both computers changed {}: kept {} ({})", c.path, if c.took_theirs { "the other's" } else { "this one's" }, if c.took_theirs { c.theirs.clone() } else { c.ours.clone() }.unwrap_or_else(|| "taken out".into()));
                }
            }
            if let Some(e) = read_json::<Last>(&place.last()).and_then(|l| l.apply_error) {
                eprintln!("what came in couldn't all be installed: {e}");
            }
            Ok(())
        }
        SyncCommand::Status { json } => {
            let git = git_here(&root, false).ok();
            let mut st = status(&place, git.as_ref());
            st.github_cli = gh(&root, false).is_some() || crate::store::Store::open(&root).ok().and_then(|s| crate::index::Index::load(&s).ok()).is_some_and(|i| i.get("gh").is_ok());
            if json {
                println!("{}", serde_json::to_string(&st)?);
                return Ok(());
            }
            if !st.on {
                println!("Off. `nami sync on` keeps your setup the same on all your computers.");
                return Ok(());
            }
            // A card, as `nami status` has it: ✓ fine, ✗ wrong; the computers, then what came lately.
            let paint = crate::style::out();
            let now = crate::tell::now();
            println!("{} {}", paint.bold("On"), paint.dim(&format!("· {} · this computer: {}", st.repo.unwrap_or_default(), st.computer.unwrap_or_default())));
            let say = |l: &str| println!("{}", paint.line(&format!("  {l}")));
            match (&st.last.ok, &st.last.error) {
                (_, Some(e)) => say(&format!("✗ Last sync failed: {e} · nami sync now")),
                (Some(t), None) => say(&format!("✓ Last synced {}", crate::tell::ago(*t, now))),
                _ => say("⚠ Not synced yet · nami sync now"),
            }
            if !st.computers.is_empty() {
                println!("\n{}", paint.bold("Computers"));
            }
            for c in &st.computers {
                println!("  {}{} {}", paint.bold(&c.name), if c.this { " (this one)" } else { "" }, paint.dim(&format!("· {} · last synced {}", c.system, crate::tell::ago(c.last, now))));
            }
            if !st.entries.is_empty() {
                println!("\n{}", paint.bold("Lately"));
            }
            for e in st.entries.iter().take(10) {
                let from = if e.from.is_empty() { String::new() } else { format!("From {}: ", e.from.join(" and ")) };
                println!("  {}  {}  {from}{}{}", paint.dim(&format!("{:>3}", e.id)), paint.dim(&crate::tell::ago(e.when, now)), e.summary, if e.undone { paint.dim(" (undone)") } else { String::new() });
            }
            Ok(())
        }
        SyncCommand::Undo { id, no_apply } => {
            let said = undo(&place, id)?;
            eprintln!("Undid: {said}");
            applier(no_apply)(&place.user_file)?;
            if settings(&place).is_some() {
                let git = git_here(&root, false)?;
                if let Err(e) = now(&place, &git, &system, &applier(true)) {
                    eprintln!("not synced yet ({e:#}); it will be at the next sync");
                }
            }
            Ok(())
        }
        SyncCommand::SignIn { json } => {
            let gh = gh(&root, true).context("GitHub's command-line tool (gh) isn't here, and the catalog doesn't have it")?;
            let user = sign_in(&gh, &mut |code| {
                if json {
                    println!("{}", serde_json::json!({ "step": "code", "percent": null, "message": code }));
                } else {
                    eprintln!("Enter this code at https://github.com/login/device: {code}");
                }
            })?;
            step(json, "done", &format!("Signed in to GitHub as {user}."));
            if !json {
                eprintln!("Signed in to GitHub as {user}.");
            }
            Ok(())
        }
        SyncCommand::Off => {
            off(&place)?;
            units_off();
            eprintln!("Off on this computer. Your file stays as it is.");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Value {
        doc(s).unwrap()
    }

    #[test]
    fn what_one_side_changed_is_taken() {
        let base = v("packages: [jq]\nnamios: { dock: { size: medium }, appearance: { look: dark } }\n");
        let ours = v("packages: [jq, gimp]\nnamios: { dock: { size: medium }, appearance: { look: dark } }\n");
        let theirs = v("packages: [jq]\nnamios: { dock: { size: small }, appearance: { look: dark } }\n");
        let m = merge(Some(&base), &ours, &theirs, false);
        assert!(same(&m.value, &v("packages: [jq, gimp]\nnamios: { dock: { size: small }, appearance: { look: dark } }\n")), "{:?}", m.value);
        assert!(m.clashes.is_empty());
    }

    #[test]
    fn apps_added_on_both_are_both_kept_and_removals_go() {
        let base = v("packages: [jq, vlc]\n");
        let ours = v("packages: [jq, vlc, gimp]\n");
        let theirs = v("packages: [jq, firefox]\n");
        let m = merge(Some(&base), &ours, &theirs, true);
        assert!(same(&m.value, &v("packages: [jq, gimp, firefox]\n")), "{:?}", m.value);
        assert!(m.clashes.is_empty());
    }

    #[test]
    fn both_changed_one_thing_last_writer_wins_and_its_said() {
        let base = v("namios: { dock: { size: medium } }\n");
        let ours = v("namios: { dock: { size: large } }\n");
        let theirs = v("namios: { dock: { size: small } }\n");
        let m = merge(Some(&base), &ours, &theirs, true);
        assert!(same(&m.value, &theirs), "{:?}", m.value);
        assert_eq!(m.clashes, vec![Clash { path: "namios.dock.size".into(), ours: Some("large".into()), theirs: Some("small".into()), took_theirs: true }]);
        let m = merge(Some(&base), &ours, &theirs, false);
        assert!(same(&m.value, &ours), "{:?}", m.value);
        assert!(!m.clashes[0].took_theirs);
    }

    #[test]
    fn a_change_against_a_removal_is_a_clash_not_a_loss() {
        let base = v("env: { EDITOR: vim }\n");
        let ours = v("{}\n");
        let theirs = v("env: { EDITOR: nvim }\n");
        let m = merge(Some(&base), &ours, &theirs, true);
        assert!(same(&m.value, &theirs), "{:?}", m.value);
        assert_eq!(m.clashes.len(), 1);
        assert_eq!(m.clashes[0].ours, None);
    }

    #[test]
    fn this_computers_own_settings_stay_and_arent_a_difference() {
        let base = v("namios: { displays: { DP-1: { scale: 1 } }, dock: { size: medium } }\n");
        let ours = v("namios: { displays: { DP-1: { scale: 1 } }, dock: { size: medium } }\n");
        let theirs = v("namios: { displays: { eDP-1: { scale: 2 } }, dock: { size: small } }\n");
        let m = merge(Some(&base), &ours, &theirs, true);
        assert!(same(&m.value, &v("namios: { displays: { DP-1: { scale: 1 } }, dock: { size: small } }\n")), "{:?}", m.value);
        assert!(m.clashes.is_empty());
        assert_eq!(strip_local(&ours), v("namios: { dock: { size: medium } }\n"));
    }

    #[test]
    fn what_doesnt_apply_here_is_kept() {
        // Ubuntu's file never had namios:, the namiOS one's does: kept, on both.
        let base = v("packages: [jq]\n");
        let ours = v("packages: [jq, htop]\n");
        let theirs = v("packages: [jq]\nnamios: { dock: { on: true } }\n");
        let m = merge(Some(&base), &ours, &theirs, false);
        assert!(same(&m.value, &v("packages: [jq, htop]\nnamios: { dock: { on: true } }\n")), "{:?}", m.value);
    }

    #[test]
    fn joining_keeps_what_either_has_and_prefers_the_repos() {
        let ours = v("packages: [htop]\nenv: { EDITOR: nano, PAGER: less }\n");
        let theirs = v("packages: [gimp]\nenv: { EDITOR: vim }\n");
        let m = merge(None, &ours, &theirs, true);
        assert!(same(&m.value, &v("packages: [htop, gimp]\nenv: { EDITOR: vim, PAGER: less }\n")), "{:?}", m.value);
        assert_eq!(m.clashes.len(), 1);
        assert_eq!(m.clashes[0].path, "env.EDITOR");
    }

    #[test]
    fn an_apps_own_settings_merge_too() {
        let base = v("packages:\n  - git: { ~/.gitconfig: { user: { name: J } } }\n");
        let ours = v("packages:\n  - git: { ~/.gitconfig: { user: { name: J, email: j@x } } }\n");
        let theirs = v("packages:\n  - git: { ~/.gitconfig: { user: { name: Ada } } }\n");
        let m = merge(Some(&base), &ours, &theirs, false);
        assert!(same(&m.value, &v("packages:\n  - git: { ~/.gitconfig: { user: { name: Ada, email: j@x } } }\n")), "{:?}", m.value);
    }

    #[test]
    fn rewriting_keeps_comments() {
        let text = "# My setup\npackages:\n  - jq # json\n  - vlc\n# The look\nnamios:\n  dock:\n    size: medium\n";
        let to = v("packages: [jq, gimp]\nnamios: { dock: { size: small } }\n");
        let out = rewrite(text, &to);
        assert_eq!(doc(&out).unwrap(), to);
        assert!(out.contains("# My setup") && out.contains("- jq # json") && out.contains("# The look"), "{out}");
    }

    #[test]
    fn summaries_in_words() {
        assert_eq!(summary("packages: [jq]\n", "packages: [jq, gimp]\n"), "added gimp");
        assert_eq!(summary("packages: [jq, vlc]\n", "packages: [jq]\nnamios: { dock: { size: small } }\n"), "removed vlc, changed Dock");
    }

    #[test]
    fn device_codes() {
        assert_eq!(device_code("! First copy your one-time code: 1A2B-3C4D\n").as_deref(), Some("1A2B-3C4D"));
        assert_eq!(device_code("Press Enter to open github.com in your browser..."), None);
    }

    // ---------- two computers and a repo ----------

    fn git_bin() -> Option<PathBuf> {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).map(|d| d.join("git")).find(|p| p.is_file())
    }

    struct World {
        tmp: PathBuf,
        repo: String,
        git: Git,
    }

    impl World {
        fn new(name: &str) -> Option<World> {
            let bin = git_bin()?;
            let tmp = std::env::temp_dir().join(format!("nami-setup-sync-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&tmp);
            std::fs::create_dir_all(&tmp).unwrap();
            assert!(Command::new(&bin).arg("init").arg("-q").arg("--bare").arg("-b").arg("main").arg(tmp.join("setup.git")).status().unwrap().success());
            Some(World { repo: tmp.join("setup.git").display().to_string(), tmp, git: Git::new(crate::machine::Git::at(bin), None) })
        }
        fn home(&self, name: &str) -> Place {
            let home = self.tmp.join(name);
            std::fs::create_dir_all(&home).unwrap();
            Place { user_file: home.join("user.nami"), state: home.join(".local/state/nami/setup-sync") }
        }
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }

    fn write(p: &Place, text: &str) {
        std::fs::write(&p.user_file, text).unwrap();
    }

    fn read(p: &Place) -> Value {
        doc(&std::fs::read_to_string(&p.user_file).unwrap()).unwrap()
    }

    /// Two homes (an Ubuntu laptop, a namiOS desktop) and a bare repo: the first one's file
    /// goes there; the second joins; changes on each reach the other; a clash goes to the
    /// last; each change that came in is logged and undoable.
    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "nami sync is Linux's for now")]
    fn two_computers_kept_the_same() {
        let Some(w) = World::new("two") else { return };
        let applied = std::cell::RefCell::new(Vec::<String>::new());
        let apply = |f: &Path| -> Result<()> {
            applied.borrow_mut().push(f.display().to_string());
            Ok(())
        };
        let laptop = w.home("laptop");
        let desktop = w.home("desktop");
        write(&laptop, "# Ada's setup\npackages:\n  - jq\n  - git:\n      ~/.gitconfig: { user: { name: Ada } }\n");
        write(&desktop, "packages: [htop]\nnamios:\n  dock: { size: large }\n  displays: { DP-1: { scale: 1.25 } }\n");

        // The laptop first: an empty repo gets its file.
        let d = on(&laptop, &w.git, &w.repo, ("laptop", "aaa", "Ubuntu 24.04"), &apply).unwrap();
        assert!(d.sent && d.came_in.is_none());

        // The desktop joins: both kept, its own displays stay.
        let d = on(&desktop, &w.git, &w.repo, ("desktop", "bbb", "namiOS"), &apply).unwrap();
        assert!(d.came_in.is_some() && d.sent);
        let joined = read(&desktop);
        assert_eq!(joined["packages"].as_sequence().unwrap().len(), 3);
        assert_eq!(joined["namios"]["displays"]["DP-1"]["scale"], Value::from(1.25));
        assert_eq!(applied.borrow().len(), 1);

        // The laptop hears of htop and the desktop's dock (namios: kept on Ubuntu too).
        let d = now(&laptop, &w.git, "Ubuntu 24.04", &apply).unwrap();
        let id = d.came_in.unwrap();
        let l = read(&laptop);
        assert!(l["packages"].as_sequence().unwrap().contains(&Value::from("htop")));
        assert_eq!(l["namios"]["dock"]["size"], Value::from("large"));
        assert!(l["namios"].get("displays").is_none(), "the desktop's displays stay the desktop's");
        let lt = std::fs::read_to_string(&laptop.user_file).unwrap();
        assert!(lt.contains("# Ada's setup"), "its comments stay: {lt}");
        let e = entries(&laptop).into_iter().find(|e| e.id == id).unwrap();
        assert_eq!(e.from, vec!["desktop".to_string()]);
        assert!(e.summary.contains("added htop"), "{}", e.summary);

        // Nothing new: nothing sent, nothing in.
        let d = now(&laptop, &w.git, "Ubuntu 24.04", &apply).unwrap();
        assert!(d.came_in.is_none() && !d.sent);

        // GIMP on the laptop reaches the desktop, said so.
        let text = std::fs::read_to_string(&laptop.user_file).unwrap();
        write(&laptop, &crate::config_edit::apply(&text, &Op::Add { path: vec!["packages".into()], value: "gimp".into() }).unwrap());
        assert!(now(&laptop, &w.git, "Ubuntu 24.04", &apply).unwrap().sent);
        let d = now(&desktop, &w.git, "namiOS", &apply).unwrap();
        let e = entries(&desktop).into_iter().find(|e| Some(e.id) == d.came_in).unwrap();
        assert_eq!(e.from, vec!["laptop".to_string()]);
        assert_eq!(e.summary, "added gimp");

        // Undo it there: gimp goes, and the laptop hears of that too.
        undo(&desktop, e.id).unwrap();
        assert!(!read(&desktop)["packages"].as_sequence().unwrap().contains(&Value::from("gimp")));
        now(&desktop, &w.git, "namiOS", &apply).unwrap();
        now(&laptop, &w.git, "Ubuntu 24.04", &apply).unwrap();
        assert!(!read(&laptop)["packages"].as_sequence().unwrap().contains(&Value::from("gimp")));

        // Both change the dock's size: the later one wins, the other's value is in the log.
        let set = |p: &Place, size: &str| {
            let text = std::fs::read_to_string(&p.user_file).unwrap();
            write(p, &crate::config_edit::apply(&text, &Op::Set { path: vec!["namios".into(), "dock".into(), "size".into()], value: size.into() }).unwrap());
        };
        set(&desktop, "small");
        now(&desktop, &w.git, "namiOS", &apply).unwrap();
        // (The laptop's edit is later than the desktop's commit.)
        std::thread::sleep(std::time::Duration::from_millis(1100));
        set(&laptop, "medium");
        let d = now(&laptop, &w.git, "Ubuntu 24.04", &apply).unwrap();
        assert_eq!(d.clashes.len(), 1);
        assert!(!d.clashes[0].took_theirs);
        assert_eq!(d.clashes[0].theirs.as_deref(), Some("small"));
        assert_eq!(read(&laptop)["namios"]["dock"]["size"], Value::from("medium"));
        now(&desktop, &w.git, "namiOS", &apply).unwrap();
        assert_eq!(read(&desktop)["namios"]["dock"]["size"], Value::from("medium"));

        // In step: syncing again sends nothing, either way (each one's displays differ, and
        // that's no reason).
        for (p, sys) in [(&laptop, "Ubuntu 24.04"), (&desktop, "namiOS"), (&laptop, "Ubuntu 24.04")] {
            let d = now(p, &w.git, sys, &apply).unwrap();
            assert!(!d.sent && d.came_in.is_none());
        }
        assert_eq!(read(&desktop)["namios"]["displays"]["DP-1"]["scale"], Value::from(1.25));

        // The repo lists both computers.
        let st = status(&desktop, Some(&w.git));
        assert_eq!(st.computers.iter().map(|c| (c.name.as_str(), c.this)).collect::<Vec<_>>(), vec![("desktop", true), ("laptop", false)]);
        assert_eq!(st.computers[1].system, "Ubuntu 24.04");

        // Off: this computer stops; its file stays.
        off(&laptop).unwrap();
        assert!(settings(&laptop).is_none() && laptop.user_file.exists());
        assert!(now(&laptop, &w.git, "Ubuntu 24.04", &apply).is_err());
    }

    /// A secret that isn't sealed never leaves the computer.
    #[test]
    fn unsealed_secrets_stay_here() {
        let Some(w) = World::new("secret") else { return };
        let p = w.home("one");
        write(&p, "env:\n  TOKEN: !secret hunter2\n");
        let err = on(&p, &w.git, &w.repo, ("one", "111", "Linux"), &|_| Ok(())).unwrap_err();
        assert!(format!("{err:#}").contains("isn't sealed"), "{err:#}");
    }

    /// Apps added on each between syncs: both kept, on both.
    #[test]
    fn changes_on_both_are_merged_not_lost() {
        let Some(w) = World::new("race") else { return };
        let a = w.home("a");
        let b = w.home("b");
        write(&a, "packages: [jq]\n");
        on(&a, &w.git, &w.repo, ("a", "aa", "Linux"), &|_| Ok(())).unwrap();
        on(&b, &w.git, &w.repo, ("b", "bb", "Linux"), &|_| Ok(())).unwrap();
        write(&a, "packages: [jq, gimp]\n");
        write(&b, "packages: [jq, vlc]\n");
        now(&a, &w.git, "Linux", &|_| Ok(())).unwrap();
        now(&b, &w.git, "Linux", &|_| Ok(())).unwrap();
        now(&a, &w.git, "Linux", &|_| Ok(())).unwrap();
        for p in [&a, &b] {
            let mut apps: Vec<String> = read(p)["packages"].as_sequence().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
            apps.sort();
            assert_eq!(apps, ["gimp", "jq", "vlc"]);
        }
    }
}
