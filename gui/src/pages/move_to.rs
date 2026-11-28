//! Move to namiOS: on another distribution (Ubuntu, Fedora, Mint, Arch), what namiOS is and what
//! comes along; namiOS in a window (the latest release's image, checked as the namiOS
//! Installer checks it, with this person's files on its boot partition, started by
//! `nami try-os --disk`); and a namiOS USB drive made here, by the Installer's own writer
//! (`nami-gui namios-write`, with an admin's password), with their apps and settings on it
//! for namiOS's first start (the seed: gui/src/stick/seed.rs). Their files
//! come over afterwards: Backups, or Transfer Your Files on namiOS.
//!
//! Shown only off namiOS (`look::on_namios`): on namiOS, NAMI_GUI_AS=other-linux shows it as
//! it is there, where making a USB drive only says what it would do.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{Context, Div, Entity, FontWeight, ParentElement, StatefulInteractiveElement, Styled, div, img, rgb};

use crate::disks::Disk;
use crate::icons::glyph;
use crate::input::TextInput;
use crate::look::{Stop, px, text};
use crate::release::Release;
use crate::seed::Seed;
use crate::ui::{button, color, destructive_button};
use crate::{Page, Shell};

/// What a USB drive needs to hold namiOS (as the Installer says: it fills the rest itself).
const LEAST: u64 = 14_000_000_000;
/// The disk namiOS gets in a window: the image grown to this (sparse: only what it uses
/// takes room).
const TRIAL_DISK: u64 = 32_000_000_000;
/// The room trying it needs here: the image, and what namiOS fills in at first.
const TRIAL_ROOM: u64 = 20_000_000_000;

/// Something going, done, or not: the USB drive, or namiOS for a window.
#[derive(Default)]
pub enum Work {
    #[default]
    Idle,
    /// The writer's progress file, its last step, how far; the writer itself (the USB drive's,
    /// behind the password prompt).
    Going { progress: PathBuf, step: String, done: u64, total: u64, child: Option<std::process::Child> },
    Done,
    Failed(String),
}

impl Work {
    fn going(&self) -> bool {
        matches!(self, Work::Going { .. })
    }
}

pub struct MoveTo {
    /// The new computer's name.
    pub name: Entity<TextInput>,
    pub release: Option<Result<Release, String>>,
    pub disks: Vec<Disk>,
    pub picked: Option<String>,
    /// Make USB Drive was pressed: the drive it would erase, asked about first.
    pub confirming: Option<Disk>,
    pub usb: Work,
    /// A preview's USB drive: what it would have done.
    pub previewed: Option<String>,
    /// namiOS for a window: getting it ready, then running (`vm`).
    pub trial: Work,
    pub vm: Option<std::process::Child>,
    /// Whether namiOS can run in a window here, and if not, why.
    pub can_try: Option<Result<(), String>>,
    /// Which file the preview shows.
    pub tab: usize,
    /// The page's clock: drives and progress looked at while it shows.
    watching: bool,
}

impl MoveTo {
    pub fn new(cx: &mut Context<Shell>) -> Self {
        let name = cx.new(|cx| TextInput::new(cx, "namios", &default_name(), false));
        // (The file it makes shows the name as it's typed.)
        cx.observe(&name, |_, _, cx| cx.notify()).detach();
        MoveTo { name, release: None, disks: Vec::new(), picked: None, confirming: None, usb: Work::Idle, previewed: None, trial: Work::Idle, vm: None, can_try: None, tab: 0, watching: false }
    }
}

/// The page is shown: the latest namiOS and whether it runs in a window here, looked up once;
/// the USB drives and what's going, looked at while it shows.
pub fn look(g: &mut Shell, cx: &mut Context<Shell>) {
    if g.move_to.release.as_ref().is_none_or(|r| r.is_err()) {
        g.move_to.release = None;
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async { crate::release::latest() }).await;
            let _ = this.update(cx, |g, cx| {
                g.move_to.release = Some(r);
                cx.notify();
            });
        })
        .detach();
    }
    cx.spawn(async move |this, cx| {
        let r = cx.background_executor().spawn(async { can_try() }).await;
        let _ = this.update(cx, |g, cx| {
            g.move_to.can_try = Some(r);
            cx.notify();
        });
    })
    .detach();
    if std::mem::replace(&mut g.move_to.watching, true) {
        return;
    }
    cx.spawn(async move |this, cx| {
        let mut tick = 0u32;
        loop {
            let shown = this.update(cx, |g, cx| {
                let m = &mut g.move_to;
                let busy = m.usb.going() || m.trial.going() || m.vm.is_some();
                if g.page != Page::MoveToNamios && !busy {
                    m.watching = false;
                    return false;
                }
                follow(g, cx);
                true
            });
            if !matches!(shown, Ok(true)) {
                break;
            }
            if tick % 7 == 0 {
                let d = cx.background_executor().spawn(async { crate::disks::list() }).await;
                let _ = this.update(cx, |g, cx| {
                    let m = &mut g.move_to;
                    if m.disks != d && !m.usb.going() {
                        if m.picked.as_ref().is_some_and(|p| !d.iter().any(|x| &x.path == p)) {
                            m.picked = None;
                        }
                        if m.picked.is_none() {
                            m.picked = d.iter().find(|x| x.bytes >= LEAST).map(|x| x.path.clone());
                        }
                        m.disks = d;
                        if m.confirming.as_ref().is_some_and(|c| !(m.picked.as_deref() == Some(c.path.as_str()) && m.disks.contains(c))) {
                            m.confirming = None;
                        }
                        cx.notify();
                    }
                });
            }
            tick += 1;
            cx.background_executor().timer(Duration::from_millis(300)).await;
        }
    })
    .detach();
}

