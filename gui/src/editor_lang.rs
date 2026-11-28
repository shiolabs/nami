//! What the editor knows of a nami file's text, apart from drawing it: its parts' colours (as
//! the website colours them, site/src/data/hl.js `yaml()`), where the cursor is in it (the keys
//! it's under, what's being typed there), what fits there (nami's schema and options), what
//! Enter types, and nami check's problems: where on their line, and the edit a suggestion
//! makes.

use std::ops::Range;

use serde::Deserialize;
use serde_json::Value;

/// What a part of a line is, for its colour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    /// A value as it's written (a package's name, `auto`), the indent, a colon.
    Plain,
    /// A line's key (`packages:`).
    Key,
    /// A key inside a value (`{ name: Ada }`).
    InnerKey,
    /// A path (`./work.nami`, `~/.gitconfig:`).
    Path,
    /// A number or a version (`150`, `3.12.4`), true or false.
    Number,
    /// A quoted string.
    Quoted,
    /// `!secret`.
    Secret,
    /// A list's dash, a comment.
    Quiet,
}

fn word_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// How long a line's key is at the start of `s` (a name, a path or a quoted string, then `:`
/// and a space or the line's end), not counting its colon.
fn key_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let first = *b.first()?;
    let len = if first == b'"' {
        s[1..].find('"')? + 2
    } else if word_char(first) || matches!(first, b'~' | b'/' | b'.' | b'@' | b'-') {
        s.find(|c: char| c == ':' || c.is_whitespace()).unwrap_or(s.len())
    } else {
        return None;
    };
    (b.get(len) == Some(&b':') && b.get(len + 1).is_none_or(|c| c.is_ascii_whitespace())).then_some(len)
}

/// A line's parts and what each is, end to end.
pub fn spans(line: &str) -> Vec<(Range<usize>, Tone)> {
    let mut out: Vec<(Range<usize>, Tone)> = Vec::new();
    if line.trim_start().starts_with('#') {
        out.push((0..line.len(), Tone::Quiet));
        return out;
    }
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let mut at = indent;
    out.push((0..indent, Tone::Plain));
    let rest = &line[at..];
    if rest.starts_with("- ") || rest == "-" {
        out.push((at..at + 1, Tone::Quiet));
        at += 1;
        if rest.len() > 1 {
            out.push((at..at + 1, Tone::Plain));
            at += 1;
        }
    }
    if let Some(k) = key_len(&line[at..]) {
        let tone = if line[at..].starts_with(['~', '/', '.']) { Tone::Path } else { Tone::Key };
        out.push((at..at + k, tone));
        out.push((at + k..at + k + 1, Tone::Plain));
        at += k + 1;
    }
    value_spans(line, at, &mut out);
    out.retain(|(r, _)| !r.is_empty());
    out
}

/// A value's parts, from `from` on: as hl.js's one pattern takes them, the first that fits at
/// each place.
fn value_spans(s: &str, from: usize, out: &mut Vec<(Range<usize>, Tone)>) {
    let b = s.as_bytes();
    let (mut i, mut plain) = (from, from);
    while i < b.len() {
        if let Some((end, parts)) = part_at(s, i, from) {
            out.push((plain..i, Tone::Plain));
            out.extend(parts);
            i = end;
            plain = end;
        } else {
            i += s[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out.push((plain..b.len(), Tone::Plain));
}

/// What starts at `i`, if anything: where it ends, and its parts.
fn part_at(s: &str, i: usize, from: usize) -> Option<(usize, Vec<(Range<usize>, Tone)>)> {
    let b = s.as_bytes();
    let rest = &s[i..];
    let one = |end: usize, tone: Tone| Some((end, vec![(i..end, tone)]));
    if rest.starts_with("!secret") {
        return one(i + 7, Tone::Secret);
    }
    if let Some(q) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
        if let Some(close) = rest[1..].find(q) {
            return one(i + close + 2, Tone::Quoted);
        }
    }
    // A path, not in the middle of a word or a URL.
    let after_word = i > from && (word_char(b[i - 1]) || b[i - 1] == b':' || b[i - 1] == b'/');
    if !after_word {
        let lead = ["../", "./", "~/", "/"].into_iter().find(|p| rest.starts_with(p));
        if let Some(lead) = lead {
            let len = rest[lead.len()..].find(|c: char| c.is_whitespace() || matches!(c, ',' | '}' | ']')).unwrap_or(rest.len() - lead.len());
            if len > 0 {
                return one(i + lead.len() + len, Tone::Path);
            }
        }
    }
    // A key inside a value: a name, its colon, a space.
    let name = rest.bytes().take_while(|c| word_char(*c) || matches!(c, b'.' | b'-')).count();
    if name > 0 && b.get(i + name) == Some(&b':') && b.get(i + name + 1).is_some_and(|c| c.is_ascii_whitespace()) {
        return Some((i + name + 1, vec![(i..i + name, Tone::InnerKey), (i + name..i + name + 1, Tone::Plain)]));
    }
    // true, false, a number or a version: whole words.
    let starts_word = (i == 0 || !word_char(b[i - 1])) && word_char(b[i]);
    if starts_word {
        let ends_word = |end: usize| b.get(end).is_none_or(|c| !word_char(*c));
        for w in ["true", "false"] {
            if rest.starts_with(w) && ends_word(i + w.len()) {
                return one(i + w.len(), Tone::Number);
            }
        }
        // Digits, and .digits after them: the longest that ends a word.
        let mut ends = Vec::new();
        let mut j = i;
        loop {
            let digits = b[j..].iter().take_while(|c| c.is_ascii_digit()).count();
            if digits == 0 {
                break;
            }
            j += digits;
            ends.push(j);
            if b.get(j) == Some(&b'.') && b.get(j + 1).is_some_and(u8::is_ascii_digit) {
                j += 1;
            } else {
                break;
            }
        }
        if let Some(end) = ends.into_iter().rev().find(|e| ends_word(*e)) {
            return one(end, Tone::Number);
        }
    }
    // A comment after the value.
    if b[i].is_ascii_whitespace() && b.get(i + 1) == Some(&b'#') {
        return one(b.len(), Tone::Quiet);
    }
    None
}

/// Where a line is in the file's keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seg {
    Key(String),
    /// A list's item.
    Item,
}

/// A line's shape: its indent, whether it's a list's item, where its words start, and its key.
struct Shape<'a> {
    indent: usize,
    item: bool,
    content: usize,
    key: Option<(Range<usize>, &'a str)>,
    /// Nothing on it but a comment, or nothing at all.
    blank: bool,
}

fn shape(line: &str) -> Shape<'_> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[indent..];
    let blank = rest.trim().is_empty() || rest.starts_with('#');
    let item = rest.starts_with("- ") || rest.trim_end() == "-";
    let content = if item { indent + 1 + (rest[1..].len() - rest[1..].trim_start_matches(' ').len()) } else { indent };
    let key = key_len(&line[content..]).map(|k| (content..content + k, line[content..content + k].trim_matches('"')));
    Shape { indent, item, content, key, blank }
}

