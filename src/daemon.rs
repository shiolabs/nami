//! `nami daemon`: lets users without root change the shared store, safely.
//!
//! Users keep their own profiles (profile.rs). Anything that writes to
//! /nix/store goes through this daemon, which only offers operations that
//! can't be abused to plant something under a path another user trusts:
//!
//! - `realise`: download paths, only from caches root trusts, signatures checked
//! - `add-nar`: add a file/dir; the path is computed here from its content
//! - `build`: run a job in the sandbox; output paths are computed here from
//!   the whole job (see `build::Job`)
//! - `replace`: copy packages with a library swapped for another (replace.rs);
//!   the new paths are computed here from the old ones and the swap
//! - `ensure-graphics`: keep GPU drivers in the system profile
//! - `run`: for admins (wheel/sudo/admin group, checked by the socket's
//!   peer credentials), run a nami command that changes the system's apps
//!   as root: install, remove, apply /etc/nami/system.nami, upgrade, rollback,
//!   gc, self-update, and `config` of system.nami (root's alone to
//!   write). What changes system.nami, or someone's password, asks
//!   polkit first: the admin's password, there and then, in their desktop's
//!   box or their terminal (`asks_password`). Someone who isn't an admin
//!   may run the same commands only with an admin's name and password,
//!   asked by polkit for every one of them (`unlocked`): Settings' Unlock.
//!   Output streams back as `{"out"}`/`{"log"}` lines, then `{"exit": code}`.
//!
//! Protocol: one request per connection, a JSON line (plus the archive bytes
//! for `add-nar`); the daemon answers with `{"log": ...}` lines while it
//! works, then one `{"ok": ...}` or `{"error": ...}`. Requests that change
//! the store are served one at a time; one that has to wait is told first
//! what it waits for (`Turns`).

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::build::{Builder, Job};
use crate::index::Index;
use crate::nar::HashingWriter;
use crate::profile::Profiles;
use crate::store::{base_name, valid_store_name, Store, STORE_DIR};

/// Largest archive a user may add (sources, patches): 16 GiB.
const MAX_NAR: u64 = 16 << 30;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    Realise { paths: Vec<String> },
    /// An archive to add (store.rs, add_nar_file), with the store paths it may refer to.
    AddNar {
        name: String,
        size: u64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        references: Vec<String>,
    },
    Build { job: Job },
    /// A cask's apps (an archive of them) for nami's casks folder, root's (casks.rs, add_folder).
    AddCask { name: String, size: u64 },
    /// Store paths `roots` with `swaps` (old -> new) put in (replace.rs).
    Replace { roots: Vec<String>, swaps: std::collections::BTreeMap<String, String> },
    EnsureGraphics,
    /// Re-fetch the package index from the source root configured.
    RefreshIndex,
    /// Build a catalog package the binary cache doesn't have, from the
    /// catalog's own (signed) build closure. Users name a package, never a
    /// derivation, so they can't make root build anything else.
    BuildCatalog { attr: String },
    /// Run `nami <args>` as root in `cwd`, for an admin; `stdin` is its
    /// input (`--password-stdin`'s passwords), else none.
    Run {
        args: Vec<String>,
        cwd: String,
        #[serde(default)]
        stdin: Option<String>,
    },
}

/// What admins may run as root through the daemon: the commands that
/// change the system's apps.
pub const ADMIN_COMMANDS: &[&str] = &["install", "remove", "apply", "layers", "upgrade", "undo", "rollback", "gc", "self-update", "os-update", "system-init", "system-use", "update", "config", "new"];

pub fn serve(root: &Path) -> Result<()> {
    let store = Store::open_direct(root, false)?;
    let socket = crate::store::daemon_socket(root);
    let _ = fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).with_context(|| format!("listening on {}", socket.display()))?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o666))?;
    eprintln!("nami daemon: listening on {}", socket.display());
    // Each connection on its own thread: one that waits is told what for,
    // a client that never asks holds up nobody, nor does an admin's command
    // (it locks the store itself). The store's requests still go one at a
    // time (`Turns`).
    let turns = Turns::default();
    std::thread::scope(|s| {
        for conn in listener.incoming() {
            match conn {
                Ok(conn) => {
                    let (store, turns) = (&store, &turns);
                    s.spawn(move || {
                        if let Err(e) = handle(store, turns, conn) {
                            eprintln!("nami daemon: {e:#}");
                        }
                    });
                }
                Err(e) => eprintln!("nami daemon: accept: {e}"),
            }
        }
    });
    Ok(())
}

/// The store's requests take turns, one at a time: they all hold the store
/// lock anyway, and what's said while one is served goes to its asker
/// (log.rs' sink is one for the daemon). Who comes while another is served
/// is told what they wait for, never left with nothing.
#[derive(Default)]
struct Turns {
    turn: Mutex<()>,
    /// What's being done now (`building tray-gui-0.18.5`).
    now: Mutex<String>,
}