/// How what's going is going: the writers' progress files, and namiOS's window.
fn follow(g: &mut Shell, cx: &mut Context<Shell>) {
    let m = &mut g.move_to;
    let usb_was = m.usb.going();
    read_progress(&mut m.usb);
    let trial_was = m.trial.going();
    read_progress(&mut m.trial);
    // Ready for a window: the disk as namiOS gets it, then namiOS in its window.
    if trial_was && matches!(m.trial, Work::Done) {
        let version = m.release.as_ref().and_then(|r| r.as_ref().ok()).map(|r| r.version.clone()).unwrap_or_default();
        match ready_trial(&version) {
            Ok(()) => start_vm(m),
            Err(e) => m.trial = Work::Failed(e),
        }
    }
    if let Some(vm) = &mut m.vm {
        if let Ok(Some(status)) = vm.try_wait() {
            m.vm = None;
            if !status.success() {
                let log = std::fs::read_to_string(trial_dir().join("try.log")).unwrap_or_default();
                eprintln!("nami-gui: nami try-os --disk: {log}");
                m.trial = Work::Failed(vm_failed(&log));
            }
        }
    }
    if usb_was || trial_was || m.usb.going() || m.trial.going() {
        cx.notify();
    }
}

/// A writer's word (`write::say`'s file), onto its `Work`.
fn read_progress(w: &mut Work) {
    let Work::Going { progress, step, done, total, child } = w else { return };
    let said: Option<serde_json::Value> = std::fs::read_to_string(&*progress).ok().and_then(|t| serde_json::from_str(t.trim()).ok());
    if let Some(v) = said {
        match v["step"].as_str().unwrap_or("") {
            "done" => {
                let p = progress.clone();
                let _ = std::fs::remove_file(&p);
                let _ = std::fs::remove_dir_all(p.with_extension("seed"));
                *w = Work::Done;
                return;
            }
            "error" => {
                let m = v["message"].as_str().unwrap_or("Something went wrong. Try again.").to_string();
                let p = progress.clone();
                let _ = std::fs::remove_file(&p);
                let _ = std::fs::remove_dir_all(p.with_extension("seed"));
                *w = Work::Failed(if m == "stopped" { "Stopped.".into() } else { m });
                return;
            }
            s => {
                *step = s.to_string();
                *done = v["done"].as_u64().unwrap_or(0);
                if let Some(t) = v["total"].as_u64().filter(|t| *t > 0) {
                    *total = t;
                }
            }
        }
    }
    // The password prompt said no, or the writer stopped without a word.
    if let Some(c) = child {
        if let Ok(Some(status)) = c.try_wait() {
            *child = None;
            if step == "start" {
                *w = Work::Failed(if status.success() { "The USB drive couldn't be made. Try again.".into() } else { "Making a USB drive needs an administrator's password. Try again, and enter it when asked.".into() });
            }
        }
    }
}

/// What a step of the writing is called, and how far it's got (none: going on, no measure).
pub fn step_said(step: &str, usb: bool, done: u64, total: u64) -> (&'static str, Option<f32>) {
    let part = Some(done as f32 / total.max(1) as f32);
    match step {
        "write" => ("Downloading namiOS…", part),
        "check" if usb => ("Verifying the USB drive…", part),
        "check" => ("Verifying namiOS…", part),
        "prepare" if usb => ("Preparing the USB drive…", None),
        "prepare" => ("Preparing…", None),
        "seed" => ("Adding your apps and settings…", None),
        _ if usb => ("Waiting for permission…", None),
        _ => ("Preparing…", None),
    }
}

// --- What's handed to namiOS -------------------------------------------------------------

/// What this computer says of where it is and how it's used, for namiOS's setup to offer
/// first: its time zone, language and keyboard.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Here {
    pub timezone: Option<String>,
    pub language: Option<String>,
    pub keyboard: Option<String>,
}

impl Here {
    pub fn now() -> Here {
        let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
        let timezone = std::fs::read_to_string("/etc/timezone").ok().map(|t| t.trim().to_string()).filter(|t| plain(t)).or_else(|| std::fs::read_link("/etc/localtime").ok().and_then(|l| zone_of_link(&l)));
        let language = std::env::var("LANG").ok().and_then(|l| language_of(&l));
        let keyboard = keyboard_of(&read("/etc/default/keyboard")).or_else(|| keyboard_of(&read("/etc/X11/xorg.conf.d/00-keyboard.conf"))).or_else(|| keyboard_of(&read("/etc/vconsole.conf")));
        Here { timezone, language, keyboard }
    }
}

/// A word a file can hold as it is, and the login screen may show.
fn plain(v: &str) -> bool {
    !v.is_empty() && v.len() <= 64 && v.chars().all(|c| c.is_ascii_alphanumeric() || "_-+./@".contains(c))
}

/// /etc/localtime's link (`/usr/share/zoneinfo/Europe/Copenhagen`): its zone.
pub fn zone_of_link(link: &Path) -> Option<String> {
    let s = link.to_string_lossy();
    let zone = s.split_once("zoneinfo/")?.1.trim_start_matches("posix/").trim_start_matches("right/");
    (plain(zone) && zone.contains('/') || zone == "UTC").then(|| zone.to_string())
}

/// LANG (`da_DK.UTF-8`): the locale namiOS takes, when it's a language's.
pub fn language_of(lang: &str) -> Option<String> {
    let l = lang.trim();
    let base = l.split(['.', '@']).next().unwrap_or("");
    if base.is_empty() || matches!(base, "C" | "POSIX") || !plain(l) {
        return None;
    }
    Some(if l.contains('.') { l.to_string() } else { format!("{l}.UTF-8") })
}

