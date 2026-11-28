//! A project's dependencies, fetched by nami as its own lock file pins
//! them, so its build runs without the network (build_kinds.rs):
//!
//! - Rust: each crate in Cargo.lock from crates.io, checked against the
//!   sha256 Cargo.lock gives, unpacked into a vendor folder Cargo reads
//!   (nixpkgs' importCargoLock). A git dependency comes as its commit's
//!   tarball from GitHub, GitLab or Codeberg, its sha256 recorded in the
//!   recipe's nami.lock the first time.
//! - Go: each module in go.sum from the Go module proxy, checked against
//!   go.sum's hash of it (Go's own `h1:`), laid out as a module proxy in a
//!   folder (`GOPROXY=file://...`). go.sum already pins every module, so
//!   there's no vendorHash to work out.
//! - Node.js: each package in package-lock.json, checked against its
//!   `integrity`, in npm's cache layout (`npm ci --offline` reads it).
//!
//! Each is one store path, named by what's in it, and made again only when
//! the lock file changes.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::Engine;
use sha2::{Digest, Sha256, Sha512};

use crate::lock::Pins;
use crate::store::{base_name, remove_tree, Store};

/// Bump when a vendor folder is laid out differently.
const LAYOUT: u32 = 1;
/// Downloads at once.
const AT_ONCE: usize = 8;

fn hex(b: &[u8]) -> String {
    crate::recipe::hex(b)
}

/// The store path made before for this lock file, if it's still there.
fn made_before(store: &Store, kind: &str, lock: &str) -> (PathBuf, Option<String>) {
    let marker = store.cache_dir().join("sources").join(format!("{}-{kind}-v{LAYOUT}", hex(&Sha256::digest(lock.as_bytes()))));
    let path = fs::read_to_string(&marker).ok().filter(|p| store.is_valid(base_name(p.trim()))).map(|p| p.trim().to_string());
    (marker, path)
}

