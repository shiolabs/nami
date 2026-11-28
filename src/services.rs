//! `services:` in your nami file: systemd units. In the system file they're the
//! computer's (system units, run as root's systemd); in a user file they're
//! yours (`systemctl --user`).
//!
//! ```yaml
//! services:
//!   syncthing@ada: true          # the unit a package ships (its template, here)
//!   tailscaled: { package: tailscale, env: { PORT: "41641" } }   # with overrides
//!   myapp:                        # your own
//!     run: myapp-server --port 8080   # first word: absolute, or found in the
//!     env: { RUST_LOG: info }         # packages, the system profile, then /usr/bin...
//!     after: [network-online.target]
//!     restart: on-failure         # the default
//!     user: myapp                 # default: a fresh user for the service (DynamicUser)
//!     /etc/myapp/config.toml:     # its own files: restarted when nami changes one
//!       port: 8080
//!   backup:
//!     script: |                   # several lines: run with bash -e
//!       restic backup /home
//!     at: daily                   # a timer (systemd's OnCalendar) starts it
//!     systemd:                    # any other systemd keys, as they are
//!       Service: { Type: oneshot, Nice: 10 }
//! ```
//!
//! Like home files: the units are rendered into one store path per change,
//! linked from the generation as `.units`. Activation copies them into
//! /etc/systemd/system (yours: ~/.config/systemd/user) under their own names,
//! so `systemctl status myapp` works and a package's unit overrides a distro
//! copy of the same name. Copies, not links into /nix: units are read at
//! boot, possibly before /nix is mounted (it's a bind mount on immutable
//! distros), and a dangling unit link makes systemd forget the service.
//! Enabling is nami's own `<target>.wants/` links (what `systemctl enable`
//! would make), tracked with the files in a state file, so whatever nami
//! added it can take away again exactly, and `nami undo` reverts services:
//! the old generation's units are copied back, then what changed restarts.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};

use crate::profile::Entry;
use crate::store::{write_atomic, Store};

pub const KEYS: &[&str] = &["enable", "run", "script", "at", "package", "user", "env", "after", "restart", "restart-on", "description", "dir", "systemd"];
/// Sections `systemd:` may add to.
const SECTIONS: &[&str] = &["Unit", "Service", "Install", "Timer"];
pub const RESTART: &[&str] = &["no", "on-success", "on-failure", "on-abnormal", "on-watchdog", "on-abort", "always"];
/// Unit types a package's unit may come with, besides the .service.
const COMPANIONS: &[&str] = &["socket", "timer", "path"];
/// systemd's own search path for non-absolute commands, after nami's profile.
/// (`/run/current-system/sw/bin`: a computer `nami usb` made, and NixOS.)
const HOST_PATH: &[&str] = &["/usr/local/sbin", "/usr/local/bin", "/usr/sbin", "/usr/bin", "/sbin", "/bin", "/run/current-system/sw/bin"];
/// The system's profile's programs, as a unit names them. Units are in the store, the same on
/// every computer: by nami's old place, which reaches its folder on every one ([`crate::migrate::VAR`]).
fn system_profile_bin() -> String {
    format!("/{}/profiles/default/bin", crate::migrate::VAR)
}

/// One entry under `services:`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Service {
    /// As written (`myapp`, `syncthing@ada`), without `.service`.
    pub name: String,
    pub enable: bool,
    /// Your own service: its command line. None: a package's unit.
    pub run: Option<String>,
    /// Your own service as a script (several lines, bash -e).
    pub script: Option<String>,
    /// When a timer starts it (OnCalendar), instead of starting with the computer.
    pub at: Option<String>,
    /// More systemd keys: section -> key -> values, written as they are.
    pub systemd: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    pub package: Option<String>,
    pub user: Option<String>,
    pub env: BTreeMap<String, String>,
    pub after: Vec<String>,
    pub restart: Option<String>,
    pub description: Option<String>,
    pub dir: Option<String>,
    /// /etc paths: when an activation changes one, the service restarts.
    pub restart_on: Vec<String>,
    /// A unit named whole (`cups.socket`, `getty@tty1.service`): that one,
    /// from whichever installed package brings it.
    pub unit: Option<String>,
}

impl Service {
    /// The package whose unit this uses (`syncthing@ada` -> syncthing), if any.
    pub fn package_name(&self) -> Option<String> {
        if self.unit.is_some() {
            return self.package.clone();
        }
        (!self.is_custom()).then(|| self.package.clone().unwrap_or_else(|| self.name.split('@').next().unwrap_or(&self.name).to_string()))
    }

    fn has_overrides(&self) -> bool {
        self.user.is_some() || !self.env.is_empty() || !self.after.is_empty() || self.restart.is_some() || self.description.is_some() || self.dir.is_some() || !self.systemd.is_empty()
    }

    /// Your own (run: or script:), not a package's unit.
    pub fn is_custom(&self) -> bool {
        self.run.is_some() || self.script.is_some()
    }

    /// A `systemd:` key's values, if set.
    fn raw(&self, section: &str, key: &str) -> Option<&Vec<String>> {
        self.systemd.get(section).and_then(|s| s.get(key))
    }
}

pub fn valid_unit_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 200 && !name.starts_with(['.', '-', '@'])
        && name.chars().all(|c| c.is_ascii_alphanumeric() || ":-_.@\\".contains(c))
}

fn valid_var(k: &str) -> bool {
    k.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn valid_user(u: &str) -> bool {
    !u.is_empty() && u.len() <= 32 && !u.starts_with('-') && u.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

fn text(v: &Value, what: &str) -> Result<String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        _ => bail!("{what} must be text"),
    }
}

/// One `services:` entry, checked. Errors name the key (`services.myapp.restart: ...`).
pub fn parse(key: &str, value: &Value) -> Result<Service> {
    let s = parse_entry(key, value)?;
    if s.is_custom() {
        return Ok(s);
    }
    // Named whole: that unit, from the packages.
    match UNIT_TYPES.iter().any(|t| key.ends_with(t)) {
        true => Ok(Service { name: key.to_string(), unit: Some(key.to_string()), ..s }),
        false => Ok(s),
    }
}

/// A unit's name, whole: `cups.socket` is a unit the packages bring.
pub const UNIT_TYPES: &[&str] = &[".service", ".socket", ".path", ".timer", ".target"];

