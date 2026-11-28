//! What nami says about itself: `nami` alone (a short welcome), `nami help`
//! (every command, in groups, one short line each), the examples under a
//! command's own `--help`, and a command line nami doesn't understand.
//!
//! Each command's short line is the first line of its doc comment in
//! cli.rs; the rest of the comment is what `nami <command> --help` adds.
//! Colour as everywhere else (`style`): headings bold, what you type cyan,
//! the secondary dim, and none at all in a pipe or with NO_COLOR.

use clap::builder::StyledStr;
use clap::error::{ContextKind, ContextValue, ErrorKind};

use crate::style::Paint;

pub const PROMISE: &str = "declarative Linux made easy";
pub const DOCS: &str = "https://namios.dev/docs";

/// The commands people use, by what they're doing, the most used first.
/// Any command left out of every group is hidden (`nami help --all` lists those).
pub const GROUPS: &[(&str, &[&str])] = &[
    ("Get started", &["install", "remove", "search", "info", "versions", "list", "upgrade", "update"]),
    ("Your setup", &["apply", "check", "config", "show", "options", "secret", "home", "layers", "import"]),
    ("Changes and backups", &["undo", "history", "diff", "backup", "sync"]),
    ("Projects", &["new", "shell", "run", "try", "up", "down", "logs", "project"]),
    ("Ship your apps", &["bundle", "build", "recipe", "push"]),
    ("Apps from other distributions", &["start", "libraries", "add-to-launcher", "package-info", "file-apps"]),
    ("namiOS", &["desktop"]),
    ("Agents", &["mcp", "propose", "proposals"]),
    ("nami itself", &["status", "doctor", "report", "self-update", "storage", "gc", "completions", "self-uninstall"]),
];

/// What `nami` alone shows.
const WELCOME: &[&str] = &["install", "search", "remove", "upgrade", "apply", "undo", "shell", "doctor"];

/// What the end of `nami help` shows.
const TOP_EXAMPLES: &[(&str, &str)] = &[
    ("Install an app", "nami install firefox"),
    ("Find one", "nami search photo"),
    ("Go back to how it was", "nami undo"),
];

/// A few examples for the commands people use most: (what it does, the command line).
pub const EXAMPLES: &[(&str, &[(&str, &str)])] = &[
    ("install", &[
        ("Install two apps for yourself", "nami install firefox gimp"),
        ("For the system: everyone on this computer (administrators)", "nami install --system docker"),
        ("An app from Flathub, or a download", "nami install flathub:org.gimp.GIMP ./app.deb"),
    ]),
    ("remove", &[
        ("Remove an app", "nami remove gimp"),
        ("The same, as other package managers say it", "nami uninstall gimp"),
    ]),
    ("search", &[
        ("Find apps for photos", "nami search photo"),
        ("Tools and libraries too", "nami search --all ripgrep"),
        ("The most used games", "nami search --category games"),
    ]),
    ("info", &[
        ("What firefox is, and whether you have it", "nami info firefox"),
        ("All of its description, and its screenshots", "nami info firefox --all"),
    ]),
    ("versions", &[
        ("Every version of firefox, newest first", "nami versions firefox"),
        ("Every release, not only each version's newest", "nami versions python3 --all"),
        ("Is there a firefox 150?", "nami search firefox@150"),
        ("Install it", "nami install firefox@150"),
    ]),
    ("upgrade", &[
        ("See what would be updated", "nami upgrade --dry-run"),
        ("Update everything", "nami upgrade"),
    ]),
    ("apply", &[
        ("Apply your nami file", "nami apply"),
        ("See what would change first", "nami apply --dry-run"),
        ("Set this computer up from your config repo", "nami apply github:ada/nami-config"),
    ]),
    ("check", &[("Check a file before applying it", "nami check ./user.nami")]),
    ("config", &[
        ("Set a value", "nami config set env.EDITOR nvim"),
        ("Add to a list", "nami config add packages tree"),
    ]),
    ("undo", &[
        ("Undo the last change", "nami undo"),
        ("Go back to change 12", "nami undo 12"),
    ]),
    ("history", &[("Every change, newest last", "nami history")]),
    ("diff", &[("What changed since change 12", "nami diff 12")]),
    ("new", &[
        ("List the examples", "nami new"),
        ("A Node project in ./shop", "nami new shop --from node"),
    ]),
    ("shell", &[
        ("A shell with this project's tools", "nami shell"),
        ("Another project's, from GitHub", "nami shell github:ada/site"),
    ]),
    ("run", &[
        ("Run the project's test task", "nami run test"),
        ("Run any command with the project's tools", "nami run -- npm test"),
    ]),
    ("try", &[("Run cowsay without installing it", "nami try cowsay hi")]),
    ("up", &[
        ("Start every service, in the foreground", "nami up"),
        ("Start postgres in the background", "nami up -d postgres"),
    ]),
    ("bundle", &[
        ("This folder's project as one file", "nami bundle ."),
        ("A package from the catalog", "nami bundle jq"),
        ("Then test it on seven distributions", "nami bundle . --test"),
    ]),
    ("sync", &[
        ("Turn it on for this computer", "nami sync on"),
        ("See what's synced", "nami sync status"),
    ]),
    ("backup", &[
        ("Back up now", "nami backup now"),
        ("List the backups", "nami backup list"),
    ]),
    ("doctor", &[("Check this computer", "nami doctor")]),
    ("storage", &[
        ("How full the disk is, and what nami takes", "nami storage"),
        ("The same, for a script", "nami storage --json"),
    ]),
    ("gc", &[
        ("See what would be freed", "nami gc --dry-run"),
        ("Keep the last month of changes", "nami gc --older-than 30d"),
    ]),
    ("self-update", &[("Is there a newer nami?", "nami self-update --check")]),
    ("self-uninstall", &[("Uninstall nami itself from this computer", "sudo nami self-uninstall")]),
];

