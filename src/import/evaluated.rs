//! Reading a flake's NixOS or home-manager configuration with Nix itself
//! (eval.nix), when Nix is installed: what its own files set, evaluated, in
//! the shape they're written in, so the same mappings read it as a file —
//! however the flake is put together (flake-parts, helper functions, a hosts
//! directory...). The static reader is the fallback.
//!
//! Store paths in the values become what nami installs: a package's
//! `/nix/store/…-x/bin/cmd` the command, `…/share/…` in shell code
//! `nami_share`, any other path in a package the same path in the profile.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::Value as J;

use super::{pname, Catalog, Kind, Reader, Tier};
use crate::nix::expr::{Thunk, Val};

const EXPR: &str = include_str!("eval.nix");

/// What eval.nix found.
pub struct Evaluated {
    pub kind: Kind,
    /// The configuration it read (`nixosConfigurations.<name>`...), and the others.
    pub name: String,
    pub names: Vec<String>,
    pub config: Option<J>,
    pub home: Option<J>,
    pub home_user: String,
    pub home_users: Vec<String>,
    /// The flake's copy in the store, which paths in it point into.
    pub src: String,
    /// The locked reference it was read from, and the arguments: for
    /// building its home-manager files afterwards.
    pub flake: String,
    pub args: String,
    /// Parts that couldn't be evaluated: (root, why).
    pub failed: Vec<(String, String)>,
}

/// The flake as a locked reference (`path:...?narHash=...`), so it can be
/// evaluated in pure mode: a configuration can't read files outside the
/// store or environment variables while nami reads it (the machine's keys
/// and tokens stay out of reach), nor fetch anything unpinned.
fn locked(flake_dir: &Path, deadline: Instant) -> Result<String, String> {
    let out = run_nix(&["flake", "prefetch", "--json", &format!("path:{}", flake_dir.display())], deadline)?;
    let j: J = serde_json::from_slice(&out).map_err(|e| format!("nix flake prefetch: {e}"))?;
    match (j["storePath"].as_str(), j["hash"].as_str()) {
        (Some(p), Some(h)) => Ok(format!("path:{p}?narHash={h}")),
        _ => Err("nix flake prefetch gave no store path".into()),
    }
}

/// Import from derivation (a configuration that builds something to read
/// it) is off unless the configuration needs it: then Nix builds that, in
/// its sandbox, and the reading is tried again.
fn nix_eval(flake: &str, args: &str, deadline: Instant) -> Result<J, String> {
    let expr = format!("({EXPR}) {{ flake = {}; {args} }}", nix_string(flake));
    let eval = |ifd: &str| run_nix(&["eval", "--json", "--pure-eval", "--option", "allow-import-from-derivation", ifd, "--expr", &expr], deadline);
    let out = match eval("false") {
        Err(why) if why.contains("cannot build") || why.contains("import-from-derivation") || why.contains("import from derivation") => eval("true")?,
        other => other?,
    };
    serde_json::from_slice(&out).map_err(|e| format!("nix eval's output: {e}"))
}

/// `nix <args>`'s output, or why not (its last error line).
fn run_nix(args: &[&str], deadline: Instant) -> Result<Vec<u8>, String> {
    run_nix_both(args, deadline).map(|(out, _)| out)
}

/// `nix <args>`'s output and messages.
fn run_nix_both(args: &[&str], deadline: Instant) -> Result<(Vec<u8>, Vec<u8>), String> {
    let nix = super::nixtool::get().ok_or("there's no Nix")?;
    let mut child = nix
        .command()
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("can't run nix: {e}"))?;
    let (mut stdout, mut stderr) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let out = std::thread::spawn(move || {
        let mut v = Vec::new();
        std::io::Read::read_to_end(&mut stdout, &mut v).ok();
        v
    });
    let err = std::thread::spawn(move || {
        let mut v = Vec::new();
        std::io::Read::read_to_end(&mut stderr, &mut v).ok();
        v
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
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    match status {
        Some(s) if s.success() => Ok((out, err)),
        Some(_) => {
            let err = String::from_utf8_lossy(&err);
            let why = err.lines().rev().find(|l| l.trim_start().starts_with("error:")).unwrap_or("it failed").trim().trim_start_matches("error:").trim();
            Err(why.to_string())
        }
        None => Err("it took too long (NAMI_IMPORT_NIX_TIMEOUT)".into()),
    }
}

fn nix_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace("${", "\\${"))
}

