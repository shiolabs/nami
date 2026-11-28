//! `files:` in the system file: files under /etc.
//!
//! ```yaml
//! files:
//!   /etc/ssh/sshd_config.d/10-nami.conf: |       # text, as is
//!     PasswordAuthentication no
//!   /etc/myapp/config.toml:                      # nested YAML -> the file's format
//!     port: 8080
//!   /etc/myapp/certs: { source: ./certs }        # a file or dir next to the nami file
//!   /etc/sudoers.d/90-ops: { content: "%ops ALL=(ALL) ALL\n", mode: "0440" }
//! ```
//!
//! Rendered like home files (same formats, see home.rs) into one store path
//! per change, linked from the generation as `.etc`. Activation copies them
//! into place (mode 0644 unless `mode:` says, owned by root): /etc is read at
//! boot, maybe before /nix is mounted, and some programs (sudo, sshd) want
//! real files owned by root. A file in the way is moved to
//! `<file>.nami-backup` and put back when nami stops managing the path; a
//! managed file someone changed by hand is kept (on removal) or moved aside
//! (on change), never silently lost. What nami wrote, with its hash, is in
//! /nami/etc-files.json.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};

use crate::store::{write_atomic, Store};

/// Paths nami won't write: breaking them locks everyone out, or they're
/// someone else's to manage (users, mounts, nami itself, units).
const REFUSED: &[(&str, &str)] = &[
    ("/etc/passwd", "user accounts aren't managed by nami yet"),
    ("/etc/shadow", "user accounts aren't managed by nami yet"),
    ("/etc/group", "user accounts aren't managed by nami yet"),
    ("/etc/gshadow", "user accounts aren't managed by nami yet"),
    ("/etc/sudoers", "a broken sudoers locks admins out; add a file under /etc/sudoers.d instead"),
    ("/etc/fstab", "a broken fstab stops the computer from booting"),
    ("/etc/crypttab", "a broken crypttab stops the computer from booting"),
    ("/etc/nami", "that's nami's own configuration"),
    ("/etc/tray", "that's nami's own configuration (its name from before)"),
    ("/etc/systemd/system", "use services: for units"),
    ("/etc/systemd/user", "use services: for units"),
];

/// `/etc/x/y` checked and normalized; anything outside /etc, or refused, is an error.
pub fn check_path(path: &str) -> Result<String> {
    let p = Path::new(path);
    ensure!(p.is_absolute(), "files.{path}: must be an absolute path under /etc (a person's own: ~/... in their user.nami)");
    ensure!(!path.starts_with("/run/"), "files.{path}: a file in /run is for a computer nami made (`nami usb`), which writes it as it starts");
    let mut parts = Vec::new();
    for c in p.components() {
        match c {
            Component::RootDir => {}
            Component::Normal(n) => parts.push(n.to_string_lossy().into_owned()),
            _ => bail!("files.{path}: no '..' or '.' allowed"),
        }
    }
    ensure!(parts.first().map(String::as_str) == Some("etc") && parts.len() > 1, "files.{path}: only files under /etc (nami doesn't write elsewhere on the system)");
    let norm = format!("/{}", parts.join("/"));
    for (refused, why) in REFUSED {
        if norm == *refused || norm.starts_with(&format!("{refused}/")) {
            bail!("files.{norm}: nami won't write {refused}: {why}");
        }
    }
    Ok(norm)
}

/// A file with a secret in it: under /etc (it links to its copy in /run),
/// or under /run itself, where it's written as the computer starts.
pub fn check_secret_path(path: &str) -> Result<String> {
    if !path.starts_with("/run/") {
        return check_path(path);
    }
    let mut parts = Vec::new();
    for c in Path::new(path).components() {
        match c {
            Component::RootDir => {}
            Component::Normal(n) => parts.push(n.to_string_lossy().into_owned()),
            _ => bail!("files.{path}: no '..' or '.' allowed"),
        }
    }
    ensure!(parts.len() > 1, "files.{path}: a file in /run, like /run/nami/secrets/<name>");
    let norm = format!("/{}", parts.join("/"));
    for own in ["/run/current-system", "/run/booted-system", "/run/wrappers", "/run/opengl-driver", "/run/opengl-driver-32", "/run/nami/secrets.list"] {
        ensure!(norm != own && !norm.starts_with(&format!("{own}/")), "files.{norm}: that's the computer's own");
    }
    Ok(norm)
}