impl Turns {
    fn take(&self, writer: &Arc<Mutex<UnixStream>>, what: String) -> std::sync::MutexGuard<'_, ()> {
        let turn = match self.turn.try_lock() {
            Ok(t) => t,
            Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                let now = self.now.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let _ = writeln!(writer.lock().unwrap(), "{}", json!({ "log": waiting_line(&now) }));
                self.turn.lock().unwrap_or_else(|e| e.into_inner())
            }
        };
        *self.now.lock().unwrap_or_else(|e| e.into_inner()) = what;
        turn
    }
}

/// What someone whose request has to wait is told.
fn waiting_line(now: &str) -> String {
    format!("waiting for another nami to finish ({now}); this goes on once it's done")
}

/// A request's work, as someone waiting for it is told.
fn doing(request: &Request) -> String {
    match request {
        Request::Realise { paths } => match paths.as_slice() {
            [one] => format!("downloading {}", base_name(one).get(33..).unwrap_or(one)),
            many => format!("downloading {} packages", many.len()),
        },
        Request::AddNar { name, .. } => format!("adding {name} to the store"),
        Request::AddCask { name, .. } => format!("keeping {} from Homebrew's casks", name.trim_start_matches("cask-")),
        Request::Build { job } => format!("building {}", job.name),
        Request::BuildCatalog { attr } => format!("building {attr}"),
        Request::Replace { .. } => "putting a library in what uses it".into(),
        Request::RefreshIndex => "updating the catalog".into(),
        Request::EnsureGraphics => "setting up the graphics drivers".into(),
        Request::Run { args, .. } => format!("nami {}", args.first().map(String::as_str).unwrap_or_default()),
    }
}

fn handle(store: &Store, turns: &Turns, conn: UnixStream) -> Result<()> {
    let mut reader = BufReader::new(conn.try_clone()?);
    let mut line = String::new();
    (&mut reader).take(1 << 24).read_line(&mut line)?;
    let writer = Arc::new(Mutex::new(conn));
    let request: Request = match serde_json::from_str(&line) {
        Ok(r) => r,
        Err(e) => return reply(&writer, Err(anyhow::anyhow!("bad request: {e}"))),
    };

    if let Request::Run { args, cwd, stdin } = request {
        // Not under the store lock, nor taking a turn: the command locks the store itself.
        return run_for_admin(&conn_of(&writer), &writer, args, cwd, stdin);
    }

    let _turn = turns.take(&writer, doing(&request));
    // Who gave up while waiting (Ctrl-C) isn't served: nobody would see it.
    if gone(&conn_of(&writer)) {
        return Ok(());
    }
    let w = writer.clone();
    let guard = crate::log::redirect(Box::new(move |l: &str| {
        let _ = writeln!(w.lock().unwrap(), "{}", json!({ "log": l }));
    }));
    let result = (|| -> Result<Value> {
        let _lock = store.lock()?;
        store.reload_caches()?;
        match request {
            Request::Realise { paths } => {
                for p in &paths {
                    ensure!(p.starts_with(&format!("{STORE_DIR}/")) && valid_store_name(&base_name(p)[33.min(base_name(p).len())..]), "bad store path {p:?}");
                }
                store.realise(&paths)?;
                Ok(Value::Null)
            }
            Request::AddNar { name, size, references } => {
                ensure!(size <= MAX_NAR, "archive too large");
                let tmp = store.staging_dir()?.join(format!(".nami-daemon-add-{}.nar", std::process::id()));
                let mut w = HashingWriter::new(File::create(&tmp)?);
                let copied = std::io::copy(&mut (&mut reader).take(size), &mut w)?;
                let nar_hash = w.finish();
                let result = if copied == size {
                    store.add_nar_file(&tmp, &nar_hash, &name, &references)
                } else {
                    Err(anyhow::anyhow!("archive truncated"))
                };
                fs::remove_file(&tmp)?;
                Ok(Value::String(result?))
            }
            Request::AddCask { name, size } => {
                ensure!(size <= MAX_NAR, "archive too large");
                // (Beside where it goes: the Mac's own volume, not /nix.)
                let dir = crate::casks::dir(store.root());
                fs::create_dir_all(&dir)?;
                let tmp = dir.join(format!(".nami-daemon-cask-{}.nar", std::process::id()));
                let mut w = HashingWriter::new(File::create(&tmp)?);
                let copied = std::io::copy(&mut (&mut reader).take(size), &mut w)?;
                let nar_hash = w.finish();
                let result = if copied == size { crate::casks::add_folder(store.root(), &tmp, &nar_hash, &name) } else { Err(anyhow::anyhow!("archive truncated")) };
                fs::remove_file(&tmp)?;
                Ok(Value::String(result?.display().to_string()))
            }
            Request::Build { job } => {
                let index = Index::load(store)?;
                let built = Builder::new(store, &index).run(job)?;
                Ok(serde_json::to_value(built.outputs)?)
            }
            Request::RefreshIndex => Ok(Value::from(Index::refresh(store)?)),
            Request::BuildCatalog { attr } => {
                let index = Index::load(store)?;
                Builder::new(store, &index).build_catalog(&attr)?;
                Ok(Value::Null)
            }
            Request::Replace { roots, swaps } => {
                for p in roots.iter().chain(swaps.keys()).chain(swaps.values()) {
                    ensure!(p.starts_with(&format!("{STORE_DIR}/")) && base_name(p).len() > 33 && valid_store_name(&base_name(p)[33..]), "bad store path {p:?}");
                }
                Ok(serde_json::to_value(crate::replace::rewrite(store, &roots, &swaps)?)?)
            }
            Request::EnsureGraphics => {
                let index = Index::load(store)?;
                Profiles::system(store).enable_graphics(&index)?;
                Ok(Value::Null)
            }
            Request::Run { .. } => unreachable!(),
        }
    })();
    drop(guard);
    reply(&writer, result)
}

