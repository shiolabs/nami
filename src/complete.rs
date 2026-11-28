//! Tab completion in bash, zsh and fish. `nami completions <shell>` prints
//! the script a shell loads (src/complete/); the script asks `nami
//! __complete <shell> -- <words>` what fits the word being typed, and nami
//! answers a line each: the word, a tab, a few words about it. A line
//! `:files` (or `:dirs`) says the shell's own file names fit too.
//!
//! The commands and options come from nami's command line itself (clap's),
//! each with its short line; the rest from what's here: packages from the
//! catalog by their first letters (its compact form, read in place: apps
//! first, the most used first), a package's versions by line (`firefox@`),
//! what's installed, the project's tasks and services, the changes to go
//! back to, a nami file's keys. A Tab never waits on the network: a
//! package's version history not here yet is fetched by a process of its
//! own, not waited for (`nami __complete-fetch`), for the next Tab.

use std::path::{Path, PathBuf};

use clap::{Arg, ArgAction, Command, ValueHint};

pub const BASH: &str = include_str!("complete/nami.bash");
pub const ZSH: &str = include_str!("complete/nami.zsh");
pub const FISH: &str = include_str!("complete/nami.fish");

/// The shells nami completes in.
pub const SHELLS: &[&str] = &["bash", "zsh", "fish"];

/// How many packages are offered at most: the first ones, apps first.
const MOST: usize = 50;

/// The script `shell` loads.
pub fn script(shell: &str) -> Option<&'static str> {
    match shell {
        "bash" => Some(BASH),
        "zsh" => Some(ZSH),
        "fish" => Some(FISH),
        _ => None,
    }
}

/// One thing that fits: what's typed, and a few words about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub value: String,
    pub about: String,
}

fn candidate(value: impl Into<String>, about: impl Into<String>) -> Candidate {
    Candidate { value: value.into(), about: about.into() }
}

/// What fits where the word is being typed.
#[derive(Debug, Default, PartialEq)]
pub struct Answer {
    pub candidates: Vec<Candidate>,
    /// The shell's own file names fit too: any (`f`) or folders (`d`).
    pub files: Option<char>,
}

impl Answer {
    fn of(candidates: Vec<Candidate>) -> Self {
        Answer { candidates, files: None }
    }

    fn files() -> Self {
        Answer { candidates: Vec::new(), files: Some('f') }
    }

    fn dirs() -> Self {
        Answer { candidates: Vec::new(), files: Some('d') }
    }
}

/// `nami __complete <shell> -- <words>`: what fits the last of `words` (the
/// words after `nami`, the last one being typed), a line each.
pub fn main(cmd: Command, args: &[String]) {
    let words: Vec<String> = match args.iter().position(|a| a == "--") {
        Some(i) => args[i + 1..].to_vec(),
        None => args.iter().skip(1).cloned().collect(),
    };
    // A project's layers as they're here: never the network (for the whole
    // process, so here and not where the project is read: tests share it).
    crate::layers::set_mode(crate::layers::Mode::Offline);
    let answer = answer(cmd, &words, &Here::from_env());
    let mut out = String::new();
    match answer.files {
        Some('d') => out.push_str(":dirs\n"),
        Some(_) => out.push_str(":files\n"),
        None => {}
    }
    for c in &answer.candidates {
        let about = one_line(&c.about);
        if about.is_empty() {
            out += &format!("{}\n", c.value);
        } else {
            out += &format!("{}\t{about}\n", c.value);
        }
    }
    use std::io::Write;
    let _ = std::io::stdout().write_all(out.as_bytes());
}

/// A few words on one line, short enough for a menu.
fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let flat = flat.trim_end_matches('.');
    if flat.chars().count() <= 72 {
        return flat.to_string();
    }
    let cut: String = flat.chars().take(71).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(a, _)| a).trim_end_matches([',', ';', ':']);
    format!("{cut}…")
}

/// Where the answers come from: the computer (or a test's scratch root).
pub struct Here {
    pub root: PathBuf,
    pub cwd: PathBuf,
}

impl Here {
    fn from_env() -> Self {
        Here {
            root: PathBuf::from(std::env::var_os("NAMI_ROOT").unwrap_or_else(|| "/".into())),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
}

/// Where the command line is: the command reached, its options so far, and
/// its positional words.
struct Place<'a> {
    cmd: &'a Command,
    /// The commands' names from the top: ["config", "set"].
    path: Vec<String>,
    /// Options of the commands above that hold below too (--color, config's --file).
    globals: Vec<&'a Arg>,
    /// Options given: (its id, its value).
    given: Vec<(String, Option<String>)>,
    positionals: Vec<String>,
    /// After `--` (or a command's own words: `nami run -- npm test`).
    raw: bool,
}

impl<'a> Place<'a> {
    fn options(&self) -> impl Iterator<Item = &'a Arg> + '_ {
        self.cmd.get_arguments().filter(|a| !a.is_positional()).chain(self.globals.iter().copied())
    }

    fn long(&self, name: &str) -> Option<&'a Arg> {
        self.options().find(|a| a.get_long() == Some(name) || a.get_all_aliases().is_some_and(|al| al.contains(&name)))
    }

    fn short(&self, c: char) -> Option<&'a Arg> {
        self.options().find(|a| a.get_short() == Some(c) || a.get_all_short_aliases().is_some_and(|al| al.contains(&c)))
    }

    fn given(&self, id: &str) -> Option<Option<&str>> {
        self.given.iter().find(|(i, _)| i == id).map(|(_, v)| v.as_deref())
    }

    fn path(&self) -> String {
        self.path.join(" ")
    }
}