/// The options a file may have, the same in every nami file, for a file
/// in /etc and one in your home: `content:` or `source:` (what it says),
/// `mode:`, `owner:` and `group:` (the computer's files), `mutable:` (a
/// file in a home, that apps may change), and what to do when nami
/// changes it (the computer's files): `reload:` runs a command,
/// `restarts:` restarts units (`restart:` before; a service's own
/// `restart:` is when it starts again).
pub const OPTIONS: &[&str] = &["content", "source", "mode", "owner", "group", "mutable", "reload", "restarts", "restart"];

/// Is `m` a file's options (`{ content: ..., mode: ... }`), not its settings?
/// Only with `content:` or `source:`: a settings file may well have a `mode:`.
pub fn is_options(m: &Mapping) -> bool {
    let has = |k: &str| m.contains_key(k);
    m.keys().all(|k| k.as_str().is_some_and(|k| OPTIONS.contains(&k))) && (has("content") || has("source"))
}

/// What a file's options can't say where the file is: in a home
/// (`home`), the computer's own options; in /etc, `mutable:`.
pub fn check_options(value: &Value, home: bool) -> Result<()> {
    let Value::Mapping(m) = value else { return Ok(()) };
    if !is_options(m) {
        return Ok(());
    }
    ensure!(!(m.contains_key("content") && m.contains_key("source")), "give either `content:` or `source:`, not both");
    if home {
        for k in ["owner", "group"] {
            ensure!(!m.contains_key(k), "{k}: is for the computer's files in /etc; a file in your home is yours");
        }
        for k in ["reload", "restarts", "restart"] {
            ensure!(!m.contains_key(k), "{k}: is for the computer's files in /etc, when nami changes one");
        }
    } else {
        ensure!(!m.contains_key("mutable"), "mutable: is for a file in someone's home; nami keeps the computer's files in /etc as it writes them");
    }
    match m.get("mutable") {
        None | Some(Value::Bool(_)) => {}
        Some(Value::String(s)) if matches!(s.as_str(), "yes" | "no" | "on" | "off") => bail!("mutable: true or false: write {} (in YAML, {s} is text)", matches!(s.as_str(), "yes" | "on")),
        Some(_) => bail!("mutable: true or false"),
    }
    mode_of(m)?;
    Ok(())
}

/// A file's `mode:` (octal, like "0640"), if it has one.
pub fn mode_of(m: &Mapping) -> Result<Option<u32>> {
    let Some(v) = m.get("mode") else { return Ok(None) };
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => bail!("mode: octal, like \"0640\""),
    };
    let mode = u32::from_str_radix(s.trim_start_matches("0o"), 8).map_err(|_| anyhow!("mode: octal, like \"0640\""))?;
    ensure!(mode <= 0o777, "mode {s}: no setuid, setgid or sticky bits");
    Ok(Some(mode))
}

/// Is a file's value its options form?
pub fn is_options_value(v: &Value) -> bool {
    matches!(v, Value::Mapping(m) if is_options(m))
}

/// A file's value in its options form: text or settings are its `content:`.
pub fn as_options(v: Value) -> Value {
    if is_options_value(&v) {
        v
    } else {
        Value::Mapping([("content".into(), v)].into_iter().collect())
    }
}

/// `owner:` and `group:` of a file (for files with a secret in them).
pub fn owner_of(value: &Value) -> Result<(Option<String>, Option<String>)> {
    let Value::Mapping(m) = value else { return Ok((None, None)) };
    if !is_options(m) {
        return Ok((None, None));
    }
    let get = |k: &str| -> Result<Option<String>> {
        match m.get(k) {
            None => Ok(None),
            Some(Value::String(s)) if !s.is_empty() && !s.contains([':', '/', '\n']) => Ok(Some(s.clone())),
            Some(Value::Number(n)) => Ok(Some(n.to_string())),
            Some(_) => bail!("{k}: a user or group's name"),
        }
    };
    Ok((get("owner")?, get("group")?))
}

