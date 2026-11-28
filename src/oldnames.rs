//! The names nami had when it was tray, still read until 1.0
//! (docs/rename-migration.md): its files (`computer.tray`, `tray.lock`, ...),
//! its environment (`TRAY_*`), its packages (`trayos`, `tray-gui`, ...) and
//! where it kept things (`/nix/var/tray`, `~/.config/tray`, ...).

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The names a file of nami's had, newest first: system.nami was computer.nami (until
/// 0.19) and tray's computer.tray; nami.lock was tray.lock, .nami-recipients
/// .tray-recipients, a set (gaming.nami) gaming.tray.
pub fn old_file_names(new: &str) -> Vec<String> {
    match new {
        "nami.lock" => vec!["tray.lock".into()],
        ".nami-recipients" => vec![".tray-recipients".into()],
        "system.nami" => vec!["computer.nami".into(), "computer.tray".into()],
        _ => new.strip_suffix(".nami").filter(|s| !s.is_empty()).map(|s| vec![format!("{s}.tray")]).unwrap_or_default(),
    }
}

/// The name nami gives now to a file it named otherwise before: the other way round
/// from [`old_file_names`] (computer.nami and computer.tray are system.nami).
pub fn new_file_name(old: &str) -> Option<String> {
    match old {
        "tray.lock" => Some("nami.lock".into()),
        ".tray-recipients" => Some(".nami-recipients".into()),
        "computer.nami" | "computer.tray" => Some("system.nami".into()),
        _ => old.strip_suffix(".tray").filter(|s| !s.is_empty()).map(|s| format!("{s}.nami")),
    }
}

/// A file name as nami names it now (`computer.tray` is `system.nami`):
/// for telling what kind of file a path is.
pub fn current(name: &str) -> String {
    new_file_name(name).unwrap_or_else(|| name.to_string())
}

/// Is `name` the file nami calls `new`, by its name now or tray's?
pub fn is_named(name: impl AsRef<OsStr>, new: &str) -> bool {
    let name = name.as_ref();
    name == new || old_file_names(new).iter().any(|o| name == o.as_str())
}


/// Does `path` end in `.nami` or tray's `.tray`?
pub fn has_nami_extension(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "nami" || e == "tray")
}

/// Does the text `entry` end in `.nami` or `.tray`?
pub fn ends_with_extension(entry: &str) -> bool {
    entry.ends_with(".nami") || entry.ends_with(".tray")
}

/// `name` without its `.nami` (or `.tray`) on the end.
pub fn strip_extension(name: &str) -> Option<&str> {
    name.strip_suffix(".nami").or_else(|| name.strip_suffix(".tray"))
}

fn there(p: &Path) -> bool {
    p.symlink_metadata().is_ok()
}

/// The same file under two names (a link, either kind)?
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

static SAID: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

/// Say `what` about `path`, once a run.
fn once(path: &Path, what: impl FnOnce() -> String) {
    let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
    if said.get_or_insert_with(HashSet::new).insert(path.to_path_buf()) {
        eprintln!("{}", what());
    }
}

/// Where the file `new` (a path with nami's name for it) is to be read:
/// `new` when it's there, or when none is; else its newest old name that is
/// (computer.nami before computer.tray). With `new` and an old one both (two
/// files, not two names for one), `new`, and a warning once a run.
pub fn find(new: &Path) -> PathBuf {
    let olds = old_paths(new);
    if there(new) {
        if let Some(old) = olds.iter().find(|o| there(o) && !same_file(new, o)) {
            once(new, || format!("note: both {} and {} are in {}: nami uses {} ({} can go)", name(new), name(old), parent(new), name(new), name(old)));
        }
        return new.to_path_buf();
    }
    olds.into_iter().find(|o| there(o)).unwrap_or_else(|| new.to_path_buf())
}

/// [`find`] for the file `name` (nami's name for it) in `dir`.
pub fn find_in(dir: &Path, name: &str) -> PathBuf {
    find(&dir.join(name))
}

/// Is the file `new` there, by its name or its old one?
pub fn exists(new: &Path) -> bool {
    find(new).exists()
}

/// Is the file `new` a file, by its name or its old one?
pub fn is_file(new: &Path) -> bool {
    find(new).is_file()
}

