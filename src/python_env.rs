//! A Python with its packages, made the way nixpkgs' `python3.withPackages`
//! makes one, without Nix:
//!
//! ```yaml
//! packages:
//!   - python3:
//!       version: 3.13
//!       with: [numpy, requests, django]
//! ```
//!
//! The file's entry is read as one package, `python313 with django, numpy,
//! requests` (config/packages.rs): the catalog's python313 and those of its
//! python313Packages, never another Python's. They're downloaded as they
//! are, and put together in two store paths of their own:
//!
//! - the Python itself (`python3-3.13.9-env`), as nixpkgs' env is: the
//!   interpreter's files and every package's (theirs, and the Python packages
//!   they need, from `nix-support/propagated-build-inputs`) merged into one
//!   tree of links, `lib/python3.13/site-packages` among them. Its
//!   `bin/python3.13` is a small script that runs the interpreter under its
//!   own name, so Python finds its prefix from where it is (this tree, its
//!   site-packages, nothing to scan or set at each start) and `PYTHONNOUSERSITE`
//!   keeps `~/.local`'s packages out, as nixpkgs' binary wrapper does.
//! - its programs (`python3-3.13.9-env-bin`): the packages' (`django-admin`,
//!   `pip`) and the interpreter's own (`pydoc3.13`), each run with
//!   `NIX_PYTHONPATH`/`NIX_PYTHONPREFIX`/`NIX_PYTHONEXECUTABLE` set to the
//!   first, which nixpkgs' Python reads at start (its sitecustomize.py), so
//!   they see all of its packages. They name the first path, which can't name
//!   itself (it's named by what it holds), so they're a path of their own.
//!
//! pip comes with every one, for `pip list`. Both paths refer to what they're
//! made of (store.rs, `add_path_referring`), so gc keeps it; nami.lock keeps
//! what that is (`inputs`), and a computer that doesn't have them makes the
//! same two paths again from it (`pinned`).

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};

use crate::build::Builder;
use crate::errors::{coded_hint, Code};
use crate::index::Index;
use crate::profile::Entry;
use crate::store::{base_name, Store, STORE_DIR};

/// Between the Python and its packages, in an entry's name.
const WITH: &str = " with ";

/// Python packages every one has: pip, so `pip list` says what's there.
const ALWAYS: &[&str] = &["pip"];

/// A Python and the packages it's made with.
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    /// The catalog's Python: `python3`, `python313`.
    pub python: String,
    /// Its packages, by their names in its set (`numpy`), sorted.
    pub with: Vec<String>,
}

impl Spec {
    /// Its package set in the catalog: python313 -> python313Packages.
    pub fn set(&self) -> String {
        format!("{}Packages", self.python)
    }

    /// How nami names the entry: `python313 with numpy, requests`.
    pub fn attr(&self) -> String {
        format!("{}{WITH}{}", self.python, self.with.join(", "))
    }

    /// What it is, for `nami list` and `nami info`: its Python's version.
    pub fn about(&self, version: &str) -> String {
        format!("Python {version} with {} (and pip), from {}", self.with.join(", "), self.set())
    }

    /// What's looked up in the catalog: what it's made with, and pip.
    fn wanted(&self) -> Vec<String> {
        let mut all = self.with.clone();
        all.extend(ALWAYS.iter().filter(|a| !self.with.iter().any(|w| w == *a)).map(|a| a.to_string()));
        all
    }
}

/// Whether `name` is a Python that takes packages (`with:`): python3, python313.
pub fn is_python(name: &str) -> bool {
    name.strip_prefix("python3").is_some_and(|v| v.chars().all(|c| c.is_ascii_digit()))
}

/// An entry as nami names it (`python313 with numpy, requests`), read back.
pub fn parse(attr: &str) -> Option<Spec> {
    let (python, with) = attr.split_once(WITH)?;
    if !is_python(python) {
        return None;
    }
    let with: Vec<String> = with.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    (!with.is_empty()).then(|| Spec { python: python.to_string(), with })
}

