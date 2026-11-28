//! A project's services: what it needs running while you work on it
//! (databases, caches, a mail catcher, its own processes), like
//! docker-compose or devenv, without containers, root or systemd.
//!
//! ```yaml
//! # project.nami
//! services:
//!   - ./services/postgres.nami                 # a service file, the newest
//!   - ./services/redis.nami: { version: 7 }    # at a version
//!   - ./services/mailpit.nami: { port: 2525 }  # any of its keys changed
//!   - worker:                                  # the project's own
//!       run: npm run worker
//!       env: { QUEUE: default }
//! ```
//!
//! A service file is a nami file of a service's keys, named after it
//! (postgres.nami is postgres); nami's examples/services has some to copy
//! into a project. A project changes any of their keys by setting it. Their
//! packages go on the project's PATH and into its nami.lock like its other
//! tools, and what they `export` (DATABASE_URL, ...) is in the project's
//! environment, running or not.
//!
//! nami runs them itself: `nami up` in the foreground (Ctrl-C stops them),
//! `nami up -d` in the background. Each has a supervisor, a nami process of
//! its own that starts it, restarts it when it ends (after 1s, 2s, 4s, ...
//! up to 30s) and stops it. A service runs in a process group of its own,
//! so stopping it stops its children too: its stop signal, then SIGKILL if
//! it's still there after a while. Everything they keep is in the project's
//! .nami folder: `<name>/` (data, sockets), `<name>.log`, `<name>.pid`;
//! sockets go to a short folder elsewhere when that path is too long for one.
//! As root, those that won't run as root (`as-root: false`, postgres) run as nobody.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};

/// A service's keys, in a project's file or a service file.
pub const KEYS: &[&str] = &["version", "about", "packages", "port", "ports", "init", "run", "args", "ready", "ready-timeout", "restart", "as-root", "stop-signal", "dir", "env", "exports"];

/// The placeholders in a service's commands and variables.
const PLACEHOLDERS: &str = "{state} (its folder in .nami), {sockets} (a folder for its unix sockets), {port}, {port.<name>}, {project} (the folder's name) and {project.dir}";

/// How long a service may take to be ready, unless it says (`ready-timeout:`):
/// a first start sets things up (initdb, mysql's install), which can take a
/// while on a slow computer.
const READY_TIMEOUT: Duration = Duration::from_secs(180);
/// How long a service may take to stop before it's killed.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// Restarts wait 1s, 2s, 4s, ... up to this; one that ran this long starts at 1s again.
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// A socket's path is at most 108 bytes: a longer folder than this gets a short one for its sockets.
const SOCKET_DIR_MAX: usize = 80;

/// One service as written, before placeholders are filled in.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    #[serde(default)]
    about: Option<String>,
    #[serde(default)]
    packages: Vec<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    ports: BTreeMap<String, u16>,
    #[serde(default)]
    init: Option<String>,
    #[serde(default)]
    run: Option<String>,
    #[serde(default)]
    args: Option<String>,
    #[serde(default)]
    ready: Option<String>,
    #[serde(default, rename = "ready-timeout")]
    ready_timeout: Option<Value>,
    #[serde(default)]
    restart: Option<Value>,
    #[serde(default, rename = "as-root")]
    as_root: Option<Value>,
    #[serde(default, rename = "stop-signal")]
    stop_signal: Option<String>,
    #[serde(default)]
    dir: Option<String>,
    #[serde(default, deserialize_with = "crate::yaml_text::text_map")]
    env: BTreeMap<String, String>,
    #[serde(default)]
    exports: BTreeMap<String, String>,
}

/// One of a project's services, ready to run.
#[derive(Debug, Clone, PartialEq)]
pub struct Service {
    pub name: String,
    /// Its entry in `services:` as written (./services/postgres.nami, worker).
    pub entry: String,
    pub about: Option<String>,
    /// Catalog names (`postgresql@16`), for the project's PATH and lock.
    pub packages: Vec<String>,
    /// Its ports: "" is `port:`, the others `ports:`.
    pub ports: Vec<(String, u16)>,
    /// Run once, before its first start.
    pub init: Option<String>,
    pub run: String,
    /// Says whether it's ready (exits 0); without it, its ports answering is.
    pub ready: Option<String>,
    /// How long it may take to be ready.
    pub ready_timeout: Duration,
    /// When it's started again after it ends.
    pub restart: Restart,
    /// It may run as root (`as-root: false`: when nami up runs as root, it runs as nobody).
    pub as_root: bool,
    pub stop_signal: i32,
    /// Where it runs: the project's folder, unless `dir:` says otherwise.
    pub dir: PathBuf,
    /// Its own folder for what it keeps: .nami/<name>.
    pub state: PathBuf,
    /// Its folder for unix sockets: `state`, or a short one when that's too long.
    pub sockets: PathBuf,
    /// Its own environment variables.
    pub env: Vec<(String, String)>,
    /// What it gives the project's environment.
    pub exports: Vec<(String, String)>,
}

/// When a service is started again after it ends (`restart:`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Restart {
    /// Whenever it ends (the default).
    Always,
    /// When it fails (exits with an error, or is killed).
    OnFailure,
    /// Never: `restart: no`.
    No,
}

impl Service {
    /// Its log: .nami/<name>.log, next to its folder.
    pub fn log(&self) -> PathBuf {
        self.state.with_extension("log")
    }
}

/// A project's `services:`, each filled in for the project in `project_dir`.
/// `false` leaves one out.
pub fn resolve(services: &Mapping, project_dir: &Path) -> Result<Vec<Service>> {
    let mut out: Vec<Service> = Vec::new();
    for (key, value) in services {
        let entry = key.as_str().ok_or_else(|| anyhow!("services: keys are names"))?;
        if let Some(other) = services.keys().filter_map(Value::as_str).take_while(|o| *o != entry).find(|o| name_of(o) == name_of(entry)) {
            bail!("services: {other} and {entry} are both {}: a service's name is its file's; copy one to another name", name_of(entry));
        }
        if let Some(s) = parse(entry, value, project_dir)? {
            out.push(s);
        }
    }
    let mut seen: BTreeMap<u16, &str> = BTreeMap::new();
    for s in &out {
        for (_, port) in &s.ports {
            if let Some(other) = seen.insert(*port, &s.name) {
                bail!("services: {other} and {} both use port {port}; give one of them another `port:`", s.name);
            }
        }
    }
    Ok(out)
}

/// Whether a `services:` entry is a service file (./services/postgres.nami).
pub fn is_file(entry: &str) -> bool {
    crate::oldnames::ends_with_extension(entry)
}

/// A service's name: its file's (postgres for ./services/postgres.nami), or the entry.
pub fn name_of(entry: &str) -> &str {
    if is_file(entry) { Path::new(entry).file_stem().and_then(|s| s.to_str()).unwrap_or(entry) } else { entry }
}

/// A project's `services:` entry: a service file with the project's changes
/// on top, or its own. None: `false`.
pub fn parse(entry: &str, value: &Value, project_dir: &Path) -> Result<Option<Service>> {
    let name = name_of(entry);
    let at = format!("services.{entry}");
    ensure!(valid_name(name), "{at}: a service's name is letters, digits, - and _{}", if is_file(entry) { " (its file's name)" } else { "" });
    let own = match value {
        Value::Bool(false) => return Ok(None),
        Value::Bool(true) | Value::Null => Mapping::new(),
        Value::Mapping(m) => m.clone(),
        _ => bail!("{at}: its settings, or nothing ({{ run: ... }} for your own)"),
    };
    for k in own.keys() {
        let k = k.as_str().ok_or_else(|| anyhow!("{at}: keys are names"))?;
        if !KEYS.contains(&k) {
            let hint = crate::config::closest(k, KEYS).map(|c| format!("did you mean `{c}`?")).unwrap_or_else(|| format!("a service has {}", KEYS.join(", ")));
            bail!("{at}: no key `{k}` ({hint})");
        }
    }
    let text = |k: &str| -> Result<Option<String>> {
        match own.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(Value::Number(n)) => Ok(Some(n.to_string())),
            Some(_) => bail!("{at}.{k} is text"),
        }
    };
    let version = text("version")?;
    let mut own = own;
    own.remove("version");
    let value = if is_file(entry) {
        let file = project_dir.join(entry);
        let text = fs::read_to_string(&file).map_err(|e| anyhow!("{at}: {e}"))?;
        let mut base: Value = crate::yaml_text::from_str(&text).with_context(|| file.display().to_string())?;
        crate::config::modern(&mut base, &file.display().to_string())?;
        // `version: 16` is the version of its (first) package.
        if let Some(v) = &version {
            let first = base.get_mut("packages").and_then(Value::as_sequence_mut).and_then(|s| s.first_mut());
            let first = first.ok_or_else(|| anyhow!("{at}.version: {name} has no package to take a version of"))?;
            let pkg = first.as_str().map(|p| crate::index::split_version(p).0.to_string()).unwrap_or_default();
            *first = Value::String(format!("{pkg}@{v}"));
        }
        crate::config::merge(base, Value::Mapping(own)).with_context(|| at.to_string())?
    } else {
        if text_of(&own, "run").is_none() {
            bail!("{at}: your own service needs `run:`, or list a service file by its path (./services/{name}.nami; nami's examples/services has postgres, redis, mysql and mailpit to copy)");
        }
        ensure!(version.is_none(), "{at}.version: only a service file has a version; for your own, name the version in `packages:`");
        Value::Mapping(own)
    };
    let mut value = value;
    crate::config::versioned_list(value.get_mut("packages"), &at)?;
    // Numbers are fine as values (PORT: 8080), as the text written.
    for k in ["env", "exports"] {
        if let Some(Value::Mapping(m)) = value.get_mut(k) {
            for v in m.values_mut() {
                if let Value::Number(n) = v {
                    *v = Value::String(n.to_string());
                } else if let Value::Bool(b) = v {
                    *v = Value::String(b.to_string());
                }
            }
        }
    }
    let spec: Spec = serde_yaml::from_value(value).map_err(|e| anyhow!("{at}: {e}"))?;
    fill(name, spec, project_dir).map(|s| Some(Service { entry: entry.to_string(), ..s })).map_err(|e| anyhow!("{at}: {e:#}"))
}

