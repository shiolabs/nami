//! Settings files apps may edit (`mutable: true`): what an app changed,
//! setting by setting, and merging that with changes to the spec.
//!
//! Every format nami writes settings in (JSON, TOML, YAML, INI, gitconfig,
//! .env) is read back into the same YAML values the spec uses, so a file
//! and the spec can be compared key by key. Values compare by how they'd be
//! written, so INI's `size = 14` equals the spec's `size: 14`.

use anyhow::Result;
use serde::Serialize;
use serde_yaml::{Mapping, Value};

/// A file's contents as settings, or None for formats nami writes as plain
/// text (those only ever compare as a whole).
pub fn parse(rel: &str, text: &str) -> Result<Option<Value>> {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let git = name == ".gitconfig" || rel.ends_with("git/config");
    Ok(Some(match ext.as_str() {
        _ if git => ini(text, true)?,
        "json" => serde_yaml::to_value(serde_json::from_str::<serde_json::Value>(&strip_json_comments(text))?)?,
        "toml" => serde_yaml::to_value(toml::from_str::<toml::Value>(text)?)?,
        "yaml" | "yml" => serde_yaml::from_str(text)?,
        "ini" | "conf" | "cfg" => ini(text, false)?,
        "env" => env(text),
        _ => return Ok(None),
    }))
}

/// VS Code and others write JSON with comments and trailing commas (JSONC).
fn strip_json_comments(text: &str) -> String {
    let (mut out, mut chars) = (String::new(), text.chars().peekable());
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                out.extend(chars.next());
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_str = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                while chars.next_if(|&c| c != '\n').is_some() {}
            }
            ('/', Some('*')) => {
                chars.next();
                while let Some(c) = chars.next() {
                    if c == '*' && chars.next_if_eq(&'/').is_some() {
                        break;
                    }
                }
            }
            (',', _) => {
                // drop a trailing comma before } or ]
                let rest: String = chars.clone().collect();
                if !rest.trim_start().starts_with(['}', ']']) {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

fn ini(text: &str, git: bool) -> Result<Value> {
    let mut root = Mapping::new();
    let mut section: Option<(String, Option<String>)> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(h) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let h = h.trim();
            section = Some(match h.split_once(char::is_whitespace) {
                Some((s, sub)) if git => (s.to_string(), Some(sub.trim().trim_matches('"').to_string())),
                _ => match h.split_once('.').filter(|_| !git) {
                    Some((s, sub)) => (s.to_string(), Some(sub.to_string())),
                    None => (h.to_string(), None),
                },
            });
            continue;
        }
        let (k, v) = line.split_once('=').map(|(k, v)| (k.trim(), v.trim())).unwrap_or((line, "true"));
        let v = unquote(v);
        let target = match &section {
            None => &mut root,
            Some((s, sub)) => {
                let s = child(&mut root, s);
                match sub {
                    Some(sub) => child(s, sub),
                    None => s,
                }
            }
        };
        let key = Value::String(k.to_string());
        match target.get_mut(&key) {
            Some(Value::Sequence(list)) => list.push(Value::String(v)),
            Some(old) => *old = Value::Sequence(vec![old.clone(), Value::String(v)]),
            None => {
                target.insert(key, Value::String(v));
            }
        }
    }
    Ok(Value::Mapping(root))
}

fn child<'a>(m: &'a mut Mapping, k: &str) -> &'a mut Mapping {
    let key = Value::String(k.to_string());
    if !m.get(&key).is_some_and(Value::is_mapping) {
        m.insert(key.clone(), Value::Mapping(Mapping::new()));
    }
    m.get_mut(&key).and_then(Value::as_mapping_mut).unwrap()
}

fn unquote(v: &str) -> String {
    match v.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        Some(inner) => inner.replace("\\\"", "\"").replace("\\\\", "\\"),
        None => v.to_string(),
    }
}

fn env(text: &str) -> Value {
    let mut m = Mapping::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        if let Some((k, v)) = line.trim_start_matches("export ").split_once('=') {
            m.insert(Value::String(k.trim().into()), Value::String(unquote(v.trim())));
        }
    }
    Value::Mapping(m)
}

