//! What a failed build's log says, in plain words, with the line to add
//! to the recipe: a header, pkg-config module, CMake package or library
//! that wasn't found is a package to add to `deps` (names.rs finds which,
//! as `nami recipe` does), a command that isn't there one for
//! `build-deps`; Cargo, Go and npm wanting the network mean the
//! project's lock file doesn't pin everything it uses. The lines of the
//! log that say so are shown with it.

use std::path::Path;

use crate::index::Index;
use crate::recipe::names::{Found, Mapper};
use crate::store::Store;

/// One thing the log says is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    Header(String),
    PkgConfig(String),
    CMake(String),
    Library(String),
    Command(String),
    PythonModule(String),
    /// The build's own message that something wasn't found (`Lua could
    /// not be found!`): its name, looked up as a pkg-config module and a
    /// command; else the message as it is.
    Named { name: String, message: String },
    /// What nami says as it is, nothing to look up.
    Said(String),
}

/// What a log's problems are, and the lines that say so.
#[derive(Debug, Default)]
pub struct Explained {
    pub problems: Vec<String>,
    pub lines: Vec<String>,
}

/// Where the catalog's names come from: names.rs, or a test's table.
pub trait Resolve {
    fn header(&mut self, h: &str) -> Found;
    fn pkg_config(&mut self, m: &str) -> Found;
    fn cmake(&mut self, n: &str) -> Found;
    fn library(&mut self, l: &str) -> Found;
    fn command(&mut self, c: &str) -> Found;
    fn python(&mut self, d: &str) -> Found;
}

impl Resolve for Mapper<'_> {
    fn header(&mut self, h: &str) -> Found {
        Mapper::header(self, h)
    }
    fn pkg_config(&mut self, m: &str) -> Found {
        Mapper::pkg_config(self, m)
    }
    fn cmake(&mut self, n: &str) -> Found {
        Mapper::cmake(self, n)
    }
    fn library(&mut self, l: &str) -> Found {
        Mapper::link_library(self, l)
    }
    fn command(&mut self, c: &str) -> Found {
        Mapper::command(self, c)
    }
    fn python(&mut self, d: &str) -> Found {
        Mapper::python(self, d, "python3Packages")
    }
}

/// A failed build's log, explained (None: nothing nami recognizes in it).
pub fn explain(store: &Store, index: &Index, log: &str) -> Option<Explained> {
    explain_with(&mut Mapper::new(store, index), log)
}

pub fn explain_with(r: &mut dyn Resolve, log: &str) -> Option<Explained> {
    let found = find(log);
    if found.is_empty() {
        return None;
    }
    let lines: Vec<&str> = log.lines().collect();
    let mut out = Explained::default();
    for (problem, at) in found.iter().take(6) {
        let said = render(r, problem);
        if !out.problems.contains(&said) {
            out.problems.push(said);
        }
        let shown = lines.get(*at).map(|l| l.trim_end().to_string()).unwrap_or_default();
        if !out.lines.contains(&shown) {
            out.lines.push(shown);
        }
    }
    Some(out)
}

/// The tools CMake's find_package finds rather than libraries.
const CMAKE_TOOLS: &[&str] = &["Python", "Python3", "PythonInterp", "Perl", "BISON", "FLEX", "Git", "Doxygen", "Gettext", "Rust", "Cargo", "PkgConfig"];
/// Commands builds try and go on without.
const SOFT_COMMANDS: &[&str] = &["git", "ccache", "lsb_release", "hostname", "svnversion", "hg", "sccache", "distcc"];
/// Python's build backends: what `python -m build` needs, not the program.
const PYTHON_BUILD: &[&str] = &["setuptools", "setuptools_scm", "wheel", "hatchling", "hatch_vcs", "flit_core", "poetry", "pdm", "mesonpy", "scikit_build_core", "maturin", "Cython", "pybind11"];

