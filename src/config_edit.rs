//! Edit nami file as text, keeping comments, order and layout.
//!
//! `nami install`, `nami remove` and `nami config ...`
//! change a config through these operations. Each edit touches only the lines
//! involved, then is checked: the new text must parse to exactly what the
//! operation means. If a layout is too unusual to edit in place (flow-style
//! maps on the path, anchors...), it refuses instead of guessing.

use anyhow::{anyhow, bail, ensure, Result};
use serde::Deserialize;
use serde_yaml::{Mapping, Value};

/// A path's step `- name` is the list item `- name:` with its setup under
/// it (`packages`, `- vscode`, `~/.config/Code/User/settings.json`).
fn item_name(key: &str) -> Option<&str> {
    key.strip_prefix("- ")
}

/// The setup of item `- name:` in a list.
fn item_mut<'a>(list: &'a mut [Value], name: &str) -> Option<&'a mut Value> {
    list.iter_mut().find_map(|v| v.as_mapping_mut().filter(|m| m.len() == 1)?.get_mut(name))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    /// Set the value at a path (creating parents), replacing what was there.
    Set { path: Vec<String>, value: Value },
    /// Remove the key at a path (and parents left empty).
    Unset { path: Vec<String> },
    /// Append to the list at a path (creating it), unless already there.
    Add { path: Vec<String>, value: Value },
    /// Remove an item from the list at a path.
    Remove { path: Vec<String>, value: Value },
}

impl Op {
    /// The edit with its value's numbers and yes/no as text (see [`document`]).
    fn as_text(&self) -> Op {
        match self.clone() {
            Op::Set { path, value } => Op::Set { path, value: as_text(value) },
            Op::Add { path, value } => Op::Add { path, value: as_text(value) },
            Op::Remove { path, value } => Op::Remove { path, value: as_text(value) },
            op => op,
        }
    }

    /// An edit from `nami config json`: a value `{"!secret": "..."}` is a
    /// secret (`!secret "..."`, plain until `nami secret seal`).
    pub fn from_json(text: &str) -> Result<Op> {
        let mut op: Op = serde_json::from_str(text).map_err(|e| anyhow!("bad edit JSON: {e}"))?;
        if let Op::Set { value, .. } | Op::Add { value, .. } | Op::Remove { value, .. } = &mut op {
            tag_secrets(value);
        }
        Ok(op)
    }
}

fn tag_secrets(v: &mut Value) {
    match v {
        Value::Mapping(m) if m.len() == 1 && m.contains_key("!secret") => {
            let inner = m.get("!secret").cloned().unwrap_or_default();
            *v = Value::Tagged(Box::new(serde_yaml::value::TaggedValue { tag: serde_yaml::value::Tag::new("secret"), value: inner }));
        }
        Value::Mapping(m) => m.values_mut().for_each(tag_secrets),
        Value::Sequence(s) => s.iter_mut().for_each(tag_secrets),
        _ => {}
    }
}

pub fn apply(text: &str, op: &Op) -> Result<String> {
    let (text, op) = new_names(text, op);
    let (text, op) = (text.as_str(), &op);
    let before = document(text)?;
    let mut expected = before;
    apply_value(&mut expected, &op.as_text())?;
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    edit_lines(&mut lines, op)?;
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    let got = document(&out).map_err(|e| anyhow!("edit produced invalid YAML: {e}"))?;
    ensure!(got == expected, "couldn't edit this file's layout in place safely");
    Ok(out)
}

/// An edit in the keys' new names (`unfree` is `unfree`), and the
/// file with the key it edits renamed in place when it has the old name:
/// what nami writes is always the new name.
fn new_names(text: &str, op: &Op) -> (String, Op) {
    let mut op = op.clone();
    let (Op::Set { path, .. } | Op::Unset { path } | Op::Add { path, .. } | Op::Remove { path, .. }) = &mut op;
    let Some(first) = path.first_mut() else { return (text.to_string(), op) };
    if let Some((_, new)) = crate::format::RENAMED_KEYS.iter().find(|(old, _)| first == old) {
        *first = new.to_string();
    }
    let mut text = text.to_string();
    if let Some((old, new)) = crate::format::RENAMED_KEYS.iter().find(|(_, new)| first == new) {
        let lines: Vec<String> = text
            .lines()
            .map(|l| match l.strip_prefix(old).filter(|r| r.starts_with(':')) {
                Some(rest) => format!("{new}{rest}"),
                None => l.to_string(),
            })
            .collect();
        let end = if text.ends_with('\n') { "\n" } else { "" };
        text = lines.join("\n") + end;
    }
    (text, op)
}

