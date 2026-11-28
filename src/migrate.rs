//! tray's places become nami's (docs/rename-migration.md): `/nix/var/tray`,
//! `/etc/tray`, and each person's `~/.config/tray`, `~/.local/state/tray`,
//! `~/.local/share/tray` and `~/.cache/tray`. Each is renamed in place (one
//! `rename(2)`: atomic, everything in it as it was) and its old name left
//! as a relative symlink to the new one, so an older tray, an older
//! generation's units and init, and the lines tray put in people's shell
//! files all find it where they always did. When a rename can't be done (a
//! mount point), the new name is made a symlink to the old one instead.
//!
//! Since 0.20 nami's own folder is `/nami`, at the top ([`to_top`], [`var_dir`]): moved there
//! from `/nix/var/nami` the same way, its old place a link to it.

use std::path::{Path, PathBuf};

/// What happened to one place.
#[derive(Debug, PartialEq, Eq)]
pub enum Moved {
    /// Nothing of tray's there, or done before.
    Nothing,
    /// Renamed, the old name a symlink to it.
    Renamed,
    /// The new name made a symlink to the old place (it couldn't be renamed).
    Linked,
    /// Both were there: what was only in the old one was moved into the new one.
    Merged { left: Vec<String> },
}

/// Is `p` a real folder (not a symlink to one)?
fn real_dir(p: &Path) -> bool {
    p.symlink_metadata().is_ok_and(|m| m.is_dir())
}

fn there(p: &Path) -> bool {
    p.symlink_metadata().is_ok()
}

/// Put a symlink to `target` at `at`, replacing whatever symlink is there
/// in one step (a new one beside it, renamed over it).
fn link(target: &Path, at: &Path) -> std::io::Result<()> {
    let tmp = at.with_file_name(format!(".{}.nami-link", at.file_name().unwrap_or_default().to_string_lossy()));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp)?;
    std::fs::rename(&tmp, at)
}

/// `parent/old` (a real folder) becomes `parent/new`, with `old -> new`
/// left behind. With `owner`, the symlink is given to that uid and gid
/// (root moving someone's folder).
pub fn move_dir(parent: &Path, old: &str, new: &str, owner: Option<(u32, u32)>) -> std::io::Result<Moved> {
    let (from, to) = (parent.join(old), parent.join(new));
    if !real_dir(&from) {
        return Ok(Moved::Nothing);
    }
    let chown = |p: &Path| {
        if let Some((uid, gid)) = owner {
            let _ = std::os::unix::fs::lchown(p, Some(uid), Some(gid));
        }
    };
    if there(&to) {
        if !to.is_dir() {
            return Ok(Moved::Nothing);
        }
        // Both (something made the new one first): what's only in the old
        // one goes over; what's in both stays where it is, and the old one too.
        let mut left = Vec::new();
        for e in std::fs::read_dir(&from)?.flatten() {
            let dest = to.join(e.file_name());
            if there(&dest) || std::fs::rename(e.path(), &dest).is_err() {
                left.push(e.file_name().to_string_lossy().into_owned());
            }
        }
        if left.is_empty() && std::fs::remove_dir(&from).is_ok() {
            link(Path::new(new), &from)?;
            chown(&from);
        }
        return Ok(Moved::Merged { left });
    }
    match std::fs::rename(&from, &to) {
        Ok(()) => {
            if let Err(e) = link(Path::new(new), &from) {
                // Never leave the old name missing: back where it was.
                let _ = std::fs::rename(&to, &from);
                return Err(e);
            }
            chown(&from);
            Ok(Moved::Renamed)
        }
        Err(_) => {
            link(Path::new(old), &to)?;
            chown(&to);
            Ok(Moved::Linked)
        }
    }
}

/// The computer's: as root, on `/` (`root`), before nami's state is opened.
/// Idempotent and quick when done: a few `lstat`s.
pub fn computer(root: &Path) {
    if !crate::profile::is_root() || std::env::var_os("NAMI_NO_MIGRATE").is_some() {
        return;
    }
    if let Err(e) = computer_at(root) {
        eprintln!("warning: moving tray's files to nami's places: {e}");
    }
}

/// [`computer`] under `root` (a test's folder, or `/`).
pub fn computer_at(root: &Path) -> std::io::Result<()> {
    let var = root.join("nix/var");
    let old_var = var.join("tray");
    if real_dir(&old_var) {
        // tray's lock, held while it moves (the open file is the same one after).
        let lock = std::fs::File::create(old_var.join("lock")).ok();
        if let Some(l) = &lock {
            let _ = l.lock();
        }
        say(&old_var, &var.join("nami"), move_dir(&var, "tray", "nami", None)?);
    }
    // nami's folder at the top, /nami (0.20).
    match to_top(root) {
        Err(e) => eprintln!("warning: moving nami's folder to /nami: {e} (it stays in /{VAR}, and works there)"),
        Ok(Top::Nothing | Top::Linked) => {}
        Ok(Top::Moved) => eprintln!("nami: its folder is /nami now (/{VAR} still reaches it)"),
        Ok(Top::Merged { left }) if left.is_empty() => eprintln!("nami: moved what was in /{VAR} into /nami"),
        Ok(Top::Merged { left }) => eprintln!("nami: /{VAR} and /nami were both there; nami uses /nami. Still only in /{VAR}: {}", left.join(", ")),
    }
    binary(&var_dir(root).join("bin"))?;
    // What namiOS keeps in /var/lib: the backups on this computer, their state,
    // the login screen's, the time zone it found.
    let lib = root.join("var/lib");
    // namiOS's record of the files it copied into /etc (the activate script's
    // $state): in /var/lib/tray, which can't move as a whole (/nix may be
    // mounted from there), so this one folder moves, its old name a link.
    let (old_state, new_state) = (lib.join("tray/etc"), lib.join("nami/etc"));
    if real_dir(&old_state) && !there(&new_state) {
        std::fs::create_dir_all(lib.join("nami"))?;
        std::fs::rename(&old_state, &new_state)?;
        std::os::unix::fs::symlink("../nami/etc", &old_state)?;
        say(&old_state, &new_state, Moved::Renamed);
    }
    for (old, new) in VAR_LIB {
        if real_dir(&lib.join(old)) {
            say(&lib.join(old), &lib.join(new), move_dir(&lib, old, new, None)?);
        }
    }
    // Other Linuxes: `nami` on PATH next to `tray`.
    let local = root.join("usr/local/bin");
    if there(&local.join("tray")) && !there(&local.join("nami")) {
        let _ = std::os::unix::fs::symlink(seen(root).join("bin/nami"), local.join("nami"));
    }
    etc(&root.join("etc"))?;
    system_file(&root.join("etc/nami"));
    Ok(())
}