/// The entry `- python3: { version: 3.13, with: [numpy] }` is: the Python of
/// that line (python313) with those packages. `None` when `with` is empty
/// (the Python alone, as `python3: 3.13` is).
pub fn entry(file: &str, pkg: &str, version: Option<&str>, with: &[String]) -> Result<Option<String>> {
    let shown = |w: &str| format!("{file}: packages: {pkg}: {w}");
    ensure!(is_python(pkg), "{}", shown("with: is for a Python and its packages (`- python3: { version: 3.13, with: [numpy] }`); other packages are each a line of their own"));
    let python = match version {
        None => pkg.to_string(),
        Some(v) => {
            let line = v.strip_prefix("3.").filter(|m| !m.is_empty() && m.chars().all(|c| c.is_ascii_digit()));
            let python = match line {
                Some(minor) => format!("python3{minor}"),
                None if v == "3" => "python3".to_string(),
                None => bail!("{}", shown(&format!("version: {v}: with packages, a Python's version is its line, like 3.13: they're the catalog's packages for it (python313Packages), made for the catalog's 3.13"))),
            };
            ensure!(pkg == "python3" || pkg == python, "{}", shown(&format!("version: {v} isn't {pkg}'s; write `- python3: {{ version: {v}, with: [...] }}`")));
            python
        }
    };
    let mut names = Vec::new();
    for w in with {
        let w = w.trim();
        let ok = !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) && !w.starts_with('.');
        ensure!(ok, "{}", shown(&format!("with: {w:?}: each is a package's name in {python}Packages, like numpy")));
        // `python313Packages.numpy` written whole: its name in the set.
        let w = w.split_once("Packages.").filter(|(set, _)| set.starts_with("python")).map_or(w, |(_, n)| n);
        if !names.iter().any(|n| n == w) {
            names.push(w.to_string());
        }
    }
    names.sort();
    Ok((!names.is_empty()).then(|| Spec { python, with: names }.attr()))
}

/// The Pythons the catalog has packages for (python3, python313, python314).
pub fn pythons(index: &Index) -> Vec<String> {
    let mut sets: BTreeSet<String> = BTreeSet::new();
    for n in index.names().filter(|n| n.starts_with("python3")) {
        if let Some(p) = n.split_once('.').and_then(|(set, _)| set.strip_suffix("Packages")).filter(|p| is_python(p)) {
            sets.insert(p.to_string());
        }
    }
    let mut out: Vec<String> = sets.into_iter().collect();
    out.sort_by_key(|p| (p != "python3", p.clone()));
    out
}

/// What's wrong with `spec` in `index`, each with what to do: (message, hint).
pub fn problems(index: &Index, spec: &Spec) -> Vec<(String, String)> {
    let has = || {
        let all = pythons(index);
        if all.is_empty() { "this catalog has no Python packages".to_string() } else { format!("the catalog has packages for {}", all.join(", ")) }
    };
    if index.get(&spec.python).is_err() {
        return vec![(format!("{} isn't in the catalog", spec.python), has())];
    }
    let set = spec.set();
    let missing: Vec<&String> = spec.with.iter().filter(|w| index.get(&format!("{set}.{w}")).is_err()).collect();
    if missing.is_empty() {
        return Vec::new();
    }
    if !index.names().any(|n| n.starts_with(&set) && n[set.len()..].starts_with('.')) {
        return vec![(format!("the catalog has no packages for {} ({set})", spec.python), has())];
    }
    missing
        .into_iter()
        .map(|w| {
            let close = crate::config::set_typo_matches(index, &set, w);
            let hint = if !close.is_empty() {
                format!("did you mean {}?", close.iter().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(" or "))
            } else if index.get(w).is_ok() {
                format!("{w} is a program, not a Python package: list it in packages: on its own")
            } else {
                format!("`nami search {w} --all` finds packages; a Python package is {set}.<name> there")
            };
            (format!("{w} isn't in {set}"), hint)
        })
        .collect()
}

/// Make the Python `spec` asks for, from the catalog: the entry, its two paths.
pub fn resolve(builder: &Builder, index: &Index, spec: &Spec) -> Result<Entry> {
    let found = problems(index, spec);
    if let Some((first, hint)) = found.first() {
        let message = if found.len() == 1 { first.clone() } else { found.iter().map(|(m, h)| format!("{m} ({h})")).collect::<Vec<_>>().join("; ") };
        return Err(coded_hint(Code::NotFound, format!("{}: {message}", spec.attr()), hint.clone()));
    }
    let out = |attr: &str| -> Result<String> {
        let pkg = index.get(attr)?;
        builder.ensure_catalog(attr)?; // unfree policy; built where no cache has it
        // A Python package's own (`out`), not its wheel (`dist`).
        pkg.outputs.get("out").cloned().flatten().or_else(|| pkg.install_paths().into_iter().next()).with_context(|| format!("{attr} has no output to install"))
    };
    let python = out(&spec.python)?;
    let set = spec.set();
    let mut modules = Vec::new();
    for w in spec.wanted() {
        let attr = format!("{set}.{w}");
        if index.get(&attr).is_err() && ALWAYS.contains(&w.as_str()) {
            continue; // (a set without pip: still a Python with its packages)
        }
        modules.push(out(&attr)?);
    }
    let paths = make(builder.store(), &python, &modules)?;
    Ok(Entry { attr: spec.attr(), name: name_of(&python), paths })
}

