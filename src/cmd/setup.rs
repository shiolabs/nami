//! Setting up a computer: its folder in /etc and who may change it, and a config repo
//! (`system-use`, `system-init`).

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::profile::Profiles;
use crate::store::Store;
use crate::cmd::files::{base_of, ensure_system_config, system_config};
use crate::cmd::apply::{apply_config_with, edit_and_apply};
use crate::{config, config_edit, oldnames, profile};

/// /etc/nami, as nami keeps it: the computer's folder in /etc/nami-config
/// (`name` when it has none yet), as in a config repo, so the packages it
/// lists by path (../packages/x) are in /etc/nami-config too. A
/// folder of its own there until now is moved in.
pub(crate) fn system_dir(name: &str) -> Result<PathBuf> {
    let dir = system_config().parent().unwrap().to_path_buf();
    if dir.is_symlink() {
        return Ok(dir);
    }
    let cfg = dir.with_file_name(config_folder_name(&dir));
    let own = cfg.join(name);
    for d in [&cfg, &own] {
        if !d.is_dir() {
            std::fs::create_dir_all(d)?;
            share_with_admins(d, if *d == own { 0o2755 } else { 0o2775 })?;
        }
    }
    if dir.is_dir() {
        std::fs::remove_dir(&own)?;
        std::fs::rename(&dir, &own)?;
    }
    std::os::unix::fs::symlink(std::path::Path::new(config_folder_name(&dir)).join(name), &dir).with_context(|| format!("linking {}", dir.display()))?;
    Ok(dir)
}

/// The name of the folder next to /etc/nami (`etc_nami`) with the computer's
/// config in it: nami-config, or tray-config on a computer that had tray
/// (kept: docs/rename-migration.md).
fn config_folder_name(etc_nami: &std::path::Path) -> &'static str {
    if !etc_nami.with_file_name("nami-config").exists() && etc_nami.with_file_name("tray-config").is_dir() { "tray-config" } else { "nami-config" }
}

/// Is `name` the config's folder's (nami-config, or tray's tray-config)?
pub(crate) fn is_config_folder(name: &std::ffi::OsStr) -> bool {
    name == "nami-config" || name == "tray-config"
}

/// Owned by root and the admin group, who may edit it (root only).
pub(crate) fn share_with_admins(p: &std::path::Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(gid) = profile::admin_group() {
        let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes())?;
        // SAFETY: a valid C path; root may chown.
        unsafe { libc::chown(c.as_ptr(), 0, gid) };
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

/// The computer's config, the admins' to edit (`/etc/nami-config`, or
/// /etc/nami when it's a folder of its own): a computer nami made starts
/// with it all root's, before there's anyone. Private files stay private,
/// and system.nami itself (system.nami, which has the people's
/// passwords and its secrets in it) is root's to write and the admins' to
/// read, nobody else's; its folder is root's to write, so the file can't be
/// put back by another (an admin changes it through the daemon, with their
/// password). Each person's folder in it (<login>/user.nami and what's with
/// it) is theirs and the admins' to read and change, nobody else's. What
/// came before is made so at the next update or start.
pub(crate) fn share_config_with_admins() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    // `person`: in someone's folder, theirs (their uid, if they're here).
    fn walk(dir: &std::path::Path, computer: &std::path::Path, person: Option<Option<u32>>) -> Result<()> {
        share_with_admins(dir, if dir == computer { 0o2755 } else { 0o2775 })?;
        for e in std::fs::read_dir(dir)?.flatten() {
            let (path, kind) = (e.path(), e.file_type()?);
            if kind.is_dir() {
                if e.file_name() != ".git" {
                    let theirs = person.or_else(|| config::person_of(&path).map(|login| profile::uid_of(&login)));
                    walk(&path, computer, theirs)?;
                }
            } else if kind.is_file() {
                let mode = e.metadata()?.permissions().mode();
                if let Some(uid) = person {
                    profile::for_person(&path, uid)?;
                } else if profile::is_system_file(&e.file_name().to_string_lossy()) {
                    profile::for_admins(&path)?;
                } else if mode & 0o040 != 0 {
                    share_with_admins(&path, 0o664 | (mode & 0o111))?;
                }
            }
        }
        Ok(())
    }
    let Some(dir) = system_config().parent().and_then(|d| d.canonicalize().ok()) else { return Ok(()) };
    let top = dir.parent().filter(|p| p.file_name().is_some_and(is_config_folder)).map(|p| p.to_path_buf()).unwrap_or_else(|| dir.clone());
    walk(&top, &dir, None)
}