/// A file's keys. One with nothing but comments (its last key just taken
/// out: `# my laptop` alone) has none, like an empty one.
/// Read as nami reads it (versions and env values as written), each
/// number and yes/no as its text: an edit is the same whether its value
/// came from the command line (8080) or from the file (the text 8080).
fn document(text: &str) -> Result<Value> {
    Ok(match crate::yaml_text::from_str(text)? {
        Value::Null => Value::Mapping(Mapping::new()),
        v => as_text(v),
    })
}

/// `v` with its numbers and yes/no as text, to compare.
fn as_text(v: Value) -> Value {
    match v {
        Value::Number(n) => Value::String(n.to_string()),
        Value::Bool(b) => Value::String(b.to_string()),
        Value::Sequence(s) => Value::Sequence(s.into_iter().map(as_text).collect()),
        Value::Mapping(m) => Value::Mapping(m.into_iter().map(|(k, v)| (as_text(k), as_text(v))).collect()),
        Value::Tagged(t) => Value::Tagged(Box::new(serde_yaml::value::TaggedValue { tag: t.tag, value: as_text(t.value) })),
        v => v,
    }
}

/// Parse a path like `env.EDITOR` or `files["~/.gitconfig"]`.
pub fn parse_path(s: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '.' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            '[' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                let q = chars.next().ok_or_else(|| anyhow!("bad path {s:?}"))?;
                ensure!(q == '"' || q == '\'', "path brackets need quotes: {s:?}");
                let mut key = String::new();
                loop {
                    match chars.next() {
                        Some(c) if c == q => break,
                        Some(c) => key.push(c),
                        None => bail!("unclosed quote in {s:?}"),
                    }
                }
                ensure!(chars.next() == Some(']'), "expected ] in {s:?}");
                out.push(key);
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    ensure!(!out.is_empty(), "empty path");
    Ok(out)
}

// ---------- the meaning of an edit, on parsed YAML ----------

fn apply_value(root: &mut Value, op: &Op) -> Result<()> {
    match op {
        Op::Set { path, value } => {
            *entry(root, path)? = value.clone();
        }
        Op::Unset { path } => {
            unset_value(root, path);
        }
        Op::Add { path, value } => {
            let slot = entry(root, path)?;
            if slot.is_null() {
                *slot = Value::Sequence(Vec::new());
            }
            let list = slot.as_sequence_mut().ok_or_else(|| anyhow!("{} isn't a list", path.join(".")))?;
            if !list.contains(value) {
                list.push(value.clone());
            }
        }
        Op::Remove { path, value } => {
            let mut empty = false;
            if let Some(Value::Sequence(list)) = get_mut(root, path) {
                list.retain(|v| v != value);
                empty = list.is_empty();
            }
            if empty {
                unset_value(root, path);
            }
        }
    }
    Ok(())
}

fn entry<'a>(root: &'a mut Value, path: &[String]) -> Result<&'a mut Value> {
    let mut cur = root;
    for key in path {
        if let Some(name) = item_name(key) {
            let list = cur.as_sequence_mut().ok_or_else(|| anyhow!("{key}: parent isn't a list"))?;
            cur = item_mut(list, name).ok_or_else(|| anyhow!("no `{key}:` in the list"))?;
            continue;
        }
        if cur.is_null() {
            *cur = Value::Mapping(Mapping::new());
        }
        let m = cur.as_mapping_mut().ok_or_else(|| anyhow!("{key}: parent isn't a map"))?;
        if !m.contains_key(key.as_str()) {
            m.insert(Value::String(key.clone()), Value::Null);
        }
        cur = m.get_mut(key.as_str()).unwrap();
    }
    Ok(cur)
}

fn get_mut<'a>(root: &'a mut Value, path: &[String]) -> Option<&'a mut Value> {
    let mut cur = root;
    for key in path {
        cur = match item_name(key) {
            Some(name) => item_mut(cur.as_sequence_mut()?, name)?,
            None => cur.as_mapping_mut()?.get_mut(key.as_str())?,
        };
    }
    Some(cur)
}

