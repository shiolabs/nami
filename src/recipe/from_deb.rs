//! `nami recipe from-deb`: a recipe (package.nami) written from a Debian
//! package, found by name in Ubuntu's archive, or at a URL or path. Its
//! Ubuntu dependencies become nixpkgs ones: by the libraries its programs
//! link to (each looked up in deb_names.yaml, else by its file in the
//! binary cache), and by the names in its control file.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, ensure, Result};
use sha2::{Digest, Sha256};

use super::names::{guesses, Mapper};
use super::{download, hex, relative_to};
use crate::index::Index;
use crate::store::{remove_tree, Store};

/// Ubuntu's archive for this computer's kind: amd64's, or (on ARM) the ports archive's arm64.
const ARCHIVE: &str = if crate::arch::ARM { "https://ports.ubuntu.com/ubuntu-ports" } else { "https://archive.ubuntu.com/ubuntu" };
/// The newest Ubuntu LTS: where `nami recipe from-deb <name>` looks.
pub const RELEASE: &str = "resolute";

/// Where a package of the recipe comes from.
#[derive(Clone)]
enum From {
    Url { url: String, sha256: String },
    Path(PathBuf),
}

/// A Debian package in the recipe: the app, a package that's part of it,
/// or one whose libraries it bundles.
struct Part {
    name: String,
    deb: Vec<u8>,
    from: From,
    /// Only its libraries (`bundle:`).
    only_libs: bool,
    /// Why it's in the recipe.
    why: String,
    control: BTreeMap<String, String>,
}

