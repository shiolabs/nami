//! Colour for what nami prints in a terminal: a little, and only where it
//! means something. Added is green, taken away red, changed amber; errors,
//! warnings and notes say which they are; what's secondary (versions, paths,
//! times, progress) is dim. The words are always the same: colour only goes
//! around them, so with it off (a pipe, a file, NO_COLOR) the text is as before.
//!
//! When to colour, by the usual rules: never with NO_COLOR or CLICOLOR=0 or
//! TERM=dumb, always with CLICOLOR_FORCE=1, else only on a terminal; `--color`
//! says it outright. The daemon's output goes to its client uncoloured; the
//! client colours it for its own terminal (`line`).

use clap::builder::styling::{AnsiColor, Effects, Style, Styles};
use std::io::IsTerminal;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Choice {
    #[default]
    Auto,
    Always,
    Never,
}

static CHOICE: AtomicU8 = AtomicU8::new(0);
static OUT: OnceLock<bool> = OnceLock::new();
static ERR: OnceLock<bool> = OnceLock::new();

/// `--color`: before anything is printed.
pub fn set_choice(c: Choice) {
    CHOICE.store(c as u8, Ordering::Relaxed);
}

fn choice() -> Choice {
    match CHOICE.load(Ordering::Relaxed) {
        1 => Choice::Always,
        2 => Choice::Never,
        _ => Choice::Auto,
    }
}

/// Whether to colour, from the choice, the environment and whether the stream is a terminal.
pub fn decide(choice: Choice, tty: bool, env: impl Fn(&str) -> Option<String>) -> bool {
    let set = |k: &str| env(k).filter(|v| !v.is_empty());
    match choice {
        Choice::Always => true,
        Choice::Never => false,
        Choice::Auto if set("NO_COLOR").is_some() => false,
        Choice::Auto if set("CLICOLOR_FORCE").is_some_and(|v| v != "0") => true,
        Choice::Auto if set("CLICOLOR").as_deref() == Some("0") => false,
        Choice::Auto if set("TERM").as_deref() == Some("dumb") => false,
        Choice::Auto => tty,
    }
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok()
}

/// For stdout.
pub fn out() -> Paint {
    Paint(*OUT.get_or_init(|| decide(choice(), std::io::stdout().is_terminal(), env)))
}

/// For stderr.
pub fn err() -> Paint {
    Paint(*ERR.get_or_init(|| decide(choice(), std::io::stderr().is_terminal(), env)))
}

const GREEN: Style = AnsiColor::Green.on_default();
const RED: Style = AnsiColor::Red.on_default();
const AMBER: Style = AnsiColor::Yellow.on_default();
const CYAN: Style = AnsiColor::Cyan.on_default();
const BOLD: Style = Style::new().effects(Effects::BOLD);
const DIM: Style = Style::new().effects(Effects::DIMMED);

/// A command's `--help`, as `nami help` (src/help.rs): headings bold, what
/// you type (commands, flags) cyan, what you fill in as the text around it.
pub const HELP: Styles = Styles::styled()
    .header(BOLD)
    .usage(BOLD)
    .literal(CYAN)
    .placeholder(Style::new())
    .error(RED.bold())
    .valid(GREEN)
    .invalid(AMBER.bold());

/// Paints text for one stream, or leaves it as it is.
#[derive(Clone, Copy, Debug)]
pub struct Paint(pub bool);

impl Paint {
    fn with(self, style: Style, text: &str) -> String {
        if !self.0 || text.is_empty() {
            return text.to_string();
        }
        format!("{}{text}{}", style.render(), style.render_reset())
    }

    pub fn added(self, t: &str) -> String {
        self.with(GREEN, t)
    }
    pub fn removed(self, t: &str) -> String {
        self.with(RED, t)
    }
    pub fn changed(self, t: &str) -> String {
        self.with(AMBER, t)
    }
    pub fn bold(self, t: &str) -> String {
        self.with(BOLD, t)
    }
    pub fn dim(self, t: &str) -> String {
        self.with(DIM, t)
    }
    pub fn error(self, t: &str) -> String {
        self.with(RED.bold(), t)
    }
    pub fn warning(self, t: &str) -> String {
        self.with(AMBER.bold(), t)
    }
    pub fn note(self, t: &str) -> String {
        self.with(CYAN, t)
    }
    /// What you type: a command, a flag (help, and the hints that name one).
    pub fn accent(self, t: &str) -> String {
        self.with(CYAN, t)
    }
    /// A link.
    pub fn underline(self, t: &str) -> String {
        self.with(Style::new().underline(), t)
    }

