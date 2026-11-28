//! Two versions of a nami file, said in words: what's added, taken off or changed,
//! by section ("Appearance: Blur: off → wallpaper", "Apps: + firefox"), and the raw
//! lines' diff for those who want it. And where a setting's line is in a file, for
//! "Show in file". Only std and serde_yaml: nami-gui has this file too (its History,
//! and each change Settings makes).

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_yaml::Value;

/// What happened to a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mark {
    Added,
    Removed,
    Changed,
}

/// One change: where it is (`["namios", "appearance", "blur"]`, a list's item by its
/// name: `["packages", "firefox"]`), and its value before and after (as written; none:
/// it wasn't there, or it's a list's item without a value of its own).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub path: Vec<String>,
    pub mark: Mark,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// A list's item (an app, a person), not a value.
    #[serde(default)]
    pub item: bool,
}

/// The changes in one part of the file: `packages` (Apps), `namios.dock` (Dock), ...
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub key: Vec<String>,
    pub title: String,
    pub rows: Vec<Row>,
}

/// One file's changes, in words and as raw lines.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileDiff {
    pub file: String,
    pub sections: Vec<Section>,
    /// The lines' unified diff (empty: the same text).
    pub raw: String,
    /// Going back to the earlier step puts these back (on a computer nami made, only
    /// its apps go back: the rest stays as the file says now).
    #[serde(default)]
    pub restores: Vec<String>,
    /// It couldn't be read as YAML: only `raw` says what changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreadable: Option<String>,
}

