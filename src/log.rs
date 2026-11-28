//! Progress messages. Normally stderr; while the daemon serves a request they
//! go back to the client instead, so users see what's happening on their behalf.
//!
//! What's said is for machines as much as people (Settings reads
//! `fetching 48 paths (210.4 MiB unpacked)` and each path as it lands): in a
//! pipe it goes out as it is. A terminal gets it as one calm line that
//! changes as it goes (`Downloading 12 of 48 packages · 220 MB`), and what
//! it came to once it's done (`Downloaded 48 packages · 220 MB`).
//!
//! A line that ends with `…` (`status`) is progress said only on a terminal:
//! the live line (`Building the new system…`), which downloads then count
//! under (`Building the new system · 214 of 512…`). In a pipe it isn't said.

use std::io::{IsTerminal, Write};
use std::sync::Mutex;

type Sink = Box<dyn Fn(&str) + Send>;

static SINK: Mutex<Option<Sink>> = Mutex::new(None);

pub fn msg(line: &str) {
    match &*SINK.lock().unwrap() {
        Some(sink) => sink(line),
        // (A sink, the daemon's client, gets it plain: it colours for its own terminal.)
        None => to_stderr(line),
    }
}

/// A line for stderr: on a terminal, progress as one line; else as it is
/// (a `status` line not at all: it's for a terminal).
pub fn to_stderr(line: &str) {
    if std::io::stderr().is_terminal() {
        LIVE.lock().unwrap().say(line, &mut std::io::stderr());
    } else if !is_status(line) {
        eprintln!("{}", crate::style::err().lines(line, crate::style::Paint::line));
    }
}

/// What's going on now, as the live line on a terminal (`Compressing 12 of
/// 38 parts…`: it ends with `…`); the next replaces it. Not said in a pipe,
/// but to the daemon's client, which may be a terminal (NAMI_FOR).
pub fn status(text: &str) {
    let text = if text.ends_with('…') { text.to_string() } else { format!("{text}…") };
    match &*SINK.lock().unwrap() {
        Some(sink) => sink(&text),
        None if std::io::stderr().is_terminal() => LIVE.lock().unwrap().say(&text, &mut std::io::stderr()),
        None if std::env::var_os("NAMI_FOR").is_some() => eprintln!("{text}"),
        None => {}
    }
}

/// A step of a long job (`Formatting`): the live line on a terminal, as
/// `status`; in a pipe, a line of its own (a log keeps what it went through).
pub fn step(text: &str) {
    if std::io::stderr().is_terminal() || SINK.lock().unwrap().is_some() {
        status(text);
    } else {
        eprintln!("{}", text.trim_end_matches('…'));
    }
}

/// Whether a line is a `status` line: one line of progress, for a terminal.
pub fn is_status(line: &str) -> bool {
    line.ends_with('…') && !line.starts_with(' ')
}

/// Done with progress for now (before what a command prints at its end).
pub fn settle() {
    if let Ok(mut live) = LIVE.lock() {
        live.end(&mut std::io::stderr());
    }
}

/// Send messages to `sink` until the returned guard is dropped.
pub fn redirect(sink: Sink) -> Guard {
    *SINK.lock().unwrap() = Some(sink);
    Guard
}

pub struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        *SINK.lock().unwrap() = None;
    }
}

#[macro_export]
macro_rules! say {
    ($($arg:tt)*) => { $crate::log::msg(&format!($($arg)*)) };
}

static LIVE: Mutex<Live> = Mutex::new(Live { total: 0, done: 0, bytes: 0, now: None, title: None, shown: false, removing: false });

/// Progress on a terminal: what's being fetched, how far along, said on one line.
#[derive(Debug, Default)]
pub struct Live {
    total: usize,
    done: usize,
    bytes: u64,
    /// What's happening besides downloading (a build), said instead.
    now: Option<String>,
    /// What it all is (`Building the new system`): downloads and builds are said under it.
    title: Option<String>,
    /// Whether a line is on the screen to be written over.
    shown: bool,
    /// What's counted is being removed (`nami gc`), not fetched.
    removing: bool,
}

