//! The package index: nixpkgs attribute name -> store paths.
//!
//! Produced once per channel release by `index/build-index.sh` (the only place
//! Nix ever runs) and downloaded by users.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::Deserialize;

use crate::index_file::Catalog;
use crate::store::{write_atomic, Cache, Store};

/// The platform catalogs are built for (`index-<SYSTEM>.json.xz`): the one nami runs on.
/// An x86_64 computer's catalog (`x86_64-linux`) is also where `nami bundle --arch aarch64`
/// finds its ARM one, beside it; an ARM Linux computer's is `aarch64-linux` (docs/arm.md).
pub const SYSTEM: &str = if cfg!(target_os = "macos") {
    if cfg!(target_arch = "aarch64") {
        "aarch64-darwin"
    } else {
        "x86_64-darwin"
    }
} else if cfg!(target_arch = "aarch64") {
    "aarch64-linux"
} else {
    "x86_64-linux"
};

/// Whether nami runs on ARM Linux (aarch64-linux): everything Linux has, from a catalog of
/// its own that has no build definitions yet (docs/arm.md).
pub const ARM_LINUX: bool = cfg!(all(target_os = "linux", target_arch = "aarch64"));

/// Whether nami runs on a Mac: nami's packages and projects, without what's Linux's own
/// (services, the desktop, namiOS, bundles, builds).
pub const MAC: bool = cfg!(target_os = "macos");

/// A catalog's folders of its own beside its index: `versions` (histories), `drvs` (build
/// definitions). x86_64 Linux's have those names; another platform's, in the same release
/// folder, `versions-aarch64-linux`, `versions-aarch64-darwin` and so on (their store paths
/// are other ones).
pub fn beside(folder: &str) -> String {
    if SYSTEM == "x86_64-linux" {
        folder.to_string()
    } else {
        format!("{folder}-{SYSTEM}")
    }
}

/// A channel's file for this platform, when it's another than x86_64 Linux's:
/// `menu/nixos-unstable` names the release that catalog is in, `menu/nixos-unstable.aarch64-linux`
/// the one ARM Linux's is, `menu/nixos-unstable.aarch64-darwin` a Mac's (each platform's catalog
/// is made in its own run, and may follow later).
pub fn channel_for_system(channel: &str) -> Option<String> {
    (SYSTEM != "x86_64-linux").then(|| format!("{channel}.{SYSTEM}"))
}

#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    pub name: String,
    #[serde(rename = "outputName", default)]
    pub output_name: Option<String>,
    pub outputs: BTreeMap<String, Option<String>>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(rename = "mainProgram", default)]
    pub main_program: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    /// .desktop files it ships: it's an app, not just a command or library.
    #[serde(default)]
    pub apps: Option<Vec<String>>,
    /// sha256 of its build definition (drvs/<attr>.json), so the signed
    /// index vouches for every definition too.
    #[serde(default)]
    pub drv: Option<String>,
    /// sha256 of its app icon (attr.png or attr.svg in icons.tar next to
    /// the index), for the same reason.
    #[serde(default)]
    pub icon: Option<String>,
    /// Not free software (its licence restricts use or sharing).
    #[serde(default)]
    pub unfree: bool,
    /// sha256 of its build closure (drvs/<attr>.closure.json): how to build
    /// it here, because the binary cache doesn't have it (Hydra doesn't
    /// build unfree software).
    #[serde(default)]
    pub closure: Option<String>,
    /// Free software whose nixpkgs build isn't in the binary cache (Hydra
    /// didn't build it: it needs software that isn't free, or it's left to
    /// whoever wants it). With a closure nami builds it here; without, it
    /// can't be had from this catalog.
    #[serde(default)]
    pub uncached: bool,
    /// Not from nixpkgs: an extra app the catalog's publisher built from a
    /// recipe (packaging/apps/) and serves from this binary cache. Trusted
    /// because the catalog is signed (see `catalog_caches`).
    #[serde(default)]
    pub cache: Option<CatalogCache>,
    /// Who makes it, for extra apps (AppStream's developer name).
    #[serde(default)]
    pub publisher: Option<String>,
    /// What people see of an app (index/meta.py, from Flathub's and
    /// Debian's AppStream): its own name ("GIMP"), one line and a few
    /// sentences about it, its freedesktop categories, search words and
    /// screenshots (URLs).
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub about: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub screenshots: Vec<String>,
    /// 0-1000: how many people use it, as its rank among the apps.
    #[serde(default)]
    pub popularity: Option<u16>,
}

/// One version a package has had: [version, date, nixpkgs revision,
/// {output: store path}], as its history lists them, newest first.
#[derive(Debug, Clone, Deserialize)]
pub struct Version(pub String, pub String, pub String, pub BTreeMap<String, String>);

/// `0.10.1` > `0.9.3`: numbers compared part by part.
/// A part's letters after its number count too: 1.1.1w > 1.1.1s > 1.1.1.
pub(crate) fn version_key(v: &str) -> Vec<(u64, String)> {
    v.split(['.', '-'])
        .enumerate()
        .map(|(i, p)| {
            let digits = p.len() - p.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            // A date after the first number is a snapshot's (gcc 14-20241116):
            // older than any release of that line (14.4.0).
            if i > 0 && digits >= 8 {
                return (0, String::new());
            }
            (p[..digits].parse().unwrap_or(0), p[digits..].to_string())
        })
        .collect()
}

#[cfg(test)]
#[test]
fn a_snapshot_is_older_than_its_releases() {
    assert!(version_key("14.4.0") > version_key("14-20241116"));
    assert!(version_key("14.2.1.20250322") > version_key("14.2.1"));
    assert!(version_key("20241116") > version_key("20240101"));
}

/// Versions, said short and newest first: the major ones (25, 24, 22), or
/// the minor ones when they all share a major (3.13, 3.12).
fn version_lines(mut have: Vec<String>) -> String {
    have.sort_by_key(|v| std::cmp::Reverse(version_key(v)));
    have.dedup();
    let major = |v: &String| v.split('.').next().unwrap_or("").to_string();
    let depth = if have.iter().all(|v| major(v) == major(&have[0])) { 2 } else { 1 };
    let mut lines: Vec<String> = have.iter().map(|v| v.split('.').take(depth).collect::<Vec<_>>().join(".")).collect();
    lines.dedup();
    let more = if lines.len() > 8 { format!(" and {} more", lines.len() - 8) } else { String::new() };
    lines.truncate(8);
    format!("{}{more}, {} versions in all", lines.join(", "), have.len())
}

/// The highest of `versions` that `wanted` fits.
fn newest_fitting<'v>(versions: &'v [Version], wanted: &str) -> Option<&'v Version> {
    versions.iter().filter(|v| version_matches(&v.0, wanted)).max_by_key(|v| version_key(&v.0))
}

/// A package the catalog doesn't describe (nixpkgs has since removed it).
fn bare(attr: &str) -> Package {
    serde_json::from_value(serde_json::json!({ "name": attr, "outputs": {} })).expect("a package")
}

/// `base` at a version from its history: that build's files. (Its build
/// definitions are the catalog's, for its own version: left out.)
fn from_history(base: &Package, found: &Version) -> Result<&'static Package> {
    let mut p = base.clone();
    let out = found.3.get("out").or_else(|| found.3.values().next()).ok_or_else(|| anyhow!("no files listed"))?;
    p.name = store_name(out).to_string();
    p.outputs = found.3.iter().map(|(o, path)| (o.clone(), Some(path.clone()))).collect();
    if !p.outputs.contains_key(p.output_name.as_deref().unwrap_or("out")) {
        p.output_name = None;
    }
    (p.drv, p.closure) = (None, None);
    Ok(Box::leak(Box::new(p)))
}

/// `firefox@150` -> ("firefox", Some("150")): a catalog name, and the
/// version wanted. Only a plain name with a version starting with a digit
/// (`github:o/r@v2` and `@angular/cli` are something else).
pub fn split_version(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('@') {
        Some((a, v)) if !a.is_empty() && !a.contains([':', '/']) && !a.starts_with('.') && v.starts_with(|c: char| c.is_ascii_digit()) => (a, Some(v)),
        _ => (name, None),
    }
}

/// Is `version` one `wanted` asks for? `150` is 150, 150.0.1 and 150.1;
/// `3.12.4` is 3.12.4 (and 3.12.4.1), not 3.12.40.
pub fn version_matches(version: &str, wanted: &str) -> bool {
    version == wanted || version.strip_prefix(wanted).is_some_and(|rest| rest.starts_with(['.', '-', '+']))
}

/// Where a catalog's version histories are, and the versions looked up.
/// They're looked up when asked for, a small piece at a time: 4096 shards
/// (versions/<first 3 hex of sha256(attr)>.json, {attr: [Version]}), each
/// signed like the catalog, next to it.
struct History {
    /// versions/ next to the index (a URL or a folder).
    source: Option<String>,
    cache: PathBuf,
    http: ureq::Agent,
    trusted: Vec<String>,
    /// The catalog was accepted unsigned (`nami update --unsigned`): so are its histories.
    unsigned_ok: bool,
    picked: std::sync::Mutex<BTreeMap<String, &'static Package>>,
}