/// A folder to put a vendor tree together in, empty.
fn work_dir(store: &Store, kind: &str) -> Result<PathBuf> {
    let dir = store.cache_dir().join("sources").join(format!(".{kind}-{}", std::process::id()));
    if dir.symlink_metadata().is_ok() {
        remove_tree(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// The folder as a store path, remembered for this lock file.
fn finish(store: &Store, dir: &Path, name: &str, marker: &Path) -> Result<String> {
    let path = store.add_path(dir, name);
    remove_tree(dir)?;
    let path = path?;
    fs::create_dir_all(marker.parent().unwrap())?;
    fs::write(marker, &path)?;
    Ok(path)
}

/// Run `each` on every item, `AT_ONCE` at a time; the first error stops it.
fn each_at_once<T: Sync>(items: &[T], each: impl Fn(&T) -> Result<()> + Sync) -> Result<()> {
    let next = AtomicUsize::new(0);
    let failed: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    std::thread::scope(|s| {
        for _ in 0..AT_ONCE.min(items.len()) {
            s.spawn(|| loop {
                if failed.lock().unwrap().is_some() {
                    return;
                }
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(item) = items.get(i) else { return };
                if let Err(e) = each(item) {
                    failed.lock().unwrap().get_or_insert(e);
                    return;
                }
            });
        }
    });
    match failed.into_inner().unwrap() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// A file from the network, with two more tries when the network blinks.
fn get(store: &Store, url: &str) -> Result<Vec<u8>> {
    let mut tries = 0;
    loop {
        match store.http().get(url).call() {
            Ok(r) => {
                let mut data = Vec::new();
                r.into_reader().take(1 << 31).read_to_end(&mut data).with_context(|| format!("downloading {url}"))?;
                return Ok(data);
            }
            Err(ureq::Error::Transport(e)) if tries < 2 => {
                tries += 1;
                crate::say!("fetching {url}: {e}; again in {}s", 2 * tries);
                std::thread::sleep(std::time::Duration::from_secs(2 * tries));
            }
            Err(ureq::Error::Status(code, _)) => bail!("fetching {url}: the server says {code}"),
            Err(e) => bail!("fetching {url}: {e}"),
        }
    }
}

// ---------- Rust ----------

/// A Rust project's crates: the vendor folder, and what Cargo's config
/// says to read them from it.
pub struct Cargo {
    pub path: String,
    pub config: String,
}

/// One package in Cargo.lock that comes from somewhere.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Crate {
    pub name: String,
    pub version: String,
    pub from: CrateSource,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CrateSource {
    /// crates.io, and the sha256 Cargo.lock gives.
    Registry(String),
    /// A git repository: its URL, the branch, tag or rev Cargo.toml names
    /// (`branch=main`), and the commit Cargo.lock pins.
    Git { url: String, reference: Option<(String, String)>, commit: String },
}

/// Cargo.lock's packages that aren't the project's own.
pub(crate) fn cargo_lock(text: &str) -> Result<Vec<Crate>> {
    let v: toml::Value = toml::from_str(text).context("reading Cargo.lock")?;
    // The first format kept checksums apart: `"checksum foo 1.0 (registry+...)" = "..."`.
    let metadata = v.get("metadata").and_then(|m| m.as_table());
    let mut out = Vec::new();
    for p in v.get("package").and_then(|p| p.as_array()).into_iter().flatten() {
        let field = |k: &str| p.get(k).and_then(|v| v.as_str()).map(String::from);
        let (Some(name), Some(version)) = (field("name"), field("version")) else { continue };
        let Some(source) = field("source") else { continue }; // the project's own, or a path
        let from = if source.starts_with("registry+") || source.starts_with("sparse+") {
            ensure!(
                source == "registry+https://github.com/rust-lang/crates.io-index" || source == "sparse+https://index.crates.io/",
                "Cargo.lock: {name} {version} comes from a registry nami doesn't fetch from ({source}); only crates.io's"
            );
            let checksum = field("checksum")
                .or_else(|| metadata.and_then(|m| m.get(&format!("checksum {name} {version} ({source})"))).and_then(|c| c.as_str()).map(String::from))
                .ok_or_else(|| anyhow!("Cargo.lock has no checksum for {name} {version}"))?;
            CrateSource::Registry(checksum)
        } else if let Some(rest) = source.strip_prefix("git+") {
            let (url_query, commit) = rest.split_once('#').ok_or_else(|| anyhow!("Cargo.lock: {name}'s git source names no commit: {source}"))?;
            let (url, query) = url_query.split_once('?').unwrap_or((url_query, ""));
            let reference = query.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| ["branch", "tag", "rev"].contains(k))).map(|(k, v)| (k.to_string(), v.to_string()));
            CrateSource::Git { url: url.trim_end_matches(".git").to_string(), reference, commit: commit.to_string() }
        } else {
            bail!("Cargo.lock: {name} {version} comes from {source}, which nami doesn't fetch from");
        };
        out.push(Crate { name, version, from });
    }
    Ok(out)
}

/// A git repository's tarball at `commit`, from the forges nami knows.
pub(crate) fn git_tarball(url: &str, commit: &str) -> Result<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).ok_or_else(|| anyhow!("a git dependency at {url}: nami fetches them over https"))?;
    let (host, path) = rest.split_once('/').ok_or_else(|| anyhow!("a git dependency at {url}: no repository in it"))?;
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let repo = path.rsplit('/').next().unwrap_or(path);
    Ok(match host {
        "github.com" => format!("https://github.com/{path}/archive/{commit}.tar.gz"),
        "gitlab.com" => format!("https://gitlab.com/{path}/-/archive/{commit}/{repo}-{commit}.tar.gz"),
        "codeberg.org" => format!("https://codeberg.org/{path}/archive/{commit}.tar.gz"),
        _ => bail!("a git dependency from {host} ({url}): nami fetches git dependencies from GitHub, GitLab and Codeberg"),
    })
}

