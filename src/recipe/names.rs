//! Other names for the catalog's packages: a library's file (`libz.so.1`),
//! an Ubuntu package's name, a header (`openssl/ssl.h`), a pkg-config
//! module, a CMake package, a command, a -sys crate, a Python
//! distribution. Each is looked up in a table (deb_names.yaml,
//! build_names.yaml), else among the names nixpkgs would give it, each
//! checked against the binary cache's listing of the package's files: the
//! package that has `include/openssl/ssl.h` is the one with that header.
//!
//! `nami recipe from-deb` and `from-url` fill in a recipe's dependencies
//! with it, and a failed build says what to add (build_errors.rs).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Read;

use anyhow::Result;
use serde::Deserialize;

use crate::index::Index;
use crate::store::{base_name, remove_tree, Store};

/// Ubuntu's names next to nixpkgs' (deb_names.yaml).
#[derive(Deserialize)]
pub(crate) struct Names {
    pub sonames: BTreeMap<String, Option<String>>,
    pub packages: BTreeMap<String, Option<String>>,
}

pub(super) fn names() -> Names {
    serde_yaml::from_str(include_str!("deb_names.yaml")).expect("deb_names.yaml")
}

/// What builds ask for, next to nixpkgs' names (build_names.yaml).
#[derive(Deserialize, Default)]
pub(crate) struct BuildNames {
    #[serde(rename = "pkg-config")]
    pub pkg_config: BTreeMap<String, Option<String>>,
    pub headers: BTreeMap<String, Option<String>>,
    pub cmake: BTreeMap<String, Option<String>>,
    pub commands: BTreeMap<String, Option<String>>,
    pub crates: BTreeMap<String, Option<String>>,
    pub python: BTreeMap<String, Option<String>>,
}

pub(crate) fn build_names() -> &'static BuildNames {
    static NAMES: std::sync::OnceLock<BuildNames> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| serde_yaml::from_str(include_str!("build_names.yaml")).expect("build_names.yaml"))
}

/// What a lookup found: a package, nothing needed (`~`: every build has
/// it), or nothing found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Found {
    Package(String),
    Nothing,
    Missing,
}

pub(crate) struct Mapper<'a> {
    pub store: &'a Store,
    pub index: &'a Index,
    pub names: Names,
    /// Store path -> its files (the binary cache's .ls listing).
    listings: HashMap<String, Option<serde_json::Value>>,
    /// (attr, soname) -> the symbol versions nixpkgs' library defines.
    defined: HashMap<(String, String), Vec<String>>,
    /// Looked up already, by kind and name.
    known: HashMap<(u8, String), Found>,
}

impl<'a> Mapper<'a> {
    pub fn new(store: &'a Store, index: &'a Index) -> Self {
        Mapper { store, index, names: names(), listings: HashMap::new(), defined: HashMap::new(), known: HashMap::new() }
    }

    /// Whether the catalog has `attr` (a package, or one of its outputs).
    pub fn has(&self, attr: &str) -> bool {
        self.index.get(crate::build::split_output(self.index, attr).0).is_ok()
    }

    /// A table's answer, when the catalog has what it names.
    fn table(&self, entry: Option<&Option<String>>) -> Option<Found> {
        match entry? {
            None => Some(Found::Nothing),
            Some(a) if self.has(a) => Some(Found::Package(a.clone())),
            Some(_) => None,
        }
    }

    /// The symbol versions of `wanted` that nixpkgs' `soname` in `attr`
    /// doesn't define; None if it has them all (or defines none, which the
    /// loader accepts).
    pub fn lacking(&mut self, attr: &str, soname: &str, wanted: &BTreeSet<String>) -> Option<Vec<String>> {
        if wanted.is_empty() {
            return None;
        }
        let key = (attr.to_string(), soname.to_string());
        if !self.defined.contains_key(&key) {
            let defined = self.versions_of(attr, soname).unwrap_or_else(|e| {
                crate::say!("couldn't look at {soname} in {attr}: {e:#}");
                Vec::new()
            });
            self.defined.insert(key.clone(), defined);
        }
        let defined = &self.defined[&key];
        let lacking: Vec<String> = wanted.iter().filter(|v| !defined.contains(v)).cloned().collect();
        (!defined.is_empty() && !lacking.is_empty()).then_some(lacking)
    }

