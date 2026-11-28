//! A file with what it includes: sets next to it and layers from repos,
//! merged under it (lists joined, maps key by key), and the flat view's keys.

use super::*;

/// [`load_with_layers`]'s value, as the file reads (what `nami show`
/// prints), and the file (with its own sets) before nami's package files.
pub(super) fn load_whole(text: &str, name: &str, base: &Path) -> anyhow::Result<(Value, Value, Vec<crate::layers::Used>)> {
    let base = if base.as_os_str().is_empty() { Path::new(".") } else { base };
    let mut layers = crate::layers::Session::new(base);
    let mark = crate::flat::mark();
    let mut value = load_in(text, name, &top_source(name, base), base, 0, &mut layers, None, Kind::Nami, None);
    // Again without the login screens it doesn't use, once it's known
    // which it uses.
    let unused = value.as_ref().ok().filter(|(_, mine)| Scope::of(name, mine) == Scope::System).map(|(v, mine)| unused_login_screens(v, mine)).unwrap_or_default();
    if !unused.is_empty() {
        crate::flat::forget(mark);
        FOLDERS.with(|f| f.borrow_mut().retain(|n, _| !unused.contains(n)));
        UNUSED.with(|u| *u.borrow_mut() = unused.into_iter().collect());
        value = load_in(text, name, &top_source(name, base), base, 0, &mut layers, None, Kind::Nami, None);
        UNUSED.with(|u| u.borrow_mut().clear());
        LISTED_UNUSED.with(|l| l.borrow_mut().clear());
    }
    let used = layers.finish();
    let (value, mine) = value?;
    // Its Wi-Fi networks, as files (see wifi.rs).
    let value = crate::wifi::expand(value, name)?;
    let mine = crate::wifi::expand(mine, name)?;
    let order: Vec<Value> = mine.as_mapping().map(|m| m.keys().cloned().collect()).unwrap_or_default();
    let mut value = complete(value, name, &mine)?;
    // Your file's keys first, in its order; then what its packages add.
    if let Some(m) = value.as_mapping_mut() {
        let mut out = serde_yaml::Mapping::new();
        for k in order.iter().chain(m.keys().cloned().collect::<Vec<_>>().iter()) {
            if let Some(v) = m.get(k).filter(|_| !out.contains_key(k)) {
                out.insert(k.clone(), v.clone());
            }
        }
        *m = out;
    }
    Ok((value, mine, used))
}