fn parse_entry(key: &str, value: &Value) -> Result<Service> {
    let name = key.strip_suffix(".service").unwrap_or(key);
    let at = format!("services.{name}");
    ensure!(valid_unit_name(name), "{at}: not a valid unit name (letters, digits, and : - _ . @)");
    ensure!(!name.ends_with('@'), "{at}: a template needs an instance, like {name}ada");
    let mut s = Service { name: name.to_string(), enable: true, ..Default::default() };
    match value {
        Value::Bool(b) => s.enable = *b,
        Value::Mapping(m) => {
            for (k, v) in m {
                let k = k.as_str().ok_or_else(|| anyhow!("{at}: keys must be names"))?;
                match k {
                    "enable" => s.enable = v.as_bool().ok_or_else(|| anyhow!("{at}.enable must be true or false"))?,
                    "run" => s.run = Some(text(v, &format!("{at}.run"))?),
                    "script" => s.script = Some(text(v, &format!("{at}.script"))?),
                    "at" => s.at = Some(text(v, &format!("{at}.at"))?),
                    "systemd" => {
                        let m = v.as_mapping().ok_or_else(|| anyhow!("{at}.systemd must be sections (Unit, Service, Install, Timer) of keys"))?;
                        for (sk, sv) in m {
                            let sk = sk.as_str().ok_or_else(|| anyhow!("{at}.systemd: sections are names"))?;
                            ensure!(SECTIONS.contains(&sk), "{at}.systemd.{sk}: one of {}", SECTIONS.join(", "));
                            let keys = sv.as_mapping().ok_or_else(|| anyhow!("{at}.systemd.{sk} must be keys and values"))?;
                            let section = s.systemd.entry(sk.to_string()).or_default();
                            for (k, v) in keys {
                                let k = k.as_str().ok_or_else(|| anyhow!("{at}.systemd.{sk}: keys are names"))?;
                                ensure!(!k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric()), "{at}.systemd.{sk}.{k}: not a systemd key name");
                                ensure!(k != "ExecStart", "{at}.systemd.{sk}.ExecStart: that's `run:` or `script:`");
                                let vals = match v {
                                    Value::Sequence(l) => l.iter().map(|x| raw_value(x, &format!("{at}.systemd.{sk}.{k}"))).collect::<Result<Vec<_>>>()?,
                                    x => vec![raw_value(x, &format!("{at}.systemd.{sk}.{k}"))?],
                                };
                                section.insert(k.to_string(), vals);
                            }
                        }
                    }
                    "package" => s.package = Some(text(v, &format!("{at}.package"))?),
                    "user" => s.user = Some(text(v, &format!("{at}.user"))?),
                    "description" => s.description = Some(text(v, &format!("{at}.description"))?),
                    "dir" => s.dir = Some(text(v, &format!("{at}.dir"))?),
                    "restart" => s.restart = Some(text(v, &format!("{at}.restart"))?),
                    "env" => {
                        let m = v.as_mapping().ok_or_else(|| anyhow!("{at}.env must be a map of NAME: value"))?;
                        for (ek, ev) in m {
                            let ek = ek.as_str().ok_or_else(|| anyhow!("{at}.env: names must be text"))?;
                            ensure!(valid_var(ek), "{at}.env: {ek:?} isn't a valid variable name");
                            s.env.insert(ek.to_string(), text(ev, &format!("{at}.env.{ek}"))?);
                        }
                    }
                    "after" => {
                        s.after = match v {
                            Value::String(one) => vec![one.clone()],
                            Value::Sequence(l) => l.iter().map(|u| text(u, &format!("{at}.after"))).collect::<Result<_>>()?,
                            _ => bail!("{at}.after must be a unit or a list of units"),
                        }
                    }
                    "restart-on" => {
                        s.restart_on = match v {
                            Value::String(one) => vec![one.clone()],
                            Value::Sequence(l) => l.iter().map(|p| text(p, &format!("{at}.restart-on"))).collect::<Result<_>>()?,
                            _ => bail!("{at}.restart-on must be a path or a list of paths"),
                        }
                    }
                    "per-person" => bail!("{at}.per-person: each person's own unit is for system.nami (in a person's user.nami, services are theirs already)"),
                    // (restart-on is nami's own: a service's files go under it.)
                    other => bail!("{at}: unknown key `{other}` (one of: {}, or its own files: /etc/...)", KEYS.iter().filter(|k| **k != "restart-on").copied().collect::<Vec<_>>().join(", ")),
                }
            }
        }
        Value::Null => bail!("{at}: say `true` (a package's unit) or give settings (`run:` for your own)"),
        _ => bail!("{at}: expected true/false or settings"),
    }
    ensure!(!(s.run.is_some() && s.script.is_some()), "{at}: either `run:` (one command) or `script:` (several lines), not both");
    if let Some(script) = &s.script {
        ensure!(!script.trim().is_empty(), "{at}.script is empty");
        ensure!(s.package.is_none(), "{at}: either `script:` (your own service) or `package:` (a package's unit), not both");
        ensure!(!name.contains('@'), "{at}: instances (name@x) are for packages' template units; give your service a plain name");
    }
    if let Some(when) = &s.at {
        ensure!(s.is_custom(), "{at}.at: a timer starts your own service (run: or script:)");
        ensure!(!when.trim().is_empty() && !when.contains('\n'), "{at}.at: one OnCalendar expression, like daily or *-*-* 04:00");
    }
    if let Some(run) = &s.run {
        ensure!(!run.trim().is_empty(), "{at}.run is empty");
        ensure!(!run.contains('\n'), "{at}.run: one command line (for more, write a script and run that)");
        ensure!(s.package.is_none(), "{at}: either `run:` (your own service) or `package:` (a package's unit), not both");
        ensure!(!name.contains('@'), "{at}: instances (name@x) are for packages' template units; give your service a plain name");
    }
    if let Some(p) = &s.package {
        ensure!(!p.is_empty() && !p.contains(['/', ' ']), "{at}.package: a catalog name, like syncthing");
    }
    if let Some(u) = &s.user {
        ensure!(valid_user(u), "{at}.user: {u:?} isn't a user name");
    }
    if let Some(r) = &s.restart {
        ensure!(RESTART.contains(&r.as_str()), "{at}.restart: {r:?} isn't one of {}", RESTART.join(", "));
    }
    if let Some(d) = &s.dir {
        ensure!(d.starts_with('/'), "{at}.dir must be an absolute path");
    }
    for u in &s.after {
        ensure!(valid_unit_name(u) && u.contains('.'), "{at}.after: {u:?} isn't a unit name (like network-online.target)");
    }
    for p in &s.restart_on {
        ensure!(p.starts_with('/') && !p.split('/').any(|c| c == ".." || c == "."), "{at}.restart-on: {p:?} must be an absolute path (like /etc/myapp/config.toml)");
    }
    for (k, v) in s.env.iter().chain(s.description.iter().map(|d| (&s.name, d))) {
        ensure!(!v.contains('\n'), "{at}: {k} must be one line");
    }
    Ok(s)
}

pub fn parse_all(map: &Mapping) -> Result<Vec<Service>> {
    let mut out: Vec<Service> = Vec::new();
    for (k, v) in map {
        let k = k.as_str().ok_or_else(|| anyhow!("services: keys must be names"))?;
        let s = parse(k, v)?;
        ensure!(!out.iter().any(|o| o.name == s.name), "services.{} is listed twice", s.name);
        out.push(s);
    }
    Ok(out)
}

/// What only a tier may have: `user:` is for system services.
pub fn check_tier(s: &Service, user_units: bool) -> Result<()> {
    if user_units {
        ensure!(s.user.is_none(), "services.{}.user: your services run as you; `user:` is for system services (the system file)", s.name);
    }
    Ok(())
}

/// Packages the services need: added to the profile with them.
pub fn packages(services: &[Service]) -> Vec<String> {
    let mut out: Vec<String> = services.iter().filter_map(Service::package_name).collect();
    out.dedup();
    out
}

/// The rendered units of a generation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServicesState {
    /// Store path holding the unit files (and drop-ins `<unit>.d/50-nami.conf`).
    pub path: String,
    /// User units (`systemctl --user`) rather than system ones.
    #[serde(default)]
    pub user: bool,
    /// Each file in `path` and its NAR hash.
    pub files: BTreeMap<String, String>,
    pub units: Vec<Unit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Unit {
    /// `myapp.service`, `syncthing@ada.service`.
    pub name: String,
    /// The `services:` entry it's from.
    pub service: String,
    /// Its files in the store path (a template's instances share the template).
    pub files: Vec<String>,
    /// Over all its files: when it changes, the unit restarts.
    pub hash: String,
    pub enable: bool,
    /// Where enabling links it: `multi-user.target.wants`, ...
    pub install: Vec<String>,
    /// `restart-on:`: /etc paths whose change restarts it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restart_on: Vec<String>,
}

/// `#` comment saying where a unit came from.
fn header(source: &str, name: &str) -> String {
    format!("# Generated by nami from {source} (services.{name}); changes here are replaced on the next apply.\n")
}

/// A `systemd:` value as unit-file text: true/false as yes/no.
fn raw_value(v: &Value, at: &str) -> Result<String> {
    let s = match v {
        Value::Bool(b) => if *b { "yes" } else { "no" }.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => bail!("{at}: a value or a list of values"),
    };
    ensure!(!s.contains('\n'), "{at}: one line per value (a list for several)");
    Ok(s)
}

/// A script as one ExecStart= line: bash -e (as NixOS runs service
/// scripts), with systemd's own unquoting undone: C escapes for newlines and
/// quotes, `$$` for a shell's `$`, `%%` for a `%`.
pub fn script_exec(script: &str) -> String {
    let body = script.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('$', "$$").replace('%', "%%");
    format!("/usr/bin/env bash -e -c \"{body}\"")
}

