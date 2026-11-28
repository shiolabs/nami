//! `nami recipe update`: a recipe moved to its project's newest release
//! (or `--version`), its source fetched, and every hash it records made
//! again: the source's sha256 (in package.nami if it's written there,
//! else in nami.lock), and its git dependencies' (a Rust project's).
//! `--refetch` fetches them again at the same version, for a file that
//! changed upstream when the change is known to be right.
//!
//! The newest release is found where the source is (upstream.rs): a
//! forge's latest release or newest tag, or the newest file in the
//! folder a plain tarball is in. package.nami is edited as text, so its
//! comments and layout stay.

use std::path::Path;

use anyhow::{anyhow, bail, ensure, Context, Result};

use super::upstream::{self, Upstream};
use crate::build::{Recipe, Sources};
use crate::lock::Pins;
use crate::store::Store;

/// What it changed, a line per field (none: it's at the newest already).
pub fn update(store: &Store, dir: &Path, version: Option<&str>, refetch: bool) -> Result<Vec<String>> {
    let file = crate::oldnames::find_in(dir, crate::build::RECIPE_FILE);
    let text = std::fs::read_to_string(&file).with_context(|| format!("no recipe at {}", file.display()))?;
    let (build, _) = crate::build::split_package(&text).with_context(|| format!("in {}", file.display()))?;
    let recipe: Recipe = serde_yaml::from_value(serde_yaml::Value::Mapping(build.ok_or_else(|| anyhow!("{}: nothing to build, so nothing to update", file.display()))?)).with_context(|| format!("in {}", file.display()))?;
    let source = match &recipe.source {
        Some(Sources::One(s)) if s.url.is_some() && !s.is_deb() => s,
        Some(Sources::One(s)) if s.is_deb() => bail!("{}: a Debian package's recipe: write it again with `nami recipe from-deb`", file.display()),
        _ => bail!("{}: `nami recipe update` moves a recipe whose source is one URL", file.display()),
    };
    let old_url = source.url.clone().unwrap_or_default();
    let old_version = recipe.version.clone();
    let name = if recipe.name.is_empty() { dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() } else { recipe.name.clone() };

    // Where the new one is.
    let (new_version, new_url) = match upstream::parse(&old_url) {
        Upstream::Repo { repo, tag: Some(old_tag), file } => {
            let new_tag = match version {
                None if refetch => old_tag.clone(),
                wanted => repo.pick_tag(store, wanted)?,
            };
            let v = upstream::tag_version(&new_tag, &repo.name()).or_else(|| upstream::tag_version(&new_tag, &name)).unwrap_or_else(|| new_tag.clone());
            let url = if new_tag == old_tag {
                old_url.clone()
            } else if file.is_some() && old_url != repo.tag_archive(&old_tag) {
                upstream::moved_url(&old_url, &old_tag, &new_tag, &old_version, &v)?
            } else {
                repo.tag_archive(&new_tag)
            };
            (v, url)
        }
        Upstream::Repo { tag: None, .. } => bail!("{old_url} names no release, so nami can't tell which one the recipe is at"),
        Upstream::File(url) => match version {
            None if refetch => (old_version.clone(), url),
            Some(v) => (v.to_string(), upstream::moved_url(&url, &old_version, v, &old_version, v)?),
            None => match upstream::newest_in_folder(store, &url, &old_version)? {
                Some((v, u)) if super::version_cmp(&v, &old_version).is_gt() => (v, u),
                _ => (old_version.clone(), url),
            },
        },
    };
    if new_url == old_url && new_version == old_version && !refetch {
        return Ok(Vec::new());
    }

    // The new source, and its sha256.
    let file_name = source.name.clone().unwrap_or_else(|| crate::build::url_file_name(&new_url).to_string());
    let (path, sha256) = match crate::build::fetch_url(store, &new_url, &file_name, None)? {
        crate::build::Fetched::Ok { path, sha256 } => (path, super::hex(&sha256)),
        crate::build::Fetched::Mismatch { .. } => unreachable!("nothing asked for"),
    };
    let mut said = Vec::new();
    let mut new_text = text.clone();
    if new_version != old_version {
        let op = crate::config_edit::Op::Set { path: vec!["version".into()], value: serde_yaml::Value::String(new_version.clone()) };
        new_text = crate::config_edit::apply(&new_text, &op).or_else(|_| replace_once(&new_text, &format!("version: {old_version}"), &format!("version: {new_version}")))?;
        said.push(format!("version: {old_version} → {new_version}"));
    }
    if new_url != old_url {
        new_text = replace_once(&new_text, &old_url, &new_url)?;
        said.push(format!("source.url: {old_url} → {new_url}"));
    }
    let mut pins = Pins::load(dir)?;
    match &source.sha256 {
        Some(old) => {
            if *old != sha256 {
                new_text = replace_once(&new_text, old, &sha256)?;
                said.push(format!("source.sha256: {} → {}", short(old), short(&sha256)));
            }
        }
        None => {
            let old = pins.remove(&old_url);
            pins.set(&new_url, &sha256);
            if old.as_deref() != Some(&sha256) {
                said.push(format!("sha256 (nami.lock): {} → {}", old.as_deref().map(short).unwrap_or_else(|| "none".into()), short(&sha256)));
            }
        }
    }
    // Its git dependencies (a Rust project's): fetched again where the new
    // source names other commits, or all of them with --refetch.
    let tree = crate::archive::peek(&store.phys(&path), &|p| p == "Cargo.lock")?;
    let wanted: Vec<String> = match tree.text("Cargo.lock") {
        Some(lock) => crate::vendor::cargo_lock(lock)?
            .iter()
            .filter_map(|c| match &c.from {
                crate::vendor::CrateSource::Git { url, commit, .. } => crate::vendor::git_tarball(url, commit).ok(),
                _ => None,
            })
            .collect(),
        None => Vec::new(),
    };
    for url in pins.urls() {
        if url != new_url && !wanted.contains(&url) {
            pins.remove(&url);
            if url != old_url {
                said.push(format!("nami.lock: {url} (not used any more) taken out"));
            }
        }
    }
    {
        let pins_lock = std::sync::Mutex::new(&mut pins);
        for url in wanted.iter() {
            let before = pins_lock.lock().unwrap().get(url).map(String::from);
            if before.is_some() && !refetch {
                continue;
            }
            if refetch {
                pins_lock.lock().unwrap().remove(url);
            }
            crate::vendor::pinned(store, url, &pins_lock)?;
            let after = pins_lock.lock().unwrap().get(url).map(String::from);
            if after != before {
                said.push(format!("sha256 of {url} (nami.lock): {} → {}", before.as_deref().map(short).unwrap_or_else(|| "none".into()), after.as_deref().map(short).unwrap_or_default()));
            }
        }
    }
    // The file as it'll be read: the same recipe, at the new place.
    let (build, _) = crate::build::split_package(&new_text)?;
    let check: Recipe = serde_yaml::from_value(serde_yaml::Value::Mapping(build.unwrap_or_default()))?;
    ensure!(check.version == new_version, "couldn't change the version in {} safely: change it by hand", file.display());
    if new_text != text {
        std::fs::write(&file, &new_text).with_context(|| format!("writing {}", file.display()))?;
    }
    pins.save()?;
    if said.is_empty() {
        said.push(format!("{new_url}: the same file as before (sha256 {})", short(&sha256)));
    }
    Ok(said)
}

/// `text` with `old` (there once, at least) as `new`, everywhere.
fn replace_once(text: &str, old: &str, new: &str) -> Result<String> {
    ensure!(text.contains(old), "couldn't find {old:?} in package.nami to change it");
    Ok(text.replace(old, new))
}

/// A hash, as a line says it: its first 12 characters.
fn short(h: &str) -> String {
    if h.len() > 14 { format!("{}…", &h[..12]) } else { h.to_string() }
}