    /// A severity's word, as `nami check` and errors say it.
    pub fn severity(self, word: &str) -> String {
        match word {
            "error" => self.error(word),
            "warning" => self.warning(word),
            _ => self.note(word),
        }
    }

    /// A message, the suggestion in "did you mean `x`?" bold.
    pub fn message(self, text: &str) -> String {
        if !self.0 {
            return text.to_string();
        }
        let mut out = String::new();
        let mut rest = text;
        while let Some(i) = rest.find("did you mean `") {
            let start = i + "did you mean `".len();
            let Some(len) = rest[start..].find('`') else { break };
            out += &rest[..start];
            out += &self.bold(&rest[start..start + len]);
            rest = &rest[start + len..];
        }
        out + rest
    }

    /// A message that may start with where (`file:3: …`, `file:3:5: …`): that bold.
    fn located(self, text: &str) -> String {
        match location(text) {
            Some(n) => self.bold(&text[..n]) + &self.message(&text[n..]),
            None => self.message(text),
        }
    }

    /// `old -> new` or `old → new`: the arrow dim, the rest in `style`.
    fn arrowed(self, style: Style, text: &str) -> String {
        for arrow in [" -> ", " → "] {
            if let Some((a, b)) = text.split_once(arrow) {
                return self.with(style, a) + " " + &self.dim(arrow.trim()) + " " + &self.with(style, b);
            }
        }
        self.with(style, text)
    }

