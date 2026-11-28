//! Run anything once: `, cowsay hi` (`nami try cowsay hi`), and what a
//! shell says when a command isn't there (`command_not_found_handler`).
//!
//! Which package has a command comes from a table of the catalog's
//! programs, made here from nix-index-database's small database (the
//! files in each package's bin/, for nixpkgs as of last week: 1.8 MB, out
//! every week), with the catalog's own `mainProgram`s added and only the
//! catalog's packages kept, best first. It's looked up by bisecting the
//! file, so a shell asks it in a few milliseconds, offline. root keeps one
//! for everyone in /nami (os-update, weekly); without that, each
//! person has their own in ~/.cache/nami.
//!
//! The table only says which package to suggest: what runs is the catalog's
//! package, from the signed catalog and the binary cache it trusts, as
//! `nami install` would have it, and the person is shown its name first.
//!
//! What runs once isn't installed: it's fetched into the store (through the
//! daemon, which anyone may ask for what the catalog has), held while it
//! runs (profiles' `running/`), and goes at the next `nami gc` after.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, IsTerminal, Read, Write};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, bail, ensure, Context, Result};

use crate::index::Index;
use crate::store::{base_name, write_atomic, Store};

/// nix-index-database's small database (only bin/), newest release. (ARM Linux's is its own.)
pub const SOURCE: &str = if crate::index::ARM_LINUX {
    "https://github.com/nix-community/nix-index-database/releases/latest/download/index-aarch64-linux-small"
} else {
    "https://github.com/nix-community/nix-index-database/releases/latest/download/index-x86_64-linux-small"
};
/// The table's first line.
const HEADER: &str = "#nami-programs 1";
/// Looked up again after this long.
pub const FRESH: Duration = Duration::from_secs(7 * 24 * 3600);
/// What a list of choices shows at most.
const SHOWN: usize = 4;

/// root's table, for everyone; else each person's.
pub fn system_table(store_var: &Path) -> PathBuf {
    store_var.join("programs.tsv")
}

pub fn own_table() -> PathBuf {
    cache_dir().join("programs.tsv")
}

fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into())).join(".cache"))
        .join("nami")
}

/// The table to look in: the newer of root's and one's own.
pub fn table(store_var: &Path) -> Option<(PathBuf, SystemTime)> {
    [system_table(store_var), own_table()]
        .into_iter()
        .filter_map(|p| fs::metadata(&p).and_then(|m| m.modified()).ok().map(|t| (p, t)))
        .max_by_key(|(_, t)| *t)
}

// ---- nix-index's database ----

/// What nix-index's database says: for each package (attr, output) that's
/// a top-level attribute, the names in its bin/.
pub fn decode_nix_index(file: &[u8]) -> Result<Vec<(String, String, Vec<String>)>> {
    ensure!(file.len() > 12 && &file[..4] == b"NIXI", "not a nix-index database");
    let version = u64::from_le_bytes(file[4..12].try_into().unwrap());
    ensure!(version == 1, "nix-index database version {version}, not 1");
    let data = zstd::stream::decode_all(&file[12..]).context("unpacking the nix-index database")?;
    decode_frcode(&data)
}

/// Its entries, front-coded: `<meta>\0<shared-length change><rest of path>\n`,
/// the change one signed byte, or 0x80 and two (big-endian). A package's
/// files come before its line, whose meta is `p` and "path" its JSON.
fn decode_frcode(data: &[u8]) -> Result<Vec<(String, String, Vec<String>)>> {
    let mut out = Vec::new();
    let (mut i, mut shared, mut last) = (0usize, 0i64, Vec::<u8>::new());
    let mut files: Vec<String> = Vec::new();
    let find = |from: usize, b: u8| data[from..].iter().position(|&c| c == b).map(|p| from + p);
    while i < data.len() {
        let z = find(i, 0).ok_or_else(|| anyhow!("nix-index database cut short"))?;
        let meta = &data[i..z];
        i = z + 1;
        let first = *data.get(i).ok_or_else(|| anyhow!("nix-index database cut short"))?;
        i += 1;
        let change = if first == 0x80 {
            let b = data.get(i..i + 2).ok_or_else(|| anyhow!("nix-index database cut short"))?;
            i += 2;
            i16::from_be_bytes([b[0], b[1]]) as i64
        } else {
            first as i8 as i64
        };
        let nl = find(i, b'\n').ok_or_else(|| anyhow!("nix-index database cut short"))?;
        shared += change;
        ensure!(shared >= 0 && shared as usize <= last.len(), "nix-index database: a bad entry");
        let mut path = last[..shared as usize].to_vec();
        path.extend_from_slice(&data[i..nl]);
        i = nl + 1;
        if meta == b"p" {
            let v: serde_json::Value = serde_json::from_slice(&path).context("nix-index database: a package's line")?;
            let origin = &v["origin"];
            if origin["toplevel"].as_bool() != Some(false) {
                if let (Some(attr), Some(output)) = (origin["attr"].as_str(), origin["output"].as_str()) {
                    out.push((attr.to_string(), output.to_string(), std::mem::take(&mut files)));
                }
            }
            files.clear();
        } else if !meta.ends_with(b"d") {
            if let Some(name) = std::str::from_utf8(&path).ok().and_then(|p| p.strip_prefix("/bin/")).filter(|n| !n.is_empty() && !n.contains('/')) {
                files.push(name.to_string());
            }
        }
        last = path;
    }
    Ok(out)
}

