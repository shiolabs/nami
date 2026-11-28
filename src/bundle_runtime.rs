//! The start of every bundle `nami bundle` makes: a small static program
//! that runs the program the bundle carries, on any glibc Linux, with
//! nothing installed first. See src/bundle_format.rs for the file, and
//! docs/guide/ship-your-app.md for why it works this way.
//!
//! On start:
//! 1. The store paths the bundle carries (its parts) are unpacked into one
//!    store every bundle shares, `~/.cache/tray-bundle/store/<hash>-<name>`,
//!    with `/nix/store` replaced by `/tmp/<prefix>` (the person's link to
//!    that folder, made here; the same length). A part another bundle
//!    unpacked is there already; one the host's own /nix/store has (NixOS,
//!    namiOS), byte for byte, is linked to; a thin bundle's are fetched.
//! 2. The host's graphics drivers are found (they must match its kernel),
//!    with the libraries they need, and listed in the bundle's glibc's own
//!    ld.so.cache: only the bundle's loader reads it, so nothing leaks into
//!    the host's programs the app starts (LD_LIBRARY_PATH would).
//! 3. The program is started; its loader is the bundle's own glibc's.
//!    The host's /lib is never read for the program's own libraries.
//!
//! Two programs are built from this: nami-bundle-runtime (src/bin), and
//! nami-bundle-runtime-net, which can also fetch a thin bundle's parts.
//! Each brings its own `crate::fetch`.

use crate::{bundle_format, elf};

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, IsTerminal, Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use crate::bundle_format::{Meta, PathEntry, Trailer, STORE};
use sha2::{Digest, Sha256};

const HELP: &str = "\
This program is a bundle made by `nami bundle`: everything it needs is inside.
Its own options (anything else goes to the program):
  --nami-bundle-info        what it is: name, version, hash, contents
  --nami-bundle-verify      check every part against the bundle's hash
  --nami-bundle-extract     unpack it (as it runs) and say where
  --nami-bundle-gl          which of this computer's graphics drivers it uses
  --nami-bundle-install     put it in the app menu (~/.local/share/applications)
  --nami-bundle-uninstall   take it out again
  --nami-bundle-trust       trust this copy's key from now on, in place of the one
                            trusted before (when its developer really changed theirs)
  --nami-bundle-clean       remove unpacked parts no bundle here needs any more
  --nami-bundle-update      look for a newer version now, and update to it
  --nami-bundle-updates on|off  whether it updates itself (if it was made to)
Variables: NAMI_BUNDLE_GL=host|bundled|none, NAMI_BUNDLE_CACHE=<dir>,
NAMI_BUNDLE_UPDATE=0 (no updates this time), NAMI_BUNDLE_ASK=0 (don't offer
to add it to the app menu), NAMI_BUNDLE_SANDBOX=require|off (a sandboxed
bundle: refuse to run without bubblewrap, or run without it), NAMI_BUNDLE_DEBUG=1.
(--tray-bundle-… and TRAY_BUNDLE_…, their names before nami, work too.)";

pub fn main() {
    // A bundle made before the rename, or anyone, says TRAY_BUNDLE_*: as nami's.
    for (k, v) in bundle_format::tray_environment() {
        std::env::set_var(k, v);
    }
    let name = std::env::args_os().next().map(|a| Path::new(&a).file_name().unwrap_or_default().to_string_lossy().into_owned()).unwrap_or_default();
    if let Err(e) = run() {
        eprintln!("{name}: {e:#}");
        std::process::exit(127);
    }
}

pub fn debug() -> bool {
    std::env::var_os("NAMI_BUNDLE_DEBUG").is_some_and(|v| !v.is_empty() && v != "0")
}

macro_rules! note {
    ($($t:tt)*) => { if crate::runtime::debug() { eprintln!("nami-bundle: {}", format!($($t)*)); } };
}

struct Bundle {
    /// The file people run: the bundle, or the AppImage.
    path: PathBuf,
    source: Source,
    /// The bundle's hash (its meta's sha256).
    hash: String,
    meta: Meta,
}

enum Source {
    /// The one file: frames at offsets.
    File(File, Trailer),
    /// An AppImage's folder (mounted, or extracted): the store paths as
    /// files, in `<APPDIR>/.nami-bundle/store`.
    AppDir(PathBuf),
}

fn open_self() -> Result<Bundle> {
    let exe = fs::read_link("/proc/self/exe").or_else(|_| std::env::current_exe()).context("finding the bundle")?;
    // Started as an AppImage's AppRun.
    if let Some(dir) = std::env::var_os("APPDIR").map(PathBuf::from).filter(|d| fs::canonicalize(d.join("AppRun")).ok().as_deref() == Some(exe.as_path())) {
        let data = fs::read(appdir_bundle(&dir).join("meta.json")).context("reading the AppImage's .tray-bundle/meta.json")?;
        let meta: Meta = serde_json::from_slice(&data).context("reading the AppImage's .tray-bundle/meta.json")?;
        if meta.paths.iter().any(|p| !bundle_format::valid_name(&p.name)) {
            bail!("damaged AppImage (a path's name)");
        }
        let path = std::env::var_os("APPIMAGE").map(PathBuf::from).unwrap_or_else(|| exe.clone());
        return Ok(Bundle { path, source: Source::AppDir(dir), hash: bundle_format::sha256_hex(&data), meta });
    }
    let mut file = File::open(&exe).with_context(|| format!("opening {}", exe.display()))?;
    let (trailer, meta) = bundle_format::read(&mut file).with_context(|| format!("reading {}", exe.display()))?;
    // (nami bundle --arch: one for another kind of computer says so.)
    if let Some(p) = meta.arch_problem() {
        bail!("{p}");
    }
    Ok(Bundle { path: exe, hash: trailer.hash(), source: Source::File(file, trailer), meta })
}

/// An AppImage's own folder of its bundle: `.tray-bundle`, which nami 0.18.6
/// to 0.18.8 named `.nami-bundle`.
fn appdir_bundle(dir: &Path) -> PathBuf {
    let nami = dir.join(".nami-bundle");
    if !dir.join(".tray-bundle").is_dir() && nami.is_dir() {
        return nami;
    }
    dir.join(".tray-bundle")
}

fn run() -> Result<()> {
    let mut args: Vec<OsString> = std::env::args_os().collect();
    // tray's words for its options (`--tray-bundle-info`): the same.
    if let Some(rest) = args.get(1).and_then(|a| a.to_str()).and_then(|a| a.strip_prefix("--tray-bundle-")).filter(|r| !r.is_empty()) {
        args[1] = format!("--nami-bundle-{rest}").into();
    }
    let argv0 = args.first().cloned().unwrap_or_default();
    let mut b = open_self()?;
    let first = args.get(1).and_then(|a| a.to_str()).unwrap_or("").to_string();
    // A newer version that's ready replaces this one, which starts it instead.
    if matches!(b.source, Source::File(..)) && (first == "--nami-bundle-update" || !first.starts_with("--nami-bundle-")) {
        if let Some(version) = crate::update::at_start(&b.path, &b.meta) {
            if io::stderr().is_terminal() && first.is_empty() {
                eprintln!("{}: updated to {version}", b.meta.name);
            }
            let e = std::process::Command::new(&b.path).arg0(&argv0).args(&args[1..]).env("NAMI_BUNDLE_UPDATED", &b.meta.version).env("TRAY_BUNDLE_UPDATED", &b.meta.version).exec();
            bail!("starting the new version, {}: {e}", b.path.display());
        }
    }
    match args.get(1).and_then(|a| a.to_str()) {
        Some("--nami-bundle-help") => {
            println!("{HELP}");
            return Ok(());
        }
        Some("--nami-bundle-info") => return info(&b),
        Some("--nami-bundle-trust") => {
            let u = b.meta.update.as_ref().context("this bundle isn't signed (it was made without --update-url)")?;
            crate::bundle_trust::forget(&b.meta);
            crate::bundle_trust::record(&b.meta)?;
            println!("{}'s updates and new copies are now checked against the key {}", b.meta.name, crate::update::fingerprint(&u.key).unwrap_or_default());
            return Ok(());
        }
        Some("--nami-bundle-update" | "--nami-bundle-updates") if matches!(b.source, Source::File(..)) => {
            let cache = Cache::open(Some(&b.hash))?;
            let rest: Vec<String> = args[1..].iter().map(|a| a.to_string_lossy().into_owned()).collect();
            return crate::update::command(&rest, &crate::update::Me { path: &b.path, meta: &b.meta, hash: &b.hash, cache: &cache.root });
        }
        Some("--nami-bundle-clean") => {
            // Each store there is: under tray's name and nami's.
            let (mut bundles, mut parts) = (0, 0);
            for cache in Cache::all() {
                cache.lock()?;
                let (b, p) = cache.gc()?;
                (bundles, parts) = (bundles + b, parts + p);
            }
            println!("removed {parts} parts, and {bundles} bundles that are gone");
            return Ok(());
        }
        Some("--nami-bundle-verify") => {
            verify(&mut b)?;
            println!("{}: all {} parts match its hash {}", b.path.display(), b.meta.paths.len(), b.hash);
            return Ok(());
        }
        _ => {}
    }
    let cache = Cache::open(Some(&b.hash))?;
    let store = cache.unpack(&mut b)?;
    match args.get(1).and_then(|a| a.to_str()) {
        Some("--nami-bundle-extract") => {
            println!("{}", store.dir.display());
            println!("runs {}", store.local(&b.meta.entry).display());
            return Ok(());
        }
        Some("--nami-bundle-gl") => {
            let gl = graphics(&b, &store, &cache);
            for line in &gl.notes {
                println!("{line}");
            }
            for (k, v) in &gl.env {
                println!("{k}={v}");
            }
            return Ok(());
        }
        Some("--nami-bundle-install") => return install(&b, &store, true),
        Some("--nami-bundle-uninstall") => return install(&b, &store, false),
        _ => {}
    }
    // Which program: the one this was started as (a link named `jq`), else its own.
    let called = Path::new(&argv0).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let own = b.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (name, program) = match b.meta.programs.get(&called).filter(|_| called != own) {
        Some(p) => (called.clone(), p.clone()),
        None => (b.meta.entry.rsplit('/').next().unwrap_or("").to_string(), b.meta.entry.clone()),
    };
    let program = store.local(&program);
    let gl = graphics(&b, &store, &cache);
    // What runs: the program, or the bundle's loader with it.
    let (exe, lead): (PathBuf, Vec<OsString>) = match &b.meta.loader {
        Some(l) => {
            let libs: Vec<String> = l.library_path.iter().map(|d| store.local(d).display().to_string()).collect();
            (store.local(&l.path), vec!["--library-path".into(), libs.join(":").into(), "--argv0".into(), name.clone().into(), program.clone().into_os_string()])
        }
        None => (program.clone(), Vec::new()),
    };
    let rest: Vec<OsString> = args.drain(1..).collect();
    let trust = start_trust(&b);
    let sandbox = sandboxed(&b, &store, &cache, &rest)?;
    let mut cmd = match &sandbox {
        Some(sb) => {
            let mut c = std::process::Command::new(&sb.bwrap);
            c.args(&sb.args).arg("--").arg(&exe).args(&lead);
            c
        }
        None => {
            let mut c = std::process::Command::new(&exe);
            if lead.is_empty() {
                c.arg0(&name);
            }
            c.args(&lead);
            c
        }
    };
    cmd.args(rest);
    for (k, v) in environment(&b, &store, &cache, gl) {
        match v {
            Some(v) => cmd.env(k, v),
            None => cmd.env_remove(k),
        };
    }
    if let Some(sb) = &sandbox {
        for (k, v) in &sb.env {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            };
        }
    }
    cmd.env_remove("NAMI_BUNDLE_UPDATED").env_remove("TRAY_BUNDLE_UPDATED");
    if matches!(b.source, Source::File(..)) {
        use std::os::fd::AsRawFd;
        crate::update::before_start(&crate::update::Me { path: &b.path, meta: &b.meta, hash: &b.hash, cache: &cache.root }, cache._lock.as_raw_fd());
        first_start(&b, &store, &trust, cache._lock.as_raw_fd());
    }
    note!("starting {}", program.display());
    let e = cmd.exec();
    bail!("starting {}: {e}", program.display())
}

