//! `nami check`: a nami file's problems, each at its line (Diagnostic), and
//! what's said of a file that does nothing on this computer.

use super::*;

/// What another distribution keeps as it is when a computer file has `key`
/// (`its own users`).
fn kept_instead(key: &str) -> &'static str {
    match key {
        "hostname" => "its own computer name",
        "users" => "its own users",
        "groups" => "its own groups",
        "timezone" => "its own time zone",
        "language" => "its own language",
        "keyboard" => "its own keyboard layout",
        "desktop" => "its own desktop",
        "login-screen" => "its own login screen",
        "env" | "environment" => "its own environment variables",
        "settings" => "its own settings",
        "login" | "login-services" => "its own login rules",
        "name-lookup" => "its own name lookup",
        "without" | "mask" | "units" => "its own services",
        "kernel" | "kernel-options" | "kernel-modules" | "early-boot" | "initrd-programs" | "initrd-modules" | "secure-boot" => "its own kernel and startup",
        "firmware" | "graphics" | "graphics-32bit" => "its own drivers",
        _ => "its own system",
    }
}

/// What a computer file says that does nothing on a computer namiOS isn't
/// (`distro`: its name, `Ubuntu`): each key only namiOS acts on, and each
/// set for namiOS it lists, a warning at its line.
pub fn not_on_this_distro(text: &str, distro: Option<&str>, index: Option<&Index>) -> Vec<Diagnostic> {
    let distro = distro.unwrap_or("This distribution");
    let mut out: Vec<Diagnostic> = only_on_nami_os_keys(text)
        .iter()
        .map(|k| {
            let hint = match k.as_str() {
                "env" | "settings" => Some(format!("each person's {k}: in their {USER_FILE} works here")),
                _ => None,
            };
            diag(find_line(text, None, k), Severity::Warning, format!("`{k}:` only works on namiOS. {distro} keeps {}.", kept_instead(k)), hint)
        })
        .collect();
    if let (Some(index), Ok(v)) = (index, crate::yaml_text::from_str(text)) {
        for p in v.get("packages").and_then(Value::as_sequence).into_iter().flatten().filter_map(Value::as_str) {
            if is_plain_entry(p) && is_set(p, index) {
                out.push(diag(find_line(text, Some("packages"), p), Severity::Warning, format!("{p} is a set for namiOS; left out here"), Some(format!("{distro} keeps its own system"))));
            }
        }
    }
    out
}

/// The notices for what `text` writes the old way, as warnings at their lines.
fn modern_notes(text: &str) -> Vec<Diagnostic> {
    let Ok(Value::Mapping(m)) = crate::yaml_text::from_str(text) else { return Vec::new() };
    RENAMED_KEYS
        .iter()
        .filter(|(old, _)| m.contains_key(*old))
        .map(|(old, new)| diag(find_line(text, None, old), Severity::Warning, format!("{old} is now {new}"), Some(format!("rename it; {old} works until nami 1.0"))))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    /// Not a problem: worth knowing (a secret you can open here).
    Note,
}

/// Whether each secret in a nami file can be opened here, without asking
/// for anything: with your keys that need no passphrase, and in a system
/// file, with the computer's own key (the one `nami usb` keeps for it).
/// Says which keys it couldn't try.
pub fn secret_notes(text: &str, system: bool) -> Vec<Diagnostic> {
    let Ok(found) = crate::secrets::find(text) else { return Vec::new() };
    let found: Vec<_> = found.into_iter().filter(|f| f.sealed).collect();
    if found.is_empty() {
        return Vec::new();
    }
    let (mine, untried) = crate::secrets::Keys::without_asking();
    // The computer's own key, kept where `nami usb` made it.
    let computer = system.then(|| {
        let host = crate::yaml_text::from_str(text).ok().and_then(|v| v.get("hostname")?.as_str().map(String::from)).unwrap_or_else(|| "nami".into());
        let key = crate::secrets::computer_key_dir().join(format!("{host}.age.key"));
        let public = crate::secrets::public_of(&key).unwrap_or_default();
        (host, public, crate::secrets::Keys::file(&key).ok())
    });
    let mut out = Vec::new();
    let tried = if mine.names.is_empty() { "you have no key here that needs no passphrase".to_string() } else { format!("tried {}", mine.names.join(", ")) };
    for f in &found {
        if mine.opens(&f.value) {
            out.push(diag(f.line, Severity::Note, format!("{}: you can open it here", f.path), None));
        } else {
            out.push(diag(f.line, Severity::Warning, format!("{}: none of your keys here open it ({tried})", f.path), Some(crate::secrets::access_hint())));
        }
        if let Some((host, public, Some(keys))) = &computer {
            if keys.opens(&f.value) {
                out.push(diag(f.line, Severity::Note, format!("{}: {host} (the computer) can open it", f.path), None));
            } else {
                out.push(diag(f.line, Severity::Warning, format!("{}: {host} (the computer) can't open it", f.path), Some(format!("`nami secret allow {host} {public}` lets it (`nami usb` does too)"))));
            }
        }
    }
    for u in untried {
        out.push(diag(0, Severity::Note, format!("couldn't try {u}"), None));
    }
    out
}

#[derive(Debug, Serialize)]
pub struct Diagnostic {
    /// 1-based; 0 when it isn't about one line.
    pub line: usize,
    pub column: usize,
    pub severity: Severity,
    pub message: String,
    /// What to do about it, when we know.
    pub hint: Option<String>,
}

impl Diagnostic {
    /// Where it is, to print: `file:line`, or the file alone when it
    /// isn't about one line.
    pub fn at(&self, file: &str) -> String {
        if self.line == 0 { file.to_string() } else { format!("{file}:{}", self.line) }
    }

    /// As `nami check` says it: `file:3: error: … (hint)`.
    pub fn shown(&self, file: &str, paint: crate::style::Paint) -> String {
        let sev = match self.severity { Severity::Error => "error", Severity::Warning => "warning", Severity::Note => "note" };
        let hint = self.hint.as_ref().map(|h| format!(" ({})", paint.message(h))).unwrap_or_default();
        format!("{}: {}: {}{hint}", paint.bold(&self.at(file)), paint.severity(sev), paint.message(&self.message))
    }
}

fn diag(line: usize, severity: Severity, message: impl Into<String>, hint: Option<String>) -> Diagnostic {
    Diagnostic { line, column: 0, severity, message: message.into(), hint }
}

/// The 1-based line of the first `needle` (as a whole word) at or after the
/// line that starts the `section` key; 0 if not found.
pub(super) fn find_line(text: &str, section: Option<&str>, needle: &str) -> usize {
    find_nth_line(text, section, needle, 0)
}

/// Like `find_line`, for the `nth` (0-based) occurrence.
fn find_nth_line(text: &str, section: Option<&str>, needle: &str, nth: usize) -> usize {
    // (Nothing to look for: no line, and `find("")` would never move on.)
    if needle.is_empty() {
        return 0;
    }
    let mut seen = 0;
    let start = section
        .and_then(|s| text.lines().position(|l| l.trim_start().starts_with(&format!("{s}:"))))
        .unwrap_or(0);
    for (i, line) in text.lines().enumerate().skip(start) {
        let mut rest = line;
        while let Some(at) = rest.find(needle) {
            let before = rest[..at].chars().last();
            let after = rest[at + needle.len()..].chars().next();
            let edge = |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '.'));
            if edge(before) && edge(after) {
                if seen == nth {
                    return i + 1;
                }
                seen += 1;
            }
            rest = &rest[at + needle.len()..];
        }
    }
    0
}