/// Two values the same setting? Scalars compare as they'd be written.
pub fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Mapping(x), Value::Mapping(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w))),
        (Value::Sequence(x), Value::Sequence(y)) => x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same(v, w)),
        (Value::Mapping(_) | Value::Sequence(_), _) | (_, Value::Mapping(_) | Value::Sequence(_)) => false,
        (x, y) => text(x) == text(y),
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Tagged(t) => text(&t.value),
        other => serde_yaml::to_string(other).unwrap_or_default().trim().to_string(),
    }
}

/// One setting that differs.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Change {
    /// Keys from the top of the file down, e.g. ["editor.fontSize"].
    pub path: Vec<String>,
    /// What the spec says (None: the spec doesn't have it).
    pub from: Option<Value>,
    /// What the file says now (None: it was removed).
    pub to: Option<Value>,
}

/// The settings that differ between the spec's version and the file.
pub fn diff(spec: &Value, file: &Value) -> Vec<Change> {
    let mut out = Vec::new();
    walk(&mut Vec::new(), Some(spec), Some(file), &mut out);
    out
}

fn walk(path: &mut Vec<String>, a: Option<&Value>, b: Option<&Value>, out: &mut Vec<Change>) {
    match (a, b) {
        (Some(Value::Mapping(x)), Some(Value::Mapping(y))) => {
            let keys: Vec<&Value> = x.keys().chain(y.keys().filter(|k| !x.contains_key(*k))).collect();
            for k in keys {
                path.push(text(k));
                walk(path, x.get(k), y.get(k), out);
                path.pop();
            }
        }
        (Some(x), Some(y)) if same(x, y) => {}
        (None, None) => {}
        // A whole section added or removed: list its settings one by one.
        (None, Some(Value::Mapping(y))) => {
            for (k, v) in y {
                path.push(text(k));
                walk(path, None, Some(v), out);
                path.pop();
            }
        }
        (Some(Value::Mapping(x)), None) => {
            for (k, v) in x {
                path.push(text(k));
                walk(path, Some(v), None, out);
                path.pop();
            }
        }
        (a, b) => out.push(Change { path: path.clone(), from: a.cloned(), to: b.cloned() }),
    }
}

/// Three-way merge of a settings file: `base` is the spec's version the
/// file was written from, `ours` the spec's new version, `theirs` the file
/// as the app left it. Each setting takes whichever side changed it; when
/// both changed the same one differently, the spec wins and its path is
/// returned as a conflict.
pub fn merge(base: &Value, ours: &Value, theirs: &Value) -> (Value, Vec<Vec<String>>) {
    let mut conflicts = Vec::new();
    let v = merge_at(&mut Vec::new(), Some(base), Some(ours), Some(theirs), &mut conflicts);
    (v.unwrap_or(Value::Mapping(Mapping::new())), conflicts)
}

fn merge_at(path: &mut Vec<String>, base: Option<&Value>, ours: Option<&Value>, theirs: Option<&Value>, conflicts: &mut Vec<Vec<String>>) -> Option<Value> {
    let eq = |a: Option<&Value>, b: Option<&Value>| match (a, b) {
        (Some(x), Some(y)) => same(x, y),
        (None, None) => true,
        _ => false,
    };
    if let (Some(Value::Mapping(o)), Some(Value::Mapping(t))) = (ours, theirs) {
        let b = base.and_then(Value::as_mapping);
        let mut out = Mapping::new();
        // Keep the spec's order, then whatever only the app added.
        let keys: Vec<&Value> = o.keys().chain(t.keys().filter(|k| !o.contains_key(*k))).collect();
        for k in keys {
            path.push(text(k));
            if let Some(v) = merge_at(path, b.and_then(|b| b.get(k)), o.get(k), t.get(k), conflicts) {
                out.insert(k.clone(), v);
            }
            path.pop();
        }
        return Some(Value::Mapping(out));
    }
    if eq(theirs, base) {
        ours.cloned()
    } else if eq(ours, base) || eq(ours, theirs) {
        theirs.cloned()
    } else {
        conflicts.push(path.clone());
        ours.cloned()
    }
}

/// A setting's value from a file, typed like the spec's value it replaces
/// when it's text that reads as one (INI and .env only have text).
pub fn typed_like(value: &Value, like: Option<&Value>) -> Value {
    match (value, like) {
        (Value::String(s), Some(Value::Bool(_))) => s.parse::<bool>().map(Value::Bool).unwrap_or_else(|_| value.clone()),
        (Value::String(s), Some(Value::Number(_))) => serde_yaml::from_str::<Value>(s).ok().filter(Value::is_number).unwrap_or_else(|| value.clone()),
        _ => value.clone(),
    }
}