/// A key's value starts a block of text (`|`, `>`): what's under it is text, not keys.
fn opens_text(line: &str, key: &Range<usize>) -> bool {
    line[key.end + 1..].trim_start().starts_with(['|', '>'])
}

/// The keys (and items) above line `n` of `lines` that it's under, from the top, for a line
/// indented `indent` that's an `item` or not. None: it's in a block of text.
fn parents(lines: &[&str], n: usize, indent: usize, item: bool) -> Option<Vec<Seg>> {
    let mut segs = Vec::new();
    let (mut want, mut in_item) = (indent, item);
    for j in (0..n).rev() {
        if want == 0 && !in_item {
            break;
        }
        let line = lines[j];
        let s = shape(line);
        if s.blank {
            continue;
        }
        if s.item {
            // Inside this item's map: under its key when further in than it, else beside it.
            if want > s.indent {
                if let Some((r, k)) = &s.key
                    && want > s.content
                {
                    if opens_text(line, r) {
                        return None;
                    }
                    segs.push(Seg::Key(k.to_string()));
                }
                segs.push(Seg::Item);
                want = s.indent;
                in_item = true;
            }
            continue;
        }
        // (A line with no key further out: a value going on from the line before it.)
        if let Some((r, k)) = &s.key
            && (s.indent < want || (in_item && s.indent == want))
        {
            if opens_text(line, r) {
                return None;
            }
            segs.push(Seg::Key(k.to_string()));
            want = s.indent;
            in_item = false;
        }
    }
    segs.reverse();
    Some(segs)
}

/// What the cursor's on: a key, a key's value, a package's name, or its version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum What {
    Key,
    Value(String),
    Package,
    Version(String),
}

/// Where the cursor is: under which keys, what it's on, and the word there (its start to its end).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub path: Vec<Seg>,
    pub what: What,
    pub word: Range<usize>,
}

/// What goes in a package's name or a key, as it's typed.
fn name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '@')
}

fn is_lists_of_packages(path: &[Seg]) -> bool {
    matches!(path, [Seg::Key(k)] if k == "packages" || k == "without")
}

/// Where the cursor at `at` is in `text`, for what fits there; None in a comment, a quoted
/// string, a block of text, or where nothing's typed (a path, a list's other words).
pub fn place(text: &str, at: usize) -> Option<Place> {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let line = &text[start..end];
    let col = at - start;
    let before = &line[..col];
    if before.trim_start().starts_with('#') || before.contains(" #") || before.matches('"').count() % 2 == 1 || before.matches('\'').count() % 2 == 1 {
        return None;
    }
    let s = shape(line);
    if col < s.content {
        return None;
    }
    let lines: Vec<&str> = text[..start].split('\n').collect();
    let n = if start == 0 { 0 } else { lines.len() - 1 };
    let mut path = parents(&lines[..n.min(lines.len())], n, s.indent, s.item)?;
    let typed = &line[s.content..col];
    // The word's end: on past the cursor while it's a name.
    let tail = line[col..].chars().take_while(|c| name_char(*c)).map(char::len_utf8).sum::<usize>();
    let word_from = |i: usize| start + i..at + tail;
    if let Some(colon) = typed.find(": ") {
        let key = typed[..colon].trim_matches('"').to_string();
        let value = typed[colon + 1..].trim_start();
        if !value.chars().all(name_char) {
            return None;
        }
        let from = col - value.len();
        if s.item && is_lists_of_packages(&path) {
            return Some(Place { path, what: What::Version(key), word: word_from(from) });
        }
        if !s.item
            && key == "version"
            && let [.., Seg::Key(list), Seg::Item, Seg::Key(package)] = path.as_slice()
            && list == "packages"
        {
            let package = package.clone();
            return Some(Place { path, what: What::Version(package), word: word_from(from) });
        }
        if s.item {
            path.push(Seg::Item);
        }
        return Some(Place { path, what: What::Value(key), word: word_from(from) });
    }
    if typed.ends_with(':') || !typed.chars().all(name_char) {
        return None;
    }
    if s.item && is_lists_of_packages(&path) {
        if let Some((name, _)) = typed.split_once('@') {
            let name = name.to_string();
            return Some(Place { path, what: What::Version(name), word: word_from(s.content + typed.find('@')? + 1) });
        }
        return Some(Place { path, what: What::Package, word: word_from(s.content) });
    }
    if s.item {
        path.push(Seg::Item);
    }
    Some(Place { path, what: What::Key, word: word_from(s.content) })
}