fn text_of<'a>(m: &'a Mapping, k: &str) -> Option<&'a str> {
    m.get(k).and_then(Value::as_str)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.starts_with(|c: char| c.is_ascii_alphanumeric()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn valid_var(k: &str) -> bool {
    k.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The project's name for `{project}`: its folder's, as a database name can have it.
pub fn project_name(dir: &Path) -> String {
    let name: String = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    if name.is_empty() { "project".into() } else { name }
}

/// A spec with its placeholders filled in.
fn fill(name: &str, spec: Spec, project_dir: &Path) -> Result<Service> {
    let run = spec.run.ok_or_else(|| anyhow!("no `run:` (the command that runs it)"))?;
    let state = crate::migrate::project_state(project_dir).join(name);
    let sockets = sockets_dir(&state, project_dir, name);
    let mut ports: Vec<(String, u16)> = spec.port.map(|p| (String::new(), p)).into_iter().collect();
    ports.extend(spec.ports.iter().map(|(k, p)| (k.clone(), *p)));
    for (k, p) in &ports {
        ensure!(*p != 0, "{}: 0 isn't a port", if k.is_empty() { "port".to_string() } else { format!("ports.{k}") });
    }
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    values.insert("state".into(), state.display().to_string());
    values.insert("sockets".into(), sockets.display().to_string());
    values.insert("project".into(), project_name(project_dir));
    values.insert("project.dir".into(), project_dir.display().to_string());
    for (k, p) in &ports {
        values.insert(if k.is_empty() { "port".into() } else { format!("port.{k}") }, p.to_string());
    }
    let shell = |what: &str, t: &str| placeholders(t, &values, true).with_context(|| what.to_string());
    let plain = |what: &str, t: &str| placeholders(t, &values, false).with_context(|| what.to_string());
    let mut run = shell("run", &run)?;
    if let Some(args) = &spec.args {
        run = format!("{} {}", run.trim_end(), shell("args", args)?);
    }
    let vars = |what: &str, m: &BTreeMap<String, String>| -> Result<Vec<(String, String)>> {
        m.iter()
            .map(|(k, v)| {
                ensure!(valid_var(k), "{what}: {k:?} isn't a variable name (letters, digits and _)");
                Ok((k.clone(), plain(&format!("{what}.{k}"), v)?))
            })
            .collect()
    };
    let dir = match &spec.dir {
        Some(d) => project_dir.join(plain("dir", d)?),
        None => project_dir.to_path_buf(),
    };
    Ok(Service {
        name: name.to_string(),
        entry: name.to_string(),
        about: spec.about,
        packages: spec.packages,
        init: spec.init.as_deref().map(|t| shell("init", t)).transpose()?,
        ready: spec.ready.as_deref().map(|t| shell("ready", t)).transpose()?,
        ready_timeout: spec.ready_timeout.as_ref().map(seconds).transpose().context("ready-timeout")?.unwrap_or(READY_TIMEOUT),
        restart: spec.restart.as_ref().map(restart).transpose()?.unwrap_or(Restart::Always),
        as_root: match &spec.as_root {
            None | Some(Value::Bool(true)) => true,
            Some(Value::Bool(false)) => false,
            // (yes and no are text in YAML: read as before, until nami 1.0.)
            Some(Value::String(s)) if s == "yes" || s == "no" => {
                crate::config::deprecated(name, &format!("as-root: {s} is now as-root: {}", s == "yes"));
                s == "yes"
            }
            Some(_) => bail!("as-root: true (the default) or false (it won't run as root: as root, nami runs it as nobody)"),
        },
        stop_signal: signal(spec.stop_signal.as_deref().unwrap_or("TERM"))?,
        // Secrets stay sealed here: `up` opens them, into memory.
        env: vars("env", &spec.env)?,
        exports: vars("exports", &spec.exports)?,
        ports,
        run,
        dir,
        state,
        sockets,
    })
}

/// Where a service's unix sockets go: its own folder, unless that's too long
/// for a socket's path (108 bytes, the socket's name included). Then a short
/// one, the same each time: $XDG_RUNTIME_DIR/nami/<hash>, or
/// /tmp/nami-<uid>/<hash> (without one, or as root).
fn sockets_dir(state: &Path, project_dir: &Path, name: &str) -> PathBuf {
    if state.as_os_str().len() <= SOCKET_DIR_MAX {
        return state.to_path_buf();
    }
    use sha2::{Digest, Sha256};
    let uid = unsafe { libc::geteuid() };
    let base = match std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        Some(d) if uid != 0 && d.is_absolute() && d.as_os_str().len() <= 40 && d.is_dir() => d.join("nami"),
        _ => PathBuf::from(format!("/tmp/nami-{uid}")),
    };
    let hash = Sha256::digest(format!("{}\0{name}", project_dir.display()));
    base.join(hash[..6].iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// `ready-timeout: 300`, `300s` or `5m` (any duration: see duration.rs).
fn seconds(v: &Value) -> Result<Duration> {
    let text = match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.trim().to_string(),
        _ => bail!("a duration, like {}", crate::duration::EXAMPLES),
    };
    let n = crate::duration::seconds(&text).map_err(|e| anyhow!(e))?;
    ensure!(n > 0, "0 isn't enough");
    Ok(Duration::from_secs(n))
}

/// `restart: always` (or true), `on-failure`, or `no` (or false).
fn restart(v: &Value) -> Result<Restart> {
    Ok(match v {
        Value::Bool(true) => Restart::Always,
        Value::Bool(false) => Restart::No,
        Value::String(s) if s == "always" => Restart::Always,
        Value::String(s) if s == "yes" => {
            crate::config::deprecated("services", "restart: yes is now restart: always");
            Restart::Always
        }
        Value::String(s) if s == "on-failure" => Restart::OnFailure,
        Value::String(s) if s == "no" || s == "never" => Restart::No,
        _ => bail!("restart: always (the default), on-failure or no"),
    })
}

/// `text` with `{state}`, `{port}`, ... filled in; in a command (`shell`),
/// a value that needs quotes gets them. `${HOME}` and `{ print $1 }` are
/// left alone: a placeholder is `{` and lowercase letters, digits and dots.
pub fn placeholders(text: &str, values: &BTreeMap<String, String>, shell: bool) -> Result<String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        out += &rest[..i];
        let after = &rest[i + 1..];
        let end = after.find('}');
        let key = end.map(|e| &after[..e]).filter(|k| !k.is_empty() && k.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-'));
        match (key, out.ends_with('$')) {
            (Some(k), false) => {
                let v = values.get(k).ok_or_else(|| match k.strip_prefix("port.") {
                    _ if k == "port" => anyhow!("{{port}} is used, but there's no `port:`"),
                    Some(p) => anyhow!("{{port.{p}}} is used, but there's no `ports: {{ {p}: ... }}`"),
                    None => anyhow!("{{{k}}} isn't a placeholder; there are {PLACEHOLDERS}"),
                })?;
                out += &if shell { quote(v) } else { v.clone() };
                rest = &after[k.len() + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out += rest;
    Ok(out)
}

/// `s` for sh: as it is when that's safe, else in single quotes.
fn quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-:@%+=,".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn signal(name: &str) -> Result<i32> {
    Ok(match name.trim_start_matches("SIG") {
        "TERM" => libc::SIGTERM,
        "INT" => libc::SIGINT,
        "QUIT" => libc::SIGQUIT,
        "HUP" => libc::SIGHUP,
        "KILL" => libc::SIGKILL,
        "USR1" => libc::SIGUSR1,
        "USR2" => libc::SIGUSR2,
        other => bail!("stop-signal: {other:?} isn't a signal nami knows (TERM, INT, QUIT, HUP, KILL, USR1, USR2)"),
    })
}

/// What `port_in_use` says when a service's port is taken.
pub fn port_in_use(entry: &str, key: &str, port: u16) -> String {
    let name = name_of(entry);
    let next = port.checked_add(1).unwrap_or(port - 1);
    let setting = if key.is_empty() { format!("port: {next}") } else { format!("ports: {{ {key}: {next} }}") };
    format!("{name} can't start: port {port} is already in use by another program (another project's {name}, or one of your own?). Give it another port in project.nami:\n\n  services:\n    - {entry}: {{ {setting} }}\n")
}

/// The ports these services need that something else has: what to say about each.
fn busy_ports(services: &[&Service]) -> Vec<String> {
    let mut out = Vec::new();
    for s in services {
        for (key, port) in &s.ports {
            match std::net::TcpListener::bind(("127.0.0.1", *port)) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => out.push(port_in_use(&s.entry, key, *port)),
                Err(e) => out.push(format!("{} can't start: port {port}: {e}", s.name)),
            }
        }
    }
    out
}

// ---- Running them ----

/// The project's .nami folder, made with a .gitignore that keeps it out of git.
fn nami_dir(project_dir: &Path) -> Result<PathBuf> {
    let dir = crate::migrate::project_state(project_dir);
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        fs::write(&ignore, "# What the project's services keep (nami up): not for git.\n*\n")?;
    }
    Ok(dir)
}

fn pid_file(project_dir: &Path, name: &str) -> PathBuf {
    crate::migrate::project_state(project_dir).join(format!("{name}.pid"))
}

pub fn log_file(project_dir: &Path, name: &str) -> PathBuf {
    crate::migrate::project_state(project_dir).join(format!("{name}.log"))
}

/// A process's state, process group and start time, from /proc/<pid>/stat.
fn proc_stat(pid: i32) -> Option<(char, i32, u64)> {
    let s = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let f: Vec<&str> = s.rsplit_once(')')?.1.split_whitespace().collect();
    Some((f.first()?.chars().next()?, f.get(2)?.parse().ok()?, f.get(19)?.parse().ok()?))
}

/// Is process `pid`, started at `started`, still there (and not a zombie)?
fn alive(pid: i32, started: u64) -> bool {
    pid > 1 && proc_stat(pid).is_some_and(|(state, _, t)| t == started && !matches!(state, 'Z' | 'X'))
}

/// Is anything (not a zombie) left in process group `pgid`?
fn group_alive(pgid: i32) -> bool {
    fs::read_dir("/proc").into_iter().flatten().flatten().any(|e| {
        e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()).and_then(proc_stat).is_some_and(|(state, group, _)| group == pgid && !matches!(state, 'Z' | 'X'))
    })
}

/// What `.nami/<name>.pid` says, on one line: the service's supervisor (its
/// pid and start time), the service's state (starting, running,
/// restarting, failed, stopped, killed), its process group (and that
/// process's start time) and how often it was restarted. A file whose
/// processes are gone (or whose numbers others have now) is a leftover.
#[derive(Debug, Clone, Default)]
struct PidFile {
    supervisor: (i32, u64),
    state: String,
    group: (i32, u64),
    restarts: u32,
}

impl PidFile {
    fn read(project_dir: &Path, name: &str) -> Option<PidFile> {
        let text = fs::read_to_string(pid_file(project_dir, name)).ok()?;
        let f: Vec<&str> = text.split_whitespace().collect();
        let n = |i: usize| f.get(i).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        Some(PidFile { supervisor: (n(0) as i32, n(1)), state: f.get(2).unwrap_or(&"stopped").to_string(), group: (n(3) as i32, n(4)), restarts: n(5) as u32 })
    }

    /// Written whole (a new file, renamed), so it's never read half written.
    fn write(&self, project_dir: &Path, name: &str) -> Result<()> {
        let path = pid_file(project_dir, name);
        let new = path.with_extension("pid-new");
        fs::write(&new, format!("{} {} {} {} {} {}\n", self.supervisor.0, self.supervisor.1, self.state, self.group.0, self.group.1, self.restarts))?;
        fs::rename(&new, &path)?;
        Ok(())
    }

    /// Its supervisor, when it's still there.
    fn supervisor(&self) -> Option<i32> {
        alive(self.supervisor.0, self.supervisor.1).then_some(self.supervisor.0)
    }

    /// The service's process group, when anything is left in it. Its first
    /// process may have ended and left others in its group.
    fn group(&self) -> Option<i32> {
        let (pgid, started) = self.group;
        (pgid > 1 && !proc_stat(pgid).is_some_and(|(_, _, t)| t != started) && group_alive(pgid)).then_some(pgid)
    }
}

/// The supervisor (or what's left of the service) of a service that's
/// running, from its pid file.
pub fn running(project_dir: &Path, name: &str) -> Option<i32> {
    let f = PidFile::read(project_dir, name)?;
    f.supervisor().or_else(|| f.group())
}

/// Ctrl-C (or a hangup or SIGTERM) while `nami up` runs: once stops the
/// services, twice kills them. In a supervisor, SIGTERM: once stops its
/// service, twice kills it.
static STOPS: AtomicUsize = AtomicUsize::new(0);

extern "C" fn on_stop(_: libc::c_int) {
    STOPS.fetch_add(1, Ordering::SeqCst);
}

fn catch_stops() {
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        unsafe { libc::signal(sig, on_stop as *const () as libc::sighandler_t) };
    }
}