impl Live {
    /// One line of what nami says, on a terminal.
    pub fn say(&mut self, line: &str, out: &mut impl Write) {
        let p = crate::style::err();
        if is_status(line) {
            self.title = Some(line.trim_end_matches('…').to_string());
            self.now = None;
            return self.draw(out);
        }
        // (The system's packages, as Settings reads it: the title says it here.)
        if line.starts_with("getting ") && line.contains(" packages") && self.title.is_some() {
            return;
        }
        // `nami gc`: what goes, then each path as it goes (as fetching says them).
        if let Some(rest) = line.strip_prefix("removing ") {
            let mut words = rest.split_whitespace();
            if let (Some(n), Some("paths")) = (words.next().and_then(|n| n.parse::<usize>().ok()), words.next()) {
                self.total += n;
                self.bytes += words.next().and_then(|m| m.trim_start_matches('(').parse::<f64>().ok()).map_or(0, |mib| (mib * 1048576.0) as u64);
                self.removing = true;
                self.now = None;
                return self.draw(out);
            }
        }
        if let Some(rest) = line.strip_prefix("fetching ") {
            let mut words = rest.split_whitespace();
            if let (Some(n), Some("paths")) = (words.next().and_then(|n| n.parse::<usize>().ok()), words.next()) {
                self.total += n;
                self.bytes += words.next().and_then(|m| m.trim_start_matches('(').parse::<f64>().ok()).map_or(0, |mib| (mib * 1048576.0) as u64);
                self.now = None;
                return self.draw(out);
            }
            // A source for a build: its file's name.
            if rest.contains("://") && !rest.contains(' ') {
                self.now = Some(format!("Downloading {}", rest.rsplit('/').next().unwrap_or(rest)));
                return self.draw(out);
            }
        }
        if landed(line) && self.total > 0 {
            self.done += 1;
            if self.done >= self.total {
                let did = if self.removing { "Removed" } else { "Downloaded" };
                let said = format!("{did} {}{}", crate::tell::count(self.total, "package", "packages"), self.size());
                self.total = 0;
                self.done = 0;
                self.bytes = 0;
                self.removing = false;
                self.clear(out);
                let _ = writeln!(out, "{}", p.dim(&said));
                return;
            }
            return self.draw(out);
        }
        if let Some(rest) = line.strip_prefix("building ").or(line.strip_prefix("compressing ")) {
            let what = rest.split(" -> ").next().unwrap_or(rest);
            let what = what.split(" (").next().unwrap_or(what);
            // (A recipe's folder, `../packages/sample-ninja`: its name.)
            let what = what.trim_end_matches('/').rsplit('/').next().unwrap_or(what);
            let what = if line.starts_with("building ") { crate::tell::shown(what) } else { format!("compressing {what}") };
            self.now = Some(match &self.title {
                Some(t) => format!("{t} · {what}"),
                None => format!("Building {what}"),
            });
            return self.draw(out);
        }
        if let Some(rest) = line.strip_prefix("built ") {
            let names: Vec<String> = rest.split_whitespace().map(|p| crate::tell::shown(crate::store::base_name(p).get(33..).unwrap_or(p))).collect();
            self.now = None;
            self.clear(out);
            let _ = writeln!(out, "{}", p.dim(&format!("Built {}", names.first().cloned().unwrap_or_default())));
            return self.draw(out);
        }
        self.clear(out);
        let _ = writeln!(out, "{}", p.lines(line, crate::style::Paint::line));
        // What it came to (✓ Switched now, an error, a change's titles): nothing's going on now.
        if ["✓ ", "✗ ", "⚠ ", "error:"].iter().any(|m| line.starts_with(m)) || crate::tell::HEADINGS.contains(&line) {
            self.title = None;
            self.now = None;
        }
        self.draw(out);
    }

