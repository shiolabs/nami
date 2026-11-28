//! A Mac's apps in Applications: what a profile's packages have in `Applications/*.app`,
//! copied into a folder nami keeps there, `~/Applications/nami Apps` for your own and
//! `/Applications/nami Apps` for everyone's (`sudo nami install --system`). Spotlight,
//! Launchpad, the Dock and Finder then have them as any other app.
//!
//! Copies, not links: Spotlight and Launchpad don't follow links into /nix/store, and a
//! Finder alias or a Dock item breaks once a store path goes. A copy is a bundle of its own
//! (nix-darwin's /Applications/Nix Apps and home-manager's copyApps do the same): its files as
//! the store has them, byte for byte, so the app's signature holds; links that stay inside the
//! bundle (a framework's `Versions/Current`) stay links, and those that leave it (alacritty's
//! `Contents/MacOS -> ../../../bin`, a wrapper's links to the unwrapped app) are copied as
//! what they point to. /nix is a volume of its own, so APFS can't clone across: it's a real
//! copy, the app's size again on the disk. A cask's app (casks.rs) is kept on the Mac's own
//! volume instead, so its copy is a clone (`clonefile`), which takes no space of its own.
//!
//! The folder's `.nami-apps` says which are nami's (`<name>.app<TAB><store path>` lines):
//! each change of the profile (install, remove, apply, undo, upgrade, update: `Profiles::switch`)
//! adds what's new, replaces what changed (another store path), and takes away what's gone,
//! and never touches anything else in the folder. An app that's open while it's replaced
//! keeps running; it says to open it again.
//!
//! Called on a Mac only (`index::MAC`); what calls macOS (Launch Services, the running
//! processes) is behind `cfg(target_os = "macos")`, so the rest is tested on Linux too.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};

/// The folder's name, in ~/Applications and /Applications.
pub const FOLDER: &str = "nami Apps";
/// Which of the folder's apps are nami's, and from which store path.
pub const LIST: &str = ".nami-apps";

/// An app a package has: `Foo.app`, at `source` (its package's store path, then
/// `/Applications/Foo.app`).
#[derive(Debug, Clone, PartialEq)]
pub struct App {
    pub name: String,
    pub source: String,
    /// From Homebrew's casks (casks.rs), by its token: a developer's download, so its copy is
    /// marked as one (quarantined), and macOS asks before it opens the first time.
    pub cask: Option<String>,
}

/// What `.nami-apps` says of one app: the store path it was copied from, and the cask it's
/// from (`cask:<token>`, a third column), if it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    pub source: String,
    pub cask: Option<String>,
}

/// Where a profile's apps go: everyone's (`system`) in /Applications, a person's in theirs.
pub fn folder(system: bool, root: &Path) -> Option<PathBuf> {
    if system {
        return Some(root.join("Applications").join(FOLDER));
    }
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(Path::new(&home).join("Applications").join(FOLDER))
}

/// The apps of `paths` (a generation's packages' store paths), in their order: each one's
/// `Applications/*.app`. A name two packages have is the first one's.
pub fn of(root: &Path, paths: &[String]) -> Vec<App> {
    let mut apps: Vec<App> = Vec::new();
    for path in paths {
        let Some(dir) = follow(root, &Path::new(path).join("Applications")) else { continue };
        let Ok(entries) = fs::read_dir(on_disk(root, &dir)) else { continue };
        let mut names: Vec<String> = entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| usable(n)).collect();
        names.sort();
        for name in names {
            let source = format!("{path}/Applications/{name}");
            if !follow(root, Path::new(&source)).is_some_and(|p| on_disk(root, &p).is_dir()) {
                continue;
            }
            if apps.iter().any(|a| a.name == name) {
                crate::say!("warning: two packages have {name}; Applications has the first one's");
                continue;
            }
            apps.push(App { name, source, cask: None });
        }
    }
    apps
}

/// A name nami puts in the folder: an app's, and nothing a line of `.nami-apps` can't hold.
fn usable(name: &str) -> bool {
    name.ends_with(".app") && name.len() > 4 && !name.starts_with('.') && !name.contains(['/', '\t', '\n'])
}

/// The app's name as people see it: `Hidden Bar` of `Hidden Bar.app`.
fn shown(name: &str) -> &str {
    name.strip_suffix(".app").unwrap_or(name)
}