/// Remove the key, then any parent maps left empty.
fn unset_value(root: &mut Value, path: &[String]) {
    if path.is_empty() {
        return;
    }
    let (last, parents) = path.split_last().unwrap();
    if let Some(Value::Mapping(m)) = get_mut(root, parents) {
        m.remove(last.as_str());
        if m.is_empty() {
            unset_value(root, parents);
        }
    }
}

// ---------- the same edit, on lines ----------

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn is_blank(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with('#')
}

/// `key:` at exactly `ind` spaces (bare or quoted), in lines[from..to].
fn find_key(lines: &[String], from: usize, to: usize, ind: usize, key: &str) -> Option<usize> {
    let (dash, key) = match item_name(key) {
        Some(name) => ("- ", name),
        None => ("", key),
    };
    let forms = [format!("{dash}{key}:"), format!("{dash}\"{key}\":"), format!("{dash}'{key}':")];
    (from..to).find(|&i| {
        let l = &lines[i];
        !is_blank(l) && indent(l) == ind && forms.iter().any(|f| {
            let rest = &l[ind..];
            rest.starts_with(f.as_str()) && rest[f.len()..].chars().next().is_none_or(|c| c == ' ')
        })
    })
}

/// Lines after `at` belonging to it (more indented, or blank), and the
/// indentation of its first child, if it has children.
fn block(lines: &[String], at: usize) -> (usize, Option<usize>) {
    // An item's (`- vscode:`): its key is after the dash, and the next item isn't its.
    let item = lines[at].trim_start().starts_with("- ");
    let ind = indent(&lines[at]) + if item { 2 } else { 0 };
    let mut end = at + 1;
    let mut child = None;
    while end < lines.len() {
        let l = &lines[end];
        if !is_blank(l) {
            // A `- item` at the same indent as its key also belongs to it.
            let seq_item = !item && indent(l) == ind && l.trim_start().starts_with("- ");
            if indent(l) <= ind && !seq_item {
                break;
            }
            child.get_or_insert(indent(l));
        }
        end += 1;
    }
    // Trailing blank lines belong to whatever comes next, and so do comments no deeper than
    // the key (`# The apps.` over `packages:`, after `hostname: x`).
    let next_ones = |l: &str| l.trim().is_empty() || (l.trim_start().starts_with('#') && indent(l) <= ind);
    while end > at + 1 && next_ones(&lines[end - 1]) {
        end -= 1;
    }
    (end, child)
}

fn inline_value(line: &str) -> &str {
    let t = line.trim_start();
    // A key may have a colon in it (`schema:/path/`): quoted, or not followed by a space.
    let after = match t.chars().next() {
        Some(q @ ('"' | '\'')) => t[1..].find(q).map(|i| &t[i + 2..]).and_then(|r| r.strip_prefix(':')).unwrap_or(""),
        _ => match t.find(": ") {
            Some(i) => &t[i + 1..],
            None => t.strip_suffix(':').map(|_| "").unwrap_or_else(|| t.split_once(':').map(|(_, v)| v).unwrap_or("")),
        },
    };
    after.trim()
}

/// Where `path` lives: (line of the last key, its indent), creating nothing.
fn locate(lines: &[String], path: &[String]) -> Result<Option<(usize, usize)>> {
    let (mut from, mut to, mut ind) = (0, lines.len(), 0);
    let mut found = None;
    for (n, key) in path.iter().enumerate() {
        let Some(at) = find_key(lines, from, to, ind, key) else { return Ok(None) };
        let key_ind = if item_name(key).is_some() { ind + 2 } else { ind };
        found = Some((at, key_ind));
        if n + 1 < path.len() {
            let v = inline_value(&lines[at]);
            ensure!(v.is_empty() || v.starts_with('#'), "{key} is written inline");
            let (end, child) = block(lines, at);
            from = at + 1;
            to = end;
            ind = child.unwrap_or(key_ind + 2);
        }
    }
    Ok(found)
}