// ---- the table ----

/// A package that has a command: its attribute, and the output with the
/// command when that isn't the one installed (vim's xxd).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub attr: String,
    pub output: Option<String>,
}

impl Candidate {
    fn parse(s: &str) -> Candidate {
        match s.split_once(':') {
            Some((a, o)) => Candidate { attr: a.into(), output: Some(o.into()) },
            None => Candidate { attr: s.into(), output: None },
        }
    }

    fn text(&self) -> String {
        match &self.output {
            Some(o) => format!("{}:{o}", self.attr),
            None => self.attr.clone(),
        }
    }
}

/// What the catalog says of a package, as the table needs it.
pub struct Known<'a> {
    pub main_program: Option<&'a str>,
    /// The outputs `nami install` puts on PATH.
    pub installed_outputs: Vec<String>,
    /// It can be had (from the cache, or built here from the catalog).
    pub available: bool,
    pub description: Option<&'a str>,
}

/// How good a package is for a command: its own name first, then one whose
/// main program it is, then the rest; top-level before a set's; shorter
/// names before longer.
fn rank(cmd: &str, attr: &str, known: &Known) -> (u8, bool, usize, String) {
    let first = if attr == cmd {
        0
    } else if known.main_program == Some(cmd) {
        1
    } else if attr.eq_ignore_ascii_case(cmd) {
        2
    } else {
        3
    };
    (first, attr.contains('.'), attr.len(), attr.to_string())
}

/// The table: `cmd\tcandidate candidate ...` lines, sorted, then an empty
/// line, then `attr\tdescription` lines, sorted.
pub fn make_table<'a>(found: &[(String, String, Vec<String>)], catalog: impl Fn(&str) -> Option<Known<'a>>, extra: impl Iterator<Item = (String, String)>, tag: &str) -> String {
    let mut by_cmd: BTreeMap<String, BTreeMap<String, Option<String>>> = BTreeMap::new();
    let mut about: BTreeMap<String, String> = BTreeMap::new();
    let mut add = |cmd: &str, attr: &str, output: Option<&str>| {
        if cmd.is_empty() || cmd.contains(['\t', '\n', ' ', '/']) || attr.contains(['\t', '\n', ' ', ':']) {
            return;
        }
        let Some(k) = catalog(attr) else { return };
        if !k.available {
            return;
        }
        let output = output.filter(|o| !k.installed_outputs.iter().any(|i| i == o)).map(String::from);
        let entry = by_cmd.entry(cmd.to_string()).or_default();
        // (Its installed outputs over another of its own.)
        match entry.get(attr) {
            Some(None) => {}
            _ => {
                entry.insert(attr.to_string(), output);
            }
        }
        if let Some(d) = k.description {
            about.entry(attr.to_string()).or_insert_with(|| d.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    };
    for (attr, output, files) in found {
        for f in files {
            add(f, attr, Some(output));
        }
    }
    for (cmd, attr) in extra {
        add(&cmd, &attr, None);
    }
    let mut text = format!("{HEADER}\t{tag}\n");
    for (cmd, attrs) in &by_cmd {
        let mut list: Vec<(&String, &Option<String>)> = attrs.iter().collect();
        list.sort_by_cached_key(|(a, _)| rank(cmd, a, &catalog(a).expect("in the catalog")));
        let line: Vec<String> = list.iter().map(|(a, o)| Candidate { attr: (*a).clone(), output: (*o).clone() }.text()).collect();
        text += &format!("{cmd}\t{}\n", line.join(" "));
    }
    text += "\n";
    for (attr, d) in &about {
        let d: String = d.chars().take(90).collect();
        text += &format!("{attr}\t{d}\n");
    }
    text
}

/// The line for `key` in a sorted section (`key\t...`), by bisecting it.
fn find_line<'a>(section: &'a [u8], key: &str) -> Option<&'a str> {
    let (mut lo, mut hi) = (0usize, section.len());
    // Invariant: the line we want starts at a line start in [lo, hi).
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let start = section[..mid].iter().rposition(|&b| b == b'\n').map(|p| p + 1).unwrap_or(0).max(lo);
        let end = section[start..].iter().position(|&b| b == b'\n').map(|p| start + p).unwrap_or(section.len());
        let line = &section[start..end];
        let k = line.split(|&b| b == b'\t').next().unwrap_or_default();
        match k.cmp(key.as_bytes()) {
            std::cmp::Ordering::Equal => return std::str::from_utf8(line).ok(),
            std::cmp::Ordering::Less => lo = end + 1,
            std::cmp::Ordering::Greater => hi = start,
        }
    }
    None
}

