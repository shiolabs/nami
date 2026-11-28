//! `nami import`: a Nix setup as nami files. A flake's dev shell becomes a
//! project's project.nami, a home-manager configuration the user file, a NixOS
//! configuration's packages the system file:
//!
//! | Nix                                   | nami                         |
//! |---------------------------------------|------------------------------|
//! | `environment.systemPackages`          | /etc/nami/system.nami          |
//! | home-manager (`home.*`, `programs.*`) | ~/user.nami     |
//! | `devShells.<system>.default`          | project.nami (+ nami.lock)   |
//!
//! Services: a NixOS module writes its unit from settings; nami runs the unit
//! a package ships, or one command of your own. So `services.<name>.enable`
//! becomes `services:` only where nixpkgs' package has the unit (a short,
//! checked list: tailscale, syncthing), `systemd.services` (home-manager:
//! `systemd.user.services`) only when it's one command, `environment.etc`
//! becomes `files:`, and openssh's settings an sshd_config.d file for the
//! distro's sshd. The rest is listed.
//!
//! Read without Nix (nix/expr.rs); when Nix happens to be installed, `nix
//! eval` fills in a dev shell's packages the static reader couldn't. It
//! merges into the target (config_edit: comments kept, nothing replaced),
//! and nothing is dropped silently: what has no nami equivalent is listed,
//! and written as a comment at the end of the file.
//!
//! A whole computer (`nami import /` on Ubuntu or Debian, or Windows' disk)
//! becomes a config repo instead: see moving.rs.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_yaml::{Mapping, Value};

use crate::config_edit::{self, Op};
use crate::index::Index;
use crate::nix::expr::{Eval, Param, Thunk, Val, SYSTEM};

mod dconf;
mod debian;
mod evaluated;
mod generated;
mod hive;
mod hm;
pub mod moving;
mod nixos;
pub mod nixtool;
mod programs;
mod windows;

pub use nixtool::set_fetch as set_nix_fetch;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Flake,
    HomeManager,
    Nixos,
}

impl Kind {
    fn describe(self) -> &'static str {
        match self {
            Kind::Flake => "a Nix dev shell",
            Kind::HomeManager => "a home-manager configuration",
            Kind::Nixos => "a NixOS configuration",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, clap::ValueEnum)]
pub enum Tier {
    System,
    User,
    Project,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Unconverted {
    pub what: String,
    pub why: String,
    /// Only unknown because it was read without Nix (`nix eval` may know it).
    #[serde(skip)]
    soft: bool,
}

/// What a Nix file says, in nami's terms, before choosing where it goes.
#[derive(Debug, Default)]
pub struct Found {
    /// Attribute names, with where each came from (for messages).
    pub packages: Vec<(String, String)>,
    pub env: BTreeMap<String, String>,
    pub aliases: BTreeMap<String, String>,
    /// NixOS's `environment.variables`/`sessionVariables`: the computer's.
    pub system_env: BTreeMap<String, String>,
    /// home.shell: bash/zsh/fish -> lines.
    pub shell: BTreeMap<String, String>,
    /// home.login-shell (users.users.<you>.shell).
    pub login_shell: Option<String>,
    /// home.files: path relative to ~ -> text, settings or `{ source: <absolute path> }`.
    pub files: BTreeMap<String, Value>,
    pub settings: BTreeMap<String, BTreeMap<String, Value>>,
    pub tasks: BTreeMap<String, String>,
    pub develop: bool,
    /// `services:` entries (name -> true or settings): NixOS's are the
    /// computer's; after `retarget`, what goes in the target file.
    pub services: BTreeMap<String, Value>,
    /// home-manager's: services that are yours (`systemctl --user`).
    pub user_services: BTreeMap<String, Value>,
    /// `files:` (/etc): path -> text, settings, `{ source }`, or with `mode`.
    pub etc_files: BTreeMap<String, Value>,
    /// `unfree: false` (nixpkgs.config.allowUnfree = false).
    pub unfree: Option<bool>,
    /// Files home-manager generated that the mappings don't cover: copied to
    /// `files/<path>` next to the nami file, as home files.
    pub generated: BTreeMap<String, generated::Generated>,
    /// Other keys to set where the file has none (`os.hostname`).
    pub top: Vec<(Vec<String>, Value)>,
    /// Nami files system.nami includes (`ubuntu`, `docker`), added
    /// to its `include:`.
    pub includes: Vec<String>,
    /// People in a group that's a nami file's `users:` (apps.yaml's
    /// feature-groups): (group, login, where it was said).
    pub group_members: Vec<(String, String, String)>,
    /// Keys set only if a nami file ends up included: (file, path, value).
    pub if_included: Vec<(String, Vec<String>, Value)>,
    pub unconverted: Vec<Unconverted>,
    pub warnings: Vec<String>,
}

impl Found {
    fn skip(&mut self, what: impl Into<String>, why: impl Into<String>) {
        let u = Unconverted { what: what.into(), why: why.into(), soft: false };
        if !self.unconverted.contains(&u) {
            self.unconverted.push(u);
        }
    }
    fn warn(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        if !self.warnings.contains(&msg) {
            self.warnings.push(msg);
        }
    }
    fn package(&mut self, attr: &str, origin: &str) {
        if !self.packages.iter().any(|(a, _)| a == attr) {
            self.packages.push((attr.to_string(), origin.to_string()));
        }
    }
    pub fn include(&mut self, file: &str) {
        if !self.includes.iter().any(|f| f == file) {
            self.includes.push(file.to_string());
        }
    }
}

/// apps.yaml's tables of what's a package of nami's own on nami's computer:
/// packages (`docker` -> nami's examples/packages/docker, copied next to the computer's folder) and groups (`docker`: its `users:`).
fn feature_tables() -> &'static (BTreeMap<String, String>, BTreeMap<String, String>) {
    static T: std::sync::OnceLock<(BTreeMap<String, String>, BTreeMap<String, String>)> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        #[derive(serde::Deserialize)]
        struct Tables {
            features: BTreeMap<String, String>,
            #[serde(rename = "feature-groups")]
            groups: BTreeMap<String, String>,
        }
        let t: Tables = serde_yaml::from_str(include_str!("import/apps.yaml")).expect("import/apps.yaml");
        (t.features, t.groups)
    })
}

/// Ubuntu's name for a package of nami's (docker: docker.io), from
/// apps.yaml's deb table; None when it's the same or not there.
pub(crate) fn deb_name(name: &str) -> Option<&'static str> {
    static DEB: std::sync::OnceLock<Vec<(String, Option<String>)>> = std::sync::OnceLock::new();
    let deb = DEB.get_or_init(|| {
        #[derive(serde::Deserialize)]
        struct Tables {
            deb: serde_yaml::Mapping,
        }
        let t: Tables = serde_yaml::from_str(include_str!("import/apps.yaml")).expect("import/apps.yaml");
        t.deb.into_iter().filter_map(|(k, v)| Some((k.as_str()?.to_string(), v.as_str().map(String::from)))).collect()
    });
    deb.iter().find(|(_, v)| v.as_deref() == Some(name)).map(|(k, _)| k.as_str())
}

/// The nami file that's what `group` is for, if there's one (`docker`).
pub(crate) fn feature_group(group: &str) -> Option<&'static str> {
    feature_tables().1.get(group).map(String::as_str)
}

/// system.nami, for a computer nami made: a package that's more
/// than an app there (a daemon, a group, settings) comes as its nami file
/// (`- docker`), when `packages` says so; the people in its group
/// (those the file has in os.users) get it in their `groups:`; and what's
/// only for an included file is set.
pub(crate) fn features(found: &mut Found, packages: bool) {
    let (files, _) = feature_tables();
    if packages {
        let (feature, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut found.packages).into_iter().partition(|(a, _)| files.contains_key(a));
        found.packages = rest;
        for (a, _) in feature {
            found.include(&files[&a]);
        }
    }
    for (group, login, what) in std::mem::take(&mut found.group_members) {
        let file = feature_group(&group).unwrap_or(&group);
        let included = found.includes.iter().any(|f| f == file);
        // One of os.users already: the group is theirs, as usermod -aG has it.
        let me = found.top.iter_mut().find(|(p, _)| p.len() == 2 && p[0] == "users" && p[1] == login).and_then(|(_, v)| v.as_mapping_mut());
        match me {
            Some(m) if included => {
                let groups = m.entry("groups".into()).or_insert(Value::Sequence(Vec::new()));
                if let Value::Sequence(g) = groups {
                    if !g.contains(&Value::String(group.clone())) {
                        g.push(Value::String(group));
                    }
                }
            }
            _ if included => found.skip(what, format!("the {group} group: `groups: [{group}]` under users.{login}, on a computer nami made; elsewhere `sudo usermod -aG {group} {login}`")),
            _ => found.skip(what, format!("the {group} group: `- {file}` in packages: (`nami new <the computer's folder> --from {file}` copies it from nami's examples) and `groups: [{group}]` under users.{login} set it up on a computer nami made ({file} wasn't installed here)")),
        }
    }
    for (file, path, v) in std::mem::take(&mut found.if_included) {
        if found.includes.contains(&file) {
            found.top.push((path, v));
        }
    }
}

// ---------- the catalog ----------

/// Package names to check imports against (the installed index).
pub struct Catalog<'a> {
    names: HashSet<&'a str>,
    /// pythonXYZPackages sets it has, the biggest (python3's) first.
    python: Vec<&'a str>,
    /// store path -> attr, for what `nix eval` reports.
    paths: HashMap<&'a str, &'a str>,
    /// attr -> its name without the version (`ripgrep-15.2.0` -> `ripgrep`).
    pnames: HashMap<&'a str, &'a str>,
}

/// `ripgrep-15.2.0` -> `ripgrep`; `python3.13-requests-2.32` -> `python3.13-requests`.
fn pname(name: &str) -> &str {
    let mut cut = name.len();
    for (i, _) in name.match_indices('-') {
        if name[i + 1..].starts_with(|c: char| c.is_ascii_digit()) {
            cut = i;
            break;
        }
    }
    &name[..cut]
}

impl<'a> Catalog<'a> {
    pub fn new(names: impl IntoIterator<Item = &'a str>) -> Self {
        let names: HashSet<&str> = names.into_iter().collect();
        let mut sets: HashMap<&str, usize> = HashMap::new();
        for set in names.iter().filter_map(|n| n.split_once('.').map(|(set, _)| set)) {
            if set.strip_prefix("python3").and_then(|r| r.strip_suffix("Packages")).is_some_and(|v| v.chars().all(|c| c.is_ascii_digit())) {
                *sets.entry(set).or_default() += 1;
            }
        }
        let mut python: Vec<&str> = sets.keys().copied().collect();
        python.sort_by_key(|s| std::cmp::Reverse((sets[s], *s)));
        Catalog { names, python, paths: HashMap::new(), pnames: HashMap::new() }
    }

    pub fn from_index(index: &'a Index) -> Self {
        let mut c = Catalog::new(index.iter().map(|(a, _)| a));
        for (attr, pkg) in index.iter() {
            c.pnames.insert(attr, pname(&pkg.name));
            for p in pkg.outputs.values().flatten() {
                c.paths.insert(p.as_str(), attr);
            }
        }
        c
    }

    pub fn has(&self, attr: &str) -> bool {
        self.names.contains(attr)
    }

    /// The catalog's name for a nixpkgs attribute: itself, or where it moved
    /// (`python3Packages.x` -> `python313Packages.x`, `nodePackages.x` -> `x`).
    /// The attribute whose package is called `pn` (`code` -> `vscode`), the
    /// shortest if several are.
    pub fn attr_for_pname(&self, pn: &str) -> Option<String> {
        self.pnames.iter().filter(|(_, n)| **n == pn).map(|(a, _)| *a).min_by_key(|a| (a.len(), *a)).map(String::from)
    }

    pub fn resolve(&self, attr: &str) -> Option<String> {
        if self.has(attr) {
            return Some(attr.to_string());
        }
        // A package's variant as an attribute of it (`vscode.fhs`,
        // `steam.run`): nixpkgs names it with a dash too (`vscode-fhs`).
        let dashed = attr.replace('.', "-");
        if attr.contains('.') && self.has(&dashed) {
            return Some(dashed);
        }
        let (set, rest) = attr.split_once('.')?;
        let set = set.strip_suffix(".pkgs").unwrap_or(set);
        let pythonish = set == "pythonPackages" || set == "python3" || (set.starts_with("python3") && set.ends_with("Packages"));
        let rest = if set == "python3" { rest.strip_prefix("pkgs.")? } else { rest };
        if pythonish {
            if let Some(hit) = self.python.iter().map(|p| format!("{p}.{rest}")).find(|a| self.has(a)) {
                return Some(hit);
            }
        }
        const MOVED: &[&str] = &["nodePackages", "nodePackages_latest", "gnome", "gnome3", "haskellPackages", "python3Packages", "pythonPackages", "xfce", "mate", "libsForQt5", "plasma5Packages", "xorg"];
        let last = attr.rsplit('.').next()?;
        (MOVED.contains(&set) || pythonish).then_some(last).filter(|l| self.has(l)).map(String::from)
    }
}

// ---------- what a path is ----------

/// What `path` is (by name, then by what it says), and the Nix file to read.
pub fn detect(path: &Path, to: Option<Tier>) -> Result<(Kind, PathBuf)> {
    let file = if path.is_dir() {
        let names = ["flake.nix", "shell.nix", "home.nix", "configuration.nix", "default.nix"];
        names.iter().map(|n| path.join(n)).find(|f| f.is_file()).with_context(|| format!("no flake.nix, shell.nix, home.nix or configuration.nix in {}", path.display()))?
    } else if path.is_file() {
        path.to_path_buf()
    } else {
        bail!("{} doesn't exist", path.display());
    };
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let kind = match name {
        "flake.nix" | "shell.nix" => Kind::Flake,
        "home.nix" => Kind::HomeManager,
        "configuration.nix" => Kind::Nixos,
        _ => {
            let text = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
            // Comments don't count.
            let code: String = text.lines().map(|l| l.split('#').next().unwrap_or("")).collect::<Vec<_>>().join("\n");
            let has = |s: &str| code.contains(s);
            if (has("outputs") && has("inputs")) || has("mkShell") {
                Kind::Flake
            } else if ["environment.systemPackages", "boot.", "networking.", "users.users", "system.stateVersion", "fileSystems"].iter().any(|s| has(s)) {
                Kind::Nixos
            } else if has("home.") || has("programs.") || has("dconf.") || has("xdg.") {
                Kind::HomeManager
            } else {
                match to {
                    Some(Tier::System) => Kind::Nixos,
                    Some(Tier::User) => Kind::HomeManager,
                    Some(Tier::Project) => Kind::Flake,
                    None => bail!("can't tell whether {} is a flake, a home-manager or a NixOS configuration; say which with --to project|user|system", file.display()),
                }
            }
        }
    };
    Ok((kind, file))
}

// ---------- reading ----------

pub struct Reader {
    pub ev: Eval,
    pub found: Found,
    /// The tier asked for, which picks between a flake's outputs.
    want: Option<Tier>,
    shells: hm::Shells,
}

const SOFT: &str = "not a nixpkgs package, or not one nami can read without Nix";

impl Reader {
    pub fn new(want: Option<Tier>) -> Self {
        Reader { ev: Eval::default(), found: Found::default(), want, shells: hm::Shells::default() }
    }

    fn force(&self, t: &Thunk) -> Val {
        self.ev.force(t)
    }

    fn empty_args() -> Thunk {
        Thunk::val(Val::Attrs(Rc::new(BTreeMap::new())))
    }

    /// Read `file` as `kind`; returns what it turned out to be (a flake may
    /// hold a home-manager or NixOS configuration).
    pub fn read(&mut self, kind: Kind, file: &Path) -> Result<Kind> {
        let kind = match kind {
            Kind::Flake => self.read_flake(file)?,
            k => {
                let v = self.ev.eval_file(file)?;
                self.module(v, k, 0);
                k
            }
        };
        // What the evaluator noticed on the way.
        for (what, why) in self.ev.skipped.take() {
            self.found.skip(what, why);
        }
        for w in self.ev.warnings.take() {
            self.found.warn(w);
        }
        for p in self.ev.bin_refs.take() {
            self.found.warn(format!("{p}: used as ${{pkgs.{p}}}/bin/…, so it's added as a package"));
            self.found.package(&p, "a ${pkgs…}/bin path");
        }
        for p in self.ev.share_refs.take() {
            self.found.package(&p, "a ${pkgs…}/share path in shell code");
        }
        self.finish(kind, file);
        Ok(kind)
    }

    /// After everything's read, however it was.
    fn finish(&mut self, kind: Kind, file: &Path) {
        self.finish_shells();
        if kind == Kind::Nixos && self.want == Some(Tier::User) {
            self.found.skip(
                "the rest of the NixOS configuration",
                format!("the computer's: packages, services and /etc files go in system.nami (`sudo nami import {} --to system`)", file.display()),
            );
        }
    }