/// Evaluate the flake in `flake_dir` for `want` (your user file: a home
/// configuration, else a NixOS one's home-manager user; the system file:
/// the NixOS configuration). None: Nix isn't usable here, or the flake has
/// neither (a dev shell's flake), or it failed altogether.
pub fn evaluate(flake_dir: &Path, want: Tier, timeout: Duration) -> Result<Option<Evaluated>, String> {
    let deadline = Instant::now() + timeout;
    let user = std::env::var("USER").unwrap_or_default();
    let host = std::fs::read_to_string("/etc/hostname").unwrap_or_default().trim().to_string();
    let want_s = if want == Tier::System { "system" } else { "user" };
    let base = format!("want = \"{want_s}\"; user = {}; host = {};", nix_string(&user), nix_string(&host));
    if let Some(nix) = super::nixtool::get() {
        nix.tidy(deadline);
    }
    let flake = locked(flake_dir, deadline)?;
    let flake_dir = flake.as_str();
    let whole = nix_eval(flake_dir, &base, deadline);
    let mut failed = Vec::new();
    let j = match whole {
        Ok(j) => j,
        Err(why) => {
            // Something in it aborts (an option nothing uses that doesn't
            // evaluate): read it part by part, keeping what does.
            let shape = nix_eval(flake_dir, &format!("{base} part = \"none\";"), deadline).map_err(|_| why.clone())?;
            if shape["kind"] == "none" {
                return Ok(None);
            }
            let mut merged = shape.clone();
            for (part, roots_key) in [("config", "roots"), ("home", "homeRoots")] {
                if part == "home" && shape["homeUser"].as_str().unwrap_or("").is_empty() && shape["kind"] != "home" {
                    continue;
                }
                let mut tree = serde_json::Map::new();
                for root in shape[roots_key].as_array().into_iter().flatten().filter_map(J::as_str) {
                    let p = if shape["kind"] == "home" { "config" } else { part };
                    match nix_eval(flake_dir, &format!("{base} part = \"{p}\"; only = \"{root}\";"), deadline) {
                        Ok(r) => {
                            if let Some(J::Object(m)) = r.get(p) {
                                tree.extend(m.clone());
                            }
                        }
                        Err(e) => failed.push((root.to_string(), e)),
                    }
                }
                merged[if shape["kind"] == "home" { "config" } else { part }] = J::Object(tree);
                if shape["kind"] == "home" {
                    break;
                }
            }
            merged
        }
    };
    let kind = match j["kind"].as_str() {
        Some("nixos") => Kind::Nixos,
        Some("home") => Kind::HomeManager,
        _ => return Ok(None),
    };
    let strs = |v: &J| v.as_array().into_iter().flatten().filter_map(J::as_str).map(String::from).collect::<Vec<_>>();
    Ok(Some(Evaluated {
        kind,
        name: j["name"].as_str().unwrap_or_default().to_string(),
        names: strs(&j["names"]),
        config: j.get("config").filter(|c| !c.is_null()).cloned(),
        home: j.get("home").filter(|c| !c.is_null()).cloned(),
        home_user: j["homeUser"].as_str().unwrap_or_default().to_string(),
        home_users: strs(&j["homeUsers"]),
        src: j["src"].as_str().unwrap_or_default().to_string(),
        flake: flake_dir.to_string(),
        args: base.clone(),
        failed,
    }))
}