fn takes_value(a: &Arg) -> bool {
    matches!(a.get_action(), ArgAction::Set | ArgAction::Append)
}

/// The positionals a command takes, in order.
fn positionals(cmd: &Command) -> Vec<&Arg> {
    cmd.get_positionals().filter(|a| !a.is_hide_set()).collect()
}

fn takes_many(a: &Arg) -> bool {
    matches!(a.get_action(), ArgAction::Append) || a.get_num_args().is_some_and(|n| n.max_values() > 1)
}

/// What fits the last of `words` (the words after `nami`).
pub fn answer(mut cmd: Command, words: &[String], here: &Here) -> Answer {
    cmd.build();
    let (cur, before) = match words.split_last() {
        Some((cur, before)) => (cur.as_str(), before),
        None => ("", &[][..]),
    };
    let mut place = Place { cmd: &cmd, path: Vec::new(), globals: Vec::new(), given: Vec::new(), positionals: Vec::new(), raw: false };
    let mut pending: Option<&Arg> = None;
    for w in before {
        if let Some(a) = pending.take() {
            place.given.push((a.get_id().to_string(), Some(w.clone())));
            continue;
        }
        if place.raw {
            place.positionals.push(w.clone());
            continue;
        }
        if w == "--" {
            place.raw = true;
            continue;
        }
        if let Some(long) = w.strip_prefix("--") {
            let (name, value) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (long, None),
            };
            if let Some(a) = place.long(name) {
                if takes_value(a) && value.is_none() {
                    pending = Some(a);
                } else {
                    place.given.push((a.get_id().to_string(), value));
                }
            }
            continue;
        }
        if w.len() > 1 && w.starts_with('-') && !w[1..].starts_with(|c: char| c.is_ascii_digit()) {
            for (i, c) in w[1..].char_indices() {
                let Some(a) = place.short(c) else { break };
                if takes_value(a) {
                    let rest = &w[1 + i + c.len_utf8()..];
                    if rest.is_empty() {
                        pending = Some(a);
                    } else {
                        place.given.push((a.get_id().to_string(), Some(rest.to_string())));
                    }
                    break;
                }
                place.given.push((a.get_id().to_string(), None));
            }
            continue;
        }
        if place.positionals.is_empty() {
            if let Some(sub) = place.cmd.find_subcommand(w) {
                place.globals.extend(place.cmd.get_arguments().filter(|a| a.is_global_set()));
                place.cmd = sub;
                place.path.push(sub.get_name().to_string());
                continue;
            }
        }
        place.positionals.push(w.clone());
        // A command's own words from here on (`nami run npm test --watch`).
        let args = positionals(place.cmd);
        if let Some(a) = args.get(place.positionals.len() - 1).or(args.last()) {
            if a.is_trailing_var_arg_set() || a.is_allow_hyphen_values_set() && takes_many(a) {
                place.raw = true;
            }
        }
    }
    if let Some(a) = pending {
        return value_of(&place, a, cur, "", here);
    }
    if !place.raw && cur.starts_with("--") {
        if let Some((name, value)) = cur[2..].split_once('=') {
            let Some(a) = place.long(name).filter(|a| takes_value(a)) else { return Answer::default() };
            let mut answer = value_of(&place, a, value, "", here);
            for c in &mut answer.candidates {
                c.value = format!("--{name}={}", c.value);
            }
            return answer;
        }
        return Answer::of(options(&place, cur));
    }
    if !place.raw && cur.starts_with('-') && cur.len() <= 2 && !cur[1..].starts_with(|c: char| c.is_ascii_digit()) {
        return Answer::of(options(&place, cur));
    }
    if !place.raw && place.positionals.is_empty() && place.cmd.has_subcommands() {
        let mut out = subcommands(place.cmd, cur);
        // A command whose subcommand may be left out takes its own words too.
        if place.cmd.is_subcommand_required_set() || positionals(place.cmd).is_empty() {
            return Answer::of(out);
        }
        if let Some(a) = positionals(place.cmd).first() {
            out.extend(value_of(&place, a, cur, "", here).candidates);
        }
        return Answer::of(out);
    }
    let args = positionals(place.cmd);
    let n = place.positionals.len();
    let Some(a) = args.get(n).or_else(|| args.last().filter(|a| takes_many(a))) else { return Answer::default() };
    value_of(&place, a, cur, &n.to_string(), here)
}

/// The subcommands of `cmd` that start with `cur`, by name.
fn subcommands(cmd: &Command, cur: &str) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = cmd
        .get_subcommands()
        .filter(|s| !s.is_hide_set() && s.get_name() != "help" && s.get_name().starts_with(cur))
        .map(|s| candidate(s.get_name(), first_line(s.get_about().map(|a| a.to_string()))))
        .collect();
    out.sort_by(|a, b| a.value.cmp(&b.value));
    out
}

fn first_line(text: Option<String>) -> String {
    text.unwrap_or_default().lines().next().unwrap_or_default().trim().to_string()
}