/// Which packages have `cmd`, best first, and what each is.
pub fn lookup(table: &[u8], cmd: &str) -> Vec<(Candidate, Option<String>)> {
    let body = match table.iter().position(|&b| b == b'\n') {
        Some(p) if table.starts_with(HEADER.as_bytes()) => &table[p + 1..],
        _ => return Vec::new(),
    };
    let split = body.windows(2).position(|w| w == b"\n\n").map(|p| p + 1).unwrap_or(body.len());
    let (programs, about) = (&body[..split], body.get(split + 1..).unwrap_or_default());
    let Some(line) = find_line(programs, cmd) else { return Vec::new() };
    line.split_once('\t')
        .map(|(_, rest)| rest)
        .unwrap_or_default()
        .split(' ')
        .filter(|s| !s.is_empty())
        .map(Candidate::parse)
        .map(|c| {
            let d = find_line(about, &c.attr).and_then(|l| l.split_once('\t')).map(|(_, d)| d.to_string());
            (c, d)
        })
        .collect()
}

/// Fetch nix-index's database, and make the table from it and the
/// catalog. Returns how many commands it has.
pub fn update_table(store: &Store, index: &Index, to: &Path) -> Result<usize> {
    let url = std::env::var("NAMI_PROGRAMS_SOURCE").unwrap_or_else(|_| SOURCE.to_string());
    let data = if let Some(file) = url.strip_prefix("file://").or_else(|| url.starts_with('/').then_some(url.as_str())) {
        fs::read(file).with_context(|| format!("reading {file}"))?
    } else {
        let mut data = Vec::new();
        store.http().get(&url).call().with_context(|| format!("fetching {url}"))?.into_reader().take(64 << 20).read_to_end(&mut data)?;
        data
    };
    let found = decode_nix_index(&data)?;
    let tag = fs::read_to_string(store.var_dir().join("index-resolved")).unwrap_or_default();
    // (Not Index::get: a name it hasn't, it looks for ones like it.)
    let known = |attr: &str| {
        index.find(attr).map(|p| {
            let installed = p.install_paths();
            Known {
                main_program: p.main_program.as_deref(),
                installed_outputs: p.outputs.iter().filter(|(_, path)| path.as_ref().is_some_and(|path| installed.contains(path))).map(|(o, _)| o.clone()).collect(),
                available: !installed.is_empty() && !(p.uncached && p.closure.is_none()),
                description: p.description.as_deref(),
            }
        })
    };
    let extra = index.iter().filter_map(|(a, p)| p.main_program.as_ref().map(|m| (m.clone(), a.to_string()))).collect::<Vec<_>>();
    let text = make_table(&found, known, extra.into_iter(), tag.trim());
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir)?;
    }
    write_atomic(to, text.as_bytes())?;
    Ok(text.split("\n\n").next().unwrap_or_default().lines().count().saturating_sub(1))
}

// ---- running it ----

