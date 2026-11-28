//! How nami says what a change did, in a terminal and out of one: the same
//! words, coloured by `style::Paint::line` (so a change the daemon made reads
//! the same as one made here). Changes go by kind (Apps, Services, Files,
//! Caches), each `+` added, `−` removed, `~` changed; the end is one line,
//! `✓ Installed firefox 160.0 · in system.nami · opens from the launcher`,
//! and how to undo it. Sizes and times are said as people say them.

use std::sync::Mutex;
use std::time::Instant;

/// `2.3 GB`, `104 MB`, `512 KB` (decimal, as a Mac says sizes).
pub fn size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e7 {
        format!("{:.0} MB", b / 1e6)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{bytes} bytes")
    }
}

/// `0.4 s`, `12 s`, `2 min 5 s`.
pub fn duration(d: std::time::Duration) -> String {
    let s = d.as_secs_f64();
    if s < 10.0 {
        format!("{s:.1} s")
    } else if s < 60.0 {
        format!("{:.0} s", s)
    } else {
        let s = s.round() as u64;
        if s % 60 == 0 { format!("{} min", s / 60) } else { format!("{} min {} s", s / 60, s % 60) }
    }
}

/// When, from now: `just now`, `5 minutes ago`, `yesterday`, `Nov 3`, `Nov 3, 2025`.
pub fn ago(then: u64, now: u64) -> String {
    if then == 0 {
        return "as it came".into();
    }
    let d = now.saturating_sub(then);
    let plural = |n: u64, w: &str| if n == 1 { format!("1 {w} ago") } else { format!("{n} {w}s ago") };
    match d {
        0..60 => "just now".into(),
        60..3600 => plural(d / 60, "minute"),
        3600..86400 => plural(d / 3600, "hour"),
        86400..172800 => "yesterday".into(),
        172800..604800 => format!("{} days ago", d / 86400),
        _ => {
            let date = crate::profile::format_time(then);
            let (y, md) = (&date[..4], crate::show::span(Some(&date[..10]), Some(&date[..10])));
            // (`Nov 3 – Nov 3, 2025`: one day, said once.)
            let one = md.split(" – ").next().unwrap_or(&md).to_string();
            if now.saturating_sub(then) < 300 * 86400 && crate::profile::format_time(now).starts_with(y) { one } else { format!("{one}, {y}") }
        }
    }
}

/// Now, in seconds.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A store name, as its name and version: `firefox-160.0` -> (`firefox`, `160.0`).
pub fn name_version(name: &str) -> (String, String) {
    let v = crate::query::version_of(name);
    match name.strip_suffix(&format!("-{v}")) {
        Some(n) if !v.is_empty() => (n.to_string(), v),
        _ => (name.to_string(), String::new()),
    }
}

/// `firefox 160.0`.
pub fn shown(name: &str) -> String {
    let (n, v) = name_version(name);
    if v.is_empty() { n } else { format!("{n} {v}") }
}