    fn read_flake(&mut self, file: &Path) -> Result<Kind> {
        let top = self.ev.eval_file(file)?;
        let top = match top {
            Val::Lambda(..) => self.ev.apply(top, Self::empty_args(), None),
            v => v,
        };
        if let Val::Shell(m) = &top {
            self.shell(m);
            return Ok(Kind::Flake);
        }
        let Val::Attrs(top) = top else { bail!("{} has no devShell (or outputs) nami can read", file.display()) };
        if let Some(Val::Attrs(inputs)) = top.get("inputs").map(|t| self.force(t)) {
            for (name, t) in inputs.iter() {
                let url = match self.force(t) {
                    Val::Str(s, _) => s,
                    Val::Attrs(m) => m.get("url").and_then(|t| self.force(t).str().map(String::from)).unwrap_or_default(),
                    _ => String::new(),
                };
                self.ev.set_input(name, &url);
            }
        }
        let Some(outputs) = top.get("outputs").map(|t| self.force(t)) else { bail!("{} has no outputs", file.display()) };
        if let Val::Lambda(p, _, _) = &outputs {
            if let Param::Pat { fields, .. } = &**p {
                for (name, _) in fields {
                    self.ev.set_input(name, "");
                }
            }
        }
        let out = self.ev.apply(outputs, Self::empty_args(), None);
        let Val::Attrs(out) = out else { bail!("couldn't read the outputs of {}", file.display()) };
        let order: &[Kind] = match self.want {
            // A NixOS configuration's home-manager users (yours) before a dev shell.
            Some(Tier::User) => &[Kind::HomeManager, Kind::Nixos, Kind::Flake],
            Some(Tier::System) => &[Kind::Nixos, Kind::Flake, Kind::HomeManager],
            _ => &[Kind::Flake, Kind::HomeManager, Kind::Nixos],
        };
        for kind in order {
            match kind {
                Kind::Flake => {
                    if let Some(shell) = self.find_shell(&out) {
                        match shell {
                            Val::Shell(m) => self.shell(&m),
                            Val::Unknown(t) => self.found.unconverted.push(Unconverted { what: t, why: "a dev shell nami couldn't read without Nix".into(), soft: true }),
                            _ => self.found.skip("devShell", "not a mkShell nami can read"),
                        }
                        return Ok(Kind::Flake);
                    }
                }
                Kind::HomeManager | Kind::Nixos => {
                    let key = if *kind == Kind::Nixos { "nixosConfigurations" } else { "homeConfigurations" };
                    let Some(Val::Attrs(configs)) = out.get(key).map(|t| self.force(t)) else { continue };
                    let Some(name) = self.pick(&configs, key) else { continue };
                    let config = self.force(&configs[&name]);
                    self.module(config, *kind, 0);
                    return Ok(*kind);
                }
            }
        }
        bail!("found no devShells, homeConfigurations or nixosConfigurations in {}", file.display())
    }

    /// Which of several named configurations: yours (by user or host name), else the first.
    fn pick(&mut self, configs: &BTreeMap<String, Thunk>, key: &str) -> Option<String> {
        let user = std::env::var("USER").unwrap_or_default();
        let host = std::fs::read_to_string("/etc/hostname").unwrap_or_default().trim().to_string();
        let name = configs
            .keys()
            .find(|n| !user.is_empty() && (**n == user || n.starts_with(&format!("{user}@"))))
            .or_else(|| configs.keys().find(|n| !host.is_empty() && **n == host))
            .or_else(|| configs.keys().next())?
            .clone();
        if configs.len() > 1 {
            let all: Vec<&str> = configs.keys().map(String::as_str).collect();
            self.found.warn(format!("{key} has {}; imported {name}", all.join(", ")));
        }
        Some(name)
    }

    /// The flake's dev shell for this platform: `devShells.<system>.default`
    /// (or the only one), or the older `devShell.<system>`.
    fn find_shell(&mut self, out: &BTreeMap<String, Thunk>) -> Option<Val> {
        for key in ["devShells", "devShell"] {
            let Some(Val::Attrs(systems)) = out.get(key).map(|t| self.force(t)) else { continue };
            let system = if systems.contains_key(SYSTEM) {
                SYSTEM.to_string()
            } else {
                let first = systems.keys().next()?.clone();
                self.found.warn(format!("{key} has nothing for {SYSTEM}; read {first}'s"));
                first
            };
            let v = self.force(&systems[&system]);
            if key == "devShell" {
                return Some(v);
            }
            let Val::Attrs(shells) = v else { return Some(v) };
            let name = if shells.contains_key("default") { "default".to_string() } else { shells.keys().next()?.clone() };
            for other in shells.keys().filter(|n| **n != name) {
                self.found.skip(format!("devShells.{system}.{other}"), "one shell per project file: only the default is imported");
            }
            return Some(self.force(&shells[&name]));
        }
        None
    }

    fn shell(&mut self, m: &BTreeMap<String, Thunk>) {
        for (k, t) in m {
            let v = self.force(t);
            match k.as_str() {
                "packages" | "nativeBuildInputs" | "buildInputs" | "propagatedBuildInputs" | "propagatedNativeBuildInputs" | "depsBuildBuild" => {
                    let items = self.ev.list(&v);
                    // Libraries to build against: a build environment, as `nix develop` gives.
                    if k == "buildInputs" && !items.is_empty() {
                        self.found.develop = true;
                    }
                    for item in items {
                        self.package(item, &format!("devShell {k}"), true);
                    }
                }
                "inputsFrom" => {
                    if !self.ev.list(&v).is_empty() {
                        self.found.skip("inputsFrom", "the build inputs of other derivations: list the tools you need under packages");
                    }
                }
                "shellHook" => self.shell_hook(&v),
                "name" | "pname" | "version" | "meta" | "passthru" | "stdenv" => {}
                "env" => {
                    if let Val::Attrs(env) = v {
                        for (name, t) in env.iter() {
                            let v = self.force(t);
                            self.env(name, &v, &format!("env.{name}"), false);
                        }
                    }
                }
                _ => self.env(k, &v, k, false),
            }
        }
    }

