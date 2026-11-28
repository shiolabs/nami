//! How a file's keys are read: old names and formats, the keys of a
//! computer's file and a package.nami in the ones nami works with (`os:`,
//! `home:`), names looked up in packages/, and the login screens a
//! computer's sets bring.

use super::*;

thread_local! {
    /// The notices said so far (each once a run).
    static SAID: std::cell::RefCell<std::collections::BTreeSet<String>> = Default::default();
    /// While `nami check` reads a file: it says its notices as warnings.
    static QUIET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A notice about something written the old way (`keep-generations is now
/// history`), said once.
pub fn deprecated(name: &str, what: &str) {
    if QUIET.with(|q| q.get()) {
        return;
    }
    let line = format!("{name}: {what} (the old way works until nami 1.0)");
    if SAID.with(|s| s.borrow_mut().insert(line.clone())) {
        crate::say!("{line}");
    }
}

/// `f` with no notices said (`nami check` gives them as warnings).
pub(super) fn quietly<T>(f: impl FnOnce() -> T) -> T {
    let was = QUIET.with(|q| q.replace(true));
    let out = f();
    QUIET.with(|q| q.set(was));
    out
}

/// A key's name as `text` writes it at the top: the old one when it has
/// that (`keep-generations:`), else `new`. To find its line.
pub(super) fn as_written(text: &str, new: &'static str) -> &'static str {
    let old = RENAMED_KEYS.iter().find(|(_, n)| *n == new).map(|(o, _)| *o);
    match old {
        Some(o) if text.lines().any(|l| l.strip_prefix(o).is_some_and(|r| r.starts_with(':'))) => o,
        _ => new,
    }
}

/// A file's own `format:` (taken out: it's for nami, not a setting), and
/// its keys in their new names, each old one said once.
pub fn modern(v: &mut Value, name: &str) -> anyhow::Result<()> {
    // (tray's `trayos:` is `namios:`, silently until 1.0.)
    crate::oldnames::keys(v);
    let Some(m) = v.as_mapping_mut() else { return Ok(()) };
    if let Some(f) = m.remove("format") {
        format_is_known(&f, name)?;
    }
    if !RENAMED_KEYS.iter().any(|(old, _)| m.contains_key(*old)) {
        return Ok(());
    }
    for (old, new) in RENAMED_KEYS {
        anyhow::ensure!(!(m.contains_key(*old) && m.contains_key(*new)), "{name}: {old}: and {new}: both say it, and {old} is now {new}: keep {new}");
    }
    // (Each in its place.)
    *m = std::mem::take(m)
        .into_iter()
        .map(|(k, val)| match RENAMED_KEYS.iter().find(|(old, _)| k.as_str() == Some(old)) {
            Some((old, new)) => {
                deprecated(name, &format!("{old} is now {new}"));
                (Value::from(*new), val)
            }
            None => (k, val),
        })
        .collect();
    Ok(())
}

/// A file's `format:` this nami reads: 1 (the only one yet).
pub fn format_is_known(f: &Value, name: &str) -> anyhow::Result<()> {
    let n = f.as_u64().filter(|n| *n >= 1).ok_or_else(|| anyhow::anyhow!("{name}: format: is a whole number, 1 for now"))?;
    anyhow::ensure!(n <= FORMAT, "{name}: format: {n}: {NEWER_NAMI}");
    Ok(())
}

/// [`modern`] without its notices: for a file read again elsewhere, which
/// says them there.
pub fn modern_quietly(v: &mut Value, name: &str) -> anyhow::Result<()> {
    quietly(|| modern(v, name))
}

/// A message in the words of the file: the keys as written (`users.ada`,
/// not `os.users.ada`; `env.X`, not `home.env.X`), and what serde says in
/// plain words (`true or false, not the text "yes"`, not `invalid type:
/// string "yes", expected a boolean`).
pub fn plain_message(msg: &str) -> String {
    // nami's own names inside, as the file writes them.
    const INSIDE: &[(&str, &str)] = &[
        ("os.environment", "env"), ("os.settings", "settings"), ("os.", ""),
        ("home.shell", "shell"), ("home.env", "env"), ("home.files", "files"), ("home.aliases", "aliases"), ("home.login-shell", "login-shell"), ("home: ", ""),
        ("environment:", "env:"),
    ];
    let mut out = String::with_capacity(msg.len());
    let mut rest = msg;
    'outer: while !rest.is_empty() {
        let at_word = out.chars().last().is_none_or(|c| !(c.is_alphanumeric() || "._-/~@:".contains(c)));
        if at_word {
            for (from, to) in INSIDE {
                if let Some(after) = rest.strip_prefix(from) {
                    // (A word that only starts so, like os.rs or homepage, stays.)
                    let next = after.chars().next();
                    let key_follows = from.ends_with(['.', ':', ' ']) || next.is_none_or(|c| !(c.is_alphanumeric() || c == '-'));
                    if key_follows && !(from == &"os." && !after.starts_with(|c: char| c.is_ascii_lowercase())) {
                        out.push_str(to);
                        rest = after;
                        continue 'outer;
                    }
                }
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    serde_words(&out)
}

/// serde's words for what's wrong, in plain ones.
fn serde_words(msg: &str) -> String {
    let thing = |t: &str| -> String {
        let t = t.trim();
        if let Some(s) = t.strip_prefix("string ") {
            return format!("the text {s}");
        }
        for (p, w) in [("integer `", "the number "), ("floating point `", "the number "), ("boolean `", ""), ("character `", "the text ")] {
            if let Some(v) = t.strip_prefix(p) {
                return format!("{w}{}", v.trim_end_matches('`'));
            }
        }
        match t {
            "map" => "a map".into(),
            "sequence" => "a list".into(),
            "unit value" | "null" => "nothing".into(),
            other => other.to_string(),
        }
    };
    let wanted = |e: &str| -> String {
        let e = e.trim();
        match e {
            "a boolean" => "true or false".into(),
            "a string" | "a borrowed string" | "string" => "text".into(),
            "a sequence" => "a list".into(),
            "usize" | "u64" | "u32" | "u16" | "u8" | "i64" | "i32" | "an integer" | "a number" => "a whole number".into(),
            "a map" | "a YAML mapping" => "a map".into(),
            e if e.starts_with("struct ") || e.starts_with("a map of ") => "a map of settings".into(),
            e => e.to_string(),
        }
    };
    // yes, no, on and off are text in YAML: a yes/no is true or false.
    if let Some(at) = msg.find("invalid type: string \"").filter(|_| msg.contains("expected a boolean")) {
        let word = msg[at + "invalid type: string \"".len()..].split('"').next().unwrap_or("");
        if ["yes", "no", "on", "off", "y", "n"].contains(&word.to_ascii_lowercase().as_str()) {
            let yes = ["yes", "on", "y"].contains(&word.to_ascii_lowercase().as_str());
            return format!("{}true or false: write {yes} (in YAML, {word} is text)", &msg[..at]);
        }
    }
    let (head, tail) = match msg.find("invalid type: ").or_else(|| msg.find("unknown field `")).or_else(|| msg.find("unknown variant `")).or_else(|| msg.find("missing field `")).or_else(|| msg.find("invalid value: ")) {
        Some(at) => (&msg[..at], &msg[at..]),
        None => return msg.to_string(),
    };
    // (serde's own place in the text: the message says where already.)
    let tail = tail.split(" at line ").next().unwrap_or(tail);
    let said = if let Some(r) = tail.strip_prefix("invalid type: ").or_else(|| tail.strip_prefix("invalid value: ")) {
        match r.split_once(", expected ") {
            Some((found, exp)) => format!("{}, not {}", wanted(exp), thing(found)),
            None => r.to_string(),
        }
    } else if let Some(r) = tail.strip_prefix("unknown field `") {
        format!("unknown key `{}`", r.split('`').next().unwrap_or(r))
    } else if let Some(r) = tail.strip_prefix("unknown variant `") {
        let (bad, exp) = r.split_once('`').unwrap_or((r, ""));
        let choices: Vec<&str> = exp.split('`').skip(1).step_by(2).collect();
        if choices.is_empty() { format!("{bad} isn't one nami knows") } else { format!("{bad} isn't one of {}", choices.join(", ")) }
    } else if let Some(r) = tail.strip_prefix("missing field `") {
        format!("{}: is needed", r.split('`').next().unwrap_or(r))
    } else {
        tail.to_string()
    };
    format!("{head}{said}")
}

/// The short key a longer name means (`desktop-settings` is `settings`):
/// nami's keys are short, and say what they are in the docs.
fn likely_key(k: &str) -> Option<&'static str> {
    match k {
        "desktop-settings" | "gsettings" | "dconf" => Some("settings"),
        "allow-unfree" | "nonfree" | "non-free" => Some("unfree"),
        "shell-init" | "shell-rc" | "rc" => Some("shell"),
        "environment" | "variables" => Some("env"),
        _ => None,
    }
}

/// What to say about a key nami doesn't know: the one it's likely to be,
/// and that a newer nami may know it.
pub fn unknown_key_hint(close: Option<&str>) -> String {
    match close {
        Some(c) => format!("did you mean `{c}`? or this file needs a newer nami (nami self-update)"),
        None => "or this file needs a newer nami (nami self-update)".to_string(),
    }
}

/// Whether a file is a package's (a package.nami).
pub(super) fn is_package_nami(name: &str) -> bool {
    Path::new(name).file_name().is_some_and(|f| crate::oldnames::is_named(f, "package.nami"))
}

/// What's not for this file, in a computer's file or a package.nami: the
/// earlier format's keys, a package's keys in a computer's file, and
/// `desktop:` in a package (one that lists a login screen is a desktop).
pub(super) fn check_keys(own: &Value, name: &str, scope: Scope) -> anyhow::Result<()> {
    if scope == Scope::User {
        return Ok(());
    }
    let Some(m) = own.as_mapping() else { return Ok(()) };
    for (k, now) in FORMER_KEYS {
        anyhow::ensure!(!m.contains_key(*k), "{name}: {k}: is {now}");
    }
    if is_package_nami(name) {
        anyhow::ensure!(
            !m.contains_key("desktop"),
            "{name}: desktop: a package that lists a login screen (gdm, namios-greeter) is a desktop by itself; which desktops a computer has is its system.nami's desktop:"
        );
    } else if let Some(k) = PACKAGE_KEYS.iter().find(|k| m.contains_key(**k)) {
        anyhow::bail!("{name}: {k}: is a package's, for the parts of Linux many packages add to: put it in a package.nami of your own (packages/<name>/package.nami) and list that");
    }
    Ok(())
}

/// A computer's file or a package.nami in the keys nami reads them with
/// ([`COMPUTER_KEYS`]): a package's systemd units are files at
/// /etc/systemd/system, its files made by a script are `build`, a service
/// in `without:` is kept off, `login:`'s `all:` lines go in every login,
/// `setuid:` paths act as root, `boot:` and `drivers:` hold their parts.
/// A package also brings its checks.nami, and one that lists a login
/// screen is a desktop.
pub(super) fn read_keys(mut own: Value, name: &str, base: &Path, scope: Scope) -> anyhow::Result<Value> {
    if scope == Scope::User {
        return Ok(own);
    }
    let package = is_package_nami(name);
    let Some(m) = own.as_mapping_mut() else { return Ok(own) };
    let mut put = serde_yaml::Mapping::new();
    if let Some(v) = m.remove("env") {
        put.insert("environment".into(), v);
    }
    if let Some(Value::Sequence(list)) = m.remove("without") {
        let (units, packages): (Vec<Value>, Vec<Value>) =
            list.into_iter().partition(|v| v.as_str().is_some_and(|n| crate::services::UNIT_TYPES.iter().any(|t| n.ends_with(t))));
        if !packages.is_empty() {
            put.insert("without".into(), Value::Sequence(packages));
        }
        if !units.is_empty() {
            put.insert("mask".into(), Value::Sequence(units));
        }
    }
    if let Some(v) = m.remove("login") {
        let Value::Mapping(login) = v else {
            anyhow::bail!("{name}: login: <program>: login, su or its rules; all: [lines every login gets]");
        };
        let mut services = serde_yaml::Mapping::new();
        for (k, v) in login {
            if k.as_str() == Some("all") {
                put.insert("login".into(), v);
            } else {
                services.insert(k, v);
            }
        }
        if !services.is_empty() {
            put.insert("login-services".into(), Value::Mapping(services));
        }
    }
    if let Some(v) = m.remove("setuid") {
        let paths: Vec<String> = serde_yaml::from_value(v).map_err(|_| anyhow::anyhow!("{name}: setuid: a list of paths in the packages, like bin/sudo"))?;
        put.insert("wrappers".into(), Value::Sequence(paths.iter().map(|p| Value::from(format!("4755 {p}"))).collect()));
    }
    // What hardware it's for: for nami doctor and nami check only.
    if let Some(v) = m.remove("hardware") {
        crate::hardware::parse(&v, name)?;
    }
    let boot: &[(&str, &str)] = &[("kernel", "kernel"), ("microcode", "early-boot"), ("initrd-programs", "initrd-programs"), ("initrd-modules", "initrd-modules"), ("kernel-options", "kernel-options")];
    let drivers: &[(&str, &str)] = &[("firmware", "firmware"), ("graphics", "graphics"), ("graphics-32bit", "graphics-32bit"), ("kernel-modules", "kernel-modules")];
    for (key, parts) in [("boot", boot), ("drivers", drivers)] {
        let Some(v) = m.remove(key) else { continue };
        let Value::Mapping(given) = v else { anyhow::bail!("{name}: {key}: {}", parts.iter().map(|p| p.0).collect::<Vec<_>>().join(", ")) };
        for (k, v) in given {
            let inner = parts.iter().find(|p| k.as_str() == Some(p.0)).map(|p| p.1);
            let inner = inner.ok_or_else(|| anyhow::anyhow!("{name}: {key}: {}: one of {}", crate::secrets::key_text(&k), parts.iter().map(|p| p.0).collect::<Vec<_>>().join(", ")))?;
            if inner == "kernel-options" {
                kernel_options(name, &v)?;
            }
            put.insert(inner.into(), v);
        }
    }
    if package {
        // Its systemd units, and its files made from what's installed.
        if let Some(Value::Mapping(files)) = m.remove("files") {
            let (mut kept, mut units, mut built, mut tools) = (serde_yaml::Mapping::new(), serde_yaml::Mapping::new(), serde_yaml::Mapping::new(), Vec::new());
            for (k, v) in files {
                let path = k.as_str().unwrap_or_default().to_string();
                if let Some(unit) = path.strip_prefix("/etc/systemd/system/") {
                    units.insert(unit.into(), v);
                } else if let Some(unit) = path.strip_prefix("/etc/systemd/user/") {
                    units.insert(format!("user/{unit}").into(), v);
                } else if let Some(script) = v.get("script") {
                    for t in v.get("tools").and_then(Value::as_sequence).into_iter().flatten() {
                        if !tools.contains(t) {
                            tools.push(t.clone());
                        }
                    }
                    if let Some(extra) = v.as_mapping().and_then(|o| o.keys().filter_map(Value::as_str).find(|x| !["script", "tools"].contains(x))) {
                        anyhow::bail!("{name}: files: {path}: {extra}: a file made by a script has script: and tools:");
                    }
                    built.insert(k, script.clone());
                } else {
                    kept.insert(k, v);
                }
            }
            for (key, map) in [("files", kept), ("units", units), ("build", built)] {
                if !map.is_empty() {
                    put.insert(key.into(), Value::Mapping(map));
                }
            }
            if !tools.is_empty() {
                put.insert("build-tools".into(), Value::Sequence(tools));
            }
        }
        // Its checks.nami: how to tell it works.
        if let Ok(text) = std::fs::read_to_string(crate::oldnames::find_in(base, "checks.nami")) {
            let checks = name.replace("package.nami", "checks.nami").replace("package.tray", "checks.tray");
            let mut value: Value = serde_yaml::from_str(&text).map_err(|e| anyhow::anyhow!("{checks}: {e}"))?;
            // (Its keys are checks' names: `format: 1` is its format, a check
            // called format is a map.)
            if let Some(m) = value.as_mapping_mut().filter(|m| m.get("format").is_some_and(Value::is_number)) {
                format_is_known(&m.remove("format").unwrap_or_default(), &checks)?;
            }
            if !value.is_null() {
                put.insert("checks".into(), value);
            }
        }
        // A package that lists a login screen is a desktop: its session goes
        // on the login screen (unless it lists a desktop too: with_login_screen).
        if lists_login_screen(m) || LISTED_UNUSED.with(|l| l.borrow().contains(name)) {
            let me = base.canonicalize().ok().and_then(|d| d.file_name().map(|f| f.to_string_lossy().into_owned()));
            if let Some(me) = me {
                put.insert("desktop".into(), Value::Sequence(vec![Value::from(me)]));
            }
        }
    }
    for (k, v) in put {
        m.insert(k, v);
    }
    Ok(own)
}

/// A file's whole value, with what it brings: for a computer, its
/// packages' files.
pub(super) fn complete(value: Value, name: &str, mine: &Value) -> anyhow::Result<Value> {
    match Scope::of(name, mine) {
        Scope::System => with_login_screen(value, mine),
        Scope::User => Ok(value),
    }
}

/// A `packages:` entry's name: a package folder's is its folder's
/// (../gdm: gdm), a catalog name's is itself (without a version).
pub fn entry_name(entry: &str) -> &str {
    if crate::build::Builder::is_recipe(entry) {
        entry.trim_end_matches('/').rsplit('/').next().unwrap_or(entry)
    } else {
        crate::index::split_version(entry).0
    }
}

/// The value as nami reads it inside (a [`Config`]): the computer's keys
/// under `os:`, a person's under `home:`. `mine` is the file (with its own
/// sets) before nami's package files added theirs: what a computer nami
/// didn't make gets (its apps, files and services); what the package files
/// add is for a computer nami makes.
pub(super) fn inside(mut value: Value, mine: &Value, name: &str) -> anyhow::Result<Value> {
    let own = mine.get("packages").and_then(Value::as_sequence).cloned().unwrap_or_default();
    let Some(m) = value.as_mapping_mut() else { return Ok(value) };
    let known: Vec<&str> = [TOP_KEYS, COMPUTER_KEYS, USER_KEYS].concat();
    for k in m.keys().filter_map(Value::as_str) {
        if !known.contains(&k) {
            anyhow::bail!("{name}: unknown key `{k}` ({})", unknown_key_hint(likely_key(k).or_else(|| closest(k, &known))));
        }
    }
    match Scope::of(name, mine) {
        Scope::System => {
            if let Some(k) = USER_KEYS.iter().find(|k| m.contains_key(**k)) {
                anyhow::bail!("{name}: {k}: is a person's, in their ~/{USER_FILE}");
            }
            let mut os = serde_yaml::Mapping::new();
            for k in COMPUTER_KEYS.iter().filter(|k| **k != "wifi") {
                if let Some(v) = m.remove(*k) {
                    os.insert((*k).into(), v);
                }
            }
            if let Some(v) = m.remove("settings") {
                os.insert("settings".into(), v);
            }
            // `updates: manual`: the daily update's timer kept off, as `without:` would.
            if let Some(v) = os.remove("updates") {
                let on = updates_of(&Value::Mapping([("updates".into(), v)].into_iter().collect())).map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
                if !on {
                    let mask = os.entry("mask".into()).or_insert_with(|| Value::Sequence(Vec::new()));
                    if let Some(list) = mask.as_sequence_mut().filter(|l| !l.iter().any(|u| u.as_str() == Some(UPDATE_TIMER))) {
                        list.push(UPDATE_TIMER.into());
                    }
                }
            }
            // The package files' /etc files: the file's own are everywhere's.
            if let Some(Value::Mapping(all)) = m.remove("files") {
                let theirs = mine.get("files").and_then(Value::as_mapping).cloned().unwrap_or_default();
                let added: serde_yaml::Mapping = all.into_iter().filter(|(k, v)| theirs.get(k) != Some(v)).collect();
                if !added.is_empty() {
                    os.insert("files".into(), Value::Mapping(added));
                }
                if !theirs.is_empty() {
                    m.insert("files".into(), Value::Mapping(theirs));
                }
            }
            // A unit the package files turn on (cups.socket: true), where its
            // [Install] says; `per-person: true` (anyone's file): each person's.
            let listed = mine.get("services").and_then(Value::as_mapping).cloned().unwrap_or_default();
            if let Some(services) = m.get_mut("services").and_then(Value::as_mapping_mut) {
                let (mut start, mut each) = (Vec::new(), Vec::new());
                services.retain(|k, v| {
                    let Some(unit) = k.as_str().filter(|n| crate::services::UNIT_TYPES.iter().any(|t| n.ends_with(t))) else { return true };
                    match v {
                        Value::Bool(true) if !listed.contains_key(k) => start.push(Value::from(unit)),
                        Value::Mapping(o) if o.len() == 1 && o.get("per-person") == Some(&Value::Bool(true)) => each.push(Value::from(unit)),
                        _ => return true,
                    }
                    false
                });
                for (key, list) in [("start", start), ("start-per-person", each)] {
                    if !list.is_empty() {
                        os.insert(key.into(), Value::Sequence(list));
                    }
                }
                // The package folders' own services: for a computer nami makes.
                let theirs: serde_yaml::Mapping = services.iter().filter(|(k, _)| !listed.contains_key(*k)).map(|(k, v)| (k.clone(), v.clone())).collect();
                services.retain(|k, _| listed.contains_key(k));
                if !theirs.is_empty() {
                    os.insert("services".into(), Value::Mapping(theirs));
                }
            }
            if m.get("services").and_then(Value::as_mapping).is_some_and(|s| s.is_empty()) {
                m.remove("services");
            }
            if let Some(v) = m.remove("packages") {
                os.insert("packages".into(), v);
                m.insert("packages".into(), Value::Sequence(own));
            }
            m.insert("os".into(), Value::Mapping(os));
        }
        Scope::User => {
            if let Some(k) = COMPUTER_KEYS.iter().find(|k| m.contains_key(**k)) {
                anyhow::bail!("{name}: {k}: is the computer's, in system.nami");
            }
            // (namiOS's desktop reads it from the file itself.)
            m.remove("namios");
            let mut home = serde_yaml::Mapping::new();
            for k in USER_KEYS {
                if let Some(v) = m.remove(*k) {
                    home.insert((*k).into(), v);
                }
            }
            if let Some(files) = m.remove("files") {
                let mut mine = serde_yaml::Mapping::new();
                for (path, v) in files.as_mapping().cloned().unwrap_or_default() {
                    let p = path.as_str().unwrap_or_default();
                    let rel = p.strip_prefix("~/").ok_or_else(|| anyhow::anyhow!("{name}: files: {p}: a person's files are in their home, ~/...; the computer's are in system.nami"))?;
                    mine.insert(rel.into(), v);
                }
                home.insert("files".into(), Value::Mapping(mine));
            }
            if !home.is_empty() {
                m.insert("home".into(), Value::Mapping(home));
            }
        }
    }
    Ok(value)
}

/// The folder names alone are looked up in: the nearest `packages` folder
/// going up from `base` (one named so, or one with packages/ in it), not
/// past the top of a git repo.
pub fn packages_dir(base: &Path) -> Option<std::path::PathBuf> {
    let base = absolute(base);
    for d in base.ancestors() {
        if d.file_name().is_some_and(|n| n == PACKAGES_DIR) {
            return Some(d.to_path_buf());
        }
        if d.join(PACKAGES_DIR).is_dir() {
            return Some(d.join(PACKAGES_DIR));
        }
        if d.join(".git").exists() {
            break;
        }
    }
    None
}

/// A name alone in `packages:` is the catalog's package, set up by the
/// folder of its name in the nearest packages/ folder when there is one (a
/// set, when the catalog has no program of that name). Read here into that
/// folder's path: the rest of nami sees the file as if it were written so.
/// A package's own name in its own package.nami is the catalog's program.
pub(crate) fn resolve_names(value: &mut Value, base: &Path) {
    let Some(dir) = packages_dir(base) else {
        for p in value.get_mut("packages").and_then(Value::as_sequence_mut).into_iter().flatten() {
            if let Some(n) = p.as_str().and_then(|n| n.strip_prefix(CATALOG_PREFIX)) {
                *p = Value::String(n.to_string());
            }
        }
        return;
    };
    let here = absolute(base);
    // The path from the file's folder: up to where the two meet, then down.
    let up = here.ancestors().position(|a| dir.starts_with(a)).unwrap_or(0);
    let meet = here.ancestors().nth(up).unwrap_or(&here).to_path_buf();
    let down = dir.strip_prefix(&meet).unwrap_or(Path::new("")).to_path_buf();
    let mut rel = if up == 0 { ".".to_string() } else { vec![".."; up].join("/") };
    if !down.as_os_str().is_empty() {
        rel = format!("{rel}/{}", down.display());
    }
    for p in value.get_mut("packages").and_then(Value::as_sequence_mut).into_iter().flatten() {
        // `nixpkgs:niri`: nixpkgs' niri itself, never a folder.
        if let Some(n) = p.as_str().and_then(|n| n.strip_prefix(CATALOG_PREFIX)) {
            *p = Value::String(n.to_string());
            continue;
        }
        let Some(n) = p.as_str().filter(|n| is_bare_name(n)) else { continue };
        // tray's name for one of its own (`- trayos`): the package it is now, when
        // that's there (tray's folder of it may be still, as tray left it).
        if let Some(now) = crate::oldnames::package(n).filter(|now| crate::oldnames::is_file(&dir.join(now).join("package.nami"))) {
            *p = Value::String(format!("{rel}/{now}"));
        } else if crate::oldnames::is_file(&dir.join(n).join("package.nami")) && dir.join(n) != here {
            *p = Value::String(format!("{rel}/{n}"));
        }
    }
}

/// `nixpkgs:<name>` in `packages:`: the catalog's package alone, where a
/// folder of that name would set it up as more (niri's is the desktop; a
/// login screen only wants the program).
pub const CATALOG_PREFIX: &str = "nixpkgs:";

/// A name alone (`htop`), not a path, a version, a repo's or a set: what
/// the packages/ folder is looked in for.
pub(crate) fn is_bare_name(n: &str) -> bool {
    !n.is_empty() && !n.contains(['/', ':', '@']) && !crate::oldnames::ends_with_extension(n)
}

/// The computer's packages in the order the files list them, each set's
/// in its place (namios: niri's, which start with desktop's, which start
/// with base's; then namiOS's apps): base's come first, as their files win
/// in sw/, and the file's own last.
fn in_listed_order(value: &mut Value, mine: &Value) {
    // (Only what's in the list: a folder's own names are as it wrote them,
    // before they're looked up.)
    fn expand(list: &[String], all: &[String], out: &mut Vec<String>, seen: &mut std::collections::HashSet<String>) {
        for name in list {
            if !seen.insert(name.clone()) {
                continue;
            }
            if let Some(setup) = FOLDERS.with(|f| f.borrow().get(crate::index::split_version(name).0).cloned()) {
                let theirs: Vec<String> = setup.get("packages").and_then(Value::as_sequence).into_iter().flatten().filter_map(Value::as_str).map(|p| entry_name(p).to_string()).collect();
                expand(&theirs, all, out, seen);
            }
            if all.contains(name) {
                out.push(name.clone());
            }
        }
    }
    let Some(list) = value.get("packages").and_then(Value::as_sequence) else { return };
    let all: Vec<String> = list.iter().filter_map(Value::as_str).map(String::from).collect();
    // The file's own list (and its sets'), expanded; then anything else.
    let top: Vec<String> = mine.get("packages").and_then(Value::as_sequence).into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
    let mut out = Vec::new();
    expand(&top, &all, &mut out, &mut Default::default());
    for p in all {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    value.as_mapping_mut().map(|m| m.insert("packages".into(), Value::Sequence(out.into_iter().map(Value::from).collect())));
}

/// One login screen for a computer: the file's, else the one listed
/// nearest to it (namios-greeter under namios, before niri's gdm); two as
/// near: say which.
fn with_login_screen(mut value: Value, mine: &Value) -> anyhow::Result<Value> {
    in_listed_order(&mut value, mine);
    // A package that lists a login screen and a desktop (namios: niri, and
    // its own login screen) only picks the login screen.
    let desktops = folder_desktops();
    let picks_only = |d: &str| FOLDERS.with(|f| f.borrow().iter().any(|(n, m)| n == d && lists_login_screen(m))) && !desktops.iter().any(|x| x == d);
    if let Some(list) = value.get_mut("desktop").and_then(Value::as_sequence_mut) {
        list.retain(|d| !d.as_str().is_some_and(picks_only));
    }
    if value.get("login-screen").is_some() {
        return Ok(value);
    }
    let found = listed_login_screens(&value);
    if let Some(&(d, p, dm)) = found.first() {
        let tied: Vec<&str> = found.iter().filter(|f| f.0 == d).map(|f| f.2).collect();
        anyhow::ensure!(tied.len() == 1, "two login screens are listed as near ({}): say which, login-screen: {}", tied.join(", "), tied[0]);
        // (The flat view: it's the file's that lists that login screen.)
        if let Some(who) = crate::flat::lister(p) {
            let mark = crate::flat::mark();
            crate::flat::record(&who, &serde_yaml::from_str(&format!("login-screen: {dm}")).unwrap_or_default());
            crate::flat::to_front(mark);
        }
        if let Some(m) = value.as_mapping_mut() {
            m.insert("login-screen".into(), Value::from(dm));
        }
    }
    Ok(value)
}

/// The login screens a computer's packages list: (how near, package,
/// display manager), the nearest first.
fn listed_login_screens(value: &Value) -> Vec<(usize, &'static str, &'static str)> {
    let listed: Vec<&str> = value.get("packages").and_then(Value::as_sequence).into_iter().flatten().filter_map(Value::as_str).collect();
    let mut found: Vec<(usize, &str, &str)> = LOGIN_SCREENS
        .iter()
        .filter(|(p, _)| listed.contains(p))
        .map(|(p, dm)| (depth_of(p).unwrap_or(usize::MAX), *p, *dm))
        .collect();
    found.sort();
    found
}

/// The login screens a computer's sets bring that it doesn't use (niri's
/// GDM, under namiOS's own): left out, with all they set up. One the file
/// lists itself stays.
pub(super) fn unused_login_screens(value: &Value, mine: &Value) -> Vec<String> {
    let found = listed_login_screens(value);
    let picked = match value.get("login-screen").and_then(Value::as_str) {
        Some(dm) => dm.to_string(),
        None => match found.as_slice() {
            [] => return Vec::new(),
            [(d, _, dm), rest @ ..] if !rest.iter().any(|r| r.0 == *d && r.2 != *dm) => dm.to_string(),
            // (Two as near: with_login_screen asks which.)
            _ => return Vec::new(),
        },
    };
    let own: Vec<&str> = mine.get("packages").and_then(Value::as_sequence).into_iter().flatten().filter_map(Value::as_str).map(entry_name).collect();
    found.iter().filter(|(_, p, dm)| *dm != picked && !own.contains(p) && is_package_file(p)).map(|(_, p, _)| p.to_string()).collect()
}

/// A package folder listed in a person's file: its packages, and what's
/// theirs of its setup (their own settings, files in their home, the
/// services each person runs: `{ per-person: true }`, theirs alone here).
/// What it sets up for the computer (files in /etc, its services, groups)
/// comes when system.nami lists it; nami says so.
/// `boot: kernel-options:`: each one word for the kernel's command line
/// (`pcie_aspm=off`). Where the system and its disk are is nami's own.
fn kernel_options(name: &str, v: &Value) -> anyhow::Result<()> {
    let given: Vec<String> = serde_yaml::from_value(v.clone()).map_err(|_| anyhow::anyhow!("{name}: boot: kernel-options: a list, like [pcie_aspm=off, quiet]"))?;
    for o in &given {
        anyhow::ensure!(!o.is_empty() && !o.contains(char::is_whitespace) && !o.contains('"'), "{name}: boot: kernel-options: {o:?}: one option each, without spaces or quotes");
        let key = o.split('=').next().unwrap_or_default();
        anyhow::ensure!(!key.starts_with("nami.") && !key.starts_with("tray.") && !["root", "init"].contains(&key), "{name}: boot: kernel-options: {o}: nami sets where the system and its disk are");
    }
    Ok(())
}

pub(super) fn personal_part(mut value: Value, label: &str) -> Value {
    let Some(m) = value.as_mapping_mut() else { return value };
    let mut left: Vec<String> = Vec::new();
    if let Some(Value::Mapping(files)) = m.get_mut("files") {
        let theirs: Vec<Value> = files.keys().filter(|k| !k.as_str().is_some_and(|p| p.starts_with("~/"))).cloned().collect();
        for k in theirs {
            files.remove(&k);
            left.push(k.as_str().unwrap_or_default().to_string());
        }
        if files.is_empty() {
            m.remove("files");
        }
    }
    // Each person's services are theirs: the computer's stay behind.
    if let Some(services) = m.get("services").cloned() {
        // (A list as written, or by now a map: name -> its settings.)
        let entries: Vec<(Value, Value)> = match services {
            Value::Sequence(list) => list.into_iter().map(|e| match e {
                Value::Mapping(one) => one.into_iter().next().unwrap_or((Value::Null, Value::Null)),
                name => (name, Value::Null),
            }).collect(),
            Value::Mapping(map) => map.into_iter().collect(),
            _ => Vec::new(),
        };
        let mut theirs = Vec::new();
        for (name, v) in entries {
            if v.get("per-person").and_then(Value::as_bool) == Some(true) {
                theirs.push(name);
            } else {
                left.push(name.as_str().unwrap_or_default().to_string());
            }
        }
        if theirs.is_empty() {
            m.remove("services");
        } else {
            m.insert("services".into(), Value::Mapping(theirs.into_iter().map(|n| (n, Value::Bool(true))).collect()));
        }
    }
    let keys: Vec<Value> = m.keys().filter(|k| k.as_str().is_some_and(|k| COMPUTER_KEYS.contains(&k) || PACKAGE_KEYS.contains(&k))).cloned().collect();
    for k in keys {
        m.remove(&k);
        left.push(format!("{}:", k.as_str().unwrap_or_default()));
    }
    if !left.is_empty() {
        let folder = Path::new(label).parent().and_then(|p| p.file_name()).map_or_else(|| label.to_string(), |f| f.to_string_lossy().into_owned());
        crate::say!("{folder}: {} {} the computer's, set up when system.nami lists {folder}", left.join(", "), if left.len() == 1 { "is" } else { "are" });
    }
    value
}

/// The login screens nami knows: (package, display manager).
const LOGIN_SCREENS: &[(&str, &str)] = &[("gdm", "gdm"), ("namios-greeter", "greetd")];

/// The package folders listed so far that list a login screen: desktops.
pub fn folder_desktops() -> Vec<String> {
    let names = |m: &serde_yaml::Mapping| -> Vec<String> { m.get("packages").and_then(Value::as_sequence).into_iter().flatten().filter_map(Value::as_str).map(|p| entry_name(p).to_string()).collect() };
    FOLDERS.with(|f| {
        let screens: Vec<(String, Vec<String>)> = f.borrow().iter().filter(|(_, m)| lists_login_screen(m)).map(|(n, m)| (n.clone(), names(m))).collect();
        // (One that lists a desktop too only picks the login screen.)
        // (niri lists niri, the program: that's itself.)
        screens.iter().filter(|(n, listed)| !listed.iter().any(|l| l != n && screens.iter().any(|(s, _)| s == l))).map(|(n, _)| n.clone()).collect()
    })
}

/// Whether a package lists a login screen (gdm, namios-greeter).
fn lists_login_screen(m: &serde_yaml::Mapping) -> bool {
    m.get("packages").and_then(Value::as_sequence).into_iter().flatten().filter_map(Value::as_str).any(|p| LOGIN_SCREENS.iter().any(|(l, _)| *l == entry_name(p)))
}
