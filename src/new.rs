//! `nami new <folder> --from <example>`: a copy of one of nami's examples
//! (examples/ in nami's source, built into it) as files of yours. nami has
//! no packages, projects or services of its own: a computer lists the
//! package folders it uses by path, so what an example brings is copied
//! next to it, and is yours to change from then on.
//!
//! - a namiOS computer (namios, namios-bare): <folder>/system.nami, and
//!   the package with every package it lists in the repo's packages/
//!   folder (../packages)
//! - any other package (tailscale, docker, printing...): copied there the
//!   same way
//! - a project (node, python, rust...): <folder>/project.nami
//! - a service (postgres, redis...): <folder>/services/<name>.nami

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use serde_yaml::Value;

include!(concat!(env!("OUT_DIR"), "/examples.rs"));

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Package,
    Project,
    Service,
}

/// The example `name`'s main file, by kind.
fn main_file(kind: Kind, name: &str) -> String {
    match kind {
        Kind::Package => format!("packages/{name}/package.nami"),
        Kind::Project => format!("projects/{name}/project.nami"),
        Kind::Service => format!("services/{name}.nami"),
    }
}

fn text(rel: &str) -> Option<&'static str> {
    EXAMPLES.iter().find(|(r, _)| *r == rel).and_then(|(_, b)| std::str::from_utf8(b).ok())
}

/// Each example package by name, with its package.nami.
pub fn example_packages() -> Vec<(String, &'static str)> {
    names(Kind::Package).into_iter().filter_map(|n| Some((n.clone(), text(&main_file(Kind::Package, &n))?))).collect()
}

fn kinds_of(name: &str) -> Vec<Kind> {
    [Kind::Package, Kind::Project, Kind::Service].into_iter().filter(|k| text(&main_file(*k, name)).is_some()).collect()
}

/// All examples of a kind, by name.
fn names(kind: Kind) -> Vec<String> {
    let mut out: Vec<String> = EXAMPLES
        .iter()
        .filter_map(|(r, _)| match kind {
            Kind::Package => r.strip_prefix("packages/")?.strip_suffix("/package.nami").filter(|n| !n.contains('/')),
            Kind::Project => r.strip_prefix("projects/")?.strip_suffix("/project.nami").filter(|n| !n.contains('/')),
            Kind::Service => r.strip_prefix("services/")?.strip_suffix(".nami").filter(|n| !n.contains('/')),
        })
        .map(String::from)
        .collect();
    out.sort();
    out
}

/// What an example is for: the first sentence of its first comment
/// (without a leading `name: `).
fn about(kind: Kind, name: &str) -> String {
    let comment: Vec<&str> = text(&main_file(kind, name)).unwrap_or("").lines().map_while(|l| l.strip_prefix("# ").or(l.strip_prefix('#'))).collect();
    let all = comment.join(" ");
    let all = all.strip_prefix(&format!("{name}: ")).unwrap_or(&all);
    let end = [all.find(". "), all.find(": ")].into_iter().flatten().min().unwrap_or(all.len());
    all[..end].trim_end_matches(['.', ',', ':']).to_string()
}

/// The package folders a package lists by path (`../niri`), by name.
fn listed(name: &str) -> Vec<String> {
    let Some(t) = text(&main_file(Kind::Package, name)) else { return Vec::new() };
    let v: Value = serde_yaml::from_str(t).unwrap_or(Value::Null);
    let entry = |e: &Value| match e {
        Value::String(s) => Some(s.clone()),
        Value::Mapping(m) => m.keys().next().and_then(Value::as_str).map(String::from),
        _ => None,
    };
    // Its neighbours, by path (../gdm) or by name (gdm), as nami reads
    // them (its own name is the catalog's program).
    v.get("packages")
        .and_then(Value::as_sequence)
        .into_iter()
        .flatten()
        .filter_map(entry)
        .filter_map(|p| match p.strip_prefix("../") {
            Some(n) => Some(n.to_string()),
            None => (p != name && is_package(&p)).then_some(p),
        })
        .collect()
}

/// A package and every package it lists, all the way down.
fn closure(name: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut todo = vec![name.to_string()];
    while let Some(n) = todo.pop() {
        if seen.insert(n.clone()) {
            todo.extend(listed(&n));
        }
    }
    seen.into_iter().collect()
}