fn stopping() -> bool {
    STOPS.load(Ordering::SeqCst) > 0
}

/// Stop process group `pgid`: `sig`, then SIGKILL when it's still there
/// after STOP_TIMEOUT (or at a second stop). `child` is reaped meanwhile,
/// when it's ours. False: it had to be killed.
fn stop_group(pgid: i32, sig: i32, mut child: Option<&mut Child>) -> bool {
    let start = Instant::now();
    let stops = STOPS.load(Ordering::SeqCst);
    unsafe { libc::kill(-pgid, sig) };
    let mut killed = false;
    loop {
        if let Some(c) = child.as_mut() {
            let _ = c.try_wait();
        }
        if !group_alive(pgid) {
            return !killed;
        }
        if !killed && (start.elapsed() > STOP_TIMEOUT || STOPS.load(Ordering::SeqCst) > stops.max(1)) {
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
            killed = true;
        }
        if start.elapsed() > STOP_TIMEOUT * 2 {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Where a service's output goes: its log, and the terminal (`nami up`).
/// What nami says about it goes to the log too, and to the terminal, or,
/// in the background, to `nami up -d` until it's ready (`report`).
#[derive(Clone)]
struct Output {
    name: String,
    log: Arc<Mutex<fs::File>>,
    /// Shown in the terminal too, after this prefix.
    prefix: Option<String>,
    /// To the `nami up` that started it, until it's ready.
    report: Arc<Mutex<Option<fs::File>>>,
}

impl Output {
    fn new(project_dir: &Path, name: &str, prefix: Option<String>, report: fs::File) -> Result<Self> {
        let path = log_file(project_dir, name);
        let log = fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
        Ok(Output { name: name.to_string(), log: Arc::new(Mutex::new(log)), prefix, report: Arc::new(Mutex::new(Some(report))) })
    }

    /// Hook up a command's output.
    fn attach(&self, cmd: &mut Command) -> Result<()> {
        if self.prefix.is_some() {
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        } else {
            let log = self.log.lock().unwrap();
            cmd.stdout(log.try_clone()?).stderr(log.try_clone()?);
        }
        Ok(())
    }

    /// Copy a started command's output (when it's piped) to the terminal and the log.
    fn pump(&self, child: &mut Child) -> Vec<std::thread::JoinHandle<()>> {
        let Some(prefix) = self.prefix.clone() else { return Vec::new() };
        let streams: Vec<Box<dyn Read + Send>> = [child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>)].into_iter().flatten().collect();
        streams
            .into_iter()
            .map(|s| {
                let (log, prefix) = (self.log.clone(), prefix.clone());
                std::thread::spawn(move || {
                    for line in BufReader::new(s).split(b'\n').map_while(Result::ok) {
                        let _ = log.lock().unwrap().write_all(&[&line[..], b"\n"].concat());
                        let mut out = std::io::stdout().lock();
                        let _ = out.write_all(&[prefix.as_bytes(), &line, b"\n"].concat());
                        let _ = out.flush();
                    }
                })
            })
            .collect()
    }

    /// A line from nami itself about the service.
    fn say(&self, msg: &str) {
        let _ = writeln!(self.log.lock().unwrap(), "[nami] {msg}");
        match &self.prefix {
            Some(p) => eprintln!("{p}{msg}"),
            None => {
                if let Some(r) = self.report.lock().unwrap().as_mut() {
                    let _ = writeln!(r, "{}: {msg}", self.name);
                }
            }
        }
    }

    /// Tell `nami up` it's ready; it hears no more after that.
    fn tell_ready(&self) {
        if let Some(mut r) = self.report.lock().unwrap().take() {
            let _ = r.write_all(b"\x01ready\n");
        }
    }

    /// Tell `nami up` why it couldn't start.
    fn tell_error(&self, e: &anyhow::Error) {
        if let Some(mut r) = self.report.lock().unwrap().take() {
            let _ = write!(r, "\x01error\n{e:#}\n");
        }
    }
}

/// Who services that won't run as root (`as-root: no`, postgres) run as
/// when `nami up` runs as root. Nobody: in a user namespace of their own
/// when there can be one: still root outside it, so nothing they keep
/// changes owner. Otherwise (Docker, say, allows no user namespaces) as the
/// real nobody, with their folders given to it.
#[derive(Clone, Debug)]
struct AsUser {
    namespace: bool,
    uid: u32,
    gid: u32,
    /// For the namespace: nobody inside is root (our uid and gid) outside (Linux only).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    uid_map: Vec<u8>,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    gid_map: Vec<u8>,
}

impl AsUser {
    fn new(namespace: bool) -> AsUser {
        // nobody's uid and gid, from /etc/passwd.
        let passwd = fs::read_to_string("/etc/passwd").unwrap_or_default();
        let nobody = passwd.lines().map(|l| l.split(':').collect::<Vec<_>>()).find(|f| f.first() == Some(&"nobody") && f.len() > 3);
        let id = |i: usize| nobody.as_ref().and_then(|f| f[i].parse().ok()).unwrap_or(65534u32);
        let (uid, gid) = (id(2), id(3));
        let (me, my_group) = unsafe { (libc::geteuid(), libc::getegid()) };
        AsUser { namespace, uid, gid, uid_map: format!("{uid} {me} 1").into_bytes(), gid_map: format!("{gid} {my_group} 1").into_bytes() }
    }

    /// Become it, in a process about to run a service's command. Only
    /// system calls: this runs between fork and exec.
    fn become_it(&self) -> std::io::Result<()> {
        let err = std::io::Error::last_os_error;
        unsafe {
            if self.namespace {
                // User namespaces are Linux's (no allocation here: between fork and exec).
                #[cfg(not(target_os = "linux"))]
                return Err(std::io::Error::from_raw_os_error(libc::ENOSYS));
                #[cfg(target_os = "linux")]
                {
                    if libc::unshare(libc::CLONE_NEWUSER) != 0 {
                        return Err(err());
                    }
                    // Older kernels have no setgroups file: fine.
                    let _ = write_proc(c"/proc/self/setgroups", b"deny");
                    write_proc(c"/proc/self/uid_map", &self.uid_map)?;
                    write_proc(c"/proc/self/gid_map", &self.gid_map)?;
                }
            } else if libc::setgroups(0, std::ptr::null()) != 0 || libc::setgid(self.gid) != 0 || libc::setuid(self.uid) != 0 {
                return Err(err());
            }
        }
        Ok(())
    }

    /// Can a process become it here? Tried in a process of its own.
    fn works(&self) -> bool {
        match unsafe { libc::fork() } {
            0 => unsafe { libc::_exit(if self.become_it().is_ok() { 0 } else { 1 }) },
            -1 => false,
            pid => {
                let mut status = 0;
                unsafe { libc::waitpid(pid, &mut status, 0) };
                libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0
            }
        }
    }

    /// Give `path` (and all in it) to it, when it's the real nobody.
    fn own(&self, path: &Path) -> Result<()> {
        if self.namespace {
            return Ok(());
        }
        let meta = fs::symlink_metadata(path)?;
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != self.uid || meta.gid() != self.gid {
            std::os::unix::fs::lchown(path, Some(self.uid), Some(self.gid)).with_context(|| format!("giving {} to nobody", path.display()))?;
        }
        if meta.is_dir() {
            for e in fs::read_dir(path)? {
                self.own(&e?.path())?;
            }
        }
        Ok(())
    }
}

/// Write `data` to a /proc file, with system calls only.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn write_proc(path: &std::ffi::CStr, data: &[u8]) -> std::io::Result<()> {
    unsafe {
        let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let n = libc::write(fd, data.as_ptr() as *const libc::c_void, data.len());
        let e = std::io::Error::last_os_error();
        libc::close(fd);
        if n != data.len() as isize {
            return Err(e);
        }
    }
    Ok(())
}

/// Who the services that won't run as root (`as-root: no`, `names`) run as:
/// themselves (None), unless nami up runs as root.
fn as_user(project_dir: &Path, names: &[&str]) -> Result<Option<AsUser>> {
    if names.is_empty() || unsafe { libc::geteuid() } != 0 {
        return Ok(None);
    }
    let ns = AsUser::new(true);
    if ns.works() {
        return Ok(Some(ns));
    }
    let names = names.join(", ");
    let nobody = AsUser::new(false);
    ensure!(
        nobody.works(),
        "nami up runs as root, and {names} won't run as root. nami runs it as nobody then, but it can't here (there are no user namespaces, and it can't switch to nobody). Run nami up as an ordinary user"
    );
    // nobody has to get to its folders.
    use std::os::unix::fs::PermissionsExt;
    for dir in project_dir.ancestors() {
        let mode = fs::metadata(dir).map(|m| m.permissions().mode()).unwrap_or(0);
        ensure!(
            mode & 0o001 != 0,
            "nami up runs as root, and {names} won't run as root, so nami runs it as nobody; but nobody can't get into {}. Let others in (chmod o+x {}), move the project (to /srv, say), or run nami up as an ordinary user",
            dir.display(),
            dir.display()
        );
    }
    let nami = crate::migrate::project_state(project_dir);
    let mode = fs::metadata(&nami)?.permissions().mode();
    fs::set_permissions(&nami, fs::Permissions::from_mode(mode | 0o001))?;
    Ok(Some(nobody))
}

/// Make a short folder for a service's sockets (see `sockets_dir`): its
/// parent is ours, others may pass through it but not look in.
fn make_sockets_dir(s: &Service) -> Result<()> {
    if s.sockets == s.state {
        return Ok(());
    }
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let base = s.sockets.parent().unwrap_or(Path::new("/"));
    if fs::create_dir(base).is_ok() {
        fs::set_permissions(base, fs::Permissions::from_mode(0o711))?;
    }
    let meta = fs::symlink_metadata(base).with_context(|| format!("creating {}", base.display()))?;
    ensure!(meta.is_dir() && meta.uid() == unsafe { libc::geteuid() }, "{} isn't yours (it's for the sockets of your projects' services); remove it", base.display());
    if !s.sockets.is_dir() {
        fs::create_dir(&s.sockets).with_context(|| format!("creating {}", s.sockets.display()))?;
    }
    fs::set_permissions(&s.sockets, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// A command for one of a service's scripts, with the project's environment
/// and the service's own.
fn command(s: &Service, script: &str, vars: &[(String, String)]) -> Command {
    let mut c = Command::new("sh");
    c.arg("-c").arg(script).current_dir(&s.dir).envs(vars.iter().cloned()).envs(s.env.iter().cloned()).stdin(Stdio::null());
    c
}

/// In a service's process, before its program runs: its own process group
/// (`group`), the signals nami set aside back as they were, the user it
/// runs as (as root), and it gets `death` when its supervisor ends.
fn child_setup(group: bool, user: Option<&AsUser>, death: i32) -> std::io::Result<()> {
    unsafe {
        if group {
            libc::setpgid(0, 0);
        }
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            libc::signal(sig, libc::SIG_DFL);
        }
        if let Some(u) = user {
            u.become_it()?;
        }
        // After becoming another user, which clears it.
        #[cfg(target_os = "linux")]
        libc::prctl(libc::PR_SET_PDEATHSIG, death);
        #[cfg(not(target_os = "linux"))]
        let _ = death;
    }
    Ok(())
}

/// How waiting for a service to be ready went.
enum Ready {
    Yes,
    Ended(std::process::ExitStatus),
    TimedOut,
    Stopped,
}

/// One service's supervisor, in a process of its own: it starts the
/// service, tells `nami up` when it's ready (or why it can't be), restarts
/// it whenever it ends, and stops it at SIGTERM.
struct Supervisor<'a> {
    s: &'a Service,
    project_dir: &'a Path,
    vars: &'a [(String, String)],
    out: Output,
    user: Option<AsUser>,
    file: PidFile,
}

impl Supervisor<'_> {
    /// Its state, in its pid file.
    fn set(&mut self, state: &str) {
        self.file.state = state.to_string();
        let _ = self.file.write(self.project_dir, &self.s.name);
    }

    /// A command for its `init:` or `ready:`.
    fn command(&self, script: &str) -> Command {
        let mut c = command(self.s, script, self.vars);
        let user = self.user.clone();
        unsafe { c.pre_exec(move || child_setup(false, user.as_ref(), libc::SIGKILL)) };
        c
    }

    /// Its first start: make its folders and run its `init:`. A folder
    /// this made is removed again when init fails (or nami was stopped
    /// meanwhile), so the next start tries again.
    fn init(&mut self) -> Result<()> {
        let s = self.s;
        let done = s.state.join(".nami-init-done");
        let started = s.state.join(".nami-init-started");
        if started.exists() && !done.exists() {
            fs::remove_dir_all(&s.state).with_context(|| format!("removing {}, from a first start that didn't finish", s.state.display()))?;
        }
        let new = !s.state.exists();
        fs::create_dir_all(&s.state).with_context(|| format!("creating {}", s.state.display()))?;
        make_sockets_dir(s)?;
        if let Some(u) = &self.user {
            u.own(&s.state)?;
            u.own(&s.sockets)?;
        }
        if done.exists() {
            return Ok(());
        }
        if let Some(script) = &s.init {
            if new {
                fs::write(&started, "")?;
            }
            self.out.say("first start: setting up");
            let mut cmd = self.command(script);
            self.out.attach(&mut cmd)?;
            let mut child = cmd.spawn().with_context(|| format!("{}: running its init", s.name))?;
            let pumps = self.out.pump(&mut child);
            let status = child.wait()?;
            pumps.into_iter().for_each(|t| drop(t.join()));
            if !status.success() {
                if new {
                    let _ = fs::remove_dir_all(&s.state);
                }
                bail!("{}: its init failed ({status}); its output is in {}", s.name, s.log().display());
            }
        }
        fs::write(&done, "")?;
        let _ = fs::remove_file(&started);
        Ok(())
    }

    /// Start the service, in its own process group.
    fn spawn(&mut self) -> Result<Child> {
        let s = self.s;
        // One command: exec, so the stop signal reaches the program itself
        // (not `a; b` or `a && b`, which exec would cut short).
        let simple = s.run.trim().lines().count() == 1 && !s.run.contains([';', '&', '|', '`']) && !s.run.contains("$(");
        let script = if simple { format!("exec {}", s.run.trim()) } else { s.run.clone() };
        let mut cmd = command(s, &script, self.vars);
        self.out.attach(&mut cmd)?;
        let (user, sig) = (self.user.clone(), s.stop_signal);
        unsafe { cmd.pre_exec(move || child_setup(true, user.as_ref(), sig)) };
        let child = cmd.spawn().with_context(|| format!("starting {}", s.name))?;
        let pid = child.id() as i32;
        self.file.group = (pid, proc_stat(pid).map(|(_, _, t)| t).unwrap_or(0));
        self.set("starting");
        Ok(child)
    }

    /// Does its `ready:` say it's ready? A check that hangs says no.
    fn check(&self, check: &str) -> bool {
        let Ok(mut c) = self.command(check).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { return false };
        let start = Instant::now();
        loop {
            match c.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) if start.elapsed() < Duration::from_secs(10) && !stopping() => std::thread::sleep(Duration::from_millis(20)),
                _ => {
                    let _ = c.kill();
                    let _ = c.wait();
                    return false;
                }
            }
        }
    }

    /// Wait until it says it's ready (`ready:`), or its ports answer.
    fn wait_ready(&self, child: &mut Child, first: bool) -> Result<Ready> {
        let s = self.s;
        let start = Instant::now();
        let mut told = 0;
        loop {
            if stopping() {
                return Ok(Ready::Stopped);
            }
            if let Some(status) = child.try_wait()? {
                return Ok(Ready::Ended(status));
            }
            let ready = match &s.ready {
                Some(check) => self.check(check),
                None if s.ports.is_empty() => start.elapsed() > Duration::from_millis(500),
                None => s.ports.iter().all(|(_, p)| std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], *p).into(), Duration::from_millis(200)).is_ok()),
            };
            if ready {
                return Ok(Ready::Yes);
            }
            let waited = start.elapsed();
            if waited >= s.ready_timeout {
                return Ok(Ready::TimedOut);
            }
            if first && waited.as_secs() / 15 > told {
                told = waited.as_secs() / 15;
                self.out.say(&format!("not ready yet ({}s; waiting up to {}s)", waited.as_secs(), s.ready_timeout.as_secs()));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Why it stopped before it was ready, with its last lines when they
    /// weren't shown.
    fn ended_early(&self, status: std::process::ExitStatus) -> anyhow::Error {
        let tail = fs::read_to_string(self.s.log()).unwrap_or_default();
        let tail: Vec<&str> = tail.lines().filter(|l| !l.starts_with("[nami] ")).rev().take(15).collect();
        let shown = if self.out.prefix.is_some() { String::new() } else { format!(":\n{}", tail.into_iter().rev().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n")) };
        anyhow!("{} stopped before it was ready ({status}){shown}", self.s.name)
    }

    /// Run it until SIGTERM, or until it ends and isn't to be restarted. An
    /// error: it couldn't be started (and ready) the first time.
    fn supervise(&mut self) -> Result<()> {
        let s = self.s;
        self.set("starting");
        self.init()?;
        let mut delay = Duration::from_secs(1);
        let mut first = true;
        loop {
            // Stopped before it (re)started.
            if stopping() {
                self.out.say("stopped");
                self.set("stopped");
                return Ok(());
            }
            let mut child = self.spawn()?;
            let started = Instant::now();
            let pumps = self.out.pump(&mut child);
            let pgid = child.id() as i32;
            let mut status = None;
            match self.wait_ready(&mut child, first)? {
                Ready::Yes => {
                    self.out.say(&ready_line(s));
                    self.set("running");
                    if first {
                        self.out.tell_ready();
                        first = false;
                    }
                }
                Ready::Ended(st) => status = Some(st),
                Ready::Stopped => {}
                Ready::TimedOut => {
                    if first {
                        stop_group(pgid, s.stop_signal, Some(&mut child));
                        pumps.into_iter().for_each(|t| drop(t.join()));
                        bail!("{} isn't ready after {}s; its output is in {} (if it's just slow, give it longer: `ready-timeout: {}` in its settings)", s.name, s.ready_timeout.as_secs(), s.log().display(), s.ready_timeout.as_secs() * 4);
                    }
                    self.out.say(&format!("not ready after {}s; leaving it running", s.ready_timeout.as_secs()));
                    self.set("running");
                }
            }
            if first {
                if let Some(st) = status {
                    stop_group(pgid, s.stop_signal, None);
                    pumps.into_iter().for_each(|t| drop(t.join()));
                    return Err(self.ended_early(st));
                }
            }
            // Until it ends, or it's stopped.
            while status.is_none() && !stopping() {
                status = child.try_wait()?;
                if status.is_none() {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            let Some(status) = status else {
                let clean = stop_group(pgid, s.stop_signal, Some(&mut child));
                let _ = child.wait();
                pumps.into_iter().for_each(|t| drop(t.join()));
                self.out.say(if clean { "stopped" } else { "killed (it didn't stop by itself)" });
                self.set(if clean { "stopped" } else { "killed" });
                return Ok(());
            };
            // What it started goes with it.
            stop_group(pgid, s.stop_signal, None);
            pumps.into_iter().for_each(|t| drop(t.join()));
            let again = match s.restart {
                Restart::Always => true,
                Restart::OnFailure => !status.success(),
                Restart::No => false,
            };
            if !again {
                let state = if status.success() { "stopped" } else { "failed" };
                self.out.say(&format!("{state} ({status})"));
                self.set(state);
                return Ok(());
            }
            if started.elapsed() >= BACKOFF_MAX {
                delay = Duration::from_secs(1);
            }
            self.file.restarts += 1;
            self.out.say(&format!("ended ({status}); restarting in {}s", delay.as_secs()));
            self.set("restarting");
            let wait = Instant::now();
            while wait.elapsed() < delay && !stopping() {
                std::thread::sleep(Duration::from_millis(100));
            }
            delay = (delay * 2).min(BACKOFF_MAX);
        }
    }
}

/// Start a service's supervisor, a fork of this process: in a session of its
/// own in the background, so it outlives `nami up -d`; in the foreground,
/// it ends with `nami up`. Its pid, and what it tells (see `wait_report`).
fn start(s: &Service, project_dir: &Path, vars: &[(String, String)], prefix: Option<String>, user: Option<&AsUser>, background: bool) -> Result<(i32, fs::File)> {
    use std::os::fd::FromRawFd;
    let mut fds = [0; 2];
    #[cfg(target_os = "linux")]
    ensure!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } == 0, "starting {}: {}", s.name, std::io::Error::last_os_error());
    #[cfg(not(target_os = "linux"))]
    {
        ensure!(unsafe { libc::pipe(fds.as_mut_ptr()) } == 0, "starting {}: {}", s.name, std::io::Error::last_os_error());
        for fd in fds {
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        }
    }
    let (read, write) = (fds[0], fds[1]);
    let parent = std::process::id() as i32;
    match unsafe { libc::fork() } {
        -1 => {
            let e = std::io::Error::last_os_error();
            unsafe { (libc::close(read), libc::close(write)) };
            bail!("starting {}: {e}", s.name)
        }
        0 => {
            unsafe { libc::close(read) };
            let code = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| supervisor_main(s, project_dir, vars, prefix, user, background, write, parent))).unwrap_or(101);
            unsafe { libc::_exit(code) }
        }
        pid => {
            unsafe { libc::close(write) };
            Ok((pid, unsafe { fs::File::from_raw_fd(read) }))
        }
    }
}