/// `GROUPS`, with namiOS's commands first in its group when its part is built in.
fn groups() -> Vec<(&'static str, Vec<&'static str>)> {
    GROUPS
        .iter()
        .map(|(title, names)| {
            let os = crate::nami_os::plugin().filter(|_| *title == "namiOS").map_or(&[][..], |os| os.help_group());
            (*title, os.iter().chain(names.iter()).copied().collect())
        })
        .collect()
}

/// `EXAMPLES`, and namiOS's.
fn all_examples() -> impl Iterator<Item = &'static (&'static str, &'static [(&'static str, &'static str)])> {
    EXAMPLES.iter().chain(crate::nami_os::plugin().map_or(&[][..], |os| os.examples()))
}

/// The commands in `GROUPS`, by group, each with its short line.
fn grouped(cmd: &clap::Command) -> Vec<(&'static str, Vec<(String, String)>)> {
    groups()
        .into_iter()
        .map(|(title, names)| (title, names.iter().map(|n| (n.to_string(), about(cmd, n))).collect()))
        .collect()
}

/// The commands no group lists (but `help`): plumbing nami runs itself.
fn others(cmd: &clap::Command) -> Vec<(String, String)> {
    let listed: Vec<&str> = groups().into_iter().flat_map(|(_, n)| n).collect();
    cmd.get_subcommands()
        .filter(|s| s.get_name() != "help" && !listed.contains(&s.get_name()))
        .map(|s| (s.get_name().to_string(), about(cmd, s.get_name())))
        .collect()
}

/// A command's short line: the first line of its doc comment.
fn about(cmd: &clap::Command, name: &str) -> String {
    cmd.find_subcommand(name)
        .and_then(|s| s.get_about())
        .map(|a| a.to_string().lines().next().unwrap_or_default().trim().to_string())
        .unwrap_or_default()
}

/// `nami – declarative Linux made easy  0.18.9`
fn header(p: Paint) -> String {
    format!("{} {} {PROMISE}  {}\n", p.bold("nami"), p.dim("–"), p.dim(env!("CARGO_PKG_VERSION")))
}

/// Rows of `name  what it does`, the names lined up.
fn rows(p: Paint, rows: &[(String, String)], width: usize) -> String {
    rows.iter().map(|(n, a)| format!("  {}{}  {a}\n", p.accent(n), " ".repeat(width - n.len()))).collect()
}

/// `$ nami install firefox`: the `$` dim, `nami <command>` cyan.
fn example(p: Paint, line: &str) -> String {
    let mut words = line.splitn(3, ' ');
    let (nami, cmd, rest) = (words.next().unwrap_or_default(), words.next().unwrap_or_default(), words.next());
    let typed = p.accent(&format!("{nami} {cmd}").trim().to_string());
    format!("  {} {typed}{}\n", p.dim("$"), rest.map(|r| format!(" {r}")).unwrap_or_default())
}