fn conn_of(writer: &Arc<Mutex<UnixStream>>) -> UnixStream {
    writer.lock().unwrap().try_clone().expect("socket")
}

/// The uid of the process on the other end of the socket, from the kernel.
fn peer_uid(conn: &UnixStream) -> Result<u32> {
    peer(conn).map(|(_, uid)| uid)
}

/// The process on the other end of the socket, and its uid, from the kernel.
#[cfg(target_os = "linux")]
fn peer(conn: &UnixStream) -> Result<(u32, u32)> {
    use std::os::fd::AsRawFd;
    let mut cred = libc::ucred { pid: 0, uid: u32::MAX, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: getsockopt writes at most `len` bytes into `cred`.
    let rc = unsafe { libc::getsockopt(conn.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len) };
    ensure!(rc == 0, "can't tell who is asking: {}", std::io::Error::last_os_error());
    Ok((cred.pid as u32, cred.uid))
}

/// The process on the other end of the socket, and its uid, from the kernel (macOS: the
/// uid from getpeereid, the pid from LOCAL_PEERPID).
#[cfg(not(target_os = "linux"))]
fn peer(conn: &UnixStream) -> Result<(u32, u32)> {
    use std::os::fd::AsRawFd;
    let (mut uid, mut gid) = (u32::MAX, u32::MAX);
    // SAFETY: getpeereid writes the two ids.
    let rc = unsafe { libc::getpeereid(conn.as_raw_fd(), &mut uid, &mut gid) };
    ensure!(rc == 0 && uid != u32::MAX, "can't tell who is asking: {}", std::io::Error::last_os_error());
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: getsockopt writes at most `len` bytes into `pid` (SOL_LOCAL = 0, LOCAL_PEERPID = 2).
    let _ = unsafe { libc::getsockopt(conn.as_raw_fd(), 0, 2, &mut pid as *mut _ as *mut libc::c_void, &mut len) };
    Ok((pid.max(0) as u32, uid))
}

/// What `nami config` may change for an admin, as root: system.nami,
/// and nothing else (`config <set|unset|add|remove|json|write> ... --file
/// <it>`).
fn config_args_ok(args: &[String]) -> Result<()> {
    ensure!(args.get(1).is_some_and(|a| ["set", "unset", "add", "remove", "json", "write", "unlock", "get"].contains(&a.as_str())), "nami config changes system.nami this way, nothing else");
    let files: Vec<usize> = args.iter().enumerate().filter(|(_, a)| a.starts_with("--file")).map(|(i, _)| i).collect();
    ensure!(files.len() == 1 && args[files[0]] == "--file" && files[0] + 2 == args.len(), "nami config changes system.nami this way, nothing else");
    let canon = |p: &Path| fs::canonicalize(p).ok();
    let system = crate::config::system_file();
    ensure!(canon(Path::new(&args[files[0] + 1])).is_some_and(|f| Some(f) == canon(&system)), "only {} is changed this way", system.display());
    Ok(())
}

/// What `nami new` may do as root: one of nami's packages copied into the computer's folder
/// (`new <it> --from <package>`), for an add-on of Settings', nothing else.
fn new_args_ok(args: &[String]) -> Result<()> {
    const NO: &str = "nami new copies one of nami's packages into the computer's folder this way, nothing else";
    ensure!(args.len() == 4 && args[2] == "--from", "{NO}");
    let name = &args[3];
    ensure!(name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit()) && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && crate::new::is_package(name), "{NO}");
    let canon = |p: &Path| fs::canonicalize(p).ok();
    let system = crate::config::system_file();
    ensure!(canon(Path::new(&args[1])).is_some_and(|d| system.parent().and_then(canon) == Some(d)), "{NO}");
    Ok(())
}