impl History {
    fn new(store: &Store) -> Self {
        let var = store.var_dir();
        let read = |f: &str| fs::read_to_string(var.join(f)).ok().map(|s| s.trim().to_string());
        // (A computer from an image before 0.18.1 has no record: it's next to the catalog.)
        let source = read("versions-source").or_else(|| read("index-resolved").and_then(|i| versions_next_to(&i)));
        let unsigned_ok = read("index-unsigned-ok").is_some_and(|u| Some(u) == read("index-source"));
        // Cached per catalog: another catalog's shards are other shards.
        let cache = store.cache_dir().join("versions").join(&crate::catalog::sha256_hex(source.as_deref().unwrap_or("").as_bytes())[..16]);
        History { source, cache, http: store.http().clone(), trusted: crate::catalog::trusted_keys(store), unsigned_ok, picked: Default::default() }
    }

    /// `attr`'s earlier versions as they're here, without the network: from
    /// the cache, or a folder the histories are in. None: not here yet.
    fn list_here(&self, attr: &str) -> Option<Vec<Version>> {
        let shard = format!("{}.json", &crate::catalog::sha256_hex(attr.as_bytes())[..3]);
        let data = fs::read(self.cache.join(&shard)).ok().or_else(|| {
            let source = self.source.as_deref().filter(|s| !s.starts_with("https://") && !s.starts_with("http://"))?;
            fs::read(Path::new(source).join(&shard)).ok()
        })?;
        let mut shard: BTreeMap<String, Vec<Version>> = serde_json::from_slice(&data).ok()?;
        Some(shard.remove(attr).unwrap_or_default())
    }

    /// `attr`'s earlier versions (none if it has had no others).
    fn list(&self, attr: &str) -> Result<Vec<Version>> {
        let shard = &crate::catalog::sha256_hex(attr.as_bytes())[..3];
        let cached = self.cache.join(format!("{shard}.json"));
        let data = match fs::read(&cached) {
            Ok(d) => d,
            Err(_) => {
                let source = self.source.as_deref().ok_or_else(|| anyhow!("this catalog has no version history to look in (nami update fetches it)"))?;
                let Some(data) = self.fetch(source, &format!("{shard}.json"))? else { return Ok(Vec::new()) };
                match self.fetch(source, &format!("{shard}.json.sig"))? {
                    Some(sig) => {
                        crate::catalog::verify(&data, &String::from_utf8_lossy(&sig), &self.trusted).with_context(|| format!("{source}/{shard}.json: signature check failed"))?;
                    }
                    None => ensure!(self.unsigned_ok, "{source}/{shard}.json isn't signed"),
                }
                // Kept for next time where it may be (root's cache isn't
                // someone else's to write: then it's for this run only).
                let _ = fs::create_dir_all(&self.cache).map_err(anyhow::Error::from).and_then(|_| write_atomic(&cached, &data).map_err(anyhow::Error::from));
                data
            }
        };
        let mut shard: BTreeMap<String, Vec<Version>> = serde_json::from_slice(&data).context("reading the version history")?;
        Ok(shard.remove(attr).unwrap_or_default())
    }

    /// A file from the histories' folder; None if it isn't there.
    fn fetch(&self, source: &str, file: &str) -> Result<Option<Vec<u8>>> {
        let mut data = Vec::new();
        if source.starts_with("https://") || source.starts_with("http://") {
            match self.http.get(&format!("{source}/{file}")).call() {
                Ok(r) => r.into_reader().read_to_end(&mut data)?,
                Err(ureq::Error::Status(404, _)) => return Ok(None),
                Err(e) => return Err(e).with_context(|| format!("fetching {source}/{file}")),
            };
            Ok(Some(data))
        } else {
            Ok(fs::read(Path::new(source).join(file)).ok())
        }
    }
}

/// Where a catalog's version histories are: next to it.
fn versions_next_to(index: &str) -> Option<String> {
    index.trim().rsplit_once('/').map(|(dir, _)| format!("{dir}/{}", beside("versions")))
}

/// A store path's name: /nix/store/<hash>-firefox-150.0.1 -> firefox-150.0.1.
fn store_name(path: &str) -> &str {
    path.rsplit('/').next().and_then(|b| b.get(33..)).unwrap_or(path)
}

/// A file name as one URL path segment (nixpkgs names can hold quotes).
pub(crate) fn url_encode(name: &str) -> String {
    name.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._+".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// A binary cache a catalog names. Unknown fields are ignored (unlike in
/// `caches:`), so a newer catalog still loads here.
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogCache {
    pub url: String,
    pub key: String,
}

/// The binary caches a catalog's packages come from, besides cache.nixos.org.
/// Only web caches with a well-formed key count.
pub fn catalog_caches<'a>(packages: impl IntoIterator<Item = &'a Package>) -> Vec<Cache> {
    let mut out: Vec<Cache> = Vec::new();
    for c in packages.into_iter().filter_map(|p| p.cache.as_ref()) {
        let web = c.url.starts_with("https://") || c.url.starts_with("http://");
        let key_ok = c.key.split_once(':').is_some_and(|(n, k)| !n.is_empty() && k.len() == 44);
        let cache = Cache { url: c.url.trim_end_matches('/').to_string(), key: c.key.clone() };
        if web && key_ok && !out.contains(&cache) {
            out.push(cache);
        }
    }
    out
}

impl Package {
    /// The outputs a user wants on PATH: `bin` if split out, else the default.
    pub fn install_paths(&self) -> Vec<String> {
        let wanted = if self.outputs.contains_key("bin") {
            "bin"
        } else {
            self.output_name.as_deref().unwrap_or("out")
        };
        self.outputs.get(wanted).cloned().flatten().into_iter().collect()
    }
}

pub struct Index {
    /// The catalog, read in place (index_file.rs): a package is read from
    /// it when it's first asked for, and kept.
    catalog: Catalog,
    parsed: Vec<OnceLock<Option<Box<Package>>>>,
    icons: std::path::PathBuf,
    history: History,
}

/// At most this many icons are fetched one at a time (from the catalogs'
/// shared objects); more, and icons-128.tar comes down in one piece.
const ICONS_ONE_BY_ONE: usize = 400;

/// Where the catalogs' objects are, kept by their sha256 (`<site>/menu/objects/`), for the
/// catalog this computer has: beside its release folder (`index-resolved`, `.../menu/<rev>/
/// index-<system>.json.xz`). None when it came from a file, or isn't known.
pub fn objects_url(var_dir: &Path) -> Option<String> {
    let resolved = fs::read_to_string(var_dir.join("index-resolved")).ok()?;
    let resolved = resolved.trim();
    if !(resolved.starts_with("https://") || resolved.starts_with("http://")) {
        return None;
    }
    let menu = resolved.rsplit_once('/')?.0.rsplit_once('/')?.0;
    Some(format!("{menu}/objects"))
}

/// A catalog's icons in one piece, beside it: x86_64 Linux's `icons-128.tar`; another
/// platform's `icons-<system>.tar` (a Mac's: the icons of Homebrew's casks), as its release
/// folder may be Linux's too.
fn icons_tar() -> String {
    if SYSTEM == "x86_64-linux" { "icons-128.tar".to_string() } else { format!("icons-{SYSTEM}.tar") }
}

/// A pinned catalog its server no longer has (it serves another under its
/// name) isn't asked for again for this long.
const GONE_FOR: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

impl Index {
    /// "✓ Catalog verified" for nami's own; someone else's catalog by its key's name.
    fn signed_words(signer: &str, keys: &[String], more: &str) -> String {
        match crate::catalog::signer_words(signer, keys).as_str() {
            "namios.dev" => format!("✓ Catalog verified{more}"),
            who => format!("catalog signed by {who}{more}"),
        }
    }

    pub fn load(store: &Store) -> Result<Self> {
        let path = store.var_dir().join("index.json");
        if !path.exists() {
            bail!("no package index yet; run `nami update --from <index-url-or-file>`");
        }
        let catalog = Catalog::open(&path, Some(&Self::idx_cache(store))).context("reading the package index")?;
        Ok(Self::with(store, catalog))
    }

    /// Where a user keeps compact forms of catalogs they may not write next to.
    fn idx_cache(store: &Store) -> PathBuf {
        store.cache_dir().join("catalogs")
    }

    fn with(store: &Store, catalog: Catalog) -> Self {
        let parsed = vec![OnceLock::new(); catalog.len()];
        Index { catalog, parsed, icons: store.var_dir().join("icons"), history: History::new(store) }
    }

    /// Another platform's catalog, already checked (`nami bundle --arch`):
    /// its packages only.
    pub fn from_json(store: &Store, data: &[u8]) -> Result<Self> {
        Ok(Self::with(store, Catalog::from_vec(data.to_vec()).context("parsing package index")?))
    }

    /// Fetch an index (`.json` or `.json.xz`, URL or local file), check its
    /// signature (`<from>.sig`) against the trusted catalog keys, install it.
    /// `unsigned` accepts a catalog without a signature (local builds).
    pub fn update(store: &Store, from: &str, unsigned: bool) -> Result<usize> {
        let n = Self::fetch_resolved(store, from, unsigned)?;
        // Remember where it came from, so users can refresh it (via the daemon).
        write_atomic(&store.var_dir().join("index-source"), from.as_bytes())?;
        let marker = store.var_dir().join("index-unsigned-ok");
        if unsigned {
            write_atomic(&marker, from.as_bytes())?;
        } else {
            let _ = fs::remove_file(marker);
        }
        Ok(n)
    }

    /// Re-fetch the index from where root last got it, with the same trust.
    pub fn refresh(store: &Store) -> Result<usize> {
        let from = std::fs::read_to_string(store.var_dir().join("index-source"))
            .map_err(|_| anyhow!("no index source recorded; root has to run `nami update --from ...` once"))?;
        let from = from.trim();
        let unsigned = fs::read_to_string(store.var_dir().join("index-unsigned-ok")).is_ok_and(|s| s.trim() == from);
        Self::fetch_resolved(store, from, unsigned)
    }

