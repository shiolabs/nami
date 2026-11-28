//! What both the command line and the GUI do: change a profile.

use std::path::Path;

use anyhow::{bail, ensure, Context, Result};

use crate::build::Builder;
use crate::index::Index;
use crate::profile::{Entry, Profiles};
use crate::store::Store;

/// `github:owner/repo[/dir][@ref]#tool`: a tool another project shares
/// (the one form of git references, gitref.rs).
pub fn is_remote(attr: &str) -> bool {
    attr.starts_with("github:")
}

/// Build a `github:` package: the repo at its ref (or at `rev`, a pinned
/// commit), and the recipe its project.nami `provides` under that name;
/// else, when it has a flake.nix, that flake's package (flake.rs).
/// Returns the entry and the commit it was built from.
pub fn resolve_remote(builder: &Builder, store: &Store, spec: &str, rev: Option<&str>) -> Result<(Entry, String)> {
    let mut r = crate::gitref::parse(spec, crate::gitref::Use::Repo)?;
    ensure!(matches!(r.repo, crate::gitref::Repo::GitHub { .. }), "{spec}: a shared tool is in a GitHub repo, as github:owner/repo[/dir][@ref]#name");
    crate::layers::say_old(&r, spec);
    let tool = r.name.take();
    let tool = tool.as_deref();
    if let Some(rev) = rev {
        r.git_ref = Some(rev.to_string());
    }
    let repo = r.to_string();
    let (dir, sha, label) = crate::project::fetch_github(store, repo.strip_prefix("github:").unwrap_or(&repo))?;
    let provides = crate::project::file_in(&dir).map(|f| crate::project::read(&f)).transpose()?.map(|p| p.provides).unwrap_or_default();
    // No nami recipe for it, but a flake: Nix says how to build it.
    let provided = match tool {
        Some(t) => provides.contains_key(t),
        None => !provides.is_empty(),
    };
    if !provided && dir.join("flake.nix").is_file() {
        let built = crate::flake::build(builder, &dir, tool, &label)?;
        return Ok((Entry { attr: spec.to_string(), name: built.name, paths: built.paths }, sha));
    }
    ensure!(crate::project::file_in(&dir).is_some(), "github:{label} has no {} saying what it provides, and no flake.nix", crate::project::FILE);
    let (name, recipe) = match tool {
        Some(t) => (t, provides.get(t).with_context(|| format!("github:{label} provides {}, not {t:?}", if provides.is_empty() { "nothing".to_string() } else { provides.keys().cloned().collect::<Vec<_>>().join(", ") }))?),
        None if provides.len() == 1 => {
            let (n, r) = provides.iter().next().unwrap();
            (n.as_str(), r)
        }
        None => bail!("github:{label} provides {}: say which one, as github:{label}#name", if provides.is_empty() { "nothing".to_string() } else { provides.keys().cloned().collect::<Vec<_>>().join(", ") }),
    };
    ensure!(Builder::is_recipe(recipe) && !recipe.starts_with('/') && !recipe.split('/').any(|c| c == ".."), "github:{label}: {name} must be a recipe inside the repo (./...)");
    let built = builder.build(&dir.join(recipe))?;
    Ok((Entry { attr: spec.to_string(), name: built.name.clone(), paths: built.install_paths() }, sha))
}

/// A nixpkgs attr via the index, or a recipe path (relative to `base`) by building it.
pub fn resolve(builder: &Builder, index: &Index, base: &Path, attr: &str) -> Result<Entry> {
    if is_remote(attr) {
        return Ok(resolve_remote(builder, builder.store(), attr, None)?.0);
    }
    if Builder::is_recipe(attr) {
        let dir = base.join(attr).canonicalize().with_context(|| format!("no recipe at {attr}"))?;
        let built = builder.build(&dir)?;
        return Ok(Entry { attr: dir.display().to_string(), name: built.name.clone(), paths: built.install_paths() });
    }
    if let Some(spec) = crate::python_env::parse(attr) {
        return crate::python_env::resolve(builder, index, &spec);
    }
    let pkg = index.get(attr)?;
    let paths = pkg.install_paths();
    ensure!(!paths.is_empty(), "{attr} has no installable output in the index");
    builder.ensure_catalog(attr)?; // unfree policy; builds what the cache lacks
    Ok(Entry { attr: attr.to_string(), name: pkg.name.clone(), paths })
}

pub fn install(store: &Store, profiles: &Profiles, index: &Index, attrs: &[String]) -> Result<Option<u64>> {
    let mut builder = Builder::new(store, index);
    builder.allow_unfree = crate::config::unfree_allowed(profiles.managed_config()?.as_deref());
    let mut wanted = profiles.current_packages()?;
    for attr in attrs {
        let entry = resolve(&builder, index, Path::new("."), attr)?;
        wanted.retain(|e| e.attr != entry.attr);
        wanted.push(entry);
    }
    profiles.commit(Some(index), wanted, None, None, None)
}