/// What the pointer can rest on for help: a key, or a package's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Key { path: Vec<Seg>, key: String },
    Package(String),
}

/// What's at `at` in `text` to say something about, and where it is.
pub fn target(text: &str, at: usize) -> Option<(Range<usize>, Target)> {
    let start = text[..at.min(text.len())].rfind('\n').map_or(0, |i| i + 1);
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let line = &text[start..end];
    let col = at - start;
    let s = shape(line);
    if s.blank {
        return None;
    }
    let lines: Vec<&str> = text[..start].split('\n').collect();
    let n = if start == 0 { 0 } else { lines.len() - 1 };
    let path = parents(&lines[..n.min(lines.len())], n, s.indent, s.item)?;
    if s.item && is_lists_of_packages(&path) {
        let name_end = s.content + line[s.content..].find(|c: char| !(name_char(c) && c != '@')).unwrap_or(line.len() - s.content);
        let name = &line[s.content..name_end];
        if (s.content..name_end).contains(&col) && !name.is_empty() && !name.starts_with('.') {
            return Some((start + s.content..start + name_end, Target::Package(name.to_string())));
        }
        return None;
    }
    let (r, k) = s.key?;
    if !r.contains(&col) {
        return None;
    }
    let mut path = path;
    if s.item {
        path.push(Seg::Item);
    }
    Some((start + r.start..start + r.end, Target::Key { path, key: k.to_string() }))
}

/// What Enter does at the end of `before` (the line up to the cursor).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NewLine {
    /// A new line starting with this (its indent, and a list's dash); the spaces at the end of
    /// `before` go.
    Type(String),
    /// An item with nothing in it: the list ends here, its dash taken off.
    EndList,
}

/// Enter: the line's indent kept; two more after `key:` (and a new list's dash where the key
/// holds a list, `list`); in a list, the next item's dash.
pub fn new_line(before: &str, list: impl Fn(&str) -> bool) -> NewLine {
    let s = shape(before);
    let rest = before[s.indent..].trim_end();
    if s.item && rest == "-" {
        return NewLine::EndList;
    }
    let mut code = rest.to_string();
    if let Some(i) = code.find(" #") {
        code.truncate(i);
        code = code.trim_end().to_string();
    }
    let spaces = |n: usize| " ".repeat(n);
    if code.ends_with(':') || code.ends_with(": |") || code.ends_with(": >") || code.ends_with(": |-") {
        let key = s.key.as_ref().map(|(_, k)| *k).unwrap_or_default();
        let dash = if code.ends_with(':') && list(key) { "- " } else { "" };
        return NewLine::Type(format!("{}{dash}", spaces(s.content + 2)));
    }
    if s.item {
        return NewLine::Type(format!("{}- ", spaces(s.indent)));
    }
    NewLine::Type(spaces(s.indent))
}

/// The block the line `n` (0-based) is in: the line with its key, the column the guide goes
/// down, and the block's last line.
pub fn block(text: &str, n: usize) -> Option<(usize, usize, usize)> {
    let lines: Vec<&str> = text.split('\n').collect();
    let line = *lines.get(n)?;
    let s = shape(line);
    let (indent, item) = if s.blank { (line.len(), false) } else { (s.indent, s.item) };
    if indent == 0 && !item {
        return None;
    }
    // The nearest line above that it's under.
    let mut parent = None;
    for j in (0..n).rev() {
        let p = shape(lines[j]);
        if p.blank {
            continue;
        }
        if p.indent < indent || (item && p.indent == indent && !p.item) {
            parent = Some((j, p.indent, if p.item { p.content } else { p.indent }));
            break;
        }
    }
    let (top, parent_indent, column) = parent?;
    let inside = |l: &str| {
        let s = shape(l);
        !s.blank && (s.indent > parent_indent || (s.indent == parent_indent && s.item && !shape(lines[top]).item))
    };
    let mut last = top;
    for (j, l) in lines.iter().enumerate().skip(top + 1) {
        if inside(l) {
            last = j;
        } else if !shape(l).blank {
            break;
        }
    }
    (last > top).then_some((top, column, last))
}

/// A problem nami check found, on a line (1-based).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    pub line: usize,
    pub message: String,
    pub hint: Option<String>,
    pub error: bool,
}

impl Mark {
    /// As it's said: what's wrong, and what to do in brackets.
    pub fn said(&self) -> String {
        match &self.hint {
            Some(h) if !h.is_empty() => format!("{} ({h})", self.message),
            _ => self.message.clone(),
        }
    }

    /// What it suggests instead: "did you mean `ripgrep`?".
    pub fn suggestion(&self) -> Option<String> {
        let said = self.said();
        let i = said.find("did you mean ")? + "did you mean ".len();
        let rest = &said[i..];
        let word = match rest.strip_prefix('`') {
            Some(r) => &r[..r.find('`')?],
            None => rest.split(|c: char| c.is_whitespace() || matches!(c, '?' | ',' | ')')).next()?,
        };
        (!word.is_empty()).then(|| word.to_string())
    }
}

