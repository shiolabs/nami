//! `nami storage`: how full the disk with nami's store is, what nami takes of it and for
//! what, the apps that take the most, and what cleaning up now frees. The nami app's Storage
//! page shows it (gui/src/pages/storage.rs).
//!
//! Sizes come from the store's own records (`/nami/db`: each path's NAR size, as `nix
//! path-info -S` adds them up), not from walking the files: a store of thousands of paths is
//! read in a moment. They're what the files hold, not the blocks they take (a file system
//! that compresses, or that rounds each file up to a block, makes those a little different).
//! What has no record is walked: a Mac's casks' apps (kept beside the store, casks.rs) and
//! nami's own folder (the catalog, icons, logs), by their files' sizes.
//!
//! Each path counts once, in the first part that uses it, in [`Part`]'s order: what your apps
//! use is yours even when the computer's apps use it too, and "Older changes" is only what
//! nothing in use needs. So the parts add up to nami's total.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::profile::{self, Profiles};
use crate::store::{Store, STORE_DIR};

/// What nami takes, in the order each path is counted in (the first that uses it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Part {
    /// Your current apps, with your home files and what they need.
    Yours,
    /// The computer's current apps (system.nami), for everyone; on namiOS, its systems too.
    Everyone,
    /// Projects' tools (`nami shell`), while their folders are there.
    Projects,
    /// Other people's current apps on this computer.
    Others,
    /// What programs running now use, though nothing else does any more.
    Running,
    /// Kept only by older changes, for undo: anyone's.
    Older,
    /// What nothing uses: `nami gc` frees it.
    Unused,
    /// A Mac's apps from Homebrew's casks that are in use, kept once beside the store.
    Casks,
    /// nami's own folder: the catalog, icons, build logs, the program.
    Nami,
}

impl Part {
    /// In the order they're counted: a path is the first's that uses it.
    const COUNTED: [Part; 7] = [Part::Yours, Part::Everyone, Part::Projects, Part::Others, Part::Running, Part::Older, Part::Unused];

    /// What it's called where people read it (`nami storage`, the app).
    pub fn said(self) -> &'static str {
        match self {
            Part::Yours => "Your apps",
            Part::Everyone => "Apps for everyone",
            Part::Projects => "Projects' tools",
            Part::Others => "Other people's apps",
            Part::Running => "In use by open programs",
            Part::Older => "Older changes",
            Part::Unused => "Not used by anything",
            Part::Casks => "Apps from Homebrew's casks",
            Part::Nami => "nami itself",
        }
    }

    /// Whether it's in use now (not only kept for undo, or nothing's).
    fn current(self) -> bool {
        !matches!(self, Part::Older | Part::Unused)
    }
}

/// The store as a graph: each registered path (by its base name) and each of a Mac's casks'
/// folders (`cask:<folder>`), its size, and what it uses.
pub struct Graph {
    nodes: HashMap<String, Node>,
}

struct Node {
    size: u64,
    refs: Vec<String>,
    /// A casks' folder.
    cask: bool,
    /// A casks' folder kept in the last hour, which `nami gc` spares (an install may not have
    /// linked it yet).
    young: bool,
}

/// A casks' folder as a node's name.
fn cask_node(folder: &str) -> String {
    format!("cask:{folder}")
}