/// `a`, `a and b`, `a, b and c`.
pub fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `146202` -> `146,202`.
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A catalog's size: `146,202 packages`, and where this computer can't install all of them (a
/// Mac: only what's ready-built), how many it can: `129,851 packages, 73,502 for this Mac`.
pub fn catalog_size(packages: usize, installable: Option<usize>) -> String {
    let here = if crate::index::MAC { "this Mac" } else { "ARM Linux" };
    match installable {
        Some(n) if n < packages => format!("{} packages, {} for {here}", thousands(packages), thousands(n)),
        _ => format!("{} packages", thousands(packages)),
    }
}

/// `1 app`, `3 apps`.
pub fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

/// The titles of the kinds of change, as `Paint::line` knows them (bold).
pub const HEADINGS: &[&str] = &["Apps", "Services", "Files", "Caches", "Settings", "Updates"];

/// What a change does, by kind: a title, then its lines (`  + firefox 160.0`).
pub fn grouped(groups: &[(&str, Vec<String>)]) -> Vec<String> {
    let mut out = Vec::new();
    for (title, lines) in groups.iter().filter(|(_, l)| !l.is_empty()) {
        out.push(title.to_string());
        out.extend(lines.iter().map(|l| format!("  {l}")));
    }
    out
}

/// Store names that came and went, as a change's lines: one in both (by
/// name) is a new version (`~ git 2.50.0 → 2.51.0`), else `+ htop 3.4.1`, `− jq 1.7.1`.
pub fn paired(added: &[String], removed: &[String]) -> Vec<String> {
    let name = |n: &String| name_version(n).0;
    let mut lines = Vec::new();
    for a in added {
        match removed.iter().find(|r| name(r) == name(a)) {
            Some(r) => {
                let ((n, from), (_, to)) = (name_version(r), name_version(a));
                lines.push(if from.is_empty() || to.is_empty() || from == to { format!("~ {n}") } else { format!("~ {n} {from} → {to}") });
            }
            None => lines.push(format!("+ {}", shown(a))),
        }
    }
    lines.extend(removed.iter().filter(|r| !added.iter().any(|a| name(a) == name(r))).map(|r| format!("− {}", shown(r))));
    lines
}

/// What a command set out to do, for its last line.
#[derive(Debug, Clone)]
pub enum Intent {
    Install { names: Vec<String>, file: String },
    Remove { names: Vec<String>, file: String },
    Upgrade,
}

/// What the last change did: its step, and what came and went.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub generation: Option<u64>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: usize,
    /// Apps there before at another version (their recipe changed): (old name, new name).
    pub updated: Vec<(String, String)>,
    pub size_added: u64,
    pub size_removed: u64,
    /// The apps that came, as entries (to say how they open).
    pub opens: Vec<(String, Opens)>,
}

/// How to undo a change: the computer's with sudo.
pub fn undo_command(computer: bool) -> &'static str {
    if computer { "sudo nami undo" } else { "nami undo" }
}

/// How an app that came opens.
#[derive(Debug, Clone, PartialEq)]
pub enum Opens {
    Launcher,
    Command(String),
    Unknown,
}

static START: Mutex<Option<Instant>> = Mutex::new(None);
static INTENT: Mutex<Option<Intent>> = Mutex::new(None);
static OUTCOME: Mutex<Option<Outcome>> = Mutex::new(None);

/// As nami starts: what "Done in 12 s" counts from.
pub fn start() {
    *START.lock().unwrap() = Some(Instant::now());
}

pub fn set_intent(i: Intent) {
    *INTENT.lock().unwrap() = Some(i);
}

pub fn set_outcome(o: Outcome) {
    *OUTCOME.lock().unwrap() = Some(o);
}

/// What `cleanup:` says, for the hint after a removal or an update: after how long old
/// changes go (none: `off`), and whether whoever runs nami frees the space at once with
/// `nami gc --old` (the computer's, as root; a person's goes with the next daily cleanup).
static CLEANUP: Mutex<Option<(Option<String>, bool)>> = Mutex::new(None);

pub fn set_cleanup(after: Option<String>, frees_now: bool) {
    *CLEANUP.lock().unwrap() = Some((after, frees_now));
}

/// What the old change still holds, and when it goes: `450 MB can be freed now with nami
/// gc --old · or it goes by itself after 30 days` (`many`: old versions, after an update).
pub fn freeable(bytes: u64, many: bool) -> Option<String> {
    if bytes < 1_000_000 {
        return None;
    }
    let (after, now) = CLEANUP.lock().unwrap().clone().unwrap_or((Some(crate::cleanup::duration_words(crate::cleanup::AFTER)), false));
    let goes = if many { "they go by themselves" } else { "it goes by itself" };
    let size = size(bytes);
    let said = match (after, now) {
        (Some(after), true) => format!("{size} can be freed now with nami gc --old · or {goes} after {after}"),
        (None, true) => format!("{size} can be freed now with nami gc --old"),
        (Some(after), false) => format!("{size} is freed by itself after {after} · or sooner with nami gc --old"),
        (None, false) => format!("{size} stays for nami undo · nami gc --old lets it go"),
    };
    Some(if many { format!("  Old versions: {said}") } else { format!("  {said}") })
}

fn elapsed() -> Option<std::time::Duration> {
    START.lock().unwrap().map(|s| s.elapsed())
}

