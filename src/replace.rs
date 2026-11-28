//! `replace:` a library in packages without building them again: a fixed
//! libpng in everything that uses libpng, say. Each package that uses it,
//! itself or through another, is copied with the old library's store path
//! written over by the new one's, as NixOS' replaceDependency does. The
//! paths are the same length, so programs stay intact; a replacement must
//! keep its library's sonames and every symbol it had.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};

use crate::build::Builder;
use crate::index::Index;
use crate::store::{base_name, store_path, Store, STORE_DIR};

/// What a nami file replaces: a package (the library) and what replaces
/// it, a recipe or another catalog package; everywhere (`replace:`) or in
/// one package only (`- firefox: { replace: ... }`).
#[derive(Debug, Default, Clone)]
pub struct Plan {
    pub everywhere: BTreeMap<String, String>,
    pub only: BTreeMap<String, BTreeMap<String, String>>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.everywhere.is_empty() && self.only.values().all(BTreeMap::is_empty)
    }

    /// What's replaced in package `attr` (`firefox`, `firefox@150`).
    fn for_package(&self, attr: &str) -> BTreeMap<String, String> {
        let mut all = self.everywhere.clone();
        let own = self.only.iter().filter(|(p, _)| attr == p.as_str() || attr.strip_prefix(p.as_str()).is_some_and(|r| r.starts_with('@')));
        for (_, m) in own {
            all.extend(m.clone());
        }
        all
    }
}

/// Put the plan's replacements into these packages (name, store paths),
/// saying what was replaced where. `base`: the folder recipe paths are from.
pub fn apply(builder: &Builder, index: &Index, base: &Path, plan: &Plan, packages: Vec<(&str, &mut Vec<String>)>) -> Result<()> {
    if plan.is_empty() {
        return Ok(());
    }
    let store = builder.store();
    // Each library's outputs, old and new, by library and what replaces it.
    let mut swaps: BTreeMap<(String, String), Vec<(String, String)>> = BTreeMap::new();
    let wanted: BTreeSet<(&String, &String)> = plan.everywhere.iter().chain(plan.only.values().flatten()).collect();
    for (lib, with) in wanted {
        let old = index.get(lib).with_context(|| format!("replace: {lib}"))?;
        let new: BTreeMap<String, String> = if Builder::is_recipe(with) {
            let dir = base.join(with).canonicalize().with_context(|| format!("replace: {lib}: no recipe at {with}"))?;
            builder.build(&dir)?.outputs
        } else {
            builder.ensure_catalog(with)?;
            index.get(with).with_context(|| format!("replace: {lib}: {with}"))?.outputs.iter().filter_map(|(o, p)| Some((o.clone(), p.clone()?))).collect()
        };
        let pairs: Vec<(String, String)> = old.outputs.iter().filter_map(|(o, p)| Some((p.clone()?, new.get(o)?.clone()))).collect();
        ensure!(!pairs.is_empty(), "replace: {lib}: {with} has none of {lib}'s outputs ({})", old.outputs.keys().cloned().collect::<Vec<_>>().join(", "));
        for (o, n) in &pairs {
            let (o, n) = (&base_name(o)[33..], &base_name(n)[33..]);
            ensure!(o.len() == n.len(), "replace: {lib}: {with} is {n}, and {lib} is {o}: the names must be as long, so the paths can be written over (a recipe that says no name: and a version: as long as the old one's keeps it)");
        }
        // The same one: nothing to do.
        let pairs: Vec<(String, String)> = pairs.into_iter().filter(|(o, n)| o != n).collect();
        swaps.insert((lib.clone(), with.clone()), pairs);
    }

    let mut used_in: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut checked = BTreeSet::new();
    for (attr, paths) in packages {
        let mine = plan.for_package(attr);
        if mine.is_empty() {
            continue;
        }
        store.realise(paths)?;
        let closure = store.closure(paths)?;
        let mut these = BTreeMap::new();
        for (lib, with) in &mine {
            let key = (lib.clone(), with.clone());
            let pairs = &swaps[&key];
            // Only the outputs it uses are fetched and checked.
            let used: Vec<&(String, String)> = pairs.iter().filter(|(o, _)| closure.contains(base_name(o))).collect();
            if !used.is_empty() {
                for (o, n) in &used {
                    if checked.insert(n.clone()) {
                        store.realise(std::slice::from_ref(n))?;
                        fits(store, lib, with, o, n)?;
                    }
                }
                used_in.entry(key).or_default().push(attr.to_string());
                these.extend(used.into_iter().cloned());
            } else if !pairs.is_empty() && !plan.everywhere.contains_key(lib) {
                crate::say!("replace: {attr} doesn't use {lib}, so there's nothing of it to replace");
            }
        }
        if !these.is_empty() {
            *paths = rewrite(store, paths, &these)?;
        }
    }
    for (key @ (lib, with), pairs) in &swaps {
        match used_in.get(key) {
            Some(pkgs) => crate::say!("{lib} replaced in {} package{}: {}", pkgs.len(), if pkgs.len() == 1 { "" } else { "s" }, pkgs.join(", ")),
            None if pairs.is_empty() => crate::say!("replace: {lib}: {with} is the same {lib}, so there's nothing to replace"),
            None if plan.everywhere.get(lib) == Some(with) => crate::say!("replace: none of the packages uses {lib}, so there's nothing of it to replace"),
            None => {}
        }
    }
    Ok(())
}