/// The options that start with `cur` (long ones; a short one only when it has no long).
fn options(place: &Place, cur: &str) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let given = |a: &Arg| place.given(a.get_id().as_str()).is_some() && !matches!(a.get_action(), ArgAction::Append | ArgAction::Count);
    for a in place.options() {
        if a.is_hide_set() || given(a) {
            continue;
        }
        let about = first_line(a.get_help().map(|h| h.to_string()));
        let name = match (a.get_long(), a.get_short()) {
            (Some(l), _) => format!("--{l}"),
            (None, Some(s)) => format!("-{s}"),
            (None, None) => continue,
        };
        if name.starts_with(cur) && !out.iter().any(|c| c.value == name) {
            out.push(candidate(name, about));
        }
    }
    let help = |c: &Candidate| c.value == "--help" || c.value == "--version";
    out.sort_by(|a, b| help(a).cmp(&help(b)).then(a.value.cmp(&b.value)));
    if "--help".starts_with(cur) && !out.iter().any(|c| c.value == "--help") {
        out.push(candidate("--help", "Show what it does, and its options"));
    }
    out
}

/// What fits as `a`'s value (`which`: how many words the command has before it, or "" for an option's).
fn value_of(place: &Place, a: &Arg, cur: &str, which: &str, here: &Here) -> Answer {
    let id = a.get_id().as_str();
    let path = place.path();
    let found = match (path.as_str(), id) {
        ("install", "packages") => Some(packages_or_files(here, cur, false)),
        ("search", "term") | ("versions", "packages") | ("bundle", "what") | ("try", "package") => Some(packages_or_files(here, cur, false)),
        ("info", "packages") => Some(packages_or_files(here, cur, true)),
        ("remove", "packages") => Some(Answer::of(starting(installed(here), cur))),
        ("run", "command") if which == "0" => Some(Answer::of(starting(tasks(here), cur))),
        ("run", "command") => Some(Answer::files()),
        ("up", "names") | ("down", "names") | ("logs", "name") => {
            let given = &place.positionals;
            Some(Answer::of(starting(services(here), cur).into_iter().filter(|c| !given.contains(&c.value)).collect()))
        }
        ("undo", "generation") | ("diff", "from") | ("diff", "to") => {
            // (Yours, unless sudo or --system says the system's.)
            let system = place.given("system").is_some();
            Some(Answer::of(starting(changes(here, system), cur)))
        }
        ("config set" | "config unset" | "config get", "path") => Some(Answer::of(config_keys(&config_file(place), cur, false))),
        ("config add" | "config remove", "path") => Some(Answer::of(config_keys(&config_file(place), cur, true))),
        ("config add", "value") if place.positionals.first().map(String::as_str) == Some("packages") => Some(packages_or_files(here, cur, false)),
        ("config remove", "value") => {
            let key = place.positionals.first().cloned().unwrap_or_default();
            Some(Answer::of(starting(list_items(&config_file(place), &key), cur)))
        }
        ("new", "from") => Some(Answer::of(starting(starters(), cur))),
        ("new", "folder") => Some(Answer::dirs()),
        ("options", "schema") => Some(Answer::of(starting(crate::schema::KINDS.iter().map(|k| candidate(*k, format!("A {k} file"))).collect(), cur))),
        ("search", "category") => Some(Answer::of(starting(crate::query::CATEGORIES.iter().map(|(k, about, _)| candidate(*k, *about)).collect(), cur))),
        ("hook" | "completions", "shell") => Some(Answer::of(starting(SHELLS.iter().map(|s| candidate(*s, "")).collect(), cur))),
        _ => None,
    };
    if let Some(found) = found {
        return found;
    }
    let possible: Vec<Candidate> = a
        .get_possible_values()
        .iter()
        .filter(|v| !v.is_hide_set())
        .map(|v| candidate(v.get_name(), v.get_help().map(|h| h.to_string()).unwrap_or_default()))
        .collect();
    if !possible.is_empty() {
        return Answer::of(starting(possible, cur));
    }
    match a.get_value_hint() {
        ValueHint::AnyPath | ValueHint::FilePath | ValueHint::ExecutablePath => Answer::files(),
        ValueHint::DirPath => Answer::dirs(),
        _ => Answer::default(),
    }
}

fn starting(list: Vec<Candidate>, cur: &str) -> Vec<Candidate> {
    list.into_iter().filter(|c| c.value.starts_with(cur)).collect()
}

/// Packages by their first letters, a version of one (`firefox@`), or a
/// file (./app.deb, a recipe's folder).
fn packages_or_files(here: &Here, cur: &str, installed_first: bool) -> Answer {
    if cur.starts_with(['.', '/', '~']) {
        return Answer::files();
    }
    if let Some((name, _)) = cur.split_once('@') {
        return Answer::of(starting(versions(here, name), cur));
    }
    if cur.contains(':') {
        return Answer::default();
    }
    let mut out = if installed_first { starting(installed(here), cur) } else { Vec::new() };
    if let Some(catalog) = catalog(here) {
        for c in packages(&catalog, cur, MOST) {
            if !out.iter().any(|o| o.value == c.value) {
                out.push(c);
            }
        }
    }
    Answer::of(out)
}

/// The catalog's compact form, read in place, where `nami update` (or nami
/// as you, in your cache) keeps it.
fn catalog(here: &Here) -> Option<crate::index_file::Catalog> {
    let var = crate::migrate::var_dir(&here.root);
    let cache = if crate::profile::is_root() {
        var.clone()
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("nami")
    };
    crate::index_file::Catalog::open(&var.join("index.json"), Some(&cache.join("catalogs"))).ok()
}