    /// The symbol versions `soname` in nixpkgs' `attr` defines: from the
    /// library itself (only its own store path is downloaded).
    fn versions_of(&mut self, attr: &str, soname: &str) -> Result<Vec<String>> {
        let (name, output) = crate::build::split_output(self.index, attr);
        let pkg = self.index.get(name)?;
        let outputs: Vec<&str> = output.map(|o| vec![o]).unwrap_or_else(|| vec!["lib", "out"]);
        for o in outputs {
            let Some(Some(path)) = pkg.outputs.get(o) else { continue };
            if !self.has_library(path, soname) {
                continue;
            }
            let (dir, peeked) = if self.store.is_valid(base_name(path)) {
                (self.store.phys(path), false)
            } else {
                let tmp = self.store.cache_dir().join(format!("peek-{}", std::process::id()));
                self.store.peek(path, &tmp)?;
                (tmp, true)
            };
            // The library, or the link to it (libfoo.so.1 -> libfoo.so.1.2.3).
            let mut file = dir.join("lib").join(soname);
            for _ in 0..8 {
                match fs::read_link(&file) {
                    Ok(to) if to.is_relative() => file = file.with_file_name(to),
                    Ok(to) => match to.strip_prefix(path) {
                        Ok(rest) => file = dir.join(rest),
                        Err(_) => break,
                    },
                    Err(_) => break,
                }
            }
            let data = fs::read(&file);
            if peeked {
                remove_tree(&dir)?;
            }
            return Ok(data.ok().and_then(|d| crate::elf::linking(&d)).map(|l| l.versions).unwrap_or_default());
        }
        Ok(Vec::new())
    }

    /// The nixpkgs package with library `soname`: Some(None) for one every
    /// build has; None if none was found.
    pub fn soname(&mut self, soname: &str, hints: &[String]) -> Option<Option<String>> {
        if let Some(attr) = self.names.sonames.get(soname).cloned() {
            match attr {
                None => return Some(None),
                Some(a) if self.has(&a) => return Some(Some(a)),
                Some(_) => {}
            }
        }
        // Names it might have: libfoo-2.so.1 is foo-2, libfoo-2, foo, libfoo;
        // and the Ubuntu packages it depends on, by their names.
        let stem = soname.strip_prefix("lib")?.split(".so").next()?.to_string();
        let mut candidates = self.candidates(&stem);
        candidates.extend(hints.iter().filter(|h| h.starts_with("lib")).flat_map(|h| guesses(h)));
        let mut seen = BTreeSet::new();
        for c in candidates.into_iter().filter(|c| !c.is_empty() && c != "lib") {
            if !seen.insert(c.clone()) {
                continue;
            }
            let Ok(pkg) = self.index.get(&c) else { continue };
            for out in ["lib", "out"] {
                let Some(Some(path)) = pkg.outputs.get(out) else { continue };
                if self.has_library(path, soname) {
                    return Some(Some(if out == "lib" { format!("{c}.lib") } else { c }));
                }
            }
        }
        None
    }

    /// Names a library or module `stem` might have in the catalog: itself,
    /// with `lib`, without its version, lowercased; and the catalog's names
    /// that start like it (libonig is oniguruma's, libaa aalib's).
    fn candidates(&self, stem: &str) -> Vec<String> {
        let stem = stem.strip_prefix("lib").unwrap_or(stem);
        let short = stem.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-' || c == '_').to_string();
        let mut candidates = vec![stem.to_string(), format!("lib{stem}"), short.clone(), format!("lib{short}")];
        candidates.extend(candidates.clone().iter().map(|c| c.to_lowercase()));
        let mut alike: Vec<&str> = self.index.names().filter(|a| !a.contains('.') && a.len() <= short.len() + 6 && short.len() >= 2 && a.starts_with(&short.to_lowercase())).collect();
        alike.sort_by_key(|a| (a.len(), a.to_string()));
        candidates.extend(alike.into_iter().take(8).map(String::from));
        let mut seen = BTreeSet::new();
        candidates.retain(|c| !c.is_empty() && c != "lib" && seen.insert(c.clone()));
        candidates
    }

