//! `nami bundle .` for Python, Node.js and Electron, Zig, Flutter and .NET
//! projects: a child of bundle_project.rs, which does the rest (the store
//! path, the name and version, the menu entry and icon).
//!
//! Each is built as the four first are: with the catalog's tools, through
//! the project's own `nami run` environment, the version its files ask for
//! when they say (requires-python, engines.node, Electron's version,
//! minimum_zig_version, TargetFramework). What runs is the catalog's
//! interpreter or runtime with the project's own files:
//!
//! - **Python**: the project and what it depends on (uv, from PyPI, or its
//!   uv.lock) in `lib/app`, compiled to hash-checked .pyc files (the
//!   bundle's files all have the same date), and a script per entry point
//!   that starts the catalog's Python on it.
//! - **Node.js**: its `bin`, on the catalog's Node, with only what it needs
//!   to run (its dependencies, not its devDependencies).
//! - **Electron**: the app folder on the catalog's Electron (the one of the
//!   major version it asks for), as nixpkgs runs Element or Signal.
//! - **Zig**: `zig build --release=safe`, installed.
//! - **Flutter**: `flutter build linux`'s bundle folder.
//! - **.NET**: `dotnet publish`, on the catalog's .NET runtime of the
//!   version its TargetFramework says.

use super::*;
use anyhow::ensure;

/// The kind of project, from the files nami knows past the first four.
pub(super) fn detect(dir: &Path) -> Option<Kind> {
    let has = |f: &str| dir.join(f).is_file();
    if has("pyproject.toml") && read_toml(&dir.join("pyproject.toml")).is_some_and(|t| t.get("project").is_some() || t.get("tool").and_then(|t| t.get("poetry")).is_some()) {
        Some(Kind::Python)
    } else if has("pubspec.yaml") && dir.join("linux").is_dir() {
        Some(Kind::Flutter)
    } else if has("build.zig") {
        Some(Kind::Zig)
    } else if !dotnet_projects(dir).is_empty() {
        Some(Kind::DotNet)
    } else if has("package.json") {
        let p = read_json(&dir.join("package.json")).unwrap_or_default();
        Some(if electron_wanted(&p).is_some() { Kind::Electron } else { Kind::Node })
    } else {
        None
    }
}

fn read_toml(p: &Path) -> Option<toml::Value> {
    toml::from_str(&fs::read_to_string(p).ok()?).ok()
}

fn read_json(p: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&fs::read_to_string(p).ok()?).ok()
}

fn tstr(v: Option<&toml::Value>) -> Option<String> {
    v.and_then(|v| v.as_str()).map(String::from)
}

fn jstr(v: &serde_json::Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(String::from)
}

// ---------- Python ----------

/// pyproject.toml's [project] (or Poetry's [tool.poetry]), and [tool.nami]
/// for what it has no key for (icon, desktop, name).
pub(super) fn python_meta(dir: &Path) -> OwnMeta {
    let mut m = OwnMeta::default();
    let Some(t) = read_toml(&dir.join("pyproject.toml")) else { return m };
    let project = t.get("project").or_else(|| t.get("tool").and_then(|t| t.get("poetry")));
    let nami = t.get("tool").and_then(|t| t.get("nami"));
    let name = tstr(project.and_then(|p| p.get("name")));
    m.name = tstr(nami.and_then(|n| n.get("name"))).map(|n| (n, "pyproject.toml [tool.nami]".into())).or_else(|| name.clone().map(|n| (n, "pyproject.toml".into())));
    m.version = tstr(project.and_then(|p| p.get("version"))).map(|v| (v, "pyproject.toml".into()));
    m.cargo = Some(CargoMeta {
        file: "pyproject.toml".into(),
        package: name.unwrap_or_default(),
        display_name: None,
        description: tstr(project.and_then(|p| p.get("description"))),
        icon: tstr(nami.and_then(|n| n.get("icon"))),
        desktop: tstr(nami.and_then(|n| n.get("desktop"))),
    });
    m
}

/// An entry point: its name, `module:function`, and whether it has a window.
#[derive(Debug, Clone, PartialEq)]
struct Script {
    name: String,
    target: String,
    gui: bool,
}

/// [project.scripts] and [project.gui-scripts] (or Poetry's scripts).
fn python_scripts(t: &toml::Value) -> Vec<Script> {
    let mut out = Vec::new();
    let mut add = |table: Option<&toml::Value>, gui: bool| {
        for (name, v) in table.and_then(|t| t.as_table()).into_iter().flatten() {
            // Poetry also takes { reference = "pkg:main", type = "console" }.
            let target = v.as_str().map(String::from).or_else(|| tstr(v.get("reference")));
            if let Some(target) = target {
                out.push(Script { name: name.clone(), target, gui });
            }
        }
    };
    let project = t.get("project");
    add(project.and_then(|p| p.get("gui-scripts")), true);
    add(project.and_then(|p| p.get("scripts")), false);
    add(t.get("tool").and_then(|t| t.get("poetry")).and_then(|p| p.get("scripts")), false);
    out
}