/// What `.nami-apps` says: each app's name, the store path it was copied from, and its cask.
pub fn read_list(folder: &Path) -> BTreeMap<String, Listed> {
    let text = fs::read_to_string(folder.join(LIST)).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let mut cols = l.split('\t');
            let (name, source) = (cols.next()?, cols.next()?);
            let cask = cols.next().and_then(|c| c.strip_prefix(crate::casks::PREFIX)).map(String::from);
            usable(name).then(|| (name.to_string(), Listed { source: source.to_string(), cask }))
        })
        .collect()
}

fn write_list(folder: &Path, list: &BTreeMap<String, Listed>) -> Result<()> {
    let text: String = list
        .iter()
        .map(|(n, l)| match &l.cask {
            Some(c) => format!("{n}\t{}\t{}{c}\n", l.source, crate::casks::PREFIX),
            None => format!("{n}\t{}\n", l.source),
        })
        .collect();
    crate::store::write_atomic(&folder.join(LIST), text.as_bytes())
}

/// Make `folder` have `apps`, as copies, and none of the others nami put there. `user`: a
/// person's own folder (Launch Services is told at once; /Applications it watches itself).
/// What can't be done is said, and tried again at the next change: it never stops one.
pub fn sync(folder: &Path, apps: &[App], root: &Path, user: bool) -> Result<()> {
    let had = read_list(folder);
    if apps.is_empty() && had.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(folder).with_context(|| format!("can't make {}", folder.display()))?;
    // What an earlier run left half done.
    for e in fs::read_dir(folder)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') && (name.ends_with(".nami-new") || name.ends_with(".nami-old")) {
            let _ = remove(&e.path());
        }
    }
    let mut list = had.clone();
    for app in apps {
        let dest = folder.join(&app.name);
        let there = dest.symlink_metadata().is_ok();
        match had.get(&app.name) {
            Some(from) if from.source == app.source && there => continue,
            None if there => {
                crate::say!("warning: {} isn't nami's copy, so nami leaves it as it is", dest.display());
                continue;
            }
            _ => {}
        }
        let open = there && is_open(&dest);
        match put(root, app, folder, &dest, user) {
            Ok(()) => {
                list.insert(app.name.clone(), Listed { source: app.source.clone(), cask: app.cask.clone() });
                let name = shown(&app.name);
                if !there {
                    crate::say!("Added {name} to Applications.");
                } else if open {
                    crate::say!("Updated {name} in Applications. It's open: quit it and open it again to use the new version.");
                } else {
                    crate::say!("Updated {name} in Applications.");
                }
            }
            Err(e) => crate::say!("warning: couldn't add {} to {}: {e:#}", shown(&app.name), folder.display()),
        }
    }
    for name in had.keys() {
        if apps.iter().any(|a| &a.name == name) {
            continue;
        }
        let dest = folder.join(name);
        if dest.symlink_metadata().is_err() {
            list.remove(name);
            continue;
        }
        unregister(&dest, user);
        match remove(&dest) {
            Ok(()) => {
                list.remove(name);
                crate::say!("Removed {} from Applications.", shown(name));
            }
            Err(e) => crate::say!("warning: couldn't remove {} from {}: {e:#}. You can drag it to the Trash.", shown(name), folder.display()),
        }
    }
    if list != had {
        write_list(folder, &list)?;
    }
    // Nothing of nami's left: nor its list, nor the folder if it's empty now.
    if list.is_empty() {
        let _ = fs::remove_file(folder.join(LIST));
        let _ = fs::remove_dir(folder);
    }
    Ok(())
}

