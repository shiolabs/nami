//! The whole computer in one file: `nami show --flat`, and
//! /etc/nami/computer.flat.nami on a computer nami makes. It's the system
//! file merged with everything it includes, as [`crate::config`] merges
//! them, and each value says which file it came from.
//!
//! While a flat file is made, `config` records each file's own values
//! (after its includes, so the list goes from the file that gives way to
//! the one that wins). A value comes from the last file that has it; a
//! list's item from the last file whose list has it.

use std::cell::RefCell;

use serde_yaml::Value;

thread_local! {
    /// Each file's own values, lowest first, while a flat file is made.
    static SOURCES: RefCell<Option<Vec<(String, Value)>>> = const { RefCell::new(None) };
}

pub(crate) fn recording() -> bool {
    SOURCES.with(|s| s.borrow().is_some())
}

/// A file's own values (`source` names the file), on top of those so far.
pub(crate) fn record(source: &str, own: &Value) {
    SOURCES.with(|s| {
        if let Some(list) = s.borrow_mut().as_mut() {
            list.push((source.to_string(), own.clone()));
        }
    })
}

/// The last file recorded whose `packages:` lists `item`.
pub(crate) fn lister(item: &str) -> Option<String> {
    SOURCES.with(|s| {
        let s = s.borrow();
        s.as_ref()?.iter().rev().find(|(_, v)| v.get("packages").and_then(Value::as_sequence).is_some_and(|l| l.iter().any(|p| p.as_str() == Some(item)))).map(|(n, _)| n.clone())
    })
}

/// How many files are recorded so far.
pub(crate) fn mark() -> usize {
    SOURCES.with(|s| s.borrow().as_ref().map_or(0, Vec::len))
}

/// The files recorded since `from` are forgotten (read again).
pub(crate) fn forget(from: usize) {
    SOURCES.with(|s| {
        if let Some(list) = s.borrow_mut().as_mut() {
            list.truncate(from);
        }
    })
}

/// The files recorded since `from` go underneath all the others (they're
/// merged underneath what's there: base, desktops, packages' files).
pub(crate) fn to_front(from: usize) {
    SOURCES.with(|s| {
        if let Some(list) = s.borrow_mut().as_mut() {
            let n = list.len() - from.min(list.len());
            list.rotate_right(n);
        }
    })
}

/// `f`, with the files it reads recorded: what it returns, and the files'
/// own values, lowest first.
pub(crate) fn recorded<T>(f: impl FnOnce() -> T) -> (T, Vec<(String, Value)>) {
    SOURCES.with(|s| *s.borrow_mut() = Some(Vec::new()));
    let out = f();
    (out, SOURCES.with(|s| s.borrow_mut().take().unwrap_or_default()))
}

#[derive(Clone, Copy)]
enum Step<'a> {
    Key(&'a Value),
    Item(&'a Value),
}

/// Does a file's own value have what's at `path`? (A file given as text
/// is its `content:`.)
fn has(v: &Value, path: &[Step]) -> bool {
    match path.split_first() {
        None => true,
        Some((Step::Key(k), rest)) => match v {
            Value::Mapping(m) => m.get(*k).is_some_and(|c| has(c, rest)),
            Value::String(_) => k.as_str() == Some("content") && rest.is_empty(),
            _ => false,
        },
        Some((Step::Item(i), _)) => v.as_sequence().is_some_and(|s| s.contains(i)),
    }
}

struct Writer<'a> {
    sources: &'a [(String, Value)],
    out: String,
}