/// A whole computer: it has what every computer has (base) and a login
/// screen.
fn is_whole(name: &str) -> bool {
    let all = closure(name);
    let has = |p: &str| all.iter().any(|n| n == p);
    has("base") && ["gdm", "namios-greeter"].iter().any(|p| has(p))
}

/// A namiOS computer to start from (namios, namios-bare): a whole computer
/// that no other example builds on (niri is one, under namiOS).
fn is_computer(name: &str) -> bool {
    is_whole(name) && !names(Kind::Package).iter().any(|o| listed(o).iter().any(|l| l == name))
}

/// What `nami new` with no example says.
pub fn list() {
    let packages = names(Kind::Package);
    let pick = |f: &dyn Fn(&str) -> bool| packages.iter().filter(|n| f(n)).cloned().collect::<Vec<_>>();
    // Each kind: its title bold, how to start one cyan, then its starters (names bold, what each is dim).
    let p = crate::style::out();
    let width = crate::show::Term::out().width;
    let show = |kind: Kind, list: &[String]| {
        for n in list {
            let lead = format!("  {}{}  ", p.bold(n), " ".repeat(14usize.saturating_sub(n.chars().count())));
            let about = about(kind, n);
            let about = match width {
                Some(w) => crate::show::trim(&about, w.saturating_sub(crate::show::cols(&lead) + 1)),
                None => about,
            };
            println!("{lead}{}", p.dim(&about));
        }
    };
    let title = |t: &str, how: &str| println!("{}{}{}", p.bold(t), p.dim(" · "), p.accent(how));
    title("namiOS computers", "nami new laptop --from <name>");
    show(Kind::Package, &pick(&is_computer));
    println!();
    title("Projects", "nami new <folder> --from <name>");
    show(Kind::Project, &names(Kind::Project));
    println!();
    title("Services for a project", "nami new <project folder> --from <name>");
    show(Kind::Service, &names(Kind::Service));
    let others = pick(&|n| !is_whole(n));
    println!();
    title("Packages to add to a computer", "nami new laptop --from <name>");
    println!("  {}", others.join(", "));
}

/// A package example as `nami new --json` shows it: its name, what it is,
/// and its pictures, when its folder has them (icon.svg, preview.svg).
fn shown(n: &str) -> serde_json::Value {
    let art = |f: &str| text(&format!("packages/{n}/{f}"));
    serde_json::json!({ "name": n, "about": about(Kind::Package, n), "icon": art("icon.svg"), "preview": art("preview.svg"), "system": sets_up_system(n) })
}

/// What `nami new --json` says: each kind's examples, with what each is.
pub fn list_json() -> serde_json::Value {
    let all = |kind: Kind, keep: &dyn Fn(&str) -> bool| -> Vec<serde_json::Value> {
        names(kind).into_iter().filter(|n| keep(n)).map(|n| if kind == Kind::Package { shown(&n) } else { serde_json::json!({ "name": n, "about": about(kind, &n) }) }).collect()
    };
    serde_json::json!({
        "computers": names(Kind::Package).into_iter().filter(|n| is_computer(n)).map(|n| {
            let mut v = shown(&n);
            v["parts"] = serde_json::json!(listed(&n));
            v
        }).collect::<Vec<_>>(),
        "packages": all(Kind::Package, &|n| !is_whole(n)),
        "projects": all(Kind::Project, &|_| true),
        "services": all(Kind::Service, &|_| true),
    })
}

/// Every example by name, with what it is, quickly (tab completion's
/// `nami new --from`): projects, services, then packages and computers.
pub fn starters() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (kind, what) in [(Kind::Project, "A project"), (Kind::Service, "A service"), (Kind::Package, "")] {
        for n in names(kind) {
            let about = about(kind, &n);
            out.push((n, if what.is_empty() { about } else if about.is_empty() { what.to_string() } else { format!("{what}: {about}") }));
        }
    }
    out
}

/// Write an example's files under `prefix` into `to`, keeping what's there:
/// how many it wrote.
fn copy(prefix: &str, to: &Path, overwrite: bool) -> Result<usize> {
    let mut n = 0;
    for (rel, bytes) in EXAMPLES.iter().filter(|(r, _)| r.starts_with(prefix)) {
        let dest = to.join(rel.strip_prefix(prefix).unwrap());
        if dest.exists() && !overwrite {
            continue;
        }
        std::fs::create_dir_all(dest.parent().unwrap())?;
        std::fs::write(&dest, bytes).with_context(|| format!("writing {}", dest.display()))?;
        n += 1;
    }
    Ok(n)
}