/// Write `<out>/package.nami` (default `./<package name>`) for `what`: an
/// http(s) URL, a .deb file, or a package name in Ubuntu's archive.
pub fn from_deb(store: &Store, index: &Index, what: &str, out: Option<&Path>, release: &str) -> Result<PathBuf> {
    let mut archive = Archive::new(store, release);
    let (deb, from) = if what.starts_with("https://") || what.starts_with("http://") {
        let data = download(store, what)?;
        let sha256 = hex(&Sha256::digest(&data));
        (data, From::Url { url: what.to_string(), sha256 })
    } else if Path::new(what).is_file() {
        (fs::read(what)?, From::Path(fs::canonicalize(what)?))
    } else {
        ensure!(!what.contains('/') && !what.ends_with(".deb"), "no file {what}");
        ensure!(what.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"+-.".contains(&b)), "{what:?} isn't a Debian package name");
        let file = out.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(what)).join("package.nami");
        ensure!(!file.exists(), "{} is there already: remove it, or pass --out <dir>", file.display());
        let stanza = archive.find(what, false)?.ok_or_else(|| anyhow!("no package {what} for {} in Ubuntu {release} (main, universe)", crate::arch::DEB))?;
        let (data, url, sha256) = archive.fetch(&stanza)?;
        (data, From::Url { url, sha256 })
    };
    let control = crate::deb::control(&deb)?;
    let field = |k: &str| control.get(k).cloned().unwrap_or_default();
    let name = field("Package");
    ensure!(!name.is_empty(), "the package has no name in its control file");
    let version = clean_version(&field("Version"));
    let dir = out.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(&name));
    let file = dir.join("package.nami");
    ensure!(!file.exists(), "{} is there already: remove it, or pass --out <dir>", file.display());
    let source_package = source_of(&control);

    let mut parts = vec![Part { name: name.clone(), deb, from, only_libs: false, why: String::new(), control: control.clone() }];
    let mut m = Mapper::new(store, index);
    // Look at what the packages so far need, and add the Ubuntu packages
    // nixpkgs has nothing for (an app's data, a library version), until
    // nothing is missing that Ubuntu has.
    let mut tried: BTreeSet<String> = BTreeSet::new();
    let found = loop {
        let found = examine(store, &parts, &mut m)?;
        if parts.len() > 12 {
            break found;
        }
        let mut added = false;
        // Ubuntu packages that are part of the app: its data
        // (architecture-independent), or built from the same source; with
        // no nixpkgs package, or named after the app (foo-data, foo-common:
        // the files it looks for, even if nixpkgs has a package by that name).
        let named_after = |n: &str| [&name, &source_package].iter().any(|a| n.strip_prefix(a.as_str()).is_some_and(|r| r.starts_with('-')) || n.strip_prefix("lib").and_then(|n| n.strip_prefix(a.as_str())).is_some_and(|r| r.starts_with('-')));
        let candidates = found.unmapped.iter().map(|n| (n, false)).chain(found.guessed.iter().filter(|n| named_after(n)).map(|n| (n, true)));
        for (deb_name, in_nixpkgs) in candidates {
            if parts.iter().any(|p| &p.name == deb_name) || !tried.insert(deb_name.clone()) {
                continue;
            }
            let Some(stanza) = archive.find(deb_name, false)? else { continue };
            let all = stanza.get("Architecture").is_some_and(|a| a == "all");
            let same = source_of(&stanza) == source_package;
            if !all && !same {
                continue;
            }
            let (deb, url, sha256) = archive.fetch(&stanza)?;
            let why = match (same, in_nixpkgs) {
                (true, _) => format!("Ubuntu's {deb_name}: part of {name} (built from the same source)"),
                (false, false) => format!("Ubuntu's {deb_name}: data {name} needs, not in nixpkgs"),
                (false, true) => format!("Ubuntu's {deb_name}: data {name} looks for in its own place"),
            };
            let control = crate::deb::control(&deb)?;
            parts.push(Part { name: deb_name.clone(), deb, from: From::Url { url, sha256 }, only_libs: false, why, control });
            added = true;
        }
        // Libraries nixpkgs has no fitting version of: Ubuntu's, bundled.
        for (soname, problem) in &found.unfit {
            if !tried.insert(format!("soname:{soname}")) {
                continue;
            }
            if let Some((stanza, deb, url, sha256)) = ubuntu_library(&mut archive, &m, &parts, soname)? {
                let deb_name = stanza["Package"].clone();
                if parts.iter().any(|p| p.name == deb_name) {
                    continue;
                }
                let control = crate::deb::control(&deb)?;
                parts.push(Part { name: deb_name, deb, from: From::Url { url, sha256 }, only_libs: true, why: format!("{soname}: {problem}"), control });
                added = true;
            }
        }
        if !added {
            break found;
        }
    };

    // The recipe.
    fs::create_dir_all(&dir)?;
    let base = fs::canonicalize(&dir)?;
    let entry = |p: &Part, first: &str, rest: &str| match &p.from {
        From::Url { url, sha256 } => format!("{first}url: {url}\n{rest}sha256: {sha256}\n"),
        From::Path(path) => format!("{first}path: {}\n", relative_to(path, &base).display()),
    };
    let sources: Vec<&Part> = parts.iter().filter(|p| !p.only_libs).collect();
    let bundled: Vec<&Part> = parts.iter().filter(|p| p.only_libs).collect();
    let summary = field("Description").lines().next().unwrap_or_default().to_string();
    let homepage = field("Homepage");
    let mut text = String::new();
    text += &format!("# {name}: {summary}{}\n", if homepage.is_empty() { String::new() } else { format!(" ({homepage})") });
    text += &match &parts[0].from {
        From::Url { url, .. } if url.starts_with(ARCHIVE) => format!("# Ubuntu {release}'s package, by `nami recipe from-deb {name}`.\n"),
        _ => format!("# By `nami recipe from-deb {what}`.\n"),
    };
    text += &format!("# Check it, then: nami build ./{}\n", dir.display().to_string().trim_start_matches("./"));
    text += &format!("name: {name}\nversion: {version}\n");
    if sources.len() == 1 {
        text += &format!("source:\n{}", entry(sources[0], "  ", "  "));
    } else {
        text += "source:\n";
        for p in &sources {
            if !p.why.is_empty() {
                text += &format!("  # {}\n", p.why);
            }
            text += &entry(p, "  - ", "    ");
        }
    }
    if !bundled.is_empty() {
        text += "# Ubuntu's own libraries, in the app's lib: nixpkgs has no version that fits.\nbundle:\n";
        for p in &bundled {
            text += &format!("  # {}: {}\n", p.name, p.why);
            text += &entry(p, "  - ", "    ");
        }
    }
    let list = |key: &str, items: &BTreeMap<String, Vec<String>>, why: &str| {
        if items.is_empty() {
            return String::new();
        }
        let mut s = format!("{key}:{why}\n");
        for (attr, from) in items {
            s += &format!("  - {attr:<20} # {}\n", from.join(", "));
        }
        s
    };
    text += &list("deps", &found.deps, "");
    text += &list("runtime-deps", &found.runtime, "   # what it may load at runtime: check these");
    if !found.missing.is_empty() {
        text += "# NOT FOUND in nixpkgs: add these to deps by hand if the build or the program asks:\n";
        for x in &found.missing {
            text += &format!("#   {x}\n");
        }
    }
    if !found.also.is_empty() {
        text += "# It also wants these programs; install them next to it if it needs them:\n";
        for x in &found.also {
            text += &format!("#   {x}\n");
        }
    }
    if !found.unpacked.redirects.is_empty() {
        text += &format!("# Paths its programs have built in, pointed at this package when they run: {}\n", found.unpacked.redirects.join(", "));
    }
    for (app, icon) in &found.unpacked.apps {
        text += &format!("# App: {app}{}\n", icon.as_ref().map(|i| format!(", icon {i}")).unwrap_or_default());
    }
    if !found.unpacked.programs.is_empty() {
        text += &format!("# Programs: {}\n", found.unpacked.programs.join(", "));
    }
    fs::write(&file, &text)?;
    crate::say!(
        "found {} libraries in nixpkgs, {} not{}{}",
        found.deps.len() + found.runtime.len(),
        found.missing.len(),
        if sources.len() > 1 { format!("; {} packages of Ubuntu's are part of it", sources.len() - 1) } else { String::new() },
        if bundled.is_empty() { String::new() } else { format!("; {} bundled", bundled.len()) },
    );
    Ok(file)
}

