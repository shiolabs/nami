//! `nami bundle .`: a project folder made into a bundle with no recipe.
//!
//! What kind of project it is comes from its files (Cargo.toml, go.mod,
//! meson.build, CMakeLists.txt, or programs already built), and it's built
//! with the same tools `nami run` gives it: the project's own project.nami
//! packages, plus the build tool its files ask for, in nixpkgs' build
//! environment (project.rs's `develop`). The network is there (cargo and go
//! fetch what they need), and nothing is written in the project but what
//! its own tools write (cargo's target/).
//!
//! It's installed under a store path of its own, `/nix/store/<hash>-<name>-
//! <version>`, staged in a folder of nami's cache: what it compiles in (a
//! data folder, its own libraries' rpath) names that path, which the bundle
//! runtime moves with everything else. The hash is made from what was
//! built, once it's built (the build uses a stand-in of the same length,
//! then replaced): the same build is the same path, so bundles share it.
//!
//! Its name, version, icon and .desktop file come from where the project
//! keeps them (Cargo.toml, CMake's or Meson's project(), git's tags, a
//! .desktop file in the repo), else from `bundle:` in project.nami or the
//! flags; what it couldn't find is said, with how to say it.
//!
//! Python, Node and Electron, Zig, Flutter and .NET projects are in
//! src/bundle_langs.rs, a child of this one.
//!
//! This is a child of bundle.rs (it fills in its private `Plan`).

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde_yaml::Value;

use super::{local_program, Plan, STORE};

/// Python, Node and Electron, Zig, Flutter and .NET.
#[path = "bundle_langs.rs"]
mod langs;
pub(crate) use langs::python_fits;
use crate::index::Index;
use crate::store::Store;

/// What the flags say (`--name`, `--app-version`, `--icon`, `--desktop-file`).
#[derive(Debug, Clone, Default)]
pub struct Hints {
    pub name: Option<String>,
    pub version: Option<String>,
    pub icon: Option<PathBuf>,
    pub desktop: Option<PathBuf>,
}

impl Hints {
    fn any(&self) -> bool {
        self.name.is_some() || self.version.is_some() || self.icon.is_some() || self.desktop.is_some()
    }
}

/// project.nami's `bundle:`, and Cargo.toml's `[package.metadata.nami]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings {
    pub name: Option<String>,
    pub version: Option<String>,
    pub program: Option<String>,
    /// A build command of the project's own, in place of the usual one.
    pub build: Option<String>,
    /// With `build:`, where the program (or the folder with it) ends up.
    pub output: Option<String>,
    pub icon: Option<String>,
    pub desktop: Option<String>,
}

/// The keys `bundle:` takes.
pub const SETTINGS_KEYS: &[&str] = &["name", "version", "program", "build", "output", "icon", "desktop", "developer", "sandbox"];

impl Settings {
    pub fn from_value(v: &Value, what: &str) -> Result<Settings> {
        let Some(m) = v.as_mapping() else {
            if v.is_null() {
                return Ok(Settings::default());
            }
            bail!("{what}: bundle: is a list of keys ({})", SETTINGS_KEYS.join(", "));
        };
        let mut s = Settings::default();
        for (k, val) in m {
            let k = k.as_str().unwrap_or_default();
            // (Who made it, and its sandbox: what goes in the bundle, read by bundle.rs.)
            if k == "developer" || k == "sandbox" {
                continue;
            }
            let text = match val {
                Value::String(t) => t.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => bail!("{what}: bundle: {k}: is a word or a line, not a list"),
            };
            let slot = match k {
                "name" => &mut s.name,
                "version" => &mut s.version,
                "program" => &mut s.program,
                "build" => &mut s.build,
                "output" => &mut s.output,
                "icon" => &mut s.icon,
                "desktop" => &mut s.desktop,
                _ => {
                    let hint = crate::config::closest(k, SETTINGS_KEYS).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
                    bail!("{what}: bundle: has no key `{k}`{hint}; it takes {}", SETTINGS_KEYS.join(", "));
                }
            };
            *slot = Some(text);
        }
        Ok(s)
    }
}

/// What kind of project a folder is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Rust,
    Go,
    Meson,
    CMake,
    /// pyproject.toml: its `[project.scripts]` or `gui-scripts`, on the catalog's Python.
    Python,
    /// package.json: a command (`bin`) on the catalog's Node.js.
    Node,
    /// package.json with Electron: the app on the catalog's Electron.
    Electron,
    /// build.zig.
    Zig,
    /// pubspec.yaml with a linux/ folder: `flutter build linux`.
    Flutter,
    /// A .csproj or .fsproj: on the catalog's .NET runtime.
    DotNet,
    /// Programs already built (a folder, or an unpacked tarball).
    Prebuilt,
    /// project.nami's `bundle: build:`.
    Custom,
}

impl Kind {
    fn about(self) -> &'static str {
        match self {
            Kind::Rust => "Rust (Cargo.toml)",
            Kind::Go => "Go (go.mod)",
            Kind::Meson => "C/C++ with Meson (meson.build)",
            Kind::CMake => "C/C++ with CMake (CMakeLists.txt)",
            Kind::Python => "Python (pyproject.toml)",
            Kind::Node => "Node.js (package.json)",
            Kind::Electron => "Electron (package.json)",
            Kind::Zig => "Zig (build.zig)",
            Kind::Flutter => "Flutter (pubspec.yaml)",
            Kind::DotNet => ".NET (.csproj)",
            Kind::Prebuilt => "programs already built",
            Kind::Custom => "its own build (bundle: build: in project.nami)",
        }
    }

    /// The catalog's tools its build needs.
    fn tools(self) -> &'static [&'static str] {
        match self {
            Kind::Rust => &["cargo", "rustc"],
            Kind::Go => &["go"],
            Kind::Meson => &["meson", "ninja", "pkg-config"],
            Kind::CMake => &["cmake", "pkg-config"],
            // (Which version of each: src/bundle_langs.rs, from the project's files.)
            Kind::Python => &["uv"],
            Kind::Node | Kind::Electron | Kind::DotNet => &["bash"],
            Kind::Zig | Kind::Flutter => &[],
            Kind::Prebuilt | Kind::Custom => &[],
        }
    }
}

/// The kind of project in `dir`, from its files (None: none nami knows).
pub fn detect(dir: &Path, settings: &Settings, program: Option<&str>) -> Option<Kind> {
    if settings.build.is_some() {
        return Some(Kind::Custom);
    }
    let has = |f: &str| dir.join(f).is_file();
    if has("Cargo.toml") {
        Some(Kind::Rust)
    } else if has("go.mod") {
        Some(Kind::Go)
    } else if has("meson.build") {
        Some(Kind::Meson)
    } else if has("CMakeLists.txt") {
        Some(Kind::CMake)
    } else if let Some(k) = langs::detect(dir) {
        Some(k)
    } else if !programs_in(dir, program).is_empty() {
        Some(Kind::Prebuilt)
    } else {
        None
    }
}

/// Is `p` an archive nami unpacks (a release tarball)?
pub fn is_archive(p: &Path) -> bool {
    let n = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
    [".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2", ".tar.zst", ".tar", ".zip"].iter().any(|e| n.ends_with(e))
}

/// A line of what was found, and where from.
struct Found {
    lines: Vec<(String, String, String)>,
    missing: Vec<String>,
}

impl Found {
    fn new() -> Self {
        Found { lines: Vec::new(), missing: Vec::new() }
    }
    fn add(&mut self, what: &str, value: &str, from: &str) {
        self.lines.push((what.into(), value.into(), from.into()));
    }
    fn say(&self, kind: Kind) {
        crate::say!("project: {}", kind.about());
        for (what, value, from) in &self.lines {
            crate::say!("  {what:8} {value}{}", if from.is_empty() { String::new() } else { format!("  ({from})") });
        }
        for m in &self.missing {
            crate::say!("  note: {m}");
        }
    }
}

/// `nami bundle <folder or tarball>` when it's a project rather than a
/// recipe: None when `opts.what` isn't one (bundle.rs goes on as before).
pub(super) fn plan(store: &Store, index: &Index, opts: &super::Options) -> Result<Option<Plan>> {
    let path = Path::new(&opts.what);
    let hints = &opts.project;
    let program = opts.program.as_deref();
    // A store path (./result): bundle.rs's own way, with its closure.
    if path.canonicalize().is_ok_and(|c| c.starts_with(STORE)) {
        return Ok(None);
    }
    let (dir, from_archive) = if path.is_file() && is_archive(path) {
        (unpack(store, path)?, true)
    } else if path.is_dir() {
        // A folder and the path of the program in it: bundle.rs's own way.
        if program.is_some_and(|p| p.contains('/') && path.join(p).is_file()) {
            return Ok(None);
        }
        (path.canonicalize()?, false)
    } else if path.is_file() && hints.any() {
        // One program, with a name or icon said for it.
        let dir = work_dir(store, &path.canonicalize()?)?.join("one");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("bin"))?;
        let name = path.file_name().unwrap_or_default();
        fs::copy(path, dir.join("bin").join(name))?;
        (dir, true)
    } else {
        return Ok(None);
    };
    let settings = settings_in(&dir)?;
    let program = program.map(String::from).or(settings.program.clone());
    let Some(kind) = detect(&dir, &settings, program.as_deref()) else {
        bail!(
            "{} isn't a project nami knows how to build: it looked for Cargo.toml (Rust), go.mod (Go), meson.build or CMakeLists.txt (C/C++), pyproject.toml (Python), package.json (Node.js, Electron), build.zig (Zig), pubspec.yaml (Flutter), a .csproj (.NET), and programs already built.\n\
             Say how it's built in its project.nami:\n\n  bundle:\n    build: make && make install PREFIX=$PREFIX DESTDIR=$DESTDIR\n\n\
             (or `output: path/to/the/program` instead of installing), or write a recipe (package.nami): {}",
            dir.display(),
            crate::site::docs_url("nami/recipes")
        );
    };
    let p = Project { store, index, dir: dir.clone(), kind, settings, hints: hints.clone(), program, from_archive, archive: from_archive.then(|| path.to_path_buf()) };
    p.plan().map(Some)
}