/// Catalog names a typo away from `name` (apps first), at most three.
pub(crate) fn typo_matches(index: &Index, name: &str) -> Vec<String> {
    // A package set's member by its name: nerd-fonts-jetbrains-mono is
    // nerd-fonts.jetbrains-mono (one hyphen is the dot).
    let dotted: Vec<String> = name.match_indices('-').map(|(i, _)| format!("{}.{}", &name[..i], &name[i + 1..])).filter(|a| index.get(a).is_ok()).collect();
    if !dotted.is_empty() {
        return dotted;
    }
    let max = if name.len() <= 4 { 1 } else { 2 };
    // (From the compact form: no package read whole.)
    let mut hits: Vec<(usize, bool, &str)> = index
        .records()
        .filter(|(_, a, _)| !a.contains('.') && a.len().abs_diff(name.len()) <= max)
        .filter_map(|(_, a, rec)| {
            let d = edit_distance(name, a);
            (d <= max).then_some((d, rec.flags & crate::index_file::APP == 0, a))
        })
        .collect();
    hits.sort();
    hits.into_iter().take(3).map(|(_, _, a)| a.to_string()).collect()
}

/// What a package of set `set` (python313Packages) misspelt might be: the
/// names in it a typo or two away, as they're written in the set (numpy).
pub(crate) fn set_typo_matches(index: &Index, set: &str, name: &str) -> Vec<String> {
    let max = if name.len() <= 4 { 1 } else { 2 };
    let prefix = format!("{set}.");
    let wanted = name.to_lowercase().replace('_', "-");
    let mut hits: Vec<(usize, &str)> = index
        .names()
        .filter_map(|a| a.strip_prefix(prefix.as_str()))
        .filter(|a| !a.contains('.') && a.len().abs_diff(name.len()) <= max)
        .filter_map(|a| {
            let d = edit_distance(&wanted, &a.to_lowercase());
            (d <= max).then_some((d, a))
        })
        .collect();
    hits.sort();
    hits.into_iter().take(3).map(|(_, a)| a.to_string()).collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != *cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The closest of `options` to `word`, if it's close enough to be a typo.
pub(crate) fn closest<'a>(word: &str, options: &[&'a str]) -> Option<&'a str> {
    fn dist(a: &str, b: &str) -> usize {
        let b: Vec<char> = b.chars().collect();
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.chars().enumerate() {
            let mut cur = vec![i + 1];
            for (j, cb) in b.iter().enumerate() {
                cur.push((prev[j] + usize::from(ca != *cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
            }
            prev = cur;
        }
        prev[b.len()]
    }
    options.iter().map(|o| (dist(word, o), *o)).filter(|(d, o)| *d <= 2.max(o.len() / 4)).min().map(|(_, o)| o)
}

/// Everything wrong with a nami file, sorted by line. `base` is where its
/// relative paths are relative to; without an index, package names aren't checked.
/// The file's text as a [`Config`], or what's wrong, where.
fn read_config(text: &str, name: &str, base: &Path) -> Result<Config, Diagnostic> {
    let typed = |e: serde_yaml::Error| {
        let (line, column) = e.location().map(|l| (l.line(), l.column())).unwrap_or((0, 0));
        let msg = e.to_string();
        let msg = msg.split(" at line ").next().unwrap_or(&msg).to_string();
        // `unknown field `pakages`, expected one of ...`: name the likely key.
        let hint = msg.starts_with("unknown field").then(|| unknown_key_hint(msg.split('`').nth(1).and_then(|bad| closest(bad, &[TOP_KEYS, HOME_KEYS].concat()))));
        let line = if line == 0 { msg.split('`').nth(1).map(|bad| find_line(text, None, bad)).unwrap_or(0) } else { line };
        Diagnostic { line, column, severity: Severity::Error, message: msg, hint }
    };
    let mut value: Value = crate::yaml_text::from_str(text).map_err(typed)?;
    modern(&mut value, "file").map_err(|e| {
        let msg = e.to_string().trim_start_matches("file: ").to_string();
        diag(find_line(text, None, msg.split(':').next().unwrap_or("")), Severity::Error, msg, None)
    })?;
    package_map(&mut value, "file").map_err(|e| {
        let msg = e.to_string().trim_start_matches("file: ").to_string();
        let key = msg.split(": ").nth(1).unwrap_or("packages").to_string();
        diag(find_line(text, Some("packages"), &key).max(find_line(text, None, "packages")), Severity::Error, msg, None)
    })?;
    if text.contains("!secret") {
        match crate::secrets::find(text) {
            Err(e) => return Err(diag(0, Severity::Error, format!("{e:#}"), None)),
            Ok(found) => {
                if let Some(f) = found.iter().find(|f| !f.sealed) {
                    return Err(diag(f.line, Severity::Error, format!("{}: this secret isn't sealed yet", f.path), Some("`nami secret seal <file>` encrypts it".into())));
                }
                if let Err(e) = crate::secrets::mark(&mut value, "file", crate::secrets::Kind::Nami) {
                    let line = found.iter().find(|s| e.to_string().contains(&format!(" {}:", s.path))).map(|s| s.line).unwrap_or(0);
                    return Err(diag(line, Severity::Error, e.to_string().trim_start_matches("file: ").to_string(), None));
                }
            }
        }
    }
    if value.is_null() {
        return Ok(Config::default());
    }
    // Wi-Fi networks, written out as files (a mistake in one: at `wifi:`).
    let value = crate::wifi::expand(value, "file").map_err(|e| {
        let line = text.lines().position(|l| l.starts_with("wifi:")).map_or(0, |i| i + 1);
        diag(line, Severity::Error, e.to_string().trim_start_matches("file: "), None)
    })?;
    let mut value = value;
    versioned_packages(&mut value, "file").map_err(|e| diag(find_line(text, None, "packages"), Severity::Error, e.to_string().trim_start_matches("file: "), None))?;
    resolve_names(&mut value, base);
    // Its keys as nami reads them (a key that's not for this file: at its line).
    let key_line = |e: anyhow::Error| {
        let msg = e.to_string();
        let msg = msg.strip_prefix(&format!("{name}: ")).unwrap_or(&msg).to_string();
        let key = msg.split(':').next().unwrap_or("").to_string();
        diag(find_line(text, None, &key), Severity::Error, msg, None)
    };
    let scope = Scope::of(name, &value);
    check_keys(&value, name, scope).map_err(key_line)?;
    let value = read_keys(value, name, base, scope).map_err(key_line)?;
    let mine = value.clone();
    let value = inside(value, &mine, name).map_err(|e| {
        let msg = e.to_string();
        let msg = msg.strip_prefix(&format!("{name}: ")).unwrap_or(&msg).to_string();
        let (msg, hint) = match msg.split_once(" (did you mean ").filter(|_| !msg.starts_with("unknown key")) {
            Some((m, h)) => (m.to_string(), Some(format!("did you mean {}", h.trim_end_matches(')')))),
            None => match msg.strip_prefix("unknown key ").and_then(|r| r.split_once(" (")) {
                Some((k, h)) => (format!("unknown key {k}"), Some(h.strip_suffix(')').unwrap_or(h).to_string())),
                None => (msg, None),
            },
        };
        let line = match msg.strip_prefix("files: ").and_then(|r| r.split(": ").next()) {
            Some(path) => find_line(text, Some("files"), path),
            None => find_line(text, None, msg.split('`').nth(1).or_else(|| msg.split(':').next()).unwrap_or("")),
        };
        diag(line, Severity::Error, msg, hint)
    })?;
    serde_yaml::from_value(value.clone()).map_err(|e| match wrong_key(&value) {
        // Parsed already, so serde has no line: the key whose value is
        // of the wrong kind (`users:` as a list) says where.
        Some(key) => {
            let msg = e.to_string();
            let msg = msg.split(" at line ").next().unwrap_or(&msg);
            diag(find_line(text, None, &key), Severity::Error, format!("{key}: {msg}"), None)
        }
        None => typed(e),
    })
}

/// The key whose value doesn't fit a [`Config`], each tried alone, and
/// within it the one that doesn't (`users`, under the `os` nami puts the
/// computer's keys in).
fn wrong_key(value: &Value) -> Option<String> {
    fn alone(path: &[Value], v: &Value) -> Value {
        path.iter().rev().fold(v.clone(), |inner, k| Value::Mapping([(k.clone(), inner)].into_iter().collect()))
    }
    let mut path: Vec<Value> = Vec::new();
    let mut here = value;
    loop {
        let found = here.as_mapping()?.iter().find(|(k, v)| {
            let mut p = path.clone();
            p.push((*k).clone());
            serde_yaml::from_value::<Config>(alone(&p, v)).is_err()
        });
        match found {
            Some((k, v)) if v.is_mapping() && path.is_empty() && k.as_str() == Some("os") => {
                path.push(k.clone());
                here = v;
            }
            Some((k, _)) => return k.as_str().map(String::from),
            None => return None,
        }
    }
}

/// `name` is the file's name: user.nami is a person's, any other a
/// computer's. What's written the old way is a warning here, at its line.
pub fn check(text: &str, name: &str, base: &Path, index: Option<&Index>) -> Vec<Diagnostic> {
    let mut out = quietly(|| check_file(text, name, base, index));
    if !out.iter().any(|d| d.message.contains(" is now ")) {
        out.extend(modern_notes(text));
    }
    for d in &mut out {
        d.message = plain_message(&d.message);
        if d.line == 0 {
            // A key the message starts with says where (`users.ada: ...`).
            let key = d.message.split([':', ' ', '.']).next().unwrap_or("");
            if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                d.line = find_line(text, None, key);
            }
        }
    }
    out.sort_by_key(|d| d.line);
    out
}

fn check_file(text: &str, name: &str, base: &Path, index: Option<&Index>) -> Vec<Diagnostic> {
    // A package.nami that builds: its recipe, and its setup as any package's.
    if is_package_nami(name) {
        if let Ok((Some(recipe), setup)) = crate::build::split_package(text) {
            let mut out = Vec::new();
            if let Err(e) = serde_yaml::from_value::<crate::build::Recipe>(Value::Mapping(recipe.clone())) {
                // (Each key alone says which one it is.)
                let key = recipe.iter().find(|(k, v)| serde_yaml::from_value::<crate::build::Recipe>(Value::Mapping([((*k).clone(), (*v).clone())].into_iter().collect())).is_err()).and_then(|(k, _)| k.as_str().map(String::from));
                let msg = e.to_string();
                let bad = msg.strip_prefix("unknown field `").and_then(|r| r.split('`').next()).map(String::from);
                let key = bad.clone().or(key).unwrap_or_default();
                let hint = bad.map(|b| unknown_key_hint(closest(&b, &crate::schema::recipe_keys())));
                let message = if msg.starts_with("unknown field") { format!("its recipe: unknown key `{key}`") } else { format!("its recipe: {key}: {}", plain_message(&msg)) };
                out.push(diag(find_line(text, None, &key), Severity::Error, message, hint));
            }
            if let Some(kind) = recipe.get("build-system").and_then(Value::as_str) {
                if let Err(e) = crate::build_kinds::Kind::parse(kind) {
                    out.push(diag(find_line(text, None, "build-system"), Severity::Error, format!("its recipe: {e}"), None));
                }
            }
            // An output written the earlier way (`libgcc^lib`).
            for key in ["deps", "build-deps", "runtime-deps"] {
                for d in recipe.get(key).and_then(Value::as_sequence).into_iter().flatten().filter_map(Value::as_str) {
                    if let Some(notice) = crate::build::old_output_spelling(d) {
                        out.push(diag(find_line(text, Some(key), d), Severity::Warning, format!("{key}: {notice}"), None));
                    }
                }
            }
            if !setup.is_empty() {
                out.extend(check(&serde_yaml::to_string(&setup).unwrap_or_default(), name, base, index));
            }
            return out;
        }
    } else if crate::project::has_project_keys(text) {
        return check_project(text, base, index);
    }
    let config = match read_config(text, name, base) {
        Ok(c) => c,
        Err(d) => return vec![d],
    };
    let mut out = Vec::new();

    // The whole of it: its sets, and nami's package files.
    let file = Path::new(name).file_name().map_or_else(|| name.to_string(), |f| f.to_string_lossy().into_owned());
    let whole = load_text(text, &file, base);
    if let Err(e) = &whole {
        let msg = e.to_string();
        let msg = msg.strip_prefix(&format!("{file}: ")).unwrap_or(&msg).to_string();
        // (A folder of its own list that isn't there is said already, on its line.)
        if !msg.starts_with("no package at ") {
            out.push(diag(find_line(text, None, "packages"), Severity::Error, msg, None));
        }
    }
    // Desktops its own packages bring (one that lists a login screen) are known too.
    let brought: Vec<String> = whole.ok().and_then(|c| c.os).map(|o| o.desktop).unwrap_or_default();
    if let Some(os) = &config.os {
        let mut os = os.clone();
        os.desktop.retain(|d| !brought.contains(d) || super::computer::desktops().contains(d));
        for p in super::computer::check(&os) {
            let key = p.split(':').next().unwrap_or("").trim_start_matches("os.").split('.').next().unwrap_or("").to_string();
            let line = find_line(text, None, &key).max(find_line(text, Some(&key), p.split(':').next().unwrap_or("").rsplit('.').next().unwrap_or("")));
            out.push(diag(line, Severity::Error, p, None));
        }
    }

    // In a config repo, a computer's folder is named after it.
    if crate::oldnames::current(&file) == SYSTEM_FILE && in_config_repo(base) && !base.ends_with(SHARED_DIR) {
        if let Some(folder) = std::fs::canonicalize(base).ok().and_then(|b| b.file_name().map(|f| f.to_string_lossy().into_owned())) {
            if let Some(host) = config.os.as_ref().and_then(|o| o.hostname.clone()).filter(|h| *h != folder) {
                out.push(diag(find_line(text, None, "hostname"), Severity::Error, format!("{folder}/{SYSTEM_FILE} says hostname: {host}: a computer's folder is named after it"), Some(format!("hostname: {folder}, or rename the folder"))));
            }
        }
    }

    check_packages(text, &config.packages, base, index, &mut out);

    // Software that isn't free, in a file that doesn't allow it.
    if config.unfree == Some(false) {
        for p in config.packages.iter().chain(config.os.iter().flat_map(|o| o.packages.iter())) {
            if index.and_then(|i| i.get(crate::index::split_version(p).0).ok()).is_some_and(|pkg| pkg.unfree) {
                out.push(diag(find_line(text, Some("packages"), crate::index::split_version(p).0), Severity::Error, format!("{p} isn't free software, and this file says unfree: false"), Some(format!("take {p} out, or unfree: false"))));
            }
        }
    }

    let scope = Scope::of(name, &crate::yaml_text::from_str(text).unwrap_or_default());
    check_place(text, config.os.as_ref(), base, index, &mut out);
    if scope == Scope::User {
        check_namios(text, &mut out);
    }

    for c in &config.caches {
        let line = find_line(text, Some("caches"), &c.url);
        let key_ok = c.key.split_once(':').is_some_and(|(n, k)| !n.is_empty() && k.len() == 44);
        if !key_ok {
            out.push(diag(find_line(text, Some("caches"), &c.key).max(line), Severity::Error, format!("cache {}: the key doesn't look like name:base64", c.url), Some("copy the public key `nami keygen` printed".into())));
        }
        let url_ok = ["https://", "http://", "file://", "/"].iter().any(|s| c.url.starts_with(s));
        if !url_ok {
            out.push(diag(line, Severity::Warning, format!("cache {}: not an http(s) URL or a directory", c.url), None));
        }
    }

    for (schema, keys) in config.settings.iter().flatten() {
        for (key, value) in keys {
            let line = find_line(text, Some(schema), key).max(find_line(text, Some("settings"), schema));
            if let Err(e) = crate::settings::gvariant(value) {
                out.push(diag(line, Severity::Error, format!("settings.{schema}.{key}: {e}"), None));
                continue;
            }
            match crate::settings::check_key(schema, key, value) {
                crate::settings::KeyCheck::SchemaMissing => out.push(diag(find_line(text, Some("settings"), schema), Severity::Warning, format!("settings: no schema {schema} on this computer"), Some("it may belong to an app that isn't installed here".into()))),
                crate::settings::KeyCheck::KeyMissing(keys) => {
                    let names: Vec<&str> = keys.iter().map(String::as_str).collect();
                    out.push(diag(line, Severity::Error, format!("settings: {schema} has no key {key}"), closest(key, &names).map(|k| format!("did you mean `{k}`?"))));
                }
                crate::settings::KeyCheck::NotAllowed(allowed) => out.push(diag(line, Severity::Error, format!("settings.{schema}.{key}: {} isn't one it takes", show_value(value)), Some(format!("one of: {}", allowed.join(", "))))),
                crate::settings::KeyCheck::Unknown | crate::settings::KeyCheck::Ok => {}
            }
        }
    }

    for (key, value) in &config.services {
        let Some(name) = key.as_str() else {
            out.push(diag(find_line(text, None, "services"), Severity::Error, "services: keys must be names", None));
            continue;
        };
        let line = find_line(text, Some("services"), name);
        match crate::services::parse(name, value) {
            Ok(s) => {
                // A person's services run as them (any user.nami, not only yours).
                if let Some(Err(e)) = (scope == Scope::User).then(|| crate::services::check_tier(&s, true)) {
                    if e.to_string().contains(".user:") {
                        out.push(diag(find_line(text, Some(name), "user").max(line), Severity::Error, e.to_string(), None));
                    }
                }
                if let (Some(p), Some(index)) = (s.package_name(), index) {
                    if index.get(&p).is_err() {
                        let close = typo_matches(index, &p);
                        let hint = if close.is_empty() { "write your own with `run:`, or name the package with `package:`".to_string() } else { format!("did you mean {}?", close.iter().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(" or ")) };
                        out.push(diag(line, Severity::Error, format!("services.{name}: no package {p} in the catalog to take its unit from"), Some(hint)));
                    }
                }
            }
            Err(e) => out.push(diag(line, Severity::Error, e.to_string(), None)),
        }
    }
    for (key, value) in &config.files {
        let Some(path) = key.as_str() else {
            out.push(diag(find_line(text, None, "files"), Severity::Error, "files: keys must be paths", None));
            continue;
        };
        let line = find_line(text, Some("files"), path);
        let secret = crate::secrets::contains(value);
        // A file in /run (on a computer nami made) is written as it starts, like one with a secret.
        let path = match if secret || path.starts_with("/run/") { crate::etc::check_secret_path(path) } else { crate::etc::check_path(path) } {
            Ok(p) => p,
            Err(e) => {
                out.push(diag(line, Severity::Error, e.to_string(), None));
                continue;
            }
        };
        if let Err(e) = crate::etc::owner_of(value).and_then(|o| {
            anyhow::ensure!(secret || o == (None, None), "owner: and group: are for files with a secret in them (the rest belong to root)");
            Ok(())
        }) {
            out.push(diag(line, Severity::Error, format!("files.{path}: {e}"), None));
            continue;
        }
        if let Err(e) = crate::etc::on_change_of(value) {
            out.push(diag(line, Severity::Error, format!("files.{path}: {e}"), None));
        }
        let rel = path.trim_start_matches("/etc/");
        match crate::etc::unwrap_options(value) {
            Err(e) => out.push(diag(line, Severity::Error, format!("files.{path}: {e}"), None)),
            Ok((Value::String(_), _)) if home::settings_only(rel) => out.push(diag(line, Severity::Error, format!("files.{path}: written as text"), Some(home::as_settings_hint(&path)))),
            Ok((Value::String(_), _)) => {}
            Ok((Value::Mapping(m), _)) if m.contains_key("source") => {
                if let Some(src) = m["source"].as_str().filter(|s| !base.join(s).exists()) {
                    out.push(diag(line, Severity::Error, format!("files.{path}: {src} doesn't exist"), None));
                }
            }
            Ok((ref content @ (Value::Mapping(_) | Value::Sequence(_)), _)) => {
                if let Err(e) = home::render(rel, content) {
                    out.push(diag(line, Severity::Error, format!("files.{path}: {e}"), None));
                }
            }
            Ok((other, _)) => out.push(diag(line, Severity::Error, format!("files.{path}: text, settings or {{ source: ... }}, not {}", show_value(&other)), None)),
        }
    }

    if config.history == Some(0) {
        out.push(diag(find_line(text, None, as_written(text, "history")), Severity::Warning, "history: 0 still keeps the current change, but none to go back to", Some("use at least 2 to be able to undo".into())));
    }

    if let Some(h) = &config.home {
        for k in h.env.keys() {
            let ok = k.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !ok {
                out.push(diag(find_line(text, Some("env"), k), Severity::Error, format!("env: {k:?} isn't a valid variable name"), Some("letters, digits and _ only, not starting with a digit".into())));
            }
        }
        for k in h.aliases.keys() {
            if k.is_empty() || k.contains(|c: char| c.is_whitespace() || "=/'\"$".contains(c)) {
                out.push(diag(find_line(text, Some("aliases"), k), Severity::Error, format!("aliases: {k:?} can't be an alias name"), None));
            }
        }
        for sh in h.shell.keys() {
            if !home::SHELLS.contains(&sh.as_str()) {
                let hint = closest(sh, home::SHELLS).map(|k| format!("did you mean `{k}`?")).unwrap_or_else(|| "bash, zsh or fish".into());
                out.push(diag(find_line(text, Some("shell"), sh), Severity::Error, format!("shell: {sh:?} isn't a shell nami knows"), Some(hint)));
            }
        }
        if let Some(sh) = h.login_shell.as_deref().filter(|s| !home::LOGIN_SHELLS.contains(s)) {
            let hint = if sh == "bash" { "bash is where terminals start: leave login-shell out".to_string() } else { "zsh or fish".to_string() };
            out.push(diag(find_line(text, None, "login-shell"), Severity::Error, format!("login-shell: {sh:?} isn't a shell nami can switch to"), Some(hint)));
        }
        for (key, value) in &h.files {
            let Some(rel) = key.as_str() else {
                out.push(diag(find_line(text, Some("files"), "files"), Severity::Error, "files: keys must be paths", None));
                continue;
            };
            let line = find_line(text, Some("files"), rel);
            let rel = match home::check_rel(rel) {
                Ok(r) => r,
                Err(e) => {
                    out.push(diag(line, Severity::Error, e.to_string(), Some("paths are relative to your home directory, like .config/app/settings.toml".into())));
                    continue;
                }
            };
            if crate::secrets::contains(value) {
                if let Err(e) = home::secret_file(&rel, value) {
                    out.push(diag(line, Severity::Error, format!("files: ~/{rel}: {e}"), None));
                }
                continue;
            }
            let (content, _, _) = match home::unwrap_options(value) {
                Ok(v) => v,
                Err(e) => {
                    out.push(diag(line, Severity::Error, format!("files: ~/{rel}: {e}"), None));
                    continue;
                }
            };
            match content {
                Value::String(_) if home::settings_only(&rel) => out.push(diag(line, Severity::Error, format!("files: ~/{rel}: written as text"), Some(home::as_settings_hint(&format!("~/{rel}"))))),
                Value::String(_) => {}
                Value::Mapping(m) if m.contains_key("source") => {
                    if let Some(src) = m["source"].as_str() {
                        if !base.join(src).exists() {
                            out.push(diag(find_line(text, Some(&rel), "source").max(line), Severity::Error, format!("files: ~/{rel}: {src} doesn't exist"), None));
                        }
                    }
                }
                Value::Mapping(_) | Value::Sequence(_) => {
                    if let Err(e) = home::render(&rel, content) {
                        out.push(diag(line, Severity::Error, format!("files: ~/{rel}: {e}"), None));
                    }
                }
                other => out.push(diag(line, Severity::Error, format!("files: ~/{rel}: text, settings or {{ source: ... }}, not {}", show_value(other)), None)),
            }
        }
    }
    out.sort_by_key(|d| d.line);
    out
}

/// The settings namiOS's desktop reads from `namios:` in a person's file
/// (nami-gui writes them; the desktop reads them as the file changes).
pub const NAMIOS_KEYS: &[&str] = &[
    "accessibility", "airpods", "all-apps", "appearance", "backups", "bar", "clipboard", "controls", "default-apps", "desktop", "displays", "dock",
    "hot-corners", "keyboard", "launcher", "lock", "minimize", "mouse", "night", "notifications", "nvidia", "plugins", "power", "screenshots",
    "shortcuts", "sound", "startup", "trackpad", "weather", "widgets", "windows",
];

/// `namios:` in a person's file: settings namiOS's desktop knows, and on
/// another distribution, that it's namiOS's.
fn check_namios(text: &str, out: &mut Vec<Diagnostic>) {
    let Some(namios) = crate::yaml_text::from_str(text).ok().and_then(|v| v.get("namios").cloned()) else { return };
    let line = find_line(text, None, "namios");
    let Some(m) = namios.as_mapping() else {
        if !namios.is_null() {
            out.push(diag(line, Severity::Error, "namios: the desktop's settings, by name (dock:, appearance:, ...)", None));
        }
        return;
    };
    // A yes or no is text: namiOS reads true or false.
    fn yes_no(v: &Value, at: &mut Vec<String>, out: &mut Vec<(String, String)>) {
        match v {
            Value::Mapping(m) => {
                for (k, v) in m {
                    at.push(k.as_str().unwrap_or_default().to_string());
                    yes_no(v, at, out);
                    at.pop();
                }
            }
            Value::String(s) if matches!(s.as_str(), "yes" | "no") => out.push((at.join("."), s.clone())),
            _ => {}
        }
    }
    let mut found = Vec::new();
    yes_no(&namios, &mut Vec::new(), &mut found);
    for (path, word) in found {
        let last = path.rsplit('.').next().unwrap_or(&path).to_string();
        out.push(diag(find_line(text, Some("namios"), &last).max(line), Severity::Warning, format!("namios: {path}: {word} is text, not true or false"), Some(format!("write {}", word == "yes"))));
    }
    for k in m.keys() {
        let Some(k) = k.as_str() else { continue };
        if !NAMIOS_KEYS.contains(&k) {
            let hint = closest(k, NAMIOS_KEYS).map(|c| format!("did you mean `{c}`?")).unwrap_or_else(|| "nami options namios lists them".into());
            out.push(diag(find_line(text, Some("namios"), k).max(line), Severity::Warning, format!("namios: {k}: not a setting namiOS's desktop has"), Some(hint)));
        }
    }
    if !crate::nami_os::here() {
        let here = if crate::index::MAC { "on a Mac" } else { "on this distribution" };
        out.push(diag(line, Severity::Note, format!("namios: is namiOS's desktop; {here} it does nothing"), None));
    }
}

/// The time zones there are: the computer's own zoneinfo (None: it has none to check with).
pub(super) fn time_zones() -> Option<(std::path::PathBuf, Vec<String>)> {
    let mut dirs: Vec<std::path::PathBuf> = std::env::var_os("TZDIR").map(std::path::PathBuf::from).into_iter().collect();
    dirs.extend(["/etc/zoneinfo", "/usr/share/zoneinfo"].map(std::path::PathBuf::from));
    let dir = dirs.into_iter().find(|d| d.join("zone1970.tab").is_file() || d.join("zone.tab").is_file())?;
    let tab = std::fs::read_to_string(dir.join("zone1970.tab")).or_else(|_| std::fs::read_to_string(dir.join("zone.tab"))).ok()?;
    let mut zones: Vec<String> = tab.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split('\t').nth(2)).map(String::from).collect();
    zones.push("UTC".into());
    Some((dir, zones))
}