fn render(key: &str, value: &Value, ind: usize) -> Result<Vec<String>> {
    let pad = " ".repeat(ind);
    // Plain when it can only be read back as this string (.gitconfig, org.gnome.x).
    let plain = !key.is_empty()
        && !key.starts_with('-')
        && key.chars().all(|c| c.is_alphanumeric() || "-_./".contains(c))
        && matches!(serde_yaml::from_str::<Value>(key), Ok(Value::String(s)) if s == key);
    let key_text = if plain { key.to_string() } else { serde_yaml::to_string(&Value::String(key.into()))?.trim().to_string() };
    match value {
        // Multi-line text as a block, indented under its key.
        Value::String(s) if s.contains('\n') && !s.contains('\r') => {
            let chomp = if !s.ends_with('\n') { "-" } else if s.ends_with("\n\n") { "+" } else { "" };
            let indicator = if s.starts_with(' ') { "2" } else { "" };
            let mut out = vec![format!("{pad}{key_text}: |{indicator}{chomp}")];
            for l in s.strip_suffix('\n').unwrap_or(s).split('\n') {
                out.push(if l.is_empty() { String::new() } else { format!("{pad}  {l}") });
            }
            Ok(out)
        }
        Value::Mapping(m) if !m.is_empty() => {
            let mut out = vec![format!("{pad}{key_text}:")];
            if m.keys().all(Value::is_string) {
                for (k, v) in m {
                    out.extend(render(k.as_str().unwrap_or_default(), v, ind + 2)?);
                }
                return Ok(out);
            }
            for l in serde_yaml::to_string(value)?.lines() {
                out.push(format!("{pad}  {l}"));
            }
            Ok(out)
        }
        Value::Sequence(s) if !s.is_empty() => {
            let mut out = vec![format!("{pad}{key_text}:")];
            for item in s {
                out.extend(item_lines(item, ind + 2)?);
            }
            Ok(out)
        }
        Value::Null => Ok(vec![format!("{pad}{key_text}:")]),
        // `!secret |`: the tag, then the value as it renders.
        Value::Tagged(t) if matches!(&t.value, Value::String(s) if s.contains('\n')) => {
            let mut out = render(key, &t.value, ind)?;
            let head = format!("{pad}{key_text}: ");
            out[0] = format!("{head}{} {}", t.tag, &out[0][head.len()..]);
            Ok(out)
        }
        v => Ok(vec![format!("{pad}{key_text}: {}", flow(v)?)]),
    }
}

/// A list item on one line: `firefox: 150` for a map of one.
/// A list item's lines: `- name: version` on one, `- name:` with its setup
/// under it (a map) as a block.
fn item_lines(v: &Value, ind: usize) -> Result<Vec<String>> {
    if let Some((k, val)) = v.as_mapping().filter(|m| m.len() == 1).and_then(|m| m.iter().next()).filter(|(_, val)| val.as_mapping().is_some_and(|m| !m.is_empty())) {
        let mut out = render(k.as_str().unwrap_or_default(), val, ind + 2)?;
        out[0] = format!("{}- {}", " ".repeat(ind), out[0].trim_start());
        return Ok(out);
    }
    Ok(vec![format!("{}- {}", " ".repeat(ind), item(v)?)])
}

fn item(v: &Value) -> Result<String> {
    match v.as_mapping().filter(|m| m.len() == 1).and_then(|m| m.iter().next()) {
        Some((k, val)) if !val.is_mapping() && !val.is_sequence() => Ok(format!("{}: {}", flow(k)?, flow(val)?)),
        _ => flow(v),
    }
}

/// One-line YAML for a value.
fn flow(v: &Value) -> Result<String> {
    Ok(match v {
        Value::Mapping(m) if m.is_empty() => "{}".into(),
        Value::Sequence(s) if s.is_empty() => "[]".into(),
        Value::Mapping(m) => {
            let parts = m.iter().map(|(k, v)| Ok(format!("{}: {}", flow(k)?, flow(v)?))).collect::<Result<Vec<_>>>()?;
            format!("{{ {} }}", parts.join(", "))
        }
        Value::Sequence(s) => format!("[{}]", s.iter().map(flow).collect::<Result<Vec<_>>>()?.join(", ")),
        other => serde_yaml::to_string(other)?.trim_end().trim_start_matches("--- ").to_string(),
    })
}

