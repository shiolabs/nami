//! The catalog and builds: `nami update`, `catalog`, `keygen`, `push`, `build` and
//! `recipe`, and Nix from the catalog when a flake needs it.

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::build::Builder;
use crate::index::Index;
use crate::profile::Profiles;
use crate::store::Store;
use crate::{catalog, daemon, import, index, push, recipe, store, style, tell};

/// Without Nix on this computer, Nix comes from the catalog when it's
/// needed (fetched like any package, through the daemon).
pub(crate) fn nix_from_catalog(root: &std::path::Path, why: &'static str) {
    let r = root.to_path_buf();
    import::set_nix_fetch(move || {
        let get = || -> Result<(PathBuf, Option<PathBuf>)> {
            let store = Store::open(&r)?;
            let index = Index::load(&store)?;
            let out = index.get("nix")?.outputs.get("out").cloned().flatten().context("the catalog's nix has no output")?;
            // A computer without certificates (a bare container): the
            // catalog's, for Nix's downloads.
            let certs = if crate::runtime::ca_bundle().is_none() { index.get("cacert")?.outputs.get("out").cloned().flatten() } else { None };
            let want: Vec<_> = std::iter::once(out.clone()).chain(certs.clone()).filter(|p| !store.is_valid(store::base_name(p))).collect();
            if !want.is_empty() {
                crate::say!("this computer has no Nix: fetching it {why} (once)");
                store.realise(&want)?;
            }
            Ok((store.phys(&out).join("bin/nix"), certs.map(|c| store.phys(&c).join("etc/ssl/certs/ca-bundle.crt"))))
        };
        get().map_err(|e| format!("{e:#}"))
    });
}

/// `nami update --refresh`.
pub(crate) fn refresh(store: &Store) -> Result<()> {
    let n = match store.daemon() {
        Some(socket) => daemon::client::refresh_index(socket)?,
        None => Index::refresh(&store)?,
    };
    println!("{}", style::out().line(&format!("✓ Catalog refreshed · {}", tell::catalog_size(n, installable(store)))));
    // A Mac's apps: Homebrew's list of casks, fetched again (it's kept a day otherwise).
    if crate::index::MAC {
        match crate::casks::refresh() {
            Ok(n) => println!("{}", style::out().line(&format!("✓ Homebrew's casks refreshed · {} casks", tell::thousands(n)))),
            Err(e) => eprintln!("Homebrew's casks weren't refreshed: {e:#}"),
        }
    }
    Ok(())
}

/// `nami update --from`.
pub(crate) fn update(store: &Store, profiles: &Profiles, from: Option<String>, drvs: Option<String>, icons: Option<String>, trust: Option<String>, unsigned: bool) -> Result<()> {
    anyhow::ensure!(profiles.is_system(), "setting the package index needs root (sudo nami update --from ...); users can run `nami update --refresh`");
    let from = from.context("--from is required")?;
    if let Some(key) = trust {
        catalog::trust(&store, &key)?;
        let name = key.split(':').next().unwrap_or(&key);
        println!("trusting catalogs signed by {}", catalog::signer_words(name, &[key.clone()]));
    }
    if let Some(icons) = icons {
        let icons = if icons.contains("://") { icons } else { std::fs::canonicalize(&icons)?.display().to_string() };
        std::fs::write(store.var_dir().join("icons-source"), icons)?;
    }
    let n = Index::update(&store, &from, unsigned)?;
    if let Some(drvs) = drvs {
        let drvs = if drvs.contains("://") { drvs } else { std::fs::canonicalize(&drvs)?.display().to_string() };
        std::fs::write(store.var_dir().join("drvs-source"), drvs)?;
        let _ = std::fs::remove_dir_all(store.var_dir().join("drvs")); // cached definitions are per index
    }
    println!("{}", style::out().line(&format!("✓ Catalog updated · {}", tell::catalog_size(n, installable(store)))));
    Ok(())
}

/// How many of the catalog's packages this computer can install, as its compact form says.
fn installable(store: &Store) -> Option<usize> {
    Index::load(store).ok().and_then(|i| i.installable())
}

/// `nami build`.
pub(crate) fn build(store: &Store, recipe: PathBuf, keep_failed: bool) -> Result<()> {
    let index = Index::load(&store)?;
    let mut builder = Builder::new(&store, &index);
    builder.keep_failed = keep_failed;
    for path in builder.build(&recipe)?.outputs.values() {
        println!("{path}");
    }
    Ok(())
}

/// `nami recipe from-deb`.
pub(crate) fn recipe_from_deb(store: &Store, what: String, out: Option<PathBuf>, release: String) -> Result<()> {
    let index = Index::load(&store)?;
    let file = recipe::from_deb(&store, &index, &what, out.as_deref(), &release)?;
    let dir = file.parent().map(|d| d.display().to_string()).unwrap_or_default();
    let p = style::out();
    println!("{}", p.line(&format!("✓ Wrote {}", file.display())));
    println!("{}", p.line(&format!("  Check it, then build it: nami build {}", if dir.starts_with('/') || dir.starts_with('.') { dir } else { format!("./{dir}") })));
    Ok(())
}