    /// One line of what nami says, coloured by its shape; any other line as it is.
    pub fn line(self, text: &str) -> String {
        if !self.0 || text.contains('\x1b') {
            return text.to_string();
        }
        let body = text.trim_start();
        let indent = &text[..text.len() - body.len()];
        // error: / warning: / note: / hint:, maybe indented.
        for word in ["error", "warning", "note", "hint"] {
            if let Some(rest) = body.strip_prefix(word).and_then(|r| r.strip_prefix(':')) {
                if word == "hint" {
                    return format!("{indent}{}:{}", self.dim(word), self.hinted(rest));
                }
                return format!("{indent}{}:{}", self.severity(word), self.located(rest));
            }
        }
        // ✓ done, ⚠ worth a look, ✗ wrong (maybe under a title): the mark
        // coloured, what follows each ` · ` dim, a command in it cyan.
        if indent.is_empty() || indent == "  " {
            for (mark, style) in [("✓ ", GREEN), ("⚠ ", AMBER), ("✗ ", RED)] {
                if let Some(rest) = body.strip_prefix(mark) {
                    return format!("{indent}{} {}", self.with(style, mark.trim_end()), self.segments(rest, false));
                }
            }
        }
        // A kind of change's title (Apps, Services…).
        if indent.is_empty() && crate::tell::HEADINGS.contains(&body) {
            return self.bold(body);
        }
        // A change under its title: `  + firefox 160.0`, `  − vlc 3.0.21`,
        // `  ~ git 2.50.0 → 2.51.0`: the mark coloured, versions dim.
        if indent == "  " {
            for (mark, style) in [("+ ", GREEN), ("− ", RED), ("~ ", AMBER)] {
                if let Some(rest) = body.strip_prefix(mark) {
                    return format!("{indent}{} {}", self.with(style, mark.trim_end()), self.versions(rest));
                }
            }
        }
        // How to undo it, and a dry run's end: the words dim, the command cyan.
        if let Some(cmd) = body.strip_prefix("Undo with ").or(body.strip_prefix("Go forward again with ")) {
            let lead = &body[..body.len() - cmd.len()];
            return format!("{indent}{}{}", self.dim(lead), self.accent(cmd));
        }
        if body.starts_with("To go back") {
            return format!("{indent}{}", self.dim(body));
        }
        if indent == "  " {
            // What to do next: `  Test it on 7 distributions: nami bundle --test ./rg`.
            if let Some((lead, cmd)) = body.split_once(": ").filter(|(_, c)| ["nami ", "curl ", "sudo "].iter().any(|p| c.starts_with(p))) {
                return format!("{indent}{} {}", self.dim(&format!("{lead}:")), self.accent(cmd));
            }
            // A line of detail under what it says: `  41 MB unpacked · 38 parts`.
            if body.contains(" · ") && !body.starts_with(['+', '−', '~', '✓', '✗', '⚠', '-']) {
                return format!("{indent}{}", self.segments(body, true));
            }
        }
        if body.starts_with("Dry run") || body.starts_with("Nothing was changed") {
            return format!("{indent}{}", self.segments(body, true));
        }
        // file:3: error: …, as `nami check` says.
        if let Some(n) = location(text) {
            for word in ["error", "warning", "note"] {
                if let Some(rest) = text[n..].strip_prefix(&format!(" {word}:")) {
                    return format!("{} {}:{}", self.bold(&text[..n]), self.severity(word), self.message(rest));
                }
            }
        }
        // What a change did: + added, - removed, ~ changed (at the start).
        if indent.is_empty() {
            if let Some(rest) = text.strip_prefix("+ ") {
                return self.added(&format!("+ {rest}"));
            }
            if let Some(rest) = text.strip_prefix("- ") {
                return self.removed(&format!("- {rest}"));
            }
            if let Some(rest) = text.strip_prefix("~ ") {
                return self.arrowed(AMBER, &format!("~ {rest}"));
            }
        }
        // change 12 active (/nix/var/…): the change bold, where dim.
        if let Some(rest) = text.strip_prefix("change ") {
            let n = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
            if n > 0 {
                let (num, tail) = rest.split_at(n);
                let tail = match tail.find(" (") {
                    Some(p) if tail.ends_with(')') => format!("{}{}", self.arrowed(Style::new(), &tail[..p]), self.dim(&tail[p..])),
                    _ => self.arrowed(Style::new(), tail),
                };
                return format!("{}{tail}", self.bold(&format!("change {num}")));
            }
        }
        if let Some(n) = text.strip_prefix("back at change ") {
            return format!("back at {}", self.bold(&format!("change {n}")));
        }
        // Progress: what's fetched and built, quietly.
        let progress = ["fetching ", "building ", "unpacking ", "downloading ", "looking in ", "waiting for ", "  | ", "  disk: ", "Downloaded ", "Built ", "wrote ", "updated ", "Versions saved in ", "Keeping the last ", "Going back to "];
        if progress.iter().any(|p| text.starts_with(p)) || store_name(text.strip_prefix("  ").unwrap_or("")) {
            return self.dim(text);
        }
        text.to_string()
    }

    /// `first · second · nami undo`: the first part as it is (or dim), the
    /// rest dim, a part that's a command to type cyan.
    fn segments(self, text: &str, all_dim: bool) -> String {
        let sep = self.dim(" · ");
        text.split(" · ")
            .enumerate()
            .map(|(i, part)| match part {
                // (`nami apply to use it`: the command cyan, what it's for dim.)
                p if p.starts_with("nami ") => match p.split_once(" to ") {
                    Some((cmd, why)) => format!("{} {}", self.accent(cmd), self.dim(&format!("to {why}"))),
                    None => self.accent(p),
                },
                p if i == 0 && !all_dim => p.to_string(),
                p => self.dim(p),
            })
            .collect::<Vec<_>>()
            .join(&sep)
    }

    /// A change's words with its versions dim: `firefox 160.0`, `git 2.50.0 → 2.51.0`.
    fn versions(self, text: &str) -> String {
        let version = |w: &str| w.starts_with(|c: char| c.is_ascii_digit());
        if let Some((a, b)) = text.split_once(" → ") {
            let (name, from) = a.rsplit_once(' ').filter(|(_, v)| version(v)).unwrap_or((a, ""));
            let from = if from.is_empty() { String::new() } else { format!(" {}", self.dim(from)) };
            return format!("{name}{from} {} {b}", self.dim("→"));
        }
        match text.rsplit_once(' ') {
            Some((name, v)) if version(v) => format!("{name} {}", self.dim(v)),
            _ => text.to_string(),
        }
    }