/// A package's source package (`Source: foo (1.2)` is foo; none: its own name).
fn source_of(control: &BTreeMap<String, String>) -> String {
    let source = control.get("Source").and_then(|s| s.split_whitespace().next()).unwrap_or_default();
    if source.is_empty() { control.get("Package").cloned().unwrap_or_default() } else { source.to_string() }
}

/// What the packages so far need, as nixpkgs packages.
#[derive(Default)]
struct Found {
    unpacked: crate::deb::Unpacked,
    deps: BTreeMap<String, Vec<String>>,
    runtime: BTreeMap<String, Vec<String>>,
    missing: Vec<String>,
    also: Vec<String>,
    /// Ubuntu packages it depends on with no nixpkgs package, by name.
    unmapped: Vec<String>,
    /// Ubuntu packages it depends on that nixpkgs has by a guess at the name.
    guessed: Vec<String>,
    /// Libraries nixpkgs has no fitting version of, and why.
    unfit: Vec<(String, String)>,
}

/// Unpack the parts together, and map what their programs link to, name
/// and depend on to nixpkgs packages.
fn examine(store: &Store, parts: &[Part], m: &mut Mapper) -> Result<Found> {
    let tmp = store.cache_dir().join(format!("from-deb-{}", std::process::id()));
    if tmp.symlink_metadata().is_ok() {
        remove_tree(&tmp)?;
    }
    let debs: Vec<(&[u8], bool)> = parts.iter().map(|p| (&p.deb[..], p.only_libs)).collect();
    let result = crate::deb::unpack(&debs, &tmp).and_then(|u| scan(&tmp).map(|s| (u, s)));
    remove_tree(&tmp)?;
    let (unpacked, scanned) = result?;
    let mut found = Found { unpacked, ..Default::default() };

    let depends_of = |p: &Part| -> Vec<Vec<String>> {
        ["Pre-Depends", "Depends"].iter().flat_map(|k| parse_depends(p.control.get(*k).map(String::as_str).unwrap_or_default())).collect()
    };
    let ours = |alts: &Vec<String>| alts.iter().any(|a| parts.iter().any(|p| &p.name == a));
    let hints: Vec<String> = parts.iter().flat_map(depends_of).filter(|a| !ours(a)).flatten().collect();
    // Only the app's own Depends (not a bundled library's) are for it to have.
    let depends: Vec<Vec<String>> = parts.iter().filter(|p| !p.only_libs).flat_map(depends_of).filter(|a| !ours(a)).collect();
    // Ubuntu's names of the libraries found (libfoo.so.1 is in libfoo1).
    let mut covered = BTreeSet::new();
    for (soname, users) in &scanned.needed {
        let wanted = scanned.versions.get(soname).cloned().unwrap_or_default();
        match m.soname(soname, &hints) {
            Some(attr) => {
                let stem = soname.trim_start_matches("lib").split(".so").next().unwrap_or_default();
                covered.insert(format!("lib{stem}"));
                covered.insert(format!("lib{}", stem.trim_end_matches(|c: char| c.is_ascii_digit() || ".-_".contains(c))));
                let Some(attr) = attr else { continue };
                // Symbol versions it needs that nixpkgs' build doesn't define
                // (Ubuntu's libcurl-gnutls has CURL_GNUTLS_3): not that one.
                if let Some(lacking) = m.lacking(&attr, soname, &wanted) {
                    found.unfit.push((soname.clone(), format!("needs {}, which nixpkgs' {attr} doesn't have", lacking.join(", "))));
                    found.missing.push(format!("{soname} (linked by {}): nixpkgs' {attr} lacks {}", users.join(", "), lacking.join(", ")));
                    continue;
                }
                let mut why = soname.clone();
                if !wanted.is_empty() && !wanted.iter().all(|v| v.starts_with("GLIBC") || v.starts_with("GCC_") || v.starts_with("GLIBCXX") || v.starts_with("CXXABI")) {
                    why += &format!(" ({})", wanted.iter().cloned().collect::<Vec<_>>().join(", "));
                }
                found.deps.entry(attr).or_default().push(why);
            }
            None => {
                found.unfit.push((soname.clone(), "nixpkgs has no such library".into()));
                found.missing.push(format!("{soname} (linked by {})", users.join(", ")));
            }
        }
    }
    // Libraries its programs name but don't link: likely loaded at runtime.
    for soname in &scanned.mentioned {
        if let Some(Some(attr)) = m.names.sonames.get(soname).cloned() {
            if m.has(&attr) && !found.deps.contains_key(&attr) {
                found.runtime.entry(attr).or_default().push(soname.clone());
            }
        }
    }
    // Ubuntu's Depends: what no program links to.
    for alternatives in &depends {
        if alternatives.iter().any(|a| guesses(a).iter().any(|g| covered.contains(g))) {
            continue;
        }
        let mapped = alternatives.iter().find_map(|a| m.package(a).map(|attr| (a, attr)));
        match mapped {
            Some((_, None)) => {}
            Some((deb_name, Some((attr, guessed)))) => {
                if guessed {
                    found.guessed.push(deb_name.clone());
                }
                if found.deps.contains_key(&attr) || found.runtime.contains_key(&attr) {
                    continue;
                }
                let why = format!("Ubuntu's {deb_name}{}", if guessed { ", by its name" } else { "" });
                if deb_name.starts_with("lib") {
                    found.runtime.entry(attr).or_default().push(why);
                } else {
                    found.also.push(format!("{attr} ({why})"));
                }
            }
            None => {
                found.unmapped.push(alternatives[0].clone());
                if alternatives[0].starts_with("lib") {
                    found.missing.push(format!("Ubuntu's {}", alternatives.join(" | ")));
                } else {
                    found.missing.push(format!(
                        "Ubuntu's {} (part of this app? `nami recipe from-deb {}`, and install that too)",
                        alternatives.join(" | "),
                        alternatives[0]
                    ));
                }
            }
        }
    }
    Ok(found)
}