    /// `export NAME=value` lines become env; the rest is listed.
    fn shell_hook(&mut self, v: &Val) {
        let Val::Str(text, _) = v else {
            self.found.skip("shellHook", "couldn't read it");
            return;
        };
        let mut rest = Vec::new();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let export = line.strip_prefix("export ").and_then(|r| r.split_once('=')).filter(|(n, _)| valid_env_name(n));
            match export {
                Some((name, value)) => {
                    let value = value.trim();
                    let value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\''))).unwrap_or(value);
                    if value.contains(['$', '`']) {
                        rest.push(line);
                    } else {
                        self.found.env.entry(name.to_string()).or_insert_with(|| value.to_string());
                    }
                }
                None => rest.push(line),
            }
        }
        if !rest.is_empty() {
            let first: String = rest[0].chars().take(60).collect();
            let lines = if rest.len() == 1 { "a line".to_string() } else { format!("{} lines", rest.len()) };
            self.found.skip("shellHook", format!("{lines} of shell nami doesn't run when entering the project: `{first}`"));
        }
    }

    /// An environment variable's value, when it's plain text.
    fn env(&mut self, name: &str, v: &Val, what: &str, home: bool) {
        if !valid_env_name(name) {
            self.found.skip(what, "not a valid variable name");
            return;
        }
        let value = match v {
            Val::Null => return,
            Val::Str(s, true) => s.clone(),
            Val::Int(n) => n.to_string(),
            Val::Float(f) => f.to_string(),
            Val::Bool(b) => if *b { "1".into() } else { String::new() },
            Val::Path(p) => p.display().to_string(),
            Val::Str(..) => return self.found.skip(what, "refers to Nix store paths or options nami can't know"),
            // NixOS joins a list with ':' (PATH-like variables).
            Val::List(_) => match self.strs(v) {
                Some(items) => items.join(":"),
                None => return self.found.skip(what, "a list with values only Nix knows"),
            },
            Val::Unknown(t) => return self.found.skip(what, format!("computed by Nix (`{t}`)")),
            _ => return self.found.skip(what, "not a plain value"),
        };
        // Your own file may name your home; a project's is shared.
        let value = match (&self.ev.home_dir, home) {
            (Some(h), true) => value.replace("${HOME}", h).replace("$HOME", h),
            _ => value,
        };
        self.found.env.entry(name.to_string()).or_insert(value);
    }

    /// One item of a package list.
    fn package(&mut self, item: Val, origin: &str, tasks: bool) {
        match item {
            Val::Pkg(p) if !p.is_empty() => self.found.package(&p, origin),
            Val::WithPkgs(interp, items) => {
                self.found.package(&interp, origin);
                for t in items.iter() {
                    let v = self.force(t);
                    self.package(v, origin, tasks);
                }
            }
            Val::Script { name, text, exact, inputs } => {
                if tasks && exact {
                    self.found.tasks.entry(name).or_insert_with(|| text.trim_end().to_string());
                    for t in inputs {
                        let v = self.force(&t);
                        self.package(v, origin, tasks);
                    }
                } else {
                    let why = if tasks { "a script that uses store paths" } else { "a script built by Nix: keep it as a file in your dotfiles instead" };
                    self.found.skip(format!("{origin}: {name}"), why);
                }
            }
            Val::Null => {}
            Val::Unknown(t) => self.found.unconverted.push(Unconverted { what: t, why: SOFT.into(), soft: true }),
            Val::Str(s, _) => self.found.skip(s, "a string, not a package"),
            Val::Path(p) => self.found.skip(p.display().to_string(), "a local Nix file: a package only Nix can build"),
            _ => self.found.skip(origin, "not a package"),
        }
    }

    /// A NixOS or home-manager module (a function, a set, a file, a list of them).
    fn module(&mut self, v: Val, kind: Kind, depth: usize) {
        if depth > 8 {
            return self.found.skip("imports", "nested too deeply");
        }
        // Your user file from a NixOS configuration: only what's yours in it
        // (home-manager.users.<you>, users.users.<you>.packages, user units).
        let user_only = kind == Kind::Nixos && self.want == Some(Tier::User);
        let v = match v {
            Val::Lambda(..) => self.ev.apply(v, Self::empty_args(), None),
            Val::Path(p) => {
                if p.file_name().is_some_and(|n| n == "hardware-configuration.nix") {
                    if user_only {
                        return;
                    }
                    return self.found.skip("hardware-configuration.nix", "this machine's disks and drivers: NixOS-only");
                }
                match self.ev.import(&p) {
                    Ok(v) => return self.module(v, kind, depth + 1),
                    Err(e) => return self.found.skip(p.display().to_string(), format!("couldn't read it: {e:#}")),
                }
            }
            Val::Config(modules) => {
                for t in modules.iter() {
                    let m = self.force(t);
                    self.module(m, kind, depth + 1);
                }
                return;
            }
            v => v,
        };
        let m = match v {
            Val::Attrs(m) => m,
            Val::Null => return,
            Val::Unknown(_) if user_only => return,
            Val::Unknown(t) if t.starts_with("home-manager.nixosModules") || t.starts_with("home-manager.darwinModules") => return,
            Val::Unknown(t) if t.starts_with("nixos-hardware.") => return self.found.skip(t, "tweaks for this computer's model: NixOS-only (the distro's kernel and firmware cover most)"),
            Val::Unknown(t) => return self.found.skip(t, "a module nami couldn't read"),
            _ => return self.found.skip("a module", "not something nami can read"),
        };
        // `{ options = ...; config = ...; }`: the settings are in `config`.
        let structural = ["config", "options", "imports", "meta", "_file", "key", "disabledModules"];
        let m = if m.contains_key("config") && m.keys().all(|k| structural.contains(&k.as_str())) {
            if let Some(imports) = m.get("imports") {
                let v = self.force(imports);
                self.imports(&v, kind, depth);
            }
            match self.force(&m["config"]) {
                Val::Attrs(c) => c,
                _ => return,
            }
        } else {
            m
        };
        let nixos = kind == Kind::Nixos;
        for (k, t) in m.iter() {
            if user_only && !matches!(k.as_str(), "imports" | "home-manager" | "users" | "programs" | "systemd") {
                continue;
            }
            let v = self.force(t);
            if matches!(v, Val::Null) {
                continue;
            }
            match k.as_str() {
                "imports" => self.imports(&v, kind, depth),
                "options" | "meta" | "_file" | "key" | "_module" | "disabledModules" => {}
                "programs" => {
                    for (name, t) in self.attrs(&v) {
                        let pv = self.force(&t);
                        if user_only {
                            self.nixos_user_units(&name, &pv);
                        } else {
                            self.program(&name, &pv, kind);
                        }
                    }
                }
                "boot" if nixos => self.boot(&v),
                "time" | "networking" | "i18n" | "console" if nixos && self.nixos_os(k, &v) => {}
                "home" if !nixos => self.home(&v),
                "xdg" if !nixos => {
                    for (k, t) in self.attrs(&v) {
                        let xv = self.force(&t);
                        match k.as_str() {
                            "configFile" => self.files(&xv, ".config/", "xdg.configFile"),
                            "dataFile" => self.files(&xv, ".local/share/", "xdg.dataFile"),
                            "enable" => {}
                            _ => self.found.skip(format!("xdg.{k}"), "no nami equivalent"),
                        }
                    }
                }
                "dconf" if !nixos => {
                    if let Some(Val::Attrs(s)) = self.ev.select(&v, "settings") {
                        self.dconf(&s);
                    }
                }
                "environment" if nixos => self.environment(&v),
                "gtk" if !nixos => self.gtk(&v),
                "fonts" => {
                    for (k, t) in self.attrs(&v) {
                        let fv = self.force(&t);
                        match k.as_str() {
                            "packages" | "fonts" => {
                                for item in self.ev.list(&fv) {
                                    self.package(item, &format!("fonts.{k}"), false);
                                }
                            }
                            "fontconfig" | "enableDefaultPackages" => {}
                            _ => self.found.skip(format!("fonts.{k}"), "font configuration: no nami equivalent"),
                        }
                    }
                }
                "users" if nixos => self.users(&v),
                "home-manager" if nixos => {
                    let users = self.ev.select(&v, "users").map(|u| self.attrs(&u)).unwrap_or_default();
                    for (user, t) in users {
                        if self.want == Some(Tier::User) && self.is_me(&user, users_len(&v, &self.ev)) {
                            let hm = self.force(&t);
                            self.module(hm, Kind::HomeManager, depth + 1);
                        } else {
                            self.found.skip(format!("home-manager.users.{user}"), format!("{user}'s home-manager settings: import them into ~/user.nami with `nami import <file> --to user`"));
                        }
                    }
                }
                "nixpkgs" => {
                    if let Some(o) = self.ev.select(&v, "overlays") {
                        if !self.ev.list(&o).is_empty() {
                            self.found.skip("nixpkgs.overlays", "nami uses nixpkgs as it is; packages an overlay adds or changes aren't in the catalog");
                        }
                    }
                    if let Some(c) = self.ev.select(&v, "config") {
                        if let Some(Val::Bool(false)) = self.ev.select(&c, "allowUnfree") {
                            self.found.unfree = Some(false);
                        }
                        if let Some(p) = self.ev.select(&c, "permittedInsecurePackages").and_then(|p| self.strs(&p)).filter(|p| !p.is_empty()) {
                            self.found.skip(
                                "nixpkgs.config.permittedInsecurePackages",
                                format!("allowed here though nixpkgs marks them insecure ({}); nami's catalog leaves insecure packages out, so what needs them may not install", p.join(", ")),
                            );
                        }
                    }
                }
                "services" => {
                    for (s, t) in self.attrs(&v) {
                        let sv = self.force(&t);
                        self.service(&s, &sv, kind);
                    }
                }
                "system" if nixos => {
                    for (s, _) in self.attrs(&v).into_iter().filter(|(s, _)| s != "stateVersion") {
                        self.found.skip(format!("system.{s}"), "NixOS system configuration: no nami equivalent");
                    }
                }
                "news" | "manual" | "targets" | "submoduleSupport" if !nixos => {}
                "systemd" => self.systemd(&v, kind),
                "virtualisation" if nixos => self.virtualisation(&v),
                _ if nixos => self.found.skip(k, "NixOS system configuration: nami manages packages, not the OS"),
                _ => self.found.skip(k, "no nami equivalent"),
            }
        }
    }

    fn imports(&mut self, v: &Val, kind: Kind, depth: usize) {
        for item in self.ev.list(v) {
            self.module(item, kind, depth + 1);
        }
    }

    fn attrs(&self, v: &Val) -> Vec<(String, Thunk)> {
        match v {
            Val::Attrs(m) => m.iter().map(|(k, t)| (k.clone(), t.clone())).collect(),
            _ => Vec::new(),
        }
    }

    fn is_me(&self, user: &str, users: usize) -> bool {
        users == 1 || self.ev.user.as_deref() == Some(user)
    }

    fn home(&mut self, v: &Val) {
        for (k, t) in self.attrs(v) {
            let hv = self.force(&t);
            match k.as_str() {
                "packages" => {
                    for item in self.ev.list(&hv) {
                        self.package(item, "home.packages", false);
                    }
                }
                "sessionVariables" => {
                    for (name, t) in self.attrs(&hv) {
                        let v = self.force(&t);
                        self.env(&name, &v, &format!("home.sessionVariables.{name}"), true);
                    }
                }
                "shellAliases" => self.aliases(&hv, "home.shellAliases"),
                "file" => self.files(&hv, "", "home.file"),
                "pointerCursor" => self.pointer_cursor(&hv),
                // Directories before the rest of PATH: env: can say so.
                "sessionPath" => match self.strs(&hv) {
                    Some(dirs) if !dirs.is_empty() => {
                        let dirs = dirs.join(":");
                        let path = self.found.env.entry("PATH".into()).or_insert_with(|| "$PATH".into());
                        *path = format!("{dirs}:{path}");
                    }
                    Some(_) => {}
                    None => self.found.skip("home.sessionPath", "refers to values only Nix knows"),
                },
                "username" | "homeDirectory" | "stateVersion" | "enableNixpkgsReleaseCheck" => {}
                _ => self.found.skip(format!("home.{k}"), "no nami equivalent"),
            }
        }
    }

    fn aliases(&mut self, v: &Val, what: &str) {
        for (name, t) in self.attrs(v) {
            match self.force(&t) {
                Val::Str(s, true) => {
                    self.found.aliases.entry(name).or_insert(s);
                }
                Val::Null => {}
                _ => self.found.skip(format!("{what}.{name}"), "refers to values only Nix knows"),
            }
        }
    }

    /// `home.file` / `xdg.configFile`: `<name>.text` or `<name>.source`.
    fn files(&mut self, v: &Val, prefix: &str, what: &str) {
        for (name, t) in self.attrs(v) {
            let spec = self.force(&t);
            let what = format!("{what}.\"{name}\"");
            let Val::Attrs(s) = spec else {
                if !matches!(spec, Val::Null) {
                    self.found.skip(what, "not a { text } or { source }");
                }
                continue;
            };
            let get = |r: &Self, k: &str| s.get(k).map(|t| r.force(t));
            if matches!(get(self, "enable"), Some(Val::Bool(false))) {
                continue;
            }
            let mut rel = match get(self, "target") {
                Some(Val::Str(t, true)) => t,
                _ => name.clone(),
            };
            if let Some(h) = &self.ev.home_dir {
                rel = rel.strip_prefix(&format!("{h}/")).map(String::from).unwrap_or(rel);
            }
            let rel = format!("{prefix}{}", rel.trim_start_matches("~/"));
            if rel.starts_with('/') || rel.split('/').any(|c| c == "..") {
                self.found.skip(what, "outside your home directory");
                continue;
            }
            let content = match (get(self, "text"), get(self, "source")) {
                (Some(Val::Str(text, true)), _) => Value::String(text),
                (Some(Val::Str(..)), _) => {
                    self.found.skip(format!("{what}.text"), "refers to Nix store paths or options nami can't know");
                    continue;
                }
                (_, Some(Val::Path(p))) => {
                    if !p.exists() {
                        self.found.warn(format!("{what}: {} doesn't exist", p.display()));
                    }
                    source(&p.display().to_string())
                }
                (_, Some(Val::Str(p, true))) if p.starts_with('/') && !p.starts_with("/nix/store") => source(&p),
                (_, Some(_)) => {
                    self.found.skip(format!("{what}.source"), "built by Nix, not a file next to your config");
                    continue;
                }
                _ => {
                    self.found.skip(what, "neither text nor source");
                    continue;
                }
            };
            if matches!(get(self, "executable"), Some(Val::Bool(true))) {
                self.found.warn(format!("{what}: nami doesn't make home files executable"));
            }
            if get(self, "onChange").is_some() {
                self.found.skip(format!("{what}.onChange"), "nami doesn't run commands when a file changes");
            }
            self.found.files.entry(rel).or_insert(content);
        }
    }

    /// `dconf.settings."org/gnome/desktop/interface".color-scheme` ->
    /// `settings: org.gnome.desktop.interface: color-scheme`.
    fn dconf(&mut self, m: &BTreeMap<String, Thunk>) {
        for (path, t) in m {
            let schema = path.trim_matches('/').replace('/', ".");
            for (key, t) in self.attrs(&self.force(t)) {
                let v = self.force(&t);
                let what = format!("dconf.settings.\"{path}\".{key}");
                match self.ev.to_yaml(&v) {
                    Ok(y) if crate::settings::gvariant(&y).is_ok() => {
                        self.found.settings.entry(schema.clone()).or_default().entry(key).or_insert(y);
                    }
                    Ok(_) => self.found.skip(what, "not text, a number, true/false or a list"),
                    Err(why) => self.found.skip(what, format!("{why}: nami settings are text, numbers, true/false or lists")),
                }
            }
        }
    }

    fn environment(&mut self, v: &Val) {
        for (k, t) in self.attrs(v) {
            let ev = self.force(&t);
            match k.as_str() {
                "systemPackages" => {
                    for item in self.ev.list(&ev) {
                        self.package(item, "environment.systemPackages", false);
                    }
                }
                "variables" | "sessionVariables" => {
                    for (name, t) in self.attrs(&ev) {
                        let v = self.force(&t);
                        let mine = self.found.env.remove(&name);
                        self.env(&name, &v, &format!("environment.{k}.{name}"), false);
                        if let Some(value) = self.found.env.remove(&name) {
                            self.found.system_env.entry(name.clone()).or_insert(value);
                        }
                        if let Some(m) = mine {
                            self.found.env.insert(name, m);
                        }
                    }
                }
                "shellAliases" => self.aliases(&ev, "environment.shellAliases"),
                "etc" => self.etc(&ev),
                _ => self.found.skip(format!("environment.{k}"), "NixOS system configuration: no nami equivalent"),
            }
        }
    }

    fn users(&mut self, v: &Val) {
        let users = self.ev.select(v, "users").map(|u| self.attrs(&u)).unwrap_or_default();
        let n = users.len();
        let user_only = self.want == Some(Tier::User);
        for (user, t) in users {
            let uv = self.force(&t);
            if let Some(sh) = self.ev.select(&uv, "shell") {
                if !matches!(sh, Val::Null) && (!user_only || self.is_me(&user, n)) {
                    let name = match &sh {
                        Val::Pkg(p) => p.clone(),
                        v => v.str().map(|s| s.rsplit('/').next().unwrap_or(s).to_string()).unwrap_or_else(|| "it".into()),
                    };
                    if user_only && crate::home::LOGIN_SHELLS.contains(&name.as_str()) {
                        // Yours: terminals switch to it, no chsh needed.
                        self.found.login_shell = Some(name.clone());
                        self.found.package(&name, &format!("users.users.{user}.shell"));
                    } else if !(user_only && name == "bash") {
                        self.found.skip(
                            format!("users.users.{user}.shell"),
                            format!("the login shell: nami doesn't change it; run `chsh -s \"$(command -v {name})\"` (the path has to be in /etc/shells)"),
                        );
                    }
                }
            }
            let groups = self.ev.select(&uv, "extraGroups").and_then(|g| self.strs(&g)).unwrap_or_default();
            // A person (not a service's account): on a computer nami made,
            // one of os.users, an admin if in wheel (another distro has its own).
            if !user_only && matches!(self.ev.select(&uv, "isNormalUser"), Some(Val::Bool(true))) {
                let mut me = Mapping::new();
                if groups.iter().any(|g| g == "wheel") {
                    me.insert("admin".into(), Value::Bool(true));
                }
                if let Some(Val::Str(d, true)) = self.ev.select(&uv, "description") {
                    me.insert("name".into(), Value::String(d));
                }
                self.found.top.push((path(&["users", &user]), Value::Mapping(me)));
            }
            if !user_only || self.is_me(&user, n) {
                let what = format!("users.users.{user}.extraGroups");
                // A group that's a nami file's people (docker's).
                let (feature, g): (Vec<String>, Vec<String>) = groups.into_iter().partition(|g| feature_group(g).is_some());
                for f in feature {
                    self.found.group_members.push((f, user.clone(), what.clone()));
                }
                if !g.is_empty() {
                    self.found.skip(what, format!("accounts are the distro's: `sudo usermod -aG {} {user}` (a group that doesn't exist yet comes with the package that uses it)", g.join(",")));
                }
            }
            let Some(pkgs) = self.ev.select(&uv, "packages") else { continue };
            if self.want == Some(Tier::User) && self.is_me(&user, n) {
                for item in self.ev.list(&pkgs) {
                    self.package(item, &format!("users.users.{user}.packages"), false);
                }
            } else if !self.ev.list(&pkgs).is_empty() {
                self.found.skip(format!("users.users.{user}.packages"), format!("{user}'s own apps: import them into their ~/user.nami with `nami import <file> --to user`"));
            }
        }
        if !user_only {
            self.found.skip("users", "accounts are the distro's; on a computer nami made, the people are users: (`nami usb` asks for their passwords), the rest (service accounts, uids, passwords) is NixOS-only");
        }
    }

    /// `virtualisation.*`: Docker is nami's docker package, with its
    /// settings; Podman a package. The rest (VMs, libvirtd...) are listed.
    fn virtualisation(&mut self, v: &Val) {
        for (k, t) in self.attrs(v) {
            let x = self.force(&t);
            match k.as_str() {
                "docker" => self.docker(&x),
                "podman" => self.podman(&x),
                _ => self.found.skip(format!("virtualisation.{k}"), "NixOS system configuration: nami manages packages, not the OS"),
            }
        }
    }

    /// `virtualisation.docker`: `- docker` (dockerd, its socket and
    /// group, compose and buildx), and its settings as /etc/docker/daemon.json.
    fn docker(&mut self, v: &Val) {
        let Val::Attrs(m) = v else { return };
        let get = |r: &Self, k: &str| m.get(k).map(|t| r.force(t)).filter(|v| !matches!(v, Val::Null));
        let rootless = get(self, "rootless");
        let rootless_on = rootless.as_ref().and_then(|r| self.ev.select(r, "enable")).is_some_and(|e| matches!(e, Val::Bool(true)));
        if !matches!(get(self, "enable"), Some(Val::Bool(true))) && !rootless_on {
            return;
        }
        self.found.include("docker");
        self.found.warn("virtualisation.docker: `- docker` (nami's docker example, copied next to the computer's folder) runs Docker on a computer nami made (`nami usb`); on another distro install its own (docker.io), and its settings are that one's /etc/docker/daemon.json all the same");
        if rootless_on {
            self.found.skip("virtualisation.docker.rootless", "rootless Docker isn't set up on nami's computer: dockerd runs as root, as on Ubuntu, and the docker group's people (users' `groups: [docker]`) use it without sudo");
        }
        // daemon.json: NixOS's own options are some of its keys.
        let mut settings = Mapping::new();
        for (opt, key) in [("storageDriver", "storage-driver"), ("logDriver", "log-driver"), ("liveRestore", "live-restore")] {
            if let Some(x) = get(self, opt) {
                match self.ev.to_yaml(&x) {
                    Ok(y) => {
                        settings.insert(key.into(), y);
                    }
                    Err(why) => self.found.skip(format!("virtualisation.docker.{opt}"), why),
                }
            }
        }
        let daemon = get(self, "daemon").and_then(|d| self.ev.select(&d, "settings"));
        match daemon.map(|d| self.ev.to_yaml(&d)) {
            None => {}
            Some(Ok(Value::Mapping(s))) => settings.extend(s),
            Some(Ok(_)) => self.found.skip("virtualisation.docker.daemon.settings", "not a set of settings"),
            Some(Err(why)) => self.found.skip("virtualisation.docker.daemon.settings", why),
        }
        if !settings.is_empty() {
            let file = Mapping::from_iter([("content".into(), Value::Mapping(settings)), ("restarts".into(), Value::String("docker.service".into()))]);
            self.found.etc_files.entry("/etc/docker/daemon.json".into()).or_insert(Value::Mapping(file));
        }
        let why = |s: &str| s.to_string();
        let notes: [(&str, String); 6] = [
            ("enableOnBoot", why("dockerd starts with the computer and through its socket, as on Ubuntu")),
            ("autoPrune", why("no timer prunes images: `docker system prune` (or a service with `at:` in system.nami)")),
            ("extraOptions", why("dockerd's flags: give their daemon.json keys instead (`files: /etc/docker/daemon.json: content:`)")),
            ("listenOptions", why("dockerd listens on /run/docker.sock: its \"hosts\" key in daemon.json for more")),
            ("extraPackages", why("dockerd's PATH is nixpkgs' docker's own")),
            ("enableNvidia", why("NVIDIA's container toolkit isn't set up by nami")),
        ];
        for (opt, why) in notes {
            let set = match get(self, opt) {
                None | Some(Val::Bool(false)) => false,
                Some(Val::List(l)) => !l.is_empty(),
                Some(Val::Attrs(a)) => !a.is_empty(),
                _ => true,
            };
            // (enableOnBoot is on unless said otherwise: only `false` is news.)
            let set = if opt == "enableOnBoot" { matches!(get(self, opt), Some(Val::Bool(false))) } else { set };
            if set {
                self.found.skip(format!("virtualisation.docker.{opt}"), why);
            }
        }
        if let Some(Val::Pkg(p)) = get(self, "package") {
            if p != "docker" && !p.starts_with("docker_") {
                self.found.skip("virtualisation.docker.package", format!("{p}: nami's computer runs nixpkgs' docker"));
            }
        }
    }

    /// `virtualisation.podman`: the package (`sudo podman` on nami's
    /// computer, and as you: nami's examples/packages/podman); not its settings.
    fn podman(&mut self, v: &Val) {
        let Val::Attrs(m) = v else { return };
        let get = |r: &Self, k: &str| m.get(k).map(|t| r.force(t)).filter(|v| !matches!(v, Val::Null));
        if !matches!(get(self, "enable"), Some(Val::Bool(true))) {
            return;
        }
        self.found.package("podman", "virtualisation.podman");
        if matches!(get(self, "dockerCompat"), Some(Val::Bool(true))) {
            self.found.skip("virtualisation.podman.dockerCompat", "a `docker` command that's podman: `alias docker=podman` in your user.nami's aliases:");
        }
        for (k, _) in m.iter().filter(|(k, _)| !matches!(k.as_str(), "enable" | "dockerCompat" | "package")) {
            let set = match get(self, k) {
                None | Some(Val::Bool(false)) => false,
                Some(Val::List(l)) => !l.is_empty(),
                Some(Val::Attrs(a)) => !a.is_empty(),
                _ => true,
            };
            if set {
                self.found.skip(format!("virtualisation.podman.{k}"), "a NixOS module setting nami can't reproduce");
            }
        }
    }

    /// `programs.<name>`: its package when enabled, and the options nami has
    /// an equivalent for.
    fn program(&mut self, name: &str, v: &Val, kind: Kind) {
        let Val::Attrs(m) = v else {
            if !matches!(v, Val::Null) {
                self.found.skip(format!("programs.{name}"), "not readable");
            }
            return;
        };
        let enabled = match m.get("enable").map(|t| self.force(t)) {
            Some(Val::Bool(b)) => Some(b),
            Some(Val::Null) | None => None,
            Some(_) => {
                self.found.warn(format!("couldn't tell whether programs.{name} is enabled; took it as yes"));
                Some(true)
            }
        };
        if enabled == Some(false) {
            return;
        }
        if enabled == Some(true) {
            let pkg = match m.get("package").map(|t| self.force(t)) {
                Some(Val::Pkg(p)) => Some(p),
                Some(Val::Null) => None,
                Some(_) => {
                    self.found.skip(format!("programs.{name}.package"), "not a nixpkgs package");
                    None
                }
                None => program_package(name, kind).map(String::from),
            };
            if let Some(p) = pkg {
                self.found.package(&p, &format!("programs.{name}.enable"));
            }
        }
        self.shell_program_start(name, enabled, kind);
        // Programs whose options are one config file, written whole.
        if enabled != Some(false) && (name == "tmux" || (name == "ssh" && kind == Kind::HomeManager)) {
            if name == "tmux" {
                self.tmux(m, kind);
            } else {
                self.ssh_config(m);
            }
            return;
        }
        let mut git = Mapping::new();
        let shell = matches!(name, "bash" | "zsh" | "fish");
        let hm = kind == Kind::HomeManager;
        // A shell's options are shell code, which may name package files
        // (`${pkgs.x}/share/...`), down to its plugins' sources.
        self.ev.shell_text.set(shell);
        for (k, t) in m.iter().filter(|(k, _)| *k != "enable" && *k != "package") {
            let v = self.force(t);
            if self.shell_option(name, k, &v, kind) {
                continue;
            }
            // Off, empty, unset: says nothing.
            let empty = match &v {
                Val::Null | Val::Bool(false) => true,
                Val::List(l) => l.is_empty(),
                Val::Attrs(a) => a.is_empty(),
                Val::Str(s, _) => s.is_empty(),
                _ => false,
            };
            if empty {
                continue;
            }
            let what = format!("programs.{name}.{k}");
            let on = matches!(v, Val::Bool(true));
            match (name, k.as_str()) {
                (_, "shellAliases") if shell => self.aliases(&v, &what),
                ("fish", "shellAbbrs") => self.aliases(&v, &what),
                ("zsh", "sessionVariables") => {
                    for (n, t) in self.attrs(&v) {
                        let ev = self.force(&t);
                        self.env(&n, &ev, &format!("{what}.{n}"), true);
                    }
                }
                ("git", _) if self.git(&mut git, k, &v) => {}
                (_, k) if kind == Kind::Nixos && self.nixos_program_option(name, k, &v) => {}
                ("zsh", k) if hm && self.zsh_option(k, &v) => {}
                ("fzf", k) if self.fzf_option(k, &v) => {}
                ("bash", k) if hm && self.bash_option(k, &v) => {}
                ("fish", k) if hm && self.fish_option(k, &v) => {}
                ("neovim", "viAlias") if on => {
                    self.found.aliases.entry("vi".into()).or_insert("nvim".into());
                }
                ("neovim", "vimAlias") if on => {
                    self.found.aliases.entry("vim".into()).or_insert("nvim".into());
                }
                ("neovim" | "vim" | "helix" | "emacs" | "kakoune" | "nano", "defaultEditor") if on => {
                    let cmd = match name {
                        "neovim" => "nvim",
                        "helix" => "hx",
                        "kakoune" => "kak",
                        n => n,
                    };
                    self.found.env.entry("EDITOR".into()).or_insert(cmd.into());
                }
                ("starship", "settings") => self.settings_file(".config/starship.toml", &v, &what),
                ("alacritty", "settings") => self.settings_file(".config/alacritty/alacritty.toml", &v, &what),
                ("helix", "settings") => self.settings_file(".config/helix/config.toml", &v, &what),
                ("helix", "languages") => self.settings_file(".config/helix/languages.toml", &v, &what),
                (_, k) if k.starts_with("enable") && k.ends_with("Integration") => {
                    self.found.skip(what, "shell integration: add the tool's init line to your shell's rc yourself")
                }
                _ => self.found.skip(what, "no nami equivalent"),
            }
        }
        self.ev.shell_text.set(false);
        if kind == Kind::Nixos && enabled == Some(true) {
            self.nixos_program_extras(name, m);
        }
        if !git.is_empty() {
            // [user] first, as people write it.
            if let Some(user) = git.remove("user") {
                git = Mapping::from_iter(std::iter::once((Value::String("user".into()), user)).chain(git));
            }
            match self.found.files.get(".gitconfig") {
                Some(_) => self.found.warn("programs.git: kept home.file.\".gitconfig\" rather than writing one from programs.git"),
                None => {
                    self.found.files.insert(".gitconfig".into(), Value::Mapping(git));
                }
            }
        }
    }

    /// programs.git's options as gitconfig sections (nami writes .gitconfig
    /// from nested YAML). False: not one it knows.
    fn git(&mut self, cfg: &mut Mapping, key: &str, v: &Val) -> bool {
        let what = format!("programs.git.{key}");
        let mut set = |section: &str, k: &str, val: Value| {
            let s = cfg.entry(Value::String(section.into())).or_insert_with(|| Value::Mapping(Mapping::new()));
            if let Value::Mapping(s) = s {
                s.insert(Value::String(k.into()), val);
            }
        };
        match key {
            "userName" | "userEmail" => match v.str() {
                Some(s) => set("user", if key == "userName" { "name" } else { "email" }, Value::String(s.into())),
                None => self.found.skip(what, "refers to values only Nix knows"),
            },
            "aliases" => {
                for (n, t) in self.attrs(v) {
                    match self.force(&t) {
                        Val::Str(s, true) => set("alias", &n, Value::String(s)),
                        _ => self.found.skip(format!("{what}.{n}"), "refers to values only Nix knows"),
                    }
                }
            }
            "extraConfig" | "settings" => match self.ev.to_yaml(v) {
                Ok(Value::Mapping(m)) => merge_yaml(cfg, m),
                Ok(_) => self.found.skip(what, "written as text: copy it into files: ~/.gitconfig"),
                Err(why) => self.found.skip(what, why),
            },
            "ignores" => {
                let lines: Option<Vec<String>> = self.ev.list(v).iter().map(|v| v.str().map(String::from)).collect();
                match lines {
                    Some(l) => {
                        self.found.files.entry(".config/git/ignore".into()).or_insert(Value::String(l.join("\n") + "\n"));
                    }
                    None => self.found.skip(what, "refers to values only Nix knows"),
                }
            }
            "signing" => {
                if let Some(key) = self.ev.select(v, "key").and_then(|k| k.str().map(String::from)) {
                    set("user", "signingKey", Value::String(key));
                }
                if let Some(Val::Bool(true)) = self.ev.select(v, "signByDefault") {
                    set("commit", "gpgSign", Value::Bool(true));
                    set("tag", "gpgSign", Value::Bool(true));
                }
            }
            "lfs" => {
                if let Some(Val::Bool(true)) = self.ev.select(v, "enable") {
                    self.found.package("git-lfs", "programs.git.lfs.enable");
                    let lfs: Mapping = serde_yaml::from_str("lfs: { clean: git-lfs clean -- %f, smudge: git-lfs smudge -- %f, process: git-lfs filter-process, required: true }").unwrap();
                    merge_yaml(cfg, Mapping::from_iter([(Value::String("filter".into()), Value::Mapping(lfs))]));
                }
            }
            _ => return false,
        }
        true
    }

    fn settings_file(&mut self, rel: &str, v: &Val, what: &str) {
        match self.ev.to_yaml(v) {
            Ok(y @ Value::Mapping(_)) => {
                self.found.files.entry(rel.into()).or_insert(y);
            }
            Ok(_) => self.found.skip(what, "not a set of settings"),
            Err(why) => self.found.skip(what, why),
        }
    }

    // ---------- services and /etc files ----------

    /// A `services:` entry, checked as `nami apply` would check it.
    fn add_service(&mut self, name: &str, value: Value, what: &str, user_units: bool) {
        match crate::services::parse(name, &value).and_then(|s| crate::services::check_tier(&s, user_units)) {
            Ok(_) => {
                let map = if user_units { &mut self.found.user_services } else { &mut self.found.services };
                map.entry(name.to_string()).or_insert(value);
            }
            Err(e) => self.found.skip(what, format!("{e:#}")),
        }
    }

    /// Exact strings: one, or a list of them.
    fn strs(&self, v: &Val) -> Option<Vec<String>> {
        match v {
            Val::Str(s, true) => Some(vec![s.clone()]),
            Val::List(_) => self.ev.list(v).iter().map(|i| i.str().map(String::from)).collect(),
            _ => None,
        }
    }

    /// `services.<name>`: a NixOS or home-manager module. What nami can run
    /// is a unit a package ships, so that's what the ones with such a unit
    /// become; the module's other settings (it writes the unit from them) are
    /// listed, except where they're a plain config file.
    fn service(&mut self, name: &str, v: &Val, kind: Kind) {
        let what = format!("services.{name}");
        let Val::Attrs(m) = v else {
            if !matches!(v, Val::Null) {
                self.found.skip(what, "not readable");
            }
            return;
        };
        let get = |r: &Self, k: &str| m.get(k).map(|t| r.force(t)).filter(|v| !matches!(v, Val::Null));
        let enabled = match get(self, "enable") {
            Some(Val::Bool(b)) => b,
            None => false,
            Some(_) => return self.found.skip(format!("{what}.enable"), "couldn't tell whether it's enabled"),
        };
        if matches!(get(self, "enable"), Some(Val::Bool(false))) {
            return;
        }
        let nixos = kind == Kind::Nixos;
        if nixos && name == "udev" {
            return self.udev(&m);
        }
        if nixos && name == "xserver" {
            self.xkb_hint(v);
        }
        if nixos && DISTRO_SERVICES.contains(&name) {
            // Whether or not it says `enable` (displayManager.gdm.enable is a level down).
            return self.found.skip(what, "part of the desktop or the OS, which the distro sets up: nami leaves it to the distro");
        }
        let mut handled: Vec<&str> = vec!["enable"];
        match (nixos, name) {
            (true, "openssh") => {
                handled.extend(["settings", "ports"]);
                if enabled {
                    self.found.skip(&what, "NixOS writes sshd.service itself and nixpkgs' openssh ships no unit: use the distro's SSH server (openssh-server); its settings are converted to an sshd_config.d file");
                }
                let (settings, ports) = (get(self, "settings"), get(self, "ports"));
                self.sshd_config(settings, ports);
            }
            (true, "tailscale") if enabled => {
                handled.extend(["port", "extraDaemonFlags", "package"]);
                let package = match get(self, "package") {
                    Some(Val::Pkg(p)) => p,
                    _ => "tailscale".into(),
                };
                // Its unit runs `tailscaled --port=${PORT} $FLAGS`; NixOS's default port.
                let port = match get(self, "port") {
                    None => Some("41641".to_string()),
                    Some(Val::Int(n)) => Some(n.to_string()),
                    Some(_) => None,
                };
                let Some(port) = port else { return self.found.skip(format!("{what}.port"), "refers to values only Nix knows") };
                let mut env = Mapping::new();
                env.insert("PORT".into(), Value::String(port));
                match get(self, "extraDaemonFlags").map(|f| self.strs(&f)) {
                    None => {}
                    Some(Some(flags)) if !flags.is_empty() => {
                        env.insert("FLAGS".into(), Value::String(flags.join(" ")));
                    }
                    Some(Some(_)) => {}
                    Some(None) => self.found.skip(format!("{what}.extraDaemonFlags"), "refers to values only Nix knows"),
                }
                if let Some(Val::Str(u, true)) = get(self, "permitCertUid") {
                    env.insert("TS_PERMIT_CERT_UID".into(), Value::String(u));
                }
                if let Some(Val::Str(i, true)) = get(self, "interfaceName") {
                    let flags = env.entry("FLAGS".into()).or_insert(Value::String(String::new()));
                    if let Value::String(f) = flags {
                        *f = format!("{f} --tun={i}").trim().to_string();
                    }
                }
                handled.extend(["permitCertUid", "interfaceName", "useRoutingFeatures", "extraUpFlags", "extraSetFlags", "authKeyFile", "authKeyParameters", "openFirewall"]);
                // Routing a subnet or an exit node: forwarding on, as NixOS turns it on.
                if let Some(Val::Str(r, true)) = get(self, "useRoutingFeatures") {
                    if r == "server" || r == "both" {
                        let e = self.found.etc_files.entry(nixos::SYSCTL.into()).or_insert(Value::String(String::new()));
                        if let Value::String(t) = e {
                            t.push_str("net.ipv4.conf.all.forwarding = 1\nnet.ipv6.conf.all.forwarding = 1\n");
                        }
                    }
                    if r == "client" || r == "both" {
                        self.found.skip(format!("{what}.useRoutingFeatures"), "using subnet routes or an exit node: set the firewall's reverse-path check to loose, if the distro's firewall has one");
                    }
                }
                let flags: Vec<String> = ["extraUpFlags", "extraSetFlags"].iter().filter_map(|k| get(self, k).and_then(|v| self.strs(&v))).flatten().collect();
                if !flags.is_empty() || get(self, "authKeyFile").is_some() {
                    let key = if get(self, "authKeyFile").is_some() { " --auth-key=file:<the key file>" } else { "" };
                    self.found.skip(format!("{what}.extraUpFlags"), format!("NixOS runs `tailscale up` for you; once, by hand: `sudo tailscale up {}{key}`", flags.join(" ")));
                }
                if matches!(get(self, "openFirewall"), Some(Val::Bool(true))) {
                    self.found.skip(format!("{what}.openFirewall"), "nami doesn't manage the firewall: open UDP 41641 with the distro's firewall tool");
                }
                let value = Value::Mapping(Mapping::from_iter([("package".into(), Value::String(package)), ("env".into(), Value::Mapping(env))]));
                self.add_service("tailscaled", value, &what, false);
            }
            (true, "syncthing") if enabled => {
                handled.push("user");
                match get(self, "user") {
                    // The package's template unit, run as that user.
                    Some(Val::Str(u, true)) => self.add_service(&format!("syncthing@{u}"), Value::Bool(true), &what, false),
                    None => self.found.skip(&what, "NixOS runs it as a `syncthing` user it creates, and nami doesn't create users: add `- syncthing@<user>` under services: for the user it should run as"),
                    Some(_) => self.found.skip(format!("{what}.user"), "refers to values only Nix knows"),
                }
            }
            (true, "postgresql") if enabled => {
                return self.found.skip(what, "NixOS writes postgresql's unit (initdb, the data directory, ensureDatabases...) and the package ships none: use the distro's postgresql, or write a service with run:");
            }
            (false, "syncthing") if enabled => self.add_service("syncthing", Value::Bool(true), &what, true),
            (true, n) if enabled && DISTRO_SERVICES.contains(&n) => {
                return self.found.skip(what, "part of the desktop or the OS, which the distro sets up: nami leaves it to the distro");
            }
            _ if enabled => {
                let why = if nixos {
                    format!("a NixOS module: it writes the unit from settings nami can't reproduce. If a package ships a unit for it, `services: [<unit>]` runs that (`nami apply` says if the package has none), e.g. `- {name}`")
                } else {
                    format!("a home-manager module: it writes the unit from settings nami can't reproduce. If a package ships a user unit for it, `services: [<unit>]` in your user.nami runs that, e.g. `- {name}`")
                };
                return self.found.skip(what, why);
            }
            _ => return self.found.skip(what, "settings for a service that isn't enabled here: no nami equivalent"),
        }
        let module = if nixos { "a NixOS module setting nami can't reproduce (the module writes the unit from it)" } else { "a home-manager module setting nami can't reproduce" };
        for (k, t) in m.iter().filter(|(k, _)| !handled.contains(&k.as_str())) {
            let empty = match self.force(t) {
                Val::Null | Val::Bool(false) => true,
                Val::List(l) => l.is_empty(),
                Val::Attrs(a) => a.is_empty(),
                _ => false,
            };
            if !empty {
                self.found.skip(format!("{what}.{k}"), module);
            }
        }
    }

    /// `services.openssh.settings` (and `ports`) as sshd_config lines, the
    /// way NixOS writes them: yes/no, lists joined. Only plain values.
    fn sshd_config(&mut self, settings: Option<Val>, ports: Option<Val>) {
        let mut lines = Vec::new();
        if let Some(p) = ports {
            for item in self.ev.list(&p) {
                match item {
                    Val::Int(n) => lines.push(format!("Port {n}")),
                    _ => self.found.skip("services.openssh.ports", "refers to values only Nix knows"),
                }
            }
        }
        match settings {
            Some(Val::Attrs(s)) => {
                for (k, t) in s.iter() {
                    let v = self.force(t);
                    let what = format!("services.openssh.settings.{k}");
                    let value = match &v {
                        Val::Null => continue,
                        Val::Bool(b) => (if *b { "yes" } else { "no" }).to_string(),
                        Val::Int(n) => n.to_string(),
                        Val::Str(s, true) => s.clone(),
                        Val::List(_) => {
                            let items: Option<Vec<String>> = self.ev.list(&v).iter().map(|i| match i {
                                Val::Str(s, true) => Some(s.clone()),
                                Val::Int(n) => Some(n.to_string()),
                                _ => None,
                            }).collect();
                            match items {
                                Some(i) if i.is_empty() => continue,
                                Some(i) => i.join(if matches!(k.as_str(), "Ciphers" | "Macs" | "KexAlgorithms") { "," } else { " " }),
                                None => {
                                    self.found.skip(what, "refers to values only Nix knows");
                                    continue;
                                }
                            }
                        }
                        _ => {
                            self.found.skip(what, "refers to values only Nix knows");
                            continue;
                        }
                    };
                    if value.contains('\n') || !k.chars().all(|c| c.is_ascii_alphanumeric()) {
                        self.found.skip(what, "not an sshd_config line");
                        continue;
                    }
                    lines.push(format!("{k} {value}"));
                }
            }
            None => {}
            Some(_) => self.found.skip("services.openssh.settings", "not readable"),
        }
        if lines.is_empty() {
            return;
        }
        let text = format!("# services.openssh from the NixOS configuration (nami import)\n{}\n", lines.join("\n"));
        self.found.etc_files.entry(SSHD_DROPIN.into()).or_insert(Value::String(text));
        self.found.warn(format!("{SSHD_DROPIN}: sshd reads it only if /etc/ssh/sshd_config has `Include /etc/ssh/sshd_config.d/*.conf` (Debian, Ubuntu, Fedora and Arch have it); reload sshd after `nami apply`"));
    }

    /// NixOS `environment.etc."<path>"`: `text` or `source`, with `mode`.
    fn etc(&mut self, v: &Val) {
        for (name, t) in self.attrs(v) {
            let spec = self.force(&t);
            let what = format!("environment.etc.\"{name}\"");
            let Val::Attrs(s) = spec else {
                if !matches!(spec, Val::Null) {
                    self.found.skip(what, "not a { text } or { source }");
                }
                continue;
            };
            let get = |r: &Self, k: &str| s.get(k).map(|t| r.force(t)).filter(|v| !matches!(v, Val::Null));
            if matches!(get(self, "enable"), Some(Val::Bool(false))) {
                continue;
            }
            let rel = match get(self, "target") {
                Some(Val::Str(t, true)) => t,
                _ => name.clone(),
            };
            let path = match crate::etc::check_path(&format!("/etc/{}", rel.trim_start_matches('/'))) {
                Ok(p) => p,
                Err(e) => {
                    let msg = e.to_string();
                    self.found.skip(what, msg.split_once(": ").map(|(_, why)| why.to_string()).unwrap_or(msg));
                    continue;
                }
            };
            let mode = match get(self, "mode") {
                None => None,
                Some(Val::Str(m, true)) if m == "symlink" || m == "direct-symlink" => None,
                Some(Val::Str(m, true)) if u32::from_str_radix(&m, 8).is_ok_and(|n| n <= 0o777) => Some(m),
                Some(_) => {
                    self.found.skip(format!("{what}.mode"), "not a mode nami can set (octal, no setuid bits)");
                    None
                }
            };
            for k in ["user", "group", "uid", "gid"] {
                match get(self, k) {
                    None | Some(Val::Int(0)) => {}
                    Some(Val::Str(u, true)) if u == "root" || u == "+0" => {}
                    Some(_) => self.found.skip(format!("{what}.{k}"), "nami's /etc files are owned by root"),
                }
            }
            let content = match (get(self, "text"), get(self, "source")) {
                (Some(Val::Str(text, true)), _) => Value::String(text),
                (Some(Val::Str(..)), _) => {
                    self.found.skip(format!("{what}.text"), "refers to Nix store paths or options nami can't know");
                    continue;
                }
                (_, Some(Val::Path(p))) => {
                    if !p.exists() {
                        self.found.warn(format!("{what}: {} doesn't exist", p.display()));
                    }
                    source(&p.display().to_string())
                }
                (_, Some(Val::Str(p, true))) if p.starts_with('/') && !p.starts_with("/nix/store") => source(&p),
                (_, Some(_)) => {
                    self.found.skip(format!("{what}.source"), "built by Nix, not a file next to your config");
                    continue;
                }
                _ => {
                    self.found.skip(what, "neither text nor source");
                    continue;
                }
            };
            let value = match (mode, content) {
                (None, c) => c,
                (Some(m), Value::Mapping(mut src)) => {
                    src.insert("mode".into(), Value::String(m));
                    Value::Mapping(src)
                }
                (Some(m), c) => Value::Mapping(Mapping::from_iter([("content".into(), c), ("mode".into(), Value::String(m))])),
            };
            self.found.etc_files.entry(path).or_insert(value);
        }
    }

    /// `systemd.*`: NixOS's `systemd.services` and home-manager's
    /// `systemd.user.services`, when they're one command nami can run.
    fn systemd(&mut self, v: &Val, kind: Kind) {
        let nixos = kind == Kind::Nixos;
        for (k, t) in self.attrs(v) {
            let sv = self.force(&t);
            match (nixos, k.as_str()) {
                (true, "services") => {
                    for (name, t) in self.attrs(&sv) {
                        let u = self.force(&t);
                        self.nixos_unit(&name, &u, false);
                    }
                }
                (true, "user") if self.want == Some(Tier::User) => {
                    for (name, t) in self.ev.select(&sv, "services").map(|s| self.attrs(&s)).unwrap_or_default() {
                        let u = self.force(&t);
                        self.nixos_unit(&name, &u, true);
                    }
                }
                (true, _) if self.want == Some(Tier::User) => {}
                (true, "user") => self.found.skip("systemd.user", "user units for everyone: put services: in each person's user.nami instead (`nami import <file> --to user` converts them)"),
                (true, "tmpfiles") => self.tmpfiles(&sv),
                (false, "user") => {
                    for (k, t) in self.attrs(&sv) {
                        match k.as_str() {
                            "services" => {
                                for (name, t) in self.attrs(&self.force(&t)) {
                                    let u = self.force(&t);
                                    self.hm_unit(&name, &u);
                                }
                            }
                            "startServices" | "systemctlPath" | "enable" => {}
                            _ => self.found.skip(format!("systemd.user.{k}"), "no nami equivalent: nami's services are .service units"),
                        }
                    }
                }
                _ if nixos => self.found.skip(format!("systemd.{k}"), "NixOS system configuration: nami's services are .service units"),
                _ => self.found.skip(format!("systemd.{k}"), "no nami equivalent"),
            }
        }
    }

    /// A unit option's value as unit-file values (one per line), if plain.
    fn unit_values(&self, v: &Val) -> Option<Vec<String>> {
        match v {
            Val::Bool(b) => Some(vec![if *b { "yes" } else { "no" }.into()]),
            Val::Int(n) => Some(vec![n.to_string()]),
            Val::Str(s, true) if !s.contains('\n') => Some(vec![s.clone()]),
            Val::Path(p) => Some(vec![p.display().to_string()]),
            Val::List(_) => self.ev.list(v).iter().map(|x| self.unit_values(x)).collect::<Option<Vec<_>>>().map(|l| l.concat()),
            _ => None,
        }
    }

    /// NixOS `systemd.services.<name>` (or `systemd.user.services`): its
    /// command or script, and every other unit setting as `systemd:` keys.
    fn nixos_unit(&mut self, name: &str, v: &Val, user_unit: bool) {
        let what = if user_unit { format!("systemd.user.services.{name}") } else { format!("systemd.services.{name}") };
        let Val::Attrs(m) = v else {
            if !matches!(v, Val::Null) {
                self.found.skip(what, "not readable");
            }
            return;
        };
        // NixOS option -> [Unit] key.
        const UNIT_KEYS: &[(&str, &str)] = &[
            ("before", "Before"), ("requires", "Requires"), ("bindsTo", "BindsTo"), ("partOf", "PartOf"), ("conflicts", "Conflicts"), ("requisite", "Requisite"),
            ("upholds", "Upholds"), ("onFailure", "OnFailure"), ("onSuccess", "OnSuccess"), ("documentation", "Documentation"),
            ("startLimitIntervalSec", "StartLimitIntervalSec"), ("startLimitBurst", "StartLimitBurst"),
        ];
        // Scripts NixOS runs around the main one -> [Service] key.
        const SCRIPTS: &[(&str, &str)] = &[("preStart", "ExecStartPre"), ("postStart", "ExecStartPost"), ("preStop", "ExecStop"), ("postStop", "ExecStopPost"), ("reload", "ExecReload")];
        let mut c = Custom::default();
        let (mut script, mut wanted_by, mut packages) = (None, None, Vec::new());
        for (k, t) in m.iter() {
            let v = self.force(t);
            if matches!(v, Val::Null) || matches!(&v, Val::List(l) if l.is_empty()) || matches!(&v, Val::Attrs(a) if a.is_empty()) {
                continue;
            }
            let plain = self.strs(&v);
            match k.as_str() {
                "enable" => match v {
                    Val::Bool(false) => return,
                    Val::Bool(true) => {}
                    _ => c.other.push("enable".into()),
                },
                "script" => script = Some(v),
                "description" | "after" | "wants" | "wantedBy" | "requiredBy" | "startAt" if plain.is_none() => c.other.push(format!("{k} (refers to values only Nix knows)")),
                "description" => c.description = plain.map(|d| d.join(" ")),
                "after" => c.after.extend(plain.unwrap_or_default()),
                "wants" => {
                    for w in plain.unwrap_or_default() {
                        if w != "network-online.target" {
                            c.raw("Unit", "Wants", vec![w]);
                        }
                    }
                }
                "wantedBy" => wanted_by = plain,
                "requiredBy" => c.raw("Install", "RequiredBy", plain.unwrap_or_default()),
                "aliases" => c.raw("Install", "Alias", plain.unwrap_or_default()),
                "startAt" => {
                    let at = plain.unwrap_or_default();
                    if at.len() > 1 {
                        c.raw("Timer", "OnCalendar", at[1..].to_vec());
                    }
                    c.at = at.first().cloned();
                }
                "environment" => {
                    for (n, t) in self.attrs(&v) {
                        let value = match self.force(&t) {
                            Val::Str(s, true) => s,
                            Val::Int(i) => i.to_string(),
                            Val::Path(p) => p.display().to_string(),
                            Val::Null => continue,
                            _ => {
                                c.other.push(format!("environment.{n} (refers to values only Nix knows)"));
                                continue;
                            }
                        };
                        c.env.insert(n, value);
                    }
                }
                "path" => {
                    for item in self.ev.list(&v) {
                        match item {
                            Val::Pkg(p) if !p.is_empty() => packages.push(p),
                            _ => c.other.push("path (not only packages)".into()),
                        }
                    }
                }
                "serviceConfig" => {
                    for (sk, st) in self.attrs(&v) {
                        let sv = self.force(&st);
                        let s = self.strs(&sv);
                        match (sk.as_str(), s.as_deref()) {
                            (_, _) if matches!(sv, Val::Null) => {}
                            ("ExecStart", Some([one])) if !one.contains('%') => c.run = Some(one.clone()),
                            ("User", Some([u])) => c.user = Some(u.clone()),
                            ("DynamicUser", _) if matches!(sv, Val::Bool(true)) => c.dynamic = true,
                            ("Restart", Some([r])) => c.restart = Some(r.clone()),
                            ("WorkingDirectory", Some([d])) if d.starts_with('/') => c.dir = Some(d.clone()),
                            ("Type", Some([t])) if t == "simple" || t == "exec" => {}
                            ("Environment", Some(list)) => {
                                for kv in list.iter().flat_map(|l| l.split_whitespace()) {
                                    match kv.trim_matches('"').split_once('=') {
                                        Some((n, val)) if valid_env_name(n) => {
                                            c.env.insert(n.into(), val.into());
                                        }
                                        _ => c.other.push(format!("serviceConfig.Environment ({kv})")),
                                    }
                                }
                            }
                            _ => match self.unit_values(&sv) {
                                Some(vals) => c.raw("Service", &sk, vals),
                                None => c.other.push(format!("serviceConfig.{sk} (refers to values only Nix knows)")),
                            },
                        }
                    }
                }
                "unitConfig" => {
                    for (uk, ut) in self.attrs(&v) {
                        let uv = self.force(&ut);
                        match self.unit_values(&uv) {
                            Some(vals) => c.raw("Unit", &uk, vals),
                            None if matches!(uv, Val::Null) => {}
                            None => c.other.push(format!("unitConfig.{uk} (refers to values only Nix knows)")),
                        }
                    }
                }
                k if SCRIPTS.iter().any(|(n, _)| *n == k) => {
                    let key = SCRIPTS.iter().find(|(n, _)| *n == k).unwrap().1;
                    match &v {
                        Val::Str(text, true) => c.raw("Service", key, vec![crate::services::script_exec(text)]),
                        _ => c.other.push(format!("{k} (refers to store paths or options nami can't know)")),
                    }
                }
                k if UNIT_KEYS.iter().any(|(n, _)| *n == k) => {
                    let key = UNIT_KEYS.iter().find(|(n, _)| *n == k).unwrap().1;
                    match self.unit_values(&v) {
                        Some(vals) => c.raw("Unit", key, vals),
                        None => c.other.push(format!("{k} (refers to values only Nix knows)")),
                    }
                }
                // How NixOS restarts it on rebuilds: nami restarts what changed.
                "restartIfChanged" | "stopIfChanged" | "restartTriggers" | "reloadTriggers" | "reloadIfChanged" | "enableStrictShellChecks" | "scriptArgs" | "notSocketActivated" => {}
                _ => c.other.push(k.clone()),
            }
        }
        match script {
            Some(_) if c.run.is_some() => c.other.push("script and ExecStart both".into()),
            Some(Val::Str(text, true)) => {
                let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
                match lines.as_slice() {
                    // One plain command: run: reads better.
                    [one] if !one.contains(['|', '&', ';', '<', '>', '$', '`', '(', ')', '*', '?', '~', '"', '\'', '\\', '%']) => c.run = Some(one.to_string()),
                    _ => c.script = Some(text),
                }
            }
            Some(_) => return self.found.skip(format!("{what}.script"), "refers to store paths or options nami can't know"),
            None => {}
        }
        if c.run.is_none() && c.script.is_none() && c.other.is_empty() {
            return self.found.skip(what, "no script or ExecStart of its own: it changes a unit NixOS or a package makes, and nami only changes its own units");
        }
        // NixOS runs a service as root unless it says otherwise; nami's default is a fresh user.
        if c.user.is_none() && !c.dynamic && !user_unit {
            c.user = Some("root".into());
        }
        let default_targets = |t: &String| t == "multi-user.target" || t == "default.target" || (!user_unit && t == "graphical.target");
        match wanted_by.as_deref() {
            None | Some([]) if c.at.is_none() => {
                c.enable = Some(false);
                self.found.warn(format!("{what} isn't wantedBy anything, so it's installed, not started (enable: false), as on NixOS"));
            }
            None | Some([]) => {}
            Some(w) if w.iter().all(default_targets) => {}
            Some(w) => c.raw("Install", "WantedBy", w.to_vec()),
        }
        let user = c.user.clone().filter(|u| u != "root");
        match c.value() {
            Ok(value) => {
                for p in packages {
                    self.found.package(&p, &format!("{what}.path"));
                }
                if let Some(u) = user {
                    self.found.warn(format!("{what} runs as {u}: NixOS creates the users it's told about, nami doesn't (`useradd --system {u}`, or drop `user:` for a fresh one each run)"));
                }
                self.add_service(name, value, &what, user_unit);
            }
            Err(why) => self.found.skip(what, why),
        }
    }

    /// home-manager `systemd.user.services.<name>`: `Unit`, `Service`, `Install`
    /// with systemd's own keys; ExecStart is the command, the rest as it is.
    fn hm_unit(&mut self, name: &str, v: &Val) {
        let what = format!("systemd.user.services.{name}");
        let Val::Attrs(m) = v else {
            if !matches!(v, Val::Null) {
                self.found.skip(what, "not readable");
            }
            return;
        };
        let mut c = Custom::default();
        let mut installed = false;
        for (section, t) in m.iter() {
            let sv = self.force(t);
            if !["Unit", "Service", "Install", "Timer"].contains(&section.as_str()) {
                if !matches!(sv, Val::Null) && !matches!(&sv, Val::Attrs(a) if a.is_empty()) {
                    c.other.push(section.clone());
                }
                continue;
            }
            for (k, t) in self.attrs(&sv) {
                let v = self.force(&t);
                if matches!(v, Val::Null) {
                    continue;
                }
                let s = self.strs(&v);
                match (section.as_str(), k.as_str(), s) {
                    ("Unit", "Description", Some(d)) => c.description = Some(d.join(" ")),
                    ("Unit", "After", Some(a)) => c.after.extend(a.iter().flat_map(|u| u.split_whitespace().map(String::from))),
                    ("Service", "ExecStart", Some(e)) if e.len() == 1 && !e[0].contains('%') => c.run = Some(e[0].clone()),
                    ("Service", "Environment", Some(e)) => {
                        for kv in e.iter().flat_map(|l| l.split_whitespace()) {
                            match kv.trim_matches('"').split_once('=') {
                                Some((n, val)) if valid_env_name(n) => {
                                    c.env.insert(n.into(), val.into());
                                }
                                _ => c.other.push(format!("Service.Environment ({kv})")),
                            }
                        }
                    }
                    ("Service", "Restart", Some(r)) if r.len() == 1 => c.restart = Some(r[0].clone()),
                    ("Service", "WorkingDirectory", Some(d)) if d.len() == 1 && d[0].starts_with('/') => c.dir = Some(d[0].clone()),
                    ("Service", "Type", Some(t)) if t == ["simple"] || t == ["exec"] => {}
                    ("Install", "WantedBy", Some(w)) if w.iter().all(|t| t == "default.target") => installed = true,
                    ("Install", "WantedBy", Some(w)) => {
                        installed = true;
                        c.raw("Install", "WantedBy", w);
                    }
                    _ => match self.unit_values(&v) {
                        Some(vals) => c.raw(section, &k, vals),
                        None => c.other.push(format!("{section}.{k} (refers to values only Nix knows)")),
                    },
                }
            }
        }
        if c.run.is_none() && c.other.is_empty() {
            return self.found.skip(what, "no ExecStart: nami's services run a command");
        }
        if !installed {
            c.enable = Some(false);
            self.found.warn(format!("{what} has no Install.WantedBy, so it's installed, not started (enable: false), as with home-manager"));
        }
        match c.value() {
            Ok(value) => self.add_service(name, value, &what, true),
            Err(why) => self.found.skip(what, why),
        }
    }
}

