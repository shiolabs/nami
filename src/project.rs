//! Projects: per-project environments, like flakes' dev shells. nami has
//! three files: the system's (/etc/nami/system.nami), each user's
//! (~/user.nami), and a project's: nami file next to its code,
//! pinned by `nami.lock`, its tools only on PATH inside `nami shell` /
//! `nami run`. Nothing goes into your profile; the store paths are shared,
//! and they stay safe from `nami gc` for as long as the project directory
//! exists.
//!
//! A nami file elsewhere that's a system or user file in all but place (a
//! dotfiles repo's: `home:`, `caches:`, ... and nothing only projects have)
//! isn't a project; `nami apply <file>` is for those.
//!
//! ```yaml
//! # project.nami
//! include:                   # a team's base: files of the repo, layers from
//!   - github:acme/dev/node   # git repos (pinned in nami.lock; see layers.rs)
//! packages:
//!   - nodejs
//!   - ripgrep
//!   - ./tools/my-linter      # a recipe in the repo
//! env:
//!   NODE_ENV: development
//! develop: true              # compilers, pkg-config, headers: like `nix develop`
//!                            # (on by itself when a compiler or library is listed)
//! services:                  # what it needs running: `nami up` (see up.rs)
//!   postgres: true
//! ```
//!
//! The lock pins the catalog too: a tool added later is looked up in the
//! same catalog, so the project resolves the same on every computer until
//! someone runs `--upgrade`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::build::Builder;
use crate::index::Index;
use crate::lock;
use crate::profile::{Entry, Profiles};
use crate::store::Store;

pub const FILE: &str = crate::config::PROJECT_FILE;

/// Top-level keys a project file has.
pub const KEYS: &[&str] = &["format", "packages", "env", "unfree", "develop", "tasks", "provides", "services", "replace", "replace-in", "bundle"];

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// Catalog names, or recipe paths relative to the project.
    #[serde(default)]
    pub packages: Vec<String>,
    /// Environment variables inside the project's shell.
    #[serde(default, deserialize_with = "crate::yaml_text::text_map")]
    pub env: BTreeMap<String, String>,
    /// Allow software that isn't free (yes unless `false`).
    #[serde(default)]
    pub unfree: Option<bool>,
    /// A full build environment, like `nix develop`: nixpkgs' stdenv
    /// (compiler, make, ...), pkg-config, and the packages' headers and
    /// libraries set up the way nixpkgs builds see them.
    /// Unset: on when the project lists a compiler/build tool or a library.
    #[serde(default)]
    pub develop: Option<bool>,
    /// Named commands: `nami run test` runs `tasks.test` with the project's
    /// tools. Each a command, or `{ run, description, depends }`.
    #[serde(default)]
    pub tasks: BTreeMap<String, Task>,
    /// Tools this project shares: name -> recipe in the repo. Anyone can
    /// install one with `nami install github:owner/repo#name`.
    #[serde(default)]
    pub provides: BTreeMap<String, String>,
    /// What it needs running while you work on it: built-in services
    /// (`- ./services/postgres.nami`, from `nami new . --from postgres`) and its own (`- worker:` with `run:`); see up.rs.
    #[serde(default)]
    pub services: serde_yaml::Mapping,
    /// Libraries swapped in its packages, as in any nami file (replace.rs).
    #[serde(default)]
    pub replace: BTreeMap<String, String>,
    #[serde(default, rename = "replace-in")]
    pub replace_in: BTreeMap<String, BTreeMap<String, String>>,
    /// How `nami bundle .` makes it one file: its name, version, program,
    /// icon, menu entry, or a build of its own (bundle_project.rs reads it).
    #[serde(default)]
    pub bundle: Option<serde_yaml::Value>,
}

/// A task: `test: cargo test`, or with more to say about it:
/// `test: { run: cargo test, description: Runs the tests, depends: [build] }`.
/// One with only `depends:` runs those.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Task {
    pub run: Option<String>,
    pub description: Option<String>,
    /// Tasks run first, in this order (each once).
    pub depends: Vec<String>,
}

/// How a task is written, for messages.
pub const TASK_FORM: &str = "a task is a command (`test: cargo test`), or `{ run: ..., description: ..., depends: [...] }`";

impl Task {
    /// A task as written, or what's wrong with it, in words.
    pub fn from_value(v: &serde_yaml::Value) -> std::result::Result<Task, String> {
        use serde_yaml::Value;
        let text = |v: &Value| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        };
        let list_of_tasks = "depends: is a list of the tasks to run first, like [build]";
        match v {
            Value::String(s) => Ok(Task { run: Some(s.clone()), ..Task::default() }),
            Value::Sequence(_) => Err(format!("{TASK_FORM}. For several commands, join them with && or write them under run: |")),
            Value::Mapping(m) => {
                let mut task = Task::default();
                for (k, v) in m {
                    match k.as_str() {
                        Some("run") => task.run = Some(text(v).ok_or("run: is one command. For several, join them with && or write them under run: |")?),
                        Some("description") => task.description = Some(text(v).ok_or("description: is a sentence about the task")?),
                        Some("depends") => {
                            task.depends = match v {
                                Value::String(s) => vec![s.clone()],
                                Value::Sequence(l) => l.iter().map(|d| d.as_str().map(String::from)).collect::<Option<Vec<_>>>().ok_or(list_of_tasks)?,
                                _ => return Err(list_of_tasks.into()),
                            };
                        }
                        Some(other) => {
                            let hint = crate::config::closest(other, &["run", "description", "depends"]).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
                            return Err(format!("`{other}` isn't part of a task{hint}. A task has run:, description: and depends:"));
                        }
                        None => return Err(TASK_FORM.into()),
                    }
                }
                if task.run.is_none() && task.depends.is_empty() {
                    return Err("a task has run: (its command), depends: (tasks to run first), or both".into());
                }
                Ok(task)
            }
            _ => Err(TASK_FORM.into()),
        }
    }

    /// What `nami project tasks` and the app show: its description, else its command.
    pub fn shown(&self) -> String {
        match (&self.description, &self.run) {
            (Some(d), _) => d.clone(),
            (None, Some(r)) => r.clone(),
            (None, None) => format!("runs {}", self.depends.join(", ")),
        }
    }
}

impl<'de> Deserialize<'de> for Task {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Task, D::Error> {
        let v = serde_yaml::Value::deserialize(d)?;
        Task::from_value(&v).map_err(serde::de::Error::custom)
    }
}

