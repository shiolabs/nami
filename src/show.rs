//! How `nami search`, `nami info` and `nami versions` look in a terminal:
//! one row per app, a short card, versions by line. Colour as everywhere
//! else (`style`): names bold, the secondary dim, what you can type cyan, a
//! green ✓ on what you have; none of it in a pipe or with NO_COLOR. The
//! hints at the end (what to type next) are for a terminal only: a pipe gets
//! the rows alone, and `--json` is untouched by any of this.

use std::io::IsTerminal;

use crate::index::Version;
use crate::query::Item;
use crate::style::Paint;

/// Where the output goes: colour or not, how wide (None: a pipe, nothing
/// cut short), and whether to add hints.
#[derive(Clone, Copy, Debug)]
pub struct Term {
    pub paint: Paint,
    pub width: Option<usize>,
    pub hints: bool,
}

impl Term {
    /// For stdout: its terminal's width, or `COLUMNS` when it says.
    pub fn out() -> Term {
        let paint = crate::style::out();
        let tty = std::io::stdout().is_terminal();
        let width = tty.then(|| {
            let mut size: libc::winsize = unsafe { std::mem::zeroed() };
            (unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut size) } == 0 && size.ws_col >= 20).then_some(size.ws_col as usize)
        });
        let columns = std::env::var("COLUMNS").ok().and_then(|c| c.parse::<usize>().ok()).filter(|c| *c >= 20);
        Term { paint, width: width.flatten().or(columns), hints: tty || paint.0 }
    }

    /// For stderr: messages, uncut.
    pub fn err() -> Term {
        let paint = crate::style::err();
        Term { paint, width: None, hints: std::io::stderr().is_terminal() || paint.0 }
    }

    /// How wide a paragraph may be: the terminal's, at most 100; 80 in a pipe.
    fn text_width(&self) -> usize {
        self.width.unwrap_or(80).min(100)
    }
}

/// How many columns `s` takes (escape codes aside).
pub fn cols(s: &str) -> usize {
    let mut n = 0;
    let mut esc = false;
    for c in s.chars() {
        match (esc, c) {
            (false, '\x1b') => esc = true,
            (true, 'm') => esc = false,
            (true, _) => {}
            _ => n += 1,
        }
    }
    n
}

/// `s` in at most `max` columns, ending in … when it's cut short.
pub fn trim(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out = out.trim_end().to_string();
    out.push('…');
    out
}

/// `text` in lines of at most `width` columns, at spaces (a word longer
/// than that on a line of its own).
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// At most `n` of `lines`, the last ending in … when there were more.
fn first_lines(mut lines: Vec<String>, n: usize, width: usize) -> (Vec<String>, bool) {
    if lines.len() <= n {
        return (lines, false);
    }
    lines.truncate(n);
    if let Some(last) = lines.last_mut() {
        *last = trim(&format!("{last}…"), width);
    }
    (lines, true)
}

/// What a row says about an item: the app's own name and its line.
pub fn about(i: &Item) -> String {
    match (&i.title, i.summary.as_ref().or(i.description.as_ref())) {
        (Some(t), Some(d)) if !t.eq_ignore_ascii_case(&i.attr) && !t.eq_ignore_ascii_case(d) => format!("{t}: {d}"),
        (Some(t), None) => t.clone(),
        (_, d) => d.cloned().unwrap_or_default(),
    }
}

/// The text a hint puts what you type in: `nami install firefox`.
fn typed(p: Paint, cmd: &str) -> String {
    p.accent(cmd)
}

/// A `·` between parts, dim.
fn dot(p: Paint) -> String {
    p.dim(" · ")
}