/// What a command changes for everyone, in words for the password's box
/// (its asking for one); None if it needs none. They change the computer's
/// file, or who uses the computer: as on any computer, an admin's password
/// says it's them, there and then. (Applying the file as it is, updates and
/// cleaning up need none: the file is as an admin wanted it.)
pub fn asks_password(args: &[String]) -> Option<String> {
    let first = args.first()?.as_str();
    let named: Vec<&str> = args[1..].iter().map(String::as_str).filter(|a| !a.starts_with('-')).collect();
    let list = |v: &[&str]| match v {
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        [] => "apps".to_string(),
    };
    match first {
        // (What Settings says is changing, a sentence of its own: `nami config unlock <about>`.)
        "config" if args.get(1).is_some_and(|a| a == "unlock") && args.get(2).is_some_and(|a| a != "--file" && !a.trim().is_empty()) => Some(args[2].trim().chars().take(200).collect()),
        // (Reading it, once unlocked: never asked for on its own, see `unlocked`.)
        "config" if args.get(1).is_some_and(|a| a == "get") => Some("Settings is showing this computer's settings.".into()),
        "config" => Some("A setting for everyone on this computer is changing.".into()),
        "install" => Some(format!("{} {} being installed on the system, for everyone on this computer.", list(&named), if named.len() == 1 { "is" } else { "are" })),
        "new" => Some(format!("{} is being set up on the system, for everyone on this computer.", args.last().map(String::as_str).unwrap_or("An app"))),
        "remove" => Some(format!("{} {} being removed from the system, for everyone on this computer.", list(&named), if named.len() == 1 { "is" } else { "are" })),
        "undo" | "rollback" => Some("This computer is going back to an earlier change, for everyone on it.".into()),
        // The computer's own file again, from the repo it was made from (an admin's first login
        // applies it, before there's a desktop to ask in): nothing changes for anyone.
        "system-use" if same_as_computers(args) => None,
        "system-use" | "system-init" => Some("This computer's settings are changing, for everyone on it.".into()),
        "apply" if args.iter().any(|a| a == "--password-stdin") => Some("Someone's password on this computer is changing.".into()),
        // Another file than the computer's own becomes it.
        "apply" if named.iter().any(|f| fs::canonicalize(f).ok() != fs::canonicalize(crate::config::system_file()).ok()) => Some("This computer's settings are changing, for everyone on it.".into()),
        _ => None,
    }
}

/// What someone who isn't an admin is asked, by polkit, for a command an admin may run: an
/// admin's name and password, for every one of them (the few minutes polkit keeps it for
/// the window that asked, `auth_admin_keep`, is what Settings' Unlock is), and whether it may
/// ask: a read (`config get`) never does, and goes only while one given a moment ago is kept.
pub fn unlocked(args: &[String]) -> (String, bool) {
    if args.first().is_some_and(|a| a == "config") && args.get(1).is_some_and(|a| a == "get") {
        return (asks_password(args).unwrap_or_default(), false);
    }
    let about = match args.first().map(String::as_str) {
        Some("os-update" | "self-update" | "upgrade") => "Updates are being installed on the system, for everyone on this computer.".to_string(),
        _ => asks_password(args).unwrap_or_else(|| "This computer's settings are changing, for everyone on it.".into()),
    };
    (about, true)
}

/// `system-use --from <folder>` (a full path) of a computer's file that says what the
/// computer's says already.
fn same_as_computers(args: &[String]) -> bool {
    let Some(from) = args.iter().position(|a| a == "--from").and_then(|i| args.get(i + 1)).map(Path::new).filter(|p| p.is_absolute()) else { return false };
    let file = if from.is_dir() { crate::oldnames::find_in(from, crate::config::SYSTEM_FILE) } else { from.to_path_buf() };
    match (fs::read(&file), fs::read(crate::config::system_file())) {
        (Ok(theirs), Ok(ours)) => theirs == ours,
        _ => false,
    }
}

/// Who polkit's asked about for nami process `pid` (of `uid`): the program that started it,
/// theirs too (Settings, a terminal's shell), as sudo goes by the terminal. A password given
/// for one change then does for the rest of it (polkit keeps it a few minutes for whoever it
/// was given to): an app going on for everyone writes system.nami more than once,
/// each with a nami of its own. Else the process itself.
pub fn asker(pid: u32, uid: u32) -> u32 {
    use std::os::unix::fs::MetadataExt;
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    let parent = stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(1)).and_then(|p| p.parse::<u32>().ok());
    match parent {
        Some(p) if p > 1 && fs::metadata(format!("/proc/{p}")).is_ok_and(|m| m.uid() == uid) => p,
        _ => pid,
    }
}

/// Whether the nami that asked was run with NAMI_NONINTERACTIVE set: then nothing may wait on a
/// password box for it.
fn noninteractive_peer(pid: u32) -> bool {
    let environ = fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
    environ.split(|b| *b == 0).filter_map(|kv| std::str::from_utf8(kv).ok()).filter_map(|kv| kv.strip_prefix("NAMI_NONINTERACTIVE=")).any(|v| !matches!(v.trim().to_lowercase().as_str(), "" | "0" | "false" | "no"))
}