// ---------------------------------------------------------------------------
// Who made it, and the first start: src/bundle_trust.rs, src/bundle_ask.rs.

/// A signed bundle's key, as it starts: trusted from its first start here;
/// another than the one trusted for the app is said, once (it runs, but
/// isn't offered a place in the menu, and won't update or install).
fn start_trust(b: &Bundle) -> crate::bundle_trust::Trust {
    use crate::bundle_trust::{self as trust, Trust};
    if !matches!(b.source, Source::File(..)) {
        return Trust::Unsigned;
    }
    let t = crate::update::settle_in(&trust::dir(), &b.meta).unwrap_or(Trust::Unsigned);
    if let (Trust::Changed { was }, Some(u)) = (&t, &b.meta.update) {
        let app = trust::app(&b.meta);
        if crate::bundle_ask::first_time(&app, &format!("key-{}", &bundle_format::sha256_hex(u.key.as_bytes())[..12])) {
            let fp = |k: &str| trust::short(&crate::update::fingerprint(k).unwrap_or_default());
            crate::bundle_ask::tell(
                &b.meta.name,
                "dialog-warning",
                &format!("{} is signed with another key than the one you trusted.", b.meta.name),
                &format!("This copy's key is {}; the one you trusted is {}. It may not be from {}. It won't update or install.", fp(&u.key), fp(was), trust::who(&b.meta)),
            );
        }
    }
    t
}

/// The question's words: "Myapp by Ada Lovelace. Updates are signed by Ada
/// Lovelace (key 3F2A 9C01 5B7E 22D4)."
pub fn about_line(m: &Meta, shown: &str) -> String {
    let key = m.update.as_ref().and_then(|u| crate::update::fingerprint(&u.key)).map(|f| crate::bundle_trust::short(&f));
    let mut s = match &m.developer {
        Some(d) => format!("{shown} by {d}."),
        None => String::new(),
    };
    match key {
        Some(k) => s += &format!("{}Updates are signed by {} (key {k}).", if s.is_empty() { "" } else { " " }, crate::bundle_trust::who(m)),
        None if m.developer.is_some() => s += " Not signed.",
        None => {}
    }
    if m.sandbox.is_some() {
        s += if s.is_empty() { "Runs in a sandbox." } else { " Runs in a sandbox." };
    }
    s
}

/// As an app with a window first starts outside the app menu: "Add Myapp to
/// your apps?", once, from a process of its own (two forks, as the update
/// check), so the app starts at once whatever the desktop does.
fn first_start(b: &Bundle, store: &Store, trust: &crate::bundle_trust::Trust, lock_fd: i32) {
    use crate::bundle_ask::{self as ask, Answer};
    let Some(d) = &b.meta.desktop else { return };
    let app = crate::bundle_trust::app(&b.meta);
    let here = ask::Here {
        gui: true,
        display: ask::display(),
        in_menu: crate::bundle_menu::entry_file(&d.id).exists(),
        answered: ask::answered(&app),
        quiet: std::env::var("NAMI_BUNDLE_ASK").is_ok_and(|v| v == "0" || v == "off"),
        key_changed: matches!(trust, crate::bundle_trust::Trust::Changed { .. }),
    };
    if !ask::should_ask(&here) {
        return;
    }
    let shown = crate::bundle_menu::entry_key(&d.file, "Name").unwrap_or_else(|| b.meta.name.clone());
    let icon = d.icon.as_deref().map(|i| store.real(i).display().to_string()).unwrap_or_default();
    let body = about_line(&b.meta, &shown);
    let file = b.path.clone();
    // SAFETY: fork with one thread (the unpacking threads are done); the
    // first child only forks again and exits.
    unsafe {
        let child = libc::fork();
        if child < 0 {
            return;
        }
        if child > 0 {
            let mut status = 0;
            libc::waitpid(child, &mut status, 0);
            return;
        }
        libc::close(lock_fd);
        if libc::fork() != 0 {
            libc::_exit(0);
        }
        libc::setsid();
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        if null >= 0 {
            for t in 0..3 {
                libc::dup2(null, t);
            }
        }
    }
    let answer = ask::ask(&shown, &icon, &format!("Add {shown} to your apps?"), &body, "Add", "Not Now");
    if let Some(a) = answer {
        ask::remember(&app, a);
        if a == Answer::Add {
            add_to_apps(&file);
        }
    }
    // SAFETY: the end of this process.
    unsafe { libc::_exit(0) }
}

/// Into the app menu: with nami where it's here (namiOS, or nami on another
/// distribution: it keeps the app in the person's user.nami), else as
/// `--nami-bundle-install` does.
fn add_to_apps(file: &Path) {
    let null = std::process::Stdio::null;
    let nami = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("nami")).find(|f| f.is_file()));
    let ok = nami.is_some_and(|n| std::process::Command::new(n).arg("install").arg(file).stdin(null()).stdout(null()).stderr(null()).status().is_ok_and(|s| s.success()));
    if !ok {
        let _ = std::process::Command::new(file).arg(bundle_format::flag(file, "install")).env("NAMI_BUNDLE_ASK", "0").stdin(null()).stdout(null()).stderr(null()).status();
    }
}

// ---------------------------------------------------------------------------
// The sandbox: src/bundle_sandbox.rs.

struct Sandboxed {
    bwrap: PathBuf,
    args: Vec<OsString>,
    env: Vec<(String, Option<String>)>,
}

/// bwrap, if it's here and can make a sandbox on this computer (checked
/// once a boot: a container may not let it).
fn sandbox_works() -> Result<PathBuf, String> {
    let Some(bwrap) = crate::bundle_sandbox::find_bwrap() else { return Err("this computer doesn't have bubblewrap (bwrap)".into()) };
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap_or_default();
    let stamp = format!("{}\n{}\n{}", boot.trim(), bwrap.display(), fs::metadata(&bwrap).map(|m| m.mtime()).unwrap_or(0));
    let mark = Cache::open(None).ok().map(|c| c.root.join("sandbox-works"));
    if mark.as_ref().and_then(|m| fs::read_to_string(m).ok()).as_deref() == Some(stamp.as_str()) {
        return Ok(bwrap);
    }
    let out = std::process::Command::new(&bwrap)
        .args(["--unshare-all", "--die-with-parent", "--ro-bind", "/", "/", "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp", "--", "true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => {
            if let Some(m) = mark {
                let _ = fs::write(m, stamp);
            }
            Ok(bwrap)
        }
        Ok(o) => Err(format!("bubblewrap can't make a sandbox here ({})", String::from_utf8_lossy(&o.stderr).trim().trim_start_matches("bwrap: "))),
        Err(e) => Err(format!("bubblewrap didn't start ({e})")),
    }
}

/// The program in its sandbox, if the bundle asks for one and the computer
/// can make it; without, it runs as it would and says so once
/// (NAMI_BUNDLE_SANDBOX=require: refused instead; =off: not sandboxed).
fn sandboxed(b: &Bundle, store: &Store, cache: &Cache, rest: &[OsString]) -> Result<Option<Sandboxed>> {
    use crate::bundle_sandbox as sb;
    let Some(want) = &b.meta.sandbox else { return Ok(None) };
    let mode = std::env::var("NAMI_BUNDLE_SANDBOX").unwrap_or_default();
    if mode == "off" || mode == "0" {
        note!("not sandboxed (NAMI_BUNDLE_SANDBOX={mode})");
        return Ok(None);
    }
    let bwrap = match sandbox_works() {
        Ok(b) => b,
        Err(why) => {
            if mode == "require" {
                bail!("{} runs in a sandbox, and {why} (NAMI_BUNDLE_SANDBOX=require)", b.meta.name);
            }
            let app = sb::app_id(&b.meta);
            if crate::bundle_ask::first_time(&app, "no-sandbox") {
                crate::bundle_ask::tell(&b.meta.name, "dialog-information", &format!("{} runs without its sandbox.", b.meta.name), &format!("It was made to run in one, but {why}. Install bubblewrap to have it sandboxed."));
            }
            note!("not sandboxed: {why}");
            return Ok(None);
        }
    };
    let home = PathBuf::from(std::env::var_os("HOME").filter(|h| !h.is_empty()).context("a sandboxed bundle needs HOME")?);
    let data_home = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
    let config = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    let app = sb::app_id(&b.meta);
    let data = data_home.join("nami-bundle/sandbox").join(&app);
    fs::create_dir_all(&data).with_context(|| format!("making {}", data.display()))?;
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|d| d.is_dir());
    let wayland = std::env::var_os("WAYLAND_DISPLAY")
        .filter(|v| !v.is_empty())
        .map(|w| {
            let w = PathBuf::from(w);
            if w.is_absolute() { w } else { runtime_dir.clone().unwrap_or_default().join(w) }
        })
        .filter(|w| w.exists());
    // X11's socket: :0 is /tmp/.X11-unix/X0 (an address over the network isn't one).
    let x11 = std::env::var("DISPLAY").ok().and_then(|d| d.strip_prefix(':').map(|n| n.split('.').next().unwrap_or("").to_string())).map(|n| PathBuf::from(format!("/tmp/.X11-unix/X{n}"))).filter(|p| p.exists());
    let xauthority = std::env::var_os("XAUTHORITY").map(PathBuf::from).or_else(|| Some(home.join(".Xauthority"))).filter(|p| p.exists());
    let devs = |prefix: &str| -> Vec<PathBuf> { fs::read_dir("/dev").into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(prefix))).collect() };
    let cwd = std::env::current_dir().unwrap_or_else(|_| home.clone());
    let mut env: Vec<(String, Option<String>)> = vec![("NAMI_BUNDLE_SANDBOXED".into(), Some("1".into())), ("SESSION_MANAGER".into(), None)];
    // The desktop's portals, through a filtered bus (as Flatpak's apps have it), where it can be made.
    let portal_id = sb::portal_id(&b.meta);
    let (bus, info, sync_fd) = match portal_bus(&bwrap, &portal_id, want, runtime_dir.as_deref()) {
        Some((bus, info, fd)) => {
            env.push(("DBUS_SESSION_BUS_ADDRESS".into(), Some(format!("unix:path={}", runtime_dir.clone().unwrap_or_default().join("bus").display()))));
            env.push(("GTK_USE_PORTAL".into(), Some("1".into())));
            (Some(bus), Some(info), Some(fd))
        }
        None => {
            env.push(("DBUS_SESSION_BUS_ADDRESS".into(), None));
            (None, None, None)
        }
    };
    let setup = sb::Setup {
        sandbox: want,
        gui: b.meta.desktop.is_some(),
        home: home.clone(),
        data,
        store_dir: store.dir.clone(),
        prefix: store.prefix.clone(),
        cache_root: cache.root.clone(),
        runtime_dir,
        wayland,
        x11,
        xauthority,
        folders: sb::folders(&home, &config),
        nvidia: devs("nvidia"),
        cameras: devs("video"),
        opened: sb::opened(rest, &cwd),
        bus,
        info,
        sync_fd,
        look: &sb::kind,
    };
    note!("{}", sb::words(Some(want), true));
    Ok(Some(Sandboxed { bwrap, args: sb::args(&setup), env }))
}