fn edit_lines(lines: &mut Vec<String>, op: &Op) -> Result<()> {
    match op {
        Op::Set { path, value } => set_lines(lines, path, value),
        Op::Unset { path } => {
            unset_lines(lines, path)?;
            Ok(())
        }
        Op::Add { path, value } => {
            let Some((at, ind)) = locate(lines, path)? else {
                return set_lines(lines, path, &Value::Sequence(vec![value.clone()]));
            };
            let inline = inline_value(&lines[at]).to_string();
            if inline.starts_with('[') {
                let mut list = in_place(path, &inline)?;
                if !list.iter().any(|v| same(v, value)) {
                    list.push(value.clone());
                }
                let key = path.last().unwrap();
                lines[at] = format!("{} {}", render(key, &Value::Null, ind)?[0], flow(&Value::Sequence(list))?);
                return Ok(());
            }
            ensure!(inline.is_empty() || inline.starts_with('#'), "{} is written inline", path.join("."));
            let (end, child) = block(lines, at);
            let existing: Vec<String> = (at + 1..end).filter(|&i| !is_blank(&lines[i])).map(|i| lines[i].trim_start().trim_start_matches("- ").trim().to_string()).collect();
            if existing.iter().any(|e| crate::yaml_text::at(path, true, e).is_some_and(|v| same(&v, value))) {
                return Ok(());
            }
            let item_ind = child.unwrap_or(ind + 2);
            for (i, l) in item_lines(value, item_ind)?.into_iter().enumerate() {
                lines.insert(end + i, l);
            }
            Ok(())
        }
        Op::Remove { path, value } => {
            let Some((at, ind)) = locate(lines, path)? else { return Ok(()) };
            let inline = inline_value(&lines[at]).to_string();
            if inline.starts_with('[') {
                let mut list = in_place(path, &inline)?;
                list.retain(|v| !same(v, value));
                if list.is_empty() {
                    return unset_lines(lines, path).map(|_| ());
                }
                let key = path.last().unwrap();
                lines[at] = format!("{} {}", render(key, &Value::Null, ind)?[0], flow(&Value::Sequence(list))?);
                return Ok(());
            }
            let (end, _) = block(lines, at);
            // An item, with the lines under it (`- git:` and its setup).
            let item_end = |lines: &[String], i: usize| {
                let ind = lines[i].len() - lines[i].trim_start().len();
                (i + 1..end).find(|&j| !is_blank(&lines[j]) && lines[j].len() - lines[j].trim_start().len() <= ind).unwrap_or(end)
            };
            let hit = (at + 1..end).find_map(|i| {
                let t = lines[i].trim_start();
                if !t.starts_with("- ") {
                    return None;
                }
                let one = crate::yaml_text::at(path, true, t[2..].split(" #").next().unwrap_or("").trim());
                let j = item_end(lines, i);
                let ind = lines[i].len() - t.len() + 2;
                let text: String = std::iter::once(t[2..].to_string()).chain(lines[i + 1..j].iter().map(|l| l.get(ind..).unwrap_or("").to_string())).collect::<Vec<_>>().join("\n");
                let whole = crate::yaml_text::at(path, true, &text);
                (one.is_some_and(|v| same(&v, value)) || whole.is_some_and(|v| same(&v, value))).then_some((i, j))
            });
            if let Some((i, j)) = hit {
                lines.drain(i..j);
                let (end, child) = block(lines, at);
                if child.is_none() && end == at + 1 {
                    unset_lines(lines, path)?;
                }
            }
            Ok(())
        }
    }
}

/// A list written inline at `path` (`[jq, { python3: 3.10 }]`), read as
/// nami reads it there.
fn in_place(path: &[String], inline: &str) -> Result<Vec<Value>> {
    match crate::yaml_text::at(path, false, inline) {
        Some(Value::Sequence(list)) => Ok(list),
        _ => Ok(serde_yaml::from_str(inline)?),
    }
}

/// Whether two values say the same, a number and its text alike.
fn same(a: &Value, b: &Value) -> bool {
    as_text(a.clone()) == as_text(b.clone())
}

/// A map written inline on the way to `path` (`packages: [jq, git]`):
/// written out, a key a line, so it can be edited in place.
fn unfold(lines: &mut Vec<String>, path: &[String]) -> Result<()> {
    for d in 1..path.len() {
        let Some((at, ind)) = locate(lines, &path[..d])? else { return Ok(()) };
        let v = inline_value(&lines[at]).split(" #").next().unwrap_or("").trim().to_string();
        if !v.starts_with('{') {
            continue;
        }
        let Ok(Value::Mapping(m)) = serde_yaml::from_str::<Value>(&v) else { continue };
        let head = &lines[at][..indent(&lines[at])];
        let mut out = vec![format!("{head}{}:", lines[at].trim_start().split(':').next().unwrap_or(""))];
        for (k, val) in &m {
            out.extend(render(k.as_str().unwrap_or_default(), val, ind + 2)?);
        }
        lines.splice(at..at + 1, out);
    }
    Ok(())
}