/// The system's file is system.nami since 0.19: computer.nami (or tray's computer.tray)
/// in `dir` is renamed to it, in one rename(2), everything in it as it was, and its old
/// name left a link to it (an older nami, gone back to, reads it there). What's beside it
/// (nami.lock, a computer.nami.before) stays as it is. In a git checkout (a config repo)
/// it keeps its name, read by it, until it's renamed there. Both there (two files): nami
/// uses system.nami and says so; nothing is moved. Once: after, a few `lstat`s.
fn system_file(dir: &Path) {
    let new = dir.join(crate::config::SYSTEM_FILE);
    if there(&new) || !crate::oldnames::old_paths(&new).iter().any(|o| there(o)) {
        return;
    }
    // (It says what it renamed.)
    let _ = crate::oldnames::for_writing(&new);
}

/// The folders in /var/lib that were tray's, and nami's names for them.
/// (Not /var/lib/tray: an immutable distro's /nix is mounted from there.)
pub const VAR_LIB: &[(&str, &str)] = &[
    ("trayos-backups", "namios-backups"),
    ("tray-backup", "nami-backup"),
    ("trayos", "namios"),
    ("trayos-greeter", "namios-greeter"),
    ("trayos-greeter-walls", "namios-greeter-walls"),
    ("trayos-greeter-look", "namios-greeter-look"),
];

/// In nami's bin folder: the program is `nami`, and `tray` the same one.
/// A `tray` that's a file was put there by something that knows the old
/// name (tray's self-update, an older installer): when it's newer than
/// `nami`, it's the one to keep.
fn binary(bin: &Path) -> std::io::Result<()> {
    let (tray, nami) = (bin.join("tray"), bin.join("nami"));
    let file = |p: &Path| p.symlink_metadata().is_ok_and(|m| m.is_file());
    if !file(&tray) {
        if !there(&tray) && file(&nami) {
            std::os::unix::fs::symlink("nami", &tray)?;
        }
        return Ok(());
    }
    let mtime = |p: &Path| p.symlink_metadata().and_then(|m| m.modified()).ok();
    if !file(&nami) || mtime(&tray) > mtime(&nami) {
        std::fs::rename(&tray, &nami)?;
    } else {
        std::fs::remove_file(&tray)?;
    }
    std::os::unix::fs::symlink("nami", &tray)
}

/// `/etc/tray`: a folder becomes `/etc/nami`; a symlink into
/// `/etc/tray-config` (namiOS's) gets `/etc/nami` pointing to the same
/// place, tray-config itself staying as it is. Either way `/etc/tray -> nami`.
fn etc(etc: &Path) -> std::io::Result<()> {
    let (old, new) = (etc.join("tray"), etc.join("nami"));
    let Ok(meta) = old.symlink_metadata() else { return Ok(()) };
    if meta.is_symlink() {
        let target = std::fs::read_link(&old)?;
        if target == Path::new("nami") {
            return Ok(());
        }
        if !there(&new) {
            std::os::unix::fs::symlink(&target, &new)?;
        }
        if new.is_dir() {
            link(Path::new("nami"), &old)?;
            say(&old, &new, Moved::Renamed);
        }
        return Ok(());
    }
    if meta.is_dir() {
        say(&old, &new, move_dir(etc, "tray", "nami", None)?);
    }
    Ok(())
}

fn say(old: &Path, new: &Path, m: Moved) {
    let (o, n) = (old.display(), new.display());
    match m {
        Moved::Nothing => {}
        Moved::Renamed | Moved::Linked => eprintln!("nami: {o} is now {n} (its old name still works)"),
        Moved::Merged { left } if left.is_empty() => eprintln!("nami: moved what was in {o} into {n}"),
        Moved::Merged { left } => eprintln!("nami: {o} and {n} were both there; nami uses {n}. Still only in {o}: {}", left.join(", ")),
    }
}

/// The folders under a home that were tray's: where they are, their name, nami's name for them.
pub const HOME_DIRS: &[(&str, &str, &str)] = &[
    (".config", "tray", "nami"),
    (".local/state", "tray", "nami"),
    (".local/share", "tray", "nami"),
    (".cache", "tray", "nami"),
    // The island's shelf (files dropped on it, kept a day).
    (".cache", "tray-shelf", "nami-shelf"),
];

/// The person running nami: their folders, as them. Not as root (root's own
/// home is done by [`home`] when root runs nami for itself on `/`).
pub fn person() {
    if std::env::var_os("NAMI_NO_MIGRATE").is_some() || std::env::var_os("NAMI_ROOT").is_some() {
        return;
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_absolute() && h != Path::new("/")) else { return };
    // Only a home that's theirs (not root's own, run with sudo's HOME kept).
    use std::os::unix::fs::MetadataExt;
    let mine = std::fs::metadata(&home).is_ok_and(|m| m.uid() == unsafe { libc::geteuid() });
    if mine {
        home_of(&home, None);
    }
}

/// Someone's folders under `home`; `owner` when root does it for them.
pub fn home_of(home: &Path, owner: Option<(u32, u32)>) {
    for (base, old, new) in HOME_DIRS {
        let parent = home.join(base);
        match move_dir(&parent, old, new, owner) {
            Ok(Moved::Nothing) => {}
            Ok(m) => say(&parent.join(old), &parent.join(new), m),
            Err(e) => eprintln!("warning: moving {} to {}: {e}", parent.join(old).display(), parent.join(new).display()),
        }
    }
}