/// `nami recipe from-url` and `nami recipe new`.
pub(crate) fn recipe_from_source(store: &Store, how: &str, what: String, version: Option<String>, out: Option<PathBuf>) -> Result<()> {
    let index = Index::load(store)?;
    let mut command = format!("nami recipe {how} {what}");
    if let Some(v) = &version {
        command += &format!(" --version {v}");
    }
    let file = recipe::from_source(store, &index, &what, &recipe::Options { version, out, command })?;
    let dir = file.parent().map(|d| d.display().to_string()).unwrap_or_default();
    let p = style::out();
    println!("{}", p.line(&format!("✓ Wrote {}", file.display())));
    println!("{}", p.line(&format!("  Check it, then build it: nami build {}", if dir.starts_with('/') || dir.starts_with('.') { dir } else { format!("./{dir}") })));
    Ok(())
}

/// `nami recipe update`.
pub(crate) fn recipe_update(store: &Store, dir: PathBuf, version: Option<String>, refetch: bool) -> Result<()> {
    let said = recipe::update(store, &dir, version.as_deref(), refetch)?;
    let p = style::out();
    if said.is_empty() {
        println!("{}", p.line(&format!("✓ {} is at its newest release", recipe::shown_dir(&std::fs::canonicalize(&dir).unwrap_or(dir)))));
    }
    for line in said {
        println!("{}", p.line(&format!("✓ {line}")));
    }
    Ok(())
}

/// `nami catalog patch`.
pub(crate) fn patch(old: PathBuf, new: PathBuf, out: PathBuf) -> Result<()> {
    let read = |p: &PathBuf| std::fs::read(p).with_context(|| format!("reading {}", p.display()));
    let (old, new) = (read(&old)?, read(&new)?);
    let patch = index::make_patch(&old, &new)?;
    store::write_atomic(&out, &patch)?;
    println!("{}", style::out().line(&format!("✓ {} · {} · the catalog is {}", out.display(), tell::size(patch.len() as u64), tell::size(new.len() as u64))));
    Ok(())
}

/// `nami catalog cask-icons`: on any computer (a GitHub runner's), from Homebrew's files.
pub(crate) fn cask_icons(casks: PathBuf, installs: PathBuf, top: Option<usize>, also: Vec<String>) -> Result<()> {
    let read = |p: &PathBuf| -> Result<serde_json::Value> { serde_json::from_slice(&std::fs::read(p).with_context(|| format!("reading {}", p.display()))?).with_context(|| format!("reading {}", p.display())) };
    let all = read(&casks)?;
    let all = all.as_array().context("Homebrew's list of casks isn't a list")?;
    let installs = crate::casks::read_installs(&read(&installs)?);
    let top = top.unwrap_or(usize::MAX);
    let set = crate::casks::for_icons(all, &installs, top, &also);
    eprintln!("{} casks: {} of the most installed that nami installs, and {} more", set.len(), set.len().min(top), set.len().saturating_sub(top));
    println!("{}", serde_json::to_string_pretty(&set.iter().map(|(p, n)| crate::casks::icon_job(p, *n)).collect::<Vec<_>>())?);
    Ok(())
}

/// `nami catalog sign`.
pub(crate) fn sign(store: &Store, path: PathBuf, key: Option<String>) -> Result<()> {
    for f in catalog::sign(&store, &path, key.as_deref())? {
        println!("{}", style::out().line(&format!("✓ Signed {}", f.display())));
    }
    Ok(())
}

/// `nami keygen`.
pub(crate) fn keygen(store: &Store, name: String) -> Result<()> {
    let public = push::keygen(&store, &name)?;
    println!("{public}");
    let p = style::err();
    eprintln!("{}", p.line(&format!("✓ Made the signing key {name}")));
    eprintln!("{}", p.dim("  Put this in your nami file on other machines:"));
    eprintln!("    caches:\n      - url: <where you serve the pushed folder>\n        key: {public}");
    Ok(())
}

/// `nami push`.
pub(crate) fn push(store: &Store, profiles: &Profiles, dest: PathBuf, targets: Vec<String>, key: Option<String>) -> Result<()> {
    let roots: Vec<String> = if targets.is_empty() {
        let m = profiles.current()?.context("nothing installed yet")?;
        m.packages.into_iter().chain(m.runtime).flat_map(|e| e.paths).collect()
    } else {
        let index = Index::load(&store)?;
        let builder = Builder::new(&store, &index);
        let mut roots = Vec::new();
        for t in &targets {
            if t.starts_with("/nix/store/") {
                roots.push(t.clone());
            } else {
                roots.extend(builder.build(std::path::Path::new(t))?.outputs.into_values());
            }
        }
        roots
    };
    let n = push::push(&store, &dest, key.as_deref(), &roots)?;
    println!("{}", style::out().line(&format!("✓ Pushed {} · to {}", tell::count(n, "path", "paths"), dest.display())));
    Ok(())
}