/// `nami search`: one row per item (✓ on what you have), then what to type
/// next. `mine` says whether you have one (for you or for everyone);
/// `hidden` is how many tools and libraries --all would add.
pub fn search(t: &Term, items: &[Item], mine: impl Fn(&Item) -> bool, term: &str, hidden: usize) -> String {
    let p = t.paint;
    let name_w = items.iter().map(|i| i.attr.chars().count()).max().unwrap_or(0).min(30);
    let ver_cap = if t.width.is_some() { 12 } else { usize::MAX };
    let ver_w = items.iter().map(|i| i.version.chars().count().min(ver_cap)).max().unwrap_or(0);
    let mut out = String::new();
    for i in items {
        let have = mine(i);
        let mark = if have { p.added("✓") } else { " ".into() };
        let name = format!("{}{}", p.bold(&i.attr), " ".repeat(name_w.saturating_sub(i.attr.chars().count())));
        let version = trim(&i.version, ver_cap);
        let version = format!("{}{}", p.dim(&version), " ".repeat(ver_w.saturating_sub(version.chars().count())));
        let tag = match i.source.as_deref() {
            Some("flathub") => Some("Flathub".to_string()),
            Some(crate::casks::SOURCE) => Some("Homebrew's casks".to_string()),
            Some(s) => Some(s.to_string()),
            None => None,
        };
        let lead = format!("{mark} {name}  {version}  ");
        let mut about = about(i);
        if let Some(w) = t.width {
            let room = w.saturating_sub(cols(&lead) + tag.as_ref().map_or(0, |t| t.len() + 2) + 1);
            about = trim(&about, room);
        }
        let mut line = format!("{lead}{about}");
        if let Some(tag) = tag {
            line += &format!("  {}", p.dim(&tag));
        }
        out += line.trim_end();
        out.push('\n');
    }
    if t.hints {
        let mut hints = Vec::new();
        if let Some(first) = items.first() {
            let mut cmds = Vec::new();
            if !mine(first) {
                cmds.push(typed(p, &format!("nami install {}", first.attr)));
            }
            cmds.push(typed(p, &format!("nami info {}", first.attr)));
            hints.push(cmds.join(&dot(p)));
        }
        if hidden > 0 {
            let what = if hidden == 1 { "1 command-line tool or library".to_string() } else { format!("{} command-line tools and libraries", thousands(hidden)) };
            let cmd = if term.is_empty() { "nami search --all".to_string() } else { format!("nami search --all {}", quote(term)) };
            hints.push(format!("{}{}", p.dim(&format!("{what} too: ")), typed(p, &cmd)));
        }
        if !hints.is_empty() {
            out += "\n";
            for h in hints {
                out += &h;
                out.push('\n');
            }
        }
    }
    out
}

/// `12345` -> `12,345`.
fn thousands(n: usize) -> String {
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

/// A search term as you'd type it again.
fn quote(term: &str) -> String {
    if term.chars().all(|c| c.is_alphanumeric() || "-_.@+:/".contains(c)) {
        term.to_string()
    } else {
        format!("'{}'", term.replace('\'', "'\\''"))
    }
}

/// Nothing found: said plainly, with what to try instead.
pub fn nothing_found(t: &Term, term: &str, category: Option<&str>, close: &[String], flathub_tried: bool) -> String {
    let p = t.paint;
    let mut out = match category {
        _ if term.is_empty() => format!("No apps in {}.\n", category.unwrap_or("that category")),
        Some(c) => format!("No {c} apps match {}.\n", p.bold(&format!("“{term}”"))),
        None => format!("Nothing in the catalog matches {}.\n", p.bold(&format!("“{term}”"))),
    };
    let close: Vec<&String> = close.iter().filter(|c| !c.eq_ignore_ascii_case(term)).collect();
    let mut tries = Vec::new();
    if !close.is_empty() {
        tries.push(format!("Did you mean {}?", close.iter().map(|c| p.bold(c)).collect::<Vec<_>>().join(" or ")));
    }
    if category.is_some() && !term.is_empty() {
        tries.push(format!("{}{}", p.dim("In every kind of app: "), typed(p, &format!("nami search {}", quote(term)))));
    }
    if !term.is_empty() {
        tries.push(p.dim("Try a shorter word, or what the app does (“photo”, “music”)."));
        if !flathub_tried {
            tries.push(format!("{}{}", p.dim("Flathub's apps too: "), typed(p, &format!("nami search --flathub {}", quote(term)))));
        }
    }
    if !tries.is_empty() {
        out.push('\n');
        for l in tries {
            out += &format!("  {l}\n");
        }
    }
    out
}

/// A name the catalog doesn't have (`nami info fierfox`).
pub fn not_a_package(t: &Term, name: &str, close: &[String]) -> String {
    let p = t.paint;
    let mut out = format!("No package in the catalog is called {}.\n", p.bold(&format!("“{name}”")));
    if !close.is_empty() {
        out += &format!("  Did you mean {}?\n", close.iter().map(|c| p.bold(c)).collect::<Vec<_>>().join(" or "));
    }
    if t.hints {
        out += &format!("  {}{}\n", p.dim("Look for it: "), typed(p, &format!("nami search {}", quote(name))));
    }
    out
}

/// One release: its version, from when to when it was the catalog's (None:
/// from before the history starts, or until now), and its nixpkgs revision.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: String,
    pub from: Option<String>,
    pub until: Option<String>,
    pub rev: String,
}

/// A package's versions (as `Index::versions` lists them: the catalog's own
/// first, dateless) with when each was current: each one from the date the
/// one before it stopped.
pub fn releases(list: &[Version]) -> Vec<Release> {
    let mut dated: Vec<&Version> = list.iter().filter(|v| !v.1.is_empty()).collect();
    dated.sort_by(|a, b| b.1.cmp(&a.1));
    let mut out = Vec::new();
    for v in list.iter().filter(|v| v.1.is_empty()) {
        out.push(Release { version: v.0.clone(), from: dated.first().map(|d| d.1.clone()), until: None, rev: String::new() });
    }
    for (k, v) in dated.iter().enumerate() {
        out.push(Release { version: v.0.clone(), from: dated.get(k + 1).map(|d| d.1.clone()), until: Some(v.1.clone()), rev: v.2.clone() });
    }
    out
}