/// `system-use`: a config repo's computer folder (system.nami, its lock,
/// the files it names) and the folders of what it lists (../shared/...)
/// become /etc/nami-config, where they are in the repo; /etc/nami is the
/// computer's folder there. The file it replaces is kept as <name>.before
pub(crate) fn system_use(from: &std::path::Path) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let ours = system_config();
    let dir = ours.parent().unwrap().to_path_buf();
    let src = oldnames::find_in(from, config::SYSTEM_FILE);
    let theirs = std::fs::read_to_string(&src).with_context(|| format!("reading {}", src.display()))?;
    let used = config::used(&src)?;
    let cfg = dir.with_file_name(config_folder_name(&dir));
    let rel = used.rel(&used.computer).to_path_buf();
    let own = cfg.join(&rel);
    // Folders under /etc/nami-config, the admins' as /etc/nami is.
    let make = |d: &std::path::Path| -> Result<()> {
        let below: Vec<&std::path::Path> = d.ancestors().take_while(|a| a.starts_with(&cfg)).collect();
        for a in below.into_iter().rev() {
            if !a.is_dir() {
                std::fs::create_dir_all(a)?;
                share_with_admins(a, 0o2775)?;
            }
        }
        Ok(())
    };
    if dir.is_dir() && !dir.is_symlink() {
        // A folder of its own until now: it's the computer's folder there.
        make(own.parent().unwrap_or(&cfg))?;
        if own.exists() {
            std::fs::remove_dir_all(&own)?;
        }
        std::fs::rename(&dir, &own)?;
    }
    make(&own)?;
    let target = std::path::Path::new(config_folder_name(&dir)).join(&rel);
    if std::fs::read_link(&dir).ok().as_deref() != Some(target.as_path()) {
        let _ = std::fs::remove_file(&dir);
        std::os::unix::fs::symlink(&target, &dir).with_context(|| format!("linking {} to {}", dir.display(), target.display()))?;
    }
    let file = oldnames::find_in(&dir, config::SYSTEM_FILE);
    if ours.exists() && std::fs::read_to_string(&ours).is_ok_and(|o| o != theirs) {
        let before = dir.join(format!("{}.before", config::SYSTEM_FILE));
        std::fs::rename(&ours, &before)?;
        eprintln!("system.nami was {}: kept as {}", ours.display(), before.display());
    }
    // (`private`: someone's folder, root's alone until it's theirs, below.)
    fn copy(from: &std::path::Path, to: &std::path::Path, private: bool) -> Result<()> {
        for e in std::fs::read_dir(from)? {
            let e = e?;
            let (src, dst) = (e.path(), to.join(e.file_name()));
            let kind = e.file_type()?;
            if e.file_name() == ".git" {
                continue;
            }
            if kind.is_dir() {
                std::fs::create_dir_all(&dst)?;
                share_with_admins(&dst, 0o2775)?;
                copy(&src, &dst, private || config::person_of(&src).is_some())?;
            } else if kind.is_file() {
                let exec = std::fs::metadata(&src)?.permissions().mode() & 0o111;
                let _ = std::fs::remove_file(&dst);
                if private {
                    use std::os::unix::fs::OpenOptionsExt;
                    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&dst)?;
                    std::io::copy(&mut std::fs::File::open(&src)?, &mut out)?;
                    std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o600 | (exec & 0o100)))?;
                } else {
                    std::fs::copy(&src, &dst)?;
                    share_with_admins(&dst, 0o664 | exec)?;
                }
            }
        }
        Ok(())
    }
    for f in &used.folders {
        let to = cfg.join(used.rel(f));
        make(&to)?;
        copy(f, &to, config::person_of(f).is_some())?;
    }
    // Where it came from: nami's edits to system.nami go there too,
    // and `nami apply` takes that file when it changes (the repo's is the truth).
    if let Ok(src) = std::fs::canonicalize(from) {
        let note = dir.join(REPO_NOTE);
        std::fs::write(&note, format!("{}\n{}\n", src.display(), sha256_hex(theirs.as_bytes())))?;
        share_with_admins(&note, 0o664)?;
    }
    // system.nami and its folder: root's to write, the admins' to read.
    share_config_with_admins()?;
    Ok(file)
}