/// The keyboard layouts there are (`us`, `dk`) and their variants
/// (`us(dvorak)`), from the computer's own xkeyboard-config.
pub(super) fn keyboard_layouts() -> Option<(Vec<String>, Vec<String>)> {
    let mut dirs: Vec<std::path::PathBuf> = std::env::var_os("XKB_CONFIG_ROOT").map(std::path::PathBuf::from).into_iter().collect();
    dirs.extend(["/run/current-system/sw/share/X11/xkb", "/usr/share/X11/xkb", "/etc/X11/xkb"].map(std::path::PathBuf::from));
    let text = dirs.iter().find_map(|d| std::fs::read_to_string(d.join("rules/base.lst")).ok())?;
    let (mut layouts, mut variants, mut section) = (Vec::new(), Vec::new(), String::new());
    for line in text.lines() {
        if let Some(s) = line.strip_prefix("! ") {
            section = s.trim().to_string();
            continue;
        }
        let Some((code, rest)) = line.trim().split_once(char::is_whitespace) else { continue };
        match section.as_str() {
            "layout" => layouts.push(code.to_string()),
            "variant" => {
                if let Some((layout, _)) = rest.trim().split_once(": ") {
                    variants.push(format!("{layout}({code})"));
                }
            }
            _ => {}
        }
    }
    Some((layouts, variants))
}