/// The Ubuntu package with library `soname`: one the parts depend on (by
/// its name first: libicuuc.so.74 is likely in libicu74), as it's in
/// Ubuntu's archive, and the package itself.
#[allow(clippy::type_complexity)]
fn ubuntu_library(archive: &mut Archive, m: &Mapper, parts: &[Part], soname: &str) -> Result<Option<(BTreeMap<String, String>, Vec<u8>, String, String)>> {
    let letters = |s: &str| -> String {
        let s = s.strip_prefix("lib").unwrap_or(s).replace("t64", "");
        s.chars().filter(|c| c.is_ascii_alphabetic()).collect()
    };
    let stem = letters(soname.split(".so").next().unwrap_or_default());
    let mut names: Vec<String> = parts
        .iter()
        .flat_map(|p| ["Pre-Depends", "Depends"].iter().flat_map(|k| parse_depends(p.control.get(*k).map(String::as_str).unwrap_or_default())))
        .flatten()
        .filter(|n| n.starts_with("lib") && !parts.iter().any(|p| &p.name == n))
        .collect();
    // Named like the library first; then the others nixpkgs has nothing for.
    let rank = |n: &str| {
        let l = letters(n);
        if l == stem { 0 } else if !l.is_empty() && stem.starts_with(&l) { 1 } else { 2 }
    };
    names.retain(|n| rank(n) < 2 || m.package(n).is_none());
    names.sort_by_key(|n| (rank(n), n.clone()));
    names.dedup();
    let m = crate::arch::MULTIARCH;
    let wanted = [format!("usr/lib/{m}/{soname}"), format!("lib/{m}/{soname}")];
    for name in names.into_iter().take(4) {
        let Some(stanza) = archive.find(&name, true)? else { continue };
        if stanza.get("Architecture").is_some_and(|a| a == "all") {
            continue;
        }
        let (deb, url, sha256) = archive.fetch(&stanza)?;
        if crate::deb::paths(&deb)?.iter().any(|p| wanted.contains(p)) {
            return Ok(Some((stanza, deb, url, sha256)));
        }
    }
    Ok(None)
}

