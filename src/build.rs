//! `nami build`: build packages from YAML recipes, the way nixpkgs does.
//!
//! A nixpkgs build is bash + `stdenv/setup` + environment variables. The Nix
//! language only computes those variables. stdenv, its hooks (cmake, meson,
//! autotools, patchelf, autoPatchelfHook, ...) and the compilers are ordinary
//! store paths in the binary cache, so nami writes the variables from YAML and
//! runs `genericBuild` in a sandbox (bubblewrap, also from nixpkgs):
//!
//! - only the inputs' closure is visible, under /nix/store, read-only
//! - no network, a fresh /build, and $out as the only writable store path
//! - the output path is a hash of the recipe and every input path, so the same
//!   recipe gives the same path on every machine, and is skipped if present.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, FileTimes};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::index::Index;

/// One of a package's outputs, as every nami file writes it: `libgcc.lib`
/// is libgcc's lib output, `util-linux.bin` util-linux's programs. A name
/// the catalog has with a dot in it (`nerd-fonts.jetbrains-mono`) is that
/// package. The earlier `libgcc^lib` still reads (a notice says the new
/// spelling): ("libgcc", Some("lib")); a package alone: (name, None).
pub fn split_output<'a>(index: &Index, dep: &'a str) -> (&'a str, Option<&'a str>) {
    if let Some((attr, output)) = dep.split_once('^') {
        return (attr, Some(output));
    }
    // (A version, `foo@1.2`, has a dot of its own.)
    if dep.contains('@') || index.get(dep).is_ok() {
        return (dep, None);
    }
    match dep.rsplit_once('.') {
        Some((attr, output)) if index.get(attr).is_ok_and(|p| p.outputs.contains_key(output)) => (attr, Some(output)),
        _ => (dep, None),
    }
}

/// The notice for an output written the earlier way (`libgcc^lib`): None
/// when it's written as it is now.
pub fn old_output_spelling(dep: &str) -> Option<String> {
    let (attr, output) = dep.split_once('^')?;
    Some(format!("{dep}: an output is written with a dot now, {attr}.{output} (the ^ spelling still works until nami 1.0)"))
}
use crate::store::{base_name, remove_tree, store_path, valid_store_name, Store, STORE_DIR};

const SYSTEM: &str = crate::index::SYSTEM;
/// Bump when the build environment changes in a way that affects outputs.
const BUILDER_VERSION: u32 = 1;
pub const RECIPE_FILE: &str = "package.nami";

/// A package.nami's keys that say how to build it; the rest is its setup
/// (its services, files, settings...), as nami's own package files have.
pub const RECIPE_KEYS: &[&str] = &[
    "extends", "name", "version", "source", "bundle", "build-system", "deps", "build-deps", "runtime-deps", "patches",
    "configure-flags", "cmake-flags", "meson-flags", "make-flags", "cargo-flags", "go-flags", "unpack", "configure", "build", "check",
    "install", "build-env",
];
/// Keys that make it a build of its own: without them, it's nixpkgs'
/// package of its name, set up.
const BUILD_KEYS: &[&str] = &["extends", "source", "bundle", "unpack", "configure", "build", "install"];

/// A package.nami's two parts: how it's built (None: it isn't, it's the
/// catalog's package of its name) and its setup.
pub fn split_package(text: &str) -> Result<(Option<serde_yaml::Mapping>, serde_yaml::Mapping)> {
    let mut value: serde_yaml::Value = crate::yaml_text::from_str(text)?;
    crate::config::modern_quietly(&mut value, "package.nami")?;
    let mut recipe = serde_yaml::Mapping::new();
    let mut setup = serde_yaml::Mapping::new();
    for (k, v) in value.as_mapping().cloned().unwrap_or_default() {
        if k.as_str().is_some_and(|k| RECIPE_KEYS.contains(&k)) {
            recipe.insert(k, v);
        } else {
            setup.insert(k, v);
        }
    }
    let builds = recipe.keys().any(|k| k.as_str().is_some_and(|k| BUILD_KEYS.contains(&k)));
    Ok((builds.then_some(recipe), setup))
}
/// Bump when deb.rs lays out a package differently.
const DEB_LAYOUT: u32 = 3;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Recipe {
    /// Rebuild this nixpkgs package with the changes below instead of
    /// starting from scratch. Lists (patches, deps, flags) are appended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    /// One source, or several Debian packages unpacked together (an app
    /// and its data package), or either for each kind of computer (a
    /// prebuilt program's download for x86_64 and for ARM).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Sources>,
    /// Debian packages whose libraries go in the package's lib: for
    /// libraries nixpkgs has no fitting version of.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bundle: Vec<Source>,
    /// What kind of project it is (`cargo`, `go`, `python`, `node`,
    /// `meson`, `cmake`, `autotools`, `make`: build_kinds.rs), when its
    /// files shouldn't decide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_system: Option<String>,
    /// Libraries and runtime dependencies (nixpkgs `buildInputs`).
    #[serde(default)]
    pub deps: Vec<String>,
    /// Tools needed only while building (nixpkgs `nativeBuildInputs`).
    #[serde(default)]
    pub build_deps: Vec<String>,
    /// Libraries a prebuilt binary loads at runtime with dlopen (libGL, libudev):
    /// autoPatchelfHook adds them to its RPATH (nixpkgs `runtimeDependencies`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtime_deps: Vec<String>,
    #[serde(default)]
    pub patches: Vec<String>,
    #[serde(default)]
    pub configure_flags: Vec<String>,
    #[serde(default)]
    pub cmake_flags: Vec<String>,
    #[serde(default)]
    pub meson_flags: Vec<String>,
    #[serde(default)]
    pub make_flags: Vec<String>,
    /// Flags for `cargo build` (`--features pcre2`) and `go install`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cargo_flags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub go_flags: Vec<String>,
    /// Shell snippets replacing stdenv's default phases.
    #[serde(default)]
    pub unpack: Option<String>,
    #[serde(default)]
    pub configure: Option<String>,
    #[serde(default)]
    pub build: Option<String>,
    #[serde(default)]
    pub check: Option<String>,
    #[serde(default)]
    pub install: Option<String>,
    /// Extra environment variables for the build (`build-env`, as
    /// `build-deps` are the build's packages: `env` is the system's).
    #[serde(default)]
    pub build_env: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Hex, or SRI (`sha256-<base64>`). Without it, the first build records
    /// the file's in the nami.lock next to the recipe, and checks it after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// A local file or directory, relative to the recipe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// File name for a downloaded source, when the URL doesn't end in one
    /// (stdenv picks how to unpack from the extension).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `deb`: a Debian package, for when its name doesn't end in .deb.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl Source {
    /// A Debian package: nami unpacks it itself, laid out like a nixpkgs
    /// package (see deb.rs), and the build starts from its files.
    pub fn is_deb(&self) -> bool {
        let file = self.name.as_deref().or(self.url.as_deref()).or(self.path.as_deref()).unwrap_or_default();
        self.kind.as_deref() == Some("deb") || file.ends_with(".deb")
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Sources {
    One(Source),
    Several(Vec<Source>),
    /// By the kind of computer it's built on (`x86_64-linux:`, `aarch64-linux:`): a prebuilt
    /// program's download for each. Each is one source or several, as above.
    PerSystem(BTreeMap<String, Sources>),
}

/// The kinds of computer a recipe's `source:` can be given for (`index::SYSTEM`'s).
pub const SYSTEMS: &[&str] = &["x86_64-linux", "aarch64-linux", "x86_64-darwin", "aarch64-darwin"];

impl Sources {
    /// What this computer builds from: the source, the several, or its kind's (none when the
    /// recipe has none for it: [`Sources::here`] says so).
    pub fn list(&self) -> &[Source] {
        self.list_for(SYSTEM)
    }

    /// What a computer of that kind (`aarch64-linux`) builds from.
    pub fn list_for(&self, system: &str) -> &[Source] {
        match self {
            Sources::One(s) => std::slice::from_ref(s),
            Sources::Several(v) => v,
            Sources::PerSystem(by) => by.get(system).map(|s| s.list_for(system)).unwrap_or_default(),
        }
    }

    /// [`Sources::list`], for a build: refused when a kind of computer it names isn't one,
    /// or when this computer's kind isn't among them.
    pub fn here(&self, name: &str) -> Result<&[Source]> {
        if let Sources::PerSystem(by) = self {
            for (system, s) in by {
                ensure!(SYSTEMS.contains(&system.as_str()), "source: {system} isn't a kind of computer nami knows: {}", SYSTEMS.join(", "));
                ensure!(!matches!(s, Sources::PerSystem(_)), "source: {system}: takes a source or a list of them, not more kinds of computer");
            }
            ensure!(
                by.contains_key(SYSTEM),
                "{name} has no source for this computer ({SYSTEM}): its recipe has one for {}",
                by.keys().map(String::as_str).collect::<Vec<_>>().join(" and ")
            );
        }
        Ok(self.list())
    }
}

/// What a build produced: output name -> store path.
/// Where built recipes are remembered (var_dir/built/<key>: their outputs),
/// as `built_key` says what went in: a computer made from an image finds
/// its packages built, offline, without stdenv or their sources.
pub const BUILT_DIR: &str = "built";

/// A remembered build (`BUILT_DIR`).
pub fn read_built(path: &Path) -> Option<Built> {
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    let outputs: BTreeMap<String, String> = v["outputs"].as_object()?.iter().map(|(k, p)| Some((k.clone(), p.as_str()?.to_string()))).collect::<Option<_>>()?;
    Some(Built { name: v["name"].as_str()?.to_string(), outputs })
}

#[derive(Debug, Clone)]
pub struct Built {
    pub name: String,
    pub outputs: BTreeMap<String, String>,
}

impl Built {
    /// What goes on PATH: `bin` if split out, else `out`.
    pub fn install_paths(&self) -> Vec<String> {
        let o = if self.outputs.contains_key("bin") { "bin" } else { "out" };
        self.outputs.get(o).or_else(|| self.outputs.values().next()).into_iter().cloned().collect()
    }

    /// What another build depends on: `dev` if split out, like mkDerivation.
    fn dep_path(&self) -> String {
        let o = if self.outputs.contains_key("dev") { "dev" } else { "out" };
        self.outputs.get(o).or_else(|| self.outputs.values().next()).cloned().unwrap_or_default()
    }
}

/// One sandboxed build: what recipes and `extends:` both boil down to, and
/// what a user sends the daemon. It names its outputs only through Nix-style
/// placeholders; their store paths are a hash of the whole job, computed by
/// whoever runs it. So nobody can build something and claim it's a path
/// someone else will trust: different content means a different job, so a
/// different path.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub name: String,
    pub outputs: Vec<String>,
    pub command: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// Files written into /build before the build starts.
    pub files: Vec<(String, String)>,
    /// Store paths whose closure is visible in the sandbox.
    pub inputs: Vec<String>,
}

impl Job {
    /// Output name -> store path.
    pub fn output_paths(&self) -> Result<BTreeMap<String, String>> {
        ensure!(valid_name(&self.name), "bad job name {:?}", self.name);
        ensure!(!self.outputs.is_empty(), "a job needs an output");
        let hash = hex(&Sha256::digest(format!("tray-job-v{BUILDER_VERSION}:{SYSTEM}:{}", serde_json::to_string(self)?).as_bytes()));
        self.outputs
            .iter()
            .map(|o| {
                ensure!(valid_name(o), "bad output name {o:?}");
                let name = if o == "out" { self.name.clone() } else { format!("{}-{o}", self.name) };
                Ok((o.clone(), store_path(&format!("{hash}:{o}"), &name)))
            })
            .collect()
    }

    /// The job with placeholders replaced by the real output paths.
    fn materialize(&self, outputs: &BTreeMap<String, String>) -> Job {
        let rewrites: Vec<(String, String)> = outputs.iter().map(|(o, p)| (placeholder(o), p.clone())).collect();
        let fix = |s: &str| rewrites.iter().fold(s.to_string(), |acc, (from, to)| acc.replace(from.as_str(), to));
        Job {
            name: self.name.clone(),
            outputs: self.outputs.clone(),
            command: self.command.iter().map(|c| fix(c)).collect(),
            env: self.env.iter().map(|(k, v)| (k.clone(), fix(v))).collect(),
            files: self.files.iter().map(|(n, d)| (n.clone(), fix(d))).collect(),
            inputs: self.inputs.clone(),
        }
    }
}

pub struct Builder<'a> {
    store: &'a Store,
    index: &'a Index,
    pub keep_failed: bool,
    /// Whether unfree packages may be installed (`unfree: false` in the file says no).
    pub allow_unfree: bool,
    /// Recipe dirs currently being built, to report dependency cycles.
    stack: Mutex<Vec<PathBuf>>,
}