    /// The nixpkgs package for Ubuntu package `name`, and whether that's a
    /// guess by its name. Some(None): nothing to add.
    pub fn package(&self, name: &str) -> Option<Option<(String, bool)>> {
        let plain = name.strip_suffix("t64").unwrap_or(name);
        if let Some(attr) = self.names.packages.get(plain) {
            return Some(attr.clone().filter(|a| self.has(a)).map(|a| (a, false)));
        }
        guesses(plain).into_iter().find(|c| self.index.get(c).is_ok()).map(|c| Some((c, true)))
    }

    /// Whether `path`'s lib/ has `soname`, by the binary cache's listing of it.
    pub fn has_library(&mut self, path: &str, soname: &str) -> bool {
        self.has_file(path, &format!("lib/{soname}"))
    }

    /// Whether store path `path` has the file (or folder) `rel`, by the
    /// binary cache's listing of it.
    pub fn has_file(&mut self, path: &str, rel: &str) -> bool {
        if !self.listings.contains_key(path) {
            let listing = self.store.caches().iter().filter(|c| c.url.starts_with("http")).find_map(|c| listing(self.store, &c.url, path));
            self.listings.insert(path.to_string(), listing);
        }
        let pointer: String = rel.trim_end_matches('/').split('/').map(|p| format!("/entries/{}", p.replace('~', "~0"))).collect();
        self.listings[path].as_ref().and_then(|l| l.pointer(&format!("/root{pointer}"))).is_some()
    }

    /// The first of `candidates` whose build output has one of `files`
    /// (dev, else out: where headers and .pc files are).
    fn with_file(&mut self, candidates: Vec<String>, files: &[String]) -> Option<String> {
        for c in candidates {
            let Ok(pkg) = self.index.get(&c) else { continue };
            let outputs: Vec<String> = ["dev", "out", "lib", "bin"].iter().filter_map(|o| pkg.outputs.get(*o).cloned().flatten()).collect();
            for path in outputs {
                if files.iter().any(|f| self.has_file(&path, f)) {
                    return Some(c);
                }
            }
        }
        None
    }

    fn remembered(&mut self, kind: u8, name: &str, find: impl FnOnce(&mut Self) -> Found) -> Found {
        if let Some(f) = self.known.get(&(kind, name.to_string())) {
            return f.clone();
        }
        let f = find(self);
        self.known.insert((kind, name.to_string()), f.clone());
        f
    }

    /// The package with pkg-config module `name` (`glib-2.0`: glib).
    pub fn pkg_config(&mut self, name: &str) -> Found {
        self.remembered(0, name, |m| {
            if let Some(f) = m.table(build_names().pkg_config.get(name)) {
                return f;
            }
            let unversioned = name.rsplit_once('-').filter(|(_, v)| v.chars().all(|c| c.is_ascii_digit() || c == '.')).map(|(n, _)| n).unwrap_or(name);
            let files = [format!("lib/pkgconfig/{name}.pc"), format!("share/pkgconfig/{name}.pc")];
            m.with_file(m.candidates(unversioned), &files).map_or(Found::Missing, Found::Package)
        })
    }

    /// The package with header `path` (`openssl/ssl.h`, `zlib.h`).
    pub fn header(&mut self, path: &str) -> Found {
        self.remembered(1, path, |m| {
            let t = &build_names().headers;
            let folder = path.split_once('/').map(|(d, _)| format!("{d}/"));
            if let Some(f) = m.table(t.get(path)).or_else(|| m.table(folder.as_ref().and_then(|d| t.get(d)))) {
                return f;
            }
            let stem = path.split('/').next().unwrap_or(path).trim_end_matches(".hpp").trim_end_matches(".h");
            m.with_file(m.candidates(stem), &[format!("include/{path}")]).map_or(Found::Missing, Found::Package)
        })
    }

    /// The package for CMake's `find_package(name)`.
    pub fn cmake(&mut self, name: &str) -> Found {
        self.remembered(2, name, |m| {
            if let Some(f) = m.table(build_names().cmake.get(name)) {
                return f;
            }
            let lower = name.to_lowercase();
            let files = [format!("lib/cmake/{name}"), format!("lib/cmake/{lower}"), format!("share/cmake/{name}"), format!("share/{lower}/cmake"), format!("lib/pkgconfig/{lower}.pc")];
            m.with_file(m.candidates(&lower), &files).map_or(Found::Missing, Found::Package)
        })
    }