/// nami's words in parts, its `code` apart (without the backticks): each part, and whether
/// it's code. A backtick with no other after it stays as it is.
pub fn code_spans(text: &str) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut parts: Vec<&str> = text.split('`').collect();
    // (An odd one out: the last backtick and what follows it are words.)
    let tail = if parts.len() % 2 == 0 { parts.pop() } else { None };
    for (i, p) in parts.into_iter().enumerate() {
        if !p.is_empty() {
            out.push((p.to_string(), i % 2 == 1));
        }
    }
    if let Some(t) = tail {
        let t = format!("`{t}");
        match out.last_mut() {
            Some((last, false)) => last.push_str(&t),
            _ => out.push((t, false)),
        }
    }
    out
}

/// What a word on a line is made of, for finding one whole.
fn token_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '/' | '~' | '+')
}

/// Where `word` is on `line`, as a whole word.
fn find_word(line: &str, word: &str) -> Option<Range<usize>> {
    if word.is_empty() {
        return None;
    }
    let mut from = 0;
    while let Some(i) = line[from..].find(word) {
        let (a, b) = (from + i, from + i + word.len());
        let before = line[..a].chars().next_back().is_none_or(|c| !token_char(c));
        let after = line[b..].chars().next().is_none_or(|c| !token_char(c));
        if before && after {
            return Some(a..b);
        }
        from = a + word.len().max(1);
    }
    None
}

fn distance(a: &str, b: &str) -> usize {
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

/// Where on its line a problem is: the word it names (the first of its words that's on the
/// line, a quoted one first), else the word nearest what it suggests; and whether it's that
/// exact a place (else the line's words, all of them).
pub fn mark_span(mark: &Mark, line: &str) -> (Range<usize>, bool) {
    let quoted = mark.message.split('`').skip(1).step_by(2);
    let words = mark.message.split_whitespace().map(|w| w.trim_matches(|c: char| !token_char(c) || c == '.'));
    for w in quoted.chain(words).filter(|w| w.len() > 1) {
        // (A key by its path, `env.GITHUB_TOKEN`: its own name's on the line.)
        let last = w.rsplit('.').next().filter(|l| *l != w && l.len() > 1);
        if let Some(r) = find_word(line, w).or_else(|| last.and_then(|l| find_word(line, l))) {
            return (r, true);
        }
    }
    if let Some(want) = mark.suggestion() {
        let mut best: Option<(usize, Range<usize>)> = None;
        let mut i = 0;
        for part in line.split(|c: char| !token_char(c)) {
            let d = distance(part, &want);
            if !part.is_empty() && part != want && d <= 2.max(want.len() / 3) && best.as_ref().is_none_or(|(b, _)| d < *b) {
                best = Some((d, i..i + part.len()));
            }
            i += part.len() + 1;
        }
        if let Some((_, r)) = best {
            return (r, true);
        }
    }
    let s = shape(line);
    let code = line.find(" #").unwrap_or(line.len());
    let from = if s.item { s.content } else { s.indent };
    let end = line[..code].trim_end().len().max(from);
    (from..end, false)
}

/// The edit a problem's suggestion makes: what in `text` it replaces, and with what.
pub fn fix(text: &str, mark: &Mark) -> Option<(Range<usize>, String)> {
    let want = mark.suggestion()?;
    let start: usize = text.split('\n').take(mark.line.checked_sub(1)?).map(|l| l.len() + 1).sum();
    let line = text.get(start..)?.split('\n').next()?;
    if mark.line > text.split('\n').count() {
        return None;
    }
    let (r, exact) = mark_span(mark, line);
    if !exact || line[r.clone()] == want {
        return None;
    }
    Some((start + r.start..start + r.end, want))
}

/// One of nami's options (`nami options --json`).
#[derive(Deserialize, Clone, Default, Debug)]
pub struct Opt {
    pub key: String,
    pub kind: String,
    pub summary: String,
    pub example: String,
}

/// What nami knows of a kind of file: its schema (`nami options --schema <kind>`), its options,
/// and namiOS's settings' names.
#[derive(Default, Clone, Debug)]
pub struct Knowledge {
    pub schema: Value,
    pub options: Vec<Opt>,
    pub namios: Vec<String>,
}

/// Something that fits: what the list shows, what it types, and a few words about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    pub insert: String,
    pub about: String,
}

/// What the pointer's shown about a key: what it's for, and an example.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Help {
    pub summary: String,
    pub example: String,
}

/// A few words: the first sentence, on one line, without its code marks.
fn first_sentence(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ").replace('`', "");
    let cut = flat.find(". ").map_or(flat.as_str(), |i| &flat[..i]);
    let cut = cut.trim_end_matches('.');
    if cut.chars().count() <= 90 {
        return cut.to_string();
    }
    let short: String = cut.chars().take(89).collect();
    format!("{}…", short.rsplit_once(' ').map_or(short.as_str(), |(a, _)| a).trim_end_matches([',', ';', ':']))
}

/// What a key's help says, from its docs (the options' summary, its example in a code block).
fn help_of(docs: &str) -> Help {
    let first = docs.split("\n\n---\n\n").next().unwrap_or(docs);
    let (summary, example) = match first.split_once("```yaml\n") {
        Some((s, rest)) => (s.trim(), rest.split("```").next().unwrap_or_default().trim_end()),
        None => (first.trim(), ""),
    };
    Help { summary: short_summary(summary), example: example.lines().take(8).collect::<Vec<_>>().join("\n") }
}