/// The whole computer back in the keys files are written with (the reverse
/// of [`read_keys`]), for the flat view: what packages bring nami-side (their
/// checks, a desktop for a login screen) isn't anyone's line, so it's left out.
pub(super) fn written_keys(merged: Value, sources: &[(String, Value)]) -> Value {
    let Value::Mapping(m) = merged else { return merged };
    let written = |path: &[&str]| -> Vec<Value> {
        let mut out = Vec::new();
        for (_, s) in sources {
            let v = path.iter().try_fold(s, |v, k| v.get(*k));
            for x in v.and_then(Value::as_sequence).into_iter().flatten() {
                if !out.contains(x) {
                    out.push(x.clone());
                }
            }
        }
        out
    };
    let mut out = serde_yaml::Mapping::new();
    let under = |out: &mut serde_yaml::Mapping, key: &str, k: Value, v: Value| {
        let e = out.entry(key.into()).or_insert_with(|| Value::Mapping(Default::default()));
        if let Some(e) = e.as_mapping_mut() {
            e.insert(k, v);
        }
    };
    let build_tools = m.get("build-tools").cloned();
    for (k, v) in m {
        match k.as_str().unwrap_or_default() {
            "checks" | "build-tools" => {}
            "environment" => {
                out.insert("env".into(), v);
            }
            "desktop" => {
                let mine = written(&["desktop"]);
                let kept: Vec<Value> = v.as_sequence().into_iter().flatten().filter(|d| mine.contains(d)).cloned().collect();
                if !kept.is_empty() {
                    out.insert(k, Value::Sequence(kept));
                }
            }
            "mask" | "without" => {
                let e = out.entry("without".into()).or_insert_with(|| Value::Sequence(Vec::new()));
                if let Some(e) = e.as_sequence_mut() {
                    e.extend(v.as_sequence().into_iter().flatten().cloned());
                }
            }
            "login" => under(&mut out, "login", "all".into(), v),
            "login-services" => {
                for (p, rules) in v.as_mapping().cloned().unwrap_or_default() {
                    under(&mut out, "login", p, rules);
                }
            }
            "wrappers" => {
                let paths = v.as_sequence().into_iter().flatten().filter_map(Value::as_str).map(|w| Value::from(w.strip_prefix("4755 ").unwrap_or(w))).collect();
                out.insert("setuid".into(), Value::Sequence(paths));
            }
            "early-boot" => under(&mut out, "boot", "microcode".into(), v),
            "kernel" | "initrd-programs" | "initrd-modules" | "kernel-options" => under(&mut out, "boot", k, v),
            "firmware" | "graphics" | "graphics-32bit" | "kernel-modules" => under(&mut out, "drivers", k, v),
            "units" => {
                for (unit, text) in v.as_mapping().cloned().unwrap_or_default() {
                    let unit = unit.as_str().unwrap_or_default();
                    let path = match unit.strip_prefix("user/") {
                        Some(u) => format!("/etc/systemd/user/{u}"),
                        None => format!("/etc/systemd/system/{unit}"),
                    };
                    under(&mut out, "files", path.into(), text);
                }
            }
            "build" => {
                for (path, script) in v.as_mapping().cloned().unwrap_or_default() {
                    let p = path.as_str().unwrap_or_default().to_string();
                    let tools: Vec<Value> = written(&["files", &p, "tools"]).into_iter().filter(|t| build_tools.as_ref().and_then(Value::as_sequence).is_some_and(|b| b.contains(t))).collect();
                    let mut file = serde_yaml::Mapping::new();
                    file.insert("script".into(), script);
                    if !tools.is_empty() {
                        file.insert("tools".into(), Value::Sequence(tools));
                    }
                    under(&mut out, "files", path, Value::Mapping(file));
                }
            }
            _ => match (out.get_mut(&k).and_then(Value::as_mapping_mut), v) {
                (Some(e), Value::Mapping(more)) => e.extend(more),
                (_, v) => {
                    out.insert(k, v);
                }
            },
        }
    }
    // What's replaced in one package goes back under it.
    if let Some(Value::Mapping(only)) = out.remove("replace-in") {
        let list = out.entry("packages".into()).or_insert_with(|| Value::Sequence(Vec::new()));
        if let Some(list) = list.as_sequence_mut() {
            for (pkg, m) in only {
                let own = Value::Mapping(serde_yaml::Mapping::from_iter([("replace".into(), m)]));
                let item = Value::Mapping(serde_yaml::Mapping::from_iter([(pkg.clone(), own)]));
                match list.iter_mut().find(|i| **i == pkg) {
                    Some(at) => *at = item,
                    None => list.push(item),
                }
            }
        }
    }
    Value::Mapping(out)
}

/// A file named `name` in `base`, as a whole path (for `nami show --flat`).
pub(super) fn top_source(name: &str, base: &Path) -> String {
    let file = Path::new(name).file_name().map_or_else(|| name.into(), |f| f.to_os_string());
    absolute(base).join(file).display().to_string()
}

/// A file in a repo's layer: the repo, the file in it, and the commit
/// (github:alice/os/gaming.nami at 1a2b3c4d5e6f).
fn repo_source(layers: &crate::layers::Session, root: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file).display();
    match layers.used.iter().rev().find(|u| u.root == root) {
        Some(u) => format!("{}/{rel} at {}", u.key, &u.pin.rev[..u.pin.rev.len().min(12)]),
        None => file.display().to_string(),
    }
}

