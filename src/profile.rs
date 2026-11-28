//! Profiles and generations.
//!
//! Every change creates a new generation: a symlink tree merging the installed
//! packages, plus a manifest. `default` points at the active one, so switching
//! and rolling back are a single atomic symlink swap.
//!
//!   /nami/profiles/default      -> default-3
//!   /nami/profiles/default-3/   bin/rg -> /nix/store/...-ripgrep-15.2.0/bin/rg
//!   /nami/profiles/default-3.json   (packages, caches, ...)
//!   /nami/profiles/runtime      -> runtime-3  (drivers, locales; see runtime.rs)
//!
//! The system profile (`profiles/`) is the system's apps, from
//! /etc/nami/system.nami; root and admins work on it (admins through the
//! daemon). Each user also has their own under `profiles/per-user/<name>/`,
//! reached through `~/.local/state/nami/profile` (like Nix's
//! `~/.nix-profile`): their home settings, and the apps of someone who isn't
//! an admin. Users change theirs without root; the store itself is changed
//! for them by the daemon (daemon.rs).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};

use crate::etc::EtcState;
use crate::home::{self, HomeState};
use crate::services::ServicesState;
use crate::index::Index;
use crate::runtime;
use crate::store::{make_writable, write_atomic, Cache, Store};

const NAME: &str = "default";
const RUNTIME: &str = "runtime";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub attr: String,
    pub name: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub generation: u64,
    pub created: u64,
    pub packages: Vec<Entry>,
    /// Support packages nami added itself (GPU drivers, locales, ...).
    #[serde(default)]
    pub runtime: Vec<Entry>,
    /// Binary caches in use, so a rollback restores them too.
    #[serde(default)]
    pub caches: Vec<Cache>,
    /// System profile: keep GPU drivers even if no system package needs
    /// them, because a user's profile does.
    #[serde(default)]
    pub graphics: bool,
    /// Rendered dotfiles (`home:` in your nami file), linked into ~.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<HomeState>,
    /// Rendered units (`services:`), copied into systemd's directories.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub services: Option<ServicesState>,
    /// Rendered /etc files (`files:`, system profile only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etc: Option<EtcState>,
    /// The nami file (and nami.lock) this generation was applied from, so
    /// install/remove edit that file and rollback can restore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<ConfigSnapshot>,
    /// Someone's own, made by undoing their first change: there's nothing before it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub start: bool,
    /// The generation this one was made from (the current one when it was made): what
    /// `nami undo` goes back to. Not the one numbered before it: after an undo, the next
    /// change is made from the one undone to, and undoing it goes back there, not forward
    /// to what was undone. (None in generations made before nami kept it.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<u64>,
}

/// A generation's services and /etc files, for `commit_with`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extras {
    pub services: Option<ServicesState>,
    pub etc: Option<EtcState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    pub file: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<String>,
}

/// Which generations `prune` deletes. The active one is always kept; a
/// generation is kept if *any* rule keeps it.
#[derive(Debug, Default, Clone, Copy)]
pub struct Prune {
    /// Delete every inactive generation.
    pub all: bool,
    /// Keep the newest N.
    pub keep: Option<usize>,
    /// Keep those younger than this many seconds.
    pub newer_than: Option<u64>,
}

/// What changed between two generations.
#[derive(Debug, Default)]
pub struct Diff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// (attr, old name, new name)
    pub changed: Vec<(String, String, String)>,
    pub runtime_added: Vec<String>,
    pub runtime_removed: Vec<String>,
    pub caches_added: Vec<String>,
    pub caches_removed: Vec<String>,
    pub home_added: Vec<String>,
    pub home_removed: Vec<String>,
    pub home_changed: Vec<String>,
    /// Units and /etc files.
    pub services_added: Vec<String>,
    pub services_removed: Vec<String>,
    pub services_changed: Vec<String>,
    pub files_added: Vec<String>,
    pub files_removed: Vec<String>,
    pub files_changed: Vec<String>,
    /// Bytes of store paths (whole closures) only in the old / new generation.
    pub size_removed: u64,
    pub size_added: u64,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
            && self.runtime_added.is_empty() && self.runtime_removed.is_empty()
            && self.caches_added.is_empty() && self.caches_removed.is_empty()
            && self.home_added.is_empty() && self.home_removed.is_empty() && self.home_changed.is_empty()
            && self.services_added.is_empty() && self.services_removed.is_empty() && self.services_changed.is_empty()
            && self.files_added.is_empty() && self.files_removed.is_empty() && self.files_changed.is_empty()
    }

    /// What changed, by kind (`tell::grouped`): Apps, Services, Files, Caches.
    /// `skip`: apps the line after says (`✓ Installed firefox`), not said twice.
    pub fn lines(&self, skip: &[String]) -> Vec<String> {
        use crate::tell::{name_version, shown};
        let mut apps = Vec::new();
        apps.extend(self.added.iter().filter(|n| !skip.contains(n)).map(|n| format!("+ {}", shown(n))));
        apps.extend(self.removed.iter().filter(|n| !skip.contains(n)).map(|n| format!("− {}", shown(n))));
        for (attr, old, new) in self.changed.iter().filter(|(_, _, n)| !skip.contains(n)) {
            let ((_, a), (name, b)) = (name_version(old), name_version(new));
            // (A package's folder, /etc/nami/packages/nami-gui: by the package's name.)
            let attr = if crate::build::Builder::is_recipe(attr) { name.as_str() } else { attr.as_str() };
            apps.push(if a.is_empty() || b.is_empty() || a == b { format!("~ {attr}") } else { format!("~ {attr} {a} → {b}") });
        }
        // What apps with windows need (graphics, fonts): one line, by name.
        let names = |l: &[String]| l.iter().map(|n| name_version(n).0).collect::<Vec<_>>().join(", ");
        if !self.runtime_added.is_empty() {
            apps.push(format!("+ for apps with windows: {}", names(&self.runtime_added)));
        }
        if !self.runtime_removed.is_empty() {
            apps.push(format!("− for apps with windows: {}", names(&self.runtime_removed)));
        }
        let mut services = Vec::new();
        for (mark, list) in [("+", &self.services_added), ("−", &self.services_removed), ("~", &self.services_changed)] {
            services.extend(list.iter().map(|u| format!("{mark} {u}")));
        }
        let mut files = Vec::new();
        for (mark, list) in [("+", &self.home_added), ("−", &self.home_removed), ("~", &self.home_changed)] {
            files.extend(list.iter().map(|f| format!("{mark} ~/{f}")));
        }
        for (mark, list) in [("+", &self.files_added), ("−", &self.files_removed), ("~", &self.files_changed)] {
            files.extend(list.iter().map(|f| format!("{mark} {f}")));
        }
        let mut caches = Vec::new();
        caches.extend(self.caches_added.iter().map(|c| format!("+ cache {c}")));
        caches.extend(self.caches_removed.iter().map(|c| format!("− cache {c}")));
        crate::tell::grouped(&[("Apps", apps), ("Services", services), ("Files", files), ("Caches", caches)])
    }

    pub fn print(&self) {
        self.print_but(&[]);
    }

    /// The lines, coloured; those of `skip` left out.
    pub fn print_but(&self, skip: &[String]) {
        let paint = crate::style::out();
        for l in self.lines(skip) {
            println!("{}", paint.line(&l));
        }
    }
}

pub struct Profiles<'a> {
    store: &'a Store,
    dir: PathBuf,
    /// The system profile owns /run/opengl-driver and the cache list.
    system: bool,
}

impl<'a> Profiles<'a> {
    pub fn system(store: &'a Store) -> Self {
        Profiles { store, dir: store.var_dir().join("profiles"), system: true }
    }