    /// `from` is an index, or a channel: a small file (`menu/nixos-unstable`)
    /// naming the current release directory next to it, which holds the
    /// index, `icons-128.tar` and `drvs/`. A channel moves on; what was
    /// installed is recorded exactly (`index-resolved`), for projects' locks.
    fn fetch_resolved(store: &Store, from: &str, unsigned: bool) -> Result<usize> {
        let index = match Self::channel_release(store, from)? {
            Some(dir) => {
                write_atomic(&store.var_dir().join("drvs-source"), format!("{dir}/{}", beside("drvs")).as_bytes())?;
                let _ = fs::remove_dir_all(store.var_dir().join("drvs")); // cached definitions are per index
                format!("{dir}/index-{}.json.xz", crate::index::SYSTEM)
            }
            None => from.to_string(),
        };
        let n = Self::fetch(store, &index, unsigned)?;
        write_atomic(&store.var_dir().join("index-resolved"), index.as_bytes())?;
        // Version histories, next to the index (fetched one at a time when asked for).
        let dir = index.rsplit_once('/').map(|(d, _)| d).unwrap_or(".");
        write_atomic(&store.var_dir().join("versions-source"), format!("{dir}/{}", beside("versions")).as_bytes())?;
        Ok(n)
    }

    /// The release directory a channel points at, or None if `from` is an index.
    fn channel_release(store: &Store, from: &str) -> Result<Option<String>> {
        let name = from.rsplit('/').next().unwrap_or(from);
        if name.ends_with(".json") || name.ends_with(".json.xz") || name.ends_with(".xz") {
            return Ok(None);
        }
        // This platform's own channel file first (a Mac's), else the one for all. (The update
        // check: a test computer's says so, site::counted.)
        let own = match channel_for_system(from) {
            Some(own) => Self::read_source(store, &crate::site::counted(store, &own))?,
            None => None,
        };
        let text = match own {
            Some(t) => t,
            None => Self::read_source(store, &crate::site::counted(store, from))?.ok_or_else(|| anyhow!("{from} doesn't exist"))?,
        };
        let release = String::from_utf8_lossy(&text).trim().to_string();
        anyhow::ensure!(
            !release.is_empty() && release.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
            "{from} isn't a channel (it should name a release directory)"
        );
        let parent = from.rsplit_once('/').map(|(d, _)| d).unwrap_or(".");
        Ok(Some(format!("{parent}/{release}")))
    }