/// Releases of one line (159, or 3.12), newest first.
#[derive(Debug, Clone)]
pub struct Line {
    pub key: String,
    pub releases: Vec<Release>,
}

impl Line {
    /// When the line was current: from its first release's start to its last's end.
    fn span(&self) -> (Option<String>, Option<String>) {
        let from = if self.releases.iter().any(|r| r.from.is_none()) { None } else { self.releases.iter().filter_map(|r| r.from.clone()).min() };
        let until = if self.releases.iter().any(|r| r.until.is_none()) { None } else { self.releases.iter().filter_map(|r| r.until.clone()).max() };
        (from, until)
    }

    fn has(&self, version: &str) -> bool {
        self.releases.iter().any(|r| r.version == version)
    }
}

/// Releases by line, newest first: by major version (160, 159), or by
/// minor when they all share one (python3's 3.13, 3.12).
pub fn lines(releases: &[Release]) -> Vec<Line> {
    let major = |v: &str| v.split('.').next().unwrap_or("").to_string();
    let first = releases.first().map(|r| major(&r.version));
    let depth = if releases.len() > 1 && releases.iter().all(|r| Some(major(&r.version)) == first) { 2 } else { 1 };
    let key = |v: &str| v.split('.').take(depth).collect::<Vec<_>>().join(".");
    let mut out: Vec<Line> = Vec::new();
    for r in releases {
        let k = key(&r.version);
        match out.iter_mut().find(|l| l.key == k) {
            Some(l) => l.releases.push(r.clone()),
            None => out.push(Line { key: k, releases: vec![r.clone()] }),
        }
    }
    for l in &mut out {
        l.releases.sort_by(|a, b| crate::index::version_key(&b.version).cmp(&crate::index::version_key(&a.version)));
    }
    out.sort_by(|a, b| crate::index::version_key(&b.key).cmp(&crate::index::version_key(&a.key)));
    out
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// `2026-11-12` -> (2026, "Nov 12").
fn day(date: &str) -> Option<(String, String)> {
    let mut parts = date.split('-');
    let (y, m, d) = (parts.next()?, parts.next()?.parse::<usize>().ok()?, parts.next()?.get(..2)?.parse::<u32>().ok()?);
    Some((y.to_string(), format!("{} {d}", MONTHS.get(m.checked_sub(1)?)?)))
}

/// When something was current, said short: `Oct 28 – Nov 12, 2026`,
/// `Dec 3, 2025 – Jan 9, 2026`, `since Nov 12, 2026`, `until Mar 31, 2017`.
pub fn span(from: Option<&str>, until: Option<&str>) -> String {
    let one = |d: &str| day(d).map(|(y, md)| format!("{md}, {y}")).unwrap_or_else(|| d.to_string());
    match (from, until) {
        (Some(f), Some(u)) => match (day(f), day(u)) {
            (Some((fy, fd)), Some((uy, ud))) if fy == uy => format!("{fd} – {ud}, {uy}"),
            _ => format!("{} – {}", one(f), one(u)),
        },
        (Some(f), None) => format!("since {}", one(f)),
        (None, Some(u)) => format!("until {}", one(u)),
        (None, None) => "now".to_string(),
    }
}

/// The year of the oldest release, and how many releases there were before the catalog's own.
pub fn history_summary(list: &[Version]) -> (usize, Option<String>) {
    let earlier = list.iter().filter(|v| !v.1.is_empty()).count();
    let oldest = list.iter().filter(|v| !v.1.is_empty()).map(|v| v.1.as_str()).min().map(|d| d.get(..4).unwrap_or(d).to_string());
    (earlier, oldest)
}

/// What `nami versions` knows of one package.
pub struct Versions<'a> {
    pub attr: &'a str,
    pub list: &'a [Version],
    /// The version you have, if you have it.
    pub installed: Option<String>,
    /// Kept under names of their own (python312): (name, version).
    pub siblings: Vec<(String, String)>,
    /// Every release, not only each line's newest.
    pub all: bool,
}

/// How many lines `nami versions` shows without --all.
const LINES_SHOWN: usize = 20;