    fn size(&self) -> String {
        if self.bytes > 0 { format!(" · {}", crate::tell::size(self.bytes)) } else { String::new() }
    }

    fn draw(&mut self, out: &mut impl Write) {
        let text = match (&self.now, &self.title) {
            (Some(now), _) => format!("{now}…"),
            (None, Some(t)) if self.total > 0 => format!("{t} · {} of {}…", self.done + 1, self.total),
            (None, None) if self.total > 0 && self.removing => format!("Removing {} of {}{}…", self.done + 1, crate::tell::count(self.total, "package", "packages"), self.size()),
            (None, None) if self.total > 0 => format!("Downloading {} of {}{}…", self.done + 1, crate::tell::count(self.total, "package", "packages"), self.size()),
            (None, Some(t)) => format!("{t}…"),
            (None, None) => return,
        };
        let _ = write!(out, "\r\x1b[K{}", crate::style::err().dim(&text));
        let _ = out.flush();
        self.shown = true;
    }

    /// Done: nothing on the line, nothing going on.
    pub fn end(&mut self, out: &mut impl Write) {
        self.title = None;
        self.now = None;
        self.clear(out);
    }

    fn clear(&mut self, out: &mut impl Write) {
        if self.shown {
            let _ = write!(out, "\r\x1b[K");
            let _ = out.flush();
            self.shown = false;
        }
    }
}

/// A store path that landed: `  <32 base32 characters>-name`.
fn landed(line: &str) -> bool {
    line.strip_prefix("  ").and_then(|r| r.split_at_checked(32)).is_some_and(|(hash, name)| {
        hash.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()) && name.starts_with('-') && !name.contains(' ')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(lines: &[&str]) -> String {
        let mut live = Live::default();
        let mut out = Vec::new();
        for l in lines {
            live.say(l, &mut out);
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn downloads_on_one_line() {
        let out = said(&["fetching 2 paths (99.2 MiB unpacked)", "  0y5xmdb7qfvimjwbq7ibg1xdgkgjwqng-firefox-160.0", "  5sgjjiqn90zmvlvlgadw7ss00nn3hpjb-gnugrep-3.12"]);
        assert!(out.contains("\r\x1b[KDownloading 1 of 2 packages · 104 MB…"));
        assert!(out.contains("\r\x1b[KDownloading 2 of 2 packages · 104 MB…"));
        assert!(out.ends_with("\r\x1b[KDownloaded 2 packages · 104 MB\n"), "{out:?}");
        assert!(!out.contains("gnugrep"), "no store paths");
    }

    #[test]
    fn other_lines_between() {
        let out = said(&["fetching 2 paths (1.0 MiB unpacked)", "warning: x", "building hello-2.12.1 -> /nix/store/fp9wvk0j583a8sjh5ccdzzdm46y1ryy9-hello-2.12.1"]);
        assert!(out.contains("\r\x1b[Kwarning: x\n\r\x1b[KDownloading 1 of 2"));
        assert!(out.ends_with("\r\x1b[KBuilding hello 2.12.1…"));
    }

    #[test]
    fn under_a_title() {
        let out = said(&["Building the new system…", "getting 512 packages (only what's not here yet)", "fetching 2 paths (1.0 MiB unpacked)", "  0y5xmdb7qfvimjwbq7ibg1xdgkgjwqng-firefox-160.0"]);
        assert!(out.contains("\r\x1b[KBuilding the new system…"), "{out:?}");
        assert!(out.contains("\r\x1b[KBuilding the new system · 2 of 2…"), "{out:?}");
        assert!(!out.contains("getting"), "the title says it");
        let out = said(&["Building the new system…", "building niri-26.04 -> /nix/store/x", "Apps", "  + htop 3.4"]);
        assert!(out.contains("Building the new system · niri 26.04…"), "{out:?}");
        assert!(out.ends_with("  + htop 3.4\n"), "no title after what it came to: {out:?}");
        assert!(is_status("Compressing 3 of 38 parts…") && !is_status("  a…") && !is_status("done"));
    }
}