/// Whether whoever's on the other end of the socket has hung up.
fn gone(conn: &UnixStream) -> bool {
    use std::os::fd::AsRawFd;
    #[cfg(target_os = "linux")]
    let (events, hup) = (libc::POLLRDHUP, libc::POLLRDHUP | libc::POLLHUP | libc::POLLERR);
    // No POLLRDHUP on macOS: readable with nothing to read is the other end gone.
    #[cfg(not(target_os = "linux"))]
    let (events, hup) = (libc::POLLIN, libc::POLLHUP | libc::POLLERR);
    let mut p = libc::pollfd { fd: conn.as_raw_fd(), events, revents: 0 };
    // SAFETY: one pollfd, not waited on.
    let n = unsafe { libc::poll(&mut p, 1, 0) };
    if n > 0 && p.revents & hup != 0 {
        return true;
    }
    #[cfg(not(target_os = "linux"))]
    if n > 0 && p.revents & libc::POLLIN != 0 {
        let mut b = 0u8;
        // SAFETY: peeks at one byte at most, not waited on.
        return unsafe { libc::recv(conn.as_raw_fd(), &mut b as *mut u8 as *mut libc::c_void, 1, libc::MSG_PEEK | libc::MSG_DONTWAIT) } == 0;
    }
    false
}

/// The polkit action an admin's change is asked for under, and where its
/// policy is: namiOS's (examples/packages/polkit), else polkit's own for
/// running something as root, which asks for an admin's password too.
pub const POLKIT_ACTION: &str = "org.namios.change-computer";
/// tray's name for it: a computer whose policy is still tray's has only this one.
const OLD_POLKIT_ACTION: &str = "org.trayos.change-computer";
const POLKIT_ACTIONS: &[&str] = &["/run/current-system/sw/share/polkit-1/actions", "/usr/share/polkit-1/actions", "/etc/polkit-1/actions"];

/// polkit's tools (pkcheck, pkttyagent) where this computer has them.
pub fn polkit_tool(name: &str) -> Option<std::path::PathBuf> {
    ["/run/current-system/sw/bin", "/usr/bin", "/bin"].iter().map(|d| Path::new(d).join(name)).find(|p| p.is_file())
}

/// Ask polkit whether process `pid` (of `uid`) may: an admin's password,
/// asked where they are (their desktop's box, or their terminal's); not
/// `interactive`, only what polkit keeps of one given a moment ago. Without
/// polkit on the computer (a container, a server), admins are trusted as
/// before; someone who isn't one (`admin` false) never is. Err: not allowed,
/// in plain words.
fn allowed(conn: &UnixStream, pid: u32, uid: u32, about: &str, admin: bool, interactive: bool) -> Result<()> {
    if uid == 0 {
        return Ok(());
    }
    const NO_POLKIT: &str = "This requires an administrator's name and password, and there's no polkit on this computer to ask for them.";
    let Some(pkcheck) = polkit_tool("pkcheck") else {
        ensure!(admin, "{NO_POLKIT}");
        return Ok(());
    };
    let has = |a: &str| POLKIT_ACTIONS.iter().any(|d| Path::new(d).join(format!("{a}.policy")).is_file());
    let action = [POLKIT_ACTION, OLD_POLKIT_ACTION].into_iter().find(|a| has(a)).unwrap_or("org.freedesktop.policykit.exec");
    // The process as polkit tells it from another that took its number: when it started.
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    let start = stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(19)).unwrap_or("0");
    let mut child = std::process::Command::new(pkcheck)
        .args(["--action-id", action, "--process", &format!("{pid},{start},{uid}")])
        .args(interactive.then_some("--allow-user-interaction"))
        // (trayos.about too: tray-desktop's password box reads that one.)
        .args(["--detail", "polkit.message", about, "--detail", "namios.about", about, "--detail", "trayos.about", about])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("asking polkit")?;
    // (Nobody answering for 10 minutes: no. Nor once whoever asked has gone: the box goes too.)
    let end = std::time::Instant::now() + std::time::Duration::from_secs(600);
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break Some(s);
        }
        if std::time::Instant::now() > end || gone(conn) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let mut said = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut said);
    }
    match status.and_then(|s| s.code()) {
        Some(0) => Ok(()),
        // polkit itself isn't running: nothing to ask with.
        _ if said.contains("org.freedesktop.PolicyKit1") && (said.contains("not provided") || said.contains("ServiceUnknown") || said.contains("NameHasNoOwner")) => {
            ensure!(admin, "{NO_POLKIT}");
            Ok(())
        }
        _ if !admin && !interactive => bail!("That needs an administrator's name and password, given a moment ago."),
        _ if !admin => bail!("Not changed: that needs an administrator's name and password."),
        _ => bail!("Not changed: that needs an admin's password."),
    }
}