/// `nami versions`: each line (159, 158…) with its newest release and when
/// it was current, ✓ on yours; with `all`, every release under its line.
pub fn versions(t: &Term, v: &Versions) -> String {
    let p = t.paint;
    let releases = releases(v.list);
    let lines = lines(&releases);
    let current = v.list.iter().find(|x| x.1.is_empty()).map(|x| x.0.clone());
    let (_, oldest) = history_summary(v.list);
    let mut out = format!("{} {}\n", p.bold(v.attr), p.dim(&match (releases.len(), &oldest) {
        (1, _) => "1 version".to_string(),
        (n, Some(y)) => format!("{n} versions, back to {y}"),
        (n, None) => format!("{n} versions"),
    }));
    out.push('\n');
    let shown = if v.all { lines.len() } else { lines.len().min(LINES_SHOWN) };
    let key_w = lines.iter().take(shown).map(|l| l.key.chars().count()).max().unwrap_or(0);
    let newest_w = lines.iter().take(shown).map(|l| l.releases[0].version.chars().count()).max().unwrap_or(0);
    let spans: Vec<String> = lines.iter().map(|l| {
        let (f, u) = l.span();
        span(f.as_deref(), u.as_deref())
    }).collect();
    let span_w = spans.iter().take(shown).map(|s| s.chars().count()).max().unwrap_or(0);
    let marks = |l: &Line| {
        let mut m = Vec::new();
        if v.installed.as_deref().is_some_and(|i| l.has(i)) {
            // (Which release, when it isn't the line's newest.)
            let which = v.installed.as_deref().filter(|i| !v.all && *i != l.releases[0].version).map(|i| format!(" ({i})")).unwrap_or_default();
            m.push(p.added(&format!("✓ installed{which}")));
        }
        if current.as_deref().is_some_and(|c| l.has(c)) {
            m.push(p.dim("current"));
        }
        m.join("  ")
    };
    for (l, s) in lines.iter().zip(&spans).take(shown) {
        let pad = |text: &str, w: usize| " ".repeat(w.saturating_sub(text.chars().count()));
        if v.all {
            // The line, then each of its releases (the marks on those), lined up across lines.
            out += &format!("  {}{}\n", p.bold(&l.key), match s.as_str() { "" => String::new(), s => format!("  {}", p.dim(s)) });
            let rel_w = lines.iter().flat_map(|l| &l.releases).map(|r| r.version.chars().count()).max().unwrap_or(0);
            let rel_spans: Vec<String> = l.releases.iter().map(|r| span(r.from.as_deref(), r.until.as_deref())).collect();
            let rs_w = lines.iter().flat_map(|l| &l.releases).map(|r| span(r.from.as_deref(), r.until.as_deref()).chars().count()).max().unwrap_or(0);
            for (r, rs) in l.releases.iter().zip(&rel_spans) {
                let mut row = format!("    {}{}  {}{}", r.version, pad(&r.version, rel_w), rs, pad(rs, rs_w));
                if !r.rev.is_empty() {
                    row += &format!("  {}", p.dim(&format!("nixpkgs {}", r.rev)));
                }
                if v.installed.as_deref() == Some(r.version.as_str()) {
                    row += &format!("  {}", p.added("✓ installed"));
                }
                if current.as_deref() == Some(r.version.as_str()) {
                    row += &format!("  {}", p.dim("current"));
                }
                out += row.trim_end();
                out.push('\n');
            }
        } else {
            let newest = &l.releases[0].version;
            let row = format!("  {}{}  {}{}  {}{}  {}", p.bold(&l.key), pad(&l.key, key_w), p.dim(newest), pad(newest, newest_w), s, pad(s, span_w), marks(l));
            out += row.trim_end();
            out.push('\n');
        }
    }
    if shown < lines.len() {
        let rest = &lines[shown..];
        let last = rest.last().map(|l| l.key.clone()).unwrap_or_default();
        let year = rest.last().and_then(|l| l.span().1.or(l.span().0)).map(|d| format!(" ({})", d.get(..4).unwrap_or(&d))).unwrap_or_default();
        out += &format!("  {}\n", p.dim(&format!("and {} older versions, back to {last}{year}", rest.len())));
    }
    if !v.siblings.is_empty() {
        out += &format!("\n{}\n", p.bold("Also in the catalog under their own names"));
        let w = v.siblings.iter().map(|(n, _)| n.chars().count()).max().unwrap_or(0);
        for (name, version) in &v.siblings {
            out += &format!("  {}{}  {}\n", p.bold(name), " ".repeat(w - name.chars().count()), p.dim(version));
        }
    }
    if t.hints {
        // An example to pin: the line before the current one, else the newest.
        let pick = lines.get(1).or(lines.first()).map(|l| l.key.clone()).unwrap_or_else(|| "1".into());
        out += "\n";
        out += &format!("{}{}{}{}\n", p.dim("Pick one: "), typed(p, &format!("nami install {}@{pick}", v.attr)), p.dim(", or in a nami file: "), typed(p, &format!("{}: {pick}", v.attr)));
        if !v.all && lines.iter().any(|l| l.releases.len() > 1) || shown < lines.len() {
            out += &format!("{}{}\n", p.dim("Every release: "), typed(p, &format!("nami versions {} --all", v.attr)));
        }
    }
    out
}

/// Where an installed package is from: your own file, the computer's, or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    None,
    Yours,
    Everyones,
    Both,
    /// Installed, but not by a nami file (a Flathub app).
    Other,
}