/// The tasks to run for `name`, in order: what it depends on first (each
/// once, depth first, in the order listed), then itself.
pub fn task_order(tasks: &BTreeMap<String, Task>, name: &str) -> Result<Vec<String>> {
    fn visit(tasks: &BTreeMap<String, Task>, name: &str, path: &mut Vec<String>, out: &mut Vec<String>) -> Result<()> {
        if out.iter().any(|o| o == name) {
            return Ok(());
        }
        if let Some(at) = path.iter().position(|p| p == name) {
            let mut cycle = path[at..].to_vec();
            cycle.push(name.to_string());
            bail!("tasks: {} depend on each other in a circle ({}). Take one of them out of depends:", path[at], cycle.join(" -> "));
        }
        let task = tasks.get(name).ok_or_else(|| match path.last() {
            Some(by) => anyhow!("tasks: {by}: depends on {name}, which isn't a task here"),
            None => anyhow!("there's no task {name} here"),
        })?;
        path.push(name.to_string());
        for d in &task.depends {
            visit(tasks, d, path, out)?;
        }
        path.pop();
        out.push(name.to_string());
        Ok(())
    }
    let mut out = Vec::new();
    visit(tasks, name, &mut Vec::new(), &mut out)?;
    Ok(out)
}

/// A project as `nami project list --json` shows it.
#[derive(Debug, serde::Serialize)]
pub struct Summary {
    pub dir: String,
    /// Its project.nami.
    pub file: String,
    pub name: String,
    pub packages: Vec<String>,
    pub tasks: BTreeMap<String, String>,
    pub provides: Vec<String>,
    pub services: Vec<String>,
    pub build_env: bool,
    pub locked: bool,
}

pub fn summary(dir: &Path, index: Option<&Index>) -> Result<Summary> {
    let file = file_in(dir).ok_or_else(|| anyhow!("no project file in {}", dir.display()))?;
    let project = read(&file)?;
    Ok(Summary {
        dir: dir.display().to_string(),
        file: file.display().to_string(),
        name: dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        build_env: develop_reason(&project, index).is_some(),
        locked: lock_for(&file).is_file(),
        provides: project.provides.keys().cloned().collect(),
        services: project.services.keys().filter_map(|k| k.as_str().map(|e| crate::up::name_of(e).to_string())).collect(),
        packages: project.packages,
        tasks: project.tasks.into_iter().map(|(n, t)| (n, t.shown())).collect(),
    })
}

/// A project file's lock: nami.lock next to its project.nami.
pub fn lock_for(file: &Path) -> PathBuf {
    crate::oldnames::find(&file.with_file_name("nami.lock"))
}

/// Is `file` a project's? A project.nami always is. Another nami file isn't when it's
/// the system or user file, or reads like one: a key only those have
/// (`home`, `caches`, ...) and none only projects have (`env`, `tasks`, ...).
/// Unreadable YAML counts as a project's, so its errors get shown.
pub fn is_project(file: &Path) -> bool {
    if file.file_name().is_some_and(|n| crate::oldnames::is_named(n, FILE)) {
        return true;
    }
    // The other two files are named for what they are, too, as a package's is.
    if file.file_name().is_some_and(|n| [crate::config::SYSTEM_FILE, crate::config::USER_FILE, "package.nami"].iter().any(|f| crate::oldnames::is_named(n, f))) {
        return false;
    }
    let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let this = canon(file);
    if [crate::config::system_file(), crate::config::user_file()].iter().any(|f| canon(f) == this) {
        return false;
    }
    fs::read_to_string(file).is_ok_and(|t| is_project_text(&t))
}

fn is_project_text(text: &str) -> bool {
    let Some((system_only, project_only)) = key_kinds(text) else { return true };
    project_only || !system_only
}

/// Does this text have keys only a project file has (so `nami apply` isn't for it)?
pub fn has_project_keys(text: &str) -> bool {
    key_kinds(text).is_some_and(|(_, project_only)| project_only)
}

/// Whether a YAML file has keys only system/user files have, and keys only
/// project files have. None: it isn't YAML.
fn key_kinds(text: &str) -> Option<(bool, bool)> {
    let value = serde_yaml::from_str::<serde_yaml::Value>(text).ok()?;
    let keys: Vec<&str> = value.as_mapping().map(|m| m.keys().filter_map(|k| k.as_str()).collect()).unwrap_or_default();
    let top: Vec<&str> = [crate::config::TOP_KEYS, crate::config::COMPUTER_KEYS, crate::config::USER_KEYS].concat();
    Some((keys.iter().any(|k| top.contains(k) && !KEYS.contains(k)), keys.iter().any(|k| KEYS.contains(k) && !top.contains(k))))
}

/// The project file in `dir`: its project.nami.
pub fn file_in(dir: &Path) -> Option<PathBuf> {
    Some(crate::oldnames::find_in(dir, FILE)).filter(|f| f.is_file())
}

/// Read a project file, with what it includes (see [`read_with_layers`]).
pub fn read(file: &Path) -> Result<Project> {
    Ok(read_with_layers(file)?.0)
}

/// Read a project file with what it includes: `include: [./nami/common.nami,
/// github:acme/dev/node-service]` (paths, and layers from git repos, pinned
/// in its nami.lock; see layers.rs). Merged as a system file's includes are:
/// lists joined, maps key by key, and where both say something, the
/// project's own file wins. Also the layers it used.
pub fn read_with_layers(file: &Path) -> Result<(Project, Vec<crate::layers::Used>)> {
    let text = fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let base = file.parent().unwrap_or(Path::new("."));
    // `- nodejs: 20` is nodejs@20, and service versions
    // as written: each file's own, as it's read.
    let (value, used) = crate::config::load_project(&text, &file.display().to_string(), base)?;
    if value.get("files").is_some() {
        bail!("{}: files: writes /etc, so it's for the system file, not a project's", file.display());
    }
    let project = serde_yaml::from_value(value).map_err(|e| {
        let msg = e.to_string();
        let hint = msg.starts_with("unknown field").then(|| format!(" ({})", crate::config::unknown_key_hint(msg.split('`').nth(1).and_then(|bad| crate::config::closest(bad, KEYS))))).unwrap_or_default();
        anyhow!("{}: {msg}{hint}", file.display())
    })?;
    Ok((project, used))
}

/// Compilers and build tools: listing one means the project is for building.
const BUILD_TOOLS: &[&str] = &[
    "gcc", "clang", "rustc", "cargo", "rustup", "go", "gnumake", "cmake", "meson", "ninja", "autoconf", "automake",
    "libtool", "pkg-config", "zig", "gfortran", "nasm", "yasm", "bazel", "scons", "gradle", "maven", "dotnet-sdk",
];

/// Why a project gets a build environment (None: it doesn't).
pub fn develop_reason(project: &Project, index: Option<&Index>) -> Option<String> {
    match project.develop {
        Some(true) => return Some("develop: true".into()),
        Some(false) => return None,
        None => {}
    }
    for name in &project.packages {
        let base = name.rsplit('.').next().unwrap_or(name);
        if BUILD_TOOLS.iter().any(|t| base == *t || base.starts_with(&format!("{t}_")) || base.starts_with(&format!("{t}-"))) {
            return Some(format!("{name} is listed"));
        }
        // A library: headers to build against, and no programs of its own.
        if let Some(pkg) = index.and_then(|i| i.get(name).ok()) {
            // (Once downloaded, programs show: postgresql has headers but is a tool.)
            let has_programs = pkg.install_paths().iter().any(|p| Path::new(p).join("bin").is_dir());
            if pkg.outputs.contains_key("dev") && !pkg.outputs.contains_key("bin") && pkg.main_program.is_none() && pkg.apps.is_none() && !has_programs {
                return Some(format!("{name} is a library"));
            }
        }
    }
    None
}