fn run_for_admin(conn: &UnixStream, writer: &Arc<Mutex<UnixStream>>, args: Vec<String>, cwd: String, stdin: Option<String>) -> Result<()> {
    let send = |v: Value| {
        let _ = writeln!(writer.lock().unwrap(), "{v}");
    };
    let who = (|| -> Result<String> {
        let uid = peer_uid(conn)?;
        let (name, gid) = crate::profile::user_by_uid(uid).ok_or_else(|| anyhow::anyhow!("no user with uid {uid}"))?;
        // namiOS's login screen sets up a computer from an image for anyone: namiOS says
        // who may (`nami first-person`).
        if let Some(allowed) = crate::nami_os::plugin().and_then(|os| os.daemon_allows(&args, uid, &name)) {
            allowed?;
            return Ok(name);
        }
        let admin = uid == 0 || crate::profile::is_admin_user(&name, gid);
        ensure!(args.first().is_some_and(|a| ADMIN_COMMANDS.contains(&a.as_str())), "only {} can run this way", ADMIN_COMMANDS.join(", "));
        if args.first().is_some_and(|a| a == "config") {
            config_args_ok(&args)?;
        }
        if args.first().is_some_and(|a| a == "new") {
            new_args_ok(&args)?;
        }
        // Someone who isn't an admin: an admin's name and password, for every command (polkit's
        // auth_admin; what it keeps for a few minutes is the asking window's alone).
        let ask = if admin { asks_password(&args).map(|a| (a, true)) } else { Some(unlocked(&args)) };
        if let Some((about, interactive)) = ask {
            let (pid, uid) = peer(conn)?;
            // (NAMI_NONINTERACTIVE=1 where it was run: no password box comes up, it says no.)
            let interactive = interactive && !noninteractive_peer(pid);
            allowed(conn, asker(pid, uid), uid, &about, admin, interactive)?;
        }
        Ok(name)
    })();
    let name = match who {
        Ok(n) => n,
        Err(e) => {
            send(json!({ "error": format!("{e:#}") }));
            return Ok(());
        }
    };
    eprintln!("nami daemon: {name}: nami {}", args.join(" "));
    let child = std::process::Command::new(own_program()?)
        .args(&args)
        .current_dir(&cwd)
        .env("NAMI_FOR", &name)
        // Its output goes to the client plain; the client colours it.
        .env_remove("CLICOLOR_FORCE")
        .stdin(if stdin.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            send(json!({ "error": format!("starting nami: {e}") }));
            return Ok(());
        }
    };
    if let (Some(text), Some(mut input)) = (stdin, child.stdin.take()) {
        let _ = input.write_all(text.as_bytes());
    }
    let out = child.stdout.take().unwrap();
    let w = writer.clone();
    let t = std::thread::spawn(move || {
        for l in lossy_lines(out) {
            let _ = writeln!(w.lock().unwrap(), "{}", json!({ "out": l }));
        }
    });
    for l in lossy_lines(child.stderr.take().unwrap()) {
        send(json!({ "log": l }));
    }
    let _ = t.join();
    let code = child.wait().map(|s| s.code().unwrap_or(1)).unwrap_or(1);
    send(json!({ "exit": code }));
    Ok(())
}

fn reply(writer: &Arc<Mutex<UnixStream>>, result: Result<Value>) -> Result<()> {
    let msg = match result {
        Ok(v) => json!({ "ok": v }),
        Err(e) => json!({ "error": format!("{e:#}") }),
    };
    writeln!(writer.lock().unwrap(), "{msg}")?;
    Ok(())
}

pub mod client {
    use std::collections::BTreeMap;

    use super::*;

    fn request(socket: &Path, req: &Request, body: Option<&Path>) -> Result<Value> {
        let mut conn = UnixStream::connect(socket).with_context(|| format!("connecting to the nami daemon at {}", socket.display()))?;
        writeln!(conn, "{}", serde_json::to_string(req)?)?;
        if let Some(path) = body {
            std::io::copy(&mut File::open(path)?, &mut conn)?;
        }
        conn.flush()?;
        for line in BufReader::new(conn).lines() {
            let v: Value = serde_json::from_str(&line?)?;
            if let Some(l) = v.get("log").and_then(Value::as_str) {
                crate::say!("{l}");
            } else if let Some(e) = v.get("error") {
                bail!("{}", e.as_str().unwrap_or("daemon error"));
            } else if let Some(ok) = v.get("ok") {
                return Ok(ok.clone());
            }
        }
        bail!("the nami daemon closed the connection")
    }

    pub fn realise(socket: &Path, paths: &[String]) -> Result<()> {
        request(socket, &Request::Realise { paths: paths.to_vec() }, None).map(|_| ())
    }

    /// A cask's apps (an archive of them), into nami's casks folder (casks.rs): the folder.
    pub fn add_cask(socket: &Path, nar_file: &Path, name: &str) -> Result<String> {
        let size = fs::metadata(nar_file)?.len();
        let v = request(socket, &Request::AddCask { name: name.into(), size }, Some(nar_file))?;
        v.as_str().map(String::from).context("bad daemon reply")
    }

    pub fn add_nar(socket: &Path, nar_file: &Path, name: &str, references: &[String]) -> Result<String> {
        let size = fs::metadata(nar_file)?.len();
        let v = request(socket, &Request::AddNar { name: name.into(), size, references: references.to_vec() }, Some(nar_file))?;
        v.as_str().map(String::from).context("bad daemon reply")
    }

    pub fn build(socket: &Path, job: &Job) -> Result<BTreeMap<String, String>> {
        Ok(serde_json::from_value(request(socket, &Request::Build { job: job.clone() }, None)?)?)
    }

    pub fn replace(socket: &Path, roots: &[String], swaps: &BTreeMap<String, String>) -> Result<Vec<String>> {
        Ok(serde_json::from_value(request(socket, &Request::Replace { roots: roots.to_vec(), swaps: swaps.clone() }, None)?)?)
    }