/// The entry nami.lock has for `attr`: its paths, made again from what it's
/// made of where they aren't here. `--locked` (`locked`) wants the same paths.
pub fn pinned(store: &Store, attr: &str, l: &crate::lock::Locked, locked: bool) -> Result<Entry> {
    let entry = |paths: Vec<String>| Entry { attr: attr.to_string(), name: l.name.clone(), paths };
    if !l.paths.is_empty() && l.paths.iter().all(|p| store.is_valid(base_name(p))) {
        return Ok(entry(l.paths.clone()));
    }
    ensure!(!l.inputs.is_empty(), "{attr}: nami.lock doesn't say what it's made of; run without --locked to make it again");
    store.realise(&l.inputs)?;
    let python = l
        .inputs
        .iter()
        .find(|p| base_name(p).get(33..).is_some_and(|n| n.starts_with("python3-")) && pyver(store, p).is_some())
        .with_context(|| format!("{attr}: nami.lock names no Python among what it's made of"))?;
    let ver = pyver(store, python).unwrap_or_default();
    let modules: Vec<String> = l.inputs.iter().filter(|p| *p != python && store.phys(p).join("lib").join(&ver).join("site-packages").is_dir()).cloned().collect();
    let paths = make(store, python, &modules)?;
    ensure!(!locked || paths == l.paths, "{attr} comes out different from what nami.lock says; run without --locked to update it");
    Ok(entry(paths))
}

/// What a Python with packages is made of (for nami.lock): what its paths
/// refer to, but each other. Nothing for any other entry.
pub fn inputs(store: &Store, entry: &Entry) -> Vec<String> {
    if parse(&entry.attr).is_none() {
        return Vec::new();
    }
    let own: Vec<&str> = entry.paths.iter().map(|p| base_name(p)).collect();
    let mut all: BTreeSet<String> = BTreeSet::new();
    for p in &entry.paths {
        for r in store.path_info(base_name(p)).map(|i| i.references).unwrap_or_default() {
            if !own.contains(&r.as_str()) {
                all.insert(format!("{STORE_DIR}/{r}"));
            }
        }
    }
    all.into_iter().collect()
}

/// A store path's name, less its hash: `python3-3.13.9`.
fn name_of(path: &str) -> String {
    base_name(path).get(33..).unwrap_or_default().to_string()
}

/// The Python's own folder in lib/, `python3.13`: where its os.py is.
fn pyver(store: &Store, python: &str) -> Option<String> {
    let lib = store.phys(python).join("lib");
    let mut found: Vec<String> = fs::read_dir(&lib)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.strip_prefix("python3.").is_some_and(|m| !m.is_empty() && m.chars().all(|c| c.is_ascii_digit())) && lib.join(n).join("os.py").exists())
        .collect();
    found.sort();
    found.pop()
}