/// Which entry point runs: the one asked for, the only GUI one, the one
/// named after the app, or the only one.
fn pick_script(scripts: &[Script], program: Option<&str>, name: &str) -> Result<Script> {
    let names = || scripts.iter().map(|s| s.name.clone()).collect::<Vec<_>>().join(", ");
    if let Some(p) = program {
        return scripts.iter().find(|s| s.name == p).cloned().with_context(|| format!("pyproject.toml has no entry point {p} (it has: {})", if scripts.is_empty() { "none".into() } else { names() }));
    }
    let guis: Vec<&Script> = scripts.iter().filter(|s| s.gui).collect();
    if guis.len() == 1 {
        return Ok(guis[0].clone());
    }
    match scripts {
        [] => bail!("pyproject.toml names no program: add one under [project.scripts] (a command) or [project.gui-scripts] (an app with a window), e.g.\n\n  [project.scripts]\n  {name} = \"{}.main:main\"", name.replace('-', "_")),
        [one] => Ok(one.clone()),
        _ => scripts.iter().find(|s| s.name == name).cloned().with_context(|| format!("pyproject.toml has several programs: say which runs with --program, or `bundle: program:` in project.nami (one of: {})", names())),
    }
}

/// `pkg.mod:func.attr [extra]` -> ("pkg.mod", "func.attr").
fn split_target(target: &str) -> Result<(String, String)> {
    let t = target.split('[').next().unwrap_or(target).trim();
    let (module, attr) = t.split_once(':').with_context(|| format!("the entry point {target:?} isn't module:function"))?;
    let ok = |s: &str| !s.is_empty() && s.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || c == '_'));
    ensure!(ok(module.trim()) && ok(attr.trim()), "the entry point {target:?} isn't module:function");
    Ok((module.trim().to_string(), attr.trim().to_string()))
}

/// The catalog's Python that `requires-python` takes, newest first (None: any).
fn pick_python(index: &Index, spec: Option<&str>) -> (String, Option<String>) {
    let Some(spec) = spec.filter(|s| !s.trim().is_empty()) else { return ("python3".into(), None) };
    // The catalog's own Python when it fits (python315 may be a pre-release).
    let default = index.get("python3").ok().map(|p| crate::query::version_of(&p.name)).unwrap_or_default();
    if let Some(minor) = default.strip_prefix("3.").and_then(|r| r.split('.').next()).and_then(|m| m.parse().ok()) {
        if python_fits(spec, 3, minor) {
            return ("python3".into(), Some(format!("requires-python {spec}")));
        }
    }
    for minor in (8..=20).rev() {
        let attr = format!("python3{minor}");
        if index.get(&attr).is_ok() && python_fits(spec, 3, minor) {
            // python3 is one of them: by its own name when it's the one.
            let v = index.get("python3").ok().map(|p| crate::query::version_of(&p.name)).unwrap_or_default();
            let attr = if v.starts_with(&format!("3.{minor}.")) { "python3".to_string() } else { attr };
            return (attr, Some(format!("requires-python {spec}")));
        }
    }
    ("python3".into(), Some(format!("requires-python {spec}: the catalog has no Python that fits; its newest is used")))
}

/// Does Python `major.minor` satisfy a requires-python like `>=3.10,<3.13`?
pub(crate) fn python_fits(spec: &str, major: u32, minor: u32) -> bool {
    let have = (major, minor);
    spec.split(',').map(str::trim).filter(|c| !c.is_empty()).all(|c| {
        let ops = ["~=", ">=", "<=", "==", "!=", ">", "<"];
        let Some(op) = ops.iter().find(|o| c.starts_with(**o)) else { return true };
        let v = c[op.len()..].trim();
        let star = v.ends_with(".*");
        let nums: Vec<u32> = v.trim_end_matches(".*").split('.').filter_map(|n| n.parse().ok()).collect();
        let want = (nums.first().copied().unwrap_or(0), nums.get(1).copied().unwrap_or(0));
        // A third number (>=3.10.4) counts as its minor version: patch releases come and go.
        let past_patch = nums.len() > 2 && nums[2] > 0;
        match *op {
            ">=" => have >= want,
            ">" => have > want || (have == want && past_patch),
            "<=" => have <= want,
            "<" => have < want,
            "==" if star || nums.len() <= 2 => have == want,
            "==" => have == want,
            "!=" if star || nums.len() <= 2 => have != want,
            "!=" => true,
            "~=" => have >= want && have.0 == want.0,
            _ => true,
        }
    })
}