impl<'a> Builder<'a> {
    pub fn new(store: &'a Store, index: &'a Index) -> Self {
        Builder { store, index, keep_failed: false, allow_unfree: true, stack: Mutex::new(Vec::new()) }
    }

    pub fn store(&self) -> &'a Store {
        self.store
    }

    /// Is this package reference a recipe (a path) rather than a nixpkgs attr?
    pub fn is_recipe(reference: &str) -> bool {
        reference.starts_with("./") || reference.starts_with("../") || reference.starts_with('/')
    }

    /// Build a recipe (a directory with package.nami, or the .nami file itself).
    pub fn build(&self, at: &Path) -> Result<Built> {
        let file = if at.is_dir() { crate::oldnames::find_in(at, RECIPE_FILE) } else { at.to_path_buf() };
        let file = file.canonicalize().with_context(|| format!("no recipe at {}", file.display()))?;
        let dir = file.parent().unwrap().to_path_buf();
        {
            let mut stack = self.stack.lock().unwrap();
            if stack.contains(&dir) {
                bail!("dependency cycle through {}", dir.display());
            }
            stack.push(dir.clone());
        }
        let result = self.build_inner(&file, &dir);
        self.stack.lock().unwrap().pop();
        result
    }

    fn build_inner(&self, file: &Path, dir: &Path) -> Result<Built> {
        let text = fs::read_to_string(file)?;
        // Its setup (services, files...) is the nami file's to apply, not the build's.
        let (build, _) = split_package(&text).with_context(|| format!("in {}", file.display()))?;
        let build = build.ok_or_else(|| anyhow!("{}: nothing to build (no source:, extends: or install:): it's the catalog's package of its name, set up", file.display()))?;
        let recipe: Recipe = serde_yaml::from_value(serde_yaml::Value::Mapping(build)).with_context(|| format!("in {}", file.display()))?;
        for s in [&recipe.name, &recipe.version] {
            ensure!(s.is_empty() || valid_name(s), "name/version may only use [A-Za-z0-9+-._?=]: {s:?}");
        }
        // Built before (here, or where this computer's image was made)?
        // Then nothing needs fetching: not stdenv, not the sources.
        let key = self.built_key(&recipe, dir)?;
        let memo = self.store.var_dir().join(BUILT_DIR).join(&key);
        if let Some(built) = read_built(&memo).filter(|b| b.outputs.values().all(|p| self.store.is_valid(base_name(p)))) {
            return Ok(built);
        }
        let built = self.build_recipe(&recipe, dir)?;
        if fs::create_dir_all(memo.parent().unwrap()).is_ok() {
            let _ = fs::write(&memo, serde_json::json!({ "name": built.name, "outputs": built.outputs }).to_string());
        }
        Ok(built)
    }

    /// What goes into a recipe's build, as a key: the recipe, its folder's
    /// files, what it's built from and with (by store path, which says
    /// their versions), stdenv's, and what it extends. All known without
    /// fetching anything.
    fn built_key(&self, recipe: &Recipe, dir: &Path) -> Result<String> {
        let mut h = Sha256::new();
        h.update(format!("tray-built-v1:{BUILDER_VERSION}:{SYSTEM}\0").as_bytes());
        h.update(serde_json::to_vec(recipe)?);
        fn walk(root: &Path, rel: &Path, out: &mut Vec<(PathBuf, PathBuf)>) -> Result<()> {
            for e in fs::read_dir(root.join(rel))? {
                let e = e?;
                let r = rel.join(e.file_name());
                let ty = e.file_type()?;
                if ty.is_dir() {
                    walk(root, &r, out)?;
                } else {
                    out.push((r, e.path()));
                }
            }
            Ok(())
        }
        let mut files = Vec::new();
        walk(dir, Path::new(""), &mut files)?;
        files.sort();
        for (rel, path) in files {
            h.update(rel.to_string_lossy().as_bytes());
            h.update([0]);
            match fs::symlink_metadata(&path)?.file_type().is_symlink() {
                true => h.update(fs::read_link(&path)?.to_string_lossy().as_bytes()),
                false => h.update(Sha256::digest(fs::read(&path)?)),
            }
        }
        outside_files_key(recipe, dir, &mut h)?;
        for p in self.resolve_inputs(&recipe.deps, dir)?.iter().chain(&self.resolve_inputs(&recipe.build_deps, dir)?).chain(&self.resolve_runtime(&recipe.runtime_deps, dir)?) {
            h.update(p.as_bytes());
            h.update([0]);
        }
        h.update(self.output(self.index.get("stdenv")?, "out")?.as_bytes());
        // What its kind adds (its kind is known only from its source, so
        // every kind's tools): another compiler, another build.
        if crate::build_kinds::applies(recipe) && !is_deb_recipe(recipe) {
            h.update(b"kinds\0");
            for (kind, _, _) in crate::build_kinds::KINDS {
                for tool in kind.tools() {
                    if let Ok(p) = self.index.get(tool).and_then(|p| self.output(p, if p.outputs.contains_key("dev") { "dev" } else { "out" })) {
                        h.update(p.as_bytes());
                    }
                }
            }
        }
        if let Some(base) = &recipe.extends {
            let pkg = self.index.get(base)?;
            let out = if pkg.outputs.contains_key("out") { "out".to_string() } else { pkg.outputs.keys().next().cloned().unwrap_or_default() };
            h.update(self.output(pkg, &out)?.as_bytes());
        }
        Ok(hex(&h.finalize()))
    }

    fn build_recipe(&self, recipe: &Recipe, dir: &Path) -> Result<Built> {
        if let Some(base) = &recipe.extends {
            return self.build_extends(recipe, base, dir);
        }
        ensure!(!recipe.name.is_empty() && !recipe.version.is_empty(), "a recipe needs `name` and `version` (or `extends`)");

        // Inputs: nixpkgs attrs via the index, other recipes by building them.
        let sources = recipe.source.as_ref().map(|s| s.here(&recipe.name)).transpose()?.unwrap_or_default();
        let deb = !sources.is_empty() && sources.iter().all(Source::is_deb);
        ensure!(deb || sources.len() <= 1, "source: several sources must all be Debian packages");
        ensure!(deb || recipe.bundle.is_empty(), "bundle: needs a Debian package as the source");
        let mut build_deps = recipe.build_deps.clone();
        if deb && !build_deps.iter().any(|d| d == "autoPatchelfHook") {
            // A .deb's programs look for libraries in /usr/lib: point them at nixpkgs'.
            build_deps.push("autoPatchelfHook".into());
        }
        let src = if deb {
            Some(self.unpack_debs(sources, &recipe.bundle, dir, &recipe.name, &recipe.version)?)
        } else {
            sources.first().map(|s| self.fetch_source(s, dir, &recipe.name, &recipe.version)).transpose()?
        };
        // What kind of project it is, and what that adds (build_kinds.rs).
        let plan = match &src {
            Some(src) if !deb && crate::build_kinds::applies(recipe) => self.plan(recipe, dir, src)?,
            _ => None,
        };
        if let Some(plan) = &plan {
            for d in &plan.build_deps {
                if !build_deps.contains(d) {
                    build_deps.push(d.clone());
                }
            }
        }
        // Paths its programs have built in that are its own (/usr/share/foo):
        // libredirect points them at the package.
        let redirects = match &src {
            Some(src) if deb => crate::deb::hard_coded(&self.store.phys(src))?,
            _ => Vec::new(),
        };
        let mut dep_refs = recipe.deps.clone();
        dep_refs.extend(plan.as_ref().map(|p| p.deps.clone()).unwrap_or_default());
        if !redirects.is_empty() {
            dep_refs.push("libredirect".into());
        }
        let deps = self.resolve_inputs(&dep_refs, dir)?;
        let build_deps = self.resolve_inputs(&build_deps, dir)?;
        let runtime_deps = self.resolve_runtime(&recipe.runtime_deps, dir)?;
        let stdenv = self.output(self.index.get("stdenv")?, "out")?;
        let patches = self.add_patches(&recipe.patches, dir)?;

        let full_name = format!("{}-{}", recipe.name, recipe.version);
        let out = placeholder("out");

        let mut env = build_env(&recipe, &out, &stdenv, src.as_deref(), &patches, &deps, &build_deps)?;
        let mut kind_inputs = Vec::new();
        if let Some(plan) = plan {
            // The recipe's own steps and build-env win over its kind's.
            for (k, v) in plan.env {
                env.entry(k).or_insert(v);
            }
            if plan.python_path {
                env.entry("pythonPath".into()).or_insert_with(|| deps.join(" "));
            }
            kind_inputs = plan.inputs;
        }
        if deb {
            // Nothing to configure or build: its files are the package.
            for (key, phase, default) in [
                ("dontConfigure", &recipe.configure, "1"), ("dontBuild", &recipe.build, "1"),
                ("installPhase", &recipe.install, "runHook preInstall\nmkdir -p $out\ncp -a . $out/\nrunHook postInstall\n"),
            ] {
                if phase.is_none() {
                    env.insert(key.into(), default.into());
                }
            }
            ensure!(!env.contains_key("preFixup"), "build-env: preFixup is set by nami for a .deb");
            env.insert("preFixup".into(), deb_fixup(&redirects));
        }
        if !runtime_deps.is_empty() {
            env.insert("runtimeDependencies".into(), runtime_deps.join(" "));
        }
        self.store.realise(std::slice::from_ref(&stdenv))?;
        let shell = stdenv_shell(self.store, &stdenv)?;
        let mut inputs: Vec<String> = deps.iter().chain(&build_deps).chain(&runtime_deps).cloned().collect();
        inputs.push(stdenv);
        inputs.extend(src);
        inputs.extend(patches);
        inputs.extend(kind_inputs);
        self.run(Job {
            name: full_name,
            outputs: vec!["out".into()],
            // nixpkgs' default-builder.sh; stdenv's traps expect a file, not `bash -c`.
            command: vec![shell, "-e".into(), "/build/.builder.sh".into()],
            env,
            files: vec![(".builder.sh".into(), "source \"$stdenv/setup\"\ngenericBuild\n".into())],
            inputs,
        })
    }

    /// Rebuild a nixpkgs package with changes. Its build definition comes from
    /// the index (`drvs/<attr>.json`); everything it depends on still comes
    /// from the binary cache, so only this one package is built.
    fn build_extends(&self, recipe: &Recipe, base: &str, dir: &Path) -> Result<Built> {
        let (_, drv) = self.load_drv(base)?;
        let mut attrs = match drv.get("structuredAttrs") {
            Some(serde_json::Value::Object(a)) => a.clone(),
            _ => bail!("{base}: only derivations with structured attrs can be extended (nixpkgs' default since 2026)"),
        };
        let old_outputs: BTreeMap<String, String> = drv["outputs"]
            .as_object()
            .ok_or_else(|| anyhow!("{base}: derivation has no outputs"))?
            .iter()
            .map(|(o, v)| Ok((o.clone(), format!("{STORE_DIR}/{}", v["path"].as_str().ok_or_else(|| anyhow!("{base}: output {o} has no path (content-addressed?)"))?))))
            .collect::<Result<_>>()?;

        let str_attr = |k: &str| attrs.get(k).and_then(|v| v.as_str()).map(String::from);
        let name = if recipe.name.is_empty() { str_attr("pname").unwrap_or_else(|| base.to_string()) } else { recipe.name.clone() };
        let version = if recipe.version.is_empty() { str_attr("version").unwrap_or_default() } else { recipe.version.clone() };
        // Without a name: of its own, the name is nixpkgs' (`libpng-apng-1.6.43`,
        // with the recipe's version), so the build can stand in for it (replace:).
        let old_version = str_attr("version").unwrap_or_default();
        let full_name = match str_attr("name") {
            Some(n) if recipe.name.is_empty() && !old_version.is_empty() && n.ends_with(&format!("-{old_version}")) => format!("{}{version}", &n[..n.len() - old_version.len()]),
            Some(n) if recipe.name.is_empty() && recipe.version.is_empty() => n,
            _ if version.is_empty() => name.clone(),
            _ => format!("{name}-{version}"),
        };

        let deps = self.resolve_inputs(&recipe.deps, dir)?;
        let build_deps = self.resolve_inputs(&recipe.build_deps, dir)?;
        let runtime_deps = self.resolve_runtime(&recipe.runtime_deps, dir)?;
        let sources = recipe.source.as_ref().map(|s| s.here(&name)).transpose()?.unwrap_or_default();
        ensure!(sources.len() <= 1 && recipe.bundle.is_empty(), "extends: takes one source, and no bundle");
        let src = sources.first().map(|s| self.fetch_source(s, dir, &name, &version)).transpose()?;
        let patches = self.add_patches(&recipe.patches, dir)?;

        // The definition mentions its own (old) output paths; point them at
        // placeholders, which become the new paths when the job runs.
        let rewrites: Vec<(String, String)> = old_outputs.iter().map(|(o, old)| (old.clone(), placeholder(o))).collect();
        let mut attrs_value = serde_json::Value::Object(std::mem::take(&mut attrs));
        rewrite_strings(&mut attrs_value, &rewrites);
        let mut attrs = match attrs_value { serde_json::Value::Object(a) => a, _ => unreachable!() };

        // Apply the recipe on top.
        use serde_json::Value as V;
        attrs.insert("name".into(), V::String(full_name.clone()));
        if !recipe.name.is_empty() {
            attrs.insert("pname".into(), V::String(name.clone()));
        }
        if !recipe.version.is_empty() {
            attrs.insert("version".into(), V::String(version.clone()));
        }
        if let Some(src) = &src {
            attrs.insert("src".into(), V::String(src.clone()));
        }
        let mut append = |key: &str, items: &[String]| {
            if items.is_empty() {
                return;
            }
            let list = attrs.entry(key.to_string()).or_insert_with(|| V::Array(Vec::new()));
            if let V::Array(list) = list {
                list.extend(items.iter().cloned().map(V::String));
            }
        };
        append("patches", &patches);
        append("buildInputs", &deps);
        append("nativeBuildInputs", &build_deps);
        append("runtimeDependencies", &runtime_deps);
        append("configureFlags", &recipe.configure_flags);
        append("cmakeFlags", &recipe.cmake_flags);
        append("mesonFlags", &recipe.meson_flags);
        append("makeFlags", &recipe.make_flags);
        for (key, phase) in [
            ("unpackPhase", &recipe.unpack), ("configurePhase", &recipe.configure), ("buildPhase", &recipe.build),
            ("checkPhase", &recipe.check), ("installPhase", &recipe.install),
        ] {
            if let Some(script) = phase {
                attrs.insert(key.into(), V::String(script.clone()));
            }
        }
        if recipe.check.is_some() {
            attrs.insert("doCheck".into(), V::Bool(true));
        }
        for (k, v) in &recipe.build_env {
            attrs.insert(k.clone(), V::String(v.clone()));
        }
        // In the files Nix writes, `outputs` is a map from name to path.
        attrs.insert("outputs".into(), V::Object(old_outputs.keys().map(|o| (o.clone(), V::String(placeholder(o)))).collect()));
        let attrs = V::Object(attrs);

        let mut command: Vec<String> = vec![drv["builder"].as_str().unwrap_or_default().to_string()];
        command.extend(drv["args"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(String::from)));
        let mut drv_env: BTreeMap<String, String> = drv["env"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
            .collect();
        for v in drv_env.values_mut().chain(command.iter_mut()) {
            for (from, to) in &rewrites {
                *v = v.replace(from, to);
            }
        }
        let mut env = nix_env();
        env.extend(drv_env);
        env.insert("NIX_ATTRS_JSON_FILE".into(), "/build/.attrs.json".into());
        env.insert("NIX_ATTRS_SH_FILE".into(), "/build/.attrs.sh".into());

        // Inputs: every store path the definition mentions, plus its sources.
        let mut inputs: Vec<String> = drv["inputs"]["srcs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str().map(|s| format!("{STORE_DIR}/{s}")))
            .collect();
        let mut mentioned = std::collections::BTreeSet::new();
        collect_store_paths(&attrs, &mut mentioned);
        for s in env.values().chain(command.iter()) {
            find_store_paths(s, &mut mentioned);
        }
        inputs.extend(mentioned.into_iter().map(|b| format!("{STORE_DIR}/{b}")));

        self.run(Job {
            name: full_name,
            outputs: old_outputs.keys().cloned().collect(),
            command,
            env,
            files: vec![(".attrs.json".into(), serde_json::to_string(&attrs)?), (".attrs.sh".into(), attrs_sh(&attrs))],
            inputs,
        })
    }

    /// Run a job in the sandbox and register its outputs. Skips the build if
    /// every output is already in the store.
    pub fn run(&self, job: Job) -> Result<Built> {
        let outputs = job.output_paths()?;
        let built = Built { name: job.name.clone(), outputs: outputs.clone() };
        if outputs.values().all(|p| self.store.is_valid(base_name(p))) {
            crate::say!("{}: already built", job.name);
            return Ok(built);
        }
        if let Some(socket) = self.store.daemon() {
            let got = crate::daemon::client::build(socket, &job)?;
            ensure!(got == outputs, "the daemon built different paths than expected");
            return Ok(built);
        }
        // Someone already built it and pushed it to one of our caches?
        let outs: Vec<String> = outputs.values().cloned().collect();
        if self.store.substitutable(&outs) {
            crate::say!("{}: downloading from cache instead of building", job.name);
            self.store.realise(&outs)?;
            return Ok(built);
        }
        let to_fetch: Vec<String> = job.inputs.iter().filter(|p| !self.store.is_valid(base_name(p))).cloned().collect();
        self.store.realise(&to_fetch)?;
        let job = job.materialize(&outputs);
        self.sandbox(&job, &outputs, None, false)?;
        Ok(built)
    }

    /// Run a build whose inputs are all present, with its outputs at
    /// `outputs`, then register them. A fixed-output build (a download:
    /// `fixed` = method and expected hash) gets the network, and its result
    /// must have exactly that hash.
    ///
    /// Recipes get their outputs as ready-made directories. nixpkgs builds
    /// (`nix_outputs`) create them themselves, as files or directories, as
    /// under Nix: for those the sandbox's /nix/store is a writable staging
    /// directory with the inputs mounted read-only inside it.
    fn sandbox(&self, job: &Job, outputs: &BTreeMap<String, String>, fixed: Option<(&str, &str)>, nix_outputs: bool) -> Result<()> {
        let bwrap = self.output(self.index.get("bubblewrap")?, "out")?;
        self.store.realise(std::slice::from_ref(&bwrap))?;
        let closure = self.store.closure(&job.inputs)?;

        let first = outputs.values().next().cloned().unwrap_or_default();
        crate::say!("building {} -> {first}", job.name);
        let work = self.store.var_dir().join("builds").join(base_name(&first));
        if work.exists() {
            remove_tree(&work)?;
        }
        let (build_dir, etc) = (work.join("build"), work.join("etc"));
        fs::create_dir_all(&build_dir)?;
        fs::create_dir_all(&etc)?;
        fs::write(etc.join("passwd"), "root:x:0:0:Nix build user:/build:/noshell\nnixbld:x:1000:100:Nix build user:/build:/noshell\nnobody:x:65534:65534:Nobody:/:/noshell\n")?;
        fs::write(etc.join("group"), "root:x:0:\nnixbld:!:100:\nnogroup:x:65534:\n")?;
        fs::write(etc.join("hosts"), "127.0.0.1 localhost\n::1 localhost\n")?;
        for (name, data) in &job.files {
            ensure!(!name.is_empty() && !name.contains('/') && name != "." && name != "..", "bad file name {name:?}");
            fs::write(build_dir.join(name), data)?;
        }
        let mut tmp_outs = BTreeMap::new();
        let staging = self.store.staging_dir()?;
        let stage = staging.join(format!(".nami-stage-{}", base_name(&first)));
        if stage.symlink_metadata().is_ok() {
            remove_tree(&stage)?;
        }
        if nix_outputs {
            fs::create_dir(&stage)?;
        }
        for (o, path) in outputs {
            if nix_outputs {
                tmp_outs.insert(o.clone(), stage.join(base_name(path)));
                continue;
            }
            let tmp = staging.join(format!(".nami-build-{}", base_name(path)));
            if tmp.symlink_metadata().is_ok() {
                remove_tree(&tmp)?;
            }
            fs::create_dir(&tmp)?;
            tmp_outs.insert(o.clone(), tmp);
        }

        let mut cmd = Command::new(self.store.phys(&format!("{bwrap}/bin/bwrap")));
        if fixed.is_some() {
            // A download: everything but the network is private, as in Nix.
            cmd.args(["--unshare-user", "--unshare-ipc", "--unshare-pid", "--unshare-uts", "--unshare-cgroup-try"]);
        } else {
            cmd.arg("--unshare-all");
        }
        cmd.args(["--die-with-parent", "--uid", "1000", "--gid", "100"]);
        cmd.args(["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"]);
        if nix_outputs {
            cmd.arg("--bind").arg(&stage).arg(STORE_DIR);
        } else {
            cmd.args(["--dir", STORE_DIR]);
        }
        for base in &closure {
            let logical = format!("{STORE_DIR}/{base}");
            cmd.arg("--ro-bind").arg(self.store.phys(&logical)).arg(&logical);
        }
        for (o, tmp) in tmp_outs.iter().filter(|_| !nix_outputs) {
            cmd.arg("--bind").arg(tmp).arg(&outputs[o]);
        }
        cmd.arg("--bind").arg(&build_dir).arg("/build");
        for f in ["passwd", "group", "hosts"] {
            cmd.arg("--ro-bind").arg(etc.join(f)).arg(format!("/etc/{f}"));
        }
        if fixed.is_some() {
            for f in ["/etc/resolv.conf", "/etc/services", "/etc/nsswitch.conf"] {
                cmd.arg("--ro-bind-try").arg(f).arg(f);
            }
        }
        // /bin/sh, as Nix provides it: the (non-interactive) bash from the inputs.
        let is_bash = |b: &&String| b.get(33..).and_then(|n| n.strip_prefix("bash-")).is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()));
        if let Some(shell) = closure.iter().find(is_bash) {
            cmd.args(["--dir", "/bin", "--symlink"]).arg(format!("{STORE_DIR}/{shell}/bin/bash")).arg("/bin/sh");
        }
        cmd.args(["--chdir", "/build", "--clearenv"]);
        for (k, v) in &job.env {
            cmd.arg("--setenv").arg(k).arg(v);
        }
        cmd.args(&job.command);

        let log_path = self.store.var_dir().join("logs").join(format!("{}.log", base_name(&first)));
        fs::create_dir_all(log_path.parent().unwrap())?;
        let (status, tail) = run_logged(cmd, &log_path)?;
        let produced = if nix_outputs {
            tmp_outs.values().all(|t| t.symlink_metadata().is_ok())
        } else {
            tmp_outs.values().any(|t| t.read_dir().map(|mut d| d.next().is_some()).unwrap_or(false))
        };
        if !(status.success() && produced) {
            // What went wrong, in plain words, when the log says something
            // nami knows (build_errors.rs); else its last lines.
            let explained = (!status.success()).then(|| tail_of(&log_path, 4 << 20)).flatten().and_then(|log| crate::build_errors::explain(self.store, self.index, &log));
            let shown = match &explained {
                Some(e) => e.lines.clone(),
                None => tail.clone(),
            };
            for line in &shown {
                crate::say!("  | {line}");
            }
            if self.keep_failed {
                crate::say!("build directory kept at {}", work.display());
            } else {
                remove_tree(&work)?;
            }
            for t in tmp_outs.values() {
                let _ = remove_tree(t);
            }
            let _ = remove_tree(&stage);
            if status.success() {
                bail!("build succeeded but produced nothing (full log: {})", log_path.display());
            }
            if let Some(e) = explained {
                bail!("{} didn't build: {}\nfull log: {}", job.name, e.problems.join("\n  and "), log_path.display());
            }
            bail!("build failed ({status}); full log: {}", log_path.display());
        }
        if !self.keep_failed {
            remove_tree(&work)?;
        }

        // References: which inputs (and outputs) does each output mention?
        let mut candidates: Vec<String> = closure.iter().cloned().collect();
        candidates.extend(outputs.values().map(|p| base_name(p).to_string()));
        for (o, tmp) in &tmp_outs {
            canonicalize(tmp)?;
            if let Some((method, want)) = fixed {
                let (algo, expected) = parse_hash(want)?;
                let got = fixed_hash(tmp, method, &algo)?;
                if got != expected {
                    let _ = remove_tree(tmp);
                    bail!("{}: the download doesn't match its hash (expected {want}, got {algo}-{})", job.name, b64(&got));
                }
            }
            let refs = if fixed.is_some() { Vec::new() } else { scan_references(tmp, &candidates)? };
            self.store.commit_path(tmp, &outputs[o], refs)?;
        }
        if nix_outputs {
            remove_tree(&stage)?; // only mount points left
        }
        crate::say!("built {}", outputs.values().cloned().collect::<Vec<_>>().join(" "));
        Ok(())
    }

    /// Make a catalog package present when the binary cache doesn't have it,
    /// by building it from its closure in the catalog. Refuses unfree
    /// packages when `allow_unfree` is off.
    pub fn ensure_catalog(&self, attr: &str) -> Result<()> {
        let pkg = self.index.get(attr)?;
        if pkg.unfree && !self.allow_unfree {
            bail!("{attr} isn't free software, and your file says `unfree: false` (remove that line to allow it)");
        }
        let paths = pkg.install_paths();
        if paths.iter().all(|p| self.store.is_valid(base_name(p))) {
            return Ok(());
        }
        // A Mac has what's ready-built for it, nothing built here (yet).
        if crate::index::MAC && pkg.cache.is_none() && pkg.uncached {
            bail!("{attr} isn't ready-built for Mac: cache.nixos.org has no {} build of it, and nami doesn't build packages on a Mac yet", crate::index::SYSTEM);
        }
        // ARM Linux's catalog has no build definitions yet: what the cache hasn't got, nami
        // can't build from it (a recipe of your own still builds here).
        if crate::index::ARM_LINUX && pkg.cache.is_none() && pkg.uncached && pkg.closure.is_none() {
            bail!("{attr} isn't ready-built for ARM Linux: cache.nixos.org has no {} build of it, and nami can't build it from the ARM catalog yet", crate::index::SYSTEM);
        }
        if pkg.closure.is_none() {
            return Ok(());
        }
        if crate::index::MAC {
            bail!("{attr} isn't ready-built for Mac, and nami doesn't build packages on a Mac yet");
        }
        if let Some(socket) = self.store.daemon() {
            return crate::daemon::client::build_catalog(socket, attr);
        }
        self.build_catalog(attr)
    }

    /// Build a package (and whatever of its dependencies the cache lacks)
    /// from the catalog's closure, at exactly the paths nixpkgs gives them.
    /// Only catalog closures are built this way, never user input: the
    /// signed index vouches for each one.
    pub fn build_catalog(&self, attr: &str) -> Result<()> {
        let pkg = self.index.get(attr)?;
        let want = pkg.closure.clone().ok_or_else(|| anyhow!("the catalog has no build closure for {attr}"))?;
        let text = self.definition(&format!("{attr}.closure.json"), Some(&want))?;
        let c = self.closure_from(&text, attr)?;
        self.build_closure(&c, attr)
    }

    /// Build a closure's derivations the cache doesn't have, at the paths
    /// they give (`what` names it in errors). Only from the signed catalog,
    /// or as root (a flake: flake.rs).
    pub(crate) fn build_closure(&self, c: &Closure, what: &str) -> Result<()> {
        // Plain files (patches, scripts) the cache doesn't have come inside it.
        for (base, nar_b64) in &c.srcs {
            ensure!(valid_base(base), "bad source name {base:?}");
            if self.store.is_valid(base) {
                continue;
            }
            use base64::Engine;
            let nar = base64::engine::general_purpose::STANDARD.decode(nar_b64)?;
            let tmp = self.store.staging_dir()?.join(format!(".nami-src-{base}"));
            if tmp.symlink_metadata().is_ok() {
                remove_tree(&tmp)?;
            }
            crate::nar::unpack(&mut &nar[..], &tmp)?;
            self.store.commit_path(&tmp, &format!("{STORE_DIR}/{base}"), Vec::new())?;
        }

        // Dependencies first.
        let mut order = Vec::new();
        let mut seen = std::collections::HashSet::new();
        fn visit(d: &str, c: &Closure, seen: &mut std::collections::HashSet<String>, order: &mut Vec<String>) {
            if !seen.insert(d.to_string()) {
                return;
            }
            if let Some(drv) = c.drvs.get(d) {
                for dep in drv["inputs"]["drvs"].as_object().into_iter().flatten().map(|(k, _)| k) {
                    visit(dep, c, seen, order);
                }
                order.push(d.to_string());
            }
        }
        visit(&c.top, c, &mut seen, &mut order);

        let paths_of = |d: &str| -> Result<BTreeMap<String, String>> {
            match (c.drvs.get(d), c.cached.get(d)) {
                (Some(drv), _) => drv_outputs(drv),
                (None, Some(outs)) => Ok(outs.iter().map(|(o, p)| (o.clone(), format!("{STORE_DIR}/{p}"))).collect()),
                _ => bail!("{what}'s build closure doesn't say what {d} is"),
            }
        };
        for d in &order {
            let drv = &c.drvs[d];
            let outputs = drv_outputs(drv)?;
            let outs: Vec<String> = outputs.values().cloned().collect();
            if outs.iter().all(|p| self.store.is_valid(base_name(p))) {
                continue;
            }
            if self.store.realise(&outs).is_ok() {
                continue; // the cache has it after all
            }
            let mut inputs: Vec<String> = drv["inputs"]["srcs"].as_array().into_iter().flatten().filter_map(|s| s.as_str()).map(|s| format!("{STORE_DIR}/{s}")).collect();
            for (dep, uses) in drv["inputs"]["drvs"].as_object().into_iter().flatten() {
                let dep_outs = paths_of(dep)?;
                for o in uses["outputs"].as_array().into_iter().flatten().filter_map(|o| o.as_str()) {
                    inputs.push(dep_outs.get(o).cloned().ok_or_else(|| anyhow!("{dep} has no output {o}"))?);
                }
            }
            let missing: Vec<String> = inputs.iter().filter(|p| !self.store.is_valid(base_name(p))).cloned().collect();
            self.store.realise(&missing)?;
            let mut job = drv_job(drv, &outputs)?;
            job.inputs = inputs;
            self.export_reference_graphs(drv, &mut job)?;
            let job = job.materialize(&outputs); // placeholders -> output paths, as Nix does at build time
            let fixed = drv["outputs"].as_object().and_then(|o| o.values().next()).and_then(|o| Some((o["method"].as_str().unwrap_or("flat"), o.get("hash")?.as_str()?)));
            self.sandbox(&job, &outputs, fixed, true)?;
        }
        Ok(())
    }

    fn resolve_inputs(&self, refs: &[String], dir: &Path) -> Result<Vec<String>> {
        refs.iter()
            .map(|r| {
                if let Some(notice) = old_output_spelling(r) {
                    crate::say!("{}: {notice}", dir.display());
                }
                if Self::is_recipe(r) {
                    Ok(self.build(&dir.join(r))?.dep_path())
                } else if let (attr, Some(output)) = split_output(&self.index, r) {
                    // `libgcc.lib` picks one output (see split_output).
                    self.output(self.index.get(attr)?, output)
                } else {
                    // Like nixpkgs' mkDerivation: use the dev output if there is
                    // one; its propagated inputs pull in the rest.
                    let pkg = self.index.get(r)?;
                    let out = if pkg.outputs.contains_key("dev") { "dev" } else if pkg.outputs.contains_key("out") { "out" } else { "bin" };
                    self.output(pkg, out)
                }
            })
            .collect()
    }

    /// Like `resolve_inputs`, but for what a program loads at runtime: the
    /// `lib` output if there is one (that's where the .so files are).
    fn resolve_runtime(&self, refs: &[String], dir: &Path) -> Result<Vec<String>> {
        refs.iter()
            .map(|r| {
                if Self::is_recipe(r) || split_output(&self.index, r).1.is_some() {
                    return Ok(self.resolve_inputs(std::slice::from_ref(r), dir)?.remove(0));
                }
                let pkg = self.index.get(r)?;
                let out = ["lib", "out"].into_iter().find(|o| pkg.outputs.contains_key(*o)).unwrap_or("out");
                self.output(pkg, out)
            })
            .collect()
    }

    fn add_patches(&self, patches: &[String], dir: &Path) -> Result<Vec<String>> {
        patches
            .iter()
            .map(|p| {
                let path = dir.join(p);
                let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| anyhow!("bad patch path {p}"))?;
                self.store.add_path(&path, &sanitize(name))
            })
            .collect()
    }

    fn output(&self, pkg: &crate::index::Package, output: &str) -> Result<String> {
        pkg.outputs
            .get(output)
            .cloned()
            .flatten()
            .ok_or_else(|| anyhow!("{} has no '{output}' output", pkg.name))
    }

    /// `exportReferencesGraph`: the build gets the closure of some paths
    /// (buildFHSEnv uses it). With structured attrs it goes into
    /// .attrs.json as path-info objects; otherwise into a file in /build,
    /// in Nix's line format.
    fn export_reference_graphs(&self, drv: &serde_json::Value, job: &mut Job) -> Result<()> {
        let path_info = |base: &str| -> Result<serde_json::Value> {
            let info = self.store.path_info(base)?;
            Ok(serde_json::json!({
                "path": format!("{STORE_DIR}/{base}"),
                "narHash": info.nar_hash,
                "narSize": info.nar_size,
                "references": info.references.iter().map(|r| format!("{STORE_DIR}/{r}")).collect::<Vec<_>>(),
                "valid": true,
            }))
        };
        if let Some(graphs) = drv["structuredAttrs"]["exportReferencesGraph"].as_object() {
            let mut attrs: serde_json::Value = serde_json::from_str(&job.files.iter().find(|(n, _)| n == ".attrs.json").map(|(_, d)| d.clone()).unwrap_or_default())?;
            for (name, roots) in graphs {
                let roots: Vec<String> = roots.as_array().into_iter().flatten().filter_map(|r| r.as_str().map(String::from)).collect();
                let closure = self.store.closure(&roots)?;
                attrs[name] = serde_json::Value::Array(closure.iter().map(|b| path_info(b)).collect::<Result<_>>()?);
            }
            for (n, d) in job.files.iter_mut() {
                if n == ".attrs.json" {
                    *d = serde_json::to_string(&attrs)?;
                } else if n == ".attrs.sh" {
                    *d = attrs_sh(&attrs);
                }
            }
        } else if let Some(spec) = drv["env"]["exportReferencesGraph"].as_str() {
            let words: Vec<&str> = spec.split_whitespace().collect();
            for pair in words.chunks(2) {
                let [name, root] = pair else { continue };
                ensure!(!name.contains('/') && !name.starts_with('.'), "bad reference graph name {name:?}");
                let closure = self.store.closure(&[root.to_string()])?;
                let mut text = String::new();
                for b in &closure {
                    let refs = self.store.path_info(b)?.references;
                    text += &format!("{STORE_DIR}/{b}\n\n{}\n", refs.len());
                    for r in refs {
                        text += &format!("{STORE_DIR}/{r}\n");
                    }
                }
                job.files.push((name.to_string(), text));
            }
        }
        Ok(())
    }

    /// The build definition of a nixpkgs attr, from the index's drvs.
    fn load_drv(&self, attr: &str) -> Result<(String, serde_json::Value)> {
        let want = self.index.get(attr).ok().and_then(|p| p.drv.clone());
        let text = self.definition(&format!("{attr}.json"), want.as_deref())?;
        let v: serde_json::Value = serde_json::from_str(&text)?;
        let (key, drv) = v.as_object().and_then(|m| m.iter().next()).ok_or_else(|| anyhow!("bad build definition for {attr}"))?;
        Ok((key.clone(), drv.clone()))
    }

    /// A file from the catalog's build definitions (`nami update --drvs`),
    /// cached, and refused unless its sha256 is the one the signed index lists.
    fn definition(&self, file: &str, want: Option<&str>) -> Result<String> {
        ensure!(!file.contains('/') && !file.starts_with('.'), "bad definition name {file:?}");
        let attr = file.trim_end_matches(".json").trim_end_matches(".closure");
        let cache = self.store.cache_dir().join("drvs");
        let cached = cache.join(file);
        // A copy cached for an older catalog doesn't count: fetch it again.
        let fresh = fs::read_to_string(&cached).ok().filter(|t| want.is_none_or(|w| hex(&Sha256::digest(t.as_bytes())) == w));
        let text = match fresh {
            Some(t) => t,
            None => {
                let source = self.definitions_source()?;
                // This catalog's are in shards, by sha256 (index/defs.py); an
                // older catalog's, and a closure in one piece, files in drvs/.
                let pooled = match want {
                    Some(w) => self.pooled(&source, w, &mut HashMap::new())?,
                    None => None,
                };
                let text = match pooled {
                    Some(t) => t,
                    None => {
                        let data = self.catalog_file(&source, file)?.ok_or_else(|| anyhow!("no build definition for {attr:?} in {source}"))?;
                        String::from_utf8(data).map_err(|_| anyhow!("the build definition for {attr} isn't text"))?
                    }
                };
                fs::create_dir_all(&cache)?;
                fs::write(&cached, &text)?;
                text
            }
        };
        // The signed index carries each definition's hash: refuse anything else.
        if let Some(want) = want {
            let got = hex(&Sha256::digest(text.as_bytes()));
            if got != want {
                let _ = fs::remove_file(&cached);
                bail!("the build definition for {attr} doesn't match the catalog (sha256 {got}, expected {want}); it was changed or is from another catalog");
            }
        }
        Ok(text)
    }

    /// Where the catalog's build definitions are (drvs/, its shards in
    /// defs/ next to it).
    fn definitions_source(&self) -> Result<String> {
        let source = fs::read_to_string(self.store.var_dir().join("drvs-source"))
            .map_err(|_| anyhow!("no build definitions available; run `nami update --from <index> --drvs <dir-or-url>`"))?;
        Ok(source.trim().trim_end_matches('/').to_string())
    }

    /// A file from the catalog's folder `dir` (a URL or a path); None if it isn't there.
    fn catalog_file(&self, dir: &str, name: &str) -> Result<Option<Vec<u8>>> {
        if dir.starts_with("http://") || dir.starts_with("https://") {
            let mut data = Vec::new();
            match self.store.http().get(&format!("{dir}/{}", crate::index::url_encode(name))).call() {
                Ok(r) => r.into_reader().read_to_end(&mut data)?,
                Err(ureq::Error::Status(404, _)) => return Ok(None),
                Err(e) => return Err(e.into()),
            };
            Ok(Some(data))
        } else {
            Ok(fs::read(Path::new(dir).join(name)).ok())
        }
    }

    /// A text from the catalog's shards (defs/<first 3 hex of its
    /// sha256>.json.xz, {sha256: text}), checked against its sha256. None
    /// when the catalog has no shards (an older one), or not this text.
    /// `shards` keeps the ones fetched, for the next text.
    fn pooled(&self, source: &str, sha: &str, shards: &mut HashMap<String, BTreeMap<String, String>>) -> Result<Option<String>> {
        let Some(dir) = source.strip_suffix("/drvs") else { return Ok(None) };
        ensure!(sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()), "bad sha256 {sha:?} in the catalog");
        let key = &sha[..3];
        if !shards.contains_key(key) {
            let held = match self.catalog_file(&format!("{dir}/defs"), &format!("{key}.json.xz"))? {
                Some(xz) => {
                    let mut data = Vec::new();
                    xz2::read::XzDecoder::new(&xz[..]).read_to_end(&mut data).context("reading the catalog's build definitions")?;
                    serde_json::from_slice(&data).context("reading the catalog's build definitions")?
                }
                None => BTreeMap::new(),
            };
            shards.insert(key.to_string(), held);
        }
        let Some(text) = shards[key].get(sha) else { return Ok(None) };
        let got = hex(&Sha256::digest(text.as_bytes()));
        ensure!(got == sha, "a build definition in the catalog doesn't match it (sha256 {got}, expected {sha}); it was changed or is from another catalog");
        Ok(Some(text.clone()))
    }

    /// A build closure as the catalog has it: in one piece, or in parts that
    /// a set's packages share (index/closures.py --parts), put together here,
    /// each part checked against the sha256 the closure lists for it.
    fn closure_from(&self, text: &str, attr: &str) -> Result<Closure> {
        #[derive(Deserialize)]
        struct InParts {
            top: String,
            cached: BTreeMap<String, BTreeMap<String, String>>,
            drvs_by_sha256: BTreeMap<String, String>,
            srcs_by_sha256: BTreeMap<String, String>,
        }
        let v: serde_json::Value = serde_json::from_str(text).with_context(|| format!("reading the build closure of {attr}"))?;
        if v.get("drvs_by_sha256").is_none() {
            return serde_json::from_value(v).with_context(|| format!("reading the build closure of {attr}"));
        }
        let p: InParts = serde_json::from_value(v).with_context(|| format!("reading the build closure of {attr}"))?;
        let source = self.definitions_source()?;
        let mut shards = HashMap::new();
        let mut part = |sha: &str| self.pooled(&source, sha, &mut shards)?.ok_or_else(|| anyhow!("the catalog is missing a part of the build closure of {attr} ({sha})"));
        let mut drvs = BTreeMap::new();
        for (d, sha) in &p.drvs_by_sha256 {
            drvs.insert(d.clone(), serde_json::from_str(&part(sha)?).with_context(|| format!("reading the build closure of {attr}"))?);
        }
        let mut srcs = BTreeMap::new();
        for (path, sha) in &p.srcs_by_sha256 {
            srcs.insert(path.clone(), part(sha)?);
        }
        Ok(Closure { top: p.top, drvs, cached: p.cached, srcs })
    }

    fn fetch_source(&self, s: &Source, dir: &Path, name: &str, version: &str) -> Result<String> {
        ensure!(s.kind.as_deref().is_none_or(|k| k == "deb"), "source.type: only `deb` is known");
        self.fetch_file(s, dir, name, version)
    }

    /// Debian packages' files (the sources', then the libraries of the
    /// bundled ones), laid out like a nixpkgs package, as one store path:
    /// what the build starts from.
    fn unpack_debs(&self, sources: &[Source], bundle: &[Source], dir: &Path, name: &str, version: &str) -> Result<String> {
        ensure!(bundle.iter().all(Source::is_deb), "bundle: only Debian packages");
        let mut debs = Vec::new();
        for (s, only_libs) in sources.iter().map(|s| (s, false)).chain(bundle.iter().map(|s| (s, true))) {
            let path = self.fetch_source(s, dir, name, version)?;
            let data = fs::read(self.store.phys(&path)).with_context(|| format!("reading {path}"))?;
            debs.push((path, data, only_libs));
        }
        let mut key = Sha256::new();
        for (_, data, only_libs) in &debs {
            key.update(Sha256::digest(data));
            key.update([*only_libs as u8]);
        }
        let cache = self.store.cache_dir().join("sources");
        fs::create_dir_all(&cache)?;
        let out_name = sanitize(&format!("{name}-{version}"));
        let marker = cache.join(format!("{}-{out_name}-deb-v{DEB_LAYOUT}", hex(&key.finalize())));
        if let Ok(path) = fs::read_to_string(&marker) {
            if self.store.is_valid(base_name(&path)) {
                return Ok(path);
            }
        }
        let names: Vec<&str> = debs.iter().map(|(p, _, _)| base_name(p)).collect();
        crate::say!("unpacking {}", names.join(", "));
        let tmp = cache.join(format!(".unpack-{}", std::process::id()));
        if tmp.symlink_metadata().is_ok() {
            remove_tree(&tmp)?;
        }
        let parts: Vec<(&[u8], bool)> = debs.iter().map(|(_, d, l)| (&d[..], *l)).collect();
        let result = crate::deb::unpack(&parts, &tmp).and_then(|_| self.store.add_path(&tmp, &out_name));
        remove_tree(&tmp)?;
        let path = result.with_context(|| format!("unpacking {}", names.join(", ")))?;
        fs::write(&marker, &path)?;
        Ok(path)
    }

    /// The kind of project a recipe's source `src` is, and what that adds.
    fn plan(&self, recipe: &Recipe, dir: &Path, src: &str) -> Result<Option<crate::build_kinds::Plan>> {
        let tree = crate::archive::peek(&self.store.phys(src), &crate::build_kinds::wanted).with_context(|| format!("looking into {}", base_name(src)))?;
        let kind = match &recipe.build_system {
            Some(k) => crate::build_kinds::Kind::parse(k)?,
            None => match crate::build_kinds::named_by_build_deps(recipe).or_else(|| crate::build_kinds::detect(&tree)) {
                Some(k) => k,
                None => return Ok(None),
            },
        };
        let mut pins = crate::lock::Pins::load(dir)?;
        let plan = crate::build_kinds::plan(self.store, self.index, recipe, kind, &tree, &mut pins);
        if pins.save().unwrap_or_else(|e| {
            crate::say!("couldn't record what it fetched in {}: {e:#}", pins.path().display());
            false
        }) {
            crate::say!("recorded what it fetched in {}", pins.path().display());
        }
        let plan = plan.with_context(|| format!("a {} project", kind.name()))?;
        crate::say!("{}: a {} project", recipe.name, kind.name());
        Ok(Some(plan))
    }

    fn fetch_file(&self, s: &Source, dir: &Path, name: &str, version: &str) -> Result<String> {
        match (&s.url, &s.path) {
            (Some(url), None) => {
                let file_name = s.name.as_deref().unwrap_or_else(|| url_file_name(url));
                // Its sha256: the recipe's, else the one its nami.lock recorded, else
                // (the first time) none yet: what's fetched is recorded.
                let mut pins = None;
                let want = match s.sha256.as_deref() {
                    Some(h) => Some(parse_sha256(h)?),
                    None => {
                        let p = crate::lock::Pins::load(dir)?;
                        let h = p.get(url).map(parse_sha256).transpose()?;
                        pins = Some(p);
                        h
                    }
                };
                match fetch_url(self.store, url, file_name, want.as_ref())? {
                    Fetched::Ok { path, sha256 } => {
                        if let Some(mut p) = pins.filter(|_| want.is_none()) {
                            p.set(url, &hex(&sha256));
                            match p.save() {
                                Ok(_) => crate::say!("recorded {url}'s sha256 in {}", p.path().display()),
                                Err(e) => crate::say!("couldn't record {url}'s sha256 in {}: {e:#}", p.path().display()),
                            }
                        }
                        Ok(path)
                    }
                    Fetched::Mismatch { got } => {
                        let want = hex(&want.unwrap_or_default());
                        match pins {
                            Some(p) => bail!("{}", crate::build_errors::changed_upstream(url, &want, &hex(&got), p.path())),
                            None => bail!("{}", crate::build_errors::wrong_hash_in_recipe(url, &want, &hex(&got), dir)),
                        }
                    }
                }
            }
            (None, Some(_)) if s.name.is_some() => bail!("source.name only applies to source.url"),
            (None, Some(p)) => {
                self.store.add_path(&dir.join(p), &format!("{name}-{version}-source"))
            }
            _ => bail!("source needs exactly one of `url` or `path`"),
        }
    }
}