/// Where the computer is: its time zone, keyboard and language, and the
/// packages its `without:` leaves out, each one there is.
fn check_place(text: &str, os: Option<&super::computer::Os>, base: &Path, index: Option<&Index>, out: &mut Vec<Diagnostic>) {
    let Some(os) = os else { return };
    // `auto`: the zone where the computer is, found by itself.
    if let (Some(tz), Some((dir, zones))) = (os.timezone.as_deref().filter(|tz| *tz != "auto"), time_zones()) {
        if !dir.join(tz).is_file() && !zones.iter().any(|z| z == tz) {
            let names: Vec<&str> = zones.iter().map(String::as_str).collect();
            let hint = closest(tz, &names).map(|c| format!("did you mean `{c}`?")).unwrap_or_else(|| "a zone like Europe/Copenhagen or America/New_York".into());
            out.push(diag(find_line(text, None, "timezone"), Severity::Error, format!("timezone: {tz} isn't a time zone"), Some(hint)));
        }
    }
    if let (Some(kb), Some((layouts, variants))) = (os.keyboard.as_deref(), keyboard_layouts()) {
        for one in kb.split(',').map(str::trim) {
            let layout = one.split('(').next().unwrap_or(one);
            let known = if one.contains('(') { variants.iter().any(|v| v == one) } else { layouts.iter().any(|l| l == layout) };
            if !known {
                let names: Vec<&str> = if one.contains('(') { variants.iter().map(String::as_str).collect() } else { layouts.iter().map(String::as_str).collect() };
                let hint = closest(one, &names).map(|c| format!("did you mean `{c}`?")).unwrap_or_else(|| "a layout like us, dk or de, or one with its variant, us(dvorak)".into());
                out.push(diag(find_line(text, None, "keyboard"), Severity::Error, format!("keyboard: {one} isn't a keyboard layout"), Some(hint)));
            }
        }
    }
    if let Some(lang) = os.language.as_deref() {
        let (name, rest) = lang.split_once('.').unwrap_or((lang, ""));
        let (ll, cc) = name.split_once('_').unwrap_or((name, ""));
        let shaped = (2..=3).contains(&ll.len())
            && ll.chars().all(|c| c.is_ascii_lowercase())
            && (cc.is_empty() || (cc.len() == 2 && cc.chars().all(|c| c.is_ascii_uppercase())) || cc.split('@').next().is_some_and(|c| c.len() == 2 && c.chars().all(|c| c.is_ascii_uppercase())))
            && (rest.is_empty() || rest.split('@').next().is_some_and(|e| e.eq_ignore_ascii_case("utf-8") || e.eq_ignore_ascii_case("utf8")));
        let supported = std::fs::read_to_string("/usr/share/i18n/SUPPORTED").ok();
        let listed = supported.as_deref().is_none_or(|s| s.lines().any(|l| l.split_whitespace().next().is_some_and(|n| n.eq_ignore_ascii_case(lang) || n.replace(".UTF-8", "") == name)));
        if !shaped || !listed {
            out.push(diag(find_line(text, None, "language"), Severity::Error, format!("language: {lang} isn't a language nami knows"), Some("a locale like en_US.UTF-8 or da_DK.UTF-8".into())));
        }
    }
    // A package left out that nothing lists is likely a typo.
    let folders = packages_dir(base);
    for name in os.without.iter().filter(|n| !crate::services::UNIT_TYPES.iter().any(|t| n.ends_with(t))) {
        let folder = folders.as_ref().is_some_and(|d| crate::oldnames::is_file(&d.join(name).join("package.nami")));
        let catalog = index.is_some_and(|i| i.get(name).is_ok());
        // (Without the catalog, a name may be its package: only a folder is known.)
        let known = folder || catalog || is_package_file(name) || index.is_none();
        if !known {
            let mut names: Vec<String> = folders.iter().flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            let hint = closest(name, &refs).map(|c| format!("did you mean `{c}`?"));
            out.push(diag(find_line(text, Some("without"), name), Severity::Warning, format!("without: {name} isn't a package here"), hint));
        }
    }
}