const SSHD_DROPIN: &str = "/etc/ssh/sshd_config.d/10-nami.conf";

/// NixOS `services.*` that are the distro's job on any other Linux.
const DISTRO_SERVICES: &[&str] = &[
    "xserver", "displayManager", "desktopManager", "libinput", "pipewire", "pulseaudio", "printing", "avahi", "udev", "dbus", "logind",
    "journald", "resolved", "timesyncd", "getty", "fwupd", "upower", "power-profiles-daemon", "thermald", "blueman", "gnome", "flatpak", "udisks2",
    "accounts-daemon", "gvfs", "geoclue2", "fprintd", "tlp", "acpid", "chrony", "ntp", "colord",
];

/// A systemd unit read from Nix, on its way to a `services:` entry of your own.
#[derive(Default)]
struct Custom {
    run: Option<String>,
    script: Option<String>,
    at: Option<String>,
    env: BTreeMap<String, String>,
    after: Vec<String>,
    user: Option<String>,
    dynamic: bool,
    restart: Option<String>,
    dir: Option<String>,
    description: Option<String>,
    enable: Option<bool>,
    /// `systemd:` keys: section -> key -> values.
    raw: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    /// What can't be carried over (values only Nix knows).
    other: Vec<String>,
}

impl Custom {
    fn raw(&mut self, section: &str, key: &str, vals: Vec<String>) {
        if !vals.is_empty() {
            self.raw.entry(section.into()).or_default().entry(key.into()).or_default().extend(vals);
        }
    }