/// Fetch a URL a recipe pins (a git dependency's tarball): checked
/// against the sha256 its nami.lock has, else recorded there.
pub(crate) fn pinned(store: &Store, url: &str, pins: &Mutex<&mut Pins>) -> Result<Vec<u8>> {
    let data = get(store, url)?;
    let got = hex(&Sha256::digest(&data));
    let mut pins = pins.lock().unwrap();
    match pins.get(url) {
        Some(want) if want != got => bail!("{}", crate::build_errors::changed_upstream(url, want, &got, pins.path())),
        Some(_) => {}
        None => pins.set(url, &got),
    }
    Ok(data)
}

/// The vendor folder for Cargo.lock `lock` (see the module's docs).
pub fn cargo(store: &Store, lock: &str, pins: &mut Pins) -> Result<Cargo> {
    let crates = cargo_lock(lock)?;
    let config = cargo_config(&crates);
    let (marker, made) = made_before(store, "cargo-vendor", lock);
    // Its git dependencies' tarballs must still be the ones pinned: a
    // vendor folder made before has them already.
    if let Some(path) = made.filter(|_| git_urls(&crates).iter().all(|u| pins.get(u).is_some())) {
        return Ok(Cargo { config: config.replace("@vendor@", &path), path });
    }
    let dir = work_dir(store, "cargo-vendor")?;
    let registry: Vec<&Crate> = crates.iter().filter(|c| matches!(c.from, CrateSource::Registry(_))).collect();
    crate::say!("fetching {} crates", crates.len());
    each_at_once(&registry, |c| {
        let CrateSource::Registry(checksum) = &c.from else { unreachable!() };
        let url = format!("https://static.crates.io/crates/{0}/{0}-{1}.crate", c.name, c.version);
        let data = get(store, &url)?;
        let got = hex(&Sha256::digest(&data));
        ensure!(&got == checksum, "{url}: its sha256 is {got}, not the {checksum} Cargo.lock says");
        let dest = dir.join(format!("{}-{}", c.name, c.version));
        crate::archive::unpack_bytes(&data, &dest, true).with_context(|| format!("unpacking {} {}", c.name, c.version))?;
        fs::write(dest.join(".cargo-checksum.json"), format!("{{\"files\":{{}},\"package\":\"{checksum}\"}}"))?;
        Ok(())
    })?;
    // Git dependencies: each repository's commit once, then each crate from it.
    let mut repos: BTreeMap<(String, String), Vec<&Crate>> = BTreeMap::new();
    for c in &crates {
        if let CrateSource::Git { url, commit, .. } = &c.from {
            repos.entry((url.clone(), commit.clone())).or_default().push(c);
        }
    }
    let pins = Mutex::new(pins);
    for ((url, commit), members) in &repos {
        let tarball = git_tarball(url, commit)?;
        let data = pinned(store, &tarball, &pins)?;
        let tmp = dir.join(format!(".git-{}", &hex(&Sha256::digest(tarball.as_bytes()))[..16]));
        crate::archive::unpack_bytes(&data, &tmp, true).with_context(|| format!("unpacking {tarball}"))?;
        let manifests = crate_dirs(&tmp)?;
        for c in members {
            let from = manifests.get(&c.name).ok_or_else(|| anyhow!("{url} at {commit} has no crate named {}", c.name))?;
            let dest = dir.join(format!("{}-{}", c.name, c.version));
            copy_tree(from, &dest)?;
            fs::write(dest.join(".cargo-checksum.json"), "{\"files\":{},\"package\":null}")?;
        }
        remove_tree(&tmp)?;
    }
    let path = finish(store, &dir, "cargo-vendor", &marker)?;
    Ok(Cargo { config: config.replace("@vendor@", &path), path })
}

fn git_urls(crates: &[Crate]) -> Vec<String> {
    crates
        .iter()
        .filter_map(|c| match &c.from {
            CrateSource::Git { url, commit, .. } => git_tarball(url, commit).ok(),
            _ => None,
        })
        .collect()
}