struct Project<'a> {
    store: &'a Store,
    index: &'a Index,
    dir: PathBuf,
    kind: Kind,
    settings: Settings,
    hints: Hints,
    program: Option<String>,
    /// From a tarball (or one file): not the developer's folder.
    from_archive: bool,
    archive: Option<PathBuf>,
}

/// What a build gives: the installed tree (under the stand-in path), and
/// what runs.
struct Built {
    tree: PathBuf,
    /// The program's path inside `tree`.
    entry: String,
    /// An app with a window (gui-scripts, Electron, Flutter): it gets a
    /// menu entry even when nothing names an icon.
    gui: bool,
}

/// project.nami's `bundle:` in `dir`, if it has one.
fn settings_in(dir: &Path) -> Result<Settings> {
    let Some(file) = crate::project::file_in(dir) else { return Ok(Settings::default()) };
    match crate::project::read(&file)?.bundle {
        Some(b) => Settings::from_value(&b, &file.display().to_string()),
        None => Ok(Settings::default()),
    }
}

/// nixpkgs' build environment points SSL_CERT_FILE nowhere (its builds
/// have no network); these builds fetch (npm, uv, NuGet, pub): the
/// computer's certificates, where it keeps them.
const CERTS: &str = "if [ \"${SSL_CERT_FILE:-}\" = /no-cert-file.crt ]; then unset SSL_CERT_FILE NIX_SSL_CERT_FILE; \
     for f in /etc/ssl/certs/ca-certificates.crt /etc/pki/tls/certs/ca-bundle.crt /etc/ssl/ca-bundle.pem /etc/ssl/cert.pem; do \
     if [ -e \"$f\" ]; then export SSL_CERT_FILE=\"$f\" NIX_SSL_CERT_FILE=\"$f\"; break; fi; done; fi";

/// Where a project's builds and stages go: nami's cache, by the folder.
fn work_dir(store: &Store, dir: &Path) -> Result<PathBuf> {
    let w = store.cache_dir().join("bundle-projects").join(crate::project::key(dir));
    fs::create_dir_all(&w).with_context(|| format!("making {}", w.display()))?;
    Ok(w)
}

