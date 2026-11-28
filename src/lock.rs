//! nami.lock: the exact result of a nami file, to reproduce it anywhere.
//!
//! Like Cargo.lock: `nami apply` writes it next to the nami file, later applies
//! keep the locked store paths (even after the catalog moves on) and only
//! resolve what's new, `--upgrade` re-resolves everything, and `--locked`
//! installs exactly what's locked or fails. Commit it with your nami file.
//!
//! Next to a package.nami it holds what the recipe fetches: each source
//! URL's sha256, recorded the first time it's fetched (`sources`, see
//! [`Pins`]), so the recipe needs no hashes written by hand and a file
//! that changes upstream afterwards is refused.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::profile::Entry;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Locked {
    pub name: String,
    pub paths: Vec<String>,
    /// For `github:` packages: the commit they were built from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// For a Python with packages (python_env.rs): what it's made of, the
    /// interpreter, its packages and bash, to make it again where it isn't.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
}

/// Which catalog the packages were picked from. A catalog from a channel
/// (`<server>/menu/nixos-unstable`) is named by the channel and its release,
/// never by the server: the lock stays good when nami moves to another
/// address, and nami asks the server this computer uses for it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    /// The channel it came from, by name (`nixos-unstable`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// The channel's release it was (the catalog's revision: `55bfb4d7437b`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    /// Where it came from when it isn't a channel's: a file, or a server of
    /// your own (`nami update --from`). Older locks have the
    /// address of nami's own server here: read as its release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// sha256 of the catalog file, to tell which one it was.
    pub sha256: String,
}

impl Catalog {
    /// Where to fetch this catalog again: its release from the server this
    /// computer gets its catalog from (nami's own if it has none), else
    /// where it came from.
    pub fn source_for(&self, store: &crate::store::Store) -> Option<String> {
        match &self.release {
            Some(release) => {
                let base = crate::selfupdate::base(store).unwrap_or_else(|_| crate::site::SERVED_FROM.to_string());
                Some(release_index(&base, release))
            }
            None => self.source.clone(),
        }
    }
}

/// A release's catalog on a server: `<base>/menu/<release>/index-x86_64-linux.json.xz`.
fn release_index(base: &str, release: &str) -> String {
    format!("{base}/menu/{release}/index-{}.json.xz", crate::index::SYSTEM)
}

/// `<base>/menu/<release>/index-<system>.json.xz`: its base and release.
fn release_of(url: &str) -> Option<(&str, &str)> {
    let (base, rest) = url.rsplit_once("/menu/")?;
    let (release, file) = rest.split_once('/')?;
    let ok = !release.is_empty() && release.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    (ok && file.starts_with("index-") && !file.contains('/')).then_some((base, release))
}

/// `<base>/menu/<channel>`: the channel's name.
fn channel_of(url: &str) -> Option<&str> {
    let (_, name) = url.rsplit_once("/menu/")?;
    (!name.is_empty() && !name.contains('/')).then_some(name)
}

/// The lock format this nami writes, and the newest it reads.
pub const VERSION: u32 = 1;

/// The kind of computer packages are built for: `x86_64-linux`, `aarch64-darwin`.
pub fn this_system() -> String {
    let os = if cfg!(target_os = "macos") { "darwin" } else { "linux" };
    format!("{}-{os}", std::env::consts::ARCH)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Lock {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<Catalog>,
    /// By the name used in your nami file (a catalog name or a recipe path).
    #[serde(default)]
    pub packages: BTreeMap<String, Locked>,
    /// GPU drivers, fonts and locales the system profile added for GUI apps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtime: Vec<Entry>,
    /// Layers included from git repos, each repo at a commit (see layers.rs).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub layers: BTreeMap<String, crate::layers::Pin>,
    /// Next to a package.nami: what its recipe fetches, by URL (see [`Pins`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sources: BTreeMap<String, SourcePin>,
}

/// A file a recipe fetches, as it was the first time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourcePin {
    /// Its sha256, in hex.
    pub sha256: String,
}