impl Writer<'_> {
    /// The file the value at `path` came from: the last that has it (or,
    /// for a value nami made, like a file's options, what it's under).
    fn blame(&self, path: &[Step]) -> &str {
        self.winner(path).map_or("?", |i| &self.sources[i].0)
    }

    fn winner(&self, path: &[Step]) -> Option<usize> {
        (0..=path.len()).rev().find_map(|len| self.sources.iter().rposition(|(_, v)| has(v, &path[..len])))
    }

    /// A map's keys, the winning file's first: yours on top, then what it
    /// includes, down to base.
    fn ordered<'v>(&self, m: &'v serde_yaml::Mapping, path: &mut Vec<Step<'v>>) -> Vec<(&'v Value, &'v Value)> {
        let mut keys: Vec<(&Value, &Value)> = m.iter().collect();
        keys.sort_by_key(|(k, _)| {
            path.push(Step::Key(k));
            let w = self.winner(path);
            path.pop();
            std::cmp::Reverse(w)
        });
        keys
    }

    fn line(&mut self, text: &str, path: &[Step]) {
        let src = self.blame(path).to_string();
        self.out.push_str(text);
        self.out.push_str("  # ");
        self.out.push_str(&src);
        self.out.push('\n');
    }

    /// `v` after `lead`: a key (`  name:`), or a list's dash (`  - `, the
    /// value goes on the same line). `ind`: where what's inside it starts.
    fn node<'v>(&mut self, lead: &str, ind: usize, v: &'v Value, path: &mut Vec<Step<'v>>) {
        let item = lead.ends_with("- ");
        let sep = if item { "" } else { " " };
        let pad = " ".repeat(ind);
        // Everyone can read the flat file; a password's hash is only for
        // /etc/shadow, as on Ubuntu.
        if let [Step::Key(u), _, Step::Key(p)] = path.as_slice() {
            if u.as_str() == Some("users") && p.as_str() == Some("password") {
                return self.line(&format!("{lead}{sep}(a hash; not shown)"), path);
            }
        }
        match v {
            // Services as written: `- name`, `- name:` with its setup (a
            // service's own files show under files:, as they're written).
            Value::Mapping(m) if !m.is_empty() && matches!(path.as_slice(), [Step::Key(k)] if k.as_str() == Some("services")) => {
                self.out.push_str(lead);
                self.out.push('\n');
                for (k, val) in self.ordered(m, path) {
                    path.push(Step::Key(k));
                    let setup: Option<serde_yaml::Mapping> = val.as_mapping().map(|m| m.iter().filter(|(k, _)| k.as_str() != Some("restart-on")).map(|(k, v)| (k.clone(), v.clone())).collect());
                    match setup.filter(|m| !m.is_empty()) {
                        None => self.line(&format!("{pad}- {}", scalar(k)), path),
                        Some(_) => {
                            let lead = format!("{pad}- {}:", scalar(k));
                            self.out.push_str(&lead);
                            self.out.push('\n');
                            let inner = " ".repeat(ind + 4);
                            for (sk, sv) in self.ordered(val.as_mapping().unwrap(), path) {
                                if sk.as_str() == Some("restart-on") {
                                    continue;
                                }
                                path.push(Step::Key(sk));
                                self.node(&format!("{inner}{}:", scalar(sk)), ind + 6, sv, path);
                                path.pop();
                            }
                        }
                    }
                    path.pop();
                }
            }
            Value::Mapping(m) if !m.is_empty() => {
                if !item {
                    self.out.push_str(lead);
                    self.out.push('\n');
                }
                for (i, (k, val)) in self.ordered(m, path).into_iter().enumerate() {
                    let key = format!("{}{}:", if item && i == 0 { lead } else { &pad }, scalar(k));
                    path.push(Step::Key(k));
                    self.node(&key, ind + 2, val, path);
                    path.pop();
                }
            }
            // The file's packages, as written: `- name`, `- name: version`.
            Value::Sequence(s) if !s.is_empty() && matches!(path.as_slice(), [Step::Key(k)] if k.as_str() == Some("packages")) => {
                self.out.push_str(lead);
                self.out.push('\n');
                for it in s {
                    let entry = match it.as_str().map(crate::index::split_version) {
                        Some((name, Some(v))) => format!("{pad}- {name}: {v}"),
                        Some((name, None)) => format!("{pad}- {name}"),
                        // `- firefox: { replace: { libpng: ./fix } }`
                        None => match it.as_mapping().and_then(|m| m.iter().next()) {
                            Some((name, own)) => format!("{pad}- {}: {}", scalar(name), flow(own)),
                            None => format!("{pad}- {}", scalar(it)),
                        },
                    };
                    path.push(Step::Item(it));
                    self.line(&entry, path);
                    path.pop();
                }
            }
            Value::Sequence(s) if !s.is_empty() => {
                if !item {
                    self.out.push_str(lead);
                    self.out.push('\n');
                }
                for (i, it) in s.iter().enumerate() {
                    let dash = format!("{}- ", if item && i == 0 { lead } else { &pad });
                    path.push(Step::Item(it));
                    self.node(&dash, dash.len(), it, path);
                    path.pop();
                }
            }
            Value::String(s) if block(s) => {
                let trailing = s.len() - s.trim_end_matches('\n').len();
                let chomp = match trailing {
                    0 => "-",
                    1 => "",
                    _ => "+",
                };
                self.line(&format!("{lead}{sep}|{chomp}"), path);
                let body = if trailing > 0 { &s[..s.len() - 1] } else { &s[..] };
                for l in body.split('\n') {
                    if !l.is_empty() {
                        self.out.push_str(&pad);
                        self.out.push_str(l);
                    }
                    self.out.push('\n');
                }
            }
            Value::Tagged(t) => {
                let text = format!("{lead}{sep}{}", serde_yaml::to_string(&Value::Tagged(t.clone())).unwrap_or_default().trim_end());
                self.line(&text, path);
            }
            _ => self.line(&format!("{lead}{sep}{}", scalar(v)), path),
        }
    }
}

/// A text that reads as it is as a block (`|`): lines, no other control
/// characters, nothing a block would change.
fn block(s: &str) -> bool {
    s.contains('\n')
        && !s.trim().is_empty()
        && !s.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
        && !s.split('\n').any(|l| !l.is_empty() && l.trim_matches(' ').is_empty())
        && s.split('\n').find(|l| !l.is_empty()).is_some_and(|l| !l.starts_with([' ', '\t']))
}