    /// A service's name, in a colour of its own (the n-th), as `nami up` and
    /// `nami logs` put it before each line.
    pub fn service(self, n: usize, name: &str) -> String {
        const SERVICES: [Style; 5] = [CYAN, AnsiColor::Magenta.on_default(), AnsiColor::Blue.on_default(), GREEN, AMBER];
        self.with(SERVICES[n % SERVICES.len()], name)
    }

    /// A hint's words, `what you type` in them cyan (the backticks kept).
    pub fn hinted(self, text: &str) -> String {
        if !self.0 {
            return text.to_string();
        }
        let mut parts = text.split('`');
        let mut out = parts.next().unwrap_or_default().to_string();
        for (i, p) in parts.enumerate() {
            out += &if i % 2 == 0 { format!("`{}", self.accent(p)) } else { format!("`{p}") };
        }
        out
    }

    /// A line of a file's changes (`nami diff`): `+`/`−`/`-` at any depth,
    /// a section's title bold, a row's `a → b` amber.
    pub fn diff_line(self, text: &str) -> String {
        if !self.0 {
            return text.to_string();
        }
        let body = text.trim_start();
        let indent = &text[..text.len() - body.len()];
        if indent.is_empty() {
            return self.line(text);
        }
        if body.starts_with("+ ") {
            return format!("{indent}{}", self.added(body));
        }
        if body.starts_with("- ") || body.starts_with("− ") {
            return format!("{indent}{}", self.removed(body));
        }
        match indent.len() {
            // A section's title, maybe "(stays as it is now)".
            2 => match body.find("  (") {
                Some(p) => format!("{indent}{}{}", self.bold(&body[..p]), self.dim(&body[p..])),
                None => format!("{indent}{}", self.bold(body)),
            },
            // A row that changed: `Time zone: Europe/Copenhagen → auto`.
            6 if body.contains(" → ") || body.contains(" -> ") => format!("{indent}{}", self.arrowed(AMBER, body)),
            _ => text.to_string(),
        }
    }

    /// A card about one thing (`nami package-info`): its first line, `name 1.2`,
    /// the name bold and the version dim; then `  Label: value` lines, the labels dim.
    pub fn card(self, text: &str) -> String {
        if !self.0 {
            return text.to_string();
        }
        let mut out = String::new();
        for (i, l) in text.split_inclusive('\n').enumerate() {
            let (l, end) = l.strip_suffix('\n').map_or((l, ""), |x| (x, "\n"));
            let painted = match (i, l.rsplit_once(' '), l.strip_prefix("  ").and_then(|b| b.split_once(": "))) {
                (0, Some((name, v)), _) if v.starts_with(|c: char| c.is_ascii_digit()) => format!("{} {}", self.bold(name), self.dim(v)),
                (0, _, _) => self.bold(l),
                (_, _, Some((label, value))) if label.len() < 24 && !label.contains("  ") => format!("  {} {value}", self.dim(&format!("{label}:"))),
                _ => self.line(l),
            };
            out += &painted;
            out += end;
        }
        out
    }

    /// A line of YAML (`nami config get`): the keys and the list's dashes dim, the values as they are.
    pub fn yaml_line(self, text: &str) -> String {
        if !self.0 {
            return text.to_string();
        }
        let body = text.trim_start();
        let indent = &text[..text.len() - body.len()];
        let (dash, body) = match body.strip_prefix("- ") {
            Some(rest) => (self.dim("-") + " ", rest),
            None => (String::new(), body),
        };
        let key_end = body.find(": ").map(|i| i + 1).or_else(|| body.ends_with(':').then(|| body.len()));
        match key_end {
            Some(n) if !body.starts_with(['"', '\'', '|', '>']) => format!("{indent}{dash}{}{}", self.dim(&body[..n]), &body[n..]),
            _ => format!("{indent}{dash}{body}"),
        }
    }