/// What [`fetch_url`] got.
pub(crate) enum Fetched {
    /// The file, in the store, and its sha256.
    Ok { path: String, sha256: [u8; 32] },
    /// Not the file asked for: its sha256 (nothing was kept).
    Mismatch { got: [u8; 32] },
}

/// A source file from `url`, into the store as `file_name`: checked
/// against `want` when there's one. Sources are kept by their sha256, so a
/// file fetched before isn't fetched again.
pub(crate) fn fetch_url(store: &Store, url: &str, file_name: &str, want: Option<&[u8; 32]>) -> Result<Fetched> {
    let name = sanitize(file_name);
    let cache = store.cache_dir().join("sources");
    fs::create_dir_all(&cache)?;
    if let Some(want) = want {
        let marker = cache.join(format!("{}-{name}", hex(want)));
        if let Ok(path) = fs::read_to_string(&marker) {
            if store.is_valid(base_name(&path)) {
                return Ok(Fetched::Ok { path, sha256: *want });
            }
        }
    }
    crate::say!("fetching {url}");
    let tmp = cache.join(format!(".download-{}-{}", std::process::id(), &hex(&Sha256::digest(url.as_bytes()))[..8]));
    // A network that blinks gets two more tries (an answer, like 404, is the answer).
    let mut tries = 0;
    let response = loop {
        match store.http().get(url).call() {
            Err(ureq::Error::Transport(e)) if tries < 2 => {
                tries += 1;
                crate::say!("fetching {url}: {e}; again in {}s", 2 * tries);
                std::thread::sleep(std::time::Duration::from_secs(2 * tries));
            }
            r => break r.with_context(|| format!("fetching {url}"))?,
        }
    };
    let mut reader = response.into_reader();
    let mut f = File::create(&tmp)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        f.write_all(&buf[..n])?;
    }
    drop(f);
    let got: [u8; 32] = hasher.finalize().into();
    if want.is_some_and(|w| *w != got) {
        fs::remove_file(&tmp)?;
        return Ok(Fetched::Mismatch { got });
    }
    let path = store.add_path(&tmp, &name)?;
    fs::remove_file(&tmp)?;
    fs::write(cache.join(format!("{}-{name}", hex(&got))), &path)?;
    Ok(Fetched::Ok { path, sha256: got })
}