/// Whether `new` can stand in for `old` (library `lib`): every library of
/// the old one there under the same name, with every symbol it had.
fn fits(store: &Store, lib: &str, with: &str, old: &str, new: &str) -> Result<()> {
    let libraries = |path: &str| -> BTreeMap<String, BTreeSet<String>> {
        let mut out = BTreeMap::new();
        for dir in ["lib", "lib64"] {
            for e in fs::read_dir(store.phys(&format!("{path}/{dir}"))).into_iter().flatten().flatten() {
                let is_file = e.file_type().is_ok_and(|t| t.is_file());
                if !is_file || !e.file_name().to_string_lossy().contains(".so") {
                    continue;
                }
                let Ok(data) = fs::read(e.path()) else { continue };
                let Some(soname) = crate::elf::linking(&data).and_then(|l| l.soname) else { continue };
                out.insert(soname, crate::elf::exported(&data).unwrap_or_default());
            }
        }
        out
    };
    let (before, after) = (libraries(old), libraries(new));
    for (soname, symbols) in &before {
        let Some(now) = after.get(soname) else {
            let has = if after.is_empty() { "none".to_string() } else { after.keys().cloned().collect::<Vec<_>>().join(", ") };
            bail!("replace: {lib}: {with} has no {soname} (it has {has}), so what uses {lib} couldn't start");
        };
        let missing: Vec<&String> = symbols.difference(now).collect();
        if !missing.is_empty() {
            let shown: Vec<&str> = missing.iter().take(5).map(|s| s.as_str()).collect();
            let more = if missing.len() > 5 { format!(" and {} more", missing.len() - 5) } else { String::new() };
            bail!("replace: {lib}: {with}'s {soname} lacks {}{more}, which what uses {lib} may call: it would need building again, not replacing", shown.join(", "));
        }
    }
    Ok(())
}

/// `roots` with the store paths in `swaps` (old -> new) replaced, all the
/// way up: each path that uses an old one, itself or through another, is
/// copied with the old one's path written over. Returns the new roots.
pub fn rewrite(store: &Store, roots: &[String], swaps: &BTreeMap<String, String>) -> Result<Vec<String>> {
    if let Some(socket) = store.daemon() {
        return crate::daemon::client::replace(socket, roots, swaps);
    }
    store.realise(&roots.iter().chain(swaps.values()).cloned().collect::<Vec<_>>())?;
    // A replacement with another name is copied under the old one's first,
    // so only hashes change in what uses it.
    let mut to: HashMap<String, String> = HashMap::new();
    for (old, new) in swaps {
        let (ob, nb) = (base_name(old), base_name(new));
        ensure!(ob.len() == nb.len() && ob.len() > 33, "{new} can't replace {old}: the paths aren't as long");
        let target = if ob[33..] == nb[33..] {
            nb.to_string()
        } else {
            let alias = base_name(&store_path(&format!("tray-replace-as:{nb}:{}", &ob[33..]), &ob[33..])).to_string();
            if !store.is_valid(&alias) {
                let refs = store.path_info(nb)?.references.into_iter().map(|r| if r == nb { alias.clone() } else { r }).collect();
                commit(store, nb, &alias, &[(nb.to_string(), alias.clone())], refs)?;
            }
            alias
        };
        to.insert(ob.to_string(), target);
    }
    let mut memo: HashMap<String, Option<String>> = HashMap::new();
    let mut out = Vec::new();
    for root in roots {
        out.push(match visit(store, base_name(root), &to, &mut memo)? {
            Some(b) => format!("{STORE_DIR}/{b}"),
            None => root.clone(),
        });
    }
    Ok(out)
}