/// JSON from eval.nix as the reader's values.
pub struct Conv<'a> {
    pub catalog: Option<&'a Catalog<'a>>,
    /// Where the target file's packages are: the profile store paths become.
    pub profile: String,
    /// Packages store paths in values pointed at.
    pub used: Vec<String>,
    /// Store paths that are no package nami knows (generated by Nix).
    pub unknown: Vec<String>,
    /// (the flake's copy in the store, the flake's directory): a path into
    /// the copy is the file in your repo, which stays when the copy is gone.
    pub flake: Option<(String, String)>,
    /// Text files Nix would write (writeText and friends), by store path:
    /// read from their derivations, so they needn't be built.
    pub texts: BTreeMap<String, String>,
}

impl<'a> Conv<'a> {
    pub fn new(catalog: Option<&'a Catalog<'a>>, tier: Tier) -> Self {
        let profile = match tier {
                        // (A file goes from computer to computer: by the place every one has.)
            Tier::System => format!("/{}/profiles/default", crate::migrate::VAR),
            _ => format!("{}/.local/state/nami/profile", std::env::var("HOME").unwrap_or_default()),
        };
        Conv { catalog, profile, used: Vec::new(), unknown: Vec::new(), flake: None, texts: BTreeMap::new() }
    }

    /// The catalog's attribute for a package Nix evaluated.
    fn attr(&self, name: &str, pn: &str, out: &str) -> Option<String> {
        let pn = if pn.is_empty() { pname(name) } else { pn };
        match self.catalog {
            None => (!pn.is_empty()).then(|| pn.to_string()),
            Some(c) => c.paths.get(out).map(|a| a.to_string()).or_else(|| c.has(pn).then(|| pn.to_string())).or_else(|| c.attr_for_pname(pn)),
        }
    }

    pub fn val(&mut self, j: &J, path: &mut Vec<String>) -> Val {
        match j {
            J::Null => Val::Null,
            J::Bool(b) => Val::Bool(*b),
            J::Number(n) => n.as_i64().map(Val::Int).unwrap_or_else(|| Val::Float(n.as_f64().unwrap_or(0.0))),
            J::String(s) => {
                let shell = path.first().is_some_and(|p| p == "programs") && path.get(1).is_some_and(|p| matches!(p.as_str(), "zsh" | "bash" | "fish"));
                self.string(s, shell)
            }
            J::Array(items) => Val::List(Rc::new(items.iter().map(|i| Thunk::val(self.val(i, path))).collect())),
            J::Object(m) => {
                if let Some(p) = m.get("__pkg") {
                    let get = |k: &str| p[k].as_str().unwrap_or_default();
                    // Not in the catalog: by its name, which the catalog check lists.
                    let fallback = if get("pname").is_empty() { pname(get("name")).to_string() } else { get("pname").to_string() };
                    return Val::Pkg(self.attr(get("name"), get("pname"), get("out")).unwrap_or(fallback));
                }
                if let Some(p) = m.get("__path").and_then(J::as_str) {
                    return Val::Path(PathBuf::from(self.in_repo(p).unwrap_or_else(|| p.to_string())));
                }
                if let Some(u) = m.get("__unknown") {
                    return Val::Unknown(u.as_str().unwrap_or("not readable").to_string());
                }
                let mut out = BTreeMap::new();
                for (k, v) in m {
                    path.push(k.clone());
                    out.insert(k.clone(), Thunk::val(self.val(v, path)));
                    path.pop();
                }
                Val::Attrs(Rc::new(out))
            }
        }
    }

    /// A path into the flake's store copy, as the same path in the repo.
    fn in_repo(&self, p: &str) -> Option<String> {
        let (src, dir) = self.flake.as_ref()?;
        let rest = p.strip_prefix(src.as_str())?;
        (rest.is_empty() || rest.starts_with('/')).then(|| format!("{dir}{rest}"))
    }