impl Project<'_> {
    fn plan(&self) -> Result<Plan> {
        let mut found = Found::new();
        let work = work_dir(self.store, &self.dir)?;
        // Name and version first: the store path is named after them.
        let meta = self.metadata(&mut found)?;
        let label = store_label(&meta.name, meta.version.as_deref());
        let stand_in = format!("{}-{label}", hash32(format!("tray-bundle-project:{label}").as_bytes()));
        let prefix = format!("{STORE}/{stand_in}");
        let destdir = work.join("destdir");
        let _ = fs::remove_dir_all(&destdir);
        let tree = destdir.join(prefix.trim_start_matches('/'));
        fs::create_dir_all(&tree)?;
        let built = match self.kind {
            Kind::Rust => self.build_rust(&meta, &tree, &mut found)?,
            Kind::Go => self.build_go(&meta, &tree, &work, &mut found)?,
            Kind::Meson | Kind::CMake => self.build_c(&meta, &tree, &prefix, &work, &destdir, &mut found)?,
            Kind::Prebuilt => self.stage_prebuilt(&meta, &tree, &mut found)?,
            Kind::Custom => self.build_custom(&meta, &tree, &prefix, &work, &destdir, &mut found)?,
            Kind::Python => self.build_python(&meta, &tree, &prefix, &work, &mut found)?,
            Kind::Node | Kind::Electron => self.build_node(&meta, &tree, &prefix, &mut found)?,
            Kind::Zig => self.build_zig(&meta, &tree, &work, &mut found)?,
            Kind::Flutter => self.build_flutter(&meta, &tree, &mut found)?,
            Kind::DotNet => self.build_dotnet(&meta, &tree, &prefix, &work, &mut found)?,
        };
        let program = Path::new(&built.entry).file_name().unwrap_or_default().to_string_lossy().into_owned();
        found.add("program", &built.entry, "");
        self.desktop_and_icon(&meta, &built, &program, &mut found)?;
        found.say(self.kind);
        // The stand-in becomes the hash of what was built.
        let (nar, _) = crate::nar::hash_path(&built.tree)?;
        let base = format!("{}-{label}", hash32(format!("{nar}:{label}").as_bytes()));
        rewrite_tree(&built.tree, stand_in[..32].as_bytes(), base[..32].as_bytes())?;
        let out = work.join("out");
        let _ = fs::remove_dir_all(&out);
        fs::create_dir_all(&out)?;
        let final_tree = out.join(&base);
        fs::rename(&built.tree, &final_tree)?;
        let mut plan = local_program(self.store, self.index, &final_tree, Some(&built.entry), Some(&base))?;
        plan.name = meta.name.clone();
        plan.version = meta.version.clone().unwrap_or_default();
        plan.tops = vec![format!("{STORE}/{base}")];
        Ok(plan)
    }

    /// The app's name and version: the flags, project.nami, then what the
    /// project's own files say.
    fn metadata(&self, found: &mut Found) -> Result<Meta> {
        let own = match self.kind {
            Kind::Rust => self.cargo_meta()?,
            Kind::Go => go_meta(&self.dir),
            Kind::Meson => meson_meta(&self.dir),
            Kind::CMake => cmake_meta(&self.dir),
            Kind::Python => langs::python_meta(&self.dir),
            Kind::Node | Kind::Electron => langs::node_meta(&self.dir),
            Kind::Zig => langs::zig_meta(&self.dir),
            Kind::Flutter => langs::flutter_meta(&self.dir),
            Kind::DotNet => langs::dotnet_meta(&self.dir),
            Kind::Prebuilt | Kind::Custom => self.prebuilt_meta(),
        };
        let pick = |flag: &Option<String>, set: &Option<String>, own: &Option<(String, String)>| -> Option<(String, String)> {
            flag.clone().map(|v| (v, "--flag".to_string())).or_else(|| set.clone().map(|v| (v, "project.nami".to_string()))).or_else(|| own.clone())
        };
        let name = pick(&self.hints.name, &self.settings.name, &own.name);
        let name = match name {
            Some((n, from)) => {
                found.add("name", &n, if from == "--flag" { "--name" } else { &from });
                n
            }
            None => {
                let n = self.dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
                found.add("name", &n, "the folder's name");
                found.missing.push("no name found: the folder's is used. Say it with --name, or `bundle: name:` in project.nami".into());
                n
            }
        };
        let mut version = pick(&self.hints.version, &self.settings.version, &own.version);
        if version.is_none() && !self.from_archive {
            version = git_version(&self.dir).map(|v| (v, "git describe".into()));
        }
        let version = match version {
            Some((v, from)) => {
                found.add("version", &v, if from == "--flag" { "--app-version" } else { &from });
                Some(v)
            }
            None => {
                found.missing.push("no version found (none in its files, and no git tag): it has none. Say it with --app-version, or `bundle: version:` in project.nami".into());
                None
            }
        };
        Ok(Meta { name, version, cargo: own.cargo })
    }

    fn prebuilt_meta(&self) -> OwnMeta {
        let mut m = OwnMeta::default();
        // A release's tarball, or the folder it unpacks to: myapp-1.2.0-linux-x86_64.
        let (file, from) = match &self.archive {
            Some(a) => (a.clone(), "the file's name"),
            None if self.kind == Kind::Prebuilt => (self.dir.clone(), "the folder's name"),
            None => return m,
        };
        let (n, v) = archive_name(&file);
        if !n.is_empty() {
            m.name = Some((n, from.into()));
        }
        if let Some(v) = v {
            m.version = Some((v, from.into()));
        }
        m
    }

    /// The project's environment, as `nami run` has it: project.nami's
    /// packages and the tools this kind of build needs.
    fn env(&self, extra: &[String]) -> Result<crate::project::Env> {
        let mut packages: Vec<String> = Vec::new();
        let mut env = serde_yaml::Mapping::new();
        let mut develop = None;
        if let Some(file) = crate::project::file_in(&self.dir) {
            let p = crate::project::read(&file)?;
            for name in p.packages {
                // Recipes by path, from the project's folder.
                let name = if name.starts_with("./") || name.starts_with("../") { self.dir.join(&name).display().to_string() } else { name };
                packages.push(name);
            }
            for (k, v) in p.env {
                env.insert(Value::from(k), Value::from(v));
            }
            develop = p.develop;
        }
        for t in self.kind.tools().iter().map(|s| s.to_string()).chain(extra.iter().cloned()) {
            // The project's own (python312, nodejs@22) over the one nami would pick.
            let have = packages.iter().any(|p| family(p) == family(&t) || (t == "cargo" && p == "rustup") || (t == "rustc" && p == "rustup"));
            if !have {
                packages.push(t);
            }
        }
        let mut m = serde_yaml::Mapping::new();
        m.insert("packages".into(), Value::Sequence(packages.into_iter().map(Value::from).collect()));
        if !env.is_empty() {
            m.insert("env".into(), Value::Mapping(env));
        }
        // A compiler, always (Go and Rust link with one).
        m.insert("develop".into(), Value::Bool(develop.unwrap_or(true)));
        let work = work_dir(self.store, &self.dir)?.join("env");
        fs::create_dir_all(&work)?;
        let file = work.join(crate::project::FILE);
        fs::write(&file, serde_yaml::to_string(&Value::Mapping(m))?)?;
        crate::project::prepare(self.store, &crate::profile::Profiles::personal(self.store)?, &file, false, false)
    }

    /// Run `script` in the project's folder, with the project's tools.
    fn sh(&self, env: &crate::project::Env, script: &str, vars: &[(&str, String)], capture: bool) -> Result<String> {
        crate::say!("$ {}", script.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("; "));
        let mut cmd = match &env.rc {
            Some(rc) => {
                let mut c = Command::new("bash");
                c.arg("-c").arg(format!("source \"$NAMI_RC\" >/dev/null 2>&1; {CERTS}; cd \"$NAMI_BUNDLE_DIR\" && set -e\n{script}", CERTS = CERTS)).env("NAMI_RC", rc).envs(env.secrets.iter().cloned());
                c
            }
            None => {
                let mut c = Command::new("bash");
                c.arg("-c").arg(format!("set -e\n{script}")).envs(env.vars.iter().cloned());
                c
            }
        };
        cmd.current_dir(&self.dir).env("NAMI_BUNDLE_DIR", &self.dir).envs(vars.iter().map(|(k, v)| (k, v)));
        cmd.stdin(Stdio::null()).stderr(Stdio::inherit());
        if capture {
            let out = cmd.stdout(Stdio::piped()).output().context("starting bash")?;
            if !out.status.success() {
                bail!("the build failed (above)");
            }
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            // The build's own output, to the terminal, as stderr (stdout is the bundle's path).
            use std::os::fd::AsFd;
            let status = cmd.stdout(Stdio::from(std::io::stderr().as_fd().try_clone_to_owned()?)).status().context("starting bash")?;
            if !status.success() {
                bail!("the build failed (above)");
            }
            Ok(String::new())
        }
    }

    fn cargo_meta(&self) -> Result<OwnMeta> {
        let text = fs::read_to_string(self.dir.join("Cargo.toml")).context("reading Cargo.toml")?;
        let toml: toml::Value = toml::from_str(&text).context("reading Cargo.toml")?;
        Ok(cargo_own_meta(&toml, self.program.as_deref()))
    }

    fn build_rust(&self, meta: &Meta, tree: &Path, found: &mut Found) -> Result<Built> {
        // (patchelf: for what its crates open at run time, below.)
        let env = self.env(&["patchelf".to_string()])?;
        // Which package and program: cargo says what the workspace has.
        let json = self.sh(&env, "cargo metadata --no-deps --format-version 1", &[], true)?;
        let m: serde_json::Value = serde_json::from_str(&json).context("reading cargo metadata")?;
        let (package, bin) = pick_cargo_bin(&m, &self.dir, self.program.as_deref())?;
        if meta.cargo.as_ref().is_some_and(|c| c.package != package) {
            found.missing.push(format!("built {package}'s {bin}"));
        }
        let out = self.sh(&env, &format!("cargo build --release --package '{package}' --bin '{bin}' --message-format=json-render-diagnostics"), &[], true)?;
        let exe = out
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v["reason"] == "compiler-artifact" && v["target"]["name"] == bin.as_str())
            .find_map(|v| v["executable"].as_str().map(PathBuf::from))
            .with_context(|| format!("cargo built no program {bin}"))?;
        let dest = tree.join("bin").join(&bin);
        fs::create_dir_all(dest.parent().unwrap())?;
        fs::copy(&exe, &dest).with_context(|| format!("copying {}", exe.display()))?;
        // What its crates open at run time (winit's X11 and Wayland, wgpu's
        // Vulkan and EGL), from Cargo.lock (there now): on its RUNPATH.
        let opened = dlopened(&self.dir);
        if !opened.is_empty() {
            let mut dirs = Vec::new();
            for (_, packages) in &opened {
                for p in *packages {
                    let pkg = self.index.get(p)?;
                    let out = pkg.outputs.get("lib").or_else(|| pkg.outputs.get("out")).cloned().flatten().with_context(|| format!("{p} has nothing ready-built"))?;
                    let dir = format!("{out}/lib");
                    if !dirs.contains(&dir) {
                        dirs.push(dir);
                    }
                }
            }
            let vars = [("NAMI_BUNDLE_RPATH", dirs.join(":")), ("NAMI_BUNDLE_OUT", dest.display().to_string())];
            self.sh(&env, "patchelf --add-rpath \"$NAMI_BUNDLE_RPATH\" \"$NAMI_BUNDLE_OUT\"", &vars, false)?;
            let crates: Vec<&str> = opened.iter().map(|(c, _)| *c).collect();
            let libs: BTreeSet<&str> = opened.iter().flat_map(|(_, p)| p.iter().copied()).collect();
            found.add("opens", &libs.into_iter().collect::<Vec<_>>().join(", "), &format!("Cargo.lock: {} load them at run time", crates.join(", ")));
        }
        Ok(Built { tree: tree.to_path_buf(), entry: format!("bin/{bin}"), gui: false })
    }

    fn build_go(&self, meta: &Meta, tree: &Path, work: &Path, found: &mut Found) -> Result<Built> {
        let (pkg, name) = pick_go_main(&self.dir, self.program.as_deref(), &meta.name)?;
        if pkg != "." {
            found.add("main", &pkg, "");
        }
        let env = self.env(&[])?;
        let dest = tree.join("bin").join(&name);
        fs::create_dir_all(dest.parent().unwrap())?;
        // Its modules come from the network, kept in nami's cache unless the developer has their own.
        let cache = work.join("go");
        let mut vars = vec![("NAMI_BUNDLE_OUT", dest.display().to_string()), ("NAMI_BUNDLE_PKG", pkg.clone())];
        if std::env::var_os("GOPATH").is_none() && !std::env::var_os("HOME").is_some_and(|h| Path::new(&h).join("go").is_dir()) {
            vars.push(("GOPATH", cache.display().to_string()));
        }
        self.sh(&env, "go build -trimpath -o \"$NAMI_BUNDLE_OUT\" \"$NAMI_BUNDLE_PKG\"", &vars, false)?;
        Ok(Built { tree: tree.to_path_buf(), entry: format!("bin/{name}"), gui: false })
    }

    /// Meson and CMake: configured to install at the stand-in path, and
    /// installed into a folder (DESTDIR) under it.
    fn build_c(&self, meta: &Meta, tree: &Path, prefix: &str, work: &Path, destdir: &Path, found: &mut Found) -> Result<Built> {
        let (mods, unknown) = c_dependencies(self.index, &self.dir, self.kind);
        if !mods.is_empty() {
            found.add("needs", &mods.join(", "), if self.kind == Kind::Meson { "meson.build's dependency()" } else { "CMakeLists.txt" });
        }
        for u in unknown {
            found.missing.push(format!("it asks for {u}, which nami doesn't know a package for: if the build fails for it, list its package under packages: in project.nami (nami search {})", u.to_lowercase()));
        }
        let env = self.env(&mods)?;
        let build = work.join("build");
        let vars = vec![
            ("PREFIX", prefix.to_string()),
            ("DESTDIR", destdir.display().to_string()),
            ("NAMI_BUNDLE_BUILD", build.display().to_string()),
            // Its own libraries, where it's installed (install() rules rarely say).
            ("LDFLAGS", format!("-Wl,-rpath,{prefix}/lib {}", std::env::var("LDFLAGS").unwrap_or_default())),
        ];
        let script = if self.kind == Kind::Meson {
            "if [ -d \"$NAMI_BUNDLE_BUILD/meson-private\" ]; then again=--reconfigure; else again=; fi\n\
             meson setup $again \"$NAMI_BUNDLE_BUILD\" . --prefix=\"$PREFIX\" --libdir=lib --buildtype=release\n\
             meson compile -C \"$NAMI_BUNDLE_BUILD\"\n\
             meson install -C \"$NAMI_BUNDLE_BUILD\" --destdir \"$DESTDIR\""
        } else {
            "cmake -S . -B \"$NAMI_BUNDLE_BUILD\" -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=\"$PREFIX\" -DCMAKE_INSTALL_LIBDIR=lib -DCMAKE_INSTALL_RPATH=\"$PREFIX/lib\"\n\
             cmake --build \"$NAMI_BUNDLE_BUILD\" --parallel \"${NIX_BUILD_CORES:-4}\"\n\
             cmake --install \"$NAMI_BUNDLE_BUILD\""
        };
        self.sh(&env, script, &vars, false)?;
        // Nothing installed in bin/ (no install rules): the programs it built.
        if programs_in(tree, None).is_empty() {
            let progs = built_programs(&build);
            if progs.is_empty() {
                bail!("the build installed no program, and made none nami could find in {}: give it install() rules, or say where it is with `bundle: output:` in project.nami", build.display());
            }
            fs::create_dir_all(tree.join("bin"))?;
            for p in &progs {
                fs::copy(p, tree.join("bin").join(p.file_name().unwrap()))?;
            }
            found.missing.push(format!(
                "its build installs nothing (no install() rules): took {} from the build folder; data files it would install aren't there",
                progs.iter().map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect::<Vec<_>>().join(", ")
            ));
        }
        let entry = choose_program(tree, self.program.as_deref(), &meta.name)?;
        Ok(Built { tree: tree.to_path_buf(), entry, gui: false })
    }

    fn build_custom(&self, meta: &Meta, tree: &Path, prefix: &str, work: &Path, destdir: &Path, found: &mut Found) -> Result<Built> {
        let env = self.env(&[])?;
        let script = self.settings.build.clone().unwrap_or_default();
        let vars = vec![("PREFIX", prefix.to_string()), ("DESTDIR", destdir.display().to_string()), ("NAMI_BUNDLE_WORK", work.display().to_string())];
        self.sh(&env, &script, &vars, false)?;
        if let Some(out) = &self.settings.output {
            let src = self.dir.join(out);
            ensure_exists(&src, "bundle: output:")?;
            if src.is_dir() {
                copy_tree(&src, tree)?;
            } else {
                let dest = tree.join("bin").join(src.file_name().unwrap());
                fs::create_dir_all(dest.parent().unwrap())?;
                fs::copy(&src, &dest)?;
            }
        } else if programs_in(tree, None).is_empty() {
            bail!("bundle: build: installed nothing in $DESTDIR$PREFIX: install there (make install PREFIX=$PREFIX DESTDIR=$DESTDIR), or say where the program is with `output:`");
        }
        let _ = found;
        let entry = choose_program(tree, self.program.as_deref(), &meta.name)?;
        Ok(Built { tree: tree.to_path_buf(), entry, gui: false })
    }

    fn stage_prebuilt(&self, meta: &Meta, tree: &Path, found: &mut Found) -> Result<Built> {
        copy_tree(&self.dir, tree)?;
        // A .desktop file beside the programs goes where menus look.
        let _ = found;
        let entry = choose_program(tree, self.program.as_deref(), &meta.name)?;
        Ok(Built { tree: tree.to_path_buf(), entry, gui: false })
    }

    /// The app menu entry and its icon: what the build installed, else
    /// what the flags, project.nami, Cargo.toml or the repo have.
    fn desktop_and_icon(&self, meta: &Meta, built: &Built, program: &str, found: &mut Found) -> Result<()> {
        let tree = &built.tree;
        let apps = tree.join("share/applications");
        let installed: Vec<PathBuf> = desktop_files(&apps);
        let cargo = meta.cargo.clone().unwrap_or_default();
        let given_desktop = self
            .hints
            .desktop
            .clone()
            .map(|d| (d, "--desktop-file"))
            .or_else(|| self.settings.desktop.as_ref().map(|d| (self.dir.join(d), "project.nami")))
            .or_else(|| cargo.desktop.as_ref().map(|d| (self.dir.join(d), cargo.file.as_str())));
        let given_icon = self
            .hints
            .icon
            .clone()
            .map(|i| (i, "--icon"))
            .or_else(|| self.settings.icon.as_ref().map(|i| (self.dir.join(i), "project.nami")))
            .or_else(|| cargo.icon.as_ref().map(|i| (self.dir.join(i), cargo.file.as_str())));
        let mut desktop: Option<PathBuf> = None;
        if let Some((d, from)) = &given_desktop {
            ensure_exists(d, from)?;
            let dest = apps.join(d.file_name().unwrap().to_string_lossy().trim_end_matches(".in").to_string());
            fs::create_dir_all(&apps)?;
            fs::write(&dest, fix_exec(&fs::read_to_string(d)?, program))?;
            found.add("desktop", &d.strip_prefix(&self.dir).unwrap_or(d).display().to_string(), from);
            desktop = Some(dest);
        } else if let Some(d) = pick_desktop(&installed, program) {
            found.add("desktop", &format!("share/applications/{}", d.file_name().unwrap().to_string_lossy()), "installed by its build");
            desktop = Some(d);
        } else if let Some(d) = self.repo_desktop(program) {
            let dest = apps.join(d.file_name().unwrap());
            fs::create_dir_all(&apps)?;
            fs::write(&dest, fix_exec(&fs::read_to_string(&d)?, program))?;
            found.add("desktop", &d.strip_prefix(&self.dir).unwrap_or(&d).display().to_string(), "in the repo");
            desktop = Some(dest);
        }
        // An icon, or a window, and no menu entry: one made for it (an app with an icon is one for the menu).
        if desktop.is_none() {
            if given_icon.is_some() || built.gui {
                let id = program.to_string();
                let display = cargo.display_name.clone().unwrap_or_else(|| meta.name.clone());
                let mut text = format!("[Desktop Entry]\nType=Application\nName={display}\nExec={program}\nIcon={id}\nTerminal=false\n");
                if let Some(c) = &cargo.description {
                    text += &format!("Comment={c}\n");
                }
                fs::create_dir_all(&apps)?;
                let dest = apps.join(format!("{id}.desktop"));
                fs::write(&dest, text)?;
                found.add("desktop", &format!("{id}.desktop"), if given_icon.is_some() { "made from the name and icon (Terminal=false)" } else { "made for its window (Terminal=false)" });
                desktop = Some(dest);
            }
        }
        let Some(desktop) = desktop else {
            found.add("desktop", "none", "");
            found.missing.push(
                "no .desktop file found, so no app menu entry (fine for a command-line tool). For an app with a window: put one in the repo, or say where with --desktop-file, or `bundle: desktop:` in project.nami".into(),
            );
            if given_icon.is_none() {
                found.add("icon", "none", "");
            }
            return Ok(());
        };
        let text = fs::read_to_string(&desktop)?;
        let icon_line = text.lines().find_map(|l| l.strip_prefix("Icon=")).map(|s| s.trim().to_string()).unwrap_or_default();
        // An absolute path names the developer's computer: the icon by its name instead.
        let icon_name = if icon_line.is_empty() {
            program.to_string()
        } else {
            Path::new(&icon_line).file_stem().unwrap_or_default().to_string_lossy().into_owned()
        };
        if icon_line != icon_name {
            let fixed: String = if icon_line.is_empty() {
                text.replacen("[Desktop Entry]\n", &format!("[Desktop Entry]\nIcon={icon_name}\n"), 1)
            } else {
                text.lines().map(|l| if l.starts_with("Icon=") { format!("Icon={icon_name}") } else { l.to_string() }).collect::<Vec<_>>().join("\n") + "\n"
            };
            fs::write(&desktop, fixed)?;
        }
        if let Some((icon, from)) = &given_icon {
            ensure_exists(icon, from)?;
            install_icon(icon, tree, &icon_name)?;
            found.add("icon", &icon.strip_prefix(&self.dir).unwrap_or(icon).display().to_string(), from);
        } else if let Some(have) = icon_in_tree(tree, &icon_name) {
            found.add("icon", &have, "installed by its build");
        } else if let Some(icon) = self.repo_icon(&icon_name) {
            install_icon(&icon, tree, &icon_name)?;
            found.add("icon", &icon.strip_prefix(&self.dir).unwrap_or(&icon).display().to_string(), "in the repo");
        } else {
            found.add("icon", "none", "");
            found.missing.push(format!("no icon named {icon_name} (.png or .svg) found in the repo: say which with --icon, or `bundle: icon:` in project.nami"));
        }
        Ok(())
    }

    /// The repo's .desktop file for `program` (or its only one).
    fn repo_desktop(&self, program: &str) -> Option<PathBuf> {
        let mut all = Vec::new();
        repo_files(&self.dir, 0, &mut |p| p.extension().is_some_and(|e| e == "desktop"), &mut all);
        all.sort();
        pick_desktop(&all, program)
    }

    /// An icon called `name` in the repo, biggest first, SVG best.
    fn repo_icon(&self, name: &str) -> Option<PathBuf> {
        let mut all = Vec::new();
        repo_files(&self.dir, 0, &mut |p| p.file_stem().is_some_and(|s| s.to_string_lossy() == name) && p.extension().is_some_and(|e| e == "png" || e == "svg"), &mut all);
        all.into_iter().max_by_key(|p| if p.extension().is_some_and(|e| e == "svg") { u32::MAX } else { png_size(p).unwrap_or(0) })
    }
}