fn render(r: &mut dyn Resolve, p: &Problem) -> String {
    let add = |found: Found, key: &str, what: &str, missing: &str| match found {
        Found::Package(a) => format!("{what}: add `{a}` to {key}"),
        Found::Nothing => format!("{what}, though every build should have it: check the lines above"),
        Found::Missing => format!("{what}, and nami knows no package that has it: {missing}"),
    };
    match p {
        Problem::Header(h) => add(r.header(h), "deps", &format!("{h} wasn't found"), "add the library it's from to deps (or it's one the build makes itself, and a step before failed)"),
        Problem::PkgConfig(m) => add(r.pkg_config(m), "deps", &format!("the pkg-config module {m} wasn't found"), "find the library it's from (`nami search`) and add it to deps"),
        Problem::CMake(n) if n == "PkgConfig" => "CMake needs pkg-config: add `pkg-config` to build-deps".into(),
        Problem::CMake(n) => {
            let key = if CMAKE_TOOLS.contains(&n.as_str()) { "build-deps" } else { "deps" };
            add(r.cmake(n), key, &format!("CMake's find_package({n}) found nothing"), "find the package it's from (`nami search`) and add it to deps")
        }
        Problem::Library(l) => add(r.library(l), "deps", &format!("the library -l{l} wasn't found"), "find the package with lib{l}.so (`nami search`) and add it to deps"),
        Problem::Command(c) => add(r.command(c), "build-deps", &format!("the build runs {c}, which isn't there"), "find the package with it and add it to build-deps"),
        Problem::PythonModule(m) => {
            let key = if PYTHON_BUILD.contains(&m.as_str()) { "build-deps" } else { "deps" };
            let dist = match m.as_str() {
                "yaml" => "pyyaml",
                "PIL" => "pillow",
                "bs4" => "beautifulsoup4",
                "dateutil" => "python-dateutil",
                "attr" => "attrs",
                "google" => "protobuf",
                "jwt" => "pyjwt",
                "cv2" => "opencv-python",
                "gi" => "pygobject",
                "mesonpy" => "meson-python",
                other => other,
            };
            add(r.python(dist), key, &format!("the Python module {m} wasn't found"), "find its package (`nami search`) and add it")
        }
        Problem::Named { name, message } => {
            // `curses/ncurses`: each, the last first (the more specific).
            for n in name.to_lowercase().split('/').rev() {
                if let Found::Package(a) = r.pkg_config(n) {
                    return format!("{message}: add `{a}` to deps");
                }
                if let Found::Package(a) = r.command(n) {
                    return format!("{message}: add `{a}` to build-deps");
                }
            }
            format!("the build stopped: {message}")
        }
        Problem::Said(s) => s.clone(),
    }
}

/// `text`'s part between `start` and `end` (the first of each).
fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = text.find(start)? + start.len();
    let j = text[i..].find(end)?;
    Some(&text[i..i + j])
}

/// A name from a log, without quotes, brackets or a version constraint.
fn clean(s: &str) -> String {
    let s = s.trim().trim_matches(|c: char| "'\"`‘’<>".contains(c));
    s.split([' ', '>', '<', '=']).next().unwrap_or(s).to_string()
}

/// A command without the version some are run by (`aclocal-1.16`).
fn unversioned(cmd: &str) -> String {
    match cmd.rsplit_once('-') {
        Some((c, v)) if !c.is_empty() && !v.is_empty() && v.chars().all(|ch| ch.is_ascii_digit() || ch == '.') => c.to_string(),
        _ => cmd.to_string(),
    }
}

