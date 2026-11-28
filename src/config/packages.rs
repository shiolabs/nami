//! `packages:` and `services:` as written (`- firefox: 150`, `- git:` with
//! its setup under it), and the edits that add and take out a package.

use super::*;

/// `- firefox: 150` in a list of packages is `firefox@150`: that package,
/// at that version (the newest 150.x the catalog has had).
pub(crate) fn versioned_packages(value: &mut Value, name: &str) -> anyhow::Result<()> {
    versioned_list(value.get_mut("packages"), name)
}

/// [`versioned_packages`] for any list of packages. The file was read with
/// [`crate::yaml_text`], so a version is the text written (3.10, not 3.1).
pub(crate) fn versioned_list(list: Option<&mut Value>, name: &str) -> anyhow::Result<()> {
    let Some(items) = list.and_then(Value::as_sequence_mut) else { return Ok(()) };
    let bad = || anyhow::anyhow!("{name}: packages: an entry is a name, or `name: version`");
    for item in items.iter_mut() {
        let Some(m) = item.as_mapping() else { continue };
        let (k, v) = m.iter().next().filter(|_| m.len() == 1).ok_or_else(bad)?;
        let pkg = k.as_str().ok_or_else(bad)?;
        let version = match v {
            Value::String(s) => s.clone(),
            // (A value made by nami, not read from a file.)
            Value::Number(n) => n.to_string(),
            _ => anyhow::bail!("{name}: packages: {pkg}: the version is a number like 150 or 3.12.4"),
        };
        *item = Value::String(format!("{pkg}@{version}"));
    }
    Ok(())
}