/// The new path for `base`, if anything it uses is replaced (None: it stays).
fn visit(store: &Store, base: &str, to: &HashMap<String, String>, memo: &mut HashMap<String, Option<String>>) -> Result<Option<String>> {
    if let Some(t) = to.get(base) {
        return Ok(Some(t.clone()));
    }
    if let Some(m) = memo.get(base) {
        return Ok(m.clone());
    }
    let refs = store.path_info(base)?.references;
    let mut changed = Vec::new();
    for r in refs.iter().filter(|r| *r != base) {
        if let Some(n) = visit(store, r, to, memo)? {
            changed.push((r.clone(), n));
        }
    }
    let new = if changed.is_empty() {
        None
    } else {
        let said: Vec<String> = changed.iter().map(|(o, n)| format!("{o}={n}")).collect();
        let name = &base[33..];
        let new = base_name(&store_path(&format!("tray-replace:{base}:{}", said.join(",")), name)).to_string();
        if !store.is_valid(&new) {
            let map: HashMap<&String, &String> = changed.iter().map(|(o, n)| (o, n)).collect();
            let refs = refs.iter().map(|r| if r == base { new.clone() } else { map.get(r).map_or_else(|| r.clone(), |n| (*n).clone()) }).collect();
            changed.push((base.to_string(), new.clone()));
            commit(store, base, &new, &changed, refs)?;
        }
        Some(new)
    };
    memo.insert(base.to_string(), new.clone());
    Ok(new)
}

/// Copy store path `from` to `to`, each old path in `pairs` written over
/// by its new one, and add it to the store with these references.
fn commit(store: &Store, from: &str, to: &str, pairs: &[(String, String)], references: Vec<String>) -> Result<()> {
    let tmp = store.staging_dir()?.join(format!(".nami-replace-{to}"));
    if tmp.symlink_metadata().is_ok() {
        crate::store::remove_tree(&tmp)?;
    }
    let by_hash: HashMap<&[u8], (&[u8], &[u8])> = pairs.iter().map(|(o, n)| (&o.as_bytes()[..32], (o.as_bytes(), n.as_bytes()))).collect();
    copy(&store.phys(&format!("{STORE_DIR}/{from}")), &tmp, &by_hash).with_context(|| format!("copying {from}"))?;
    crate::build::canonicalize(&tmp)?;
    store.commit_path(&tmp, &format!("{STORE_DIR}/{to}"), references)
}

fn copy(from: &Path, to: &Path, pairs: &HashMap<&[u8], (&[u8], &[u8])>) -> Result<()> {
    let meta = from.symlink_metadata()?;
    if meta.file_type().is_symlink() {
        let target = written_over(fs::read_link(from)?.as_os_str().as_encoded_bytes(), pairs);
        // SAFETY: from an OsStr's bytes, with ASCII written over ASCII.
        std::os::unix::fs::symlink(unsafe { std::ffi::OsStr::from_encoded_bytes_unchecked(&target) }, to)?;
    } else if meta.is_dir() {
        fs::create_dir(to)?;
        for e in fs::read_dir(from)? {
            let e = e?;
            copy(&e.path(), &to.join(e.file_name()), pairs)?;
        }
    } else {
        fs::write(to, written_over(&fs::read(from)?, pairs))?;
        fs::set_permissions(to, fs::Permissions::from_mode(meta.permissions().mode() & 0o755 | 0o200))?;
    }
    Ok(())
}