/// The project file in `start` or the nearest directory above it.
pub fn find(start: &Path) -> Option<PathBuf> {
    start.ancestors().find_map(file_in)
}

/// A ready project environment.
pub struct Env {
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    pub vars: Vec<(String, String)>,
    /// `develop: true`: a bash script that sets up the build environment
    /// (sourced by `nami shell`/`run`/`env`).
    pub rc: Option<PathBuf>,
    /// Its services (`nami up`).
    pub services: Vec<crate::up::Service>,
    /// Its secrets, opened (in `vars` too; `rc` doesn't have them).
    pub secrets: Vec<(String, String)>,
}

impl Env {
    /// The variables with `develop`'s set up (`rc`), or `vars`.
    pub fn all_vars(&self) -> Result<Vec<(String, String)>> {
        match &self.rc {
            Some(rc) => {
                let mut v = develop_vars(rc)?;
                v.extend(self.secrets.iter().cloned());
                Ok(v)
            }
            None => Ok(self.vars.clone()),
        }
    }
}

/// Every catalog name in `wanted` there, but what `pinned` says the lock
/// has; else each one that isn't, at its line in `file`, as `nami check`
/// says it (all of them, not the first alone, halfway through).
fn all_in_catalog(file: &Path, wanted: &[String], index: &Index, pinned: impl Fn(&str) -> bool) -> Result<()> {
    let unknown: Vec<&str> = wanted
        .iter()
        .map(String::as_str)
        .filter(|n| !crate::ops::is_remote(n) && !Builder::is_recipe(n) && !pinned(n))
        .filter(|n| index.get(n).is_err())
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    let problems = crate::config::not_in_catalog(&fs::read_to_string(file).unwrap_or_default(), &unknown, index);
    if problems.is_empty() {
        return Ok(());
    }
    Err(crate::config::file_problems(file, &problems))
}

/// A catalog package no binary cache has (nixpkgs' own build of it failed,
/// or it isn't free software), and not here yet: said before anything is
/// fetched. Built here when the catalog knows how (said, as it takes a
/// while), else why it can't be had, not a store path "not in any binary
/// cache" halfway through. `pinned`: the lock's paths, when it has them.
fn no_cache_has(store: &Store, index: &Index, name: &str, pinned: Option<&[String]>) -> Result<()> {
    let Ok(pkg) = index.get(name) else { return Ok(()) };
    let paths = pkg.install_paths();
    if pinned.is_some_and(|p| p != paths.as_slice()) || pkg.cache.is_some() || !(pkg.uncached || pkg.unfree) || paths.iter().all(|p| store.is_valid(crate::store::base_name(p))) {
        return Ok(());
    }
    let why = if pkg.unfree { "it isn't free software, so no binary cache has it" } else { "no binary cache has nixpkgs' build of it (nixpkgs couldn't build it)" };
    ensure!(pkg.closure.is_some(), "{name}: {why}, and this catalog has no way to build it here; a later catalog may have it, or a recipe of your own (package.nami) can make it");
    crate::say!("building {name} on this computer ({why}): a few minutes or more");
    Ok(())
}