/// Where to write the file `new`: itself, unless only its old name is
/// there. Then, outside a git repo, the file is renamed to its new name
/// (and nami says so); inside one it keeps its name (renaming it there is a
/// change to commit, a surprise to whoever shares the repo), and nami says
/// how to rename it.
pub fn for_writing(new: &Path) -> PathBuf {
    // (Given by its old name, as [`find`] gives it: the same.)
    let renamed = new.file_name().and_then(|n| new_file_name(&n.to_string_lossy())).map(|n| new.with_file_name(n));
    let new = renamed.as_deref().unwrap_or(new);
    if there(new) {
        return new.to_path_buf();
    }
    let Some(old) = old_paths(new).into_iter().find(|o| there(o)) else { return new.to_path_buf() };
    let dir = new.parent().unwrap_or(Path::new("."));
    if in_git_repo(dir) {
        once(&old, || format!("note: {} keeps its old name in this git repo; nami reads it as {}. To rename it: git mv {} {}", old.display(), name(new), name(&old), name(new)));
        return old;
    }
    // A link keeps pointing where it did, under the new name. The old name
    // stays, a link to the new one, for an older nami or tray (going back to it).
    match move_once(&old, new) {
        Ok(()) => {
            let _ = std::os::unix::fs::symlink(name(new), &old);
            eprintln!("renamed {} to {} (nami's name for it)", old.display(), name(new));
            new.to_path_buf()
        }
        // Another nami moved it a moment ago (root's commands run side by side): it's there.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => new.to_path_buf(),
        Err(_) => old,
    }
}

/// `old` to `new`, never over a `new` that's there: renameat2's RENAME_NOREPLACE, one step
/// (a link moved as the link, a folder too). Two namis moving it at once: one moves it, the
/// other is told it exists (a plain rename(2) put its old name, by then a link to `new`, over
/// the file). Where the filesystem hasn't got it: a second hard link, then the old name goes.
pub(crate) fn move_once(old: &Path, new: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let (o, n) = (std::ffi::CString::new(old.as_os_str().as_bytes())?, std::ffi::CString::new(new.as_os_str().as_bytes())?);
        const RENAME_NOREPLACE: libc::c_uint = 1;
        let r = unsafe { libc::syscall(libc::SYS_renameat2, libc::AT_FDCWD, o.as_ptr(), libc::AT_FDCWD, n.as_ptr(), RENAME_NOREPLACE) };
        if r == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if !matches!(e.raw_os_error(), Some(libc::EINVAL) | Some(libc::ENOSYS)) {
            return Err(e);
        }
    }
    // A Mac's: renameatx_np(RENAME_EXCL), APFS's.
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let (o, n) = (std::ffi::CString::new(old.as_os_str().as_bytes())?, std::ffi::CString::new(new.as_os_str().as_bytes())?);
        // SAFETY: two paths.
        let r = unsafe { libc::renameatx_np(libc::AT_FDCWD, o.as_ptr(), libc::AT_FDCWD, n.as_ptr(), libc::RENAME_EXCL) };
        if r == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if !matches!(e.raw_os_error(), Some(libc::EINVAL) | Some(libc::ENOTSUP)) {
            return Err(e);
        }
    }
    match std::fs::hard_link(old, new) {

        Ok(()) => std::fs::remove_file(old),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
        Err(_) if there(new) => Err(std::io::ErrorKind::AlreadyExists.into()),
        Err(_) => std::fs::rename(old, new),
    }
}

/// The paths `new` had before, newest first (system.nami: computer.nami, then tray's
/// computer.tray).
pub fn old_paths(new: &Path) -> Vec<PathBuf> {
    let Some(n) = new.file_name().and_then(|n| n.to_str()) else { return Vec::new() };
    old_file_names(n).into_iter().map(|o| new.with_file_name(o)).collect()
}

fn name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn parent(p: &Path) -> String {
    p.parent().map(|d| d.display().to_string()).filter(|d| !d.is_empty()).unwrap_or_else(|| ".".into())
}

/// A path tray wrote (a link's target, a file's place under a home) as
/// it is since nami moved tray's folders: `/nix/var/tray/...` is
/// `/nix/var/nami/...`, `.config/tray/...` is `.config/nami/...`.
pub fn nami_place(path: &str) -> String {
    let mut p = path.replace("/nix/var/tray/", "/nix/var/nami/").replace("/.local/state/tray/", "/.local/state/nami/").replace("/.config/tray/", "/.config/nami/");
    for (old, new) in [(".config/tray/", ".config/nami/"), (".local/state/tray/", ".local/state/nami/")] {
        if let Some(rest) = p.strip_prefix(old) {
            p = format!("{new}{rest}");
        }
    }
    p
}