/// `data` with each old path written over by its new one (found by the
/// 32 characters of its hash, as Nix finds references). Where only the
/// hashes differ, a hash alone is written over too.
fn written_over(data: &[u8], pairs: &HashMap<&[u8], (&[u8], &[u8])>) -> Vec<u8> {
    let is_b32 = |b: u8| b.is_ascii_digit() || (b.is_ascii_lowercase() && !b"eotu".contains(&b));
    let mut out = data.to_vec();
    let mut run = 0usize;
    for i in 0..data.len() {
        run = if is_b32(data[i]) { run + 1 } else { 0 };
        if run < 32 {
            continue;
        }
        let at = i + 1 - 32;
        let Some((old, new)) = pairs.get(&data[at..=i]) else { continue };
        if old[32..] == new[32..] {
            out[at..=i].copy_from_slice(&new[..32]);
        } else if data[at..].starts_with(old) {
            out[at..at + new.len()].copy_from_slice(new);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_over_hashes_and_whole_names() {
        let (a, b) = ("a".repeat(32), "b".repeat(32));
        let pairs_same = [(format!("{a}-libpng-1.6"), format!("{b}-libpng-1.6"))];
        let m: HashMap<&[u8], (&[u8], &[u8])> = pairs_same.iter().map(|(o, n)| (&o.as_bytes()[..32], (o.as_bytes(), n.as_bytes()))).collect();
        let text = format!("rpath=/nix/store/{a}-libpng-1.6/lib:{a}");
        assert_eq!(written_over(text.as_bytes(), &m), format!("rpath=/nix/store/{b}-libpng-1.6/lib:{b}").into_bytes());
        let pairs_named = [(format!("{a}-libpng-1.6"), format!("{b}-libpng-1.7"))];
        let m: HashMap<&[u8], (&[u8], &[u8])> = pairs_named.iter().map(|(o, n)| (&o.as_bytes()[..32], (o.as_bytes(), n.as_bytes()))).collect();
        assert_eq!(written_over(format!("/nix/store/{a}-libpng-1.6/x {a}").as_bytes(), &m), format!("/nix/store/{b}-libpng-1.7/x {a}").into_bytes());
    }

    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "macOS lets only root rename a read-only folder (a Mac's store is root's)")]
    fn rewrites_what_uses_it_all_the_way_up() {
        let root = std::env::temp_dir().join(format!("nami-replace-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = Store::open_direct(&root, false).unwrap();
        let add = |hash: char, name: &str, files: &[(&str, String)], refs: Vec<String>| {
            let base = format!("{}-{name}", hash.to_string().repeat(32));
            let dir = store.phys(&format!("{STORE_DIR}/{base}"));
            for (f, text) in files {
                fs::create_dir_all(dir.join(f).parent().unwrap()).unwrap();
                fs::write(dir.join(f), text).unwrap();
            }
            store.register(&format!("{STORE_DIR}/{base}"), refs).unwrap();
            base
        };
        let lib = add('a', "libpng-1.6", &[("lib/x", "png".into())], Vec::new());
        let fixed = add('b', "libpng-1.7", &[("lib/x", format!("png, fixed, me: {STORE_DIR}/{}-libpng-1.7", "b".repeat(32)))], Vec::new());
        let gui = add('c', "gtk-4", &[("lib/y", format!("{STORE_DIR}/{lib}/lib"))], vec![lib.clone()]);
        let app = add('d', "app-1", &[("bin/app", format!("{STORE_DIR}/{gui}/lib:{STORE_DIR}/{}-app-1", "d".repeat(32)))], vec![gui.clone(), format!("{}-app-1", "d".repeat(32))]);
        let other = add('f', "other-1", &[("bin/o", "nothing".into())], Vec::new());
        let swaps = BTreeMap::from([(format!("{STORE_DIR}/{lib}"), format!("{STORE_DIR}/{fixed}"))]);
        let out = rewrite(&store, &[format!("{STORE_DIR}/{app}"), format!("{STORE_DIR}/{other}")], &swaps).unwrap();
        assert_eq!(out[1], format!("{STORE_DIR}/{other}"), "what doesn't use it stays");
        let new_app = base_name(&out[0]).to_string();
        assert!(new_app.ends_with("-app-1") && new_app != app);
        let refs = store.path_info(&new_app).unwrap().references;
        let new_gui = refs.iter().find(|r| r.ends_with("-gtk-4")).unwrap().clone();
        assert!(refs.contains(&new_app), "its reference to itself follows it: {refs:?}");
        let text = fs::read_to_string(store.phys(&format!("{STORE_DIR}/{new_app}/bin/app"))).unwrap();
        assert_eq!(text, format!("{STORE_DIR}/{new_gui}/lib:{STORE_DIR}/{new_app}"));
        // The fixed one, under the old one's name.
        let gui_refs = store.path_info(&new_gui).unwrap().references;
        let alias = &gui_refs[0];
        assert!(alias.ends_with("-libpng-1.6") && alias != &lib, "{gui_refs:?}");
        assert_eq!(fs::read_to_string(store.phys(&format!("{STORE_DIR}/{alias}/lib/x"))).unwrap(), format!("png, fixed, me: {STORE_DIR}/{alias}"));
        assert_eq!(store.path_info(alias).unwrap().references, Vec::<String>::new());
        // Again: the same paths, made once.
        assert_eq!(rewrite(&store, &[format!("{STORE_DIR}/{app}")], &swaps).unwrap()[0], out[0]);
        crate::store::remove_tree(&root).unwrap();
    }

    #[test]
    fn for_package_picks_its_own() {
        let mut plan = Plan::default();
        plan.everywhere.insert("zlib".into(), "./z".into());
        plan.only.insert("firefox".into(), BTreeMap::from([("libpng".into(), "./p".into())]));
        assert_eq!(plan.for_package("firefox@150").len(), 2);
        assert_eq!(plan.for_package("firefoxpwa").len(), 1);
    }
}