impl Graph {
    /// Every path nami registered, from its record, and the casks' folders beside the store.
    pub fn read(store: &Store) -> Graph {
        let mut nodes = HashMap::new();
        let casks = crate::casks::dir(store.root());
        for entry in std::fs::read_dir(store.var_dir().join("db")).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(base) = name.strip_suffix(".json") else { continue };
            if !store.is_valid(base) {
                continue;
            }
            let Ok(info) = store.path_info(base) else { continue };
            let mut refs = info.references;
            // A cask's store path links to its apps' folder: it uses it.
            if base.contains("-cask-") {
                let apps = store.phys(&format!("{STORE_DIR}/{base}/Applications"));
                for a in std::fs::read_dir(&apps).into_iter().flatten().flatten() {
                    if let Some(folder) = std::fs::read_link(a.path()).ok().and_then(|to| to.strip_prefix(&casks).ok().and_then(|r| r.components().next()).map(|c| c.as_os_str().to_string_lossy().into_owned())) {
                        refs.push(cask_node(&folder));
                    }
                }
            }
            nodes.insert(base.to_string(), Node { size: info.nar_size, refs, cask: false, young: false });
        }
        for entry in std::fs::read_dir(&casks).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let young = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() < CASK_SPARED);
            nodes.insert(cask_node(&name), Node { size: files_size(&entry.path()), refs: Vec::new(), cask: true, young });
        }
        Graph { nodes }
    }

    /// What's reached from `roots` that isn't in `seen` yet, added to it.
    fn reach<'a>(&self, roots: impl IntoIterator<Item = &'a String>, seen: &mut HashSet<String>) -> Vec<String> {
        let mut found = Vec::new();
        let mut stack: Vec<String> = roots.into_iter().map(|r| crate::store::base_name(r).to_string()).collect();
        while let Some(b) = stack.pop() {
            let Some(node) = self.nodes.get(&b) else { continue };
            if seen.insert(b.clone()) {
                stack.extend(node.refs.iter().cloned());
                found.push(b);
            }
        }
        found
    }

    fn size(&self, name: &str) -> u64 {
        self.nodes.get(name).map_or(0, |n| n.size)
    }

    /// What `nami gc` would take with `roots` kept: how many paths and casks' folders, and
    /// their size.
    pub fn unused(&self, roots: &[String]) -> (usize, u64) {
        let mut live = HashSet::new();
        self.reach(roots, &mut live);
        self.nodes.iter().filter(|(name, n)| !live.contains(*name) && !n.young).fold((0, 0), |(c, b), (_, n)| (c + 1, b + n.size))
    }
}

/// How long `casks::sweep` spares a folder just kept.
const CASK_SPARED: u64 = 3600;

/// What `nami gc` would take with `roots` kept (its `--dry-run`): store paths and a Mac's
/// casks' folders.
pub fn unused(store: &Store, roots: &[String]) -> Result<(usize, u64)> {
    Ok(Graph::read(store).unused(roots))
}

/// A folder's files' size (as `casks::sweep` counts what it frees), not following links.
fn files_size(p: &Path) -> u64 {
    match p.symlink_metadata() {
        Ok(m) if m.is_dir() => std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| files_size(&e.path())).sum(),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

/// The disk the store is on, as the file system says (statvfs).
#[derive(Serialize, Debug, Default, Clone, PartialEq)]
pub struct Disk {
    /// Where it was asked: the store.
    pub path: String,
    pub size: u64,
    /// Everything on it: size less what's free.
    pub used: u64,
    /// What can still be written (not what's kept for root).
    pub free: u64,
    /// A Mac's: the store is a volume of its own (the Nix Store volume) in an APFS container
    /// it shares with the Mac's other volumes. `size`, `used` and `free` are the container's,
    /// as Finder, System Settings and `diskutil info` (Container Total and Free Space) say them.
    pub container: bool,
}

/// The disk at `path`. (On a Mac statvfs says the APFS container's size and what's free in it,
/// whichever of its volumes is asked.)
pub fn disk(path: &Path) -> Disk {
    use std::os::unix::ffi::OsStrExt;
    let shown = path.display().to_string();
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return Disk { path: shown, ..Default::default() } };
    // SAFETY: a path, and the struct statvfs fills.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return Disk { path: shown, ..Default::default() };
    }
    let block = s.f_frsize as u64;
    let (size, free) = (s.f_blocks as u64 * block, s.f_bavail as u64 * block);
    Disk { path: shown, size, used: size.saturating_sub(free), free, container: crate::index::MAC }
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Share {
    pub part: Part,
    pub bytes: u64,
    /// How many store paths (and casks' folders) it is; files, for nami's own folder.
    pub paths: usize,
}