/// `packages:` as written: a name a line (`- ripgrep`), `- firefox: 150`
/// for a version, and under a name (`- git:`) what's that package's: its files (`~/`
/// or `/` paths), its desktop settings (by schema, `org.gnome.TextEditor`),
/// its `service:`, `login-shell: true`, `version:`. Read here as the list
/// nami works with (`firefox@150`), what's under each going to the file's
/// own `files:`, `settings:`, `services:` and `login-shell:`.
pub fn package_map(v: &mut Value, name: &str) -> anyhow::Result<()> {
    let Some(m) = v.as_mapping_mut() else { return Ok(()) };
    anyhow::ensure!(!m.contains_key("recipes"), "{name}: recipes: is gone: list your own packages by path (`- ./packages/foo`)");
    service_list(m, name)?;
    service_files(m, name)?;
    anyhow::ensure!(!m.contains_key("login-shell"), "{name}: login-shell: goes under its package, `- zsh: {{ login-shell: true }}`");
    anyhow::ensure!(!m.contains_key("replace-in"), "{name}: replace-in: isn't a key: what's replaced in one package goes under it, `- firefox: {{ replace: {{ libpng: ./libpng-fix }} }}`");
    let items = match m.get("packages") {
        None => return Ok(()),
        Some(Value::Sequence(p)) => p.clone(),
        Some(Value::Null) => Vec::new(),
        Some(_) => anyhow::bail!("{name}: packages: a list, `- name` a line (`- firefox: 150` for a version, `- git:` with its setup under it)"),
    };
    let mut list = Vec::new();
    let put = |m: &mut serde_yaml::Mapping, key: &str, k: Value, v: Value, pkg: &str| -> anyhow::Result<()> {
        let at = m.entry(key.into()).or_insert_with(|| Value::Mapping(Default::default()));
        let at = at.as_mapping_mut().ok_or_else(|| anyhow::anyhow!("{name}: {key}: should be a map"))?;
        let shown = crate::secrets::key_text(&k);
        anyhow::ensure!(!at.contains_key(&k), "{name}: {key}: {shown} is given twice (also under packages: {pkg})");
        at.insert(k, v);
        Ok(())
    };
    for item in items {
        let (k, v) = match item {
            Value::String(_) => {
                list.push(item);
                continue;
            }
            Value::Mapping(one) if one.len() == 1 => one.into_iter().next().unwrap(),
            Value::Mapping(two) if two.contains_key("with") => {
                let python = two.keys().filter_map(Value::as_str).find(|k| *k != "with").unwrap_or("python3").to_string();
                anyhow::bail!("{name}: packages: {python}: with: goes under it, with its version: `- {python}: {{ version: 3.13, with: [numpy] }}`")
            }
            _ => anyhow::bail!("{name}: packages: an entry is a name, `name: version`, or a name with its setup under it"),
        };
        let pkg = k.as_str().ok_or_else(|| anyhow::anyhow!("{name}: packages: each is a name"))?.to_string();
        let version = match v {
            Value::Null => None,
            Value::Mapping(own) => {
                let mut version = None;
                let mut with = None;
                for (key, val) in own {
                    let key_s = key.as_str().ok_or_else(|| anyhow::anyhow!("{name}: packages: {pkg}: each key is a name or a path"))?.to_string();
                    match key_s.as_str() {
                        "version" => version = Some(val),
                        // A Python's packages (python_env.rs).
                        "with" => with = Some(val),
                        // A bundle's (`- bundle:github:ada/myapp: { key: 3F2A … }`): the key
                        // trusted for it, read where a version goes (`bundle:…@3F2A …`).
                        "key" if crate::bundle_app::is_entry(&pkg) => version = Some(val),
                        "replace" => put(m, "replace-in", Value::from(pkg.as_str()), val, &pkg)?,
                        "service" | "services" => anyhow::bail!("{name}: packages: {pkg}: services go under services: (`- {pkg}`), not under the package"),
                        "login-shell" => {
                            anyhow::ensure!(val == Value::Bool(true), "{name}: packages: {pkg}: login-shell: true (or leave it out)");
                            anyhow::ensure!(!m.contains_key("login-shell"), "{name}: login-shell: given twice (also under packages: {pkg})");
                            m.insert("login-shell".into(), Value::from(pkg.as_str()));
                        }
                        p if p.starts_with("~/") || p.starts_with('/') => put(m, "files", key, val, &pkg)?,
                        // (A relocatable schema's with its path: `schema:/path/`.)
                        s if s.contains('.') && (!s.contains('/') || s.contains(":/")) => put(m, "settings", key, val, &pkg)?,
                        other => anyhow::bail!("{name}: packages: {pkg}: {other}: under a package go its files (~/... or /etc/...), its settings (by schema, like org.gnome.TextEditor), login-shell:, version:, replace:, or a Python's with:"),
                    }
                }
                if let Some(with) = with {
                    let names: Vec<String> = match with {
                        Value::Sequence(items) => items.iter().map(|i| i.as_str().map(String::from).ok_or_else(|| anyhow::anyhow!("{name}: packages: {pkg}: with: a list of its packages' names, like [numpy, requests]"))).collect::<anyhow::Result<_>>()?,
                        Value::String(one) => vec![one],
                        Value::Null => Vec::new(),
                        _ => anyhow::bail!("{name}: packages: {pkg}: with: a list of its packages' names, like [numpy, requests]"),
                    };
                    let version = match &version {
                        Some(Value::String(v)) => Some(v.clone()),
                        Some(Value::Number(n)) => Some(n.to_string()),
                        Some(_) => anyhow::bail!("{name}: packages: {pkg}: the version is a number like 3.13"),
                        None => None,
                    };
                    if let Some(entry) = crate::python_env::entry(name, &pkg, version.as_deref(), &names)? {
                        list.push(Value::String(entry));
                        continue;
                    }
                }
                version
            }
            other => Some(other),
        };
        list.push(match version {
            None => Value::from(pkg),
            Some(v) => Value::Mapping(serde_yaml::Mapping::from_iter([(Value::from(pkg), v)])),
        });
    }
    m.insert("packages".into(), Value::Sequence(list));
    Ok(())
}