/// xdg-dbus-proxy (it comes with Flatpak), started for this run in a
/// sandbox of its own with the same /.flatpak-info, so the portals see a
/// sandboxed app: (its socket, the info file, the fd that keeps it going).
fn portal_bus(bwrap: &Path, portal_id: &str, want: &crate::bundle_format::Sandbox, runtime_dir: Option<&Path>) -> Option<(PathBuf, PathBuf, i32)> {
    let rt = runtime_dir?;
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().filter(|a| !a.is_empty())?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let proxy = std::env::split_paths(&path).chain([PathBuf::from("/usr/libexec"), PathBuf::from("/usr/lib/flatpak")]).map(|d| d.join("xdg-dbus-proxy")).find(|f| f.is_file())?;
    let dir = rt.join("nami-bundle");
    fs::create_dir_all(&dir).ok()?;
    // Earlier runs' (each named by its runtime's pid, which became the sandbox): gone with them.
    for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let pid = name.strip_prefix("info-").or_else(|| name.strip_prefix("bus-")).and_then(|p| p.parse::<u32>().ok());
        if pid.is_some_and(|p| !Path::new(&format!("/proc/{p}")).exists()) {
            let _ = fs::remove_file(e.path());
        }
    }
    let id = std::process::id();
    let (sock, info) = (dir.join(format!("bus-{id}")), dir.join(format!("info-{id}")));
    let _ = fs::remove_file(&sock);
    fs::write(&info, crate::bundle_sandbox::flatpak_info(portal_id, want)).ok()?;
    // A pipe: the proxy writes a byte when it's ready, and ends when the
    // read end closes (the sandbox keeps it open: --sync-fd).
    let mut fds = [0i32; 2];
    // SAFETY: a pipe; its write end goes to the proxy, its read end stays here.
    #[cfg(target_os = "linux")]
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return None;
    }
    // (Bundles run on Linux; this only has to build elsewhere.)
    #[cfg(not(target_os = "linux"))]
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return None;
    }
    let (r, w) = (fds[0], fds[1]);
    let mut c = std::process::Command::new(bwrap);
    c.args(["--ro-bind", "/", "/", "--dev", "/dev", "--bind"]).arg(rt).arg(rt).arg("--ro-bind").arg(&info).arg("/.flatpak-info").args(["--die-with-parent", "--"]).arg(&proxy).arg(format!("--fd={w}")).arg(&address).arg(&sock);
    c.args(crate::bundle_sandbox::proxy_rules(portal_id));
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    // SAFETY: in the child, before exec: the write end stays open for the proxy.
    unsafe {
        c.pre_exec(move || {
            libc::fcntl(w, libc::F_SETFD, 0);
            Ok(())
        });
    }
    let child = c.spawn();
    // SAFETY: our copy of the write end: the proxy has its own.
    unsafe { libc::close(w) };
    if child.is_err() {
        // SAFETY: the read end, unused.
        unsafe { libc::close(r) };
        return None;
    }
    let mut byte = [0u8; 1];
    // SAFETY: one byte from our pipe (0: the proxy ended without starting).
    let n = unsafe { libc::read(r, byte.as_mut_ptr() as *mut libc::c_void, 1) };
    if n != 1 || !sock.exists() {
        // SAFETY: the read end: the proxy ends with it.
        unsafe { libc::close(r) };
        return None;
    }
    // Kept open in the sandbox (bwrap's --sync-fd), so the proxy lives as long as it.
    // SAFETY: clearing FD_CLOEXEC on our own fd.
    unsafe { libc::fcntl(r, libc::F_SETFD, 0) };
    Some((sock, info, r))
}

fn info(b: &Bundle) -> Result<()> {
    let m = &b.meta;
    let size: u64 = m.paths.iter().map(|p| p.size).sum();
    let fetched = m.paths.iter().filter(|p| p.fetch).count();
    let unpacked: u64 = m.paths.iter().map(|p| p.nar_size).sum();
    println!("{} {}", m.name, m.version);
    println!("hash:     {}", b.hash);
    println!("runs:     {}", m.entry);
    if m.programs.len() > 1 {
        println!("programs: {}", m.programs.keys().cloned().collect::<Vec<_>>().join(" "));
    }
    println!("contents: {} store paths, {:.1} MiB ({:.1} MiB unpacked)", m.paths.len(), size as f64 / 1048576.0, unpacked as f64 / 1048576.0);
    if fetched > 0 {
        let from: Vec<&str> = m.substituters.iter().map(|s| s.url.as_str()).collect();
        println!("thin:     {fetched} of them fetched when needed, from {}", from.join(", "));
    }
    if let Some(d) = &m.desktop {
        println!("app:      {}", d.id);
    }
    if let (Source::File(..), Ok(cache)) = (&b.source, Cache::open(Some(&b.hash))) {
        for line in crate::update::info(&crate::update::Me { path: &b.path, meta: m, hash: &b.hash, cache: &cache.root }) {
            println!("{line}");
        }
    }
    for line in who_and_sandbox(b) {
        println!("{line}");
    }
    println!("made by:  {}", m.made_by);
    for p in &m.paths {
        println!("  {}", p.name);
    }
    Ok(())
}

/// --nami-bundle-info's lines on who made it, whether this computer trusts
/// their key, and its sandbox.
fn who_and_sandbox(b: &Bundle) -> Vec<String> {
    use crate::bundle_trust::{self as trust, Trust};
    let m = &b.meta;
    let mut out = Vec::new();
    if let Some(d) = &m.developer {
        out.push(format!("developer: {d}"));
    }
    let who = trust::who(m);
    out.push(match (&m.update, crate::update::trust_in(&trust::dir(), m)) {
        (Some(u), Trust::Same) => format!("signed:   updates are signed by {who} (key {}), the key you trust for it", trust::short(&crate::update::fingerprint(&u.key).unwrap_or_default())),
        (Some(u), Trust::New) => format!("signed:   updates are signed by {who} (key {}): trusted from its first start here", trust::short(&crate::update::fingerprint(&u.key).unwrap_or_default())),
        (Some(u), Trust::Changed { was }) => format!(
            "signed:   by key {}, but you trust another key for it ({}): its updates and new copies are refused",
            trust::short(&crate::update::fingerprint(&u.key).unwrap_or_default()),
            trust::short(&crate::update::fingerprint(&was).unwrap_or_default())
        ),
        _ => "signed:   no (it doesn't update itself)".into(),
    });
    let here = m.sandbox.is_some() && sandbox_works().is_ok();
    out.push(format!("sandbox:  {}", crate::bundle_sandbox::words(m.sandbox.as_ref(), here)));
    out
}

fn verify(b: &mut Bundle) -> Result<()> {
    match &mut b.source {
        Source::File(f, t) => Ok(bundle_format::verify(f, t, &b.meta)?),
        Source::AppDir(_) => bail!("an AppImage: its own signature is what vouches for it (appimagetool --sign)"),
    }
}

/// Where bundles unpack: `~/.cache/tray-bundle` (bundle_format::cache_dirs).
///
/// ```text
/// store/<hash>-<name>   every bundle's parts, each once (or a link to the
///                       host's identical /nix/store/<hash>-<name>)
/// store/.nar/<name>     the NAR hash of the part there (Nix's: what it is)
/// bundles/<hash>        a bundle that unpacked here: its file, name, kind
///                       and parts, until its file is gone or replaced
/// prefix                /tmp/<prefix>, the link to store/ all parts use
/// ```
struct Cache {
    root: PathBuf,
    uid: u32,
    _lock: File,
}

/// The shared store: `dir` is what `prefix` (in /tmp) links to.
struct Store {
    prefix: String,
    dir: PathBuf,
}

impl Store {
    /// A store path, as this computer reaches it: /nix/store/x -> /tmp/<prefix>/x.
    fn local(&self, logical: &str) -> PathBuf {
        match logical.strip_prefix(STORE) {
            Some(rest) => PathBuf::from(format!("{}{rest}", self.prefix)),
            None => PathBuf::from(logical),
        }
    }

    /// The same, but the real folder (not through /tmp).
    fn real(&self, logical: &str) -> PathBuf {
        match logical.strip_prefix(STORE) {
            Some(rest) => self.dir.join(rest.trim_start_matches('/')),
            None => PathBuf::from(logical),
        }
    }

    fn nar_mark(&self, name: &str) -> PathBuf {
        self.dir.join(".nar").join(name)
    }
}

fn uid() -> u32 {
    // SAFETY: getuid can't fail.
    unsafe { libc::getuid() }
}

impl Cache {
    /// The store for the bundle with this hash: where it's unpacked, of
    /// the folders under tray's and nami's names (bundle_format::cache_dirs).
    fn open(hash: Option<&str>) -> Result<Cache> {
        let uid = uid();
        let wanted = bundle_format::pick_cache(&bundle_format::cache_dirs(), hash);
        let fallbacks = bundle_format::cache_fallbacks();
        let fallback = bundle_format::pick_cache(&fallbacks, hash).unwrap_or_else(|| fallbacks[0].clone());
        let root = match wanted.filter(|w| fs::create_dir_all(w).is_ok() && writable(w)) {
            Some(w) => w,
            None => {
                fs::create_dir_all(&fallback).with_context(|| format!("creating {}", fallback.display()))?;
                let m = fs::symlink_metadata(&fallback)?;
                if m.uid() != uid || !m.is_dir() {
                    bail!("{} isn't yours: set NAMI_BUNDLE_CACHE to a folder of your own", fallback.display());
                }
                fallback
            }
        };
        let _ = fs::set_permissions(&root, fs::Permissions::from_mode(0o700));
        let lock = File::create(root.join(".lock")).context("locking the bundle cache")?;
        Ok(Cache { root, uid, _lock: lock })
    }

    /// Every store there is here, to clean: the one bundles use now, and
    /// one under the other name that's there too.
    fn all() -> Vec<Cache> {
        let mut out: Vec<Cache> = Cache::open(None).into_iter().collect();
        for d in bundle_format::cache_dirs().into_iter().chain(bundle_format::cache_fallbacks()) {
            let mine = fs::symlink_metadata(&d).is_ok_and(|m| m.is_dir() && m.uid() == uid());
            if mine && d.join("store").is_dir() && !out.iter().any(|c| c.root == d) && writable(&d) {
                if let Ok(lock) = File::create(d.join(".lock")) {
                    out.push(Cache { root: d, uid: uid(), _lock: lock });
                }
            }
        }
        out
    }