/// Cargo's config: crates.io and each git source read from the vendor
/// folder (`@vendor@`, its store path once made).
pub(crate) fn cargo_config(crates: &[Crate]) -> String {
    let mut text = String::from("[source.crates-io]\nreplace-with = \"vendored-sources\"\n\n");
    let mut seen = BTreeSet::new();
    for c in crates {
        let CrateSource::Git { url, reference, .. } = &c.from else { continue };
        let key = match reference {
            Some((k, v)) => format!("git+{url}?{k}={v}"),
            None => format!("git+{url}"),
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        text += &format!("[source.\"{key}\"]\ngit = \"{url}\"\n");
        if let Some((k, v)) = reference {
            text += &format!("{k} = \"{v}\"\n");
        }
        text += "replace-with = \"vendored-sources\"\n\n";
    }
    text += "[source.vendored-sources]\ndirectory = \"@vendor@\"\n";
    text
}

/// The folder of each crate in a repository, by its name.
fn crate_dirs(root: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)?.flatten() {
            let p = e.path();
            if e.file_type()?.is_dir() {
                if e.file_name() != "target" && e.file_name() != ".git" {
                    stack.push(p);
                }
            } else if e.file_name() == "Cargo.toml" {
                let name = fs::read_to_string(&p).ok().and_then(|t| toml::from_str::<toml::Value>(&t).ok()).and_then(|v| v.get("package")?.get("name")?.as_str().map(String::from));
                if let Some(name) = name {
                    out.entry(name).or_insert(d.clone());
                }
            }
        }
    }
    Ok(out)
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)?.flatten() {
        let (src, dest) = (e.path(), to.join(e.file_name()));
        let ty = e.file_type()?;
        if ty.is_dir() {
            copy_tree(&src, &dest)?;
        } else if ty.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&src)?, &dest)?;
        } else {
            fs::copy(&src, &dest)?;
        }
    }
    Ok(())
}

// ---------- Go ----------

/// What go.sum pins for one module version: its zip's hash and its
/// go.mod's (`h1:...`).
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct GoModule {
    pub zip: Option<String>,
    pub go_mod: Option<String>,
}

/// go.sum's lines, by (module, version).
pub(crate) fn go_sum(text: &str) -> BTreeMap<(String, String), GoModule> {
    let mut out: BTreeMap<(String, String), GoModule> = BTreeMap::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let (Some(module), Some(version), Some(hash)) = (words.next(), words.next(), words.next()) else { continue };
        // The toolchain itself (golang.org/toolchain) isn't the project's: GOTOOLCHAIN=local.
        if module == "golang.org/toolchain" {
            continue;
        }
        match version.strip_suffix("/go.mod") {
            Some(v) => out.entry((module.into(), v.into())).or_default().go_mod = Some(hash.into()),
            None => out.entry((module.into(), version.into())).or_default().zip = Some(hash.into()),
        }
    }
    out
}

/// A module path or version as the proxy has it: `!` and the lower case
/// letter for each capital.
pub(crate) fn go_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            out.push('!');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Go's `h1:` hash of files (name, contents): sha256 of their sha256s and
/// names, a line each, sorted by name (golang.org/x/mod/sumdb/dirhash).
pub(crate) fn go_h1(files: &mut [(String, Vec<u8>)]) -> String {
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut summary = String::new();
    for (name, data) in files.iter() {
        summary += &format!("{}  {name}\n", hex(&Sha256::digest(data)));
    }
    format!("h1:{}", base64::engine::general_purpose::STANDARD.encode(Sha256::digest(summary.as_bytes())))
}

