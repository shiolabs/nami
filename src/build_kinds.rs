//! What kind of project a recipe builds, and how each kind is built.
//!
//! The kind comes from the source's files (Cargo.toml, go.mod, a configure
//! script, meson.build, CMakeLists.txt, configure.ac, pyproject.toml,
//! package.json, a Makefile), unless the recipe says (`build-system:
//! cargo`), or names a build tool in its build-deps (cmake, meson,
//! autoreconfHook). A recipe with build steps of its own (`build:`,
//! `install:`) and no `build-system:` is built as it says, as before. A
//! configure script that's there comes before CMake and Meson: it's what
//! stdenv always ran, and what a release tarball that has both is made for.
//!
//! Each kind maps onto what nixpkgs does for it, with nixpkgs' tools from
//! the catalog: CMake, Meson and autotools are stdenv's own hooks (cmake,
//! meson and ninja, autoreconfHook) added to the build's tools; Python is
//! buildPythonApplication's hooks (pypaBuildHook, pypaInstallHook,
//! wrapPython); Rust, Go and Node.js are built with phases written here,
//! like buildRustPackage, buildGoModule and buildNpmPackage, from their
//! dependencies as vendor.rs fetched them before the build. The build
//! itself never has the network.

use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Result};

use crate::archive::Tree;
use crate::build::Recipe;
use crate::index::Index;
use crate::lock::Pins;
use crate::store::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Cargo,
    Go,
    Python,
    Node,
    Meson,
    CMake,
    Autotools,
    Make,
}

/// Each kind: its name in `build-system:`, and what it is.
pub const KINDS: &[(Kind, &str, &str)] = &[
    (Kind::Cargo, "cargo", "Rust, with Cargo: its crates as its Cargo.lock pins them"),
    (Kind::Go, "go", "Go modules: its modules as its go.sum pins them"),
    (Kind::Python, "python", "Python, with pyproject.toml or setup.py: an app, with its dependencies from the catalog's Python packages"),
    (Kind::Node, "node", "Node.js, with npm: its packages as its package-lock.json pins them"),
    (Kind::Meson, "meson", "Meson and Ninja"),
    (Kind::CMake, "cmake", "CMake"),
    (Kind::Autotools, "autotools", "./configure && make (its configure made first when the source has only configure.ac)"),
    (Kind::Make, "make", "make && make install, as it is"),
];

impl Kind {
    pub fn name(self) -> &'static str {
        KINDS.iter().find(|(k, _, _)| *k == self).map(|(_, n, _)| *n).unwrap_or_default()
    }

    pub fn parse(s: &str) -> Result<Kind> {
        KINDS.iter().find(|(_, n, _)| *n == s).map(|(k, _, _)| *k).ok_or_else(|| {
            anyhow!("build-system: {s:?} isn't one nami knows: {}", KINDS.iter().map(|(_, n, _)| *n).collect::<Vec<_>>().join(", "))
        })
    }

    /// The tools every build of this kind has, by the catalog's names.
    pub fn tools(self) -> &'static [&'static str] {
        match self {
            Kind::Cargo => &["cargo", "rustc"],
            Kind::Go => &["go"],
            Kind::Python => &[],
            Kind::Node => &["nodejs"],
            Kind::Meson => &["meson", "ninja"],
            Kind::CMake => &["cmake"],
            Kind::Autotools | Kind::Make => &[],
        }
    }
}

/// The source's files the kinds read: manifests and lock files, and the
/// build files anywhere in it (what they ask for, for `nami recipe`).
pub fn wanted(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let top = !path.contains('/');
    let anywhere = ["CMakeLists.txt", "meson.build", "meson_options.txt", "meson.options", "Cargo.toml", "configure.ac", "configure.in"];
    let at_top = [
        "Cargo.lock", "go.mod", "go.sum", "pyproject.toml", "setup.py", "setup.cfg", "requirements.txt", "package.json", "package-lock.json",
        "npm-shrinkwrap.json", "Makefile", "makefile", "GNUmakefile", "configure", "build.zig", "README.md",
    ];
    let tests = path.split('/').any(|p| ["test", "tests", "testing", "examples", "example", "doc", "docs", "vendor", "third_party", "thirdparty", "external", "deps"].contains(&p));
    (anywhere.contains(&name) && path.matches('/').count() <= 3 && !tests)
        || (top && at_top.contains(&name))
        || (path.ends_with(".cmake") && path.matches('/').count() <= 2)
        || path == "vendor/modules.txt"
}