    /// The `services:` value, or why not.
    fn value(mut self) -> std::result::Result<Value, String> {
        if !self.other.is_empty() {
            self.other.dedup();
            return Err(format!("has {}, which nami can't carry over: write that part of the service yourself", self.other.join(", ")));
        }
        if self.run.is_none() && self.script.is_none() {
            return Err("no ExecStart or script".into());
        }
        if self.run.as_deref().is_some_and(|r| r.contains('%')) {
            return Err("uses systemd's % specifiers, which nami's run: doesn't".into());
        }
        // A made-up user for the service: NixOS's DynamicUser, nami's default without user:.
        if self.dynamic {
            self.user = None;
        }
        let s = |v: String| Value::String(v);
        let mut m = Mapping::new();
        if let Some(run) = self.run {
            m.insert("run".into(), s(run));
        }
        if let Some(script) = self.script {
            m.insert("script".into(), s(script));
        }
        if let Some(at) = self.at {
            m.insert("at".into(), s(at));
        }
        if !self.env.is_empty() {
            m.insert("env".into(), Value::Mapping(self.env.into_iter().map(|(k, v)| (s(k), s(v))).collect()));
        }
        if !self.after.is_empty() {
            m.insert("after".into(), Value::Sequence(self.after.into_iter().map(s).collect()));
        }
        for (k, v) in [("user", self.user), ("restart", self.restart), ("dir", self.dir), ("description", self.description)] {
            if let Some(v) = v {
                m.insert(k.into(), s(v));
            }
        }
        if let Some(e) = self.enable {
            m.insert("enable".into(), Value::Bool(e));
        }
        if !self.raw.is_empty() {
            let sections: Mapping = self
                .raw
                .into_iter()
                .map(|(sec, keys)| {
                    let keys: Mapping = keys.into_iter().map(|(k, v)| (s(k), if v.len() == 1 { s(v[0].clone()) } else { Value::Sequence(v.into_iter().map(s).collect()) })).collect();
                    (s(sec), Value::Mapping(keys))
                })
                .collect();
            m.insert("systemd".into(), Value::Mapping(sections));
        }
        Ok(Value::Mapping(m))
    }
}

fn users_len(v: &Val, ev: &Eval) -> usize {
    ev.select(v, "users").and_then(|u| u.attrs().map(|m| m.len())).unwrap_or(0)
}

fn valid_env_name(n: &str) -> bool {
    n.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn source(path: &str) -> Value {
    Value::Mapping(Mapping::from_iter([(Value::String("source".into()), Value::String(path.into()))]))
}

/// Merge `b` into `a`: maps merge, anything else from `b` wins.
fn merge_yaml(a: &mut Mapping, b: Mapping) {
    for (k, v) in b {
        match (a.get_mut(&k), v) {
            (Some(Value::Mapping(x)), Value::Mapping(y)) => merge_yaml(x, y),
            (_, v) => {
                a.insert(k, v);
            }
        }
    }
}

/// The package `programs.<name>.enable` installs, if any.
fn program_package(name: &str, kind: Kind) -> Option<&str> {
    Some(match name {
        // Settings for something that's there anyway, or home-manager itself.
        "home-manager" | "man" | "info" | "bash" | "command-not-found" | "dconf" | "nix-ld" | "xwayland" | "gnupg" | "ssh" | "fuse" | "nix-index-database" | "texlive" => return None,
        "gpg" => "gnupg",
        "password-store" => "pass",
        "java" => "jdk",
        "neovim" => "neovim",
        "git" if kind == Kind::Nixos => "git",
        n => n,
    })
}

// ---------- with Nix ----------

#[derive(Debug, serde::Deserialize)]
pub struct NixPkg {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub pname: String,
    #[serde(default)]
    pub out: String,
}

const NIX_APPLY: &str = "d: map (p: { name = p.name or \"\"; pname = p.pname or \"\"; out = builtins.unsafeDiscardStringContext (p.outPath or \"\"); }) ((d.packages or []) ++ (d.nativeBuildInputs or []) ++ (d.buildInputs or []) ++ (d.propagatedBuildInputs or []))";

/// The dev shell's packages as Nix sees them, if Nix is installed and
/// answers within `timeout`. Never required: None means "read statically".
pub fn nix_shell_packages(flake_dir: &Path, timeout: Duration, extra: &[&str]) -> Option<Vec<NixPkg>> {
    let nix = nixtool::get()?;
    let deadline = Instant::now() + timeout;
    for attr in [format!("devShells.{SYSTEM}.default"), format!("devShell.{SYSTEM}")] {
        let mut child = nix
            .command()
            .args(["eval", "--json", "--no-write-lock-file"])
            .args(extra)
            .arg(format!("path:{}#{attr}", flake_dir.display()))
            .args(["--apply", NIX_APPLY])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take()?;
        let reader = std::thread::spawn(move || {
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut stdout, &mut out).ok();
            out
        });
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let out = reader.join().unwrap_or_default();
        match status {
            Some(s) if s.success() => return serde_json::from_slice(&out).ok(),
            Some(_) => continue,
            None => return None,
        }
    }
    None
}