/// The problems a log shows, each with its line. A failure's own lines
/// (an error) come first; ones a build goes on after (an optional
/// command missing) only when there's nothing else.
pub fn find(log: &str) -> Vec<(Problem, usize)> {
    let lines: Vec<&str> = log.lines().collect();
    let (mut hard, mut soft) = (Vec::<(Problem, usize)>::new(), Vec::<(Problem, usize)>::new());
    let mut last_cmake_error: Option<usize> = None;
    let mut pending_pkg: Option<(String, usize)> = None;
    for (i, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        let mut push = |p: Problem, is_hard: bool| {
            let list = if is_hard { &mut hard } else { &mut soft };
            if !list.iter().any(|(q, _)| *q == p) {
                list.push((p, i));
            }
        };
        if line.starts_with("CMake Error") {
            last_cmake_error = Some(i);
        }
        // Compilers.
        if let Some(rest) = line.split_once("fatal error: ").map(|(_, r)| r) {
            if let Some(h) = rest.strip_suffix(": No such file or directory") {
                push(Problem::Header(clean(h)), true);
            } else if let Some(h) = rest.strip_suffix(" file not found") {
                push(Problem::Header(clean(h)), true);
            }
        }
        if let Some(rest) = line.split_once("cannot find -l").map(|(_, r)| r) {
            let l: String = rest.chars().take_while(|c| !c.is_whitespace() && *c != ':').collect();
            if !l.is_empty() {
                push(Problem::Library(l), true);
            }
        }
        // pkg-config, from autoconf, CMake, Meson and Rust's build scripts.
        if let Some(m) = between(line, "No package '", "' found") {
            push(Problem::PkgConfig(clean(m)), true);
        }
        if let Some(m) = between(line, "Package '", "', required by") {
            pending_pkg = Some((clean(m), i));
        }
        if line.contains("A required package was not found") || line.contains("None of the required") {
            if let Some(m) = between(line, "None of the required '", "' found") {
                push(Problem::PkgConfig(clean(m)), true);
            } else if let Some((m, _)) = pending_pkg.take() {
                push(Problem::PkgConfig(m), true);
            }
        }
        if let Some(m) = line.strip_prefix("Package ").and_then(|r| r.strip_suffix(" was not found in the pkg-config search path.")) {
            push(Problem::PkgConfig(clean(m)), true);
        }
        if let Some(m) = between(line, "The system library `", "` required by crate") {
            push(Problem::PkgConfig(clean(m)), true);
        }
        if line.contains("The pkg-config command could not be found") || line.contains("Pkg-config binary for machine") || line.contains("Pkg-config for machine host machine not found") || line.contains("pkg-config: not found") {
            push(Problem::Command("pkg-config".into()), true);
        }
        if line.contains("Could not find directory of OpenSSL installation") || line.contains("Could not find openssl via pkg-config") {
            push(Problem::PkgConfig("openssl".into()), true);
        }
        if let Some(m) = between(line, "Dependency \"", "\" not found").or_else(|| between(line, "Dependency '", "' not found")) {
            if line.contains("ERROR") {
                push(Problem::PkgConfig(clean(m)), true);
            }
        }
        // CMake's find_package.
        if let Some(rest) = line.split_once("Could NOT find ").map(|(_, r)| r) {
            let n: String = rest.chars().take_while(|c| !c.is_whitespace() && *c != '(' && *c != ',').collect();
            if !n.is_empty() {
                push(Problem::CMake(n), !line.starts_with("--"));
            }
        }
        if let Some(n) = between(line, "package configuration file provided by \"", "\"") {
            let in_error = last_cmake_error.is_some_and(|e| i - e < 8);
            push(Problem::CMake(n.to_string()), in_error);
        }
        // Commands.
        if let Some(before) = line.strip_suffix(": command not found").or_else(|| line.strip_suffix(": not found")) {
            let cmd = before.rsplit(": ").next().unwrap_or(before).trim();
            if !cmd.is_empty() && !cmd.contains(' ') && !cmd.starts_with("No package") {
                let cmd = unversioned(cmd.rsplit('/').next().unwrap_or(cmd));
                let is_hard = !SOFT_COMMANDS.contains(&cmd.as_str());
                push(Problem::Command(cmd), is_hard);
            }
        }
        if let Some(c) = between(line, "Program '", "' not found").or_else(|| between(line, "Program \"", "\" not found")) {
            if line.contains("ERROR") {
                push(Problem::Command(c.to_string()), true);
            }
        }
        if let Some(c) = between(line, "'", "' is missing on your system") {
            push(Problem::Command(unversioned(c)), true);
        }
        if line.starts_with("make") && line.ends_with(": No such file or directory") && !line.contains("***") {
            let cmd = line.trim_end_matches(": No such file or directory").rsplit(": ").next().unwrap_or("");
            if !cmd.is_empty() && !cmd.contains(['/', ' ', '.']) {
                push(Problem::Command(cmd.to_string()), true);
            }
        }
        for env in ["/usr/bin/env: ", "env: "] {
            if let Some(c) = line.strip_prefix(env).and_then(|r| r.strip_suffix(": No such file or directory")) {
                push(Problem::Command(clean(c)), true);
            }
        }
        if let Some(interp) = between(line, ": ", ": bad interpreter") {
            let interp = interp.rsplit(": ").next().unwrap_or(interp);
            push(Problem::Command(interp.rsplit('/').next().unwrap_or(interp).to_string()), true);
        }
        for (mac, cmd) in [("PKG_CHECK_MODULES", "pkg-config"), ("PKG_PROG_PKG_CONFIG", "pkg-config"), ("AM_GNU_GETTEXT", "gettext"), ("LT_INIT", "libtool"), ("AC_PROG_LIBTOOL", "libtool")] {
            if line.contains(&format!("possibly undefined macro: {mac}")) || line.contains(&format!("{mac}: command not found")) {
                push(Problem::Command(cmd.into()), true);
            }
        }
        // A build's own message: Meson's error(), CMake's FATAL_ERROR, configure's.
        for start in ["ERROR: Problem encountered: ", "configure: error: ", "CMake Error: "] {
            let Some(msg) = line.split_once(start).map(|(_, r)| r.trim().trim_end_matches(['!', '.'])) else { continue };
            let lower = msg.to_lowercase();
            // `X could not be found`, `X is required`; `cannot find (required) X (library)`.
            let before = ["could not be found", "not found", "is required", "was not found"].iter().find_map(|end| lower.find(end).map(|i| msg[..i].trim()));
            let after = ["cannot find", "can't find", "could not find", "unable to find", "requires"].iter().find_map(|start| lower.find(start).map(|i| msg[i + start.len()..].trim()));
            let name = before.and_then(|n| n.split_whitespace().last()).or_else(|| {
                after.and_then(|n| n.split_whitespace().find(|w| !["required", "the", "a", "any", "usable", "working"].contains(&w.to_lowercase().as_str())))
            });
            if let Some(name) = name.filter(|n| n.len() > 1) {
                push(Problem::Named { name: clean(name), message: msg.to_string() }, true);
            }
        }
        if line.contains("Compatibility with CMake < 3.5 has been removed") {
            push(Problem::Said("it's written for a CMake older than 3.5: add `-DCMAKE_POLICY_VERSION_MINIMUM=3.5` to cmake-flags".into()), true);
        }
        // Python.
        if let Some(m) = between(line, "No module named '", "'") {
            push(Problem::PythonModule(m.split('.').next().unwrap_or(m).to_string()), true);
        }
        // Cargo, Go and npm, which nami gave what their lock files pin.
        if line.contains("needs to be updated but --frozen was passed") || line.contains("needs to be updated but --locked was passed") {
            push(Problem::Said("Cargo.lock in the source is out of date with its Cargo.toml, and nami builds with what Cargo.lock pins: use a release whose Cargo.lock is up to date, or add a patch that updates it (patches:)".into()), true);
        } else if line.contains("--offline was specified") || line.contains("attempting to make an HTTP request") {
            push(Problem::Said("Cargo wanted the network: something it builds isn't pinned in Cargo.lock (it's out of date, or a dependency comes from somewhere nami doesn't fetch). Use a release whose Cargo.lock is up to date, or add a patch that updates it".into()), true);
        }
        if let Some(rest) = line.split_once("requires rustc ").map(|(_, r)| r) {
            let v = rest.split_whitespace().next().unwrap_or("");
            push(Problem::Said(format!("it needs Rust {v} or newer, newer than the catalog's: `nami update` for a newer catalog, or build an older release")), true);
        }
        if let Some(rest) = line.split_once("missing go.sum entry").map(|(_, r)| r) {
            let what = rest.rsplit(' ').next().unwrap_or("").trim_end_matches(';');
            push(Problem::Said(format!("go.sum doesn't pin {}: nami fetches only what go.sum pins. Use a release whose go.sum is complete (`go mod tidy`), or add a patch to it (patches:)", if what.is_empty() { "everything it uses" } else { what })), true);
        }
        if line.contains("module lookup disabled by GOPROXY=off") || (line.contains("reading file:///nix/store/") && line.contains("/@v/")) {
            push(Problem::Said("Go wanted a module that go.sum doesn't pin (or the source's vendor/ folder lacks): use a release whose go.sum is complete, or add a patch to it".into()), true);
        }
        if let Some(rest) = line.split_once("go.mod requires go >= ").map(|(_, r)| r) {
            let v = rest.split_whitespace().next().unwrap_or("");
            push(Problem::Said(format!("go.mod asks for Go {v}, newer than the catalog's: `nami update` for a newer catalog")), true);
        }
        if line.contains("cache mode is 'only-if-cached'") || (line.contains("ENOTCACHED") && line.contains("request to ")) {
            let pkg = between(line, "request to ", " failed").unwrap_or("a package");
            push(Problem::Said(format!("npm wanted {pkg} from the network: package-lock.json doesn't pin it (it's out of date with package.json). Use a release whose lock file is up to date, or add a patch to it")), true);
        }
        if line.contains("can only install packages when your package.json and package-lock.json") {
            push(Problem::Said("package-lock.json is out of date with package.json, so npm won't install from it: use a release whose lock file is up to date, or add a patch to it".into()), true);
        }
        if line.contains("gyp ERR! find Python") {
            push(Problem::Command("python3".into()), true);
        }
    }
    if hard.is_empty() { soft } else { hard }
}