/// Make the two paths: the Python with `modules` (and the Python packages
/// they need) in its site-packages, and its programs.
fn make(store: &Store, python: &str, modules: &[String]) -> Result<Vec<String>> {
    let mut wanted = vec![python.to_string()];
    wanted.extend(modules.iter().cloned());
    store.realise(&wanted).context("getting the Python and its packages")?;
    let ver = pyver(store, python).with_context(|| format!("{python} isn't a Python (no lib/python3.x/os.py)"))?;
    // nixpkgs' bash, which the Python itself refers to: what the scripts run with.
    let bash = store
        .path_info(base_name(python))?
        .references
        .into_iter()
        .map(|r| format!("{STORE_DIR}/{r}"))
        .find(|r| base_name(r).get(33..).is_some_and(|n| n.starts_with("bash-")) && store.phys(r).join("bin/bash").is_file())
        .with_context(|| format!("{python} refers to no bash"))?;
    let modules = with_needed(store, python, &ver, modules);
    let mut candidates: Vec<String> = modules.iter().chain([python.to_string(), bash.clone()].iter()).map(|p| base_name(p).to_string()).collect();
    let name = name_of(python);

    let work = store.cache_dir().join(format!("python-env-{}", std::process::id()));
    let result = (|| -> Result<Vec<String>> {
        if work.symlink_metadata().is_ok() {
            crate::store::remove_tree(&work)?;
        }
        // The Python: everything merged, but bin/, which is its own.
        let env = work.join("env");
        fs::create_dir_all(&env).with_context(|| format!("making {}", env.display()))?;
        let mut sources = vec![python.to_string()];
        sources.extend(modules.iter().cloned());
        merge(store, &env, &sources, &["bin", "nix-support"]).with_context(|| format!("putting the packages together in {}", env.display()))?;
        let bin = env.join("bin");
        fs::create_dir(&bin).with_context(|| format!("making {}", bin.display()))?;
        let interpreter = &ver;
        let own_bin = store.phys(python).join("bin");
        let mut theirs: Vec<(String, String)> = Vec::new(); // (name, what it runs), for the programs' path
        for name in sorted_names(&own_bin)? {
            let at = own_bin.join(&name);
            let logical = format!("{python}/bin/{name}");
            if name == *interpreter {
                write_script(&bin.join(&name), &interpreter_script(&bash, &logical))?;
            } else if fs::symlink_metadata(&at)?.file_type().is_symlink() && points_to(&own_bin, &name, interpreter) {
                std::os::unix::fs::symlink(fs::read_link(&at)?, bin.join(&name))?;
            } else {
                theirs.push((name, logical));
            }
        }
        let env_path = store.add_path_referring(&env, &format!("{name}-env"), &candidates).with_context(|| format!("adding {} to the store", env.display()))?;

        // Its programs, each run with the Python above around it.
        let progs = work.join("bin");
        fs::create_dir_all(progs.join("bin"))?;
        let mut taken: BTreeSet<String> = sorted_names(&bin)?.into_iter().collect();
        for module in &modules {
            let dir = store.phys(module).join("bin");
            for name in sorted_names(&dir).unwrap_or_default() {
                if !name.starts_with('.') && is_program(store, &format!("{module}/bin/{name}")) {
                    theirs.push((name.clone(), format!("{module}/bin/{name}")));
                }
            }
        }
        for (name, target) in theirs {
            if !taken.insert(name.clone()) {
                continue; // the first one of a name: the Python's own, then by package
            }
            write_script(&progs.join("bin").join(&name), &program_script(&bash, &env_path, &ver, &target))?;
        }
        candidates.push(base_name(&env_path).to_string());
        let progs_path = store.add_path_referring(&progs, &format!("{name}-env-bin"), &candidates).with_context(|| format!("adding {} to the store", progs.display()))?;
        Ok(vec![env_path, progs_path])
    })();
    let _ = crate::store::remove_tree(&work);
    result
}

/// `modules` and every Python package they need (what each propagates, as
/// nixpkgs' `requiredPythonModules` follows it), each once, in a fixed order
/// (by name), whatever order they were found in.
fn with_needed(store: &Store, python: &str, ver: &str, modules: &[String]) -> Vec<String> {
    let is_module = |p: &str| p != python && store.phys(p).join("lib").join(ver).join("site-packages").is_dir();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut todo: Vec<String> = modules.to_vec();
    while let Some(m) = todo.pop() {
        if !seen.insert(m.clone()) {
            continue;
        }
        let listed = fs::read_to_string(store.phys(&m).join("nix-support/propagated-build-inputs")).unwrap_or_default();
        todo.extend(listed.split_whitespace().filter(|p| p.starts_with(STORE_DIR) && !seen.contains(*p) && is_module(p)).map(String::from));
    }
    let mut out: Vec<String> = seen.into_iter().collect();
    out.sort_by(|a, b| name_of(a).cmp(&name_of(b)).then(a.cmp(b)));
    out
}