/// The nami file path of a setting in a file: `files."~/<rel>"`, plus
/// `content` when the file is written `{ mutable: true, content: ... }`.
/// Where a user.nami (`spec`) sets ~/`rel`: its own `files:`, or under a
/// package (`- vscode: { ~/.config/Code/User/settings.json: ... }`); the
/// path to it (for [`crate::config_edit`]) and what it says.
pub fn spec_entry<'a>(spec: &'a Value, rel: &str) -> Option<(Vec<String>, &'a Value)> {
    let key = format!("~/{rel}");
    if let Some(v) = spec.get("files").and_then(|f| f.get(key.as_str())) {
        return Some((vec!["files".into(), key], v));
    }
    spec.get("packages")?.as_sequence()?.iter().find_map(|p| {
        let (name, setup) = p.as_mapping().filter(|m| m.len() == 1)?.iter().next()?;
        let v = setup.get(key.as_str())?;
        Some((vec!["packages".into(), format!("- {}", name.as_str()?), key.clone()], v))
    })
}

/// The path to `key` in ~/`rel`'s settings, at `at` (from [`spec_entry`]).
pub fn spec_path(at: &[String], wrapped: bool, key: &[String]) -> Vec<String> {
    let mut p = at.to_vec();
    if wrapped {
        p.push("content".into());
    }
    p.extend(key.iter().cloned());
    p
}

pub fn describe(c: &Change) -> String {
    let show = |v: &Option<Value>| v.as_ref().map(text).unwrap_or_else(|| "(not set)".into());
    format!("{}: {} -> {}", c.path.join("."), show(&c.from), show(&c.to))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn y(s: &str) -> Value {
        serde_yaml::from_str(s).unwrap()
    }

    #[test]
    fn diff_finds_changed_settings() {
        let spec = y("editor.fontSize: 14\nworkbench.colorTheme: Dark\n");
        let file = parse("settings.json", "{\n  // mine\n  \"editor.fontSize\": 16,\n  \"workbench.colorTheme\": \"Dark\",\n  \"files.autoSave\": \"on\",\n}\n").unwrap().unwrap();
        let d = diff(&spec, &file);
        assert_eq!(d.len(), 2);
        assert_eq!(describe(&d[0]), "editor.fontSize: 14 -> 16");
        assert_eq!(describe(&d[1]), "files.autoSave: (not set) -> on");
    }

    #[test]
    fn a_file_under_a_package_is_found() {
        let spec: Value = serde_yaml::from_str("files:\n  ~/.gitconfig: { user: { name: J } }\npackages:\n  - git\n  - vscode:\n      ~/.config/Code/User/settings.json: { mutable: true, content: { a: 1 } }\n").unwrap();
        let (at, v) = spec_entry(&spec, ".config/Code/User/settings.json").unwrap();
        assert_eq!(at, ["packages", "- vscode", "~/.config/Code/User/settings.json"]);
        assert_eq!(spec_path(&at, v.get("content").is_some(), &["a".into()]), ["packages", "- vscode", "~/.config/Code/User/settings.json", "content", "a"]);
        assert_eq!(spec_entry(&spec, ".gitconfig").unwrap().0, ["files", "~/.gitconfig"]);
        assert!(spec_entry(&spec, ".nope").is_none());
    }

    #[test]
    fn ini_values_compare_with_typed_spec_values() {
        let spec = y("user: { name: Alice }\ncore: { autocrlf: false }\nremote: { origin: { url: x } }\n");
        let file = parse(".gitconfig", "[user]\n\tname = Alice\n[core]\n\tautocrlf = false\n[remote \"origin\"]\n\turl = x\n").unwrap().unwrap();
        assert!(diff(&spec, &file).is_empty());
    }

    #[test]
    fn merge_keeps_both_sides() {
        let base = y("a: 1\nb: 1\nc: 1\n");
        let ours = y("a: 2\nb: 1\nc: 5\n"); // spec changed a and c
        let theirs = y("a: 1\nb: 3\nc: 6\nd: 4\n"); // app changed b and c, added d
        let (m, conflicts) = merge(&base, &ours, &theirs);
        assert_eq!(m, y("a: 2\nb: 3\nc: 5\nd: 4\n"));
        assert_eq!(conflicts, vec![vec!["c".to_string()]]);
    }
}