    /// Text with store paths in it: what they are once nami installs the
    /// packages. Exact unless one is something only Nix makes.
    fn string(&mut self, s: &str, shell: bool) -> Val {
        const STORE: &str = "/nix/store/";
        if !s.contains(STORE) {
            return Val::Str(s.to_string(), true);
        }
        if let Some(p) = self.in_repo(s) {
            return Val::Path(PathBuf::from(p));
        }
        let (mut out, mut rest, mut exact) = (String::new(), s, true);
        while let Some(i) = rest.find(STORE) {
            out += &rest[..i];
            let after = &rest[i + STORE.len()..];
            let end = after.find(|c: char| c.is_whitespace() || "\"'`;:,)(|&<>$".contains(c)).unwrap_or(after.len());
            let full = &after[..end];
            let (base, inner) = full.split_once('/').unwrap_or((full, ""));
            let name = if base.len() > 33 && base.as_bytes()[32] == b'-' { &base[33..] } else { base };
            match self.attr(name, "", &format!("{STORE}{base}")) {
                Some(attr) => {
                    if !self.used.contains(&attr) {
                        self.used.push(attr);
                    }
                    out += &match (shell, inner.split_once('/')) {
                        (true, Some(("share", p))) => format!("\"$(nami_share {p})\""),
                        (_, Some(("bin", cmd))) if !cmd.contains('/') => cmd.to_string(),
                        _ if inner.is_empty() => self.profile.clone(),
                        _ => format!("{}/{inner}", self.profile),
                    };
                }
                None => {
                    exact = false;
                    self.unknown.push(format!("{STORE}{base}"));
                    out += &format!("{STORE}{full}");
                }
            }
            rest = &after[end..];
        }
        out += rest;
        Val::Str(out, exact)
    }
}