    pub fn store(&self) -> &'a Store {
        self.store
    }

    pub fn user(store: &'a Store, user: &str) -> Self {
        Profiles { store, dir: store.var_dir().join("profiles/per-user").join(user), system: false }
    }

    /// Where the active generation's home files are (what ~'s links point into).
    pub fn home_link_base(&self) -> PathBuf {
        self.dir.join(NAME).join(".home")
    }

    /// The profile for whoever runs nami: the computer's (system) for root
    /// and admins, else the user's own.
    pub fn for_current_user(store: &'a Store) -> Result<Self> {
        if is_root() || is_admin() {
            return Ok(Self::system(store));
        }
        Self::personal(store)
    }

    /// Your own profile, admin or not: what your personal nami file
    /// (home settings, and a non-admin's apps) applies to.
    pub fn personal(store: &'a Store) -> Result<Self> {
        if is_root() {
            return Ok(Self::system(store));
        }
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .ok()
            .filter(|u| !u.is_empty())
            .or_else(passwd_name)
            .ok_or_else(|| anyhow!("can't tell who you are: USER is unset"))?;
        if user.is_empty() || user.contains('/') || user.starts_with('.') {
            bail!("bad user name {user:?}");
        }
        let p = Self::user(store, &user);
        fs::create_dir_all(&p.dir).map_err(|e| anyhow!("can't create {}: {e} (was nami installed with install.sh?)", p.dir.display()))?;
        Ok(p)
    }


    pub fn is_system(&self) -> bool {
        self.system
    }

    /// Where its steps are.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn current_generation(&self) -> Option<u64> {
        let target = fs::read_link(self.dir.join(NAME)).ok()?;
        parse_generation(target.file_name()?.to_str()?)
    }

    pub fn current(&self) -> Result<Option<Manifest>> {
        self.current_generation().map(|g| self.manifest(g)).transpose()
    }

    pub fn current_packages(&self) -> Result<Vec<Entry>> {
        Ok(self.current()?.map(|m| m.packages).unwrap_or_default())
    }

    pub fn manifest(&self, generation: u64) -> Result<Manifest> {
        let path = self.dir.join(format!("{NAME}-{generation}.json"));
        let data = fs::read(&path).map_err(|_| self.missing(generation))?;
        let mut m: Manifest = serde_json::from_slice(&data)?;
        // The file as it was: beside it, the admins' alone (the computer's) or
        // its person's alone (theirs). Unreadable, it isn't there.
        if m.config.is_none() {
            if let Ok(text) = fs::read(self.snapshot_file(generation)) {
                m.config = serde_json::from_slice(&text).ok();
            }
        }
        Ok(m)
    }

    /// Where a profile keeps its nami file as a step had it: out of its
    /// manifest, which everyone reads (the apps are listed there, and so
    /// what's kept from being deleted).
    fn snapshot_file(&self, generation: u64) -> PathBuf {
        self.dir.join(format!("{NAME}-{generation}.config.json"))
    }

    /// Write step `generation`'s manifest. Its nami file goes beside it: the
    /// computer's for root and the admins alone (it can have the people's
    /// passwords in it, and secrets), a person's for them alone.
    fn write_manifest(&self, generation: u64, m: &Manifest) -> Result<()> {
        let path = self.dir.join(format!("{NAME}-{generation}.json"));
        let snap = self.snapshot_file(generation);
        match &m.config {
            Some(c) => {
                let tmp = snap.with_extension("tmp");
                let _ = fs::remove_file(&tmp);
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
                    std::io::Write::write_all(&mut f, &serde_json::to_vec_pretty(c)?)?;
                    f.sync_all()?;
                }
                if self.system {
                    for_admins(&tmp)?;
                }
                fs::rename(&tmp, &snap)?;
            }
            None => {
                let _ = fs::remove_file(&snap);
            }
        }
        let mut open = serde_json::to_value(m)?;
        if let Some(o) = open.as_object_mut() {
            o.remove("config");
        }
        write_atomic(&path, &serde_json::to_vec_pretty(&open)?)
    }

    /// The manifests that still have their nami file in them (written before
    /// it was kept beside them): moved out. The computer's by root, a
    /// person's by them (root writes nothing in someone's folder). Someone
    /// who isn't an admin keeps no copy of system.nami in theirs
    /// either (from when everyone could read it): that step goes on without.
    pub fn seal_snapshots(&self) -> Result<()> {
        if self.system != is_root() {
            return Ok(());
        }
        let not_admin = !self.system && !is_admin();
        let computers = |c: &Option<ConfigSnapshot>| not_admin && c.as_ref().is_some_and(|c| Path::new(&c.file).file_name().is_some_and(|n| is_system_file(&n.to_string_lossy())));
        for g in self.generations()? {
            let path = self.dir.join(format!("{NAME}-{g}.json"));
            let Ok(data) = fs::read(&path) else { continue };
            let Ok(mut m) = serde_json::from_slice::<Manifest>(&data) else { continue };
            if m.config.is_some() {
                if computers(&m.config) {
                    m.config = None;
                }
                self.write_manifest(g, &m)?;
            } else if computers(&fs::read(self.snapshot_file(g)).ok().and_then(|t| serde_json::from_slice(&t).ok())) {
                fs::remove_file(self.snapshot_file(g))?;
            }
        }
        Ok(())
    }

    /// Step `g` isn't there: cleaned up (an older one than a step kept), or never made.
    pub fn missing(&self, g: u64) -> anyhow::Error {
        let newest = self.generations().ok().and_then(|l| l.last().copied()).or(self.current_generation());
        missing_change(g, newest)
    }

    pub fn generations(&self) -> Result<Vec<u64>> {
        let mut gens: Vec<u64> = match fs::read_dir(&self.dir) {
            Ok(d) => d.filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".json").and_then(parse_generation)).collect(),
            Err(_) => Vec::new(),
        };
        gens.sort();
        Ok(gens)
    }

    /// Fetch everything `packages` needs, build a new generation and switch to
    /// it. `caches`/`home`: `None` keeps the current ones. Returns `None` if
    /// nothing changed.
    pub fn commit(&self, index: Option<&Index>, packages: Vec<Entry>, caches: Option<Vec<Cache>>, home: Option<Option<HomeState>>, locked_runtime: Option<Vec<Entry>>) -> Result<Option<u64>> {
        self.commit_with(index, packages, caches, home, None, locked_runtime)
    }

    /// `commit`, with services and /etc files (`None` keeps the current ones).
    pub fn commit_with(&self, index: Option<&Index>, mut packages: Vec<Entry>, caches: Option<Vec<Cache>>, home: Option<Option<HomeState>>, extras: Option<Extras>, locked_runtime: Option<Vec<Entry>>) -> Result<Option<u64>> {
        packages.sort_by(|a, b| a.attr.cmp(&b.attr));
        packages.dedup_by(|a, b| a.attr == b.attr);
        let current = self.current()?;
        let caches = match caches {
            Some(c) => c,
            None => current.as_ref().map(|m| m.caches.clone()).unwrap_or_default(),
        };
        let home = match home {
            Some(h) => h,
            None => current.as_ref().and_then(|m| m.home.clone()),
        };
        let Extras { services, etc } = match extras {
            Some(x) => x,
            None => current.as_ref().map(|m| Extras { services: m.services.clone(), etc: m.etc.clone() }).unwrap_or_default(),
        };
        match &current {
            Some(m) if m.packages == packages && m.caches == caches && m.home == home && m.services == services && m.etc == etc => {
                // (A Mac's apps, in step even so: an app taken out of Applications by hand comes back.)
                self.sync_mac_apps(m);
                return Ok(None);
            }
            None if packages.is_empty() && home.is_none() && caches.is_empty() && services.is_none() && etc.is_none() => return Ok(None),
            _ => {}
        }
        if self.system {
            self.store.set_caches(&caches)?;
        } else {
            self.store.check_user_caches(&caches)?;
        }

        let paths: Vec<String> = packages.iter().flat_map(|p| p.paths.clone()).collect();
        self.store.realise(&paths)?;
        let host = self.store.root();
        let graphics = current.as_ref().is_some_and(|m| m.graphics);
        let (runtime, graphics) = if self.system {
            let needed = graphics || runtime::needs_graphics(self.store, &paths)?;
            let runtime = match locked_runtime.filter(|r| !r.is_empty()) {
                Some(r) if needed => r, // nami.lock says exactly which drivers, fonts, locales
                _ => {
                    let index = index.ok_or_else(|| anyhow!("no package catalog here yet; run `sudo nami update --from ...` first"))?;
                    runtime::wanted(self.store, index, &paths, host, graphics)?
                }
            };
            (runtime, graphics)
        } else {
            // GPU drivers are a machine thing: ask the system profile for them.
            if runtime::needs_graphics(self.store, &paths)? {
                self.store.ensure_graphics()?;
            }
            (Vec::new(), false)
        };
        let runtime_paths: Vec<String> = runtime.iter().flat_map(|p| p.paths.clone()).collect();
        self.store.realise(&runtime_paths)?;

        let generation = self.generations()?.last().copied().unwrap_or(0) + 1;
        let tree = self.build_tree(&format!("{NAME}-{generation}"), &paths)?;
        link_parts(&tree, home.as_ref(), services.as_ref(), etc.as_ref())?;
        if self.system {
            let runtime_tree = self.build_tree(&format!("{RUNTIME}-{generation}"), &runtime_paths)?;
            runtime::populate(&runtime_tree, host, &runtime)?;
        }

        let manifest = Manifest { generation, created: now(), packages, runtime, caches, graphics, home, services, etc, config: current.as_ref().and_then(|m| m.config.clone()), start: false, from: self.current_generation() };
        self.write_manifest(generation, &manifest)?;
        self.switch(generation)?;
        Ok(Some(generation))
    }

    /// Someone's own profile before its first change: a new change with nothing in it,
    /// and no nami file of its own (so undoing the first change takes its apps out of
    /// the file too). Returns its number.
    pub fn back_to_the_start(&self) -> Result<u64> {
        anyhow::ensure!(!self.system, "the computer's first change can't be undone this way");
        let g = self.commit_with(None, Vec::new(), Some(Vec::new()), Some(None), Some(Extras::default()), None)?
            .ok_or_else(|| anyhow!("nothing to undo: nothing is installed"))?;
        let mut m = self.manifest(g)?;
        m.config = None;
        m.start = true;
        self.write_manifest(g, &m)?;
        Ok(g)
    }

    /// System profile: make sure GPU drivers stay installed (a user needs
    /// them). A new generation only if they weren't already there.
    pub fn enable_graphics(&self, index: &Index) -> Result<()> {
        let current = self.current()?;
        if current.as_ref().is_some_and(|m| m.graphics || m.runtime.iter().any(|e| e.attr == "mesa")) {
            return Ok(());
        }
        let (packages, caches, home, config) = current.as_ref().map(|m| (m.packages.clone(), m.caches.clone(), m.home.clone(), m.config.clone())).unwrap_or_default();
        let (services, etc) = current.map(|m| (m.services, m.etc)).unwrap_or_default();
        let paths: Vec<String> = packages.iter().flat_map(|p| p.paths.clone()).collect();
        let runtime = runtime::wanted(self.store, index, &paths, self.store.root(), true)?;
        let runtime_paths: Vec<String> = runtime.iter().flat_map(|p| p.paths.clone()).collect();
        self.store.realise(&runtime_paths)?;
        let generation = self.generations()?.last().copied().unwrap_or(0) + 1;
        let tree = self.build_tree(&format!("{NAME}-{generation}"), &paths)?;
        link_parts(&tree, home.as_ref(), services.as_ref(), etc.as_ref())?;
        let runtime_tree = self.build_tree(&format!("{RUNTIME}-{generation}"), &runtime_paths)?;
        runtime::populate(&runtime_tree, self.store.root(), &runtime)?;
        let manifest = Manifest { generation, created: now(), packages, runtime, caches, graphics: true, home, services, etc, config, start: false, from: self.current_generation() };
        self.write_manifest(generation, &manifest)?;
        self.switch(generation)
    }

    /// Remember which nami file the current generation came from.
    pub fn record_config(&self, snapshot: ConfigSnapshot) -> Result<()> {
        let Some(g) = self.current_generation() else { return Ok(()) };
        let mut m = self.manifest(g)?;
        if m.config.as_ref() == Some(&snapshot) {
            return Ok(());
        }
        m.config = Some(snapshot);
        self.write_manifest(g, &m)
    }

    /// The nami file this profile is managed with, if it still exists.
    pub fn managed_config(&self) -> Result<Option<PathBuf>> {
        Ok(self.current()?.and_then(|m| m.config).map(|c| PathBuf::from(c.file)).filter(|f| f.exists()))
    }

    /// Build a merged symlink tree of `paths` at `<dir>/<name>`, atomically.
    fn build_tree(&self, name: &str, paths: &[String]) -> Result<PathBuf> {
        let tree = self.dir.join(name);
        let tmp = self.dir.join(format!(".{name}.tmp"));
        for stale in [&tmp, &tree] {
            if stale.exists() {
                make_writable(stale)?;
                fs::remove_dir_all(stale)?;
            }
        }
        fs::create_dir(&tmp)?;
        for path in paths {
            let src = self.store.phys(path);
            if src.is_dir() {
                self.link_tree(&src, path, &tmp)?;
            }
        }
        fs::rename(&tmp, &tree)?;
        Ok(tree)
    }

    /// Merge `src` (on disk) into `dst`, symlinking files to `logical` paths.
    /// A folder that's a link to another (chromium's share/applications is its
    /// unwrapped package's) is linked whole while one package alone has it, and
    /// made a real folder of both once another has it too: else the second
    /// package's apps, icons and manuals would be left out.
    fn link_tree(&self, src: &Path, logical: &str, dst: &Path) -> Result<()> {
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let name = entry.file_name();
            let (from, to) = (entry.path(), dst.join(&name));
            let logical = format!("{logical}/{}", name.to_string_lossy());
            let kind = entry.file_type()?;
            let linked_dir = kind.is_symlink().then(|| self.dir_behind(&logical)).flatten();
            if kind.is_dir() || linked_dir.is_some() {
                let from = linked_dir.unwrap_or(from);
                match to.symlink_metadata() {
                    Ok(m) if m.is_dir() => {}
                    // What an earlier package linked whole: a real folder now, with its things.
                    Ok(m) if m.file_type().is_symlink() => {
                        let old = fs::read_link(&to)?.to_string_lossy().into_owned();
                        let Some(old_dir) = self.dir_behind(&old) else {
                            crate::say!("warning: {} conflicts, keeping the first", to.display());
                            continue;
                        };
                        fs::remove_file(&to)?;
                        fs::create_dir(&to)?;
                        self.link_tree(&old_dir, &old, &to)?;
                    }
                    Ok(_) => {
                        crate::say!("warning: {} conflicts, keeping the first", to.display());
                        continue;
                    }
                    Err(_) if kind.is_symlink() => {
                        symlink(&logical, &to)?;
                        continue;
                    }
                    Err(_) => fs::create_dir(&to)?,
                }
                self.link_tree(&from, &logical, &to)?;
            } else if to.symlink_metadata().is_ok() {
                crate::say!("warning: {} is provided twice, keeping the first", to.display());
            } else {
                symlink(&logical, &to)?;
            }
        }
        Ok(())
    }

    /// The folder on disk a store path leads to, its links followed (to other
    /// store paths, or next door); None if it isn't a folder.
    fn dir_behind(&self, logical: &str) -> Option<PathBuf> {
        let mut cur = logical.to_string();
        for _ in 0..16 {
            let phys = self.store.phys(&cur);
            let meta = fs::symlink_metadata(&phys).ok()?;
            if !meta.file_type().is_symlink() {
                return meta.is_dir().then_some(phys);
            }
            let target = fs::read_link(&phys).ok()?.to_string_lossy().into_owned();
            cur = if target.starts_with('/') { target } else { normalize(&format!("{}/{target}", cur.rsplit_once('/')?.0)) };
            if !cur.starts_with(crate::store::STORE_DIR) {
                return None;
            }
        }
        None
    }

    pub fn switch(&self, generation: u64) -> Result<()> {
        if !self.dir.join(format!("{NAME}-{generation}")).is_dir() {
            return Err(self.missing(generation));
        }
        let manifest = self.manifest(generation)?;
        // Flathub's apps of the step before (they're outside the store: followed below).
        let was = if self.system { Vec::new() } else { self.current().ok().flatten().map(|m| m.packages).unwrap_or_default() };
        if self.system {
            // The cache list belongs to the generation too.
            self.store.set_caches(&manifest.caches)?;
        }
        // Runtime first: a profile must never point at newer apps than its drivers.
        for name in [RUNTIME, NAME] {
            if !self.dir.join(format!("{name}-{generation}")).is_dir() {
                continue; // user profiles, and generations from before the runtime existed
            }
            let tmp = self.dir.join(format!(".{name}.link"));
            let _ = fs::remove_file(&tmp);
            symlink(format!("{name}-{generation}"), &tmp)?;
            fs::rename(&tmp, self.dir.join(name))?;
        }
        if self.system {
            runtime::ensure_driver_link(self.store.root())?;
        } else {
            self.ensure_home_link()?;
        }
        // App menu entries, refreshed so the desktop notices new apps at once.
        let stable = if self.system {
            self.dir.join(NAME)
        } else {
            std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/state/nami/profile")).unwrap_or_else(|| self.dir.join(NAME))
        };
        if let Some(dest) = crate::desktop::entries_dir(self.system, self.store.root()) {
            if let Err(e) = crate::desktop::sync(&self.dir.join(format!("{NAME}-{generation}")), &stable, &dest) {
                crate::say!("warning: couldn't update app menu entries in {}: {e:#}", dest.display());
            }
        }
        self.sync_mac_apps(&manifest);
        // Dotfiles: links in ~ point through `default`, which now is this generation.
        if let Some(home_dir) = std::env::var_os("HOME") {
            home::activate(Path::new(&home_dir), &self.dir.join(NAME).join(".home"), manifest.home.as_ref())?;
        }
        if !self.system {
            crate::flathub::follow(&was, &manifest.packages);
            if !is_root() {
                crate::bundle_app::follow(&crate::foreign::Places::here(), &was, &manifest.packages);
            }
        }
        self.activate_services_and_files(&manifest)
    }

    /// A Mac's apps with a window, in Applications (`~/Applications/nami Apps`, or
    /// everyone's in /Applications): copies of what `manifest`'s packages have in
    /// Applications/, nami's alone kept in step (mac_apps.rs). Never fails a change.
    fn sync_mac_apps(&self, manifest: &Manifest) {
        if !crate::index::MAC {
            return;
        }
        let Some(folder) = crate::mac_apps::folder(self.system, self.store.root()) else { return };
        let paths: Vec<String> = manifest.packages.iter().flat_map(|p| p.paths.clone()).collect();
        let mut apps = crate::mac_apps::of(self.store.root(), &paths);
        // Homebrew's casks' apps, said so (their copies are marked as downloaded).
        for e in manifest.packages.iter().filter(|e| crate::casks::is_cask_entry(e)) {
            for app in apps.iter_mut().filter(|a| e.paths.iter().any(|p| a.source.starts_with(&format!("{p}/")))) {
                app.cask = crate::casks::entry_token(e).map(String::from);
            }
        }
        if let Err(e) = crate::mac_apps::sync(&folder, &apps, self.store.root(), !self.system) {
            crate::say!("warning: couldn't update the apps in {}: {e:#}", folder.display());
        }
    }

    /// Where this profile's units go: the computer's, or yours.
    pub fn services_target(&self) -> Option<crate::services::Target> {
        // systemd's: a Mac has none (launchd's are for later).
        if crate::index::MAC {
            return None;
        }
        let root = self.store.root();
        if self.system {
            return Some(crate::services::Target::system(root, &self.store.var_dir()));
        }
        std::env::var_os("HOME").map(|h| crate::services::Target::user(root, Path::new(&h)))
    }

    /// Units and /etc files, from what was installed to `manifest`'s: units
    /// that go away stop first (their files are still there), then the
    /// files, then units start or restart. Copied from the store paths the
    /// generation links as `.units`/`.etc` (on disk, so under NAMI_ROOT too).
    fn activate_services_and_files(&self, manifest: &Manifest) -> Result<()> {
        let Some(target) = self.services_target() else { return Ok(()) };
        let on_disk = |p: Option<&String>| p.map(|p| self.store.phys(p)).unwrap_or_default();
        // Which /etc files change, before they do: `restart-on:` restarts for them.
        let etc_changed = if self.system { crate::etc::changes(self.store.root(), &self.store.var_dir(), manifest.etc.as_ref()) } else { Vec::new() };
        let services = crate::services::Activation::new(target, &on_disk(manifest.services.as_ref().map(|s| &s.path)), manifest.services.as_ref(), &etc_changed);
        for w in services.overrides() {
            crate::say!("{w}");
        }
        services.stop();
        let mut said = Vec::new();
        if self.system {
            said = crate::etc::activate(self.store.root(), &self.store.var_dir(), &on_disk(manifest.etc.as_ref().map(|e| &e.path)), manifest.etc.as_ref())?;
            said.extend(crate::etc::apply_now(self.store.root(), &etc_changed));
            said.extend(crate::etc::run_changed(self.store.root(), &etc_changed, manifest.etc.as_ref()));
        }
        said.extend(services.finish()?);
        for line in said {
            crate::say!("{line}");
        }
        Ok(())
    }

    /// Uninstalling: as a generation with no services or /etc files would
    /// (what's changed by hand stays; what nami's replaced comes back).
    pub fn let_go(&self) -> Result<()> {
        let none = Manifest { generation: 0, created: 0, packages: Vec::new(), runtime: Vec::new(), caches: Vec::new(), graphics: false, home: None, services: None, etc: None, config: None, start: false, from: None };
        self.activate_services_and_files(&none)
    }

    /// `~/.local/state/nami/profile` -> this user's active profile, so shells
    /// and desktop sessions can find it without knowing the user name.
    fn ensure_home_link(&self) -> Result<()> {
        let Some(home) = std::env::var_os("HOME") else { return Ok(()) };
        let state = Path::new(&home).join(".local/state/nami");
        fs::create_dir_all(&state)?;
        let link = state.join("profile");
        let target = self.dir.join(NAME);
        if fs::read_link(&link).ok().as_deref() != Some(target.as_path()) {
            let tmp = state.join(".profile.link");
            let _ = fs::remove_file(&tmp);
            symlink(&target, &tmp)?;
            fs::rename(&tmp, &link)?;
        }
        Ok(())
    }

    /// Delete inactive generations according to `policy`. Returns which.
    /// The steps `policy` would delete (never the current one).
    pub fn pick(&self, policy: &Prune) -> Result<Vec<u64>> {
        let stay: Vec<u64> = self.current_generation().into_iter().collect();
        Ok(pick_by(&self.steps()?, &stay, policy, now()))
    }

    pub fn prune(&self, policy: Prune) -> Result<Vec<u64>> {
        self.remove(&self.pick(&policy)?)
    }

    /// Each step, with when it was made: for `cleanup::pick`.
    pub fn steps(&self) -> Result<Vec<(u64, u64)>> {
        self.generations()?.into_iter().map(|g| Ok((g, self.manifest(g)?.created))).collect()
    }

    /// Clean up by `policy` (`cleanup:`): old steps go, never the current one nor the newest
    /// `keep`; all but those when space is `low`. Which went.
    pub fn clean_up(&self, policy: &crate::cleanup::Policy, now: u64, low: bool) -> Result<Vec<u64>> {
        let stay: Vec<u64> = self.current_generation().into_iter().collect();
        self.remove(&crate::cleanup::pick(&self.steps()?, &stay, policy, now, low))
    }

    /// Delete steps `gens` (never the current one). Which went.
    pub fn remove(&self, gens: &[u64]) -> Result<Vec<u64>> {
        let current = self.current_generation();
        let mut removed = Vec::new();
        for &g in gens.iter().filter(|g| Some(**g) != current) {
            for name in [NAME, RUNTIME] {
                let tree = self.dir.join(format!("{name}-{g}"));
                if tree.exists() {
                    make_writable(&tree)?;
                    fs::remove_dir_all(&tree)?;
                }
            }
            fs::remove_file(self.dir.join(format!("{NAME}-{g}.json")))?;
            let _ = fs::remove_file(self.snapshot_file(g));
            removed.push(g);
        }
        Ok(removed)
    }

    /// Every store path some generation of this profile uses.
    /// Keep a project's packages safe from gc while its directory exists.
    pub fn register_project(&self, dir: &Path, paths: &[String]) -> Result<()> {
        let projects = self.dir.join("projects");
        fs::create_dir_all(&projects)?;
        let record = serde_json::json!({ "dir": dir, "paths": paths });
        write_atomic(&projects.join(format!("{}.json", crate::project::key(dir))), &serde_json::to_vec_pretty(&record)?)
    }

    /// A new project, listed (`nami project list`, the window's Projects)
    /// before its first `nami shell`; one listed already keeps its record.
    pub fn note_project(&self, dir: &Path) -> Result<()> {
        let record = self.dir.join("projects").join(format!("{}.json", crate::project::key(dir)));
        if record.is_file() { Ok(()) } else { self.register_project(dir, &[]) }
    }

    /// Directories of this profile's projects that still exist.
    pub fn projects(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = fs::read_dir(self.dir.join("projects"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|f| fs::read(f.path()).ok())
            .filter_map(|d| serde_json::from_slice::<serde_json::Value>(&d).ok())
            .filter_map(|v| v["dir"].as_str().map(PathBuf::from))
            .filter(|d| crate::project::file_in(d).is_some())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Keep `paths` while this process runs (`nami try`): a root that
    /// goes when it's let go, or by itself when the process is gone. After
    /// that they're nobody's, and the next `nami gc` deletes them.
    pub fn hold(&self, paths: &[String]) -> Result<Held> {
        let dir = self.dir.join("running");
        fs::create_dir_all(&dir)?;
        let pid = std::process::id();
        let file = dir.join(format!("{pid}.json"));
        let record = serde_json::json!({ "pid": pid, "started": process_start(pid), "paths": paths });
        write_atomic(&file, &serde_json::to_vec(&record)?)?;
        Ok(Held(file))
    }

    /// What running programs hold (stale records are dropped).
    pub(crate) fn running_roots(&self) -> Vec<String> {
        let mut roots = Vec::new();
        for f in fs::read_dir(self.dir.join("running")).into_iter().flatten().flatten() {
            let Ok(v) = fs::read(f.path()).map_err(anyhow::Error::from).and_then(|d| Ok(serde_json::from_slice::<serde_json::Value>(&d)?)) else { continue };
            let alive = v["pid"].as_u64().and_then(|p| u32::try_from(p).ok()).is_some_and(|p| process_start(p).is_some() && process_start(p).as_deref() == v["started"].as_str());
            if alive {
                roots.extend(v["paths"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(String::from)));
            } else {
                drop_stale(&f.path());
            }
        }
        roots
    }

    /// Packages of projects that still exist (stale records are dropped).
    pub(crate) fn project_roots(&self) -> Vec<String> {
        let mut roots = Vec::new();
        for f in fs::read_dir(self.dir.join("projects")).into_iter().flatten().flatten() {
            let Ok(v) = fs::read(f.path()).map_err(anyhow::Error::from).and_then(|d| Ok(serde_json::from_slice::<serde_json::Value>(&d)?)) else { continue };
            let dir = v["dir"].as_str().map(PathBuf::from).unwrap_or_default();
            if crate::project::file_in(&dir).is_some() {
                roots.extend(v["paths"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(String::from)));
            } else {
                drop_stale(&f.path());
            }
        }
        roots
    }

    pub fn roots(&self) -> Result<Vec<String>> {
        self.roots_without(&[])
    }

    /// The same, as if steps `skip` were gone.
    pub fn roots_without(&self, skip: &[u64]) -> Result<Vec<String>> {
        let mut roots = self.project_roots();
        roots.extend(self.running_roots());
        for g in self.generations()?.into_iter().filter(|g| !skip.contains(g)) {
            roots.extend(step_roots(&self.manifest(g)?));
        }
        Ok(roots)
    }

    /// What changed from generation `from` to generation `to`.
    pub fn diff(&self, from: u64, to: u64) -> Result<Diff> {
        let (a, b) = (self.manifest(from)?, self.manifest(to)?);
        let mut d = Diff::default();
        let old: BTreeMap<&str, &Entry> = a.packages.iter().map(|e| (e.attr.as_str(), e)).collect();
        let new: BTreeMap<&str, &Entry> = b.packages.iter().map(|e| (e.attr.as_str(), e)).collect();
        for (attr, e) in &new {
            match old.get(attr) {
                None => d.added.push(e.name.clone()),
                // (Flathub's apps have no store paths: their version is in their name.)
                Some(o) if o.paths != e.paths || e.paths.is_empty() && o.name != e.name => d.changed.push((attr.to_string(), o.name.clone(), e.name.clone())),
                Some(_) => {}
            }
        }
        d.removed = old.iter().filter(|(a, _)| !new.contains_key(*a)).map(|(_, e)| e.name.clone()).collect();
        let names = |v: &[Entry]| v.iter().map(|e| e.name.clone()).collect::<BTreeSet<_>>();
        let (ra, rb) = (names(&a.runtime), names(&b.runtime));
        d.runtime_added = rb.difference(&ra).cloned().collect();
        d.runtime_removed = ra.difference(&rb).cloned().collect();
        let urls = |v: &[Cache]| v.iter().map(|c| c.url.clone()).collect::<BTreeSet<_>>();
        let (ca, cb) = (urls(&a.caches), urls(&b.caches));
        d.caches_added = cb.difference(&ca).cloned().collect();
        d.caches_removed = ca.difference(&cb).cloned().collect();

        let home_files = |m: &Manifest| m.home.as_ref().map(|h| h.files.iter().cloned().collect::<BTreeSet<_>>()).unwrap_or_default();
        let (ha, hb) = (home_files(&a), home_files(&b));
        d.home_added = hb.difference(&ha).cloned().collect();
        d.home_removed = ha.difference(&hb).cloned().collect();
        if let (Some(x), Some(y)) = (&a.home, &b.home) {
            for f in ha.intersection(&hb) {
                let hash = |root: &str| crate::nar::hash_path(&self.store.phys(&format!("{root}/{f}"))).ok();
                if x.path != y.path && hash(&x.path) != hash(&y.path) {
                    d.home_changed.push(f.clone());
                }
            }
        }

        let units = |m: &Manifest| m.services.as_ref().map(|s| s.units.iter().map(|u| (u.name.clone(), u.hash.clone())).collect::<BTreeMap<_, _>>()).unwrap_or_default();
        let (ua, ub) = (units(&a), units(&b));
        d.services_added = ub.keys().filter(|u| !ua.contains_key(*u)).cloned().collect();
        d.services_removed = ua.keys().filter(|u| !ub.contains_key(*u)).cloned().collect();
        d.services_changed = ub.iter().filter(|(u, h)| ua.get(*u).is_some_and(|o| o != *h)).map(|(u, _)| u.clone()).collect();
        let files = |m: &Manifest| m.etc.as_ref().map(|e| e.files.iter().map(|f| (f.path.clone(), (f.hash.clone(), f.mode))).collect::<BTreeMap<_, _>>()).unwrap_or_default();
        let (fa, fb) = (files(&a), files(&b));
        d.files_added = fb.keys().filter(|f| !fa.contains_key(*f)).cloned().collect();
        d.files_removed = fa.keys().filter(|f| !fb.contains_key(*f)).cloned().collect();
        d.files_changed = fb.iter().filter(|(f, h)| fa.get(*f).is_some_and(|o| o != *h)).map(|(f, _)| f.clone()).collect();

        let (ca, cb) = (self.store.closure(&step_roots(&a))?, self.store.closure(&step_roots(&b))?);
        let size = |set: BTreeSet<&String>| -> u64 {
            set.into_iter().filter_map(|p| self.store.path_info(p).ok()).map(|i| i.nar_size).sum()
        };
        d.size_added = size(cb.difference(&ca).collect());
        d.size_removed = size(ca.difference(&cb).collect());
        Ok(d)
    }
}

/// Reading only (`nami storage`, `nami gc --dry-run`): stale records of projects and running
/// programs are left for the next cleanup, not dropped as they're read.
static READING_ONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn reading_only() {
    READING_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// A record of a project that's gone, or a program that's ended: dropped, unless only reading.
fn drop_stale(record: &Path) {
    if !READING_ONLY.load(std::sync::atomic::Ordering::Relaxed) {
        let _ = fs::remove_file(record);
    }
}

/// What a step uses: its apps' and support packages' paths, and its rendered home files,
/// units and /etc files.
pub fn step_roots(m: &Manifest) -> Vec<String> {
    let mut r: Vec<String> = m.packages.iter().chain(&m.runtime).flat_map(|e| e.paths.clone()).collect();
    r.extend(m.home.as_ref().map(|h| h.path.clone()));
    r.extend(m.services.as_ref().map(|s| s.path.clone()));
    r.extend(m.etc.as_ref().map(|e| e.path.clone()));
    r
}

/// A generation's rendered parts, linked from its tree: `.home`, `.units`, `.etc`.
fn link_parts(tree: &Path, home: Option<&HomeState>, services: Option<&ServicesState>, etc: Option<&EtcState>) -> Result<()> {
    for (name, path) in [(".home", home.map(|h| &h.path)), (".units", services.map(|s| &s.path)), (".etc", etc.map(|e| &e.path))] {
        if let Some(p) = path {
            symlink(p, tree.join(name))?;
        }
    }
    Ok(())
}

/// A root while a program runs from it (`Profiles::hold`).
pub struct Held(PathBuf);

impl Drop for Held {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// When a process started (its /proc stat's 22nd field), so a pid used again isn't taken for it.
fn process_start(pid: u32) -> Option<String> {
    crate::sys::process_start(pid)
}

/// The steps (number, when made) `policy` takes (`nami gc --old/--keep/--older-than`): never
/// those in `stay`; a step is kept if any rule keeps it. One whose time isn't known (0) is
/// kept by an age.
pub fn pick_by(steps: &[(u64, u64)], stay: &[u64], policy: &Prune, now: u64) -> Vec<u64> {
    let mut steps = steps.to_vec();
    steps.sort_unstable();
    let cutoff = policy.newer_than.map(|s| now.saturating_sub(s));
    let n = steps.len();
    steps
        .iter()
        .enumerate()
        .filter(|(_, (g, _))| !stay.contains(g))
        .filter(|(i, (_, created))| {
            let kept_by_count = policy.keep.is_some_and(|k| n - i <= k);
            let kept_by_age = cutoff.is_some_and(|c| *created >= c || *created == 0);
            policy.all || ((policy.keep.is_some() || cutoff.is_some()) && !kept_by_count && !kept_by_age)
        })
        .map(|(_, (g, _))| *g)
        .collect()
}

/// What's said of change `g` that isn't there: if there are newer ones, it was cleaned up
/// (`cleanup:`, `nami gc`), else it was never made.
pub fn missing_change(g: u64, newest: Option<u64>) -> anyhow::Error {
    match newest {
        Some(n) if g >= 1 && g < n => anyhow!("change {g} was cleaned up: old changes are removed after a while (`cleanup:` in your nami file says when). `nami history` lists the ones kept"),
        _ => anyhow!("change {g} doesn't exist (`nami history` lists them)"),
    }
}

/// GC roots across the system profile and every user's, and what running programs use.
pub fn all_roots(store: &Store) -> Result<Vec<String>> {
    all_roots_without(store, &[], &[])
}

/// The same, as if each profile's steps in `skip` were gone, and namiOS's systems `systems`
/// (`nami gc --dry-run`, `nami storage`).
pub fn all_roots_without(store: &Store, skip: &[(&Profiles, &[u64])], systems: &[u64]) -> Result<Vec<String>> {
    let of = |p: &Profiles| match skip.iter().find(|(who, _)| who.dir == p.dir) {
        Some((_, steps)) => p.roots_without(steps),
        None => p.roots(),
    };
    let mut roots = of(&Profiles::system(store))?;
    // A computer `nami usb` made: its systems (never without them, on namiOS: they're
    // what it runs).
    match crate::nami_os::plugin() {
        Some(os) => roots.extend(os.roots(store.root(), systems)),
        None if crate::nami_os::has_systems(store.root()) => anyhow::bail!("{}", crate::nami_os::BUILT_WITHOUT),
        None => {}
    }
    // What running programs use, though no step has it any more (an app still open after
    // it was removed, a `nami shell` of a project that's gone): never taken from under them.
    roots.extend(crate::cleanup::running_paths());
    if let Ok(users) = fs::read_dir(store.var_dir().join("profiles/per-user")) {
        for u in users.flatten() {
            if let Some(name) = u.file_name().to_str() {
                roots.extend(of(&Profiles::user(store, name))?);
            }
        }
    }
    Ok(roots)
}

/// Your login name from /etc/passwd, for when USER and LOGNAME aren't set
/// (`su -c`, `podman exec -u`, some service managers). Without libc, like
/// is_root: your uid is the owner of /proc/self.
fn passwd_name() -> Option<String> {
    if crate::index::MAC {
        return crate::sys::account_by_uid(crate::sys::euid()).map(|a| a.name);
    }
    use std::os::unix::fs::MetadataExt;
    let uid = fs::metadata("/proc/self").ok()?.uid().to_string();
    fs::read_to_string("/etc/passwd").ok()?.lines().find_map(|l| {
        let mut f = l.split(':');
        let name = f.next()?;
        (f.nth(1)? == uid).then(|| name.to_string())
    })
}

/// Groups whose members administer the computer, as distros name them.
const ADMIN_GROUPS: &[&str] = &["wheel", "sudo", "admin"];

/// Is `user` (primary group `gid`) an admin: in wheel, sudo or admin?
/// Admins change the system's apps (the system profile) through the
/// daemon without sudo, like Homebrew's admins; they could with sudo anyway.
pub fn is_admin_user(user: &str, gid: u32) -> bool {
    // A Mac's admins change the computer's apps with sudo (there's no polkit to ask them).
    if crate::index::MAC {
        let _ = (user, gid);
        return false;
    }
    let groups = fs::read_to_string("/etc/group").unwrap_or_default();
    groups.lines().any(|l| {
        let f: Vec<&str> = l.split(':').collect();
        f.len() >= 4 && ADMIN_GROUPS.contains(&f[0]) && (f[3].split(',').any(|m| m == user) || f[2].parse::<u32>().ok() == Some(gid))
    })
}

/// Whether whoever runs nami (not root) is an admin.
pub fn is_admin() -> bool {
    if is_root() {
        return false;
    }
    let Some((name, gid)) = whoami() else { return false };
    is_admin_user(&name, gid)
}

/// Whether whoever runs nami (not root) is a child on this computer (in
/// the children group, os.users.<them>.child): their apps are the
/// computer's, which an admin chooses.
pub fn is_child() -> bool {
    if is_root() {
        return false;
    }
    let Some((name, _)) = whoami() else { return false };
    let groups = fs::read_to_string("/etc/group").unwrap_or_default();
    groups.lines().any(|l| {
        let f: Vec<&str> = l.split(':').collect();
        f.len() >= 4 && f[0] == crate::config::computer::CHILDREN && f[3].split(',').any(|m| m == name)
    })
}

/// The admin group that exists here (it owns /etc/nami), with its gid.
pub fn admin_group() -> Option<u32> {
    let groups = fs::read_to_string("/etc/group").ok()?;
    ADMIN_GROUPS.iter().find_map(|g| groups.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 3 && f[0] == *g).then(|| f[2].parse().ok()).flatten()
    }))
}