/// Add what `nix eval` found that the static reader didn't.
fn merge_nix(found: &mut Found, pkgs: &[NixPkg], catalog: Option<&Catalog>) {
    let attr_of = |p: &NixPkg| -> Option<String> {
        let by_path = catalog.and_then(|c| c.paths.get(p.out.as_str()).map(|a| a.to_string()));
        let pname = if p.pname.is_empty() { pname(&p.name).to_string() } else { p.pname.clone() };
        by_path.or_else(|| match catalog {
            Some(c) => c.has(&pname).then_some(pname),
            None => Some(pname),
        })
    };
    let same = |attr: &str, p: &NixPkg, mapped: Option<&str>| -> bool {
        let pn = if p.pname.is_empty() { pname(&p.name) } else { p.pname.as_str() };
        mapped == Some(attr)
            || attr.rsplit('.').next() == Some(pn)
            || attr.starts_with(&format!("{pn}_"))
            || catalog.and_then(|c| c.pnames.get(attr)).is_some_and(|n| *n == pn || n.ends_with(&format!("-{pn}")))
    };
    found.unconverted.retain(|u| !u.soft);
    for p in pkgs {
        let pn = if p.pname.is_empty() { pname(&p.name).to_string() } else { p.pname.clone() };
        if found.tasks.contains_key(&pn) || found.tasks.contains_key(&p.name) {
            continue;
        }
        let mapped = attr_of(p);
        if found.packages.iter().any(|(a, _)| same(a, p, mapped.as_deref())) {
            continue;
        }
        match mapped {
            Some(a) => found.package(&a, "nix eval"),
            None => found.skip(&p.name, "in the dev shell (says `nix eval`), but not in the catalog"),
        }
    }
    found.warn("read the dev shell's packages with `nix eval` too");
}

/// Check the packages against the catalog: renamed ones get the catalog's
/// name, unknown ones are listed instead.
fn check_packages(found: &mut Found, catalog: Option<&Catalog>) {
    let mut out: Vec<(String, String)> = Vec::new();
    for (attr, origin) in std::mem::take(&mut found.packages) {
        let name = match catalog {
            None => Some(attr.clone()),
            Some(c) => c.resolve(&attr),
        };
        match name {
            Some(n) => {
                if n != attr {
                    found.warn(format!("{attr} is {n} in the catalog"));
                }
                if !out.iter().any(|(a, _)| *a == n) {
                    out.push((n, origin));
                }
            }
            None if origin.starts_with("programs.") => found.skip(origin, format!("no package {attr} in the catalog")),
            None => found.skip(attr, "not in the catalog"),
        }
    }
    if catalog.is_none() && !out.is_empty() {
        found.warn("no catalog on this machine, so package names weren't checked (`nami update` fetches one)");
    }
    found.packages = out;
}

// ---------- writing ----------

/// Move what the tier has no place for into `unconverted`, and point
/// `source:` paths at the files from where the target is.
fn retarget(found: &mut Found, tier: Tier, target_dir: &Path) {
    let elsewhere = |what: &str| match tier {
        Tier::System => format!("system.nami holds the system's apps; {what} go in ~/user.nami (--to user)"),
        Tier::Project => format!("a project file has no {what}; they go in ~/user.nami (--to user)"),
        Tier::User => format!("{what} belong in a project's project.nami (--to project)"),
    };
    let mut moved = Vec::new();
    // Nami files for the computer, and their people: the system file's.
    if tier == Tier::System {
        features(found, false);
    } else {
        let why = "the computer's: it goes in system.nami, /etc/nami/system.nami (--to system)";
        moved.extend(std::mem::take(&mut found.includes).into_iter().map(|f| (format!("- {f}"), why.to_string())));
        for (group, login, what) in std::mem::take(&mut found.group_members) {
            moved.push((what, format!("the {group} group: the computer's (`sudo usermod -aG {group} {login}`, or `groups: [{group}]` under users.{login} in system.nami)")));
        }
        found.if_included.clear();
    }
    if tier != Tier::User {
        moved.extend(std::mem::take(&mut found.aliases).into_keys().map(|k| (format!("alias {k}"), elsewhere("aliases"))));
        moved.extend(std::mem::take(&mut found.shell).into_keys().map(|k| (format!("shell.{k}"), elsewhere("shell startup lines"))));
        moved.extend(std::mem::take(&mut found.generated).into_keys().map(|k| (format!("home file {k}"), elsewhere("dotfiles"))));
        moved.extend(std::mem::take(&mut found.files).into_keys().map(|k| (format!("home file {k}"), elsewhere("dotfiles"))));
        moved.extend(std::mem::take(&mut found.settings).into_keys().map(|k| (format!("settings {k}"), elsewhere("desktop settings"))));
    }
    if tier == Tier::System {
        moved.extend(std::mem::take(&mut found.env).into_keys().map(|k| (format!("env {k}"), elsewhere("environment variables"))));
    }
    // The computer's environment (NixOS's): files NixOS would have made.
    let system_env = std::mem::take(&mut found.system_env);
    if tier == Tier::System && !system_env.is_empty() {
        for (path, text) in nixos::env_files(&system_env) {
            found.etc_files.entry(path.into()).or_insert(Value::String(text));
        }
    } else {
        let why = "the computer's environment: it goes in system.nami as /etc/environment.d and /etc/profile.d files (--to system)";
        moved.extend(system_env.into_keys().map(|k| (format!("env {k}"), why.to_string())));
    }
    if tier != Tier::Project {
        moved.extend(std::mem::take(&mut found.tasks).into_keys().map(|k| (format!("task {k}"), elsewhere("tasks"))));
        found.develop = false;
    }
    // Services: the computer's (NixOS) go in system.nami, yours
    // (home-manager) in the user file; /etc files in system.nami.
    let system_services = std::mem::take(&mut found.services);
    let user_services = std::mem::take(&mut found.user_services);
    let etc_files = std::mem::take(&mut found.etc_files);
    let to_system = |what: &str| format!("{what} go in system.nami, /etc/nami/system.nami (--to system)");
    let (keep, other, other_why) = match tier {
        Tier::System => (system_services, user_services, elsewhere("your own services")),
        Tier::User => (user_services, system_services, to_system("the computer's services")),
        Tier::Project => {
            moved.extend(user_services.into_keys().map(|k| (format!("service {k}"), elsewhere("services"))));
            (BTreeMap::new(), system_services, to_system("the computer's services"))
        }
    };
    found.services = keep;
    moved.extend(other.into_keys().map(|k| (format!("service {k}"), other_why.clone())));
    if tier == Tier::System {
        found.etc_files = etc_files;
    } else {
        moved.extend(etc_files.into_keys().map(|k| (format!("file {k}"), to_system("files under /etc"))));
    }
    for (what, why) in moved {
        found.skip(what, why);
    }
    for v in found.files.values_mut().chain(found.etc_files.values_mut()) {
        if let Value::Mapping(m) = v {
            if let Some(Value::String(src)) = m.get_mut("source") {
                if let Ok(rel) = Path::new(src.as_str()).strip_prefix(target_dir) {
                    *src = format!("./{}", rel.display());
                }
            }
        }
    }
}

fn path(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn lookup<'v>(v: &'v Value, path: &[String]) -> Option<&'v Value> {
    path.iter().try_fold(v, |v, k| v.get(k.as_str()))
}

/// Merge `found` into a nami file's text: packages added, keys added where
/// they aren't set. Returns the new text, what was added, and what was kept
/// because the file already says otherwise.
pub fn merge(text: &str, found: &Found, tier: Tier, source_name: &str) -> Result<(String, Vec<String>, Vec<String>)> {
    let mut text = if text.trim().is_empty() { format!("# Imported from {source_name} by `nami import`.\n") } else { text.to_string() };
    let existing: Value = serde_yaml::from_str(&text).unwrap_or(Value::Null);
    let (mut added, mut kept) = (Vec::new(), Vec::new());
    let mut ops: Vec<Op> = Vec::new();
    let listed: Vec<String> = crate::config::listed_packages(&text).into_iter().map(|(n, _)| crate::index::split_version(&n).0.to_string()).collect();
    // nami's example packages (ubuntu, docker): copied into the repo's
    // packages/ folder (see `new::copy_packages`), and listed by name.
    for f in &found.includes {
        let entry = f.clone();
        if !listed.iter().any(|l| crate::config::entry_name(l) == f) {
            added.push(format!("package {entry} (from nami's examples)"));
            ops.extend(crate::config::add_package(&text, &entry)?);
        }
    }
    for (p, _) in &found.packages {
        if !listed.contains(p) && found.login_shell.as_ref() != Some(p) {
            added.push(format!("package {p}"));
            ops.extend(crate::config::add_package(&text, p)?);
        }
    }
    // The login shell, under its package: `- zsh: { login-shell: true }`.
    if let Some(sh) = found.login_shell.as_ref().filter(|_| existing.get("login-shell").is_none()) {
        let has = crate::config::listed_packages(&text).into_iter().find(|(n, _)| crate::index::split_version(n).0 == sh);
        if !has.as_ref().is_some_and(|(_, w)| w.get(sh.as_str()).and_then(|o| o.get("login-shell")).is_some()) {
            ops.extend(has.map(|(_, w)| Op::Remove { path: path(&["packages"]), value: w }));
            let own = Mapping::from_iter([(Value::from("login-shell"), Value::Bool(true))]);
            ops.push(Op::Add { path: path(&["packages"]), value: Value::Mapping(Mapping::from_iter([(Value::from(sh.as_str()), Value::Mapping(own))])) });
            added.push(format!("login-shell {sh} (terminals open it)"));
        }
    }
    let mut set = |p: Vec<String>, value: Value, label: String| match lookup(&existing, &p) {
        Some(old) if *old == value => {}
        Some(old) => kept.push(format!("{label}: kept {}", serde_yaml::to_string(old).unwrap_or_default().trim())),
        None => {
            added.push(label);
            ops.push(Op::Set { path: p, value });
        }
    };
    // A person's files are in their home: `~/.gitconfig` in user.nami.
    let home = |rest: &[&str]| -> Vec<String> {
        let mut p = path(rest);
        if tier != Tier::Project && p[0] == "files" {
            p[1] = format!("~/{}", p[1]);
        }
        p
    };
    for (k, v) in &found.env {
        set(home(&["env", k]), Value::String(v.clone()), format!("env {k}"));
    }
    for (k, v) in &found.aliases {
        set(home(&["aliases", k]), Value::String(v.clone()), format!("alias {k}"));
    }
    for (k, v) in &found.shell {
        set(home(&["shell", k]), Value::String(v.clone()), format!("shell.{k}"));
    }
    // A file in a format nami writes itself (JSON, TOML, gitconfig...): as settings.
    let as_settings = |rel: &str, v: &Value| match v {
        Value::String(t) if crate::home::settings_only(rel) => crate::drift::parse(rel, t).ok().flatten().unwrap_or_else(|| v.clone()),
        _ => v.clone(),
    };
    for (k, v) in &found.files {
        set(home(&["files", k]), as_settings(k, v), format!("home file {k}"));
    }
    for (k, g) in &found.generated {
        let from = match g {
            generated::Generated::Link(p) => p.clone(),
            _ => format!("./{GENERATED_DIR}/{k}"),
        };
        let src = Value::Mapping(Mapping::from_iter([(Value::String("source".into()), Value::String(from))]));
        set(home(&["files", k]), src, format!("home file {k} (as home-manager made it)"));
    }
    for (schema, keys) in &found.settings {
        for (k, v) in keys {
            set(path(&["settings", schema, k]), v.clone(), format!("setting {schema} {k}"));
        }
    }
    for (k, v) in &found.tasks {
        set(path(&["tasks", k]), Value::String(v.clone()), format!("task {k}"));
    }
    for (k, v) in &found.etc_files {
        set(path(&["files", k]), as_settings(k, v), format!("file {k}"));
    }
    for (p, v) in &found.top {
        set(p.clone(), v.clone(), p.join("."));
    }
    // Services are a list: `- name`, or `- name:` with its setup under it.
    for (k, v) in &found.services {
        match lookup(&existing, &path(&["services", k])) {
            Some(old) if old == v => {}
            Some(old) => kept.push(format!("service {k}: kept {}", serde_yaml::to_string(old).unwrap_or_default().trim())),
            None => {
                added.push(format!("service {k}"));
                let item = if *v == Value::Bool(true) { Value::from(k.as_str()) } else { Value::Mapping(Mapping::from_iter([(Value::from(k.as_str()), v.clone())])) };
                ops.push(Op::Add { path: path(&["services"]), value: item });
            }
        }
    }
    if found.unfree == Some(false) && tier != Tier::Project && existing.get("unfree").is_none() {
        added.push("unfree: false".into());
        ops.push(Op::Set { path: path(&["unfree"]), value: Value::Bool(false) });
    }
    if found.develop && tier == Tier::Project && existing.get("develop").is_none() {
        added.push("develop: true".into());
        ops.push(Op::Set { path: path(&["develop"]), value: Value::Bool(true) });
    }
    // Comments at the very end (an earlier import's notes) stay at the end,
    // rather than becoming part of the last key's block.
    let lines: Vec<&str> = text.lines().collect();
    let body_end = lines.iter().rposition(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')).map_or(lines.len(), |i| i + 1);
    let mut tail = lines[body_end..].join("\n").trim().to_string();
    text = lines[..body_end].join("\n") + "\n";
    // The computer's own keys first (its name, its people), then its packages.
    ops.sort_by_key(|op| match op {
        Op::Set { path, .. } | Op::Add { path, .. } if path.first().is_some_and(|k| crate::config::COMPUTER_KEYS.contains(&k.as_str())) => 0,
        Op::Set { path, .. } | Op::Add { path, .. } if path.first().is_some_and(|k| k == "packages") => 1,
        _ => 2,
    });
    for op in &ops {
        text = config_edit::apply(&text, op).with_context(|| format!("adding {:?} to the file", op_label(op)))?;
    }
    if !found.unconverted.is_empty() {
        let mut block = format!("# Not converted from {source_name} (nami import):\n");
        for u in &found.unconverted {
            block += &format!("#   {}: {}\n", u.what.replace('\n', " "), u.why);
        }
        if !tail.contains(block.trim_end()) {
            tail = format!("{tail}\n\n{block}").trim().to_string();
        }
    }
    if !tail.is_empty() {
        text = format!("{}\n\n{tail}\n", text.trim_end());
    }
    Ok((text, added, kept))
}

fn op_label(op: &Op) -> String {
    match op {
        Op::Set { path, .. } | Op::Add { path, .. } | Op::Unset { path } | Op::Remove { path, .. } => path.join("."),
    }
}

// ---------- the command ----------

pub struct Options {
    pub path: PathBuf,
    pub to: Option<Tier>,
    pub file: Option<PathBuf>,
    pub dry_run: bool,
    pub json: bool,
}

#[derive(Serialize)]
struct Report<'a> {
    kind: Kind,
    target: String,
    packages: Vec<&'a str>,
    env: &'a BTreeMap<String, String>,
    aliases: &'a BTreeMap<String, String>,
    /// home.shell: bash/zsh/fish -> lines.
    shell: &'a BTreeMap<String, String>,
    files: Vec<&'a str>,
    settings: Vec<&'a str>,
    tasks: &'a BTreeMap<String, String>,
    /// `services:` entries for the target file: name -> true or settings.
    services: &'a BTreeMap<String, Value>,
    /// `files:` paths (under /etc) for the system file.
    etc_files: Vec<&'a str>,
    unconverted: &'a [Unconverted],
    warnings: &'a [String],
    yaml: &'a str,
}

/// Everything up to writing: (kind, tier, target, found, new text, added, kept).
pub struct Plan {
    pub kind: Kind,
    pub tier: Tier,
    pub target: PathBuf,
    pub found: Found,
    pub yaml: String,
    pub added: Vec<String>,
    pub kept: Vec<String>,
}

pub fn plan(opts: &Options, catalog: Option<&Catalog>) -> Result<Plan> {
    let path = std::path::absolute(&opts.path)?;
    let (kind, file) = detect(&path, opts.to)?;
    let dir = file.parent().unwrap_or(Path::new("/")).to_path_buf();
    let is_flake = file.file_name().is_some_and(|n| n == "flake.nix");
    // Nix here, or fetched for this (only for a flake that needs it).
    let nix_ok = is_flake && opts.to != Some(Tier::Project) && nixtool::get().is_some();
    let mut reader = Reader::new(opts.to);
    // A flake's NixOS or home-manager configuration, with Nix here: Nix
    // evaluates it, whatever shape the flake has; the reader is the fallback.
    // Without --to: a home configuration is yours, a NixOS one the computer's
    // (a flake with neither is a project's dev shell, read as before).
    let (mut evaluated, mut nix_error) = (None, None);
    if nix_ok {
        let secs = std::env::var("NAMI_IMPORT_NIX_TIMEOUT").ok().and_then(|s| s.parse().ok()).unwrap_or(600);
        match evaluated::evaluate(&dir, opts.to.unwrap_or(Tier::User), Duration::from_secs(secs)) {
            Ok(Some(e)) => evaluated = Some(e),
            Ok(None) => {}
            Err(why) => {
                reader.found.warn(format!("Nix couldn't evaluate the flake ({why}), so it was read without Nix"));
                nix_error = Some(why);
            }
        }
    }
    let kind = match &evaluated {
        Some(e) => {
            let tier = opts.to.unwrap_or(if e.kind == Kind::HomeManager { Tier::User } else { Tier::System });
            reader.read_evaluated(e, catalog, tier, &file)
        }
        None => match reader.read(kind, &file) {
            Ok(k) => k,
            Err(e) => {
                if let Some(why) = nix_error {
                    return Err(e.context(format!("Nix couldn't evaluate the flake either: {why}")));
                }
                if is_flake && !nix_ok && opts.to != Some(Tier::Project) {
                    return Err(e.context("and nami couldn't get Nix to evaluate it (it fetches Nix through the nami daemon: is nami installed, and online?)"));
                }
                return Err(e);
            }
        },
    };
    let mut found = reader.found;
    if evaluated.is_none() && kind == Kind::Flake && is_flake {
        let secs = std::env::var("NAMI_IMPORT_NIX_TIMEOUT").ok().and_then(|s| s.parse().ok()).unwrap_or(30);
        if let Some(pkgs) = nix_shell_packages(&dir, Duration::from_secs(secs), &[]) {
            merge_nix(&mut found, &pkgs, catalog);
        }
    }
    check_packages(&mut found, catalog);
    let tier = opts.to.unwrap_or(match kind {
        Kind::Flake => Tier::Project,
        Kind::HomeManager => Tier::User,
        Kind::Nixos => Tier::System,
    });
    let target = match (&opts.file, tier) {
        (Some(f), _) => std::path::absolute(f)?,
        (None, Tier::Project) => dir.join(crate::project::FILE),
        (None, Tier::User) => crate::config::user_file(),
        (None, Tier::System) => crate::config::system_file(),
    };
    retarget(&mut found, tier, target.parent().unwrap_or(Path::new("/")));
    let text = std::fs::read_to_string(&target).unwrap_or_default();
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (yaml, added, kept) = merge(&text, &found, tier, &name)?;
    Ok(Plan { kind, tier, target, found, yaml, added, kept })
}