    /// One bundle at a time unpacks, links its graphics drivers or cleans
    /// up (until it starts its program: the lock doesn't outlive exec).
    fn lock(&self) -> Result<()> {
        use std::os::fd::AsRawFd;
        // SAFETY: flock on a file we hold open.
        if unsafe { libc::flock(self._lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            bail!("locking the bundle cache: {}", io::Error::last_os_error());
        }
        Ok(())
    }

    /// The person's prefix in /tmp, linked to the store: the one used
    /// before, or (taken by someone else after a restart) the next free
    /// candidate, which means starting the store again (its files name it).
    fn prefix(&self) -> Result<Store> {
        let dir = self.root.join("store");
        let recorded = fs::read_to_string(self.root.join("prefix")).ok().map(|p| p.trim().to_string()).filter(|p| p.len() == STORE.len() && p.starts_with("/tmp/"));
        let seed = format!("{}:{}", self.uid, self.root.display());
        let candidates = recorded.iter().cloned().chain((0..64).map(|n| bundle_format::prefix_candidate(&seed, n)));
        for prefix in candidates {
            match fs::symlink_metadata(&prefix) {
                Ok(m) if m.file_type().is_symlink() && m.uid() == self.uid => {
                    let to = fs::read_link(&prefix).ok();
                    if to.as_deref() != Some(dir.as_path()) {
                        // Another store of this person's (NAMI_BUNDLE_CACHE) still there: theirs.
                        if to.as_ref().is_some_and(|t| t.is_dir()) {
                            continue;
                        }
                        fs::remove_file(&prefix)?;
                        std::os::unix::fs::symlink(&dir, &prefix)?;
                    }
                }
                Ok(_) => continue,
                Err(e) if e.kind() == io::ErrorKind::NotFound => match std::os::unix::fs::symlink(&dir, &prefix) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(e).with_context(|| format!("making {prefix} (bundles need /tmp)")),
                },
                Err(_) => continue,
            }
            if recorded.as_deref() != Some(prefix.as_str()) {
                if recorded.is_some() {
                    note!("{} was taken: unpacking everything again for {prefix}", recorded.as_deref().unwrap_or(""));
                }
                let _ = remove_tree(&dir);
                let _ = remove_tree(&self.root.join("bundles"));
                fs::write(self.root.join("prefix"), format!("{prefix}\n"))?;
            }
            fs::create_dir_all(dir.join(".nar"))?;
            return Ok(Store { prefix, dir });
        }
        bail!("no free name in /tmp for this person's bundles")
    }

    /// Every part of the bundle in the shared store: those there already
    /// (another bundle's, checked to be the same by their NAR hash) as they
    /// are, the rest unpacked (or linked from the host's store, or fetched),
    /// each checked against its hash before it's used. Then the bundle is
    /// recorded, and what no bundle needs any more is removed.
    fn unpack(&self, b: &mut Bundle) -> Result<Store> {
        self.lock()?;
        let store = self.prefix()?;
        let entry = self.root.join("bundles").join(&b.hash);
        let there = |p: &PathEntry| store.dir.join(&p.name).symlink_metadata().is_ok();
        let record = format!("{}\n{}\n{}\n{}", b.path.display(), b.meta.name, if matches!(b.source, Source::File(..)) { "file" } else { "appimage" }, b.meta.paths.iter().map(|p| format!("{}\n", p.name)).collect::<String>());
        if b.meta.paths.iter().all(there) && fs::read_to_string(&entry).is_ok_and(|e| e == record) {
            return Ok(store);
        }
        for p in b.meta.paths.iter().filter(|p| there(p) && !p.nar_hash.is_empty()) {
            if let Ok(have) = fs::read_to_string(store.nar_mark(&p.name)) {
                if have.trim() != p.nar_hash {
                    bail!("{} is in {} already, and isn't what this bundle has (its NAR hash differs): remove it, or the whole folder", p.name, store.dir.display());
                }
            }
        }
        let todo: Vec<usize> = (0..b.meta.paths.len()).filter(|i| !there(&b.meta.paths[*i])).collect();
        let from_host = host_parts(&b.meta, &todo);
        let bytes: u64 = todo.iter().filter(|i| !from_host.contains(&b.meta.paths[**i].name)).map(|i| b.meta.paths[*i].nar_size).sum();
        if bytes > 16 << 20 && io::stderr().is_terminal() {
            eprintln!("{}: unpacking {:.0} MiB, the first time only…", b.meta.name, bytes as f64 / 1048576.0);
        }
        // A thin bundle's parts: fetched eight at a time, each into its own
        // temporary folder, renamed into place below once checked.
        let fetch: Vec<usize> = todo.iter().copied().filter(|i| b.meta.paths[*i].fetch && !from_host.contains(&b.meta.paths[*i].name)).collect();
        let fetched: Vec<std::sync::Mutex<Option<Result<()>>>> = fetch.iter().map(|_| std::sync::Mutex::new(None)).collect();
        let tmp_of = |p: &PathEntry| store.dir.join(format!(".tmp-{}-{}", p.name, std::process::id()));
        {
            let next = std::sync::atomic::AtomicUsize::new(0);
            let (meta, store, tmp_of) = (&b.meta, &store, &tmp_of);
            std::thread::scope(|s| {
                for _ in 0..8.min(fetch.len()) {
                    s.spawn(|| loop {
                        let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(i) = fetch.get(k) else { break };
                        let p = &meta.paths[*i];
                        let tmp = tmp_of(p);
                        let _ = remove_tree(&tmp);
                        let r = crate::fetch::fetch(meta, p, &tmp, STORE.as_bytes(), store.prefix.as_bytes());
                        note!("fetched {}", p.name);
                        *fetched[k].lock().unwrap() = Some(r);
                    });
                }
            });
        }
        let mut fetched: HashMap<usize, Result<()>> = fetch.iter().copied().zip(fetched.into_iter().map(|m| m.into_inner().unwrap().unwrap_or_else(|| Err(anyhow::anyhow!("not fetched"))))).collect();
        for i in todo {
            let p = b.meta.paths[i].clone();
            let dest = store.dir.join(&p.name);
            if from_host.contains(&p.name) {
                std::os::unix::fs::symlink(format!("{STORE}/{}", p.name), &dest)?;
                fs::write(store.nar_mark(&p.name), &p.nar_hash)?;
                note!("linked {} to the host's", p.name);
                continue;
            }
            let tmp = tmp_of(&p);
            let result = if let Some(r) = fetched.remove(&i) {
                r
            } else {
                if tmp.symlink_metadata().is_ok() {
                    remove_tree(&tmp)?;
                }
                match &mut b.source {
                    Source::File(file, _) => unpack_frame(file, &p, &tmp, &store.prefix),
                    Source::AppDir(dir) => copy_rewriting(&appdir_bundle(dir).join("store").join(&p.name), &tmp, STORE.as_bytes(), store.prefix.as_bytes()).with_context(|| format!("copying {} out of the AppImage", p.name)),
                }
            };
            if let Err(e) = result {
                let _ = remove_tree(&tmp);
                return Err(e);
            }
            fs::rename(&tmp, &dest)?;
            fs::write(store.nar_mark(&p.name), &p.nar_hash)?;
            note!("unpacked {}", p.name);
        }
        fs::create_dir_all(self.root.join("bundles"))?;
        fs::write(&entry, record)?;
        self.gc()?;
        Ok(store)
    }

    /// Bundles whose file is gone (or is now another bundle) are
    /// forgotten, then parts none of the rest need are removed: (bundles
    /// forgotten, parts removed).
    fn gc(&self) -> Result<(usize, usize)> {
        let (mut live, mut dead) = (BTreeSet::new(), 0);
        for e in fs::read_dir(self.root.join("bundles")).into_iter().flatten().flatten() {
            let text = fs::read_to_string(e.path()).unwrap_or_default();
            let mut lines = text.lines();
            let (path, _name, kind) = (lines.next().unwrap_or(""), lines.next(), lines.next().unwrap_or(""));
            let hash = e.file_name().to_string_lossy().into_owned();
            let alive = match kind {
                "file" => trailer_hash(Path::new(path)).as_deref() == Some(hash.as_str()),
                _ => Path::new(path).exists(),
            };
            if alive {
                live.extend(lines.map(str::to_string));
            } else {
                note!("forgetting {path} (gone or replaced)");
                let _ = fs::remove_file(e.path());
                dead += 1;
            }
        }
        let store = self.root.join("store");
        let mut removed = 0;
        for e in fs::read_dir(&store).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // Half-made parts (only whoever holds the lock makes them): an earlier failure's.
            if name.starts_with(".tmp-") {
                let _ = remove_tree(&e.path());
                continue;
            }
            if name.starts_with('.') || live.contains(&name) {
                continue;
            }
            note!("removing {name}: no bundle needs it");
            let _ = remove_tree(&e.path());
            let _ = fs::remove_file(store.join(".nar").join(&name));
            removed += 1;
        }
        Ok((dead, removed))
    }
}

/// The hash in a bundle file's trailer, if it is one.
fn trailer_hash(path: &Path) -> Option<String> {
    let mut f = File::open(path).ok()?;
    let len = f.seek(SeekFrom::End(0)).ok()?;
    f.seek(SeekFrom::Start(len.checked_sub(bundle_format::TRAILER_LEN)?)).ok()?;
    let mut t = [0u8; bundle_format::TRAILER_LEN as usize];
    f.read_exact(&mut t).ok()?;
    Trailer::decode(&t).map(|t| t.hash())
}

/// A part from its frame, checked against the frame's sha256.
fn unpack_frame(file: &mut File, p: &PathEntry, tmp: &Path, prefix: &str) -> Result<()> {
    file.seek(SeekFrom::Start(p.offset))?;
    let mut hashing = Hashing { inner: (&*file).take(p.size), hash: Sha256::new() };
    let result = (|| -> Result<()> {
        let mut z = zstd::stream::read::Decoder::new(&mut hashing)?;
        bundle_format::unpack(&mut z, tmp, STORE.as_bytes(), prefix.as_bytes())?;
        io::copy(&mut z, &mut io::sink())?;
        drop(z);
        io::copy(&mut hashing, &mut io::sink())?;
        Ok(())
    })();
    if result.is_ok() && bundle_format::hex(&hashing.hash.finalize()) == p.sha256 {
        return Ok(());
    }
    Err(result.err().unwrap_or_else(|| anyhow::anyhow!("doesn't match the bundle's hash"))).with_context(|| format!("unpacking {} (the bundle is damaged?)", p.name))
}