/// How an installed package opens: from the launcher (it has a .desktop
/// file), or by a command (its program, named like it when it can be).
pub fn opens(attr: &str, paths: &[String], phys: impl Fn(&str) -> std::path::PathBuf) -> Opens {
    let has = |p: &str, sub: &str| std::fs::read_dir(phys(p).join(sub)).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>();
    if paths.iter().any(|p| has(p, "share/applications").iter().any(|f| f.ends_with(".desktop"))) {
        return Opens::Launcher;
    }
    let bins: Vec<String> = paths.iter().flat_map(|p| has(p, "bin")).filter(|b| !b.starts_with('.')).collect();
    let base = crate::index::split_version(attr).0;
    let base = base.rsplit('/').next().unwrap_or(base);
    match bins.iter().find(|b| *b == base).or(bins.iter().min_by_key(|b| b.len())) {
        Some(b) => Opens::Command(b.clone()),
        None => Opens::Unknown,
    }
}

/// The last line of a change: what it did, where, and how to undo it. `file`: the nami file
/// an apply made so, as it's said (`~/user.nami`, `system.nami`); `computer`: the
/// change was the computer's (undone with sudo).
pub fn finish(file: &str, computer: bool) -> Vec<String> {
    let intent = INTENT.lock().unwrap().take();
    let outcome = OUTCOME.lock().unwrap().take();
    let disk = |o: &Outcome| match (o.size_added, o.size_removed) {
        (a, r) if a > r && a - r >= 1_000_000 => Some(format!("{} more on disk", size(a - r))),
        (a, r) if r > a && r - a >= 1_000_000 => Some(format!("{} less in use", size(r - a))),
        _ => None,
    };
    let undo_cmd = undo_command(computer);
    let undo = format!("  Undo with {undo_cmd}");
    match (intent, outcome) {
        (Some(Intent::Install { names, file }), o) => {
            let o = o.unwrap_or_default();
            // (A package's folder, /etc/nami/packages/nami-gui, by its name: nami-gui.)
            let plain = |n: &String| if crate::build::Builder::is_recipe(n) { crate::config::entry_name(n).to_string() } else { n.clone() };
            let came: Vec<String> = names.iter().map(|n| o.added.iter().find(|a| same(a, n)).map(|a| shown(a)).unwrap_or_else(|| plain(n))).collect();
            // One that was there at another version (its recipe changed, or another version
            // asked for): updated, from what to what.
            let updated = |n: &String| {
                let (r, a) = match o.updated.iter().find(|(_, a)| same(a, n)) {
                    Some((r, a)) => (r, a),
                    None => (o.removed.iter().find(|r| same(r, n))?, o.added.iter().find(|a| same(a, n))?),
                };
                let ((name, from), (_, to)) = (name_version(r), name_version(a));
                Some(if from.is_empty() || to.is_empty() || from == to { name } else { format!("{name} {from} → {to}") })
            };
            let new = names.iter().any(|n| o.added.iter().any(|a| same(a, n)) || updated(n).is_some());
            if !new {
                let be = if names.len() == 1 { "is" } else { "are" };
                let mut out = vec![format!("✓ {} {be} already installed · in {file}", and_list(&came))];
                // (What else the file had waiting came with it, said above.)
                out.extend(o.generation.map(|_| undo.clone()));
                return out;
            }
            let (ups, fresh): (Vec<(usize, Option<String>)>, Vec<(usize, Option<String>)>) = names.iter().map(updated).enumerate().partition(|(_, u)| u.is_some());
            let ups: Vec<String> = ups.into_iter().filter_map(|(_, u)| u).collect();
            let fresh: Vec<String> = fresh.into_iter().map(|(i, _)| came[i].clone()).collect();
            let mut parts = match (fresh.is_empty(), ups.is_empty()) {
                (true, _) => vec![format!("✓ Updated {}", and_list(&ups))],
                (false, true) => vec![format!("✓ Installed {}", and_list(&fresh))],
                (false, false) => vec![format!("✓ Installed {}", and_list(&fresh)), format!("updated {}", and_list(&ups))],
            };
            parts.push(format!("in {file}"));
            parts.extend(disk(&o));
            if let ([(_, how)], true) = (o.opens.as_slice(), ups.is_empty()) {
                match how {
                    Opens::Launcher => parts.push("opens from the launcher".into()),
                    Opens::Command(c) => parts.push(format!("run it with {c}")),
                    Opens::Unknown => {}
                }
            }
            vec![parts.join(" · "), undo]
        }
        (Some(Intent::Remove { names, file }), o) => {
            let o = o.unwrap_or_default();
            let went: Vec<String> = names.iter().map(|n| o.removed.iter().find(|a| same(a, n)).map(|a| shown(a)).unwrap_or_else(|| n.clone())).collect();
            let mut out = vec![format!("✓ Removed {} · from {file}", and_list(&went)), undo];
            // (What it used stays with the change before, for nami undo: until it's cleaned up.)
            out.extend(freeable(o.size_removed, false));
            out
        }
        (Some(Intent::Upgrade), None) => vec!["✓ Everything is up to date".to_string()],
        (None, None) => vec![format!("✓ Nothing to change · {file} is in effect")],
        (Some(Intent::Upgrade), Some(o)) => {
            let mut parts = vec![format!("✓ Updated {}", count(o.changed, "app", "apps"))];
            parts.extend(o.generation.map(|g| format!("change {g}")));
            parts.extend(disk(&o));
            parts.push(undo_cmd.into());
            let mut out = vec![parts.join(" · ")];
            out.extend(freeable(o.size_removed, true));
            out
        }
        (None, Some(o)) => {
            let mut parts = vec![match elapsed() {
                Some(d) => format!("✓ Applied {file} in {}", duration(d)),
                None => format!("✓ Applied {file}"),
            }];
            parts.extend(o.generation.map(|g| format!("change {g}")));
            parts.extend(disk(&o));
            parts.push(undo_cmd.into());
            vec![parts.join(" · ")]
        }
    }
}