/// What's wrong with the entries of a `packages:` list.
/// A package's output written the earlier way in `packages:`
/// (`util-linux^bin`): read as it's written now (`util-linux.bin`), with a
/// notice saying so.
pub(super) fn old_outputs(value: &mut Value, name: &str) {
    for p in value.get_mut("packages").and_then(Value::as_sequence_mut).into_iter().flatten() {
        let Some(old) = p.as_str().filter(|p| !p.contains([':', '/'])).map(String::from) else { continue };
        if let Some(notice) = crate::build::old_output_spelling(&old) {
            crate::say!("{name}: {notice}");
            *p = Value::String(old.replacen('^', ".", 1));
        }
    }
}

/// The 1-based line of `p`'s `nth` (0-based) listing in the file's
/// packages; 0 if it isn't there (a service's or a set's).
fn package_line(text: &str, p: &str, nth: usize) -> usize {
    // A version is written `nodejs: 22` in the file, and a folder found
    // by its name (packages/): find those then.
    // A Python with packages (python313 with numpy) by the Python it's written as.
    let python = crate::python_env::parse(p).map(|s| s.python);
    let line = [p, crate::index::split_version(p).0, p.rsplit('/').next().unwrap_or(p)]
        .into_iter()
        .chain(python.as_deref().into_iter().chain(python.is_some().then_some("python3")))
        .map(|w| find_nth_line(text, Some("packages"), w, nth))
        .find(|l| *l > 0)
        .unwrap_or(0);
    line
}