/// A summary short enough for a tooltip: its first two sentences, without code marks.
fn short_summary(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ").replace('`', "");
    let mut out = String::new();
    for (n, sentence) in flat.split_inclusive(". ").enumerate() {
        if n == 2 || (n > 0 && out.len() + sentence.len() > 260) {
            break;
        }
        out.push_str(sentence);
    }
    let out = out.trim_end().to_string();
    if out.chars().count() > 300 { format!("{}…", out.chars().take(299).collect::<String>().trim_end()) } else { out }
}

impl Knowledge {
    /// The schema's part for `path`.
    fn node(&self, path: &[Seg]) -> Option<&Value> {
        let mut node = &self.schema;
        for seg in path {
            node = match seg {
                Seg::Key(k) => node.get("properties").and_then(|p| p.get(k)).or_else(|| node.get("additionalProperties").filter(|a| a.is_object()))?,
                Seg::Item => {
                    let items = node.get("items")?;
                    match items.get("anyOf").and_then(Value::as_array) {
                        Some(any) => any.iter().find(|v| v.get("properties").is_some() || v.get("type").and_then(Value::as_str) == Some("object"))?,
                        None => items,
                    }
                }
            };
        }
        Some(node)
    }

    /// The options about key `base` (their key's first word).
    fn options_of<'a>(&'a self, base: &'a str) -> impl Iterator<Item = &'a Opt> + 'a {
        self.options.iter().filter(move |o| o.key.split([' ', '/']).next() == Some(base))
    }

    /// The keys a map under `path` may have, each with what it's for.
    pub fn keys(&self, path: &[Seg]) -> Vec<Item> {
        let item = |k: &str, about: String| Item { label: k.to_string(), insert: k.to_string(), about };
        if let [Seg::Key(n)] = path
            && n == "namios"
        {
            return self.namios.iter().map(|k| item(k, String::new())).collect();
        }
        if let [Seg::Key(list), Seg::Item, Seg::Key(pkg)] = path
            && list == "packages"
        {
            let shell = self.options_of("login-shell").next().map(|o| first_sentence(&o.summary)).unwrap_or_default();
            let mut keys = vec![item("version", "Which version: 150 is the newest 150.x".into()), item("login-shell", shell)];
            // A Python's packages (python3, python313).
            if is_python(pkg) {
                let with = self.options_of("with").next().map(|o| first_sentence(&o.summary)).unwrap_or_else(|| "Its Python packages".into());
                keys.push(item("with", with));
            }
            return keys;
        }
        let mut out = Vec::new();
        if let Some(props) = self.node(path).and_then(|n| n.get("properties")).and_then(Value::as_object) {
            for (k, v) in props {
                // (Keys nami refuses, or that have a new name: not offered.)
                if v.get("not").is_some() || v.get("deprecationMessage").is_some() {
                    continue;
                }
                out.push(item(k, first_sentence(v.get("description").and_then(Value::as_str).unwrap_or_default())));
            }
            out.sort_by(|a, b| a.label.cmp(&b.label));
        }
        // What the options' kinds list: `name: { password, ssid, ... }`.
        let listed = match path {
            [Seg::Key(base), Seg::Key(_)] => Some((base, "name: {")),
            [Seg::Key(base), Seg::Item, Seg::Key(_)] => Some((base, "- name: {")),
            _ => None,
        };
        if let Some((base, lead)) = listed {
            for o in self.options_of(base) {
                let Some(inner) = o.kind.strip_prefix(lead).and_then(|r| r.split('}').next()) else { continue };
                for k in inner.split(',').map(str::trim).filter(|k| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')) {
                    if !out.iter().any(|i| i.label == k) {
                        out.push(item(k, String::new()));
                    }
                }
            }
        }
        out
    }

    /// The values key `key` under `path` may have, where nami knows them all.
    pub fn values(&self, path: &[Seg], key: &str) -> Vec<Item> {
        let item = |v: &str| Item { label: v.to_string(), insert: v.to_string(), about: String::new() };
        let mut full = path.to_vec();
        full.push(Seg::Key(key.to_string()));
        if let Some(node) = self.node(&full) {
            if let Some(e) = node.get("enum").and_then(Value::as_array) {
                return e.iter().filter_map(Value::as_str).map(item).collect();
            }
            let types: Vec<&str> = match node.get("type") {
                Some(Value::String(t)) => vec![t.as_str()],
                Some(Value::Array(t)) => t.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            if types == ["boolean"] {
                return vec![item("true"), item("false")];
            }
        }
        if matches!(full.as_slice(), [Seg::Key(l), Seg::Item, Seg::Key(_), Seg::Key(k)] if l == "packages" && k == "login-shell") {
            return vec![item("true"), item("false")];
        }
        if path.is_empty() {
            // The options' kinds: `auto or manual`, `zsh | fish`.
            for o in self.options_of(key) {
                let parts: Vec<&str> = o.kind.split(" or ").flat_map(|p| p.split(" | ")).map(str::trim).collect();
                if parts.len() > 1 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')) {
                    return parts.into_iter().map(item).collect();
                }
            }
        }
        Vec::new()
    }

    /// Whether key `key` under `path` holds a list.
    pub fn is_list(&self, path: &[Seg], key: &str) -> bool {
        let mut full = path.to_vec();
        full.push(Seg::Key(key.to_string()));
        self.node(&full).and_then(|n| n.get("type")).is_some_and(|t| t == "array")
    }

    /// What key `key` under `path` is for, and an example.
    pub fn help(&self, path: &[Seg], key: &str) -> Option<Help> {
        if let [Seg::Key(l), Seg::Item, Seg::Key(_)] = path
            && l == "packages"
            && (key == "login-shell" || key == "with")
        {
            let o = self.options_of(key).next()?;
            return Some(Help { summary: short_summary(&o.summary), example: o.example.clone() });
        }
        let mut full = path.to_vec();
        full.push(Seg::Key(key.to_string()));
        let node = self.node(&full)?;
        if node.get("not").is_some() {
            let said = node.get("description").and_then(Value::as_str)?;
            return Some(Help { summary: said.replace('`', ""), example: String::new() });
        }
        if let Some(docs) = node.get("markdownDescription").and_then(Value::as_str) {
            return Some(help_of(docs));
        }
        let d = node.get("description").and_then(Value::as_str)?;
        Some(Help { summary: short_summary(d), example: String::new() })
    }
}