/// The module proxy folder for go.sum `sum` (see the module's docs).
pub fn go(store: &Store, sum: &str) -> Result<String> {
    let modules: Vec<((String, String), GoModule)> = go_sum(sum).into_iter().collect();
    let (marker, made) = made_before(store, "go-modules", sum);
    if let Some(path) = made {
        return Ok(path);
    }
    let dir = work_dir(store, "go-modules")?;
    crate::say!("fetching {} Go modules", modules.len());
    each_at_once(&modules, |((module, version), want)| {
        let at = dir.join(go_escape(module)).join("@v");
        fs::create_dir_all(&at)?;
        let base = format!("https://proxy.golang.org/{}/@v/{}", go_escape(module), go_escape(version));
        let go_mod = get(store, &format!("{base}.mod"))?;
        if let Some(h) = &want.go_mod {
            let got = go_h1(&mut [("go.mod".to_string(), go_mod.clone())]);
            ensure!(&got == h, "{module} {version}: its go.mod's hash is {got}, not the {h} go.sum says");
        }
        let v = go_escape(version);
        fs::write(at.join(format!("{v}.mod")), &go_mod)?;
        fs::write(at.join(format!("{v}.info")), format!("{{\"Version\":\"{version}\"}}\n"))?;
        if let Some(h) = &want.zip {
            let zip = get(store, &format!("{base}.zip"))?;
            let got = go_h1(&mut crate::archive::zip_files(&zip).with_context(|| format!("reading {module} {version}'s zip"))?);
            ensure!(&got == h, "{module} {version}: its hash is {got}, not the {h} go.sum says");
            fs::write(at.join(format!("{v}.zip")), &zip)?;
        }
        Ok(())
    })?;
    finish(store, &dir, "go-modules", &marker)
}

// ---------- Node.js ----------

/// One package in package-lock.json: where it's from and its integrity.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NpmPackage {
    pub resolved: String,
    pub integrity: String,
}

/// package-lock.json's (or npm-shrinkwrap.json's) packages to fetch.
pub(crate) fn npm_lock(text: &str) -> Result<Vec<NpmPackage>> {
    let v: serde_json::Value = serde_json::from_str(text).context("reading package-lock.json")?;
    let mut out = Vec::new();
    let mut add = |at: &str, p: &serde_json::Value| -> Result<()> {
        if p["link"].as_bool() == Some(true) || p["inBundle"].as_bool() == Some(true) || p["bundled"].as_bool() == Some(true) {
            return Ok(());
        }
        let Some(resolved) = p["resolved"].as_str() else { return Ok(()) };
        if resolved.starts_with("file:") {
            return Ok(()); // in the project itself
        }
        ensure!(resolved.starts_with("https://") || resolved.starts_with("http://"), "package-lock.json: {at} comes from {resolved}: nami fetches only from npm registries (a dependency from git isn't pinned by a hash)");
        let integrity = p["integrity"].as_str().ok_or_else(|| anyhow!("package-lock.json: {at} has no integrity"))?;
        out.push(NpmPackage { resolved: resolved.to_string(), integrity: integrity.to_string() });
        Ok(())
    };
    if let Some(packages) = v["packages"].as_object() {
        for (at, p) in packages {
            if !at.is_empty() {
                add(at, p)?;
            }
        }
    } else {
        // lockfileVersion 1: dependencies within dependencies.
        fn walk(deps: &serde_json::Value, add: &mut dyn FnMut(&str, &serde_json::Value) -> Result<()>) -> Result<()> {
            for (name, p) in deps.as_object().into_iter().flatten() {
                add(name, p)?;
                walk(&p["dependencies"], add)?;
            }
            Ok(())
        }
        walk(&v["dependencies"], &mut add)?;
    }
    let mut seen = BTreeSet::new();
    out.retain(|p| seen.insert(p.integrity.clone()));
    Ok(out)
}

/// An integrity string's hash: (algorithm, bytes), the strongest it has.
fn integrity(s: &str) -> Result<(String, Vec<u8>)> {
    let mut best: Option<(String, Vec<u8>)> = None;
    for part in s.split_whitespace() {
        let Some((algo, b64)) = part.split_once('-') else { continue };
        let b64 = b64.split('?').next().unwrap_or(b64);
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else { continue };
        let rank = |a: &str| ["sha1", "sha256", "sha512"].iter().position(|x| *x == a);
        if rank(algo).is_some() && best.as_ref().is_none_or(|(b, _)| rank(algo) > rank(b)) {
            best = Some((algo.to_string(), bytes));
        }
    }
    best.ok_or_else(|| anyhow!("an integrity nami doesn't read: {s}"))
}