/// Copy `app` to `dest`, beside it first, then in its place (an older copy's taken away).
fn put(root: &Path, app: &App, folder: &Path, dest: &Path, user: bool) -> Result<()> {
    let real = follow(root, Path::new(&app.source)).with_context(|| format!("{} isn't there", app.source))?;
    let new = folder.join(format!(".{}.nami-new", app.name));
    let _ = remove(&new);
    // A cask's app, kept on the Mac's own volume (casks.rs): a clone, which shares its blocks.
    // Otherwise (the store's apps; a home on another volume), a copy.
    let cloned = !real.starts_with("/nix") && clone(&real, &new).is_ok();
    if !cloned {
        let _ = remove(&new);
        if let Err(e) = copy_tree(root, &real, &new, 0) {
            let _ = remove(&new);
            return Err(e);
        }
    }
    // A cask's app, as its developer's download; any other, as nami's own.
    match &app.cask {
        Some(_) => quarantine(&new),
        None => unquarantine(&new),
    }
    if dest.symlink_metadata().is_ok() && swap(&new, dest).is_ok() {
        // The two swapped at once (APFS): never a moment without the app. The old copy's
        // where the new one was.
        let _ = remove(&new);
    } else if dest.symlink_metadata().is_ok() {
        // The old copy aside, the new one in its place.
        let old = folder.join(format!(".{}.nami-old", app.name));
        let _ = remove(&old);
        if let Err(e) = fs::rename(dest, &old) {
            let _ = remove(&new);
            return Err(e).with_context(|| format!("can't move the copy that's there ({})", dest.display()));
        }
        fs::rename(&new, dest)?;
        unregister(&old, user);
        let _ = remove(&old);
    } else {
        fs::rename(&new, dest)?;
    }
    register(dest, user);
    Ok(())
}

/// A bundle cloned whole (macOS's clonefile: the files share their blocks with `src`'s until
/// either changes), owned by whoever clones it; its folders then writable for them, as a
/// copy's are. Fails across volumes (and on Linux), and nothing's left half made.
fn clone(src: &Path, dest: &Path) -> std::io::Result<()> {
    clone_whole(src, dest)?;
    fn writable(p: &Path) {
        let Ok(meta) = p.symlink_metadata() else { return };
        if meta.is_dir() {
            let _ = fs::set_permissions(p, fs::Permissions::from_mode((meta.permissions().mode() & 0o7777) | 0o700));
            for e in fs::read_dir(p).into_iter().flatten().flatten() {
                writable(&e.path());
            }
        }
    }
    writable(dest);
    Ok(())
}

#[cfg(target_os = "macos")]
fn clone_whole(src: &Path, dest: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    // <sys/clonefile.h>: links as links, and the clone the caller's (not the source's owner's).
    const CLONE_NOFOLLOW: u32 = 0x0001;
    const CLONE_NOOWNERCOPY: u32 = 0x0002;
    let (s, d) = (std::ffi::CString::new(src.as_os_str().as_bytes())?, std::ffi::CString::new(dest.as_os_str().as_bytes())?);
    // SAFETY: both are NUL-terminated strings that live through the call.
    if unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), CLONE_NOFOLLOW | CLONE_NOOWNERCOPY) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn clone_whole(_src: &Path, _dest: &Path) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

/// Swap two paths' names in one step (macOS's renamex_np, RENAME_SWAP).
#[cfg(target_os = "macos")]
fn swap(a: &Path, b: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let (a, b) = (std::ffi::CString::new(a.as_os_str().as_bytes())?, std::ffi::CString::new(b.as_os_str().as_bytes())?);
    // SAFETY: both are NUL-terminated strings that live through the call.
    if unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn swap(_a: &Path, _b: &Path) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

/// Copy the folder `src` (a store path's, logical, its links followed) to `dest` on disk:
/// files byte for byte with their modes, links within `src` as links, links out of it as what
/// they lead to. Folders are writable for whoever owns the copy (it can be moved and
/// removed); files stay read-only, as in the store.
fn copy_tree(root: &Path, src: &Path, dest: &Path, depth: u32) -> Result<()> {
    if depth > 16 {
        bail!("{} has links that lead too far", src.display());
    }
    fs::create_dir(dest).with_context(|| format!("can't make {}", dest.display()))?;
    copy_into(root, src, Path::new(""), dest, depth)?;
    let mode = on_disk(root, src).metadata()?.permissions().mode();
    fs::set_permissions(dest, fs::Permissions::from_mode((mode & 0o7777) | 0o700))?;
    Ok(())
}

/// `copy_tree` for `src`'s folder `rel` (a real one, within it).
fn copy_into(root: &Path, src: &Path, rel: &Path, dest: &Path, depth: u32) -> Result<()> {
    let here = src.join(rel);
    let mut entries: Vec<_> = fs::read_dir(on_disk(root, &here)).with_context(|| format!("can't read {}", here.display()))?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name();
        let (to, inner) = (dest.join(rel).join(&name), rel.join(&name));
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            let target = fs::read_link(entry.path())?;
            if target.is_relative() && within(&rel.join(&target)) {
                symlink(&target, &to)?;
                continue;
            }
            let led_to = if target.is_absolute() { target.clone() } else { here.join(&target) };
            match follow(root, &led_to) {
                Some(p) if on_disk(root, &p).is_dir() => copy_tree(root, &p, &to, depth + 1)?,
                Some(p) if on_disk(root, &p).is_file() => copy_file(&on_disk(root, &p), &to)?,
                // (Leading nowhere: as it is.)
                _ => symlink(&target, &to)?,
            }
        } else if kind.is_dir() {
            fs::create_dir(&to)?;
            copy_into(root, src, &inner, dest, depth)?;
            let mode = entry.metadata()?.permissions().mode();
            fs::set_permissions(&to, fs::Permissions::from_mode((mode & 0o7777) | 0o700))?;
        } else {
            copy_file(&entry.path(), &to)?;
        }
    }
    Ok(())
}

