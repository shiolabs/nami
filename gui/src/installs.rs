//! Apps asked for from outside Settings' window (the launcher), and how
//! Settings' installs go, for the desktop to show and tell. Settings does
//! the installing, one app at a time (`jobs`); this is only what passes
//! between it and the desktop: files in $XDG_RUNTIME_DIR.
//!
//! - `nami-gui-installs`: apps asked for, a line each (`<ms> <attr>`), read
//!   and emptied by the Settings that's running.
//! - `nami-gui-jobs.json`: Settings' apps going on and off, and how each
//!   went, written each time that changes.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How an app Settings was asked to install (or remove) goes.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Told {
    pub attr: String,
    /// "install", "remove" or "update".
    pub kind: String,
    /// "waiting", "going", "done" or "failed".
    pub state: String,
    /// What nami said, when it failed.
    #[serde(default)]
    pub said: String,
    /// When it was asked for (ms since 1970).
    pub asked: u64,
    /// The file it changes, and how: to try again after a Settings that's gone.
    #[serde(default)]
    pub file: PathBuf,
    #[serde(default)]
    pub ops: Vec<Value>,
}

/// How long something asked for stays asked: a line older than this is from
/// a Settings that never came, not for now.
const FRESH_MS: u64 = 2 * 60 * 1000;

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn runtime(name: &str) -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join(name))
}

/// Ask for `attr` to be installed: the Settings running (or starting) takes it.
pub fn ask(attr: &str) -> Result<(), String> {
    use std::io::Write;
    let file = runtime("nami-gui-installs").ok_or("no XDG_RUNTIME_DIR")?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    writeln!(f, "{} {attr}", now_ms()).map_err(|e| e.to_string())
}

/// What's been asked for and not taken yet: taken now, each once.
pub fn take_asked() -> Vec<String> {
    let Some(file) = runtime("nami-gui-installs") else { return Vec::new() };
    let taking = file.with_extension("taking");
    if std::fs::rename(&file, &taking).is_err() {
        return Vec::new();
    }
    let text = std::fs::read_to_string(&taking).unwrap_or_default();
    let _ = std::fs::remove_file(&taking);
    asked_in(&text, now_ms())
}

fn asked_in(text: &str, now: u64) -> Vec<String> {
    let mut asked: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some((at, attr)) = line.trim().split_once(' ') else { continue };
        let fresh = at.parse::<u64>().is_ok_and(|at| at + FRESH_MS >= now);
        // (Flathub's: `flathub:org.example.App`.)
        let name_ok = !attr.is_empty() && attr.chars().all(|c| c.is_ascii_alphanumeric() || "-_.+:".contains(c));
        if fresh && name_ok && !asked.iter().any(|a| a == attr) {
            asked.push(attr.to_string());
        }
    }
    asked
}

/// How Settings' installs go, as it last wrote.
pub fn read() -> Vec<Told> {
    let Some(file) = runtime("nami-gui-jobs.json") else { return Vec::new() };
    std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Written whole, at once: what reads it never sees half.
pub fn write(all: &[Told]) {
    let Some(file) = runtime("nami-gui-jobs.json") else { return };
    let new = file.with_extension("new");
    if serde_json::to_vec(all).ok().is_some_and(|text| std::fs::write(&new, text).is_ok()) {
        let _ = std::fs::rename(&new, &file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_whats_fresh_and_a_name_is_asked() {
        let now = 10_000_000;
        let text = format!("{} xterm\n{} geogebra\n{} old\n{} rm -rf\n{} xterm\nnonsense\n{} flathub:com.github.tchx84.Flatseal\n{} a;b\n", now - 5, now, now - FRESH_MS - 1, now, now, now, now);
        assert_eq!(asked_in(&text, now), vec!["xterm", "geogebra", "flathub:com.github.tchx84.Flatseal"]);
    }
}