/// What `nami check` says of each of `names` the catalog doesn't have (or
/// not at that version), at its line in the file's `text`: what resolves
/// a file's packages says it so too, every one of them at once.
pub fn not_in_catalog(text: &str, names: &[&str], index: &Index) -> Vec<Diagnostic> {
    names
        .iter()
        .flat_map(|p| {
            let line = package_line(text, p, 0);
            match crate::python_env::parse(p) {
                Some(spec) => python_problems(index, &spec, line),
                None => check_name(index, p, line).into_iter().collect(),
            }
        })
        .filter(|d| d.severity == Severity::Error)
        .collect()
}

/// A Python with packages (`- python3: { with: [numpy] }`): each package its
/// set doesn't have, said with what it might be.
fn python_problems(index: &Index, spec: &crate::python_env::Spec, line: usize) -> Vec<Diagnostic> {
    crate::python_env::problems(index, spec).into_iter().map(|(m, h)| diag(line, Severity::Error, m, Some(h))).collect()
}

/// `file` as it's reached from the folder `cwd`: `project.nami`,
/// `../../project.nami` from a project's subfolder; elsewhere its whole path.
pub fn path_from(cwd: &Path, file: &Path) -> PathBuf {
    for (up, dir) in cwd.ancestors().enumerate().take(4) {
        if let (Ok(rest), false) = (file.strip_prefix(dir), dir.parent().is_none()) {
            return std::iter::repeat_n(Path::new(".."), up).collect::<PathBuf>().join(rest);
        }
    }
    file.to_path_buf()
}

/// The error for a file's `problems`, each line as `nami check` says it
/// (the file as it's reached from here).
pub fn file_problems(file: &Path, problems: &[Diagnostic]) -> anyhow::Error {
    let file = std::env::current_dir().map(|d| path_from(&d, file)).unwrap_or_else(|_| file.to_path_buf());
    let file = file.display().to_string();
    anyhow::anyhow!("{}", problems.iter().map(|d| d.shown(&file, crate::style::Paint(false))).collect::<Vec<_>>().join("\n"))
}

pub(super) fn check_packages(text: &str, packages: &[String], base: &Path, index: Option<&Index>, out: &mut Vec<Diagnostic>) {
    check_packages_in(text, packages, base, index, out, true)
}