/// `nami help` (with `all`, the plumbing too).
pub fn top(p: Paint, cmd: &clap::Command, all: bool) -> String {
    let groups = grouped(cmd);
    let width = groups.iter().flat_map(|(_, r)| r.iter().map(|(n, _)| n.len())).max().unwrap_or(0);
    let mut out = header(p);
    out += &format!("\n{} {} {}\n", p.bold("Usage:"), p.accent("nami"), "<COMMAND> [OPTIONS]");
    for (title, list) in &groups {
        out += &format!("\n{}\n", p.bold(title));
        out += &rows(p, list, width);
    }
    if all {
        // (Their own column: their names are longer.)
        let list = others(cmd);
        out += &format!("\n{}\n", p.bold("Other commands (nami runs these itself)"));
        out += &rows(p, &list, list.iter().map(|(n, _)| n.len()).max().unwrap_or(0));
    }
    out += &format!("\n{}\n", p.bold("Options"));
    let opts = [
        ("--color <WHEN>".to_string(), "auto, always or never".to_string()),
        ("-h, --help".to_string(), "Show help".to_string()),
        ("-V, --version".to_string(), "Show nami's version".to_string()),
    ];
    out += &rows(p, &opts, width);
    out += &format!("\n{}\n", p.bold("Examples"));
    for (_, line) in TOP_EXAMPLES {
        out += &example(p, line);
    }
    out += &format!("\n{}\n", p.bold("Learn more"));
    out += &format!("  {} shows a command's details and examples\n", p.accent("nami <command> --help"));
    if !all {
        out += &format!("  {} lists every command, the rarely typed ones too\n", p.accent("nami help --all"));
    }
    out += &format!("  {}\n", p.underline(DOCS));
    out
}

/// `nami` alone: a few commands to start with.
pub fn welcome(p: Paint, cmd: &clap::Command, first_time: bool) -> String {
    let list: Vec<(String, String)> = WELCOME.iter().map(|n| (n.to_string(), about(cmd, n))).collect();
    let width = list.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    let mut out = header(p);
    out += &format!("\n{}\n", p.bold("Common commands"));
    out += &rows(p, &list, width);
    if first_time {
        out += &format!("\n{} {}\n", p.bold("Get started:"), p.accent("nami install firefox"));
    }
    out += &format!("\nSee every command with {}, or read {}\n", p.accent("nami help"), p.underline(DOCS));
    out
}

/// A command's examples, under its `--help`.
pub fn examples(p: Paint, name: &str) -> Option<String> {
    let (_, list) = all_examples().find(|(n, _)| *n == name)?;
    let mut out = format!("{}\n", p.bold("Examples:"));
    for (i, (what, line)) in list.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out += &format!("  {}\n", p.dim(&format!("# {what}")));
        out += &example(p, line);
    }
    Some(out.trim_end().to_string())
}

/// nami's command line, ready to parse: the top-level help ours, each
/// common command's examples under its own, colour as `--color` says.
/// (The colours are written in; clap leaves them out where colour is off.)
pub fn command(choice: crate::style::Choice) -> clap::Command {
    let mut cmd = crate::cli::command();
    let colour = Paint(true);
    let help = top(colour, &cmd, false);
    for (name, _) in all_examples() {
        let text = examples(colour, name).unwrap_or_default();
        cmd = cmd.mut_subcommand(name, |s| s.after_help(StyledStr::from(text)));
    }
    cmd.override_help(StyledStr::from(help)).color(match choice {
        crate::style::Choice::Always => clap::ColorChoice::Always,
        crate::style::Choice::Never => clap::ColorChoice::Never,
        crate::style::Choice::Auto => clap::ColorChoice::Auto,
    })
}

/// `--color WHEN` (or `--color=WHEN`) before nami's own parsing, which needs it.
pub fn color_arg(args: &[String]) -> crate::style::Choice {
    use crate::style::Choice;
    let mut it = args.iter().skip(1).take_while(|a| *a != "--");
    let mut choice = Choice::Auto;
    while let Some(a) = it.next() {
        let v = match a.strip_prefix("--color") {
            Some("") => it.next().map(String::as_str),
            Some(v) => v.strip_prefix('='),
            None => None,
        };
        choice = match v {
            Some("always") => Choice::Always,
            Some("never") => Choice::Never,
            Some("auto") => Choice::Auto,
            _ => choice,
        };
    }
    choice
}

/// What nami says itself instead of clap, for the arguments as they are
/// (`--color` taken out): `nami` alone, or `nami help`, `--all` or `--json`.
pub enum Screen {
    Welcome,
    Help { all: bool },
    Json,
}

pub fn screen(args: &[String]) -> Option<Screen> {
    let mut rest = Vec::new();
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--color" => {
                it.next();
            }
            a if a.starts_with("--color=") => {}
            a => rest.push(a),
        }
    }
    match rest.as_slice() {
        [] => Some(Screen::Welcome),
        ["help" | "--help" | "-h"] => Some(Screen::Help { all: false }),
        ["help" | "--help" | "-h", "--all" | "-a"] | ["--all" | "-a", "--help" | "-h"] => Some(Screen::Help { all: true }),
        ["help", "--json"] => Some(Screen::Json),
        _ => None,
    }
}