/// The packages the command named (to install or remove).
pub fn named() -> Vec<String> {
    match &*INTENT.lock().unwrap() {
        Some(Intent::Install { names, .. }) | Some(Intent::Remove { names, .. }) => names.clone(),
        _ => Vec::new(),
    }
}

/// What `nami install` was asked for, if it's an install.
pub fn installing() -> Vec<String> {
    match &*INTENT.lock().unwrap() {
        Some(Intent::Install { names, .. }) => names.clone(),
        _ => Vec::new(),
    }
}

/// Whether there's a command's intent (install, remove) to say at the end.
pub fn has_intent() -> bool {
    INTENT.lock().unwrap().is_some()
}

/// `nami history`: a change a line, newest first: its number, when (in
/// words), what it did to the apps (`+ git, htop  − jq`), the one in use
/// marked; then how to look closer and go back.
pub fn timeline(t: &crate::show::Term, steps: &[crate::query::Step], now: u64, undo: &str) -> String {
    let p = t.paint;
    if steps.is_empty() {
        let mut out = "No changes yet.\n".to_string();
        if t.hints {
            out += &format!("\n{}{}\n", p.dim("The first: "), p.accent("nami install <app>"));
        }
        return out;
    }
    let num_w = steps.iter().map(|s| s.generation.to_string().len()).max().unwrap_or(1);
    let whens: Vec<String> = steps.iter().map(|s| if s.created == 0 { "as it came".into() } else { ago(s.created, now) }).collect();
    let when_w = whens.iter().map(|w| w.chars().count()).max().unwrap_or(0);
    let oldest = steps.iter().map(|s| s.generation).min();
    let mut out = String::new();
    for (s, when) in steps.iter().zip(&whens) {
        let names = |l: &mut dyn Iterator<Item = String>| {
            let all: Vec<String> = l.collect();
            if all.len() > 4 { format!("{} and {} more", all[..3].join(", "), all.len() - 3) } else { all.join(", ") }
        };
        let mut what = Vec::new();
        if !s.added.is_empty() {
            what.push(format!("{} {}", p.added("+"), names(&mut s.added.iter().map(|e| crate::index::split_version(&e.attr).0.to_string()))));
        }
        if !s.removed.is_empty() {
            what.push(format!("{} {}", p.removed("−"), names(&mut s.removed.iter().map(|e| crate::index::split_version(&e.attr).0.to_string()))));
        }
        if !s.changed.is_empty() {
            let one = |c: &crate::query::Change| if s.changed.len() == 1 { format!("{} {} {} {}", c.attr, p.dim(&c.from), p.dim("→"), c.to) } else { c.attr.clone() };
            what.push(format!("{} {}", p.changed("~"), names(&mut s.changed.iter().map(one))));
        }
        if s.files > 0 {
            what.push(p.dim(&count(s.files, "file", "files")));
        }
        if what.is_empty() {
            what.push(p.dim(if Some(s.generation) == oldest { &"the start" } else { &"no changes to apps" }));
        }
        let mark = if s.current { p.added("●") } else { " ".into() };
        let num = format!("{:>num_w$}", s.generation);
        let num = if s.current { p.bold(&num) } else { num };
        let lead = format!("{mark} {num}  {}{}  ", p.dim(when), " ".repeat(when_w - when.chars().count()));
        let mut line = format!("{lead}{}", what.join("  "));
        if s.current {
            line += &format!("  {}", p.dim("· in use"));
        }
        if let Some(w) = t.width {
            if crate::show::cols(&line) > w {
                // (Too long: the counts, not every name.)
                let short = format!("{} {}  {} {}  {} {}", p.added("+"), s.added.len(), p.removed("−"), s.removed.len(), p.changed("~"), s.changed.len());
                line = format!("{lead}{short}");
            }
        }
        out += &line;
        out.push('\n');
    }
    if t.hints && steps.len() > 1 {
        let current = steps.iter().find(|s| s.current).map(|s| s.generation);
        let mut said = vec![format!("{}{}", p.dim("What a change did: "), p.accent(&format!("nami diff {}", current.unwrap_or(steps[0].generation))))];
        if current.is_some_and(|c| Some(c) != oldest) {
            said.push(format!("{}{}", p.dim("Go back one: "), p.accent(undo)));
        }
        out += &format!("\n{}\n", said.join(&p.dim(" · ")));
    }
    out
}