/// A project's own folder of state (`.tray`, its services' data) is `.nami`
/// now: renamed the first time nami looks (it's never in git).
pub fn project_state(project_dir: &Path) -> PathBuf {
    let (old, new) = (project_dir.join(".tray"), project_dir.join(".nami"));
    if real_dir(&old) && !there(&new) && old.join("..").is_dir() {
        let _ = std::fs::rename(&old, &new);
    }
    new
}

/// nami's own folder (its programs, catalog, profiles, generations, secrets): `/nami`, at
/// the top of the computer. Where `/` can't hold it (read-only, as on Fedora Silverblue; on
/// another file system than /nix, so wiped with `/` on an ephemeral root, or not movable to
/// it) it stays where it was before 0.20, [`VAR`].
pub const TOP: &str = "nami";
/// Where nami's folder was, and is where `/` can't hold it: always reaches it, as the folder
/// itself or a link to `/nami`. (The package store stays /nix/store: cache.nixos.org's signed
/// builds name it.)
pub const VAR: &str = "nix/var/nami";
/// The link left at [`VAR`] when the folder is at [`TOP`]: relative, so it's the same from an
/// initrd's /newroot (an older namiOS generation's boot entry names /nix/var/nami/os/...).
pub const TO_TOP: &str = "../../nami";
/// The link put at [`TOP`] where the folder stays at [`VAR`] (so /nami reaches it too).
pub const TO_VAR: &str = "nix/var/nami";

/// nami's folder under `root` (`/`, or a test's): `/nami`; or `/nix/var/nami` where it is
/// (not moved yet, or `/` can't hold it); or tray's, `/nix/var/tray`, while it hasn't been
/// moved (someone who isn't root, before root's first run of nami: they can't move it).
/// Nothing there yet: where [`make`] would make it. A few `lstat`s.
pub fn var_dir(root: &Path) -> PathBuf {
    let (top, var) = (root.join(TOP), root.join(VAR));
    if is_top(root) {
        return top;
    }
    if there(&var) {
        return var;
    }
    let tray = root.join("nix/var/tray");
    if real_dir(&tray) {
        return tray;
    }
    if real_dir(&top) || top_possible(root) { top } else { var }
}

/// nami's folder as the computer at `root` names it, for what's written onto it (a unit, a
/// launcher, a script): `/nami`, or `/nix/var/nami` where it's there.
pub fn seen(root: &Path) -> PathBuf {
    Path::new("/").join(var_dir(root).strip_prefix(root).unwrap_or(Path::new(VAR)))
}

/// nami's folder on this computer, as it names it: `/nami` (or `/nix/var/nami`, where it's there).
pub fn here() -> PathBuf {
    seen(Path::new("/"))
}

/// Is `root`'s `/nami` nami's folder? It's a real folder, and the old place is a link (to it),
/// or nothing, or a stray folder made while there was nothing there (nami's program is then
/// at /nami: someone's own /nami hasn't got it).
fn is_top(root: &Path) -> bool {
    let (top, var) = (root.join(TOP), root.join(VAR));
    real_dir(&top) && (!real_dir(&var) || top.join("bin/nami").is_file())
}

/// Can `root` have nami's folder at its top? Its `/` can be written, and is on the file system
/// /nix is on (a separate /nix: the folder stays with the store, it can be moved in one step,
/// and an ephemeral `/`, wiped at each start, doesn't take it with it).
fn top_possible(root: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    if there(&root.join(TOP)) || !writable(root) {
        return false;
    }
    match (std::fs::metadata(root), std::fs::metadata(root.join("nix"))) {
        (Ok(r), Ok(n)) => r.dev() == n.dev(),
        // (No /nix yet: it's made on `/`.)
        (Ok(_), Err(_)) => true,
        _ => false,
    }
}

/// Can this process make something in `dir`, on a file system that isn't read-only?
fn writable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return false };
    // SAFETY: a path, and the struct statvfs fills.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    let ro = unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 || s.f_flag & libc::ST_RDONLY != 0;
    // SAFETY: a path.
    !ro && unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0
}