/// `check_packages`; `casks`: a plain name can be one of Homebrew's casks on a Mac (a person's file or
/// the computer's, not a project's).
fn check_packages_in(text: &str, packages: &[String], base: &Path, index: Option<&Index>, out: &mut Vec<Diagnostic>, casks: bool) {
    let mut seen: std::collections::HashMap<&String, usize> = std::collections::HashMap::new();
    for p in packages {
        let n = seen.entry(p).or_insert(0);
        let line = package_line(text, p, *n);
        *n += 1;
        if *n > 1 {
            out.push(diag(line, Severity::Warning, format!("{p} is listed twice"), Some("remove one of them".into())));
            continue;
        }
        let (plain, version) = crate::index::split_version(p);
        if let Some(other) = version.and_then(|v| packages.iter().take_while(|q| *q != p).find_map(|q| crate::index::split_version(q).1.filter(|w| *w != v && crate::index::split_version(q).0 == plain))) {
            out.push(diag(line, Severity::Warning, format!("{plain} is listed at two versions, {other} and {}", version.unwrap_or_default()), Some("keep the one you want".into())));
            continue;
        }
        if let Some(id) = crate::flathub::id(p) {
            if !crate::flathub::valid_id(id) {
                out.push(diag(line, Severity::Error, format!("{p}: a Flathub app is named by its id, as flathub:org.gnome.Calculator"), Some("`nami search <word> --flathub` finds them".into())));
            }
        } else if crate::casks::is_cask(p) {
            if let Some((severity, message, hint)) = crate::casks::check_entry(p) {
                out.push(diag(line, severity, message, hint));
            }
        } else if crate::bundle_app::is_entry(p) {
            match crate::bundle_app::parse_entry(p) {
                None => out.push(diag(line, Severity::Error, format!("{p}: a bundle is bundle:github:<owner>/<repo>, or bundle:https://<the folder it updates from>"), None)),
                Some((_, None)) => out.push(diag(line, Severity::Warning, format!("{p}: no key: any key that signs it is trusted the first time"), Some("installing it in Settings writes its key here (`key: 3F2A …`)".into()))),
                Some(_) => {}
            }
        } else if p.starts_with("./") || p.starts_with("../") || p.starts_with('/') {
            let dir = base.join(p);
            if !crate::oldnames::exists(&dir.join("package.nami")) && !(dir.is_file() && crate::oldnames::has_nami_extension(&dir)) {
                out.push(diag(line, Severity::Error, format!("no package at {p}"), Some(format!("expected {}", dir.join("package.nami").display()))));
            }
        } else if is_set_file(p) {
            // a set from a repo: read (and its mistakes said) as the file loads
        } else if p.contains(':') && (p.ends_with(".nami") || p.contains(".nami@")) {
            out.push(diag(line, Severity::Error, format!("{p}: a set from a repo is github:owner/repo/<name>.nami[@ref], or git+https://host/repo.git[@ref]#<name>.nami"), None));
        } else if p.starts_with("github:") {
            if !p.contains('/') {
                out.push(diag(line, Severity::Error, format!("{p}: write it as github:owner/repo#tool"), None));
            }
        } else if let Some(spec) = crate::python_env::parse(p) {
            if let Some(index) = index {
                out.extend(python_problems(index, &spec, line));
            }
        } else if let Some(notice) = crate::build::old_output_spelling(p) {
            out.push(diag(line, Severity::Warning, notice, None));
        } else if let Some(index) = index.filter(|i| !is_set(p, i)) {
            if let Some(d) = check_name_in(index, p, line, casks) {
                out.push(d);
            }
        }
    }
}

/// A package no binary cache has: built here (said, as it takes a while),
/// or left out (said now, not halfway through a build).
fn how_its_had(pkg: &crate::index::Package, p: &str, line: usize) -> Option<Diagnostic> {
    // A Mac's catalog says exactly which aren't in the cache; nothing's built on a Mac.
    if crate::index::MAC {
        return (pkg.cache.is_none() && pkg.uncached).then(|| diag(line, Severity::Warning, format!("{p}: it'll be left out: it isn't ready-built for Mac"), Some("nami doesn't build packages on a Mac yet".into())));
    }
    if pkg.cache.is_some() || !(pkg.unfree || pkg.uncached) {
        return None;
    }
    let why = if pkg.unfree { "it isn't free software, so no binary cache has it" } else { "no binary cache has nixpkgs' build of it" };
    Some(match pkg.closure {
        Some(_) => diag(line, Severity::Note, format!("{p}: built on this computer the first time ({why}): a few minutes or more"), None),
        None => diag(line, Severity::Warning, format!("{p}: it'll be left out: {why}, and this catalog can't build it"), Some("a later catalog may have it".into())),
    })
}

/// A catalog name that isn't there (or not at that version): what to say.
fn check_name(index: &Index, p: &str, line: usize) -> Option<Diagnostic> {
    check_name_in(index, p, line, true)
}

fn check_name_in(index: &Index, p: &str, line: usize, casks: bool) -> Option<Diagnostic> {
    let (name, version) = crate::index::split_version(p);
    // A Mac: a name nixpkgs hasn't ready-built that's one of Homebrew's casks is that cask.
    if let Some((severity, message, hint)) = casks.then(|| crate::casks::check_name(index, p, name, version)).flatten() {
        return Some(diag(line, severity, message, hint));
    }
    if let (Some(_), Ok(_), Err(e)) = (version, index.get(name), index.get(p)) {
        // The package is there, not that version: say which there are.
        return Some(diag(line, Severity::Error, format!("{e:#}"), None));
    }
    // One of a package's outputs (util-linux.bin), as a computer takes it.
    if let Some((attr, output)) = p.rsplit_once('.').filter(|_| index.get(p).is_err()) {
        if let Ok(pkg) = index.get(attr) {
            let outputs: Vec<&str> = pkg.outputs.keys().map(String::as_str).collect();
            if !outputs.contains(&output) {
                return Some(diag(line, Severity::Error, format!("{attr} has no output {output} (it has: {})", outputs.join(", ")), None));
            }
            return how_its_had(pkg, p, line);
        }
    }
    if let Ok(pkg) = index.get(p) {
        return how_its_had(pkg, p, line);
    }
    index.get(p).is_err().then(|| {
        let close = typo_matches(index, p);
        let hint = if close.is_empty() {
            "search with `nami search <word> --all`".to_string()
        } else {
            format!("did you mean {}?", close.iter().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(" or "))
        };
        diag(line, Severity::Error, format!("{p} isn't in the catalog"), Some(hint))
    })
}