/// The keyboard layout a system file says: Debian's `XKBLAYOUT="dk"`, Fedora's and Arch's
/// `Option "XkbLayout" "dk"`, or the console's `KEYMAP=dk`. The first of several.
pub fn keyboard_of(text: &str) -> Option<String> {
    for line in text.lines().map(str::trim) {
        let value = if let Some(v) = line.strip_prefix("XKBLAYOUT=").or_else(|| line.strip_prefix("KEYMAP=")) {
            v.trim_matches(['"', '\''])
        } else if line.starts_with("Option") && line.contains("\"XkbLayout\"") {
            line.split('"').nth(3).unwrap_or("")
        } else {
            continue;
        };
        let first = value.split(',').next().unwrap_or("").trim();
        if !first.is_empty() && first.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Some(first.to_string());
        }
    }
    None
}

/// A computer's name as namiOS takes it (its hostname): small letters, digits and dashes.
/// What's typed, made so; empty if nothing's left.
pub fn hostname_of(typed: &str) -> String {
    let mut out = String::new();
    for c in typed.trim().chars() {
        match c {
            'a'..='z' | '0'..='9' => out.push(c),
            'A'..='Z' => out.push(c.to_ascii_lowercase()),
            ' ' | '-' | '_' | '.' if !out.is_empty() && !out.ends_with('-') => out.push('-'),
            _ => {}
        }
    }
    out.truncate(63);
    out.trim_end_matches('-').to_string()
}

fn default_name() -> String {
    let h = std::fs::read_to_string("/etc/hostname").unwrap_or_default();
    let h = hostname_of(h.lines().next().unwrap_or(""));
    if h.is_empty() || h == "localhost" { "namios".into() } else { h }
}

/// The new computer's file: namiOS as it comes (what its image has: packaging/namios), named
/// `name`, with what this computer says of its time zone, language and keyboard for the
/// setup to offer, and the apps this one has for everyone. namiOS's setup still asks who.
pub fn computer_file(name: &str, here: &Here, apps: &[String], unfree: bool, from: &str) -> String {
    let mut out = String::from("# This computer, written down. namiOS sets itself up from this file:\n# add a package below and it's installed.\n");
    out += &format!("# Made on {from} with Move to namiOS.\n");
    let name = hostname_of(name);
    out += &format!("hostname: {}\n", if name.is_empty() { "namios" } else { &name });
    for (key, value) in [("timezone", &here.timezone), ("language", &here.language), ("keyboard", &here.keyboard)] {
        if let Some(v) = value.as_deref().filter(|v| plain(v)) {
            out += &format!("{key}: {v}\n");
        }
    }
    if unfree {
        out += "unfree: true\n";
    }
    out += "packages:\n  - namios\n";
    for a in apps {
        out += &format!("  - {a}\n");
    }
    out
}

/// The apps a computer's file lists that come along by name: the catalog's (not a folder's or
/// a repo's, which aren't on the new computer), and not namiOS itself.
pub fn apps_that_come(packages: &[serde_json::Value]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in packages {
        let name = match p {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Object(m) => m.keys().next().cloned().unwrap_or_default(),
            _ => continue,
        };
        let ok = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.+@".contains(c)) && !name.starts_with(['.', '-']);
        if ok && !matches!(name.as_str(), "namios" | "namios-bare") && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// This Linux's name, as /etc/os-release says it (`NAME`).
pub fn linux_name_of(os_release: &str) -> Option<String> {
    os_release.lines().find_map(|l| l.strip_prefix("NAME=")).map(|v| v.trim().trim_matches(['"', '\'']).to_string()).filter(|v| !v.is_empty())
}

/// This Linux's name, if it has one that isn't namiOS's (the preview's: no other to name).
pub fn linux_name() -> Option<String> {
    linux_name_of(&std::fs::read_to_string("/etc/os-release").unwrap_or_default()).filter(|n| !n.to_lowercase().starts_with("nami"))
}

/// What namiOS gets: this computer's file made for it, and their own file whole.
fn seed(g: &Shell, cx: &gpui::App) -> Seed {
    let name = g.move_to.name.read(cx).content.to_string();
    let apps = apps_that_come(&super::packages(&g.system));
    let unfree = g.system.get("unfree").and_then(|u| u.as_bool()) == Some(true);
    let system = computer_file(&name, &here(), &apps, unfree, &linux_name().unwrap_or_else(|| "another distribution".into()));
    let user = g.role.as_ref().ok().and_then(|r| std::fs::read_to_string(&r.user).ok()).filter(|t| !t.trim().is_empty());
    Seed { system: Some(system), user, source: None }
}

fn here() -> Here {
    static HERE: std::sync::OnceLock<Here> = std::sync::OnceLock::new();
    HERE.get_or_init(Here::now).clone()
}

// --- namiOS in a window ------------------------------------------------------------------

fn trial_dir() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    cache.join("nami").join("try-namios")
}

/// The trial's disk, and the version on it once it's ready.
fn trial_disk() -> PathBuf {
    trial_dir().join("namios.raw")
}

fn trial_ready() -> Option<String> {
    let v = std::fs::read_to_string(trial_dir().join("ready")).ok()?;
    trial_disk().is_file().then(|| v.trim().to_string())
}

/// Whether namiOS can run in a window here: virtualization, a nami that starts disks, room.
fn can_try() -> Result<(), String> {
    let kvm = Path::new("/dev/kvm");
    if !kvm.exists() {
        return Err("Virtualization is turned off on this computer, so namiOS can't run in a window. You can turn it on in your computer's firmware settings, where it's called VT-x, AMD-V or SVM.".into());
    }
    if std::fs::OpenOptions::new().read(true).write(true).open(kvm).is_err() {
        return Err("You don't have permission to use virtualization on this computer. Add yourself to the kvm group, log out and back in, then try again.".into());
    }
    let help = std::process::Command::new(std::env::var_os("NAMI").unwrap_or_else(|| "nami".into())).args(["try-os", "--help"]).output();
    if !help.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("--disk")) {
        return Err("This version of nami can't run namiOS in a window. Update nami, then try again.".into());
    }
    if trial_ready().is_none() {
        let dir = trial_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Some(free) = free_space(&dir).filter(|f| *f < TRIAL_ROOM) {
            return Err(format!("Running namiOS in a window needs {} of free space. {} is available.", gb(TRIAL_ROOM), gb(free)));
        }
    }
    Ok(())
}