pub fn path_for(config: &Path) -> PathBuf {
    // ~/user.nami's isn't loose in your home: it's next to the file it links
    // to (a config repo's checkout), else in ~/.config/nami.
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from).filter(|h| config.parent() == Some(h.as_path())) {
        return match fs::canonicalize(crate::oldnames::find_in(&home, crate::config::USER_FILE)) {
            Ok(real) if real.parent() != Some(home.as_path()) => crate::oldnames::find(&real.with_file_name("nami.lock")),
            _ => crate::oldnames::find(&home.join(".config/nami/nami.lock")),
        };
    }
    crate::oldnames::find(&config.with_file_name("nami.lock"))
}

pub fn load(path: &Path) -> Result<Lock> {
    match fs::read(path) {
        Ok(data) => read(&data, &this_system()).with_context(|| format!("reading {}", path.display())),
        Err(_) => Ok(Lock { version: VERSION, ..Default::default() }),
    }
}

/// A lock's text, on a computer of kind `system`: refused when a newer nami
/// wrote it, or when it was made for another kind of computer.
fn read(data: &[u8], system: &str) -> Result<Lock> {
    let raw: serde_json::Value = serde_json::from_slice(data)?;
    let version = raw.get("version").and_then(serde_json::Value::as_u64).unwrap_or(0);
    anyhow::ensure!(version <= u64::from(VERSION), "this file needs a newer nami: nami self-update (it's version {version} of the format; this nami reads up to {VERSION})");
    if let Some(theirs) = raw.get("system").and_then(serde_json::Value::as_str) {
        anyhow::ensure!(
            theirs == system,
            "it was made on a {theirs} computer, and this one is {system}, so its packages won't run here. Move it aside, and nami makes a new one for this computer."
        );
    }
    let mut lock: Lock = serde_json::from_value(raw)?;
    // Older locks named the server its catalog came from: nami's
    // own is its release now, wherever nami is.
    if let Some(c) = lock.catalog.as_mut().filter(|c| c.release.is_none()) {
        if let Some((base, release)) = c.source.as_deref().and_then(release_of) {
            if [crate::site::SERVED_FROM, crate::site::PUBLIC].contains(&base) {
                c.release = Some(release.to_string());
                c.source = None;
            }
        }
    }
    Ok(lock)
}

/// Writes the lock in this nami's format, with the kind of computer it's for.
pub fn save(path: &Path, lock: &Lock) -> Result<()> {
    /// A [`Lock`] as written: `system` after `version`, as people read it.
    #[derive(Serialize)]
    struct Written<'a> {
        version: u32,
        system: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        catalog: &'a Option<Catalog>,
        packages: &'a BTreeMap<String, Locked>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        runtime: &'a Vec<Entry>,
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        layers: &'a BTreeMap<String, crate::layers::Pin>,
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        sources: &'a BTreeMap<String, SourcePin>,
    }
    let Lock { version: _, catalog, packages, runtime, layers, sources } = lock;
    let written = Written { version: VERSION, system: this_system(), catalog, packages, runtime, layers, sources };
    let mut data = serde_json::to_vec_pretty(&written)?;
    data.push(b'\n');
    if fs::read(path).ok().as_deref() == Some(&data[..]) {
        return Ok(());
    }
    // (tray.lock becomes nami.lock, outside a git repo.)
    let path = &crate::oldnames::for_writing(path);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    crate::store::write_atomic(path, &data)
}

/// A recipe's pins: the `sources` of the nami.lock next to its
/// package.nami. Only that key is changed when they're saved; a lock that
/// has packages too (a project's, in the same folder) keeps them. The
/// hashes are the same on every kind of computer, so a lock with nothing
/// but these says no `system`.
pub struct Pins {
    path: PathBuf,
    sources: BTreeMap<String, SourcePin>,
    changed: bool,
}