/// Make nami's folder under `root` where [`var_dir`] says, when there's none: `/nami`, with
/// `/nix/var/nami` a link to it; or `/nix/var/nami` where `/` can't hold it. Gives the folder.
pub fn make(root: &Path) -> std::io::Result<PathBuf> {
    let dir = var_dir(root);
    if dir.is_dir() {
        return Ok(dir);
    }
    let var = root.join(VAR);
    if dir == root.join(TOP) {
        match std::fs::create_dir(&dir) {
            Ok(()) | Err(_) if dir.is_dir() => {}
            // (`/` can't have it after all.)
            _ => {
                std::fs::create_dir_all(&var)?;
                return Ok(var);
            }
        }
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
        if std::fs::create_dir_all(root.join("nix/var")).is_ok() && !there(&var) {
            let _ = std::os::unix::fs::symlink(TO_TOP, &var);
        }
        return Ok(dir);
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// What [`to_top`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Top {
    /// Nothing to do: at the top already, nothing installed, or someone's own `/nami` there.
    Nothing,
    /// Moved to `/nami`, `/nix/var/nami` a link to it.
    Moved,
    /// It can't be moved (another file system than `/`): `/nami` a link to it.
    Linked,
    /// Moved before, and a folder made at the old place since: what was only in it moved in.
    Merged { left: Vec<String> },
}

/// `/nix/var/nami` becomes `/nami`, in one step, once, as root: everything in it as it was,
/// and the old place a relative link to it, so what names it (an older nami, a running
/// daemon, older namiOS generations' boot entries, units, people's PATH) finds it there.
///
/// renameat2's RENAME_EXCHANGE swaps the folder with a link to it put at `/nami` first, so
/// `/nix/var/nami` is never missing, not even for a moment: a nami running all along (0.19's
/// daemon) reaches the same files through it, its open files and socket as they were. Two
/// namis at once: one moves it (the other waits on /nix/var's lock, then finds it moved). A
/// folder made at the old place anyway (by something that removed the link) is merged in.
pub fn to_top(root: &Path) -> std::io::Result<Top> {
    let (top, var) = (root.join(TOP), root.join(VAR));
    if is_top(root) {
        if real_dir(&var) {
            return merge_into(&var, &top);
        }
        if !there(&var) && root.join("nix/var").is_dir() {
            std::os::unix::fs::symlink(TO_TOP, &var)?;
        }
        return Ok(Top::Nothing);
    }
    if !real_dir(&var) {
        return Ok(Top::Nothing);
    }
    // At /nami: nothing, or a link of ours. Anything else is someone's own: it stays.
    let ours = |t: PathBuf| t == Path::new(TO_VAR) || t == Path::new(TO_TOP);
    if top.symlink_metadata().is_ok() && !std::fs::read_link(&top).is_ok_and(ours) {
        return Ok(Top::Nothing);
    }
    if !movable(root) {
        return Ok(link_up(root));
    }
    // One at a time (the lock on /nix/var, which doesn't move): the next finds it moved.
    let lock = std::fs::File::open(root.join("nix/var"))?;
    lock.lock()?;
    if !real_dir(&var) || is_top(root) {
        return Ok(Top::Nothing);
    }
    let was_link = there(&top);
    if !was_link {
        // (A link to itself until the swap: nothing used /nami before.)
        std::os::unix::fs::symlink(TO_TOP, &top)?;
    }
    match exchange(&var, &top) {
        Ok(()) => {
            if was_link {
                // The swapped link pointed down: now at the old place, it points up.
                link(Path::new(TO_TOP), &var)?;
            }
            Ok(Top::Moved)
        }
        // A file system without the swap: renamed, then the link (a moment without it).
        Err(e) if matches!(e.raw_os_error(), Some(libc::EINVAL) | Some(libc::ENOSYS)) => {
            std::fs::remove_file(&top)?;
            crate::oldnames::move_once(&var, &top)?;
            match std::os::unix::fs::symlink(TO_TOP, &var) {
                Ok(()) => Ok(Top::Moved),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => merge_into(&var, &top),
                Err(e) => Err(e),
            }
        }
        // Can't be moved (a mount point, another file system): it stays, /nami a link to it.
        Err(_) => {
            link(Path::new(TO_VAR), &top)?;
            Ok(Top::Linked)
        }
    }
}

/// Can `root`'s nami folder move to its top: `/` writable, and on the folder's file system?
fn movable(root: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    writable(root)
        && match (std::fs::metadata(root), std::fs::metadata(root.join(VAR))) {
            (Ok(r), Ok(v)) => r.dev() == v.dev(),
            _ => false,
        }
}

/// The folder stays at the old place: `/nami` a link to it, where `/` takes one.
fn link_up(root: &Path) -> Top {
    let top = root.join(TOP);
    if std::fs::read_link(&top).is_ok_and(|t| t == Path::new(TO_VAR)) || !writable(root) {
        return Top::Nothing;
    }
    match link(Path::new(TO_VAR), &top) {
        Ok(()) => Top::Linked,
        Err(_) => Top::Nothing,
    }
}

/// renameat2(RENAME_EXCHANGE): `a` and `b` swap places, in one step.
fn exchange(a: &Path, b: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let (a, b) = (std::ffi::CString::new(a.as_os_str().as_bytes())?, std::ffi::CString::new(b.as_os_str().as_bytes())?);
        const RENAME_EXCHANGE: libc::c_uint = 2;
        // SAFETY: two paths.
        let r = unsafe { libc::syscall(libc::SYS_renameat2, libc::AT_FDCWD, a.as_ptr(), libc::AT_FDCWD, b.as_ptr(), RENAME_EXCHANGE) };
        if r == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }
    // A Mac's: renameatx_np(RENAME_SWAP), APFS's. (Its own folder never moves, as its `/` is
    // read-only: this is for what's under a test's root.)
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let (a, b) = (std::ffi::CString::new(a.as_os_str().as_bytes())?, std::ffi::CString::new(b.as_os_str().as_bytes())?);
        // SAFETY: two paths.
        let r = unsafe { libc::renameatx_np(libc::AT_FDCWD, a.as_ptr(), libc::AT_FDCWD, b.as_ptr(), libc::RENAME_SWAP) };
        if r == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        // (A file system without the swap: as Linux's says it.)
        Err(if e.raw_os_error() == Some(libc::ENOTSUP) { std::io::Error::from_raw_os_error(libc::ENOSYS) } else { e })
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (a, b);
        Err(std::io::Error::from_raw_os_error(libc::ENOSYS))
    }
}