/// One of the apps that take the most.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct App {
    pub attr: String,
    /// Its name and version, as the step has it (`gimp-3.0.4`).
    pub name: String,
    /// "you" or "system".
    pub whose: &'static str,
    /// What it needs that nothing else in use does: what removing it frees, once its older
    /// changes are cleaned up too.
    pub adds: u64,
    /// All it needs, shared or not.
    pub size: u64,
}

/// What cleaning up now frees: the nami app's Clean Up (your own old changes, `nami gc --now
/// --user`, then the computer's and what nothing uses, `nami gc --now`), as `cleanup:` keeps
/// the newest.
#[derive(Serialize, Debug, Clone, Default, PartialEq)]
pub struct Cleanup {
    /// Every change that goes, yours then the computer's (as `nami gc --dry-run` says them).
    pub changes: Vec<u64>,
    pub yours: Vec<u64>,
    pub computers: Vec<u64>,
    /// Store paths (and a Mac's casks' folders) nothing uses then, and their size.
    pub paths: usize,
    pub bytes: u64,
    /// How many of your newest changes stay, and of the computer's.
    pub keep: usize,
    pub keep_computer: usize,
    /// Whether cleaning up frees the space itself: as an administrator (on a Mac, with an
    /// administrator's password). Otherwise your old changes go, and the space they took is
    /// freed when an administrator cleans up.
    pub now: bool,
}

#[derive(Serialize, Debug, Clone)]
pub struct Report {
    pub disk: Disk,
    /// All nami takes: its parts added up.
    pub total: u64,
    /// The parts, in the order they're counted (nami's own folder last); none left out.
    pub parts: Vec<Share>,
    /// The apps that take the most, by what each adds, at most 10.
    pub largest: Vec<App>,
    pub cleanup: Cleanup,
    /// How a path used by more than one part is counted.
    pub counted: &'static str,
}

pub const COUNTED: &str = "Each store path once, in the first part that uses it: your apps, apps for everyone, projects' tools, other people's apps, open programs, older changes, then what nothing uses. Sizes are the store's records (NAR sizes); casks' apps and nami's folder are their files' sizes.";

/// Whose profiles there are: yours, the computer's, and everyone else's.
struct Whose<'a> {
    me: Option<Profiles<'a>>,
    system: Profiles<'a>,
    others: Vec<Profiles<'a>>,
}

fn whose(store: &Store) -> Result<Whose<'_>> {
    let me = Profiles::personal(store)?;
    let me = (!me.is_system()).then_some(me);
    let mut others = Vec::new();
    for u in std::fs::read_dir(store.var_dir().join("profiles/per-user")).into_iter().flatten().flatten() {
        let Some(name) = u.file_name().to_str().map(String::from) else { continue };
        let p = Profiles::user(store, &name);
        if me.as_ref().is_none_or(|m| m.dir() != p.dir()) {
            others.push(p);
        }
    }
    others.sort_by(|a, b| a.dir().cmp(b.dir()));
    Ok(Whose { me, system: Profiles::system(store), others })
}

fn current_roots(p: &Profiles) -> Vec<String> {
    p.current().ok().flatten().map(|m| profile::step_roots(&m)).unwrap_or_default()
}

/// What only a profile's older steps use: their roots.
fn older_roots(p: &Profiles) -> Vec<String> {
    let current = p.current_generation();
    p.generations().unwrap_or_default().into_iter().filter(|g| Some(*g) != current).filter_map(|g| p.manifest(g).ok()).flat_map(|m| profile::step_roots(&m)).collect()
}

/// How many of the newest changes cleaning up keeps: yours (your file's `cleanup:`) and the
/// computer's (system.nami's).
#[derive(Clone, Copy, Debug)]
pub struct Keep {
    pub yours: usize,
    pub computers: usize,
}