pub fn run(opts: Options, index: Option<&Index>) -> Result<()> {
    let catalog = index.map(Catalog::from_index);
    let plan = plan(&opts, catalog.as_ref())?;
    let f = &plan.found;
    if opts.json {
        let report = Report {
            kind: plan.kind,
            target: plan.target.display().to_string(),
            packages: f.packages.iter().map(|(a, _)| a.as_str()).collect(),
            env: &f.env,
            aliases: &f.aliases,
            shell: &f.shell,
            files: f.files.keys().chain(f.generated.keys()).map(String::as_str).collect(),
            settings: f.settings.keys().map(String::as_str).collect(),
            tasks: &f.tasks,
            services: &f.services,
            etc_files: f.etc_files.keys().map(String::as_str).collect(),
            unconverted: &f.unconverted,
            warnings: &f.warnings,
            yaml: &plan.yaml,
        };
        println!("{}", serde_json::to_string(&report)?);
    }
    if !opts.dry_run {
        if !f.generated.is_empty() {
            let dir = plan.target.parent().unwrap_or(Path::new(".")).join(GENERATED_DIR);
            generated::write(&f.generated, &dir).with_context(|| format!("writing {}", dir.display()))?;
        }
        if plan.target == crate::config::system_file() {
            crate::cmd::setup::system_dir(&crate::config::hostname().unwrap_or_else(|| "computer".into()))?;
        }
        write(&plan.target, &plan.yaml)?;
        if plan.tier == Tier::System {
            crate::new::copy_packages(plan.target.parent().unwrap_or(Path::new(".")), &f.includes)?;
        }
    } else if !opts.json {
        print!("{}", plan.yaml);
    }
    if !opts.json {
        summary(&plan, opts.dry_run);
    }
    Ok(())
}

/// Where generated files are copied, next to the nami file.
const GENERATED_DIR: &str = "files";

fn write(target: &Path, yaml: &str) -> Result<()> {
    if std::fs::read_to_string(target).ok().as_deref() == Some(yaml) {
        return Ok(());
    }
    if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(target, yaml).map_err(|e| {
        let hint = if e.kind() == std::io::ErrorKind::PermissionDenied && *target == crate::config::system_file() {
            " (admins can edit system.nami once it exists: run `sudo nami apply` once, or import with --file)"
        } else {
            ""
        };
        anyhow::anyhow!("can't write {}: {e}{hint}", target.display())
    })?;
    // Your settings, likely to become a repo: this computer's own files stay out of it.
    if *target == crate::config::user_file() {
        crate::machine::ignore_own_files(target.parent().unwrap());
    }
    Ok(())
}