/// The file a URL is: its last part (SourceForge's `<file>/download` is
/// <file>), without a query; what stdenv unpacks it by.
pub(crate) fn url_file_name(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or(url).trim_end_matches('/');
    let path = path.strip_suffix("/download").unwrap_or(path);
    path.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("source")
}

/// Whether a recipe's source is Debian packages (nothing to work out).
fn is_deb_recipe(recipe: &Recipe) -> bool {
    recipe.source.as_ref().is_some_and(|s| s.list().iter().all(Source::is_deb))
}

/// The local files a recipe is built from that live outside its folder
/// (`source: path: ../../work/pkg`, a patch from a shared folder), into
/// `h` as the store gets them (their NAR): a changed file there is another
/// source path, so another build, not the remembered one. Files in the
/// folder are hashed with it already; nothing is added for a recipe that
/// has none outside, so its key stays what it was.
fn outside_files_key(recipe: &Recipe, dir: &Path, h: &mut Sha256) -> Result<()> {
    let sources = recipe.source.as_ref().map(Sources::list).unwrap_or_default();
    let paths = sources.iter().chain(&recipe.bundle).filter_map(|s| s.path.as_deref()).chain(recipe.patches.iter().map(String::as_str));
    for p in paths {
        // Not there: the build says so when it gets to it.
        let Ok(full) = dir.join(p).canonicalize() else { continue };
        if full.starts_with(dir) {
            continue;
        }
        h.update(b"outside\0");
        h.update(p.as_bytes());
        h.update([0]);
        let mut nar = Sha256::new();
        crate::nar::dump(&dir.join(p), &mut nar).with_context(|| format!("reading {}", dir.join(p).display()))?;
        h.update(nar.finalize());
    }
    Ok(())
}