/// Everything wrong with a project's project.nami: its keys, packages and services.
pub fn check_project(text: &str, base: &Path, index: Option<&Index>) -> Vec<Diagnostic> {
    let yaml_error = |e: serde_yaml::Error| {
        let (line, column) = e.location().map(|l| (l.line(), l.column())).unwrap_or((0, 0));
        let msg = e.to_string();
        Diagnostic { line, column, severity: Severity::Error, message: msg.split(" at line ").next().unwrap_or(&msg).to_string(), hint: None }
    };
    let mut value: Value = match crate::yaml_text::from_str(text) {
        Ok(Value::Null) => Value::Mapping(Default::default()),
        Ok(v) => v,
        Err(e) => return vec![yaml_error(e)],
    };
    if let Err(e) = quietly(|| modern(&mut value, "file")) {
        let msg = e.to_string().trim_start_matches("file: ").to_string();
        return vec![diag(find_line(text, None, msg.split(':').next().unwrap_or("")), Severity::Error, msg, None)];
    }
    if let Err(e) = package_map(&mut value, "file") {
        return vec![diag(find_line(text, None, "packages"), Severity::Error, e.to_string().trim_start_matches("file: "), None)];
    }
    if value.get("files").is_some() {
        return vec![diag(find_line(text, None, "files"), Severity::Error, "files: writes /etc, so it's for the system file (/etc/nami/system.nami), not a project's", None)];
    }
    match crate::secrets::find(text) {
        Err(e) => return vec![diag(0, Severity::Error, format!("{e:#}"), None)],
        Ok(found) => {
            if let Some(f) = found.iter().find(|f| !f.sealed) {
                return vec![diag(f.line, Severity::Error, format!("{}: this secret isn't sealed yet", f.path), Some("`nami secret seal <file>` encrypts it".into()))];
            }
            if let Err(e) = crate::secrets::mark(&mut value, "file", crate::secrets::Kind::Project) {
                let line = found.iter().find(|s| e.to_string().contains(&format!(" {}:", s.path))).map(|s| s.line).unwrap_or(0);
                return vec![diag(line, Severity::Error, e.to_string().trim_start_matches("file: ").to_string(), None)];
            }
        }
    }
    if let Err(e) = versioned_packages(&mut value, "packages") {
        return vec![diag(find_line(text, None, "packages"), Severity::Error, e.to_string(), None)];
    }
    resolve_names(&mut value, base);
    // What it includes: each file and layer read, and the whole a project
    // (its services, from every layer, side by side).
    let mut out = Vec::new();
    let mut services_of_all = None;
    let mut sets = false;
    if let Some(list) = value.get_mut("packages").and_then(Value::as_sequence_mut) {
        let before = list.len();
        list.retain(|p| !p.as_str().is_some_and(is_set_file));
        sets = list.len() != before;
    }
    if sets {
        let whole = load_project(text, "file", base).and_then(|(v, _)| {
            anyhow::ensure!(v.get("files").is_none(), "files: writes /etc, so it's for the system file, not a project's");
            Ok(serde_yaml::from_value::<crate::project::Project>(v)?.services)
        });
        match whole {
            Ok(s) => services_of_all = Some(s),
            Err(e) => out.push(diag(find_line(text, None, "packages"), Severity::Error, format!("{e:#}").trim_start_matches("file: ").to_string(), None)),
        }
    }
    // Each task, at its line: a command, or `{ run, description, depends }`.
    if let Some(tasks) = value.get("tasks").and_then(Value::as_mapping) {
        let mut bad = Vec::new();
        for (k, v) in tasks {
            let name = crate::secrets::key_text(k);
            if let Err(e) = crate::project::Task::from_value(v) {
                bad.push(diag(find_line(text, Some("tasks"), &name).max(find_line(text, None, "tasks")), Severity::Error, format!("tasks: {name}: {e}"), None));
            }
        }
        if !bad.is_empty() {
            return bad;
        }
    } else if value.get("tasks").is_some_and(|t| !t.is_null()) {
        return vec![diag(find_line(text, None, "tasks"), Severity::Error, "tasks: a name and its command each, like `test: cargo test`", None)];
    }
    let project: crate::project::Project = match serde_yaml::from_value(value) {
        Ok(p) => p,
        Err(e) => {
            // `unknown field `servics``: name the key it's likely to be.
            let msg = e.to_string();
            let msg = msg.split(" at line ").next().unwrap_or(&msg).to_string();
            let bad = msg.split('`').nth(1).unwrap_or("");
            let hint = msg.starts_with("unknown field").then(|| unknown_key_hint(closest(bad, crate::project::KEYS)));
            // (No key named: the one it starts with, `env: ...`.)
            let at = if bad.is_empty() { msg.split(':').next().unwrap_or("") } else { bad };
            return vec![diag(find_line(text, None, at), Severity::Error, msg, hint)];
        }
    };
    for name in project.tasks.keys() {
        if let Err(e) = crate::project::task_order(&project.tasks, name) {
            out.push(diag(find_line(text, Some("tasks"), name), Severity::Error, format!("{e:#}"), None));
            break;
        }
    }
    check_packages_in(text, &project.packages, base, index, &mut out, false);
    for p in project.packages.iter().filter(|p| crate::flathub::is_flathub(p) || crate::bundle_app::is_entry(p) || crate::casks::is_cask(p)) {
        out.push(diag(find_line(text, Some("packages"), p), Severity::Error, format!("{p}: an app from a download is a person's (their user.nami), not a project's tool"), None));
    }
    for p in project.packages.iter().filter(|p| index.is_some_and(|i| is_set(p, i))) {
        out.push(diag(find_line(text, Some("packages"), p), Severity::Error, format!("{p}: a set for a computer (system.nami), not a project's tools"), None));
    }
    // A package folder that builds nothing sets the catalog's package up on a computer.
    for p in project.packages.iter().filter(|p| crate::build::Builder::is_recipe(p) && !is_set_file(p)) {
        let setup_only = std::fs::read_to_string(crate::oldnames::find(&base.join(p).join("package.nami"))).ok().and_then(|t| crate::build::split_package(&t).ok()).is_some_and(|(build, _)| build.is_none());
        if setup_only {
            out.push(diag(find_line(text, Some("packages"), p), Severity::Error, format!("{p}: a package for a computer (system.nami): it builds nothing, it sets a package up; a project lists its tools by name"), None));
        }
    }
    // A service's changes in the file can be to one a layer brings.
    match crate::up::resolve(services_of_all.as_ref().unwrap_or(&project.services), base) {
        Err(e) => {
            // `services.redis: ...`: on the service's line.
            let msg = format!("{e:#}");
            let name = msg.strip_prefix("services.").map(|m| match m.find(".nami") {
                Some(at) => &m[..at + 5],
                None => m.split([':', '.']).next().unwrap_or(m),
            }).unwrap_or("services");
            out.push(diag(find_line(text, Some("services"), name), Severity::Error, msg, None));
        }
        Ok(services) => {
            for s in &services {
                for p in s.packages.iter().filter(|_| index.is_some()) {
                    if let Some(mut d) = check_name_in(index.unwrap(), p, find_line(text, Some("services"), &s.entry), false) {
                        d.message = format!("services.{}: {}", s.entry, d.message);
                        out.push(d);
                    }
                }
            }
        }
    }
    out.sort_by_key(|d| d.line);
    out
}

/// Which file a nami file is, for the rules only one of them has.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tier {
    /// /etc/nami/system.nami, or any file root applies.
    System,
    /// Yours: ~/user.nami, or another file you apply.
    User,
}

/// `check`, plus what only the system or a user file may have.
pub fn check_tier(text: &str, base: &Path, index: Option<&Index>, tier: Tier) -> Vec<Diagnostic> {
    let name = match tier {
        Tier::System => SYSTEM_FILE,
        Tier::User => USER_FILE,
    };
    let mut out = check(text, name, base, index);
    let Ok(config) = read_config(text, name, base) else { return out };
    if tier == Tier::System {
        for p in config.packages.iter().filter(|p| crate::flathub::is_flathub(p)) {
            out.push(diag(find_line(text, Some("packages"), p), Severity::Error, format!("{p}: Flathub's apps are each person's own (flatpak --user)"), Some("list it in your ~/user.nami (Your Apps in Settings)".into())));
        }
        for p in config.packages.iter().filter(|p| crate::bundle_app::is_entry(p)) {
            out.push(diag(find_line(text, Some("packages"), p), Severity::Error, format!("{p}: a bundle is each person's own app"), Some("list it in your ~/user.nami (Your Apps in Settings)".into())));
        }
    }
    out.sort_by_key(|d| d.line);
    out
}