    fn read_source(store: &Store, from: &str) -> Result<Option<Vec<u8>>> {
        let mut data = Vec::new();
        if from.starts_with("https://") || from.starts_with("http://") {
            match store.http().get(from).call() {
                Ok(r) => r.into_reader().read_to_end(&mut data)?,
                Err(ureq::Error::Status(404, _)) => return Ok(None),
                Err(e) => return Err(e).with_context(|| format!("fetching {from}")),
            };
        } else {
            match fs::File::open(Path::new(from)) {
                Ok(mut f) => f.read_to_end(&mut data)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e).with_context(|| format!("opening {from}")),
            };
        }
        Ok(Some(data))
    }

    fn fetch(store: &Store, from: &str, unsigned: bool) -> Result<usize> {
        match Self::fetch_patched(store, from) {
            Ok(Some(n)) => return Ok(n),
            Ok(None) => {}
            Err(e) => crate::say!("note: {e:#}; downloading the whole catalog"),
        }
        let raw = Self::read_source(store, from)?.ok_or_else(|| anyhow!("{from} doesn't exist"))?;
        let signed = match Self::read_source(store, &format!("{from}.sig"))? {
            Some(sig) => {
                let keys = crate::catalog::trusted_keys(store);
                let signer = crate::catalog::verify(&raw, &String::from_utf8_lossy(&sig), &keys)
                    .with_context(|| format!("{from}: signature check failed"))?;
                crate::say!("{}", Self::signed_words(&signer, &keys, ""));
                true
            }
            None if unsigned => {
                crate::say!("warning: {from} isn't signed; accepted because of --unsigned");
                false
            }
            None => anyhow::bail!("{from} isn't signed ({from}.sig is missing); use a signed catalog, or pass --unsigned for one you built yourself"),
        };
        let mut reader: Box<dyn Read> = if from.ends_with(".xz") { Box::new(xz2::read::XzDecoder::new(&raw[..])) } else { Box::new(&raw[..]) };
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        Self::install(store, from, &data, signed)
    }

    /// The faster way to a published catalog, when this computer has one
    /// already and the server signed the new one's JSON
    /// (`index-<system>.json.sig`, next to the `.json.xz`): nothing to
    /// download if it's the same catalog, else a patch from the one here
    /// (`patches/<its sha256>.zst`), checked against that signature. None:
    /// the whole catalog it is (an older server, or no patch from this one).
    fn fetch_patched(store: &Store, from: &str) -> Result<Option<usize>> {
        let Some(json_from) = from.strip_suffix(".xz").filter(|f| f.ends_with(".json")) else { return Ok(None) };
        let Ok(have) = Catalog::open(&store.var_dir().join("index.json"), None) else { return Ok(None) };
        let Some(sig) = Self::read_source(store, &format!("{json_from}.sig"))? else { return Ok(None) };
        let sig = String::from_utf8_lossy(&sig).into_owned();
        let keys = crate::catalog::trusted_keys(store);
        if let Ok(signer) = crate::catalog::verify_sha256(&have.sha256(), &sig, &keys) {
            crate::say!("{}", Self::signed_words(&signer, &keys, "; it hasn't changed"));
            // (Its caches are trusted now, if it was taken unsigned before.)
            let named: Vec<Package> = (0..have.len())
                .filter(|i| have.record(*i).flags & crate::index_file::NAMES_CACHE != 0)
                .filter_map(|i| serde_json::from_slice(have.value(i)).ok())
                .collect();
            Self::trust_caches(store, catalog_caches(named.iter()), !named.is_empty(), true)?;
            let icons = Self::icons_named(&have);
            let n = have.len();
            drop(have);
            Self::fetch_icons(store, from, &icons)?;
            return Ok(Some(n));
        }
        let dir = from.rsplit_once('/').map(|(d, _)| d).unwrap_or(".");
        let Some(patch) = Self::read_source(store, &format!("{dir}/patches/{}.zst", have.sha256()))? else { return Ok(None) };
        let data = apply_patch(have.json(), &patch).context("applying the catalog's patch")?;
        drop(have);
        let signer = crate::catalog::verify(&data, &sig, &keys).context("the patched catalog isn't the one signed")?;
        crate::say!("{}", Self::signed_words(&signer, &keys, &format!("; {} KB of changes", patch.len().div_ceil(1024))));
        Self::install(store, from, &data, true).map(Some)
    }

    /// Install catalog `data` (checked) as this computer's.
    fn install(store: &Store, from: &str, data: &[u8], signed: bool) -> Result<usize> {
        let made = crate::index_file::make(data)?;
        let path = store.var_dir().join("index.json");
        write_atomic(&path, data)?;
        // (If it can't be kept, the first to read the catalog makes it again.)
        let _ = crate::index_file::keep(&path, &made.idx);
        Self::trust_caches(store, made.caches.clone(), made.any_cache, signed)?;
        Self::keep_catalog(&store.var_dir().join("menus"), Some(&path), data, &made.idx)?;
        let catalog = Catalog::open(&path, None)?;
        let icons = Self::icons_named(&catalog);
        Self::fetch_icons(store, from, &icons)?;
        Ok(made.count)
    }

    /// Use the binary caches the catalog names for its extra apps, if its
    /// signature checked out: whoever signs the catalog already decides what
    /// `nami install <name>` means, so their cache adds no trust beyond that.
    /// An unsigned catalog (`--unsigned`) names no caches.
    fn trust_caches(store: &Store, named: Vec<Cache>, any_cache: bool, signed: bool) -> Result<()> {
        let file = store.var_dir().join(crate::store::CATALOG_CACHES);
        let caches = if signed { named } else { Vec::new() };
        if caches.is_empty() {
            if !signed && any_cache {
                crate::say!("warning: the catalog isn't signed, so the binary caches it names aren't used");
            }
            if file.exists() {
                fs::remove_file(&file)?;
            }
        } else {
            write_atomic(&file, &serde_json::to_vec_pretty(&caches)?)?;
        }
        store.reload_caches()
    }

    /// Keep a copy of each catalog by its sha256 (with its compact form),
    /// so projects pinned to it work after the catalog moves on: a link to
    /// `linked` (the same file) where it can be. The newest few are kept.
    fn keep_catalog(dir: &Path, linked: Option<&Path>, data: &[u8], idx: &[u8]) -> Result<()> {
        fs::create_dir_all(dir)?;
        let sha = crate::catalog::sha256_hex(data);
        let kept = dir.join(format!("{sha}.json"));
        let tmp = dir.join(format!("{sha}.json.{}.tmp", std::process::id()));
        let _ = fs::remove_file(&tmp);
        match linked.map(|l| fs::hard_link(l, &tmp)) {
            Some(Ok(())) => fs::rename(&tmp, &kept)?,
            _ => write_atomic(&kept, data)?,
        }
        crate::index_file::keep(&kept, idx)?;
        let _ = fs::remove_file(dir.join(format!("{sha}.gone")));
        let mut kept: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(dir)?
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        kept.sort();
        for (_, old) in kept.iter().rev().skip(6) {
            let _ = fs::remove_file(old);
            let _ = fs::remove_file(old.with_extension("idx"));
            let _ = fs::remove_file(old.with_extension("idx-tie"));
        }
        Ok(())
    }

    /// The catalog with this sha256 (what a lock pins): the current one, a
    /// kept copy, or fetched again from where it came from (`source`) and
    /// checked like any catalog: its signature, and then its sha256.
    pub fn load_pinned(store: &Store, sha256: &str, source: Option<&str>) -> Result<Self> {
        ensure!(sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit()), "bad catalog hash in the lock");
        let cache = Self::idx_cache(store);
        let candidates = [
            store.var_dir().join("index.json"),
            store.var_dir().join("menus").join(format!("{sha256}.json")),
            store.cache_dir().join("menus").join(format!("{sha256}.json")),
        ];
        for c in candidates.iter().filter(|c| c.exists()) {
            if let Ok(catalog) = Catalog::open(c, Some(&cache)) {
                if catalog.sha256() == sha256 {
                    return Ok(Self::with(store, catalog));
                }
            }
        }
        let source = source.ok_or_else(|| anyhow!("the pinned catalog isn't on this computer, and the lock doesn't say where it came from"))?;
        // Asked for lately, and the server had another under that name:
        // not asked again (every `nami run` would download it).
        let gone = store.cache_dir().join("menus").join(format!("{sha256}.gone"));
        let recent = fs::metadata(&gone).and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|e| e < GONE_FOR));
        if recent && fs::read_to_string(&gone).is_ok_and(|s| s == source) {
            bail!("{source} now serves a different catalog than the one pinned");
        }
        let raw = Self::read_source(store, source)?.ok_or_else(|| anyhow!("{source} doesn't exist any more"))?;
        let system_source = fs::read_to_string(store.var_dir().join("index-source")).unwrap_or_default();
        let resolved = fs::read_to_string(store.var_dir().join("index-resolved")).unwrap_or_default();
        let from_system = system_source.trim() == source || resolved.trim() == source;
        let unsigned_ok = from_system && fs::read_to_string(store.var_dir().join("index-unsigned-ok")).is_ok_and(|s| s.trim() == system_source.trim());
        match Self::read_source(store, &format!("{source}.sig"))? {
            Some(sig) => {
                crate::catalog::verify(&raw, &String::from_utf8_lossy(&sig), &crate::catalog::trusted_keys(store)).with_context(|| format!("{source}: signature check failed"))?;
            }
            None => ensure!(unsigned_ok, "{source} isn't signed, and this computer doesn't accept it unsigned"),
        }
        let mut reader: Box<dyn Read> = if source.ends_with(".xz") { Box::new(xz2::read::XzDecoder::new(&raw[..])) } else { Box::new(&raw[..]) };
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        if crate::catalog::sha256_hex(&data) != sha256 {
            let _ = fs::create_dir_all(store.cache_dir().join("menus")).and_then(|_| fs::write(&gone, source));
            bail!("{source} now serves a different catalog than the one pinned");
        }
        let made = crate::index_file::make(&data)?;
        let dir = store.cache_dir().join("menus");
        Self::keep_catalog(&dir, None, &data, &made.idx)?;
        Ok(Self::with(store, Catalog::open(&dir.join(format!("{sha256}.json")), Some(&cache))?))
    }

    /// Every icon a catalog names, (its name in the icons folder, sha256): its packages' (`gimp`),
    /// and on a Mac's, those of Homebrew's casks (`cask:google-chrome`, casks.rs's ICONS_ENTRY).
    fn icons_named(catalog: &Catalog) -> Vec<(String, String)> {
        let mut icons: Vec<(String, String)> = catalog.icons().into_iter().filter(|(a, _)| *a != crate::casks::ICONS_ENTRY).map(|(a, s)| (a.to_string(), s.to_string())).collect();
        if let Some(i) = catalog.find(crate::casks::ICONS_ENTRY) {
            icons.extend(crate::casks::named_icons(catalog.value(i)));
        }
        icons
    }

    /// App icons: each kept only if its sha256 is the one the (signed)
    /// index lists for it, so the server needn't be trusted. Only the ones
    /// not here already are fetched: a few, one at a time from the
    /// catalogs' shared objects (`menu/objects/<sha256>`); many, or from a
    /// server without those, as one icons.tar (next to the index, or
    /// wherever `nami update --icons` said).
    fn fetch_icons(store: &Store, from: &str, icons: &[(String, String)]) -> Result<()> {
        if icons.is_empty() {
            return Ok(());
        }
        let dest = store.var_dir().join("icons");
        let have = |attr: &str, sha: &str| {
            ["png", "svg"].iter().any(|ext| fs::read(dest.join(format!("{attr}.{ext}"))).is_ok_and(|b| crate::catalog::sha256_hex(&b) == sha))
        };
        let missing: Vec<&(String, String)> = icons.iter().filter(|(a, sha)| !have(a, sha)).collect();
        if missing.is_empty() {
            return Ok(());
        }
        let own_source = fs::read_to_string(store.var_dir().join("icons-source")).ok();
        let dir = from.rsplit_once('/').map(|(d, _)| d).unwrap_or(".");
        if own_source.is_none() && missing.len() <= ICONS_ONE_BY_ONE && dest.is_dir() {
            let objects = format!("{}/objects", dir.rsplit_once('/').map(|(d, _)| d).unwrap_or(".."));
            if Self::fetch_icons_one_by_one(store, &objects, &dest, icons, &missing) {
                crate::say!("{} app icons, {} new", icons.len(), missing.len());
                return Ok(());
            }
        }
        let source = match own_source {
            Some(s) => s.trim().to_string(),
            // Another platform's catalog (a Mac's: the icons of Homebrew's casks) has its own,
            // beside it: the release folder may be Linux's too.
            None if SYSTEM != "x86_64-linux" => format!("{dir}/{}", icons_tar()),
            None => {
                // Next to the index: 128px icons (published), else the full set.
                match Self::read_source(store, &format!("{dir}/icons-128.tar")) {
                    Ok(Some(_)) => format!("{dir}/icons-128.tar"),
                    _ => format!("{dir}/icons.tar"),
                }
            }
        };
        let tar = match Self::read_source(store, &source) {
            Ok(Some(tar)) => tar,
            Ok(None) => {
                crate::say!("warning: {source} is missing; apps show without icons");
                return Ok(());
            }
            Err(e) => {
                crate::say!("warning: no app icons ({e:#}); `nami update --refresh` tries again");
                return Ok(());
            }
        };
        let wanted: std::collections::HashMap<&str, &str> = icons.iter().map(|(a, s)| (a.as_str(), s.as_str())).collect();
        let fresh = store.var_dir().join("icons.new");
        let _ = fs::remove_dir_all(&fresh);
        fs::create_dir_all(&fresh)?;
        let mut kept = 0;
        for (name, body) in tar_entries(&tar) {
            let Some(attr) = name.strip_suffix(".png").or_else(|| name.strip_suffix(".svg")) else { continue };
            let want = wanted.get(attr).copied();
            if attr.contains('/') || attr.starts_with('.') || want.is_none() {
                continue;
            }
            if want == Some(crate::catalog::sha256_hex(body).as_str()) {
                fs::write(fresh.join(&name), body)?;
                kept += 1;
            } else {
                crate::say!("warning: icon for {attr} doesn't match the catalog; skipped");
            }
        }
        let _ = fs::remove_dir_all(&dest);
        fs::rename(&fresh, &dest)?;
        crate::say!("{kept} app icons");
        Ok(())
    }

    /// The `missing` icons, each from `objects/<sha256>`, into `dest`, and
    /// the icons the catalog no longer has taken out. False if any can't be
    /// had (a server without objects): then the whole set it is.
    fn fetch_icons_one_by_one(store: &Store, objects: &str, dest: &Path, icons: &[(String, String)], missing: &[&(String, String)]) -> bool {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let failed = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|s| {
            for _ in 0..8.min(missing.len()) {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((attr, sha)) = missing.get(i) else { break };
                    if failed.load(std::sync::atomic::Ordering::Relaxed) || attr.contains('/') || attr.starts_with('.') {
                        failed.store(true, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                    let ok = Self::read_source(store, &format!("{objects}/{sha}")).ok().flatten().filter(|b| crate::catalog::sha256_hex(b) == *sha).is_some_and(|body| {
                        let ext = if body.starts_with(b"\x89PNG\r\n\x1a\n") { "png" } else { "svg" };
                        let other = if ext == "png" { "svg" } else { "png" };
                        let _ = fs::remove_file(dest.join(format!("{attr}.{other}")));
                        write_atomic(&dest.join(format!("{attr}.{ext}")), &body).is_ok()
                    });
                    if !ok {
                        failed.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                });
            }
        });
        if failed.into_inner() {
            return false;
        }
        let wanted: HashSet<&str> = icons.iter().map(|(a, _)| a.as_str()).collect();
        for e in fs::read_dir(dest).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let attr = name.strip_suffix(".png").or_else(|| name.strip_suffix(".svg")).unwrap_or(&name);
            if !wanted.contains(attr) {
                let _ = fs::remove_file(e.path());
            }
        }
        true
    }

    /// An entry of the catalog, its JSON as the catalog has it, every field (a Mac's
    /// `casks::ICONS_ENTRY`, which no `Package` reads).
    pub fn raw(&self, name: &str) -> Option<&[u8]> {
        self.catalog.find(name).map(|i| self.catalog.value(i))
    }

    /// The catalog's icon for a package (PNG or SVG), once `nami update` fetched it.
    pub fn icon(&self, attr: &str) -> Option<std::path::PathBuf> {
        let i = self.catalog.find(attr)?;
        if self.catalog.record(i).icon.is_empty() {
            return None;
        }
        ["png", "svg"].iter().map(|ext| self.icons.join(format!("{attr}.{ext}"))).find(|p| p.exists())
    }

    /// A package by name, or a version of one: `firefox@150` is the newest
    /// 150.x the catalog or its history has, ready-built.
    pub fn get(&self, attr: &str) -> Result<&Package> {
        let (name, wanted) = split_version(attr);
        let Some(wanted) = wanted else { return self.get_plain(name) };
        if let Some(p) = self.history.picked.lock().unwrap().get(attr) {
            return Ok(p);
        }
        let catalog = self.get_plain(name);
        // The newest that fits, of: the catalog's own version, its history
        // (a package nixpkgs has since removed has only that: nodejs_20),
        // and its siblings' (nixpkgs keeps some versions under their own
        // names: `nodejs: 16` is nodejs_16's 16.20, newer than nodejs' 16.17).
        let mut found: Vec<&Package> = Vec::new();
        if let Ok(base) = catalog {
            if version_matches(&crate::query::version_of(&base.name), wanted) {
                found.push(base);
            }
        }
        let versions = match catalog {
            Ok(_) => self.history.list(name)?,
            Err(_) => self.history.list(name).unwrap_or_default(),
        };
        if let Some(v) = newest_fitting(&versions, wanted) {
            let base = catalog.as_ref().map_or_else(|_| bare(name), |b| (*b).clone());
            found.push(from_history(&base, v).with_context(|| format!("{name} {wanted}"))?);
        }
        found.extend(self.sibling(name, wanted));
        if let Some(p) = found.into_iter().max_by_key(|p| version_key(&crate::query::version_of(&p.name))) {
            let p: &'static Package = Box::leak(Box::new(p.clone()));
            self.history.picked.lock().unwrap().insert(attr.to_string(), p);
            return Ok(p);
        }
        let base = catalog?;
        let mut have: Vec<String> = vec![crate::query::version_of(&base.name)];
        have.extend(versions.iter().map(|v| v.0.clone()));
        // And the ones under their own names (nodejs_22).
        have.extend(self.siblings(name).iter().filter_map(|s| self.get_plain(s).ok()).map(|p| crate::query::version_of(&p.name)));
        bail!("{name} {wanted}: not a version the catalog has (it has {}); `nami versions {name}` lists them", version_lines(have))
    }

    /// Packages named like `name` with a version number in the name
    /// (nodejs: nodejs_20, nodejs_22; python3: python312), as the catalog has them.
    pub fn siblings(&self, name: &str) -> Vec<&str> {
        let stem = name.trim_end_matches(|c: char| c.is_ascii_digit());
        let c = &self.catalog;
        (c.first_from(stem)..c.len())
            .map(|i| c.name(i))
            .take_while(|a| a.starts_with(stem))
            .filter(|a| *a != name)
            .filter(|a| a.strip_prefix(stem).is_some_and(|rest| {
                let rest = rest.strip_prefix('_').unwrap_or(rest);
                rest.starts_with(|c: char| c.is_ascii_digit()) && rest.chars().all(|c| c.is_ascii_digit() || c == '_')
            }))
            .collect()
    }

    /// The newest `wanted` of `name`'s siblings: the catalog's (their own
    /// versions, then their histories, for the ones whose number fits:
    /// nodejs_20 for 20.1, python312 for 3.12.4), and ones nixpkgs has
    /// since removed (nodejs_16, python38), named the usual ways.
    fn sibling(&self, name: &str, wanted: &str) -> Option<&Package> {
        let siblings = self.siblings(name);
        let mut found: Vec<&Package> = siblings
            .iter()
            .filter_map(|s| self.get_plain(s).ok())
            .filter(|p| version_matches(&crate::query::version_of(&p.name), wanted))
            .collect();
        let digits: String = wanted.chars().filter(char::is_ascii_digit).collect();
        let stem = name.trim_end_matches(|c: char| c.is_ascii_digit());
        let parts: Vec<&str> = wanted.split('.').collect();
        let mut candidates: Vec<String> = siblings
            .iter()
            .filter(|s| digits.starts_with(&s[stem.len()..].chars().filter(char::is_ascii_digit).collect::<String>()))
            .map(|s| s.to_string())
            .collect();
        let (major, minor) = (parts[0], parts.get(1).copied());
        candidates.extend([format!("{name}_{major}"), format!("{stem}_{major}"), format!("{stem}{major}")]);
        if let Some(minor) = minor {
            candidates.extend([format!("{stem}{major}{minor}"), format!("{stem}_{major}_{minor}"), format!("{stem}{major}_{minor}")]);
        }
        let mut tried = HashSet::new();
        for c in candidates {
            if c == name || !tried.insert(c.clone()) {
                continue;
            }
            let Ok(versions) = self.history.list(&c) else { continue };
            if let Some(v) = newest_fitting(&versions, wanted) {
                // Described as the one asked for, if it's not in the catalog.
                let base = self.get_plain(&c).or_else(|_| self.get_plain(name)).cloned().unwrap_or_else(|_| bare(&c));
                found.extend(from_history(&base, v).ok());
            }
        }
        found.into_iter().max_by_key(|p| version_key(&crate::query::version_of(&p.name)))
    }

    /// Every version of `attr` there is, newest first: the catalog's own
    /// (date and revision empty), then its history's.
    pub fn versions(&self, attr: &str) -> Result<Vec<Version>> {
        let mut out = Vec::new();
        match self.get_plain(attr) {
            Ok(base) => out.push(Version(crate::query::version_of(&base.name), String::new(), String::new(), BTreeMap::new())),
            // Not in nixpkgs any more: its history only, if it has one.
            Err(e) if self.history.list(attr).map_or(true, |h| h.is_empty()) => return Err(e),
            Err(_) => {}
        }
        for v in self.history.list(attr)? {
            if !out.iter().any(|have| have.0 == v.0) {
                out.push(v);
            }
        }
        Ok(out)
    }

    /// `attr`'s versions as `versions` lists them, without the network (tab
    /// completion): None when its history isn't here yet.
    pub fn versions_here(&self, attr: &str) -> Option<Vec<Version>> {
        let earlier = self.history.list_here(attr)?;
        let mut out: Vec<Version> = self.find(attr).map(|base| Version(crate::query::version_of(&base.name), String::new(), String::new(), BTreeMap::new())).into_iter().collect();
        for v in earlier {
            if !out.iter().any(|have| have.0 == v.0) {
                out.push(v);
            }
        }
        Some(out)
    }

    /// The `i`th package (in name order), read from the catalog the first
    /// time it's asked for.
    pub fn package_at(&self, i: usize) -> Option<&Package> {
        self.parsed.get(i)?.get_or_init(|| serde_json::from_slice(self.catalog.value(i)).ok().map(Box::new)).as_deref()
    }

    /// A package by its exact name (no versions, no suggestions).
    pub fn find(&self, attr: &str) -> Option<&Package> {
        self.package_at(self.catalog.find(attr)?)
    }

    fn get_plain(&self, attr: &str) -> Result<&Package> {
        self.find(attr).ok_or_else(|| {
            let close: Vec<&str> = self.search(attr).take(5).map(|(a, _)| a).collect();
            let hint = format!("`nami search {attr}` looks for it.");
            if close.is_empty() {
                crate::errors::coded_hint(crate::errors::Code::NotFound, format!("unknown package {attr:?}"), hint)
            } else {
                crate::errors::coded_hint(crate::errors::Code::NotFound, format!("unknown package {attr:?}; did you mean: {}", close.join(", ")), hint)
            }
        })
    }

    pub fn len(&self) -> usize {
        self.catalog.len()
    }

    /// How many of them this computer can install (a Mac's ready-built ones): None where the
    /// catalog's compact form doesn't say (made by an older nami).
    pub fn installable(&self) -> Option<usize> {
        self.catalog.installable()
    }

    /// Every package's name, in order (nothing read but the names).
    pub fn names(&self) -> impl Iterator<Item = &str> {
        (0..self.catalog.len()).map(|i| self.catalog.name(i))
    }

    /// What search looks at of every package, by its place and name.
    pub fn records(&self) -> impl Iterator<Item = (usize, &str, crate::index_file::Record<'_>)> {
        (0..self.catalog.len()).map(|i| (i, self.catalog.name(i), self.catalog.record(i)))
    }

    /// Every package (each read from the catalog: slow; `names` and
    /// `records` aren't).
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Package)> {
        (0..self.catalog.len()).filter_map(|i| Some((self.catalog.name(i), self.package_at(i)?)))
    }

    pub fn search<'a>(&'a self, term: &'a str) -> impl Iterator<Item = (&'a str, &'a Package)> + 'a {
        let term = term.to_lowercase();
        (0..self.catalog.len())
            .filter(move |i| self.catalog.name(*i).to_lowercase().contains(&term))
            .filter_map(|i| Some((self.catalog.name(i), self.package_at(i)?)))
    }
}