/// Whether a file of the computer's config is system.nami itself
/// (system.nami, a copy of it set aside, the whole computer in one file):
/// it has the people's passwords in it, and secrets, so only root and admins
/// read it, and only root writes it (an admin through the daemon, with
/// their password).
pub fn is_system_file(name: &str) -> bool {
    // (By its names before too: computer.nami until 0.19, tray's computer.tray.)
    let rest = ["system.nami", "computer.nami", "computer.tray"].iter().find_map(|n| name.strip_prefix(n));
    rest.is_some_and(|r| r.is_empty() || r.starts_with('.')) || name == "computer.flat.nami" || name == "computer.flat.tray"
}

/// Make `p` root's and the admins' to read, nobody else's (as root; elsewhere
/// only its mode).
pub fn for_admins(p: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if is_root() {
        let gid = admin_group().unwrap_or(0);
        let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes())?;
        // SAFETY: a valid C path; root may chown.
        if unsafe { libc::chown(c.as_ptr(), 0, gid) } != 0 {
            bail!("can't give {} to the admins: {}", p.display(), std::io::Error::last_os_error());
        }
    }
    fs::set_permissions(p, fs::Permissions::from_mode(0o640))?;
    Ok(())
}

/// Make `p`, a file of a person's folder in the computer's config
/// (<login>/user.nami and what's with it), theirs and the admins' to read
/// and change, nobody else's: a person's file is theirs, and the admins
/// manage the people (and could read it as root anyway). `uid`: theirs,
/// when they have an account here (else root's). As root; elsewhere nothing.
/// Its programs stay programs (for them and the admins).
pub fn for_person(p: &Path, uid: Option<u32>) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if !is_root() {
        return Ok(());
    }
    let meta = fs::symlink_metadata(p)?;
    if !meta.is_file() {
        return Ok(());
    }
    let gid = admin_group().unwrap_or(0);
    std::os::unix::fs::lchown(p, Some(uid.unwrap_or(0)), Some(gid)).map_err(|e| anyhow!("can't give {} to its person: {e}", p.display()))?;
    fs::set_permissions(p, fs::Permissions::from_mode(0o660 | (meta.mode() & 0o110)))?;
    Ok(())
}