fn set_lines(lines: &mut Vec<String>, path: &[String], value: &Value) -> Result<()> {
    unfold(lines, path)?;
    if let Some((at, ind)) = locate(lines, path)? {
        let (end, _) = block(lines, at);
        // Keep a trailing comment on a one-line scalar.
        let comment = if end == at + 1 && !value.is_mapping() && !value.is_sequence() {
            lines[at].split_once(" #").map(|(_, c)| format!(" #{c}"))
        } else {
            None
        };
        let mut new = render(path.last().unwrap(), value, ind)?;
        if let (Some(c), Some(first)) = (comment, new.first_mut()) {
            first.push_str(&c);
        }
        lines.splice(at..end, new);
        return Ok(());
    }
    // Find the deepest existing parent and add the rest below it.
    let mut depth = path.len() - 1;
    let parent = loop {
        if depth == 0 {
            break None;
        }
        if let Some(p) = locate(lines, &path[..depth])? {
            break Some(p);
        }
        depth -= 1;
    };
    let mut nested = value.clone();
    for key in path[depth + 1..].iter().rev() {
        let mut m = Mapping::new();
        m.insert(Value::String(key.clone()), nested);
        nested = Value::Mapping(m);
    }
    match parent {
        None => {
            while lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.pop();
            }
            lines.extend(render(&path[0], &if depth + 1 == path.len() { value.clone() } else { nested_from(&path[1..], value) }, 0)?);
        }
        Some((at, ind)) => {
            let v = inline_value(&lines[at]);
            ensure!(v.is_empty() || v.starts_with('#'), "{} is written inline", path[..depth].join("."));
            let (end, child) = block(lines, at);
            let child_ind = child.unwrap_or(ind + 2);
            lines.splice(end..end, render(&path[depth], &nested, child_ind)?);
        }
    }
    Ok(())
}

fn nested_from(rest: &[String], value: &Value) -> Value {
    let mut v = value.clone();
    for key in rest.iter().rev() {
        let mut m = Mapping::new();
        m.insert(Value::String(key.clone()), v);
        v = Value::Mapping(m);
    }
    v
}

fn unset_lines(lines: &mut Vec<String>, path: &[String]) -> Result<bool> {
    unfold(lines, path)?;
    let Some((at, _)) = locate(lines, path)? else { return Ok(false) };
    let (end, _) = block(lines, at);
    lines.drain(at..end);
    // (An item left with nothing under it stays: `- vscode:` is on.)
    if path.len() > 1 && !path.last().and_then(|_| path.get(path.len() - 2)).is_some_and(|k| item_name(k).is_some()) {
        let parent = &path[..path.len() - 1];
        if let Some((p, _)) = locate(lines, parent)? {
            let (pend, child) = block(lines, p);
            if child.is_none() && pend == p + 1 && inline_value(&lines[p]).is_empty() {
                unset_lines(lines, parent)?;
            }
        }
    }
    Ok(true)
}

/// A path as `nami config` takes it: `env.EDITOR`, `files["~/.gitconfig"].user`.
pub fn shown_path(path: &[String]) -> String {
    let mut out = String::new();
    for k in path {
        if k.is_empty() || k.contains(['.', '[', ']', '"', ' ', '/', '~']) {
            out += &format!("[\"{k}\"]");
        } else {
            if !out.is_empty() {
                out.push('.');
            }
            out += k;
        }
    }
    out
}

/// A value in words, as it reads in the file: `nvim`, `on` (true), `[a, b]`.
pub fn shown_value(v: &Value) -> String {
    let text = match v {
        Value::String(s) => s.clone(),
        Value::Null => "nothing".into(),
        Value::Bool(_) | Value::Number(_) => serde_yaml::to_string(v).unwrap_or_default().trim().to_string(),
        _ => serde_json::to_string(v).unwrap_or_default(),
    };
    crate::file_diff::say(&text)
}