/// Resolve, build and download a project's packages (pinned by its lock),
/// keep them safe from gc, and work out its environment variables.
pub fn prepare(store: &Store, profiles: &Profiles, file: &Path, locked: bool, upgrade: bool) -> Result<Env> {
    let project = read(file)?;
    let dir = fs::canonicalize(file.parent().unwrap_or(Path::new(".")))?;
    let services = crate::up::resolve(&project.services, &dir).with_context(|| format!("in {}", file.display()))?;
    // Its services' packages are its tools too: on PATH, and in the lock.
    let mut wanted = project.packages.clone();
    for p in services.iter().flat_map(|s| &s.packages) {
        if !wanted.contains(p) {
            wanted.push(p.clone());
        }
    }
    let lock_path = dir.join(lock_for(file).file_name().unwrap_or_default());
    let lock_name = lock_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let file_name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let old_lock = lock::load(&lock_path)?;
    // The catalog the lock pins (unless moving on with --upgrade).
    let pinned_catalog = old_lock.catalog.as_ref().filter(|_| !upgrade);
    let index = match pinned_catalog {
        Some(c) => match Index::load_pinned(store, &c.sha256, c.source_for(store).as_deref()) {
            Ok(i) => Some(i),
            Err(e) => {
                let missing: Vec<&String> = wanted.iter().filter(|n| !Builder::is_recipe(n) && !old_lock.packages.contains_key(*n)).collect();
                if !missing.is_empty() || project.develop == Some(true) {
                    bail!("this project's catalog isn't available here ({e:#}); `nami shell --upgrade` moves it to today's catalog");
                }
                Index::load(store).ok() // everything is pinned by path anyway
            }
        },
        None => Index::load(store).ok(),
    };
    if let (Some(index), false) = (index.as_ref(), locked) {
        all_in_catalog(file, &wanted, index, |n| !upgrade && old_lock.packages.contains_key(n))?;
    }

    let mut entries = Vec::new();
    let mut locked_now = BTreeMap::new();
    for name in &wanted {
        if crate::ops::is_remote(name) {
            let index = index.as_ref().ok_or_else(|| anyhow!("no package catalog here yet; run `sudo nami update --from ...`"))?;
            let b = { let mut b = Builder::new(store, index); b.allow_unfree = project.unfree.unwrap_or(true); b };
            let pinned = old_lock.packages.get(name).filter(|_| !upgrade);
            let (entry, rev) = match pinned {
                Some(l) if l.paths.iter().all(|p| store.is_valid(crate::store::base_name(p))) => (Entry { attr: name.clone(), name: l.name.clone(), paths: l.paths.clone() }, l.rev.clone()),
                Some(l) => {
                    let (e, sha) = crate::ops::resolve_remote(&b, store, name, l.rev.as_deref())?;
                    ensure!(!locked || e.paths == l.paths, "{name} builds something different from what the lock says");
                    (e, Some(sha))
                }
                None => {
                    ensure!(!locked, "{name} isn't in the lock; run without --locked to add it");
                    let (e, sha) = crate::ops::resolve_remote(&b, store, name, None)?;
                    (e, Some(sha))
                }
            };
            locked_now.insert(name.clone(), lock::Locked { name: entry.name.clone(), paths: entry.paths.clone(), rev, inputs: Vec::new() });
            entries.push(entry);
            continue;
        }
        let pinned = old_lock.packages.get(name).filter(|_| !upgrade && !Builder::is_recipe(name));
        if let Some(index) = &index {
            no_cache_has(store, index, name, pinned.map(|l| l.paths.as_slice()))?;
        }
        let entry = match (pinned, &index) {
            (Some(l), Some(_)) if crate::python_env::parse(name).is_some() => crate::python_env::pinned(store, name, l, locked)?,
            (Some(l), Some(index)) => {
                // Pinned: the lock's paths, built here if the catalog says so.
                if index.get(name).is_ok_and(|p| p.install_paths() == l.paths) {
                    let mut b = Builder::new(store, index);
                    b.allow_unfree = project.unfree.unwrap_or(true);
                    b.ensure_catalog(name)?;
                }
                Entry { attr: name.clone(), name: l.name.clone(), paths: l.paths.clone() }
            }
            // A Python with packages: made here again from what the lock says it's made of.
            (Some(l), None) if crate::python_env::parse(name).is_some() => crate::python_env::pinned(store, name, l, locked)?,
            (Some(l), None) => Entry { attr: name.clone(), name: l.name.clone(), paths: l.paths.clone() },
            (None, _) => {
                // Recipes are always built from the repo (and checked against the lock below).
                ensure!(!locked || Builder::is_recipe(name), "{name} isn't in {lock_name}; run without --locked to add it");
                let index = index.as_ref().ok_or_else(|| anyhow!("no package catalog here yet; run `sudo nami update --from ...`"))?;
                let mut b = Builder::new(store, index);
                b.allow_unfree = project.unfree.unwrap_or(true);
                crate::ops::resolve(&b, index, &dir, name)?
            }
        };
        if locked && Builder::is_recipe(name) {
            if let Some(l) = old_lock.packages.get(name) {
                ensure!(l.paths == entry.paths, "{name} builds something different from what {lock_name} says; run without --locked to update it");
            }
        }
        let inputs = crate::python_env::inputs(store, &entry);
        locked_now.insert(name.clone(), lock::Locked { name: entry.name.clone(), paths: entry.paths.clone(), rev: None, inputs });
        entries.push(entry);
    }
    let plan = crate::replace::Plan { everywhere: project.replace.clone(), only: project.replace_in.clone() };
    if !plan.is_empty() {
        let index = index.as_ref().ok_or_else(|| anyhow!("no package catalog here yet; run `sudo nami update --from ...`"))?;
        let mut b = Builder::new(store, index);
        b.allow_unfree = project.unfree.unwrap_or(true);
        crate::replace::apply(&b, index, &dir, &plan, entries.iter_mut().map(|e| (e.attr.as_str(), &mut e.paths)).collect())?;
    }
    let mut paths: Vec<String> = entries.iter().flat_map(|e| e.paths.clone()).collect();
    store.realise(&paths)?;
    // What the services give the project (DATABASE_URL, ...), then its own env.
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    let mut from: BTreeMap<String, &str> = BTreeMap::new();
    for s in &services {
        for (k, v) in &s.exports {
            if env.insert(k.clone(), v.clone()).is_some_and(|old| &old != v) && !project.env.contains_key(k) {
                eprintln!("{k}: {} and {} both set it; {}'s is used (`env: {{ {k}: ... }}` in {file_name} picks one)", from[k], s.name, s.name);
            }
            from.insert(k.clone(), &s.name);
        }
    }
    env.extend(project.env.clone());
    // Secrets are opened into memory only: the environment, never a file
    // (the build environment's script below doesn't get them).
    let mut secret_env = BTreeMap::new();
    if env.values().any(|v| crate::secrets::marked(v).is_some()) {
        let keys = crate::secrets::Keys::mine();
        for (k, v) in env.iter().filter(|(_, v)| crate::secrets::marked(v).is_some()) {
            let where_ = if project.env.contains_key(k) { format!("env.{k}") } else { format!("services.{}.exports.{k}", from.get(k).copied().unwrap_or("?")) };
            let plain = keys.reveal(v).map_err(|e| anyhow!("{}: {where_}: can't open the secret: {e:#}; {}", file.display(), crate::secrets::access_hint()))?;
            secret_env.insert(k.clone(), plain);
        }
        env.retain(|k, _| !secret_env.contains_key(k));
    }
    let reason = develop_reason(&project, index.as_ref());
    if let Some(r) = reason.as_ref().filter(|_| project.develop.is_none()) {
        eprintln!("build environment on ({r}); `develop: false` in {file_name} turns it off");
    }
    let rc = if reason.is_some() {
        let index = index.as_ref().ok_or_else(|| anyhow!("no package catalog here yet; run `sudo nami update --from ...`"))?;
        Some(develop_rc(store, index, &dir, &project, &env, &entries, &mut paths)?)
    } else {
        None
    };

    let new_lock = lock::Lock {
        version: 1,
        catalog: if pinned_catalog.is_none() { lock::catalog(&store.var_dir()) } else { old_lock.catalog.clone() },
        packages: locked_now,
        runtime: old_lock.runtime.clone(), // never a project's; kept rather than dropped
        layers: old_lock.layers.clone(),
        // (A recipe of the project's, built just now, may have pinned its sources here.)
        sources: lock::load(&lock_path).map(|l| l.sources).unwrap_or_else(|_| old_lock.sources.clone()),
    };
    if locked {
        ensure!(new_lock.packages == old_lock.packages, "{lock_name} would change; run without --locked");
    } else if new_lock != old_lock {
        lock::save(&lock_path, &new_lock)?;
        eprintln!("wrote {}", lock_path.display());
    }
    profiles.register_project(&dir, &paths)?;

    let secrets: Vec<(String, String)> = secret_env.clone().into_iter().collect();
    env.extend(secret_env);
    let vars = env_vars(&dir, &paths, &env);
    Ok(Env { dir, entries, vars, rc, services, secrets })
}