impl Project<'_> {
    /// Where a tool the env has is: `<store path>/bin/<prog>` (logical).
    fn tool_bin(&self, env: &crate::project::Env, prog: &str) -> Result<String> {
        env.entries
            .iter()
            .flat_map(|e| e.paths.iter())
            .map(|p| format!("{p}/bin/{prog}"))
            .find(|p| self.store.phys(p).exists())
            .with_context(|| format!("the project's tools have no {prog}"))
    }

    /// A program of the project's: a launcher script in bin/.
    fn launcher(&self, tree: &Path, name: &str, text: &str) -> Result<String> {
        let dest = tree.join("bin").join(name);
        fs::create_dir_all(dest.parent().unwrap())?;
        fs::write(&dest, text)?;
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))?;
        Ok(format!("bin/{name}"))
    }

    pub(super) fn build_python(&self, meta: &Meta, tree: &Path, prefix: &str, work: &Path, found: &mut Found) -> Result<Built> {
        let t = read_toml(&self.dir.join("pyproject.toml")).context("reading pyproject.toml")?;
        let script = pick_script(&python_scripts(&t), self.program.as_deref(), &meta.name)?;
        let (module, attr) = split_target(&script.target)?;
        found.add("runs", &format!("{} = {}", script.name, script.target), if script.gui { "[project.gui-scripts]" } else { "[project.scripts]" });
        let spec = tstr(t.get("project").and_then(|p| p.get("requires-python")));
        let (python, why) = pick_python(self.index, spec.as_deref());
        let env = self.env(&[python])?;
        let py = self.tool_bin(&env, "python3")?;
        let version = self.sh(&env, "python3 -c 'import sys; print(\"%d.%d\" % sys.version_info[:2])'", &[], true)?.trim().to_string();
        found.add("python", &version, why.as_deref().unwrap_or("the catalog's"));
        let lib = tree.join("lib/app");
        fs::create_dir_all(&lib)?;
        // Its dependencies as its lock has them, else as pyproject.toml asks; then itself.
        let locked = self.dir.join("uv.lock").is_file();
        if locked {
            found.add("locked", "uv.lock", "");
        }
        let script_text = if locked {
            "uv export --frozen --no-dev --no-emit-project --no-header --quiet -o \"$NAMI_BUNDLE_WORK/requirements.txt\"\n\
             uv pip install --python \"$NAMI_PY\" --target \"$NAMI_BUNDLE_LIB\" -r \"$NAMI_BUNDLE_WORK/requirements.txt\"\n\
             uv pip install --python \"$NAMI_PY\" --target \"$NAMI_BUNDLE_LIB\" --no-deps .\n\
             \"$NAMI_PY\" -m compileall -q -j 0 --invalidation-mode checked-hash \"$NAMI_BUNDLE_LIB\""
        } else {
            "uv pip install --python \"$NAMI_PY\" --target \"$NAMI_BUNDLE_LIB\" .\n\
             \"$NAMI_PY\" -m compileall -q -j 0 --invalidation-mode checked-hash \"$NAMI_BUNDLE_LIB\""
        };
        let vars = [
            ("NAMI_PY", self.store.phys(&py).display().to_string()),
            ("NAMI_BUNDLE_LIB", lib.display().to_string()),
            ("NAMI_BUNDLE_WORK", work.display().to_string()),
            ("UV_PYTHON_DOWNLOADS", "never".to_string()),
            ("UV_NO_PROGRESS", "1".to_string()),
        ];
        self.sh(&env, script_text, &vars, false)?;
        // uv's own scripts name the build's Python: ours start the bundle's.
        let _ = fs::remove_dir_all(lib.join("bin"));
        let first = attr.split('.').next().unwrap_or(&attr);
        let text = format!(
            "#!{py} -I\n# {}: {}, as `nami bundle` started it\nimport sys\nsys.path.insert(0, \"{prefix}/lib/app\")\nfrom {module} import {first}\nsys.exit({attr}())\n",
            script.name, script.target
        );
        let entry = self.launcher(tree, &script.name, &text)?;
        Ok(Built { tree: tree.to_path_buf(), entry, gui: script.gui })
    }
}

// ---------- Node.js and Electron ----------

/// The Electron a package.json asks for: its version range, if any.
fn electron_wanted(p: &serde_json::Value) -> Option<String> {
    ["dependencies", "devDependencies"].iter().find_map(|k| jstr(&p[*k]["electron"]))
}

/// The major version a range like `^43.1.0`, `~22`, `>=20 <23` or `43.x` starts at.
fn major_of(range: &str) -> Option<u32> {
    let r = range.trim().trim_start_matches(['^', '~', '=', 'v', ' ']);
    r.split(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse().ok())
}

/// The scope less: @acme/app is app.
fn npm_name(n: &str) -> String {
    n.rsplit('/').next().unwrap_or(n).to_string()
}

/// package.json's name, version, productName, description; its "nami" key
/// for an icon or menu entry; electron-builder's icon.
pub(super) fn node_meta(dir: &Path) -> OwnMeta {
    let mut m = OwnMeta::default();
    let Some(p) = read_json(&dir.join("package.json")) else { return m };
    let nami = &p["nami"];
    let name = jstr(&p["name"]).map(|n| npm_name(&n));
    m.name = jstr(&nami["name"]).map(|n| (n, "package.json's nami".into())).or_else(|| name.clone().map(|n| (n, "package.json".into())));
    m.version = jstr(&p["version"]).map(|v| (v, "package.json".into()));
    // electron-builder's icon: a file, or a folder of sizes (the biggest); else its build/icon.png.
    let builder_icon = jstr(&p["build"]["linux"]["icon"]).or_else(|| jstr(&p["build"]["icon"])).and_then(|i| {
        let path = dir.join(&i);
        if path.is_dir() {
            let mut pngs: Vec<PathBuf> = fs::read_dir(&path).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "png" || e == "svg")).collect();
            pngs.sort();
            pngs.into_iter().max_by_key(|p| if p.extension().is_some_and(|e| e == "svg") { u32::MAX } else { png_size(p).unwrap_or(0) }).map(|p| p.strip_prefix(dir).unwrap_or(&p).display().to_string())
        } else {
            // electron-builder takes a name without .png.
            [i.clone(), format!("{i}.png")].into_iter().find(|c| dir.join(c).is_file() && !c.ends_with(".ico") && !c.ends_with(".icns"))
        }
    });
    let icon = jstr(&nami["icon"]).or(builder_icon).or_else(|| ["build/icon.svg", "build/icon.png"].iter().find(|c| dir.join(c).is_file()).map(|c| c.to_string()));
    m.cargo = Some(CargoMeta {
        file: "package.json".into(),
        package: name.unwrap_or_default(),
        display_name: jstr(&p["productName"]).or_else(|| jstr(&p["build"]["productName"])),
        description: jstr(&p["description"]),
        icon,
        desktop: jstr(&nami["desktop"]),
    });
    m
}