    pub fn build_catalog(socket: &Path, attr: &str) -> Result<()> {
        request(socket, &Request::BuildCatalog { attr: attr.into() }, None).map(|_| ())
    }

    pub fn refresh_index(socket: &Path) -> Result<usize> {
        request(socket, &Request::RefreshIndex, None)?.as_u64().map(|n| n as usize).context("bad daemon reply")
    }

    /// Run `nami <args>` as root through the daemon (admins only); its
    /// output is printed here as it comes. Returns the exit code. With
    /// --password-stdin, what comes on stdin goes along as its input.
    pub fn run(socket: &Path, args: &[String], cwd: &Path) -> Result<i32> {
        let stdin = if args.iter().any(|a| a == "--password-stdin") {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
            Some(text)
        } else {
            None
        };
        run_with(socket, args, cwd, stdin)
    }

    /// The same, with `stdin` for the command's input.
    pub fn run_with(socket: &Path, args: &[String], cwd: &Path, stdin: Option<String>) -> Result<i32> {
        // An admin's password, if it asks for one: asked in this terminal
        // when there's no desktop to ask in (polkit's agent for it).
        let _asker = asks_password(args).and_then(|_| tty_agent());
        let mut conn = UnixStream::connect(socket).with_context(|| format!("connecting to the nami daemon at {}", socket.display()))?;
        writeln!(conn, "{}", serde_json::to_string(&Request::Run { args: args.to_vec(), cwd: cwd.display().to_string(), stdin })?)?;
        conn.flush()?;
        for line in BufReader::new(conn).lines() {
            let v: Value = serde_json::from_str(&line?)?;
            // (Plain from the daemon: coloured here, for this terminal.)
            if let Some(l) = v.get("out").and_then(Value::as_str) {
                crate::log::settle();
                println!("{}", crate::style::out().line(l));
            } else if let Some(l) = v.get("log").and_then(Value::as_str) {
                // (Progress as one line, on a terminal.)
                crate::log::to_stderr(l);
            } else if let Some(e) = v.get("error") {
                bail!("{}", e.as_str().unwrap_or("daemon error"));
            } else if let Some(code) = v.get("exit").and_then(Value::as_i64) {
                crate::log::settle();
                return Ok(code as i32);
            }
        }
        bail!("the nami daemon closed the connection")
    }

    /// polkit's agent for this terminal (pkttyagent), for this process alone
    /// and only if its session has none of its own (a desktop's): there
    /// while it's kept. None off a terminal, or without one.
    struct TtyAgent(std::process::Child);
    impl Drop for TtyAgent {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn tty_agent() -> Option<TtyAgent> {
        use std::os::fd::{FromRawFd, OwnedFd};
        // (Nobody to ask: no terminal, or NAMI_NONINTERACTIVE.)
        if !crate::errors::may_ask() {
            return None;
        }
        let agent = polkit_tool("pkttyagent")?;
        // It closes the pipe once it's taken: then the daemon may ask.
        let mut fds = [0; 2];
        // SAFETY: pipe writes two descriptors into `fds`.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return None;
        }
        // SAFETY: both are ours, just made.
        let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        let child = std::process::Command::new(agent)
            .args(["--process", &asker(std::process::id(), unsafe { libc::getuid() }).to_string(), "--fallback", "--notify-fd", &fds[1].to_string()])
            .stdout(std::process::Stdio::null())
            .spawn()
            .ok()?;
        drop(write);
        let mut ready = [0u8; 1];
        let _ = std::fs::File::from(read).read(&mut ready);
        Some(TtyAgent(child))
    }

    pub fn ensure_graphics(socket: &Path) -> Result<()> {
        request(socket, &Request::EnsureGraphics, None).map(|_| ())
    }
}

/// Lines of `r`, invalid UTF-8 replaced rather than ending the read: a reader
/// that stopped early would kill the writer with SIGPIPE.
fn lossy_lines(r: impl std::io::Read) -> impl Iterator<Item = String> {
    let mut r = BufReader::new(r);
    std::iter::from_fn(move || {
        let mut buf = Vec::new();
        match r.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(String::from_utf8_lossy(&buf).trim_end_matches('\n').to_string()),
        }
    })
}

/// nami's own program, to run for someone: as it is now. Replaced since the daemon started (an
/// update), the kernel names the one it started from "… (deleted)", which can't be started
/// again: the new one at the same place is.
fn own_program() -> std::io::Result<std::path::PathBuf> {
    Ok(now_at(std::env::current_exe()?))
}

fn now_at(exe: std::path::PathBuf) -> std::path::PathBuf {
    match exe.to_str().and_then(|p| p.strip_suffix(" (deleted)")) {
        Some(p) if std::path::Path::new(p).exists() => p.into(),
        _ => exe,
    }
}

#[cfg(test)]
mod tests {
    fn args(line: &str) -> Vec<String> {
        line.split(' ').map(String::from).collect()
    }