/// Build the home-manager files of what `e` read (what `home-manager
/// switch` builds), for what the mappings don't cover. NAMI_IMPORT_BUILD=0
/// skips it.
pub fn home_files(e: &Evaluated, timeout: Duration) -> Result<Option<HomeFiles>, String> {
    use super::generated::Item;
    use super::nixtool::{canonical, real};
    // NAMI_IMPORT_BUILD=0: nothing built (what needs building is listed).
    let no_build = std::env::var("NAMI_IMPORT_BUILD").is_ok_and(|v| v == "0");
    let deadline = Instant::now() + timeout;
    let j = nix_eval(&e.flake, &format!("{} part = \"files\";", e.args), deadline)?;
    let Some(files) = j["files"].as_array() else { return Ok(None) };
    let files: Vec<&J> = files.iter().filter(|f| f["target"].is_string()).collect();
    let strs = |v: &J| v.as_array().into_iter().flatten().filter_map(J::as_str).map(String::from).collect::<Vec<_>>();

    // Text files read from their derivations (writeText, writeShellScript...).
    let all: std::collections::BTreeSet<String> = files.iter().flat_map(|f| strs(&f["drvs"]).into_iter().chain(strs(&f["refs"]))).collect();
    let texts = derivation_texts(&all.iter().cloned().collect::<Vec<_>>(), deadline);

    // The rest that Nix makes: a package's comes from the binary cache (nami
    // installs the package instead), what's only this configuration's is
    // built (small things: a config file a generator writes).
    let mut wanted: Vec<String> = Vec::new();
    for f in &files {
        let src = f["source"].as_str().unwrap_or_default();
        if f["text"].is_string() || texts.contains_key(src) {
            continue;
        }
        wanted.extend(strs(&f["drvs"]).into_iter().map(|d| format!("{d}^*")));
    }
    wanted.sort();
    wanted.dedup();
    let mut fetched: std::collections::BTreeSet<String> = Default::default();
    let mut build_error = None;
    if !wanted.is_empty() {
        let args: Vec<&str> = ["build", "--dry-run", "--no-link"].into_iter().chain(wanted.iter().map(String::as_str)).collect();
        let (_, err) = run_nix_both(&args, deadline)?;
        let (mut build, mut section) = (Vec::new(), "");
        for line in String::from_utf8_lossy(&err).lines() {
            let t = line.trim();
            if t.contains("will be built") {
                section = "built";
            } else if t.contains("will be fetched") {
                section = "fetched";
            } else if t.starts_with("/nix/store/") {
                match section {
                    "built" => build.push(format!("{t}^*")),
                    "fetched" => {
                        fetched.insert(t.to_string());
                    }
                    _ => {}
                }
            } else {
                section = "";
            }
        }
        // Only what the files themselves are made by: their inputs come
        // along as Nix needs them.
        let own: std::collections::BTreeSet<&String> = wanted.iter().collect();
        let build: Vec<&str> = build.iter().filter(|d| own.contains(d)).map(String::as_str).collect();
        if !build.is_empty() && no_build {
            build_error = Some("NAMI_IMPORT_BUILD=0".to_string());
        } else if !build.is_empty() {
            // Building may not work here (no user namespaces for Nix's
            // sandbox): then only these files are missing.
            let args: Vec<&str> = ["build", "--no-link", "--keep-going"].into_iter().chain(build).collect();
            build_error = run_nix(&args, deadline).err();
        }
    }

    let mut items = BTreeMap::new();
    for f in files {
        // (zsh's files are at `${dotDir}/.zshrc`, `./.zshrc` without one.)
        let rel = f["target"].as_str().unwrap_or_default().trim_start_matches("./").to_string();
        let src = f["source"].as_str().unwrap_or_default();
        if let Some(t) = f["text"].as_str() {
            items.insert(rel, Item::File(t.as_bytes().to_vec()));
            continue;
        }
        if let Some(t) = texts.get(src) {
            items.insert(rel, Item::File(t.as_bytes().to_vec()));
            continue;
        }
        // In a store path the binary cache has: a package's file.
        let top: String = src.split('/').take(4).collect::<Vec<_>>().join("/");
        if fetched.contains(&top) {
            items.insert(rel, Item::Package(PathBuf::from(src)));
            continue;
        }
        let p = Path::new(src);
        match std::fs::metadata(real(p)) {
            Ok(m) if m.is_dir() => {
                items.insert(rel, Item::Dir(canonical(p)));
            }
            Ok(_) => {
                if let Ok(b) = std::fs::read(real(p)) {
                    items.insert(rel, Item::File(b));
                }
            }
            Err(_) => {
                items.insert(rel, Item::Missing(build_error.clone().unwrap_or_else(|| format!("{src} isn't there"))));
            }
        }
    }
    Ok(Some(HomeFiles { items, texts }))
}

/// home-manager's files, read.
pub struct HomeFiles {
    pub items: BTreeMap<String, super::generated::Item>,
    pub texts: BTreeMap<String, String>,
}

/// The files derivations write from text they carry (nixpkgs'
/// writeTextFile: writeText, writeShellScript, writeShellScriptBin...), by
/// the path they'd be at: no building needed to read them.
fn derivation_texts(drvs: &[String], deadline: Instant) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for chunk in drvs.chunks(200) {
        let args: Vec<&str> = ["derivation", "show"].into_iter().chain(chunk.iter().map(String::as_str)).collect();
        let Ok(bytes) = run_nix(&args, deadline) else { continue };
        let Ok(j) = serde_json::from_slice::<J>(&bytes) else { continue };
        texts_in(&j, &mut out);
    }
    out
}

/// `nix derivation show`'s output: the text files in it, by path.
fn texts_in(j: &J, out: &mut BTreeMap<String, String>) {
    {
        // Newer Nix: { derivations: {...}, version }.
        let all = j.get("derivations").and_then(J::as_object).or_else(|| j.as_object());
        for d in all.into_iter().flat_map(|m| m.values()) {
            let env = &d["env"];
            // Structured attributes keep them in one JSON document.
            let structured = d.get("structuredAttrs").cloned().or_else(|| env["__json"].as_str().and_then(|s| serde_json::from_str(s).ok()));
            let get = |k: &str| structured.as_ref().and_then(|s| s[k].as_str().map(String::from)).or_else(|| env[k].as_str().map(String::from));
            let (Some(text), Some(cmd)) = (get("text"), get("buildCommand")) else { continue };
            if !cmd.contains("$out$destination") && !cmd.contains("${out}${destination}") {
                continue;
            }
            let store_out = env["out"].as_str().map(String::from).or_else(|| d["outputs"]["out"]["path"].as_str().map(|p| if p.starts_with('/') { p.to_string() } else { format!("/nix/store/{p}") }));
            if let Some(o) = store_out {
                out.insert(format!("{o}{}", get("destination").unwrap_or_default()), text);
            }
        }
    }
}