/// What's said about a package, read from its entry alone.
#[derive(serde::Deserialize)]
struct Brief {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// The catalog's packages whose names start with `prefix`: apps first (the
/// most used first), then the rest by name, `most` of them.
pub fn packages(catalog: &crate::index_file::Catalog, prefix: &str, most: usize) -> Vec<Candidate> {
    let mut found: Vec<(bool, u16, usize)> = Vec::new();
    for i in catalog.first_from(prefix)..catalog.len() {
        if !catalog.name(i).starts_with(prefix) {
            break;
        }
        let r = catalog.record(i);
        found.push((r.flags & crate::index_file::APP != 0, r.popularity, i));
    }
    // Apps first, the most used first; then the rest in name order (a name
    // itself before the longer ones that start with it).
    found.sort_by(|a, b| b.0.cmp(&a.0).then(if a.0 { b.1.cmp(&a.1) } else { std::cmp::Ordering::Equal }).then(a.2.cmp(&b.2)));
    found
        .into_iter()
        .take(most)
        .map(|(_, _, i)| {
            let brief: Option<Brief> = serde_json::from_slice(catalog.value(i)).ok();
            let about = brief.and_then(|b| b.summary.filter(|s| !s.is_empty()).or(b.description)).unwrap_or_default();
            candidate(catalog.name(i), about)
        })
        .collect()
}

/// The store, read only: as root, the computer's; else as nami runs for you
/// (its caches yours).
fn store(here: &Here) -> Option<crate::store::Store> {
    if crate::profile::is_root() {
        return crate::store::Store::open_direct_readonly(&here.root).ok();
    }
    crate::store::Store::open(&here.root).or_else(|_| crate::store::Store::open_direct_readonly(&here.root)).ok()
}

/// How long the fetch behind a Tab may take before it gives up.
const FETCH_FOR: std::time::Duration = std::time::Duration::from_secs(10);

/// `name`'s versions by line (`firefox@159`), as `nami versions` groups them.
fn versions(here: &Here, name: &str) -> Vec<Candidate> {
    versions_with(here, name, fetch_later)
}

/// The same, never waiting on the network: from the history that's here.
/// When it isn't here yet, the catalog's own version, and `fetch` is asked
/// to get it for the next Tab.
fn versions_with(here: &Here, name: &str, fetch: impl FnOnce(&str)) -> Vec<Candidate> {
    let Some(store) = store(here) else { return Vec::new() };
    let Ok(index) = crate::index::Index::load(&store) else { return Vec::new() };
    let list = match index.versions_here(name) {
        Some(list) => list,
        None => {
            let Some(p) = index.find(name) else { return Vec::new() };
            fetch(name);
            vec![crate::index::Version(crate::query::version_of(&p.name), String::new(), String::new(), Default::default())]
        }
    };
    let siblings: Vec<(String, String)> = index
        .siblings(name)
        .into_iter()
        .filter_map(|s| index.find(s).map(|p| (s.to_string(), crate::query::version_of(&p.name))))
        .collect();
    version_lines(name, &list, &siblings)
}

/// Fetch `name`'s version history in a process of its own, not waited
/// for: `nami __complete-fetch <name>` (it gives up after `FETCH_FOR`).
fn fetch_later(name: &str) {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let Ok(me) = std::env::current_exe() else { return };
    let _ = Command::new(me)
        .args(["__complete-fetch", name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Its own process group: the shell's Ctrl-C and job control leave it be.
        .process_group(0)
        .spawn();
}

/// `nami __complete-fetch <name>`: fetch its version history into the cache,
/// for the next Tab; gone after `FETCH_FOR` whatever happens.
pub fn fetch(args: &[String]) {
    std::thread::spawn(|| {
        std::thread::sleep(FETCH_FOR);
        std::process::exit(0);
    });
    let (Some(name), Some(store)) = (args.first(), store(&Here::from_env())) else { return };
    if let Ok(index) = crate::index::Index::load(&store) {
        let _ = index.versions(name);
    }
}

/// A package's lines (159, 158; or python3's 3.13, 3.12), newest first, each
/// with its newest release and when it was the catalog's; then the ones kept
/// under their own names (nodejs_20), when no line has them.
pub fn version_lines(name: &str, list: &[crate::index::Version], siblings: &[(String, String)]) -> Vec<Candidate> {
    let releases = crate::show::releases(list);
    let lines = crate::show::lines(&releases);
    let mut out: Vec<Candidate> = Vec::new();
    for l in &lines {
        let Some(newest) = l.releases.first() else { continue };
        let from = if l.releases.iter().any(|r| r.from.is_none()) { None } else { l.releases.iter().filter_map(|r| r.from.clone()).min() };
        let until = if l.releases.iter().any(|r| r.until.is_none()) { None } else { l.releases.iter().filter_map(|r| r.until.clone()).max() };
        let when = match crate::show::span(from.as_deref(), until.as_deref()) {
            s if s == "now" => "the catalog's".to_string(),
            s => s,
        };
        out.push(candidate(format!("{name}@{}", l.key), format!("{}, {when}", newest.version)));
    }
    let depth = if lines.iter().any(|l| l.key.contains('.')) { 2 } else { 1 };
    for (sibling, version) in siblings {
        let key: String = version.split('.').take(depth).collect::<Vec<_>>().join(".");
        if key.is_empty() || !key.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let value = format!("{name}@{key}");
        if !out.iter().any(|c| c.value == value) {
            out.push(candidate(value, format!("{version}, as {sibling}")));
        }
    }
    out
}

/// What's installed: yours first, then the computer's.
fn installed(here: &Here) -> Vec<Candidate> {
    let Some(store) = store(here) else { return Vec::new() };
    let mut out: Vec<Candidate> = Vec::new();
    let user = std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).ok().filter(|u| !u.is_empty() && !u.contains('/') && !u.starts_with('.'));
    let mut add = |attr: String, about: &str| {
        if !out.iter().any(|c| c.value == attr) {
            out.push(candidate(attr, about));
        }
    };
    if let Some(u) = user.filter(|_| !crate::profile::is_root()) {
        for e in crate::profile::Profiles::user(&store, &u).current_packages().unwrap_or_default() {
            add(e.attr, "Installed for you");
        }
    }
    for e in crate::profile::Profiles::system(&store).current_packages().unwrap_or_default() {
        add(e.attr, "Installed for everyone");
    }
    if let Some((flat, _)) = crate::nami_os::plugin().and_then(|os| os.current_system(&here.root)) {
        for (name, _) in crate::query::flat_packages(&flat) {
            add(name, "Part of this computer");
        }
    }
    out
}

/// The nearest project's file, read without the network.
fn project(here: &Here) -> Option<(PathBuf, crate::project::Project)> {
    let file = crate::project::find(&here.cwd)?;
    let p = crate::project::read(&file).ok()?;
    Some((file, p))
}

/// The nearest project's tasks.
fn tasks(here: &Here) -> Vec<Candidate> {
    let Some((_, p)) = project(here) else { return Vec::new() };
    p.tasks.iter().map(|(name, t)| candidate(name, t.shown())).collect()
}

/// The nearest project's services.
fn services(here: &Here) -> Vec<Candidate> {
    let Some((file, p)) = project(here) else { return Vec::new() };
    let dir = file.parent().unwrap_or(Path::new("."));
    match crate::up::resolve(&p.services, dir) {
        Ok(list) => list.into_iter().map(|s| candidate(s.name, s.about.unwrap_or_default())).collect(),
        Err(_) => p.services.keys().filter_map(|k| k.as_str()).map(|k| candidate(crate::up::name_of(k), "")).collect(),
    }
}

/// The changes to go back to, newest first, each with what it changed.
fn changes(here: &Here, system: bool) -> Vec<Candidate> {
    use crate::profile::{is_root, Profiles};
    let Some(store) = store(here) else { return Vec::new() };
    let user = std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).unwrap_or_default();
    let profiles = if is_root() || system || user.is_empty() { Profiles::system(&store) } else { Profiles::user(&store, &user) };
    let steps = if profiles.is_system() && crate::nami_os::here() {
        crate::nami_os::require().and_then(|os| os.history(&here.root)).map(|mut s| {
            s.reverse();
            s
        })
    } else {
        crate::query::history(&profiles)
    };
    steps.unwrap_or_default().iter().map(|s| candidate(s.generation.to_string(), change_about(s))).collect()
}

/// What a change did, in a few words: `Nov 12: added firefox, removed gimp`.
pub fn change_about(s: &crate::query::Step) -> String {
    let names = |l: &[crate::profile::Entry]| l.iter().map(|e| e.attr.as_str()).collect::<Vec<_>>().join(", ");
    let mut what: Vec<String> = Vec::new();
    if !s.added.is_empty() {
        what.push(format!("added {}", names(&s.added)));
    }
    if !s.removed.is_empty() {
        what.push(format!("removed {}", names(&s.removed)));
    }
    if !s.changed.is_empty() {
        let updated: Vec<String> = s.changed.iter().map(|c| format!("{} {}", c.attr, c.to)).collect();
        what.push(format!("updated {}", updated.join(", ")));
    }
    if s.files > 0 {
        what.push(format!("{} file{}", s.files, if s.files == 1 { "" } else { "s" }));
    }
    if what.is_empty() {
        what.push(format!("{} packages", s.packages));
    }
    let when = s.date.get(..10).map(day).unwrap_or_default();
    let now = if s.current { "now; " } else { "" };
    format!("{now}{}{}", if when.is_empty() { String::new() } else { format!("{when}: ") }, what.join("; "))
}

/// `2026-11-12` -> `Nov 12`.
fn day(date: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mut parts = date.split('-').skip(1);
    match (parts.next().and_then(|m| m.parse::<usize>().ok()), parts.next().and_then(|d| d.parse::<u32>().ok())) {
        (Some(m @ 1..=12), Some(d)) => format!("{} {d}", MONTHS[m - 1]),
        _ => String::new(),
    }
}

/// The nami file `nami config` edits: --file, else the computer's for admins, else yours.
fn config_file(place: &Place) -> PathBuf {
    if let Some(Some(f)) = place.given("file") {
        return PathBuf::from(f);
    }
    // (Sudo decides, as nami config does: root's, or --system, the system's.)
    if crate::profile::is_root() || place.given("system").is_some() {
        crate::config::system_file()
    } else {
        crate::config::user_file()
    }
}

/// The kind of nami file `file` is, for its schema.
fn kind_of(file: &Path) -> &'static str {
    match file.file_name().and_then(|n| n.to_str()).unwrap_or_default() {
        n if n.starts_with("system.") || n.starts_with("computer.") => "computer",
        n if n.starts_with("project.") => "project",
        n if n.starts_with("package.") => "package",
        _ => "user",
    }
}