    /// The package with library `name` to link (`-lz`: zlib), as a build
    /// lists it: its dev output, which brings the rest.
    pub fn link_library(&mut self, name: &str) -> Found {
        self.remembered(3, name, |m| {
            let prefix = format!("lib{name}.so.");
            let listed: Vec<Option<String>> = m.names.sonames.iter().filter(|(s, _)| s.starts_with(&prefix)).map(|(_, a)| a.clone()).collect();
            if let Some(first) = listed.first() {
                return match first {
                    None => Found::Nothing,
                    Some(a) => Found::Package(a.strip_suffix(".lib").unwrap_or(a).to_string()),
                };
            }
            let files = [format!("lib/lib{name}.so"), format!("lib/lib{name}.a")];
            m.with_file(m.candidates(name), &files).map_or(Found::Missing, Found::Package)
        })
    }

    /// The package with command `cmd`, for build-deps.
    pub fn command(&mut self, cmd: &str) -> Found {
        self.remembered(4, cmd, |m| {
            if let Some(f) = m.table(build_names().commands.get(cmd)) {
                return f;
            }
            // The catalog's table of programs, when this computer has it.
            if let Some((table, _)) = crate::run_once::table(&m.store.var_dir()) {
                if let Ok(data) = fs::read(table) {
                    if let Some((c, _)) = crate::run_once::lookup(&data, cmd).into_iter().find(|(c, _)| m.has(&c.attr)) {
                        return Found::Package(c.attr);
                    }
                }
            }
            if m.index.get(cmd).is_ok_and(|p| p.main_program.as_deref() == Some(cmd)) {
                return Found::Package(cmd.to_string());
            }
            Found::Missing
        })
    }

    /// The package a -sys crate links (`openssl-sys`: openssl); Nothing
    /// for one that bundles its library or is for another system.
    pub fn crate_library(&self, name: &str) -> Found {
        match self.table(build_names().crates.get(name)) {
            Some(f) => f,
            None => Found::Missing,
        }
    }

    /// The package of Python distribution `name` in package set `set`
    /// (`python3Packages`): `PyYAML` is python3Packages.pyyaml.
    pub fn python(&self, name: &str, set: &str) -> Found {
        let norm = python_name(name);
        match build_names().python.get(&norm) {
            Some(None) => return Found::Nothing,
            Some(Some(a)) if self.has(&format!("{set}.{a}")) => return Found::Package(format!("{set}.{a}")),
            _ => {}
        }
        [norm.clone(), norm.replace('-', "_"), format!("py{norm}"), norm.trim_start_matches("python-").to_string(), norm.trim_start_matches("py").to_string()]
            .into_iter()
            .map(|n| format!("{set}.{n}"))
            .find(|a| self.has(a))
            .map_or(Found::Missing, Found::Package)
    }
}