/// The package manager its lock file says, and its commands: (name, the
/// tool, install everything, install what it needs to run).
fn package_manager(dir: &Path) -> (&'static str, &'static str, &'static str, &'static str) {
    if dir.join("pnpm-lock.yaml").is_file() {
        ("pnpm", "pnpm", "pnpm install --frozen-lockfile", "pnpm install --frozen-lockfile --prod")
    } else if dir.join("yarn.lock").is_file() {
        ("yarn", "yarn", "yarn install --frozen-lockfile", "yarn install --frozen-lockfile --production")
    } else if dir.join("package-lock.json").is_file() || dir.join("npm-shrinkwrap.json").is_file() {
        ("npm", "", "npm ci", "npm ci --omit=dev")
    } else {
        ("npm", "", "npm install", "npm install --omit=dev")
    }
}

/// package.json's `bin`: (name, path) for each command.
fn node_bins(p: &serde_json::Value, name: &str) -> Vec<(String, String)> {
    match &p["bin"] {
        serde_json::Value::String(s) => vec![(name.to_string(), s.clone())],
        serde_json::Value::Object(m) => m.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string()))).collect(),
        _ => Vec::new(),
    }
}

/// A copy of the project's files, without node_modules or .git.
fn copy_source(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for e in fs::read_dir(src)? {
        let e = e?;
        let n = e.file_name();
        if n == "node_modules" || n == ".git" {
            continue;
        }
        let (from, to) = (e.path(), dest.join(&n));
        let m = fs::symlink_metadata(&from)?;
        if m.file_type().is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&from)?, &to)?;
        } else if m.is_dir() {
            copy_tree(&from, &to)?;
        } else if m.is_file() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

impl Project<'_> {
    pub(super) fn build_node(&self, meta: &Meta, tree: &Path, prefix: &str, found: &mut Found) -> Result<Built> {
        let p = read_json(&self.dir.join("package.json")).context("reading package.json")?;
        let electron = (self.kind == Kind::Electron).then(|| electron_wanted(&p)).flatten();
        let mut tools = vec![node_tool(self.index, jstr(&p["engines"]["node"]).as_deref(), found)];
        let (pm, pm_tool, install, install_prod) = package_manager(&self.dir);
        if !pm_tool.is_empty() {
            tools.push(pm_tool.to_string());
        }
        // Electron, the major version it asks for.
        let electron_tool = electron.as_deref().map(|range| {
            let t = major_of(range).map(|m| format!("electron@{m}")).filter(|t| self.index.get(t).is_ok());
            match t {
                Some(t) => {
                    found.add("electron", &t[9..], &format!("package.json: electron {range}"));
                    t
                }
                None => {
                    let v = self.index.get("electron").map(|p| crate::query::version_of(&p.name)).unwrap_or_default();
                    found.add("electron", &v, "the catalog's");
                    found.missing.push(format!("package.json asks for Electron {range}, which the catalog doesn't have: it runs on Electron {v}"));
                    "electron".to_string()
                }
            }
        });
        tools.extend(electron_tool.clone());
        let env = self.env(&tools)?;
        let vars = [
            // The catalog's Electron runs it: not the one npm would download.
            ("ELECTRON_SKIP_BINARY_DOWNLOAD", "1".to_string()),
            ("npm_config_fund", "false".to_string()),
            ("npm_config_audit", "false".to_string()),
            ("npm_config_update_notifier", "false".to_string()),
        ];
        let has_build = p["scripts"]["build"].is_string();
        let script = if has_build { format!("{install}\n{pm} run build") } else { install.to_string() };
        self.sh(&env, &script, &vars, false)?;
        // The app's files, and what it needs to run (not its build tools).
        let app = tree.join("lib/app");
        copy_source(&self.dir, &app)?;
        let has_deps = p["dependencies"].as_object().is_some_and(|d| d.keys().any(|k| k != "electron"));
        if has_deps {
            let mut vars = vars.to_vec();
            vars.push(("NAMI_BUNDLE_APP", app.display().to_string()));
            self.sh(&env, &format!("cd \"$NAMI_BUNDLE_APP\"\n{install_prod}"), &vars, false)?;
        }
        let bash = self.tool_bin(&env, "bash")?;
        if let Some(_) = &electron_tool {
            let electron = self.tool_bin(&env, "electron")?;
            let main = jstr(&p["main"]).unwrap_or_else(|| "index.js".into());
            ensure!(app.join(&main).is_file(), "package.json's main, {main}, isn't there{}", if has_build { " after its build" } else { "" });
            found.add("main", &main, "package.json");
            let name = meta.name.clone();
            // Packaged, as electron-builder's would be (app.isPackaged), not a development run.
            let text = format!("#!{bash}\n# {name}, on the catalog's Electron, as `nami bundle` started it\nexport ELECTRON_FORCE_IS_PACKAGED=1 NODE_ENV=production\nexec \"{electron}\" \"{prefix}/lib/app\" \"$@\"\n");
            let entry = self.launcher(tree, &name, &text)?;
            return Ok(Built { tree: tree.to_path_buf(), entry, gui: true });
        }
        let node = self.tool_bin(&env, "node")?;
        let mut bins = node_bins(&p, &meta.name);
        if bins.is_empty() {
            if let Some(main) = jstr(&p["main"]) {
                bins.push((meta.name.clone(), main));
            }
        }
        let names = bins.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>().join(", ");
        let (name, path) = match self.program.as_deref() {
            Some(want) => bins.iter().find(|(n, _)| n == want).cloned().with_context(|| format!("package.json has no program {want} (its bin has: {names})"))?,
            None => match bins.as_slice() {
                [] => bail!("package.json names no program: give it a \"bin\" (\"bin\": \"cli.js\"), or a \"main\""),
                [one] => one.clone(),
                _ => bins.iter().find(|(n, _)| *n == meta.name).cloned().with_context(|| format!("package.json's bin has several programs: say which runs with --program (one of: {names})"))?,
            },
        };
        let path = path.trim_start_matches("./").to_string();
        ensure!(app.join(&path).is_file(), "package.json's bin, {path}, isn't there{}", if has_build { " after its build" } else { "" });
        found.add("runs", &format!("{name} = {path}"), "package.json's bin");
        let text = format!("#!{bash}\n# {name}, on the catalog's Node.js, as `nami bundle` started it\nexec \"{node}\" \"{prefix}/lib/app/{path}\" \"$@\"\n");
        let entry = self.launcher(tree, &name, &text)?;
        Ok(Built { tree: tree.to_path_buf(), entry, gui: false })
    }
}