impl Pins {
    /// The pins of the recipe in folder `dir` (none yet: an empty set).
    pub fn load(dir: &Path) -> Result<Pins> {
        let path = crate::oldnames::find(&dir.join("nami.lock"));
        let sources = match fs::read(&path) {
            Ok(data) => {
                let raw: serde_json::Value = serde_json::from_slice(&data).with_context(|| format!("reading {}", path.display()))?;
                serde_json::from_value(raw.get("sources").cloned().unwrap_or_default()).ok().unwrap_or_default()
            }
            Err(_) => BTreeMap::new(),
        };
        Ok(Pins { path, sources, changed: false })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The sha256 (hex) recorded for `url`.
    pub fn get(&self, url: &str) -> Option<&str> {
        self.sources.get(url).map(|p| p.sha256.as_str())
    }

    pub fn set(&mut self, url: &str, sha256: &str) {
        if self.get(url) != Some(sha256) {
            self.sources.insert(url.to_string(), SourcePin { sha256: sha256.to_string() });
            self.changed = true;
        }
    }

    pub fn remove(&mut self, url: &str) -> Option<String> {
        let old = self.sources.remove(url).map(|p| p.sha256);
        self.changed |= old.is_some();
        old
    }

    pub fn urls(&self) -> Vec<String> {
        self.sources.keys().cloned().collect()
    }

    /// Write them, if they changed: only `sources`, the rest as it was.
    pub fn save(&mut self) -> Result<bool> {
        if !self.changed {
            return Ok(false);
        }
        let mut raw: serde_json::Map<String, serde_json::Value> = match fs::read(&self.path) {
            Ok(data) => serde_json::from_slice(&data).with_context(|| format!("reading {}", self.path.display()))?,
            Err(_) => serde_json::Map::new(),
        };
        raw.remove("sources");
        let mut data = if raw.keys().all(|k| k == "version") {
            // Only pins: `version` first, as every lock starts.
            #[derive(Serialize)]
            struct Written<'a> {
                version: u32,
                #[serde(skip_serializing_if = "BTreeMap::is_empty")]
                sources: &'a BTreeMap<String, SourcePin>,
            }
            serde_json::to_vec_pretty(&Written { version: VERSION, sources: &self.sources })?
        } else {
            if !self.sources.is_empty() {
                raw.insert("sources".into(), serde_json::to_value(&self.sources)?);
            }
            serde_json::to_vec_pretty(&raw)?
        };
        data.push(b'\n');
        crate::store::write_atomic(&self.path, &data).with_context(|| format!("writing {}", self.path.display()))?;
        self.changed = false;
        Ok(true)
    }
}