/// A package that's a recipe's path (`./tools/lint`), as `at` says from
/// the file it's in: an included file's recipes stay its own.
fn local_paths(value: &mut Value, at: &dyn Fn(&str) -> String) {
    let local = |p: &str| p.starts_with("./") || p.starts_with("../");
    let at = |p: &str| Value::String(at(p));
    for item in value.get_mut("packages").and_then(Value::as_sequence_mut).into_iter().flatten() {
        if let Some(p) = item.as_str().filter(|p| local(p)) {
            *item = at(p);
        }
    }
    // What replaces a library, too.
    let fix = |m: &mut Value| {
        for v in m.as_mapping_mut().into_iter().flat_map(|m| m.values_mut()) {
            if let Some(p) = v.as_str().filter(|p| local(p)) {
                *v = at(p);
            }
        }
    };
    if let Some(r) = value.get_mut("replace") {
        fix(r);
    }
    for r in value.get_mut("replace-in").and_then(Value::as_mapping_mut).into_iter().flat_map(|m| m.values_mut()) {
        fix(r);
    }
    // Its service files too.
    if let Some(m) = value.get_mut("services").and_then(Value::as_mapping_mut) {
        *m = std::mem::take(m).into_iter().map(|(k, v)| match k.as_str().filter(|k| local(k) && crate::up::is_file(k)) {
            Some(k) => (at(k), v),
            None => (k, v),
        }).collect();
    }
}

/// Its files' `source: ./x` (anywhere in it: `files:`, a package's own
/// files), from the folder it's relative to.
fn local_sources(value: &mut Value, at: &dyn Fn(&str) -> String) {
    match value {
        Value::Mapping(m) => {
            for (k, v) in m.iter_mut() {
                match v {
                    Value::String(s) if k.as_str() == Some("source") && !s.starts_with('/') && !s.starts_with('~') => *s = at(s),
                    _ => local_sources(v, at),
                }
            }
        }
        Value::Sequence(l) => l.iter_mut().for_each(|v| local_sources(v, at)),
        Value::Tagged(t) => local_sources(&mut t.value, at),
        _ => {}
    }
}

/// `path` (absolute) from folder `from` (absolute): `./x`, `../../x`.
pub(super) fn relative_to(path: &Path, from: &Path) -> String {
    let (p, f): (Vec<_>, Vec<_>) = (path.components().collect(), from.components().collect());
    let same = p.iter().zip(&f).take_while(|(a, b)| a == b).count();
    let mut out = std::path::PathBuf::new();
    for _ in same..f.len() {
        out.push("..");
    }
    out.extend(&p[same..]);
    let out = out.display().to_string();
    if out.starts_with("..") { out } else { format!("./{out}") }
}