/// `develop: true`: the stdenv setup nixpkgs builds run, with the project's
/// packages as inputs: their `dev` outputs (headers, pkg-config files) and
/// their programs. Writes it as a bash script to source; returns its path.
fn develop_rc(store: &Store, index: &Index, dir: &Path, project: &Project, env: &BTreeMap<String, String>, entries: &[Entry], roots: &mut Vec<String>) -> Result<PathBuf> {
    let out = |attr: &str, o: &str| index.get(attr).ok().and_then(|p| p.outputs.get(o).cloned().flatten());
    let stdenv = out("stdenv", "out").ok_or_else(|| anyhow!("the catalog has no stdenv"))?;
    let mut build_inputs = Vec::new();
    let mut native = Vec::new();
    for e in entries {
        if let Ok(pkg) = index.get(&e.attr) {
            for o in ["dev", "out", "lib", "bin"] {
                if let Some(Some(p)) = pkg.outputs.get(o) {
                    if o == "bin" || o == "out" && !pkg.outputs.contains_key("bin") {
                        native.push(p.clone());
                    }
                    if o != "bin" {
                        build_inputs.push(p.clone());
                    }
                }
            }
        } else {
            native.extend(e.paths.clone()); // a recipe: what it built
        }
    }
    if !project.packages.iter().any(|p| p == "pkg-config") {
        native.extend(out("pkg-config", "out")); // so `pkg-config --cflags` just works
    }
    let mut needed = vec![stdenv.clone()];
    // The bash it's sourced by: nixpkgs' (setup needs bash 4 or newer; a Mac's own is 3.2).
    let bash = if crate::index::MAC { Some(out("bashInteractive", "out").or_else(|| out("bash", "out")).ok_or_else(|| anyhow!("the catalog has no bash, which a build environment needs on a Mac"))?) } else { None };
    needed.extend(bash.clone());
    needed.extend(build_inputs.iter().cloned());
    needed.extend(native.iter().cloned());
    needed.sort();
    needed.dedup();
    store.realise(&needed)?;
    roots.extend(needed);

    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let work = store.cache_dir().join("plates").join(key(dir));
    fs::create_dir_all(&work)?;
    let mut rc = String::from("# Written by nami for the project in ");
    rc += &format!("{}: nixpkgs' build environment, like `nix develop`.\n", dir.display());
    rc += "_nami_path=\"$PATH\"\n";
    rc += &format!("export stdenv={}\n", quote(&stdenv));
    rc += &format!("export buildInputs={}\n", quote(&build_inputs.join(" ")));
    rc += &format!("export nativeBuildInputs={}\n", quote(&native.join(" ")));
    rc += &format!("export out={} NIX_BUILD_TOP=\"${{TMPDIR:-/tmp}}\" NIX_BUILD_CORES=\"$(nproc 2>/dev/null || echo 1)\"\n", quote(&work.join("out").display().to_string()));
    rc += "export SHELL=\"${SHELL:-/bin/bash}\" NIX_STORE=/nix/store SOURCE_DATE_EPOCH=315532800 TERM=\"${TERM:-xterm-256color}\"\n";
    rc += &source_setup();
    // Back to an interactive shell: the build's strict mode and exit trap off,
    // your own programs after the project's.
    rc += "set +eu; set +o pipefail; trap - EXIT; export PATH=\"$PATH:$_nami_path\"; unset _nami_path\n";
    // Your project is outside the store: the compiler mustn't skip its paths (-I ~/code/x).
    rc += "export NIX_ENFORCE_PURITY=0\n";
    for (k, v) in env {
        rc += &format!("export {k}={}\n", quote(v));
    }
    rc += &format!("export NAMI_PROJECT={}\n", quote(&dir.display().to_string()));
    let path = work.join("develop.sh");
    fs::write(&path, rc)?;
    let link = work.join("bash");
    let _ = fs::remove_file(&link);
    if let Some(bash) = bash {
        std::os::unix::fs::symlink(store.phys(&bash).join("bin/bash"), &link)?;
    }
    Ok(path)
}

/// The bash a build environment (`rc`) is sourced by: nixpkgs' beside it where it chose one
/// (a Mac), else the computer's.
pub fn bash_for(rc: &Path) -> PathBuf {
    let own = rc.with_file_name("bash");
    if own.exists() {
        own
    } else {
        PathBuf::from("bash")
    }
}