fn copy_file(from: &Path, to: &Path) -> Result<()> {
    // (macOS: clonefile where it can, else copyfile, with the file's mode.)
    fs::copy(from, to).with_context(|| format!("can't copy {}", from.display()))?;
    Ok(())
}

/// Whether a relative path stays inside the folder it starts in.
fn within(rel: &Path) -> bool {
    let mut depth = 0i32;
    for c in rel.components() {
        match c {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => return false,
        }
    }
    true
}

/// A store path as it's found on disk: under `root` when nami's store is elsewhere (NAMI_ROOT).
fn on_disk(root: &Path, logical: &Path) -> PathBuf {
    if root == Path::new("/") || !logical.starts_with("/nix") {
        return logical.to_path_buf();
    }
    root.join(logical.strip_prefix("/").unwrap_or(logical))
}

/// `path` with every link in it followed (into the store under `root` too); None if it
/// leads nowhere.
pub fn follow(root: &Path, path: &Path) -> Option<PathBuf> {
    let mut done = PathBuf::from("/");
    let mut rest: VecDeque<PathBuf> = path.components().filter(|c| matches!(c, Component::Normal(_) | Component::ParentDir)).map(|c| PathBuf::from(c.as_os_str())).collect();
    let mut hops = 0;
    while let Some(c) = rest.pop_front() {
        if c == Path::new("..") {
            done.pop();
            continue;
        }
        let next = done.join(&c);
        let meta = fs::symlink_metadata(on_disk(root, &next)).ok()?;
        if !meta.file_type().is_symlink() {
            done = next;
            continue;
        }
        hops += 1;
        if hops > 40 {
            return None;
        }
        let target = fs::read_link(on_disk(root, &next)).ok()?;
        if target.is_absolute() {
            done = PathBuf::from("/");
        }
        for c in target.components().rev() {
            if matches!(c, Component::Normal(_) | Component::ParentDir) {
                rest.push_front(PathBuf::from(c.as_os_str()));
            }
        }
    }
    Some(done)
}

/// Take away a copy (its folders made writable first).
fn remove(path: &Path) -> Result<()> {
    let meta = path.symlink_metadata()?;
    if meta.is_dir() {
        make_owner_writable(path);
        fs::remove_dir_all(path).with_context(|| format!("can't remove {}", path.display()))?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn make_owner_writable(path: &Path) {
    let Ok(meta) = path.symlink_metadata() else { return };
    if meta.is_dir() {
        let mode = meta.permissions().mode();
        if mode & 0o700 != 0o700 {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o700));
        }
        for e in fs::read_dir(path).into_iter().flatten().flatten() {
            make_owner_writable(&e.path());
        }
    }
}

#[cfg(target_os = "macos")]
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/// Tell Launch Services about a new app now (Spotlight, Open With, the Dock), not at its
/// next look. A person's own: /Applications is everyone's, and macOS watches it.
fn register(app: &Path, user: bool) {
    #[cfg(target_os = "macos")]
    if user {
        let _ = std::process::Command::new(LSREGISTER).arg("-f").arg(app).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    }
    let _ = (app, user);
}

/// And that one's going.
fn unregister(app: &Path, user: bool) {
    #[cfg(target_os = "macos")]
    if user {
        let _ = std::process::Command::new(LSREGISTER).arg("-u").arg(app).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    }
    let _ = (app, user);
}