fn free_space(at: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(at.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0).then(|| s.f_bavail as u64 * s.f_frsize as u64)
}

/// Get namiOS ready for a window: the image written onto a disk file as onto a USB drive
/// (downloaded, checked against the release's signed checksum, read back), with their files.
fn start_trial(g: &mut Shell, cx: &gpui::App) {
    let Some(Ok(r)) = g.move_to.release.clone() else { return };
    let dir = trial_dir();
    let disk = trial_disk();
    let progress = dir.join("progress.json");
    let _ = std::fs::remove_file(dir.join("ready"));
    let made = std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::File::create(&disk).map(|_| ()))
        .and_then(|_| std::fs::write(&progress, "{\"step\":\"prepare\"}\n"));
    let _ = std::fs::remove_file(progress.with_extension("stop"));
    let _ = std::fs::remove_file(dir.join("namios.vars"));
    if let Err(e) = made {
        g.move_to.trial = Work::Failed(format!("Couldn't make room for namiOS in {}: {e}", dir.display()));
        return;
    }
    let seed = seed(g, cx);
    let job = crate::write::Job { url: r.url.clone(), sha256: r.sha256.clone(), disk: disk.display().to_string(), progress: progress.clone(), seed: (!seed.is_empty()).then_some(seed) };
    std::thread::spawn(move || crate::write::run(&job));
    g.move_to.trial = Work::Going { progress, step: "prepare".into(), done: 0, total: r.size, child: None };
}

/// Written and checked: room for namiOS to grow into, and it's ready.
fn ready_trial(version: &str) -> Result<(), String> {
    let disk = trial_disk();
    let f = std::fs::OpenOptions::new().write(true).open(&disk).map_err(|e| format!("{}: {e}", disk.display()))?;
    if f.metadata().map(|m| m.len()).unwrap_or(0) < TRIAL_DISK {
        f.set_len(TRIAL_DISK).map_err(|e| format!("{}: {e}", disk.display()))?;
    }
    std::fs::write(trial_dir().join("ready"), version).map_err(|e| e.to_string())
}

fn start_vm(m: &mut MoveTo) {
    let log = std::fs::File::create(trial_dir().join("try.log"));
    let mut c = std::process::Command::new(std::env::var_os("NAMI").unwrap_or_else(|| "nami".into()));
    c.args(["try-os", "--disk"]).arg(trial_disk()).stdin(std::process::Stdio::null());
    if let Ok(log) = log {
        if let Ok(l2) = log.try_clone() {
            c.stdout(log).stderr(l2);
        }
    }
    match c.spawn() {
        Ok(child) => {
            m.vm = Some(child);
            m.trial = Work::Idle;
        }
        Err(e) => m.trial = Work::Failed(format!("Couldn't start namiOS: {e}")),
    }
}

/// What `nami try-os --disk` said as it stopped, in a sentence.
fn vm_failed(log: &str) -> String {
    let last = log.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if last.is_empty() { "namiOS stopped unexpectedly. Try again.".into() } else { format!("namiOS stopped unexpectedly. {last}") }
}

fn remove_trial() {
    let _ = std::fs::remove_dir_all(trial_dir());
}

// --- The USB drive -----------------------------------------------------------------------

/// Erase was pressed: written, if it's still the drive asked about.
fn make_usb(g: &mut Shell, cx: &gpui::App) {
    let Some(asked) = g.move_to.confirming.take() else { return };
    let m = &g.move_to;
    if !(m.picked.as_deref() == Some(asked.path.as_str()) && m.disks.contains(&asked)) {
        return;
    }
    let Some(Ok(r)) = m.release.clone() else { return };
    let is_file = std::fs::metadata(&asked.path).is_ok_and(|m| m.is_file());
    // (The preview on namiOS: a real drive isn't touched. A file standing in for one is.)
    if crate::look::as_other_linux() && !is_file {
        g.move_to.previewed = Some(asked.named());
        return;
    }
    g.move_to.previewed = None;
    let progress = std::env::temp_dir().join(format!("nami-gui-usb-{}.json", std::process::id()));
    let _ = std::fs::remove_file(progress.with_extension("stop"));
    if std::fs::write(&progress, "{\"step\":\"start\"}\n").is_err() {
        g.move_to.usb = Work::Failed("The USB drive couldn't be made. Try again.".into());
        return;
    }
    let mut args = vec!["namios-write".to_string(), "--disk".into(), asked.path.clone(), "--url".into(), r.url.clone(), "--sha256".into(), r.sha256.clone(), "--progress".into(), progress.display().to_string()];
    let seed = seed(g, cx);
    if !seed.is_empty() {
        let dir = progress.with_extension("seed");
        if seed.save(&dir).is_err() {
            g.move_to.usb = Work::Failed("The USB drive couldn't be made. Try again.".into());
            return;
        }
        args.extend(["--seed".into(), dir.display().to_string()]);
    }
    match elevated(&args, is_file) {
        Ok(child) => g.move_to.usb = Work::Going { progress, step: "start".into(), done: 0, total: r.size, child: Some(child) },
        Err(e) => g.move_to.usb = Work::Failed(e),
    }
}

/// `nami-gui <args>`, as root: with an admin's password (pkexec). A file standing in for a
/// drive needs none.
fn elevated(args: &[String], is_file: bool) -> Result<std::process::Child, String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut c = if is_file || unsafe { libc::geteuid() } == 0 {
        std::process::Command::new(&me)
    } else {
        let mut c = std::process::Command::new("pkexec");
        c.arg(&me);
        c
    };
    c.args(args).stdin(std::process::Stdio::null());
    c.spawn().map_err(|e| format!("Couldn't ask for an administrator's password (pkexec): {e}"))
}