/// The app's name and version, and Cargo.toml's say on the rest.
struct Meta {
    name: String,
    version: Option<String>,
    cargo: Option<CargoMeta>,
}

/// What the project's own files say: (value, where from).
#[derive(Default)]
struct OwnMeta {
    name: Option<(String, String)>,
    version: Option<(String, String)>,
    cargo: Option<CargoMeta>,
}

/// Cargo.toml's `[package.metadata.nami]` (or cargo-bundle's
/// `[package.metadata.bundle]`), and the package's description; for the
/// other kinds, what their own files say of the same (package.json's
/// productName, electron-builder's icon, ...).
#[derive(Debug, Clone, Default, PartialEq)]
struct CargoMeta {
    /// The file it's from, as said: Cargo.toml, package.json, ...
    file: String,
    package: String,
    display_name: Option<String>,
    description: Option<String>,
    icon: Option<String>,
    desktop: Option<String>,
}

fn cargo_own_meta(toml: &toml::Value, program: Option<&str>) -> OwnMeta {
    let mut m = OwnMeta::default();
    let Some(pkg) = toml.get("package") else {
        // A workspace: its members say (cargo metadata, at build time).
        return m;
    };
    let s = |v: Option<&toml::Value>| v.and_then(|v| v.as_str()).map(String::from);
    let name = s(pkg.get("name"));
    // A version from the workspace (version.workspace = true) isn't a string here.
    let version = s(pkg.get("version")).or_else(|| s(toml.get("workspace").and_then(|w| w.get("package")).and_then(|p| p.get("version"))));
    // ([package.metadata.tray], as projects said it before the rename, where there's no nami one.)
    let (nami, which) = match pkg.get("metadata").and_then(|m| m.get("nami")) {
        Some(n) => (Some(n), "nami"),
        None => (pkg.get("metadata").and_then(|m| m.get("tray")), "tray"),
    };
    let bundle = pkg.get("metadata").and_then(|m| m.get("bundle"));
    let icon = s(nami.and_then(|t| t.get("icon"))).or_else(|| {
        // cargo-bundle lists several sizes: the biggest (by its name), SVG best.
        let list = bundle.and_then(|b| b.get("icon")).and_then(|i| i.as_array())?;
        let names: Vec<String> = list.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        names.iter().find(|n| n.ends_with(".svg")).cloned().or_else(|| names.iter().max_by_key(|n| n.split(|c: char| !c.is_ascii_digit()).filter_map(|d| d.parse::<u32>().ok()).max().unwrap_or(0)).cloned())
    });
    let nami_name = s(nami.and_then(|t| t.get("name")));
    let bin = s(nami.and_then(|t| t.get("program"))).or(program.map(String::from));
    // The bundle is named after what runs: a program other than the package's name says so.
    m.name = nami_name.map(|n| (n, format!("Cargo.toml [package.metadata.{which}]"))).or_else(|| bin.clone().map(|b| (b, "the program".into()))).or_else(|| name.clone().map(|n| (n, "Cargo.toml".into())));
    m.version = version.map(|v| (v, "Cargo.toml".into()));
    m.cargo = Some(CargoMeta {
        file: "Cargo.toml".into(),
        package: name.unwrap_or_default(),
        display_name: s(bundle.and_then(|b| b.get("name"))),
        description: s(pkg.get("description")),
        icon,
        desktop: s(nami.and_then(|t| t.get("desktop"))),
    });
    m
}