/// Whether a package is a Python that takes packages (`with:`): python3, python313.
fn is_python(name: &str) -> bool {
    name.strip_prefix("python3").is_some_and(|v| v.chars().all(|c| c.is_ascii_digit()))
}

/// `items` that start with what's `typed` (in any case), then the ones that have it in them.
pub fn matching(items: Vec<Item>, typed: &str) -> Vec<Item> {
    let t = typed.to_lowercase();
    let (mut first, mut then): (Vec<Item>, Vec<Item>) = (Vec::new(), Vec::new());
    for i in items {
        let l = i.label.to_lowercase();
        if l.starts_with(&t) {
            first.push(i);
        } else if !t.is_empty() && l.contains(&t) {
            then.push(i);
        }
    }
    first.extend(then);
    first
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(line: &str) -> Vec<(&str, Tone)> {
        spans(line).into_iter().filter(|(_, t)| *t != Tone::Plain).map(|(r, t)| (&line[r], t)).collect()
    }

    #[test]
    fn colours_as_the_website_does() {
        assert_eq!(parts("packages:"), [("packages", Tone::Key)]);
        assert_eq!(parts("  - ripgrep"), [("-", Tone::Quiet)]);
        assert_eq!(parts("  - firefox: 150"), [("-", Tone::Quiet), ("firefox", Tone::Key), ("150", Tone::Number)]);
        assert_eq!(parts("  - python3: 3.12.4  # the newest"), [("-", Tone::Quiet), ("python3", Tone::Key), ("3.12.4", Tone::Number), (" # the newest", Tone::Quiet)]);
        assert_eq!(parts("  - ./work.nami"), [("-", Tone::Quiet), ("./work.nami", Tone::Path)]);
        assert_eq!(parts("      ~/.gitconfig:"), [("~/.gitconfig", Tone::Path)]);
        assert_eq!(parts("        user: { name: Ada, email: ada@example.org }"), [("user", Tone::Key), ("name", Tone::InnerKey), ("email", Tone::InnerKey)]);
        assert_eq!(parts("  GITHUB_TOKEN: !secret abc"), [("GITHUB_TOKEN", Tone::Key), ("!secret", Tone::Secret)]);
        assert_eq!(parts("  EDITOR: \"hx # not a comment\""), [("EDITOR", Tone::Key), ("\"hx # not a comment\"", Tone::Quoted)]);
        assert_eq!(parts("# A comment"), [("# A comment", Tone::Quiet)]);
        assert_eq!(parts("unfree: false"), [("unfree", Tone::Key), ("false", Tone::Number)]);
        // Not a number inside a word, nor a path inside a URL.
        assert_eq!(parts("hostname: ada-laptop2"), [("hostname", Tone::Key)]);
        assert_eq!(parts("  - url: https://cache.example.org/x"), [("-", Tone::Quiet), ("url", Tone::Key)]);
        // The parts cover the line, end to end.
        let line = "  - firefox: 150 # newest";
        let s = spans(line);
        assert_eq!(s.first().unwrap().0.start, 0);
        assert_eq!(s.last().unwrap().0.end, line.len());
        assert!(s.windows(2).all(|w| w[0].0.end == w[1].0.start));
    }

    fn at(text: &str) -> (String, usize) {
        let i = text.rfind('|').unwrap();
        (format!("{}{}", &text[..i], &text[i + 1..]), i)
    }

    fn place_of(text: &str) -> Option<Place> {
        let (t, i) = at(text);
        place(&t, i)
    }

    fn key(k: &str) -> Seg {
        Seg::Key(k.into())
    }

    #[test]
    fn knows_where_the_cursor_is() {
        // A key at the top, and under one.
        let p = place_of("format: 1\npack|").unwrap();
        assert_eq!((p.path, p.what), (vec![], What::Key));
        assert_eq!(p.word, 10..14);
        let p = place_of("namios:\n  dock:\n    on: true\n  min|").unwrap();
        assert_eq!((p.path, p.what), (vec![key("namios")], What::Key));
        // A package, its version after `: ` or `@`; a list at the key's own indent.
        let p = place_of("packages:\n  - ripgrep\n  - fi|").unwrap();
        assert_eq!((p.path, p.what, p.word), (vec![key("packages")], What::Package, 26..28));
        assert_eq!(place_of("packages:\n- fire|fox").unwrap().word, 12..19);
        assert_eq!(place_of("packages:\n  - firefox: 15|").unwrap().what, What::Version("firefox".into()));
        let p = place_of("packages:\n  - firefox@1|").unwrap();
        assert_eq!((p.what, p.word), (What::Version("firefox".into()), 22..23));
        // A package's own setup, and its version there.
        let p = place_of("packages:\n  - zsh:\n      log|").unwrap();
        assert_eq!((p.path, p.what), (vec![key("packages"), Seg::Item, key("zsh")], What::Key));
        assert_eq!(place_of("packages:\n  - python3:\n      version: 3|").unwrap().what, What::Version("python3".into()));
        // A value.
        let p = place_of("updates: a|").unwrap();
        assert_eq!((p.path, p.what, p.word), (vec![], What::Value("updates".into()), 9..10));
        assert_eq!(place_of("users:\n  ada:\n    admin: |").unwrap().path, vec![key("users"), key("ada")]);
        // Nothing in a comment, a string, a block of text, or a path.
        assert_eq!(place_of("# pack|"), None);
        assert_eq!(place_of("hostname: \"ad|"), None);
        assert_eq!(place_of("shell:\n  zsh: |\n    eval|"), None);
        assert_eq!(place_of("packages:\n  - ./wo|"), None);
    }

    #[test]
    fn says_what_the_pointer_is_on() {
        let text = "packages:\n  - ripgrep\n  - zsh:\n      login-shell: true\nupdates: auto";
        let (r, t) = target(text, 16).unwrap();
        assert_eq!((&text[r], t), ("ripgrep", Target::Package("ripgrep".into())));
        assert_eq!(target(text, 27).map(|(_, t)| t), Some(Target::Package("zsh".into())));
        let at = text.find("login").unwrap();
        assert_eq!(target(text, at).map(|(_, t)| t), Some(Target::Key { path: vec![key("packages"), Seg::Item, key("zsh")], key: "login-shell".into() }));
        assert_eq!(target(text, 2).map(|(_, t)| t), Some(Target::Key { path: vec![], key: "packages".into() }));
        assert_eq!(target(text, text.len() - 1), None);
    }

    #[test]
    fn enter_keeps_the_indent_and_the_list() {
        let list = |k: &str| k == "packages";
        assert_eq!(new_line("namios:", list), NewLine::Type("  ".into()));
        assert_eq!(new_line("packages:", list), NewLine::Type("  - ".into()));
        assert_eq!(new_line("packages: ", list), NewLine::Type("  - ".into()));
        assert_eq!(new_line("  - ripgrep", list), NewLine::Type("  - ".into()));
        assert_eq!(new_line("  - zsh:", list), NewLine::Type("      ".into()));
        assert_eq!(new_line("  dock:  # the Dock", list), NewLine::Type("    ".into()));
        assert_eq!(new_line("  zsh: |", list), NewLine::Type("    ".into()));
        assert_eq!(new_line("    size: medium", list), NewLine::Type("    ".into()));
        assert_eq!(new_line("  - ", list), NewLine::EndList);
    }

    #[test]
    fn a_blocks_guide() {
        let text = "packages:\n  - ripgrep\n  - zsh:\n      login-shell: true\nupdates: auto";
        assert_eq!(block(text, 1), Some((0, 0, 3)));
        assert_eq!(block(text, 3), Some((2, 4, 3)));
        assert_eq!(block(text, 4), None);
        assert_eq!(block("packages:\n- a\n- b\nx: 1", 2), Some((0, 0, 2)));
    }

    fn mark(line: usize, message: &str, hint: Option<&str>) -> Mark {
        Mark { line, message: message.into(), hint: hint.map(Into::into), error: true }
    }

    #[test]
    fn problems_where_they_are_and_their_fixes() {
        let text = "packages:\n  - ripgrpe\nnamios:\n  dok: 1\npakages: []";
        let m = mark(2, "ripgrpe isn't in the catalog", Some("did you mean `ripgrep`?"));
        assert_eq!(m.said(), "ripgrpe isn't in the catalog (did you mean `ripgrep`?)");
        assert_eq!(m.suggestion().as_deref(), Some("ripgrep"));
        assert_eq!(mark_span(&m, "  - ripgrpe"), (4..11, true));
        let (r, with) = fix(text, &m).unwrap();
        assert_eq!((&text[r.clone()], with.as_str()), ("ripgrpe", "ripgrep"));
        let fixed = format!("{}{with}{}", &text[..r.start], &text[r.end..]);
        assert!(fixed.contains("  - ripgrep\n"));
        // The word it names comes first, though another of its words is on another line.
        let m = mark(4, "namios: dok: not a setting namiOS's desktop has", Some("did you mean `dock`?"));
        assert_eq!(fix(text, &m).map(|(r, w)| (&text[r], w)), Some(("dok", "dock".into())));
        let m = mark(5, "unknown key `pakages`", Some("did you mean `packages`? or this file needs a newer nami (nami self-update)"));
        assert_eq!(fix(text, &m).map(|(r, w)| (&text[r], w)), Some(("pakages", "packages".into())));
        // A key named by its path.
        let m = mark(4, "env.GITHUB_TOKEN: this secret isn't sealed yet", None);
        assert_eq!(mark_span(&m, "  GITHUB_TOKEN: !secret abc"), (2..14, true));
        // A suggestion in the message itself, without quotes.
        let m = mark(4, "wifi.home: pasword: did you mean password?", None);
        assert_eq!(m.suggestion().as_deref(), Some("password"));
        // No suggestion: no fix, and the line's words marked.
        let m = mark(4, "something's wrong", None);
        assert_eq!(fix(text, &m), None);
        assert_eq!(mark_span(&m, "  dok: 1  # here"), (2..8, false));
        // A line past the end: nothing.
        assert_eq!(fix(text, &mark(40, "x", Some("did you mean `y`?"))), None);
    }

    #[test]
    fn code_in_what_nami_says() {
        let parts = |t: &str| code_spans(t).into_iter().map(|(p, c)| (p, c)).collect::<Vec<_>>();
        assert_eq!(parts("ripgrpe isn't in the catalog (did you mean `ripgrep`?)"), [("ripgrpe isn't in the catalog (did you mean ".into(), false), ("ripgrep".into(), true), ("?)".into(), false)]);
        assert_eq!(parts("`a` or `b`"), [("a".into(), true), (" or ".into(), false), ("b".into(), true)]);
        assert_eq!(parts("no code"), [("no code".into(), false)]);
        // A backtick on its own stays.
        assert_eq!(parts("it's `x` and ` alone"), [("it's ".into(), false), ("x".into(), true), (" and ` alone".into(), false)]);
    }

    fn knowledge() -> Knowledge {
        Knowledge {
            schema: serde_json::json!({
                "properties": {
                    "packages": { "type": "array", "description": "What to install, - name a line. More.", "markdownDescription": "What to install, `- name` a line. A second sentence. A third one.\n\n```yaml\npackages:\n  - htop\n```" },
                    "updates": { "enum": ["auto", "manual"], "description": "How updates come." },
                    "unfree": { "type": "boolean", "description": "Whether software that isn't free may be installed." },
                    "recipes": { "not": {}, "description": "recipes: is gone" },
                    "keep-generations": { "deprecationMessage": "now history", "description": "now history" },
                    "users": { "type": "object", "additionalProperties": { "type": "object", "properties": { "admin": { "type": "boolean", "description": "May change the computer (sudo, settings for everyone)." } } } },
                    "wifi": { "type": "object" },
                }
            }),
            options: vec![
                Opt { key: "wifi".into(), kind: "name: { password, ssid, hidden }".into(), ..Default::default() },
                Opt { key: "login-shell (user.nami)".into(), kind: "zsh | fish".into(), summary: "The shell your terminals open: their bash hands over to it. More.".into(), example: "packages:\n  - zsh:\n      login-shell: true".into() },
                Opt { key: "login-screen".into(), kind: "gdm or greetd".into(), ..Default::default() },
                Opt { key: "with (packages: python3)".into(), kind: "a list of names".into(), summary: "A Python's packages, from its own set. More.".into(), example: "packages:\n  - python3:\n      with: [numpy]".into() },
            ],
            namios: vec!["dock".into(), "minimize".into()],
        }
    }

    #[test]
    fn what_fits_where() {
        let k = knowledge();
        let labels = |items: Vec<Item>| items.into_iter().map(|i| i.label).collect::<Vec<_>>();
        assert_eq!(labels(k.keys(&[])), ["packages", "unfree", "updates", "users", "wifi"]);
        assert_eq!(k.keys(&[])[0].about, "What to install, - name a line");
        assert_eq!(labels(k.keys(&[key("namios")])), ["dock", "minimize"]);
        assert_eq!(labels(k.keys(&[key("users"), key("ada")])), ["admin"]);
        assert_eq!(labels(k.keys(&[key("wifi"), key("home")])), ["password", "ssid", "hidden"]);
        assert_eq!(labels(k.keys(&[key("packages"), Seg::Item, key("zsh")])), ["version", "login-shell"]);
        assert_eq!(labels(k.keys(&[key("packages"), Seg::Item, key("python313")])), ["version", "login-shell", "with"]);
        assert_eq!(k.keys(&[key("packages"), Seg::Item, key("python3")])[2].about, "A Python's packages, from its own set");
        assert_eq!(k.help(&[key("packages"), Seg::Item, key("python3")], "with").unwrap().example, "packages:\n  - python3:\n      with: [numpy]");
        assert_eq!(labels(k.values(&[], "updates")), ["auto", "manual"]);
        assert_eq!(labels(k.values(&[], "unfree")), ["true", "false"]);
        assert_eq!(labels(k.values(&[key("users"), key("ada")], "admin")), ["true", "false"]);
        assert_eq!(labels(k.values(&[], "login-screen")), ["gdm", "greetd"]);
        assert!(k.values(&[], "packages").is_empty());
        assert!(k.is_list(&[], "packages") && !k.is_list(&[], "updates"));
        let h = k.help(&[], "packages").unwrap();
        assert_eq!(h.summary, "What to install, - name a line. A second sentence.");
        assert_eq!(h.example, "packages:\n  - htop");
        assert_eq!(k.help(&[key("users"), key("ada")], "admin").unwrap().summary, "May change the computer (sudo, settings for everyone).");
        assert_eq!(k.help(&[key("packages"), Seg::Item, key("zsh")], "login-shell").unwrap().example, "packages:\n  - zsh:\n      login-shell: true");
        // Those that start with it first, then those that have it.
        assert_eq!(labels(matching(k.keys(&[]), "u")), ["unfree", "updates", "users"]);
        assert_eq!(labels(matching(k.keys(&[]), "Up")), ["updates"]);
        assert_eq!(labels(matching(k.keys(&[]), "free")), ["unfree"]);
    }
}