/// A copy made here isn't marked as downloaded; should anything have marked it, the mark goes
/// (Gatekeeper would stop it the first time).
fn unquarantine(app: &Path) {
    #[cfg(target_os = "macos")]
    {
        fn walk(p: &Path) {
            use std::os::unix::ffi::OsStrExt;
            if let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) {
                // SAFETY: both are NUL-terminated strings that live through the call.
                unsafe { libc::removexattr(c.as_ptr(), c"com.apple.quarantine".as_ptr(), libc::XATTR_NOFOLLOW) };
            }
            if p.symlink_metadata().is_ok_and(|m| m.is_dir()) {
                for e in fs::read_dir(p).into_iter().flatten().flatten() {
                    walk(&e.path());
                }
            }
        }
        walk(app);
    }
    let _ = app;
}

/// A cask's app, marked as downloaded (`com.apple.quarantine` on the bundle, as a browser marks
/// a download: casks.rs's QUARANTINE_FLAGS, when, and by whom), so Gatekeeper checks it and macOS
/// asks before it opens the first time, as for any app from the internet. (Homebrew marks its
/// casks' apps the same way.) The bundle alone: Gatekeeper assesses the app as a whole, and its
/// files stay as the store has them (read-only).
fn quarantine(app: &Path) {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let value = format!("{};{:08x};nami;", crate::casks::QUARANTINE_FLAGS, crate::casks::now());
        if let Ok(c) = std::ffi::CString::new(app.as_os_str().as_bytes()) {
            // SAFETY: the name strings are NUL-terminated, the value is value.len() bytes, and all
            // live through the call.
            let rc = unsafe { libc::setxattr(c.as_ptr(), c"com.apple.quarantine".as_ptr(), value.as_ptr().cast(), value.len(), 0, libc::XATTR_NOFOLLOW) };
            if rc != 0 {
                crate::say!("warning: couldn't mark {} as downloaded: {}", app.display(), std::io::Error::last_os_error());
            }
        }
    }
    let _ = app;
}