/// The catalog's Node.js for `engines.node`: a range that starts at a
/// major version (^22, 22.x, ~22.1) gets that one; >=, or none, the newest.
fn node_tool(index: &Index, engines: Option<&str>, found: &mut Found) -> String {
    let fixed = engines.filter(|e| !e.trim_start().starts_with('>') && !e.contains("||"));
    if let Some(m) = fixed.and_then(major_of) {
        let t = format!("nodejs@{m}");
        if index.get(&t).is_ok() {
            found.add("node", &m.to_string(), &format!("package.json: engines.node {}", engines.unwrap_or_default()));
            return t;
        }
        found.missing.push(format!("package.json asks for Node.js {}, which the catalog doesn't have: it runs on the catalog's", engines.unwrap_or_default()));
    }
    let v = index.get("nodejs").map(|p| crate::query::version_of(&p.name)).unwrap_or_default();
    found.add("node", &v, "the catalog's");
    "nodejs".into()
}

// ---------- Zig ----------

/// build.zig.zon's `.name` (a string, or `.name = .foo` since 0.14) and `.version`.
pub(super) fn zig_meta(dir: &Path) -> OwnMeta {
    let mut m = OwnMeta::default();
    let Ok(text) = fs::read_to_string(dir.join("build.zig.zon")) else { return m };
    if let Some(n) = zon_field(&text, "name") {
        m.name = Some((n.replace('_', "-"), "build.zig.zon".into()));
    }
    if let Some(v) = zon_field(&text, "version") {
        m.version = Some((v, "build.zig.zon".into()));
    }
    m
}

/// `.field = "value"` or `.field = .value` in a .zon file (its top level).
fn zon_field(text: &str, field: &str) -> Option<String> {
    let text = strip_comments(&text.replace("//", "\u{1}"), '\u{1}');
    let at = text.find(&format!(".{field}"))?;
    let rest = text[at + field.len() + 1..].trim_start().strip_prefix('=')?.trim_start();
    if let Some(r) = rest.strip_prefix('"') {
        r.split('"').next().map(String::from)
    } else {
        let r = rest.strip_prefix('.')?;
        let r = r.strip_prefix("@\"").map(|q| q.split('"').next().unwrap_or("")).unwrap_or(r);
        Some(r.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect()).filter(|s: &String| !s.is_empty())
    }
}

impl Project<'_> {
    pub(super) fn build_zig(&self, meta: &Meta, tree: &Path, work: &Path, found: &mut Found) -> Result<Built> {
        // The Zig its build.zig.zon asks for (each release changes the build API).
        let min = fs::read_to_string(self.dir.join("build.zig.zon")).ok().and_then(|t| zon_field(&t, "minimum_zig_version"));
        let tool = match &min {
            Some(v) => {
                let mm: String = v.split('.').take(2).collect::<Vec<_>>().join(".");
                let t = format!("zig@{mm}");
                if self.index.get(&t).is_ok() {
                    found.add("zig", &mm, &format!("build.zig.zon: minimum_zig_version {v}"));
                    t
                } else {
                    found.missing.push(format!("build.zig.zon asks for Zig {v}, which the catalog doesn't have: it's built with the catalog's"));
                    "zig".into()
                }
            }
            None => {
                found.add("zig", &self.index.get("zig").map(|p| crate::query::version_of(&p.name)).unwrap_or_default(), "the catalog's");
                "zig".into()
            }
        };
        let env = self.env(&[tool])?;
        let vars = [("NAMI_BUNDLE_TREE", tree.display().to_string()), ("ZIG_GLOBAL_CACHE_DIR", work.join("zig-global").display().to_string()), ("NAMI_BUNDLE_WORK", work.display().to_string())];
        self.sh(&env, "zig build install --release=safe --prefix \"$NAMI_BUNDLE_TREE\" --cache-dir \"$NAMI_BUNDLE_WORK/zig-cache\" --global-cache-dir \"$ZIG_GLOBAL_CACHE_DIR\"", &vars, false)?;
        ensure!(!programs_in(tree, None).is_empty(), "zig build installed no program: give build.zig a b.installArtifact(exe)");
        // Its debug information names the Zig it was built with (its std's
        // source files), which would bring the whole compiler along: that
        // name is made no store path's, as nixpkgs' remove-references-to does.
        let zig = self.tool_bin(&env, "zig")?;
        let (base, _) = super::super::split_store(&zig);
        if base.len() > 32 {
            rewrite_tree(tree, base[..32].as_bytes(), "e".repeat(32).as_bytes())?;
        }
        let entry = choose_program(tree, self.program.as_deref(), &meta.name)?;
        Ok(Built { tree: tree.to_path_buf(), entry, gui: false })
    }
}

// ---------- Flutter ----------