impl Reader {
    /// Read what eval.nix found, as `read` reads a file.
    pub fn read_evaluated(&mut self, e: &Evaluated, catalog: Option<&Catalog>, tier: Tier, file: &Path) -> Kind {
        let mut conv = Conv::new(catalog, tier);
        if !e.src.is_empty() {
            conv.flake = Some((e.src.clone(), file.parent().unwrap_or(file).display().to_string()));
        }
        let mut path = Vec::new();
        let config = e.config.as_ref().map(|c| conv.val(c, &mut path));
        // Your home-manager settings only go in your user file.
        let home = e.home.as_ref().map(|h| if tier == Tier::User { conv.val(h, &mut path) } else { Val::Null });
        if let Some(c) = config {
            self.module(c, e.kind, 0);
        }
        match (home, tier) {
            (Some(h), Tier::User) => self.module(h, Kind::HomeManager, 0),
            (Some(_), _) => self.found.skip(
                format!("home-manager.users.{}", e.home_user),
                format!("{}'s home-manager settings: import them into ~/user.nami with `nami import {} --to user`", e.home_user, file.parent().unwrap_or(file).display()),
            ),
            (None, _) => {}
        }
        if e.kind == Kind::Nixos && tier == Tier::User && e.home_user.is_empty() && !e.home_users.is_empty() {
            self.found.skip("home-manager.users", format!("several people's ({}); none is you ($USER): set USER to the one to import", e.home_users.join(", ")));
        }
        for p in std::mem::take(&mut conv.used) {
            self.found.package(&p, "a store path in a setting");
        }
        // The rest of home-manager: the files it generates, built and read.
        if tier == Tier::User && (e.kind == Kind::HomeManager || !e.home_user.is_empty()) {
            let secs = std::env::var("NAMI_IMPORT_NIX_TIMEOUT").ok().and_then(|s| s.parse().ok()).unwrap_or(1800);
            match home_files(e, Duration::from_secs(secs)) {
                Ok(Some(hf)) => {
                    conv.texts = hf.texts;
                    self.read_generated(hf.items, &mut conv);
                    for p in std::mem::take(&mut conv.used) {
                        self.found.package(&p, "a store path in a generated file");
                    }
                }
                Ok(None) => {}
                Err(why) => self.found.warn(format!("couldn't read home-manager's files ({why}): what the mappings don't cover is listed instead")),
            }
        }
        for (root, why) in &e.failed {
            self.found.skip(root.as_str(), format!("Nix couldn't evaluate this part ({why})"));
        }
        if e.names.len() > 1 {
            let key = if e.kind == Kind::Nixos { "nixosConfigurations" } else { "homeConfigurations" };
            self.found.warn(format!("{key} has {}; imported {}", e.names.join(", "), e.name));
        }
        self.found.warn(format!("read with Nix (`nix eval`): {}", if e.kind == Kind::Nixos { format!("nixosConfigurations.{}", e.name) } else { format!("homeConfigurations.{}", e.name) }));
        self.finish(e.kind, file);
        e.kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv_val(c: &mut Conv, j: &str, path: &[&str]) -> Val {
        c.val(&serde_json::from_str(j).unwrap(), &mut path.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn text_files_from_derivations() {
        let h = "0".repeat(32);
        let cmd = "target=$out$destination\nmkdir -p \"$(dirname \"$target\")\"";
        // Nix 2.3x: { derivations, version }, outputs without /nix/store.
        let new = serde_json::json!({ "version": 4, "derivations": {
            format!("{h}-start.drv"): { "env": { "text": "#!/bin/sh\necho hi\n", "buildCommand": cmd, "destination": "/bin/start" }, "outputs": { "out": { "path": format!("{h}-start") } } },
            format!("{h}-gen.drv"): { "env": { "buildCommand": "remarshal ..." }, "outputs": { "out": { "path": format!("{h}-gen") } } },
        }});
        // Older: drv path -> drv, `out` in env; structured attributes in __json.
        let old = serde_json::json!({
            format!("/nix/store/{h}-hm_x.drv"): { "env": { "out": format!("/nix/store/{h}-hm_x"), "__json": serde_json::json!({ "text": "a = 1\n", "buildCommand": cmd, "destination": "" }).to_string() } },
        });
        let mut t = BTreeMap::new();
        texts_in(&new, &mut t);
        texts_in(&old, &mut t);
        assert_eq!(t.len(), 2, "{t:?}");
        assert_eq!(t[&format!("/nix/store/{h}-start/bin/start")], "#!/bin/sh\necho hi\n");
        assert_eq!(t[&format!("/nix/store/{h}-hm_x")], "a = 1\n");
    }

    #[test]
    fn packages_and_store_paths() {
        let catalog = Catalog::new(["ripgrep", "zsh-autocomplete", "vscode-fhs", "hello"]);
        let mut c = Conv::new(Some(&catalog), Tier::System);
        // A package by its name, else by what the catalog calls its pname.
        assert!(matches!(conv_val(&mut c, r#"{"__pkg":{"name":"ripgrep-15.0.0","pname":"ripgrep","out":"/nix/store/aaaa"}}"#, &[]), Val::Pkg(p) if p == "ripgrep"));
        assert!(matches!(conv_val(&mut c, r#"{"__pkg":{"name":"lolcat-1.0","pname":"","out":""}}"#, &[]), Val::Pkg(p) if p == "lolcat"), "not in the catalog: by name, for the catalog check to list");
        // Shell code: share files through nami_share, commands by name.
        let h = "0".repeat(32);
        let s = format!(r#""source /nix/store/{h}-zsh-autocomplete-25.03.19/share/zsh-autocomplete/zsh-autocomplete.plugin.zsh\n/nix/store/{h}-hello-2.12/bin/hello""#);
        match conv_val(&mut c, &s, &["programs", "zsh", "initContent"]) {
            Val::Str(t, true) => assert_eq!(t, "source \"$(nami_share zsh-autocomplete/zsh-autocomplete.plugin.zsh)\"\nhello"),
            other => panic!("{}", matches!(other, Val::Str(..))),
        }
        // Anywhere else: the same file in the profile.
        let s = format!(r#""spawn /nix/store/{h}-hello-2.12/share/doc/x; /nix/store/{h}-ripgrep-15.0.0""#);
        match conv_val(&mut c, &s, &["xdg", "configFile", "a", "text"]) {
            // (By the place every computer has: a file goes from one to another.)
            Val::Str(t, true) => assert_eq!(t, "spawn /nix/var/nami/profiles/default/share/doc/x; /nix/var/nami/profiles/default"),
            other => panic!("{}", matches!(other, Val::Str(..))),
        }
        assert_eq!(c.used, ["zsh-autocomplete", "hello", "ripgrep"]);
        // Something only Nix makes: kept, and not exact.
        let s = format!(r#""/nix/store/{h}-hm_zshrc""#);
        assert!(matches!(conv_val(&mut c, &s, &[]), Val::Str(_, false)));
        assert!(matches!(conv_val(&mut c, r#"{"__unknown":"a function"}"#, &[]), Val::Unknown(_)));
        assert!(matches!(conv_val(&mut c, r#"{"__path":"/home/x/y"}"#, &[]), Val::Path(_)));
    }
}