/// Do `found` (a link tray or an older nami may have made) and `want` name the same place,
/// tray's folders moved, and nami's to /nami (0.20: `/nix/var/nami/...` and `/nix/var/tray/...`
/// are `/nami/...`, its old names links to it, or it where it stayed there)?
pub fn same_place(found: &Path, want: &Path) -> bool {
    let one = |p: &str| p.replace("/nix/var/nami/", "/nami/");
    found == want || one(&nami_place(&found.to_string_lossy())) == one(&want.to_string_lossy())
}

/// Is `dir` inside a git work tree (a `.git` in it or above)?
pub fn in_git_repo(dir: &Path) -> bool {
    // (Where it really is: /etc/nami can be a link into a config repo's checkout.)
    let dir = std::fs::canonicalize(dir).or_else(|_| std::path::absolute(dir)).unwrap_or_else(|_| dir.to_path_buf());
    dir.ancestors().any(|d| d.join(".git").exists())
}

/// tray's environment: each `TRAY_X` that's set, as `NAMI_X` when that one
/// isn't (`NAMI_X` wins). Run first thing, before any thread starts.
pub fn environment() {
    for (k, v) in from_old(std::env::vars_os()) {
        std::env::set_var(k, v);
    }
}

/// Of the environment `vars`, the `NAMI_X` to set from `TRAY_X`s: those not set already.
fn from_old(vars: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>) -> Vec<(String, std::ffi::OsString)> {
    let vars: Vec<(String, std::ffi::OsString)> = vars.filter_map(|(k, v)| Some((k.into_string().ok()?, v))).collect();
    vars.iter()
        .filter_map(|(k, v)| Some((format!("NAMI_{}", k.strip_prefix("TRAY_")?), v.clone())))
        .filter(|(k, _)| !vars.iter().any(|(have, _)| have == k))
        .collect()
}

/// The packages tray's own were called, and what they're called now:
/// `- trayos` in a computer.tray is namios.
pub const PACKAGES: &[(&str, &str)] = &[
    ("trayos", "namios"),
    ("trayos-apps", "namios-apps"),
    ("trayos-backups", "namios-backups"),
    ("trayos-bare", "namios-bare"),
    ("trayos-boot", "namios-boot"),
    ("trayos-dev", "namios-dev"),
    ("trayos-dock", "namios-dock"),
    ("trayos-files", "namios-files"),
    ("trayos-greeter", "namios-greeter"),
    ("trayos-icons", "namios-icons"),
    ("trayos-look", "namios-look"),
    ("trayos-pointer", "namios-pointer"),
    ("trayos-screens", "namios-screens"),
    ("trayos-wallpapers", "namios-wallpapers"),
    ("trayos-windows", "namios-windows"),
    ("tray-gui", "nami-gui"),
    ("tray-polkit", "nami-polkit"),
];

/// The name now of a package tray had under `old`.
pub fn package(old: &str) -> Option<&'static str> {
    PACKAGES.iter().find(|(o, _)| *o == old).map(|(_, n)| *n)
}

/// The keys of a file tray read under another name (`trayos:`), and theirs now.
pub const KEYS: &[(&str, &str)] = &[("trayos", "namios")];

/// A file's keys under tray's names (`trayos:`) read as nami's (`namios:`);
/// when both are there, what's in both is nami's.
pub fn keys(value: &mut serde_yaml::Value) {
    let Some(m) = value.as_mapping_mut() else { return };
    // Files at the places tray's packages read them: where nami's do.
    if let Some(files) = m.get_mut("files").and_then(serde_yaml::Value::as_mapping_mut) {
        let old: Vec<(serde_yaml::Value, String)> = files.keys().filter_map(|k| Some((k.clone(), file_place(k.as_str()?)?))).collect();
        for (k, new) in old {
            let v = files.remove(&k).unwrap_or_default();
            if !files.contains_key(new.as_str()) {
                files.insert(new.into(), v);
            }
        }
    }
    for (old, new) in KEYS {
        if let Some(v) = m.remove(*old) {
            match m.get_mut(*new) {
                Some(n) => under(n, v),
                None => {
                    m.insert((*new).into(), v);
                }
            }
        }
    }
}