/// What to do when nami changes a file (and the computer is running):
/// `reload:` commands to run, `restart:` units to restart (if they run).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OnChange {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reload: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restart: Vec<String>,
}

impl OnChange {
    pub fn is_empty(&self) -> bool {
        self.reload.is_empty() && self.restart.is_empty()
    }
}

/// A file's `reload:` and `restarts:` (each one, or a list; `restart:`
/// is the old name).
pub fn on_change_of(value: &Value) -> Result<OnChange> {
    let Value::Mapping(m) = value else { return Ok(OnChange::default()) };
    if !is_options(m) {
        return Ok(OnChange::default());
    }
    let list = |k: &str| -> Result<Vec<String>> {
        match m.get(k) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::String(s)) => Ok(vec![s.clone()]),
            Some(Value::Sequence(l)) => l.iter().map(|v| v.as_str().map(String::from).ok_or_else(|| anyhow!("{k}: a command, or a list of them"))).collect(),
            Some(_) => bail!("{k}: a command, or a list of them"),
        }
    };
    ensure!(!(m.contains_key("restart") && m.contains_key("restarts")), "restart: and restarts: both say it: restart: is now restarts:, so keep that one");
    if m.contains_key("restart") {
        crate::config::deprecated("files", "a file's restart: is now restarts: (a service's restart: is when it starts again)");
    }
    let mut restart = list("restarts")?;
    restart.extend(list("restart")?);
    let on = OnChange { reload: list("reload")?, restart };
    for u in &on.restart {
        ensure!(!u.is_empty() && !u.contains(char::is_whitespace), "restarts: {u:?}: a unit's name, like sshd.service");
    }
    Ok(on)
}

