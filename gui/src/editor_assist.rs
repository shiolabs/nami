//! What the editor asks nami, always off the window's thread: what nami knows of a kind of file
//! (`nami options --schema <kind>`, `nami options --json`), the catalog's packages and a
//! package's versions as Tab completion has them (`nami __complete`), and a package's version
//! and what it is (`nami info --json`). Each answer is kept: it's asked once. Without nami (the
//! Installer, on Windows or macOS), nothing.

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

use super::lang::{self, Item, Knowledge, What};

/// The nami command: `NAMI` (or tray's `TRAY`), else the one on PATH.
fn nami() -> Command {
    let program = std::env::var_os("NAMI").or_else(|| std::env::var_os("TRAY")).unwrap_or_else(|| "nami".into());
    let mut c = Command::new(program);
    c.stdin(Stdio::null()).stderr(Stdio::null());
    c
}

fn said(args: &[&str]) -> Option<String> {
    let out = nami().args(args).output().ok()?;
    String::from_utf8(out.stdout).ok()
}

fn known() -> &'static Mutex<HashMap<String, Arc<Knowledge>>> {
    static KNOWN: OnceLock<Mutex<HashMap<String, Arc<Knowledge>>>> = OnceLock::new();
    KNOWN.get_or_init(Default::default)
}

/// What nami knows of a `kind` file, asked once.
pub fn knowledge(kind: &str) -> Arc<Knowledge> {
    if let Some(k) = cached(kind) {
        return k;
    }
    let schema = said(&["options", "--schema", kind]).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let options = said(&["options", "--json"]).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let namios = if matches!(kind, "user" | "set") {
        complete(&["config", "set", "--file", "/dev/null", "namios."]).into_iter().filter_map(|(v, _)| v.strip_prefix("namios.").map(str::to_string)).filter(|v| !v.is_empty() && !v.ends_with('.')).collect()
    } else {
        Vec::new()
    };
    let k = Arc::new(Knowledge { schema, options, namios });
    known().lock().unwrap().insert(kind.to_string(), k.clone());
    k
}

/// The same, only if it's been asked already (for what can't wait: Enter).
pub fn cached(kind: &str) -> Option<Arc<Knowledge>> {
    known().lock().unwrap().get(kind).cloned()
}

/// What Tab completion says fits after `nami <words>`: each value and its few words.
fn complete(words: &[&str]) -> Vec<(String, String)> {
    let mut args = vec!["__complete", "fish", "--"];
    args.extend_from_slice(words);
    let Some(text) = said(&args) else { return Vec::new() };
    text.lines()
        .filter(|l| !l.starts_with(':') && !l.is_empty())
        .map(|l| match l.split_once('\t') {
            Some((v, a)) => (v.to_string(), a.to_string()),
            None => (l.to_string(), String::new()),
        })
        .collect()
}

/// What fits where the cursor is, and the word it replaces.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub word: std::ops::Range<usize>,
    pub items: Vec<Item>,
}

/// What fits at `at` in `text`, a `kind` file. Without `asked` (Ctrl+Space), only once
/// something's typed, or a value's begun.
pub fn found(text: &str, at: usize, kind: &str, asked: bool) -> Option<Found> {
    let place = lang::place(text, at)?;
    let typed = &text[place.word.start..at];
    let items = match &place.what {
        What::Key => {
            if typed.is_empty() && !asked {
                return None;
            }
            // (A key with its colon there already: only its name.)
            let colon = text[place.word.end..].starts_with(':');
            knowledge(kind).keys(&place.path).into_iter().map(|i| Item { insert: if colon { i.label.clone() } else { format!("{}: ", i.label) }, ..i }).collect()
        }
        What::Value(key) => knowledge(kind).values(&place.path, key),
        What::Package => {
            if typed.is_empty() && !asked {
                return None;
            }
            complete(&["install", typed]).into_iter().map(|(v, about)| Item { label: v.clone(), insert: v, about }).collect()
        }
        What::Version(package) => complete(&["install", &format!("{package}@")])
            .into_iter()
            .filter_map(|(v, about)| {
                let line = v.strip_prefix(&format!("{package}@"))?.to_string();
                Some(Item { label: line.clone(), insert: line, about })
            })
            .collect(),
    };
    let items = lang::matching(items, typed);
    // (What's typed is all there is: nothing to offer.)
    if items.is_empty() || (items.len() == 1 && items[0].label == typed) {
        return None;
    }
    Some(Found { word: place.word, items })
}

/// A package's version, and what it is (`nami info --json`), asked once.
pub fn package(name: &str) -> Option<(String, String)> {
    static SEEN: OnceLock<Mutex<HashMap<String, Option<(String, String)>>>> = OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    if let Some(p) = seen.lock().unwrap().get(name) {
        return p.clone();
    }
    #[derive(serde::Deserialize)]
    struct Info {
        #[serde(default)]
        version: String,
        #[serde(default)]
        summary: Option<String>,
        #[serde(default)]
        description: Option<String>,
    }
    let info = said(&["info", name, "--json"])
        .and_then(|t| serde_json::from_str::<Vec<Info>>(&t).ok())
        .and_then(|v| v.into_iter().next())
        .map(|i| (i.version, i.summary.filter(|s| !s.is_empty()).or(i.description).unwrap_or_default()));
    seen.lock().unwrap().insert(name.to_string(), info.clone());
    info
}