fn digest(algo: &str, data: &[u8]) -> Vec<u8> {
    match algo {
        "sha512" => Sha512::digest(data).to_vec(),
        "sha256" => Sha256::digest(data).to_vec(),
        _ => {
            let mut h = crate::openpgp::Sha1::default();
            h.update(data);
            h.finish().to_vec()
        }
    }
}

/// npm's cache for package-lock.json `lock` (see the module's docs): each
/// tarball in content-v2/<algorithm>/<hex>, and an index entry naming it
/// by its URL, as npm writes them.
pub fn npm(store: &Store, lock: &str) -> Result<String> {
    let packages = npm_lock(lock)?;
    let (marker, made) = made_before(store, "npm-cache", lock);
    if let Some(path) = made {
        return Ok(path);
    }
    let dir = work_dir(store, "npm-cache")?;
    crate::say!("fetching {} npm packages", packages.len());
    each_at_once(&packages, |p| {
        let (algo, want) = integrity(&p.integrity)?;
        let data = get(store, &p.resolved)?;
        let got = digest(&algo, &data);
        ensure!(got == want, "{}: its {algo} isn't the one package-lock.json says", p.resolved);
        let h = hex(&want);
        let content = dir.join("_cacache/content-v2").join(&algo).join(&h[..2]).join(&h[2..4]).join(&h[4..]);
        fs::create_dir_all(content.parent().unwrap())?;
        fs::write(&content, &data)?;
        // The index entry, as make-fetch-happen keys a download.
        let key = format!("make-fetch-happen:request-cache:{}", p.resolved);
        let entry = serde_json::json!({
            "key": key,
            "integrity": format!("{algo}-{}", base64::engine::general_purpose::STANDARD.encode(&want)),
            "time": 0,
            "size": data.len(),
            "metadata": { "url": p.resolved, "reqHeaders": {}, "resHeaders": {}, "options": { "compress": true } },
        })
        .to_string();
        let bucket = hex(&Sha256::digest(key.as_bytes()));
        let index = dir.join("_cacache/index-v5").join(&bucket[..2]).join(&bucket[2..4]).join(&bucket[4..]);
        fs::create_dir_all(index.parent().unwrap())?;
        let mut sha1 = crate::openpgp::Sha1::default();
        sha1.update(entry.as_bytes());
        fs::write(&index, format!("\n{}\t{entry}", hex(&sha1.finish())))?;
        Ok(())
    })?;
    finish(store, &dir, "npm-cache", &marker)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cargo_lock() {
        let lock = r#"
version = 3

[[package]]
name = "myapp"
version = "0.1.0"
dependencies = ["memchr"]

[[package]]
name = "memchr"
version = "2.7.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "78ca9ab1a0babb1e7d5695e3530886289c18cf2f87ec19a575a0abdce112e3a3"

[[package]]
name = "gitdep"
version = "0.2.0"
source = "git+https://github.com/acme/gitdep?branch=main#0123456789abcdef0123456789abcdef01234567"
"#;
        let crates = cargo_lock(lock).unwrap();
        assert_eq!(crates.len(), 2, "the project's own isn't fetched");
        assert_eq!(crates[0].from, CrateSource::Registry("78ca9ab1a0babb1e7d5695e3530886289c18cf2f87ec19a575a0abdce112e3a3".into()));
        let CrateSource::Git { url, reference, commit } = &crates[1].from else { panic!() };
        assert_eq!((url.as_str(), reference.clone(), commit.as_str()), ("https://github.com/acme/gitdep", Some(("branch".into(), "main".into())), "0123456789abcdef0123456789abcdef01234567"));
        let config = cargo_config(&crates);
        assert!(config.contains("[source.\"git+https://github.com/acme/gitdep?branch=main\"]\ngit = \"https://github.com/acme/gitdep\"\nbranch = \"main\"\nreplace-with = \"vendored-sources\""), "{config}");
        assert!(config.ends_with("[source.vendored-sources]\ndirectory = \"@vendor@\"\n"));
        assert_eq!(git_tarball(url, commit).unwrap(), "https://github.com/acme/gitdep/archive/0123456789abcdef0123456789abcdef01234567.tar.gz");
        assert!(git_tarball("https://example.org/x.git", "ab").is_err());
        // The first format's checksums, kept apart.
        let v1 = "[[package]]\nname = \"a\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n[metadata]\n\"checksum a 1.0.0 (registry+https://github.com/rust-lang/crates.io-index)\" = \"ab\"\n";
        assert_eq!(cargo_lock(v1).unwrap()[0].from, CrateSource::Registry("ab".into()));
        assert!(cargo_lock("[[package]]\nname = \"a\"\nversion = \"1\"\nsource = \"registry+https://my.registry/index\"\nchecksum = \"ab\"\n").is_err());
    }

    #[test]
    fn reads_go_sum_and_hashes_like_go() {
        let sum = "github.com/BurntSushi/toml v1.4.0 h1:kuoIxZQy2WRRk1pttg9asf+WVv6tWQuBNVmK4+nh1fs=\ngithub.com/BurntSushi/toml v1.4.0/go.mod h1:ukJfTF/6rtPPRCnwkur4qwRxa8vTRFBF0uk2lLoLwho=\ngolang.org/x/sys v0.1.0/go.mod h1:abc=\ngolang.org/toolchain v0.0.1-go1.22.0.linux-amd64 h1:x=\n";
        let m = go_sum(sum);
        assert_eq!(m.len(), 2);
        let toml = &m[&("github.com/BurntSushi/toml".to_string(), "v1.4.0".to_string())];
        assert!(toml.zip.is_some() && toml.go_mod.is_some());
        assert_eq!(m[&("golang.org/x/sys".to_string(), "v0.1.0".to_string())].zip, None);
        assert_eq!(go_escape("github.com/BurntSushi/toml"), "github.com/!burnt!sushi/toml");
        // A go.mod's hash, as `go mod download -json` reports it for this one.
        let go_mod = b"module github.com/BurntSushi/toml\n\ngo 1.18\n".to_vec();
        assert_eq!(go_h1(&mut [("go.mod".into(), go_mod)]), "h1:ukJfTF/6rtPPRCnwkur4qwRxa8vTRFBF0uk2lLoLwho=");
    }

    #[test]
    fn reads_package_lock() {
        let v3 = r#"{"lockfileVersion": 3, "packages": {
            "": {"name": "app"},
            "node_modules/a": {"version": "1.0.0", "resolved": "https://registry.npmjs.org/a/-/a-1.0.0.tgz", "integrity": "sha512-AAAA"},
            "node_modules/b": {"link": true, "resolved": "packages/b"},
            "node_modules/c/node_modules/d": {"resolved": "https://registry.npmjs.org/d/-/d-2.0.0.tgz", "integrity": "sha1-BBBB sha512-CCCC"},
            "node_modules/e": {"inBundle": true}
        }}"#;
        let p = npm_lock(v3).unwrap();
        assert_eq!(p.iter().map(|p| p.resolved.as_str()).collect::<Vec<_>>(), ["https://registry.npmjs.org/a/-/a-1.0.0.tgz", "https://registry.npmjs.org/d/-/d-2.0.0.tgz"]);
        assert_eq!(integrity(&p[1].integrity).unwrap().0, "sha512", "the strongest");
        let v1 = r#"{"lockfileVersion": 1, "dependencies": {"a": {"resolved": "https://r/a.tgz", "integrity": "sha1-AAAA", "dependencies": {"b": {"resolved": "https://r/b.tgz", "integrity": "sha1-BBBB"}}}}}"#;
        assert_eq!(npm_lock(v1).unwrap().len(), 2);
        let git = r#"{"lockfileVersion": 3, "packages": {"node_modules/g": {"resolved": "git+ssh://git@github.com/a/g.git#abc"}}}"#;
        assert!(npm_lock(git).unwrap_err().to_string().contains("from git"));
        // sha1, for the oldest lock files' integrity.
        assert_eq!(hex(&digest("sha1", b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }
}