/// The people with an account here (uid 1000 up, below 60000, as
/// distros give them): login, uid, primary gid and home.
pub fn people() -> Vec<(String, u32, u32, PathBuf)> {
    // (A Mac's aren't in /etc/passwd; nami doesn't look after them there.)
    if crate::index::MAC {
        return Vec::new();
    }
    fs::read_to_string("/etc/passwd").unwrap_or_default().lines().filter_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        let (uid, gid) = (f.get(2)?.parse::<u32>().ok()?, f.get(3)?.parse::<u32>().ok()?);
        ((1000..60000).contains(&uid) && !f[0].is_empty()).then(|| (f[0].to_string(), uid, gid, PathBuf::from(f.get(5).copied().unwrap_or_default())))
    }).collect()
}

/// `login`'s uid, when they have an account here.
pub fn uid_of(login: &str) -> Option<u32> {
    if crate::index::MAC {
        return crate::sys::account_by_name(login).map(|a| a.uid);
    }
    fs::read_to_string("/etc/passwd").ok()?.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 3 && f[0] == login).then(|| f[2].parse().ok()).flatten()
    })
}

/// Your login name (from /etc/passwd by your uid, else $USER).
pub fn login() -> Option<String> {
    whoami().map(|(n, _)| n).or_else(|| std::env::var("USER").ok()).filter(|n| !n.is_empty())
}