/// Where nami's example packages are copied, next to a computer's folder
/// (the repo's packages/ folder, where names are looked up).
pub fn packages_next_to(folder: &Path) -> PathBuf {
    let parent = folder.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    parent.join(crate::config::PACKAGES_DIR)
}

/// Whether nami has an example package called `name`.
pub fn is_package(name: &str) -> bool {
    text(&main_file(Kind::Package, name)).is_some()
}

/// What in a package.nami is the whole computer's, not one person's: its
/// services, files in /etc, groups, drivers, the kernel's command line...
const COMPUTER_KEYS: &[&str] = &["services", "files", "groups", "users", "boot", "drivers", "login", "setuid", "hardware", "name-lookup", "libraries", "secure-boot", "without", "env", "settings"];

/// Whether installing `name` sets up the computer, so it's everyone's (an
/// administrator installs it, in system.nami): nami's package of that
/// name, or one it lists, has a service, files in /etc, groups, drivers...
/// (`docker`, `steam`, `tailscale`). Anything else can be one person's
/// alone, in their user.nami.
pub fn sets_up_system(name: &str) -> bool {
    is_package(name)
        && closure(name).iter().any(|p| {
            let v: Value = text(&main_file(Kind::Package, p)).and_then(|t| serde_yaml::from_str(t).ok()).unwrap_or(Value::Null);
            v.as_mapping().is_some_and(|m| m.keys().filter_map(Value::as_str).any(|k| COMPUTER_KEYS.contains(&k)))
        })
}

/// nami's own apps, which no catalog has (nami's window): `nami install
/// nami-gui` brings its recipe as this nami has it, for the computer.
const OWN_APPS: &[&str] = &["nami-gui"];

/// Whether `name` is one of nami's own apps.
pub fn is_own_app(name: &str) -> bool {
    OWN_APPS.contains(&name) && is_package(name)
}

/// The example package `name` in the packages/ folder `shared` as this nami
/// has it, in place of what was there.
pub fn write_package(shared: &Path, name: &str) -> Result<()> {
    ensure!(is_package(name) && !name.contains('/'), "nami has no package {name}");
    let dir = shared.join(name);
    let new = shared.join(format!(".{name}.new"));
    let _ = std::fs::remove_dir_all(&new);
    copy(&format!("packages/{name}/"), &new, true)?;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&new, &dir).with_context(|| format!("writing {}", dir.display()))?;
    Ok(())
}

/// Example packages, each with what it lists, copied into the packages/
/// folder next to the computer folder `folder` (what's there is kept), as
/// `nami import` brings them.
pub fn copy_packages(folder: &Path, names: &[String]) -> Result<()> {
    let shared = packages_next_to(folder);
    for p in names.iter().filter(|n| is_package(n)).flat_map(|n| closure(n)).collect::<BTreeSet<_>>() {
        if !shared.join(&p).exists() {
            copy(&format!("packages/{p}/"), &shared.join(&p), false)?;
        }
    }
    Ok(())
}

/// Where nami notes what it last wrote into a packages/ folder (each file's
/// sha256): anything else in there is someone's own change.
const WRITTEN_NOTE: &str = ".nami-examples";
/// Files replaced at the first refresh, when there's no note to tell whether
/// they were someone's own: kept here.
const BEFORE_DIR: &str = ".before-update";

/// What [`refresh_packages`] did.
#[derive(Debug, Default)]
pub struct Refreshed {
    /// The packages now as this nami has them, and those kept as someone changed them.
    pub changed: Vec<String>,
    pub kept: Vec<String>,
    /// Each file it touched and what was in it: for [`Refreshed::undo`].
    before: Vec<(PathBuf, Option<Vec<u8>>)>,
}

impl Refreshed {
    /// Put back what it changed.
    pub fn undo(&self) -> Result<()> {
        for (path, was) in self.before.iter().rev() {
            match was {
                Some(bytes) => std::fs::write(path, bytes)?,
                None => std::fs::remove_file(path)?,
            }
        }
        Ok(())
    }
}