/// What `nami info` says of one package, besides the item itself.
pub struct Card<'a> {
    pub item: &'a Item,
    pub place: Place,
    /// Earlier versions, and the year of the oldest.
    pub history: Option<(usize, Option<String>)>,
    /// Everything: the whole description, the screenshots.
    pub all: bool,
}

/// `nami info`: a short card.
pub fn info(t: &Term, c: &Card) -> String {
    let p = t.paint;
    let i = c.item;
    let width = t.text_width();
    let mut out = format!("{} {}", p.bold(&i.attr), p.dim(&i.version));
    out.push('\n');
    let line = about(i);
    if !line.is_empty() {
        out += &format!("{}\n", trim(&line, if c.all { usize::MAX } else { t.width.unwrap_or(usize::MAX) }));
    }
    // Licence · homepage.
    let mut facts = Vec::new();
    if let Some(l) = &i.license {
        facts.push(if i.unfree { format!("{l} (not free software)") } else { l.clone() });
    } else if i.unfree {
        facts.push("not free software".into());
    }
    if let Some(h) = &i.homepage {
        facts.push(p.underline(h));
    }
    if !facts.is_empty() {
        out += &format!("{}\n", facts.join(&dot(p)));
    }
    // What it is.
    let cask = i.source.as_deref() == Some(crate::casks::SOURCE);
    let opens = if cask { "App · in Applications, in nami Apps" } else { "App · opens from the launcher" };
    let mut kind = vec![if i.app { opens.to_string() } else { "Command-line tool or library".to_string() }];
    if i.source.as_deref() == Some("flathub") {
        kind.push("from Flathub".into());
    } else if cask {
        kind.push(crate::casks::FROM.into());
    } else if let Some(by) = &i.publisher {
        kind.push(format!("from {by}"));
    }
    if i.builds_here {
        kind.push("built on this computer the first time".into());
    }
    out += &format!("{}\n", p.dim(&kind.join(" · ")));
    out.push('\n');
    // Installed or not.
    let base = crate::index::split_version(&i.attr).0;
    out += &match c.place {
        Place::Yours => format!("{} {}\n", p.added("✓ Installed for you"), p.dim("· in your user.nami")),
        Place::Everyones => format!("{} {}\n", p.added("✓ Installed for everyone"), p.dim("· in system.nami")),
        Place::Both => format!("{} {}\n", p.added("✓ Installed for you and for everyone"), p.dim("· in user.nami and system.nami")),
        Place::Other => format!("{}\n", p.added("✓ Installed")),
        Place::None => match &i.unavailable {
            Some(why) => format!("Not installed{}{why}\n", dot(p)),
            None => format!("Not installed{}{}\n", dot(p), typed(p, &format!("nami install {}", i.attr))),
        },
    };
    if let Some((n, oldest)) = &c.history {
        if *n > 0 {
            let what = match (n, oldest) {
                (1, Some(y)) => format!("1 earlier version, from {y}"),
                (1, None) => "1 earlier version".into(),
                (n, Some(y)) => format!("{n} earlier versions, back to {y}"),
                (n, None) => format!("{n} earlier versions"),
            };
            out += &format!("{what}{}{}\n", dot(p), typed(p, &format!("nami versions {base}")));
        }
    }
    // A few lines about it.
    let summary = i.summary.as_ref().or(i.description.as_ref());
    let text = i.about.clone().or_else(|| i.description.clone().filter(|d| Some(d) != summary));
    let mut more = false;
    if let Some(text) = text.filter(|t| !t.trim().is_empty()) {
        let (lines, cut) = if c.all { (wrap(&text, width), false) } else { first_lines(wrap(&text, width), 3, width) };
        more |= cut;
        out.push('\n');
        for l in lines {
            out += &format!("{l}\n");
        }
    }
    if c.all {
        if !i.screenshots.is_empty() {
            out += &format!("\n{}\n", p.bold("Screenshots"));
            for s in &i.screenshots {
                out += &format!("  {}\n", p.underline(s));
            }
        }
    } else {
        more |= !i.screenshots.is_empty();
    }
    if more && t.hints {
        let what = if i.screenshots.is_empty() { "All of it" } else { "All of it, and screenshots" };
        out += &format!("{}{}\n", p.dim(&format!("{what}: ")), typed(p, &format!("nami info {} --all", i.attr)));
    }
    out
}

/// What `nami search firefox@150` found.
pub enum Found {
    /// The version, and when it was the catalog's (or the name it has there now).
    Yes { version: String, when: String },
    /// Not one there is: the lines there are, newest first.
    No { lines: Vec<String> },
}