/// The kind of project in `tree`, from its files.
pub fn detect(tree: &Tree) -> Option<Kind> {
    let has = |p: &str| tree.has(p);
    if has("Cargo.toml") {
        Some(Kind::Cargo)
    } else if has("go.mod") {
        Some(Kind::Go)
    } else if has("configure") {
        Some(Kind::Autotools)
    } else if has("meson.build") {
        Some(Kind::Meson)
    } else if has("CMakeLists.txt") {
        Some(Kind::CMake)
    } else if has("configure.ac") || has("configure.in") {
        Some(Kind::Autotools)
    } else if has("pyproject.toml") || has("setup.py") {
        Some(Kind::Python)
    } else if has("package.json") {
        Some(Kind::Node)
    } else if has("Makefile") || has("makefile") || has("GNUmakefile") {
        Some(Kind::Make)
    } else {
        None
    }
}

/// The kind a recipe's build-deps say, by the build tool they name.
pub fn named_by_build_deps(recipe: &Recipe) -> Option<Kind> {
    let has = |t: &str| recipe.build_deps.iter().any(|d| d == t);
    if has("meson") {
        Some(Kind::Meson)
    } else if has("cmake") {
        Some(Kind::CMake)
    } else if has("autoreconfHook") {
        Some(Kind::Autotools)
    } else {
        None
    }
}

/// Whether a recipe's kind is worked out (or said): it builds from a
/// source and has no build steps of its own, or it names its kind.
pub fn applies(recipe: &Recipe) -> bool {
    recipe.build_system.is_some()
        || (recipe.extends.is_none() && recipe.unpack.is_none() && recipe.configure.is_none() && recipe.build.is_none() && recipe.install.is_none())
}

/// How a recipe of a kind is built, on top of what it says itself: the
/// tools it adds (catalog names; the recipe's own come first), the build's
/// variables (the recipe's own phases and `build-env` win), and the store
/// paths of its fetched dependencies.
#[derive(Debug, Default)]
pub struct Plan {
    pub build_deps: Vec<String>,
    /// What its programs run with (Node.js, for their `#!/usr/bin/env node`).
    pub deps: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub inputs: Vec<String>,
    /// Python: the recipe's deps are the program's (wrapPython's pythonPath).
    pub python_path: bool,
}