/// `nami-gui namios-write --url .. --sha256 .. --disk .. --progress .. [--seed <folder>]`: the
/// namiOS Installer's writer (as root). Whether it was this; it exits.
pub fn command(args: &[String]) -> bool {
    if args.first().map(String::as_str) != Some("namios-write") {
        return false;
    }
    let arg = |n: &str| args.iter().position(|a| a == n).and_then(|i| args.get(i + 1)).cloned();
    let need = |n: &str| arg(n).unwrap_or_else(|| {
        eprintln!("nami-gui namios-write: {n} <...>");
        std::process::exit(2)
    });
    let progress = PathBuf::from(need("--progress"));
    let seed = match arg("--seed") {
        None => None,
        Some(dir) => match Seed::load(Path::new(&dir)) {
            Ok(s) => Some(s),
            Err(e) => {
                crate::write::say(&progress, serde_json::json!({ "step": "error", "message": e }));
                std::process::exit(1)
            }
        },
    };
    let job = crate::write::Job { url: need("--url"), sha256: need("--sha256"), disk: need("--disk"), progress, seed };
    std::process::exit(crate::write::run(&job))
}

// --- The page ----------------------------------------------------------------------------

fn gb(b: u64) -> String {
    let g = b as f64 / 1e9;
    if g < 10. { format!("{g:.1} GB") } else { format!("{g:.0} GB") }
}

fn muted(t: impl Into<String>) -> Div {
    div().text_size(px(text::CALLOUT)).line_height(gpui::relative(text::leading(text::CALLOUT))).text_color(rgb(color::muted())).child(t.into())
}

fn bad(t: impl Into<String>) -> Div {
    div().p(px(12.)).rounded(px(crate::look::shape::CONTROL)).bg(rgb(color::bad_soft())).text_color(rgb(color::bad())).text_size(px(text::CALLOUT)).child(t.into())
}

fn title(t: &str) -> Div {
    div().text_size(px(text::SECTION)).font_weight(FontWeight::SEMIBOLD).child(t.to_string())
}

/// namiOS's desktop, as the site shows it: light or dark, as the window is.
fn picture() -> Arc<gpui::Image> {
    static LIGHT: std::sync::OnceLock<Arc<gpui::Image>> = std::sync::OnceLock::new();
    static DARK: std::sync::OnceLock<Arc<gpui::Image>> = std::sync::OnceLock::new();
    let dark = color::DARK.load(std::sync::atomic::Ordering::Relaxed);
    let (cell, bytes): (_, &'static [u8]) = if dark {
        (&DARK, include_bytes!("../../assets/move-to/desktop-dark.webp"))
    } else {
        (&LIGHT, include_bytes!("../../assets/move-to/desktop.webp"))
    };
    cell.get_or_init(|| Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Webp, bytes.to_vec()))).clone()
}

fn point(icon: &str, head: &str, body: String) -> Div {
    div()
        .flex()
        .gap(px(10.))
        .child(div().pt(px(2.)).child(glyph(icon, 16., color::muted())))
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.)).child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(head.to_string())).child(muted(body)))
}

fn progress_view(id: &'static str, w: &Work, usb: bool) -> Option<Div> {
    let Work::Going { step, done, total, .. } = w else { return None };
    let (what, part) = step_said(step, usb, *done, *total);
    let of = (part.is_some() && *total > 0).then(|| format!("{} of {}", gb(*done), gb(*total)));
    Some(
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(div().flex().justify_between().child(div().text_size(px(text::BODY)).child(what)).children(of.map(muted)))
            .child(crate::motion::bar(id, part.map(|p| p.min(1.)), 6., color::fill())),
    )
}

pub fn render(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    let linux = linux_name().unwrap_or_else(|| "your current system".into());
    let preview = crate::look::as_other_linux();
    let mine = super::packages(&g.user).len();
    let theirs = apps_that_come(&super::packages(&g.system)).len();
    let apps = match (mine, theirs) {
        (0, 0) => "Everything you install with nami comes along: your apps and settings are in your user.nami, which namiOS sets you up from.".to_string(),
        (n, 0) => format!("Your {n} app{} and your settings come along. namiOS sets you up from the same user.nami.", if n == 1 { "" } else { "s" }),
        (n, t) => format!("Your {n} app{}, this computer's {t}, and your settings come along. namiOS sets you up from the same files.", if n == 1 { "" } else { "s" }),
    };
    let mut page = div().flex().flex_col().gap(px(crate::look::shape::GAP)).child(crate::ui::intro("namiOS is the operating system built on nami, with a desktop of its own. It's set up from the same files nami uses here."));
    if preview {
        page = page.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(12.))
                .py(px(8.))
                .rounded(px(crate::look::shape::CONTROL))
                .bg(color::push())
                .child(glyph("computer", 14., color::muted()))
                .child(muted("Preview: this is how Move to namiOS looks on another distribution. A USB drive is not erased here.")),
        );
    }

    // What namiOS is: its desktop, and what you'd get.
    page = page.child(
        crate::look::module().flex().flex_row().gap(px(18.)).p(px(16.)).items_center().child(img(picture()).w(px(272.)).h(px(170.)).flex_none().rounded(px(crate::look::shape::CONTROL))).child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(point("setup", "Set Up From Files", "This computer, your account and your projects are each described in a file. Change a line and it's done, and you can always go back to an earlier version.".into()))
                .child(point("apps", "Ready to Use", "A desktop, apps and settings that are chosen to work together, with updates you can undo.".into()))
                .child(point("check", "Your Apps Come Along", apps)),
        ),
    );

    page = page.child(trial_part(g, cx));
    page = page.child(usb_part(g, &linux, cx));

    // Their files: not on the drive. How they come over.
    page = page.child(
        crate::look::module().flex().flex_col().gap(px(10.)).p(px(16.)).child(title("Your Files"))
            .child(muted("Your documents, photos and other files aren't added to the USB drive. After namiOS is set up, open Settings > Transfer Your Files to copy them from your old disk or from a backup."))
            .child(muted("If namiOS will replace this system, back up your files to another disk first."))
            // (Where there's a Backups page to go to.)
            .children(crate::os::page_named("backups").map(|backups| div().flex().child(button("move-backups", "Back Up Now…", false, true).on_click(cx.listener(move |g, _, w, cx| g.go(backups, w, cx)))))),
    );
    page
}