    #[test]
    fn a_change_for_everyone_asks_for_a_password() {
        use super::asks_password;
        assert_eq!(asks_password(&args("install gimp vlc")).as_deref(), Some("gimp and vlc are being installed on the system, for everyone on this computer."));
        assert!(asks_password(&args("remove gimp")).unwrap().starts_with("gimp is being removed"));
        assert!(asks_password(&args("config json - --file /etc/nami/system.nami")).is_some());
        assert_eq!(asks_password(&args("config unlock --file /etc/nami/system.nami")).as_deref(), Some("A setting for everyone on this computer is changing."));
        let said = vec!["config".to_string(), "unlock".into(), "The firewall is turning off, for everyone on this computer.".into(), "--file".into(), "/etc/nami/system.nami".into()];
        assert_eq!(asks_password(&said).as_deref(), Some("The firewall is turning off, for everyone on this computer."));
        assert!(asks_password(&args("undo 3")).is_some());
        assert!(asks_password(&args("rollback")).is_some());
        assert!(asks_password(&args("apply --password-stdin")).is_some());
        // The file as it is, updates, cleaning up: nothing to ask.
        for none in ["apply", "upgrade", "gc --keep 5", "self-update", "update --refresh", "layers --update"] {
            assert_eq!(asks_password(&args(none)), None, "{none}");
        }
    }

    #[test]
    fn someone_not_an_admin_is_asked_for_every_command_and_never_for_a_read() {
        use super::unlocked;
        // (A change: asked, saying what it is; one that asks an admin nothing asks them too.)
        assert_eq!(unlocked(&args("install gimp")), ("gimp is being installed on the system, for everyone on this computer.".into(), true));
        for cmd in ["apply /etc/nami/system.nami", "upgrade", "gc --keep 5", "config json - --file /etc/nami/system.nami"] {
            let (about, asks) = unlocked(&args(cmd));
            assert!(asks && !about.is_empty(), "{cmd}");
        }
        // Reading the file: only while a password given a moment ago is kept.
        assert!(!unlocked(&args("config get --json --file /etc/nami/system.nami")).1);
    }

    #[test]
    fn nami_config_changes_only_the_system_file() {
        use super::config_args_ok;
        // nami new: one of its packages, into the computer's folder alone.
        assert!(super::new_args_ok(&args("new /tmp --from docker")).is_err());
        assert!(super::new_args_ok(&args("new /etc/nami --from no-such-package-here")).is_err());
        assert!(super::new_args_ok(&args("new /etc/nami --from docker --force")).is_err());
        // Not a change, or another file than the computer's, or two.
        assert!(config_args_ok(&args("config path --file /etc/nami/system.nami")).is_err());
        assert!(config_args_ok(&args("config json - --file /etc/shadow")).is_err());
        assert!(config_args_ok(&args("config json - --file /etc/nami/system.nami --file /etc/shadow")).is_err());
        assert!(config_args_ok(&args("config json - --file=/etc/shadow")).is_err());
        assert!(config_args_ok(&args("config json -")).is_err());
    }

    #[test]
    fn who_waits_their_turn_is_told_what_for() {
        // `nami run` with shellcheck behind someone's long build: it waited
        // without a word, as the daemon served one connection at a time.
        use super::*;
        let turns = Turns::default();
        let (busy_end, _busy_client) = UnixStream::pair().unwrap();
        let busy = turns.take(&Arc::new(Mutex::new(busy_end)), "building tray-gui-0.18.5".into());
        let (end, client) = UnixStream::pair().unwrap();
        std::thread::scope(|s| {
            let waiter = s.spawn(|| {
                let _turn = turns.take(&Arc::new(Mutex::new(end)), "downloading shellcheck-0.11.0-bin".into());
            });
            let mut line = String::new();
            BufReader::new(&client).read_line(&mut line).unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(v["log"], "waiting for another nami to finish (building tray-gui-0.18.5); this goes on once it's done");
            assert!(!waiter.is_finished(), "it waits for its turn");
            drop(busy);
            waiter.join().unwrap();
        });
        // Its turn is said to the next one who waits.
        assert_eq!(*turns.now.lock().unwrap(), "downloading shellcheck-0.11.0-bin");
        assert_eq!(doing(&Request::Realise { paths: vec!["/nix/store/yrynapygpcy20zxrwr15n1g769vk59ji-shellcheck-0.11.0-bin".into()] }), "downloading shellcheck-0.11.0-bin");
        // Nobody waiting: nothing said.
        let (end, client) = UnixStream::pair().unwrap();
        let end = Arc::new(Mutex::new(end));
        drop(turns.take(&end, "x".into()));
        client.set_nonblocking(true).unwrap();
        assert!(BufReader::new(&client).read_line(&mut String::new()).is_err());
    }

    #[test]
    fn runs_nami_as_it_is_now_after_an_update() {
        let dir = std::env::temp_dir().join(format!("nami-daemon-exe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let nami = dir.join("nami");
        std::fs::write(&nami, "").unwrap();
        assert_eq!(super::now_at(dir.join("nami (deleted)")), nami);
        assert_eq!(super::now_at(nami.clone()), nami);
        // (Gone for good: as it was, to say so.)
        assert_eq!(super::now_at(dir.join("gone (deleted)")), dir.join("gone (deleted)"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