/// Run what `on` says, now that `what` changed. Says what it did.
pub fn run_on_change(what: &str, on: &OnChange) -> Vec<String> {
    let mut said = Vec::new();
    // The computer's programs, wherever this runs from (the daemon too).
    let path = ["/run/wrappers/bin", "/run/current-system/sw/bin"].iter().filter(|p| Path::new(p).is_dir()).map(|p| p.to_string()).chain(std::env::var("PATH").ok()).collect::<Vec<_>>().join(":");
    let mut run = |cmd: &mut std::process::Command, shown: &str| match cmd.env("PATH", &path).output() {
        Ok(o) if o.status.success() => said.push(format!("{what} changed: {shown}")),
        Ok(o) => said.push(format!("{what} changed, and `{shown}` failed ({}); it applies at the next start", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => said.push(format!("{what} changed, and `{shown}` couldn't run ({e}); it applies at the next start")),
    };
    for c in &on.reload {
        run(std::process::Command::new("sh").arg("-c").arg(c), c);
    }
    for u in &on.restart {
        run(crate::services::systemctl().args(["try-restart", u]), &format!("systemctl try-restart {u}"));
    }
    said
}

/// `{ content|source, mode }`: the content and the mode, if the value is that
/// wrapper; else the value itself.
pub fn unwrap_options(value: &Value) -> Result<(Value, Option<u32>)> {
    let Value::Mapping(m) = value else { return Ok((value.clone(), None)) };
    if !is_options(m) {
        return Ok((value.clone(), None));
    }
    check_options(value, false)?;
    let mode = mode_of(m)?;
    match (m.get("content"), m.get("source")) {
        (Some(c), None) => Ok((c.clone(), mode)),
        (None, Some(src)) => Ok((Value::Mapping([("source".into(), src.clone())].into_iter().collect()), mode)),
        _ => bail!("give either `content:` or `source:`"),
    }
}

/// A generation's /etc files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EtcState {
    /// Store path; `/etc/a/b` is `<path>/a/b`.
    pub path: String,
    pub files: Vec<EtcFile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EtcFile {
    /// `/etc/...`
    pub path: String,
    /// NAR hash of the content.
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    /// What to run when it changes.
    #[serde(default, flatten)]
    pub on_change: OnChange,
}

impl EtcFile {
    /// Where it is in the generation's store path.
    pub fn rel(&self) -> &str {
        self.path.trim_start_matches("/etc/")
    }
}

/// Render `files` (sources relative to `base`) into the store.
pub fn build(store: &Store, files: &Mapping, base: &Path) -> Result<EtcState> {
    let dir = store.cache_dir().join(format!("etc-build-{}", std::process::id()));
    if dir.symlink_metadata().is_ok() {
        crate::store::remove_tree(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    let result = (|| {
        let mut out: Vec<EtcFile> = Vec::new();
        for (key, value) in files {
            let path = key.as_str().ok_or_else(|| anyhow!("files: each is a path, like /etc/app/config.toml"))?;
            let path = check_path(path)?;
            if let Some(other) = out.iter().find(|f| path.starts_with(&format!("{}/", f.path)) || f.path.starts_with(&format!("{path}/"))) {
                bail!("files.{path}: inside files.{} (or the other way round)", other.path);
            }
            ensure!(!crate::secrets::contains(value), "files.{path}: a file with a secret in it works on a computer nami made (`nami usb`), which decrypts it into /run as it starts; here, keep the file out of the system file (or the secret out of the file)");
            ensure!(owner_of(value)? == (None, None), "files.{path}: owner: and group: are for files with a secret in them (the rest belong to root)");
            let (content, mode) = unwrap_options(value).with_context(|| format!("files.{path}"))?;
            let on_change = on_change_of(value).with_context(|| format!("files.{path}"))?;
            let content = &content;
            let rel = path.trim_start_matches("/etc/");
            let dest = dir.join(rel);
            fs::create_dir_all(dest.parent().unwrap())?;
            crate::home::write_entry(&dest, rel, content, base).with_context(|| format!("files.{path}"))?;
            ensure!(mode.is_none() || dest.is_file(), "files.{path}: `mode:` is for files, not directories");
            out.push(EtcFile { hash: crate::nar::hash_path(&dest)?.0, path, mode, on_change });
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(EtcState { path: store.add_path(&dir, "etc-files")?, files: out })
    })();
    crate::store::remove_tree(&dir)?;
    result
}

/// What nami wrote under /etc: path -> hash.
fn load(state_file: &Path) -> BTreeMap<String, String> {
    fs::read(state_file).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
}

pub fn state_file(var_dir: &Path) -> PathBuf {
    var_dir.join("etc-files.json")
}

/// Lines for `--dry-run`: what activating `new` would do.
pub fn describe(root: &Path, var_dir: &Path, new: Option<&EtcState>) -> Vec<String> {
    let old = load(&state_file(var_dir));
    let files = new.map(|n| n.files.as_slice()).unwrap_or(&[]);
    let mut out = Vec::new();
    for p in old.keys().filter(|p| !files.iter().any(|f| &f.path == *p)) {
        out.push(format!("remove {p}"));
    }
    for f in files {
        let now = hash(&root.join(f.path.trim_start_matches('/')));
        match (old.get(&f.path), now) {
            (_, Some(h)) if h == f.hash => {}
            (Some(_), _) => out.push(format!("update {}", f.path)),
            (None, Some(_)) => out.push(format!("write {} (the file there is kept as {}.nami-backup)", f.path, f.path)),
            (None, None) => out.push(format!("write {}", f.path)),
        }
    }
    out
}

/// The paths activating `new` writes or removes (for `restart-on:`): what's
/// on disk differs from the generation, or nami stops managing it.
pub fn changes(root: &Path, var_dir: &Path, new: Option<&EtcState>) -> Vec<String> {
    let old = load(&state_file(var_dir));
    let files = new.map(|n| n.files.as_slice()).unwrap_or(&[]);
    let mut out: Vec<String> = old.keys().filter(|p| !files.iter().any(|f| &f.path == *p)).cloned().collect();
    out.extend(files.iter().filter(|f| hash(&root.join(f.path.trim_start_matches('/'))).as_deref() != Some(f.hash.as_str())).map(|f| f.path.clone()));
    out
}

/// Files the system reads at boot or on request: after an activation changed
/// some (`changes`, before it), apply them now the way the distro would, on
/// the real system only. What it did, or what to do, worth saying.
pub fn apply_now(root: &Path, changed: &[String]) -> Vec<String> {
    let mut said = Vec::new();
    if root != Path::new("/") {
        return said;
    }
    let run = |said: &mut Vec<String>, cmd: &str, args: &[&str], what: &str| match std::process::Command::new(cmd).args(args).output() {
        Ok(o) if o.status.success() => said.push(format!("applied {what}")),
        Ok(o) => said.push(format!("{what}: `{cmd} {}` failed ({}); it applies at the next boot", args.join(" "), String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => said.push(format!("{what}: couldn't run {cmd} ({e}); it applies at the next boot")),
    };
    let (mut udev, mut env, mut modprobe) = (false, false, false);
    for p in changed {
        let exists = Path::new(p).exists();
        if p.starts_with("/etc/tmpfiles.d/") && exists {
            run(&mut said, "systemd-tmpfiles", &["--create", p], p);
        } else if p.starts_with("/etc/sysctl.d/") && exists {
            run(&mut said, "sysctl", &["-q", "-p", p], p);
        } else if p.starts_with("/etc/modules-load.d/") && exists {
            let text = fs::read_to_string(p).unwrap_or_default();
            let modules: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with(['#', ';'])).collect();
            if !modules.is_empty() {
                run(&mut said, "modprobe", &[&["-a"], modules.as_slice()].concat(), p);
            }
        } else if p.starts_with("/etc/udev/rules.d/") {
            udev = true;
        } else if p.starts_with("/etc/environment.d/") || p.starts_with("/etc/profile.d/") {
            env = true;
        } else if p.starts_with("/etc/modprobe.d/") {
            modprobe = true;
        }
    }
    if udev {
        run(&mut said, "udevadm", &["control", "--reload"], "udev rules (to devices plugged in from now; `sudo udevadm trigger` for the rest)");
    }
    if env {
        said.push("environment variables for everyone: they apply from the next login".into());
    }
    if modprobe {
        said.push("kernel module options: they apply when the module next loads (or after a reboot)".into());
    }
    said
}

/// After an activation changed `changed` (see `changes`): what their
/// `reload:` and `restart:` say, on the real system only.
pub fn run_changed(root: &Path, changed: &[String], new: Option<&EtcState>) -> Vec<String> {
    if root != Path::new("/") {
        return Vec::new();
    }
    let files = new.map(|n| n.files.as_slice()).unwrap_or(&[]);
    files.iter().filter(|f| !f.on_change.is_empty() && changed.contains(&f.path)).flat_map(|f| run_on_change(&f.path, &f.on_change)).collect()
}

/// Managed files and whether each was changed by hand since nami wrote it.
pub fn status(root: &Path, var_dir: &Path) -> Vec<(String, bool)> {
    load(&state_file(var_dir)).into_iter().map(|(p, h)| {
        let now = hash(&root.join(p.trim_start_matches('/')));
        (p, now.as_deref() != Some(h.as_str()))
    }).collect()
}

/// Make /etc (under `root`) match `new`, copying from `link_base` (the active
/// generation's `.etc`). Returns what it did worth saying.
pub fn activate(root: &Path, var_dir: &Path, link_base: &Path, new: Option<&EtcState>) -> Result<Vec<String>> {
    let sf = state_file(var_dir);
    let old = load(&sf);
    let files = new.map(|n| n.files.as_slice()).unwrap_or(&[]);
    let mut said = Vec::new();
    if files.is_empty() && old.is_empty() {
        return Ok(said);
    }
    let mut now = BTreeMap::new();
    for (p, recorded) in &old {
        if files.iter().any(|f| &f.path == p) || check_path(p).is_err() {
            continue;
        }
        let dest = root.join(p.trim_start_matches('/'));
        match hash(&dest) {
            Some(h) if &h == recorded => {
                remove(&dest)?;
                said.push(format!("removed {p}"));
            }
            Some(_) => said.push(format!("kept {p}: changed since nami wrote it, so it's yours now")),
            None => {}
        }
        restore_backup(&dest, &mut said)?;
    }
    for f in files {
        let path = check_path(&f.path)?;
        let dest = root.join(path.trim_start_matches('/'));
        if place(&link_base.join(f.rel()), &dest, &f.hash, old.get(&path).map(String::as_str), f.mode, &mut said)? {
            said.push(format!("wrote {path}"));
        }
        now.insert(path, f.hash.clone());
    }
    fs::create_dir_all(var_dir)?;
    write_atomic(&sf, &serde_json::to_vec_pretty(&now)?)?;
    Ok(said)
}

/// Copy `src` to `dest` unless it already has `wanted` (then only fix the
/// mode). `recorded`: the hash nami last wrote there, if it's nami's. A file
/// that isn't nami's, or that was changed by hand, is moved aside first.
/// Returns whether it wrote.
pub fn place(src: &Path, dest: &Path, wanted: &str, recorded: Option<&str>, mode: Option<u32>, said: &mut Vec<String>) -> Result<bool> {
    let current = hash(dest);
    if current.as_deref() == Some(wanted) {
        if let Some(m) = mode.filter(|_| dest.is_file()) {
            fs::set_permissions(dest, fs::Permissions::from_mode(m))?;
        }
        return Ok(false);
    }
    // The new copy first, so a failure leaves what's there alone.
    let parent = dest.parent().ok_or_else(|| anyhow!("{} has no parent", dest.display()))?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".{}.nami-new", dest.file_name().unwrap_or_default().to_string_lossy()));
    if tmp.symlink_metadata().is_ok() {
        remove(&tmp)?;
    }
    copy_out(src, &tmp, mode).with_context(|| format!("copying {}", src.display()))?;
    if current.is_some() || dest.symlink_metadata().is_ok() {
        if recorded.is_some() && current.as_deref() == recorded {
            remove(dest)?; // ours, untouched
        } else {
            let backup = free_backup(dest);
            fs::rename(dest, &backup)?;
            let why = if recorded.is_some() { "it was changed by hand" } else { "it was there before" };
            said.push(format!("moved {} to {} ({why})", dest.display(), backup.display()));
        }
    }
    fs::rename(&tmp, dest)?;
    Ok(true)
}

/// Copy out of the (read-only) store: files 0644 (0755 if executable, or
/// `mode`), directories 0755.
fn copy_out(src: &Path, dest: &Path, mode: Option<u32>) -> Result<()> {
    let meta = fs::symlink_metadata(src)?;
    if meta.is_dir() {
        fs::create_dir(dest)?;
        for e in fs::read_dir(src)? {
            let e = e?;
            copy_out(&e.path(), &dest.join(e.file_name()), None)?;
        }
        fs::set_permissions(dest, fs::Permissions::from_mode(0o755))?;
    } else if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(src)?, dest)?;
    } else {
        fs::copy(src, dest)?;
        let exec = meta.permissions().mode() & 0o111 != 0;
        fs::set_permissions(dest, fs::Permissions::from_mode(mode.unwrap_or(if exec { 0o755 } else { 0o644 })))?;
    }
    Ok(())
}

/// NAR hash of a file or directory; None if there's nothing there.
pub fn hash(path: &Path) -> Option<String> {
    path.symlink_metadata().ok()?;
    crate::nar::hash_path(path).ok().map(|(h, _)| h)
}

pub fn remove(path: &Path) -> Result<()> {
    if path.symlink_metadata()?.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn backup_of(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".nami-backup");
    PathBuf::from(s)
}

/// `<file>.nami-backup`, or `<file>.nami-backup-<n>` if that's taken.
fn free_backup(dest: &Path) -> PathBuf {
    let first = backup_of(dest);
    if first.symlink_metadata().is_err() {
        return first;
    }
    (1..).map(|n| PathBuf::from(format!("{}-{n}", first.display()))).find(|p| p.symlink_metadata().is_err()).unwrap()
}

/// Put `<file>.nami-backup` back, if it's there and the place is free.
pub fn restore_backup(dest: &Path, said: &mut Vec<String>) -> Result<()> {
    let mut backup = backup_of(dest);
    // (One tray kept: `<file>.tray-backup`.)
    if backup.symlink_metadata().is_err() {
        let mut s = dest.as_os_str().to_owned();
        s.push(".tray-backup");
        backup = PathBuf::from(s);
    }
    if backup.symlink_metadata().is_ok() && dest.symlink_metadata().is_err() {
        fs::rename(&backup, dest)?;
        said.push(format!("restored {} from its .nami-backup", dest.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_etc_and_not_the_dangerous_parts() {
        assert_eq!(check_path("/etc//ssh/sshd_config.d/10-nami.conf").unwrap(), "/etc/ssh/sshd_config.d/10-nami.conf");
        for (p, says) in [
            ("etc/x", "absolute"),
            ("/usr/local/x", "only files under /etc"),
            ("/etc", "only files under /etc"),
            ("/etc/../root/x", "'..'"),
            ("/etc/passwd", "user accounts"),
            ("/etc/sudoers", "sudoers.d"),
            ("/etc/fstab", "booting"),
            ("/etc/nami/system.nami", "nami's own"),
            ("/etc/systemd/system/x.service", "services:"),
        ] {
            let e = check_path(p).unwrap_err().to_string();
            assert!(e.contains(says), "{p}: {e}");
        }
        assert!(check_path("/etc/sudoers.d/90-ops").is_ok());
        assert!(check_path("/etc/passwd.d/x").is_ok()); // only the file itself (and what's under it)
    }

    #[test]
    fn mode_wrapper() {
        let v: Value = serde_yaml::from_str("content: hi\nmode: \"0440\"").unwrap();
        let (c, m) = unwrap_options(&v).unwrap();
        assert_eq!((c.as_str(), m), (Some("hi"), Some(0o440)));
        let settings: Value = serde_yaml::from_str("port: 8080").unwrap();
        assert_eq!(unwrap_options(&settings).unwrap(), (settings.clone(), None));
        // reload: and restart: are the file's, not its content.
        let v: Value = serde_yaml::from_str("source: ./x\nmode: \"0600\"\nreload: nmcli connection reload\nrestarts: [a.service]").unwrap();
        assert_eq!(unwrap_options(&v).unwrap(), (serde_yaml::from_str("source: ./x").unwrap(), Some(0o600)));
        assert_eq!(on_change_of(&v).unwrap(), OnChange { reload: vec!["nmcli connection reload".into()], restart: vec!["a.service".into()] });
        let v: Value = serde_yaml::from_str("source: ./x\nrestart: sshd.service").unwrap();
        assert_eq!(on_change_of(&v).unwrap().restart, vec!["sshd.service"]);
        assert!(on_change_of(&serde_yaml::from_str("content: x\nrestart: two words").unwrap()).is_err());
        assert!(on_change_of(&settings).unwrap().is_empty());
        assert!(unwrap_options(&serde_yaml::from_str("content: x\nmode: \"4755\"").unwrap()).is_err());
        // A settings file with a `mode:` of its own is settings.
        let dark: Value = serde_yaml::from_str("mode: dark").unwrap();
        assert_eq!(unwrap_options(&dark).unwrap(), (dark.clone(), None));
        // The old name still restarts it.
        let old: Value = serde_yaml::from_str("content: x\nrestart: [b.service]").unwrap();
        assert_eq!(on_change_of(&old).unwrap().restart, vec!["b.service"]);
        // The options are the same everywhere; each place refuses what isn't its.
        let y = |s: &str| serde_yaml::from_str::<Value>(s).unwrap();
        assert!(unwrap_options(&y("content: x\nmutable: true")).unwrap_err().to_string().contains("mutable: is for a file in someone's home"));
        assert!(check_options(&y("content: x\nowner: ada"), true).unwrap_err().to_string().contains("owner: is for the computer's files in /etc"));
        assert!(check_options(&y("content: x\nrestarts: a.service"), true).is_err());
        assert!(check_options(&y("content: x\nmode: \"0400\"\nmutable: true"), true).is_ok());
        assert!(check_options(&y("content: x\nsource: ./y"), true).is_err());
    }

    /// A scratch root: files written, backed up, changed, restored.
    #[test]
    fn backup_and_restore() {
        let tmp = std::env::temp_dir().join(format!("nami-etc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let (root, var) = (tmp.join("root"), tmp.join("var"));
        fs::create_dir_all(root.join("etc/myapp")).unwrap();
        fs::write(root.join("etc/myapp/app.conf"), "theirs\n").unwrap();
        let gen = |n: u32, text: &str, mode: Option<u32>| -> (PathBuf, EtcState) {
            let base = tmp.join(format!("gen{n}"));
            fs::create_dir_all(base.join("myapp")).unwrap();
            fs::write(base.join("myapp/app.conf"), text).unwrap();
            let file = EtcFile { path: "/etc/myapp/app.conf".into(), hash: hash(&base.join("myapp/app.conf")).unwrap(), mode, on_change: OnChange::default() };
            (base, EtcState { path: String::new(), files: vec![file] })
        };
        let dest = root.join("etc/myapp/app.conf");
        let backup = root.join("etc/myapp/app.conf.nami-backup");

        let (b1, s1) = gen(1, "port = 1\n", Some(0o640));
        assert_eq!(describe(&root, &var, Some(&s1)), vec!["write /etc/myapp/app.conf (the file there is kept as /etc/myapp/app.conf.nami-backup)"]);
        assert_eq!(changes(&root, &var, Some(&s1)), vec!["/etc/myapp/app.conf"]);
        let said = activate(&root, &var, &b1, Some(&s1)).unwrap();
        assert!(said.iter().any(|s| s.contains("it was there before")), "{said:?}");
        assert_eq!(fs::read_to_string(&dest).unwrap(), "port = 1\n");
        assert_eq!(fs::metadata(&dest).unwrap().permissions().mode() & 0o777, 0o640);
        assert_eq!(fs::read_to_string(&backup).unwrap(), "theirs\n");
        assert_eq!(status(&root, &var), vec![("/etc/myapp/app.conf".to_string(), false)]);
        assert!(describe(&root, &var, Some(&s1)).is_empty());
        assert!(changes(&root, &var, Some(&s1)).is_empty());

        // A new generation replaces nami's own (untouched) file without a backup.
        let (b2, s2) = gen(2, "port = 2\n", None);
        activate(&root, &var, &b2, Some(&s2)).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "port = 2\n");
        assert_eq!(fs::metadata(&dest).unwrap().permissions().mode() & 0o777, 0o644);
        assert!(!root.join("etc/myapp/app.conf.nami-backup-1").exists());

        // Changed by hand, then the spec changes: the edit is moved aside.
        fs::write(&dest, "edited\n").unwrap();
        assert_eq!(status(&root, &var), vec![("/etc/myapp/app.conf".to_string(), true)]);
        activate(&root, &var, &b1, Some(&s1)).unwrap();
        assert_eq!(fs::read_to_string(root.join("etc/myapp/app.conf.nami-backup-1")).unwrap(), "edited\n");
        assert_eq!(fs::read_to_string(&dest).unwrap(), "port = 1\n");

        // No longer managed: the original comes back.
        assert_eq!(describe(&root, &var, None), vec!["remove /etc/myapp/app.conf"]);
        assert_eq!(changes(&root, &var, None), vec!["/etc/myapp/app.conf"]);
        activate(&root, &var, &b1, None).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "theirs\n");
        assert!(!backup.exists());
        assert!(status(&root, &var).is_empty());

        // Managed again, edited, then unmanaged: the edit stays.
        activate(&root, &var, &b1, Some(&s1)).unwrap();
        fs::write(&dest, "mine now\n").unwrap();
        let said = activate(&root, &var, &b1, None).unwrap();
        assert!(said.iter().any(|s| s.contains("kept /etc/myapp/app.conf")), "{said:?}");
        assert_eq!(fs::read_to_string(&dest).unwrap(), "mine now\n");
        fs::remove_dir_all(&tmp).unwrap();
    }
}