/// A file in `files:` at a place tray's packages read it (its backups'
/// settings, the secrets in /run/tray/secrets): the place nami's read it.
pub fn file_place(path: &str) -> Option<String> {
    match path {
        "/etc/tray-backup.conf" => return Some("/etc/nami-backup.conf".into()),
        // (Your synced folders' settings, as the sync package's nami-sync reads them.)
        "~/.config/tray-sync.conf" => return Some("~/.config/nami-sync.conf".into()),
        _ => {}
    }
    path.strip_prefix("/run/tray/").map(|rest| format!("/run/nami/{rest}"))
}

/// A file's text before a change at `path`: when that's under one of
/// nami's keys (`namios.dock.on`) and the file has it under tray's name
/// alone (`trayos:`), that key renamed in place, so the change goes in with
/// what's there.
pub fn keys_in_text(text: &str, path: &[String]) -> String {
    // (`files: /etc/nami-backup.conf`, the file saying /etc/tray-backup.conf.)
    if let [files, place, ..] = path {
        if files == "files" {
            let old = [("/etc/nami-backup.conf", "/etc/tray-backup.conf".to_string())].into_iter().find(|(n, _)| n == place).map(|(_, o)| o).or_else(|| place.strip_prefix("/run/nami/").map(|r| format!("/run/tray/{r}")));
            if let Some(old) = old.filter(|o| text.contains(&format!("{o}:")) && !text.contains(&format!("{place}:"))) {
                return text.replace(&format!("{old}:"), &format!("{place}:"));
            }
        }
    }
    let Some((old, new)) = path.first().and_then(|k| KEYS.iter().find(|(_, n)| n == k)) else { return text.to_string() };
    let at = |k: &str| text.lines().any(|l| l.strip_prefix(k).is_some_and(|r| r.starts_with(':')));
    if !at(old) || at(new) {
        return text.to_string();
    }
    let mut out: String = text.lines().map(|l| match l.strip_prefix(old).filter(|r| r.starts_with(':')) {
        Some(rest) => format!("{new}{rest}\n"),
        None => format!("{l}\n"),
    }).collect();
    if !text.ends_with('\n') {
        out.pop();
    }
    out
}