/// `services:` as written, a list like packages: (`- jellyfin`, `- backup:`
/// with its setup under it), read as name -> setup (`true` when there's none).
fn service_list(m: &mut serde_yaml::Mapping, name: &str) -> anyhow::Result<()> {
    let items = match m.get("services") {
        None => return Ok(()),
        Some(Value::Sequence(items)) => items.clone(),
        Some(Value::Null) => Vec::new(),
        Some(_) => anyhow::bail!("{name}: services: a list, `- name` a line (`- backup:` with its setup under it)"),
    };
    let mut out = serde_yaml::Mapping::new();
    for item in items {
        let (k, v) = match item {
            Value::String(s) => (Value::String(s), Value::Bool(true)),
            Value::Mapping(one) if one.len() == 1 => {
                let (k, v) = one.into_iter().next().unwrap();
                match v {
                    Value::Null => (k, Value::Bool(true)),
                    Value::Mapping(setup) => (k, Value::Mapping(setup)),
                    other => anyhow::bail!("{name}: services: {}: its setup is a map under it (`run:`, `env:`, its files...), not {}", crate::secrets::key_text(&k), show_value(&other)),
                }
            }
            _ => anyhow::bail!("{name}: services: an entry is a name, or a name with its setup under it"),
        };
        let shown = crate::secrets::key_text(&k);
        anyhow::ensure!(k.is_string(), "{name}: services: each is a name");
        anyhow::ensure!(!out.contains_key(&k), "{name}: services: {shown} is given twice");
        anyhow::ensure!(v.get("restart-on").is_none(), "{name}: services: {shown}: its own files go under it, and restart it (no restart-on:)");
        out.insert(k, v);
    }
    m.insert("services".into(), Value::Mapping(out));
    Ok(())
}

/// A service's own files, under its entry (`jellyfin: { /etc/jellyfin/x.xml:
/// ... }`), go to `files:`; the service restarts when nami changes one. An
/// entry with nothing else is on.
fn service_files(m: &mut serde_yaml::Mapping, name: &str) -> anyhow::Result<()> {
    let Some(Value::Mapping(services)) = m.get_mut("services") else { return Ok(()) };
    let mut moved = Vec::new();
    for (svc, entry) in services.iter_mut() {
        let Value::Mapping(e) = entry else { continue };
        let paths: Vec<Value> = e.keys().filter(|k| k.as_str().is_some_and(|p| p.starts_with('/') || p.starts_with("~/"))).cloned().collect();
        if paths.is_empty() {
            continue;
        }
        let svc_name = svc.as_str().unwrap_or_default().to_string();
        let mut restart = Vec::new();
        for p in paths {
            let content = e.remove(&p).unwrap();
            if p.as_str().is_some_and(|p| p.starts_with('/')) {
                restart.push(p.clone());
            }
            moved.push((svc_name.clone(), p, content));
        }
        if !restart.is_empty() {
            let at = e.entry("restart-on".into()).or_insert_with(|| Value::Sequence(Vec::new()));
            let list = at.as_sequence_mut().ok_or_else(|| anyhow::anyhow!("{name}: services: {svc_name}: restart-on: a list of paths"))?;
            for p in restart {
                if !list.contains(&p) {
                    list.push(p);
                }
            }
        }
    }
    if moved.is_empty() {
        return Ok(());
    }
    let files = m.entry("files".into()).or_insert_with(|| Value::Mapping(Default::default()));
    let files = files.as_mapping_mut().ok_or_else(|| anyhow::anyhow!("{name}: files: should be a map"))?;
    for (svc, path, content) in moved {
        let shown = crate::secrets::key_text(&path);
        anyhow::ensure!(!files.contains_key(&path), "{name}: files: {shown} is given twice (also under services: {svc})");
        files.insert(path, content);
    }
    Ok(())
}