/// On a computer with its own /nix/store (NixOS, namiOS), the parts it
/// has, byte for byte (the same NAR hash), with everything they refer to:
/// those are linked to rather than unpacked. Their files name /nix/store,
/// where all they need is too. Not glibc, whose ld.so.cache the bundle
/// writes; and only on NixOS or namiOS (where its programs find the driver
/// in /run/opengl-driver themselves), unless NAMI_BUNDLE_HOST_STORE=1.
fn host_parts(meta: &Meta, todo: &[usize]) -> BTreeSet<String> {
    let wanted = match std::env::var("NAMI_BUNDLE_HOST_STORE").as_deref() {
        Ok("0") => false,
        Ok("1") => true,
        _ => Path::new("/etc/NIXOS").exists() || Path::new("/run/current-system").exists() && Path::new("/run/opengl-driver").exists(),
    };
    let mut out = BTreeSet::new();
    if !wanted || todo.is_empty() || !Path::new(STORE).is_dir() {
        return out;
    }
    let by_name: HashMap<&str, &PathEntry> = meta.paths.iter().map(|p| (p.name.as_str(), p)).collect();
    let glibc = meta.glibc.rsplit('/').next().unwrap_or("");
    let mut same: HashMap<String, bool> = HashMap::new();
    // The host has it, the same, and everything it refers to too.
    fn usable(name: &str, by_name: &HashMap<&str, &PathEntry>, same: &mut HashMap<String, bool>, depth: usize) -> bool {
        if let Some(s) = same.get(name) {
            return *s;
        }
        same.insert(name.to_string(), false); // a cycle: no
        let Some(p) = by_name.get(name) else { return false };
        let host = Path::new(STORE).join(name);
        let ok = !p.nar_hash.is_empty()
            && depth < 200
            && host.symlink_metadata().is_ok()
            && bundle_format::nar_hash(&host).is_ok_and(|(h, _)| h == p.nar_hash)
            && p.references.iter().all(|r| r == name || usable(r, by_name, same, depth + 1));
        same.insert(name.to_string(), ok);
        ok
    }
    for i in todo {
        let p = &meta.paths[*i];
        if p.name != glibc && usable(&p.name, &by_name, &mut same, 0) {
            out.insert(p.name.clone());
        }
    }
    if !out.is_empty() {
        note!("{} parts are in this computer's /nix/store already", out.len());
    }
    out
}

struct Hashing<R> {
    inner: R,
    hash: Sha256,
}

impl<R: Read> Read for Hashing<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hash.update(&buf[..n]);
        Ok(n)
    }
}

/// A store path from an AppImage's folder, as `bundle_format::unpack`
/// writes one from a frame: `from` replaced by `to`, mtimes 1, files read-only.
fn copy_rewriting(src: &Path, dest: &Path, from: &[u8], to: &[u8]) -> io::Result<()> {
    let m = fs::symlink_metadata(src)?;
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
    if m.file_type().is_symlink() {
        let mut target = fs::read_link(src)?.into_os_string().into_vec();
        bundle_format::replace_all(&mut target, from, to);
        std::os::unix::fs::symlink(OsStr::from_bytes(&target), dest)
    } else if m.is_dir() {
        fs::create_dir(dest)?;
        for e in fs::read_dir(src)? {
            let e = e?;
            copy_rewriting(&e.path(), &dest.join(e.file_name()), from, to)?;
        }
        File::open(dest)?.set_times(fs::FileTimes::new().set_modified(t).set_accessed(t))
    } else {
        let mut data = fs::read(src)?;
        bundle_format::replace_all(&mut data, from, to);
        let mut f = File::create(dest)?;
        f.write_all(&data)?;
        f.set_permissions(fs::Permissions::from_mode(if m.permissions().mode() & 0o111 != 0 { 0o555 } else { 0o444 }))?;
        f.set_times(fs::FileTimes::new().set_modified(t).set_accessed(t))
    }
}

fn writable(dir: &Path) -> bool {
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).unwrap();
    // SAFETY: a valid C string.
    unsafe { libc::access(c.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
}

fn remove_tree(p: &Path) -> io::Result<()> {
    // Files are read-only like the store's; folders aren't, so this works.
    if p.is_dir() && !p.is_symlink() {
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
}

// ---------------------------------------------------------------------------
// Graphics: the host's drivers, which must match its kernel.

struct Gl {
    /// Where libraries are, for the bundle's loader's own ld.so.cache: the
    /// host's drivers and what they need (the host's, or the bundle's own
    /// where it has what they need). Only the bundle's glibc reads that
    /// cache, so the programs the app starts from the host never see it.
    libraries: BTreeMap<String, PathBuf>,
    /// The host's drivers are used (and so their own variables are right).
    host: bool,
    /// Host libraries newer than the bundle's copies, which replace them:
    /// only LD_LIBRARY_PATH comes before a library's own RUNPATH. A folder
    /// of links to host files only, so harmless to the host's programs.
    lib_dir: Option<PathBuf>,
    env: Vec<(String, String)>,
    notes: Vec<String>,
}

/// A file mapped read-only: drivers are big, only their headers are read.
struct Mapped {
    ptr: *mut libc::c_void,
    len: usize,
}

impl Mapped {
    fn open(path: &Path) -> Option<Mapped> {
        use std::os::fd::AsRawFd;
        let f = File::open(path).ok()?;
        let len = f.metadata().ok()?.len() as usize;
        if len < 64 {
            return None;
        }
        // SAFETY: a private read-only mapping of a file we opened; unmapped in drop.
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_PRIVATE, f.as_raw_fd(), 0) };
        (ptr != libc::MAP_FAILED).then_some(Mapped { ptr, len })
    }
    fn bytes(&self) -> &[u8] {
        // SAFETY: the mapping is len bytes and lives as long as self.
        unsafe { std::slice::from_raw_parts(self.ptr as *const u8, self.len) }
    }
}

impl Drop for Mapped {
    fn drop(&mut self) {
        // SAFETY: what mmap gave.
        unsafe { libc::munmap(self.ptr, self.len) };
    }
}

fn linking(path: &Path) -> Option<elf::Linking> {
    elf::linking(Mapped::open(path)?.bytes())
}

/// The host's libraries for this computer's kind (x86-64's, or aarch64's) by soname,
/// from /etc/ld.so.cache (glibc's "1.1" format, with or without the old one in front).
fn ld_so_cache(data: &[u8]) -> Vec<(String, PathBuf)> {
    // (glibc's elf/cache.h: an entry's architecture, in its flags' second byte.)
    const CACHE_ARCH_FLAG: u32 = if cfg!(target_arch = "aarch64") { 0x0a00 } else { 0x0300 };
    const MAGIC: &[u8] = b"glibc-ld.so.cache1.1";
    let Some(base) = data.windows(MAGIC.len()).position(|w| w == MAGIC) else { return Vec::new() };
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let Some(n) = u32_at(base + 20) else { return Vec::new() };
    let string = |off: u32| {
        let s = data.get(base + off as usize..)?;
        let end = s.iter().position(|b| *b == 0)?;
        Some(OsStr::from_bytes(&s[..end]).to_owned())
    };
    let mut out = Vec::new();
    for i in 0..n.min(1 << 20) as usize {
        let e = base + 48 + i * 24;
        let (Some(flags), Some(key), Some(value)) = (u32_at(e), u32_at(e + 4), u32_at(e + 8)) else { break };
        // FLAG_ELF_LIBC6 | FLAG_X8664_LIB64 (FLAG_AARCH64_LIB64 on ARM)
        if flags & 0xff != 3 || flags & 0xff00 != CACHE_ARCH_FLAG {
            continue;
        }
        if let (Some(k), Some(v)) = (string(key), string(value)) {
            out.push((k.to_string_lossy().into_owned(), PathBuf::from(v)));
        }
    }
    out
}

/// Where the host keeps its libraries: what ld.so.cache lists (by soname)
/// and the usual folders, NixOS's driver folder first.
struct Host {
    by_name: HashMap<String, PathBuf>,
    dirs: Vec<PathBuf>,
}

impl Host {
    fn find() -> Host {
        let mut by_name = HashMap::new();
        let mut dirs: Vec<PathBuf> = Vec::new();
        let add_dir = |d: PathBuf, dirs: &mut Vec<PathBuf>| {
            if d.is_dir() && !dirs.contains(&d) {
                dirs.push(d);
            }
        };
        add_dir(PathBuf::from("/run/opengl-driver/lib"), &mut dirs);
        if let Ok(data) = fs::read("/etc/ld.so.cache") {
            for (name, path) in ld_so_cache(&data) {
                if let Some(d) = path.parent() {
                    add_dir(d.to_path_buf(), &mut dirs);
                }
                by_name.entry(name).or_insert(path);
            }
        }
        // (Debian's folders are named for the computer's kind: x86_64-linux-gnu, aarch64-linux-gnu.)
        let multiarch = if cfg!(target_arch = "aarch64") { "aarch64-linux-gnu" } else { "x86_64-linux-gnu" };
        for d in [format!("/usr/lib/{multiarch}"), format!("/lib/{multiarch}"), "/usr/lib64".into(), "/lib64".into(), "/usr/lib".into(), "/lib".into(), "/usr/local/lib".into()] {
            add_dir(PathBuf::from(d), &mut dirs);
        }
        Host { by_name, dirs }
    }

    fn lookup(&self, soname: &str, extra: &[PathBuf]) -> Option<PathBuf> {
        extra
            .iter()
            .chain(self.dirs.iter().take_while(|d| d.as_path() == Path::new("/run/opengl-driver/lib")))
            .map(|d| d.join(soname))
            .find(|p| p.exists())
            .or_else(|| self.by_name.get(soname).cloned())
            .or_else(|| self.dirs.iter().map(|d| d.join(soname)).find(|p| p.exists()))
    }

    /// Files in the host's library folders whose names `keep` likes.
    fn matching(&self, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for name in self.by_name.keys().filter(|n| keep(n)) {
            if seen.insert(name.clone()) {
                out.push(self.by_name[name].clone());
            }
        }
        for d in &self.dirs {
            for e in fs::read_dir(d).into_iter().flatten().flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if keep(&n) && seen.insert(n) {
                    out.push(e.path());
                }
            }
        }
        out
    }
}

/// glibc's own libraries: always the bundle's (one glibc per process).
const GLIBC: &[&str] = &["libc.so.6", "libm.so.6", "libdl.so.2", "libpthread.so.0", "librt.so.1", "ld-linux-x86-64.so.2", "ld-linux-aarch64.so.1", "libresolv.so.2", "libutil.so.1", "libanl.so.1", "libmvec.so.1", "libBrokenLocale.so.1", "libnsl.so.1", "libc_malloc_debug.so.0"];

/// Libraries that belong with the driver: always the host's.
fn driver_part(soname: &str) -> bool {
    ["libglapi.so", "libgallium", "libnvidia-", "libGLX_", "libEGL_", "libvulkan_", "libcuda.so", "libnvcuvid", "libdrm_nouveau", "libLLVM"].iter().any(|p| soname.starts_with(p))
}

/// Works out which host libraries a set of driver files can load with.
struct Resolver<'a> {
    host: &'a Host,
    bundle_libs: &'a BTreeMap<String, String>,
    store: &'a Store,
    /// Bundle library versions (verdef), parsed once.
    bundle_versions: HashMap<String, Vec<String>>,
    /// Decided: a host file's usability.
    usable: HashMap<PathBuf, bool>,
    /// For the loader's cache: soname -> file (the host's, or the bundle's).
    links: BTreeMap<String, PathBuf>,
    /// Host libraries that replace the bundle's.
    overrides: BTreeSet<String>,
    why: Vec<String>,
}