/// systemd unit-file value quoting for `Environment=`, and `%` specifiers.
fn env_line(k: &str, v: &str) -> String {
    format!("Environment=\"{}={}\"\n", k, v.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%"))
}

fn unit_section(s: &Service, out: &mut String) {
    for a in &s.after {
        out.push_str(&format!("After={a}\n"));
        if a == "network-online.target" {
            out.push_str("Wants=network-online.target\n"); // After= alone doesn't pull it in
        }
    }
}

/// `systemd:` keys for `section`, as unit-file lines.
fn raw_lines(s: &Service, section: &str) -> String {
    let mut out = String::new();
    for (k, vals) in s.systemd.get(section).into_iter().flatten() {
        for v in vals {
            out += &format!("{k}={v}\n");
        }
    }
    out
}

/// Your own service, from `run:` (`exec`: with its command resolved) or
/// `script:` (`exec` is script_exec's line, escaped already).
pub fn custom_unit(s: &Service, exec: &str, user_units: bool, source: &str) -> String {
    let mut u = header(source, &s.name);
    u += "\n[Unit]\n";
    u += &format!("Description={}\n", s.description.clone().unwrap_or_else(|| format!("{} (nami)", s.name)));
    unit_section(s, &mut u);
    if !user_units {
        u += "RequiresMountsFor=/nix/store\n";
    }
    u += &raw_lines(s, "Unit");
    u += "\n[Service]\n";
    u += &format!("ExecStart={}\n", if s.script.is_some() { exec.to_string() } else { exec.replace('%', "%%") });
    // A oneshot (or a timer's job) isn't restarted unless asked.
    let oneshot = s.raw("Service", "Type").is_some_and(|t| t.iter().any(|t| t == "oneshot")) || s.at.is_some();
    match (&s.restart, oneshot, s.raw("Service", "Restart")) {
        (_, _, Some(_)) => {}
        (Some(r), _, None) => u += &format!("Restart={r}\n"),
        (None, false, None) => u += "Restart=on-failure\n",
        (None, true, None) => {}
    }
    let path = if user_units { format!("%h/.local/state/nami/profile/bin:{}", system_profile_bin()) } else { system_profile_bin() };
    u += &format!("Environment=\"PATH={path}:{}\"\n", HOST_PATH.join(":"));
    for (k, v) in &s.env {
        u += &env_line(k, v);
    }
    if let Some(d) = &s.dir {
        u += &format!("WorkingDirectory={d}\n");
    }
    let own_user = ["User", "DynamicUser"].iter().any(|k| s.raw("Service", k).is_some());
    if !user_units && !own_user {
        match s.user.as_deref() {
            Some(user) => u += &format!("User={user}\n"),
            // A user made up for the service while it runs, with /var/lib/<name> to keep its data.
            None => u += &format!("DynamicUser=yes\nStateDirectory={}\n", s.name),
        }
    }
    u += &raw_lines(s, "Service");
    // Started by its timer, not with the computer.
    if s.at.is_none() || s.systemd.contains_key("Install") {
        u += "\n[Install]\n";
        if s.raw("Install", "WantedBy").is_none() && s.at.is_none() {
            u += &format!("WantedBy={}\n", if user_units { "default.target" } else { "multi-user.target" });
        }
        u += &raw_lines(s, "Install");
    }
    u
}

/// The timer for `at:`.
pub fn timer_unit(s: &Service, source: &str) -> Option<String> {
    let when = s.at.as_ref()?;
    let mut t = header(source, &s.name);
    t += &format!("\n[Unit]\nDescription=Starts {} ({when})\n\n[Timer]\nOnCalendar={when}\nPersistent=true\n", s.name);
    t += &raw_lines(s, "Timer");
    t += "\n[Install]\nWantedBy=timers.target\n";
    Some(t)
}

/// A package unit's overrides, as a drop-in. System units always get one:
/// their commands live in /nix/store, which must be mounted first.
pub fn dropin(s: &Service, user_units: bool, source: &str) -> Option<String> {
    if user_units && !s.has_overrides() {
        return None;
    }
    let mut d = header(source, &s.name);
    d += "\n[Unit]\n";
    if let Some(desc) = &s.description {
        d += &format!("Description={desc}\n");
    }
    unit_section(s, &mut d);
    if !user_units {
        d += "RequiresMountsFor=/nix/store\n";
    }
    d += &raw_lines(s, "Unit");
    let mut svc = String::new();
    for (k, v) in &s.env {
        svc += &env_line(k, v);
    }
    if let Some(r) = &s.restart {
        svc += &format!("Restart={r}\n");
    }
    if let Some(dir) = &s.dir {
        svc += &format!("WorkingDirectory={dir}\n");
    }
    if let Some(user) = s.user.as_deref().filter(|_| !user_units) {
        svc += &format!("User={user}\n");
    }
    svc += &raw_lines(s, "Service");
    if !svc.is_empty() {
        d += "\n[Service]\n";
        d += &svc;
    }
    let install = raw_lines(s, "Install");
    if !install.is_empty() {
        d += "\n[Install]\n";
        d += &install;
    }
    Some(d)
}

/// `[Install]` of a unit file: the `.wants`/`.requires` directories enabling
/// links it. None given: the usual target for its type.
pub fn install_dirs(unit_file: &str, name: &str, user_units: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_install = false;
    for line in unit_file.lines().map(str::trim) {
        if line.starts_with('[') {
            in_install = line == "[Install]";
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let suffix = match (in_install, k.trim()) {
            (true, "WantedBy") => "wants",
            (true, "RequiredBy") => "requires",
            _ => continue,
        };
        out.extend(v.split_whitespace().map(|t| format!("{t}.{suffix}")));
    }
    if out.is_empty() {
        let target = match name.rsplit('.').next() {
            Some("timer") => "timers.target",
            Some("socket") => "sockets.target",
            Some("path") => "paths.target",
            _ if user_units => "default.target",
            _ => "multi-user.target",
        };
        out.push(format!("{target}.wants"));
    }
    out.sort();
    out.dedup();
    out
}

/// `run:` with its first word made absolute: as written if it is, else the
/// first match in `dirs` (logical paths, looked up under `root`). systemd's
/// `-@:+!` prefixes are kept.
pub fn resolve_exec(root: &Path, run: &str, dirs: &[String]) -> Result<String> {
    let run = run.trim();
    let prefix_len = run.find(|c: char| !"-@:+!".contains(c)).unwrap_or(run.len());
    let (prefix, rest) = run.split_at(prefix_len);
    let (cmd, args) = rest.split_once(char::is_whitespace).map(|(c, a)| (c, Some(a))).unwrap_or((rest, None));
    let found = if cmd.starts_with('/') {
        cmd.to_string()
    } else {
        ensure!(!cmd.contains('/'), "{cmd}: give an absolute path, or a command's name");
        dirs.iter()
            .map(|d| format!("{d}/{cmd}"))
            .find(|p| root.join(p.trim_start_matches('/')).is_file())
            .ok_or_else(|| anyhow!("{cmd} isn't in the packages, the system profile or {}; add its package, or give an absolute path", HOST_PATH.join(":")))?
    };
    Ok(match args {
        Some(a) => format!("{prefix}{found} {}", a.trim_start()),
        None => format!("{prefix}{found}"),
    })
}

/// Unsubstituted placeholders (`@out@`) in a package's unit make it unusable.
fn check_package_unit(name: &str, file: &str, text: &str) -> Result<()> {
    for line in text.lines().map(str::trim).filter(|l| l.starts_with("Exec")) {
        let Some((_, cmd)) = line.split_once('=') else { continue };
        let at: Vec<&str> = cmd.split('@').collect();
        ensure!(!(at.len() >= 3 && at[1..at.len() - 1].iter().any(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))),
            "services.{name}: {file} still has a build placeholder ({line}); the package's unit isn't usable as is, so write your own with `run:`");
        let program = cmd.trim_start_matches(|c: char| "-@:+!".contains(c)).split_whitespace().next().unwrap_or("");
        if !program.is_empty() && !program.starts_with('/') {
            crate::say!("warning: services.{name}: {file} runs {program:?} by name; systemd finds it only in /usr/bin and the like");
        }
    }
    Ok(())
}

/// Where a package keeps its units.
fn unit_dirs(user_units: bool) -> [String; 3] {
    let kind = if user_units { "user" } else { "system" };
    [format!("lib/systemd/{kind}"), format!("etc/systemd/{kind}"), format!("share/systemd/{kind}")]
}

/// Render `services` (the profile gets `packages`) into the store.
/// `source` is the nami file, for the generated files' headers.
pub fn build(store: &Store, services: &[Service], packages: &[Entry], user_units: bool, source: &str) -> Result<ServicesState> {
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    let mut units: Vec<Unit> = Vec::new();
    let add = |files: &mut BTreeMap<String, String>, rel: String, text: String| -> Result<()> {
        if let Some(old) = files.get(&rel) {
            ensure!(*old == text, "two services make {rel}");
        }
        files.insert(rel, text);
        Ok(())
    };
    // Where `run:` commands are looked for: the profile's packages first.
    let mut dirs: Vec<String> = packages.iter().flat_map(|e| e.paths.iter().flat_map(|p| [format!("{p}/bin"), format!("{p}/sbin")])).collect();
    if user_units {
        dirs.push(system_profile_bin());
    }
    dirs.extend(HOST_PATH.iter().map(|d| d.to_string()));
    for s in services {
        check_tier(s, user_units)?;
        if s.is_custom() {
            let exec = match (&s.run, &s.script) {
                (Some(run), _) => resolve_exec(store.root(), run, &dirs).with_context(|| format!("services.{}.run", s.name))?,
                (None, Some(script)) => script_exec(script),
                (None, None) => unreachable!(),
            };
            let name = format!("{}.service", s.name);
            let text = custom_unit(s, &exec, user_units, source);
            let install = install_dirs(&text, &name, user_units);
            add(&mut files, name.clone(), text)?;
            match timer_unit(s, source) {
                Some(timer) => {
                    // The timer is what's enabled and started; it starts the service.
                    let tname = format!("{}.timer", s.name);
                    let tinstall = install_dirs(&timer, &tname, user_units);
                    add(&mut files, tname.clone(), timer)?;
                    units.push(Unit { name: name.clone(), service: s.name.clone(), files: vec![name], hash: String::new(), enable: false, install, restart_on: Vec::new() });
                    units.push(Unit { name: tname.clone(), service: s.name.clone(), files: vec![tname], hash: String::new(), enable: s.enable, install: tinstall, restart_on: s.restart_on.clone() });
                }
                None => units.push(Unit { name: name.clone(), service: s.name.clone(), files: vec![name], hash: String::new(), enable: s.enable, install, restart_on: s.restart_on.clone() }),
            }
            continue;
        }
        if let Some(unit) = &s.unit {
            // An instance (getty@tty1.service) comes from its template.
            let (file, instance) = match unit.split_once('@') {
                Some((b, rest)) => {
                    let (inst, kind) = rest.rsplit_once('.').unwrap_or((rest, "service"));
                    (format!("{b}@.{kind}"), Some(inst.to_string()))
                }
                None => (unit.clone(), None),
            };
            let from: Vec<&Entry> = packages.iter().filter(|e| s.package.as_ref().is_none_or(|p| e.attr == *p)).collect();
            let file_ref = &file;
            let text = from.iter().flat_map(|e| e.paths.iter()).flat_map(|p| unit_dirs(user_units).into_iter().map(move |d| store.phys(p).join(d).join(file_ref))).find_map(|f| fs::read_to_string(f).ok());
            let Some(text) = text else {
                let whose = s.package.as_ref().map_or("none of the installed packages".to_string(), |p| format!("the package {p}"));
                bail!("services.{}: {whose} has the unit {file}", s.name);
            };
            check_package_unit(&s.name, &file, &text)?;
            let name = instance.as_ref().map_or(file.clone(), |i| file.replacen("@.", &format!("@{i}."), 1));
            let mut own = vec![file.clone()];
            add(&mut files, file.clone(), text.clone())?;
            if let Some(d) = dropin(s, user_units, source) {
                let rel = format!("{name}.d/50-nami.conf");
                add(&mut files, rel.clone(), d)?;
                own.push(rel);
            }
            let install = install_dirs(&text, &name, user_units);
            ensure!(!units.iter().any(|u| u.name == name), "services.{}: {name} is made twice", s.name);
            let restart_on = if name.ends_with(".service") { s.restart_on.clone() } else { Vec::new() };
            units.push(Unit { name, service: s.name.clone(), files: own, hash: String::new(), enable: s.enable, install, restart_on });
            continue;
        }
        let pkg = s.package_name().unwrap_or_default();
        let entry = packages.iter().find(|e| e.attr == pkg).ok_or_else(|| anyhow!("services.{}: the package {pkg} isn't installed", s.name))?;
        let (base, instance) = match s.name.split_once('@') {
            Some((b, i)) => (b.to_string(), Some(i.to_string())),
            None => (s.name.clone(), None),
        };
        let wanted: Vec<String> = match &instance {
            Some(_) => vec![format!("{base}@.service")],
            None => std::iter::once("service").chain(COMPANIONS.iter().copied()).map(|t| format!("{base}.{t}")).collect(),
        };
        let mut found = Vec::new();
        let mut shipped = BTreeSet::new();
        for p in &entry.paths {
            for d in unit_dirs(user_units) {
                let dir = store.phys(p).join(&d);
                for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
                    shipped.insert(e.file_name().to_string_lossy().into_owned());
                }
                for w in &wanted {
                    if !found.iter().any(|(n, _): &(String, String)| n == w) && dir.join(w).is_file() {
                        found.push((w.clone(), fs::read_to_string(dir.join(w))?));
                    }
                }
            }
        }
        if found.is_empty() {
            let kind = if user_units { "user" } else { "system" };
            let shipped: Vec<String> = shipped.into_iter().filter(|f| f.contains('.')).collect();
            let hint = if shipped.is_empty() {
                format!("it ships no systemd {kind} units; write your own with `run:`")
            } else {
                let templates: Vec<&String> = shipped.iter().filter(|f| f.contains("@.")).collect();
                format!("it ships {}{}", shipped.join(", "), if templates.is_empty() { String::new() } else { format!(" (a template: name an instance, like {}ada)", templates[0].split('@').next().unwrap_or("x").to_string() + "@") })
            };
            bail!("services.{}: {pkg} has no {} {kind} unit; {hint}", s.name, wanted[0]);
        }
        for (file, text) in &found {
            check_package_unit(&s.name, file, text)?;
            let name = match &instance {
                Some(i) => file.replacen("@.", &format!("@{i}."), 1),
                None => file.clone(),
            };
            let mut own = vec![file.clone()];
            add(&mut files, file.clone(), text.clone())?;
            if let Some(d) = dropin(s, user_units, source) {
                let rel = format!("{name}.d/50-nami.conf");
                add(&mut files, rel.clone(), d)?;
                own.push(rel);
            }
            let install = install_dirs(text, &name, user_units);
            ensure!(!units.iter().any(|u| u.name == name), "services.{}: {name} is made twice", s.name);
            // A socket or timer goes with its service; the service restarts.
            let restart_on = if name.ends_with(".service") { s.restart_on.clone() } else { Vec::new() };
            units.push(Unit { name, service: s.name.clone(), files: own, hash: String::new(), enable: s.enable, install, restart_on });
        }
    }

    let dir = store.cache_dir().join(format!("units-build-{}", std::process::id()));
    if dir.symlink_metadata().is_ok() {
        crate::store::remove_tree(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    let result = (|| {
        let mut hashes = BTreeMap::new();
        for (rel, text) in &files {
            let dest = dir.join(rel);
            fs::create_dir_all(dest.parent().unwrap())?;
            fs::write(&dest, text)?;
            hashes.insert(rel.clone(), crate::nar::hash_path(&dest)?.0);
        }
        let path = store.add_path(&dir, "nami-units")?;
        let units = units
            .into_iter()
            .map(|mut u| {
                u.hash = unit_hash(&u.files, &hashes);
                u
            })
            .collect();
        Ok(ServicesState { path, user: user_units, files: hashes, units })
    })();
    crate::store::remove_tree(&dir)?;
    result
}

fn unit_hash(files: &[String], hashes: &BTreeMap<String, String>) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for f in files {
        h.update(f.as_bytes());
        h.update(b"\0");
        h.update(hashes.get(f).map(String::as_str).unwrap_or("").as_bytes());
        h.update(b"\n");
    }
    format!("{:x}", h.finalize())
}

/// What's installed (the last activation): files nami wrote and the units.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Installed {
    pub files: BTreeMap<String, String>,
    pub units: BTreeMap<String, InstalledUnit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstalledUnit {
    pub hash: String,
    pub enable: bool,
    /// Enabling links nami made, relative to the unit directory.
    #[serde(default)]
    pub links: Vec<String>,
}

/// What switching from `old` to `new` does to the services.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub stop: Vec<String>,
    pub start: Vec<String>,
    pub restart: Vec<String>,
    /// Files written / deleted (relative to the unit directory).
    pub write: Vec<String>,
    pub delete: Vec<String>,
    /// Enabling links to drop / make.
    pub unlink: Vec<String>,
    pub link: Vec<(String, String)>,
    /// Restarted because a `restart-on:` file changed: unit -> the file.
    pub because: BTreeMap<String, String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        *self == Plan::default()
    }

    /// Lines for `--dry-run` and the apply report.
    pub fn describe(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.extend(self.stop.iter().map(|u| format!("stop {u}")));
        out.extend(self.write.iter().map(|f| format!("write {f}")));
        out.extend(self.delete.iter().map(|f| format!("delete {f}")));
        out.extend(self.start.iter().map(|u| format!("start {u}")));
        out.extend(self.restart.iter().map(|u| match self.because.get(u) {
            Some(f) => format!("restart {u} ({f} changed)"),
            None => format!("restart {u}"),
        }));
        out
    }
}