/// The plan for `recipe` of `kind`, whose source has `tree`.
pub fn plan(store: &Store, index: &Index, recipe: &Recipe, kind: Kind, tree: &Tree, pins: &mut Pins) -> Result<Plan> {
    let mut p = Plan { build_deps: kind.tools().iter().map(|t| t.to_string()).collect(), ..Default::default() };
    let phase = |p: &mut Plan, key: &str, body: &str, own: &Option<String>| {
        if own.is_none() {
            let (pre, post) = match key {
                "configurePhase" => ("preConfigure", "postConfigure"),
                "buildPhase" => ("preBuild", "postBuild"),
                _ => ("preInstall", "postInstall"),
            };
            p.env.insert(key.into(), format!("runHook {pre}\n{body}\nrunHook {post}\n"));
        }
    };
    match kind {
        Kind::Cargo => {
            let lock = tree.text("Cargo.lock").ok_or_else(|| {
                anyhow!("the source has no Cargo.lock, so its crates aren't pinned and nami can't fetch them for the build. A program's own repository usually has one; a library's doesn't (build the program that uses it)")
            })?;
            let vendor = crate::vendor::cargo(store, lock, pins)?;
            if crate::vendor::cargo_lock(lock)?.iter().any(|c| matches!(crate::recipe::names::build_names().crates.get(&c.name), Some(Some(_)))) {
                p.build_deps.push("pkg-config".into());
            }
            p.env.insert("CARGO_HOME".into(), "/build/.cargo-home".into());
            p.env.insert("CARGO_TERM_COLOR".into(), "never".into());
            p.env.insert("namiCargoFlags".into(), recipe.cargo_flags.join(" "));
            phase(&mut p, "configurePhase", &format!("mkdir -p \"$CARGO_HOME\"\ncat > \"$CARGO_HOME/config.toml\" <<'EOF'\n{}EOF", vendor.config), &recipe.configure);
            phase(&mut p, "buildPhase", "cargo build -j \"$NIX_BUILD_CORES\" --release --frozen $namiCargoFlags", &recipe.build);
            phase(
                &mut p,
                "installPhase",
                "mkdir -p \"$out/bin\"\nfind target/release -maxdepth 1 -type f -executable ! -name '*.so' ! -name '*.d' ! -name 'build-script-*' -exec cp {} \"$out/bin/\" \\;",
                &recipe.install,
            );
            p.inputs.push(vendor.path);
        }
        Kind::Go => {
            let go_mod = tree.text("go.mod").ok_or_else(|| anyhow!("the source has no go.mod"))?;
            p.build_deps = vec![go_tool(index, go_mod)?];
            let vendored = tree.has("vendor/modules.txt");
            let proxy = match tree.text("go.sum") {
                Some(sum) if !vendored && !crate::vendor::go_sum(sum).is_empty() => Some(crate::vendor::go(store, sum)?),
                _ => None,
            };
            p.env.insert("GOPROXY".into(), proxy.as_ref().map_or("off".into(), |path| format!("file://{path}")));
            p.env.insert("GOSUMDB".into(), "off".into());
            p.env.insert("GOTOOLCHAIN".into(), "local".into());
            p.env.insert("GOFLAGS".into(), if vendored { "-mod=vendor -trimpath".into() } else { "-mod=readonly -trimpath".into() });
            p.env.insert("GOPATH".into(), "/build/.go".into());
            p.env.insert("GOCACHE".into(), "/build/.go-cache".into());
            p.env.insert("namiGoFlags".into(), recipe.go_flags.join(" "));
            if recipe.configure.is_none() {
                p.env.insert("dontConfigure".into(), "1".into());
            }
            // The version, where Go programs usually keep it (as their own
            // Makefiles and goreleaser set it; one that has none is unchanged).
            phase(&mut p, "buildPhase", "GOBIN=/build/.go-bin go install -ldflags=\"-s -w -X main.version=$version -X main.Version=$version\" $namiGoFlags ./...", &recipe.build);
            phase(&mut p, "installPhase", "mkdir -p \"$out/bin\"\ncp /build/.go-bin/* \"$out/bin/\"", &recipe.install);
            p.inputs.extend(proxy);
        }
        Kind::Python => {
            let set = python_set(index, recipe, tree);
            p.build_deps = vec![format!("{set}.python"), format!("{set}.pypaBuildHook"), format!("{set}.pypaInstallHook"), format!("{set}.wrapPython")];
            for b in crate::recipe::wants::python_backend(tree) {
                if let crate::recipe::names::Found::Package(a) = python_package(index, &b, &set) {
                    p.build_deps.push(a);
                }
            }
            p.python_path = true;
            p.env.insert("pypaBuildFlags".into(), "--skip-dependency-check".into());
            for v in ["SETUPTOOLS_SCM_PRETEND_VERSION", "PDM_BUILD_SCM_VERSION"] {
                p.env.insert(v.into(), recipe.version.clone());
            }
            if recipe.configure.is_none() {
                p.env.insert("dontConfigure".into(), "1".into());
            }
            p.env.insert("postFixup".into(), "wrapPythonPrograms".into());
        }
        Kind::Node => {
            let pkg: serde_json::Value = serde_json::from_str(tree.text("package.json").ok_or_else(|| anyhow!("the source has no package.json"))?)?;
            let lock = tree.text("package-lock.json").or_else(|| tree.text("npm-shrinkwrap.json")).ok_or_else(|| {
                let other = ["yarn.lock", "pnpm-lock.yaml", "bun.lockb", "bun.lock"].into_iter().find(|f| tree.has(f));
                match other {
                    Some(f) => anyhow!("the source pins its packages with {f}: nami builds Node.js projects from npm's package-lock.json so far"),
                    None => anyhow!("the source has no package-lock.json, so its packages aren't pinned and nami can't fetch them for the build"),
                }
            })?;
            let cache = crate::vendor::npm(store, lock)?;
            let node = node_tool(index, pkg["engines"]["node"].as_str());
            p.build_deps = vec![node.clone()];
            if !recipe.deps.contains(&node) {
                p.deps.push(node);
            }
            let name = pkg["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(&recipe.name).to_string();
            p.env.insert("npm_config_cache".into(), "/build/.npm".into());
            for (k, v) in [("npm_config_offline", "true"), ("npm_config_audit", "false"), ("npm_config_fund", "false"), ("npm_config_update_notifier", "false"), ("npm_config_progress", "false")] {
                p.env.insert(k.into(), v.into());
            }
            phase(
                &mut p,
                "configurePhase",
                &format!("export HOME=/build/.home\nmkdir -p \"$npm_config_cache\"\ncp -r {cache}/_cacache \"$npm_config_cache/\"\nchmod -R u+w \"$npm_config_cache\"\nnpm ci --ignore-scripts\npatchShebangs node_modules\nnpm rebuild"),
                &recipe.configure,
            );
            if pkg["scripts"]["build"].is_string() {
                phase(&mut p, "buildPhase", "npm run build", &recipe.build);
            } else if recipe.build.is_none() {
                p.env.insert("dontBuild".into(), "1".into());
            }
            let mut install = format!(
                "npm prune --omit=dev --ignore-scripts\ndest=\"$out/lib/node_modules/{name}\"\nmkdir -p \"$dest\" \"$out/bin\"\n\
                 npm pack --dry-run --json --ignore-scripts > /build/.pack.json\n\
                 node -e 'for (const f of require(\"/build/.pack.json\")[0].files) console.log(f.path)' | while IFS= read -r f; do\n  mkdir -p \"$dest/$(dirname \"$f\")\"\n  cp -a \"$f\" \"$dest/$f\"\ndone\n\
                 [ ! -d node_modules ] || cp -a node_modules \"$dest/\""
            );
            for (bin, path) in node_bins(&pkg, &name) {
                let path = path.trim_start_matches("./");
                install += &format!("\nchmod +x \"$dest/{path}\"\nln -s \"../lib/node_modules/{name}/{path}\" \"$out/bin/{bin}\"");
            }
            phase(&mut p, "installPhase", &install, &recipe.install);
            p.inputs.push(cache);
        }
        Kind::Meson => {
            if tree.texts(|f| f.ends_with("meson.build")).any(|(_, t)| t.contains("dependency(")) {
                p.build_deps.push("pkg-config".into());
            }
        }
        // CMake 4 refuses a project written for CMake before 3.5 (its
        // cmake_minimum_required): this lets it build as it did.
        Kind::CMake => {
            p.env.insert("CMAKE_POLICY_VERSION_MINIMUM".into(), "3.5".into());
        }
        Kind::Autotools => {
            if !tree.has("configure") {
                p.build_deps.push("autoreconfHook".into());
                let ac = tree.text("configure.ac").or_else(|| tree.text("configure.in")).unwrap_or_default();
                if ac.contains("PKG_CHECK_") || ac.contains("PKG_PROG_PKG_CONFIG") {
                    p.build_deps.push("pkg-config".into());
                }
                if ac.contains("AM_GNU_GETTEXT") {
                    p.build_deps.push("gettext".into());
                }
            }
        }
        Kind::Make => {}
    }
    // The recipe's own tools stay as they are; these come after.
    p.build_deps.retain(|d| !recipe.build_deps.contains(d));
    p.build_deps.dedup();
    Ok(p)
}

/// The catalog's Go for go.mod's `go` line: its newest, if that's new
/// enough (Go builds what older ones did).
fn go_tool(index: &Index, go_mod: &str) -> Result<String> {
    let wanted = go_mod.lines().find_map(|l| l.trim().strip_prefix("go ")).map(|v| v.trim().to_string());
    let version = |attr: &str| index.get(attr).ok().map(|p| crate::query::version_of(&p.name));
    let have = version("go").unwrap_or_default();
    if let Some(w) = &wanted {
        if crate::recipe::version_cmp(&have, w) == std::cmp::Ordering::Less {
            bail!("go.mod asks for Go {w}, and the catalog's newest is {have}: `nami update` for a newer catalog");
        }
    }
    Ok("go".into())
}

/// The catalog's Node.js for `engines.node`: a range that starts at a
/// major version (^22, 22.x, ~22.1) gets that one if the catalog has it;
/// else the catalog's own.
fn node_tool(index: &Index, engines: Option<&str>) -> String {
    let fixed = engines.filter(|e| !e.trim_start().starts_with('>') && !e.contains("||"));
    let major = fixed.and_then(|r| r.trim_start_matches(['^', '~', '=', 'v', ' ']).split(['.', ' ', 'x']).next()?.parse::<u32>().ok());
    major.map(|m| format!("nodejs_{m}")).filter(|a| index.get(a).is_ok()).unwrap_or_else(|| "nodejs".into())
}

/// package.json's `bin`: (command, path) for each.
pub(crate) fn node_bins(p: &serde_json::Value, name: &str) -> Vec<(String, String)> {
    match &p["bin"] {
        serde_json::Value::String(s) => vec![(name.rsplit('/').next().unwrap_or(name).to_string(), s.clone())],
        serde_json::Value::Object(m) => m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect(),
        _ => Vec::new(),
    }
}

/// Which of the catalog's Python package sets a recipe builds with: the
/// one its deps name (`python313Packages.click`), else the newest that
/// `requires-python` takes.
pub(crate) fn python_set(index: &Index, recipe: &Recipe, tree: &Tree) -> String {
    let named = recipe.deps.iter().chain(&recipe.build_deps).find_map(|d| d.split_once('.').map(|(s, _)| s).filter(|s| s.starts_with("python3") && s.ends_with("Packages")).map(String::from));
    if let Some(s) = named {
        return s;
    }
    python_set_for(index, crate::recipe::wants::requires_python(tree).as_deref())
}

/// The newest package set whose Python `spec` (`>=3.10,<3.14`) takes.
pub(crate) fn python_set_for(index: &Index, spec: Option<&str>) -> String {
    let Some(spec) = spec else { return "python3Packages".into() };
    let minor = |attr: &str| index.get(attr).ok().and_then(|p| crate::query::version_of(&p.name).strip_prefix("3.")?.split('.').next()?.parse::<u32>().ok());
    if minor("python3").is_some_and(|m| crate::bundle::project::python_fits(spec, 3, m)) {
        return "python3Packages".into();
    }
    (8..=20).rev().map(|m| format!("python3{m}Packages")).find(|s| index.get(&format!("{s}.python")).is_ok() && crate::bundle::project::python_fits(spec, 3, s[7..s.len() - 8].parse().unwrap_or(0))).unwrap_or_else(|| "python3Packages".into())
}

fn python_package(index: &Index, name: &str, set: &str) -> crate::recipe::names::Found {
    use crate::recipe::names::{build_names, python_name, Found};
    let norm = python_name(name);
    let attr = match build_names().python.get(&norm) {
        Some(None) => return Found::Nothing,
        Some(Some(a)) => a.clone(),
        None => norm,
    };
    let full = format!("{set}.{attr}");
    if index.get(&full).is_ok() { Found::Package(full) } else { Found::Missing }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(paths: &[&str]) -> Tree {
        Tree { paths: paths.iter().map(|p| p.to_string()).collect(), files: Default::default() }
    }

    #[test]
    fn detects_the_kind() {
        assert_eq!(detect(&tree(&["Cargo.toml", "Cargo.lock", "Makefile"])), Some(Kind::Cargo));
        assert_eq!(detect(&tree(&["go.mod", "Makefile"])), Some(Kind::Go));
        assert_eq!(detect(&tree(&["meson.build", "CMakeLists.txt"])), Some(Kind::Meson));
        assert_eq!(detect(&tree(&["CMakeLists.txt", "configure", "configure.ac"])), Some(Kind::Autotools), "a configure script that's there first");
        assert_eq!(detect(&tree(&["CMakeLists.txt", "configure.ac"])), Some(Kind::CMake));
        assert_eq!(detect(&tree(&["CMakeLists.txt", "setup.py"])), Some(Kind::CMake));
        assert_eq!(detect(&tree(&["configure", "Makefile.in"])), Some(Kind::Autotools));
        assert_eq!(detect(&tree(&["configure.ac", "Makefile.am"])), Some(Kind::Autotools));
        assert_eq!(detect(&tree(&["pyproject.toml", "Makefile"])), Some(Kind::Python));
        assert_eq!(detect(&tree(&["setup.py"])), Some(Kind::Python));
        assert_eq!(detect(&tree(&["package.json", "package-lock.json"])), Some(Kind::Node));
        assert_eq!(detect(&tree(&["Makefile", "main.c"])), Some(Kind::Make));
        assert_eq!(detect(&tree(&["README.md"])), None);
        // A build file down in the tree isn't the project's kind.
        assert_eq!(detect(&tree(&["src/CMakeLists.txt", "Makefile"])), Some(Kind::Make));
        assert_eq!(Kind::parse("cargo").unwrap(), Kind::Cargo);
        assert!(Kind::parse("bazel").unwrap_err().to_string().contains("cargo, go, python, node, meson, cmake, autotools, make"));
        assert!(wanted("Cargo.lock") && wanted("crates/core/Cargo.toml") && wanted("cmake/FindFoo.cmake") && !wanted("src/main.rs") && !wanted("sub/package.json"));
    }

    #[test]
    fn which_recipes_are_worked_out() {
        let r = |yaml: &str| -> Recipe { serde_yaml::from_str(yaml).unwrap() };
        assert!(applies(&r("name: a\nversion: '1'\nsource: { url: 'https://x/a.tar.gz' }\n")));
        assert!(!applies(&r("name: a\nversion: '1'\nsource: { path: ./bin }\ninstall: cp -r . $out\n")), "build steps of its own: as before");
        assert!(applies(&r("name: a\nversion: '1'\nbuild-system: cargo\ninstall: cp x $out\n")));
        assert!(!applies(&r("extends: jq\n")));
        assert_eq!(named_by_build_deps(&r("name: a\nbuild-deps: [cmake, pkg-config]\n")), Some(Kind::CMake));
        assert_eq!(named_by_build_deps(&r("name: a\nbuild-deps: [pkg-config]\n")), None);
        let pkg: serde_json::Value = serde_json::from_str(r#"{"name": "@acme/tool", "bin": "./cli.js"}"#).unwrap();
        assert_eq!(node_bins(&pkg, "@acme/tool"), vec![("tool".to_string(), "./cli.js".to_string())]);
        let pkg: serde_json::Value = serde_json::from_str(r#"{"bin": {"a": "a.js", "b": "bin/b.js"}}"#).unwrap();
        assert_eq!(node_bins(&pkg, "x").len(), 2);
    }
}