/// Whether a program from inside `app` is running (its process's executable is in it).
#[cfg(target_os = "macos")]
pub(crate) fn is_open(app: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let mut pids = vec![0 as libc::pid_t; 8192];
    // SAFETY: the buffer is pids.len() pids long, and its size in bytes is what's passed.
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int) };
    let prefix = app.as_os_str().as_bytes();
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    pids.iter().take(n.max(0) as usize).any(|&pid| {
        // SAFETY: proc_pidpath writes at most buf.len() bytes.
        let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
        len > 0 && buf[..len as usize].starts_with(prefix) && buf.get(prefix.len()) == Some(&b'/')
    })
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn is_open(_app: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("nami-mac-apps-{name}-{}", std::process::id()));
        if root.exists() {
            make_owner_writable(&root);
            fs::remove_dir_all(&root).unwrap();
        }
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(p: &Path, text: &str, mode: u32) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
    }

    /// Store paths as nixpkgs has them: alacritty's MacOS folder links out of its bundle, an
    /// app with a framework links inside its own, a wrapper's Applications is the unwrapped
    /// package's, by an absolute link.
    fn store(root: &Path) {
        let s = |p: &str| root.join("nix/store").join(p);
        write(&s("aaa-alacritty-0.17.0/bin/alacritty"), "alacritty 0.17", 0o555);
        write(&s("aaa-alacritty-0.17.0/Applications/Alacritty.app/Contents/Info.plist"), "<plist/>", 0o444);
        symlink("../../../bin", s("aaa-alacritty-0.17.0/Applications/Alacritty.app/Contents/MacOS")).unwrap();
        let fw = "bbb-itsycal-0.15/Applications/Itsycal.app/Contents/Frameworks/K.framework";
        write(&s(&format!("{fw}/Versions/A/K")), "framework", 0o555);
        symlink("A", s(&format!("{fw}/Versions/Current"))).unwrap();
        symlink("Versions/Current/K", s(&format!("{fw}/K"))).unwrap();
        write(&s("bbb-itsycal-0.15/Applications/Itsycal.app/Contents/MacOS/Itsycal"), "itsycal", 0o555);
        write(&s("ccc-firefox-unwrapped-150/Applications/Firefox.app/Contents/MacOS/firefox"), "firefox 150", 0o555);
        fs::create_dir_all(s("ddd-firefox-150")).unwrap();
        symlink("/nix/store/ccc-firefox-unwrapped-150/Applications", s("ddd-firefox-150/Applications")).unwrap();
        write(&s("eee-alacritty-0.18.0/bin/alacritty"), "alacritty 0.18", 0o555);
        write(&s("eee-alacritty-0.18.0/Applications/Alacritty.app/Contents/Info.plist"), "<plist/>", 0o444);
        symlink("../../../bin", s("eee-alacritty-0.18.0/Applications/Alacritty.app/Contents/MacOS")).unwrap();
        write(&s("fff-ripgrep-15/bin/rg"), "rg", 0o555);
        // The store's folders are read-only.
        for e in walk(&root.join("nix/store")) {
            if e.symlink_metadata().unwrap().is_dir() {
                fs::set_permissions(&e, fs::Permissions::from_mode(0o555)).unwrap();
            }
        }
    }

    fn walk(p: &Path) -> Vec<PathBuf> {
        let mut out = vec![p.to_path_buf()];
        if p.symlink_metadata().unwrap().is_dir() {
            for e in fs::read_dir(p).unwrap() {
                out.extend(walk(&e.unwrap().path()));
            }
        }
        out
    }

    fn paths(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| format!("/nix/store/{n}")).collect()
    }

    #[test]
    fn finds_each_packages_apps() {
        let root = scratch("of");
        store(&root);
        let apps = of(&root, &paths(&["fff-ripgrep-15", "aaa-alacritty-0.17.0", "ddd-firefox-150", "bbb-itsycal-0.15", "eee-alacritty-0.18.0"]));
        let got: Vec<(&str, &str)> = apps.iter().map(|a| (a.name.as_str(), a.source.as_str())).collect();
        assert_eq!(got, [
            ("Alacritty.app", "/nix/store/aaa-alacritty-0.17.0/Applications/Alacritty.app"),
            ("Firefox.app", "/nix/store/ddd-firefox-150/Applications/Firefox.app"),
            ("Itsycal.app", "/nix/store/bbb-itsycal-0.15/Applications/Itsycal.app"),
        ]);
        make_owner_writable(&root);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn copies_whole_bundles_and_keeps_them_in_step() {
        let root = scratch("sync");
        store(&root);
        let folder = root.join("home/Applications").join(FOLDER);
        let sync_to = |names: &[&str]| sync(&folder, &of(&root, &paths(names)), &root, true).unwrap();

        // Nothing to add, nothing there: no folder.
        sync_to(&["fff-ripgrep-15"]);
        assert!(!folder.exists());

        sync_to(&["aaa-alacritty-0.17.0", "bbb-itsycal-0.15", "ddd-firefox-150"]);
        // alacritty's MacOS, a link out of the bundle: a real folder in the copy.
        let macos = folder.join("Alacritty.app/Contents/MacOS");
        assert!(macos.symlink_metadata().unwrap().is_dir());
        assert_eq!(fs::read_to_string(macos.join("alacritty")).unwrap(), "alacritty 0.17");
        assert_eq!(fs::metadata(macos.join("alacritty")).unwrap().permissions().mode() & 0o777, 0o555);
        // A framework's links, inside the bundle: links still.
        let fw = folder.join("Itsycal.app/Contents/Frameworks/K.framework");
        assert_eq!(fs::read_link(fw.join("Versions/Current")).unwrap(), Path::new("A"));
        assert_eq!(fs::read_link(fw.join("K")).unwrap(), Path::new("Versions/Current/K"));
        assert_eq!(fs::read_to_string(fw.join("K")).unwrap(), "framework");
        // A wrapper's: the unwrapped app, through its absolute link.
        assert_eq!(fs::read_to_string(folder.join("Firefox.app/Contents/MacOS/firefox")).unwrap(), "firefox 150");
        assert!(!folder.join("Firefox.app").symlink_metadata().unwrap().file_type().is_symlink());
        let list = read_list(&folder);
        assert_eq!(list.get("Alacritty.app").map(|l| l.source.as_str()), Some("/nix/store/aaa-alacritty-0.17.0/Applications/Alacritty.app"));
        assert_eq!(list.len(), 3);

        // A person's own things in the folder, and one named like an app nami would add.
        write(&folder.join("notes.txt"), "mine", 0o644);
        write(&folder.join("Mine.app/Contents/Info.plist"), "mine", 0o644);

        // Unchanged: left as it is (the same file, not a new copy).
        let ino = fs::metadata(macos.join("alacritty")).unwrap().ino();
        sync_to(&["aaa-alacritty-0.17.0", "bbb-itsycal-0.15", "ddd-firefox-150"]);
        assert_eq!(fs::metadata(macos.join("alacritty")).unwrap().ino(), ino);

        // Another version: replaced. Firefox removed. Nothing half done left.
        sync_to(&["eee-alacritty-0.18.0", "bbb-itsycal-0.15"]);
        assert_eq!(fs::read_to_string(macos.join("alacritty")).unwrap(), "alacritty 0.18");
        assert!(!folder.join("Firefox.app").exists());
        let mut names: Vec<String> = fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, [".nami-apps", "Alacritty.app", "Itsycal.app", "Mine.app", "notes.txt"]);

        // Gone by hand: back at the next change.
        remove(&folder.join("Itsycal.app")).unwrap();
        sync_to(&["eee-alacritty-0.18.0", "bbb-itsycal-0.15"]);
        assert!(folder.join("Itsycal.app/Contents/MacOS/Itsycal").is_file());

        // Everything removed: the person's own things stay, and with them the folder.
        sync_to(&["fff-ripgrep-15"]);
        assert!(!folder.join("Alacritty.app").exists() && !folder.join(LIST).exists());
        assert!(folder.join("notes.txt").exists() && folder.join("Mine.app").exists());

        // Without anything of theirs, the folder goes too.
        sync_to(&["aaa-alacritty-0.17.0"]);
        remove(&folder.join("notes.txt")).unwrap();
        remove(&folder.join("Mine.app")).unwrap();
        sync_to(&[]);
        assert!(!folder.exists());
        make_owner_writable(&root);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_app_of_its_own_is_left_alone() {
        let root = scratch("own");
        store(&root);
        let folder = root.join("Applications").join(FOLDER);
        write(&folder.join("Alacritty.app/Contents/Info.plist"), "my own build", 0o644);
        sync(&folder, &of(&root, &paths(&["aaa-alacritty-0.17.0"])), &root, false).unwrap();
        assert_eq!(fs::read_to_string(folder.join("Alacritty.app/Contents/Info.plist")).unwrap(), "my own build");
        assert!(read_list(&folder).is_empty());
        make_owner_writable(&root);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_casks_app_is_listed_as_one() {
        let root = scratch("cask");
        store(&root);
        let folder = root.join("home/Applications").join(FOLDER);
        let mut apps = of(&root, &paths(&["bbb-itsycal-0.15", "aaa-alacritty-0.17.0"]));
        apps.iter_mut().find(|a| a.name == "Itsycal.app").unwrap().cask = Some("itsycal".into());
        sync(&folder, &apps, &root, true).unwrap();
        let text = fs::read_to_string(folder.join(LIST)).unwrap();
        assert!(text.contains("Itsycal.app\t/nix/store/bbb-itsycal-0.15/Applications/Itsycal.app\tcask:itsycal\n"), "{text}");
        assert!(text.contains("Alacritty.app\t/nix/store/aaa-alacritty-0.17.0/Applications/Alacritty.app\n"), "{text}");
        let list = read_list(&folder);
        assert_eq!(list["Itsycal.app"].cask.as_deref(), Some("itsycal"));
        assert_eq!(list["Alacritty.app"].cask, None);
        // The same app again: as it is (a cask's that updated itself isn't replaced).
        let ino = fs::metadata(folder.join("Itsycal.app/Contents/MacOS/Itsycal")).unwrap().ino();
        sync(&folder, &apps, &root, true).unwrap();
        assert_eq!(fs::metadata(folder.join("Itsycal.app/Contents/MacOS/Itsycal")).unwrap().ino(), ino);
        make_owner_writable(&root);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn paths_within_a_folder() {
        assert!(within(Path::new("Versions/Current/K")));
        assert!(within(Path::new("Versions/../K")));
        assert!(!within(Path::new("Contents/../../bin")));
        assert!(!within(Path::new("../bin")));
    }
}