/// A package's name less its version (python312, nodejs@22, electron_43:
/// python, nodejs, electron), to tell the project's own from nami's pick.
fn family(p: &str) -> &str {
    let p = p.split('@').next().unwrap_or(p);
    let t = p.trim_end_matches(|c: char| c.is_ascii_digit() || c == '_' || c == '.');
    if t.is_empty() {
        p
    } else {
        t
    }
}

/// From `cargo metadata`: the package and the program to build.
fn pick_cargo_bin(m: &serde_json::Value, dir: &Path, program: Option<&str>) -> Result<(String, String)> {
    let members: Vec<&serde_json::Value> = m["packages"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let bins = |p: &serde_json::Value| -> Vec<String> {
        p["targets"].as_array().into_iter().flatten().filter(|t| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "bin"))).filter_map(|t| t["name"].as_str().map(String::from)).collect()
    };
    let root = dir.join("Cargo.toml");
    let here = members.iter().find(|p| p["manifest_path"].as_str().is_some_and(|mp| Path::new(mp) == root));
    // A program by name, in any member.
    if let Some(want) = program {
        for p in members.iter().copied() {
            if bins(p).iter().any(|b| b == want) {
                return Ok((p["name"].as_str().unwrap_or_default().to_string(), want.to_string()));
            }
        }
        let all: Vec<String> = members.iter().flat_map(|p| bins(p)).collect();
        bail!("the project has no program {want} (it has: {})", if all.is_empty() { "none".into() } else { all.join(", ") });
    }
    let with_bins: Vec<&serde_json::Value> = match here {
        Some(p) if !bins(p).is_empty() => vec![p],
        _ => members.iter().copied().filter(|p| !bins(p).is_empty()).collect(),
    };
    match with_bins.as_slice() {
        [] => bail!("the project has no programs (no [[bin]], no src/main.rs): nami bundles programs, not libraries"),
        [p] => {
            let name = p["name"].as_str().unwrap_or_default().to_string();
            let b = bins(p);
            let pick = p["default_run"].as_str().map(String::from).or_else(|| b.iter().find(|x| **x == name).cloned()).or_else(|| (b.len() == 1).then(|| b[0].clone()));
            match pick {
                Some(bin) => Ok((name, bin)),
                None => bail!("{name} has several programs: say which runs with --program, or `bundle: program:` in project.nami (one of: {})", b.join(", ")),
            }
        }
        several => {
            let all: Vec<String> = several.iter().flat_map(|p| bins(p)).collect();
            bail!("the workspace has several programs: say which runs with --program, or `bundle: program:` in project.nami (one of: {})", all.join(", "))
        }
    }
}

/// go.mod's module: its last part is the program's name (`/v2` aside).
fn go_meta(dir: &Path) -> OwnMeta {
    let mut m = OwnMeta::default();
    if let Some(module) = fs::read_to_string(dir.join("go.mod")).ok().and_then(|t| go_module(&t)) {
        m.name = Some((go_name(&module), "go.mod's module".into()));
    }
    m
}

fn go_module(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.trim().strip_prefix("module ")).map(|m| m.trim().trim_matches('"').to_string())
}

fn go_name(module: &str) -> String {
    let parts: Vec<&str> = module.split('/').collect();
    let last = parts.last().copied().unwrap_or(module);
    let is_major = last.len() > 1 && last.starts_with('v') && last[1..].chars().all(|c| c.is_ascii_digit());
    if is_major && parts.len() > 1 {
        parts[parts.len() - 2].to_string()
    } else {
        last.to_string()
    }
}

/// The main package to build: `--program` (cmd/<it>), the folder itself,
/// or its one command under cmd/. (package, program name)
fn pick_go_main(dir: &Path, program: Option<&str>, name: &str) -> Result<(String, String)> {
    let cmds: Vec<String> = {
        let mut v: Vec<String> = fs::read_dir(dir.join("cmd")).into_iter().flatten().flatten().filter(|e| e.path().is_dir() && is_go_main(&e.path())).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    };
    if let Some(p) = program {
        if cmds.iter().any(|c| c == p) {
            return Ok((format!("./cmd/{p}"), p.to_string()));
        }
        if dir.join(p).is_dir() && is_go_main(&dir.join(p)) {
            let n = Path::new(p).file_name().unwrap_or_default().to_string_lossy().into_owned();
            return Ok((format!("./{}", p.trim_start_matches("./")), n));
        }
        if is_go_main(dir) && p == name {
            return Ok((".".into(), name.to_string()));
        }
        bail!("no Go program {p} here (it has: {})", if cmds.is_empty() { "the folder's own".to_string() } else { cmds.join(", ") });
    }
    if is_go_main(dir) {
        return Ok((".".into(), name.to_string()));
    }
    match cmds.as_slice() {
        [] => bail!("no Go program here (no package main in the folder, or under cmd/): say which folder with --program"),
        [one] => Ok((format!("./cmd/{one}"), one.clone())),
        _ if cmds.iter().any(|c| c == name) => Ok((format!("./cmd/{name}"), name.to_string())),
        _ => bail!("several Go programs under cmd/: say which with --program, or `bundle: program:` in project.nami (one of: {})", cmds.join(", ")),
    }
}

fn is_go_main(dir: &Path) -> bool {
    fs::read_dir(dir).into_iter().flatten().flatten().any(|e| {
        let p = e.path();
        p.extension().is_some_and(|x| x == "go")
            && !p.file_name().unwrap_or_default().to_string_lossy().ends_with("_test.go")
            && fs::read_to_string(&p).is_ok_and(|t| t.lines().any(|l| l.trim() == "package main"))
    })
}

/// meson.build's `project('name', 'c', version: '1.2')`.
fn meson_meta(dir: &Path) -> OwnMeta {
    let mut m = OwnMeta::default();
    let Ok(text) = fs::read_to_string(dir.join("meson.build")) else { return m };
    let text = strip_comments(&text, '#');
    let Some(args) = call_args(&text, "project") else { return m };
    let strings = quoted(&args, '\'');
    if let Some(n) = strings.first() {
        m.name = Some((n.clone(), "meson.build's project()".into()));
    }
    if let Some(at) = args.find("version") {
        let rest = args[at + 7..].trim_start();
        if let Some(rest) = rest.strip_prefix(':') {
            if let Some(v) = quoted(rest, '\'').first() {
                m.version = Some((v.clone(), "meson.build's project()".into()));
            }
        }
    }
    m
}

/// CMakeLists.txt's `project(name VERSION 1.2 ...)`.
fn cmake_meta(dir: &Path) -> OwnMeta {
    let mut m = OwnMeta::default();
    let Ok(text) = fs::read_to_string(dir.join("CMakeLists.txt")) else { return m };
    let text = strip_comments(&text, '#');
    let Some(args) = call_args(&text, "project") else { return m };
    let words: Vec<String> = args.split_whitespace().map(|w| w.trim_matches('"').to_string()).collect();
    if let Some(n) = words.first().filter(|n| !n.contains("${")) {
        m.name = Some((n.clone(), "CMakeLists.txt's project()".into()));
    }
    if let Some(i) = words.iter().position(|w| w == "VERSION") {
        if let Some(v) = words.get(i + 1).filter(|v| !v.contains("${")) {
            m.version = Some((v.clone(), "CMakeLists.txt's project()".into()));
        }
    }
    m
}

/// What a C/C++ project's build asks for, as catalog packages: CMake's
/// find_package() and pkg_check_modules(), Meson's dependency(). (found,
/// what nami doesn't know)
fn c_dependencies(index: &Index, dir: &Path, kind: Kind) -> (Vec<String>, Vec<String>) {
    let mut asked = Vec::new();
    let mut files = Vec::new();
    let file_name = if kind == Kind::Meson { "meson.build" } else { "CMakeLists.txt" };
    repo_files(dir, 0, &mut |p| p.file_name().is_some_and(|n| n == file_name), &mut files);
    for f in files {
        let Ok(text) = fs::read_to_string(&f) else { continue };
        let text = strip_comments(&text, '#');
        if kind == Kind::Meson {
            for args in all_call_args(&text, "dependency") {
                if let Some(first) = quoted(&args, '\'').first() {
                    asked.push(first.clone());
                }
            }
        } else {
            for args in all_call_args(&text, "find_package") {
                if let Some(first) = args.split_whitespace().next() {
                    asked.push(first.trim_matches('"').to_string());
                }
            }
            for args in all_call_args(&text, "pkg_check_modules").into_iter().chain(all_call_args(&text, "pkg_search_module")) {
                // pkg_check_modules(PREFIX [REQUIRED] [IMPORTED_TARGET] gtk4 glib-2.0>=2.70)
                for w in args.split_whitespace().skip(1) {
                    let w = w.trim_matches('"');
                    if w.chars().all(|c| c.is_ascii_uppercase() || c == '_') || w.contains("${") {
                        continue;
                    }
                    asked.push(w.split(['>', '<', '=']).next().unwrap_or(w).to_string());
                }
            }
        }
    }
    let mut found = BTreeSet::new();
    let mut unknown = BTreeSet::new();
    for a in asked {
        match package_for(index, &a) {
            Some(Some(p)) => {
                found.insert(p);
            }
            Some(None) => {}
            None => {
                unknown.insert(a);
            }
        }
    }
    (found.into_iter().collect(), unknown.into_iter().collect())
}