/// The wanted enabling links of a unit: `multi-user.target.wants/myapp.service`
/// -> the unit's (main) file.
fn links_of(u: &Unit) -> Vec<(String, String)> {
    if !u.enable {
        return Vec::new();
    }
    let file = u.files.first().cloned().unwrap_or_else(|| u.name.clone());
    u.install.iter().map(|d| (format!("{d}/{}", u.name), file.clone())).collect()
}

/// Whether a change to `changed` (an /etc file or folder nami writes) is one
/// to `watched` (a `restart-on:` path): the same, or one inside the other.
fn touches(watched: &str, changed: &str) -> bool {
    let w = watched.trim_end_matches('/');
    changed == w || changed.starts_with(&format!("{w}/")) || w.starts_with(&format!("{changed}/"))
}

/// The diff, old generation (as installed) -> new: stop what goes away or
/// is turned off (before its file goes), start what's new or turned on,
/// restart what changed: its unit or drop-in, or a `restart-on:` file among
/// `etc_changed` (the /etc paths this activation writes or removes, which
/// happens before units start or restart).
pub fn plan(old: &Installed, new: Option<&ServicesState>, etc_changed: &[String]) -> Plan {
    let mut p = Plan::default();
    let empty = BTreeMap::new();
    let new_files = new.map(|n| &n.files).unwrap_or(&empty);
    let new_units: BTreeMap<&str, &Unit> = new.map(|n| n.units.iter().map(|u| (u.name.as_str(), u)).collect()).unwrap_or_default();
    for (name, was) in &old.units {
        match new_units.get(name.as_str()) {
            None if was.enable => p.stop.push(name.clone()),
            Some(u) if was.enable && !u.enable => p.stop.push(name.clone()),
            _ => {}
        }
    }
    for (name, u) in &new_units {
        match old.units.get(*name) {
            None if u.enable => p.start.push(name.to_string()),
            Some(was) if u.enable && !was.enable => p.start.push(name.to_string()),
            Some(was) if u.enable && was.hash != u.hash => p.restart.push(name.to_string()),
            Some(_) if u.enable => {
                if let Some(f) = etc_changed.iter().find(|c| u.restart_on.iter().any(|w| touches(w, c))) {
                    p.restart.push(name.to_string());
                    p.because.insert(name.to_string(), f.clone());
                }
            }
            _ => {}
        }
    }
    p.write = new_files.iter().filter(|(f, h)| old.files.get(*f) != Some(h)).map(|(f, _)| f.clone()).collect();
    p.delete = old.files.keys().filter(|f| !new_files.contains_key(*f)).cloned().collect();
    let wanted: BTreeSet<(String, String)> = new_units.values().flat_map(|u| links_of(u)).collect();
    let had: BTreeSet<&String> = old.units.values().flat_map(|u| &u.links).collect();
    p.unlink = had.iter().filter(|l| !wanted.iter().any(|(w, _)| w == **l)).map(|l| l.to_string()).collect();
    p.link = wanted.into_iter().filter(|(l, _)| !had.contains(l)).collect();
    p
}