    /// Every line of `text` by `each`.
    pub fn lines(self, text: &str, each: impl Fn(Paint, &str) -> String) -> String {
        if !self.0 {
            return text.to_string();
        }
        text.split_inclusive('\n').map(|l| match l.strip_suffix('\n') {
            Some(l) => each(self, l) + "\n",
            None => each(self, l),
        }).collect()
    }
}

/// Whether a message is a file's problems as `nami check` says them
/// (`project.nami:3: error: …`), not to be put after an `error: ` of its own.
pub fn is_said_as_check(text: &str) -> bool {
    // (Or the file alone, `project.nami: error: …`, when it isn't about one line.)
    let file_alone = text.split_once(": error:").is_some_and(|(f, _)| !f.is_empty() && !f.contains([' ', '\n']) && (f.contains('.') || f.contains('/')));
    file_alone || location(text).is_some_and(|n| text[n..].starts_with(" error:"))
}

/// The length of `file:3:` or `file:3:5:` at the start of a message, if it starts so.
fn location(text: &str) -> Option<usize> {
    let lead = text.len() - text.trim_start().len();
    let t = &text[lead..];
    let colon = t.find(':')?;
    let path = &t[..colon];
    if path.is_empty() || path.contains(' ') || !(path.contains('.') || path.contains('/')) {
        return None;
    }
    let mut end = colon;
    let mut numbers = 0;
    while numbers < 2 {
        let after = &t[end + 1..];
        let digits = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
        if digits == 0 || !after[digits..].starts_with(':') {
            break;
        }
        end += 1 + digits;
        numbers += 1;
    }
    (numbers > 0).then_some(lead + end + 1)
}