/// Everything, for whoever asks (reading only: no daemon, no lock).
pub fn report(store: &Store) -> Result<Report> {
    let w = whose(store)?;
    let keep = Keep { yours: w.me.as_ref().map_or(3, |m| crate::cmd::generations::my_cleanup(m).keep), computers: crate::cleanup::of_file(&crate::config::system_file()).keep };
    report_with(store, w, keep)
}

fn report_with(store: &Store, w: Whose, keep: Keep) -> Result<Report> {
    // (What's walked or scanned, beside the rest: nami's folder, and what programs use.)
    let folder = store.var_dir();
    std::thread::scope(|s| {
        let nami_folder = s.spawn(|| folder_size(&folder));
        let open = s.spawn(crate::cleanup::running_paths);
        report_from(store, &w, keep, || nami_folder.join().unwrap_or_default(), || open.join().unwrap_or_default())
    })
}

fn report_from(store: &Store, w: &Whose, keep: Keep, nami_folder: impl FnOnce() -> (usize, u64), open: impl FnOnce() -> Vec<String>) -> Result<Report> {
    let graph = Graph::read(store);
    let everyone: Vec<&Profiles> = w.me.iter().chain([&w.system]).chain(&w.others).collect();
    // On namiOS its systems are the computer's (the older ones among them too: it can't tell).
    let os_roots = crate::nami_os::plugin().map(|os| os.roots(store.root(), &[])).unwrap_or_default();
    let mine = w.me.as_ref().map(current_roots).unwrap_or_default();
    let computers: Vec<String> = current_roots(&w.system).into_iter().chain(os_roots.iter().cloned()).collect();
    let projects: Vec<String> = everyone.iter().flat_map(|p| p.project_roots()).collect();
    let others: Vec<String> = w.others.iter().flat_map(current_roots).collect();
    let running: Vec<String> = everyone.iter().flat_map(|p| p.running_roots()).chain(open()).collect();
    let older: Vec<String> = everyone.iter().flat_map(|p| older_roots(p)).collect();

    // Each part: what it reaches that no part before it did.
    let mut seen = HashSet::new();
    let mut of: HashMap<String, Part> = HashMap::new();
    for (part, roots) in [(Part::Yours, &mine), (Part::Everyone, &computers), (Part::Projects, &projects), (Part::Others, &others), (Part::Running, &running), (Part::Older, &older)] {
        for n in graph.reach(roots.iter(), &mut seen) {
            of.insert(n, part);
        }
    }
    let part_of = |name: &str, node: &Node| -> Part {
        match of.get(name) {
            // (A cask's apps in use: their own line.)
            Some(p) if node.cask && p.current() => Part::Casks,
            Some(p) => *p,
            // (One just kept, which the install hasn't linked yet: in use.)
            None if node.young => Part::Casks,
            None => Part::Unused,
        }
    };
    let mut parts: Vec<Share> = Part::COUNTED.iter().map(|&part| Share { part, bytes: 0, paths: 0 }).collect();
    parts.insert(Part::COUNTED.len() - 1, Share { part: Part::Casks, bytes: 0, paths: 0 });
    for (name, node) in &graph.nodes {
        let part = part_of(name, node);
        if let Some(s) = parts.iter_mut().find(|s| s.part == part) {
            s.bytes += node.size;
            s.paths += 1;
        }
    }
    // (No casks without a Mac's: not a line of its own.)
    parts.retain(|s| s.part != Part::Casks || s.paths > 0 || crate::index::MAC);
    let (files, bytes) = nami_folder();
    parts.push(Share { part: Part::Nami, bytes, paths: files });
    let total = parts.iter().map(|s| s.bytes).sum();

    Ok(Report {
        disk: disk(&store.phys(STORE_DIR)),
        total,
        parts,
        largest: largest(&graph, w, [&projects, &others, &running, &os_roots]),
        cleanup: cleanup(store, &graph, w, keep)?,
        counted: COUNTED,
    })
}