/// A catalog build closure (see index/closures.py).
#[derive(Deserialize, Default)]
pub(crate) struct Closure {
    pub top: String,
    /// Derivations to build: name -> derivation JSON (Nix's format, version 4).
    pub drvs: BTreeMap<String, serde_json::Value>,
    /// Inputs the binary cache has: derivation -> output -> path.
    pub cached: BTreeMap<String, BTreeMap<String, String>>,
    /// Plain files the cache doesn't have: path -> base64 NAR.
    pub srcs: BTreeMap<String, String>,
}

/// A derivation's outputs: name -> store path. A download's path isn't
/// listed with its output, but it's in its environment.
pub(crate) fn drv_outputs(drv: &serde_json::Value) -> Result<BTreeMap<String, String>> {
    drv["outputs"]
        .as_object()
        .ok_or_else(|| anyhow!("derivation without outputs"))?
        .iter()
        .map(|(o, v)| {
            let p = v["path"].as_str().map(|p| format!("{STORE_DIR}/{}", p.trim_start_matches(&format!("{STORE_DIR}/"))))
                .or_else(|| drv["env"][o].as_str().map(String::from))
                .or_else(|| drv["structuredAttrs"]["outputs"][o].as_str().map(String::from))
                .ok_or_else(|| anyhow!("output {o} has no path"))?;
            ensure!(p.starts_with(&format!("{STORE_DIR}/")) && valid_base(base_name(&p)), "bad output path {p:?}");
            Ok((o.clone(), p))
        })
        .collect()
}