fn summary(plan: &Plan, dry_run: bool) {
    let f = &plan.found;
    let mut out = String::new();
    let verb = if dry_run { "would add" } else { "added" };
    if plan.added.is_empty() {
        out += &format!("nothing new for {} (from {})\n", plan.target.display(), plan.kind.describe());
    } else {
        out += &format!("{verb} to {} (from {}):\n", plan.target.display(), plan.kind.describe());
        for a in &plan.added {
            out += &format!("  + {a}\n");
        }
    }
    for k in &plan.kept {
        out += &format!("  = {k} (the file already says so)\n");
    }
    if !f.unconverted.is_empty() {
        out += &format!("not converted ({}, listed at the end of the file):\n", f.unconverted.len());
        for u in &f.unconverted {
            out += &format!("  - {}: {}\n", u.what, u.why);
        }
    }
    for w in &f.warnings {
        out += &format!("note: {w}\n");
    }
    if !dry_run {
        let next = match plan.tier {
            Tier::Project => {
                let dir = plan.target.parent().unwrap_or(Path::new("."));
                if std::env::current_dir().ok().as_deref() == Some(dir) { "nami shell".to_string() } else { format!("nami shell {}", dir.display()) }
            }
            _ => {
                // (What a plain `nami apply` applies: root's the computer's, anyone else's their own.)
                if plan.target == crate::config::user_file() && !crate::profile::is_root() {
                    "nami apply".to_string()
                } else if plan.target == crate::config::system_file() {
                    "sudo nami apply".to_string()
                } else {
                    format!("nami apply {}", plan.target.display())
                }
            }
        };
        out += &format!("next: {next}\n");
    }
    if dry_run {
        eprint!("{out}");
    } else {
        print!("{out}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/import").join(name)
    }

    fn read(name: &str, to: Option<Tier>) -> (Kind, Found) {
        let (kind, file) = detect(&fixture(name), to).unwrap();
        let mut r = Reader::new(to);
        let kind = r.read(kind, &file).unwrap();
        (kind, r.found)
    }

    fn attrs(f: &Found) -> Vec<&str> {
        f.packages.iter().map(|(a, _)| a.as_str()).collect()
    }

    fn unconverted(f: &Found) -> Vec<&str> {
        f.unconverted.iter().map(|u| u.what.as_str()).collect()
    }

    #[test]
    fn flake_with_flake_utils() {
        let (kind, f) = read("flake-utils", None);
        assert_eq!(kind, Kind::Flake);
        assert_eq!(attrs(&f), ["openssl", "pkg-config", "rustc", "cargo", "rust-analyzer", "nodejs_22", "python313", "python313Packages.requests", "python313Packages.rich", "jq", "cargo-watch"]);
        assert_eq!(f.env.get("RUST_BACKTRACE").map(String::as_str), Some("1"));
        assert_eq!(f.env.get("RUST_LOG").map(String::as_str), Some("debug"));
        assert_eq!(f.env.get("DATABASE_URL").map(String::as_str), Some("postgres://localhost/dev"));
        assert_eq!(f.tasks.get("dev").map(String::as_str), Some("cargo watch -x run"));
        assert!(f.develop, "buildInputs mean a build environment");
        let u = unconverted(&f);
        assert!(u.contains(&"shellHook") && u.iter().any(|w| w.contains("callPackage")), "{u:?}");
        assert!(!f.env.contains_key("LD_LIBRARY_PATH") && u.contains(&"LD_LIBRARY_PATH"), "{u:?}");
    }

    #[test]
    fn flake_with_dev_shells_per_system() {
        let (kind, f) = read("devshells/flake.nix", None);
        assert_eq!(kind, Kind::Flake);
        assert_eq!(attrs(&f), ["go", "gopls", "golangci-lint", "delve"]);
        assert_eq!(f.env.get("CGO_ENABLED").map(String::as_str), Some("0"));
        assert!(unconverted(&f).contains(&"devShells.x86_64-linux.ci"));
        assert!(!f.develop);
    }

    #[test]
    fn home_manager() {
        let (kind, f) = read("home/home.nix", None);
        assert_eq!(kind, Kind::HomeManager);
        let a = attrs(&f);
        for p in ["ripgrep", "fd", "htop", "nerd-fonts.fira-code", "git", "neovim", "starship", "direnv", "eza"] {
            assert!(a.contains(&p), "{p} missing from {a:?}");
        }
        assert!(!a.contains(&"steam"), "optionals false: {a:?}");
        assert_eq!(f.env.get("EDITOR").map(String::as_str), Some("nvim"));
        assert_eq!(f.env.get("GOPATH").map(String::as_str), Some(format!("{}/go", std::env::var("HOME").unwrap()).as_str()));
        assert_eq!(f.aliases.get("ll").map(String::as_str), Some("eza -l"));
        assert_eq!(f.aliases.get("gs").map(String::as_str), Some("git status"));
        assert_eq!(f.aliases.get("vim").map(String::as_str), Some("nvim"));
        let git = serde_yaml::to_string(&f.files[".gitconfig"]).unwrap();
        assert!(git.contains("name: Ada") && git.contains("defaultBranch: main") && git.contains("co: checkout") && git.contains("rebase: true"), "{git}");
        assert_eq!(f.files[".config/git/ignore"], Value::String(".direnv\nresult\n".into()));
        assert_eq!(f.files[".config/starship.toml"]["add_newline"], Value::Bool(false));
        assert_eq!(f.files[".vimrc"], Value::String("set number\nsyntax on\n".into()));
        let src = f.files[".config/nvim"]["source"].as_str().unwrap();
        assert!(src.ends_with("tests/fixtures/import/home/dotfiles/nvim"), "{src}");
        assert_eq!(f.settings["org.gnome.desktop.interface"]["color-scheme"], Value::String("prefer-dark".into()));
        assert_eq!(f.settings["org.gnome.desktop.wm.preferences"]["num-workspaces"], Value::Number(4.into()));
        let u = unconverted(&f);
        // bash's init, then the tools that hook into it (on by default), then
        // nix-direnv's stand-in: nami's own project hook.
        assert_eq!(f.shell["bash"], "set -o vi\neval \"$(direnv hook bash)\"\neval \"$(starship init bash)\"\neval \"$(nami hook bash)\"\n");
        assert_eq!(f.env["PATH"], "$HOME/.local/bin:$PATH", "home.sessionPath, before the rest of PATH");
        for w in ["programs.neovim.plugins", "services.gpg-agent"] {
            assert!(u.contains(&w), "{w} not in {u:?}");
        }
        assert!(u.iter().any(|w| w.contains("repeat-interval")), "{u:?}");
        // Services: yours, from home-manager.
        assert!(f.services.is_empty());
        assert_eq!(f.user_services["syncthing"], Value::Bool(true));
        let sync: Value = serde_yaml::from_str("run: rclone bisync remote:notes /home/ada/notes\nenv: { RCLONE_VERBOSE: '1' }\nafter: [network-online.target]\ndescription: Sync my notes").unwrap();
        assert_eq!(f.user_services["notes-sync"], sync);
        assert!(a.contains(&"rclone"), "{a:?}");
        // The rest of a unit comes along as systemd keys.
        let watcher: Value = serde_yaml::from_str("run: /home/ada/bin/watch\nsystemd: { Service: { ExecStartPre: /home/ada/bin/prepare } }").unwrap();
        assert_eq!(f.user_services["watcher"], watcher);
    }

    #[test]
    fn nixos() {
        let (kind, f) = read("nixos/configuration.nix", None);
        assert_eq!(kind, Kind::Nixos);
        assert_eq!(attrs(&f), ["vim", "wget", "git", "firefox", "htop", "noto-fonts", "steam", "zsh", "curl", "podman", "hello"]);
        // Docker: its nami file and settings; the docker group's people
        // once the file is the system's (retarget), when they're os.users.
        assert_eq!(f.includes, ["docker"]);
        let tops: BTreeMap<String, Value> = f.top.iter().map(|(p, v)| (p.join("."), v.clone())).collect();
        let yv = |s: &str| -> Value { serde_yaml::from_str(s).unwrap() };
        assert_eq!(tops["users.ada"], yv("{ admin: true, name: Ada }"));
        assert_eq!(f.etc_files["/etc/docker/daemon.json"], yv("{ content: { log-driver: json-file, data-root: /srv/docker, registry-mirrors: [https://mirror.example.org], default-address-pools: [{ base: 172.30.0.0/16, size: 24 }] }, restarts: docker.service }"));
        assert_eq!(f.group_members, [("docker".to_string(), "ada".to_string(), "users.users.ada.extraGroups".to_string())]);
        let u = unconverted(&f);
        for w in ["virtualisation.docker.autoPrune", "virtualisation.podman.dockerCompat", "virtualisation.podman.defaultNetwork", "virtualisation.libvirtd"] {
            assert!(u.contains(&w), "{w} not in {u:?}");
        }
        let mut sys = Found { includes: f.includes.clone(), top: f.top.clone(), group_members: f.group_members.clone(), ..Default::default() };
        retarget(&mut sys, Tier::System, Path::new("/"));
        assert!(sys.top.iter().any(|(p, v)| p.join(".") == "users.ada" && *v == yv("{ admin: true, name: Ada, groups: [docker] }")), "{:?}", sys.top);
        assert!(!sys.unconverted.iter().any(|u| u.what.contains("extraGroups")), "{:?}", sys.unconverted);
        let mut user = Found { includes: f.includes.clone(), group_members: f.group_members.clone(), ..Default::default() };
        retarget(&mut user, Tier::User, Path::new("/"));
        assert!(user.top.is_empty() && unconverted(&user) == ["- docker", "users.users.ada.extraGroups"], "{:?}", user.unconverted);
        let u = unconverted(&f);
        for w in ["hardware-configuration.nix", "boot.loader", "networking.hostName", "services.openssh", "users.users.ada.packages", "home-manager.users.ada",
            "services.openssh.openFirewall", "services.xserver", "services.tailscale.useRoutingFeatures", "services.syncthing.dataDir", "services.postgresql",
            "environment.etc.\"sudoers\"", "environment.etc.\"resolv.conf\".source", "systemd.services.nix-daemon", "time.timeZone"] {
            assert!(u.contains(&w), "{w} not in {u:?}");
        }
        assert!(!u.iter().any(|w| w.starts_with("services.printing")), "off: says nothing: {u:?}");
        let why = |what: &str| f.unconverted.iter().find(|u| u.what == what).unwrap().why.clone();
        assert!(why("services.openssh").contains("ships no unit"));
        assert!(why("environment.etc.\"sudoers\"").contains("sudoers.d"));
        assert_eq!(f.services["backup"], serde_yaml::from_str::<Value>("script: |\n  cd /srv\n  tar czf /backup/srv.tgz .\nuser: root").unwrap(), "several lines: a script");
        assert!(why("time.timeZone").contains("timedatectl set-timezone Europe/Copenhagen"));
        assert!(why("networking.hostName").contains("hostnamectl set-hostname laptop"));
        // Services with a unit a package ships, and your own.
        let y = |s: &str| -> Value { serde_yaml::from_str(s).unwrap() };
        assert_eq!(f.services["tailscaled"], y("package: tailscale\nenv: { PORT: '41641' }"));
        assert_eq!(f.services["syncthing@ada"], Value::Bool(true));
        assert_eq!(f.services["myapp"], y("run: hello --greeting hi\nenv: { PORT: '8080', RUST_LOG: info }\nafter: [network-online.target]\nuser: myapp\nrestart: always\ndescription: My app"));
        assert_eq!(f.services["ping"], y("run: ping -c1 example.org\nuser: root"), "NixOS runs it as root");
        assert_eq!(f.services.len(), 5, "{:?}", f.services.keys());
        // /etc files, and openssh's settings as a drop-in.
        assert_eq!(f.etc_files["/etc/motd"], Value::String("Welcome to the laptop.\n".into()));
        let app = &f.etc_files["/etc/myapp/config.toml"];
        assert!(app["source"].as_str().unwrap().ends_with("tests/fixtures/import/nixos/myapp.toml") && app["mode"] == "0640", "{app:?}");
        assert_eq!(f.etc_files[SSHD_DROPIN].as_str().unwrap().lines().skip(1).collect::<Vec<_>>(),
            ["Port 2222", "AllowUsers ada backup", "KexAlgorithms curve25519-sha256,sntrup761x25519-sha512", "PasswordAuthentication no", "PermitRootLogin no"]);
        assert_eq!(f.etc_files[nixos::TMPFILES], Value::String("d /srv 0755 root root -\n".into()));
        assert_eq!(f.etc_files.len(), 5, "{:?}", f.etc_files.keys());
        // As the user: their own packages and home-manager settings.
        let (_, f) = read("nixos/configuration.nix", Some(Tier::User));
        assert!(attrs(&f).contains(&"thunderbird") && f.aliases.contains_key("update"), "{:?}", attrs(&f));
    }

    #[test]
    fn nixos_system_file_is_valid_and_lints_clean() {
        let (_, mut f) = read("nixos/configuration.nix", None);
        check_packages(&mut f, None);
        let dir = fixture("nixos");
        retarget(&mut f, Tier::System, &dir);
        assert_eq!(f.etc_files["/etc/myapp/config.toml"]["source"], Value::String("./myapp.toml".into()));
        let (text, added, _) = merge("", &f, Tier::System, "configuration.nix").unwrap();
        // Where it's written: next to its files, with the examples it lists copied in.
        let dir = {
            let to = std::env::temp_dir().join(format!("nami-nixos-import-{}/laptop", std::process::id()));
            std::fs::create_dir_all(&to).unwrap();
            for e in std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.path().is_file()) {
                std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
            }
            crate::new::copy_packages(&to, &f.includes).unwrap();
            to
        };
        let diags = crate::config::check_tier(&text, &dir, None, crate::config::Tier::System);
        let errors: Vec<_> = diags.iter().filter(|d| d.severity == crate::config::Severity::Error).map(|d| &d.message).collect();
        assert!(errors.is_empty(), "{errors:?}\n{text}");
        assert!(added.contains(&"service syncthing@ada".to_string()) && added.contains(&"file /etc/motd".to_string()), "{added:?}");
        let parsed = crate::config::load_text(&text, "t", &dir).unwrap();
        assert_eq!(crate::services::parse_all(&parsed.services).unwrap().len(), 5);
        for f in ["/etc/docker/daemon.json", "/etc/motd", "/etc/myapp/config.toml", "/etc/ssh/sshd_config.d/10-nami.conf", "/etc/tmpfiles.d/60-nami.conf"] {
            assert!(parsed.files.contains_key(f), "no {f}: {:?}", parsed.files.keys().collect::<Vec<_>>());
        }
        // The same configuration into a user file: the computer's services and /etc files are pointed at, once.
        let (_, mut f) = read("nixos/configuration.nix", Some(Tier::User));
        retarget(&mut f, Tier::User, &dir);
        assert!(f.services.is_empty() && f.etc_files.is_empty());
        assert!(f.unconverted.iter().any(|u| u.what == "the rest of the NixOS configuration" && u.why.contains("--to system")), "{:?}", f.unconverted);
    }

    #[test]
    fn shells_and_desktop_look() {
        let (_, f) = read("shells/home.nix", None);
        assert_eq!(
            f.shell["zsh"],
            [
                "bindkey -v",
                "autoload -U compinit && compinit",
                "setopt autocd",
                "setopt HIST_IGNORE_DUPS\nSAVEHIST=50000\nsetopt SHARE_HISTORY\nHISTSIZE=50000",
                "ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE='fg=#888'",
                "source \"$(nami_share zsh/plugins/zsh-autosuggestions/zsh-autosuggestions.zsh zsh-autosuggestions/zsh-autosuggestions.zsh)\"",
                "source \"$(nami_share zsh-nix-shell)\"/nix-shell.plugin.zsh",
                "alias -g G='| grep'",
                "source \"$(nami_share zsh/plugins/you-should-use/you-should-use.plugin.zsh)\"\nbindkey '^R' history-incremental-search-backward",
                "eval \"$(zoxide init zsh)\"",
                "source <(fzf --zsh)",
                "eval \"$(atuin init zsh)\"",
                "source \"$(nami_share zsh-syntax-highlighting/zsh-syntax-highlighting.zsh zsh/plugins/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh)\"",
                "source \"$(nami_share zsh/plugins/zsh-history-substring-search/zsh-history-substring-search.zsh zsh-history-substring-search/zsh-history-substring-search.zsh)\"",
                "bindkey '^[[A' history-substring-search-up\nbindkey '^[[B' history-substring-search-down",
            ]
            .map(|l| format!("{l}\n"))
            .concat()
        );
        assert_eq!(
            f.shell["fish"],
            "function gst\ngit status\nend\nfunction mkcd --description 'make and enter'\nmkdir -p $argv[1]; and cd $argv[1]\nend\nset fish_greeting\nzoxide init fish | source\natuin init fish | source\n",
            "fzf's fish integration is off"
        );
        assert!(!f.shell.contains_key("bash"), "bash isn't enabled, so nothing hooks into it");
        assert_eq!(f.aliases.get("ll").map(String::as_str), Some("eza -l"));
        let a = attrs(&f);
        for p in ["zsh-autosuggestions", "zsh-syntax-highlighting", "zsh-history-substring-search", "zsh-nix-shell", "zsh-you-should-use", "bibata-cursors", "gnome-themes-extra", "papirus-icon-theme", "zoxide", "eza"] {
            assert!(a.contains(&p), "{p} missing from {a:?}");
        }
        let u = unconverted(&f);
        for w in ["programs.zsh.plugins.fetched", "programs.atuin.enableNushellIntegration", "programs.kitty.shellIntegration"] {
            assert!(u.contains(&w), "{w} not in {u:?}");
        }
        // The cursor everywhere, and GTK's look.
        assert_eq!(f.env["XCURSOR_THEME"], "Bibata-Modern-Classic");
        assert_eq!(f.env["XCURSOR_SIZE"], "32");
        assert!(f.files[".icons/default/index.theme"].as_str().unwrap().contains("Inherits=Bibata-Modern-Classic"));
        let gtk3 = &f.files[".config/gtk-3.0/settings.ini"]["Settings"];
        assert_eq!(gtk3["gtk-theme-name"], "Adwaita-dark");
        assert_eq!(gtk3["gtk-icon-theme-name"], "Papirus-Dark");
        assert_eq!(gtk3["gtk-font-name"], "Inter 11");
        assert_eq!(gtk3["gtk-cursor-theme-size"], 32);
        assert_eq!(gtk3["gtk-application-prefer-dark-theme"], 1);
        assert!(f.files[".config/gtk-4.0/settings.ini"]["Settings"].get("gtk-application-prefer-dark-theme").is_none(), "gtk3.extraConfig is GTK 3's");
        assert_eq!(f.files[".config/gtk-3.0/bookmarks"], Value::String("file:///home/sam/Projects\n".into()));
        let gnome = &f.settings["org.gnome.desktop.interface"];
        assert_eq!((gnome["cursor-theme"].as_str(), gnome["icon-theme"].as_str(), gnome["font-name"].as_str()), (Some("Bibata-Modern-Classic"), Some("Papirus-Dark"), Some("Inter 11")));
        // Programs whose options are their own config file.
        assert_eq!(
            f.files[".config/tmux/tmux.conf"].as_str().unwrap(),
            "set -g base-index 1\nsetw -g pane-base-index 1\nset -g status-keys vi\nset -g mode-keys vi\nset -g mouse on\nunbind C-b\nset -g prefix C-a\nbind C-a send-prefix\n\
             run-shell 'for f in ~/.local/state/nami/profile/share/tmux-plugins/sensible/*.tmux; do \"$f\"; done'\nset -g @resurrect-strategy-nvim 'session'\n\
             run-shell 'for f in ~/.local/state/nami/profile/share/tmux-plugins/resurrect/*.tmux; do \"$f\"; done'\nbind r source-file ~/.config/tmux/tmux.conf\n"
        );
        assert!(a.contains(&"tmuxPlugins.resurrect") && a.contains(&"tmux"), "{a:?}");
        assert_eq!(
            f.files[".ssh/config"].as_str().unwrap(),
            "Include ~/.ssh/config.d/*\n\nHost zeta\n  HostName zeta.example.org\n\nHost work\n  HostName 10.0.0.5\n  IdentitiesOnly yes\n  IdentityFile ~/.ssh/work\n  LocalForward 8080 localhost:80\n  Port 2222\n\nHost *.example.org\n  ForwardAgent yes\n  User sam\n\nHost *\n  ServerAliveInterval 60\n"
        );
        assert_eq!(f.env["FZF_DEFAULT_COMMAND"], "fd --type f");
        assert_eq!(f.env["FZF_DEFAULT_OPTS"], "--height 40% --border");
        // And it's a user file nami accepts, as it is.
        let (mut f, dir) = (f, fixture("shells"));
        retarget(&mut f, Tier::User, &dir);
        let (text, _, _) = merge("", &f, Tier::User, "home.nix").unwrap();
        let diags = crate::config::check(&text, crate::config::USER_FILE, &dir, None);
        let errors: Vec<_> = diags.iter().filter(|d| d.severity == crate::config::Severity::Error).map(|d| &d.message).collect();
        assert!(errors.is_empty(), "{errors:?}\n{text}");
        let parsed = crate::config::parse_config(&text, crate::config::USER_FILE).unwrap();
        assert!(parsed.home.unwrap().shell["zsh"].contains("compinit"));
    }

    #[test]
    fn nixos_flake_for_the_user_and_the_system() {
        // --to user: the NixOS configuration's home-manager user and user
        // units, not its dev shell or the computer's packages.
        let (kind, f) = read("nixos-desktop", Some(Tier::User));
        assert_eq!(kind, Kind::Nixos);
        assert_eq!(attrs(&f), ["ripgrep", "fish", "starship"]);
        assert_eq!(f.shell["fish"], "starship init fish | source\n");
        assert_eq!(f.login_shell.as_deref(), Some("fish"), "users.users.sam.shell: terminals open fish");
        assert_eq!(f.aliases.get("g").map(String::as_str), Some("git"));
        let y = |s: &str| -> Value { serde_yaml::from_str(s).unwrap() };
        assert_eq!(f.user_services["notes"], y("run: notes-server --port 4000\ndescription: Notes server"), "a user unit: no user: root");
        let why = |f: &Found, what: &str| f.unconverted.iter().find(|u| u.what == what).map(|u| u.why.clone()).unwrap_or_else(|| panic!("{what} not in {:?}", unconverted(f)));
        assert!(why(&f, "users.users.sam.extraGroups").contains("usermod -aG wheel,video sam"));
        assert!(!unconverted(&f).iter().any(|w| *w == "users.users.sam.shell"));
        assert!(why(&f, "the rest of the NixOS configuration").contains("--to system"));
        assert!(!unconverted(&f).iter().any(|w| w.starts_with("boot") || w.starts_with("i18n")), "the computer's settings aren't listed one by one: {:?}", unconverted(&f));

        // --to system: everything that has a place on any distro.
        let (_, mut f) = read("nixos-desktop", Some(Tier::System));
        let a = attrs(&f);
        for p in ["htop", "hyprland", "xdg-desktop-portal-hyprland", "firefox", "localsend"] {
            assert!(a.contains(&p), "{p} missing from {a:?}");
        }
        assert!(!a.contains(&"ripgrep") && !a.contains(&"nixfmt"), "{a:?}");
        let text_of = |f: &Found, p: &str| f.etc_files[p].as_str().map(String::from).unwrap_or_default();
        let e = |p: &str| text_of(&f, p);
        assert_eq!(e(nixos::SYSCTL), "net.ipv4.ip_forward = 1\nvm.swappiness = 10\n");
        assert_eq!(e(nixos::MODULES_LOAD), "v4l2loopback\n");
        assert_eq!(e(nixos::MODPROBE), "blacklist pcspkr\noptions v4l2loopback exclusive_caps=1\n");
        assert_eq!(e(nixos::TMPFILES), "d /srv/media 0775 sam users\n");
        assert!(e(nixos::UDEV_RULES).contains("idVendor"));
        assert_eq!(f.etc_files["/etc/firefox/policies/policies.json"], y("policies: { DisableTelemetry: true }"));
        assert_eq!(f.etc_files["/etc/gitconfig"], y("init: { defaultBranch: main }"));
        assert_eq!(f.unfree, Some(false));
        for (what, says) in [
            ("i18n", "localectl set-locale LANG=en_GB.UTF-8 LC_TIME=de_DE.UTF-8"),
            ("console.keyMap", "localectl set-keymap uk"),
            ("services.xserver.xkb", "localectl set-x11-keymap gb \"\" extd"),
            ("programs.localsend.openFirewall", "firewall"),
            ("systemd.user", "--to user"),
        ] {
            assert!(why(&f, what).contains(says), "{what}: {}", why(&f, what));
        }
        assert!(f.warnings.iter().any(|w| w.contains("wayland-sessions")), "{:?}", f.warnings);
        retarget(&mut f, Tier::System, &fixture("nixos-desktop"));
        assert!(text_of(&f, nixos::ENVIRONMENT_D).contains("MOZ_ENABLE_WAYLAND=1"));
        assert!(text_of(&f, nixos::PROFILE_D).contains("export MOZ_ENABLE_WAYLAND='1'"));
        let (text, _, _) = merge("", &f, Tier::System, "flake.nix").unwrap();
        let diags = crate::config::check_tier(&text, &fixture("nixos-desktop"), None, crate::config::Tier::System);
        let errors: Vec<_> = diags.iter().filter(|d| d.severity == crate::config::Severity::Error).map(|d| &d.message).collect();
        assert!(errors.is_empty(), "{errors:?}\n{text}");
        assert!(text.contains("\nunfree: false\n"), "{text}");
    }

    #[test]
    fn nasty_shapes() {
        let (_, f) = read("nasty/shell.nix", None);
        // (Read for the platform nami runs on: a Mac's has its `isDarwin` list too.)
        let mut want = vec!["gnumake", "hello", "cowsay", "lolcat", "python3", "python3Packages.numpy", "nodePackages.typescript"];
        if crate::index::MAC {
            want.insert(4, "darwin.apple_sdk.frameworks.Security");
        }
        assert_eq!(attrs(&f), want);
        assert!(unconverted(&f).contains(&"nixpkgs overlays"), "{:?}", f.unconverted);
        assert_eq!(f.env.get("GREETING").map(String::as_str), Some("hello, world"));
        assert_eq!(f.env.get("MULTI").map(String::as_str), Some("a b"));
        assert!(unconverted(&f).contains(&"PROJECT_ROOT"), "{:?}", f.unconverted);
    }

    #[test]
    fn catalog_renames_and_rejects() {
        let c = Catalog::new(["hello", "cowsay", "python3", "python313Packages.numpy", "python312Packages.numpy", "typescript", "gnumake", "gnome-tweaks", "vscode-fhs"]);
        assert_eq!(c.resolve("vscode.fhs").as_deref(), Some("vscode-fhs"));
        assert_eq!(c.resolve("python3Packages.numpy").as_deref(), Some("python313Packages.numpy"));
        assert_eq!(c.resolve("python3.pkgs.numpy").as_deref(), Some("python313Packages.numpy"));
        assert_eq!(c.resolve("nodePackages.typescript").as_deref(), Some("typescript"));
        assert_eq!(c.resolve("gnome.gnome-tweaks").as_deref(), Some("gnome-tweaks"));
        assert_eq!(c.resolve("rust-bin.stable.latest.default"), None);
        let (_, mut f) = read("nasty/shell.nix", None);
        check_packages(&mut f, Some(&c));
        assert_eq!(attrs(&f), ["gnumake", "hello", "cowsay", "python3", "python313Packages.numpy", "typescript"]);
        assert!(f.unconverted.iter().any(|u| u.what == "lolcat" && u.why == "not in the catalog"));
    }

    #[test]
    fn merging_keeps_comments_and_values() {
        let existing = "# my project\npackages:\n  - jq   # for scripts\nenv:\n  RUST_LOG: info  # quieter\n";
        let (_, mut f) = read("flake-utils", None);
        check_packages(&mut f, None);
        retarget(&mut f, Tier::Project, Path::new("/nowhere"));
        let (text, added, kept) = merge(existing, &f, Tier::Project, "flake.nix").unwrap();
        assert!(text.starts_with("# my project\npackages:\n  - jq   # for scripts\n"), "{text}");
        assert!(text.contains("RUST_LOG: info  # quieter"), "{text}");
        assert!(kept.iter().any(|k| k.starts_with("env RUST_LOG")), "{kept:?}");
        assert!(added.contains(&"package rustc".to_string()) && !added.contains(&"package jq".to_string()));
        assert!(text.contains("\n# Not converted from flake.nix (nami import):\n#   LD_LIBRARY_PATH: computed by Nix (`pkgs.lib.makeLibraryPath [ pkgs.openssl ]`)\n") && text.contains("#   shellHook: a line of shell"), "{text}");
        let mut parsed: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        crate::config::package_map(&mut parsed, "t").unwrap();
        let parsed: crate::project::Project = serde_yaml::from_value(parsed).unwrap();
        assert_eq!(parsed.tasks["dev"].run.as_deref(), Some("cargo watch -x run"));
        assert_eq!(parsed.develop, Some(true));
        // Again: nothing changes.
        let (again, added, _) = merge(&text, &f, Tier::Project, "flake.nix").unwrap();
        assert_eq!(again, text);
        assert!(added.is_empty());
        // Another import: its keys go above the earlier notes, its notes below them.
        let mut more = Found::default();
        more.env.insert("EXTRA".into(), "1".into());
        more.skip("something", "a reason");
        let (text, _, _) = merge(&text, &more, Tier::Project, "shell.nix").unwrap();
        let (env_at, notes_at) = (text.find("EXTRA: '1'").unwrap(), text.find("# Not converted from flake.nix").unwrap());
        assert!(env_at < notes_at && text.ends_with("\n\n# Not converted from shell.nix (nami import):\n#   something: a reason\n"), "{text}");
    }

    #[test]
    fn user_file_is_valid_and_lints_clean() {
        let (_, mut f) = read("home/home.nix", None);
        check_packages(&mut f, None);
        let dir = fixture("home");
        retarget(&mut f, Tier::User, &dir);
        assert_eq!(f.files[".config/nvim"]["source"], Value::String("./dotfiles/nvim".into()));
        let (text, _, _) = merge("", &f, Tier::User, "home.nix").unwrap();
        let diags = crate::config::check(&text, crate::config::USER_FILE, &dir, None);
        let errors: Vec<_> = diags.iter().filter(|d| d.severity == crate::config::Severity::Error).map(|d| &d.message).collect();
        assert!(errors.is_empty(), "{errors:?}\n{text}");
        assert!(text.contains("\n  ~/.gitconfig:\n    user:\n      email: ada@example.org\n"), "{text}");
        assert!(text.contains("  ~/.vimrc: |\n    set number\n"), "{text}");
        assert!(text.contains("\nservices:\n") && text.contains("  - syncthing\n") && text.contains("  - notes-sync:\n      run: rclone bisync"), "{text}");
    }

    #[test]
    fn system_file_takes_only_packages() {
        let (_, mut f) = read("home/home.nix", None);
        retarget(&mut f, Tier::System, Path::new("/etc/nami"));
        assert!(f.env.is_empty() && f.files.is_empty() && f.aliases.is_empty() && f.settings.is_empty() && f.services.is_empty());
        assert!(f.unconverted.iter().any(|u| u.what == "env EDITOR" && u.why.contains("--to user")));
        assert!(f.unconverted.iter().any(|u| u.what == "service syncthing" && u.why.contains("--to user")));
    }

    #[test]
    fn nix_layer_when_nix_is_here() {
        if !nixtool::here() || std::env::var("NAMI_TEST_NIX").is_ok_and(|v| v == "0") {
            eprintln!("skipped: no nix");
            return;
        }
        let dir = fixture("devshells");
        let Some(pkgs) = nix_shell_packages(&dir, Duration::from_secs(120), &["--override-input", "nixpkgs", "flake:nixpkgs"]) else {
            eprintln!("skipped: nix couldn't evaluate the flake here (offline?)");
            return;
        };
        let names: Vec<&str> = pkgs.iter().map(|p| p.pname.as_str()).collect();
        for p in ["go", "gopls", "delve"] {
            assert!(names.contains(&p), "{p} not in {names:?}");
        }
        let (_, mut f) = read("devshells/flake.nix", None);
        let before = f.packages.len();
        merge_nix(&mut f, &pkgs, None);
        assert_eq!(f.packages.len(), before, "nothing new: the static reader had them all: {:?}", f.packages);
    }
}