/// pubspec.yaml's name, version (less its +build) and description.
pub(super) fn flutter_meta(dir: &Path) -> OwnMeta {
    let mut m = OwnMeta::default();
    let Some(y) = fs::read_to_string(dir.join("pubspec.yaml")).ok().and_then(|t| serde_yaml::from_str::<Value>(&t).ok()) else { return m };
    let s = |k: &str| y.get(k).and_then(|v| v.as_str()).map(String::from);
    let name = s("name");
    m.name = name.clone().map(|n| (n.replace('_', "-"), "pubspec.yaml".into()));
    m.version = s("version").map(|v| (v.split('+').next().unwrap_or(&v).to_string(), "pubspec.yaml".into()));
    m.cargo = Some(CargoMeta { file: "pubspec.yaml".into(), package: name.unwrap_or_default(), description: s("description"), ..Default::default() });
    m
}

impl Project<'_> {
    pub(super) fn build_flutter(&self, meta: &Meta, tree: &Path, found: &mut Found) -> Result<Built> {
        let env = self.env(&["flutter".to_string()])?;
        self.sh(&env, "flutter --no-version-check config --no-analytics >/dev/null 2>&1 || true\nflutter --no-version-check build linux --release", &[("FLUTTER_SUPPRESS_ANALYTICS", "true".to_string())], false)?;
        let out = ["build/linux/x64/release/bundle", "build/linux/release/bundle"].iter().map(|d| self.dir.join(d)).find(|d| d.is_dir()).context("flutter build linux made no build/linux/x64/release/bundle")?;
        let app = tree.join("lib/app");
        copy_tree(&out, &app)?;
        // The runner (BINARY_NAME in linux/CMakeLists.txt): the program in the bundle's folder.
        let progs = programs_in(&app, None);
        let runner = match progs.as_slice() {
            [one] => one.clone(),
            _ => progs.iter().find(|p| **p == meta.name || p.replace('-', "_") == meta.name.replace('-', "_")).cloned().with_context(|| format!("flutter's bundle has no one program (it has: {})", progs.join(", ")))?,
        };
        found.add("runner", &runner, "linux/CMakeLists.txt's BINARY_NAME");
        // It finds its data and libraries beside itself (/proc/self/exe): a link, not a copy.
        fs::create_dir_all(tree.join("bin"))?;
        std::os::unix::fs::symlink(format!("../lib/app/{runner}"), tree.join("bin").join(&runner))?;
        Ok(Built { tree: tree.to_path_buf(), entry: format!("bin/{runner}"), gui: true })
    }
}

// ---------- .NET ----------

/// The .csproj or .fsproj files at the top of the folder.
fn dotnet_projects(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "csproj" || e == "fsproj")).collect();
    v.sort();
    v
}

/// `<Tag>value</Tag>` in an MSBuild project, the first.
fn msbuild_prop(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let at = text.find(&open)? + open.len();
    let end = text[at..].find("</")?;
    Some(text[at..at + end].trim().to_string()).filter(|s| !s.is_empty() && !s.contains("$("))
}

pub(super) fn dotnet_meta(dir: &Path) -> OwnMeta {
    let mut m = OwnMeta::default();
    let Some(proj) = dotnet_projects(dir).into_iter().next() else { return m };
    let text = fs::read_to_string(&proj).unwrap_or_default();
    let file = proj.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let stem = proj.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    m.name = msbuild_prop(&text, "AssemblyName").map(|n| (n, file.clone())).or_else(|| Some((stem.clone(), format!("{file}'s name"))));
    m.version = msbuild_prop(&text, "Version").or_else(|| msbuild_prop(&text, "VersionPrefix")).map(|v| (v, file.clone()));
    m.cargo = Some(CargoMeta { file: file.clone(), package: stem, display_name: msbuild_prop(&text, "Product"), description: msbuild_prop(&text, "Description"), ..Default::default() });
    m
}