/// The keys of `file` (`nami config set|get|unset`), a part at a time:
/// the schema's and the file's own (env.EDITOR), with the setting's value
/// or what it's for; `lists`: only the ones that are lists (add, remove).
fn config_keys(file: &Path, cur: &str, lists: bool) -> Vec<Candidate> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    keys_of(&text, kind_of(file), cur, lists)
}

pub fn keys_of(text: &str, kind: &str, cur: &str, lists: bool) -> Vec<Candidate> {
    use serde_yaml::Value;
    // Every key: (path, what it's for, whether it's a list, whether it has keys under it).
    let mut all: Vec<(String, String, bool, bool)> = Vec::new();
    if let Some(schema) = crate::schema::schema(kind) {
        if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
            for (k, v) in props {
                let about = v.get("description").and_then(|d| d.as_str()).unwrap_or_default();
                // (The ones that have moved say so first: not to be offered.)
                if about.contains(" is gone") || about.contains(" is now ") || about.contains(" isn't a key") {
                    continue;
                }
                let list = v.get("type").and_then(|t| t.as_str()) == Some("array");
                let first = about.split(". ").next().unwrap_or(about);
                all.push((k.clone(), first.to_string(), list, k == "namios"));
            }
        }
    }
    if kind == "user" {
        for k in crate::config::NAMIOS_KEYS {
            all.push((format!("namios.{k}"), String::new(), false, false));
        }
    }
    fn walk(v: &Value, at: &str, depth: usize, out: &mut Vec<(String, String, bool, bool)>) {
        let Value::Mapping(m) = v else { return };
        for (k, v) in m {
            let Some(k) = k.as_str().map(str::to_string).or_else(|| k.as_i64().map(|n| n.to_string())) else { continue };
            let path = if at.is_empty() { k } else { format!("{at}.{k}") };
            let about = match v {
                Value::String(s) => format!("now: {s}"),
                Value::Number(n) => format!("now: {n}"),
                Value::Bool(b) => format!("now: {b}"),
                Value::Sequence(s) => format!("a list of {}", s.len()),
                _ => String::new(),
            };
            let (list, has_keys) = (matches!(v, Value::Sequence(_)), matches!(v, Value::Mapping(m) if !m.is_empty()));
            match out.iter_mut().find(|e| e.0 == path) {
                Some(e) => {
                    e.2 |= list;
                    e.3 |= has_keys;
                    if e.1.is_empty() || !about.is_empty() && at.contains('.') {
                        e.1 = about;
                    }
                }
                None => out.push((path.clone(), about, list, has_keys)),
            }
            if depth < 4 {
                walk(v, &path, depth + 1, out);
            }
        }
    }
    if let Ok(v) = serde_yaml::from_str::<Value>(text) {
        walk(&v, "", 0, &mut all);
    }
    // A part at a time: what comes after `cur`, up to the next dot.
    let done = cur.rfind('.').map_or(0, |i| i + 1);
    let mut out: Vec<Candidate> = Vec::new();
    for (path, about, list, has_keys) in &all {
        if !path.starts_with(cur) {
            continue;
        }
        let rest = &path[done..];
        let (value, about) = match rest.find('.') {
            Some(i) => (format!("{}.", &path[..done + i]), String::new()),
            None => (path.clone(), about.clone()),
        };
        let goes_on = value.ends_with('.');
        // add and remove: the lists, and the keys with lists under them.
        if lists && !(if goes_on { all.iter().any(|e| e.2 && e.0.starts_with(&value)) } else { *list }) {
            continue;
        }
        if out.iter().any(|c| c.value == value) {
            continue;
        }
        // A key with more under it: `namios.` to go on; itself too, to set or get it whole.
        if *has_keys && !goes_on && !lists {
            let more = format!("{value}.");
            if !out.iter().any(|c| c.value == more) {
                out.push(candidate(more, ""));
            }
        }
        out.push(candidate(value, about));
    }
    // Under a key with nothing more (`env.`), nothing to offer but itself.
    out.sort_by(|a, b| a.value.trim_end_matches('.').cmp(b.value.trim_end_matches('.')).then(a.value.len().cmp(&b.value.len())));
    out
}