impl FileDiff {
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty() && self.raw.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Leaf {
    /// A list's item, without a value of its own (`- firefox`).
    Member,
    Value(String),
}

/// A value as written in the file, on one line.
fn scalar(v: &Value) -> String {
    match v {
        Value::Null => "~".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Tagged(t) if t.tag == "!secret" => "(a secret)".into(),
        other => {
            let text = serde_yaml::to_string(other).unwrap_or_default();
            let text = text.trim();
            // `{a: 1, b: 2}` in one line, as a flow map would be.
            match other {
                Value::Mapping(_) | Value::Sequence(_) => serde_json::to_string(&serde_yaml::from_str::<serde_json::Value>(text).unwrap_or_default()).unwrap_or_else(|_| text.replace('\n', " ")),
                _ => text.replace('\n', " "),
            }
        }
    }
}

/// A list's item's name and what it holds: `- firefox` (firefox), `- firefox: 150`
/// (firefox, 150), `- git: { ... }` (git, its map). None: not such an item.
fn item(v: &Value) -> Option<(String, Option<&Value>)> {
    match v {
        Value::String(_) | Value::Number(_) | Value::Bool(_) => Some((scalar(v), None)),
        Value::Mapping(m) if m.len() == 1 => {
            let (k, inner) = m.iter().next()?;
            matches!(k, Value::String(_) | Value::Number(_)).then(|| (scalar(k), Some(inner)))
        }
        _ => None,
    }
}

/// Every value in the file, by where it is, in the file's order.
fn leaves(v: &Value, path: &mut Vec<String>, out: &mut Vec<(Vec<String>, Leaf)>) {
    match v {
        Value::Mapping(m) if !m.is_empty() => {
            for (k, val) in m {
                path.push(scalar(k));
                leaves(val, path, out);
                path.pop();
            }
        }
        Value::Sequence(s) if !s.is_empty() && s.iter().all(|i| item(i).is_some()) => {
            for i in s {
                let (name, inner) = item(i).unwrap();
                path.push(name);
                match inner {
                    None => out.push((path.clone(), Leaf::Member)),
                    Some(Value::Mapping(m)) if !m.is_empty() => {
                        out.push((path.clone(), Leaf::Member));
                        leaves(&Value::Mapping(m.clone()), path, out);
                    }
                    Some(inner) => out.push((path.clone(), Leaf::Value(scalar(inner)))),
                }
                path.pop();
            }
        }
        Value::Tagged(t) if t.tag == "!secret" => out.push((path.clone(), Leaf::Value("(a secret)".into()))),
        other => {
            if !path.is_empty() {
                out.push((path.clone(), Leaf::Value(scalar(other))));
            }
        }
    }
}

/// Where a change goes: its section's key.
fn section_key(path: &[String]) -> Vec<String> {
    match path.first().map(String::as_str) {
        Some("namios") if path.len() > 2 => path[..2].to_vec(),
        Some("namios") => vec!["namios".into()],
        Some(k) if path.len() == 1 && !LISTS.contains(&k) => Vec::new(),
        Some(k) => vec![k.to_string()],
        None => Vec::new(),
    }
}

/// Keys that hold a list or a map of their own, a section each (the rest of the top
/// level is General's).
const LISTS: &[&str] = &["packages", "users", "files", "env", "aliases", "services", "home", "without", "wifi", "desktop", "namios", "include", "projects", "secrets"];

/// A section's name, in words.
pub fn section_title(key: &[String]) -> String {
    match key {
        [] => "General".into(),
        [k] => match k.as_str() {
            "packages" => "Apps".into(),
            "users" => "People".into(),
            "files" => "Files".into(),
            "env" => "Environment".into(),
            "aliases" => "Aliases".into(),
            "services" => "Services".into(),
            "home" => "Home folder files".into(),
            "without" => "Left out".into(),
            "wifi" => "Wi-Fi".into(),
            "desktop" => "Desktop".into(),
            "namios" => "namiOS".into(),
            other => words(other),
        },
        [_, k] => match k.as_str() {
            "hot-corners" => "Hot corners".into(),
            "bar" => "Menu bar".into(),
            "night" => "Night light".into(),
            other => words(other),
        },
        _ => key.last().map(|k| words(k)).unwrap_or_default(),
    }
}

/// A key in words: `text-size` → Text size, `hostname` → Name.
pub fn words(key: &str) -> String {
    let known = match key {
        "hostname" => Some("Name"),
        "timezone" => Some("Time zone"),
        "unfree" => Some("Apps that aren't free"),
        "keyboard" => Some("Keyboard"),
        "language" => Some("Language"),
        "on" => Some("On"),
        _ => None,
    };
    if let Some(k) = known {
        return k.to_string();
    }
    let spaced = key.replace(['-', '_'], " ");
    let mut c = spaced.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// A value in words: true is on, false off.
pub fn say(value: &str) -> String {
    match value {
        "true" => "on".into(),
        "false" => "off".into(),
        "~" | "" => "nothing".into(),
        other => other.to_string(),
    }
}

/// A row in words, without its section: `Blur: off → wallpaper`, `firefox`.
pub fn row_words(section: &[String], row: &Row) -> String {
    let rest = &row.path[section.len().min(row.path.len())..];
    // (A person's login, a file's path, a variable's name: as written.)
    let named = row.item || matches!(section.first().map(String::as_str), Some("users" | "files" | "env" | "aliases" | "wifi" | "services" | "packages" | "home" | "secrets"));
    let name = if rest.is_empty() { row.path.last().map(|k| if row.item { k.clone() } else { words(k) }).unwrap_or_default() } else { rest.iter().enumerate().map(|(i, k)| if i == 0 && named { k.clone() } else { words(k) }).collect::<Vec<_>>().join(" › ") };
    match (row.mark, &row.from, &row.to) {
        (Mark::Changed, Some(f), Some(t)) => format!("{name}: {} → {}", say(f), say(t)),
        (Mark::Added, _, Some(t)) | (Mark::Removed, Some(t), _) => format!("{name}: {}", say(t)),
        _ => name,
    }
}

/// What changed from `before` to `after` (texts of nami files), by section in the
/// order they're in the file.
pub fn changes(before: &str, after: &str) -> Result<Vec<Section>, String> {
    let read = |t: &str| -> Result<Vec<(Vec<String>, Leaf)>, String> {
        let v: Value = if t.trim().is_empty() { Value::Null } else { serde_yaml::from_str(t).map_err(|e| e.to_string())? };
        let mut out = Vec::new();
        leaves(&v, &mut Vec::new(), &mut out);
        Ok(out)
    };
    let (was, now) = (read(before)?, read(after)?);
    let (was_at, now_at): (std::collections::HashMap<&[String], &Leaf>, std::collections::HashMap<&[String], &Leaf>) = (was.iter().map(|(p, l)| (p.as_slice(), l)).collect(), now.iter().map(|(p, l)| (p.as_slice(), l)).collect());
    let find = |list: &std::collections::HashMap<&[String], &Leaf>, p: &[String]| list.get(p).map(|l| (*l).clone());
    // (An item come or gone says so once: not each value under it too.)
    let under = |p: &[String], items: &[Vec<String>]| items.iter().any(|i| p.len() > i.len() && p.starts_with(i));
    let mut rows: Vec<Row> = Vec::new();
    let mut gone_items: Vec<Vec<String>> = Vec::new();
    let mut new_items: Vec<Vec<String>> = Vec::new();
    for (p, leaf) in &now {
        if under(p, &new_items) {
            continue;
        }
        let value = |l: &Leaf| match l {
            Leaf::Value(v) => Some(v.clone()),
            Leaf::Member => None,
        };
        match find(&was_at, p) {
            None => {
                let item = matches!(leaf, Leaf::Member) || p.len() >= 2 && is_list_item(&now, p);
                if item {
                    new_items.push(p.clone());
                }
                rows.push(Row { path: p.clone(), mark: Mark::Added, from: None, to: value(leaf), item });
            }
            Some(old) if old != *leaf => {
                let item = is_list_item(&now, p);
                rows.push(Row { path: p.clone(), mark: Mark::Changed, from: value(&old).or(Some(String::new())), to: value(leaf).or(Some(String::new())), item });
            }
            _ => {}
        }
    }
    for (p, leaf) in &was {
        if under(p, &gone_items) || find(&now_at, p).is_some() {
            continue;
        }
        let item = matches!(leaf, Leaf::Member) || is_list_item(&was, p);
        if item {
            gone_items.push(p.clone());
        }
        let value = match leaf {
            Leaf::Value(v) => Some(v.clone()),
            Leaf::Member => None,
        };
        // (Where it was: after what's before it in the file.)
        rows.push(Row { path: p.clone(), mark: Mark::Removed, from: value, to: None, item });
    }
    let mut sections: Vec<Section> = Vec::new();
    for r in rows {
        let key = section_key(&r.path);
        match sections.iter_mut().find(|s| s.key == key) {
            Some(s) => s.rows.push(r),
            None => sections.push(Section { title: section_title(&key), key, rows: vec![r] }),
        }
    }
    Ok(sections)
}

/// Whether `p` is a list's item with a value of its own (`- firefox: 150`): a
/// Member marks those with a map; this, those with a plain value, by their parent
/// holding other members.
fn is_list_item(all: &[(Vec<String>, Leaf)], p: &[String]) -> bool {
    p.first().is_some_and(|k| k == "packages") && p.len() == 2 || all.iter().any(|(q, l)| *l == Leaf::Member && q.as_slice() == p)
}

/// The whole of one file's changes: in words, and its raw lines.
pub fn file(name: &str, before: &str, after: &str, before_label: &str, after_label: &str) -> FileDiff {
    let raw = unified(before, after, &format!("{before_label}/{name}"), &format!("{after_label}/{name}"));
    match changes(before, after) {
        Ok(sections) => FileDiff { file: name.into(), sections, raw, restores: Vec::new(), unreadable: None },
        Err(e) => FileDiff { file: name.into(), sections: Vec::new(), raw, restores: Vec::new(), unreadable: Some(e) },
    }
}

/// The file's changes, printed as `nami diff` says them.
pub fn print(d: &FileDiff, out: &mut String) {
    use std::fmt::Write;
    let _ = writeln!(out, "{}", d.file);
    if let Some(e) = &d.unreadable {
        let _ = writeln!(out, "  (can't be read as YAML: {e}; `--raw` shows its lines)");
    }
    for s in &d.sections {
        let back = if d.restores.is_empty() || d.restores.iter().any(|r| *r == s.key.join(".")) { "" } else { "  (stays as it is now)" };
        let _ = writeln!(out, "  {}{back}", s.title);
        for r in &s.rows {
            let mark = match r.mark {
                Mark::Added => "+",
                Mark::Removed => "−",
                Mark::Changed => " ",
            };
            let _ = writeln!(out, "    {mark} {}", row_words(&s.key, r));
        }
    }
}

/// The lines' unified diff (3 lines around each change), as `diff -u` has it.
pub fn unified(before: &str, after: &str, a: &str, b: &str) -> String {
    let x: Vec<&str> = before.lines().collect();
    let y: Vec<&str> = after.lines().collect();
    if x == y {
        return String::new();
    }
    // What's the same at the start and the end (most of a big file: a computer's flat
    // one is thousands of lines) is left out of the work.
    let (n, m) = (x.len(), y.len());
    let head = x.iter().zip(&y).take_while(|(a, b)| a == b).count();
    let tail = x[head..].iter().rev().zip(y[head..].iter().rev()).take_while(|(a, b)| a == b).count();
    let (xm, ym) = (&x[head..n - tail], &y[head..m - tail]);
    // Each line: (' ', '-', '+'), its line in x and y.
    let mut ops: Vec<(char, usize, usize)> = (0..head).map(|i| (' ', i, i)).collect();
    let (p, q) = (xm.len(), ym.len());
    match myers(xm, ym, 4000) {
        Some(steps) => ops.extend(steps.into_iter().map(|(op, i, j)| (op, head + i, head + j))),
        None => {
            // (Too much changed to line up: all of it out, and in.)
            ops.extend((0..p).map(|i| ('-', head + i, head)));
            ops.extend((0..q).map(|j| ('+', head + p, head + j)));
        }
    }
    ops.extend((0..tail).map(|k| (' ', n - tail + k, m - tail + k)));
    const AROUND: usize = 3;
    let changed: Vec<usize> = (0..ops.len()).filter(|&k| ops[k].0 != ' ').collect();
    // Hunks: each change with AROUND lines either side, those that touch as one.
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for &k in &changed {
        let (from, to) = (k.saturating_sub(AROUND), (k + AROUND + 1).min(ops.len()));
        match hunks.last_mut() {
            Some(h) if from <= h.1 => h.1 = to,
            _ => hunks.push((from, to)),
        }
    }
    let mut out = format!("--- {a}\n+++ {b}\n");
    for (from, to) in hunks {
        let hunk = &ops[from..to];
        let (xs, ys) = (hunk[0].1, hunk[0].2);
        let xl = hunk.iter().filter(|o| o.0 != '+').count();
        let yl = hunk.iter().filter(|o| o.0 != '-').count();
        out.push_str(&format!("@@ -{},{xl} +{},{yl} @@\n", xs + 1, ys + 1));
        for &(op, i, j) in hunk {
            out.push(op);
            out.push_str(if op == '+' { y[j] } else { x[i] });
            out.push('\n');
        }
    }
    out
}

/// Myers' diff of two lists of lines: each (' ', '-', '+') with where it is in each. Quick
/// when little differs (O((n+m)·d)); None past `most` differences.
fn myers(x: &[&str], y: &[&str], most: usize) -> Option<Vec<(char, usize, usize)>> {
    let (n, m) = (x.len() as isize, y.len() as isize);
    let max = (n + m) as usize;
    let off = max as isize + 1;
    let mut v = vec![0isize; 2 * max + 3];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = None;
    for d in 0..=max.min(most) as isize {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let down = k == -d || k != d && v[(off + k - 1) as usize] < v[(off + k + 1) as usize];
            let mut i = if down { v[(off + k + 1) as usize] } else { v[(off + k - 1) as usize] + 1 };
            let mut j = i - k;
            while i < n && j < m && x[i as usize] == y[j as usize] {
                i += 1;
                j += 1;
            }
            v[(off + k) as usize] = i;
            if i >= n && j >= m {
                found = Some(d);
                break;
            }
            k += 2;
        }
        if found.is_some() {
            break;
        }
    }
    let d_end = found?;
    // Back from the end, through each d's furthest reaches.
    let mut out = Vec::new();
    let (mut i, mut j) = (n, m);
    for d in (1..=d_end).rev() {
        let v = &trace[d as usize];
        let k = i - j;
        let down = k == -d || k != d && v[(off + k - 1) as usize] < v[(off + k + 1) as usize];
        let pk = if down { k + 1 } else { k - 1 };
        let pi = v[(off + pk) as usize];
        let pj = pi - pk;
        while i > pi + if down { 0 } else { 1 } && j > pj + if down { 1 } else { 0 } {
            i -= 1;
            j -= 1;
            out.push((' ', i as usize, j as usize));
        }
        if down {
            j -= 1;
            out.push(('+', i as usize, j as usize));
        } else {
            i -= 1;
            out.push(('-', i as usize, j as usize));
        }
    }
    while i > 0 && j > 0 {
        i -= 1;
        j -= 1;
        out.push((' ', i as usize, j as usize));
    }
    out.reverse();
    Some(out)
}

/// A line's key, if it has one (`blur: off` → blur, `"a b": 1` → a b), and what's
/// after its colon.
fn key_of(rest: &str) -> Option<(String, &str)> {
    let bytes = rest.as_bytes();
    if let Some(q @ (b'"' | b'\'')) = bytes.first().copied() {
        let close = rest[1..].find(q as char)? + 1;
        let after = rest[close + 1..].trim_start();
        return after.strip_prefix(':').map(|v| (rest[1..close].to_string(), v.trim()));
    }
    let mut at = 0;
    while let Some(c) = rest[at..].find(':') {
        let i = at + c;
        let next = rest[i + 1..].chars().next();
        if next.is_none() || next == Some(' ') || next == Some('\t') {
            let key = rest[..i].trim();
            if key.starts_with('#') || key.is_empty() || key.starts_with('{') || key.starts_with('[') {
                return None;
            }
            return Some((key.to_string(), rest[i + 1..].trim()));
        }
        at = i + 1;
    }
    None
}

/// Without its comment: `firefox  # a browser` → firefox.
fn uncommented(v: &str) -> &str {
    let mut quote = None;
    for (i, c) in v.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '#') if i == 0 || v[..i].ends_with(' ') || v[..i].ends_with('\t') => return v[..i].trim_end(),
            _ => {}
        }
    }
    v.trim_end()
}

