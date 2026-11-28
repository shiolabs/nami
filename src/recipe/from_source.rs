//! `nami recipe from-url` and `nami recipe new`: a recipe for source code.
//!
//! From a URL (a repository's latest release or tag, a release, any
//! tarball: upstream.rs) or a folder. Its name, version and source are
//! filled in, the source's sha256 goes in the nami.lock next to the
//! recipe, and what kind of project it is and what its build files ask
//! for (wants.rs) become its build-deps and deps, by the catalog's names
//! (names.rs). What isn't found is marked NOT FOUND, as from-deb does, so
//! the recipe is ready for `nami build` or says what to look at.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, ensure, Context, Result};

use super::names::{build_names, Found, Mapper};
use super::upstream::{self, Upstream};
use super::wants::{self, Want};
use crate::build_kinds::Kind;
use crate::index::Index;
use crate::store::Store;

pub struct Options {
    /// A version (or tag) other than the newest.
    pub version: Option<String>,
    /// Where to write it (default ./<name>).
    pub out: Option<PathBuf>,
    /// How the command was typed, for the recipe's first lines.
    pub command: String,
}

/// Where the source is.
struct Origin {
    name: String,
    version: String,
    source: Source,
    homepage: Option<String>,
}

enum Source {
    Url(String),
    Folder(PathBuf),
}

/// Write `<out>/package.nami` for `what`: a URL or a folder.
pub fn from_source(store: &Store, index: &Index, what: &str, opts: &Options) -> Result<PathBuf> {
    let origin = if what.starts_with("https://") || what.starts_with("http://") {
        resolve(store, what, opts.version.as_deref())?
    } else {
        let dir = Path::new(what);
        ensure!(dir.is_dir(), "{what}: no such folder (a URL starts with https://)");
        ensure!(opts.version.is_none(), "--version is for a URL; a folder's recipe has the version its files say");
        let dir = fs::canonicalize(dir)?;
        let name = dir.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_else(|| "app".into());
        Origin { name, version: String::new(), source: Source::Folder(dir), homepage: None }
    };

    // Look into it.
    let (tree, fetched) = match &origin.source {
        Source::Url(url) => {
            let (path, sha256) = match crate::build::fetch_url(store, url, crate::build::url_file_name(url), None)? {
                crate::build::Fetched::Ok { path, sha256 } => (path, sha256),
                crate::build::Fetched::Mismatch { .. } => unreachable!("nothing asked for"),
            };
            (crate::archive::peek(&store.phys(&path), &crate::build_kinds::wanted).with_context(|| format!("looking into {url}"))?, Some(super::hex(&sha256)))
        }
        Source::Folder(dir) => (crate::archive::peek(dir, &crate::build_kinds::wanted)?, None),
    };
    let w = wants::read(&tree);
    let name = clean_name(match &origin.source {
        Source::Folder(_) => w.name.as_deref().unwrap_or(&origin.name),
        Source::Url(_) => &origin.name,
    });
    let version = if origin.version.is_empty() { w.version.clone().unwrap_or_else(|| "0.1.0".into()) } else { origin.version.clone() };
    let dir = opts.out.clone().unwrap_or_else(|| PathBuf::from(&name));
    let file = dir.join("package.nami");
    ensure!(!file.exists(), "{} is there already: remove it, or pass --out <dir>", file.display());

    // Its dependencies, by the catalog's names.
    let mut m = Mapper::new(store, index);
    let found = map(&mut m, &w, &tree, index);

    fs::create_dir_all(&dir)?;
    let base = fs::canonicalize(&dir)?;
    let shown = format!("./{}", dir.display().to_string().trim_start_matches("./"));
    let mut text = String::new();
    let about = w.description.as_deref().map(|d| format!(": {}", d.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches('.'))).unwrap_or_default();
    let home = origin.homepage.clone().or(w.homepage.clone()).map(|h| format!(" ({h})")).unwrap_or_default();
    text += &format!("# {name}{about}{home}\n");
    text += &format!("# By `{}`", opts.command);
    text += &match w.kind {
        Some(k) => format!(": {}.\n", kind_said(k)),
        None => ".\n".into(),
    };
    if fetched.is_some() {
        text += "# Its sha256 is in nami.lock, next to this file. ";
    } else {
        text += "# ";
    }
    text += &format!("Check it, then: nami build {shown}\n");
    text += &format!("name: {name}\nversion: {}\nsource:\n", yaml_scalar(&version));
    match &origin.source {
        Source::Url(url) => text += &format!("  url: {url}\n"),
        Source::Folder(path) => text += &format!("  path: {}\n", super::relative_to(path, &base).display()),
    }
    if w.kind.is_none() {
        text += "# nami couldn't tell how it's built (no CMakeLists.txt, meson.build, configure, Makefile,\n# Cargo.toml, go.mod, pyproject.toml or package.json at its top): say how with\n# build: and install: (or build-system:)\n";
    }
    let width = found.build_deps.keys().chain(found.deps.keys()).chain(found.optional.keys()).map(|a| a.len()).max().unwrap_or(0).max(12);
    let list = |key: &str, items: &BTreeMap<String, Vec<String>>| {
        if items.is_empty() {
            return String::new();
        }
        let mut s = format!("{key}:\n");
        for (attr, from) in items {
            s += &format!("  - {attr:<width$}   # {}\n", from.join(", "));
        }
        s
    };
    text += &list("build-deps", &found.build_deps);
    text += &list("deps", &found.deps);
    if !w.make_flags.is_empty() {
        text += &format!("make-flags: [{}]   # it installs to /usr/local otherwise\n", w.make_flags.join(", "));
    }
    if !found.optional.is_empty() {
        text += "# It can also use these, if you want them (add them to deps):\n";
        for (attr, from) in &found.optional {
            text += &format!("#   {attr:<width$}   # {}\n", from.join(", "));
        }
    }
    if !found.missing.is_empty() {
        text += "# NOT FOUND in the catalog: add these to deps by hand if the build asks:\n";
        for x in &found.missing {
            text += &format!("#   {x}\n");
        }
    }
    for n in &w.notes {
        text += &format!("# {n}\n");
    }
    fs::write(&file, &text)?;
    if let (Some(sha256), Source::Url(url)) = (&fetched, &origin.source) {
        let mut pins = crate::lock::Pins::load(&base)?;
        pins.set(url, sha256);
        pins.save()?;
    }
    let kind = w.kind.map(kind_said).unwrap_or_else(|| "nami couldn't tell how it's built".into());
    crate::say!("{kind}{}", needs_said(found.deps.len() + found.build_deps.len(), found.missing.len()));
    Ok(file)
}