/// Each package's setup hook sourced once. A package with no `bin` output
/// (zig, cmake, a compiler) is both a tool and an input, so stdenv's setup
/// activates it twice, and a hook that isn't written to be sourced again
/// (zig's `readonly zigDefaultCpuFlag`) stops the whole setup. The first
/// time is as a tool (stdenv activates those first), the role that matters
/// in a shell. Only hooks are skipped: setup's other sources go through.
const HOOKS_ONCE: &str = r#"declare -A _nami_hooks=()
source() {
    if [[ $# -eq 1 && $1 == */nix-support/setup-hook ]]; then
        [[ -n ${_nami_hooks[$1]-} ]] && return 0
        _nami_hooks[$1]=1
    fi
    builtin source "$@"
}
"#;

/// Sourcing `$stdenv/setup`, each hook once (HOOKS_ONCE).
fn source_setup() -> String {
    format!("{HOOKS_ONCE}builtin source \"$stdenv/setup\"\nunset -f source; unset _nami_hooks\n")
}

/// The environment `develop` sets up, as variables (for `nami env`).
pub fn develop_vars(rc: &Path) -> Result<Vec<(String, String)>> {
    let out = std::process::Command::new(bash_for(rc))
        .arg("-c")
        .arg(format!("source '{}' >/dev/null 2>&1; env -0", rc.display()))
        .output()
        .context("running bash")?;
    let before: BTreeMap<String, String> = std::env::vars().collect();
    let skip = ["_", "SHLVL", "PWD", "OLDPWD"];
    Ok(out
        .stdout
        .split(|&b| b == 0)
        .filter_map(|kv| String::from_utf8_lossy(kv).split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .filter(|(k, v)| !skip.contains(&k.as_str()) && before.get(k) != Some(v) && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .collect())
}

/// PATH and friends with the project's packages first, then its own `env`.
fn env_vars(dir: &Path, paths: &[String], env: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let join = |sub: &str, existing: &str, fallback: &str| -> String {
        let mut parts: Vec<String> = paths.iter().map(|p| format!("{p}/{sub}")).filter(|p| Path::new(p).exists()).collect();
        let rest = std::env::var(existing).unwrap_or_else(|_| fallback.to_string());
        if !rest.is_empty() {
            parts.push(rest);
        }
        parts.join(":")
    };
    let mut vars = vec![
        ("PATH".to_string(), join("bin", "PATH", "/usr/bin:/bin")),
        ("XDG_DATA_DIRS".to_string(), join("share", "XDG_DATA_DIRS", "/usr/local/share:/usr/share")),
        ("MANPATH".to_string(), join("share/man", "MANPATH", "")),
        ("NAMI_PROJECT".to_string(), dir.display().to_string()),
    ];
    vars.extend(env.iter().map(|(k, v)| (k.clone(), v.clone())));
    vars
}

/// A stable name for a project directory (its gc root file).
pub fn key(dir: &Path) -> String {
    Sha256::digest(dir.as_os_str().as_encoded_bytes()).iter().take(10).map(|b| format!("{b:02x}")).collect()
}

/// `export K='v'` lines for sh-like shells (direnv: `eval "$(nami env)"`), or fish's `set -gx`.
pub fn exports(vars: &[(String, String)], fish: bool) -> String {
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let lists = ["PATH", "XDG_DATA_DIRS", "MANPATH"];
    vars.iter()
        .map(|(k, v)| match (fish, lists.contains(&k.as_str())) {
            (true, true) => format!("set -gx {k} {}\n", v.split(':').filter(|p| !p.is_empty()).map(quote).collect::<Vec<_>>().join(" ")),
            (true, false) => format!("set -gx {k} {}\n", quote(v)),
            (false, _) => format!("export {k}={}\n", quote(v)),
        })
        .collect()
}

/// The project file for `<dir|file|github:owner/repo[/dir][@ref]>`, or the nearest one.
pub fn locate(store: &Store, arg: Option<&str>) -> Result<PathBuf> {
    if let Some(spec) = arg.and_then(|a| a.strip_prefix("github:")) {
        return remote(store, spec);
    }
    let start = match arg.map(PathBuf::from) {
        Some(p) if p.is_file() => {
            ensure!(is_project(&p), "{} is a system or user file, not a project's; `nami apply {}` applies it", p.display(), p.display());
            return Ok(p);
        }
        Some(p) => p,
        None => std::env::current_dir()?,
    };
    match find(&start) {
        Some(f) => Ok(f),
        None => bail!("no {FILE} in {} or above; create one with the tools this project needs:\n\n  packages:\n    - nodejs\n    - ripgrep\n", start.display()),
    }
}

/// The project file for `<dir|file>`, or the nearest one: a project of
/// yours, for its services (they keep their data in its folder).
pub fn locate_local(arg: Option<&str>) -> Result<PathBuf> {
    ensure!(!arg.is_some_and(|a| a.starts_with("github:")), "services run in a project of yours: clone it, then `nami up` in its folder");
    let start = arg.map(PathBuf::from).unwrap_or(std::env::current_dir()?);
    if start.is_file() {
        return Ok(start);
    }
    find(&start).ok_or_else(|| anyhow!("no {FILE} in {} or above", start.display()))
}

/// `github:owner/repo[/dir][@ref]`: someone else's project, downloaded at an
/// exact commit (the ref is resolved once, then cached by commit), so its
/// own nami.lock decides the tools.
fn remote(store: &Store, spec: &str) -> Result<PathBuf> {
    let (dir, sha, label) = fetch_github(store, spec)?;
    let file = file_in(&dir).ok_or_else(|| anyhow!("github:{label} has no {FILE}"))?;
    eprintln!("project from github:{label} at {}", &sha[..12]);
    Ok(file)
}

/// Download `owner/repo[/dir][@ref]` at the commit the ref points to (once:
/// cached by commit). Returns the directory, the commit, and a label.
/// NAMI_GITHUB_MIRROR=<dir> reads from a local mirror instead (offline use,
/// tests): `<owner>/<repo>/refs/<ref>` holds a commit, `<owner>/<repo>/<commit>.tar.gz` its source.
pub fn fetch_github(store: &Store, spec: &str) -> Result<(PathBuf, String, String)> {
    // The one form of git references (gitref.rs), without a #name.
    let parsed = crate::gitref::parse(&format!("github:{spec}"), crate::gitref::Use::Repo)
        .ok()
        .filter(|r| r.name.is_none())
        .ok_or_else(|| anyhow!("write it as github:owner/repo, github:owner/repo/some/dir or github:owner/repo@v1.2"))?;
    let crate::gitref::Repo::GitHub { owner, repo } = &parsed.repo else { unreachable!() };
    let (owner, repo) = (owner.as_str(), repo.as_str());
    let sub = parsed.path.as_str();
    let git_ref = parsed.git_ref.as_deref().unwrap_or("HEAD");
    let mirror = std::env::var_os("NAMI_GITHUB_MIRROR").map(PathBuf::from);
    let sha = if git_ref.len() == 40 && git_ref.bytes().all(|b| b.is_ascii_hexdigit()) {
        git_ref.to_string()
    } else if let Some(m) = &mirror {
        fs::read_to_string(m.join(owner).join(repo).join("refs").join(git_ref)).with_context(|| format!("{owner}/{repo}@{git_ref} isn't in the mirror"))?.trim().to_string()
    } else {
        store
            .http()
            .get(&format!("https://api.github.com/repos/{owner}/{repo}/commits/{git_ref}"))
            .set("Accept", "application/vnd.github.sha")
            .set("User-Agent", "nami")
            .call()
            .with_context(|| format!("looking up {owner}/{repo}@{git_ref} on GitHub"))?
            .into_string()?
            .trim()
            .to_string()
    };
    ensure!(sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()), "GitHub didn't return a commit for {owner}/{repo}@{git_ref}");
    let dest = store.cache_dir().join("remote").join(format!("github-{owner}-{repo}-{}", &sha[..12]));
    if !dest.is_dir() {
        eprintln!("downloading github:{owner}/{repo} at {}", &sha[..12]);
        let tmp = dest.with_extension("tmp");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp)?;
        let tarball = tmp.join("source.tar.gz");
        match &mirror {
            Some(m) => {
                fs::copy(m.join(owner).join(repo).join(format!("{sha}.tar.gz")), &tarball).with_context(|| format!("{owner}/{repo} at {sha} isn't in the mirror"))?;
            }
            None => {
                let mut body = store
                    .http()
                    .get(&format!("https://codeload.github.com/{owner}/{repo}/tar.gz/{sha}"))
                    .call()
                    .with_context(|| format!("downloading {owner}/{repo}"))?
                    .into_reader();
                std::io::copy(&mut body, &mut fs::File::create(&tarball)?)?;
            }
        }
        let status = std::process::Command::new("tar")
            .args(["-xzf", "source.tar.gz", "--strip-components=1", "--no-same-owner"])
            .current_dir(&tmp)
            .status()
            .context("running tar")?;
        ensure!(status.success(), "unpacking {owner}/{repo} failed");
        fs::remove_file(&tarball)?;
        fs::rename(&tmp, &dest)?;
    }
    let label = format!("{owner}/{repo}{}", if sub.is_empty() { String::new() } else { format!("/{sub}") });
    Ok((dest.join(sub), sha, label))
}

/// What a project's environment looked like before `nami hook` loaded it:
/// variable -> its old value (None: it wasn't set). Kept hex-encoded in
/// NAMI_HOOK_SAVED, so any value survives the trip through the shell.
fn saved_vars() -> BTreeMap<String, Option<String>> {
    let hex = hook_var("SAVED").unwrap_or_default();
    let bytes: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()).collect();
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// The stamp of a project: when its file and lock last changed.
fn stamp(file: &Path) -> String {
    let m = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).map(|t| format!("{:?}", t)).unwrap_or_default();
    format!("{}|{}|{}", file.display(), m(file), m(&lock_for(file)))
}

/// The hook's own state: NAMI_HOOK_<name>, or NAMI_PLATE_<name> in a shell
/// an older nami set up.
fn hook_var(name: &str) -> Option<String> {
    std::env::var(format!("NAMI_HOOK_{name}")).or_else(|_| std::env::var(format!("NAMI_PLATE_{name}"))).ok()
}

/// `nami hook-env`: run before every prompt by `nami hook <shell>`. Loads the
/// project of the current directory, unloads the one you left, and is silent
/// (and doesn't touch the store) when nothing changed.
pub fn hook_env(fish: bool, prepare: impl FnOnce(&Path) -> Result<Vec<(String, String)>>) -> Result<String> {
    let here = std::env::current_dir()?;
    let target = find(&here);
    let active = hook_var("DIR").filter(|s| !s.is_empty());
    let target_dir = target.as_ref().and_then(|f| f.parent()).map(|d| d.display().to_string());
    if target_dir == active && target.as_ref().map(|f| stamp(f)) == hook_var("STAMP") {
        return Ok(String::new());
    }
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let set = |k: &str, v: &str| if fish { format!("set -gx {k} {}\n", quote(v)) } else { format!("export {k}={}\n", quote(v)) };
    let unset = |k: &str| if fish { format!("set -e {k}\n") } else { format!("unset {k}\n") };
    let saved = saved_vars();
    let mut out = String::new();
    // Back to how things were before the project we're leaving (or reloading).
    for (k, v) in &saved {
        match v {
            Some(v) => {
                out += &set(k, v);
                std::env::set_var(k, v);
            }
            None => {
                out += &unset(k);
                std::env::remove_var(k);
            }
        }
    }
    for k in ["NAMI_HOOK_DIR", "NAMI_HOOK_STAMP", "NAMI_HOOK_SAVED", "NAMI_PLATE_DIR", "NAMI_PLATE_STAMP", "NAMI_PLATE_SAVED"] {
        out += &unset(k);
    }
    if active.is_some() && target.is_none() {
        eprintln!("nami: left the project in {}", active.unwrap_or_default());
    }
    let Some(file) = target else { return Ok(out) };
    let dir = file.parent().unwrap_or(Path::new("."));
    // Whatever happens, remember this project, so a broken one isn't retried at every prompt.
    out += &set("NAMI_HOOK_DIR", &dir.display().to_string());
    out += &set("NAMI_HOOK_STAMP", &stamp(&file));
    match prepare(&file) {
        Ok(vars) => {
            let before: BTreeMap<String, Option<String>> = vars.iter().map(|(k, _)| (k.clone(), std::env::var(k).ok())).collect();
            for (k, v) in &vars {
                out += &if fish && ["PATH", "XDG_DATA_DIRS", "MANPATH"].contains(&k.as_str()) {
                    format!("set -gx {k} {}\n", v.split(':').filter(|p| !p.is_empty()).map(&quote).collect::<Vec<_>>().join(" "))
                } else {
                    set(k, v)
                };
            }
            let hex: String = serde_json::to_vec(&before)?.iter().map(|b| format!("{b:02x}")).collect();
            out += &set("NAMI_HOOK_SAVED", &hex);
            eprintln!("nami: project loaded from {}", dir.display());
        }
        // Its file's problems, as `nami check` says them, each on its line.
        Err(e) if crate::style::is_said_as_check(&format!("{e:#}")) => eprintln!("nami: the project in {} didn't load:\n{e:#}", dir.display()),
        Err(e) => eprintln!("nami: the project in {} didn't load: {e:#}", dir.display()),
    }
    Ok(out)
}

/// The shell code `nami hook <shell>` prints, to put in your shell's startup file.
pub fn hook_script(shell: &str) -> Result<&'static str> {
    Ok(match shell {
        "bash" => "_nami_hook() { local s=$?; eval \"$(nami hook-env)\"; return $s; }\ncase \";${PROMPT_COMMAND:-};\" in *\";_nami_hook;\"*) ;; *) PROMPT_COMMAND=\"_nami_hook${PROMPT_COMMAND:+;$PROMPT_COMMAND}\" ;; esac\n",
        "zsh" => "_nami_hook() { eval \"$(nami hook-env)\"; }\nautoload -Uz add-zsh-hook\nadd-zsh-hook precmd _nami_hook\nadd-zsh-hook chpwd _nami_hook\n",
        "fish" => "function _nami_hook --on-variable PWD --on-event fish_prompt\n    nami hook-env --fish | source\nend\n",
        other => bail!("nami hook knows bash, zsh and fish, not {other:?}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasks_are_a_command_or_a_map_and_run_what_they_depend_on_first() {
        let p: Project = serde_yaml::from_str("tasks:\n  build: make\n  lint: { run: lint ., description: Checks the style }\n  test: { run: make test, depends: [build, lint] }\n  all: { depends: [test, build] }\n").unwrap();
        assert_eq!(p.tasks["build"], Task { run: Some("make".into()), ..Task::default() });
        assert_eq!(p.tasks["lint"].shown(), "Checks the style");
        assert_eq!(p.tasks["build"].shown(), "make");
        assert_eq!(task_order(&p.tasks, "test").unwrap(), ["build", "lint", "test"]);
        // Each once.
        assert_eq!(task_order(&p.tasks, "all").unwrap(), ["build", "lint", "test", "all"]);
        assert!(format!("{:#}", task_order(&p.tasks, "deploy").unwrap_err()).contains("no task deploy"));
        let e = serde_yaml::from_str::<Project>("tasks: { test: [a, b] }\n").unwrap_err().to_string();
        assert!(e.contains("a task is a command"), "{e}");
        let e = serde_yaml::from_str::<Project>("tasks: { test: { description: x } }\n").unwrap_err().to_string();
        assert!(e.contains("run:"), "{e}");
        let cyc: Project = serde_yaml::from_str("tasks: { a: { run: x, depends: b }, b: { run: y, depends: [a] } }\n").unwrap();
        assert!(format!("{:#}", task_order(&cyc.tasks, "a").unwrap_err()).contains("in a circle (a -> b -> a)"));
    }

    #[test]
    fn service_versions_as_written() {
        let text = "services:\n  - python-api:\n      use: postgres\n      version: 3.10\n  - redis: { version: 7 }\n  - db: { version: \"16\" }\n";
        let mut value: serde_yaml::Value = crate::yaml_text::from_str(text).unwrap();
        crate::config::package_map(&mut value, "t").unwrap();
        let v = |n: &str| value["services"][n]["version"].as_str().map(String::from).or_else(|| value["services"][n]["version"].as_u64().map(|n| n.to_string()));
        assert_eq!(v("python-api").as_deref(), Some("3.10"), "not 3.1");
        assert_eq!(v("redis").as_deref(), Some("7"));
        assert_eq!(v("db").as_deref(), Some("16"));
    }

    fn tree(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nami-project-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("a/b")).unwrap();
        dir
    }

    #[test]
    fn system_and_user_style_files_are_not_projects() {
        assert!(is_project_text("packages: [jq]\nenv: { A: b }\n"));
        assert!(is_project_text("packages: [jq]\n")); // could be either: a project
        assert!(is_project_text("packages: [jq]\nunfree: false\n"));
        assert!(is_project_text("tasks: { test: cargo test }\nhostname: x\n")); // a mistake; its errors get shown
        assert!(is_project_text(": not yaml: [")); // so are these
        assert!(!is_project_text("packages: [jq]\naliases: { ll: ls -l }\n"));
        assert!(!is_project_text("history: 10\n"));
        assert!(!is_project_text("caches: []\nsecrets: { recipients: [] }\n"));
        assert!(has_project_keys("develop: true\n") && !has_project_keys("packages: [jq]\nhostname: x\n"));
    }

    #[test]
    fn a_package_no_cache_has_is_said_before_anything_is_fetched() {
        let root = std::env::temp_dir().join(format!("nami-project-uncached-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("nix/var/nami")).unwrap();
        let index = r#"{"firecracker":{"name":"firecracker-1.17.0","outputs":{"out":"/nix/store/hp3j28k0v0c3dpdap5i04z2cbgg8m9x4-firecracker-1.17.0"},"uncached":true},
            "buildable":{"name":"buildable-1","outputs":{"out":"/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-buildable-1"},"uncached":true,"closure":"0000"},
            "shellcheck":{"name":"shellcheck-0.11.0","outputs":{"bin":"/nix/store/yrynapygpcy20zxrwr15n1g769vk59ji-shellcheck-0.11.0-bin"}}}"#;
        fs::write(root.join("nix/var/nami/index.json"), index).unwrap();
        let store = Store::open_direct_readonly(&root).unwrap();
        let index = Index::load(&store).unwrap();
        let e = no_cache_has(&store, &index, "firecracker", None).unwrap_err().to_string();
        assert_eq!(e, "firecracker: no binary cache has nixpkgs' build of it (nixpkgs couldn't build it), and this catalog has no way to build it here; a later catalog may have it, or a recipe of your own (package.nami) can make it");
        // Built here (said), from the cache, a recipe, or pinned elsewhere: nothing to say.
        no_cache_has(&store, &index, "buildable", None).unwrap();
        no_cache_has(&store, &index, "shellcheck", None).unwrap();
        no_cache_has(&store, &index, "./vm/firecracker", None).unwrap();
        no_cache_has(&store, &index, "firecracker", Some(&["/nix/store/kw4sgn8pmd35jmpm302jq0ffkpk2ww1r-firecracker-1.17.0".to_string()])).unwrap();
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_name_not_in_the_catalog_is_said_as_check_says_it() {
        let root = std::env::temp_dir().join(format!("nami-project-typo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("nix/var/nami")).unwrap();
        let pkg = |n: &str| format!("\"{n}\":{{\"name\":\"{n}-1\",\"outputs\":{{\"out\":\"/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-{n}-1\"}}}}");
        fs::write(root.join("nix/var/nami/index.json"), format!("{{{},{},{}}}", pkg("jq"), pkg("nodejs"), pkg("ripgrep"))).unwrap();
        let store = Store::open_direct_readonly(&root).unwrap();
        let index = Index::load(&store).unwrap();
        let file = root.join(FILE);
        fs::write(&file, "packages:\n  - nodejs\n  - ripgrpe\n  - jqq\n").unwrap();
        let wanted: Vec<String> = ["nodejs", "ripgrpe", "jqq"].map(String::from).to_vec();
        let shown = crate::config::path_from(&std::env::current_dir().unwrap(), &file).display().to_string();
        let e = all_in_catalog(&file, &wanted, &index, |_| false).unwrap_err().to_string();
        assert_eq!(
            e,
            format!("{shown}:3: error: ripgrpe isn't in the catalog (did you mean `ripgrep`?)\n{shown}:4: error: jqq isn't in the catalog (did you mean `jq`?)")
        );
        // Said as such, not after an `error: ` of its own.
        assert!(crate::style::is_said_as_check(&e));
        assert!(!crate::style::is_said_as_check("unknown package \"ripgrpe\""));
        // What the lock pins is taken as it is; names the catalog has, too.
        all_in_catalog(&file, &wanted, &index, |n| n != "nodejs").unwrap();
        all_in_catalog(&file, &wanted[..1], &index, |_| false).unwrap();
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn find_takes_the_nearest_project() {
        let dir = tree("find");
        let deep = dir.join("a/b");
        assert_eq!(find(&deep), None);
        // A dotfiles repo's user.nami: not a project, silently.
        fs::write(dir.join("a/user.nami"), "packages: [git]\naliases: { gs: git status }\n").unwrap();
        assert_eq!(find(&deep), None);
        fs::write(dir.join("project.nami"), "packages: [ripgrep]\ntasks: { t: rg --version }\n").unwrap();
        assert_eq!(find(&deep), Some(dir.join("project.nami")));
        assert_eq!(lock_for(&dir.join("project.nami")), dir.join("nami.lock"));
        assert_eq!(summary(&dir, None).unwrap().file, dir.join("project.nami").display().to_string());
        // The nearest project wins.
        fs::write(dir.join("a/b/project.nami"), "packages: []\n").unwrap();
        assert_eq!(find(&deep), Some(deep.join("project.nami")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "the computer's bash: a Mac's is 3.2, and its build environments use nixpkgs' bash")]
    fn a_setup_hook_sourced_twice_runs_once() {
        // As stdenv's setup does for zig, a tool and an input both: its
        // hook from inside activatePackage, once per role.
        let dir = tree("hooks");
        fs::create_dir_all(dir.join("zig/nix-support")).unwrap();
        fs::write(dir.join("zig/nix-support/setup-hook"), "readonly zigDefaultCpuFlag=-Dcpu=baseline\nhooked=$((hooked + 1))\n").unwrap();
        fs::write(dir.join("other.sh"), "other=$((other + 1))\n").unwrap();
        fs::write(
            dir.join("setup"),
            "set -eu\ndeclare -a fromSetup=(a b)\nhooked=0 other=0\nactivatePackage() { local -r hostOffset=$2; source \"$1/nix-support/setup-hook\"; source \"$1/../other.sh\"; }\nactivatePackage \"$Z\" -1\nactivatePackage \"$Z\" 0\n",
        )
        .unwrap();
        let script = format!("stdenv={}\n{}\necho \"$hooked $other ${{#fromSetup[@]}} $zigDefaultCpuFlag $(type -t source)\"", dir.display(), source_setup());
        let out = std::process::Command::new("bash").arg("-c").arg(&script).env("Z", dir.join("zig")).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        // The hook once, other scripts each time, setup's arrays global, and `source` the builtin again.
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "1 2 2 -Dcpu=baseline builtin");
        // Without it, the second sourcing stops setup.
        let plain = format!("stdenv={}\nsource \"$stdenv/setup\"\necho ok", dir.display());
        let out = std::process::Command::new("bash").arg("-c").arg(&plain).env("Z", dir.join("zig")).output().unwrap();
        assert!(!out.status.success() || !String::from_utf8_lossy(&out.stdout).contains("ok"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