/// Rust crates that open libraries at run time (dlopen) rather than link
/// them, and the catalog's packages with those libraries. The program
/// doesn't name them, so without this they'd be missing from the bundle
/// (winit's "libX11.so.6: cannot open shared object file"); nixpkgs adds
/// the same to such programs' RUNPATH.
const DLOPENED: &[(&str, &[&str])] = &[
    // winit's X11: Xlib, Xcursor, XInput 2, Xlib-xcb; x11-dl opens the rest of these too.
    ("x11-dl", &["libx11", "libxcursor", "libxi", "libxrandr", "libxinerama", "libxxf86vm", "libxext", "libxrender"]),
    ("x11rb", &["libxcb"]),
    ("xkbcommon-dl", &["libxkbcommon"]),
    ("wayland-sys", &["wayland"]),
    // Vulkan (wgpu, vulkano, ash), EGL and GLX (wgpu's GLES, glutin).
    ("ash", &["vulkan-loader"]),
    ("khronos-egl", &["libglvnd"]),
    ("glutin_egl_sys", &["libglvnd"]),
    ("glutin_glx_sys", &["libglvnd"]),
    ("yeslogic-fontconfig-sys", &["fontconfig"]),
];

/// What the project's crates (its Cargo.lock) open at run time.
fn dlopened(dir: &Path) -> Vec<(&'static str, &'static [&'static str])> {
    let Some(lock) = dir.ancestors().map(|d| d.join("Cargo.lock")).find(|p| p.is_file()) else { return Vec::new() };
    let Ok(text) = fs::read_to_string(&lock) else { return Vec::new() };
    let Ok(v) = toml::from_str::<toml::Value>(&text) else { return Vec::new() };
    let names: BTreeSet<&str> = v.get("package").and_then(|p| p.as_array()).into_iter().flatten().filter_map(|p| p.get("name").and_then(|n| n.as_str())).collect();
    DLOPENED.iter().filter(|(c, _)| names.contains(c)).copied().collect()
}

/// The catalog package for what a build asks for by its CMake or
/// pkg-config name. Some(None): nothing to get (the C library, the build
/// tools, CMake's own modules); None: nami doesn't know.
fn package_for(index: &Index, asked: &str) -> Option<Option<String>> {
    let lower = asked.to_lowercase();
    let builtin = ["threads", "m", "dl", "rt", "pkgconfig", "gnuinstalldirs", "cmakepackageconfighelpers", "openmp", "python3", "python", "git", "doxygen", "gettext", "intl", "iconv"];
    if builtin.contains(&lower.as_str()) {
        return Some(None);
    }
    let known: &[(&str, &str)] = &[
        ("zlib", "zlib"),
        ("png", "libpng"),
        ("libpng", "libpng"),
        ("jpeg", "libjpeg"),
        ("libjpeg", "libjpeg"),
        ("curl", "curl"),
        ("libcurl", "curl"),
        ("openssl", "openssl"),
        ("sdl2", "SDL2"),
        ("sdl3", "sdl3"),
        ("glfw3", "glfw"),
        ("glfw", "glfw"),
        ("opengl", "libGL"),
        ("gl", "libGL"),
        ("egl", "libGL"),
        ("glew", "glew"),
        ("vulkan", "vulkan-loader"),
        ("x11", "xorg.libX11"),
        ("wayland-client", "wayland"),
        ("freetype", "freetype"),
        ("freetype2", "freetype"),
        ("fontconfig", "fontconfig"),
        ("gtk4", "gtk4"),
        ("gtk+-3.0", "gtk3"),
        ("gtk3", "gtk3"),
        ("glib-2.0", "glib"),
        ("gio-2.0", "glib"),
        ("gobject-2.0", "glib"),
        ("gmodule-2.0", "glib"),
        ("libadwaita-1", "libadwaita"),
        ("cairo", "cairo"),
        ("pango", "pango"),
        ("libxml-2.0", "libxml2"),
        ("libxml2", "libxml2"),
        ("sqlite3", "sqlite"),
        ("sqlite", "sqlite"),
        ("ncurses", "ncurses"),
        ("curses", "ncurses"),
        ("readline", "readline"),
        ("bzip2", "bzip2"),
        ("liblzma", "xz"),
        ("lzma", "xz"),
        ("libzstd", "zstd"),
        ("zstd", "zstd"),
        ("libuv", "libuv"),
        ("fmt", "fmt"),
        ("boost", "boost"),
        ("eigen3", "eigen"),
        ("libarchive", "libarchive"),
        ("expat", "expat"),
        ("libffi", "libffi"),
        ("json-glib-1.0", "json-glib"),
        ("libsoup-3.0", "libsoup_3"),
        ("qt5", "qt5.qtbase"),
        ("qt6", "qt6.qtbase"),
    ];
    let candidates: Vec<String> = known.iter().filter(|(k, _)| *k == lower).map(|(_, p)| p.to_string()).chain([asked.to_string(), lower.clone(), format!("lib{lower}")]).collect();
    // `foo-1.0` is foo, too.
    let trimmed = lower.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.').trim_end_matches('-').to_string();
    for c in candidates.into_iter().chain([trimmed]) {
        if !c.is_empty() && index.get(&c).is_ok() {
            return Some(Some(c));
        }
    }
    None
}

/// The arguments of the first call to `name(...)` (case-insensitive).
fn call_args(text: &str, name: &str) -> Option<String> {
    all_call_args(text, name).into_iter().next()
}

fn all_call_args(text: &str, name: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find(name) {
        let start = from + at;
        from = start + name.len();
        // A whole word: not some_project( or project_name(.
        if lower[..start].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let rest = &text[from..];
        let Some(open) = rest.find(|c: char| !c.is_whitespace()).filter(|i| rest[*i..].starts_with('(')) else { continue };
        let mut depth = 0;
        let mut end = None;
        for (i, c) in rest[open..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        if let Some(e) = end {
            out.push(rest[open + 1..e].to_string());
        }
    }
    out
}

/// The strings in `text` quoted with `q`.
fn quoted(text: &str, q: char) -> Vec<String> {
    text.split(q).skip(1).step_by(2).map(String::from).collect()
}

fn strip_comments(text: &str, c: char) -> String {
    text.lines().map(|l| l.split(c).next().unwrap_or("")).collect::<Vec<_>>().join("\n")
}

/// `git describe --tags`, when the project is a repo of its own (not a
/// folder in someone else's), less a leading v.
fn git_version(dir: &Path) -> Option<String> {
    let top = Command::new("git").args(["rev-parse", "--show-toplevel"]).current_dir(dir).stderr(Stdio::null()).output().ok()?;
    if !top.status.success() || fs::canonicalize(String::from_utf8_lossy(&top.stdout).trim()).ok()? != fs::canonicalize(dir).ok()? {
        return None;
    }
    let out = Command::new("git").args(["describe", "--tags", "--dirty"]).current_dir(dir).stderr(Stdio::null()).output().ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !v.is_empty()).then(|| v.strip_prefix('v').unwrap_or(&v).to_string())
}

/// `myapp-1.2.3-linux-x86_64.tar.gz` -> ("myapp", Some("1.2.3")).
pub fn archive_name(p: &Path) -> (String, Option<String>) {
    let mut n = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
    for e in [".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2", ".tar.zst", ".tar", ".zip", ".gz", ".xz"] {
        if let Some(s) = n.strip_suffix(e) {
            n = s.to_string();
            break;
        }
    }
    let noise = ["linux", "x86_64", "x86-64", "amd64", "x64", "64bit", "gnu", "musl", "glibc", "static", "unknown", "pc", "bin", "release", "portable"];
    let parts: Vec<&str> = n.split(['-', '_']).collect();
    let mut name = Vec::new();
    let mut version = None;
    let mut i = 0;
    while i < parts.len() {
        let w = parts[i];
        let lw = w.to_lowercase();
        let is_version = w.chars().next().is_some_and(|c| c.is_ascii_digit()) || (w.len() > 1 && w.starts_with('v') && w[1..].starts_with(|c: char| c.is_ascii_digit()));
        if version.is_none() && is_version && !name.is_empty() {
            version = Some(w.trim_start_matches('v').to_string());
        } else if noise.contains(&lw.as_str()) || (lw == "x86" && parts.get(i + 1) == Some(&"64")) {
            if lw == "x86" {
                i += 1;
            }
        } else if version.is_none() {
            name.push(w);
        }
        i += 1;
    }
    (name.join("-"), version)
}

/// A store path's name for the app: `<name>-<version>`, in letters Nix
/// takes in a store path name.
fn store_label(name: &str, version: Option<&str>) -> String {
    let full = match version {
        Some(v) if !v.is_empty() => format!("{name}-{v}"),
        _ => name.to_string(),
    };
    let s: String = full.chars().map(|c| if c.is_ascii_alphanumeric() || "+-._?=".contains(c) { c } else { '-' }).collect();
    let s = s.trim_start_matches('.').to_string();
    if s.is_empty() {
        "app".into()
    } else {
        s
    }
}

/// 32 letters of Nix's alphabet, from a hash of `data`.
fn hash32(data: &[u8]) -> String {
    use sha2::Digest;
    let h = sha2::Sha256::digest(data);
    crate::nix::base32::encode(&h[..20])
}

/// Every `from` becomes `to` (as long), in each file and link under `dir`.
fn rewrite_tree(dir: &Path, from: &[u8], to: &[u8]) -> Result<()> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        let m = fs::symlink_metadata(&p)?;
        if m.is_dir() {
            for e in fs::read_dir(&p)? {
                stack.push(e?.path());
            }
        } else if m.file_type().is_symlink() {
            let target = fs::read_link(&p)?;
            let mut t = target.as_os_str().as_encoded_bytes().to_vec();
            if crate::bundle_format::replace_all(&mut t, from, to) > 0 {
                fs::remove_file(&p)?;
                std::os::unix::fs::symlink(String::from_utf8_lossy(&t).as_ref(), &p)?;
            }
        } else if m.is_file() {
            let mut data = fs::read(&p)?;
            if crate::bundle_format::replace_all(&mut data, from, to) > 0 {
                let mode = m.permissions().mode();
                fs::set_permissions(&p, fs::Permissions::from_mode(mode | 0o200))?;
                fs::write(&p, &data)?;
                fs::set_permissions(&p, fs::Permissions::from_mode(mode))?;
            }
        }
    }
    Ok(())
}