pub fn remove(profiles: &Profiles, index: &Index, attrs: &[String]) -> Result<Option<u64>> {
    let mut wanted = profiles.current_packages()?;
    for attr in attrs {
        if !wanted.iter().any(|e| &e.attr == attr) {
            bail!("{attr} is not installed");
        }
        wanted.retain(|e| &e.attr != attr);
    }
    profiles.commit(Some(index), wanted, None, None, None)
}

/// Installed packages the current index has a newer build of, and on a Mac the apps from
/// Homebrew's casks it has a newer version of (not those that update themselves): what `nami
/// upgrade` moves. Recipes aren't checked here.
pub fn updates(profiles: &Profiles, index: &Index) -> Result<Vec<crate::query::Update>> {
    let installed = profiles.current_packages()?;
    let mut out = Vec::new();
    for e in &installed {
        if Builder::is_recipe(&e.attr) || crate::casks::is_cask_entry(e) {
            continue;
        }
        if let Ok(pkg) = index.get(&e.attr) {
            if pkg.install_paths() != e.paths {
                out.push(crate::query::Update { attr: e.attr.clone(), from: crate::query::version_of(&e.name), to: crate::query::version_of(&pkg.name) });
            }
        }
    }
    out.extend(crate::casks::updates(&installed).into_iter().map(|(attr, from, to)| crate::query::Update { attr, from, to }));
    Ok(out)
}

/// Re-resolve everything installed against the current index (and rebuild
/// recipes, which picks up their changes).
pub fn upgrade(store: &Store, profiles: &Profiles, index: &Index) -> Result<Option<u64>> {
    let mut builder = Builder::new(store, index);
    builder.allow_unfree = crate::config::unfree_allowed(profiles.managed_config()?.as_deref());
    let wanted = profiles
        .current_packages()?
        .iter()
        // (Flathub's apps are Flatpak's to update; bundles update themselves; Homebrew's casks
        // are only ever in a file, and move as it's applied with upgrade: apply.rs, casks.rs.)
        .map(|e| if crate::flathub::is_flathub(&e.attr) || crate::bundle_app::is_entry(&e.attr) || crate::casks::is_cask_entry(e) { Ok(e.clone()) } else { resolve(&builder, index, Path::new("/"), &e.attr) })
        .collect::<Result<Vec<_>>>()?;
    profiles.commit(Some(index), wanted, None, None, None)
}

/// The change `nami undo` goes back to from `current`: the one it was made from (after an
/// undo, the next change is made from the one undone to, and undoing it goes back there, not
/// forward to what was undone). Where it doesn't say (made before nami kept it) or that one
/// was cleaned up, the one numbered before it. None: there's none before it.
pub fn undo_target(profiles: &Profiles, current: u64) -> Result<Option<u64>> {
    let generations = profiles.generations()?;
    if let Some(from) = profiles.manifest(current)?.from.filter(|f| generations.contains(f)) {
        return Ok(Some(from));
    }
    Ok(generations.iter().filter(|&&g| g < current).last().copied())
}

/// Switch to `generation`, or the one the current was made from. Returns (from, to).
pub fn rollback(profiles: &Profiles, generation: Option<u64>) -> Result<(u64, u64)> {
    // Nothing of yours to go back to: the system's changes are sudo's.
    let none_of_yours = |what: String| -> anyhow::Error {
        if profiles.is_system() {
            anyhow::anyhow!("{what}")
        } else {
            crate::errors::coded_hint(crate::errors::Code::NotFound, what, "To undo the computer's last change: sudo nami undo")
        }
    };
    let Some(current) = profiles.current_generation() else {
        return Err(none_of_yours(if profiles.is_system() { "nothing installed yet".into() } else { "nothing to undo: you haven't changed anything yet".into() }));
    };
    if generation.is_none() && profiles.manifest(current)?.start {
        return Err(none_of_yours(format!("nothing to undo: this is before your first change (`nami undo {}` goes forward again)", current - 1)));
    }
    let target = match generation {
        Some(g) => g,
        None => match undo_target(profiles, current)? {
            Some(g) => g,
            None if current > 1 => anyhow::bail!("nothing to undo: the changes before {current} were cleaned up (`cleanup:` in your nami file says when old changes go)"),
            // Your very first change: back to before it, as a change of its own (so
            // `nami undo 1` brings it back).
            None if !profiles.is_system() => return Ok((current, profiles.back_to_the_start()?)),
            None => anyhow::bail!("nothing to undo: this is the first change"),
        },
    };
    profiles.switch(target)?;
    Ok((current, target))
}