impl<'a> Resolver<'a> {
    fn versions_in_bundle(&mut self, soname: &str) -> Option<Vec<String>> {
        if let Some(v) = self.bundle_versions.get(soname) {
            return Some(v.clone());
        }
        let logical = self.bundle_libs.get(soname)?;
        let v = linking(&self.store.real(logical)).map(|l| l.versions).unwrap_or_default();
        self.bundle_versions.insert(soname.to_string(), v.clone());
        Some(v)
    }

    /// Can `file` (a host library) be loaded into the bundle's programs?
    /// Its libraries are linked in as needed (only on success).
    fn check(&mut self, file: &Path) -> bool {
        let real = fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
        if let Some(u) = self.usable.get(&real) {
            return *u;
        }
        self.usable.insert(real.clone(), true); // cycles: assume yes while deciding
        let saved = (self.links.clone(), self.overrides.clone());
        let ok = self.check_inner(&real);
        if !ok {
            (self.links, self.overrides) = saved;
        }
        self.usable.insert(real, ok);
        ok
    }

    fn check_inner(&mut self, file: &Path) -> bool {
        let Some(l) = linking(file) else {
            self.why.push(format!("{}: not a 64-bit library", file.display()));
            return false;
        };
        let origin = file.parent().unwrap_or(Path::new("/")).to_path_buf();
        let own: Vec<PathBuf> = l.rpath.iter().chain(l.runpath.iter()).flat_map(|r| r.split(':').map(|d| PathBuf::from(d.replace("$ORIGIN", &origin.display().to_string())))).collect();
        for soname in &l.needed {
            let wants = l.versions_needed.get(soname).cloned().unwrap_or_default();
            let glibc = GLIBC.contains(&soname.as_str());
            if !driver_part(soname) {
                if let Some(has) = self.versions_in_bundle(soname) {
                    let missing: Vec<&String> = wants.iter().filter(|w| !has.contains(w)).collect();
                    if missing.is_empty() {
                        // The bundle's: found through the cache (only the
                        // bundle's own libraries have rpaths to it).
                        if !glibc && !self.overrides.contains(soname) {
                            let local = self.store.local(&self.bundle_libs[soname]);
                            self.links.entry(soname.clone()).or_insert(local);
                        }
                        continue;
                    }
                    if glibc {
                        self.why.push(format!("{}: needs {} {}, newer than the bundle's", file.display(), soname, missing[0]));
                        return false;
                    }
                    // The host's is newer: it replaces the bundle's, if it has all the bundle's too.
                    let Some(theirs) = self.host.lookup(soname, &own) else { return false };
                    let theirs_has = linking(&theirs).map(|l| l.versions).unwrap_or_default();
                    if !has.iter().all(|v| theirs_has.contains(v)) || !wants.iter().all(|w| theirs_has.contains(w)) {
                        self.why.push(format!("{}: needs a {soname} neither the bundle's nor the host's can stand in for", file.display()));
                        return false;
                    }
                    if !self.check(&theirs) {
                        return false;
                    }
                    self.links.insert(soname.clone(), theirs);
                    self.overrides.insert(soname.clone());
                    continue;
                } else if glibc {
                    continue; // e.g. libnsl, which a bundle without it won't load anyway
                }
            }
            let Some(theirs) = self.host.lookup(soname, &own) else {
                self.why.push(format!("{}: {soname} isn't on this computer", file.display()));
                return false;
            };
            if !self.check(&theirs) {
                return false;
            }
            self.links.insert(soname.clone(), theirs);
        }
        true
    }
}

/// `"library_path": "…"` from a Vulkan, EGL or EGL-platform JSON file.
fn json_library(path: &Path) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    v.get("ICD")?.get("library_path")?.as_str().map(str::to_string)
}

fn jsons(dirs: &[&str]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for d in dirs {
        let mut here: Vec<PathBuf> = fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
        here.sort();
        out.extend(here);
    }
    out
}

/// Which graphics drivers the program uses, and how it finds them.
fn graphics(b: &Bundle, store: &Store, cache: &Cache) -> Gl {
    let mode = std::env::var("NAMI_BUNDLE_GL").unwrap_or_default();
    let libs = &b.meta.libs;
    let mut gl = Gl { libraries: BTreeMap::new(), host: false, lib_dir: None, env: Vec::new(), notes: Vec::new() };
    let uses_gl = libs.keys().any(|l| l.starts_with("libGLX.so") || l.starts_with("libEGL.so") || l.starts_with("libvulkan.so") || l.starts_with("libGL.so") || l.starts_with("libgbm.so") || l.starts_with("libva.so") || l.starts_with("libcuda"));
    if mode == "none" {
        gl.notes.push("graphics drivers: none (NAMI_BUNDLE_GL=none)".into());
    } else if !uses_gl && mode.is_empty() {
        gl.notes.push("graphics drivers: not needed (no OpenGL, EGL or Vulkan in the bundle)".into());
    } else {
        if mode != "bundled" {
            // The cache is the glibc's, which bundles share: what's in it
            // can't depend on the bundle. So: the host's drivers and the
            // host's own copies of what they need (found after a library's
            // own rpath, so the bundle's libraries keep theirs). What this
            // bundle makes of them (newer host copies it must use instead,
            // which drivers it can use) is worked out on its own.
            let glibc_only: BTreeMap<String, String> = libs.iter().filter(|(k, _)| GLIBC.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
            let mut shared = Gl { libraries: BTreeMap::new(), host: false, lib_dir: None, env: Vec::new(), notes: Vec::new() };
            host_graphics(b, store, cache, &mut shared, &glibc_only);
            host_graphics(b, store, cache, &mut gl, libs);
            gl.libraries = shared.libraries;
            write_ld_so_cache(b, store, &mut gl);
        }
        if !gl.host && mode != "host" {
            bundled_graphics(b, store, &mut gl);
        }
    }
    gl
}

/// The bundle's glibc's own ld.so.cache: the host's driver libraries.
fn write_ld_so_cache(b: &Bundle, store: &Store, gl: &mut Gl) {
    if !b.meta.glibc.is_empty() {
        let file = store.real(&format!("{}/etc/ld.so.cache", b.meta.glibc));
        let have = fs::read(&file).ok();
        let want = (!gl.libraries.is_empty()).then(|| ld_so_cache_file(&gl.libraries));
        if have != want {
            let written = match &want {
                Some(data) => fs::create_dir_all(file.parent().unwrap()).and_then(|_| {
                    let tmp = file.with_extension(format!("tmp{}", std::process::id()));
                    fs::write(&tmp, data)?;
                    fs::rename(&tmp, &file)
                }),
                None => fs::remove_file(&file),
            };
            if let Err(e) = written {
                gl.notes.push(format!("couldn't write {}: {e}", file.display()));
            }
        }
        if want.is_some() {
            gl.notes.push(format!("{} libraries in the bundle's {}", gl.libraries.len(), store.local(&format!("{}/etc/ld.so.cache", b.meta.glibc)).display()));
        }
    }
}

/// _dl_cache_libcmp: glibc's order of sonames (numbers by value).
fn libcmp(a: &[u8], b: &[u8]) -> i64 {
    let (mut i, mut j) = (0, 0);
    let digit = |s: &[u8], k: usize| s.get(k).is_some_and(|c| c.is_ascii_digit());
    while i < a.len() {
        if digit(a, i) {
            if !digit(b, j) {
                return 1;
            }
            let (mut x, mut y) = (0i64, 0i64);
            while digit(a, i) {
                x = x.saturating_mul(10) + (a[i] - b'0') as i64;
                i += 1;
            }
            while digit(b, j) {
                y = y.saturating_mul(10) + (b[j] - b'0') as i64;
                j += 1;
            }
            if x != y {
                return x - y;
            }
        } else if digit(b, j) {
            return -1;
        } else if Some(&a[i]) != b.get(j) {
            return a[i] as i64 - *b.get(j).unwrap_or(&0) as i64;
        } else {
            i += 1;
            j += 1;
        }
    }
    -(*b.get(j).unwrap_or(&0) as i64)
}

/// An ld.so.cache (glibc's "1.1" format) listing these libraries: sorted
/// the way ld.so's binary search expects (descending, by libcmp).
fn ld_so_cache_file(libs: &BTreeMap<String, PathBuf>) -> Vec<u8> {
    let mut list: Vec<(&String, &PathBuf)> = libs.iter().collect();
    list.sort_by(|a, b| libcmp(b.0.as_bytes(), a.0.as_bytes()).cmp(&0));
    let n = list.len();
    let strings_at = 48 + 24 * n;
    let mut strings: Vec<u8> = Vec::new();
    let mut entries: Vec<u8> = Vec::new();
    for (name, path) in list {
        let key = strings_at + strings.len();
        strings.extend(name.as_bytes());
        strings.push(0);
        let value = strings_at + strings.len();
        strings.extend(path.as_os_str().as_bytes());
        strings.push(0);
        entries.extend(0x0303u32.to_le_bytes()); // FLAG_ELF_LIBC6 | FLAG_X8664_LIB64
        entries.extend((key as u32).to_le_bytes());
        entries.extend((value as u32).to_le_bytes());
        entries.extend([0u8; 12]); // osversion, hwcap
    }
    let mut out = b"glibc-ld.so.cache1.1".to_vec();
    out.extend((n as u32).to_le_bytes());
    out.extend((strings.len() as u32).to_le_bytes());
    out.push(2); // little-endian
    out.resize(48, 0);
    out.extend(entries);
    out.extend(strings);
    out
}

fn host_graphics(b: &Bundle, store: &Store, cache: &Cache, gl: &mut Gl, bundle_libs: &BTreeMap<String, String>) {
    let host = Host::find();
    let mut r = Resolver { host: &host, bundle_libs, store, bundle_versions: HashMap::new(), usable: HashMap::new(), links: BTreeMap::new(), overrides: BTreeSet::new(), why: Vec::new() };
    let mut seeds_ok = 0;
    // GLX and EGL vendors, by name (glvnd loads libGLX_<vendor>.so.0).
    let vendors = host.matching(|n| (n.starts_with("libGLX_") || n.starts_with("libEGL_")) && n.ends_with(".so.0"));
    // Mesa's drivers, which it opens by their path: they must load too.
    let mut mesa_parts: Vec<PathBuf> = Vec::new();
    for d in &host.dirs {
        mesa_parts.extend(fs::read_dir(d.join("dri")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "so") && !p.to_string_lossy().ends_with("_drv_video.so")));
    }
    let mesa_ok = mesa_parts.is_empty() || {
        let mut any = false;
        let mut seen = BTreeSet::new();
        for p in &mesa_parts {
            let ino = fs::metadata(p).map(|m| m.ino()).unwrap_or(0);
            if seen.insert(ino) && r.check(p) {
                any = true;
            }
        }
        any
    };
    for v in &vendors {
        let name = v.file_name().unwrap().to_string_lossy().into_owned();
        if name.contains("mesa") && !mesa_ok {
            gl.notes.push(format!("host {name}: its drivers can't load here"));
            continue;
        }
        if r.check(v) {
            r.links.insert(name.clone(), v.clone());
            gl.notes.push(format!("host {name}: {}", v.display()));
            seeds_ok += 1;
        } else {
            gl.notes.push(format!("host {name}: can't load here"));
        }
    }
    // NVIDIA's and CUDA's own libraries, which programs load by name.
    for v in host.matching(|n| ["libcuda.so.1", "libnvidia-ml.so.1", "libnvcuvid.so.1", "libnvidia-encode.so.1"].contains(&n)) {
        let name = v.file_name().unwrap().to_string_lossy().into_owned();
        if r.check(&v) {
            r.links.insert(name, v);
        }
    }
    // Vulkan drivers, EGL vendors and EGL platforms, by their JSON files.
    let ok_jsons = |dirs: &[&str], r: &mut Resolver, notes: &mut Vec<String>| -> Vec<String> {
        let mut out = Vec::new();
        for j in jsons(dirs) {
            let Some(lib) = json_library(&j) else { continue };
            let file = if lib.contains('/') { Some(PathBuf::from(&lib)) } else { host.lookup(&lib, &[]) };
            let Some(file) = file else { continue };
            if lib.contains("mesa") && !mesa_ok {
                continue;
            }
            if r.check(&file) {
                if !lib.contains('/') {
                    r.links.insert(lib.clone(), file.clone());
                }
                notes.push(format!("host {}: {}", j.display(), file.display()));
                out.push(j.display().to_string());
            }
        }
        out
    };
    let vulkan = ok_jsons(&["/run/opengl-driver/share/vulkan/icd.d", "/etc/vulkan/icd.d", "/usr/local/share/vulkan/icd.d", "/usr/share/vulkan/icd.d"], &mut r, &mut gl.notes);
    let egl = ok_jsons(&["/run/opengl-driver/share/glvnd/egl_vendor.d", "/etc/glvnd/egl_vendor.d", "/usr/local/share/glvnd/egl_vendor.d", "/usr/share/glvnd/egl_vendor.d"], &mut r, &mut gl.notes);
    let platforms = ok_jsons(&["/run/opengl-driver/share/egl/egl_external_platform.d", "/etc/egl/egl_external_platform.d", "/usr/local/share/egl/egl_external_platform.d", "/usr/share/egl/egl_external_platform.d"], &mut r, &mut gl.notes);
    seeds_ok += vulkan.len();
    // Video decoding and GBM: their driver folders, where every driver loads.
    let dir_of = |sub: &str, filter: &dyn Fn(&str) -> bool, r: &mut Resolver| -> Option<String> {
        for d in &host.dirs {
            let dir = d.join(sub);
            let files: Vec<PathBuf> = fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| filter(&p.file_name().unwrap().to_string_lossy())).collect();
            // Every one checked (each usable one's libraries listed).
            if files.iter().filter(|f| r.check(f)).count() > 0 {
                return Some(dir.display().to_string());
            }
        }
        None
    };
    let va = if b.meta.libs.contains_key("libva.so.2") { dir_of("dri", &|n| n.ends_with("_drv_video.so"), &mut r) } else { None };
    let gbm = if b.meta.libs.contains_key("libgbm.so.1") { dir_of("gbm", &|n| n.ends_with(".so"), &mut r) } else { None };
    if debug() {
        for w in &r.why {
            note!("{w}");
        }
    }
    if seeds_ok == 0 {
        gl.notes.push("host graphics drivers: none this bundle can use".into());
        return;
    }
    gl.host = true;
    gl.libraries = r.links.clone();
    // Host libraries replacing the bundle's: a folder of links, named after what's in it.
    if !r.overrides.is_empty() {
        let mut h = Sha256::new();
        for k in &r.overrides {
            h.update(format!("{k}={}\n", r.links[k].display()));
        }
        let dir = cache.root.join("host-libs").join(&bundle_format::hex(&h.finalize())[..16]);
        if !dir.exists() {
            let tmp = dir.with_extension(format!("tmp{}", std::process::id()));
            let _ = remove_tree(&tmp);
            let made = fs::create_dir_all(&tmp).is_ok() && r.overrides.iter().all(|k| std::os::unix::fs::symlink(&r.links[k], tmp.join(k)).is_ok());
            if !made || fs::rename(&tmp, &dir).is_err() {
                let _ = remove_tree(&tmp);
            }
        }
        if dir.exists() {
            gl.notes.push(format!("host libraries newer than the bundle's, used instead: {}", r.overrides.iter().cloned().collect::<Vec<_>>().join(", ")));
            gl.lib_dir = Some(dir);
        }
    }
    let mut set = |k: &str, v: String| {
        if std::env::var_os(k).is_none() && !v.is_empty() {
            gl.env.push((k.to_string(), v));
        }
    };
    if std::env::var_os("__EGL_VENDOR_LIBRARY_DIRS").is_none() {
        set("__EGL_VENDOR_LIBRARY_FILENAMES", egl.join(":"));
    }
    set("__EGL_EXTERNAL_PLATFORM_CONFIG_FILENAMES", platforms.join(":"));
    if std::env::var_os("VK_ICD_FILENAMES").is_none() && std::env::var_os("VK_DRIVER_FILES").is_none() {
        set("VK_ADD_DRIVER_FILES", vulkan.join(":"));
    }
    if let Some(va) = va {
        set("LIBVA_DRIVERS_PATH", va);
    }
    if let Some(gbm) = gbm {
        set("GBM_BACKENDS_PATH", gbm);
    }
}