/// A copy of `src` in `dest`, links as links.
fn copy_tree(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for e in fs::read_dir(src)? {
        let e = e?;
        let (from, to) = (e.path(), dest.join(e.file_name()));
        let m = fs::symlink_metadata(&from)?;
        if m.file_type().is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&from)?, &to)?;
        } else if m.is_dir() {
            // Not the repo's own bookkeeping.
            if e.file_name() == ".git" {
                continue;
            }
            copy_tree(&from, &to)?;
        } else if m.is_file() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn ensure_exists(p: &Path, from: &str) -> Result<()> {
    if !p.exists() {
        bail!("{} isn't there (from {from})", p.display());
    }
    Ok(())
}

/// Programs in a folder: bin/'s, else the folder's own (64-bit Linux
/// programs and scripts, executable). By path inside it.
fn programs_in(dir: &Path, program: Option<&str>) -> Vec<String> {
    let look = |sub: &str| -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir.join(sub))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| is_program(&e.path()))
            .map(|e| if sub.is_empty() { e.file_name().to_string_lossy().into_owned() } else { format!("{sub}/{}", e.file_name().to_string_lossy()) })
            .collect();
        v.sort();
        v
    };
    let mut found = look("bin");
    if found.is_empty() {
        found = look("");
    }
    if let Some(p) = program {
        if dir.join(p).is_file() && is_program(&dir.join(p)) && !found.iter().any(|f| f == p) {
            found.push(p.to_string());
        }
    }
    found
}

fn is_program(p: &Path) -> bool {
    let Ok(m) = fs::metadata(p) else { return false };
    if !m.is_file() || m.permissions().mode() & 0o111 == 0 {
        return false;
    }
    let mut head = vec![0u8; 4096];
    let n = fs::File::open(p).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).unwrap_or(0);
    head.truncate(n);
    let name = p.file_name().unwrap_or_default().to_string_lossy();
    if name.contains(".so") {
        return false;
    }
    if head.starts_with(b"#!") {
        return true;
    }
    // A program (with a loader, or static), not a library: e_type EXEC, or DYN with a loader.
    if head.len() < 64 || &head[..4] != b"\x7fELF" || head[4] != 2 {
        return false;
    }
    let data = fs::read(p).unwrap_or_default();
    matches!(crate::other_linux::kind_of(&data), crate::other_linux::Kind::Native | crate::other_linux::Kind::Foreign)
}

/// Programs a build made (CMake's or Meson's build folder), when it
/// installed none.
fn built_programs(build: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![build.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            let n = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() && !p.is_symlink() {
                if n != "CMakeFiles" && n != "meson-private" && n != "meson-logs" && !n.ends_with(".p") {
                    stack.push(p);
                }
            } else if is_program(&p) && p.extension().is_none() && fs::read(&p).is_ok_and(|d| d.starts_with(b"\x7fELF")) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Which program runs: the one asked for, the one named after the app,
/// or the only one. Its path inside `tree`.
fn choose_program(tree: &Path, program: Option<&str>, name: &str) -> Result<String> {
    let progs = programs_in(tree, program);
    let base = |p: &String| p.rsplit('/').next().unwrap_or(p).to_string();
    if let Some(want) = program {
        return progs.iter().find(|p| *p == want || base(p) == want).cloned().with_context(|| format!("no program {want} here (there's: {})", if progs.is_empty() { "none".into() } else { progs.join(", ") }));
    }
    match progs.as_slice() {
        [] => bail!("no program found (in bin/ or the top folder)"),
        [one] => Ok(one.clone()),
        _ => progs.iter().find(|p| base(p) == name || base(p).to_lowercase() == name.to_lowercase()).cloned().with_context(|| {
            format!("several programs: say which runs with --program, or `bundle: program:` in project.nami (one of: {})", progs.iter().map(base).collect::<Vec<_>>().join(", "))
        }),
    }
}

/// The repo's files matching `want`, a few folders down, not in build
/// folders or hidden ones.
fn repo_files(dir: &Path, depth: usize, want: &mut dyn FnMut(&Path) -> bool, out: &mut Vec<PathBuf>) {
    if depth > 4 {
        return;
    }
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        let n = e.file_name().to_string_lossy().into_owned();
        let Ok(m) = fs::symlink_metadata(&p) else { continue };
        if m.is_dir() {
            if n.starts_with('.') || ["target", "build", "builddir", "node_modules", "vendor", "_build", "dist", "out"].contains(&n.as_str()) {
                continue;
            }
            repo_files(&p, depth + 1, want, out);
        } else if m.is_file() && want(&p) {
            out.push(p);
        }
    }
}

fn desktop_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "desktop")).collect();
    v.sort();
    v
}

/// The .desktop file that starts `program` (or the only one there is).
fn pick_desktop(files: &[PathBuf], program: &str) -> Option<PathBuf> {
    let runs = |f: &PathBuf| {
        fs::read_to_string(f).is_ok_and(|t| t.lines().filter_map(|l| l.strip_prefix("Exec=")).any(|e| e.split_whitespace().next().and_then(|p| p.rsplit('/').next()) == Some(program)))
    };
    files.iter().find(|f| runs(f)).cloned().or_else(|| (files.len() == 1).then(|| files[0].clone()))
}