/// `nami search firefox@150`: whether there's such a version, and how to have it.
pub fn version_search(t: &Term, name: &str, wanted: &str, found: &Found) -> String {
    let p = t.paint;
    match found {
        Found::Yes { version, when } => {
            let mut out = format!("{} {}\n{when}\n", p.bold(name), p.dim(version));
            if t.hints {
                let rows = [
                    ("Install it", format!("nami install {name}@{wanted}")),
                    ("Or in a nami file", format!("{name}: {wanted}")),
                    ("Every version", format!("nami versions {name}")),
                ];
                out.push('\n');
                for (what, cmd) in rows {
                    out += &format!("  {}{}  {}\n", p.dim(what), " ".repeat(17 - what.len()), typed(p, &cmd));
                }
            }
            out
        }
        Found::No { lines } => {
            let mut out = format!("{} has no version {}.\n", p.bold(name), p.bold(wanted));
            if !lines.is_empty() {
                let shown: Vec<&str> = lines.iter().take(6).map(String::as_str).collect();
                let more = if lines.len() > shown.len() { format!(", and {} older", lines.len() - shown.len()) } else { String::new() };
                out += &format!("{}{}{more}.\n", p.dim("It has "), shown.join(", "));
            }
            if t.hints {
                out += &format!("\n{}{}\n", p.dim("Every version: "), typed(p, &format!("nami versions {name}")));
            }
            out
        }
    }
}

/// One thing `nami list` lists.
#[derive(Debug, Clone)]
pub struct Row {
    pub name: String,
    /// The version the file asks for (`firefox@150`), if it asks for one.
    pub pin: Option<String>,
    pub version: String,
    pub about: String,
}