/// What a list in `file` has (`nami config remove packages <Tab>`).
fn list_items(file: &Path, key: &str) -> Vec<Candidate> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let Ok(mut v) = serde_yaml::from_str::<serde_yaml::Value>(&text) else { return Vec::new() };
    for part in key.split('.') {
        let Some(next) = v.get(part).cloned() else { return Vec::new() };
        v = next;
    }
    let Some(list) = v.as_sequence() else { return Vec::new() };
    list.iter()
        .filter_map(|item| match item {
            serde_yaml::Value::String(s) => Some(s.clone()),
            serde_yaml::Value::Mapping(m) => m.keys().next().and_then(|k| k.as_str()).map(str::to_string),
            _ => None,
        })
        .map(|s| candidate(s, ""))
        .collect()
}

/// The examples `nami new --from` starts from.
fn starters() -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for (name, about) in crate::new::starters() {
        if !out.iter().any(|c| c.value == name) {
            out.push(candidate(name, about));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Version;
    use std::collections::BTreeMap;

    fn catalog() -> crate::index_file::Catalog {
        let json = serde_json::json!({
            "fd": { "name": "fd-10.2.0", "outputs": {}, "description": "A simple, fast alternative to find" },
            "file": { "name": "file-5.45", "outputs": {}, "description": "A program that shows the type of files" },
            "filezilla": { "name": "filezilla-3.68", "outputs": {}, "apps": ["filezilla.desktop"], "summary": "Transfer files with FTP", "popularity": 300 },
            "firefox": { "name": "firefox-160.0", "outputs": {}, "apps": ["firefox.desktop"], "summary": "Browse the web", "popularity": 990 },
            "firefox-esr": { "name": "firefox-esr-140.5", "outputs": {}, "apps": ["firefox-esr.desktop"], "summary": "Browse the web, longer supported", "popularity": 400 },
            "fish": { "name": "fish-4.1", "outputs": {}, "description": "Smart and user-friendly command line shell" },
            "gimp": { "name": "gimp-3.0", "outputs": {}, "apps": ["gimp.desktop"], "summary": "Edit pictures", "popularity": 800 },
        });
        crate::index_file::Catalog::from_vec(serde_json::to_vec(&json).unwrap()).unwrap()
    }

    fn values(c: &[Candidate]) -> Vec<&str> {
        c.iter().map(|c| c.value.as_str()).collect()
    }

    #[test]
    fn packages_by_their_first_letters_apps_first_the_most_used_first() {
        let c = catalog();
        let found = packages(&c, "fi", 50);
        assert_eq!(values(&found), ["firefox", "firefox-esr", "filezilla", "file", "fish"]);
        assert_eq!(found[0].about, "Browse the web");
        assert_eq!(found[3].about, "A program that shows the type of files");
        assert_eq!(values(&packages(&c, "fi", 2)), ["firefox", "firefox-esr"]);
        assert_eq!(values(&packages(&c, "firefox", 50)), ["firefox", "firefox-esr"]);
        assert!(packages(&c, "zz", 50).is_empty());
        assert_eq!(packages(&c, "", 50).len(), 7);
    }

    #[test]
    fn versions_by_line_newest_first() {
        let v = |version: &str, date: &str| Version(version.into(), date.into(), "rev".into(), BTreeMap::new());
        let list = [v("160.0", ""), v("159.0.2", "2026-11-12"), v("159.0", "2026-10-28"), v("158.0", "2026-10-01")];
        let lines = version_lines("firefox", &list, &[]);
        assert_eq!(values(&lines), ["firefox@160", "firefox@159", "firefox@158"]);
        assert_eq!(lines[0].about, "160.0, since Nov 12, 2026");
        assert_eq!(lines[1].about, "159.0.2, Oct 1 – Nov 12, 2026");
        // python3: one major, lines by minor; a sibling's when no line has it.
        let py = [v("3.13.1", ""), v("3.12.8", "2026-06-01")];
        let lines = version_lines("python3", &py, &[("python311".into(), "3.11.11".into()), ("python313".into(), "3.13.1".into())]);
        assert_eq!(values(&lines), ["python3@3.13", "python3@3.12", "python3@3.11"]);
        assert_eq!(lines[2].about, "3.11.11, as python311");
        // Only the catalog's own: one line.
        assert_eq!(values(&version_lines("jq", &[v("1.7.1", "")], &[])), ["jq@1"]);
    }

    #[test]
    fn versions_never_wait_on_the_network() {
        let d = tmp("versions");
        let var = d.join("root/nix/var/nami");
        std::fs::create_dir_all(&var).unwrap();
        std::fs::write(var.join("index.json"), serde_json::to_vec(&serde_json::json!({ "firefox": { "name": "firefox-160.0", "outputs": {} } })).unwrap()).unwrap();
        // Histories on a server no one answers at: asked, a Tab would hang.
        let source = "http://10.255.255.1/versions";
        std::fs::write(var.join("versions-source"), source).unwrap();
        let here = here(&d);
        // Not here yet: the catalog's own version at once, and a fetch for the next Tab.
        let started = std::time::Instant::now();
        let mut asked = Vec::new();
        let found = versions_with(&here, "firefox", |n| asked.push(n.to_string()));
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
        assert_eq!(values(&found), ["firefox@160"]);
        assert_eq!(asked, ["firefox"]);
        // Not a package: nothing, and nothing fetched.
        assert!(versions_with(&here, "nope", |_| panic!("fetched")).is_empty());
        // Once the fetch has kept it: every line, and no fetch.
        let cache = var.join("versions").join(&crate::catalog::sha256_hex(source.as_bytes())[..16]);
        std::fs::create_dir_all(&cache).unwrap();
        let shard = &crate::catalog::sha256_hex(b"firefox")[..3];
        std::fs::write(cache.join(format!("{shard}.json")), r#"{"firefox": [["159.0.2", "2026-11-12", "r", {}], ["158.0", "2026-10-01", "r", {}]]}"#).unwrap();
        let found = versions_with(&here, "firefox", |_| panic!("fetched though it's here"));
        assert_eq!(values(&found), ["firefox@160", "firefox@159", "firefox@158"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn config_keys_a_part_at_a_time() {
        let text = "packages:\n  - firefox\nenv:\n  EDITOR: nvim\nnamios:\n  dock:\n    size: small\n";
        let top = keys_of(text, "user", "", false);
        let v = values(&top);
        assert!(v.contains(&"packages") && v.contains(&"env.") && v.contains(&"namios."), "{v:?}");
        assert!(!v.iter().any(|k| k.contains("EDITOR")), "{v:?}");
        let env = keys_of(text, "user", "env.", false);
        assert_eq!(values(&env), ["env.EDITOR"]);
        assert_eq!(env[0].about, "now: nvim");
        let namios = keys_of(text, "user", "namios.d", false);
        let v = values(&namios);
        assert!(v.contains(&"namios.dock.") && v.contains(&"namios.dock") && v.contains(&"namios.displays"), "{v:?}");
        assert_eq!(values(&keys_of(text, "user", "namios.dock.", false)), ["namios.dock.size"]);
        // add and remove: lists only.
        let lists = keys_of(text, "user", "", true);
        assert!(values(&lists).contains(&"packages") && !values(&lists).contains(&"env."), "{:?}", values(&lists));
        // A computer's file: the computer's keys.
        assert!(values(&keys_of("", "computer", "host", false)).contains(&"hostname"));
    }

    #[test]
    fn a_change_in_a_few_words() {
        let e = |a: &str| crate::profile::Entry { attr: a.into(), name: a.into(), paths: Vec::new() };
        let s = crate::query::Step {
            generation: 12,
            date: "2026-11-12 10:00".into(),
            created: 0,
            current: true,
            packages: 30,
            added: vec![e("firefox"), e("gimp")],
            removed: vec![e("vlc")],
            changed: vec![crate::query::Change { attr: "vim".into(), name: "vim-9.2".into(), from: "9.1".into(), to: "9.2".into() }],
            files: 0,
        };
        assert_eq!(change_about(&s), "now; Nov 12: added firefox, gimp; removed vlc; updated vim 9.2");
    }

    fn here(dir: &Path) -> Here {
        Here { root: dir.join("root"), cwd: dir.to_path_buf() }
    }

    fn ask(words: &[&str], dir: &Path) -> Answer {
        let words: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        let here = here(dir);
        // (nami's whole command line, unoptimised, needs more than a test thread's stack.)
        std::thread::scope(|s| {
            std::thread::Builder::new()
                .stack_size(64 << 20)
                .spawn_scoped(s, || answer(crate::cli::command(), &words, &here))
                .unwrap()
                .join()
                .unwrap()
        })
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nami-complete-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn commands_options_and_values_from_the_command_line() {
        let d = tmp("cli");
        let a = ask(&["inst"], &d);
        assert_eq!(values(&a.candidates)[0], "install");
        assert!(values(&a.candidates).iter().all(|c| c.starts_with("inst")));
        assert_eq!(a.candidates[0].about, "Install apps");
        assert!(values(&ask(&["config", ""], &d).candidates).contains(&"set"));
        assert_eq!(values(&ask(&["install", "--sys"], &d).candidates), ["--system"]);
        // nami itself, apart from an app's removal (`nami uninstall` is remove's own other name).
        assert_eq!(values(&ask(&["self-un"], &d).candidates), ["self-uninstall"]);
        assert_eq!(values(&ask(&["self-uninstall", "--y"], &d).candidates), ["--yes"]);
        // Global options, and an option's own values.
        assert!(values(&ask(&["install", "--"], &d).candidates).contains(&"--color"));
        assert_eq!(values(&ask(&["--color", "a"], &d).candidates), ["auto", "always"]);
        assert_eq!(values(&ask(&["install", "--color=n"], &d).candidates), ["--color=never"]);
        // A ValueEnum's values, with what each is (nami desktop look).
        let look = ask(&["desktop", "look", ""], &d);
        assert_eq!(values(&look.candidates), ["light", "dark", "auto"]);
        assert_eq!(look.candidates[2].about, "Light by day, dark at night");
        assert_eq!(values(&ask(&["desktop", "wi"], &d).candidates), ["wifi", "windows"]);
        // A path: the shell's file names.
        assert_eq!(ask(&["apply", ""], &d).files, Some('f'));
        assert_eq!(ask(&["install", "./"], &d).files, Some('f'));
        assert_eq!(ask(&["new", "shop", "--from", "no"], &d).candidates.first().map(|c| c.value.as_str()), Some("node"));
        assert_eq!(values(&ask(&["completions", ""], &d).candidates), ["bash", "zsh", "fish"]);
        // No catalog in the scratch root: nothing, and no error.
        assert!(ask(&["install", "fi"], &d).candidates.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_projects_tasks_and_services() {
        let d = tmp("project");
        std::fs::write(d.join("project.nami"), "tasks:\n  test: cargo test\n  lint: { run: cargo clippy, description: Checks the style }\nservices:\n  - worker:\n      run: sleep 100\n      about: Does the work\n").unwrap();
        let tasks = ask(&["run", ""], &d);
        assert_eq!(values(&tasks.candidates), ["lint", "test"]);
        assert_eq!(tasks.candidates[0].about, "Checks the style");
        assert_eq!(values(&ask(&["run", "t"], &d).candidates), ["test"]);
        // After the task: its words, files.
        assert_eq!(ask(&["run", "test", ""], &d).files, Some('f'));
        assert_eq!(values(&ask(&["up", ""], &d).candidates), ["worker"]);
        assert_eq!(values(&ask(&["logs", "w"], &d).candidates), ["worker"]);
        assert!(ask(&["up", "worker", ""], &d).candidates.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn every_shell_has_a_script() {
        for s in SHELLS {
            let text = script(s).unwrap();
            assert!(text.contains("nami __complete"), "{s}");
        }
        assert!(script("tcsh").is_none());
    }
}