/// What can run units here.
#[derive(Debug, Clone, PartialEq)]
pub enum Backend {
    /// systemd is running: units are installed, and started.
    Live,
    /// A scratch root (NAMI_ROOT): unit files and links only.
    Offline,
    /// Services can't work here; says why.
    Unsupported(String),
}

pub fn backend(root: &Path) -> Backend {
    if root != Path::new("/") {
        return Backend::Offline;
    }
    if root.join("run/systemd/system").is_dir() {
        return Backend::Live;
    }
    let exists = |p: &str| root.join(p).exists();
    let what = if exists("run/openrc") || exists("sbin/openrc-run") {
        "this system uses OpenRC"
    } else if exists("run/runit") || exists("etc/runit/runsvdir") {
        "this system uses runit"
    } else if exists("run/s6") || exists("run/s6-rc") {
        "this system uses s6"
    } else {
        "systemd isn't running here (a container without an init system?)"
    };
    Backend::Unsupported(format!("services need systemd, and {what}: not supported yet (files: still work)"))
}

/// Where a scope's units go.
pub struct Target {
    /// On disk (under NAMI_ROOT).
    pub dir: PathBuf,
    /// As systemd sees it: enabling links point here.
    pub logical: String,
    pub state_file: PathBuf,
    pub user: bool,
    pub live: bool,
    /// NAMI_ROOT (or /), under which `vendor` is looked up.
    pub root: PathBuf,
    /// Where the distro (or an admin) keeps units of this scope that ours
    /// override, as systemd sees them.
    pub vendor: Vec<String>,
}

/// Unit directories below ours in systemd's search path.
const SYSTEM_VENDOR: &[&str] = &["/usr/local/lib/systemd/system", "/usr/lib/systemd/system", "/lib/systemd/system"];
const USER_VENDOR: &[&str] = &["/etc/systemd/user", "/usr/local/lib/systemd/user", "/usr/lib/systemd/user", "/lib/systemd/user"];

/// Where the computer's own units go, as systemd sees it, on the computer
/// at `root`: /etc/systemd/system, but on NixOS (not namiOS: it has no
/// /etc/NIXOS) /etc/systemd/system.control, the folder NixOS leaves for
/// local units: its /etc/systemd/system is its configuration's, read-only.
/// (packaging/setup.sh puts nami's daemon there too.)
pub fn local_units(root: &Path) -> &'static str {
    if root.join("etc/NIXOS").exists() { "/etc/systemd/system.control" } else { "/etc/systemd/system" }
}

impl Target {
    pub fn system(root: &Path, var_dir: &Path) -> Target {
        let logical = local_units(root);
        let mut vendor: Vec<String> = SYSTEM_VENDOR.iter().map(|d| d.to_string()).collect();
        if logical != "/etc/systemd/system" {
            // (NixOS's own units: below ours in systemd's search path.)
            vendor.insert(0, "/etc/systemd/system".into());
        }
        Target {
            dir: root.join(logical.trim_start_matches('/')),
            logical: logical.into(),
            state_file: var_dir.join("units.json"),
            user: false,
            live: backend(root) == Backend::Live,
            root: root.to_path_buf(),
            vendor,
        }
    }

    pub fn user(root: &Path, home: &Path) -> Target {
        let dir = home.join(".config/systemd/user");
        Target {
            logical: dir.display().to_string(),
            dir,
            state_file: home.join(".local/state/nami/units.json"),
            user: true,
            live: backend(root) == Backend::Live,
            root: root.to_path_buf(),
            vendor: USER_VENDOR.iter().map(|d| d.to_string()).collect(),
        }
    }

    pub fn installed(&self) -> Installed {
        fs::read(&self.state_file).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
    }

    fn systemctl(&self, args: &[&str]) -> Result<()> {
        let mut cmd = systemctl();
        if self.user {
            cmd.arg("--user");
        }
        let out = cmd.args(args).output().context("running systemctl")?;
        ensure!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr).trim());
        Ok(())
    }
}

/// Activation, in two halves around the /etc files: `stop` first (units
/// going away still have their files), then `finish`: files, links,
/// daemon-reload, start and restart.
pub struct Activation<'a> {
    target: Target,
    link_base: PathBuf,
    new: Option<&'a ServicesState>,
    old: Installed,
    pub plan: Plan,
}