/// Your login name and primary gid, from /etc/passwd by your uid.
fn whoami() -> Option<(String, u32)> {
    if crate::index::MAC {
        return user_by_uid(crate::sys::euid());
    }
    use std::os::unix::fs::MetadataExt;
    let uid = fs::metadata("/proc/self").ok()?.uid();
    user_by_uid(uid)
}

/// A user's name and primary gid, from /etc/passwd.
pub fn user_by_uid(uid: u32) -> Option<(String, u32)> {
    if crate::index::MAC {
        return crate::sys::account_by_uid(uid).map(|a| (a.name, a.gid));
    }
    fs::read_to_string("/etc/passwd").ok()?.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 4 && f[2].parse::<u32>().ok() == Some(uid)).then(|| Some((f[0].to_string(), f[3].parse().ok()?))).flatten()
    })
}

pub fn is_root() -> bool {
    // (A Mac has no /proc.)
    if crate::index::MAC {
        return crate::sys::euid() == 0;
    }
    // Without libc: root owns /proc/self, as it owns every process it runs.
    use std::os::unix::fs::MetadataExt;
    fs::metadata("/proc/self").map(|m| m.uid() == 0).unwrap_or(false)
}

/// `/a/b/../c` -> `/a/c`.
fn normalize(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    format!("/{}", out.join("/"))
}