/// Identify the installed catalog.
pub fn catalog(var_dir: &Path) -> Option<Catalog> {
    // (From its compact form, when it's there: reading 40-60 MB to hash
    // it would be most of what a `nami run` takes.)
    let sha256 = crate::index_file::sha256_of(&var_dir.join("index.json"), None).ok()?;
    let read = |f: &str| fs::read_to_string(var_dir.join(f)).ok().map(|s| s.trim().to_string());
    let (from, resolved) = (read("index-source"), read("index-resolved"));
    // A channel's: its name and the exact release (a channel moves on).
    if let (Some(from), Some(resolved)) = (&from, &resolved) {
        if let (Some(channel), Some((base, release))) = (channel_of(from), release_of(resolved)) {
            if from.starts_with(&format!("{base}/menu/")) {
                return Some(Catalog { channel: Some(channel.to_string()), release: Some(release.to_string()), source: None, sha256 });
            }
        }
    }
    // Else where it came from.
    Some(Catalog { source: resolved.or(from), sha256, ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_text(extra: &str) -> String {
        format!("{{ \"version\": 1, {extra} \"packages\": {{}} }}")
    }

    #[test]
    fn it_names_the_channel_not_the_server() {
        let dir = std::env::temp_dir().join(format!("nami-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("index.json"), "{}").unwrap();
        fs::write(dir.join("index-source"), "https://namios.dev/menu/nixos-unstable\n").unwrap();
        fs::write(dir.join("index-resolved"), "https://namios.dev/menu/55bfb4d7437b/index-x86_64-linux.json.xz").unwrap();
        let c = catalog(&dir).unwrap();
        assert_eq!((c.channel.as_deref(), c.release.as_deref(), c.source.as_deref()), (Some("nixos-unstable"), Some("55bfb4d7437b"), None));
        let lock = Lock { version: 1, catalog: Some(c), ..Default::default() };
        save(&dir.join("nami.lock"), &lock).unwrap();
        let text = fs::read_to_string(dir.join("nami.lock")).unwrap();
        assert!(!text.contains("https://"), "no server in the lock: {text}");
        assert!(text.contains(&format!("\"system\": \"{}\"", this_system())), "{text}");
        assert!(text.find("\"version\"").unwrap() < text.find("\"system\"").unwrap() && text.find("\"system\"").unwrap() < text.find("\"catalog\"").unwrap(), "{text}");
        assert_eq!(load(&dir.join("nami.lock")).unwrap(), lock);
        // A catalog of your own (a file): where it came from.
        fs::write(dir.join("index-source"), "/srv/my-catalog.json\n").unwrap();
        fs::remove_file(dir.join("index-resolved")).unwrap();
        let c = catalog(&dir).unwrap();
        assert_eq!((c.channel, c.release, c.source.as_deref()), (None, None, Some("/srv/my-catalog.json")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn old_locks_with_the_servers_address_still_read() {
        let old = lock_text("\"catalog\": { \"source\": \"https://namios.dev/menu/55bfb4d7437b/index-x86_64-linux.json.xz\", \"sha256\": \"ab\" },");
        let c = read(old.as_bytes(), "x86_64-linux").unwrap().catalog.unwrap();
        assert_eq!((c.release.as_deref(), c.source.as_deref(), c.sha256.as_str()), (Some("55bfb4d7437b"), None, "ab"));
        assert_eq!(release_index("https://namios.dev", "55bfb4d7437b"), format!("https://namios.dev/menu/55bfb4d7437b/index-{}.json.xz", crate::index::SYSTEM));
        // A server of your own stays as it was.
        let own = lock_text("\"catalog\": { \"source\": \"https://catalog.example.org/menu/r1/index-x86_64-linux.json.xz\", \"sha256\": \"ab\" },");
        let c = read(own.as_bytes(), "x86_64-linux").unwrap().catalog.unwrap();
        assert_eq!((c.release, c.source.as_deref()), (None, Some("https://catalog.example.org/menu/r1/index-x86_64-linux.json.xz")));
        // And one without `system:` is for any computer.
        assert!(read(lock_text("").as_bytes(), "aarch64-linux").is_ok());
    }

    #[test]
    fn a_recipes_pins_keep_the_rest_of_its_lock() {
        let dir = std::env::temp_dir().join(format!("nami-pins-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // None yet: nothing written until there's one.
        let mut pins = Pins::load(&dir).unwrap();
        assert_eq!(pins.get("https://x/a.tar.gz"), None);
        assert!(!pins.save().unwrap() && !dir.join("nami.lock").exists());
        pins.set("https://x/a.tar.gz", "ab");
        assert!(pins.save().unwrap());
        let text = fs::read_to_string(dir.join("nami.lock")).unwrap();
        assert!(text.contains("\"sources\"") && !text.contains("system"), "{text}");
        assert_eq!(Pins::load(&dir).unwrap().get("https://x/a.tar.gz"), Some("ab"));
        // A project's lock in the same folder keeps its packages, and its own save keeps the pins.
        let mut lock = load(&dir.join("nami.lock")).unwrap();
        assert_eq!(lock.sources["https://x/a.tar.gz"].sha256, "ab");
        lock.packages.insert("jq".into(), Locked { name: "jq-1".into(), paths: vec![], rev: None, inputs: vec![] });
        save(&dir.join("nami.lock"), &lock).unwrap();
        let mut pins = Pins::load(&dir).unwrap();
        pins.set("https://x/b.tar.gz", "cd");
        assert_eq!(pins.remove("https://x/a.tar.gz").as_deref(), Some("ab"));
        pins.save().unwrap();
        let lock = load(&dir.join("nami.lock")).unwrap();
        assert!(lock.packages.contains_key("jq") && lock.sources.len() == 1 && lock.sources["https://x/b.tar.gz"].sha256 == "cd");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn it_refuses_a_newer_lock_or_another_computers() {
        let newer = read(b"{ \"version\": 2, \"packages\": {} }", "x86_64-linux").unwrap_err().to_string();
        assert!(newer.contains("this file needs a newer nami: nami self-update"), "{newer}");
        let other = read(lock_text("\"system\": \"aarch64-linux\",").as_bytes(), "x86_64-linux").unwrap_err().to_string();
        assert!(other.contains("made on a aarch64-linux computer, and this one is x86_64-linux"), "{other}");
        assert!(read(lock_text("\"system\": \"x86_64-linux\",").as_bytes(), "x86_64-linux").is_ok());
        assert_eq!(this_system(), format!("{}-{}", std::env::consts::ARCH, if cfg!(target_os = "macos") { "darwin" } else { "linux" }));
    }
}