/// Merge the folders `sources` (store paths) into `dst`, as nixpkgs'
/// buildEnv does: what one alone has is a link to it, a folder more than one
/// has is a folder of its own with theirs merged in, and of files with the
/// same name the first one's is taken. `skip`: names left out at the top.
fn merge(store: &Store, dst: &Path, sources: &[String], skip: &[&str]) -> Result<()> {
    let mut names: Vec<(String, Vec<String>)> = Vec::new();
    for src in sources {
        let Some(dir) = dir_behind(store, src) else { continue };
        for name in sorted_names(&dir)? {
            if skip.contains(&name.as_str()) {
                continue;
            }
            let logical = format!("{src}/{name}");
            match names.iter_mut().find(|(n, _)| *n == name) {
                Some((_, from)) => from.push(logical),
                None => names.push((name, vec![logical])),
            }
        }
    }
    for (name, from) in names {
        let to = dst.join(&name);
        let dirs: Vec<&String> = from.iter().filter(|f| dir_behind(store, f).is_some()).collect();
        if dirs.len() > 1 {
            fs::create_dir(&to)?;
            merge(store, &to, &dirs.into_iter().cloned().collect::<Vec<_>>(), &[])?;
        } else {
            std::os::unix::fs::symlink(&from[0], &to).with_context(|| format!("linking {}", to.display()))?;
        }
    }
    Ok(())
}

/// The folder a store path is, through links: on disk, or `None` when it isn't one.
fn dir_behind(store: &Store, logical: &str) -> Option<PathBuf> {
    let mut cur = logical.to_string();
    for _ in 0..16 {
        let phys = store.phys(&cur);
        let meta = fs::symlink_metadata(&phys).ok()?;
        if !meta.file_type().is_symlink() {
            return meta.is_dir().then_some(phys);
        }
        let target = fs::read_link(&phys).ok()?.to_string_lossy().into_owned();
        cur = if target.starts_with('/') { target } else { format!("{}/{target}", cur.rsplit_once('/')?.0) };
        if !cur.starts_with(STORE_DIR) {
            return None;
        }
    }
    None
}

/// Whether a store path is a program: a file (through links) that runs.
fn is_program(store: &Store, logical: &str) -> bool {
    let mut cur = logical.to_string();
    for _ in 0..16 {
        let Ok(meta) = fs::symlink_metadata(store.phys(&cur)) else { return false };
        if !meta.file_type().is_symlink() {
            return meta.is_file() && meta.permissions().mode() & 0o111 != 0;
        }
        let Ok(target) = fs::read_link(store.phys(&cur)) else { return false };
        let target = target.to_string_lossy().into_owned();
        cur = if target.starts_with('/') { target } else { format!("{}/{target}", cur.rsplit_once('/').map_or("", |c| c.0)) };
    }
    false
}

/// Whether `name` in `dir` is a link that ends at `target` there (python3 -> python3.13).
fn points_to(dir: &Path, name: &str, target: &str) -> bool {
    let mut cur = name.to_string();
    for _ in 0..8 {
        if cur == target {
            return true;
        }
        match fs::read_link(dir.join(&cur)) {
            Ok(t) if !t.to_string_lossy().contains('/') => cur = t.to_string_lossy().into_owned(),
            _ => return false,
        }
    }
    false
}

fn sorted_names(dir: &Path) -> Result<Vec<String>> {
    let mut names: Vec<String> = fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    Ok(names)
}

fn write_script(path: &Path, text: &str) -> Result<()> {
    fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o555))?;
    Ok(())
}

/// The Python's bin/python3.x: the interpreter, under the name it was run
/// by (so it finds this tree as its prefix), without ~/.local's packages.
fn interpreter_script(bash: &str, interpreter: &str) -> String {
    format!("#!{bash}/bin/bash\n# A Python with its packages, made by nami (as nixpkgs' python.withPackages).\nexport PYTHONNOUSERSITE=true\nexec -a \"$0\" {interpreter} \"$@\"\n")
}