/// The sandbox job for a nixpkgs derivation, as Nix would run it: its
/// builder and args, its environment, and with structured attrs the
/// `.attrs.json`/`.attrs.sh` files; `passAsFile` attributes become files.
fn drv_job(drv: &serde_json::Value, outputs: &BTreeMap<String, String>) -> Result<Job> {
    let builder = drv["builder"].as_str().unwrap_or_default();
    ensure!(builder.starts_with(&format!("{STORE_DIR}/")), "unsupported builder {builder:?} (Nix's built-in builders aren't supported)");
    let mut command = vec![builder.to_string()];
    command.extend(drv["args"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(String::from)));
    let mut env = nix_env();
    for (k, v) in drv["env"].as_object().into_iter().flatten() {
        if let Some(v) = v.as_str() {
            env.insert(k.clone(), v.to_string());
        }
    }
    let mut files = Vec::new();
    match drv.get("structuredAttrs") {
        Some(serde_json::Value::Object(attrs)) => {
            let mut attrs = attrs.clone();
            attrs.insert("outputs".into(), serde_json::Value::Object(outputs.iter().map(|(o, p)| (o.clone(), serde_json::Value::String(p.clone()))).collect()));
            let attrs = serde_json::Value::Object(attrs);
            env.insert("NIX_ATTRS_JSON_FILE".into(), "/build/.attrs.json".into());
            env.insert("NIX_ATTRS_SH_FILE".into(), "/build/.attrs.sh".into());
            files.push((".attrs.json".into(), serde_json::to_string(&attrs)?));
            files.push((".attrs.sh".into(), attrs_sh(&attrs)));
        }
        _ => {
            let pass: Vec<String> = env.get("passAsFile").map(|s| s.split_whitespace().map(String::from).collect()).unwrap_or_default();
            for (i, name) in pass.iter().enumerate() {
                let value = env.remove(name).unwrap_or_default();
                let file = format!(".attr-{i}");
                env.insert(format!("{name}Path"), format!("/build/{file}"));
                files.push((file, value));
            }
        }
    }
    Ok(Job {
        name: drv["name"].as_str().unwrap_or("build").to_string(),
        outputs: outputs.keys().cloned().collect(),
        command,
        env,
        files,
        inputs: Vec::new(), // set by the caller: sources and input outputs
    })
}

/// `<32 hash chars>-<name>`: a store path's basename.
fn valid_base(b: &str) -> bool {
    b.len() > 33 && b.as_bytes()[32] == b'-' && b[..32].bytes().all(|c| c.is_ascii_alphanumeric()) && valid_store_name(&b[33..])
}

/// The hash a download is checked against: of the file itself (`flat`), or
/// of its NAR serialisation (`nar`, a directory such as an unpacked zip).
fn fixed_hash(path: &Path, method: &str, algo: &str) -> Result<Vec<u8>> {
    let data = match method {
        "flat" => fs::read(path)?,
        "nar" | "recursive" => {
            let mut nar = Vec::new();
            crate::nar::dump(path, &mut nar)?;
            nar
        }
        other => bail!("unsupported download hash method {other:?}"),
    };
    Ok(match algo {
        "sha256" => Sha256::digest(&data).to_vec(),
        "sha512" => sha2::Sha512::digest(&data).to_vec(),
        other => bail!("unsupported download hash {other:?}"),
    })
}

/// `sha256-<base64>`, `sha512-<base64>` (SRI) or 64 hex characters (sha256).
fn parse_hash(s: &str) -> Result<(String, Vec<u8>)> {
    use base64::Engine;
    if let Some((algo, b64)) = s.split_once('-').filter(|(a, _)| a.starts_with("sha")) {
        return Ok((algo.to_string(), base64::engine::general_purpose::STANDARD.decode(b64)?));
    }
    Ok(("sha256".into(), parse_sha256(s)?.to_vec()))
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[allow(clippy::too_many_arguments)]
/// A .deb's fixup, before autoPatchelfHook's: its text files' paths into
/// the package (deb.rs wrote them as @out@), and the paths its programs
/// have built in (`redirects`, from deb::hard_coded) pointed at the
/// package. Those go through nixpkgs' libredirect, as nixpkgs does for
/// prebuilt programs: each program links it (so it changes their file
/// calls, and not the calls of other programs they start), and the
/// programs in bin set NIX_REDIRECTS, the paths it changes. A mount
/// namespace would need user namespaces, which Ubuntu turns off for
/// programs without an AppArmor profile.
fn deb_fixup(redirects: &[String]) -> String {
    let mut s = String::from("{ grep -rlIF @out@ \"$out\" || true; } | while IFS= read -r f; do\n  [ -L \"$f\" ] || sed -i \"s|@out@|$out|g\" \"$f\"\ndone\n");
    if redirects.is_empty() {
        return s;
    }
    let pairs: Vec<String> = redirects.iter().map(|p| format!("{p}=$out/{}", crate::deb::place(p))).collect();
    s += &format!("redirects=\"{}\"\n", pairs.join(":"));
    s += r#"find "$out" -type f | while IFS= read -r f; do
  if [ "$(head -c4 "$f")" = "$(printf '\177ELF')" ] && patchelf --print-interpreter "$f" >/dev/null 2>&1; then
    patchelf --add-needed libredirect.so "$f"
  fi
done
for f in "$out"/bin/*; do
  [ -f "$f" ] && [ -x "$f" ] || continue
  if [ -L "$f" ]; then real=$(readlink -f "$f"); rm "$f"; else real="$out/bin/.${f##*/}-wrapped"; mv "$f" "$real"; fi
  cat > "$f" <<EOF
#!$SHELL
export NIX_REDIRECTS='$redirects'
exec -a "\$0" '$real' "\$@"
EOF
  chmod +x "$f"
done
"#;
    s
}

fn build_env(
    recipe: &Recipe,
    out: &str,
    stdenv: &str,
    src: Option<&str>,
    patches: &[String],
    deps: &[String],
    build_deps: &[String],
) -> Result<BTreeMap<String, String>> {
    let mut env = nix_env();
    // What mkDerivation would derive from the recipe.
    let name = format!("{}-{}", recipe.name, recipe.version);
    env.insert("name".into(), name);
    env.insert("pname".into(), recipe.name.clone());
    env.insert("version".into(), recipe.version.clone());
    env.insert("out".into(), out.into());
    env.insert("outputs".into(), "out".into());
    env.insert("system".into(), SYSTEM.into());
    env.insert("stdenv".into(), stdenv.into());
    env.insert("enableParallelBuilding".into(), "1".into());
    env.insert("buildInputs".into(), deps.join(" "));
    env.insert("nativeBuildInputs".into(), build_deps.join(" "));
    env.insert("patches".into(), patches.join(" "));
    match src {
        Some(src) => {
            env.insert("src".into(), src.into());
        }
        None if recipe.unpack.is_none() => {
            env.insert("dontUnpack".into(), "1".into());
        }
        None => {}
    }
    for (key, flags) in [
        ("configureFlags", &recipe.configure_flags), ("cmakeFlags", &recipe.cmake_flags),
        ("mesonFlags", &recipe.meson_flags), ("makeFlags", &recipe.make_flags),
    ] {
        if !flags.is_empty() {
            env.insert(key.into(), flags.join(" "));
        }
    }
    for (key, phase) in [
        ("unpackPhase", &recipe.unpack), ("configurePhase", &recipe.configure), ("buildPhase", &recipe.build),
        ("checkPhase", &recipe.check), ("installPhase", &recipe.install),
    ] {
        if let Some(script) = phase {
            env.insert(key.into(), script.clone());
        }
    }
    if recipe.check.is_some() {
        env.insert("doCheck".into(), "1".into());
    }
    for (k, v) in &recipe.build_env {
        ensure!(!env.contains_key(k), "build-env: {k} is set by nami and can't be overridden");
        env.insert(k.clone(), v.clone());
    }
    Ok(env)
}