fn trial_part(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    let m = &g.move_to;
    let mut part = crate::look::module().flex().flex_col().gap(px(10.)).p(px(16.)).child(title("Try namiOS in a Window"));
    let ready = trial_ready();
    let running = m.vm.is_some();
    if running {
        return part.child(muted("namiOS is running in its own window. Close that window to stop it.")).child(div().flex().child(button("move-try-stop", "Stop namiOS", false, true).on_click(cx.listener(|g, _, _, cx| {
            if let Some(vm) = &mut g.move_to.vm {
                let _ = vm.kill();
            }
            cx.notify();
        }))));
    }
    if let Some(p) = progress_view("move-try-bar", &m.trial, false) {
        return part.child(p).child(div().flex().child(button("move-try-cancel", "Stop", false, true).on_click(cx.listener(|g, _, _, cx| {
            if let Work::Going { progress, .. } = &g.move_to.trial {
                let _ = std::fs::write(progress.with_extension("stop"), "");
            }
            cx.notify();
        }))));
    }
    match &m.can_try {
        None => return part.child(muted("Checking this computer…")),
        Some(Err(why)) => return part.child(muted(why.clone())),
        Some(Ok(())) => {}
    }
    if let Some(version) = ready {
        let used = std::fs::metadata(trial_disk()).map(|m| {
            use std::os::unix::fs::MetadataExt;
            m.blocks() * 512
        });
        part = part.child(muted(format!("namiOS {version} is ready to run in a window, with your apps and settings. Changes you make in it are kept until you remove it.")));
        if let Work::Failed(e) = &m.trial {
            part = part.child(bad(e.clone()));
        }
        return part.child(
            div()
                .flex()
                .gap(px(8.))
                .child(button("move-try-open", "Open namiOS", true, true).on_click(cx.listener(|g, _, _, cx| {
                    start_vm(&mut g.move_to);
                    cx.notify();
                })))
                .child(button("move-try-remove", match used { Ok(b) => format!("Remove ({})", gb(b)), Err(_) => "Remove".into() }, false, true).on_click(cx.listener(|g, _, _, cx| {
                    remove_trial();
                    g.move_to.trial = Work::Idle;
                    cx.notify();
                }))),
        );
    }
    let said = match &m.release {
        None => "Finding the latest namiOS…".to_string(),
        Some(Err(_)) => "namiOS isn't available to download right now. Check your internet connection, then try again.".to_string(),
        Some(Ok(r)) => format!("namiOS {} starts in a window, set up with your apps and settings, so you can see how it works. Nothing on this computer changes. The download is {}, and it uses up to {} of disk space.", r.version, gb(r.size), gb(TRIAL_DISK)),
    };
    part = part.child(muted(said));
    if let Some(Err(e)) = &m.release {
        eprintln!("nami-gui: the latest namiOS: {e}");
    }
    if let Work::Failed(e) = &m.trial {
        part = part.child(bad(e.clone()));
    }
    // (Not found: looked for again. Found: downloaded, checked, and started.)
    let lost = matches!(m.release, Some(Err(_)));
    let ready = lost || matches!(m.release, Some(Ok(_)));
    let label = if lost || matches!(m.trial, Work::Failed(_)) { "Try Again" } else { "Try namiOS in a Window" };
    part.child(div().flex().gap(px(8.)).child(button("move-try", label, true, ready).when(ready, |b| {
        b.on_click(cx.listener(|g, _, _, cx| {
            if matches!(g.move_to.release, Some(Err(_))) {
                look(g, cx);
            } else {
                start_trial(g, cx);
                follow(g, cx);
            }
            cx.notify();
        }))
    })))
}