/// What a project needs from the catalog, after what it is: nothing said when it needs
/// nothing (`; it needs 3 packages, all in the catalog`).
fn needs_said(found: usize, missing: usize) -> String {
    let packages = |n: usize| if n == 1 { "1 package".to_string() } else { format!("{n} packages") };
    match (found, missing) {
        (0, 0) => String::new(),
        (n, 0) => format!("; it needs {}, all in the catalog", packages(n)),
        (0, m) => format!("; it needs {} the catalog doesn't have", packages(m)),
        (n, m) => format!("; it needs {}: {n} in the catalog, {m} not", packages(n + m)),
    }
}

/// What a kind of project is, as a recipe's first lines say it.
fn kind_said(k: Kind) -> String {
    match k {
        Kind::Cargo => "a Rust project, built with Cargo; its crates come as its Cargo.lock pins them",
        Kind::Go => "a Go project; its modules come as its go.sum pins them",
        Kind::Python => "a Python app, its dependencies the catalog's Python packages",
        Kind::Node => "a Node.js project, built with npm; its packages come as its package-lock.json pins them",
        Kind::Meson => "built with Meson",
        Kind::CMake => "built with CMake",
        Kind::Autotools => "built with ./configure and make",
        Kind::Make => "built with make",
    }
    .into()
}

/// A name nami takes: lowercase, letters, digits and `+-._`.
fn clean_name(name: &str) -> String {
    let s: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || "+-._".contains(c) { c } else { '-' }).collect();
    let s = s.trim_matches(['-', '.']).to_string();
    if s.is_empty() { "app".into() } else { s }
}

/// A version as YAML text that stays text (`1.10` isn't 1.1).
fn yaml_scalar(v: &str) -> String {
    if v.parse::<f64>().is_ok() { format!("\"{v}\"") } else { v.to_string() }
}

/// Where a URL's source is, at the version asked for or the newest.
fn resolve(store: &Store, url: &str, version: Option<&str>) -> Result<Origin> {
    match upstream::parse(url) {
        Upstream::Repo { repo, tag, file } => {
            let name = repo.name();
            let (tag, file) = match (tag, version) {
                (Some(t), None) => (t, file),
                (_, wanted) => (repo.pick_tag(store, wanted)?, None),
            };
            let version = upstream::tag_version(&tag, &name).unwrap_or_else(|| tag.clone());
            let source = match file {
                Some(f) => f,
                None => {
                    // A release's own tarball (configure made, submodules in) over the tag's.
                    let files = repo.release_files(store, &tag).unwrap_or_default();
                    upstream::source_file(&files, &name, &tag, &version).unwrap_or_else(|| repo.tag_archive(&tag))
                }
            };
            crate::say!("{name} {version}: {source}");
            Ok(Origin { name, version, source: Source::Url(source), homepage: Some(repo.url()) })
        }
        Upstream::File(file_url) => {
            let (name, v) = upstream::name_version(&file_url).ok_or_else(|| anyhow!("{file_url}: nami can't tell its name and version from the file's name; download it into a folder and use `nami recipe new <folder>`"))?;
            let (source, v) = match version {
                Some(w) if w != v => (upstream::moved_url(&file_url, &v, w, &v, w)?, w.to_string()),
                _ => (file_url, v),
            };
            Ok(Origin { name, version: v, source: Source::Url(source), homepage: None })
        }
    }
}

