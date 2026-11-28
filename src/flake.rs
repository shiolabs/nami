//! A tool from a repo's flake.nix (`github:owner/repo#name`, when the repo
//! has no nami recipe for it). Nix only works out how to build it, its
//! derivations, as the catalog's index does for nixpkgs; nami then builds
//! what the binary cache doesn't have, in its own sandbox, at the paths Nix
//! gives them. The Nix is the computer's, or the one `nami import` fetches
//! from the catalog (its own store, in ~/.cache/nami/import-nix).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;

use crate::build::{Builder, Closure};
use crate::import::nixtool;
use crate::store::{base_name, STORE_DIR};

/// What a flake's package installs: its name, and its paths (`bin`, else `out`).
pub struct Built {
    pub name: String,
    pub paths: Vec<String>,
}

/// The flake's packages for this computer: `packages.x86_64-linux`, `packages.aarch64-linux`.
const SYSTEM: &str = crate::index::SYSTEM;

/// Build `packages.<system>.<name>` (or `default`) of the flake in `dir`,
/// the repo `label` at a commit.
pub fn build(builder: &Builder, dir: &Path, name: Option<&str>, label: &str) -> Result<Built> {
    // A person's own apps go into the store through the daemon, which can't
    // check that a derivation's paths are its own: an admin's can.
    ensure!(
        builder.store().daemon().is_none(),
        "github:{label} comes from its flake.nix, and only an administrator can install from a flake, for everyone (in system.nami, or `nami install --system`): a person's own apps come from nami recipes"
    );
    let nix = nixtool::get().with_context(|| format!("github:{label} comes from its flake.nix, which takes Nix to read, and nami couldn't get it"))?;
    let attr = format!("packages.{SYSTEM}.{}", name.unwrap_or("default"));
    let flake = format!("path:{}#{attr}", dir.display());
    crate::say!("github:{label} has no nami recipe: building its flake ({attr})");

    let run = |args: &[&str]| -> Result<String> {
        let out = nix.command().args(args).arg("--no-write-lock-file").output().context("running Nix")?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let last: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty() && !l.starts_with("warning:") && !l.starts_with("•") && !l.starts_with("    '")).collect();
            bail!("github:{label}: Nix couldn't read its flake:\n{}", last[last.len().saturating_sub(6)..].join("\n"));
        }
        Ok(String::from_utf8(out.stdout)?)
    };
    // What it has, to say so when the name isn't one of them.
    let has: Vec<String> = serde_json::from_str(&run(&["eval", "--json", &format!("path:{}#packages.{SYSTEM}", dir.display()), "--apply", "builtins.attrNames"])?).unwrap_or_default();
    let want = name.unwrap_or("default");
    ensure!(has.iter().any(|h| h == want), "github:{label}'s flake has {} for {SYSTEM}, not {want:?}{}", if has.is_empty() { "no packages".to_string() } else { has.join(", ") }, if name.is_none() { format!(": say which, as github:{label}#name") } else { String::new() });
    let top = run(&["eval", "--raw", &format!("{flake}.drvPath")])?;
    let top = base_name(top.trim()).to_string();
    let shown: Value = serde_json::from_str(&run(&["derivation", "show", "--recursive", &format!("{STORE_DIR}/{top}")])?)?;
    let shown = shown.get("derivations").cloned().unwrap_or(shown);
    let all: BTreeMap<String, Value> = shown.as_object().into_iter().flatten().map(|(k, v)| (base_name(k).to_string(), v.clone())).collect();

    let closure = closure(builder, &all, &top)?;
    let outputs = crate::build::drv_outputs(&all[&top])?;
    builder.build_closure(&closure, &format!("github:{label}"))?;
    let o = if outputs.contains_key("bin") { "bin" } else { "out" };
    let paths: Vec<String> = outputs.get(o).or_else(|| outputs.values().next()).into_iter().cloned().collect();
    let name = all[&top]["name"].as_str().map(|n| n.rsplit_once('-').filter(|(_, v)| v.starts_with(|c: char| c.is_ascii_digit())).map_or(n, |(n, _)| n)).unwrap_or("flake").to_string();
    Ok(Built { name, paths })
}

/// What has to be built, as the catalog's build closures say it: the walk
/// stops at derivations whose outputs the binary cache has (closures.py).
fn closure(builder: &Builder, all: &BTreeMap<String, Value>, top: &str) -> Result<Closure> {
    let store = builder.store();
    let mut c = Closure { top: top.to_string(), ..Default::default() };
    let mut todo = vec![top.to_string()];
    while let Some(d) = todo.pop() {
        if c.drvs.contains_key(&d) {
            continue;
        }
        let drv = all.get(&d).with_context(|| format!("Nix didn't show {d}"))?;
        for s in drv["inputs"]["srcs"].as_array().into_iter().flatten().filter_map(Value::as_str).map(base_name) {
            if !c.srcs.contains_key(s) && !store.is_valid(s) && !store.in_a_cache(s)? {
                // A source of the flake's own (its tree, a patch): from Nix's store.
                let mut nar = Vec::new();
                crate::nar::dump(&nixtool::real(Path::new(&format!("{STORE_DIR}/{s}"))), &mut nar)?;
                use base64::Engine;
                c.srcs.insert(s.to_string(), base64::engine::general_purpose::STANDARD.encode(nar));
            }
        }
        for (dep, uses) in drv["inputs"]["drvs"].as_object().into_iter().flatten() {
            let dep = base_name(dep).to_string();
            if c.drvs.contains_key(&dep) || c.cached.contains_key(&dep) {
                continue;
            }
            let outs = crate::build::drv_outputs(all.get(&dep).with_context(|| format!("Nix didn't show {dep}"))?)?;
            let wanted: Vec<&String> = uses["outputs"].as_array().into_iter().flatten().filter_map(Value::as_str).filter_map(|o| outs.get(o)).collect();
            let mut cached = true;
            for p in &wanted {
                if !store.is_valid(base_name(p)) && !store.in_a_cache(base_name(p))? {
                    cached = false;
                    break;
                }
            }
            if cached {
                c.cached.insert(dep, outs.into_iter().map(|(o, p)| (o, base_name(&p).to_string())).collect());
            } else {
                todo.push(dep);
            }
        }
        c.drvs.insert(d, drv.clone());
    }
    Ok(c)
}