/// What Nix itself puts in every build environment.
fn nix_env() -> BTreeMap<String, String> {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).to_string();
    [
        ("NIX_BUILD_TOP", "/build"), ("TMPDIR", "/build"), ("TEMPDIR", "/build"), ("TMP", "/build"),
        ("TEMP", "/build"), ("HOME", "/homeless-shelter"), ("PATH", "/path-not-set"),
        ("NIX_STORE", STORE_DIR), ("TERM", "xterm-256color"),
        ("SOURCE_DATE_EPOCH", "315532800"), ("NIX_BUILD_CORES", cores.as_str()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    // NAMI_BUILD_DEBUG=1: stdenv traces every command into the build log.
    .chain(std::env::var("NAMI_BUILD_DEBUG").ok().filter(|v| !v.is_empty() && v != "0").map(|_| ("NIX_DEBUG".to_string(), "7".to_string())))
    .collect()
}

/// Nix's placeholder for an output, `/<nixbase32(sha256("nix-output:<name>"))>`.
fn placeholder(output: &str) -> String {
    format!("/{}", crate::nix::base32::encode(&Sha256::digest(format!("nix-output:{output}").as_bytes())))
}

fn rewrite_strings(v: &mut serde_json::Value, rewrites: &[(String, String)]) {
    match v {
        serde_json::Value::String(s) => {
            for (from, to) in rewrites {
                if s.contains(from.as_str()) {
                    *s = s.replace(from.as_str(), to);
                }
            }
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(|x| rewrite_strings(x, rewrites)),
        serde_json::Value::Object(o) => o.values_mut().for_each(|x| rewrite_strings(x, rewrites)),
        _ => {}
    }
}

fn collect_store_paths(v: &serde_json::Value, out: &mut std::collections::BTreeSet<String>) {
    match v {
        serde_json::Value::String(s) => find_store_paths(s, out),
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_store_paths(x, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| collect_store_paths(x, out)),
        _ => {}
    }
}

/// Basenames of `/nix/store/<32 hash chars>-<name>` mentions in `s`.
fn find_store_paths(s: &str, out: &mut std::collections::BTreeSet<String>) {
    let prefix = format!("{STORE_DIR}/");
    let mut rest = s;
    while let Some(i) = rest.find(&prefix) {
        rest = &rest[i + prefix.len()..];
        let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || "+-._?=".contains(c))).unwrap_or(rest.len());
        let base = &rest[..end];
        if base.len() > 33 && base.as_bytes()[32] == b'-' && base[..32].bytes().all(|b| b.is_ascii_alphanumeric()) {
            out.insert(base.to_string());
        }
        rest = &rest[end..];
    }
}

/// `.attrs.sh` exactly as Nix writes it: sorted `declare`s for strings,
/// numbers, booleans, and flat lists/maps of those; anything nested is left out.
fn attrs_sh(attrs: &serde_json::Value) -> String {
    use serde_json::Value as V;
    fn quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
    fn simple(v: &V) -> Option<String> {
        match v {
            V::String(s) => Some(quote(s)),
            V::Number(n) => Some(n.to_string()),
            V::Bool(true) => Some("1".into()),
            V::Bool(false) => Some(String::new()),
            V::Null => Some("''".into()),
            _ => None,
        }
    }
    let is_var = |k: &str| k.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let mut out = String::new();
    for (k, v) in attrs.as_object().into_iter().flatten() {
        if !is_var(k) {
            continue;
        }
        match v {
            V::Array(items) => {
                if let Some(items) = items.iter().map(simple).collect::<Option<Vec<_>>>() {
                    out += &format!("declare -a {k}=({})\n", items.iter().map(|i| format!("{i} ")).collect::<String>());
                }
            }
            V::Object(map) => {
                if let Some(items) = map.iter().map(|(mk, mv)| Some(format!("[{}]={} ", quote(mk), simple(mv)?))).collect::<Option<Vec<_>>>() {
                    out += &format!("declare -A {k}=({})\n", items.concat());
                }
            }
            other => {
                if let Some(s) = simple(other) {
                    out += &format!("declare {k}={s}\n");
                }
            }
        }
    }
    out
}

/// Run `cmd`, writing its output to `log_path`; returns the exit status and
/// the last lines, for error messages.
fn run_logged(mut cmd: Command, log_path: &Path) -> Result<(std::process::ExitStatus, Vec<String>)> {
    let log = Arc::new(Mutex::new(File::create(log_path)?));
    let tail = Arc::new(Mutex::new(std::collections::VecDeque::<String>::new()));
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .context("starting the build sandbox")?;
    let pipes: Vec<Box<dyn Read + Send>> = vec![Box::new(child.stdout.take().unwrap()), Box::new(child.stderr.take().unwrap())];
    let threads: Vec<_> = pipes
        .into_iter()
        .map(|pipe| {
            let (log, tail) = (log.clone(), tail.clone());
            std::thread::spawn(move || {
                // Bytes, not UTF-8 lines: builds print binary now and then
                // (gzip with no files writes its header to stdout), and a
                // reader that stopped there would kill the build with SIGPIPE.
                let mut reader = BufReader::new(pipe);
                let mut buf = Vec::new();
                while reader.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
                    let _ = log.lock().unwrap().write_all(&buf);
                    let line = String::from_utf8_lossy(&buf).trim_end_matches('\n').to_string();
                    let mut t = tail.lock().unwrap();
                    t.push_back(line);
                    if t.len() > 25 {
                        t.pop_front();
                    }
                    buf.clear();
                }
            })
        })
        .collect();
    let status = child.wait()?;
    for t in threads {
        let _ = t.join();
    }
    let tail = tail.lock().unwrap().iter().cloned().collect();
    Ok((status, tail))
}

/// The last `max` bytes of a file, as text (a failed build's log).
fn tail_of(path: &Path, max: u64) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(max))).ok()?;
    let mut data = Vec::new();
    f.read_to_end(&mut data).ok()?;
    Some(String::from_utf8_lossy(&data).into_owned())
}

/// stdenv's setup starts with `export SHELL=/nix/store/...-bash/bin/bash`.
fn stdenv_shell(store: &Store, stdenv: &str) -> Result<String> {
    let setup = fs::read_to_string(store.phys(&format!("{stdenv}/setup")))?;
    setup
        .lines()
        .find_map(|l| l.strip_prefix("export SHELL="))
        .map(|s| s.trim_matches('"').to_string())
        .ok_or_else(|| anyhow!("can't find the shell in {stdenv}/setup"))
}

/// Make a build output look like every other store path: read-only, no
/// setuid bits, mtime 1.
pub(crate) fn canonicalize(path: &Path) -> Result<()> {
    let meta = path.symlink_metadata()?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        for e in fs::read_dir(path)? {
            canonicalize(&e?.path())?;
        }
    }
    let exec = meta.is_dir() || meta.permissions().mode() & 0o111 != 0;
    fs::set_permissions(path, fs::Permissions::from_mode(if exec { 0o555 } else { 0o444 }))?;
    let t = UNIX_EPOCH + Duration::from_secs(1);
    File::open(path)?.set_times(FileTimes::new().set_modified(t).set_accessed(t))?;
    Ok(())
}

/// Find which candidate store paths are mentioned anywhere in `path`'s
/// contents, by their 32-character hash part (what Nix does too).
pub(crate) fn scan_references(path: &Path, candidates: &[String]) -> Result<Vec<String>> {
    let by_hash: HashMap<&[u8], &String> = candidates.iter().map(|c| (&c.as_bytes()[..32], c)).collect();
    let mut found: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut scan = |data: &[u8]| {
        let is_b32 = |b: u8| b.is_ascii_digit() || (b.is_ascii_lowercase() && !b"eotu".contains(&b));
        let mut run = 0usize;
        for i in 0..data.len() {
            run = if is_b32(data[i]) { run + 1 } else { 0 };
            if run >= 32 {
                if let Some(c) = by_hash.get(&data[i + 1 - 32..=i]) {
                    found.insert((*c).clone());
                }
            }
        }
    };
    walk(path, &mut |p, meta| {
        if meta.file_type().is_symlink() {
            scan(fs::read_link(p)?.as_os_str().as_encoded_bytes());
        } else if meta.is_file() {
            scan(&fs::read(p)?);
        }
        Ok(())
    })?;
    Ok(found.into_iter().collect())
}

fn walk(path: &Path, f: &mut dyn FnMut(&Path, &fs::Metadata) -> Result<()>) -> Result<()> {
    let meta = path.symlink_metadata()?;
    f(path, &meta)?;
    if meta.is_dir() {
        for e in fs::read_dir(path)? {
            walk(&e?.path(), f)?;
        }
    }
    Ok(())
}