/// A map on one line, as YAML's `{ a: b }`.
fn flow(v: &Value) -> String {
    match v {
        Value::Mapping(m) => format!("{{ {} }}", m.iter().map(|(k, v)| format!("{}: {}", scalar(k), flow(v))).collect::<Vec<_>>().join(", ")),
        _ => scalar(v),
    }
}

/// One value on one line, as YAML. A secret stays sealed.
fn scalar(v: &Value) -> String {
    match v {
        Value::Mapping(_) => "{}".into(),
        Value::Sequence(_) => "[]".into(),
        Value::String(s) if crate::secrets::marked(s).is_some() => format!("!secret {}", serde_json::to_string(crate::secrets::marked(s).unwrap()).unwrap()),
        Value::String(s) if s.contains('\n') => serde_json::to_string(s).unwrap(),
        _ => serde_yaml::to_string(v).unwrap_or_default().trim_end_matches('\n').to_string(),
    }
}

/// `merged` as YAML, each value's line ending in the file it came from
/// (`sources`: each file's own values, lowest first).
pub(crate) fn text(merged: &Value, sources: &[(String, Value)]) -> String {
    let mut w = Writer { sources, out: String::new() };
    match merged {
        Value::Mapping(m) => {
            for (k, v) in w.ordered(m, &mut Vec::new()) {
                let mut path = vec![Step::Key(k)];
                w.node(&format!("{}:", scalar(k)), 2, v, &mut path);
            }
        }
        _ => w.node("", 0, merged, &mut Vec::new()),
    }
    w.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn y(s: &str) -> Value {
        serde_yaml::from_str(s).unwrap()
    }

    #[test]
    fn each_value_says_its_file() {
        let sources = vec![
            ("packages/base/package.nami".to_string(), y("{ hostname: nami, timezone: UTC, packages: [bash, sudo] }")),
            ("system.nami".to_string(), y("{ hostname: laptop, packages: [git, bash, firefox@150] }")),
        ];
        let merged = y("{ hostname: laptop, timezone: UTC, packages: [git, bash, firefox@150, sudo] }");
        let t = text(&merged, &sources);
        assert!(t.contains("hostname: laptop  # system.nami\n"), "{t}");
        assert!(t.contains("timezone: UTC  # packages/base/package.nami\n"), "{t}");
        assert!(t.contains("  - git  # system.nami\n"), "{t}");
        assert!(t.contains("  - bash  # system.nami\n"), "{t}");
        assert!(t.contains("  - firefox: 150  # system.nami\n"), "{t}");
        assert!(t.contains("  - sudo  # packages/base/package.nami\n"), "{t}");
    }

    #[test]
    fn text_is_a_block_and_secrets_stay_sealed() {
        let unit = "[Service]\nExecStart=/bin/true # not a comment\n\n  indented\n";
        let mut merged = Value::Mapping(Default::default());
        merged.as_mapping_mut().unwrap().insert("units".into(), y("{}"));
        merged["units"].as_mapping_mut().unwrap().insert("a.service".into(), unit.into());
        let sealed = format!("{}YWdlLWVuY3J5cHRpb24ub3JnL3YxCi0+IFgyNTUxOSBhYmMKZGVm", crate::secrets::MARK);
        merged.as_mapping_mut().unwrap().insert("wifi".into(), y("{ home: { password: x } }"));
        merged["wifi"]["home"]["password"] = sealed.clone().into();
        let sources = vec![("layers/x.nami".to_string(), merged.clone())];
        let t = text(&merged, &sources);
        assert!(t.contains("  a.service: |  # layers/x.nami\n    [Service]\n"), "{t}");
        assert!(t.contains("    password: !secret \"YWdl"), "{t}");
        let back = y(&t);
        assert_eq!(back["units"], merged["units"]);
        // A user's password hash isn't for everyone's eyes.
        let merged = y("users: { ada: { admin: true, password: \"$6$salt$hash\" } }");
        let t = text(&merged, &[("system.nami".to_string(), merged.clone())]);
        assert!(t.contains("password: (a hash; not shown)") && !t.contains("$6$"), "{t}");
    }

    #[test]
    fn file_options_come_from_the_file() {
        let sources = vec![
            ("layers/a.nami".to_string(), y("files: { /etc/x: \"hi\\n\" }")),
            ("system.nami".to_string(), y("files: { /etc/x: { restart: x.service } }")),
        ];
        let merged = y("files: { /etc/x: { content: \"hi\\n\", restart: x.service } }");
        let t = text(&merged, &sources);
        assert!(t.contains("content: \"hi\\n\"  # layers/a.nami") || t.contains("content: |  # layers/a.nami"), "{t}");
        assert!(t.contains("restart: x.service  # system.nami"), "{t}");
    }
}