/// The edit that adds `name` (`firefox@150`) to a file's packages:
/// `- firefox: 150`, in place of another version of it.
pub fn add_package(text: &str, name: &str) -> anyhow::Result<Vec<crate::config_edit::Op>> {
    use crate::config_edit::Op;
    let (plain, version) = crate::index::split_version(name);
    let mut ops = Vec::new();
    for (have, written) in listed_packages(text) {
        if crate::index::split_version(&have).0 != plain {
            continue;
        }
        if have == name {
            return Ok(Vec::new());
        }
        anyhow::ensure!(!written.get(plain).is_some_and(Value::is_mapping), "{plain} has its own setup in the file: change its version: there");
        ops.push(Op::Remove { path: vec!["packages".into()], value: written });
    }
    let item = match version {
        None => Value::from(plain),
        Some(v) => {
            let ver = serde_yaml::from_str::<Value>(v).ok().filter(|n| n.is_number() && n.as_f64().map(|f| f.to_string()) == Some(v.to_string())).unwrap_or_else(|| v.into());
            Value::Mapping(serde_yaml::Mapping::from_iter([(Value::from(plain), ver)]))
        }
    };
    ops.push(Op::Add { path: vec!["packages".into()], value: item });
    Ok(ops)
}

/// The edits that take package `name` (any version) out of a file, with
/// what's under it.
pub fn remove_package(text: &str, name: &str) -> Vec<crate::config_edit::Op> {
    let plain = crate::index::split_version(name).0;
    listed_packages(text)
        .into_iter()
        .filter(|(have, _)| have == name || crate::index::split_version(have).0 == plain)
        .map(|(_, written)| crate::config_edit::Op::Remove { path: vec!["packages".into()], value: written })
        .collect()
}

/// The folder a `packages:` entry of a file in `base` is built from: a path
/// (`./packages/nami-gui`), or a name alone with a folder of its name in the
/// nearest packages/ folder (`nami-gui`), as [`resolve_names`] reads it.
fn package_folder(entry: &str, base: &Path) -> Option<PathBuf> {
    let dir = if crate::build::Builder::is_recipe(entry) {
        base.join(entry)
    } else if is_bare_name(entry) {
        packages_dir(base)?.join(entry)
    } else {
        return None;
    };
    crate::oldnames::is_file(&dir.join("package.nami")).then(|| dir.canonicalize().ok()).flatten()
}

/// Whether the entry `have` of a file in `base` is `name` as written: the
/// same name and version, or the same package folder by its path or its name
/// (`nami install nami-gui` where `./packages/nami-gui` is listed).
pub fn same_package(have: &str, name: &str, base: &Path) -> bool {
    have == name || package_folder(have, base).is_some_and(|f| package_folder(name, base).is_some_and(|g| f == g))
}

/// [`remove_package`] for a file in `base`: its entry at any version, by
/// name or by path (`nami remove nami-gui` takes out `./packages/nami-gui`).
pub fn remove_package_in(text: &str, name: &str, base: &Path) -> Vec<crate::config_edit::Op> {
    let plain = crate::index::split_version(name).0;
    listed_packages(text)
        .into_iter()
        .filter(|(have, _)| crate::index::split_version(have).0 == plain || same_package(have, name, base))
        .map(|(_, written)| crate::config_edit::Op::Remove { path: vec!["packages".into()], value: written })
        .collect()
}

/// A file's own `packages:` entries: how nami reads each (`firefox@150`),
/// and the entry as written (`firefox: 150`), to edit it.
pub fn listed_packages(text: &str) -> Vec<(String, Value)> {
    let value: Value = crate::yaml_text::from_str(text).unwrap_or_default();
    let Some(items) = value.get("packages").and_then(Value::as_sequence) else { return Vec::new() };
    items
        .iter()
        .filter_map(|item| {
            let plain = match item {
                Value::Mapping(m) if m.len() == 1 => {
                    let (k, v) = m.iter().next()?;
                    match v {
                        Value::Mapping(own) => match own.get("version") {
                            Some(ver) => Value::Mapping(serde_yaml::Mapping::from_iter([(k.clone(), ver.clone())])),
                            None => k.clone(),
                        },
                        Value::Null => k.clone(),
                        _ => item.clone(),
                    }
                }
                _ => item.clone(),
            };
            let mut list = Value::Sequence(vec![plain]);
            versioned_list(Some(&mut list), "").ok()?;
            Some((list.as_sequence()?.first()?.as_str()?.to_string(), item.clone()))
        })
        .collect()
}