/// What an edit did to `file`, said in one line: `✓ env.EDITOR is nvim in user.nami`,
/// `✓ Added tree to packages in user.nami`; `changed` false: the file said so already.
pub fn said(op: &Op, file: &str, changed: bool) -> String {
    match (op, changed) {
        (Op::Set { path, value }, true) => format!("✓ {} is {} in {file}", shown_path(path), shown_value(value)),
        (Op::Unset { path }, true) => format!("✓ {} is out of {file}", shown_path(path)),
        (Op::Add { path, value }, true) => format!("✓ Added {} to {} in {file}", shown_value(value), shown_path(path)),
        (Op::Remove { path, value }, true) => format!("✓ Removed {} from {} in {file}", shown_value(value), shown_path(path)),
        (Op::Set { path, value }, false) => format!("✓ {} is {} in {file} already", shown_path(path), shown_value(value)),
        (Op::Unset { path }, false) => format!("✓ {file} has no {} already", shown_path(path)),
        (Op::Add { path, value }, false) => format!("✓ {} is in {} in {file} already", shown_value(value), shown_path(path)),
        (Op::Remove { path, value }, false) => format!("✓ {} isn't in {} in {file}", shown_value(value), shown_path(path)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_what_changed() {
        let p = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(said(&Op::Set { path: p(&["env", "EDITOR"]), value: s("nvim") }, "user.nami", true), "✓ env.EDITOR is nvim in user.nami");
        assert_eq!(said(&Op::Set { path: p(&["namios", "dock", "on"]), value: Value::Bool(false) }, "system.nami", true), "✓ namios.dock.on is off in system.nami");
        assert_eq!(said(&Op::Add { path: p(&["packages"]), value: s("tree") }, "user.nami", true), "✓ Added tree to packages in user.nami");
        assert_eq!(said(&Op::Unset { path: p(&["files", "~/.gitconfig", "user"]) }, "user.nami", true), "✓ files[\"~/.gitconfig\"].user is out of user.nami");
        assert_eq!(said(&Op::Add { path: p(&["packages"]), value: s("tree") }, "user.nami", false), "✓ tree is in packages in user.nami already");
    }

    fn s(v: &str) -> Value {
        Value::String(v.into())
    }
    fn p(path: &str) -> Vec<String> {
        parse_path(path).unwrap()
    }

    #[test]
    fn edits_under_a_list_item() {
        let text = "packages:\n  - git\n  - vscode:\n      ~/.config/Code/User/settings.json:\n        mutable: true\n        content: { editor.fontSize: 14 }  # mine\n  - jq\n";
        let path = vec!["packages".into(), "- vscode".into(), "~/.config/Code/User/settings.json".into(), "content".into(), "editor.fontSize".into()];
        let t = apply(text, &Op::Set { path: path.clone(), value: Value::from(16) }).unwrap();
        assert_eq!(t, "packages:\n  - git\n  - vscode:\n      ~/.config/Code/User/settings.json:\n        mutable: true\n        content:\n          editor.fontSize: 16\n  - jq\n");
        let mut added = path.clone();
        *added.last_mut().unwrap() = "files.autoSave".into();
        let t = apply(&t, &Op::Set { path: added, value: s("afterDelay") }).unwrap();
        assert!(t.contains("          editor.fontSize: 16\n          files.autoSave: afterDelay\n  - jq\n"), "{t}");
        let t = apply(&t, &Op::Unset { path }).unwrap();
        assert!(t.contains("        content:\n          files.autoSave: afterDelay\n  - jq\n"), "{t}");
        // Not there: said so.
        assert!(apply(text, &Op::Set { path: vec!["packages".into(), "- nope".into(), "x".into()], value: s("y") }).is_err());
    }

    const CONFIG: &str = "# my laptop\npackages:\n  - firefox   # browser\n  - jq\n\nkeep-generations: 5  # enough\nhome:\n  env:\n    EDITOR: nvim\n  files:\n    .gitconfig:\n      user: { name: Ada }\n";

    /// From JSON (the window's edits), `{"!secret": ...}` is a secret.
    #[test]
    fn a_secret_from_json() {
        let op = Op::from_json(r#"{"set": {"path": ["files", "/run/nami/secrets/backup-password"], "value": {"!secret": "hunter2"}}}"#).unwrap();
        let t = apply("packages:\n  - jq\n", &op).unwrap();
        assert!(t.contains("/run/nami/secrets/backup-password: !secret hunter2"), "{t}");
    }

    /// A whole file sealed (`!secret` text): a block under the tag.
    #[test]
    fn tagged_text() {
        let secret = Value::Tagged(Box::new(serde_yaml::value::TaggedValue { tag: serde_yaml::value::Tag::new("secret"), value: s("a=1\nb=2\n") }));
        let path = vec!["files".to_string(), "~/.npmrc".to_string()];
        let t = apply("packages:\n  - jq\n", &Op::Set { path, value: secret.clone() }).unwrap();
        assert!(t.contains(": !secret |\n    a=1\n    b=2\n"), "{t}");
        assert_eq!(serde_yaml::from_str::<Value>(&t).unwrap()["files"]["~/.npmrc"], secret);
    }

    #[test]
    fn flow_lists_too() {
        let t = apply("packages: [jq, git]\n", &Op::Add { path: p("packages"), value: s("tree") }).unwrap();
        assert_eq!(t, "packages: [jq, git, tree]\n");
        let t = apply(&t, &Op::Remove { path: p("packages"), value: s("jq") }).unwrap();
        assert_eq!(t, "packages: [git, tree]\n");
    }

    #[test]
    fn set_scalars_keep_trailing_comment_and_create_parents() {
        let t = apply(CONFIG, &Op::Set { path: p("keep-generations"), value: 7.into() }).unwrap();
        assert!(t.contains("history: 7 # enough") || t.contains("history: 7  # enough"), "the old name is renamed in place: {t}");
        let t = apply(&t, &Op::Set { path: p("home.aliases.ll"), value: s("ls -la") }).unwrap();
        assert!(t.contains("  aliases:\n    ll: ls -la"), "{t}");
        let t = apply("packages: []\n", &Op::Set { path: p("home.env.PAGER"), value: s("less") }).unwrap();
        assert_eq!(t, "packages: []\nhome:\n  env:\n    PAGER: less\n");
    }

    /// A comment over the next key is the next key's: a scalar set before it keeps it.
    #[test]
    fn the_comment_over_the_next_key_stays() {
        let t = apply("hostname: namios\n# The apps.\npackages:\n  - namios\n", &Op::Set { path: p("hostname"), value: s("laptop") }).unwrap();
        assert_eq!(t, "hostname: laptop\n# The apps.\npackages:\n  - namios\n");
        let t = apply("users:\n  ada:\n    admin: true\n    # hers\n# Theirs.\nkeyboard: us\n", &Op::Set { path: p("users.ada"), value: serde_yaml::from_str("{admin: false}").unwrap() }).unwrap();
        assert!(t.contains("# Theirs.\nkeyboard: us\n"), "{t}");
    }

    #[test]
    fn unset_removes_empty_parents() {
        let t = apply(CONFIG, &Op::Unset { path: p("home.env.EDITOR") }).unwrap();
        assert!(!t.contains("env:") && t.contains("home:\n  files:"), "{t}");
        let t = apply(&t, &Op::Unset { path: p(r#"home.files[".gitconfig"]"#) }).unwrap();
        assert!(!t.contains("home:"), "{t}");
    }

    #[test]
    fn replace_a_nested_block() {
        let mut v = Mapping::new();
        v.insert(s("mutable"), Value::Bool(true));
        v.insert(s("content"), serde_yaml::from_str("{ user: { name: Ada } }").unwrap());
        let t = apply(CONFIG, &Op::Set { path: p(r#"home.files[".gitconfig"]"#), value: Value::Mapping(v) }).unwrap();
        assert!(t.contains("mutable: true") && t.contains("# my laptop"), "{t}");
    }

    #[test]
    fn multi_line_text_and_dotted_keys() {
        let t = apply(CONFIG, &Op::Set { path: vec!["home".into(), "files".into(), ".vimrc".into()], value: s("set number\n\nsyntax on\n") }).unwrap();
        assert!(t.contains("    .vimrc: |\n      set number\n\n      syntax on\n"), "{t}");
        let t = apply(&t, &Op::Set { path: vec!["settings".into(), "org.gnome.desktop.interface".into(), "color-scheme".into()], value: s("prefer-dark") }).unwrap();
        assert!(t.ends_with("settings:\n  org.gnome.desktop.interface:\n    color-scheme: prefer-dark\n"), "{t}");
        let t = apply("a: 1\n", &Op::Set { path: p("b"), value: s("  indented\nno newline") }).unwrap();
        assert_eq!(serde_yaml::from_str::<Value>(&t).unwrap()["b"], s("  indented\nno newline"));
    }
}