fn unquoted(v: &str) -> &str {
    let v = v.trim();
    if v.len() >= 2 && (v.starts_with('"') && v.ends_with('"') || v.starts_with('\'') && v.ends_with('\'')) { &v[1..v.len() - 1] } else { v }
}

/// Where `path` is in a nami file's text: its line (1-based), and how much of the
/// path that line is (all of it, or the deepest part there is: a setting not in the
/// file yet is its section's line, or none). A list's item is its name:
/// `["packages", "gimp"]` is `  - gimp`.
pub fn line_of(text: &str, path: &[&str]) -> Option<(usize, usize)> {
    if path.is_empty() {
        return None;
    }
    // (indent, key) of what each line is in.
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut best: Option<(usize, usize)> = None;
    let mut block: Option<usize> = None;
    for (n, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed == "---" {
            continue;
        }
        // Inside a `key: |` text: not keys of its own.
        if let Some(b) = block {
            if indent > b {
                continue;
            }
            block = None;
        }
        let mut at = indent;
        let mut rest = trimmed;
        let mut entries: Vec<(usize, String, &str)> = Vec::new();
        // `- name`, `- name: v`, `- - x`: each dash an item.
        loop {
            if let Some(r) = rest.strip_prefix("- ").or_else(|| (rest == "-").then_some("")) {
                let after = r.trim_start();
                let inner = at + (rest.len() - after.len());
                match key_of(after) {
                    // `- git:` (an item by its name) or `- name: x` as a map: its key.
                    Some((k, v)) => {
                        entries.push((at, k, v));
                        break;
                    }
                    None if after.starts_with("- ") => {
                        at = inner;
                        rest = after;
                    }
                    None => {
                        entries.push((at, unquoted(uncommented(after)).to_string(), ""));
                        break;
                    }
                }
            } else {
                if let Some((k, v)) = key_of(rest) {
                    entries.push((at, k, v));
                }
                break;
            }
        }
        for (ind, key, value) in entries {
            while stack.last().is_some_and(|(i, _)| *i >= ind) {
                stack.pop();
            }
            stack.push((ind, key));
            let v = uncommented(value);
            if v.starts_with('|') || v.starts_with('>') {
                block = Some(indent);
            }
            let depth = stack.len();
            if depth <= path.len() && stack.iter().zip(path).all(|((_, k), p)| k == p) && best.is_none_or(|(_, d)| depth > d) {
                best = Some((n + 1, depth));
                if depth == path.len() {
                    return best;
                }
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = "# Ada's own\npackages:\n  - ripgrep\n  - firefox: 150   # pinned\n  - git:\n      ~/.gitconfig:\n        user: { name: A }\nnamios:\n  appearance:\n    look: dark\n    blur: wallpaper  # see-through\n  dock:\n    on: true\n  power:\n    quiet:\n      when: hours\nfiles:\n  \"/etc/x.conf\": |\n    blur: not a key\n  /etc/y.conf:\n    a: 1\n";

    #[test]
    fn finds_a_settings_line() {
        assert_eq!(line_of(USER, &["namios", "appearance", "blur"]), Some((11, 3)));
        assert_eq!(line_of(USER, &["namios", "appearance", "look"]), Some((10, 3)));
        assert_eq!(line_of(USER, &["namios", "dock", "on"]), Some((13, 3)));
        assert_eq!(line_of(USER, &["namios", "power", "quiet", "when"]), Some((16, 4)));
    }

    #[test]
    fn a_setting_not_there_is_its_sections_line() {
        assert_eq!(line_of(USER, &["namios", "dock", "size"]), Some((12, 2)));
        assert_eq!(line_of(USER, &["namios", "weather", "on"]), Some((8, 1)));
        assert_eq!(line_of(USER, &["timezone"]), None);
        assert_eq!(line_of("", &["namios", "appearance", "blur"]), None);
    }

    #[test]
    fn finds_a_lists_item() {
        assert_eq!(line_of(USER, &["packages", "ripgrep"]), Some((3, 2)));
        assert_eq!(line_of(USER, &["packages", "firefox"]), Some((4, 2)));
        assert_eq!(line_of(USER, &["packages", "git"]), Some((5, 2)));
        assert_eq!(line_of(USER, &["packages", "git", "~/.gitconfig"]), Some((6, 3)));
        assert_eq!(line_of(USER, &["packages", "gimp"]), Some((2, 1)));
    }

    #[test]
    fn quoted_keys_and_texts() {
        assert_eq!(line_of(USER, &["files", "/etc/x.conf"]), Some((18, 2)));
        // (`blur:` inside the text isn't a key.)
        assert_eq!(line_of(USER, &["files", "/etc/y.conf", "a"]), Some((21, 3)));
        assert_eq!(line_of(USER, &["files", "/etc/x.conf", "blur"]), Some((18, 2)));
    }

    #[test]
    fn a_people_section() {
        let text = "hostname: box\nusers:\n  ann:\n    admin: true\n  bo:\n    child: true\n    without: [firefox]\n";
        assert_eq!(line_of(text, &["hostname"]), Some((1, 1)));
        assert_eq!(line_of(text, &["users", "bo", "child"]), Some((6, 3)));
        assert_eq!(line_of(text, &["users", "ann", "child"]), Some((3, 2)));
    }

    #[test]
    fn flow_and_comment_lines() {
        let text = "namios:   # the desktop\n  # appearance: comment\n  bar: { clock: 24 }\n  appearance:\n    blur: off\n";
        assert_eq!(line_of(text, &["namios", "appearance", "blur"]), Some((5, 3)));
        assert_eq!(line_of(text, &["namios", "bar", "clock"]), Some((3, 2)));
    }

    #[test]
    fn a_change_in_words() {
        let before = "hostname: box\nnamios:\n  appearance:\n    blur: off\n  dock:\n    on: true\npackages:\n  - gimp\n  - firefox: 149\n";
        let after = "hostname: box\nnamios:\n  appearance:\n    blur: wallpaper\n  dock:\n    on: false\npackages:\n  - firefox: 150\n  - ripgrep\n";
        let s = changes(before, after).unwrap();
        let said: Vec<(String, Vec<String>)> = s.iter().map(|s| (s.title.clone(), s.rows.iter().map(|r| format!("{:?} {}", r.mark, row_words(&s.key, r))).collect())).collect();
        assert_eq!(
            said,
            vec![
                ("Appearance".to_string(), vec!["Changed Blur: off → wallpaper".to_string()]),
                ("Dock".to_string(), vec!["Changed On: on → off".to_string()]),
                ("Apps".to_string(), vec!["Changed firefox: 149 → 150".to_string(), "Added ripgrep".to_string(), "Removed gimp".to_string()]),
            ]
        );
    }

    #[test]
    fn general_and_people() {
        let before = "hostname: box\ntimezone: Europe/Berlin\nusers:\n  ann:\n    admin: true\n";
        let after = "hostname: box\ntimezone: Europe/Copenhagen\nusers:\n  ann:\n    admin: true\n  bo:\n    child: true\n";
        let s = changes(before, after).unwrap();
        assert_eq!(s[0].title, "General");
        assert_eq!(row_words(&s[0].key, &s[0].rows[0]), "Time zone: Europe/Berlin → Europe/Copenhagen");
        assert_eq!(s[1].title, "People");
        assert_eq!(row_words(&s[1].key, &s[1].rows[0]), "bo › Child: on");
        assert_eq!(s[1].rows[0].mark, Mark::Added);
    }

    #[test]
    fn an_item_gone_says_so_once() {
        let before = "packages:\n  - git:\n      ~/.gitconfig:\n        user: { name: A }\n  - ripgrep\n";
        let after = "packages:\n  - ripgrep\n";
        let s = changes(before, after).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].rows.len(), 1);
        assert_eq!(row_words(&s[0].key, &s[0].rows[0]), "git");
        assert_eq!(s[0].rows[0].mark, Mark::Removed);
    }

    #[test]
    fn comments_alone_are_no_change() {
        let before = "namios:\n  appearance:\n    blur: off  # system.nami\n";
        let after = "# Generated\nnamios:\n  appearance:\n    blur: off  # ../shared/user.nami\n";
        assert!(changes(before, after).unwrap().is_empty());
        assert!(!unified(before, after, "a", "b").is_empty());
    }

    #[test]
    fn the_raw_diff() {
        let before = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
        let after = "a\nb\nc\nD\ne\nf\ng\nh\ni\nj\nk\n";
        let d = unified(before, after, "change 3/user.nami", "now/user.nami");
        assert!(d.starts_with("--- change 3/user.nami\n+++ now/user.nami\n@@ -1,"), "{d}");
        assert!(d.contains("-d\n+D\n"), "{d}");
        assert!(d.contains("+k\n"), "{d}");
        assert_eq!(unified(before, before, "a", "b"), "");
    }

    #[test]
    fn a_big_file_is_quick() {
        let before: String = (0..6000).map(|i| format!("k{i}: {i}\n")).collect();
        let after = before.replace("k3000: 3000", "k3000: changed");
        let t = std::time::Instant::now();
        let d = file("system.nami", &before, &after, "a", "b");
        assert!(t.elapsed().as_secs() < 2);
        assert_eq!(d.sections[0].rows.len(), 1);
        assert!(d.raw.contains("-k3000: 3000\n+k3000: changed\n"), "{}", d.raw);
    }

    #[test]
    fn a_big_file_changed_at_both_ends() {
        let mut before: String = (0..6000).map(|i| format!("k{i}: {i}\n")).collect();
        before.insert_str(0, "# from a\n");
        let after = before.replace("# from a", "# from b").replace("k3000: 3000", "k3000: changed") + "end: 1\n";
        let d = unified(&before, &after, "a", "b");
        assert_eq!(d.lines().filter(|l| l.starts_with('-') && !l.starts_with("---")).count(), 2, "{d}");
        assert_eq!(d.lines().filter(|l| l.starts_with('+') && !l.starts_with("+++")).count(), 3, "{d}");
        assert_eq!(d.matches("@@ -").count(), 3, "{d}");
    }

    #[test]
    fn printed() {
        let mut out = String::new();
        print(&file("system.nami", "unfree: true\n", "unfree: false\n", "change 2", "now"), &mut out);
        assert_eq!(out, "system.nami\n  General\n      Apps that aren't free: on → off\n");
    }

    #[test]
    fn not_yaml_is_raw_only() {
        let d = file("user.nami", "a: 1\n", "a: [\n", "x", "y");
        assert!(d.unreadable.is_some());
        assert!(!d.raw.is_empty());
    }
}