/// The files under `dir`, by path, with what's in them.
fn files_in(dir: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(out) };
    for e in entries.flatten() {
        let (path, kind) = (e.path(), e.file_type()?);
        anyhow::ensure!(!kind.is_symlink(), "{} is a link: leaving this folder as it is", path.display());
        if kind.is_dir() {
            out.extend(files_in(&path)?);
        } else {
            out.push((path.clone(), std::fs::read(&path)?));
        }
    }
    Ok(out)
}

/// A new file or folder in `shared` belongs to whoever `shared` belongs to.
fn same_owner(shared: &Path, path: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let top = std::fs::metadata(shared)?;
    std::os::unix::fs::chown(path, Some(top.uid()), Some(top.gid()))?;
    let dir = std::fs::metadata(path)?.is_dir();
    let mode = match (dir, top.mode() & 0o020 != 0) {
        (true, _) => top.mode() & 0o7777,
        (false, true) => 0o664,
        (false, false) => 0o644,
    };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// Copy the folder `from` to `to`.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let (src, dst, kind) = (e.path(), to.join(e.file_name()), e.file_type()?);
        if kind.is_dir() {
            copy_tree(&src, &dst)?;
        } else if kind.is_file() {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// The example packages the computer in folder `computer` uses with this
/// nami's versions of them, found as making the computer finds them: read
/// with all of them at hand (so what it leaves out with `without:` stays out),
/// in a scratch copy.
fn needed_now(shared: &Path, computer: &Path) -> Result<BTreeSet<String>> {
    let scratch = std::env::temp_dir().join(format!("nami-refresh-{}-{}", std::process::id(), &crate::catalog::sha256_hex(shared.to_string_lossy().as_bytes())[..8]));
    let _ = std::fs::remove_dir_all(&scratch);
    let packages = scratch.join(crate::config::PACKAGES_DIR);
    let found = (|| -> Result<BTreeSet<String>> {
        std::fs::create_dir_all(&packages)?;
        for e in std::fs::read_dir(shared)?.flatten() {
            if e.file_type()?.is_dir() && !e.file_name().to_string_lossy().starts_with('.') {
                copy_tree(&e.path(), &packages.join(e.file_name()))?;
            }
        }
        copy("packages/", &packages, true)?;
        let folder = scratch.join("computer");
        copy_tree(computer, &folder)?;
        let used = crate::config::used(&crate::oldnames::find_in(&folder, crate::config::SYSTEM_FILE))?;
        let packages = packages.canonicalize()?;
        Ok(used.folders.iter().filter_map(|f| f.strip_prefix(&packages).ok()?.components().next()?.as_os_str().to_str().map(String::from)).filter(|n| is_package(n)).collect())
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    found
}

/// Nami's example packages in the packages/ folder `shared`, as this nami has
/// them, with whatever the computer in folder `computer` needs of them now. A package someone changed since nami
/// last wrote it stays as it is; with no note of that yet (the first time),
/// a file that's different is replaced, and kept in .before-update.
pub fn refresh_packages(shared: &Path, computer: &Path) -> Result<Refreshed> {
    let sha = |b: &[u8]| crate::catalog::sha256_hex(b);
    let note = shared.join(WRITTEN_NOTE);
    // (tray's note, until nami writes its own: what tray wrote, so what's changed since is kept.)
    let old_note = shared.join(".tray-examples");
    let seen: Option<std::collections::BTreeMap<String, String>> = std::fs::read_to_string(&note)
        .or_else(|_| std::fs::read_to_string(&old_note))
        .ok()
        .map(|t| t.lines().filter_map(|l| l.split_once("  ").map(|(h, f)| (f.to_string(), h.to_string()))).collect());
    let rel_of = |path: &Path| path.strip_prefix(shared).unwrap_or(path).display().to_string();
    let mut here = BTreeSet::new();
    for e in std::fs::read_dir(shared)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if is_package(&name) && e.file_type()?.is_dir() {
            here.insert(name);
        }
    }
    here.extend(needed_now(shared, computer)?);
    let mut done = Refreshed::default();
    let mut written = Vec::new();
    for name in here {
        let dir = shared.join(&name);
        let theirs = files_in(&dir)?;
        if let Some(seen) = &seen {
            if theirs.iter().any(|(path, bytes)| seen.get(&rel_of(path)) != Some(&sha(bytes))) {
                written.extend(theirs.iter().filter_map(|(path, _)| seen.get(&rel_of(path)).map(|h| (rel_of(path), h.clone()))));
                done.kept.push(name);
                continue;
            }
        }
        let prefix = format!("packages/{name}/");
        let ours: Vec<(PathBuf, &[u8])> = EXAMPLES.iter().filter_map(|(r, b)| Some((dir.join(r.strip_prefix(&prefix)?), *b))).collect();
        let mut changed = false;
        if !dir.exists() {
            std::fs::create_dir_all(&dir)?;
            same_owner(shared, &dir)?;
        }
        for (path, bytes) in &ours {
            let was = theirs.iter().find(|(p, _)| p == path).map(|(_, b)| b);
            written.push((rel_of(path), sha(bytes)));
            if was.is_some_and(|b| b.as_slice() == *bytes) {
                continue;
            }
            if let (None, Some(was)) = (&seen, was) {
                let keep = shared.join(BEFORE_DIR).join(path.strip_prefix(shared).unwrap_or(path));
                std::fs::create_dir_all(keep.parent().unwrap())?;
                std::fs::write(&keep, was)?;
            }
            if let Some(parent) = path.parent().filter(|p| !p.exists()) {
                std::fs::create_dir_all(parent)?;
                same_owner(shared, parent)?;
            }
            done.before.push((path.clone(), was.cloned()));
            std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))?;
            if was.is_none() {
                same_owner(shared, path)?;
            }
            changed = true;
        }
        for (path, bytes) in theirs.iter().filter(|(p, _)| !ours.iter().any(|(o, _)| o == p)) {
            done.before.push((path.clone(), Some(bytes.clone())));
            std::fs::remove_file(path)?;
            changed = true;
        }
        if changed {
            done.changed.push(name);
        }
    }
    let text: String = written.iter().map(|(rel, hash)| format!("{hash}  {rel}\n")).collect();
    let had_note = note.exists();
    std::fs::write(&note, text)?;
    if !had_note {
        same_owner(shared, &note)?;
    }
    let _ = std::fs::remove_file(&old_note);
    Ok(done)
}

/// `nami new <folder> --from <name>`.
pub fn new(folder: &Path, name: &str, force: bool) -> Result<()> {
    let kinds = kinds_of(name);
    let kind = match kinds.as_slice() {
        [] => {
            let all: Vec<String> = [Kind::Package, Kind::Project, Kind::Service].into_iter().flat_map(names).collect();
            let all: Vec<&str> = all.iter().map(String::as_str).collect();
            let close = crate::config::closest(name, &all).map(|c| format!("did you mean `{c}`? ")).unwrap_or_default();
            bail!("nami has no example called {name} ({close}`nami new` lists them)");
        }
        [k] => *k,
        _ => bail!("{name} is more than one example; this can't happen"),
    };
    match kind {
        Kind::Package => new_computer(folder, name),
        Kind::Project => {
            let file = folder.join(crate::project::FILE);
            ensure!(force || !file.exists(), "{} already exists (--force replaces it)", file.display());
            std::fs::create_dir_all(folder)?;
            copy(&format!("projects/{name}/"), folder, true)?;
            // How to work in it, for coding agents (AGENTS.md); one that's there is kept.
            let agents = folder.join("AGENTS.md");
            let wrote_agents = !agents.exists() && std::fs::write(&agents, agents_md(&file)).is_ok();
            println!("wrote {}{}: `nami shell` opens it, `nami run <task>` runs a task", file.display(), if wrote_agents { " and AGENTS.md" } else { "" });
            Ok(())
        }
        Kind::Service => {
            let file = folder.join("services").join(format!("{name}.nami"));
            ensure!(force || !file.exists(), "{} already exists (--force replaces it)", file.display());
            ensure!(folder.is_dir(), "no folder {}", folder.display());
            std::fs::create_dir_all(file.parent().unwrap())?;
            std::fs::write(&file, text(&main_file(kind, name)).unwrap())?;
            // A project here: listed under its services, if it isn't.
            let entry = format!("./services/{name}.nami");
            let project = folder.join(crate::project::FILE);
            if project.is_file() {
                let before = std::fs::read_to_string(&project)?;
                let listed = crate::project::read(&project).map(|p| p.services.keys().any(|k| k.as_str() == Some(entry.as_str()))).unwrap_or(false);
                if !listed {
                    let op = crate::config_edit::Op::Add { path: vec!["services".into()], value: serde_yaml::Value::from(entry.clone()) };
                    std::fs::write(&project, crate::config_edit::apply(&before, &op)?)?;
                }
                println!("wrote {} and listed it in {}: `nami up` starts it", file.display(), project.display());
            } else {
                println!("wrote {}: list it under services: in project.nami, and `nami up` starts it\n\n  services:\n    - {entry}\n", file.display());
            }
            Ok(())
        }
    }
}

/// A new project's AGENTS.md (examples/projects/AGENTS.md): how to use nami in it, with its
/// tasks as project.nami has them.
fn agents_md(project_file: &Path) -> String {
    let template = text("projects/AGENTS.md").unwrap_or_default();
    let tasks = crate::project::read(project_file).map(|p| p.tasks).unwrap_or_default();
    let listed = if tasks.is_empty() {
        "No tasks yet: add them under `tasks:` in `project.nami` (`test: cargo test`).".to_string()
    } else {
        tasks.iter().map(|(name, t)| format!("- `nami run {name}`: {}", t.shown())).collect::<Vec<_>>().join("\n")
    };
    template.replace("{tasks}", &listed)
}

/// A package: its folder and what it lists, in packages/ next to
/// `folder`; and `folder`/system.nami listing it, for a whole computer.
fn new_computer(folder: &Path, name: &str) -> Result<()> {
    let shared = packages_next_to(folder);
    let computer = crate::oldnames::find_in(folder, crate::config::SYSTEM_FILE);
    if !computer.exists() {
        ensure!(is_whole(name), "{name} isn't a whole computer: start from namios or namios-bare (`nami new` lists them), then `nami new {} --from {name}` copies it next to it", folder.display());
    }
    let mut kept = Vec::new();
    let mut wrote = 0;
    for p in closure(name) {
        let dest = shared.join(&p);
        if dest.exists() {
            kept.push(p);
            continue;
        }
        wrote += copy(&format!("packages/{p}/"), &dest, false)?;
    }
    if computer.exists() {
        println!("copied {name} into {}{}", shared.display(), if wrote == 0 { " (it was there)" } else { "" });
        println!("list it in {}:\n\n  packages:\n    - {name}\n", computer.display());
    } else {
        let host = folder.canonicalize().ok().or_else(|| std::env::current_dir().ok().map(|d| d.join(folder))).and_then(|d| d.file_name().map(|f| f.to_string_lossy().into_owned())).unwrap_or_else(|| "computer".into());
        let user = std::env::var("USER").ok().filter(|u| !u.is_empty() && u != "root" && u.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')).unwrap_or_else(|| "me".into());
        std::fs::create_dir_all(folder)?;
        std::fs::write(
            &computer,
            format!(
                "# {host}, from nami's {name} example: {}.\n# `nami usb {} --to /dev/sdX` makes a USB drive of it.\nhostname: {host}\n# A name is nixpkgs' package, set up by its folder in ../packages when\n# there is one (yours to change); namios, niri and the like are only there.\npackages:\n  - {name}\nusers:\n  {user}: {{ admin: true }}\n",
                about(Kind::Package, name),
                folder.display()
            ),
        )?;
        println!("wrote {}, and {name} with the packages it lists in {}", computer.display(), shared.display());
    }
    if !kept.is_empty() {
        println!("kept yours (already in {}): {}", shared.display(), kept.join(", "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_project_says_how_to_work_in_it() {
        let dir = std::env::temp_dir().join(format!("nami-new-agents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        new(&dir, "rust", false).unwrap();
        let agents = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
        assert!(agents.contains("- `nami run build`: cargo build --release\n- `nami run test`: cargo test"), "{agents}");
        assert!(agents.contains("NAMI_NONINTERACTIVE=1") && !agents.contains("{tasks}"));
        // One of theirs stays as it is.
        std::fs::write(dir.join("AGENTS.md"), "mine\n").unwrap();
        new(&dir, "python", true).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("AGENTS.md")).unwrap(), "mine\n");
        // A project without tasks says how to add one.
        let empty = dir.join("empty");
        new(&empty, "empty", false).unwrap();
        assert!(std::fs::read_to_string(empty.join("AGENTS.md")).unwrap().contains("No tasks yet"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nami_s_own_app_is_written_as_this_nami_has_it() {
        assert!(is_own_app("nami-gui") && !is_own_app("docker") && !is_own_app("firefox"));
        let shared = std::env::temp_dir().join(format!("nami-own-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&shared);
        std::fs::create_dir_all(shared.join("nami-gui")).unwrap();
        std::fs::write(shared.join("nami-gui/package.nami"), "name: nami-gui\nversion: 0.0.1\n").unwrap();
        std::fs::write(shared.join("nami-gui/old"), "from an older nami\n").unwrap();
        write_package(&shared, "nami-gui").unwrap();
        assert_eq!(std::fs::read_to_string(shared.join("nami-gui/package.nami")).unwrap(), text("packages/nami-gui/package.nami").unwrap());
        assert!(!shared.join("nami-gui/old").exists() && !shared.join(".nami-gui.new").exists());
        assert!(write_package(&shared, "../x").is_err());
        let _ = std::fs::remove_dir_all(&shared);
    }

    #[test]
    fn refresh_updates_nami_s_packages_and_keeps_changed_ones() {
        let top = std::env::temp_dir().join(format!("nami-refresh-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&top);
        let shared = top.join("packages");
        let computer = top.join("pc");
        std::fs::create_dir_all(&computer).unwrap();
        // It leaves out what niri lists: gdm is no reason to bring its folder.
        std::fs::write(computer.join("system.nami"), "packages:\n  - base\n  - niri\nwithout:\n  - gdm\n").unwrap();
        let file = shared.join("base/package.nami");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::create_dir_all(shared.join("mine")).unwrap();
        std::fs::write(shared.join("mine/package.nami"), "packages: []\n").unwrap();
        std::fs::write(&file, "old\n").unwrap();
        std::fs::write(shared.join("base/gone.nami"), "old\n").unwrap();
        let ours = text("packages/base/package.nami").unwrap();

        // The first time: as nami has it, what was there kept to one side.
        let first = refresh_packages(&shared, &computer).unwrap();
        assert!(first.changed.contains(&"base".to_string()));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), ours);
        assert!(!shared.join("base/gone.nami").exists());
        assert_eq!(std::fs::read_to_string(shared.join(".before-update/base/package.nami")).unwrap(), "old\n");
        // What it uses comes too, not what it leaves out; a package of someone's own stays.
        assert!(shared.join("niri/package.nami").is_file());
        assert!(!shared.join("gdm").exists());
        assert_eq!(std::fs::read_to_string(shared.join("mine/package.nami")).unwrap(), "packages: []\n");

        // Nothing to do the next time, and undo has nothing to put back.
        let again = refresh_packages(&shared, &computer).unwrap();
        assert!(again.changed.is_empty() && again.kept.is_empty() && again.before.is_empty());

        // Someone's change stays, every time.
        std::fs::write(&file, "mine\n").unwrap();
        for _ in 0..2 {
            let kept = refresh_packages(&shared, &computer).unwrap();
            assert_eq!(kept.kept, vec!["base".to_string()]);
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "mine\n");
        }

        // A stale file, written by nami, moves on; undo puts it back.
        std::fs::write(&file, ours).unwrap();
        refresh_packages(&shared, &computer).unwrap();
        let note = std::fs::read_to_string(shared.join(WRITTEN_NOTE)).unwrap();
        let old_hash = crate::catalog::sha256_hex(b"stale\n");
        std::fs::write(shared.join(WRITTEN_NOTE), note.replace(&crate::catalog::sha256_hex(ours.as_bytes()), &old_hash)).unwrap();
        std::fs::write(&file, "stale\n").unwrap();
        let moved = refresh_packages(&shared, &computer).unwrap();
        assert_eq!(moved.changed, vec!["base".to_string()]);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), ours);
        moved.undo().unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "stale\n");
        std::fs::remove_dir_all(&top).unwrap();
    }

    #[test]
    fn what_sets_up_the_system_is_the_systems() {
        // A service, files in /etc, groups, 32-bit drivers: the computer's.
        for n in ["docker", "steam", "tailscale", "cups", "kvm", "gamemode", "nvidia", "kernel-zen", "printing"] {
            assert!(sets_up_system(n), "{n} sets up the computer");
        }
        // Programs alone (the catalog's, or nami's recipes) can be one person's.
        for n in ["hello", "bat-bin", "vscode", "claude-desktop", "bolt", "gimp", "firefox", "flathub:org.gimp.GIMP", "./packages/mine"] {
            assert!(!sets_up_system(n), "{n} can be one person's");
        }
        assert!(list_json()["packages"].as_array().unwrap().iter().any(|p| p["name"] == "docker" && p["system"] == true));
        assert!(list_json()["packages"].as_array().unwrap().iter().any(|p| p["name"] == "hello" && p["system"] == false));
    }

    #[test]
    fn examples_have_one_name_each() {
        for kind in [Kind::Package, Kind::Project, Kind::Service] {
            for n in names(kind) {
                assert_eq!(kinds_of(&n).len(), 1, "{n}");
                assert!(!about(kind, &n).is_empty(), "{n}: its first line says what it is");
            }
        }
        let pick = |f: &dyn Fn(&str) -> bool| names(Kind::Package).into_iter().filter(|n| f(n)).collect::<Vec<_>>();
        if is_package("namios") {
            // namiOS is the only computer, with or without its apps; niri is
            // whole too, but namiOS builds on it.
            assert_eq!(pick(&is_computer), ["namios", "namios-bare"]);
            assert!(is_whole("niri") && !is_computer("niri"));
            // Each computer shows a picture of itself.
            for n in pick(&is_computer) {
                assert!(text(&format!("packages/{n}/preview.svg")).is_some(), "{n}: preview.svg");
            }
        } else {
            // Without namiOS's own packages (nami's public repo): niri is.
            assert_eq!(pick(&is_computer), ["niri"]);
        }
        // What they list is there too.
        for n in names(Kind::Package) {
            for p in closure(&n) {
                assert!(text(&main_file(Kind::Package, &p)).is_some(), "{n} lists ../{p}, which isn't an example");
            }
        }
    }

    #[test]
    fn a_computer_from_an_example() {
        let d = std::env::temp_dir().join(format!("nami-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let laptop = d.join("laptop");
        // namiOS barebones, or niri where namiOS's own packages aren't (nami's public repo).
        let (from, copied): (&str, &[&str]) =
            if is_package("namios-bare") { ("namios-bare", &["namios-bare", "niri", "namios-greeter", "base", "desktop"]) } else { ("niri", &["niri", "gdm", "base", "desktop"]) };
        new(&laptop, from, false).unwrap();
        let text = std::fs::read_to_string(laptop.join("system.nami")).unwrap();
        assert!(text.contains("hostname: laptop\n") && text.contains(&format!("packages:\n  - {from}\n")) && !text.contains("from:"), "{text}");
        for p in copied {
            assert!(d.join(format!("packages/{p}/package.nami")).is_file(), "{p}");
        }
        // It loads, from the copies alone.
        let c = crate::config::load(&laptop.join("system.nami")).unwrap();
        assert!(c.os.unwrap().packages.iter().any(|p| p == "nautilus"));
        // Another package next to it: copied, yours kept.
        std::fs::write(d.join("packages/base/package.nami"), "# mine\npackages: [bash]\n").unwrap();
        new(&laptop, "tailscale", false).unwrap();
        assert!(d.join("packages/tailscale/package.nami").is_file());
        assert_eq!(std::fs::read_to_string(d.join("packages/base/package.nami")).unwrap(), "# mine\npackages: [bash]\n");
        assert!(new(&d.join("server"), "tailscale", false).unwrap_err().to_string().contains("isn't a whole computer"));
        // A project, and a service for it.
        let shop = d.join("shop");
        new(&shop, "node", false).unwrap();
        new(&shop, "postgres", false).unwrap();
        assert!(shop.join("project.nami").is_file() && shop.join("services/postgres.nami").is_file());
        let listed = std::fs::read_to_string(shop.join("project.nami")).unwrap();
        assert!(listed.contains("services:\n  - ./services/postgres.nami"), "{listed}");
        new(&shop, "postgres", true).unwrap();
        assert_eq!(std::fs::read_to_string(shop.join("project.nami")).unwrap().matches("postgres.nami").count(), 1, "listed once");
        assert!(new(&shop, "node", false).is_err(), "it's there");
        assert!(new(&shop, &from[..from.len() - 1], false).unwrap_err().to_string().contains(&format!("did you mean `{from}`?")));
        std::fs::remove_dir_all(&d).ok();
    }
}