fn parse_sha256(s: &str) -> Result<[u8; 32]> {
    use base64::Engine;
    let bytes = if let Some(b64) = s.strip_prefix("sha256-") {
        base64::engine::general_purpose::STANDARD.decode(b64)?
    } else if s.len() == 64 {
        (0..32).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16)).collect::<Result<_, _>>()?
    } else {
        bail!("sha256 must be 64 hex characters or SRI (sha256-...): {s}");
    };
    bytes.try_into().map_err(|_| anyhow!("sha256 has the wrong length"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn valid_name(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('.') && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-._?=".contains(c))
}

fn sanitize(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || "+-._?=".contains(c) { c } else { '_' }).collect();
    s.trim_start_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_source_outside_the_recipe_is_another_build() {
        // A recipe that takes `source: path: ../../work/pkg`: a new
        // binary there was "already built" as the old one.
        let tmp = std::env::temp_dir().join(format!("nami-outside-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("nix/var/nami")).unwrap();
        let index = r#"{"stdenv":{"name":"stdenv-linux","outputs":{"out":"/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-stdenv-linux"}}}"#;
        fs::write(tmp.join("nix/var/nami/index.json"), index).unwrap();
        let store = Store::open_direct_readonly(&tmp).unwrap();
        let index = Index::load(&store).unwrap();
        let builder = Builder::new(&store, &index);
        let recipe_dir = tmp.join("packaging/pkg");
        fs::create_dir_all(&recipe_dir).unwrap();
        fs::create_dir_all(tmp.join("work/pkg/bin")).unwrap();
        let bin = tmp.join("work/pkg/bin/tool");
        fs::write(&bin, "one").unwrap();
        let text = "name: pkg\nversion: 0.1.0\nsource:\n  path: ../../work/pkg\ninstall: cp -r . $out\n";
        fs::write(recipe_dir.join("package.nami"), text).unwrap();
        let dir = recipe_dir.canonicalize().unwrap();
        let key = || {
            let recipe: Recipe = serde_yaml::from_value(serde_yaml::Value::Mapping(split_package(text).unwrap().0.unwrap())).unwrap();
            builder.built_key(&recipe, &dir).unwrap()
        };
        let first = key();
        assert_eq!(key(), first, "nothing changed: the same build");
        fs::write(&bin, "two").unwrap();
        let second = key();
        assert_ne!(second, first, "new contents: a new build");
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        assert_ne!(key(), second, "now executable: a new build");
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&bin, "one").unwrap();
        assert_eq!(key(), first, "back as it was: the first build again");
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn an_output_is_written_with_a_dot_and_the_caret_still_reads() {
        let tmp = std::env::temp_dir().join(format!("nami-outputs-{}", std::process::id()));
        fs::create_dir_all(tmp.join("nix/var/nami")).unwrap();
        let index = r#"{"libgcc":{"name":"libgcc-14","outputs":{"out":"/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-libgcc-14","lib":"/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-libgcc-14-lib"}},
            "nerd-fonts.jetbrains-mono":{"name":"jbm-3","outputs":{"out":"/nix/store/cccccccccccccccccccccccccccccccc-jbm-3"}}}"#;
        fs::write(tmp.join("nix/var/nami/index.json"), index).unwrap();
        let store = Store::open_direct_readonly(&tmp).unwrap();
        let index = Index::load(&store).unwrap();
        assert_eq!(split_output(&index, "libgcc.lib"), ("libgcc", Some("lib")));
        assert_eq!(split_output(&index, "libgcc^lib"), ("libgcc", Some("lib")));
        assert_eq!(split_output(&index, "libgcc"), ("libgcc", None));
        // A name with a dot of its own, an output it hasn't, a version.
        assert_eq!(split_output(&index, "nerd-fonts.jetbrains-mono"), ("nerd-fonts.jetbrains-mono", None));
        assert_eq!(split_output(&index, "libgcc.man"), ("libgcc.man", None));
        assert_eq!(split_output(&index, "libgcc@14.2"), ("libgcc@14.2", None));
        assert_eq!(old_output_spelling("libgcc.lib"), None);
        assert!(old_output_spelling("libgcc^lib").unwrap().contains("written with a dot now, libgcc.lib"));
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn build_definitions_from_shards_and_from_files() {
        let tmp = std::env::temp_dir().join(format!("nami-defs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("nix/var/nami")).unwrap();
        let sha = |t: &str| hex(&Sha256::digest(t.as_bytes()));
        let def = |name: &str| format!("{{\"/nix/store/{}-{name}.drv\":{{\"env\":{{\"name\":\"{name} \\\"é\\\"\"}}}}}}\n", "d".repeat(32));
        let (plugin, old) = ("vimPlugins.telescope-nvim", "rPackages.ggplot2");
        // A closure in parts (index/closures.py --parts), and one in one piece.
        let part = r#"{"env":{"name":"tidyverse"},"inputs":{"drvs":{},"srcs":[]},"outputs":{"out":{"path":"x"}}}"#;
        let parts = serde_json::json!({"top": "t.drv", "cached": {"c.drv": {"out": "c"}}, "drvs_by_sha256": {"t.drv": sha(part)}, "srcs_by_sha256": {"/nix/store/p.patch": sha("UEFUQ0g=")}}).to_string();
        let whole = serde_json::json!({"top": "s.drv", "cached": {}, "drvs": {"s.drv": {"env": {}}}, "srcs": {}}).to_string();
        let index = serde_json::json!({
            plugin: {"name": "t-1", "outputs": {}, "drv": sha(&def(plugin))},
            old: {"name": "g-1", "outputs": {}, "drv": sha(&def(old))},
            "tampered": {"name": "x-1", "outputs": {}, "drv": sha(&def("tampered"))},
            "rPackages.tidyverse": {"name": "r-tidyverse-2", "outputs": {}, "uncached": true, "closure": sha(&parts)},
            "steam": {"name": "steam-1", "outputs": {}, "unfree": true, "closure": sha(&whole)},
        });
        fs::write(tmp.join("nix/var/nami/index.json"), index.to_string()).unwrap();
        // This catalog's: shards by sha256 (index/defs.py), next to drvs/.
        let cat = tmp.join("cat");
        fs::create_dir_all(cat.join("defs")).unwrap();
        fs::create_dir_all(cat.join("drvs")).unwrap();
        let write_shard = |s: &str, held: &BTreeMap<String, String>| {
            let mut xz = xz2::write::XzEncoder::new(Vec::new(), 6);
            xz.write_all(serde_json::to_string(held).unwrap().as_bytes()).unwrap();
            fs::write(cat.join(format!("defs/{s}.json.xz")), xz.finish().unwrap()).unwrap();
        };
        let mut shards: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for (key, text) in [(sha(&def(plugin)), def(plugin)), (sha(&def("tampered")), def("other")), (sha(&parts), parts.clone()), (sha(part), part.into()), (sha("UEFUQ0g="), "UEFUQ0g=".into())] {
            shards.entry(key[..3].to_string()).or_default().insert(key, text);
        }
        for (s, held) in &shards {
            write_shard(s, held);
        }
        // An older catalog's definition, a file of its own; a closure in one piece too.
        assert!(!shards.contains_key(&sha(&def(old))[..3]), "(not in a shard that's there)");
        fs::write(cat.join(format!("drvs/{old}.json")), def(old)).unwrap();
        fs::write(cat.join("drvs/steam.closure.json"), &whole).unwrap();
        fs::write(tmp.join("nix/var/nami/drvs-source"), cat.join("drvs").display().to_string()).unwrap();
        let store = Store::open_direct_readonly(&tmp).unwrap();
        let index = Index::load(&store).unwrap();
        let b = Builder::new(&store, &index);
        let (key, drv) = b.load_drv(plugin).unwrap();
        assert!(key.ends_with(&format!("-{plugin}.drv")), "{key}");
        assert_eq!(drv["env"]["name"], format!("{plugin} \"é\""));
        assert!(b.load_drv(old).unwrap().0.ends_with(&format!("-{old}.drv")), "from its own file");
        let err = format!("{:#}", b.load_drv("tampered").unwrap_err());
        assert!(err.contains("doesn't match"), "{err}");
        let err = format!("{:#}", b.load_drv("missing").unwrap_err());
        assert!(err.contains("no build definition"), "{err}");
        let closure = |attr: &str| {
            let want = index.get(attr).unwrap().closure.clone().unwrap();
            b.closure_from(&b.definition(&format!("{attr}.closure.json"), Some(&want))?, attr)
        };
        let c = closure("rPackages.tidyverse").unwrap();
        assert_eq!((c.top.as_str(), c.drvs["t.drv"]["env"]["name"].as_str()), ("t.drv", Some("tidyverse")));
        assert_eq!((c.srcs["/nix/store/p.patch"].as_str(), c.cached["c.drv"]["out"].as_str()), ("UEFUQ0g=", "c"));
        assert_eq!(closure("steam").unwrap().top, "s.drv");
        // A part changed: refused.
        let s = sha(part)[..3].to_string();
        let mut held = shards[&s].clone();
        held.insert(sha(part), part.replace("tidyverse", "evil"));
        write_shard(&s, &held);
        let Err(err) = closure("rPackages.tidyverse") else { panic!("a changed part was taken") };
        let err = format!("{err:#}");
        assert!(err.contains("doesn't match"), "{err}");
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn finds_references_by_hash() {
        let dir = std::env::temp_dir().join(format!("nami-refs-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a"), b"rpath=/nix/store/0mdqa9w1p6cmli6976v4wi0sw9r4p5pr-glibc/lib\0junk").unwrap();
        let refs = scan_references(&dir, &[
            "0mdqa9w1p6cmli6976v4wi0sw9r4p5pr-glibc".into(),
            "1mdqa9w1p6cmli6976v4wi0sw9r4p5pr-unused".into(),
        ]).unwrap();
        assert_eq!(refs, vec!["0mdqa9w1p6cmli6976v4wi0sw9r4p5pr-glibc".to_string()]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn attrs_sh_matches_nix() {
        // Captured from a real Nix build (see git history for the probe).
        let attrs = serde_json::json!({"builder":"/bin/sh","env":{"FOO":"bar"},"f":false,"list":["a b","c"],"n":null,
            "name":"attrs-probe","nested":[{"a":1}],"num":3,"outputs":{"dev":"/nix/store/a-dev","out":"/nix/store/b"},
            "set":{"k":"v"},"str":"it's","system":"x86_64-linux","t":true});
        assert_eq!(attrs_sh(&attrs), "declare builder='/bin/sh'
declare -A env=(['FOO']='bar' )
declare f=
declare -a list=('a b' 'c' )
declare n=''
declare name='attrs-probe'
declare num=3
declare -A outputs=(['dev']='/nix/store/a-dev' ['out']='/nix/store/b' )
declare -A set=(['k']='v' )
declare str='it'\\''s'
declare system='x86_64-linux'
declare t=1
");
    }

    #[test]
    fn placeholder_matches_nix() {
        // `builtins.placeholder "out"`
        assert_eq!(placeholder("out"), "/1rz4g4znpzjwh1xymhjpm42vipw92pr73vdgl6xs1hycac8kf2n9");
    }

    #[test]
    fn parses_hash_formats() {
        let hex = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let sri = "sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=";
        assert_eq!(parse_sha256(hex).unwrap(), parse_sha256(sri).unwrap());
    }

    #[test]
    fn debs_in_a_recipe() {
        let one: Recipe = serde_yaml::from_str("name: a\nversion: '1'\nsource: { url: https://x/a.deb, sha256: ab }\n").unwrap();
        assert_eq!(one.source.as_ref().unwrap().list().len(), 1);
        let several: Recipe = serde_yaml::from_str(
            "name: a\nversion: '1'\nsource:\n  - { url: https://x/a.deb, sha256: ab }\n  - { path: ./a-data.deb }\nbundle:\n  - { url: https://x/libb1.deb, sha256: cd }\n",
        )
        .unwrap();
        let list = several.source.as_ref().unwrap().list();
        assert!(list.len() == 2 && list.iter().all(Source::is_deb));
        assert_eq!(several.bundle.len(), 1);
        assert!(serde_yaml::from_str::<Recipe>("name: a\nversion: '1'\nsource: { url: https://x/a.deb, sha: ab }\n").is_err());

        // A source for each kind of computer: this one's is built from.
        let by: Recipe = serde_yaml::from_str(
            "name: a\nversion: '1'\nsource:\n  x86_64-linux:\n    url: https://x/a-x86_64-linux.tar.gz\n    sha256: ab\n  aarch64-linux:\n    url: https://x/a-aarch64-linux.tar.gz\n    sha256: cd\n",
        )
        .unwrap();
        let s = by.source.as_ref().unwrap();
        assert_eq!(s.list_for("aarch64-linux")[0].sha256.as_deref(), Some("cd"));
        assert_eq!(s.list_for("x86_64-linux")[0].url.as_deref(), Some("https://x/a-x86_64-linux.tar.gz"));
        assert!(s.list_for("aarch64-darwin").is_empty());
        // (Linux's kinds: a Mac's tests have none of its own in it.)
        if SYSTEM.ends_with("-linux") {
            let here = s.here("a").unwrap();
            assert!(here.len() == 1 && here[0].url.as_deref().unwrap().ends_with(&format!("-{SYSTEM}.tar.gz")), "{here:?}");
        } else {
            assert!(s.here("a").unwrap_err().to_string().contains("has no source for this computer"));
        }
        // (Written again as it was: a recipe's built key is its JSON.)
        assert_eq!(serde_json::to_value(s).unwrap()["aarch64-linux"]["sha256"], "cd");
        // One kind's several Debian packages; a kind it hasn't; one that isn't a kind; a typo in one.
        let debs: Recipe = serde_yaml::from_str("name: a\nversion: '1'\nsource:\n  x86_64-linux: [{ url: https://x/a_amd64.deb }, { url: https://x/b_amd64.deb }]\n  aarch64-linux: [{ url: https://x/a_arm64.deb }, { url: https://x/b_arm64.deb }]\n").unwrap();
        if SYSTEM.ends_with("-linux") {
            assert!(debs.source.as_ref().unwrap().here("a").unwrap().iter().all(Source::is_deb));
        }
        let other = if SYSTEM == "x86_64-linux" { "aarch64-linux" } else { "x86_64-linux" };
        let only: Recipe = serde_yaml::from_str(&format!("name: a\nversion: '1'\nsource:\n  {other}: {{ url: https://x/a.tar.gz }}\n")).unwrap();
        let e = only.source.as_ref().unwrap().here("a").unwrap_err().to_string();
        assert_eq!(e, format!("a has no source for this computer ({SYSTEM}): its recipe has one for {other}"));
        let wrong: Recipe = serde_yaml::from_str(&format!("name: a\nversion: '1'\nsource:\n  {SYSTEM}: {{ url: https://x/a.tar.gz }}\n  arm64: {{ url: https://x/b.tar.gz }}\n")).unwrap();
        assert!(wrong.source.as_ref().unwrap().here("a").unwrap_err().to_string().contains("arm64 isn't a kind of computer"));
        assert!(serde_yaml::from_str::<Recipe>("name: a\nversion: '1'\nsource:\n  x86_64-linux: { url: https://x/a.tar.gz, sha: ab }\n").is_err());

        // Paths built in: each program links libredirect, each launcher says where they go.
        let plain = deb_fixup(&[]);
        assert!(plain.contains("@out@") && !plain.contains("libredirect"));
        let fix = deb_fixup(&["/usr/share/foo".into(), "/opt/Foo".into(), "/usr/bin/foo-helper".into()]);
        assert!(fix.contains("redirects=\"/usr/share/foo=$out/share/foo:/opt/Foo=$out/opt/Foo:/usr/bin/foo-helper=$out/bin/foo-helper\"\n"), "{fix}");
        assert!(fix.contains("patchelf --add-needed libredirect.so"));
    }
}