/// The libraries the package's programs link to and name, but doesn't ship.
#[derive(Default)]
struct Scanned {
    /// soname -> the files that link to it.
    needed: BTreeMap<String, Vec<String>>,
    /// soname -> the symbol versions they need from it.
    versions: BTreeMap<String, BTreeSet<String>>,
    mentioned: BTreeSet<String>,
}

fn scan(dir: &Path) -> Result<Scanned> {
    let mut out = Scanned::default();
    let mut own: BTreeSet<String> = BTreeSet::new();
    let mut mentioned = BTreeSet::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)?.flatten() {
            let (path, meta) = (e.path(), e.metadata()?);
            own.insert(e.file_name().to_string_lossy().into_owned());
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let mut head = [0u8; 4];
            if fs::File::open(&path)?.read(&mut head)? < 4 || &head != b"\x7fELF" {
                continue;
            }
            let data = fs::read(&path)?;
            let Some(l) = crate::elf::linking(&data) else { continue };
            own.extend(l.soname);
            let rel = path.strip_prefix(dir).unwrap().display().to_string();
            for n in l.needed {
                out.needed.entry(n).or_default().push(rel.clone());
            }
            for (lib, versions) in l.versions_needed {
                out.versions.entry(lib).or_default().extend(versions);
            }
            mentioned.extend(crate::elf::mentioned_libraries(&data));
        }
    }
    out.needed.retain(|n, _| !own.contains(n));
    out.mentioned = mentioned.into_iter().filter(|n| !own.contains(n) && !out.needed.contains_key(n)).collect();
    for users in out.needed.values_mut() {
        users.sort();
        users.truncate(3);
    }
    Ok(out)
}