/// A supervisor's process, just forked. Its exit code.
#[allow(clippy::too_many_arguments)]
fn supervisor_main(s: &Service, project_dir: &Path, vars: &[(String, String)], prefix: Option<String>, user: Option<&AsUser>, background: bool, report: i32, parent: i32) -> i32 {
    use std::os::fd::FromRawFd;
    unsafe {
        if background {
            libc::setsid();
        } else {
            #[cfg(target_os = "linux")]
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            if libc::getppid() != parent {
                return 1;
            }
        }
        // No terminal input; in the background, no terminal at all.
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        libc::dup2(null, 0);
        if background {
            libc::dup2(null, 1);
            libc::dup2(null, 2);
        }
        if null > 2 {
            libc::close(null);
        }
    }
    // Nothing else of nami's stays open in it (a terminal's pipe, a lock).
    let fds: Vec<i32> = fs::read_dir("/proc/self/fd").into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect();
    for fd in fds.into_iter().filter(|&fd| fd > 2 && fd != report) {
        unsafe { libc::close(fd) };
    }
    STOPS.store(0, Ordering::SeqCst);
    unsafe {
        libc::signal(libc::SIGTERM, on_stop as *const () as libc::sighandler_t);
        // Ctrl-C is for `nami up`, which stops the services in order.
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
    let mut report = unsafe { fs::File::from_raw_fd(report) };
    let out = match Output::new(project_dir, &s.name, prefix, report.try_clone().expect("a pipe")) {
        Ok(o) => o,
        Err(e) => {
            let _ = write!(report, "\x01error\n{e:#}\n");
            return 1;
        }
    };
    drop(report);
    let me = std::process::id() as i32;
    let file = PidFile { supervisor: (me, proc_stat(me).map(|(_, _, t)| t).unwrap_or(0)), state: "starting".into(), ..Default::default() };
    let mut sup = Supervisor { s, project_dir, vars, out, user: user.cloned(), file };
    let code = match sup.supervise() {
        Ok(()) => 0,
        Err(e) => {
            sup.set("failed");
            sup.out.say(&format!("{e:#}"));
            sup.out.tell_error(&e);
            1
        }
    };
    if s.sockets != s.state {
        let _ = fs::remove_dir(&s.sockets);
    }
    code
}

/// Wait until a supervisor says its service is ready, showing what it says
/// meanwhile (in the background; in the foreground it shows that itself).
/// Ctrl-C stops waiting.
fn wait_report(s: &Service, mut report: fs::File) -> Result<()> {
    use std::os::fd::AsRawFd;
    let mut buf: Vec<u8> = Vec::new();
    let mut error: Option<String> = None;
    loop {
        ensure!(!stopping(), "stopped");
        let mut poll = libc::pollfd { fd: report.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut poll, 1, 200) } <= 0 {
            continue;
        }
        let mut chunk = [0u8; 4096];
        let n = match report.read(&mut chunk) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        while let Some(i) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=i).collect();
            let line = String::from_utf8_lossy(&line[..i]).into_owned();
            match (&mut error, line.as_str()) {
                (Some(e), _) => *e += &format!("{line}\n"),
                (None, "\x01ready") => return Ok(()),
                (None, "\x01error") => error = Some(String::new()),
                (None, _) => eprintln!("{line}"),
            }
        }
    }
    match error {
        Some(e) => bail!("{}", e.trim_end()),
        None => bail!("{} stopped before it was ready; its output is in {}", s.name, s.log().display()),
    }
}