impl<'a> Activation<'a> {
    /// `etc_changed`: the /etc paths written or removed before the units
    /// start (for `restart-on:`).
    pub fn new(target: Target, link_base: &Path, new: Option<&'a ServicesState>, etc_changed: &[String]) -> Activation<'a> {
        let old = target.installed();
        let plan = plan(&old, new, etc_changed);
        Activation { target, link_base: link_base.to_path_buf(), new, old, plan }
    }

    /// Warnings for units nami is about to install for the first time that
    /// are already there: a distro's (or an admin's) copy further down
    /// systemd's search path, which ours overrides, or a file in our own
    /// directory that isn't nami's, which is moved aside. Not errors: taking
    /// over a unit is often the point, but it shouldn't happen unnoticed.
    pub fn overrides(&self) -> Vec<String> {
        let t = &self.target;
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        for u in self.new.map(|n| n.units.as_slice()).unwrap_or(&[]) {
            if self.old.units.contains_key(&u.name) {
                continue;
            }
            for f in u.files.iter().filter(|f| !f.contains('/')) {
                if !seen.insert(f.clone()) {
                    continue;
                }
                if t.dir.join(f).symlink_metadata().is_ok() && !self.old.files.contains_key(f) {
                    out.push(format!("warning: services.{}: {}/{f} is already here and isn't nami's; it's moved to {f}.nami-backup (and comes back without services.{})", u.service, t.logical, u.service));
                }
                let mut real = BTreeSet::new();
                for d in &t.vendor {
                    let p = t.root.join(d.trim_start_matches('/')).join(f);
                    // /lib is often /usr/lib: say it once.
                    if p.is_file() && real.insert(fs::canonicalize(&p).unwrap_or(p)) {
                        out.push(format!("warning: services.{}: nami's {}/{f} overrides {d}/{f}, which is already here (it's used again without services.{})", u.service, t.logical, u.service));
                    }
                }
            }
        }
        out
    }

    fn run(&self, verb: &str, units: &[String]) {
        if units.is_empty() || !self.target.live {
            return;
        }
        let mut args = vec![verb];
        args.extend(units.iter().map(String::as_str));
        if let Err(e) = self.target.systemctl(&args) {
            let how = if self.target.user { "systemctl --user status" } else { "systemctl status" };
            crate::say!("warning: couldn't {verb} {}: {e} (see `{how} {}`)", units.join(" "), units[0]);
        }
    }

    pub fn stop(&self) {
        self.run("stop", &self.plan.stop);
    }

    pub fn finish(self) -> Result<Vec<String>> {
        let (target, plan, old) = (&self.target, &self.plan, &self.old);
        let mut said = Vec::new();
        if self.new.is_none() && *old == Installed::default() {
            return Ok(said); // never had services: leave no trace
        }
        let empty = BTreeMap::new();
        let new_files = self.new.map(|n| &n.files).unwrap_or(&empty);
        fs::create_dir_all(&target.dir).with_context(|| format!("creating {}", target.dir.display()))?;
        let mut changed = !plan.is_empty();
        for l in &plan.unlink {
            let p = target.dir.join(l);
            if fs::read_link(&p).is_ok() {
                fs::remove_file(&p)?;
            }
        }
        for (rel, recorded) in &old.files {
            if new_files.contains_key(rel) {
                continue;
            }
            let dest = target.dir.join(rel);
            match crate::etc::hash(&dest) {
                Some(h) if &h == recorded => crate::etc::remove(&dest)?,
                Some(_) => said.push(format!("kept {}: changed since nami wrote it", dest.display())),
                None => {}
            }
            crate::etc::restore_backup(&dest, &mut said)?;
            if let Some(parent) = dest.parent().filter(|p| *p != target.dir) {
                let _ = fs::remove_dir(parent); // an emptied `<unit>.d`
            }
        }
        for (rel, wanted) in new_files {
            // Also puts back a unit file someone edited (moving theirs aside).
            changed |= crate::etc::place(&self.link_base.join(rel), &target.dir.join(rel), wanted, old.files.get(rel).map(String::as_str), None, &mut said)?;
        }
        let mut units = BTreeMap::new();
        for u in self.new.map(|n| n.units.as_slice()).unwrap_or(&[]) {
            let mut links = Vec::new();
            for (link, file) in links_of(u) {
                let p = target.dir.join(&link);
                let to = format!("{}/{file}", target.logical);
                let had = old.units.get(&u.name).is_some_and(|w| w.links.contains(&link));
                match fs::read_link(&p) {
                    Ok(t) if t == Path::new(&to) => links.push(link),
                    _ if p.symlink_metadata().is_ok() && !had => {} // someone else's (`systemctl enable`): leave it
                    _ => {
                        if p.symlink_metadata().is_ok() {
                            fs::remove_file(&p)?;
                        }
                        fs::create_dir_all(p.parent().unwrap())?;
                        symlink(&to, &p)?;
                        links.push(link);
                    }
                }
            }
            units.insert(u.name.clone(), InstalledUnit { hash: u.hash.clone(), enable: u.enable, links });
        }
        for l in &plan.unlink {
            if let Some(parent) = target.dir.join(l).parent() {
                let _ = fs::remove_dir(parent); // an emptied `.wants`
            }
        }
        if let Some(parent) = target.state_file.parent() {
            fs::create_dir_all(parent)?;
        }
        write_atomic(&target.state_file, &serde_json::to_vec_pretty(&Installed { files: new_files.clone(), units })?)?;

        if target.live && changed {
            if let Err(e) = target.systemctl(&["daemon-reload"]) {
                let hint = if target.user { " (log in again, or `loginctl enable-linger` to run your services without a session)" } else { "" };
                said.push(format!("warning: systemctl daemon-reload failed: {e}{hint}; the units are installed, nothing was started"));
                return Ok(said);
            }
        }
        self.run("start", &plan.start);
        self.run("restart", &plan.restart);
        let scope = if target.user { " (yours)" } else { "" };
        let would = if target.live { "" } else { "would have " }; // offline: nothing runs
        for (verb, units) in [("stopped", &plan.stop), ("started", &plan.start), ("restarted", &plan.restart)] {
            said.extend(units.iter().map(|u| format!("{would}{verb} {u}{scope}")));
        }
        Ok(said)
    }
}

/// systemctl: the computer's own when nami made it (units like the first
/// login's run with a PATH of nami's tools alone), else the PATH's.
pub fn systemctl() -> std::process::Command {
    let sw = std::path::Path::new("/run/current-system/sw/bin/systemctl");
    std::process::Command::new(if sw.exists() { sw } else { std::path::Path::new("systemctl") })
}

/// `active`, `failed`, `inactive`, ... per unit (`unknown` without systemd).
pub fn states(target: &Target, units: &[String]) -> Vec<String> {
    if !target.live || units.is_empty() {
        return units.iter().map(|_| "unknown".to_string()).collect();
    }
    let mut cmd = systemctl();
    if target.user {
        cmd.arg("--user");
    }
    let out = cmd.arg("is-active").args(units).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let mut lines: Vec<String> = out.lines().map(String::from).collect();
    lines.resize(units.len(), "unknown".into());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn y(s: &str) -> Value {
        serde_yaml::from_str(s).unwrap()
    }

    #[test]
    fn parses_and_checks_entries() {
        let s = parse("myapp", &y("run: myapp --port 8080\nenv: { RUST_LOG: info, N: 3 }\nafter: network-online.target\nuser: myapp")).unwrap();
        assert_eq!(s.run.as_deref(), Some("myapp --port 8080"));
        assert_eq!(s.env["N"], "3");
        assert_eq!(s.after, vec!["network-online.target"]);
        assert_eq!(s.package_name(), None);
        let p = parse("syncthing@ada", &y("true")).unwrap();
        assert_eq!((p.name.as_str(), p.package_name().as_deref(), p.enable), ("syncthing@ada", Some("syncthing"), true));
        // Named whole: that unit, from whichever package brings it.
        let p = parse("pipewire-pulse.socket", &y("true")).unwrap();
        assert_eq!((p.unit.as_deref(), p.package_name()), (Some("pipewire-pulse.socket"), None));
        assert!(parse("pipewire.socket", &y("per-person: true")).unwrap_err().to_string().contains("for system.nami"));
        assert_eq!(parse("tailscaled", &y("package: tailscale")).unwrap().package_name().as_deref(), Some("tailscale"));
        assert!(!parse("x", &y("false")).unwrap().enable);
        for (k, v, says) in [
            ("x", "restart: sometimes", "restart"),
            ("x", "colour: red", "unknown key `colour`"),
            ("x", "run: a\npackage: b", "not both"),
            ("x@y", "run: a", "instances"),
            ("bad name", "true", "valid unit name"),
            ("x", "env: { 2X: a }", "variable name"),
            ("x", "~", "say `true`"),
            ("x", "dir: relative", "absolute"),
            ("x@", "true", "instance"),
        ] {
            let e = parse(k, &y(v)).unwrap_err().to_string();
            assert!(e.contains(says), "{k}: {v} -> {e}");
        }
        assert!(check_tier(&s, true).is_err());
        assert!(check_tier(&s, false).is_ok());
    }

    #[test]
    fn renders_your_own_service() {
        let s = parse("myapp", &y("run: x\nenv: { MSG: 'say \"hi\" 100%' }\nafter: [network-online.target]")).unwrap();
        let u = custom_unit(&s, "/opt/myapp/bin/server --port 8080", false, "/etc/nami/system.nami");
        assert!(u.starts_with("# Generated by nami from /etc/nami/system.nami (services.myapp)"));
        for line in ["ExecStart=/opt/myapp/bin/server --port 8080", "After=network-online.target", "Wants=network-online.target", "Restart=on-failure",
            "Environment=\"MSG=say \\\"hi\\\" 100%%\"", "DynamicUser=yes", "StateDirectory=myapp", "RequiresMountsFor=/nix/store", "WantedBy=multi-user.target"] {
            assert!(u.lines().any(|l| l == line), "missing {line:?} in\n{u}");
        }
        let mine = custom_unit(&parse("x", &y("run: x\nrestart: always")).unwrap(), "/bin/x", true, "f");
        assert!(mine.contains("WantedBy=default.target") && mine.contains("Restart=always") && !mine.contains("DynamicUser") && !mine.contains("RequiresMountsFor"));
        assert_eq!(install_dirs(&u, "myapp.service", false), vec!["multi-user.target.wants"]);
    }

    #[test]
    fn scripts_timers_and_systemd_keys() {
        let s = parse("backup", &y("script: |\n  echo \"$HOME\" 100% \\ done\n  restic backup /home\nat: daily\nsystemd:\n  Service: { Type: oneshot, Nice: 10, ProtectHome: true, ExecStartPre: [a, b] }\n  Unit: { Before: sleep.target }")).unwrap();
        assert!(s.is_custom() && s.package_name().is_none());
        let exec = script_exec(s.script.as_deref().unwrap());
        // What systemd hands bash, after its own unquoting.
        let inner = exec.strip_prefix("/usr/bin/env bash -e -c \"").and_then(|x| x.strip_suffix('"')).unwrap();
        let (mut back, mut chars) = (String::new(), inner.chars().peekable());
        while let Some(c) = chars.next() {
            match (c, chars.peek().copied()) {
                ('\\', Some('n')) => { back.push('\n'); chars.next(); }
                ('\\', Some(x)) => { back.push(x); chars.next(); }
                ('$', Some('$')) | ('%', Some('%')) => { back.push(c); chars.next(); }
                _ => back.push(c),
            }
        }
        assert_eq!(back, "echo \"$HOME\" 100% \\ done\nrestic backup /home\n");
        let u = custom_unit(&s, &exec, false, "f");
        for line in ["Type=oneshot", "Nice=10", "ProtectHome=yes", "ExecStartPre=a", "ExecStartPre=b", "Before=sleep.target", "DynamicUser=yes"] {
            assert!(u.lines().any(|l| l == line), "missing {line:?} in\n{u}");
        }
        assert!(!u.contains("Restart=") && !u.contains("[Install]"), "a timer's oneshot: not restarted, not enabled itself\n{u}");
        let t = timer_unit(&s, "f").unwrap();
        assert!(t.contains("OnCalendar=daily") && t.contains("WantedBy=timers.target"), "{t}");
        // Your own User= wins over a made-up one; Install keys replace the default target.
        let own = parse("x", &y("run: x\nsystemd: { Service: { User: nobody }, Install: { WantedBy: graphical.target } }")).unwrap();
        let u = custom_unit(&own, "/bin/x", false, "f");
        assert!(!u.contains("DynamicUser") && u.contains("User=nobody") && u.contains("WantedBy=graphical.target") && !u.contains("multi-user"), "{u}");
        for (v, says) in [("script: a\nrun: b", "not both"), ("package: x\nat: daily", "your own"), ("run: x\nsystemd: { Service: { ExecStart: y } }", "run:"), ("run: x\nsystemd: { Mount: { What: y } }", "one of")] {
            let e = parse("x", &y(v)).unwrap_err().to_string();
            assert!(e.contains(says), "{v} -> {e}");
        }
    }

    /// NAMI_UNIT_DUMP=<dir> cargo test dump_units -- --ignored: units for
    /// `systemd-analyze verify` and a real run (`systemctl --user link`).
    #[test]
    #[ignore]
    fn dump_units() {
        let dir = PathBuf::from(std::env::var("NAMI_UNIT_DUMP").expect("NAMI_UNIT_DUMP"));
        fs::create_dir_all(&dir).unwrap();
        let script = parse("namitest-script", &y("script: |\n  echo \"home is $HOME, 100% \\\"quoted\\\" \\\\ back\"\n  for i in 1 2; do echo \"line $i\"; done\nsystemd:\n  Service: { Type: oneshot, RemainAfterExit: true }")).unwrap();
        fs::write(dir.join("namitest-script.service"), custom_unit(&script, &script_exec(script.script.as_deref().unwrap()), true, "test")).unwrap();
        let timer = parse("namitest-timer", &y("run: /usr/bin/true\nat: '*-*-* 04:00'")).unwrap();
        fs::write(dir.join("namitest-timer.service"), custom_unit(&timer, "/usr/bin/true", true, "test")).unwrap();
        fs::write(dir.join("namitest-timer.timer"), timer_unit(&timer, "test").unwrap()).unwrap();
    }

    #[test]
    fn package_units_and_dropins() {
        let plain = parse("syncthing", &y("true")).unwrap();
        assert_eq!(dropin(&plain, true, "f"), None);
        assert!(dropin(&plain, false, "f").unwrap().contains("RequiresMountsFor=/nix/store"));
        let over = parse("syncthing", &y("env: { A: b }\nuser: ada\nrestart: always")).unwrap();
        let d = dropin(&over, false, "f").unwrap();
        assert!(d.contains("[Service]\nEnvironment=\"A=b\"\nRestart=always\nUser=ada\n"), "{d}");
        let unit = "[Unit]\nDescription=x\n[Service]\nExecStart=/nix/store/abc-x/bin/x\n[Install]\nWantedBy=multi-user.target graphical.target\nRequiredBy=foo.target\n";
        assert_eq!(install_dirs(unit, "x.service", false), vec!["foo.target.requires", "graphical.target.wants", "multi-user.target.wants"]);
        assert_eq!(install_dirs("[Service]\nExecStart=/bin/x\n", "x.timer", false), vec!["timers.target.wants"]);
        assert!(check_package_unit("x", "x.service", "[Service]\nExecStart=@out@/bin/x\n").is_err());
        assert!(check_package_unit("x", "x.service", "[Service]\nExecStart=/nix/store/a-x/bin/x -u %i@host\n").is_ok());
    }

    #[test]
    fn resolves_commands() {
        let root = std::env::temp_dir().join(format!("nami-exec-{}", std::process::id()));
        fs::create_dir_all(root.join("nix/store/abc-hello/bin")).unwrap();
        fs::write(root.join("nix/store/abc-hello/bin/hello"), "").unwrap();
        let dirs = vec!["/nix/store/abc-hello/bin".to_string(), "/usr/bin".to_string()];
        assert_eq!(resolve_exec(&root, "hello -g hi", &dirs).unwrap(), "/nix/store/abc-hello/bin/hello -g hi");
        assert_eq!(resolve_exec(&root, "-/opt/x", &dirs).unwrap(), "-/opt/x");
        assert!(resolve_exec(&root, "nope", &dirs).unwrap_err().to_string().contains("nope isn't in"));
        assert!(resolve_exec(&root, "bin/x", &dirs).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    fn state(units: &[(&str, &str, bool)]) -> ServicesState {
        ServicesState {
            path: "/nix/store/x-nami-units".into(),
            user: false,
            files: units.iter().map(|(n, h, _)| (n.to_string(), h.to_string())).collect(),
            units: units.iter().map(|(n, h, e)| Unit { name: n.to_string(), service: n.to_string(), files: vec![n.to_string()], hash: h.to_string(), enable: *e, install: vec!["multi-user.target.wants".into()], restart_on: Vec::new() }).collect(),
        }
    }

    fn installed(s: &ServicesState) -> Installed {
        Installed {
            files: s.files.clone(),
            units: s.units.iter().map(|u| (u.name.clone(), InstalledUnit { hash: u.hash.clone(), enable: u.enable, links: links_of(u).into_iter().map(|l| l.0).collect() })).collect(),
        }
    }

    #[test]
    fn plans_from_the_diff() {
        let first = state(&[("a.service", "1", true), ("b.service", "1", true), ("c.service", "1", false)]);
        let p = plan(&Installed::default(), Some(&first), &[]);
        assert_eq!(p.start, vec!["a.service", "b.service"]);
        assert_eq!(p.write.len(), 3);
        assert_eq!(p.link.len(), 2);
        // a changed, b removed, c turned on, d new
        let second = state(&[("a.service", "2", true), ("c.service", "1", true), ("d.service", "1", true)]);
        let p = plan(&installed(&first), Some(&second), &[]);
        assert_eq!(p.stop, vec!["b.service"]);
        assert_eq!(p.restart, vec!["a.service"]);
        assert_eq!(p.start, vec!["c.service", "d.service"]);
        assert_eq!(p.write, vec!["a.service", "d.service"]);
        assert_eq!(p.delete, vec!["b.service"]);
        assert_eq!(p.unlink, vec!["multi-user.target.wants/b.service"]);
        // undo: exactly the reverse
        let back = plan(&installed(&second), Some(&first), &[]);
        assert_eq!(back.stop, vec!["c.service", "d.service"]);
        assert_eq!(back.start, vec!["b.service"]);
        assert_eq!(back.restart, vec!["a.service"]);
        // nothing changed: nothing to do
        assert!(plan(&installed(&second), Some(&second), &[]).is_empty());
        // all gone
        let none = plan(&installed(&second), None, &[]);
        assert_eq!(none.stop, vec!["a.service", "c.service", "d.service"]);
        assert_eq!(none.delete.len(), 3);
    }

    #[test]
    fn activates_into_a_unit_dir() {
        let tmp = std::env::temp_dir().join(format!("nami-units-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let gen = |n: u32, text: &str| -> (PathBuf, ServicesState) {
            let base = tmp.join(format!("gen{n}"));
            fs::create_dir_all(&base).unwrap();
            fs::write(base.join("a.service"), text).unwrap();
            let h = crate::nar::hash_path(&base.join("a.service")).unwrap().0;
            let mut s = state(&[("a.service", &h, true)]);
            s.units[0].hash = h;
            (base, s)
        };
        let target = || Target { dir: tmp.join("etc/systemd/system"), logical: "/etc/systemd/system".into(), state_file: tmp.join("units.json"), user: false, live: false, root: tmp.clone(), vendor: Vec::new() };
        fs::create_dir_all(tmp.join("etc/systemd/system")).unwrap();
        fs::write(tmp.join("etc/systemd/system/a.service"), "mine").unwrap(); // in the way
        let (b1, s1) = gen(1, "one");
        let a = Activation::new(target(), &b1, Some(&s1), &[]);
        assert_eq!(a.plan.start, vec!["a.service"]);
        a.finish().unwrap();
        let unit = tmp.join("etc/systemd/system/a.service");
        assert_eq!(fs::read_to_string(&unit).unwrap(), "one");
        assert_eq!(fs::read_to_string(tmp.join("etc/systemd/system/a.service.nami-backup")).unwrap(), "mine");
        let link = tmp.join("etc/systemd/system/multi-user.target.wants/a.service");
        assert_eq!(fs::read_link(&link).unwrap(), Path::new("/etc/systemd/system/a.service"));
        let (b2, s2) = gen(2, "two");
        let a = Activation::new(target(), &b2, Some(&s2), &[]);
        assert_eq!(a.plan.restart, vec!["a.service"]);
        a.finish().unwrap();
        assert_eq!(fs::read_to_string(&unit).unwrap(), "two");
        let a = Activation::new(target(), &b2, None, &[]);
        assert_eq!(a.plan.stop, vec!["a.service"]);
        a.finish().unwrap();
        assert_eq!(fs::read_to_string(&unit).unwrap(), "mine", "the file that was there is back");
        assert!(link.symlink_metadata().is_err());
        assert!(!tmp.join("etc/systemd/system/multi-user.target.wants").exists());
        fs::remove_dir_all(&tmp).unwrap();
    }

    /// NixOS: the computer's services go in /etc/systemd/system.control (its
    /// /etc/systemd/system is read-only), linked there; NixOS's own units
    /// are below them.
    #[test]
    fn on_nixos_units_go_in_system_control() {
        let tmp = std::env::temp_dir().join(format!("nami-units-nixos-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("etc/systemd/system")).unwrap();
        fs::create_dir_all(tmp.join("var")).unwrap();
        assert_eq!(local_units(&tmp), "/etc/systemd/system");
        fs::write(tmp.join("etc/NIXOS"), "").unwrap();
        assert_eq!(local_units(&tmp), "/etc/systemd/system.control");
        let target = Target::system(&tmp, &tmp.join("var"));
        assert_eq!((target.dir.clone(), target.logical.as_str()), (tmp.join("etc/systemd/system.control"), "/etc/systemd/system.control"));
        assert_eq!(target.vendor[0], "/etc/systemd/system");
        // NixOS's own sshd.service: ours overrides it, and says so.
        fs::write(tmp.join("etc/systemd/system/sshd.service"), "nixos").unwrap();
        let base = tmp.join("gen1");
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("sshd.service"), "ours").unwrap();
        let mut st = state(&[("sshd.service", "1", true)]);
        st.units[0].hash = crate::nar::hash_path(&base.join("sshd.service")).unwrap().0;
        let a = Activation::new(target, &base, Some(&st), &[]);
        assert!(a.overrides().iter().any(|l| l.contains("/etc/systemd/system.control/sshd.service overrides /etc/systemd/system/sshd.service")), "{:?}", a.overrides());
        a.finish().unwrap();
        assert_eq!(fs::read_to_string(tmp.join("etc/systemd/system.control/sshd.service")).unwrap(), "ours");
        assert_eq!(fs::read_link(tmp.join("etc/systemd/system.control/multi-user.target.wants/sshd.service")).unwrap(), Path::new("/etc/systemd/system.control/sshd.service"));
        assert_eq!(fs::read_to_string(tmp.join("etc/systemd/system/sshd.service")).unwrap(), "nixos");
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn restarts_when_a_watched_file_changes() {
        let s = parse("myapp", &y("run: x\nrestart-on: [/etc/myapp/config.toml, /etc/certs/]")).unwrap();
        assert_eq!(s.restart_on, vec!["/etc/myapp/config.toml", "/etc/certs/"]);
        assert_eq!(parse("x", &y("restart-on: /etc/x.conf")).unwrap().restart_on, vec!["/etc/x.conf"]);
        assert!(parse("x", &y("restart-on: etc/x.conf")).unwrap_err().to_string().contains("absolute"));
        assert!(parse("x", &y("restart-on: /etc/../x")).is_err());
        let mut st = state(&[("a.service", "1", true), ("b.service", "1", true), ("off.service", "1", false)]);
        st.units[0].restart_on = vec!["/etc/myapp/config.toml".into()];
        st.units[1].restart_on = vec!["/etc/certs/".into()];
        st.units[2].restart_on = vec!["/etc/myapp/config.toml".into()];
        let was = installed(&st);
        // Nothing of theirs changed: nothing restarts.
        assert!(plan(&was, Some(&st), &["/etc/other.conf".into()]).is_empty());
        // The file, a file in the watched folder: restarted, after the files are written.
        let p = plan(&was, Some(&st), &["/etc/myapp/config.toml".into(), "/etc/certs/site.pem".into()]);
        assert_eq!(p.restart, vec!["a.service", "b.service"], "off.service is off: it isn't started for it");
        assert_eq!(p.because["a.service"], "/etc/myapp/config.toml");
        assert!(p.describe().contains(&"restart b.service (/etc/certs/site.pem changed)".to_string()), "{:?}", p.describe());
        // A watched file inside a managed folder that changed.
        assert_eq!(plan(&was, Some(&st), &["/etc/myapp".into()]).restart, vec!["a.service"]);
        // New or changed units start or restart anyway, once.
        let mut changed = st.clone();
        changed.units[0].hash = "2".into();
        let p = plan(&was, Some(&changed), &["/etc/myapp/config.toml".into()]);
        assert_eq!(p.restart, vec!["a.service"]);
        assert!(p.because.is_empty(), "restarted for its own change");
        let p = plan(&Installed::default(), Some(&st), &["/etc/myapp/config.toml".into()]);
        assert_eq!((p.start.len(), p.restart.len()), (2, 0));
    }

    #[test]
    fn warns_about_units_already_there() {
        let tmp = std::env::temp_dir().join(format!("nami-override-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        for d in ["usr/lib/systemd/system", "etc/systemd/system"] {
            fs::create_dir_all(tmp.join(d)).unwrap();
        }
        std::os::unix::fs::symlink("usr/lib", tmp.join("lib")).unwrap(); // merged /usr: said once
        fs::write(tmp.join("usr/lib/systemd/system/sshd.service"), "distro").unwrap();
        fs::write(tmp.join("etc/systemd/system/myapp.service"), "by hand").unwrap();
        let target = Target::system(&tmp, &tmp.join("var"));
        assert!(!target.live);
        let st = state(&[("sshd.service", "1", true), ("myapp.service", "1", true), ("fresh.service", "1", true)]);
        let a = Activation::new(Target::system(&tmp, &tmp.join("var")), &tmp, Some(&st), &[]);
        let w = a.overrides();
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w.iter().any(|l| l.contains("services.sshd.service: nami's /etc/systemd/system/sshd.service overrides /usr/lib/systemd/system/sshd.service")), "{w:?}");
        assert!(w.iter().any(|l| l.contains("/etc/systemd/system/myapp.service is already here and isn't nami's")), "{w:?}");
        // Once nami has installed them, it's said no more.
        fs::create_dir_all(tmp.join("var")).unwrap();
        fs::write(tmp.join("var/units.json"), serde_json::to_vec(&installed(&st)).unwrap()).unwrap();
        assert!(Activation::new(Target::system(&tmp, &tmp.join("var")), &tmp, Some(&st), &[]).overrides().is_empty());
        // Yours: the distro's user units, and the admin's in /etc/systemd/user.
        let home = tmp.join("home");
        fs::create_dir_all(tmp.join("etc/systemd/user")).unwrap();
        fs::write(tmp.join("etc/systemd/user/syncthing.service"), "admin").unwrap();
        let mine = state(&[("syncthing.service", "1", true)]);
        let w = Activation::new(Target::user(&tmp, &home), &tmp, Some(&mine), &[]).overrides();
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("overrides /etc/systemd/user/syncthing.service"), "{w:?}");
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn backends() {
        assert_eq!(backend(Path::new("/tmp/scratch")), Backend::Offline);
    }
}