/// `path` with its `.` and `..` worked out (by its text).
pub(super) fn tidy(path: &Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// [`load_value`], with the layers from repos it includes pinned in
/// `layers`; `repo` is the top of the fetched repo the file is in, if
/// it's one (its names and paths are the repo's first). `kind`: a system
/// or user file's, or a project's.
/// A file's whole value with what it lists, and "mine": the same without
/// what package folders set up (their services, files, groups...), which
/// is for a computer nami makes; a computer nami didn't make gets mine.
/// `scope`: the including file's (a set is part of it); None for the
/// file itself, by its name and what it says.
#[allow(clippy::too_many_arguments)]
pub(super) fn load_in(text: &str, name: &str, source: &str, base: &Path, depth: usize, layers: &mut crate::layers::Session, repo: Option<&Path>, kind: Kind, scope: Option<Scope>) -> anyhow::Result<(Value, Value)> {
    anyhow::ensure!(depth < 8, "{name}: includes go too deep (does a file include itself?)");
    if kind == Kind::Project && repo.is_some() {
        // A project's secrets are sealed for its own people, in its own file.
        anyhow::ensure!(
            crate::secrets::find(text).map_or(true, |f| f.is_empty()),
            "{name}: a layer from a repo can't bring secrets into a project (they'd be sealed for its author's people, not this project's): put them in the project's own file, under env:"
        );
    }
    let mut own = parse(text, name, kind)?;
    if own.is_null() {
        own = Value::Mapping(Default::default());
    }
    versioned_packages(&mut own, name)?;
    resolve_names(&mut own, base);
    old_outputs(&mut own, name);
    let scope = scope.unwrap_or_else(|| Scope::of(name, &own));
    if kind != Kind::Project {
        check_keys(&own, name, scope)?;
    }
    // `desktop: niri` is a list of one, as other files' lists are.
    if let Some(d) = own.get_mut("desktop").filter(|d| d.is_string()) {
        *d = Value::Sequence(vec![d.clone()]);
    }
    // Sets of its own in its packages (./work.nami, github:ada/me/user.nami):
    // what they bring goes underneath, as nami's package files do.
    let mut sets: Vec<String> = Vec::new();
    if let Some(list) = own.get_mut("packages").and_then(Value::as_sequence_mut) {
        list.retain(|p| match p.as_str().filter(|p| is_set_file(p)) {
            Some(set) => {
                sets.push(set.to_string());
                false
            }
            None => true,
        });
    }
    // A package folder in the top file's `without:` is left out wherever
    // it's listed, its setup too.
    if depth == 0 {
        let mut names: std::collections::BTreeSet<String> = own.get("without").and_then(Value::as_sequence).into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
        names.extend(UNUSED.with(|u| u.borrow().clone()));
        LEFT_OUT.with(|l| *l.borrow_mut() = names);
    }
    if let Some(list) = own.get_mut("packages").and_then(Value::as_sequence_mut) {
        list.retain(|p| {
            let folder = p.as_str().filter(|p| crate::build::Builder::is_recipe(p) && !is_set_file(p)).and_then(|p| Path::new(p).file_name()).map(|f| f.to_string_lossy().into_owned());
            let out = folder.as_ref().is_some_and(|f| LEFT_OUT.with(|l| l.borrow().contains(f)));
            if folder.is_some_and(|f| UNUSED.with(|u| u.borrow().contains(&f))) {
                LISTED_UNUSED.with(|l| l.borrow_mut().insert(name.to_string()));
            }
            !out
        });
    }
    // Packages of its own (./packages/mytool: a folder with package.nami):
    // their setup goes underneath, as a set's does; one that builds nothing
    // is the catalog's package of its name, set up.
    let mut folders: Vec<(String, std::path::PathBuf, String)> = Vec::new();
    if let Some(list) = own.get_mut("packages").and_then(Value::as_sequence_mut) {
        for p in list.iter_mut() {
            let Some(path) = p.as_str().filter(|p| crate::build::Builder::is_recipe(p) && !is_set_file(p)).map(String::from) else { continue };
            let dir = base.join(&path);
            let Ok(text) = std::fs::read_to_string(crate::oldnames::find_in(&dir, "package.nami")) else {
                anyhow::ensure!(dir.is_file() && crate::oldnames::has_nami_extension(&dir), "{name}: no package at {path} (expected {})", dir.join("package.nami").display());
                continue;
            };
            let (build, setup) = crate::build::split_package(&text).map_err(|e| anyhow::anyhow!("{name}: {path}/package.nami: {e}"))?;
            if build.is_none() {
                let own_name = crate::yaml_text::from_str(&text).ok().and_then(|v| v.get("name")?.as_str().map(String::from));
                let folder = dir.canonicalize().ok().and_then(|d| d.file_name().map(|f| f.to_string_lossy().into_owned()));
                let pkg = own_name.or(folder).ok_or_else(|| anyhow::anyhow!("{name}: {path}: which package? (name:)"))?;
                FOLDERS.with(|f| f.borrow_mut().insert(pkg.clone(), setup.clone()));
                *p = Value::String(pkg);
            } else if let Some(folder) = dir.canonicalize().ok().and_then(|d| d.file_name().map(|f| f.to_string_lossy().into_owned())) {
                // One that builds its program can be a desktop just the same (niri).
                FOLDERS.with(|f| f.borrow_mut().insert(folder, setup.clone()));
            }
            if let Some(k) = setup.keys().filter_map(Value::as_str).find(|k| SINGLE_KEYS.contains(k)) {
                anyhow::bail!("{name}: {path}/package.nami: {k}: a package only adds; {k}: goes in the file that lists it");
            }
            // (Even with nothing to set up: its checks.nami comes along.)
            folders.push((format!("{path}/package.nami"), dir, serde_yaml::to_string(&Value::Mapping(setup))?));
        }
    }
    // How far down each package is listed (0: by the file itself), for what
    // must be one: the nearer one wins.
    if depth == 0 {
        DEPTHS.with(|d| d.borrow_mut().clear());
    }
    for p in own.get("packages").and_then(Value::as_sequence).into_iter().flatten().filter_map(Value::as_str) {
        note_depth(crate::index::split_version(p).0, depth);
    }
    // It builds on one file at most: what must be one comes down one line.
    if sets.len() > 1 {
        // The one to list the others: one of your own, which you can change.
        let mine = sets.iter().position(|s| !crate::layers::is_remote(s)).unwrap_or(0);
        let others: Vec<&str> = sets.iter().enumerate().filter(|(i, _)| *i != mine).map(|(_, s)| s.as_str()).collect();
        anyhow::bail!("{name}: it builds on one file, not {}: list {} in {} instead", sets.join(" and "), others.join(", "), sets[mine]);
    }
    // An included file's paths are from its folder: from the top file's
    // (what `nami show --flat` shows works as one file); a repo's are
    // where it was fetched to.
    if depth == 0 {
        TOP.with(|t| *t.borrow_mut() = absolute(base));
    } else {
        let here = absolute(base);
        let top = TOP.with(|t| t.borrow().clone());
        let at = |p: &str| {
            let full = tidy(&here.join(p));
            if kind == Kind::Project || repo.is_some() { full.display().to_string() } else { relative_to(&full, &top) }
        };
        local_paths(&mut own, &at);
        if kind != Kind::Project {
            local_sources(&mut own, &at);
        }
    }
    let mut merged = Value::Mapping(Default::default());
    // What it lists, side by side: none wins over another; where two say
    // different things, this file says which (checked below).
    let mut siblings: Vec<(String, Value)> = Vec::new();
    for (label, dir, text) in &folders {
        let source = top_source(label, dir);
        let (value, _) = match scope {
            Scope::System => load_in(text, label, &source, dir, depth + 1, layers, repo, kind, Some(Scope::System))?,
            // In a person's file, what a person can have of it.
            Scope::User => {
                let (value, mine) = load_in(text, label, &source, dir, depth + 1, layers, repo, kind, Some(Scope::User))?;
                (personal_part(value, label), mine)
            }
        };
        siblings.push((label.clone(), value.clone()));
        merged = merge(merged, value).map_err(|e| anyhow::anyhow!("{name}: {label}: {e}"))?;
    }
    let mut mine = Value::Mapping(Default::default());
    for set in &sets {
        let (value, theirs) = if crate::layers::is_remote(set) {
            let what = remote_set(set).unwrap_or_else(|| set.clone());
            let (root, file) = layers.fetch(&what).map_err(|e| anyhow::anyhow!("{name}: {set}: {e:#}"))?;
            if let Some(u) = layers.used.last_mut() {
                u.include = set.clone();
            }
            let text = std::fs::read_to_string(&file).map_err(|e| anyhow::anyhow!("{name}: {set}: {e}"))?;
            load_in(&text, set, &repo_source(layers, &root, &file), file.parent().unwrap_or(&root), depth + 1, layers, Some(&root), kind, Some(scope))?
        } else {
            let path = base.join(set);
            if let Some(r) = repo {
                // A fetched set's files are its repo's.
                let inside = path.canonicalize().ok().zip(r.canonicalize().ok()).is_some_and(|(p, r)| p.starts_with(r));
                anyhow::ensure!(!set.starts_with('/') && inside, "{name}: {set}: a set from a repo lists files of that repo, or packages by name");
            }
            let text = std::fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("{name}: {set}: {e}"))?;
            let source = match repo {
                Some(r) => repo_source(layers, r, &path),
                None => top_source(set, path.parent().unwrap_or(base)),
            };
            load_in(&text, set, &source, path.parent().unwrap_or(base), depth + 1, layers, repo, kind, Some(scope))?
        };
        siblings.push((set.clone(), value.clone()));
        merged = merge(merged, value).map_err(|e| anyhow::anyhow!("{name}: {set}: {e}"))?;
        mine = merge(mine, theirs).map_err(|e| anyhow::anyhow!("{name}: {set}: {e}"))?;
    }
    // In the keys nami reads it with (a package's units, its checks...), as
    // what it lists already is; `nami show --flat` shows it as written.
    let read = if kind == Kind::Project { own.clone() } else { read_keys(own.clone(), name, base, scope)? };
    let mut clashes = Vec::new();
    // Two kernels: a package's to say, so one of them goes.
    let mut kernels = false;
    for (i, (a_name, a)) in siblings.iter().enumerate() {
        for (b_name, b) in &siblings[i + 1..] {
            disagree(a, b, &mut Vec::new(), &mut |path, x, y| {
                if lookup_path(&read, path).is_none() {
                    let shown = if path == ["kernel"] { "boot.kernel".to_string() } else { path.join(".") };
                    kernels |= path == ["kernel"];
                    clashes.push(format!("{shown}: {a_name} says {}, {b_name} says {}", show_value(x), show_value(y)));
                }
            });
        }
    }
    let which = if kernels { "list one kernel".to_string() } else { format!("write it in {name}") };
    anyhow::ensure!(clashes.is_empty(), "{name}: what it lists disagrees, so it says which ({which}):\n  {}", clashes.join("\n  "));
    if crate::flat::recording() {
        crate::flat::record(source, &crate::wifi::expand(own.clone(), name).unwrap_or_else(|_| own.clone()));
    }
    let mine = merge(mine, read.clone()).map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
    Ok((merge(merged, read).map_err(|e| anyhow::anyhow!("{name}: {e}"))?, mine))
}