/// A store path's name (`<32 chars>-name`), as fetching lists them.
fn store_name(t: &str) -> bool {
    t.len() > 33 && t.as_bytes()[32] == b'-' && t[..32].bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

/// The text without colour (for tests).
#[cfg(test)]
pub fn strip(text: &str) -> String {
    let mut out = String::new();
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' && it.peek() == Some(&'[') {
            for d in it.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINES: &[&str] = &[
        "+ firefox-150.0.1",
        "- vlc-3.0.21",
        "~ git: git-2.50.0 -> git-2.51.0",
        "~ ~/.config/foot/foot.ini",
        "change 12 active (/nix/var/nami/profiles/default/bin)",
        "change 3 → now",
        "back at change 4",
        "error: user.nami:3: unknown key `pakages` (did you mean `packages`?)",
        "warning: the catalog isn't signed, so the binary caches it names aren't used",
        "  note: a service",
        "hint: try `nami search`",
        "system.nami:7:3: error: firefx isn't in the catalog (did you mean `firefox`?)",
        "user.nami: no problems",
        "fetching 12 paths (40.1 MiB unpacked)",
        "  0123456789abcdfghijklmnpqrsvwxyz-bash-5.2",
        "  disk: +12.0 MiB -0 B",
        "nothing to do",
        "  - ./services/postgres.nami",
        "",
        "✓ Installed firefox 160.0 · in system.nami · opens from the launcher",
        "  Undo with nami undo",
        "Apps",
        "  + firefox 160.0",
        "  − vlc 3.0.21",
        "  ~ git 2.50.0 → 2.51.0",
        "  ✗ a check",
        "⚠ user.nami has changes not applied yet · nami apply",
        "Dry run · 3 changes · nothing was changed yet · nami apply",
        "hint: `nami search fierfox` looks for it.",
    ];

    #[test]
    fn same_words_coloured_or_not() {
        for l in LINES {
            assert_eq!(strip(&Paint(true).line(l)), *l);
            assert_eq!(strip(&Paint(true).diff_line(l)), *l);
            assert_eq!(Paint(false).line(l), *l, "off: as it is");
            assert_eq!(Paint(false).diff_line(l), *l);
        }
        let block = LINES.join("\n");
        assert_eq!(strip(&Paint(true).lines(&block, Paint::line)), block);
        assert_eq!(Paint(false).lines(&block, Paint::line), block);
    }

    #[test]
    fn colours_mean_something() {
        let p = Paint(true);
        assert_eq!(p.line("+ firefox-150.0.1"), "\x1b[32m+ firefox-150.0.1\x1b[0m");
        assert_eq!(p.line("- vlc-3.0.21"), "\x1b[31m- vlc-3.0.21\x1b[0m");
        assert!(p.line("~ git: a -> b").contains("\x1b[2m->\x1b[0m"));
        assert!(p.line("error: x").starts_with("\x1b[1m\x1b[31merror\x1b[0m:"));
        assert!(p.line("error: x (did you mean `packages`?)").contains("\x1b[1mpackages\x1b[0m"));
        assert!(p.line("change 12 active (/x/bin)").starts_with("\x1b[1mchange 12\x1b[0m active\x1b[2m (/x/bin)"));
        assert!(p.line("system.nami:7:3: error: x").starts_with("\x1b[1msystem.nami:7:3:\x1b[0m"));
        // Indented: part of a message (a YAML example in an error), not a change.
        assert_eq!(p.line("  - ./services/postgres.nami"), "  - ./services/postgres.nami");
        assert_eq!(p.line("nothing to do"), "nothing to do");
        // A change's lines: the marks coloured, versions dim, what to type cyan.
        assert_eq!(p.line("  + firefox 160.0"), "  \x1b[32m+\x1b[0m firefox \x1b[2m160.0\x1b[0m");
        assert_eq!(p.line("  − vlc 3.0.21"), "  \x1b[31m−\x1b[0m vlc \x1b[2m3.0.21\x1b[0m");
        assert!(p.line("  ~ git 2.50.0 → 2.51.0").starts_with("  \x1b[33m~\x1b[0m git \x1b[2m2.50.0\x1b[0m \x1b[2m→\x1b[0m 2.51.0"));
        assert_eq!(p.line("Apps"), "\x1b[1mApps\x1b[0m");
        let done = p.line("✓ Done in 12 s · change 34 · nami undo");
        assert!(done.starts_with("\x1b[32m✓\x1b[0m Done in 12 s\x1b[2m · \x1b[0m\x1b[2mchange 34\x1b[0m"), "{done:?}");
        assert!(done.ends_with("\x1b[36mnami undo\x1b[0m"));
        assert!(p.line("  Undo with nami undo").ends_with("\x1b[36mnami undo\x1b[0m"));
        assert!(p.line("hint: `nami search x` looks").contains("`\x1b[36mnami search x\x1b[0m`"));
        // Already coloured: left alone.
        assert_eq!(p.line("\x1b[32m+ a\x1b[0m"), "\x1b[32m+ a\x1b[0m");
    }

    #[test]
    fn when_to_colour() {
        let none = |_: &str| None;
        assert!(decide(Choice::Auto, true, none));
        assert!(!decide(Choice::Auto, false, none), "not a terminal: no colour");
        let with = |k: &'static str, v: &'static str| move |q: &str| (q == k).then(|| v.to_string());
        assert!(!decide(Choice::Auto, true, with("NO_COLOR", "1")));
        assert!(decide(Choice::Auto, true, with("NO_COLOR", "")), "NO_COLOR empty is unset");
        assert!(!decide(Choice::Auto, true, with("CLICOLOR", "0")));
        assert!(!decide(Choice::Auto, true, with("TERM", "dumb")));
        assert!(decide(Choice::Auto, false, with("CLICOLOR_FORCE", "1")));
        assert!(!decide(Choice::Auto, false, with("CLICOLOR_FORCE", "0")));
        assert!(decide(Choice::Always, false, with("NO_COLOR", "1")));
        assert!(!decide(Choice::Never, true, none));
    }

    #[test]
    fn off_without_a_terminal() {
        // As tests run (output captured), unless forced.
        assert!(!decide(Choice::Auto, false, |_| None));
        let p = Paint(decide(Choice::Auto, false, |_| None));
        assert_eq!(p.line("+ firefox"), "+ firefox");
        assert_eq!(p.lines("+ a\n- b\n", Paint::line), "+ a\n- b\n");
    }
}