/// Whether a store name is the package someone named (`firefox`, `firefox@150`).
pub fn same(name: &str, asked: &str) -> bool {
    let asked = crate::index::split_version(asked).0;
    let asked = asked.rsplit('/').next().unwrap_or(asked);
    let (n, _) = name_version(name);
    n == asked || n.eq_ignore_ascii_case(asked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_times_as_people_say_them() {
        assert_eq!(size(2_345_000_000), "2.3 GB");
        assert_eq!(size(104_400_000), "104 MB");
        assert_eq!(size(6_700_000), "6.7 MB");
        assert_eq!(size(512_000), "512 KB");
        assert_eq!(duration(std::time::Duration::from_millis(400)), "0.4 s");
        assert_eq!(duration(std::time::Duration::from_secs(12)), "12 s");
        assert_eq!(duration(std::time::Duration::from_secs(125)), "2 min 5 s");
        let now = 1_795_000_000;
        assert_eq!(ago(now - 5, now), "just now");
        assert_eq!(ago(now - 300, now), "5 minutes ago");
        assert_eq!(ago(now - 3600, now), "1 hour ago");
        assert_eq!(ago(now - 90000, now), "yesterday");
        assert_eq!(ago(now - 3 * 86400, now), "3 days ago");
        assert!(ago(now - 400 * 86400, now).ends_with(", 2025"));
    }

    #[test]
    fn names_and_versions() {
        assert_eq!(shown("firefox-160.0"), "firefox 160.0");
        assert_eq!(shown("glibc-locales-2.44-57"), "glibc-locales 2.44-57");
        assert_eq!(and_list(&["a".into(), "b".into(), "c".into()]), "a, b and c");
        assert!(same("firefox-160.0", "firefox"));
        assert!(same("firefox-150.0.1", "firefox@150"));
        assert!(!same("firefox-esr-153.0", "firefox"));
    }

    #[test]
    fn new_versions_paired() {
        let s = |l: &[&str]| l.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(paired(&s(&["htop-3.4.1", "git-2.51.0"]), &s(&["git-2.50.0", "jq-1.7.1"])), s(&["+ htop 3.4.1", "~ git 2.50.0 → 2.51.0", "− jq 1.7.1"]));
    }

    #[test]
    fn the_last_line_says_what_happened() {
        set_intent(Intent::Install { names: vec!["firefox".into()], file: "user.nami".into() });
        set_outcome(Outcome { generation: Some(3), added: vec!["firefox-160.0".into()], opens: vec![("firefox".into(), Opens::Launcher)], ..Default::default() });
        assert_eq!(finish("~/user.nami", false), vec!["✓ Installed firefox 160.0 · in user.nami · opens from the launcher".to_string(), "  Undo with nami undo".into()]);
        // A package's folder that's listed already, its recipe changed: updated, by its name.
        set_intent(Intent::Install { names: vec!["/etc/nami/packages/nami-gui".into()], file: "system.nami".into() });
        set_outcome(Outcome { generation: Some(3), changed: 1, updated: vec![("nami-gui-0.18.6".into(), "nami-gui-0.18.10".into())], ..Default::default() });
        assert_eq!(finish("system.nami", true), vec!["✓ Updated nami-gui 0.18.6 → 0.18.10 · in system.nami".to_string(), "  Undo with sudo nami undo".into()]);
        // (Another version asked for: one went, one came.)
        set_intent(Intent::Install { names: vec!["firefox@150".into()], file: "user.nami".into() });
        set_outcome(Outcome { generation: Some(3), added: vec!["firefox-150.0.1".into()], removed: vec!["firefox-160.0".into()], opens: vec![("firefox".into(), Opens::Launcher)], ..Default::default() });
        assert_eq!(finish("~/user.nami", false), vec!["✓ Updated firefox 160.0 → 150.0.1 · in user.nami".to_string(), "  Undo with nami undo".into()]);
        set_intent(Intent::Install { names: vec!["htop".into(), "./packages/nami-gui".into()], file: "system.nami".into() });
        set_outcome(Outcome { generation: Some(3), added: vec!["htop-3.4.1".into()], updated: vec![("nami-gui-0.18.6".into(), "nami-gui-0.18.10".into())], ..Default::default() });
        assert_eq!(finish("system.nami", true)[0], "✓ Installed htop 3.4.1 · updated nami-gui 0.18.6 → 0.18.10 · in system.nami");
        // As it was: said by its name, not its folder.
        set_intent(Intent::Install { names: vec!["/etc/nami/packages/nami-gui".into()], file: "system.nami".into() });
        assert_eq!(finish("system.nami", true), vec!["✓ nami-gui is already installed · in system.nami".to_string()]);
        set_outcome(Outcome { generation: Some(4), size_added: 104_400_000, ..Default::default() });
        assert_eq!(finish("~/user.nami", false), vec!["✓ Applied ~/user.nami · change 4 · 104 MB more on disk · nami undo".to_string()]);
        assert_eq!(finish("~/user.nami", false), vec!["✓ Nothing to change · ~/user.nami is in effect".to_string()]);
        set_outcome(Outcome { generation: Some(5), ..Default::default() });
        assert_eq!(finish("system.nami", true), vec!["✓ Applied system.nami · change 5 · sudo nami undo".to_string()]);
        // A removal says what it can free (here, in one test: the words are the process's).
        set_cleanup(Some("30 days".into()), true);
        set_intent(Intent::Remove { names: vec!["brave".into()], file: "system.nami".into() });
        set_outcome(Outcome { generation: Some(5), removed: vec!["brave-1.80".into()], size_removed: 450_200_000, ..Default::default() });
        assert_eq!(finish("system.nami", true), ["✓ Removed brave 1.80 · from system.nami", "  Undo with sudo nami undo", "  450 MB can be freed now with nami gc --old · or it goes by itself after 30 days"]);
        assert_eq!(freeable(2_300_000_000, true).unwrap(), "  Old versions: 2.3 GB can be freed now with nami gc --old · or they go by themselves after 30 days");
        assert_eq!(freeable(450_000_000, false).unwrap(), "  450 MB can be freed now with nami gc --old · or it goes by itself after 30 days");
        set_cleanup(None, true);
        assert_eq!(freeable(450_000_000, false).unwrap(), "  450 MB can be freed now with nami gc --old");
        // Someone's own: the space is freed by the daily cleanup.
        set_cleanup(Some("7 days".into()), false);
        assert_eq!(freeable(450_000_000, false).unwrap(), "  450 MB is freed by itself after 7 days · or sooner with nami gc --old");
        // Under a megabyte: nothing to say.
        assert_eq!(freeable(200_000, false), None);
        set_cleanup(Some("30 days".into()), false);
    }
}