/// `Depends:` as lists of alternatives, without versions or architectures.
pub fn parse_depends(text: &str) -> Vec<Vec<String>> {
    text.split(',')
        .map(|entry| {
            entry
                .split('|')
                .map(|alt| alt.split(['(', '[', '<']).next().unwrap_or_default().trim().split(':').next().unwrap_or_default().to_string())
                .filter(|a| !a.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|alts| !alts.is_empty())
        .collect()
}

/// A Debian version as a recipe's: no epoch (`1:`), `~` as `-`.
fn clean_version(v: &str) -> String {
    let v = match v.split_once(':') {
        Some((epoch, rest)) if epoch.bytes().all(|b| b.is_ascii_digit()) => rest,
        _ => v,
    };
    let s: String = v.chars().map(|c| if c.is_ascii_alphanumeric() || "+-._".contains(c) { c } else { '-' }).collect();
    if s.is_empty() { "0".into() } else { s }
}

/// Older Ubuntu LTS releases, for packages built for them (`libicu70`)
/// that the newest no longer has.
const OLDER: &[&str] = &["noble", "jammy"];

/// Ubuntu's archive: each package's newest build for amd64 in a release
/// (with its updates), main and universe. Read once, when first asked.
struct Archive<'a> {
    store: &'a Store,
    release: String,
    /// release -> package name -> its stanza (the fields nami uses).
    loaded: HashMap<String, HashMap<String, BTreeMap<String, String>>>,
    /// Packages fetched already, by URL.
    fetched: HashMap<String, Vec<u8>>,
}

impl<'a> Archive<'a> {
    fn new(store: &'a Store, release: &str) -> Self {
        Archive { store, release: release.to_string(), loaded: HashMap::new(), fetched: HashMap::new() }
    }

    /// Package `name`'s stanza: in the release, else (`older`) in an older LTS.
    fn find(&mut self, name: &str, older: bool) -> Result<Option<BTreeMap<String, String>>> {
        let older = OLDER.iter().filter(|r| older && **r != self.release).map(|r| r.to_string());
        let releases: Vec<String> = std::iter::once(self.release.clone()).chain(older).collect();
        for release in releases {
            if !self.loaded.contains_key(&release) {
                let packages = self.load(&release)?;
                self.loaded.insert(release.clone(), packages);
            }
            if let Some(found) = self.loaded[&release].get(name) {
                return Ok(Some(found.clone()));
            }
        }
        Ok(None)
    }

    fn load(&self, release: &str) -> Result<HashMap<String, BTreeMap<String, String>>> {
        let mut packages: HashMap<String, BTreeMap<String, String>> = HashMap::new();
        for suite in [release.to_string(), format!("{release}-updates"), format!("{release}-security")] {
            for component in ["main", "universe"] {
                let url = format!("{ARCHIVE}/dists/{suite}/{component}/binary-{}/Packages.xz", crate::arch::DEB);
                crate::say!("looking in {url}");
                let resp = match self.store.http().get(&url).call() {
                    Ok(r) => r,
                    Err(ureq::Error::Status(404, _)) if suite != release => continue,
                    Err(e) => return Err(anyhow!("fetching {url}: {e}")),
                };
                let reader = BufReader::new(xz2::read::XzDecoder::new_multi_decoder(resp.into_reader()));
                each_stanza(reader, |s| {
                    let newer = packages.get(&s["Package"]).is_none_or(|b| version_cmp(&s["Version"], &b["Version"]) == Ordering::Greater);
                    if newer {
                        packages.insert(s["Package"].clone(), s);
                    }
                })?;
            }
        }
        Ok(packages)
    }

    /// A package from its stanza: its contents, URL and sha256 (checked).
    fn fetch(&mut self, stanza: &BTreeMap<String, String>) -> Result<(Vec<u8>, String, String)> {
        let name = &stanza["Package"];
        let file = stanza.get("Filename").ok_or_else(|| anyhow!("{name}: Ubuntu's index has no file for it"))?;
        let sha256 = stanza.get("SHA256").ok_or_else(|| anyhow!("{name}: Ubuntu's index has no sha256 for it"))?.to_lowercase();
        let url = format!("{ARCHIVE}/{file}");
        if let Some(data) = self.fetched.get(&url) {
            return Ok((data.clone(), url, sha256));
        }
        let data = download(self.store, &url)?;
        let got = hex(&Sha256::digest(&data));
        ensure!(got == sha256, "{url}: its sha256 is {got}, not the {sha256} Ubuntu's index says");
        self.fetched.insert(url.clone(), data.clone());
        Ok((data, url, sha256))
    }
}

/// Each amd64 (on ARM, arm64) or architecture-independent package's stanza in a
/// Packages index, with the fields nami uses.
fn each_stanza(reader: impl BufRead, mut f: impl FnMut(BTreeMap<String, String>)) -> Result<()> {
    const KEEP: &[&str] = &["Package", "Version", "Architecture", "Source", "Depends", "Pre-Depends", "Filename", "SHA256"];
    let mut stanza = String::new();
    let mut done = |stanza: &mut String| {
        if !stanza.is_empty() {
            let mut c = crate::deb::parse_control(stanza);
            c.retain(|k, _| KEEP.contains(&k.as_str()));
            if c.contains_key("Package") && c.contains_key("Version") && matches!(c.get("Architecture").map(String::as_str), Some(a) if a == crate::arch::DEB || a == "all") {
                f(c);
            }
            stanza.clear();
        }
    };
    let mut keep = false;
    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            done(&mut stanza);
            continue;
        }
        // Only the fields nami uses, and their continuation lines.
        if !line.starts_with([' ', '\t']) {
            keep = KEEP.iter().any(|k| line.strip_prefix(k).is_some_and(|r| r.starts_with(':')));
        }
        if keep {
            stanza.push_str(&line);
            stanza.push('\n');
        }
    }
    done(&mut stanza);
    Ok(())
}