fn usb_part(g: &mut Shell, linux: &str, cx: &mut Context<Shell>) -> Div {
    let preview_files = seed(g, cx);
    let m = &g.move_to;
    let mut part = crate::look::module().flex().flex_col().gap(px(12.)).p(px(16.)).child(title("Make a namiOS USB Drive")).child(muted(format!(
        "Start this computer or another one from the USB drive to install namiOS. namiOS replaces {linux} on the disk you install it on, or goes on a disk of its own. It can't be installed next to {linux}."
    )));
    // What's going, or done.
    if let Some(p) = progress_view("move-usb-bar", &m.usb, true) {
        return part.child(p).child(div().flex().child(button("move-usb-stop", "Stop", false, true).on_click(cx.listener(|g, _, _, cx| {
            if let Work::Going { progress, .. } = &g.move_to.usb {
                let _ = std::fs::write(progress.with_extension("stop"), "");
            }
            cx.notify();
        }))));
    }
    if matches!(m.usb, Work::Done) {
        return part
            .child(div().flex().items_center().gap(px(8.)).child(glyph("check", 16., color::good())).child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child("Your namiOS USB drive is ready.")))
            .child(muted("Restart the computer and choose the USB drive as it starts (often with F12, F10 or Esc). namiOS sets itself up with your apps and settings when it first starts."))
            .child(div().flex().child(button("move-usb-another", "Make Another", false, true).on_click(cx.listener(|g, _, _, cx| {
                g.move_to.usb = Work::Idle;
                cx.notify();
            }))));
    }

    // The name, and the files namiOS gets.
    part = part.child(
        div().flex().items_center().gap(px(12.)).child(div().w(px(140.)).flex_none().text_size(px(text::BODY)).child("Computer name")).child(div().w(px(240.)).child(m.name.clone())),
    );
    let files: Vec<(&str, String)> = [("system.nami", preview_files.system.clone()), ("user.nami", preview_files.user.clone())].into_iter().filter_map(|(n, t)| Some((n, t?))).collect();
    let tab = m.tab.min(files.len().saturating_sub(1));
    let mut tabs = div().flex().gap(px(6.)).items_center();
    for (i, (n, _)) in files.iter().enumerate() {
        tabs = tabs.child(crate::look::tab(("move-tab", i), *n, tab == i).stop(cx).on_click(cx.listener(move |g, _, _, cx| {
            g.move_to.tab = i;
            cx.notify();
        })));
    }
    part = part
        .child(div().text_size(px(text::META)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(color::muted())).child("Added to the USB drive for namiOS's first start"))
        .child(tabs)
        .children(files.get(tab).map(|(_, t)| div().h(px(180.)).flex().flex_col().child(crate::file_view::file_view("move-file", t, None))));

    // The drive.
    match &m.release {
        None => part = part.child(muted("Finding the latest namiOS…")),
        Some(Err(_)) => part = part.child(muted("namiOS isn't available to download right now. Check your internet connection, then try again.")),
        Some(Ok(r)) => part = part.child(muted(format!("Download size: {}. You need a USB drive of 16 GB or more. All data on it will be erased.", gb(r.size)))),
    }
    if m.disks.is_empty() {
        part = part.child(div().p(px(16.)).rounded(px(crate::look::shape::CONTROL)).border_1().border_color(rgb(color::line())).text_size(px(text::BODY)).text_color(rgb(color::muted())).child("Plug in a USB drive."));
    }
    for (i, d) in m.disks.iter().enumerate() {
        let fits = d.bytes >= LEAST;
        let path = d.path.clone();
        let picked = m.picked.as_deref() == Some(&d.path);
        let card = crate::ui::choice(("move-disk", i), d.describe(), if fits { d.path.clone() } else { format!("{} (too small, needs 16 GB or more)", d.path) }, picked);
        part = part.child(if fits {
            card.on_click(cx.listener(move |g, _, _, cx| {
                g.move_to.picked = Some(path.clone());
                cx.notify();
            }))
        } else {
            card.opacity(0.5)
        });
    }
    if let Some(named) = &m.previewed {
        part = part.child(muted(format!("Preview: on another distribution, {named} would now be erased, and namiOS written to it with the files above. Nothing was written.")));
    }
    if let Work::Failed(e) = &m.usb {
        part = part.child(bad(e.clone()));
    }
    if let Some(asked) = m.confirming.clone() {
        return part.child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .p(px(14.))
                .rounded(px(crate::look::shape::CONTROL))
                .border_1()
                .border_color(rgb(color::line()))
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(format!("Erase {}?", asked.named())))
                .child(muted(format!("All data on this USB drive ({}) will be permanently erased.", asked.path)))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(button("move-cancel-erase", "Cancel", false, true).on_click(cx.listener(|g, _, _, cx| {
                            g.move_to.confirming = None;
                            cx.notify();
                        })))
                        .child(destructive_button("move-erase", "Erase").on_click(cx.listener(|g, _, _, cx| {
                            make_usb(g, cx);
                            follow(g, cx);
                            cx.notify();
                        }))),
                ),
        );
    }
    let named_ok = !hostname_of(&m.name.read(cx).content).is_empty();
    let ready = matches!(m.release, Some(Ok(_))) && m.picked.is_some() && named_ok;
    let label = if matches!(m.usb, Work::Failed(_)) { "Try Again" } else { "Make USB Drive" };
    part.child(div().flex().justify_end().child(button("move-make", label, true, ready).when(ready, |b| {
        b.on_click(cx.listener(|g, _, _, cx| {
            let m = &mut g.move_to;
            m.previewed = None;
            m.confirming = m.disks.iter().find(|d| Some(&d.path) == m.picked.as_ref()).cloned();
            cx.notify();
        }))
    })))
}

/// A line on Overview, off namiOS: the way to this page.
pub fn home_row(cx: &mut Context<Shell>) -> impl IntoElement {
    crate::look::group().child(
        crate::ui::link_row("home-move-to-namios", true)
            .justify_start()
            .child(glyph("computer", 16., color::muted()))
            .child(div().flex_1().text_size(px(text::BODY)).child("Move to namiOS"))
            .child(muted("Your apps and settings come along"))
            .child(crate::ui::chevron(""))
            .on_click(cx.listener(|g, _, w, cx| g.go(Page::MoveToNamios, w, cx))),
    )
}

/// Whether the page is offered: not on namiOS (or a computer nami made).
pub fn offered(g: &Shell) -> bool {
    // (Not on a Mac: its nami leaves namiOS out, as docs/macos.md has it.)
    namios_public() && !crate::look::MAC && !crate::look::on_namios() && !g.nami_os
}