pub(crate) fn sha256_hex(data: &[u8]) -> String {
    crate::catalog::sha256_hex(data)
}

/// system.nami as it was when it and the repo's were last the same
/// (its sha256, the note's second line): which of the two changed since.
pub(crate) fn repo_note_seen() -> Option<String> {
    let note = std::fs::read_to_string(system_config().parent()?.join(REPO_NOTE)).ok()?;
    note.lines().nth(1).map(str::to_string).filter(|h| !h.is_empty())
}

pub(crate) fn repo_note_set_seen(text: &str) -> Result<()> {
    let path = system_config().parent().context("no folder for system.nami")?.join(REPO_NOTE);
    let Ok(note) = std::fs::read_to_string(&path) else { return Ok(()) };
    let from = note.lines().next().unwrap_or_default().to_string();
    std::fs::write(&path, format!("{from}\n{}\n", sha256_hex(text.as_bytes())))?;
    if profile::is_root() {
        share_with_admins(&path, 0o664)?;
    }
    Ok(())
}

/// In /etc/nami: the folder of the config repo's checkout the computer's
/// file came from (`nami apply github:you/nami-config`).
const REPO_NOTE: &str = ".from";

/// system.nami in the checkout it came from, if it's still there
/// (a plain file: nami writes it).
pub(crate) fn repo_system_file() -> Option<PathBuf> {
    let dir = system_config().parent()?.to_path_buf();
    let from = std::fs::read_to_string(dir.join(REPO_NOTE)).ok()?;
    let file = oldnames::find(&PathBuf::from(from.lines().next().unwrap_or_default().trim()).join(config::SYSTEM_FILE));
    std::fs::symlink_metadata(&file).ok().filter(|m| m.is_file()).map(|_| file)
}

/// `nami system-use`.
pub(crate) fn system_use_command(store: &Store, profiles: &Profiles, from: PathBuf, locked: bool, upgrade: bool, dry_run: bool) -> Result<()> {
    anyhow::ensure!(profile::is_root(), "this runs as root (through the daemon)");
    // A dry run applies the repo's file where it is: /etc/nami stays.
    let file = if dry_run { oldnames::find_in(&from, config::SYSTEM_FILE) } else { system_use(&from)? };
    apply_config_with(&store, &profiles, &file, locked, upgrade, dry_run)?;
    Ok(())
}

/// `nami system-init`.
pub(crate) fn system_init(store: &Store, profiles: &Profiles, from: PathBuf) -> Result<()> {
    anyhow::ensure!(profile::is_root(), "this runs as root (through the daemon)");
    let file = ensure_system_config(&profiles)?;
    let dir = base_of(&from);
    let theirs: Config = config::parse_config_in(&std::fs::read_to_string(&from).with_context(|| format!("reading {}", from.display()))?, &from.display().to_string(), Some(&dir))?;
    let ours: Config = config::parse_config(&std::fs::read_to_string(&file)?, &file.display().to_string()).unwrap_or_default();
    let mut ops = Vec::new();
    let text = std::fs::read_to_string(&file)?;
    for p in &theirs.packages {
        // Recipes are relative to the file they were in.
        let name = if p.starts_with("./") || p.starts_with("../") { dir.join(p).display().to_string() } else { p.clone() };
        ops.extend(config::add_package(&text, &name)?);
    }
    for c in &theirs.caches {
        ops.push(config_edit::Op::Add { path: vec!["caches".into()], value: serde_yaml::to_value(c)? });
    }
    if let (Some(false), None) = (theirs.unfree, ours.unfree) {
        ops.push(config_edit::Op::Set { path: vec!["unfree".into()], value: false.into() });
    }
    if let (Some(k), None) = (theirs.history, ours.history) {
        ops.push(config_edit::Op::Set { path: vec!["history".into()], value: (k as u64).into() });
    }
    edit_and_apply(&store, &profiles, &file, &ops, false)?;
    Ok(())
}