impl Project<'_> {
    pub(super) fn build_dotnet(&self, meta: &Meta, tree: &Path, prefix: &str, work: &Path, found: &mut Found) -> Result<Built> {
        let projects = dotnet_projects(&self.dir);
        let proj = match projects.as_slice() {
            [one] => one.clone(),
            _ => projects
                .iter()
                .find(|p| p.file_stem().is_some_and(|s| Some(s.to_string_lossy().as_ref()) == self.program.as_deref()))
                .cloned()
                .with_context(|| format!("several projects here: say which with --program (one of: {})", projects.iter().map(|p| p.file_stem().unwrap_or_default().to_string_lossy().into_owned()).collect::<Vec<_>>().join(", ")))?,
        };
        let text = fs::read_to_string(&proj)?;
        let framework = msbuild_prop(&text, "TargetFramework").or_else(|| msbuild_prop(&text, "TargetFrameworks").and_then(|f| f.split(';').next().map(String::from)));
        // net8.0 -> 8: the SDK to build with and the runtime it runs on.
        let major = framework.as_deref().and_then(|f| f.strip_prefix("net")).and_then(|v| v.split('.').next()).and_then(|v| v.parse::<u32>().ok());
        let (sdk, runtime) = match major.map(|m| (format!("dotnet-sdk@{m}"), format!("dotnet-runtime@{m}"))) {
            Some((s, r)) if self.index.get(&s).is_ok() && self.index.get(&r).is_ok() => (s, r),
            _ => {
                found.missing.push(format!("its TargetFramework, {}, isn't one the catalog has a .NET for: it's built with the catalog's", framework.as_deref().unwrap_or("none")));
                ("dotnet-sdk".into(), "dotnet-runtime".into())
            }
        };
        let rt_version = self.index.get(&runtime).map(|p| crate::query::version_of(&p.name)).unwrap_or_default();
        found.add("runtime", &format!(".NET {rt_version}"), &format!("TargetFramework {}", framework.as_deref().unwrap_or("?")));
        let env = self.env(&[sdk, runtime.clone()])?;
        let assembly = msbuild_prop(&text, "AssemblyName").unwrap_or_else(|| proj.file_stem().unwrap_or_default().to_string_lossy().into_owned());
        let app = tree.join("lib/app");
        let vars = [
            ("NAMI_BUNDLE_PROJ", proj.display().to_string()),
            ("NAMI_BUNDLE_APP", app.display().to_string()),
            ("DOTNET_CLI_TELEMETRY_OPTOUT", "1".to_string()),
            ("DOTNET_NOLOGO", "1".to_string()),
            ("DOTNET_SKIP_FIRST_TIME_EXPERIENCE", "1".to_string()),
            ("NAMI_BUNDLE_WORK", work.display().to_string()),
        ];
        self.sh(&env, "dotnet publish \"$NAMI_BUNDLE_PROJ\" -c Release -o \"$NAMI_BUNDLE_APP\" --self-contained false -p:UseAppHost=false", &vars, false)?;
        let dll = format!("{assembly}.dll");
        ensure!(app.join(&dll).is_file(), "dotnet publish made no {dll}");
        // The runtime's own dotnet, not the SDK's (which is ten times its size).
        let rt_path = env.entries.iter().find(|e| e.attr == runtime).and_then(|e| e.paths.first().cloned()).context("the .NET runtime isn't among the project's tools")?;
        let dotnet = format!("{rt_path}/bin/dotnet");
        ensure!(self.store.phys(&dotnet).exists(), "{runtime} has no bin/dotnet");
        let bash = self.tool_bin(&env, "bash")?;
        let name = meta.name.clone();
        let text_l = format!("#!{bash}\n# {name}, on the catalog's .NET runtime, as `nami bundle` started it\nexport DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1\nexec \"{dotnet}\" \"{prefix}/lib/app/{dll}\" \"$@\"\n");
        let entry = self.launcher(tree, &name, &text_l)?;
        // Avalonia's and other apps with a window say WinExe.
        let gui = msbuild_prop(&text, "OutputType").is_some_and(|t| t.eq_ignore_ascii_case("WinExe"));
        Ok(Built { tree: tree.to_path_buf(), entry, gui })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nami-bundle-langs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn detects_the_new_kinds() {
        let d = tmp("detect");
        assert_eq!(detect(&d), None);
        fs::write(d.join("package.json"), r#"{"name":"x","bin":"cli.js"}"#).unwrap();
        assert_eq!(detect(&d), Some(Kind::Node));
        fs::write(d.join("package.json"), r#"{"name":"x","devDependencies":{"electron":"^43.1.0"}}"#).unwrap();
        assert_eq!(detect(&d), Some(Kind::Electron));
        fs::write(d.join("app.csproj"), "<Project/>").unwrap();
        assert_eq!(detect(&d), Some(Kind::DotNet));
        fs::write(d.join("build.zig"), "").unwrap();
        assert_eq!(detect(&d), Some(Kind::Zig));
        fs::write(d.join("pubspec.yaml"), "name: x\n").unwrap();
        assert_eq!(detect(&d), Some(Kind::Zig), "Flutter needs its linux/ folder");
        fs::create_dir_all(d.join("linux")).unwrap();
        assert_eq!(detect(&d), Some(Kind::Flutter));
        // A pyproject.toml with only a tool's settings isn't a Python project.
        fs::write(d.join("pyproject.toml"), "[tool.black]\nline-length = 100\n").unwrap();
        assert_eq!(detect(&d), Some(Kind::Flutter));
        fs::write(d.join("pyproject.toml"), "[project]\nname = 'x'\n").unwrap();
        assert_eq!(detect(&d), Some(Kind::Python));
        // The first four come first: a Rust project with a package.json for its web side.
        assert_eq!(super::super::detect(&d, &Settings::default(), None), Some(Kind::Python));
        fs::write(d.join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
        assert_eq!(super::super::detect(&d, &Settings::default(), None), Some(Kind::Rust));
    }

    #[test]
    fn reads_pyproject() {
        let d = tmp("py");
        fs::write(
            d.join("pyproject.toml"),
            "[project]\nname = 'py-hello'\nversion = '1.2.0'\ndescription = 'Says hello'\nrequires-python = '>=3.11'\n\
             [project.scripts]\npy-hello = 'py_hello.cli:main'\npy-tool = 'py_hello.tool:run'\n\
             [project.gui-scripts]\npy-hello-gui = 'py_hello.gui:App.run'\n[tool.nami]\nicon = 'icon.svg'\n",
        )
        .unwrap();
        let m = python_meta(&d);
        assert_eq!(m.name.unwrap().0, "py-hello");
        assert_eq!(m.version.unwrap().0, "1.2.0");
        let c = m.cargo.unwrap();
        assert_eq!((c.file.as_str(), c.icon.as_deref(), c.description.as_deref()), ("pyproject.toml", Some("icon.svg"), Some("Says hello")));
        let t = read_toml(&d.join("pyproject.toml")).unwrap();
        let s = python_scripts(&t);
        assert_eq!(s.len(), 3);
        // The only GUI one, unless another is asked for.
        assert_eq!(pick_script(&s, None, "py-hello").unwrap().name, "py-hello-gui");
        assert_eq!(pick_script(&s, Some("py-tool"), "py-hello").unwrap().target, "py_hello.tool:run");
        assert!(pick_script(&s, Some("nope"), "x").unwrap_err().to_string().contains("py-hello"));
        let clis: Vec<Script> = s.iter().filter(|s| !s.gui).cloned().collect();
        assert_eq!(pick_script(&clis, None, "py-hello").unwrap().name, "py-hello");
        assert!(pick_script(&clis, None, "other").unwrap_err().to_string().contains("--program"));
        assert!(pick_script(&[], None, "my-app").unwrap_err().to_string().contains("my_app.main:main"));
        assert_eq!(split_target("py_hello.gui:App.run [gui]").unwrap(), ("py_hello.gui".into(), "App.run".into()));
        assert!(split_target("no_colon").is_err());
        assert!(split_target("os:system('x')").is_err());
    }

    #[test]
    fn requires_python() {
        assert!(python_fits(">=3.10", 3, 14));
        assert!(!python_fits(">=3.10,<3.13", 3, 13));
        assert!(python_fits(">=3.10,<3.13", 3, 12));
        assert!(python_fits("~=3.11", 3, 13));
        assert!(python_fits("==3.12.*", 3, 12) && !python_fits("==3.12.*", 3, 13));
        assert!(python_fits(">3.11", 3, 12) && !python_fits(">3.11", 3, 11));
        assert!(python_fits("<=3.12", 3, 12));
        assert!(python_fits(">=3.9.2", 3, 9));
    }

    #[test]
    fn reads_package_json() {
        let d = tmp("node");
        fs::create_dir_all(d.join("build/icons")).unwrap();
        fs::write(d.join("build/icons/512x512.png"), b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\x02\0\0\0\x02\0").unwrap();
        fs::write(d.join("build/icons/64x64.png"), b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x40\0\0\0\x40").unwrap();
        fs::write(d.join("package.json"), r#"{"name":"@acme/hello-app","version":"2.1.0","productName":"Hello App","description":"Hi","main":"main.js","devDependencies":{"electron":"^43.1.0"},"build":{"linux":{"icon":"build/icons"}}}"#).unwrap();
        let m = node_meta(&d);
        assert_eq!(m.name.unwrap().0, "hello-app");
        assert_eq!(m.version.unwrap().0, "2.1.0");
        let c = m.cargo.unwrap();
        assert_eq!(c.display_name.as_deref(), Some("Hello App"));
        assert_eq!(c.icon.as_deref(), Some("build/icons/512x512.png"));
        let p = read_json(&d.join("package.json")).unwrap();
        assert_eq!(electron_wanted(&p).as_deref(), Some("^43.1.0"));
        assert_eq!(major_of("^43.1.0"), Some(43));
        assert_eq!(major_of("~22"), Some(22));
        assert_eq!(major_of("22.x"), Some(22));
        assert_eq!(major_of("latest"), None);
        assert_eq!(node_bins(&serde_json::json!({"bin":"./cli.js"}), "x"), vec![("x".to_string(), "./cli.js".to_string())]);
        assert_eq!(node_bins(&serde_json::json!({"bin":{"a":"a.js","b":"b.js"}}), "x").len(), 2);
        assert_eq!(package_manager(&d).2, "npm install");
        fs::write(d.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(package_manager(&d).0, "pnpm");
        assert_eq!(npm_name("@acme/app"), "app");
    }

    #[test]
    fn reads_zon_pubspec_and_csproj() {
        let zon = ".{\n    // the package\n    .name = .zig_hello,\n    .version = \"0.2.0\",\n    .minimum_zig_version = \"0.15.1\",\n}\n";
        assert_eq!(zon_field(zon, "name").as_deref(), Some("zig_hello"));
        assert_eq!(zon_field(zon, "version").as_deref(), Some("0.2.0"));
        assert_eq!(zon_field(zon, "minimum_zig_version").as_deref(), Some("0.15.1"));
        assert_eq!(zon_field(".{ .name = \"old-style\" }", "name").as_deref(), Some("old-style"));
        assert_eq!(zon_field(".{ .name = .@\"with-dash\" }", "name").as_deref(), Some("with-dash"));
        let d = tmp("flutter");
        fs::write(d.join("pubspec.yaml"), "name: flutter_hello\ndescription: A Flutter app\nversion: 1.0.3+7\n").unwrap();
        let m = flutter_meta(&d);
        assert_eq!(m.name.unwrap().0, "flutter-hello");
        assert_eq!(m.version.unwrap().0, "1.0.3");
        let d = tmp("dotnet");
        fs::write(d.join("Hello.csproj"), "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <OutputType>Exe</OutputType>\n    <TargetFramework>net8.0</TargetFramework>\n    <AssemblyName>dotnet-hello</AssemblyName>\n    <Version>3.0.1</Version>\n  </PropertyGroup>\n</Project>\n").unwrap();
        let m = dotnet_meta(&d);
        assert_eq!(m.name.unwrap(), ("dotnet-hello".to_string(), "Hello.csproj".to_string()));
        assert_eq!(m.version.unwrap().0, "3.0.1");
        assert_eq!(msbuild_prop("<A>$(B)</A>", "A"), None);
    }

    #[test]
    fn a_projects_own_tool_version_wins() {
        assert_eq!(family("python312"), "python");
        assert_eq!(family("python3"), "python");
        assert_eq!(family("nodejs@22"), "nodejs");
        assert_eq!(family("electron_43"), "electron");
        assert_eq!(family("zig@0.15"), "zig");
        assert_eq!(family("dotnet-sdk_9"), "dotnet-sdk");
        assert_eq!(family("pkg-config"), "pkg-config");
    }
}