thread_local! {
    /// The top file's folder, while it loads.
    static TOP: std::cell::RefCell<std::path::PathBuf> = Default::default();
    static DEPTHS: std::cell::RefCell<std::collections::HashMap<String, usize>> = Default::default();
}

fn note_depth(name: &str, depth: usize) {
    DEPTHS.with(|d| {
        let mut d = d.borrow_mut();
        let at = d.entry(name.to_string()).or_insert(depth);
        *at = (*at).min(depth);
    });
}

pub(super) fn depth_of(name: &str) -> Option<usize> {
    DEPTHS.with(|d| d.borrow().get(name).copied())
}

/// What must be one, so only a computer's file (and the one file it builds
/// on) says it: a package only adds.
pub const SINGLE_KEYS: &[&str] = &["hostname", "timezone", "language", "keyboard", "login-screen", "unfree", "history", "cleanup", "updates"];

/// Where two files side by side give one value differently: maps key by
/// key; lists add up, so they never disagree.
fn disagree(a: &Value, b: &Value, path: &mut Vec<String>, found: &mut dyn FnMut(&[String], &Value, &Value)) {
    match (a, b) {
        (Value::Mapping(x), Value::Mapping(y)) => {
            for (k, va) in x {
                if let Some(vb) = y.get(k) {
                    path.push(crate::secrets::key_text(k));
                    disagree(va, vb, path, found);
                    path.pop();
                }
            }
        }
        (Value::Sequence(_), Value::Sequence(_)) => {}
        (x, y) if x != y => found(path, x, y),
        _ => {}
    }
}