/// Stop a service: its supervisor (`ours`, or the one its pid file names),
/// which stops it, then whatever is left of it. Its pid file goes. What
/// happened ("stopped", "killed"), or None when nothing was running.
fn stop(project_dir: &Path, name: &str, sig: i32, ours: Option<i32>) -> Option<String> {
    let file = PidFile::read(project_dir, name);
    let supervisor = ours.or_else(|| file.as_ref().and_then(PidFile::supervisor));
    let mut state = None;
    if let Some(pid) = supervisor {
        let start = Instant::now();
        let mut sent = STOPS.load(Ordering::SeqCst);
        unsafe { libc::kill(pid, libc::SIGTERM) };
        loop {
            // Ours (or ours to reap): wait for it; else watch it go.
            let w = unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) };
            let gone = match ours {
                Some(_) => w == pid || w < 0,
                None => w == pid || !file.as_ref().is_some_and(|f| alive(pid, f.supervisor.1)),
            };
            if gone {
                break;
            }
            // Ctrl-C again: it kills the service.
            let now = STOPS.load(Ordering::SeqCst);
            if now > sent {
                unsafe { libc::kill(pid, libc::SIGTERM) };
                sent = now;
            }
            if start.elapsed() > STOP_TIMEOUT * 3 {
                unsafe { libc::kill(pid, libc::SIGKILL) };
                unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) };
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        state = PidFile::read(project_dir, name).map(|f| f.state);
    }
    // Its supervisor is gone (or was killed): what's left of the service.
    if let Some(group) = PidFile::read(project_dir, name).as_ref().and_then(PidFile::group) {
        state = Some(if stop_group(group, sig, None) { "stopped" } else { "killed" }.to_string());
    }
    let _ = fs::remove_file(pid_file(project_dir, name));
    state.map(|s| if matches!(s.as_str(), "stopped" | "killed") { s } else { "stopped".to_string() })
}