/// A program of the Python's packages, run with the whole Python around it:
/// nixpkgs' Python reads these at start (sitecustomize.py).
fn program_script(bash: &str, env: &str, ver: &str, target: &str) -> String {
    format!(
        "#!{bash}/bin/bash\n# A program of a Python with its packages, made by nami (as nixpkgs' python.withPackages).\nexport NIX_PYTHONPREFIX={env} NIX_PYTHONEXECUTABLE={env}/bin/{ver} NIX_PYTHONPATH={env}/lib/{ver}/site-packages PYTHONNOUSERSITE=true\nexec {target} \"$@\"\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_is_its_python_and_packages() {
        let with = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(entry("f", "python3", Some("3.13"), &with(&["requests", "numpy"])).unwrap().as_deref(), Some("python313 with numpy, requests"));
        assert_eq!(entry("f", "python3", None, &with(&["numpy", "numpy"])).unwrap().as_deref(), Some("python3 with numpy"));
        assert_eq!(entry("f", "python314", None, &with(&["python314Packages.django"])).unwrap().as_deref(), Some("python314 with django"));
        assert_eq!(entry("f", "python313", Some("3.13"), &with(&["numpy"])).unwrap().as_deref(), Some("python313 with numpy"));
        assert_eq!(entry("f", "python3", Some("3.13"), &[]).unwrap(), None);
        let err = |pkg, v, w: &[&str]| entry("f", pkg, v, &with(w)).unwrap_err().to_string();
        assert!(err("python3", Some("3.13.2"), &["numpy"]).contains("its line, like 3.13"));
        assert!(err("python313", Some("3.12"), &["numpy"]).contains("isn't python313's"));
        assert!(err("nodejs", None, &["typescript"]).contains("with: is for a Python"));
        assert!(err("python3", None, &["numpy, requests"]).contains("each is a package's name"));
    }

    #[test]
    fn its_name_reads_back() {
        let s = parse("python313 with numpy, requests").unwrap();
        assert_eq!(s, Spec { python: "python313".into(), with: vec!["numpy".into(), "requests".into()] });
        assert_eq!(s.set(), "python313Packages");
        assert_eq!(parse(&s.attr()), Some(s.clone()));
        assert_eq!(s.wanted(), ["numpy", "requests", "pip"]);
        assert_eq!(parse("python3 with pip").unwrap().wanted(), ["pip"]);
        for not in ["python313", "ripgrep", "nodejs with x", "python3 with ", "./python3 with x", "python3@3.13"] {
            assert_eq!(parse(not), None, "{not}");
        }
        assert!(!crate::config::is_plain_entry("python313 with numpy"));
    }

    /// A store of a pretend Python and packages (hashes in Nix's letters: no e, o, t, u): what's made of them.
    #[test]
    fn the_python_has_its_packages_and_their_programs() {
        let root = std::env::temp_dir().join(format!("nami-python-env-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        // (As the file system names it: a Mac's temp dir is under /var, a link to /private/var.)
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let store = Store::open_direct(&root, false).unwrap();

        let path = |c: char, name: &str| format!("{STORE_DIR}/{}-{name}", c.to_string().repeat(32));
        let put = |p: &str, files: &[(&str, &str)], links: &[(&str, &str)], refs: &[&str]| {
            for (f, text) in files {
                let at = store.phys(p).join(f);
                fs::create_dir_all(at.parent().unwrap()).unwrap();
                fs::write(&at, text).unwrap();
                if f.contains("bin/") {
                    fs::set_permissions(&at, fs::Permissions::from_mode(0o755)).unwrap();
                }
            }
            for (l, to) in links {
                let at = store.phys(p).join(l);
                fs::create_dir_all(at.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink(to, at).unwrap();
            }
            store.register(p, refs.iter().map(|r| base_name(r).to_string()).collect()).unwrap();
        };
        let bash = path('b', "bash-5.3");
        put(&bash, &[("bin/bash", "")], &[], &[]);
        let py = path('p', "python3-3.13.9");
        put(
            &py,
            &[("bin/python3.13", "elf"), ("bin/pydoc3.13", "pydoc"), ("lib/python3.13/os.py", ""), ("lib/python3.13/site-packages/sitecustomize.py", ""), ("lib/libpython3.13.so", "")],
            &[("bin/python3", "python3.13"), ("bin/pydoc", "pydoc3.13")],
            &[&bash],
        );
        let pv = |name: &str| format!("lib/python3.13/site-packages/{name}");
        let (idna, urllib3) = (path('i', "python3.13-idna-3.10"), path('w', "python3.13-urllib3-2.5"));
        put(&idna, &[(&pv("idna/__init__.py"), "")], &[], &[]);
        put(&urllib3, &[(&pv("urllib3/__init__.py"), "")], &[], &[]);
        let requests = path('r', "python3.13-requests-2.32");
        put(&requests, &[(&pv("requests/__init__.py"), ""), ("nix-support/propagated-build-inputs", &format!("{py} {idna} {urllib3}"))], &[], &[&py, &idna, &urllib3]);
        // Two packages of one namespace (jaraco/): merged.
        let (ja, jb) = (path('j', "python3.13-jaraco.a-1"), path('k', "python3.13-jaraco.b-1"));
        put(&ja, &[(&pv("jaraco/a.py"), ""), ("bin/a-tool", "#!"), ("bin/.a-tool-wrapped", "#!")], &[], &[]);
        put(&jb, &[(&pv("jaraco/b.py"), "")], &[], &[]);

        let paths = make(&store, &py, &[requests.clone(), ja.clone(), jb.clone()]).unwrap_or_else(|e| panic!("{e:#}"));
        let (env, progs) = (store.phys(&paths[0]), store.phys(&paths[1]));
        assert!(paths[0].ends_with("-python3-3.13.9-env") && paths[1].ends_with("-python3-3.13.9-env-bin"), "{paths:?}");
        let site = env.join("lib/python3.13/site-packages");
        // Each package (and what requests needs) in one site-packages, links to their own.
        assert_eq!(fs::read_link(site.join("requests")).unwrap(), PathBuf::from(format!("{requests}/{}", pv("requests"))));
        for dep in ["idna", "urllib3", "sitecustomize.py"] {
            assert!(fs::symlink_metadata(site.join(dep)).unwrap().file_type().is_symlink(), "{dep}");
        }
        assert!(fs::symlink_metadata(site.join("jaraco")).unwrap().is_dir(), "a namespace both have: a folder of their own");
        assert!(fs::symlink_metadata(site.join("jaraco/a.py")).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(env.join("lib/python3.13/os.py")).unwrap(), PathBuf::from(format!("{py}/lib/python3.13/os.py")));
        assert_eq!(fs::read_link(env.join("lib/libpython3.13.so")).unwrap(), PathBuf::from(format!("{py}/lib/libpython3.13.so")));
        // The interpreter: run by its own name, from bash; python3 still a link to it.
        let script = fs::read_to_string(env.join("bin/python3.13")).unwrap();
        assert!(script.starts_with(&format!("#!{bash}/bin/bash\n")) && script.contains(&format!("exec -a \"$0\" {py}/bin/python3.13 \"$@\"")) && script.contains("PYTHONNOUSERSITE=true"), "{script}");
        assert_eq!(fs::read_link(env.join("bin/python3")).unwrap(), PathBuf::from("python3.13"));
        assert!(!env.join("bin/pydoc3.13").exists());
        // The programs: the packages' and the Python's own, with the Python around them.
        let tool = fs::read_to_string(progs.join("bin/a-tool")).unwrap();
        assert!(tool.contains(&format!("NIX_PYTHONPATH={}/lib/python3.13/site-packages", paths[0])) && tool.contains(&format!("exec {ja}/bin/a-tool \"$@\"")), "{tool}");
        assert!(progs.join("bin/pydoc3.13").is_file() && progs.join("bin/pydoc").is_file());
        assert!(!progs.join("bin/.a-tool-wrapped").exists() && !progs.join("bin/python3").exists());
        assert_eq!(fs::metadata(progs.join("bin/a-tool")).unwrap().permissions().mode() & 0o111, 0o111);
        // What they refer to: what gc keeps and nami.lock records.
        let e = Entry { attr: "python313 with jaraco.a, jaraco.b, requests".into(), name: "python3-3.13.9".into(), paths: paths.clone() };
        assert_eq!(inputs(&store, &e), {
            let mut all = vec![bash.clone(), py.clone(), idna.clone(), urllib3.clone(), requests.clone(), ja.clone(), jb.clone()];
            all.sort();
            all
        });
        assert!(store.closure(&paths).unwrap().contains(base_name(&urllib3)));
        // Made again from those: the same two paths.
        let l = crate::lock::Locked { name: e.name.clone(), paths: paths.clone(), rev: None, inputs: inputs(&store, &e) };
        crate::store::make_writable(&store.phys(STORE_DIR)).unwrap();
        for p in &paths {
            crate::store::remove_tree(&store.phys(p)).unwrap();
            fs::remove_file(store.var_dir().join("db").join(format!("{}.json", base_name(p)))).unwrap();
        }
        assert_eq!(pinned(&store, &e.attr, &l, true).unwrap().paths, paths);
        let _ = fs::remove_dir_all(&root);
    }
}