/// Exec= starts `program` (the bundle rewrites it to itself), its
/// arguments kept.
fn fix_exec(text: &str, program: &str) -> String {
    let mut out: String = text
        .lines()
        .map(|l| match l.strip_prefix("Exec=") {
            Some(e) => format!("Exec={program}{}", e.split_once(' ').map(|(_, r)| format!(" {r}")).unwrap_or_default()),
            None => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}

/// A PNG's width, from its header.
fn png_size(p: &Path) -> Option<u32> {
    let mut head = [0u8; 24];
    std::io::Read::read_exact(&mut fs::File::open(p).ok()?, &mut head).ok()?;
    (&head[..8] == b"\x89PNG\r\n\x1a\n").then(|| u32::from_be_bytes([head[16], head[17], head[18], head[19]]))
}

/// Where an icon goes so menus (and bundle.rs) find it: SVG as scalable,
/// a PNG by its size, else pixmaps.
fn icon_dest(icon: &Path, name: &str) -> String {
    if icon.extension().is_some_and(|e| e == "svg") {
        return format!("share/icons/hicolor/scalable/apps/{name}.svg");
    }
    match png_size(icon) {
        Some(s) if [48, 64, 96, 128, 192, 256, 512].contains(&s) => format!("share/icons/hicolor/{s}x{s}/apps/{name}.png"),
        _ => format!("share/pixmaps/{name}.png"),
    }
}

fn install_icon(icon: &Path, tree: &Path, name: &str) -> Result<()> {
    let dest = tree.join(icon_dest(icon, name));
    fs::create_dir_all(dest.parent().unwrap())?;
    fs::copy(icon, &dest).with_context(|| format!("copying the icon {}", icon.display()))?;
    Ok(())
}

fn icon_in_tree(tree: &Path, name: &str) -> Option<String> {
    let mut found = Vec::new();
    repo_files(&tree.join("share"), 0, &mut |p| p.file_stem().is_some_and(|s| s.to_string_lossy() == name) && p.extension().is_some_and(|e| e == "png" || e == "svg"), &mut found);
    found.first().map(|p| p.strip_prefix(tree).unwrap_or(p).display().to_string())
}

/// A tarball, unpacked in nami's cache: its one folder, if that's all it has.
fn unpack(store: &Store, archive: &Path) -> Result<PathBuf> {
    let archive = archive.canonicalize()?;
    let dir = work_dir(store, &archive)?.join("unpacked");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    let zip = archive.extension().is_some_and(|e| e == "zip");
    let status = if zip {
        Command::new("unzip").arg("-q").arg(&archive).arg("-d").arg(&dir).status()
    } else {
        Command::new("tar").arg("-xf").arg(&archive).arg("-C").arg(&dir).status()
    }
    .with_context(|| format!("unpacking {} needs {}", archive.display(), if zip { "unzip" } else { "tar" }))?;
    if !status.success() {
        bail!("couldn't unpack {}", archive.display());
    }
    let entries: Vec<PathBuf> = fs::read_dir(&dir)?.flatten().map(|e| e.path()).collect();
    match entries.as_slice() {
        [one] if one.is_dir() => Ok(one.clone()),
        _ => Ok(dir),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nami-bundle-project-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn detects_the_kind_of_project() {
        let d = tmp("detect");
        let none = Settings::default();
        assert_eq!(detect(&d, &none, None), None);
        fs::write(d.join("CMakeLists.txt"), "project(x)").unwrap();
        assert_eq!(detect(&d, &none, None), Some(Kind::CMake));
        fs::write(d.join("meson.build"), "project('x', 'c')").unwrap();
        assert_eq!(detect(&d, &none, None), Some(Kind::Meson));
        fs::write(d.join("go.mod"), "module example.com/x\n").unwrap();
        assert_eq!(detect(&d, &none, None), Some(Kind::Go));
        fs::write(d.join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
        assert_eq!(detect(&d, &none, None), Some(Kind::Rust));
        let custom = Settings { build: Some("make".into()), ..Default::default() };
        assert_eq!(detect(&d, &custom, None), Some(Kind::Custom));
        // Programs already built.
        let p = tmp("prebuilt");
        fs::create_dir_all(p.join("bin")).unwrap();
        fs::write(p.join("bin/app"), "#!/bin/sh\necho hi\n").unwrap();
        fs::set_permissions(p.join("bin/app"), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(detect(&p, &none, None), Some(Kind::Prebuilt));
        assert_eq!(choose_program(&p, None, "whatever").unwrap(), "bin/app");
    }

    #[test]
    fn finds_what_crates_open_at_run_time() {
        let d = tmp("dlopen");
        assert!(dlopened(&d).is_empty(), "no Cargo.lock");
        fs::write(d.join("Cargo.lock"), "version = 4\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\n").unwrap();
        assert!(dlopened(&d).is_empty(), "nothing that opens libraries");
        // An egui app's: winit (X11 and Wayland) and wgpu (Vulkan, EGL).
        let lock = ["serde", "winit", "x11-dl", "x11rb", "xkbcommon-dl", "wayland-sys", "wgpu-hal", "ash", "khronos-egl"]
            .iter()
            .map(|n| format!("[[package]]\nname = \"{n}\"\nversion = \"0.1.0\"\n"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(d.join("Cargo.lock"), format!("version = 4\n\n{lock}")).unwrap();
        let found: Vec<&str> = dlopened(&d).iter().map(|(c, _)| *c).collect();
        assert_eq!(found, ["x11-dl", "x11rb", "xkbcommon-dl", "wayland-sys", "ash", "khronos-egl"]);
        let packages: BTreeSet<&str> = dlopened(&d).iter().flat_map(|(_, p)| p.iter().copied()).collect();
        for p in ["libx11", "libxcursor", "libxi", "libxcb", "libxkbcommon", "wayland", "vulkan-loader", "libglvnd"] {
            assert!(packages.contains(p), "{p}: {packages:?}");
        }
        // A crate of a workspace: the lock is at the workspace's top.
        fs::create_dir_all(d.join("crates/app")).unwrap();
        assert_eq!(dlopened(&d.join("crates/app")).len(), 6);
    }

    #[test]
    fn reads_cargo_toml() {
        let t: toml::Value = toml::from_str(
            r#"
            [package]
            name = "hello-gui"
            version = "0.4.2"
            description = "Says hello"
            [package.metadata.bundle]
            name = "Hello GUI"
            icon = ["icons/32x32.png", "icons/256x256.png", "icons/128x128.png"]
            "#,
        )
        .unwrap();
        let m = cargo_own_meta(&t, None);
        assert_eq!(m.name.unwrap().0, "hello-gui");
        assert_eq!(m.version.unwrap().0, "0.4.2");
        let c = m.cargo.unwrap();
        assert_eq!(c.icon.as_deref(), Some("icons/256x256.png"));
        assert_eq!(c.display_name.as_deref(), Some("Hello GUI"));
        assert_eq!(c.description.as_deref(), Some("Says hello"));
        // [package.metadata.nami] wins.
        let t: toml::Value = toml::from_str("[package]\nname='a'\nversion='1.0.0'\n[package.metadata.nami]\nname='b'\nicon='x.svg'\ndesktop='a.desktop'\n").unwrap();
        let m = cargo_own_meta(&t, None);
        assert_eq!(m.name.unwrap().0, "b");
        assert_eq!(m.cargo.as_ref().unwrap().icon.as_deref(), Some("x.svg"));
        assert_eq!(m.cargo.unwrap().desktop.as_deref(), Some("a.desktop"));
        // A project's from before the rename: [package.metadata.tray], read as it was.
        let t: toml::Value = toml::from_str("[package]\nname='a'\nversion='1.0.0'\n[package.metadata.tray]\nname='c'\nicon='y.svg'\n").unwrap();
        let m = cargo_own_meta(&t, None);
        assert_eq!(m.name.unwrap(), ("c".to_string(), "Cargo.toml [package.metadata.tray]".to_string()));
        assert_eq!(m.cargo.unwrap().icon.as_deref(), Some("y.svg"));
    }

    #[test]
    fn picks_the_cargo_program() {
        let dir = Path::new("/p");
        let m: serde_json::Value = serde_json::json!({"packages": [
            {"name": "app", "manifest_path": "/p/Cargo.toml", "default_run": null, "targets": [
                {"kind": ["lib"], "name": "app"}, {"kind": ["bin"], "name": "app"}, {"kind": ["bin"], "name": "helper"}]},
        ]});
        assert_eq!(pick_cargo_bin(&m, dir, None).unwrap(), ("app".into(), "app".into()));
        assert_eq!(pick_cargo_bin(&m, dir, Some("helper")).unwrap(), ("app".into(), "helper".into()));
        assert!(pick_cargo_bin(&m, dir, Some("nope")).unwrap_err().to_string().contains("app, helper"));
        // A workspace with two programs: which, said.
        let ws: serde_json::Value = serde_json::json!({"packages": [
            {"name": "a", "manifest_path": "/p/a/Cargo.toml", "targets": [{"kind": ["bin"], "name": "a"}]},
            {"name": "b", "manifest_path": "/p/b/Cargo.toml", "targets": [{"kind": ["bin"], "name": "b"}]},
        ]});
        assert!(pick_cargo_bin(&ws, dir, None).unwrap_err().to_string().contains("--program"));
        assert_eq!(pick_cargo_bin(&ws, dir, Some("b")).unwrap(), ("b".into(), "b".into()));
    }

    #[test]
    fn reads_go_meson_and_cmake_projects() {
        assert_eq!(go_module("// x\nmodule github.com/acme/tool/v2\n\ngo 1.22\n").as_deref(), Some("github.com/acme/tool/v2"));
        assert_eq!(go_name("github.com/acme/tool/v2"), "tool");
        assert_eq!(go_name("hello"), "hello");
        let d = tmp("meson");
        fs::write(d.join("meson.build"), "# a comment\nproject('hello-meson', 'c',\n  version : '0.9.2',\n  license: 'MIT')\nzdep = dependency('zlib')\n").unwrap();
        let m = meson_meta(&d);
        assert_eq!(m.name.unwrap().0, "hello-meson");
        assert_eq!(m.version.unwrap().0, "0.9.2");
        let d = tmp("cmake");
        fs::write(d.join("CMakeLists.txt"), "cmake_minimum_required(VERSION 3.16)\nPROJECT(cmake-hello\n  VERSION 1.4.0\n  LANGUAGES C)\nfind_package(ZLIB REQUIRED)\npkg_check_modules(GTK REQUIRED IMPORTED_TARGET gtk4>=4.10)\n").unwrap();
        let m = cmake_meta(&d);
        assert_eq!(m.name.unwrap().0, "cmake-hello");
        assert_eq!(m.version.unwrap().0, "1.4.0");
        let text = fs::read_to_string(d.join("CMakeLists.txt")).unwrap();
        assert_eq!(all_call_args(&text, "find_package"), vec!["ZLIB REQUIRED"]);
        assert_eq!(all_call_args(&text, "pkg_check_modules"), vec!["GTK REQUIRED IMPORTED_TARGET gtk4>=4.10"]);
    }

    #[test]
    fn finds_main_go_packages() {
        let d = tmp("go");
        fs::create_dir_all(d.join("cmd/tool")).unwrap();
        fs::write(d.join("cmd/tool/main.go"), "package main\nfunc main() {}\n").unwrap();
        fs::write(d.join("lib.go"), "package tool\n").unwrap();
        assert_eq!(pick_go_main(&d, None, "x").unwrap(), ("./cmd/tool".into(), "tool".into()));
        fs::create_dir_all(d.join("cmd/other")).unwrap();
        fs::write(d.join("cmd/other/main.go"), "package main\n").unwrap();
        assert!(pick_go_main(&d, None, "x").unwrap_err().to_string().contains("other, tool"));
        assert_eq!(pick_go_main(&d, Some("other"), "x").unwrap(), ("./cmd/other".into(), "other".into()));
        assert_eq!(pick_go_main(&d, None, "tool").unwrap(), ("./cmd/tool".into(), "tool".into()));
    }

    #[test]
    fn names_from_a_tarball() {
        assert_eq!(archive_name(Path::new("myapp-1.2.3-linux-x86_64.tar.gz")), ("myapp".into(), Some("1.2.3".into())));
        assert_eq!(archive_name(Path::new("ripgrep-14.1.0-x86_64-unknown-linux-musl.tar.gz")), ("ripgrep".into(), Some("14.1.0".into())));
        assert_eq!(archive_name(Path::new("tool_v0.5.0_linux_amd64.tar.gz")), ("tool".into(), Some("0.5.0".into())));
        assert_eq!(archive_name(Path::new("some-tool.tgz")), ("some-tool".into(), None));
    }

    #[test]
    fn bundle_settings() {
        let v: Value = serde_yaml::from_str("name: hello\nversion: 1.10\nprogram: hi\n").unwrap();
        let s = Settings::from_value(&v, "project.nami").unwrap();
        assert_eq!(s.name.as_deref(), Some("hello"));
        assert_eq!(s.program.as_deref(), Some("hi"));
        let bad: Value = serde_yaml::from_str("nmae: x\n").unwrap();
        assert!(Settings::from_value(&bad, "project.nami").unwrap_err().to_string().contains("did you mean `name`"));
    }

    #[test]
    fn desktop_files_and_icons() {
        assert_eq!(fix_exec("[Desktop Entry]\nExec=/usr/bin/hello --gui %U\nName=Hello", "hello"), "[Desktop Entry]\nExec=hello --gui %U\nName=Hello\n");
        assert_eq!(icon_dest(Path::new("a.svg"), "x"), "share/icons/hicolor/scalable/apps/x.svg");
        assert_eq!(icon_dest(Path::new("/nonexistent.png"), "x"), "share/pixmaps/x.png");
        assert_eq!(store_label("My App", Some("1.0")), "My-App-1.0");
        assert_eq!(hash32(b"x").len(), 32);
        // The stand-in is replaced in files and links.
        let d = tmp("rewrite");
        fs::write(d.join("f"), b"before /nix/store/AAAA-x/share after").unwrap();
        std::os::unix::fs::symlink("/nix/store/AAAA-x/lib", d.join("l")).unwrap();
        rewrite_tree(&d, b"AAAA", b"BBBB").unwrap();
        assert_eq!(fs::read(d.join("f")).unwrap(), b"before /nix/store/BBBB-x/share after");
        assert_eq!(fs::read_link(d.join("l")).unwrap(), PathBuf::from("/nix/store/BBBB-x/lib"));
    }
}