/// What was fetched for a package, for the next time: there at once, without
/// reading the catalog, while the catalog's the same.
#[derive(serde::Serialize, serde::Deserialize)]
struct Fetched {
    catalog: String,
    paths: Vec<String>,
}

fn fetched_file(c: &Candidate) -> PathBuf {
    cache_dir().join("run-once").join(format!("{}.json", c.text().replace(['/', ':'], "_")))
}

/// Quoted for a shell, only where it has to be.
pub fn quoted(arg: &str) -> String {
    if !arg.is_empty() && arg.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c)) {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// What someone answered to "Run which?": the candidate's number (0-based),
/// None for no. Enter is the first.
pub fn parse_choice(answer: &str, count: usize) -> Option<usize> {
    let a = answer.trim().to_lowercase();
    if a.is_empty() || a == "y" || a == "yes" {
        return (count > 0).then_some(0);
    }
    let n: usize = a.parse().ok()?;
    (1..=count).contains(&n).then(|| n - 1)
}

/// What the shell's command-not-found says, before it asks.
/// `width`: the terminal's, which a package's line is cut to.
pub fn not_found_text(cmd: &str, args: &[String], found: &[(Candidate, Option<String>)], width: usize) -> String {
    let line = std::iter::once(cmd).chain(args.iter().map(String::as_str)).map(quoted).collect::<Vec<_>>().join(" ");
    match found {
        [] => format!("{cmd}: command not found\n"),
        [(c, _)] => format!(
            "{cmd} isn't installed. It's in the package {}.\n  Run it once:  , {line}\n  Keep it:      nami install {}\n",
            c.attr, c.attr
        ),
        many => {
            let shown = &many[..many.len().min(SHOWN)];
            let name_width = shown.iter().map(|(c, _)| c.attr.len()).max().unwrap_or(0);
            let mut t = format!("{cmd} isn't installed. These packages have it:\n");
            for (i, (c, d)) in shown.iter().enumerate() {
                let l = format!("  {}  {:w$}  {}", i + 1, c.attr, d.as_deref().unwrap_or(""), w = name_width);
                t += &cut(l.trim_end(), width);
                t += "\n";
            }
            if many.len() > SHOWN {
                t += &format!("  (and {} more: `nami search {cmd}`)\n", many.len() - SHOWN);
            }
            t += &format!("  Run it once:  , {line}\n  Keep it:      nami install {}\n", shown[0].0.attr);
            t
        }
    }
}

/// A line no wider than `width`, with … where it's cut.
fn cut(line: &str, width: usize) -> String {
    if line.chars().count() <= width {
        return line.to_string();
    }
    line.chars().take(width.saturating_sub(1)).collect::<String>().trim_end().to_string() + "…"
}

/// How wide the terminal is (stderr's), else 80.
fn columns() -> usize {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(2, libc::TIOCGWINSZ, &mut size) } == 0 && size.ws_col > 20 {
        size.ws_col as usize
    } else {
        80
    }
}