/// The new catalog from the one here (`old`) and a patch from the server
/// (zstd, `--patch-from` the old one), in one go: as much memory as the new
/// catalog, no more.
fn apply_patch(old: &[u8], patch: &[u8]) -> Result<Vec<u8>> {
    use zstd::zstd_safe::{DCtx, DParameter};
    let size = zstd::zstd_safe::get_frame_content_size(patch).ok().flatten().ok_or_else(|| anyhow!("the patch doesn't say how big the catalog is"))?;
    ensure!(size < 1 << 32, "the patched catalog would be too big");
    let mut out: Vec<u8> = Vec::with_capacity(size as usize);
    let mut dctx = DCtx::create();
    dctx.set_parameter(DParameter::WindowLogMax(31)).map_err(|e| anyhow!("zstd: {}", zstd::zstd_safe::get_error_name(e)))?;
    dctx.ref_prefix(old).map_err(|e| anyhow!("zstd: {}", zstd::zstd_safe::get_error_name(e)))?;
    dctx.decompress(&mut out, patch).map_err(|e| anyhow!("zstd: {}", zstd::zstd_safe::get_error_name(e)))?;
    ensure!(out.len() as u64 == size, "the patch made a catalog of the wrong size");
    Ok(out)
}

/// The patch from catalog `old` to `new`, as `apply_patch` reads it (for
/// `nami catalog patch`, which packaging/publish.sh runs).
pub fn make_patch(old: &[u8], new: &[u8]) -> Result<Vec<u8>> {
    Ok(crate::bundle_delta::make(old, new, 19)?)
}

