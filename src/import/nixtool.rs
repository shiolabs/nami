//! Which Nix `nami import` evaluates flakes with. The machine's own when it
//! has one; else nami fetches Nix from the catalog (once, like any package)
//! and gives it a store of its own in ~/.cache/nami/import-nix, so a flake
//! imports the same with or without Nix installed. Nix there sees its store
//! as /nix/store (a "chroot store"): paths it prints are read through
//! [`real`].

use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Fetches Nix: its `nix`, and a CA bundle when the computer has none.
type Fetch = Box<dyn FnOnce() -> Result<(PathBuf, Option<PathBuf>), String> + Send>;

static FETCH: Mutex<Option<Fetch>> = Mutex::new(None);
static NIX: OnceLock<Option<Nix>> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct Nix {
    pub bin: PathBuf,
    /// The private store's root (nami's own Nix), or None (the machine's).
    pub root: Option<PathBuf>,
    /// Certificates fetched with it, for a computer without any (a bare
    /// container): without them Nix can't download a flake's inputs.
    pub certs: Option<PathBuf>,
}

/// How nami gets Nix when the machine has none: fetch it (unless it's
/// there from last time), return the `nix` binary (and certificates, when
/// the computer has none). Set by main, which has the store.
pub fn set_fetch(f: impl FnOnce() -> Result<(PathBuf, Option<PathBuf>), String> + Send + 'static) {
    *FETCH.lock().unwrap() = Some(Box::new(f));
}

fn on_path(cmd: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(cmd)).find(|p| p.is_file())
}

/// NAMI_IMPORT_NIX: 0 never uses Nix; a path is the `nix` to run with a
/// private store (as when nami fetched it).
fn choose() -> Option<Nix> {
    match std::env::var("NAMI_IMPORT_NIX").ok().as_deref() {
        Some("0") => return None,
        Some(p) if p.starts_with('/') => return Some(Nix { bin: p.into(), root: Some(private_root()), certs: None }),
        _ => {}
    }
    if let Some(bin) = on_path("nix") {
        return Some(Nix { bin, root: None, certs: None });
    }
    let fetch = FETCH.lock().unwrap().take()?;
    match fetch() {
        Ok((bin, certs)) => Some(Nix { bin, root: Some(private_root()), certs }),
        Err(why) => {
            crate::say!("couldn't fetch Nix ({why}): reading the flake without it");
            None
        }
    }
}

/// The Nix to use, fetching it if needed (the first time only).
pub fn get() -> Option<&'static Nix> {
    NIX.get_or_init(choose).as_ref()
}

#[cfg(test)]
/// Whether Nix can be had without fetching anything.
pub fn here() -> bool {
    std::env::var("NAMI_IMPORT_NIX").map_or(on_path("nix").is_some(), |v| v != "0")
}

fn private_root() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into())).join(".cache"));
    cache.join("nami/import-nix")
}

/// A private store bigger than this is emptied before the next import (a
/// flake's inputs and home-manager's files add up; they're fetched again).
const KEEP: u64 = 4 << 30;

impl Nix {
    /// `nix` with the arguments every call needs.
    pub fn command(&self) -> std::process::Command {
        let mut c = std::process::Command::new(&self.bin);
        // Features configurations use (pipe operators); Nix only warns
        // about ones it doesn't know.
        c.args(["--extra-experimental-features", "nix-command flakes pipe-operators"]);
        if let Some(root) = &self.root {
            c.args(["--store", &format!("local?root={}", root.display())]);
            // Its own settings (defaults), not a nix.conf some other Nix
            // left in /etc; yours in ~/.config/nix still count.
            c.env("NIX_CONF_DIR", root.join("etc"));
            // No build users: the store is whoever runs nami's (root too).
            c.args(["--option", "build-users-group", "", "--option", "require-drop-supplementary-groups", "false"]);
            // A Nix from the store looks for certificates where NixOS keeps
            // them; distros keep them elsewhere.
            if std::env::var_os("NIX_SSL_CERT_FILE").is_none() {
                if let Some(f) = crate::runtime::ca_bundle().map(PathBuf::from).or_else(|| self.certs.clone()) {
                    c.env("NIX_SSL_CERT_FILE", f);
                }
            }
        }
        c
    }