fn lookup_path<'v>(v: &'v Value, path: &[String]) -> Option<&'v Value> {
    path.iter().try_fold(v, |v, k| v.get(k.as_str()))
}

pub(super) fn show_value(v: &Value) -> String {
    match v {
        Value::String(s) if !s.contains('\n') => s.clone(),
        Value::String(_) => "a text".into(),
        Value::Tagged(_) => "a secret".into(),
        other => serde_yaml::to_string(other).map(|t| t.trim().replace('\n', " ")).unwrap_or_default(),
    }
}

/// `over` on top of `under`: maps key by key, lists joined (`over`'s entries
/// first, nothing twice); anything else is `over`'s. A list in one and a
/// single value in the other is a mistake. Where both give the same file
/// (in a `files:`) and one gives it with options (`{ content, restart }`),
/// the other's value is its content: the contents merge, the options stay.
pub(crate) fn merge(under: Value, over: Value) -> anyhow::Result<Value> {
    merge_at(under, over, &mut Vec::new())
}

/// Two `files:` merged as [`merge`] does: `os.files` under `files:`.
pub(crate) fn merge_files(under: serde_yaml::Mapping, over: serde_yaml::Mapping) -> anyhow::Result<serde_yaml::Mapping> {
    match merge_at(Value::Mapping(under), Value::Mapping(over), &mut vec!["files".into()])? {
        Value::Mapping(m) => Ok(m),
        _ => unreachable!("two maps merge into a map"),
    }
}