/// `nami list`: what's installed, by where it's from (the computer's, yours,
/// Flathub's, downloads), each with its version (a pinned one marked with
/// the `@150` that pins it), then how many and what to type next.
pub fn list(t: &Term, groups: &[(String, Vec<Row>)]) -> String {
    let p = t.paint;
    let rows = || groups.iter().flat_map(|(_, r)| r);
    let label = |r: &Row| r.name.chars().count() + r.pin.as_ref().map_or(0, |v| v.chars().count() + 1);
    let name_w = rows().map(label).max().unwrap_or(0).min(30);
    let ver_cap = if t.width.is_some() { 14 } else { usize::MAX };
    let ver_w = rows().map(|r| r.version.chars().count().min(ver_cap)).max().unwrap_or(0);
    let mut out = String::new();
    let total = rows().count();
    if total == 0 {
        out += "No apps installed yet.\n";
        if t.hints {
            out += &format!("\n{}{}\n", p.dim("Find one: "), typed(p, "nami search <what it does>"));
        }
        return out;
    }
    for (i, (title, rows)) in groups.iter().filter(|(_, r)| !r.is_empty()).enumerate() {
        if i > 0 {
            out.push('\n');
        }
        // `System · system.nami`: the title bold, where dim.
        let title = match title.split_once(" · ") {
            Some((a, b)) => format!("{}{}{}", p.bold(a), dot(p), p.dim(b)),
            None => p.bold(title),
        };
        out += &format!("{title}\n");
        for r in rows {
            let pin = r.pin.as_ref().map(|v| p.changed(&format!("@{v}"))).unwrap_or_default();
            let name = format!("{}{pin}{}", p.bold(&r.name), " ".repeat(name_w.saturating_sub(label(r))));
            let version = trim(&r.version, ver_cap);
            let version = format!("{}{}", p.dim(&version), " ".repeat(ver_w.saturating_sub(version.chars().count())));
            let lead = format!("  {name}  {version}  ");
            let about = match t.width {
                Some(w) => trim(&r.about, w.saturating_sub(cols(&lead) + 1)),
                None => r.about.clone(),
            };
            out += format!("{lead}{}", p.dim(&about)).trim_end();
            out.push('\n');
        }
    }
    if t.hints {
        let pinned = rows().any(|r| r.pin.is_some());
        let mut said = vec![p.dim(&crate::tell::count(total, "app", "apps"))];
        if pinned {
            said.push(format!("{}{}", p.changed("@"), p.dim(" pinned to a version")));
        }
        if let Some(first) = rows().next() {
            said.push(typed(p, &format!("nami info {}", first.name)));
        }
        said.push(typed(p, "nami upgrade --dry-run"));
        out += &format!("\n{}\n", said.join(&dot(p)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::strip;

    fn item(attr: &str, version: &str, summary: &str) -> Item {
        Item {
            attr: attr.into(),
            name: format!("{attr}-{version}"),
            version: version.into(),
            description: Some(summary.into()),
            homepage: Some("https://example.org".into()),
            license: Some("MPL-2.0".into()),
            app: true,
            installed: false,
            program: None,
            icon: None,
            unfree: false,
            builds_here: false,
            publisher: None,
            title: None,
            summary: Some(summary.into()),
            about: None,
            categories: Vec::new(),
            screenshots: Vec::new(),
            popularity: None,
            desktop: Vec::new(),
            source: None,
            unavailable: None,
        }
    }

    fn v(version: &str, date: &str) -> Version {
        Version(version.into(), date.into(), if date.is_empty() { String::new() } else { "abc123".into() }, Default::default())
    }

    const PIPE: Term = Term { paint: Paint(false), width: None, hints: false };

    #[test]
    fn trimming_and_wrapping() {
        assert_eq!(trim("Fast, Private & Safe Web Browser", 12), "Fast, Priva…");
        assert_eq!(trim("short", 12), "short");
        assert_eq!(trim("ab cd", 4), "ab…", "no space before the …");
        assert_eq!(trim("æøåæøå", 4).chars().count(), 4, "counted in characters, not bytes");
        let lines = wrap("one two three four five six seven", 10);
        assert_eq!(lines, ["one two", "three four", "five six", "seven"]);
        assert!(lines.iter().all(|l| l.chars().count() <= 10));
        let (l, cut) = first_lines(wrap("a b c d e f g h", 3), 2, 3);
        assert!(cut && l.len() == 2 && l[1].ends_with('…'), "{l:?}");
    }

    #[test]
    fn search_rows_fit_the_terminal() {
        let mut long = item("firefox", "160.0", &"A very long description that goes on and on ".repeat(5));
        long.installed = true;
        let items = vec![long, item("librewolf", "1.9.4-unstable-2026-09-09", "Web browser")];
        let t = Term { paint: Paint(true), width: Some(60), hints: true };
        let out = search(&t, &items, |i| i.installed, "firefox", 12);
        // (The rows; the hints after them say a whole command.)
        for l in strip(&out).lines().take_while(|l| !l.is_empty()) {
            assert!(l.chars().count() <= 60, "{} columns: {l}", l.chars().count());
        }
        let plain = strip(&out);
        assert!(plain.starts_with("✓ firefox  "), "{plain}");
        assert!(plain.contains("…"));
        assert!(plain.contains("1.9.4-unsta…"), "a long version cut short: {plain}");
        assert!(plain.contains("nami info firefox") && !plain.contains("nami install firefox"), "installed: no install hint\n{plain}");
        assert!(plain.contains("12 command-line tools and libraries too: nami search --all firefox"), "{plain}");
        assert!(out.contains("\x1b[32m✓"), "the ✓ green");
    }

    #[test]
    fn no_colour_or_hints_in_a_pipe() {
        let items = vec![item("firefox", "160.0", "Web browser"), item("gimp", "3.2", "Image editor")];
        let out = search(&PIPE, &items, |_| false, "firefox", 3);
        assert!(!out.contains('\x1b'));
        assert_eq!(out, "  firefox  160.0  Web browser\n  gimp     3.2    Image editor\n", "rows only, nothing cut");
        let card = info(&PIPE, &Card { item: &items[0], place: Place::None, history: Some((43, Some("2016".into()))), all: false });
        assert!(!card.contains('\x1b'));
        assert!(card.contains("Not installed · nami install firefox\n43 earlier versions, back to 2016 · nami versions firefox\n"), "{card}");
        let list = [v("160.0", ""), v("159.0", "2026-11-12")];
        let out = versions(&PIPE, &Versions { attr: "firefox", list: &list, installed: None, siblings: Vec::new(), all: false });
        assert!(!out.contains('\x1b') && !out.contains("Pick one"), "{out}");
        let none = nothing_found(&PIPE, "zzqqx", None, &["zsh".into()], false);
        assert!(!none.contains('\x1b') && none.contains("Did you mean zsh?") && none.contains("nami search --flathub zzqqx"), "{none}");
    }

    #[test]
    fn colour_changes_only_the_colour() {
        let items = vec![item("firefox", "160.0", "Web browser")];
        let on = Term { paint: Paint(true), width: Some(80), hints: true };
        let off = Term { paint: Paint(false), ..on };
        assert_eq!(strip(&search(&on, &items, |_| true, "firefox", 2)), search(&off, &items, |_| true, "firefox", 2));
        let card = |t: &Term| info(t, &Card { item: &items[0], place: Place::Yours, history: None, all: true });
        assert_eq!(strip(&card(&on)), card(&off));
        let list = [v("160.0", ""), v("159.0.1", "2026-11-12"), v("159.0", "2026-11-02")];
        let ver = |t: &Term| versions(t, &Versions { attr: "firefox", list: &list, installed: Some("159.0".into()), siblings: Vec::new(), all: false });
        assert_eq!(strip(&ver(&on)), ver(&off));
    }

    #[test]
    fn versions_by_line_with_when() {
        let list = [v("160.0", ""), v("159.0.1", "2026-11-12"), v("159.0", "2026-11-02"), v("158.0", "2026-10-20"), v("45.0", "2016-03-31")];
        let r = releases(&list);
        assert_eq!(r[0], Release { version: "160.0".into(), from: Some("2026-11-12".into()), until: None, rev: String::new() });
        assert_eq!(r[1].from.as_deref(), Some("2026-11-02"), "159.0.1 from when 159.0 stopped");
        assert_eq!(r.last().unwrap().from, None, "the oldest: from before the history");
        let l = lines(&r);
        assert_eq!(l.iter().map(|l| l.key.as_str()).collect::<Vec<_>>(), ["160", "159", "158", "45"]);
        assert_eq!(l[1].releases.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), ["159.0.1", "159.0"]);
        assert_eq!(l[1].span(), (Some("2026-10-20".into()), Some("2026-11-12".into())));
        // Python's all share a major: by minor, newest first, however the dates go.
        let py = [v("3.14.10", ""), v("3.13.18", "2026-11-17"), v("3.14.8", "2026-11-07"), v("3.12.6", "2024-10-24"), v("3.12.5", "2024-10-10")];
        let l = lines(&releases(&py));
        assert_eq!(l.iter().map(|l| l.key.as_str()).collect::<Vec<_>>(), ["3.14", "3.13", "3.12"]);
        assert_eq!(l[0].releases.len(), 2);
        let t = Term { paint: Paint(false), width: Some(80), hints: true };
        let out = versions(&t, &Versions { attr: "firefox", list: &list, installed: Some("159.0".into()), siblings: vec![], all: false });
        assert!(out.starts_with("firefox 5 versions, back to 2016\n"), "{out}");
        assert!(out.contains("  160  160.0    since Nov 12, 2026           current\n"), "{out}");
        assert!(out.contains("  159  159.0.1  Oct 20 – Nov 12, 2026        ✓ installed (159.0)\n"), "{out}");
        assert!(out.contains("  45   45.0     until Mar 31, 2016\n"), "{out}");
        assert!(out.contains("Pick one: nami install firefox@159, or in a nami file: firefox: 159\n"), "{out}");
        assert!(out.contains("nami versions firefox --all"));
        let all = versions(&t, &Versions { attr: "firefox", list: &list, installed: Some("159.0".into()), siblings: vec![], all: true });
        assert!(all.contains("  159  Oct 20 – Nov 12, 2026\n    159.0.1  Nov 2 – Nov 12, 2026         nixpkgs abc123\n    159.0    Oct 20 – Nov 2, 2026         nixpkgs abc123  ✓ installed\n"), "{all}");
        assert!(!all.contains("--all"));
    }

    #[test]
    fn spans_said_short() {
        assert_eq!(span(Some("2026-10-28"), Some("2026-11-12")), "Oct 28 – Nov 12, 2026");
        assert_eq!(span(Some("2025-12-03"), Some("2026-01-09")), "Dec 3, 2025 – Jan 9, 2026");
        assert_eq!(span(Some("2026-11-12"), None), "since Nov 12, 2026");
        assert_eq!(span(None, Some("2017-03-31")), "until Mar 31, 2017");
    }

    #[test]
    fn the_card_says_a_few_lines() {
        let mut i = item("firefox", "160.0", "Fast, Private & Safe Web Browser");
        i.about = Some("When it comes to your life online, you have a choice. ".repeat(10));
        i.screenshots = vec!["https://example.org/1.png".into()];
        let t = Term { paint: Paint(false), width: Some(60), hints: true };
        let out = info(&t, &Card { item: &i, place: Place::None, history: None, all: false });
        assert!(out.starts_with("firefox 160.0\nFast, Private & Safe Web Browser\nMPL-2.0 · https://example.org\nApp · opens from the launcher\n"), "{out}");
        let body: Vec<&str> = out.split("\n\n").nth(2).unwrap().lines().collect();
        assert_eq!(body.len(), 4, "three lines and the hint: {body:?}");
        assert!(body[2].ends_with('…'));
        assert!(!out.contains("example.org/1.png"), "screenshots only with --all");
        assert!(out.ends_with("All of it, and screenshots: nami info firefox --all\n"), "{out}");
        let all = info(&t, &Card { item: &i, place: Place::Everyones, history: None, all: true });
        assert!(all.contains("✓ Installed for everyone · in system.nami") && all.contains("https://example.org/1.png") && !all.contains('…'), "{all}");
    }

    #[test]
    fn a_version_found_or_not() {
        let t = Term { paint: Paint(false), width: Some(80), hints: true };
        let out = version_search(&t, "firefox", "150", &Found::Yes { version: "150.0.1".into(), when: "The catalog's from Jul 2 – Jul 29, 2026.".into() });
        assert!(out.contains("firefox 150.0.1\n") && out.contains("nami install firefox@150") && out.contains("firefox: 150"), "{out}");
        let out = version_search(&t, "firefox", "999", &Found::No { lines: vec!["160".into(), "159".into()] });
        assert!(out.starts_with("firefox has no version 999.\nIt has 160, 159.\n"), "{out}");
    }
}