/// What `stop` says, for people.
fn stopped_line(state: &str) -> &str {
    if state == "killed" { "killed (it didn't stop by itself)" } else { state }
}

/// What a ready service says about itself.
fn ready_line(s: &Service) -> String {
    let ports: Vec<String> = s.ports.iter().map(|(k, p)| if k.is_empty() { format!("port {p}") } else { format!("{k} {p}") }).collect();
    if ports.is_empty() { "ready".into() } else { format!("ready ({})", ports.join(", ")) }
}

/// The services `names` picks (all when empty), in the project's order.
fn pick<'a>(services: &'a [Service], names: &[String]) -> Result<Vec<&'a Service>> {
    for n in names {
        if !services.iter().any(|s| &s.name == n) {
            let all: Vec<&str> = services.iter().map(|s| s.name.as_str()).collect();
            let hint = crate::config::closest(n, &all).map(|c| format!(" (did you mean {c}?)")).unwrap_or_default();
            if all.is_empty() {
                bail!("this project has no services; add some to its project.nami:\n\n  services:\n    - ./services/postgres.nami\n");
            }
            bail!("this project has no service called {n}{hint}; it has {}", all.join(", "));
        }
    }
    Ok(services.iter().filter(|s| names.is_empty() || names.contains(&s.name)).collect())
}