/// A source that isn't the file it was, said plainly: what was recorded,
/// what it is now, and how to take it if it's right (`accept`).
pub fn changed_upstream(url: &str, want: &str, got: &str, lock: &Path) -> String {
    let dir = crate::recipe::shown_dir(lock.parent().unwrap_or(Path::new(".")));
    format!(
        "{url} isn't the file it was: {} has its sha256 as {want}, and it's {got} now. The project may have published it again, or it was changed on the way. If you trust the new one, take it: nami recipe update {dir} --refetch",
        lock.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "nami.lock".into())
    )
}

/// The same, for a sha256 the recipe itself gives.
pub fn wrong_hash_in_recipe(url: &str, want: &str, got: &str, dir: &Path) -> String {
    let dir = crate::recipe::shown_dir(dir);
    format!(
        "{url} isn't the file the recipe says: its sha256 is {got}, and package.nami says {want}. If you trust the new one, take it: nami recipe update {dir} --refetch (or put the new sha256 in package.nami)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table in place of the catalog.
    struct Table;
    impl Resolve for Table {
        fn header(&mut self, h: &str) -> Found {
            match h {
                "openssl/ssl.h" => Found::Package("openssl".into()),
                "zlib.h" => Found::Package("zlib".into()),
                _ => Found::Missing,
            }
        }
        fn pkg_config(&mut self, m: &str) -> Found {
            match m {
                "lua" => Found::Package("lua5_4".into()),
                "ncurses" => Found::Package("ncurses".into()),
                "libcurl" => Found::Package("curl".into()),
                "glib-2.0" => Found::Package("glib".into()),
                "openssl" => Found::Package("openssl".into()),
                _ => Found::Missing,
            }
        }
        fn cmake(&mut self, n: &str) -> Found {
            match n {
                "ZLIB" => Found::Package("zlib".into()),
                "Python3" => Found::Package("python3".into()),
                _ => Found::Missing,
            }
        }
        fn library(&mut self, l: &str) -> Found {
            if l == "z" { Found::Package("zlib".into()) } else { Found::Missing }
        }
        fn command(&mut self, c: &str) -> Found {
            match c {
                "cmake" => Found::Package("cmake".into()),
                "pkg-config" => Found::Package("pkg-config".into()),
                "python3" => Found::Package("python3".into()),
                "makeinfo" => Found::Package("texinfo".into()),
                "aclocal" => Found::Package("autoreconfHook".into()),
                _ => Found::Missing,
            }
        }
        fn python(&mut self, d: &str) -> Found {
            Found::Package(format!("python3Packages.{d}"))
        }
    }

    fn said(log: &str) -> Vec<String> {
        explain_with(&mut Table, log).map(|e| e.problems).unwrap_or_default()
    }

    #[test]
    fn headers_libraries_and_modules() {
        assert_eq!(said("cc -c main.c\nmain.c:3:10: fatal error: openssl/ssl.h: No such file or directory\n    3 | #include <openssl/ssl.h>\ncompilation terminated.\n"), ["openssl/ssl.h wasn't found: add `openssl` to deps"]);
        assert_eq!(said("main.c:1:10: fatal error: 'zlib.h' file not found\n"), ["zlib.h wasn't found: add `zlib` to deps"]);
        assert_eq!(said("/nix/store/x-binutils/bin/ld: cannot find -lz: No such file or directory\ncollect2: error: ld returned 1 exit status\n"), ["the library -lz wasn't found: add `zlib` to deps"]);
        let ac = "checking for GLIB... no\nconfigure: error: Package requirements (glib-2.0 >= 2.40) were not met:\n\nNo package 'glib-2.0' found\n";
        assert_eq!(said(ac), ["the pkg-config module glib-2.0 wasn't found: add `glib` to deps"]);
        let cm = "-- Checking for module 'libcurl'\n--   Package 'libcurl', required by 'virtual:world', not found\nCMake Error at /nix/store/x-cmake/share/cmake-4.1/Modules/FindPkgConfig.cmake:1093 (message):\n  A required package was not found\n";
        assert_eq!(said(cm), ["the pkg-config module libcurl wasn't found: add `curl` to deps"]);
        let me = "Run-time dependency glib-2.0 found: NO (tried pkgconfig and cmake)\n\nmeson.build:10:7: ERROR: Dependency \"glib-2.0\" not found, tried pkgconfig and cmake\n";
        assert_eq!(said(me), ["the pkg-config module glib-2.0 wasn't found: add `glib` to deps"]);
        let rs = "  --- stderr\n  Could not find directory of OpenSSL installation, and this `-sys` crate cannot\n";
        assert_eq!(said(rs), ["the pkg-config module openssl wasn't found: add `openssl` to deps"]);
        assert_eq!(said("  The system library `libgit2` required by crate `libgit2-sys` was not found.\n")[0], "the pkg-config module libgit2 wasn't found, and nami knows no package that has it: find the library it's from (`nami search`) and add it to deps");
    }

    #[test]
    fn cmake_and_commands() {
        // An optional package missing is a line CMake goes on after; a required one an error.
        let log = "-- Could NOT find Doxygen (missing: DOXYGEN_EXECUTABLE)\nCMake Error at /nix/store/x/FindPackageHandleStandardArgs.cmake:227 (message):\n  Could NOT find ZLIB (missing: ZLIB_LIBRARY ZLIB_INCLUDE_DIR)\n";
        assert_eq!(said(log), ["CMake's find_package(ZLIB) found nothing: add `zlib` to deps"]);
        assert_eq!(said("CMake Error at CMakeLists.txt:4 (find_package):\n  Could NOT find PkgConfig (missing: PKG_CONFIG_EXECUTABLE)\n"), ["CMake needs pkg-config: add `pkg-config` to build-deps"]);
        assert_eq!(said("/nix/store/x-bash/bin/bash: line 1: cmake: command not found\n"), ["the build runs cmake, which isn't there: add `cmake` to build-deps"]);
        assert_eq!(said("WARNING: 'makeinfo' is missing on your system.\nmake: *** [Makefile:1: doc/hello.info] Error 127\n"), ["the build runs makeinfo, which isn't there: add `texinfo` to build-deps"]);
        assert_eq!(said("/bin/sh: line 1: aclocal-1.16: command not found\n"), ["the build runs aclocal, which isn't there: add `autoreconfHook` to build-deps"]);
        assert_eq!(said("sh: frobnicate: not found\n"), ["the build runs frobnicate, which isn't there, and nami knows no package that has it: find the package with it and add it to build-deps"]);
        assert_eq!(said("WARNING: 'aclocal-1.16' is missing on your system.\n"), ["the build runs aclocal, which isn't there: add `autoreconfHook` to build-deps"]);
        assert_eq!(said("/bin/sh: ./gen.py: /usr/bin/python3: bad interpreter: No such file or directory\n"), ["the build runs python3, which isn't there: add `python3` to build-deps"]);
        assert_eq!(said("meson.build:20:0: ERROR: Program 'python3' not found or not executable\n"), ["the build runs python3, which isn't there: add `python3` to build-deps"]);
        // git missing alone is said; with something else, the other thing.
        assert_eq!(said("sh: git: command not found\n").len(), 1);
        assert_eq!(said("sh: git: command not found\nmain.c:1:10: fatal error: zlib.h: No such file or directory\n"), ["zlib.h wasn't found: add `zlib` to deps"]);
        assert!(said("all fine\n").is_empty());
        // The build's own words.
        assert_eq!(said("src/meson.build:41:2: ERROR: Problem encountered: Lua could not be found!\n"), ["Lua could not be found: add `lua5_4` to deps"]);
        assert_eq!(said("configure: error: libfoo is required\n"), ["the build stopped: libfoo is required"]);
        assert_eq!(said("configure: error: cannot find required curses/ncurses library\n"), ["cannot find required curses/ncurses library: add `ncurses` to deps"]);
        assert_eq!(said("CMake Error at CMakeLists.txt:1 (cmake_minimum_required):\n  Compatibility with CMake < 3.5 has been removed from CMake.\n"), ["it's written for a CMake older than 3.5: add `-DCMAKE_POLICY_VERSION_MINIMUM=3.5` to cmake-flags"]);
    }

    #[test]
    fn python_and_lock_files() {
        assert_eq!(said("ModuleMissingError: No module named 'setuptools_scm'\n"), ["the Python module setuptools_scm wasn't found: add `python3Packages.setuptools_scm` to build-deps"]);
        assert_eq!(said("ModuleMissingError: No module named 'yaml.loader'\n"), ["the Python module yaml wasn't found: add `python3Packages.pyyaml` to deps"]);
        assert!(said("error: the lock file /build/source/Cargo.lock needs to be updated but --frozen was passed to prevent this\n")[0].starts_with("Cargo.lock in the source is out of date"));
        assert!(said("go: example.com/x: missing go.sum entry for module providing package golang.org/x/sys/unix; to add:\n")[0].starts_with("go.sum doesn't pin"));
        assert!(said("npm error code ENOTCACHED\nnpm error request to https://registry.npmjs.org/left-pad failed: cache mode is 'only-if-cached' but no cached response is available.\n")[0].contains("https://registry.npmjs.org/left-pad"));
        assert!(said("error: package `clap v4.6.0` cannot be built because it requires rustc 1.90 or newer, while the currently active rustc version is 1.89.0\n")[0].contains("Rust 1.90 or newer"));
        let msg = changed_upstream("https://x/a.tar.gz", "ab", "cd", Path::new("/tmp/r/nami.lock"));
        assert!(msg.contains("nami.lock has its sha256 as ab, and it's cd now") && msg.contains("nami recipe update /tmp/r --refetch"), "{msg}");
    }
}