/// The groups as JSON, for the docs (packaging/docs-data.py):
/// `[{"group", "commands": [{"name", "about"}]}]`.
pub fn json(cmd: &clap::Command) -> serde_json::Value {
    grouped(cmd)
        .into_iter()
        .map(|(g, list)| serde_json::json!({ "group": g, "commands": list.into_iter().map(|(n, a)| serde_json::json!({ "name": n, "about": a })).collect::<Vec<_>>() }))
        .collect()
}

/// A command line clap didn't take, said as nami says its errors; `None`:
/// clap's own words (help, the version, and the rarer mistakes).
pub fn error(p: Paint, e: &clap::Error) -> Option<String> {
    let get = |k: ContextKind| match e.get(k) {
        Some(ContextValue::String(s)) => Some(s.clone()),
        Some(ContextValue::Strings(v)) => v.last().cloned(), // (clap lists suggestions best last.)
        Some(ContextValue::StyledStr(s)) => Some(s.to_string()),
        Some(ContextValue::StyledStrs(v)) => v.first().map(|s| s.to_string()),
        _ => None,
    };
    let usage = get(ContextKind::Usage).map(|u| u.trim().trim_start_matches("Usage:").trim().to_string());
    // `nami install` of `Usage: nami install [OPTIONS] ...`.
    let path = usage.as_deref().map(|u| u.split(' ').take_while(|w| !w.starts_with(['[', '<', '-'])).collect::<Vec<_>>().join(" ")).unwrap_or_else(|| "nami".into());
    let quoted = |s: &str| p.bold(&format!("'{s}'"));
    let (line, hint) = match e.kind() {
        ErrorKind::InvalidSubcommand => {
            let bad = get(ContextKind::InvalidSubcommand)?;
            let mut line = format!("{} isn't a {} command.", quoted(&bad), if path == "nami" { "nami".to_string() } else { format!("'{path}'") });
            if let Some(s) = get(ContextKind::SuggestedSubcommand) {
                line += &format!(" Did you mean {}?", quoted(&s));
            }
            let all = if path == "nami" { "nami help".to_string() } else { format!("{path} --help") };
            (line, format!("{} lists them all", p.accent(&all)))
        }
        ErrorKind::UnknownArgument => {
            let bad = get(ContextKind::InvalidArg)?;
            let mut line = format!("{} doesn't take {}.", p.accent(&path), quoted(&bad));
            if let Some(s) = get(ContextKind::SuggestedArg) {
                line += &format!(" Did you mean {}?", quoted(&s));
            }
            (line, format!("{} lists what it takes", p.accent(&format!("{path} --help"))))
        }
        ErrorKind::MissingRequiredArgument => {
            let missing = match e.get(ContextKind::InvalidArg) {
                Some(ContextValue::Strings(v)) => v.clone(),
                _ => return None,
            };
            let line = format!("{} needs {}.", p.accent(&path), missing.iter().map(|m| p.bold(m)).collect::<Vec<_>>().join(" and "));
            (line, format!("{} says more", p.accent(&format!("{path} --help"))))
        }
        _ => return None,
    };
    let mut out = format!("{}: {line}\n", p.error("error"));
    if let Some(u) = usage.filter(|_| e.kind() == ErrorKind::MissingRequiredArgument) {
        out += &format!("\n  {} {u}\n\n", p.bold("Usage:"));
    }
    out += &format!("{}: {hint}\n", p.note("hint"));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (clap's unoptimised build of all of nami's commands needs a big stack.)
    fn big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new().stack_size(32 << 20).spawn(f).unwrap().join().unwrap()
    }

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    /// Escape codes out, for counting columns.
    fn plain(s: &str) -> String {
        let mut out = String::new();
        let mut esc = false;
        for c in s.chars() {
            match (esc, c) {
                (false, '\x1b') => esc = true,
                (true, 'm') => esc = false,
                (true, _) => {}
                _ => out.push(c),
            }
        }
        out
    }

    #[test]
    fn every_command_is_in_one_group_or_hidden() {
        big_stack(|| {
            let cmd = crate::cli::command();
            let listed: Vec<&str> = groups().into_iter().flat_map(|(_, n)| n).collect();
            for n in &listed {
                assert_eq!(listed.iter().filter(|m| *m == n).count(), 1, "{n} is in two groups");
                let s = cmd.find_subcommand(n).unwrap_or_else(|| panic!("no command {n}"));
                assert!(!s.is_hide_set(), "{n} is hidden but listed");
            }
            for s in cmd.get_subcommands() {
                let n = s.get_name();
                assert!(n == "help" || s.is_hide_set() || listed.contains(&n), "{n}: in no group, and not hidden");
            }
            for (n, _) in all_examples() {
                assert!(cmd.find_subcommand(n).is_some(), "examples for no command {n}");
            }
        })
    }

    #[test]
    fn the_help_is_short_lines() {
        big_stack(|| {
            let cmd = crate::cli::command();
            for all in [false, true] {
                let text = top(Paint(true), &cmd, all);
                assert_eq!(plain(&text), top(Paint(false), &cmd, all), "colour changes only the colour");
                assert!(!top(Paint(false), &cmd, all).contains('\x1b'));
                for l in plain(&text).lines() {
                    assert!(l.chars().count() <= 70, "{} columns: {l}", l.chars().count());
                }
            }
            let text = top(Paint(false), &cmd, false);
            assert!(text.contains("  install  ") && text.contains("Install apps\n"), "{text}");
            assert!(!text.contains("daemon") && !text.contains("attach-bundle-runtime") && !text.contains("os-update"));
            assert!(top(Paint(false), &cmd, true).contains("attach-bundle-runtime"));
            for l in plain(&welcome(Paint(true), &cmd, true)).lines() {
                assert!(l.chars().count() <= 70, "{l}");
            }
            // Every command a short line of its own, in sentence case, no full stop.
            for s in cmd.get_subcommands().filter(|s| s.get_name() != "help") {
                let a = about(&cmd, s.get_name());
                assert!(!a.is_empty() && a.len() <= 50 && !a.ends_with('.') && a.starts_with(|c: char| c.is_uppercase()), "{}: {a:?}", s.get_name());
            }
        })
    }

    #[test]
    fn hidden_commands_still_parse() {
        big_stack(|| {
            for a in [&["nami", "daemon"][..], &["nami", "os-update"], &["nami", "hook-env"], &["nami", "attach-bundle-runtime", "a", "b"], &["nami", "history"], &["nami", "generations"], &["nami", "rollback"]] {
                command(crate::style::Choice::Never).try_get_matches_from(strings(a)).unwrap_or_else(|e| panic!("{a:?}: {e}"));
            }
        })
    }

    #[test]
    fn mistakes_are_said_plainly() {
        big_stack(|| {
            let parse = |a: &[&str]| command(crate::style::Choice::Never).try_get_matches_from(strings(a)).err().unwrap();
            let said = error(Paint(false), &parse(&["nami", "instal", "firefox"])).unwrap();
            assert_eq!(said, "error: 'instal' isn't a nami command. Did you mean 'install'?\nhint: nami help lists them all\n");
            let said = error(Paint(false), &parse(&["nami", "install", "--sytem"])).unwrap();
            assert!(said.starts_with("error: nami install doesn't take '--sytem'. Did you mean '--system'?\n"), "{said}");
            assert!(said.ends_with("hint: nami install --help lists what it takes\n"), "{said}");
            let said = error(Paint(false), &parse(&["nami", "keygen"])).unwrap();
            assert!(said.starts_with("error: nami keygen needs <NAME>.\n"), "{said}");
            let said = error(Paint(false), &parse(&["nami", "config", "sett"])).unwrap();
            assert!(said.starts_with("error: 'sett' isn't a 'nami config' command. Did you mean 'set'?\n"), "{said}");
        })
    }

    #[test]
    fn screens_and_colour_from_the_arguments() {
        use crate::style::Choice;
        assert!(matches!(screen(&strings(&["nami"])), Some(Screen::Welcome)));
        assert!(matches!(screen(&strings(&["nami", "--color", "always"])), Some(Screen::Welcome)));
        assert!(matches!(screen(&strings(&["nami", "--help"])), Some(Screen::Help { all: false })));
        assert!(matches!(screen(&strings(&["nami", "help", "--all"])), Some(Screen::Help { all: true })));
        assert!(matches!(screen(&strings(&["nami", "help", "--json"])), Some(Screen::Json)));
        assert!(screen(&strings(&["nami", "help", "install"])).is_none());
        assert!(screen(&strings(&["nami", "install", "--help"])).is_none());
        assert_eq!(color_arg(&strings(&["nami", "--color", "never", "list"])), Choice::Never);
        assert_eq!(color_arg(&strings(&["nami", "list", "--color=always"])), Choice::Always);
        assert_eq!(color_arg(&strings(&["nami", "run", "--", "ls", "--color=always"])), Choice::Auto);
    }
}