fn merge_at(under: Value, over: Value, at: &mut Vec<String>) -> anyhow::Result<Value> {
    Ok(match (under, over) {
        (Value::Mapping(mut a), Value::Mapping(b)) => {
            let files = *at == ["files"] || *at == ["os", "files"];
            for (k, v) in b {
                at.push(crate::secrets::key_text(&k));
                let merged = match a.get_mut(&k).map(std::mem::take) {
                    // A setting's value is one value, even a list: yours replaces it.
                    Some(Value::Mapping(mut old)) if k.as_str() == Some("settings") => {
                        for (schema, keys) in v.as_mapping().cloned().unwrap_or_default() {
                            let mut both = old.remove(&schema).and_then(|o| o.as_mapping().cloned()).unwrap_or_default();
                            both.extend(keys.as_mapping().cloned().unwrap_or_default());
                            old.insert(schema, Value::Mapping(both));
                        }
                        Value::Mapping(old)
                    }
                    Some(old) if files && (crate::etc::is_options_value(&old) || crate::etc::is_options_value(&v)) => {
                        let (mut old, v) = (crate::etc::as_options(old), crate::etc::as_options(v));
                        // A `source:` replaces a `content:`, and the other way round.
                        for (mine, theirs) in [("content", "source"), ("source", "content")] {
                            if v.get(mine).is_some() {
                                old.as_mapping_mut().map(|o| o.remove(theirs));
                            }
                        }
                        merge_at(old, v, at)?
                    }
                    Some(old) => merge_at(old, v, at)?,
                    None => v,
                };
                at.pop();
                // A key that's there already keeps its place.
                a.insert(k, merged);
            }
            Value::Mapping(a)
        }
        // The computer's packages: the files underneath first, as their
        // files win in sw/ (base's, listed first, before the desktop's).
        (Value::Sequence(mut a), Value::Sequence(b)) if *at == ["packages"] => {
            for v in b {
                if !a.contains(&v) {
                    a.push(v);
                }
            }
            Value::Sequence(a)
        }
        (Value::Sequence(a), Value::Sequence(mut b)) => {
            for v in a {
                if !b.contains(&v) {
                    b.push(v);
                }
            }
            Value::Sequence(b)
        }
        (Value::Sequence(_), Value::String(_) | Value::Number(_) | Value::Bool(_)) | (Value::String(_) | Value::Number(_) | Value::Bool(_), Value::Sequence(_)) => {
            anyhow::bail!("{}: a list in one file, a single value in another: write it as a list, like [x]", at.join("."))
        }
        (_, b) => b,
    })
}