fn parse_generation(name: &str) -> Option<u64> {
    name.strip_prefix(NAME)?.strip_prefix('-')?.parse().ok()
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `YYYY-MM-DD HH:MM` (UTC) for a Unix time, without a date library.
pub fn format_time(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let (h, m) = ((secs % 86400) / 3600, (secs % 3600) / 60);
    // Howard Hinnant's civil_from_days.
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02} {h:02}:{m:02}")
}

/// `30d`, `12h`, `2w`, `90m` or plain seconds (see duration.rs).
pub fn parse_duration(s: &str) -> Result<u64> {
    crate::duration::seconds(s).map_err(|e| anyhow!(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A computer with namiOS's systems: the store's cleanup keeps what they use, or, with
    /// a nami built without namiOS's part (that can't tell what they use), doesn't clean.
    #[test]
    fn the_store_keeps_namios_systems_or_isnt_cleaned() {
        let root = std::env::temp_dir().join(format!("nami-roots-systems-{}", std::process::id()));
        let path = "/nix/store/00000000000000000000000000000001-bash-5.3";
        std::fs::create_dir_all(root.join("nix/var/nami/os/system-1")).unwrap();
        std::fs::write(root.join("nix/var/nami/os/system-1/packages"), format!("{path}\n")).unwrap();
        let store = Store::open_direct(&root, false).unwrap();
        let roots = all_roots(&store);
        std::fs::remove_dir_all(&root).unwrap();
        match crate::nami_os::plugin() {
            Some(_) => assert!(roots.unwrap().iter().any(|r| r == path)),
            None => assert!(roots.unwrap_err().to_string().contains("without namiOS support")),
        }
    }

    #[test]
    fn the_system_file_isnt_in_the_steps_everyone_reads() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("nami-profile-snapshot-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = Store::open_direct(&root, false).unwrap();
        let snap = ConfigSnapshot { file: "/etc/nami/system.nami".into(), text: "users:\n  ann:\n    password: secret-hash\n".into(), lock: None };
        let m = |config: Option<ConfigSnapshot>| Manifest { generation: 1, created: 0, packages: Vec::new(), runtime: Vec::new(), caches: Vec::new(), graphics: false, home: None, services: None, etc: None, config, start: false, from: None };
        // The computer's: beside the step, for root and the admins alone.
        let system = Profiles::system(&store);
        fs::create_dir_all(&system.dir).unwrap();
        system.write_manifest(1, &m(Some(snap.clone()))).unwrap();
        let open = fs::read_to_string(system.dir.join("default-1.json")).unwrap();
        assert!(!open.contains("secret-hash") && !open.contains("\"config\""), "{open}");
        let beside = system.dir.join("default-1.config.json");
        assert_eq!(fs::metadata(&beside).unwrap().permissions().mode() & 0o777, 0o640);
        assert_eq!(system.manifest(1).unwrap().config, Some(snap.clone()));
        // One written before: moved out (as root alone; here, as it would be).
        fs::write(system.dir.join("default-2.json"), serde_json::to_vec(&m(Some(snap.clone()))).unwrap()).unwrap();
        assert!(fs::read_to_string(system.dir.join("default-2.json")).unwrap().contains("secret-hash"));
        system.write_manifest(2, &system.manifest(2).unwrap()).unwrap();
        assert!(!fs::read_to_string(system.dir.join("default-2.json")).unwrap().contains("secret-hash"));
        // Unreadable (not an admin): the step is there, without it.
        fs::remove_file(&beside).unwrap();
        assert_eq!(system.manifest(1).unwrap().config, None);
        // A person's own: beside the step too, theirs alone.
        let mine = Profiles::user(&store, "ann");
        fs::create_dir_all(&mine.dir).unwrap();
        let theirs = ConfigSnapshot { file: "/home/ann/user.nami".into(), text: "env:\n  DIARY: private\n".into(), lock: None };
        mine.write_manifest(1, &m(Some(theirs.clone()))).unwrap();
        assert!(!fs::read_to_string(mine.dir.join("default-1.json")).unwrap().contains("DIARY"));
        assert_eq!(fs::metadata(mine.dir.join("default-1.config.json")).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(mine.manifest(1).unwrap().config, Some(theirs.clone()));
        // One of theirs written before: moved out as they use nami (not as root).
        fs::write(mine.dir.join("default-2.json"), serde_json::to_vec(&m(Some(theirs.clone()))).unwrap()).unwrap();
        if !is_root() {
            mine.seal_snapshots().unwrap();
            assert!(!fs::read_to_string(mine.dir.join("default-2.json")).unwrap().contains("DIARY"));
            assert_eq!(mine.manifest(2).unwrap().config, Some(theirs));
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// Install, undo, install again, undo: back to before the second install, not forward to
    /// what was undone; where a change doesn't say what it was made from, the one before it.
    #[test]
    fn undo_goes_back_to_what_a_change_was_made_from() {
        let root = std::env::temp_dir().join(format!("nami-profile-undo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = Store::open_direct(&root, false).unwrap();
        let mine = Profiles::user(&store, "ada");
        fs::create_dir_all(&mine.dir).unwrap();
        let step = |g: u64, from: Option<u64>| {
            let m = Manifest { generation: g, created: g, packages: Vec::new(), runtime: Vec::new(), caches: Vec::new(), graphics: false, home: None, services: None, etc: None, config: None, start: false, from };
            mine.write_manifest(g, &m).unwrap();
            fs::create_dir_all(mine.dir.join(format!("{NAME}-{g}"))).unwrap();
        };
        // 1 firefox, 2 + vlc, 3 + gimp; undo (to 2); 4 + inkscape, made from 2.
        step(1, None);
        step(2, Some(1));
        step(3, Some(2));
        step(4, Some(2));
        // Undoing 4 goes back to 2 (before inkscape), not forward to 3 (gimp, undone).
        assert_eq!(crate::ops::undo_target(&mine, 4).unwrap(), Some(2));
        assert_eq!(crate::ops::undo_target(&mine, 2).unwrap(), Some(1));
        // Made before nami kept it: the one numbered before.
        step(5, None);
        assert_eq!(crate::ops::undo_target(&mine, 5).unwrap(), Some(4));
        // The one it was made from cleaned up: the one numbered before, too.
        fs::remove_file(mine.dir.join(format!("{NAME}-2.json"))).unwrap();
        let _ = fs::remove_dir_all(mine.dir.join(format!("{NAME}-2")));
        assert_eq!(mine.generations().unwrap(), [1, 3, 4, 5]);
        assert_eq!(crate::ops::undo_target(&mine, 4).unwrap(), Some(3));
        assert_eq!(crate::ops::undo_target(&mine, 1).unwrap(), None);
        let _ = fs::remove_dir_all(&root);
    }

    /// Install, remove, and 40 days on: the cleanup takes the old change and what only it
    /// used, and keeps the current change, the newest, a project's pinned tools, and what a
    /// running program uses; undo past it says it was cleaned up.
    #[test]
    fn old_changes_and_what_only_they_used_go() {
        let root = std::env::temp_dir().join(format!("nami-profile-cleanup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = Store::open_direct(&root, false).unwrap();
        // Store paths: a library two apps share, the apps, a project's tool, and a program
        // that's running (this test's own, when it runs from the store).
        let path = |name: &str| format!("/nix/store/{}{}-{name}", "0".repeat(31), &name[..1]);
        let lib = path("libshared-1.0");
        let fill = |p: &str, bytes: usize, refs: Vec<String>| {
            fs::create_dir_all(store.phys(p)).unwrap();
            fs::write(store.phys(p).join("data"), vec![7u8; bytes]).unwrap();
            store.register(p, refs).unwrap();
        };
        fill(&lib, 1000, Vec::new());
        let (editor, browser, tool) = (path("editor-2.0"), path("browser-130"), path("projecttool-3"));
        fill(&editor, 2000, vec![crate::store::base_name(&lib).to_string()]);
        fill(&browser, 450_000, vec![crate::store::base_name(&lib).to_string()]);
        fill(&tool, 3000, Vec::new());
        let mine = Profiles::user(&store, "ada");
        fs::create_dir_all(&mine.dir).unwrap();
        let day = 86400;
        let made = 1_800_000_000;
        let entry = |attr: &str, p: &str| Entry { attr: attr.into(), name: attr.into(), paths: vec![p.to_string()] };
        let step = |g: u64, created: u64, packages: Vec<Entry>| {
            let m = Manifest { generation: g, created, packages, runtime: Vec::new(), caches: Vec::new(), graphics: false, home: None, services: None, etc: None, config: None, start: false, from: None };
            mine.write_manifest(g, &m).unwrap();
            fs::create_dir_all(mine.dir.join(format!("default-{g}"))).unwrap();
        };
        // 1: the editor; 2: and the browser; 3: the browser removed; 4 and 5: more changes.
        step(1, made, vec![entry("editor", &editor)]);
        step(2, made + day, vec![entry("editor", &editor), entry("browser", &browser)]);
        step(3, made + 2 * day, vec![entry("editor", &editor)]);
        step(4, made + 3 * day, vec![entry("editor", &editor)]);
        step(5, made + 4 * day, vec![entry("editor", &editor)]);
        symlink("default-5", mine.dir.join(NAME)).unwrap();
        // A project, its tools pinned (nami.lock), while its folder has its file.
        let project = root.join("home/ada/app");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join(crate::project::FILE), "packages: [projecttool]\n").unwrap();
        mine.register_project(&project, std::slice::from_ref(&tool)).unwrap();
        // What removing the browser left with change 2, from the closures' difference.
        let d = mine.diff(2, 3).unwrap();
        assert!(d.size_removed >= 450_000 && d.size_removed < 500_000, "{}", d.size_removed);
        // A day on: nothing's old enough; space low: all but the newest three go.
        let policy = crate::cleanup::Policy::default();
        assert!(crate::cleanup::pick(&mine.steps().unwrap(), &[5], &policy, made + 5 * day, false).is_empty());
        assert_eq!(crate::cleanup::pick(&mine.steps().unwrap(), &[5], &policy, made + 5 * day, true), [1, 2]);
        // Forty days on: 1 and 2 go; the current one and the newest three stay.
        let gone = mine.clean_up(&policy, made + 40 * day, false).unwrap();
        assert_eq!(gone, [1, 2]);
        assert_eq!(mine.generations().unwrap(), [3, 4, 5]);
        assert!(!mine.dir.join("default-2").exists());
        // Then the store: the browser goes (only change 2 had it); the editor, the library
        // and the project's tool stay.
        let (paths, bytes) = store.gc(&all_roots(&store).unwrap()).unwrap();
        assert_eq!(paths, 1);
        assert!(bytes >= 450_000, "{bytes}");
        assert!(!store.phys(&browser).exists() && !store.is_valid(crate::store::base_name(&browser)));
        for kept in [&editor, &lib, &tool] {
            assert!(store.phys(kept).exists() && store.is_valid(crate::store::base_name(kept)), "{kept}");
        }
        // The project gone: its tool goes too.
        fs::remove_dir_all(&project).unwrap();
        assert_eq!(store.gc(&all_roots(&store).unwrap()).unwrap().0, 1);
        assert!(!store.phys(&tool).exists());
        // Undo to a change that was cleaned up says so.
        let e = mine.switch(2).unwrap_err().to_string();
        assert!(e.contains("change 2 was cleaned up"), "{e}");
        assert!(mine.manifest(9).unwrap_err().to_string().contains("doesn't exist"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn steps_by_rules_and_none_whose_time_isnt_known_by_age() {
        let now = 1_800_000_000;
        let steps = [(1, 0), (2, now - 50 * 86400), (3, now - 40 * 86400), (4, now - 86400), (5, now)];
        let older_than = |d: u64| Prune { newer_than: Some(d * 86400), ..Default::default() };
        assert_eq!(pick_by(&steps, &[5], &older_than(30), now), [2, 3]);
        assert_eq!(pick_by(&steps, &[5], &Prune { keep: Some(2), ..Default::default() }, now), [1, 2, 3]);
        assert_eq!(pick_by(&steps, &[5, 4], &Prune { all: true, ..Default::default() }, now), [1, 2, 3]);
        assert!(pick_by(&steps, &[5], &Prune::default(), now).is_empty());
        assert_eq!(missing_change(2, Some(5)).to_string().split(':').next(), Some("change 2 was cleaned up"));
        assert!(missing_change(7, Some(5)).to_string().contains("doesn't exist"));
    }

    #[test]
    fn system_files_by_name() {
        for yes in ["system.nami", "system.nami.before", "computer.nami", "computer.nami.before", "computer.tray", "computer.flat.nami"] {
            assert!(is_system_file(yes), "{yes}");
        }
        for no in ["user.nami", "nami.lock", "package.nami", "system.namis", "computer.namis", ".nami-recipients"] {
            assert!(!is_system_file(no), "{no}");
        }
    }

    #[test]
    fn formats_dates() {
        assert_eq!(format_time(0), "1970-01-01 00:00");
        assert_eq!(format_time(1791331200), "2026-10-07 00:00");
        assert_eq!(format_time(951782400 + 3661), "2000-02-29 01:01");
    }

    #[test]
    fn merges_folders_that_are_links() {
        let root = std::env::temp_dir().join(format!("nami-profile-merge-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = Store::open_direct_readonly(&root).unwrap();
        let s = |p: &str| root.join("nix/store").join(p);
        // chromium's share/applications is its unwrapped package's, by a link; gimp's is its own.
        for d in ["u-chromium/share/applications", "a-chromium/share", "g-gimp/share/applications", "v-vlc/share/applications"] {
            fs::create_dir_all(s(d)).unwrap();
        }
        fs::write(s("u-chromium/share/applications/chromium.desktop"), "").unwrap();
        symlink("/nix/store/u-chromium/share/applications", s("a-chromium/share/applications")).unwrap();
        fs::write(s("g-gimp/share/applications/gimp.desktop"), "").unwrap();
        fs::write(s("v-vlc/share/applications/vlc.desktop"), "").unwrap();
        let profiles = Profiles::user(&store, "someone");
        fs::create_dir_all(&profiles.dir).unwrap();
        // Alone, it's linked whole.
        let tree = profiles.build_tree("one", &["/nix/store/a-chromium".into()]).unwrap();
        assert!(tree.join("share/applications").symlink_metadata().unwrap().file_type().is_symlink());
        // With others, either way round: every one's apps.
        for paths in [["/nix/store/a-chromium", "/nix/store/g-gimp", "/nix/store/v-vlc"], ["/nix/store/g-gimp", "/nix/store/a-chromium", "/nix/store/v-vlc"]] {
            let tree = profiles.build_tree("all", &paths.map(String::from)).unwrap();
            let mut apps: Vec<String> = fs::read_dir(tree.join("share/applications")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            apps.sort();
            assert_eq!(apps, ["chromium.desktop", "gimp.desktop", "vlc.desktop"]);
            assert_eq!(fs::read_link(tree.join("share/applications/chromium.desktop")).unwrap(), Path::new("/nix/store/a-chromium/share/applications/chromium.desktop"));
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("30d").unwrap(), 30 * 86400);
        assert_eq!(parse_duration("2w").unwrap(), 14 * 86400);
        assert_eq!(parse_duration("90").unwrap(), 90);
        assert!(parse_duration("3x").is_err());
    }
}