/// What the recipe lists: catalog name -> where the source asks for it.
#[derive(Default)]
struct Mapped {
    build_deps: BTreeMap<String, Vec<String>>,
    deps: BTreeMap<String, Vec<String>>,
    optional: BTreeMap<String, Vec<String>>,
    missing: Vec<String>,
}

fn map(m: &mut Mapper, w: &wants::Wants, tree: &crate::archive::Tree, index: &Index) -> Mapped {
    let mut out = Mapped::default();
    let implied: Vec<&str> = w.kind.map(|k| k.tools().to_vec()).unwrap_or_default();
    let add = |list: &mut BTreeMap<String, Vec<String>>, attr: String, why: String| {
        let whys = list.entry(attr).or_default();
        if !whys.contains(&why) {
            whys.push(why);
        }
    };
    let place = |out: &mut Mapped, found: Found, want: &Want, key: &str, why: String, what: &str| match (found, want.optional) {
        (Found::Package(a), false) if key == "build-deps" => {
            if !implied.contains(&a.as_str()) {
                add(&mut out.build_deps, a, why)
            }
        }
        (Found::Package(a), false) => add(&mut out.deps, a, why),
        (Found::Package(a), true) => add(&mut out.optional, a, why),
        (Found::Missing, false) if !want.conditional => out.missing.push(format!("{what} ({})", want.from)),
        _ => {}
    };
    for t in &w.tools {
        if t.optional {
            continue;
        }
        let f = m.command(&t.name);
        place(&mut out, f, t, "build-deps", t.from.clone(), &format!("the command {}", t.name));
    }
    for p in &w.pkg_config {
        // One of several (`lua-5.4|lua-5.3|lua`): the first the catalog has.
        let names: Vec<&str> = p.name.split('|').collect();
        let (name, f) = names.iter().map(|n| (*n, m.pkg_config(n))).find(|(_, f)| matches!(f, Found::Package(_))).unwrap_or((names[0], Found::Missing));
        place(&mut out, f, p, "deps", format!("{}: {name}", p.from), &format!("pkg-config module {}", names.join(" or ")));
    }
    for c in &w.cmake {
        let f = m.cmake(&c.name);
        let tool = ["Python", "Python3", "PythonInterp", "Perl", "BISON", "FLEX", "Git", "Doxygen", "Gettext"].contains(&c.name.as_str());
        if tool && c.optional {
            continue;
        }
        place(&mut out, f, c, if tool { "build-deps" } else { "deps" }, format!("{}: find_package({})", c.from, c.name), &format!("CMake package {}", c.name));
    }
    for l in &w.libraries {
        let f = m.link_library(&l.name);
        place(&mut out, f, l, "deps", format!("{}: -l{}", l.from, l.name), &format!("library -l{}", l.name));
    }
    for h in &w.headers {
        let t = &build_names().headers;
        let folder = h.name.split_once('/').map(|(d, _)| format!("{d}/"));
        let found = match t.get(&h.name).or_else(|| folder.as_ref().and_then(|d| t.get(d))) {
            Some(Some(a)) if m.has(a) => Found::Package(a.clone()),
            _ => Found::Nothing, // most are the C library's own
        };
        place(&mut out, found, h, "deps", format!("{}: {}", h.from, h.name), &h.name);
    }
    for c in &w.crates {
        match m.crate_library(&c.name) {
            Found::Package(a) => add(&mut out.deps, a, format!("Cargo.lock: {}", c.name)),
            Found::Missing if c.name.ends_with("-sys") => out.missing.push(format!("the C library the crate {} links, if it doesn't bring its own (Cargo.lock)", c.name)),
            _ => {}
        }
    }
    if w.kind == Some(Kind::Python) {
        let set = crate::build_kinds::python_set_for(index, wants::requires_python(tree).as_deref());
        for b in wants::python_backend(tree) {
            match m.python(&b, &set) {
                Found::Package(a) => add(&mut out.build_deps, a, "pyproject.toml: build-system".into()),
                Found::Missing => out.missing.push(format!("Python build backend {b} (pyproject.toml)")),
                Found::Nothing => {}
            }
        }
        for p in &w.python {
            let f = m.python(&p.name, &set);
            place(&mut out, f, p, "deps", format!("{}: {}", p.from, p.name), &format!("Python package {}", p.name));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_it_needs_in_words() {
        assert_eq!(needs_said(0, 0), "");
        assert_eq!(needs_said(3, 0), "; it needs 3 packages, all in the catalog");
        assert_eq!(needs_said(0, 1), "; it needs 1 package the catalog doesn't have");
        assert_eq!(needs_said(2, 1), "; it needs 3 packages: 2 in the catalog, 1 not");
    }

    #[test]
    fn names_and_versions_as_written() {
        assert_eq!(clean_name("CMark"), "cmark");
        assert_eq!(clean_name("@scope/x"), "scope-x");
        assert_eq!(yaml_scalar("1.10"), "\"1.10\"");
        assert_eq!(yaml_scalar("14.1.1"), "14.1.1");
        assert_eq!(yaml_scalar("3"), "\"3\"");
    }
}