/// No host driver it can use: Mesa from nixpkgs, if the bundle has it
/// (`nami bundle --mesa`), drawing in software where there's no GPU it knows.
fn bundled_graphics(b: &Bundle, store: &Store, gl: &mut Gl) {
    let Some(egl) = b.meta.libs.get("libEGL_mesa.so.0").or_else(|| b.meta.libs.get("libGLX_mesa.so.0")) else {
        gl.notes.push("bundled Mesa: none (`nami bundle --mesa` adds it); OpenGL may not work here".into());
        return;
    };
    let lib = Path::new(egl).parent().unwrap().to_path_buf();
    let out = lib.parent().unwrap().to_path_buf();
    let s = |p: &Path| store.local(&p.display().to_string()).display().to_string();
    // Its own folder first (not the cache, which is every bundle's).
    gl.lib_dir = Some(store.local(&lib.display().to_string()));
    gl.notes.push(format!("bundled Mesa: {}", s(&out)));
    let real_out = store.real(&out.display().to_string());
    let files = |sub: &str| -> String {
        let mut v: Vec<String> = fs::read_dir(real_out.join(sub)).into_iter().flatten().flatten().map(|e| format!("{}/{sub}/{}", s(&out), e.file_name().to_string_lossy())).collect();
        v.sort();
        v.join(":")
    };
    gl.env.push(("__EGL_VENDOR_LIBRARY_FILENAMES".into(), files("share/glvnd/egl_vendor.d")));
    gl.env.push(("VK_DRIVER_FILES".into(), files("share/vulkan/icd.d")));
    gl.env.push(("LIBGL_DRIVERS_PATH".into(), format!("{}/lib/dri", s(&out))));
    gl.env.push(("GBM_BACKENDS_PATH".into(), format!("{}/lib/gbm", s(&out))));
    gl.env.push(("__GLX_VENDOR_LIBRARY_NAME".into(), "mesa".into()));
}

// ---------------------------------------------------------------------------
// The rest of what comes from the host: fonts, locale, certificates, themes.

/// Host variables that name plugins built for the host's libraries: loaded
/// into the bundle's programs they break them. Kept as NAMI_BUNDLE_HOST_<name>.
const HOST_ONLY: &[&str] = &[
    "LD_LIBRARY_PATH", "GIO_EXTRA_MODULES", "GIO_MODULE_DIR", "GDK_PIXBUF_MODULE_FILE", "GDK_PIXBUF_MODULEDIR", "GTK_PATH", "GTK_EXE_PREFIX", "GTK_DATA_PREFIX",
    "GTK_IM_MODULE_FILE", "QT_PLUGIN_PATH", "QML2_IMPORT_PATH", "QML_IMPORT_PATH", "QTWEBENGINEPROCESS_PATH", "GST_PLUGIN_SYSTEM_PATH", "GST_PLUGIN_SYSTEM_PATH_1_0",
    "GST_PLUGIN_PATH", "GST_PLUGIN_PATH_1_0", "GST_PLUGIN_SCANNER", "GI_TYPELIB_PATH", "PYTHONPATH", "PYTHONHOME", "PERL5LIB", "LIBGL_DRIVERS_PATH",
];

fn environment(b: &Bundle, store: &Store, cache: &Cache, gl: Gl) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    let unset = |k: &str| std::env::var_os(k).is_none_or(|v| v.is_empty());
    for k in HOST_ONLY {
        // The host's Mesa reads LIBGL_DRIVERS_PATH: its own value is right for it.
        if *k == "LIBGL_DRIVERS_PATH" && gl.host {
            continue;
        }
        if let Some(v) = std::env::var_os(k) {
            out.push((format!("NAMI_BUNDLE_HOST_{k}"), Some(v.to_string_lossy().into_owned())));
            out.push((format!("TRAY_BUNDLE_HOST_{k}"), Some(v.to_string_lossy().into_owned())));
            out.push((k.to_string(), None));
        }
    }
    if let Some(d) = &gl.lib_dir {
        out.push(("LD_LIBRARY_PATH".into(), Some(d.display().to_string())));
    }
    for (k, v) in gl.env {
        out.push((k, Some(v)));
    }
    // Where the desktop's data is (themes, icons, schemas, fonts): the
    // programs' wrappers add theirs in front; an empty one would hide the host's.
    if unset("XDG_DATA_DIRS") {
        out.push(("XDG_DATA_DIRS".into(), Some("/usr/local/share:/usr/share".into())));
    }
    if unset("XDG_CONFIG_DIRS") {
        out.push(("XDG_CONFIG_DIRS".into(), Some("/etc/xdg".into())));
    }
    // Certificates: nixpkgs' OpenSSL and curl look where NixOS keeps them.
    if unset("SSL_CERT_FILE") && unset("NIX_SSL_CERT_FILE") {
        if let Some(f) = ["/etc/ssl/certs/ca-certificates.crt", "/etc/pki/tls/certs/ca-bundle.crt", "/etc/ssl/ca-bundle.pem", "/etc/ssl/cert.pem", "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem"].iter().find(|f| Path::new(f).is_file()) {
            out.push(("SSL_CERT_FILE".into(), Some(f.to_string())));
            out.push(("NIX_SSL_CERT_FILE".into(), Some(f.to_string())));
        }
    }
    // Fonts: the host's configuration (its fonts, and the person's choices).
    if unset("FONTCONFIG_FILE") {
        if Path::new("/etc/fonts/fonts.conf").is_file() {
            out.push(("FONTCONFIG_FILE".into(), Some("/etc/fonts/fonts.conf".into())));
        } else if let Some(f) = own_fonts_conf(b, store, cache) {
            out.push(("FONTCONFIG_FILE".into(), Some(f.display().to_string())));
        }
    }
    // The host's locales, which glibc's data format has kept readable.
    if unset("LOCALE_ARCHIVE") && unset("LOCPATH") {
        if Path::new("/usr/lib/locale/locale-archive").is_file() {
            out.push(("LOCALE_ARCHIVE".into(), Some("/usr/lib/locale/locale-archive".into())));
        } else if Path::new("/usr/lib/locale").is_dir() {
            out.push(("LOCPATH".into(), Some("/usr/lib/locale".into())));
        }
    }
    if unset("TZDIR") && Path::new("/usr/share/zoneinfo").is_dir() {
        out.push(("TZDIR".into(), Some("/usr/share/zoneinfo".into())));
    }
    for (k, v) in &b.meta.env {
        out.push((k.clone(), Some(v.replace(STORE, &store.prefix))));
    }
    out.push(("NAMI_BUNDLE".into(), Some(b.path.display().to_string())));
    // (And as tray named it, for programs written then.)
    out.push(("TRAY_BUNDLE".into(), Some(b.path.display().to_string())));
    out
}