/// A folder's files, as `du` counts them (allocated blocks, links not followed), and how many.
fn folder_size(p: &Path) -> (usize, u64) {
    use std::os::unix::fs::MetadataExt;
    let Ok(m) = p.symlink_metadata() else { return (0, 0) };
    let own = m.blocks() * 512;
    if !m.is_dir() {
        return (1, own);
    }
    std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| folder_size(&e.path())).fold((0, own), |(n, b), (n2, b2)| (n + n2, b + b2))
}

/// The apps that take the most, by what each needs that nothing else in use does (`rest`:
/// what else is in use, besides the apps of yours and the computer's).
fn largest(graph: &Graph, w: &Whose, rest: [&Vec<String>; 4]) -> Vec<App> {
    let mut holders: Vec<(Option<(profile::Entry, &'static str)>, Vec<String>)> = Vec::new();
    let mut others: Vec<String> = rest.iter().flat_map(|r| r.iter().cloned()).collect();
    for (p, whose) in w.me.iter().map(|p| (p, "you")).chain([(&w.system, "system")]) {
        let Some(m) = p.current().ok().flatten() else { continue };
        others.extend(m.runtime.iter().flat_map(|e| e.paths.clone()));
        others.extend(m.home.as_ref().map(|h| h.path.clone()));
        others.extend(m.services.as_ref().map(|s| s.path.clone()));
        others.extend(m.etc.as_ref().map(|e| e.path.clone()));
        for e in m.packages {
            let paths = e.paths.clone();
            holders.push((Some((e, whose)), paths));
        }
    }
    holders.push((None, others));
    // How many holders need each path, and what each needs.
    let mut needed: HashMap<String, usize> = HashMap::new();
    let closures: Vec<Vec<String>> = holders.iter().map(|(_, roots)| graph.reach(roots.iter(), &mut HashSet::new())).collect();
    for c in &closures {
        for n in c {
            *needed.entry(n.clone()).or_default() += 1;
        }
    }
    let mut apps: Vec<App> = holders
        .into_iter()
        .zip(&closures)
        .filter_map(|((h, _), c)| {
            let (e, whose) = h?;
            let adds = c.iter().filter(|n| needed[*n] == 1).map(|n| graph.size(n)).sum();
            Some(App { attr: e.attr, name: e.name, whose, adds, size: c.iter().map(|n| graph.size(n)).sum() })
        })
        .filter(|a| a.adds > 0)
        .collect();
    apps.sort_by(|a, b| b.adds.cmp(&a.adds).then_with(|| a.attr.cmp(&b.attr)));
    apps.truncate(10);
    apps
}

/// Whether cleaning up as this person frees the store's space itself: root, an admin (through
/// the daemon), or on a Mac anyone (the app asks for an administrator's password).
pub fn frees_now() -> bool {
    profile::is_root() || profile::is_admin() || crate::index::MAC
}

/// What Clean Up frees: your old changes as your `cleanup:` keeps them, then (where it's freed
/// now) the computer's as system.nami's does, and what nothing uses after.
fn cleanup(store: &Store, graph: &Graph, w: &Whose, k: Keep) -> Result<Cleanup> {
    let now = frees_now();
    let pick = |p: &Profiles, keep: usize| p.pick(&profile::Prune { keep: Some(keep), ..Default::default() }).unwrap_or_default();
    let (keep, keep_computer) = (k.yours, k.computers);
    let yours = w.me.as_ref().map(|m| pick(m, keep)).unwrap_or_default();
    let policy = profile::Prune { keep: Some(keep_computer), ..Default::default() };
    // (On namiOS the computer's changes are its systems.)
    let systems = if now && crate::nami_os::here() { crate::nami_os::require()?.pick(store.root(), &policy) } else { Vec::new() };
    let computers = if !now { Vec::new() } else if crate::nami_os::here() { systems.clone() } else { pick(&w.system, keep_computer) };
    let mut skip: Vec<(&Profiles, &[u64])> = Vec::new();
    if let Some(m) = &w.me {
        skip.push((m, &yours));
    }
    if !crate::nami_os::here() {
        skip.push((&w.system, &computers));
    }
    let (paths, bytes) = graph.unused(&profile::all_roots_without(store, &skip, &systems)?);
    Ok(Cleanup { changes: yours.iter().chain(&computers).copied().collect(), yours, computers, paths, bytes, keep, keep_computer, now })
}

/// `nami storage`, as words: the disk, nami's parts, the largest apps, what cleaning up frees.
pub fn print(r: &Report) {
    use crate::tell::size;
    let paint = crate::style::out();
    let d = &r.disk;
    let on = if d.container { "the Mac's disk" } else { "the disk" };
    println!("{}", paint.line(&format!("{} of {} used on {on} · {} free", size(d.used), size(d.size), size(d.free))));
    println!("{}", paint.line(&format!("nami takes {}", size(r.total))));
    let shown: Vec<&Share> = r.parts.iter().filter(|s| s.bytes > 0).collect();
    let wide = shown.iter().map(|s| s.part.said().len()).max().unwrap_or(0);
    for s in shown {
        println!("{}", paint.line(&format!("  {:wide$}  {:>9}", s.part.said(), size(s.bytes))));
    }
    if !r.largest.is_empty() {
        println!("{}", paint.line("Largest apps (what removing each frees)"));
        let wide = r.largest.iter().map(|a| a.attr.len()).max().unwrap_or(0);
        for a in &r.largest {
            let whose = if a.whose == "system" { "  for everyone" } else { "" };
            println!("{}", paint.line(&format!("  {:wide$}  {:>9}{whose}", a.attr, size(a.adds))));
        }
    }
    println!("{}", paint.line(&cleanup_line(&r.cleanup)));
}

/// What cleaning up frees, in a line.
fn cleanup_line(c: &Cleanup) -> String {
    if c.bytes == 0 && c.changes.is_empty() {
        return "✓ Nothing to clean up".into();
    }
    let mut said = vec![format!("Cleaning up now frees {}", crate::tell::size(c.bytes))];
    if !c.changes.is_empty() {
        said.push(crate::tell::count(c.changes.len(), "older change", "older changes"));
    }
    said.push(if c.now { "nami gc --now --user, then nami gc --now".to_string() } else { "nami gc --now --user (an administrator frees the space: sudo nami gc)".to_string() });
    said.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{Entry, Manifest};

    /// A scratch computer: its store's records (each path's size and references) and its
    /// people's steps, written as nami writes them.
    struct Scratch {
        root: std::path::PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let root = std::env::temp_dir().join(format!("nami-storage-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            Store::open_direct(&root, false).unwrap();
            Scratch { root }
        }

        fn store(&self) -> Store {
            Store::open_direct_readonly(&self.root).unwrap()
        }

        /// A path of `size` bytes using `refs` (names: `<32 a's>-<name>`).
        fn path(&self, name: &str, size: u64, refs: &[&str]) {
            let store = self.store();
            let info = crate::store::PathInfo { references: refs.iter().map(|r| base(r)).collect(), nar_hash: "sha256:x".into(), nar_size: size, built: false };
            std::fs::write(store.var_dir().join("db").join(format!("{}.json", base(name))), serde_json::to_vec(&info).unwrap()).unwrap();
        }

        /// A person's step (`who`: None for the computer's), current or not, of apps
        /// (attr, the paths they use).
        fn step(&self, who: Option<&str>, g: u64, current: bool, apps: &[(&str, &[&str])]) {
            let store = self.store();
            let p = match who {
                Some(u) => Profiles::user(&store, u),
                None => Profiles::system(&store),
            };
            std::fs::create_dir_all(p.dir()).unwrap();
            let packages = apps.iter().map(|(attr, paths)| Entry { attr: attr.to_string(), name: format!("{attr}-1.0"), paths: paths.iter().map(|n| format!("{STORE_DIR}/{}", base(n))).collect() }).collect();
            let m = Manifest { generation: g, created: g, packages, runtime: Vec::new(), caches: Vec::new(), graphics: false, home: None, services: None, etc: None, config: None, start: false, from: None };
            std::fs::write(p.dir().join(format!("default-{g}.json")), serde_json::to_vec(&m).unwrap()).unwrap();
            std::fs::create_dir_all(p.dir().join(format!("default-{g}"))).unwrap();
            if current {
                let link = p.dir().join("default");
                let _ = std::fs::remove_file(&link);
                std::os::unix::fs::symlink(format!("default-{g}"), &link).unwrap();
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn base(name: &str) -> String {
        format!("{}-{name}", "a".repeat(32))
    }

    fn share(r: &Report, part: Part) -> u64 {
        r.parts.iter().find(|s| s.part == part).map_or(0, |s| s.bytes)
    }

    /// The report as the person `me` sees it (the defaults of `cleanup:` kept), others being
    /// `others`.
    fn seen_by(s: &Scratch, me: &str, others: &[&str]) -> Report {
        let store = s.store();
        let w = Whose { me: Some(Profiles::user(&store, me)), system: Profiles::system(&store), others: others.iter().map(|o| Profiles::user(&store, o)).collect() };
        report_with(&store, w, Keep { yours: 3, computers: 3 }).unwrap()
    }

    #[test]
    fn each_path_counts_once_and_the_parts_add_up() {
        let s = Scratch::new("parts");
        // glibc: everyone's. gimp yours, firefox the computer's, an old vlc only in an older step.
        s.path("glibc", 1000, &[]);
        s.path("gtk", 400, &["glibc"]);
        s.path("gimp", 300, &["gtk", "glibc"]);
        s.path("firefox", 200, &["gtk", "glibc"]);
        s.path("vlc", 50, &["glibc"]);
        s.path("leftover", 7, &[]);
        s.step(Some("ada"), 1, false, &[("gimp", &["gimp"]), ("vlc", &["vlc"])]);
        s.step(Some("ada"), 2, true, &[("gimp", &["gimp"])]);
        s.step(Some("grace"), 1, true, &[("gimp", &["gimp"])]);
        s.step(None, 1, true, &[("firefox", &["firefox"])]);
        let r = seen_by(&s, "ada", &["grace"]);
        // Shared paths are the first part's: yours.
        assert_eq!(share(&r, Part::Yours), 1700);
        assert_eq!(share(&r, Part::Everyone), 200);
        assert_eq!(share(&r, Part::Others), 0, "grace's gimp is ada's too");
        assert_eq!(share(&r, Part::Older), 50);
        assert_eq!(share(&r, Part::Unused), 7);
        assert_eq!(r.total, r.parts.iter().map(|s| s.bytes).sum::<u64>());
        let store_bytes: u64 = r.parts.iter().filter(|s| s.part != Part::Nami).map(|s| s.bytes).sum();
        assert_eq!(store_bytes, 1957, "every path once: {:?}", r.parts);
        assert!(share(&r, Part::Nami) > 0, "nami's own folder: its records and steps");
        // The largest: what each alone needs (gtk and glibc are shared by both).
        let largest: Vec<(&str, u64, u64)> = r.largest.iter().map(|a| (a.attr.as_str(), a.adds, a.size)).collect();
        assert_eq!(largest, vec![("firefox", 200, 1600)], "gimp is grace's too: removing it frees nothing");
        // As grace sees it: ada's older vlc is still an older change, gimp hers.
        let r = seen_by(&s, "grace", &["ada"]);
        assert_eq!((share(&r, Part::Yours), share(&r, Part::Others), share(&r, Part::Older)), (1700, 0, 50));
    }

    #[test]
    fn cleaning_up_frees_what_only_the_older_changes_beyond_keep_use() {
        let s = Scratch::new("cleanup");
        for (n, size) in [("one", 10), ("two", 20), ("three", 40), ("four", 80), ("five", 160), ("now", 1)] {
            s.path(n, size, &[]);
        }
        s.path("junk", 5, &[]);
        // Six steps, each its own app: the newest three stay, the current one among them
        // (cleanup:'s default keep), so steps 1 to 3 go, and what only they used.
        for (g, n) in [(1, "one"), (2, "two"), (3, "three"), (4, "four"), (5, "five"), (6, "now")] {
            s.step(Some("ada"), g, g == 6, &[(n, &[n])]);
        }
        s.step(None, 1, true, &[]);
        let store = s.store();
        let r = seen_by(&s, "ada", &[]);
        let c = &r.cleanup;
        assert_eq!(c.keep, 3);
        assert_eq!(c.changes, vec![1, 2, 3], "{c:?}");
        assert_eq!((c.paths, c.bytes), (4, 75), "the three steps' apps, and what nothing used: {c:?}");
        // As `nami gc --dry-run` says what nothing uses now.
        assert_eq!(unused(&store, &profile::all_roots(&store).unwrap()).unwrap(), (1, 5));
    }

    #[test]
    fn nothing_to_clean_up_is_said_plainly() {
        let c = Cleanup { keep: 3, now: true, ..Default::default() };
        assert_eq!(cleanup_line(&c), "✓ Nothing to clean up");
        let c = Cleanup { changes: vec![4, 5], yours: vec![4, 5], bytes: 2_300_000_000, paths: 120, keep: 3, now: true, ..Default::default() };
        assert_eq!(cleanup_line(&c), "Cleaning up now frees 2.3 GB · 2 older changes · nami gc --now --user, then nami gc --now");
    }

    /// A Mac's casks' apps, kept beside the store: in use, their own part; what no step's
    /// cask links to any more, what cleaning up frees.
    #[test]
    fn casks_apps_are_counted_by_their_files_and_freed_with_their_cask() {
        let s = Scratch::new("casks");
        let store = s.store();
        let casks = crate::casks::dir(&s.root);
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * CASK_SPARED);
        for (folder, bytes) in [("rectangle-1-abc", 3000usize), ("chrome-1-def", 9000)] {
            std::fs::create_dir_all(casks.join(folder).join("X.app")).unwrap();
            std::fs::write(casks.join(folder).join("X.app/binary"), vec![0u8; bytes]).unwrap();
            std::fs::File::open(casks.join(folder)).unwrap().set_modified(old).unwrap();
        }
        // Rectangle's cask is a step's, linking to its folder; Chrome's folder nothing links to.
        s.path("cask-rectangle-1", 1, &[]);
        let apps = store.phys(&format!("{STORE_DIR}/{}/Applications", base("cask-rectangle-1")));
        std::fs::create_dir_all(&apps).unwrap();
        std::os::unix::fs::symlink(casks.join("rectangle-1-abc/X.app"), apps.join("X.app")).unwrap();
        s.step(Some("ada"), 1, true, &[("cask:rectangle", &["cask-rectangle-1"])]);
        let r = seen_by(&s, "ada", &[]);
        assert_eq!(share(&r, Part::Casks), 3000);
        assert_eq!(share(&r, Part::Unused), 9000);
        assert_eq!((r.cleanup.paths, r.cleanup.bytes), (1, 9000));
        assert_eq!(r.largest.first().map(|a| (a.attr.as_str(), a.adds)), Some(("cask:rectangle", 3001)));
    }

    #[test]
    fn the_parts_have_names_people_read() {
        for p in Part::COUNTED.iter().chain(&[Part::Casks, Part::Nami]) {
            let said = p.said();
            assert!(!said.to_lowercase().contains("tray") && !said.contains("store path"), "{said}");
        }
        assert_eq!(serde_json::to_value(Part::Everyone).unwrap(), "everyone");
    }
}