/// A Python distribution's name as PEP 503 compares them: lowercase,
/// runs of `-_.` as one `-`.
pub(crate) fn python_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if "-_.".contains(c) {
            if !out.ends_with('-') {
                out.push('-');
            }
        } else {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// The catalog's package with library `soname` (`libfoo.so.1`): its name
/// as a nami file lists it (`krb5.lib` for a lib output). By deb_names.yaml,
/// else by the names nixpkgs would give it, each checked against the binary
/// cache's listing of its files. None when none has it (or it's glibc's).
pub fn package_with_library(store: &Store, index: &Index, soname: &str) -> Option<String> {
    Mapper::new(store, index).soname(soname, &[]).flatten()
}

/// The nixpkgs package for Debian's or Ubuntu's package `name`, as
/// deb_names.yaml has it (`t64` endings aside): Some(None) for one every
/// system has (glibc). None when it isn't listed.
pub fn deb_package(name: &str) -> Option<Option<String>> {
    static NAMES: std::sync::OnceLock<Names> = std::sync::OnceLock::new();
    let plain = name.strip_suffix("t64").unwrap_or(name);
    NAMES.get_or_init(names).packages.get(plain).cloned()
}

/// The same for a library, by its file's name (`libgtk-3.so.0`).
pub fn soname_package(soname: &str) -> Option<Option<String>> {
    static NAMES: std::sync::OnceLock<Names> = std::sync::OnceLock::new();
    NAMES.get_or_init(names).sonames.get(soname).cloned()
}

/// A store path's files, as the binary cache lists them (`<hash>.ls`).
fn listing(store: &Store, cache: &str, path: &str) -> Option<serde_json::Value> {
    let hash = base_name(path).get(..32)?;
    let resp = store.http().get(&format!("{}/{hash}.ls", cache.trim_end_matches('/'))).call().ok()?;
    let encoding = resp.header("content-encoding").unwrap_or_default().to_string();
    let mut data = Vec::new();
    resp.into_reader().take(64 << 20).read_to_end(&mut data).ok()?;
    let data = match encoding.as_str() {
        "zstd" => zstd::stream::decode_all(&data[..]).ok()?,
        "xz" => {
            let mut v = Vec::new();
            xz2::read::XzDecoder::new(&data[..]).read_to_end(&mut v).ok()?;
            v
        }
        "" | "identity" => data,
        _ => return None,
    };
    serde_json::from_slice(&data).ok()
}

/// Names nixpkgs might give Ubuntu's package `name`: libfoo2 is libfoo or
/// foo, libfoo-1.0-0 is libfoo-1.0, foo-1.0, libfoo or foo.
pub(super) fn guesses(name: &str) -> Vec<String> {
    let plain = name.strip_suffix("t64").unwrap_or(name);
    let trimmed = plain.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches(['-', '.']);
    // Without a version at the end: libjson-glib-1.0 is libjson-glib.
    let unversioned = trimmed.rsplit_once('-').filter(|(_, v)| v.bytes().all(|b| b.is_ascii_digit() || b == b'.')).map(|(n, _)| n).unwrap_or(trimmed);
    let mut out: Vec<String> = Vec::new();
    for n in [plain, trimmed, unversioned] {
        for n in [Some(n), n.strip_prefix("lib")].into_iter().flatten() {
            if n.len() > 1 && !out.iter().any(|o| o == n) {
                out.push(n.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_tables() {
        assert_eq!(guesses("libtre5"), vec!["libtre5", "tre5", "libtre", "tre"]);
        assert_eq!(guesses("libjson-glib-1.0-0"), vec!["libjson-glib-1.0-0", "json-glib-1.0-0", "libjson-glib-1.0", "json-glib-1.0", "libjson-glib", "json-glib"]);
        assert_eq!(guesses("libssl3t64"), vec!["libssl3", "ssl3", "libssl", "ssl"]);
        assert_eq!(guesses("xdg-utils"), vec!["xdg-utils"]);
        // Every name in the tables reads, and Ubuntu's t64 names map like the others.
        let n = names();
        assert_eq!(n.sonames["libc.so.6"], None);
        assert_eq!(n.sonames["libgtk-3.so.0"].as_deref(), Some("gtk3"));
        assert_eq!(n.packages["libnss3"].as_deref(), Some("nss"));
        // ABI-compatible builds: Ubuntu's libjpeg8 is libjpeg-turbo's jpeg8 build.
        assert_eq!(n.sonames["libjpeg.so.8"].as_deref(), Some("libjpeg8"));
        let b = build_names();
        assert_eq!(b.pkg_config["glib-2.0"].as_deref(), Some("glib"));
        assert_eq!(b.headers["openssl/"].as_deref(), Some("openssl"));
        assert_eq!(b.cmake["ZLIB"].as_deref(), Some("zlib"));
        assert_eq!(b.commands["autoreconf"].as_deref(), Some("autoreconfHook"));
        assert_eq!(b.crates["openssl-sys"].as_deref(), Some("openssl"));
        assert_eq!(b.crates["windows-sys"], None);
        assert_eq!(python_name("PyYAML"), "pyyaml");
        assert_eq!(python_name("typing_extensions"), "typing-extensions");
        assert_eq!(python_name("zope.interface"), "zope-interface");
        assert_eq!(python_name("a__b"), "a-b");
    }
}