/// A fontconfig file for a computer without one (a container): the host's
/// usual font folders, and any fonts the bundle carries.
fn own_fonts_conf(b: &Bundle, store: &Store, cache: &Cache) -> Option<PathBuf> {
    let mut dirs = vec!["/usr/share/fonts".to_string(), "/usr/local/share/fonts".to_string(), "~/.local/share/fonts".to_string(), "~/.fonts".to_string()];
    for p in &b.meta.paths {
        let d = store.dir.join(&p.name).join("share/fonts");
        if d.is_dir() {
            dirs.push(format!("{}/{}/share/fonts", store.prefix, p.name));
        }
    }
    let mut text = String::from("<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"urn:fontconfig:fonts.dtd\">\n<fontconfig>\n");
    for d in dirs {
        text += &format!("  <dir>{d}</dir>\n");
    }
    text += &format!("  <cachedir>{}</cachedir>\n</fontconfig>\n", cache.root.join("fontconfig").display());
    let f = cache.root.join(format!("fonts-{}.conf", &bundle_format::sha256_hex(text.as_bytes())[..12]));
    if !f.exists() {
        fs::write(&f, text).ok()?;
    }
    Some(f)
}

// ---------------------------------------------------------------------------
// The app menu: as namiOS installs a bundle, as far as any Linux goes
// (src/bundle_menu.rs): its entry and icon in ~/.local/share, what it
// opens, the links it opens where nothing else does.

fn install(b: &Bundle, store: &Store, add: bool) -> Result<()> {
    use crate::bundle_menu as menu;
    use crate::bundle_trust::{self as trust, Trust};
    // A signed one: its key trusted from now on, or refused if it isn't the one trusted for it.
    if add {
        if let (Some(u), Trust::Changed { was }) = (&b.meta.update, crate::update::trust_in(&trust::dir(), &b.meta)) {
            let fp = |k: &str| crate::update::fingerprint(k).unwrap_or_default();
            // (Said as it is: it names the app itself.)
            eprintln!("{}", trust::refusal(&b.meta, &fp(&was), &fp(&u.key), "run the new copy once with --nami-bundle-trust, then install it again"));
            std::process::exit(1);
        }
        crate::update::settle_in(&trust::dir(), &b.meta)?;
        crate::bundle_ask::remember(&trust::app(&b.meta), crate::bundle_ask::Answer::Add);
        if b.meta.desktop.is_none() {
            println!("{} is a command, with no app menu entry: run it as {}", b.meta.name, b.path.display());
            return Ok(());
        }
    } else {
        // Taken out on purpose: the next copy's key is trusted anew.
        trust::forget(&b.meta);
        let _ = fs::remove_file(crate::bundle_ask::asked_file(&trust::app(&b.meta)));
        if b.meta.desktop.is_none() {
            println!("{} is a command, with no app menu entry: delete {} to remove it", b.meta.name, b.path.display());
            return Ok(());
        }
    }
    let d = b.meta.desktop.as_ref().context("this bundle has no app menu entry (it's a command)")?;
    let id = d.id.trim_end_matches(".desktop");
    let entry = menu::entry_file(id);
    let apps = entry.parent().unwrap().to_path_buf();
    let entry_name = format!("{}.desktop", menu::entry_id(id));
    let icon = d.icon.as_deref().and_then(|i| menu::icon_file(id, i));
    let name = menu::entry_key(&d.file, "Name").unwrap_or_else(|| b.meta.name.clone());
    // namiOS installed it (Settings, `nami install`): its entry is namiOS's.
    if let Some(old) = fs::read_to_string(&entry).ok().filter(|t| menu::entry_key(t, "X-Tray-App").is_some()) {
        let app = menu::entry_key(&old, "X-Tray-App").unwrap_or_default();
        let at = menu::exec_program(&old).unwrap_or_default();
        if add {
            if Path::new(&at) == b.path {
                println!("{name} is installed (Settings has it, in Your Apps)");
            } else {
                println!("{name} is installed already, with Settings ({at}): the app menu starts that one. Remove it in Settings (Your Apps) to put this one there instead.");
            }
            return Ok(());
        }
        bail!("{name} was installed with Settings: remove it there (Your Apps), or with `nami remove {app}`");
    }
    let mimeapps = menu::config_home().join("mimeapps.list");
    let links = menu::link_kinds(&d.file);
    if !add {
        let _ = fs::remove_file(&entry);
        if let Some(i) = &icon {
            let _ = fs::remove_file(i);
        }
        if let (false, Ok(text)) = (links.is_empty(), fs::read_to_string(&mimeapps)) {
            let _ = fs::write(&mimeapps, menu::defaults(&text, &links, &entry_name, false));
        }
        // One under the other name too (made before the rename, or after), if it's this bundle's.
        let picked = menu::entry_id(id);
        for other in menu::entry_ids(id).into_iter().filter(|o| *o != picked) {
            let file = apps.join(format!("{other}.desktop"));
            let Ok(text) = fs::read_to_string(&file) else { continue };
            if menu::entry_key(&text, "X-Tray-App").is_some() || menu::exec_program(&text).as_deref().map(Path::new) != Some(b.path.as_path()) {
                continue;
            }
            let _ = fs::remove_file(&file);
            if let Some(i) = &icon {
                let name = i.file_name().unwrap_or_default().to_string_lossy().replacen(&picked, &other, 1);
                let _ = fs::remove_file(i.with_file_name(name));
            }
            if let (false, Ok(text)) = (links.is_empty(), fs::read_to_string(&mimeapps)) {
                let _ = fs::write(&mimeapps, menu::defaults(&text, &links, &format!("{other}.desktop"), false));
            }
        }
        menu::refresh_menu(&apps);
        println!("removed {}", entry.display());
        return Ok(());
    }
    let icon_name = menu::entry_id(id);
    let text = menu::menu_entry(&d.file, &b.path.display().to_string(), icon.as_ref().map(|_| icon_name.as_str()), id, &[("X-Tray-Bundle", &b.meta.name)]);
    fs::create_dir_all(&apps)?;
    fs::write(&entry, text)?;
    if let (Some(i), Some(src)) = (&icon, &d.icon) {
        fs::create_dir_all(i.parent().unwrap())?;
        let _ = fs::remove_file(i);
        fs::copy(store.real(src), i)?;
        fs::set_permissions(i, fs::Permissions::from_mode(0o644))?;
    }
    // The links it opens (`vscode://`): its, where nothing else opens them.
    if !links.is_empty() {
        let text = fs::read_to_string(&mimeapps).unwrap_or_default();
        let new = menu::defaults(&text, &links, &entry_name, true);
        if new != text {
            fs::create_dir_all(mimeapps.parent().unwrap())?;
            fs::write(&mimeapps, new)?;
        }
    }
    menu::refresh_menu(&apps);
    let _ = io::stdout().write_all(format!("added {}\n", entry.display()).as_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ld_so_cache() {
        // A "1.1" cache with one entry for this computer (x86-64, or aarch64), one i386 one,
        // and one for the other 64-bit kind.
        let (here, other) = if cfg!(target_arch = "aarch64") { (0x0a03, 0x0303) } else { (0x0303, 0x0a03) };
        let mut data = b"glibc-ld.so.cache1.1".to_vec();
        data.extend(3u32.to_le_bytes()); // nlibs
        data.extend(0u32.to_le_bytes()); // len_strings
        data.resize(48, 0);
        let strings = 48 + 3 * 24;
        let push = |data: &mut Vec<u8>, flags: u32, key: u32, value: u32| {
            data.extend(flags.to_le_bytes());
            data.extend(key.to_le_bytes());
            data.extend(value.to_le_bytes());
            data.extend([0u8; 12]);
        };
        push(&mut data, here, strings, strings + 10);
        push(&mut data, 0x0003, strings, strings + 10);
        push(&mut data, other, strings, strings + 10);
        data.extend(b"libz.so.1\0/lib/libz.so.1\0");
        let mut with_old = b"ld.so-1.7.0\0".to_vec();
        with_old.extend(0u32.to_le_bytes());
        with_old.extend(&data);
        for d in [data, with_old] {
            assert_eq!(ld_so_cache(&d), vec![("libz.so.1".to_string(), PathBuf::from("/lib/libz.so.1"))]);
        }
    }

    #[test]
    fn local_paths() {
        let s = Store { prefix: "/tmp/abcde".into(), dir: PathBuf::from("/home/x/.cache/tray-bundle/abcde") };
        assert_eq!(s.local("/nix/store/aaa-jq/bin/jq"), PathBuf::from("/tmp/abcde/aaa-jq/bin/jq"));
        assert_eq!(s.real("/nix/store/aaa-jq/bin/jq"), PathBuf::from("/home/x/.cache/tray-bundle/abcde/aaa-jq/bin/jq"));
    }

    #[test]
    fn writes_an_ld_so_cache_ld_so_can_search() {
        let mut libs = BTreeMap::new();
        for n in ["libdrm.so.2", "libLLVM-12.so.1", "libGLX_mesa.so.0", "libz.so.1", "libLLVM.so.19.1", "libX11.so.6", "libdrm_amdgpu.so.1"] {
            libs.insert(n.to_string(), PathBuf::from(format!("/usr/lib/{n}")));
        }
        let data = ld_so_cache_file(&libs);
        let read = ld_so_cache(&data);
        assert_eq!(read.len(), libs.len());
        // Descending by libcmp, as ldconfig writes it.
        for w in read.windows(2) {
            assert!(libcmp(w[0].0.as_bytes(), w[1].0.as_bytes()) > 0, "{} before {}", w[0].0, w[1].0);
        }
        // ld.so's binary search finds each.
        for name in libs.keys() {
            let (mut left, mut right) = (0i64, read.len() as i64 - 1);
            let mut found = None;
            while left <= right {
                let mid = (left + right) / 2;
                let c = libcmp(name.as_bytes(), read[mid as usize].0.as_bytes());
                if c == 0 {
                    found = Some(&read[mid as usize].1);
                    break;
                }
                if c < 0 { left = mid + 1 } else { right = mid - 1 }
            }
            assert_eq!(found, Some(&libs[name]), "{name}");
        }
        assert!(libcmp(b"libfoo.so.10", b"libfoo.so.9") > 0);
        assert!(libcmp(b"libfoo.so.1", b"libfoo.so.1") == 0);
    }

    #[test]
    fn driver_parts() {
        assert!(driver_part("libglapi.so.0"));
        assert!(driver_part("libnvidia-glcore.so.550.54"));
        assert!(!driver_part("libstdc++.so.6"));
    }
}