    /// Before an import: keep the private store from growing without end.
    pub fn tidy(&self, deadline: Instant) {
        let Some(root) = &self.root else { return };
        if size(&root.join("nix/store"), deadline.min(Instant::now() + Duration::from_secs(20))) > KEEP {
            crate::say!("emptying nami's Nix store for imports ({})", root.display());
            let _ = self.command().args(["store", "gc", "--quiet"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
        }
    }
}

fn size(dir: &Path, deadline: Instant) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let mut total = 0;
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        if Instant::now() > deadline {
            break;
        }
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => todo.push(e.path()),
                Ok(m) => total += m.blocks() * 512,
                Err(_) => {}
            }
        }
    }
    total
}

/// Where a path Nix printed is on disk: itself, or, in a private store,
/// under its root (links in it followed as Nix sees them).
pub fn real(path: &Path) -> PathBuf {
    match get().and_then(|n| n.root.as_deref()) {
        Some(root) => on_disk(root, &canonical_in(root, path)),
        None => path.to_path_buf(),
    }
}

/// The path with every link followed, as Nix sees it (`/nix/store/...`):
/// what `fs::canonicalize` gives for the machine's own store.
pub fn canonical(path: &Path) -> PathBuf {
    match get().and_then(|n| n.root.as_deref()) {
        Some(root) => canonical_in(root, path),
        None => std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
    }
}

fn on_disk(root: &Path, logical: &Path) -> PathBuf {
    if logical.starts_with("/nix/store") {
        root.join(logical.strip_prefix("/").unwrap())
    } else {
        logical.to_path_buf()
    }
}

/// `path` as seen from inside `root`: links are followed, absolute ones to
/// /nix/store within it. What isn't under /nix/store is the machine's.
fn canonical_in(root: &Path, path: &Path) -> PathBuf {
    if !path.starts_with("/nix/store") {
        return path.to_path_buf();
    }
    let parts = |p: &Path| -> Vec<std::ffi::OsString> {
        let mut v: Vec<_> = p
            .components()
            .filter_map(|c| match c {
                Component::Normal(n) => Some(n.to_os_string()),
                Component::ParentDir => Some("..".into()),
                _ => None,
            })
            .collect();
        v.reverse();
        v
    };
    let mut logical = PathBuf::from("/");
    let mut todo = parts(path);
    let mut hops = 0;
    while let Some(name) = todo.pop() {
        if name == ".." {
            logical.pop();
            continue;
        }
        let next = logical.join(&name);
        match std::fs::read_link(on_disk(root, &next)) {
            Ok(target) if hops < 40 => {
                hops += 1;
                if target.is_absolute() {
                    logical = PathBuf::from("/");
                }
                todo.extend(parts(&target));
            }
            _ => logical = next,
        }
    }
    logical
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_in_a_private_store() {
        let t = std::env::temp_dir().join(format!("nami-nixtool-{}", std::process::id()));
        let store = t.join("nix/store");
        std::fs::create_dir_all(store.join("aaa-theme/share/icons/X")).unwrap();
        std::fs::write(store.join("aaa-theme/share/icons/X/index.theme"), "x").unwrap();
        std::fs::create_dir_all(store.join("bbb-home-files/.local/share/icons")).unwrap();
        std::os::unix::fs::symlink("/nix/store/aaa-theme/share/icons/X", store.join("bbb-home-files/.local/share/icons/X")).unwrap();
        std::os::unix::fs::symlink("../X/index.theme", store.join("aaa-theme/share/icons/X/rel")).unwrap();
        let c = |p: &str| canonical_in(&t, Path::new(p));
        assert_eq!(c("/nix/store/bbb-home-files/.local/share/icons/X"), PathBuf::from("/nix/store/aaa-theme/share/icons/X"));
        assert_eq!(c("/nix/store/bbb-home-files/.local/share/icons/X/rel"), PathBuf::from("/nix/store/aaa-theme/share/icons/X/index.theme"));
        assert_eq!(on_disk(&t, &c("/nix/store/bbb-home-files/.local/share/icons/X/rel")), store.join("aaa-theme/share/icons/X/index.theme"));
        assert_eq!(c("/etc/hostname"), PathBuf::from("/etc/hostname"));
        std::fs::remove_dir_all(&t).unwrap();
    }
}