/// Whether namiOS is offered to everyone yet. It isn't: it's an invite-only alpha, so the
/// app doesn't point people at it. `NAMI_SHOW_NAMIOS=1` shows Move to namiOS all the same
/// (to try it); at namiOS's launch this becomes true.
fn namios_public() -> bool {
    const LAUNCHED: bool = false;
    LAUNCHED || std::env::var_os("NAMI_SHOW_NAMIOS").is_some_and(|v| v == "1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_computers_name_as_namios_takes_it() {
        assert_eq!(hostname_of("Ada's Laptop"), "adas-laptop");
        assert_eq!(hostname_of("  ubuntu-desktop  "), "ubuntu-desktop");
        assert_eq!(hostname_of("my_pc.local"), "my-pc-local");
        assert_eq!(hostname_of("--x--"), "x");
        assert_eq!(hostname_of("Ærø"), "r");
        assert_eq!(hostname_of("!!!"), "");
        assert_eq!(hostname_of(&"a".repeat(80)).len(), 63);
    }

    #[test]
    fn where_and_how_this_computer_is_used() {
        assert_eq!(zone_of_link(Path::new("/usr/share/zoneinfo/Europe/Copenhagen")).as_deref(), Some("Europe/Copenhagen"));
        assert_eq!(zone_of_link(Path::new("../usr/share/zoneinfo/America/New_York")).as_deref(), Some("America/New_York"));
        assert_eq!(zone_of_link(Path::new("/usr/share/zoneinfo/UTC")).as_deref(), Some("UTC"));
        assert_eq!(zone_of_link(Path::new("/etc/elsewhere")), None);
        assert_eq!(language_of("da_DK.UTF-8").as_deref(), Some("da_DK.UTF-8"));
        assert_eq!(language_of("en_US").as_deref(), Some("en_US.UTF-8"));
        assert_eq!(language_of("C.UTF-8"), None);
        assert_eq!(language_of("POSIX"), None);
        assert_eq!(language_of(""), None);
        assert_eq!(keyboard_of("XKBMODEL=\"pc105\"\nXKBLAYOUT=\"dk,us\"\n").as_deref(), Some("dk"));
        assert_eq!(keyboard_of("Section \"InputClass\"\n        Option \"XkbLayout\" \"de\"\nEndSection\n").as_deref(), Some("de"));
        assert_eq!(keyboard_of("KEYMAP=us\n").as_deref(), Some("us"));
        assert_eq!(keyboard_of("FONT=x\n"), None);
    }

    #[test]
    fn the_new_computers_file() {
        let here = Here { timezone: Some("Europe/Copenhagen".into()), language: Some("da_DK.UTF-8".into()), keyboard: Some("dk".into()) };
        let text = computer_file("Ada Laptop", &here, &["htop".into(), "gimp".into()], true, "Ubuntu");
        let v: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        assert_eq!(v["hostname"].as_str(), Some("ada-laptop"));
        assert_eq!(v["timezone"].as_str(), Some("Europe/Copenhagen"));
        assert_eq!(v["language"].as_str(), Some("da_DK.UTF-8"));
        assert_eq!(v["keyboard"].as_str(), Some("dk"));
        assert_eq!(v["unfree"].as_bool(), Some(true));
        let packages: Vec<&str> = v["packages"].as_sequence().unwrap().iter().filter_map(|p| p.as_str()).collect();
        assert_eq!(packages, ["namios", "htop", "gimp"]);
        assert!(text.starts_with("# This computer, written down."), "{text}");
        assert!(text.contains("Made on Ubuntu"), "{text}");
        // (Nothing known of where it is, nobody named: namiOS's setup asks.)
        let bare: serde_yaml::Value = serde_yaml::from_str(&computer_file("", &Here::default(), &[], false, "Fedora")).unwrap();
        assert_eq!(bare["hostname"].as_str(), Some("namios"));
        assert!(bare.get("timezone").is_none() && bare.get("unfree").is_none() && bare.get("users").is_none());
        // (What isn't a plain word stays out.)
        let odd = Here { timezone: Some("a: b".into()), ..Default::default() };
        assert!(!computer_file("x", &odd, &[], false, "Arch").contains("timezone"));
    }

    #[test]
    fn the_apps_that_come_along() {
        let listed = [json!("htop"), json!({"gimp": {"version": "3"}}), json!("namios"), json!("./mine"), json!("github:me/pkgs"), json!("htop"), json!("python3Packages.numpy")];
        assert_eq!(apps_that_come(&listed), ["htop", "gimp", "python3Packages.numpy"]);
    }

    #[test]
    fn this_linuxs_name() {
        assert_eq!(linux_name_of("PRETTY_NAME=\"Ubuntu 24.04 LTS\"\nNAME=\"Ubuntu\"\nID=ubuntu\n").as_deref(), Some("Ubuntu"));
        assert_eq!(linux_name_of("NAME=Fedora Linux\n").as_deref(), Some("Fedora Linux"));
        assert_eq!(linux_name_of("ID=arch\n"), None);
    }

    #[test]
    fn each_step_says_what_it_is() {
        assert_eq!(step_said("write", true, 1, 4), ("Downloading namiOS…", Some(0.25)));
        assert_eq!(step_said("check", true, 0, 1).0, "Verifying the USB drive…");
        assert_eq!(step_said("check", false, 0, 1).0, "Verifying namiOS…");
        assert_eq!(step_said("seed", false, 0, 0), ("Adding your apps and settings…", None));
        assert_eq!(step_said("start", true, 0, 0).0, "Waiting for permission…");
    }

    #[test]
    fn a_writers_word() {
        let dir = std::env::temp_dir().join(format!("nami-gui-move-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("p.json");
        let mut w = Work::Going { progress: p.clone(), step: "start".into(), done: 0, total: 10, child: None };
        std::fs::write(&p, "{\"step\":\"write\",\"done\":5,\"total\":20}\n").unwrap();
        read_progress(&mut w);
        assert!(matches!(&w, Work::Going { step, done: 5, total: 20, .. } if step == "write"));
        std::fs::write(&p, "{\"step\":\"error\",\"message\":\"stopped\"}\n").unwrap();
        read_progress(&mut w);
        assert!(matches!(&w, Work::Failed(m) if m == "Stopped."));
        let mut w = Work::Going { progress: p.clone(), step: "check".into(), done: 0, total: 10, child: None };
        std::fs::write(&p, "{\"step\":\"done\"}\n").unwrap();
        read_progress(&mut w);
        assert!(matches!(w, Work::Done) && !p.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