fn ask(question: &str) -> String {
    eprint!("{question} ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    line
}

fn interactive() -> bool {
    !crate::errors::noninteractive() && std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// The candidates for `cmd`: from the table (fetched first if there's none
/// yet), else what the catalog itself names so.
fn candidates(root: &Path, cmd: &str, quiet: bool) -> Result<Vec<(Candidate, Option<String>)>> {
    let var = crate::migrate::var_dir(root);
    match table(&var) {
        Some((path, made)) => {
            if made.elapsed().unwrap_or_default() > FRESH {
                refresh_later();
            }
            Ok(lookup(&fs::read(path)?, cmd))
        }
        None => {
            if !quiet {
                eprintln!("{}", crate::style::err().dim("Finding out which packages have which commands: once, about 2 MB."));
            }
            let store = Store::open_direct_readonly(root)?;
            let index = Index::load(&store)?;
            match update_table(&store, &index, &own_table()) {
                Ok(_) => Ok(lookup(&fs::read(own_table())?, cmd)),
                // Offline: the catalog's own word.
                Err(_) => Ok(index
                    .iter()
                    .filter(|(a, p)| *a == cmd || p.main_program.as_deref() == Some(cmd))
                    .map(|(a, p)| (Candidate { attr: a.to_string(), output: None }, p.description.clone()))
                    .take(SHOWN)
                    .collect()),
            }
        }
    }
}

/// Look again, in the background, so this lookup isn't kept waiting.
fn refresh_later() {
    let Ok(me) = std::env::current_exe() else { return };
    let marker = cache_dir().join("programs.refreshing");
    // (Once a day at most, if it can't.)
    if fs::metadata(&marker).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|e| e < Duration::from_secs(86400)) {
        return;
    }
    let _ = fs::create_dir_all(cache_dir());
    let _ = fs::write(&marker, b"");
    use std::os::unix::process::CommandExt;
    let mut c = std::process::Command::new(me);
    c.args(["try", "--update-table"]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let _ = c.spawn();
}

/// `nami try --update-table`: root's for everyone, else one's own.
pub fn update_cli(root: &Path) -> Result<()> {
    let store = Store::open_direct_readonly(root)?;
    let index = Index::load(&store)?;
    let to = if crate::profile::is_root() { system_table(&store.var_dir()) } else { own_table() };
    let n = update_table(&store, &index, &to)?;
    println!("{}", crate::style::out().line(&format!("✓ {n} commands · in {}", to.display())));
    Ok(())
}

/// The shell's command-not-found: say which package has it, ask, run it.
/// Exits 127 when it isn't run.
pub fn not_found(root: &Path, cmd: &str, args: &[String]) -> Result<i32> {
    let asks = interactive();
    let found = candidates(root, cmd, !asks).unwrap_or_default();
    eprint!("{}", not_found_text(cmd, args, &found, columns()));
    if found.is_empty() || !asks || crate::profile::is_child() {
        return Ok(127);
    }
    let shown = found.len().min(SHOWN);
    let question = if shown == 1 { "Run it now? [Y/n]".to_string() } else { format!("Run which? [1-{shown}, Enter for 1, n for none]") };
    let Some(pick) = parse_choice(&ask(&question), shown) else { return Ok(127) };
    run(root, &found[pick].0, cmd, args)
}

/// What a package runs by itself: its main program, else its name.
pub fn main_program(root: &Path, package: &str) -> Result<String> {
    let store = Store::open_direct_readonly(root)?;
    let index = Index::load(&store)?;
    let c = Candidate::parse(package);
    let p = index.get(&c.attr)?;
    Ok(p.main_program.clone().unwrap_or_else(|| c.attr.rsplit('.').next().unwrap_or(&c.attr).to_string()))
}

/// `, cmd args` / `nami try [--package p] cmd args`.
pub fn run_once(root: &Path, package: Option<&str>, cmd: &str, args: &[String]) -> Result<i32> {
    ensure!(!crate::profile::is_child(), "you're a child on this computer: an admin adds your apps");
    let candidate = match package {
        Some(p) => Candidate::parse(p),
        None => {
            let found = candidates(root, cmd, false)?;
            match found.len() {
                0 => bail!("no package in the catalog has a command named {cmd} (`nami search {cmd}` looks wider)"),
                1 => found[0].0.clone(),
                // (One run once before: that one again, not asked again.)
                _ if found.iter().any(|(c, _)| fetched_file(c).is_file()) => found.iter().find(|(c, _)| fetched_file(c).is_file()).unwrap().0.clone(),
                n if interactive() => {
                    let shown = n.min(SHOWN);
                    let mut t = format!("{cmd} is in several packages:\n");
                    for (i, (c, d)) in found.iter().take(shown).enumerate() {
                        t += &cut(&format!("  {}  {}  {}", i + 1, c.attr, d.as_deref().unwrap_or("")), columns());
                        t += "\n";
                    }
                    eprint!("{t}");
                    let Some(pick) = parse_choice(&ask(&format!("Run which? [1-{shown}, Enter for 1, n for none]")), shown) else { return Ok(1) };
                    found[pick].0.clone()
                }
                _ => found[0].0.clone(),
            }
        }
    };
    run(root, &candidate, cmd, args)
}

/// Fetch the package if it isn't here, then run `cmd` from it, held from
/// `nami gc` while it runs.
fn run(root: &Path, c: &Candidate, cmd: &str, args: &[String]) -> Result<i32> {
    let catalog = fs::read_to_string(crate::migrate::var_dir(root).join("index-resolved")).unwrap_or_default();
    let had: Option<Fetched> = fs::read(fetched_file(c)).ok().and_then(|d| serde_json::from_slice(&d).ok());
    let (paths, held) = {
        let store = Store::open(root)?;
        let paths = match had.filter(|f| f.catalog == catalog && !f.paths.is_empty() && f.paths.iter().all(|p| store.is_valid(base_name(p)))) {
            Some(f) => f.paths,
            None => {
                let index = Index::load(&store)?;
                let pkg = index.get(&c.attr).map_err(|_| anyhow!("{} isn't in this computer's catalog now", c.attr))?;
                let paths = match &c.output {
                    Some(o) => pkg.outputs.get(o).cloned().flatten().into_iter().collect(),
                    None => pkg.install_paths(),
                };
                ensure!(!paths.is_empty(), "{} has nothing to run", c.attr);
                let profiles = crate::profile::Profiles::personal(&store)?;
                let mut builder = crate::build::Builder::new(&store, &index);
                builder.allow_unfree = crate::config::unfree_allowed(profiles.managed_config()?.as_deref());
                if c.output.is_none() {
                    builder.ensure_catalog(&c.attr)?;
                }
                if !paths.iter().all(|p| store.is_valid(base_name(p))) {
                    eprintln!("Getting {} (it won't be installed)…", pkg.name);
                }
                store.realise(&paths)?;
                let _ = fs::create_dir_all(fetched_file(c).parent().unwrap());
                let _ = write_atomic(&fetched_file(c), &serde_json::to_vec(&Fetched { catalog: catalog.clone(), paths: paths.clone() })?);
                paths
            }
        };
        let held = crate::profile::Profiles::personal(&store)?.hold(&paths)?;
        // (The store, and root's lock on it, let go before it runs.)
        (paths, held)
    };
    let program = paths.iter().map(|p| Path::new(p).join("bin").join(cmd)).find(|p| p.exists());
    let Some(program) = program else {
        let has: BTreeSet<String> = paths.iter().flat_map(|p| fs::read_dir(Path::new(p).join("bin")).into_iter().flatten().flatten()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        bail!("{} has no command named {cmd}{}", c.attr, if has.is_empty() { String::new() } else { format!("; it has: {}", has.into_iter().take(12).collect::<Vec<_>>().join(", ")) });
    };
    let join = |sub: &str, var: &str, fallback: &str| {
        let mut parts: Vec<String> = paths.iter().map(|p| format!("{p}/{sub}")).filter(|p| Path::new(p).exists()).collect();
        parts.push(std::env::var(var).unwrap_or_else(|_| fallback.into()));
        parts.join(":")
    };
    let mut child = std::process::Command::new(&program);
    child.arg0(cmd).args(args).env("PATH", join("bin", "PATH", "/usr/bin:/bin")).env("XDG_DATA_DIRS", join("share", "XDG_DATA_DIRS", "/usr/local/share:/usr/share"));
    // Ctrl-C and Ctrl-\ are the program's: nami waits on.
    let old = unsafe { (libc::signal(libc::SIGINT, libc::SIG_IGN), libc::signal(libc::SIGQUIT, libc::SIG_IGN)) };
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(&mut child, move || {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
            Ok(())
        });
    }
    let status = child.status().with_context(|| format!("starting {}", program.display()));
    unsafe {
        libc::signal(libc::SIGINT, old.0);
        libc::signal(libc::SIGQUIT, old.1);
    }
    drop(held);
    let status = status?;
    use std::os::unix::process::ExitStatusExt;
    Ok(status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// nix-index's encoding of entries, as it writes them.
    fn frcode(lines: &[(&str, &str)]) -> Vec<u8> {
        let (mut out, mut last, mut shared) = (Vec::new(), String::new(), 0i64);
        for (meta, path) in lines {
            let common = last.bytes().zip(path.bytes()).take_while(|(a, b)| a == b).count() as i64;
            out.extend_from_slice(meta.as_bytes());
            out.push(0);
            let change = common - shared;
            if change.abs() > 127 {
                out.push(0x80);
                out.extend_from_slice(&(change as i16).to_be_bytes());
            } else {
                out.push(change as i8 as u8);
            }
            out.extend_from_slice(&path.as_bytes()[common as usize..]);
            out.push(b'\n');
            (last, shared) = (path.to_string(), common);
        }
        out
    }

    fn pkg(attr: &str, output: &str, toplevel: bool) -> String {
        format!(r#"{{"store_dir":"/nix/store","hash":"x","name":"{attr}-1","origin":{{"attr":"{attr}","output":"{output}","toplevel":{toplevel},"system":"x86_64-linux"}}}}"#)
    }

    #[test]
    fn reads_nix_index_entries() {
        let long = format!("/bin/{}", "a".repeat(300));
        let p1 = pkg("cowsay", "out", true);
        let p2 = pkg("vim", "xxd", true);
        let p3 = pkg("mangohud", "out", false);
        let data = frcode(&[
            ("1200x", "/bin/cowsay"),
            ("1200x", "/bin/cowthink"),
            ("d", "/bin/sub"),
            ("10x", "/bin/sub/inner"),
            ("p", &p1),
            ("900x", "/bin/xxd"),
            ("12x", &long),
            ("p", &p2),
            ("1x", "/bin/jq"),
            ("p", &p3),
        ]);
        let found = decode_frcode(&data).unwrap();
        assert_eq!(found.len(), 2, "not a top-level one: {found:?}");
        assert_eq!(found[0], ("cowsay".into(), "out".into(), vec!["cowsay".to_string(), "cowthink".to_string()]));
        assert_eq!(found[1].0, "vim");
        assert_eq!(found[1].2, vec!["xxd".to_string(), "a".repeat(300)]);
        // The file: magic, version, zstd.
        let mut file = b"NIXI".to_vec();
        file.extend_from_slice(&1u64.to_le_bytes());
        file.extend(zstd::stream::encode_all(&data[..], 3).unwrap());
        assert_eq!(decode_nix_index(&file).unwrap(), found);
        assert!(decode_nix_index(b"nope").is_err());
    }

    fn sample() -> String {
        let found = vec![
            ("neo-cowsay".to_string(), "out".to_string(), vec!["cowsay".to_string(), "cowthink".to_string()]),
            ("cowsay".to_string(), "out".to_string(), vec!["cowsay".to_string()]),
            ("sapling".to_string(), "out".to_string(), vec!["sl".to_string()]),
            ("sl".to_string(), "out".to_string(), vec!["sl".to_string()]),
            ("vim".to_string(), "xxd".to_string(), vec!["xxd".to_string()]),
            ("vim".to_string(), "out".to_string(), vec!["vim".to_string(), "xxd".to_string()]),
            ("tinyxxd".to_string(), "out".to_string(), vec!["xxd".to_string()]),
            ("gone".to_string(), "out".to_string(), vec!["gone".to_string()]),
            ("imagemagick_light".to_string(), "out".to_string(), vec!["convert".to_string()]),
            ("imagemagick".to_string(), "out".to_string(), vec!["convert".to_string(), "magick".to_string()]),
        ];
        let catalog = |attr: &str| -> Option<Known<'static>> {
            let (main, desc) = match attr {
                "cowsay" => (Some("cowsay"), Some("A program which generates   ASCII pictures")),
                "neo-cowsay" => (Some("cowsay"), Some("Cowsay reborn")),
                "sl" => (Some("sl"), Some("Steam Locomotive")),
                "sapling" => (Some("sl"), None),
                "vim" | "tinyxxd" | "imagemagick_light" => (None, None),
                "imagemagick" => (Some("magick"), Some("Edit images")),
                "code" => (Some("code"), Some("VS Code")),
                _ => return None,
            };
            Some(Known { main_program: main, installed_outputs: vec!["out".into()], available: true, description: desc })
        };
        make_table(&found, catalog, [("code".to_string(), "code".to_string())].into_iter(), "test")
    }

    #[test]
    fn the_table_ranks_and_finds() {
        let t = sample();
        let t = t.as_bytes();
        let attrs = |cmd: &str| lookup(t, cmd).into_iter().map(|(c, _)| c.text()).collect::<Vec<_>>();
        // Its own name first; then whose main program it is; then by length.
        assert_eq!(attrs("cowsay"), ["cowsay", "neo-cowsay"]);
        assert_eq!(attrs("cowthink"), ["neo-cowsay"]);
        assert_eq!(attrs("sl"), ["sl", "sapling"]);
        assert_eq!(attrs("convert"), ["imagemagick", "imagemagick_light"]);
        // Another output than the installed one is said; installed wins for the same package.
        assert_eq!(attrs("xxd"), ["vim", "tinyxxd"]);
        // Not in the catalog: left out. Only the catalog's mainProgram: in.
        assert!(attrs("gone").is_empty());
        assert_eq!(attrs("code"), ["code"]);
        assert!(attrs("nothing").is_empty());
        assert!(attrs("").is_empty());
        assert!(attrs("zzzz").is_empty());
        assert!(attrs("a").is_empty());
        // Descriptions, on one line.
        assert_eq!(lookup(t, "cowsay")[0].1.as_deref(), Some("A program which generates ASCII pictures"));
        assert_eq!(lookup(t, "convert")[1].1, None);
        // Something else than a table: nothing.
        assert!(lookup(b"cowsay\tcowsay\n", "cowsay").is_empty());
    }

    #[test]
    fn bisecting_finds_every_line() {
        let mut text = format!("{HEADER}\tt\n");
        let keys: Vec<String> = (0..500).map(|i| format!("cmd{i:04}")).collect();
        for k in &keys {
            text += &format!("{k}\tpkg-{k}\n");
        }
        text += "\n";
        for k in &keys {
            assert_eq!(lookup(text.as_bytes(), k)[0].0.attr, format!("pkg-{k}"));
        }
        assert!(lookup(text.as_bytes(), "cmd").is_empty());
        assert!(lookup(text.as_bytes(), "cmd9999").is_empty());
    }

    #[test]
    fn other_outputs_are_named() {
        let found = vec![("vim".to_string(), "xxd".to_string(), vec!["xxd".to_string()])];
        let catalog = |_: &str| Some(Known { main_program: None, installed_outputs: vec!["out".into()], available: true, description: None });
        let t = make_table(&found, catalog, std::iter::empty(), "t");
        assert_eq!(lookup(t.as_bytes(), "xxd")[0].0, Candidate { attr: "vim".into(), output: Some("xxd".into()) });
        // Can't be had: left out.
        let none = |_: &str| Some(Known { main_program: None, installed_outputs: vec![], available: false, description: None });
        assert!(lookup(make_table(&found, none, std::iter::empty(), "t").as_bytes(), "xxd").is_empty());
    }

    #[test]
    fn what_the_shell_says() {
        let one = vec![(Candidate { attr: "cowsay".into(), output: None }, Some("Cows".into()))];
        let t = not_found_text("cowsay", &["hi".into(), "there you".into()], &one, 80);
        assert_eq!(t, "cowsay isn't installed. It's in the package cowsay.\n  Run it once:  , cowsay hi 'there you'\n  Keep it:      nami install cowsay\n");
        assert_eq!(not_found_text("nope", &[], &[], 80), "nope: command not found\n");
        let many: Vec<_> = ["sl", "sapling", "femtolisp", "a", "b"].iter().map(|a| (Candidate { attr: a.to_string(), output: None }, None)).collect();
        let t = not_found_text("sl", &[], &many, 80);
        assert!(t.starts_with("sl isn't installed. These packages have it:\n  1  sl\n  2  sapling\n"), "{t}");
        assert!(t.contains("(and 1 more: `nami search sl`)"), "{t}");
        assert!(t.contains("nami install sl\n"), "{t}");
        // Cut to the terminal's width.
        let long = vec![(Candidate { attr: "sl".into(), output: None }, Some("Steam Locomotive runs across your terminal".into())), (Candidate { attr: "x".into(), output: None }, None)];
        let t = not_found_text("sl", &[], &long, 30);
        assert!(t.contains("\n  1  sl  Steam Locomotive run…\n"), "{t}");
    }

    #[test]
    fn answers() {
        assert_eq!(parse_choice("\n", 1), Some(0));
        assert_eq!(parse_choice("Y", 1), Some(0));
        assert_eq!(parse_choice("n", 1), None);
        assert_eq!(parse_choice("no", 3), None);
        assert_eq!(parse_choice("2", 3), Some(1));
        assert_eq!(parse_choice(" 3 ", 3), Some(2));
        assert_eq!(parse_choice("4", 3), None);
        assert_eq!(parse_choice("0", 3), None);
        assert_eq!(parse_choice("", 0), None);
    }

    #[test]
    fn quoting() {
        assert_eq!(quoted("hi"), "hi");
        assert_eq!(quoted("--name=x"), "--name=x");
        assert_eq!(quoted("two words"), "'two words'");
        assert_eq!(quoted("it's"), "'it'\\''s'");
        assert_eq!(quoted(""), "''");
    }
}