/// The regular files in an (uncompressed, ustar) tar archive.
fn tar_entries(data: &[u8]) -> Vec<(String, &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 512 <= data.len() {
        let h = &data[at..at + 512];
        if h.iter().all(|&b| b == 0) {
            break;
        }
        let field = |r: std::ops::Range<usize>| String::from_utf8_lossy(&h[r]).trim_end_matches('\0').trim().to_string();
        let (name, prefix) = (field(0..100), field(345..500));
        let size = usize::from_str_radix(&field(124..136), 8).unwrap_or(0);
        let start = at + 512;
        let Some(body) = data.get(start..start + size) else { break };
        if matches!(h[156], b'0' | 0) {
            out.push((if prefix.is_empty() { name } else { format!("{prefix}/{name}") }, body));
        }
        at = start + size.div_ceil(512) * 512;
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_version_histories_are_next_to_the_catalog() {
        assert_eq!(super::versions_next_to(&format!("https://x/menu/abc/index-{}.json.xz\n", crate::index::SYSTEM)), Some(format!("https://x/menu/abc/{}", super::beside("versions"))));
        assert_eq!(super::versions_next_to("index.json"), None);
    }

    use super::*;

    /// A catalog with one nixpkgs package and one extra app from our cache.
    fn catalog(cache_url: &str, key: &str) -> String {
        serde_json::json!({
            "hello": {"name": "hello-2.12", "outputs": {"out": "/nix/store/00000000000000000000000000000000-hello-2.12"}},
            "wordcraft": {"name": "wordcraft-0.5.0", "outputs": {"out": "/nix/store/11111111111111111111111111111111-wordcraft-0.5.0"},
                "apps": ["wordcraft.desktop"], "publisher": "Storytold", "cache": {"url": cache_url, "key": key, "priority": 50}},
        })
        .to_string()
    }

    #[test]
    fn versions() {
        assert_eq!(split_version("firefox@150"), ("firefox", Some("150")));
        assert_eq!(split_version("github:o/r@v2"), ("github:o/r@v2", None));
        assert_eq!(split_version("@angular/cli"), ("@angular/cli", None));
        assert_eq!(split_version("./tool@1"), ("./tool@1", None));
        assert!(version_matches("150.0.1", "150") && version_matches("150", "150") && version_matches("3.12.4", "3.12.4"));
        assert!(!version_matches("1500", "150") && !version_matches("3.12.40", "3.12.4"));
        let s = |v: &[&str]| version_lines(v.iter().map(|v| v.to_string()).collect());
        assert_eq!(s(&["24.21.0", "18.14.1", "22.1.0", "24.20.0", "22.1.0"]), "24, 22, 18, 4 versions in all");
        assert_eq!(s(&["3.12.11", "3.13.7", "3.13.6"]), "3.13, 3.12, 3 versions in all");

        // A signed catalog whose hello has had 2.12.1 and 2.10 before: its
        // history is in one shard next to the index, signed the same way.
        let root = std::env::temp_dir().join(format!("nami-versions-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = Store::open_direct(&root, false).unwrap();
        let key = crate::push::keygen(&store, "tray-1").unwrap();
        crate::catalog::trust(&store, &key).unwrap();
        let history = serde_json::json!({"hello": [
            ["2.12.1", "2026-05-31", "aaaa", {"out": "/nix/store/22222222222222222222222222222222-hello-2.12.1"}],
            ["2.10", "2025-01-31", "bbbb", {"out": "/nix/store/33333333333333333333333333333333-hello-2.10"}],
        ]})
        .to_string();
        let shard = &crate::catalog::sha256_hex(b"hello")[..3];
        fs::create_dir_all(root.join(format!("cat/{}", crate::index::beside("versions")))).unwrap();
        fs::write(root.join(format!("cat/{}/{shard}.json", crate::index::beside("versions"))), &history).unwrap();
        fs::write(root.join(format!("cat/{}/abc.json", crate::index::beside("versions"))), "{}").unwrap();
        // nodejs_20 isn't in nixpkgs any more: only in the history.
        let node20 = &crate::catalog::sha256_hex(b"nodejs_20")[..3];
        assert_ne!(node20, shard);
        fs::write(
            root.join(format!("cat/{}/{node20}.json", crate::index::beside("versions"))),
            serde_json::json!({"nodejs_20": [["20.19.5", "2026-04-30", "cccc", {"out": "/nix/store/55555555555555555555555555555555-nodejs-20.19.5"}]]}).to_string(),
        )
        .unwrap();
        crate::catalog::sign(&store, &root.join(format!("cat/{}", crate::index::beside("versions"))), Some("tray-1")).unwrap();
        let catalog = serde_json::json!({
            "hello": {"name": "hello-2.12.3", "outputs": {"out": "/nix/store/00000000000000000000000000000000-hello-2.12.3"}},
            "jq": {"name": "jq-1.8", "outputs": {"out": "/nix/store/44444444444444444444444444444444-jq-1.8"}},
            "nodejs": {"name": "nodejs-24.1.0", "mainProgram": "node", "outputs": {"out": "/nix/store/66666666666666666666666666666666-nodejs-24.1.0"}},
            "nodejs_22": {"name": "nodejs-22.5.0", "outputs": {"out": "/nix/store/77777777777777777777777777777777-nodejs-22.5.0"}},
        });
        let file = root.join(&format!("cat/index-{}.json", crate::index::SYSTEM));
        fs::write(&file, catalog.to_string()).unwrap();
        crate::catalog::sign(&store, &file, Some("tray-1")).unwrap();
        Index::update(&store, file.to_str().unwrap(), false).unwrap();
        let index = Index::load(&store).unwrap();

        assert_eq!(index.get("hello@2.12").unwrap().name, "hello-2.12.3", "the catalog's own, when it fits");
        assert_eq!(index.get("hello@2.12.1").unwrap().name, "hello-2.12.1");
        assert_eq!(index.get("hello@2.12.1").unwrap().install_paths(), ["/nix/store/22222222222222222222222222222222-hello-2.12.1"]);
        assert_eq!(index.get("hello@2").unwrap().name, "hello-2.12.3", "the newest 2.x");
        assert_eq!(index.get("hello@2.10").unwrap().name, "hello-2.10");
        let err = format!("{:#}", index.get("hello@3").unwrap_err());
        assert!(err.contains("it has 2.12, 2.10, 3 versions in all"), "{err}");
        assert!(index.get("jq@1.7").is_err(), "no history: only its own version");
        // Versions nixpkgs keeps under their own names: the catalog's
        // nodejs_22, and nodejs_20, since removed (its history only).
        assert_eq!(index.get("nodejs@22").unwrap().name, "nodejs-22.5.0");
        let n20 = index.get("nodejs@20").unwrap();
        assert_eq!((n20.name.as_str(), n20.main_program.as_deref()), ("nodejs-20.19.5", Some("node")), "described as nodejs");
        assert_eq!(index.get("nodejs_20@20.19").unwrap().name, "nodejs-20.19.5");
        assert!(index.get("nodejs_20").is_err(), "not in the catalog without a version");
        assert_eq!(index.versions("nodejs_20").unwrap().len(), 1);
        assert_eq!(index.siblings("nodejs"), ["nodejs_22"]);
        assert!(version_key("1.1.1w") > version_key("1.1.1s") && version_key("1.1.1s") > version_key("1.1.1"));
        assert!(version_key("16.20.0") > version_key("16.9.1"));
        let listed: Vec<String> = index.versions("hello").unwrap().into_iter().map(|v| v.0).collect();
        assert_eq!(listed, ["2.12.3", "2.12.1", "2.10"]);
        // Only the shards asked for came down.
        let cached: Vec<String> = fs::read_dir(store.cache_dir().join("versions")).unwrap().flatten()
            .flat_map(|d| fs::read_dir(d.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned())).collect();
        assert!(cached.contains(&format!("{shard}.json")) && !cached.contains(&"abc.json".to_string()), "{cached:?}");

        // A shard changed on the server (not what was signed) is refused.
        fs::write(root.join(format!("cat/{}/{shard}.json", crate::index::beside("versions"))), history.replace("2.10", "6.66")).unwrap();
        let _ = fs::remove_dir_all(store.cache_dir().join("versions"));
        let index = Index::load(&store).unwrap();
        assert!(format!("{:#}", index.get("hello@2.10").unwrap_err()).contains("signature check failed"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_mirror_counts_only_what_is_signed() {
        let root = std::env::temp_dir().join(format!("nami-mirror-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = Store::open_direct(&root, false).unwrap();
        // What a mirror has counts only signed: had, not had, and badly signed (an error, never
        // "not had", so nothing is quietly left out).
        let mirror = root.join("mirror");
        fs::create_dir_all(&mirror).unwrap();
        fs::write(mirror.join("knhpzwcavw3d68lcq14f71d4ag8ykgz1.narinfo"), crate::narinfo::tests::RIPGREP).unwrap();
        fs::write(mirror.join("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.narinfo"), crate::narinfo::tests::RIPGREP.replace("knhpzwcavw3d68lcq14f71d4ag8ykgz1", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")).unwrap();
        fs::write(store.var_dir().join("nixos-mirror"), format!("file://{}", mirror.display())).unwrap();
        store.reload_caches().unwrap();
        assert!(store.in_a_cache("knhpzwcavw3d68lcq14f71d4ag8ykgz1-ripgrep-15.2.0").unwrap());
        assert!(!store.in_a_cache("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-ripgrep-15.2.0").unwrap());
        let e = store.in_a_cache("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-ripgrep-15.2.0").unwrap_err();
        assert!(format!("{e:#}").contains("signature doesn't check out"), "{e:#}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn catalog_caches_only_when_signed() {
        let root = std::env::temp_dir().join(format!("nami-catalog-caches-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = Store::open_direct(&root, false).unwrap();
        let apps_key = crate::push::keygen(&store, "tray-apps-1").unwrap();
        let cat_key = crate::push::keygen(&store, "tray-1").unwrap();
        crate::catalog::trust(&store, &cat_key).unwrap();
        let file = root.join(&format!("index-{}.json", crate::index::SYSTEM));
        let ours = Cache { url: "https://catalog.example.org/cache".into(), key: apps_key.clone() };

        // Signed by a trusted key: its cache comes after cache.nixos.org.
        fs::write(&file, catalog("https://catalog.example.org/cache/", &apps_key)).unwrap();
        crate::catalog::sign(&store, &file, Some("tray-1")).unwrap();
        Index::update(&store, file.to_str().unwrap(), false).unwrap();
        let caches = store.caches();
        assert_eq!(caches[0].url, crate::store::NIXOS_CACHE);
        assert!(caches.contains(&ours), "{caches:?}");
        // A mirror of cache.nixos.org takes its place, with cache.nixos.org's key.
        fs::write(store.var_dir().join("nixos-mirror"), "http://127.0.0.1:5480/\n").unwrap();
        store.reload_caches().unwrap();
        let caches = store.caches();
        assert_eq!((caches[0].url.as_str(), caches[0].key.as_str()), ("http://127.0.0.1:5480", crate::store::NIXOS_CACHE_KEY));
        assert!(!caches.iter().any(|c| c.url == crate::store::NIXOS_CACHE), "{caches:?}");
        fs::remove_file(store.var_dir().join("nixos-mirror")).unwrap();
        store.reload_caches().unwrap();
        let index = Index::load(&store).unwrap();
        assert_eq!(index.get("wordcraft").unwrap().publisher.as_deref(), Some("Storytold"));
        // ... and the daemon sees it too (it re-reads per request).
        assert!(Store::open_direct_readonly(&root).unwrap().caches().contains(&ours));

        // Changed after signing: refused, and the trusted caches stay as they were.
        fs::write(&file, catalog("https://evil.example/cache", &apps_key)).unwrap();
        assert!(Index::update(&store, file.to_str().unwrap(), false).is_err());
        assert!(!store.caches().iter().any(|c| c.url.contains("evil")));

        // Unsigned (--unsigned): the catalog loads, but names no caches.
        fs::remove_file(root.join(&format!("index-{}.json.sig", crate::index::SYSTEM))).unwrap();
        Index::update(&store, file.to_str().unwrap(), true).unwrap();
        assert_eq!(store.caches().len(), 1);
        assert!(!store.var_dir().join(crate::store::CATALOG_CACHES).exists());

        // Signed by a key this machine doesn't trust: refused.
        crate::push::keygen(&store, "stranger-1").unwrap();
        crate::catalog::sign(&store, &file, Some("stranger-1")).unwrap();
        assert!(Index::update(&store, file.to_str().unwrap(), false).is_err());
        assert_eq!(store.caches().len(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn catalog_caches_are_web_caches_with_keys() {
        let packages: BTreeMap<String, Package> = serde_json::from_str(&catalog("/etc", "k:x")).unwrap();
        assert!(catalog_caches(packages.values()).is_empty());
        let key = format!("tray-apps-1:{}", "A".repeat(43) + "=");
        let packages: BTreeMap<String, Package> = serde_json::from_str(&catalog("https://catalog.example.org/cache", &key)).unwrap();
        assert_eq!(catalog_caches(packages.values()), vec![Cache { url: "https://catalog.example.org/cache".into(), key }]);
    }

    /// A ustar archive of `files`, as publish-icons.py writes icons-128.tar.
    fn tar(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, body) in files {
            let mut h = [0u8; 512];
            h[..name.len()].copy_from_slice(name.as_bytes());
            h[124..136].copy_from_slice(format!("{:011o}\0", body.len()).as_bytes());
            h[156] = b'0';
            out.extend(h);
            out.extend(*body);
            out.resize(out.len().div_ceil(512) * 512, 0);
        }
        out.extend([0u8; 1024]);
        out
    }

    /// A catalog published as packaging/publish.sh does, in `site`: the
    /// channel `chan` pointing at `menu/<rev>`, its signed .json.xz and
    /// icons-128.tar; `new_server`: its signed JSON, and its icons kept by
    /// content in menu/objects/. Returns its JSON.
    fn publish(store: &Store, site: &Path, rev: &str, packages: &serde_json::Value, icons: &[(&str, &[u8])], new_server: bool) -> Vec<u8> {
        let dir = site.join("menu").join(rev);
        fs::create_dir_all(&dir).unwrap();
        let json = packages.to_string().into_bytes();
        let xz_file = dir.join(&format!("index-{}.json.xz", crate::index::SYSTEM));
        let mut xz = xz2::write::XzEncoder::new(Vec::new(), 6);
        std::io::Write::write_all(&mut xz, &json).unwrap();
        fs::write(&xz_file, xz.finish().unwrap()).unwrap();
        crate::catalog::sign(store, &xz_file, Some("tray-1")).unwrap();
        let named: Vec<(String, &[u8])> = icons.iter().map(|(a, b)| (format!("{a}.png"), *b)).collect();
        fs::write(dir.join(icons_tar()), tar(&named.iter().map(|(n, b)| (n.as_str(), *b)).collect::<Vec<_>>())).unwrap();
        if new_server {
            let json_file = dir.join(&format!("index-{}.json", crate::index::SYSTEM));
            fs::write(&json_file, &json).unwrap();
            crate::catalog::sign(store, &json_file, Some("tray-1")).unwrap();
            fs::remove_file(&json_file).unwrap();
            fs::create_dir_all(site.join("menu/objects")).unwrap();
            for (_, body) in icons {
                fs::write(site.join("menu/objects").join(crate::catalog::sha256_hex(body)), body).unwrap();
            }
        }
        fs::write(site.join("menu/chan"), rev).unwrap();
        json
    }

    fn packages(version: &str, icons: &[(&str, &[u8])]) -> serde_json::Value {
        let mut p = serde_json::json!({
            "hello": {"name": format!("hello-{version}"), "outputs": {"out": format!("/nix/store/00000000000000000000000000000000-hello-{version}")}, "description": "Says hello"},
            "jq": {"name": "jq-1.8", "outputs": {"out": "/nix/store/44444444444444444444444444444444-jq-1.8"}},
        });
        for (attr, body) in icons {
            p[*attr] = serde_json::json!({"name": format!("{attr}-1"), "outputs": {}, "apps": [format!("{attr}.desktop")], "icon": crate::catalog::sha256_hex(body)});
        }
        p
    }

    #[test]
    fn a_new_catalog_comes_as_a_patch_or_whole() {
        let root = std::env::temp_dir().join(format!("nami-catalog-patch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = Store::open_direct(&root, false).unwrap();
        let key = crate::push::keygen(&store, "tray-1").unwrap();
        crate::catalog::trust(&store, &key).unwrap();
        let site = root.join("site");
        let chan = site.join("menu/chan");
        let chan = chan.to_str().unwrap();
        let installed = || fs::read(store.var_dir().join("index.json")).unwrap();
        let icon = |attr: &str| fs::read(store.var_dir().join(format!("icons/{attr}.png"))).ok();

        // From a server as it was (no signed JSON, no objects): whole, and
        // its icons as one tar.
        let icons1: &[(&str, &[u8])] = &[("gimp", b"\x89PNG\r\n\x1a\ngimp1"), ("zed", b"\x89PNG\r\n\x1a\nzed1")];
        let one = publish(&store, &site, "r1", &packages("2.12", icons1), icons1, false);
        assert_eq!(Index::update(&store, chan, false).unwrap(), 4);
        assert_eq!(installed(), one);
        assert_eq!(icon("zed").as_deref(), Some(&b"\x89PNG\r\n\x1a\nzed1"[..]));
        let index = Index::load(&store).unwrap();
        assert_eq!(index.get("hello").unwrap().name, "hello-2.12");
        assert!(index.icon("gimp").is_some());

        // A newer server: a patch from the catalog here, and only the icon
        // that changed, by its sha256. (Neither the whole catalog nor the
        // tar is there to be had: they're not what's used.)
        let icons2: &[(&str, &[u8])] = &[("gimp", b"\x89PNG\r\n\x1a\ngimp1"), ("zed", b"\x89PNG\r\n\x1a\nzed2")];
        let two = publish(&store, &site, "r2", &packages("2.13", icons2), icons2, true);
        fs::create_dir_all(site.join("menu/r2/patches")).unwrap();
        fs::write(site.join(format!("menu/r2/patches/{}.zst", crate::catalog::sha256_hex(&one))), make_patch(&one, &two).unwrap()).unwrap();
        fs::remove_file(site.join(&format!("menu/r2/index-{}.json.xz", crate::index::SYSTEM))).unwrap();
        fs::remove_file(site.join("menu/r2").join(icons_tar())).unwrap();
        fs::remove_file(site.join(format!("menu/objects/{}", crate::catalog::sha256_hex(icons2[0].1)))).unwrap();
        assert_eq!(Index::update(&store, chan, false).unwrap(), 4);
        assert_eq!(installed(), two);
        assert_eq!(icon("zed").as_deref(), Some(&b"\x89PNG\r\n\x1a\nzed2"[..]));
        assert_eq!(icon("gimp").as_deref(), Some(&b"\x89PNG\r\n\x1a\ngimp1"[..]));
        assert_eq!(Index::load(&store).unwrap().get("hello").unwrap().name, "hello-2.13");
        assert!(store.var_dir().join(format!("menus/{}.json", crate::catalog::sha256_hex(&two))).exists(), "kept for locks");

        // The same catalog again: nothing to download.
        fs::remove_dir_all(site.join("menu/r2/patches")).unwrap();
        assert_eq!(Index::refresh(&store).unwrap(), 4);
        assert_eq!(installed(), two);

        // A patch that doesn't make the catalog signed: the whole one, and
        // a new icon without its object: the tar.
        let icons3: &[(&str, &[u8])] = &[("gimp", b"\x89PNG\r\n\x1a\ngimp3"), ("zed", b"\x89PNG\r\n\x1a\nzed2")];
        let three = publish(&store, &site, "r3", &packages("2.14", icons3), icons3, true);
        fs::create_dir_all(site.join("menu/r3/patches")).unwrap();
        fs::write(site.join(format!("menu/r3/patches/{}.zst", crate::catalog::sha256_hex(&two))), make_patch(&two, &one).unwrap()).unwrap();
        fs::remove_file(site.join(format!("menu/objects/{}", crate::catalog::sha256_hex(icons3[0].1)))).unwrap();
        assert_eq!(Index::update(&store, chan, false).unwrap(), 4);
        assert_eq!(installed(), three);
        assert_eq!(icon("gimp").as_deref(), Some(&b"\x89PNG\r\n\x1a\ngimp3"[..]));

        // The JSON's signature from a key this computer doesn't trust: refused,
        // and then so is the whole catalog (its own signature, the same key).
        crate::push::keygen(&store, "stranger-1").unwrap();
        let four = packages("2.15", &[]);
        let dir = site.join("menu/r4");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(&format!("index-{}.json", crate::index::SYSTEM)), four.to_string()).unwrap();
        crate::catalog::sign(&store, &dir.join(&format!("index-{}.json", crate::index::SYSTEM)), Some("stranger-1")).unwrap();
        fs::create_dir_all(dir.join("patches")).unwrap();
        fs::write(dir.join(format!("patches/{}.zst", crate::catalog::sha256_hex(&three))), make_patch(&three, four.to_string().as_bytes()).unwrap()).unwrap();
        fs::rename(dir.join(&format!("index-{}.json", crate::index::SYSTEM)), dir.join(&format!("index-{}.json.xz", crate::index::SYSTEM))).unwrap();
        crate::catalog::sign(&store, &dir.join(&format!("index-{}.json.xz", crate::index::SYSTEM)), Some("stranger-1")).unwrap();
        fs::write(&site.join("menu/chan"), "r4").unwrap();
        assert!(Index::update(&store, chan, false).is_err());
        assert_eq!(installed(), three);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_objects_are_beside_the_catalogs_release() {
        let dir = std::env::temp_dir().join(format!("nami-objects-url-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(objects_url(&dir), None);
        fs::write(dir.join("index-resolved"), "https://namios.dev/menu/19cfded70b91/index-aarch64-darwin.json.xz\n").unwrap();
        assert_eq!(objects_url(&dir).as_deref(), Some("https://namios.dev/menu/objects"));
        fs::write(dir.join("index-resolved"), "/home/ada/out/index-aarch64-darwin.json.xz").unwrap();
        assert_eq!(objects_url(&dir), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_macs_catalog_names_the_icons_of_homebrews_casks() {
        let root = std::env::temp_dir().join(format!("nami-catalog-cask-icons-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = Store::open_direct(&root, false).unwrap();
        let key = crate::push::keygen(&store, "tray-1").unwrap();
        crate::catalog::trust(&store, &key).unwrap();
        let site = root.join("site");
        let chan = site.join("menu/chan");
        let chan = chan.to_str().unwrap();
        let icon = |name: &str| fs::read(store.var_dir().join(format!("icons/{name}.png"))).ok();
        let sha = crate::catalog::sha256_hex;
        let catalog = |icons: &[(&str, &[u8])]| {
            let mut p = packages("1", &[]);
            let named: serde_json::Map<String, serde_json::Value> = icons.iter().map(|(t, b)| (t.to_string(), sha(b).into())).collect();
            p[crate::casks::ICONS_ENTRY] = serde_json::json!({"name": "nami-cask-icons-1", "outputs": {}, "uncached": true, "cask_icons": named});
            p
        };
        let (rect1, rect2, iterm, other): (&[u8], &[u8], &[u8], &[u8]) = (b"\x89PNG\r\n\x1a\nrect1", b"\x89PNG\r\n\x1a\nrect2", b"\x89PNG\r\n\x1a\niterm", b"\x89PNG\r\n\x1a\nother");
        // In one piece: those the signed catalog names, as cask:<token>.png; one it doesn't, or
        // of other bytes than it says, isn't kept.
        let named: &[(&str, &[u8])] = &[("rectangle", rect1), ("iterm2", iterm), ("zoom", rect1)];
        let files: &[(&str, &[u8])] = &[("cask:rectangle", rect1), ("cask:iterm2", iterm), ("cask:zoom", other), ("cask:slack", other)];
        publish(&store, &site, "r1", &catalog(named), files, false);
        assert_eq!(Index::update(&store, chan, false).unwrap(), 3);
        assert_eq!(icon("cask:rectangle").as_deref(), Some(rect1));
        assert_eq!(icon("cask:iterm2").as_deref(), Some(iterm));
        assert_eq!(icon("cask:zoom"), None);
        assert_eq!(icon("cask:slack"), None);
        // It's no package: nothing to install, nothing for search.
        let index = Index::load(&store).unwrap();
        assert!(index.get(crate::casks::ICONS_ENTRY).is_ok_and(|p| p.outputs.is_empty() && p.uncached));
        assert!(index.records().all(|(_, a, r)| a != crate::casks::ICONS_ENTRY || r.icon.is_empty()));
        drop(index);
        // A newer one: only the icon that changed, by its sha256 (not the tar); one it no
        // longer names goes.
        let named: &[(&str, &[u8])] = &[("rectangle", rect2), ("zoom", rect1)];
        let files: &[(&str, &[u8])] = &[("cask:rectangle", rect2), ("cask:zoom", rect1)];
        publish(&store, &site, "r2", &catalog(named), files, true);
        fs::remove_file(site.join("menu/r2").join(icons_tar())).unwrap();
        assert_eq!(Index::update(&store, chan, false).unwrap(), 3);
        assert_eq!(icon("cask:rectangle").as_deref(), Some(rect2));
        assert_eq!(icon("cask:zoom").as_deref(), Some(rect1));
        assert_eq!(icon("cask:iterm2"), None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_pinned_catalog_the_server_no_longer_has_isnt_fetched_every_time() {
        let root = std::env::temp_dir().join(format!("nami-catalog-gone-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = Store::open_direct(&root, false).unwrap();
        let key = crate::push::keygen(&store, "tray-1").unwrap();
        crate::catalog::trust(&store, &key).unwrap();
        let site = root.join("site");
        publish(&store, &site, "r1", &packages("2.12", &[]), &[], false);
        let source = site.join(&format!("menu/r1/index-{}.json.xz", crate::index::SYSTEM));
        let source = source.to_str().unwrap();
        let pinned = "ab".repeat(32);
        let err = format!("{:#}", Index::load_pinned(&store, &pinned, Some(source)).err().unwrap());
        assert!(err.contains("now serves a different catalog"), "{err}");
        // Not asked again: it would say it's gone otherwise.
        fs::remove_file(source).unwrap();
        let err = format!("{:#}", Index::load_pinned(&store, &pinned, Some(source)).err().unwrap());
        assert!(err.contains("now serves a different catalog"), "{err}");
        // The pinned one, found where it's kept, without reading the source.
        let json = packages("2.12", &[]).to_string();
        fs::create_dir_all(store.cache_dir().join("menus")).unwrap();
        let sha = crate::catalog::sha256_hex(json.as_bytes());
        fs::write(store.cache_dir().join(format!("menus/{sha}.json")), &json).unwrap();
        assert_eq!(Index::load_pinned(&store, &sha, Some(source)).unwrap().get("hello").unwrap().name, "hello-2.12");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reads_tar() {
        // A one-file archive as Python's tarfile writes it.
        let mut h = [0u8; 512];
        h[..9].copy_from_slice(b"hello.png");
        h[124..136].copy_from_slice(b"00000000005\0");
        h[156] = b'0';
        let mut data = h.to_vec();
        let mut body = b"world".to_vec();
        body.resize(512, 0);
        data.extend(body);
        data.extend([0u8; 1024]);
        let e = super::tar_entries(&data);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].0, "hello.png");
        assert_eq!(e[0].1, b"world");
    }
}