/// What's in `old` and not in `new`, put in `new`.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nami-oldnames-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn names_both_ways() {
        for (new, old) in [("user.nami", "user.tray"), ("project.nami", "project.tray"), ("package.nami", "package.tray"), ("checks.nami", "checks.tray"), ("gaming.nami", "gaming.tray"), ("nami.lock", "tray.lock"), (".nami-recipients", ".tray-recipients")] {
            assert_eq!(old_file_names(new), [old]);
            assert_eq!(new_file_name(old).as_deref(), Some(new));
            assert!(is_named(old, new) && is_named(new, new));
        }
        // The system's file had two: nami's computer.nami until 0.19, tray's computer.tray.
        assert_eq!(old_file_names("system.nami"), ["computer.nami", "computer.tray"]);
        for old in ["computer.nami", "computer.tray"] {
            assert_eq!(new_file_name(old).as_deref(), Some("system.nami"));
            assert!(is_named(old, "system.nami"));
            assert_eq!(current(old), "system.nami");
        }
        // (namiOS's generated whole-system file keeps its name.)
        assert_eq!(current("computer.flat.tray"), "computer.flat.nami");
        assert!(old_file_names("README.md").is_empty() && old_file_names(".nami").is_empty());
        assert!(!is_named("system.nami", "user.nami"));
    }

    #[test]
    fn the_old_name_is_read_when_its_the_only_one() {
        let d = tmp("read");
        assert_eq!(find_in(&d, "system.nami"), d.join("system.nami"), "none: the new name");
        std::fs::write(d.join("computer.tray"), "packages: [jq]\n").unwrap();
        assert_eq!(find_in(&d, "system.nami"), d.join("computer.tray"));
        assert!(is_file(&d.join("system.nami")));
        std::fs::write(d.join("computer.nami"), "packages: [git]\n").unwrap();
        assert_eq!(find_in(&d, "system.nami"), d.join("computer.nami"), "the newer old name first");
        std::fs::write(d.join("system.nami"), "packages: [vim]\n").unwrap();
        assert_eq!(find_in(&d, "system.nami"), d.join("system.nami"), "all three: the new one wins");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn one_file_under_both_names_isnt_both() {
        let d = tmp("link");
        std::fs::write(d.join("project.nami"), "packages: [jq]\n").unwrap();
        std::os::unix::fs::symlink("project.nami", d.join("project.tray")).unwrap();
        assert!(same_file(&d.join("project.nami"), &d.join("project.tray")));
        assert_eq!(find_in(&d, "project.nami"), d.join("project.nami"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn writing_renames_the_old_file_outside_git_only() {
        let d = tmp("write");
        std::fs::write(d.join("user.tray"), "packages: [jq]\n").unwrap();
        assert_eq!(for_writing(&d.join("user.nami")), d.join("user.nami"));
        assert!(d.join("user.nami").is_file() && std::fs::read_link(d.join("user.tray")).unwrap() == Path::new("user.nami"), "renamed, the old name a link for an older tray");
        assert_eq!(find_in(&d, "user.nami"), d.join("user.nami"), "one file, two names: no note");
        let repo = d.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("laptop")).unwrap();
        std::fs::write(repo.join("laptop/computer.nami"), "packages: [jq]\n").unwrap();
        assert_eq!(for_writing(&repo.join("laptop/system.nami")), repo.join("laptop/computer.nami"), "in a repo it keeps its name");
        assert!(repo.join("laptop/computer.nami").is_file() && !repo.join("laptop/system.nami").exists());
        // Outside one, computer.nami becomes system.nami, all it had kept, its old name a link.
        std::fs::write(d.join("computer.nami"), "packages: [git]\nhostname: pc\n").unwrap();
        assert_eq!(for_writing(&d.join("system.nami")), d.join("system.nami"));
        assert_eq!(std::fs::read_to_string(d.join("system.nami")).unwrap(), "packages: [git]\nhostname: pc\n");
        assert_eq!(std::fs::read_link(d.join("computer.nami")).unwrap(), Path::new("system.nami"));
        // Given by its old name, the same.
        assert_eq!(for_writing(&d.join("computer.nami")), d.join("system.nami"));
        assert_eq!(for_writing(&d.join("new.nami")), d.join("new.nami"), "a new file: the new name");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn tray_variables_are_read_and_nami_ones_win() {
        let vars = [("TRAY_ROOT", "/old"), ("NAMI_ROOT", "/new"), ("TRAY_CHANNEL", "x"), ("PATH", "/bin"), ("TRAY_", "")];
        let got = from_old(vars.iter().map(|(k, v)| (k.into(), v.into())));
        assert_eq!(got, [("NAMI_CHANNEL".to_string(), "x".into()), ("NAMI_".to_string(), "".into())]);
    }

    #[test]
    fn a_change_under_the_new_key_renames_the_old_one() {
        let path = |p: &str| p.split('.').map(String::from).collect::<Vec<_>>();
        let text = "packages: [jq]\ntrayos:\n  dock: { on: false }\n";
        assert_eq!(keys_in_text(text, &path("namios.accent")), "packages: [jq]\nnamios:\n  dock: { on: false }\n");
        assert_eq!(keys_in_text(text, &path("packages")), text);
        let both = "trayos: { a: 1 }\nnamios: { b: 2 }";
        assert_eq!(keys_in_text(both, &path("namios.c")), both);
    }

    #[test]
    fn trays_links_are_namis() {
        let want = Path::new("/nix/var/nami/profiles/per-user/ada/profile/.home/.config/nami/env.sh");
        assert!(same_place(Path::new("/nix/var/tray/profiles/per-user/ada/profile/.home/.config/tray/env.sh"), want));
        assert!(!same_place(Path::new("/nix/var/tray/profiles/per-user/bob/profile/.home/.config/tray/env.sh"), want));
        assert_eq!(nami_place(".config/tray/env.sh"), ".config/nami/env.sh");
        assert_eq!(nami_place(".config/ghostty/config"), ".config/ghostty/config");
        // nami's folder at /nami: the links an older nami (or tray) made are nami's still.
        let top = Path::new("/nami/profiles/per-user/ada/profile/.home/.config/nami/env.sh");
        assert!(same_place(Path::new("/nix/var/nami/profiles/per-user/ada/profile/.home/.config/nami/env.sh"), top));
        assert!(same_place(Path::new("/nix/var/tray/profiles/per-user/ada/profile/.home/.config/tray/env.sh"), top));
        assert!(same_place(top, want), "and where it stayed in /nix/var/nami");
        assert!(!same_place(Path::new("/nix/var/nami/profiles/per-user/bob/profile/.home/.config/nami/env.sh"), top));
    }

    #[test]
    fn files_at_trays_places_are_at_namis() {
        let mut v: serde_yaml::Value = serde_yaml::from_str("files:\n  /etc/tray-backup.conf: { to: disk:B }\n  /run/tray/secrets/backup-password: x\n  /etc/hosts: y\n").unwrap();
        keys(&mut v);
        assert_eq!(v["files"]["/etc/nami-backup.conf"]["to"], serde_yaml::Value::from("disk:B"));
        assert_eq!(v["files"]["/run/nami/secrets/backup-password"], serde_yaml::Value::from("x"));
        assert!(v["files"].get("/etc/tray-backup.conf").is_none() && v["files"]["/etc/hosts"] == serde_yaml::Value::from("y"));
        let path = ["files".to_string(), "/etc/nami-backup.conf".to_string()];
        assert_eq!(keys_in_text("files:\n  /etc/tray-backup.conf:\n    to: x\n", &path), "files:\n  /etc/nami-backup.conf:\n    to: x\n");
    }

    #[test]
    fn old_packages_are_the_new_ones() {
        assert_eq!(package("trayos"), Some("namios"));
        assert_eq!(package("tray-gui"), Some("nami-gui"));
        assert_eq!(package("trayos-look"), Some("namios-look"));
        assert_eq!(package("firefox"), None);
        assert!(PACKAGES.iter().all(|(o, n)| crate::oldnames::package(o) == Some(n)));
    }

    #[test]
    fn a_computer_listing_trays_packages_gets_namis() {
        let d = tmp("packages");
        for p in ["namios", "trayos", "nami-gui", "htop"] {
            std::fs::create_dir_all(d.join("packages").join(p)).unwrap();
        }
        std::fs::write(d.join("packages/namios/package.nami"), "packages: [niri]\n").unwrap();
        std::fs::write(d.join("packages/trayos/package.tray"), "packages: [niri]\n").unwrap();
        std::fs::write(d.join("packages/nami-gui/package.nami"), "packages: []\n").unwrap();
        std::fs::write(d.join("packages/htop/package.tray"), "packages: [htop]\n").unwrap();
        std::fs::create_dir_all(d.join("pc")).unwrap();
        let mut v: serde_yaml::Value = serde_yaml::from_str("packages: [trayos, tray-gui, htop, firefox]\n").unwrap();
        crate::config::resolve_names(&mut v, &d.join("pc"));
        let got: Vec<&str> = v["packages"].as_sequence().unwrap().iter().filter_map(|p| p.as_str()).collect();
        assert_eq!(got, ["../packages/namios", "../packages/nami-gui", "../packages/htop", "firefox"], "tray's left behind folder isn't used; a package.tray still is");
        // A whole computer.tray, its packages in package.tray files, loads as before.
        std::fs::write(d.join("pc/computer.tray"), "hostname: pc\npackages: [htop]\n").unwrap();
        let file = find_in(&d.join("pc"), "system.nami");
        assert_eq!(file, d.join("pc/computer.tray"));
        let c = crate::config::load(&file).unwrap();
        assert!(c.packages.iter().any(|p| p == "htop"), "{:?}", c.packages);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_old_key_is_read_and_the_new_wins() {
        let mut v: serde_yaml::Value = serde_yaml::from_str("trayos: { dock: { on: false } }\n").unwrap();
        keys(&mut v);
        assert!(v.get("trayos").is_none() && v["namios"]["dock"]["on"] == serde_yaml::Value::Bool(false));
        let mut v: serde_yaml::Value = serde_yaml::from_str("trayos: { a: 1, b: { c: 3, d: 4 } }\nnamios: { a: 2, b: { c: 5 } }\n").unwrap();
        keys(&mut v);
        assert_eq!(v["namios"]["a"], serde_yaml::Value::from(2));
        assert_eq!(v["namios"]["b"]["c"], serde_yaml::Value::from(5));
        assert_eq!(v["namios"]["b"]["d"], serde_yaml::Value::from(4), "what only tray's has is kept");
    }
}