/// `nami up`: start the project's services (or the ones named), in order,
/// each once the one before is ready. In the foreground, their output is
/// shown prefixed with their names until Ctrl-C stops them; `background`
/// returns once they're all ready. Their supervisors restart them when they
/// end, in both.
pub fn up(project_dir: &Path, services: &[Service], vars: &[(String, String)], names: &[String], background: bool) -> Result<()> {
    // Their secrets, opened: in memory, handed to them in their environment.
    let opened: Vec<Service>;
    let services = if services.iter().any(|s| s.env.iter().any(|(_, v)| crate::secrets::marked(v).is_some())) {
        let keys = crate::secrets::Keys::mine();
        let mut all = services.to_vec();
        for s in all.iter_mut() {
            for (k, v) in s.env.iter_mut() {
                *v = keys.reveal(v).map_err(|e| anyhow!("services.{}.env.{k}: can't open the secret: {e:#}; {}", s.name, crate::secrets::access_hint()))?;
            }
        }
        opened = all;
        &opened[..]
    } else {
        services
    };
    let picked = pick(services, names)?;
    ensure!(!picked.is_empty(), "this project has no services; add some to its project.nami:\n\n  services:\n    - ./services/postgres.nami\n");
    let (already, to_start): (Vec<&Service>, Vec<&Service>) = picked.into_iter().partition(|s| running(project_dir, &s.name).is_some());
    let already_names: Vec<&str> = already.iter().map(|s| s.name.as_str()).collect();
    if !background && !already.is_empty() {
        bail!("already running in the background: {}. `nami logs` follows their output; `nami down` stops them", already_names.join(", "));
    }
    if !already.is_empty() {
        eprintln!("already running: {}", already_names.join(", "));
    }
    if to_start.is_empty() {
        return Ok(());
    }
    let busy = busy_ports(&to_start);
    ensure!(busy.is_empty(), "{}", busy.join("\n"));
    nami_dir(project_dir)?;
    let not_as_root: Vec<&str> = to_start.iter().filter(|s| !s.as_root).map(|s| s.name.as_str()).collect();
    let user = as_user(project_dir, &not_as_root)?;
    catch_stops();

    let width = to_start.iter().map(|s| s.name.len()).max().unwrap_or(0);
    let paint = crate::style::out();
    let mut started: Vec<(&Service, i32)> = Vec::new();
    let mut failed = None;
    for (i, s) in to_start.iter().enumerate() {
        if stopping() {
            failed = Some(anyhow!("stopped"));
            break;
        }
        let prefix = (!background).then(|| {
            // Each service's name in a colour of its own (none in a pipe, or with NO_COLOR).
            format!("{}{}", paint.service(i, &format!("{:width$}", s.name)), paint.dim(" | "))
        });
        let result = start(s, project_dir, vars, prefix, user.as_ref().filter(|_| !s.as_root), background).and_then(|(pid, report)| {
            started.push((s, pid));
            wait_report(s, report)
        });
        if let Err(e) = result {
            failed = Some(e);
            break;
        }
    }
    if let Some(e) = failed {
        // Not half started: what this started stops again.
        stop_all(project_dir, &started, background);
        return Err(e);
    }
    if background {
        let names: Vec<&str> = to_start.iter().map(|s| s.name.as_str()).collect();
        eprintln!("{}", crate::style::err().line(&format!("✓ Running in the background: {} · nami logs · nami down", names.join(", "))));
        return Ok(());
    }

    // The foreground: until Ctrl-C, or until they've all ended (and aren't restarted).
    let mut ended = vec![false; started.len()];
    while !stopping() && ended.iter().any(|e| !e) {
        for (i, (_, pid)) in started.iter().enumerate() {
            if !ended[i] && unsafe { libc::waitpid(*pid, std::ptr::null_mut(), libc::WNOHANG) } == *pid {
                ended[i] = true;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let left: Vec<(&Service, i32)> = started.iter().zip(&ended).filter(|(_, e)| !**e).map(|(s, _)| *s).collect();
    if !left.is_empty() {
        eprintln!("\nstopping (Ctrl-C again to kill them)");
    }
    stop_all(project_dir, &left, false);
    // Nothing of a foreground run stays behind.
    for (s, _) in &started {
        let _ = fs::remove_file(pid_file(project_dir, &s.name));
    }
    Ok(())
}

/// Stop services `up` started, the last first. In the background, say so
/// (their supervisors only tell their logs).
fn stop_all(project_dir: &Path, started: &[(&Service, i32)], background: bool) {
    for (s, pid) in started.iter().rev() {
        let state = stop(project_dir, &s.name, s.stop_signal, Some(*pid));
        if background {
            eprintln!("{}: {}", s.name, stopped_line(state.as_deref().unwrap_or("stopped")));
        }
    }
}

/// `nami down`: stop the project's running services (or the ones named),
/// the last started first. Ones no longer in project.nami stop too.
pub fn down(project_dir: &Path, services: &[Service], names: &[String]) -> Result<()> {
    let mut order: Vec<String> = services.iter().map(|s| s.name.clone()).collect();
    // Running ones the file no longer has: their pid files.
    for e in fs::read_dir(crate::migrate::project_state(project_dir)).into_iter().flatten().flatten() {
        if let Some(n) = e.file_name().to_str().and_then(|f| f.strip_suffix(".pid")) {
            if !order.iter().any(|o| o == n) {
                order.push(n.to_string());
            }
        }
    }
    for n in names {
        ensure!(order.contains(n), "this project has no service called {n}{}", crate::config::closest(n, &order.iter().map(String::as_str).collect::<Vec<_>>()).map(|c| format!(" (did you mean {c}?)")).unwrap_or_default());
    }
    let mut stopped = 0;
    for name in order.iter().rev().filter(|n| names.is_empty() || names.contains(n)) {
        let sig = services.iter().find(|s| &s.name == name).map(|s| s.stop_signal).unwrap_or(libc::SIGTERM);
        if running(project_dir, name).is_none() {
            // A leftover (it failed, or nami was killed): nothing to stop.
            let _ = fs::remove_file(pid_file(project_dir, name));
            if !names.is_empty() {
                eprintln!("{name} isn't running");
            }
            continue;
        }
        let state = stop(project_dir, name, sig, None).unwrap_or_else(|| "stopped".into());
        eprintln!("{name}: {}", stopped_line(&state));
        stopped += 1;
    }
    if stopped == 0 && names.is_empty() {
        eprintln!("nothing is running");
    }
    Ok(())
}

/// A service as `nami project services` shows it.
#[derive(Debug, Serialize)]
pub struct Status {
    pub name: String,
    /// running, starting, restarting, failed or stopped.
    pub state: String,
    pub running: bool,
    /// Its supervisor's.
    pub pid: Option<i32>,
    /// How often it was restarted since `nami up`.
    pub restarts: u32,
    pub ports: Vec<u16>,
    pub about: Option<String>,
    pub log: String,
}

pub fn status(project_dir: &Path, services: &[Service]) -> Vec<Status> {
    services
        .iter()
        .map(|s| {
            let file = PidFile::read(project_dir, &s.name);
            let pid = running(project_dir, &s.name);
            let state = match &file {
                Some(f) if f.supervisor().is_some() => f.state.clone(),
                // Its supervisor was killed, and left it running.
                Some(f) if f.group().is_some() => "running".into(),
                Some(f) if f.state == "failed" => "failed".into(),
                _ => "stopped".into(),
            };
            let state = if pid.is_some() && matches!(state.as_str(), "stopped" | "killed" | "failed") { "stopping".into() } else { state };
            Status { name: s.name.clone(), running: pid.is_some(), state, pid, restarts: file.map_or(0, |f| f.restarts), ports: s.ports.iter().map(|(_, p)| *p).collect(), about: s.about.clone(), log: log_file(project_dir, &s.name).display().to_string() }
        })
        .collect()
}

/// `nami logs`: the services' output (or one's), the last lines first, then
/// following it as it comes, until Ctrl-C.
pub fn logs(project_dir: &Path, services: &[Service], name: Option<&str>) -> Result<()> {
    let names: Vec<String> = match name {
        Some(n) => {
            pick(services, &[n.to_string()])?;
            vec![n.to_string()]
        }
        None => services.iter().map(|s| s.name.clone()).collect(),
    };
    let width = names.iter().map(String::len).max().unwrap_or(0);
    let paint = crate::style::out();
    let prefix = |n: &str| if name.is_some() { String::new() } else { format!("{}{}", paint.service(names.iter().position(|x| x == n).unwrap_or(0), &format!("{n:width$}")), paint.dim(" | ")) };
    if !names.iter().any(|n| log_file(project_dir, n).exists()) {
        bail!("no output yet: `nami up` starts the services");
    }
    // name -> (read up to, the start of a line not yet ended)
    let mut at: BTreeMap<String, (u64, Vec<u8>)> = BTreeMap::new();
    for n in &names {
        let data = fs::read(log_file(project_dir, n)).unwrap_or_default();
        let text = String::from_utf8_lossy(&data);
        let lines: Vec<&str> = text.lines().collect();
        for l in &lines[lines.len().saturating_sub(100)..] {
            println!("{}{l}", prefix(n));
        }
        at.insert(n.clone(), (data.len() as u64, Vec::new()));
    }
    loop {
        for n in &names {
            let Ok(mut f) = fs::File::open(log_file(project_dir, n)) else { continue };
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            let (pos, partial) = at.get_mut(n).unwrap();
            if len < *pos {
                *pos = 0; // started again: a new log
                partial.clear();
            }
            if len == *pos {
                continue;
            }
            f.seek(std::io::SeekFrom::Start(*pos))?;
            let mut new = Vec::new();
            f.take(len - *pos).read_to_end(&mut new)?;
            *pos = len;
            partial.extend(new);
            while let Some(i) = partial.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = partial.drain(..=i).collect();
                println!("{}{}", prefix(n), String::from_utf8_lossy(&line[..line.len() - 1]));
            }
        }
        std::io::stdout().flush()?;
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(s: &str) -> Value {
        serde_yaml::from_str(s).unwrap()
    }

    /// An example service file, by its path.
    fn ex(name: &str) -> String {
        format!("{}/examples/services/{name}.nami", env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn example_services_are_complete() {
        let dir = Path::new("/home/me/shop");
        let all: Vec<String> = fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/services")).unwrap().flatten().map(|e| e.path().display().to_string()).collect();
        for file in &all {
            let s = parse(file, &Value::Bool(true), dir).unwrap().unwrap_or_else(|| panic!("{file}"));
            assert!(!s.packages.is_empty() && !s.ports.is_empty() && !s.exports.is_empty(), "{file}: packages, a port and exports");
            assert!(s.about.is_some(), "{file}: about");
            assert!(!s.run.contains('{') || s.run.contains("${"), "{file}: {}", s.run);
        }
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn postgres_with_changes() {
        let dir = Path::new("/home/me/my shop");
        let s = parse(&ex("postgres"), &yaml("{ version: 16, port: 5433, args: -c fsync=off, env: { TZ: UTC }, exports: { DATABASE_URL: postgres://x } }"), dir).unwrap().unwrap();
        assert_eq!(s.packages, ["postgresql@16"]);
        assert_eq!(s.ports, [(String::new(), 5433)]);
        assert_eq!(s.state, Path::new("/home/me/my shop/.nami/postgres"));
        // In commands, a path with a space is quoted; in variables it's as it is.
        assert_eq!(s.run, "postgres -D '/home/me/my shop/.nami/postgres'/data -k '/home/me/my shop/.nami/postgres' -p 5433 -c listen_addresses=localhost -c fsync=off");
        let exports: BTreeMap<_, _> = s.exports.iter().cloned().collect();
        assert_eq!(exports["PGHOST"], "/home/me/my shop/.nami/postgres");
        assert_eq!(exports["PGPORT"], "5433");
        assert_eq!(exports["PGDATABASE"], "my_shop");
        assert_eq!(exports["DATABASE_URL"], "postgres://x", "the project's own wins");
        assert_eq!(s.env, [("TZ".to_string(), "UTC".to_string())]);
        assert_eq!(s.stop_signal, libc::SIGINT);
        assert!(!s.as_root && parse(&ex("redis"), &Value::Bool(true), dir).unwrap().unwrap().as_root);
        assert!(s.init.unwrap().contains("CREATE DATABASE \"my_shop\""));
        // `false`: left out.
        assert_eq!(parse(&ex("postgres"), &Value::Bool(false), dir).unwrap(), None);
    }

    #[test]
    fn own_services() {
        let dir = Path::new("/p");
        let s = parse("worker", &yaml("{ run: npm run worker, env: { QUEUE: default, N: 2 }, dir: web, port: 3000, exports: { API: 'http://localhost:{port}' } }"), dir).unwrap().unwrap();
        assert_eq!((s.run.as_str(), s.dir.as_path()), ("npm run worker", Path::new("/p/web")));
        assert_eq!(s.env, [("N".to_string(), "2".to_string()), ("QUEUE".to_string(), "default".to_string())]);
        assert_eq!(s.exports, [("API".to_string(), "http://localhost:3000".to_string())]);
        // A second postgres: a copy of its file, by another name.
        let d = std::env::temp_dir().join(format!("nami-testdb-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        fs::copy(ex("postgres"), d.join("testdb.nami")).unwrap();
        let db = parse("./testdb.nami", &yaml("{ port: 5433 }"), &d).unwrap().unwrap();
        assert_eq!((db.name.as_str(), db.state.clone()), ("testdb", d.join(".nami/testdb")));
        assert!(db.run.contains("-p 5433"));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn mistakes_are_explained() {
        let dir = Path::new("/p");
        let err = |v: &str| format!("{:#}", parse(&ex("postgres"), &yaml(v), dir).unwrap_err());
        assert!(err("{ prot: 5433 }").contains("no key `prot` (did you mean `port`?)"), "{}", err("{ prot: 5433 }"));
        assert!(err("{ stop-signal: BOOM }").contains("isn't a signal"));
        assert!(err("exports:\n  X: x{nope}\n").contains("{nope} isn't a placeholder"), "{}", err("exports:\n  X: x{nope}\n"));
        let e = format!("{:#}", parse("postgres", &Value::Bool(true), dir).unwrap_err());
        assert!(e.contains("your own service needs `run:`, or list a service file by its path (./services/postgres.nami"), "{e}");
        let e = format!("{:#}", parse("w", &yaml("run: x --port {port}\n"), dir).unwrap_err());
        assert!(e.contains("there's no `port:`"), "{e}");
        assert!(parse("bad/name", &Value::Bool(true), dir).is_err());
        let two: Mapping = serde_yaml::from_str(&format!("{}: true\ndb: {{ run: x, port: 5432 }}\n", ex("postgres"))).unwrap();
        assert!(resolve(&two, dir).unwrap_err().to_string().contains("both use port 5432"));
        let same: Mapping = serde_yaml::from_str(&format!("{}: true\n./other/postgres.nami: true\n", ex("postgres"))).unwrap();
        assert!(resolve(&same, dir).unwrap_err().to_string().contains("are both postgres"));
    }

    #[test]
    fn placeholders_leave_shell_alone() {
        let v: BTreeMap<String, String> = [("state".to_string(), "/a b".to_string()), ("port".to_string(), "1".to_string())].into();
        assert_eq!(placeholders("x {state} ${HOME} {port} awk '{ print $1 }' {}", &v, true).unwrap(), "x '/a b' ${HOME} 1 awk '{ print $1 }' {}");
        assert_eq!(placeholders("{state}/s", &v, false).unwrap(), "/a b/s");
    }

    #[test]
    fn a_taken_port_says_what_to_do() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let s = parse(&ex("mailpit"), &yaml(&format!("{{ ports: {{ web: {port} }} }}")), Path::new("/p")).unwrap().unwrap();
        let busy = busy_ports(&[&s]);
        assert_eq!(busy.len(), 1, "{busy:?}");
        assert!(busy[0].contains(&format!("mailpit can't start: port {port} is already in use")), "{}", busy[0]);
        assert!(busy[0].contains(&format!("mailpit.nami: {{ ports: {{ web: {} }} }}", port + 1)), "{}", busy[0]);
        assert!(port_in_use("./services/postgres.nami", "", 5432).contains("- ./services/postgres.nami: { port: 5433 }"));
    }

    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "a project's services (nami up) are Linux's for now")]
    fn runs_and_stops_a_process_group() {
        let dir = std::env::temp_dir().join(format!("nami-up-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let services = resolve(&serde_yaml::from_str("sleeper:\n  run: sh -c 'sleep 300 & sleep 300'\n  init: echo hi > {state}/hello\n").unwrap(), &dir).unwrap();
        let vars = vec![("PATH".to_string(), std::env::var("PATH").unwrap())];
        up(&dir, &services, &vars, &[], true).unwrap();
        assert_eq!(fs::read_to_string(dir.join(".nami/.gitignore")).unwrap().lines().last(), Some("*"));
        assert_eq!(fs::read_to_string(dir.join(".nami/sleeper/hello")).unwrap(), "hi\n");
        let supervisor = running(&dir, "sleeper").expect("running");
        let pgid = PidFile::read(&dir, "sleeper").unwrap().group().expect("its group");
        assert_eq!(status(&dir, &services)[0].state, "running");
        // Started twice: it says so instead.
        assert!(up(&dir, &services, &vars, &[], false).unwrap_err().to_string().contains("already running"));
        up(&dir, &services, &vars, &[], true).unwrap();
        assert_eq!(status(&dir, &services)[0].pid, Some(supervisor));
        down(&dir, &services, &[]).unwrap();
        assert!(!group_alive(pgid), "its children stopped too");
        assert_eq!(running(&dir, "sleeper"), None);
        assert!(!pid_file(&dir, "sleeper").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Wait (up to `secs`) until `f` says so.
    fn until(secs: u64, f: impl Fn() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(secs) {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    #[test]
    fn restarts_what_ends() {
        let dir = std::env::temp_dir().join(format!("nami-up-restart-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let services = resolve(&serde_yaml::from_str("crashy: { run: 'sleep 1; exit 3' }\nonce: { run: 'sleep 1; exit 3', restart: no }\n").unwrap(), &dir).unwrap();
        let vars = vec![("PATH".to_string(), std::env::var("PATH").unwrap())];
        up(&dir, &services, &vars, &[], true).unwrap();
        assert!(until(10, || status(&dir, &services)[0].restarts >= 2), "{:?}", status(&dir, &services));
        let log = fs::read_to_string(log_file(&dir, "crashy")).unwrap();
        assert!(log.contains("ended (exit status: 3); restarting in 1s") && log.contains("restarting in 2s"), "{log}");
        // restart: no: it ends, and says it failed.
        assert!(until(10, || status(&dir, &services)[1].state == "failed"), "{:?}", status(&dir, &services));
        assert!(running(&dir, "once").is_none());
        down(&dir, &services, &[]).unwrap();
        assert!(running(&dir, "crashy").is_none() && !pid_file(&dir, "crashy").exists() && !pid_file(&dir, "once").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "a project's services (nami up) are Linux's for now")]
    fn a_slow_start_waits_as_long_as_it_says() {
        let dir = std::env::temp_dir().join(format!("nami-up-slow-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let text = |timeout: u32| format!("slow: {{ run: 'sleep 2; touch {{state}}/up; exec sleep 300', ready: 'test -f {{state}}/up', ready-timeout: {timeout} }}\n");
        let vars = vec![("PATH".to_string(), std::env::var("PATH").unwrap())];
        let services = resolve(&serde_yaml::from_str(&text(1)).unwrap(), &dir).unwrap();
        let e = format!("{:#}", up(&dir, &services, &vars, &[], true).unwrap_err());
        assert!(e.contains("slow isn't ready after 1s") && e.contains("ready-timeout: 4"), "{e}");
        assert!(running(&dir, "slow").is_none() && !pid_file(&dir, "slow").exists(), "stopped again");
        let services = resolve(&serde_yaml::from_str(&text(20)).unwrap(), &dir).unwrap();
        up(&dir, &services, &vars, &[], true).unwrap();
        assert_eq!(status(&dir, &services)[0].state, "running");
        down(&dir, &services, &[]).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn restart_and_ready_timeout_settings() {
        let dir = Path::new("/p");
        let s = |v: &str| parse("w", &yaml(v), dir);
        assert_eq!(s("{ run: x }").unwrap().unwrap().restart, Restart::Always);
        assert_eq!(s("{ run: x }").unwrap().unwrap().ready_timeout, READY_TIMEOUT);
        assert_eq!(s("{ run: x, restart: no }").unwrap().unwrap().restart, Restart::No);
        assert_eq!(s("{ run: x, restart: false }").unwrap().unwrap().restart, Restart::No);
        assert_eq!(s("{ run: x, restart: on-failure }").unwrap().unwrap().restart, Restart::OnFailure);
        assert_eq!(s("{ run: x, ready-timeout: 5m }").unwrap().unwrap().ready_timeout, Duration::from_secs(300));
        assert_eq!(s("{ run: x, ready-timeout: 45 }").unwrap().unwrap().ready_timeout, Duration::from_secs(45));
        assert!(format!("{:#}", s("{ run: x, restart: sometimes }").unwrap_err()).contains("on-failure or no"));
        assert!(format!("{:#}", s("{ run: x, ready-timeout: soon }").unwrap_err()).contains("isn't a duration"));
    }

    #[test]
    fn sockets_go_somewhere_short_when_the_path_is_long() {
        let deep = Path::new("/home/me/work/clients/some-client/projects/a-rather-long-project-name/backend");
        let s = parse(&ex("postgres"), &Value::Bool(true), deep).unwrap().unwrap();
        assert_ne!(s.sockets, s.state);
        assert!(s.sockets.join(".s.PGSQL.5432.lock").as_os_str().len() < 108, "{}", s.sockets.display());
        let exports: BTreeMap<_, _> = s.exports.iter().cloned().collect();
        assert_eq!(exports["PGHOST"], s.sockets.display().to_string());
        assert!(s.run.contains(&format!("-k {}", s.sockets.display())));
        // The same each time, and another service's is another.
        assert_eq!(parse(&ex("postgres"), &Value::Bool(true), deep).unwrap().unwrap().sockets, s.sockets);
        assert_ne!(parse(&ex("redis"), &Value::Bool(true), deep).unwrap().unwrap().sockets, s.sockets);
        // A short path keeps them in its folder.
        let s = parse(&ex("postgres"), &Value::Bool(true), Path::new("/home/me/shop")).unwrap().unwrap();
        assert_eq!(s.sockets, s.state);
    }
}