/// Debian's version order (`dpkg --compare-versions`): epoch, then the
/// upstream version, then the revision; `~` sorts before anything.
pub fn version_cmp(a: &str, b: &str) -> Ordering {
    fn split(v: &str) -> (u64, &str, &str) {
        let (epoch, rest) = match v.split_once(':') {
            Some((e, r)) if e.bytes().all(|c| c.is_ascii_digit()) => (e.parse().unwrap_or(0), r),
            _ => (0, v),
        };
        let (up, rev) = rest.rsplit_once('-').unwrap_or((rest, ""));
        (epoch, up, rev)
    }
    let ((ea, ua, ra), (eb, ub, rb)) = (split(a), split(b));
    ea.cmp(&eb).then_with(|| part_cmp(ua, ub)).then_with(|| part_cmp(ra, rb))
}

fn part_cmp(a: &str, b: &str) -> Ordering {
    let order = |c: Option<&u8>| -> i32 {
        match c {
            None => 0,
            Some(b'~') => -1,
            Some(c) if c.is_ascii_digit() => 0,
            Some(c) if c.is_ascii_alphabetic() => *c as i32,
            Some(c) => *c as i32 + 256,
        }
    };
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        while (i < a.len() && !a[i].is_ascii_digit()) || (j < b.len() && !b[j].is_ascii_digit()) {
            let (x, y) = (order(a.get(i)), order(b.get(j)));
            if x != y {
                return x.cmp(&y);
            }
            i += 1;
            j += 1;
        }
        while i < a.len() && a[i] == b'0' {
            i += 1;
        }
        while j < b.len() && b[j] == b'0' {
            j += 1;
        }
        let mut first = Ordering::Equal;
        while i < a.len() && a[i].is_ascii_digit() && j < b.len() && b[j].is_ascii_digit() {
            if first == Ordering::Equal {
                first = a[i].cmp(&b[j]);
            }
            i += 1;
            j += 1;
        }
        if i < a.len() && a[i].is_ascii_digit() {
            return Ordering::Greater;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            return Ordering::Less;
        }
        if first != Ordering::Equal {
            return first;
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debian_versions() {
        assert_eq!(version_cmp("1.2-1", "1.10-1"), Ordering::Less);
        assert_eq!(version_cmp("1:0.9", "2.0"), Ordering::Greater);
        assert_eq!(version_cmp("1.0~rc1", "1.0"), Ordering::Less);
        assert_eq!(version_cmp("2.6.1-1ubuntu0.1", "2.6.1-1"), Ordering::Greater);
        assert_eq!(version_cmp("1.0", "1.0"), Ordering::Equal);
        assert_eq!(version_cmp("1.0a", "1.0"), Ordering::Greater);
        assert_eq!(clean_version("1:2.3~rc1-0ubuntu2"), "2.3-rc1-0ubuntu2");
    }

    #[test]
    fn depends() {
        assert_eq!(
            parse_depends("libc6 (>= 2.34), libgtk-3-0t64 (>= 3.9.10) | libgtk-3-0 (>= 3.9.10), python3:any, foo [amd64]"),
            vec![vec!["libc6".to_string()], vec!["libgtk-3-0t64".into(), "libgtk-3-0".into()], vec!["python3".into()], vec!["foo".into()]]
        );
    }

    #[test]
    fn finds_a_package_in_an_index() {
        // (This computer's kind's, amd64 or arm64; not the other's.)
        let (here, other) = if crate::arch::ARM { ("arm64", "amd64") } else { ("amd64", "arm64") };
        let index = format!("Package: foo\nArchitecture: {other}\nVersion: 1\n\nPackage: foo\nArchitecture: {here}\nVersion: 2\nFilename: pool/main/f/foo/foo_2_amd64.deb\nDescription: a\n long one\nDepends: libc6,\n libbar1\nSHA256: AB\n\nPackage: foo-data\nSource: foo (2)\nArchitecture: all\nVersion: 3");
        let mut got = Vec::new();
        each_stanza(index.as_bytes(), |s| got.push(s)).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["Version"], "2");
        assert_eq!(got[0]["Filename"], "pool/main/f/foo/foo_2_amd64.deb");
        assert_eq!(got[0]["Depends"], "libc6,\nlibbar1");
        assert!(!got[0].contains_key("Description"));
        assert_eq!(source_of(&got[0]), "foo");
        assert_eq!(source_of(&got[1]), "foo");
        assert_eq!(relative_to(Path::new("/a/b/x.deb"), Path::new("/a/c/d")), PathBuf::from("../../b/x.deb"));
    }
}