/// A stray folder at the old place, `from`, and nami's at `to`: what's only in the stray one
/// moves in (what's in both stays where it is: nami's is the one used); emptied, it becomes
/// the link again.
fn merge_into(from: &Path, to: &Path) -> std::io::Result<Top> {
    let mut left = Vec::new();
    for e in std::fs::read_dir(from)?.flatten() {
        let (src, dest) = (e.path(), to.join(e.file_name()));
        if real_dir(&src) && real_dir(&dest) {
            // (Both have db/: the stray one's entries go over, one by one.)
            if let Top::Merged { left: l } = merge_into(&src, &dest)? {
                if !l.is_empty() {
                    left.push(e.file_name().to_string_lossy().into_owned());
                }
            }
            continue;
        }
        if there(&dest) || crate::oldnames::move_once(&src, &dest).is_err() {
            // An empty file made in the moment (a lock): nothing's lost.
            if src.symlink_metadata().is_ok_and(|m| m.is_file() && m.len() == 0) && std::fs::remove_file(&src).is_ok() {
                continue;
            }
            left.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    if left.is_empty() && std::fs::remove_dir(from).is_ok() && from.ends_with(VAR) {
        link(Path::new(TO_TOP), from)?;
    }
    Ok(Top::Merged { left })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nami-migrate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // (As `pwd -P` says it, as uninstall.sh finds it: a Mac's temp_dir is under /var, a link
        // to /private/var.)
        std::fs::canonicalize(&d).unwrap()
    }

    /// Root's commands run side by side (the daemon's, an update, a service), each moving
    /// the system's file as it starts: it's moved once, and never lost. A rename(2) let the
    /// second put the old name, by then a link to system.nami, over the file.
    #[test]
    fn the_system_file_moves_once_with_namis_side_by_side() {
        for round in 0..100 {
            for tray_link in [false, true] {
                let r = tmp(&format!("side-by-side-{round}-{tray_link}"));
                let etc = r.join("etc/nami");
                std::fs::create_dir_all(&etc).unwrap();
                std::fs::write(etc.join("computer.nami"), "packages: [htop]\n").unwrap();
                // (A computer from tray's days has its oldest name too, a link.)
                if tray_link {
                    std::os::unix::fs::symlink("computer.nami", etc.join("computer.tray")).unwrap();
                }
                let namis: Vec<_> = (0..4).map(|_| {
                    let etc = etc.clone();
                    std::thread::spawn(move || system_file(&etc))
                }).collect();
                for n in namis {
                    n.join().unwrap();
                }
                let file = etc.join("system.nami");
                assert!(file.symlink_metadata().unwrap().is_file(), "round {round}: system.nami is the file");
                assert_eq!(std::fs::read_to_string(&file).unwrap(), "packages: [htop]\n", "round {round}");
                assert_eq!(std::fs::read_link(etc.join("computer.nami")).unwrap(), Path::new("system.nami"));
                assert_eq!(std::fs::read_to_string(etc.join("computer.nami")).unwrap(), "packages: [htop]\n", "the old name reads it");
                let _ = std::fs::remove_dir_all(&r);
            }
        }
    }

    /// computer.nami becomes system.nami once, as it was, with what's beside it left as it
    /// is; never with both, never in a git checkout; a link is moved as a link.
    #[test]
    fn the_system_file_becomes_system_nami() {
        let r = tmp("system-file");
        let etc = r.join("etc/nami");
        std::fs::create_dir_all(&etc).unwrap();
        let text = "hostname: pc\npackages:\n  - htop\nusers:\n  ada: { admin: true }\n";
        std::fs::write(etc.join("computer.nami"), text).unwrap();
        std::fs::write(etc.join("nami.lock"), "{\"packages\": {}}").unwrap();
        std::fs::write(etc.join("computer.nami.before"), "packages: []\n").unwrap();
        system_file(&etc);
        assert_eq!(std::fs::read_to_string(etc.join("system.nami")).unwrap(), text);
        assert_eq!(std::fs::read_link(etc.join("computer.nami")).unwrap(), Path::new("system.nami"));
        assert_eq!(std::fs::read_to_string(etc.join("computer.nami")).unwrap(), text, "an older nami reads it by its old name");
        assert!(etc.join("nami.lock").is_file() && etc.join("computer.nami.before").is_file());
        assert_eq!(crate::config::system_file_in(&r), etc.join("system.nami"));
        // Again: nothing to do, nothing changed.
        system_file(&etc);
        assert_eq!(std::fs::read_to_string(etc.join("system.nami")).unwrap(), text);
        // tray's name, alone: the same.
        let t = r.join("tray");
        std::fs::create_dir_all(&t).unwrap();
        std::fs::write(t.join("computer.tray"), text).unwrap();
        system_file(&t);
        assert_eq!(std::fs::read_to_string(t.join("system.nami")).unwrap(), text);
        // Both, two files: nothing moves, nothing's lost; system.nami is read.
        let b = r.join("both");
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("computer.nami"), "packages: [old]\n").unwrap();
        std::fs::write(b.join("system.nami"), "packages: [new]\n").unwrap();
        system_file(&b);
        assert_eq!(std::fs::read_to_string(b.join("computer.nami")).unwrap(), "packages: [old]\n");
        assert_eq!(crate::oldnames::find_in(&b, "system.nami"), b.join("system.nami"));
        // A link (to a config repo's copy): moved as the link, pointing where it did.
        let l = r.join("link");
        std::fs::create_dir_all(l.join("repo/pc")).unwrap();
        std::fs::write(l.join("repo/pc/computer.nami"), text).unwrap();
        std::fs::create_dir_all(l.join("etc")).unwrap();
        std::os::unix::fs::symlink("../repo/pc/computer.nami", l.join("etc/computer.nami")).unwrap();
        system_file(&l.join("etc"));
        assert_eq!(std::fs::read_link(l.join("etc/system.nami")).unwrap(), Path::new("../repo/pc/computer.nami"));
        assert_eq!(std::fs::read_to_string(l.join("etc/system.nami")).unwrap(), text);
        // In a git checkout (a config repo /etc/nami links to): it keeps its name.
        let g = r.join("git");
        std::fs::create_dir_all(g.join(".git")).unwrap();
        std::fs::create_dir_all(g.join("pc")).unwrap();
        std::fs::write(g.join("pc/computer.nami"), text).unwrap();
        std::os::unix::fs::symlink(g.join("pc"), r.join("etc-to-repo")).unwrap();
        system_file(&r.join("etc-to-repo"));
        assert!(!there(&g.join("pc/system.nami")) && g.join("pc/computer.nami").is_file());
        assert_eq!(crate::oldnames::find_in(&r.join("etc-to-repo"), "system.nami"), r.join("etc-to-repo/computer.nami"));
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn a_folder_moves_and_its_old_name_still_reaches_it() {
        let d = tmp("move");
        std::fs::create_dir_all(d.join("tray/profiles")).unwrap();
        std::os::unix::fs::symlink("/nix/store/abc-profile", d.join("tray/profiles/default-3-link")).unwrap();
        std::fs::write(d.join("tray/lock"), "").unwrap();
        assert_eq!(move_dir(&d, "tray", "nami", None).unwrap(), Moved::Renamed);
        assert!(real_dir(&d.join("nami")));
        assert_eq!(std::fs::read_link(d.join("tray")).unwrap(), Path::new("nami"), "relative: the same from an initrd's /sysroot");
        assert_eq!(std::fs::read_link(d.join("tray/profiles/default-3-link")).unwrap(), Path::new("/nix/store/abc-profile"));
        assert!(d.join("tray/lock").is_file());
        // Again: nothing to do.
        assert_eq!(move_dir(&d, "tray", "nami", None).unwrap(), Moved::Nothing);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn both_there_merges_what_isnt_in_both() {
        let d = tmp("merge");
        std::fs::create_dir_all(d.join("tray/profile")).unwrap();
        std::fs::write(d.join("tray/settings.json"), "old").unwrap();
        std::fs::create_dir_all(d.join("nami")).unwrap();
        std::fs::write(d.join("nami/settings.json"), "new").unwrap();
        assert_eq!(move_dir(&d, "tray", "nami", None).unwrap(), Moved::Merged { left: vec!["settings.json".into()] });
        assert!(d.join("nami/profile").is_dir() && real_dir(&d.join("tray")));
        assert_eq!(std::fs::read_to_string(d.join("nami/settings.json")).unwrap(), "new");
        std::fs::remove_file(d.join("tray/settings.json")).unwrap();
        assert_eq!(move_dir(&d, "tray", "nami", None).unwrap(), Moved::Merged { left: vec![] });
        assert_eq!(std::fs::read_link(d.join("tray")).unwrap(), Path::new("nami"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_computer_moves_its_var_bin_and_etc() {
        let r = tmp("computer");
        std::fs::create_dir_all(r.join("nix/var/tray/bin")).unwrap();
        std::fs::create_dir_all(r.join("nix/var/tray/os")).unwrap();
        std::os::unix::fs::symlink("/nix/store/x-system", r.join("nix/var/tray/os/system-142")).unwrap();
        std::fs::write(r.join("nix/var/tray/bin/tray"), "program").unwrap();
        std::fs::create_dir_all(r.join("var/lib/trayos-backups/ada/repo")).unwrap();
        std::fs::create_dir_all(r.join("var/lib/tray/etc/sudoers.d")).unwrap();
        std::fs::create_dir_all(r.join("var/lib/tray/nix/store")).unwrap();
        std::fs::create_dir_all(r.join("etc/tray-config/pc")).unwrap();
        std::fs::write(r.join("etc/tray-config/pc/computer.tray"), "hostname: pc\n").unwrap();
        std::os::unix::fs::symlink("tray-config/pc", r.join("etc/tray")).unwrap();
        std::fs::create_dir_all(r.join("usr/local/bin")).unwrap();
        std::os::unix::fs::symlink("/nix/var/tray/bin/tray", r.join("usr/local/bin/tray")).unwrap();
        computer_at(&r).unwrap();
        let var = r.join("nix/var");
        assert_eq!(std::fs::read_link(var.join("tray")).unwrap(), Path::new("nami"));
        assert_eq!(std::fs::read_to_string(var.join("nami/bin/nami")).unwrap(), "program");
        assert_eq!(std::fs::read_link(var.join("nami/bin/tray")).unwrap(), Path::new("nami"));
        assert_eq!(std::fs::read_to_string(var.join("tray/bin/tray")).unwrap(), "program", "the old path runs nami");
        assert!(var.join("tray/os/system-142").symlink_metadata().is_ok(), "old generations where they were");
        assert!(r.join("var/lib/namios-backups/ada/repo").is_dir() && r.join("var/lib/trayos-backups/ada/repo").is_dir());
        assert!(r.join("var/lib/nami/etc/sudoers.d").is_dir() && std::fs::read_link(r.join("var/lib/tray/etc")).unwrap() == Path::new("../nami/etc"), "the record of /etc's copies moves");
        assert!(real_dir(&r.join("var/lib/tray/nix/store")), "an immutable distro's /nix stays where it's mounted from");
        assert_eq!(std::fs::read_link(r.join("etc/nami")).unwrap(), Path::new("tray-config/pc"));
        assert_eq!(std::fs::read_link(r.join("etc/tray")).unwrap(), Path::new("nami"));
        assert!(r.join("etc/tray/computer.tray").is_file() && r.join("etc/nami/computer.tray").is_file());
        assert!(r.join("etc/tray-config/pc").is_dir(), "tray-config stays");
        assert_eq!(std::fs::read_link(r.join("usr/local/bin/nami")).unwrap(), Path::new("/nami/bin/nami"));
        assert!(real_dir(&r.join("nami/os")) && std::fs::read_link(var.join("nami")).unwrap() == Path::new(TO_TOP), "and at the top");
        // Done again, as every run as root does: nothing changes.
        computer_at(&r).unwrap();
        assert_eq!(std::fs::read_link(r.join("etc/tray")).unwrap(), Path::new("nami"));
        // An older updater puts a new program at the old name: it becomes nami.
        std::fs::remove_file(var.join("nami/bin/tray")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(var.join("nami/bin/tray"), "newer").unwrap();
        computer_at(&r).unwrap();
        assert_eq!(std::fs::read_to_string(var.join("nami/bin/nami")).unwrap(), "newer");
        assert_eq!(std::fs::read_link(var.join("nami/bin/tray")).unwrap(), Path::new("nami"));
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn an_etc_folder_is_renamed() {
        let r = tmp("etc");
        std::fs::create_dir_all(r.join("etc/tray")).unwrap();
        std::fs::write(r.join("etc/tray/computer.tray"), "packages: [jq]\n").unwrap();
        computer_at(&r).unwrap();
        assert!(real_dir(&r.join("etc/nami")) && r.join("etc/nami/computer.tray").is_file());
        assert_eq!(std::fs::read_link(r.join("etc/tray")).unwrap(), Path::new("nami"));
        assert!(!r.join("nix/var/nami").exists(), "no var to move: none made");
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn a_home_moves() {
        let h = tmp("home");
        std::fs::create_dir_all(h.join(".local/state/tray/profile")).unwrap();
        std::fs::create_dir_all(h.join(".config/tray")).unwrap();
        std::fs::write(h.join(".config/tray/env.sh"), "export A=b\n").unwrap();
        home_of(&h, None);
        for base in [".local/state", ".config"] {
            assert_eq!(std::fs::read_link(h.join(base).join("tray")).unwrap(), Path::new("nami"));
        }
        assert!(h.join(".config/tray/env.sh").is_file(), "what tray put in .bashrc still reads it");
        assert!(!h.join(".cache/nami").exists() && !h.join(".cache/tray").exists());
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn var_falls_back_to_trays_until_moved() {
        let r = tmp("var");
        assert_eq!(var_dir(&r), r.join("nami"), "a new computer's: at the top");
        std::fs::create_dir_all(r.join("nix/var/tray")).unwrap();
        assert_eq!(var_dir(&r), r.join("nix/var/tray"));
        computer_at(&r).unwrap();
        assert_eq!(var_dir(&r), r.join("nami"));
        let _ = std::fs::remove_dir_all(&r);
    }

    /// A computer with nami's folder in /nix/var/nami (0.19's): everything in it the same.
    fn installed(name: &str) -> PathBuf {
        let r = tmp(name);
        let var = r.join(VAR);
        for d in ["bin", "db", "os", "profiles/per-user/ada"] {
            std::fs::create_dir_all(var.join(d)).unwrap();
        }
        std::fs::write(var.join("bin/nami"), "program").unwrap();
        std::fs::write(var.join("db/abc-hello.json"), "{}").unwrap();
        std::fs::write(var.join("lock"), "").unwrap();
        std::os::unix::fs::symlink("/nix/store/x-system", var.join("os/system-7")).unwrap();
        std::os::unix::fs::symlink("nami", r.join("nix/var/tray")).unwrap();
        r
    }

    fn moved(r: &Path) {
        assert!(real_dir(&r.join("nami")), "the folder is at the top");
        assert_eq!(std::fs::read_link(r.join(VAR)).unwrap(), Path::new(TO_TOP), "the old place a relative link to it");
        assert_eq!(std::fs::read_to_string(r.join("nami/bin/nami")).unwrap(), "program");
        assert_eq!(std::fs::read_to_string(r.join(VAR).join("bin/nami")).unwrap(), "program", "the old place reaches it");
        assert_eq!(std::fs::read_to_string(r.join("nix/var/tray/bin/nami")).unwrap(), "program", "tray's name too");
        assert_eq!(std::fs::read_link(r.join("nami/os/system-7")).unwrap(), Path::new("/nix/store/x-system"));
        assert!(r.join("nami/db/abc-hello.json").is_file());
        assert_eq!(var_dir(r), r.join("nami"));
        assert_eq!(seen(r), Path::new("/nami"));
    }

    #[test]
    fn the_folder_moves_to_the_top_once() {
        let r = installed("top");
        assert_eq!(var_dir(&r), r.join(VAR), "where it is until it moves");
        assert_eq!(seen(&r), Path::new("/nix/var/nami"));
        // A file open in it (a daemon's) is the same file after.
        let open = std::fs::File::open(r.join(VAR).join("db/abc-hello.json")).unwrap();
        assert_eq!(to_top(&r).unwrap(), Top::Moved);
        moved(&r);
        use std::os::unix::fs::MetadataExt;
        assert_eq!(open.metadata().unwrap().ino(), std::fs::metadata(r.join("nami/db/abc-hello.json")).unwrap().ino());
        // Again (every run as root): nothing.
        assert_eq!(to_top(&r).unwrap(), Top::Nothing);
        computer_at(&r).unwrap();
        moved(&r);
        // An older generation's boot entry names /nix/var/nami/os/system-7: from an initrd's
        // /newroot, the link reaches it there.
        let initrd = tmp("top-initrd");
        std::os::unix::fs::symlink(&r, initrd.join("newroot")).unwrap();
        assert_eq!(std::fs::canonicalize(initrd.join("newroot/nix/var/nami/os")).unwrap(), std::fs::canonicalize(r.join("nami/os")).unwrap());
        let _ = std::fs::remove_dir_all(&initrd);
        let _ = std::fs::remove_dir_all(&r);
    }

    /// Root's commands run side by side (the daemon, an update, a service), each moving it as it
    /// starts: it's moved once, never swapped back, and nothing's lost.
    #[test]
    fn the_folder_moves_once_with_namis_side_by_side() {
        for round in 0..30 {
            let r = installed(&format!("top-race-{round}"));
            let namis: Vec<_> = (0..8)
                .map(|_| {
                    let r = r.clone();
                    std::thread::spawn(move || to_top(&r).unwrap())
                })
                .collect();
            let done: Vec<Top> = namis.into_iter().map(|n| n.join().unwrap()).collect();
            assert_eq!(done.iter().filter(|d| **d == Top::Moved).count(), 1, "round {round}: {done:?}");
            assert!(done.iter().all(|d| matches!(d, Top::Moved | Top::Nothing)), "round {round}: {done:?}");
            moved(&r);
            let _ = std::fs::remove_dir_all(&r);
        }
    }

    /// Moved, and something made a folder at the old place since (the link gone): what's only
    /// in it goes in, what's in both stays, and the old place is the link again once it's empty.
    #[test]
    fn a_stray_folder_at_the_old_place_is_merged() {
        let r = installed("top-split");
        to_top(&r).unwrap();
        let var = r.join(VAR);
        std::fs::remove_file(&var).unwrap();
        std::fs::create_dir_all(var.join("db")).unwrap();
        std::fs::create_dir_all(var.join("profiles/per-user")).unwrap();
        std::fs::write(var.join("lock"), "").unwrap();
        std::fs::write(var.join("db/def-jq.json"), "{}").unwrap();
        std::fs::write(var.join("updated"), "1").unwrap();
        assert_eq!(var_dir(&r), r.join("nami"), "nami's own is the one used, meanwhile");
        assert_eq!(to_top(&r).unwrap(), Top::Merged { left: vec![] });
        moved(&r);
        assert!(r.join("nami/db/def-jq.json").is_file() && r.join("nami/db/abc-hello.json").is_file());
        assert_eq!(std::fs::read_to_string(r.join("nami/updated")).unwrap(), "1");
        // Something in both, different: it stays where it was, and is said.
        std::fs::remove_file(&var).unwrap();
        std::fs::create_dir_all(var.join("bin")).unwrap();
        std::fs::write(var.join("bin/nami"), "other").unwrap();
        assert_eq!(to_top(&r).unwrap(), Top::Merged { left: vec!["bin".into()] });
        assert_eq!(std::fs::read_to_string(r.join("nami/bin/nami")).unwrap(), "program");
        assert_eq!(var_dir(&r), r.join("nami"));
        let _ = std::fs::remove_dir_all(&r);
    }

    /// The link gone (someone's `rm`): put back.
    #[test]
    fn a_missing_link_is_put_back() {
        let r = installed("top-link");
        to_top(&r).unwrap();
        std::fs::remove_file(r.join(VAR)).unwrap();
        assert_eq!(var_dir(&r), r.join("nami"));
        assert_eq!(to_top(&r).unwrap(), Top::Nothing);
        moved(&r);
        let _ = std::fs::remove_dir_all(&r);
    }

    /// Someone's own /nami (a folder, a file, a link elsewhere): left as it is, nami where it was.
    #[test]
    fn someones_own_nami_stays() {
        for theirs in ["dir", "file", "link"] {
            let r = installed(&format!("top-theirs-{theirs}"));
            let top = r.join("nami");
            match theirs {
                "dir" => std::fs::create_dir_all(top.join("notes")).unwrap(),
                "file" => std::fs::write(&top, "mine").unwrap(),
                _ => std::os::unix::fs::symlink("/home/ada/nami", &top).unwrap(),
            }
            assert_eq!(to_top(&r).unwrap(), Top::Nothing, "{theirs}");
            assert!(real_dir(&r.join(VAR)), "{theirs}");
            assert_eq!(var_dir(&r), r.join(VAR), "{theirs}");
            assert_eq!(seen(&r), Path::new("/nix/var/nami"));
            let _ = std::fs::remove_dir_all(&r);
        }
    }

    /// uninstall.sh takes nami's folder and its other names, wherever it is; someone's own /nami stays.
    #[test]
    fn the_uninstaller_takes_the_folder_and_its_links() {
        let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("uninstall.sh")).unwrap();
        let line = |start: &str| text.lines().find(|l| l.starts_with(start)).unwrap_or_else(|| panic!("uninstall.sh: no {start}")).to_string();
        let (places, found) = (line("top=/nami "), line("if V=$(cd \"$var\""));
        assert_eq!(places, "top=/nami var=/nix/var/nami tray=/nix/var/tray");
        let block = &text[text.find("case $V in").unwrap()..text.find("# (end of nami's folder)").unwrap()];
        let run = |r: &Path| {
            let places = format!("top={0}/nami var={0}/nix/var/nami tray={0}/nix/var/tray", r.display());
            let ok = std::process::Command::new("sh").arg("-euc").arg(format!("{places}\n{found}\n{block}")).status().unwrap();
            assert!(ok.success());
        };
        let gone = |r: &Path, what: &str| {
            for p in ["nami", VAR, "nix/var/tray"] {
                assert!(!there(&r.join(p)), "{what}: {p} is still there");
            }
            assert!(r.join("nix/store").is_dir() && r.join("nix/var/nix").is_dir(), "{what}: the store, and Nix's, stay");
        };
        // Moved to /nami (or a new computer's).
        let r = installed("uninstall-top");
        std::fs::create_dir_all(r.join("nix/store")).unwrap();
        std::fs::create_dir_all(r.join("nix/var/nix")).unwrap();
        to_top(&r).unwrap();
        run(&r);
        gone(&r, "at the top");
        let _ = std::fs::remove_dir_all(&r);
        // Where it stayed (another file system than /): /nami a link to it.
        let r = installed("uninstall-var");
        std::fs::create_dir_all(r.join("nix/store")).unwrap();
        std::fs::create_dir_all(r.join("nix/var/nix")).unwrap();
        std::os::unix::fs::symlink(TO_VAR, r.join("nami")).unwrap();
        run(&r);
        gone(&r, "at the old place");
        let _ = std::fs::remove_dir_all(&r);
        // Someone's own /nami: it stays.
        let r = installed("uninstall-theirs");
        std::fs::create_dir_all(r.join("nami/notes")).unwrap();
        run(&r);
        assert!(!there(&r.join(VAR)) && r.join("nami/notes").is_dir());
        let _ = std::fs::remove_dir_all(&r);
    }

    /// Nothing installed: nothing made.
    #[test]
    fn nothing_installed_nothing_moved() {
        let r = tmp("top-none");
        assert_eq!(to_top(&r).unwrap(), Top::Nothing);
        assert!(!there(&r.join("nami")) && !there(&r.join(VAR)));
        let _ = std::fs::remove_dir_all(&r);
    }

    /// A new computer's: /nami, with /nix/var/nami a link to it; where `/` can't be written, the
    /// old place, and nothing at the top.
    #[test]
    fn a_new_folder_is_made_where_it_can_be() {
        let r = tmp("make");
        assert_eq!(make(&r).unwrap(), r.join("nami"));
        assert!(real_dir(&r.join("nami")));
        assert_eq!(std::fs::read_link(r.join(VAR)).unwrap(), Path::new(TO_TOP));
        assert_eq!(make(&r).unwrap(), r.join("nami"), "again: the same");
        assert_eq!(var_dir(&r), r.join("nami"));
        let _ = std::fs::remove_dir_all(&r);
        // (Root writes anywhere.)
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        // A read-only `/` (Silverblue): /nix a mount that can be written.
        let ro = tmp("make-ro");
        std::fs::create_dir_all(ro.join("nix/var")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        assert_eq!(var_dir(&ro), ro.join(VAR));
        assert_eq!(make(&ro).unwrap(), ro.join(VAR));
        assert!(real_dir(&ro.join(VAR)) && !there(&ro.join("nami")));
        assert_eq!(seen(&ro), Path::new("/nix/var/nami"));
        // Installed there, it stays there: it can't move.
        std::fs::create_dir_all(ro.join(VAR).join("bin")).unwrap();
        assert_eq!(to_top(&ro).unwrap(), Top::Nothing);
        assert!(real_dir(&ro.join(VAR)) && !there(&ro.join("nami")));
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&ro);
    }

    #[test]
    fn a_projects_state_is_renamed() {
        let p = tmp("project");
        std::fs::create_dir_all(p.join(".tray/postgres")).unwrap();
        assert_eq!(project_state(&p), p.join(".nami"));
        assert!(p.join(".nami/postgres").is_dir() && !p.join(".tray").exists());
        let _ = std::fs::remove_dir_all(&p);
    }
}
