//! Backups of each person's home, as a Mac's Time Machine keeps them: every hour, kept
//! hourly for a day, daily for a month and weekly after that, so anything can go back to
//! how it was. restic does the work (each piece of a file once, however many backups have
//! it; only what changed is read again), into a repository of the person's own on the same
//! disk (/var/lib/namios-backups/<login>, theirs alone and root's), and also onto a drive
//! when one's chosen and plugged in.
//!
//! Settings are `namios: backups:` in the person's ~/user.nami (Settings → Backups writes
//! them): `on` (true by itself), `to` (`disk:<label>` or a folder: also there), and
//! `leave-out` (what isn't backed up; [`DEFAULT_LEAVE_OUT`] without it).
//!
//! The computer's timer runs `nami backup run` as root every hour: each person's folder is
//! made theirs, and their backup runs as them, at the lowest CPU and disk priority, not on a
//! battery under 20%, nor on a disk under 10% free (they're told). The rest is the person's
//! own, without an admin: `nami backup now|list|restore|status`, and the timeline (nami-gui
//! --go-back) reads through `nami backup ls|versions`.

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// What isn't backed up unless the person says otherwise: what's made again by itself
/// (caches, build output, downloaded packages and images), the Trash, and disk images of
/// virtual machines (large, and changing as a whole every time). Folders that say they're a
/// cache (a CACHEDIR.TAG, as Rust's target/ has) are left out too.
pub const DEFAULT_LEAVE_OUT: &[&str] = &[
    "~/.cache",
    "~/.local/share/Trash",
    // nami's and Nix's own: made again from user.nami.
    "~/.local/state/nami",
    "~/.local/state/nix",
    "~/.nix-profile",
    "~/.nix-defexpr",
    // Containers' and virtual machines' images.
    "~/.local/share/containers",
    "~/.local/share/gnome-boxes/images",
    "~/.local/share/libvirt/images",
    "~/VirtualBox VMs",
    "*.qcow2",
    "*.vdi",
    "*.vmdk",
    "*.vhdx",
    // Downloaded again by the tools: packages, toolchains, games.
    "node_modules",
    "~/.npm",
    "~/.cargo/registry",
    "~/.cargo/git",
    "~/.rustup/toolchains",
    "~/.local/share/Steam/steamapps",
    "~/.var/app/*/cache",
];

/// The backups' own folders on this computer: one each, `<login>`.
fn root_dir() -> PathBuf {
    std::env::var_os("NAMI_BACKUPS_DIR").map(PathBuf::from).unwrap_or_else(|| "/var/lib/namios-backups".into())
}

const HOUR: i64 = 3600;
const DAY: i64 = 86400;

// ---------- settings ----------

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// What the person said; none: on, if the first backup fits easily ([`fits_by_itself`]).
    pub on: Option<bool>,
    /// Also there: `disk:<label>`, or a folder.
    pub to: Option<String>,
    /// As the person wrote it (or the defaults).
    pub leave_out: Vec<String>,
    /// Whether that's the defaults (no `leave-out:` in the file).
    pub default_leave_out: bool,
}

/// `namios: backups:` from a user.nami's text.
pub fn settings_from(text: &str) -> Settings {
    let doc: serde_yaml::Value = serde_yaml::from_str(text).unwrap_or(serde_yaml::Value::Null);
    let b = &doc["namios"]["backups"];
    let listed = b["leave-out"].as_sequence().map(|s| s.iter().filter_map(|v| v.as_str().map(str::to_string)).collect::<Vec<_>>());
    Settings {
        on: b["on"].as_bool(),
        to: b["to"].as_str().map(str::trim).filter(|s| !s.is_empty() && *s != "this computer").map(str::to_string),
        default_leave_out: listed.is_none(),
        leave_out: listed.unwrap_or_else(|| DEFAULT_LEAVE_OUT.iter().map(|s| s.to_string()).collect()),
    }
}

fn settings(home: &Path) -> Settings {
    settings_from(&std::fs::read_to_string(crate::config::user_file_in(home)).unwrap_or_default())
}

/// restic's patterns for what's left out of `home`, and what was refused: a line that would
/// leave out the whole home (`~`, `/`, `*`) isn't a backup's.
pub fn exclude_patterns(home: &Path, leave_out: &[String]) -> (Vec<String>, Vec<String>) {
    let home_s = home.to_string_lossy().trim_end_matches('/').to_string();
    let (mut out, mut refused) = (Vec::new(), Vec::new());
    for line in leave_out {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let l = l.trim_end_matches('/');
        let p = if l == "~" {
            home_s.clone()
        } else if let Some(rest) = l.strip_prefix("~/") {
            format!("{home_s}/{}", rest.trim_start_matches('/'))
        } else {
            l.to_string()
        };
        // (Everything: the home itself, a folder it's in, or a pattern for anything.)
        let everything = p.is_empty() || p == "*" || p == "**" || p == home_s || (p.starts_with('/') && Path::new(&home_s).starts_with(&p)) || p.chars().all(|c| c == '*' || c == '/');
        if everything {
            refused.push(line.clone());
        } else if !out.contains(&p) {
            out.push(p);
        }
    }
    // A restore under way (its folder, for a moment), and restic's cache.
    out.push(".nami-restore-*".into());
    out.push(format!("{home_s}/.cache/restic"));
    (out, refused)
}

// ---------- which backups to keep ----------

/// Which of `snaps` (id, time in seconds) to keep at `now`, as a Mac does: every one of the
/// last 24 hours (hourly, and any asked for between: what was there for minutes is kept
/// too), each day's newest for the last 30 days, each week's (Monday to Sunday) before that. The newest is always kept. `offset` is the local time's offset
/// from UTC at a time, in seconds (days and hours are the person's own).
pub fn keep(snaps: &[(String, i64)], now: i64, offset: impl Fn(i64) -> i64) -> HashSet<String> {
    let mut sorted: Vec<&(String, i64)> = snaps.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    let mut seen = HashSet::new();
    let mut kept = HashSet::new();
    for (i, (id, t)) in sorted.into_iter().enumerate() {
        let local = t + offset(*t);
        let age = now - t;
        let bucket = if age < DAY {
            ('a', i as i64)
        } else if age < 30 * DAY {
            ('d', local.div_euclid(DAY))
        } else {
            // (1970-01-01 was a Thursday: Monday's the week's first day.)
            ('w', (local.div_euclid(DAY) + 3).div_euclid(7))
        };
        if seen.insert(bucket) || i == 0 {
            kept.insert(id.clone());
        }
    }
    kept
}

// ---------- time ----------

/// Days since 1970-01-01 of a date (Howard Hinnant's).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// (year, month, day) of days since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

/// Seconds since 1970 of restic's times: `2026-11-09T10:00:00.123456789+01:00` (or `Z`).
pub fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let rest = &s[19..];
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let off = match rest.chars().next() {
        Some('Z') | None => 0,
        Some(sign @ ('+' | '-')) => {
            let oh = rest.get(1..3)?.parse::<i64>().ok()?;
            let om = rest.get(4..6).and_then(|m| m.parse::<i64>().ok()).unwrap_or(0);
            (oh * 3600 + om * 60) * if sign == '-' { -1 } else { 1 }
        }
        _ => return None,
    };
    Some(days_from_civil(y, mo, d) * DAY + h * HOUR + mi * 60 + se - off)
}

/// This computer's offset from UTC at `t`, in seconds.
#[allow(deprecated)]
pub fn local_offset(t: i64) -> i64 {
    unsafe {
        let tt = t as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&tt, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff as i64
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// `Nov 9, 10:00`, `Today, 10:00`, `Yesterday, 18:30`; with `year` when it isn't this one.
pub fn when(t: i64, now: i64, offset: impl Fn(i64) -> i64) -> String {
    let local = t + offset(t);
    let today = (now + offset(now)).div_euclid(DAY);
    let day = local.div_euclid(DAY);
    let (y, m, d) = civil_from_days(day);
    let (ny, _, _) = civil_from_days(today);
    let clock = format!("{:02}:{:02}", local.rem_euclid(DAY) / HOUR, local.rem_euclid(HOUR) / 60);
    match today - day {
        0 => format!("Today, {clock}"),
        1 => format!("Yesterday, {clock}"),
        _ if y != ny => format!("{} {d}, {y}, {clock}", MONTHS[(m - 1) as usize]),
        _ => format!("{} {d}, {clock}", MONTHS[(m - 1) as usize]),
    }
}

/// `Nov 9, 10.00` (`Nov 9 2025, 10.00` another year): for a name, as it's still true tomorrow.
pub fn stamp(t: i64, offset: impl Fn(i64) -> i64) -> String {
    let local = t + offset(t);
    let (y, m, d) = civil_from_days(local.div_euclid(DAY));
    let (ny, _, _) = civil_from_days((now() + offset(now())).div_euclid(DAY));
    let year = if y != ny { format!(" {y}") } else { String::new() };
    format!("{} {d}{year}, {:02}.{:02}", MONTHS[(m - 1) as usize], local.rem_euclid(DAY) / HOUR, local.rem_euclid(HOUR) / 60)
}

/// `10 minutes ago`, `3 hours ago`, `yesterday`.
pub fn ago(t: i64, now: i64) -> String {
    let d = (now - t).max(0);
    match d {
        0..60 => "just now".into(),
        60..3600 => plural(d / 60, "minute"),
        3600..86400 => plural(d / 3600, "hour"),
        86400..172800 => "yesterday".into(),
        _ => plural(d / 86400, "day"),
    }
}

fn plural(n: i64, what: &str) -> String {
    format!("{n} {what}{} ago", if n == 1 { "" } else { "s" })
}

/// The backup `from` names among `snaps` (id, time), newest first or not: an id (or its
/// start), `latest`, a day (`2026-11-09`: its newest), a day and time (`2026-11-09 10:00`),
/// `yesterday`, or a while ago (`2h`, `3d`, `1w`, `30m`): the newest at or before then.
pub fn pick_backup(snaps: &[(String, i64)], from: &str, now: i64, offset: impl Fn(i64) -> i64) -> Result<String> {
    let from = from.trim();
    if snaps.is_empty() {
        bail!("there are no backups yet");
    }
    let newest_by = |at: i64| snaps.iter().filter(|(_, t)| *t <= at).max_by_key(|(_, t)| *t).map(|(id, _)| id.clone());
    if from.is_empty() || from == "latest" || from == "now" {
        return Ok(newest_by(i64::MAX).unwrap_or_default());
    }
    if from.len() >= 4 && from.chars().all(|c| c.is_ascii_hexdigit()) {
        let found: Vec<_> = snaps.iter().filter(|(id, _)| id.starts_with(from)).collect();
        if found.len() == 1 {
            return Ok(found[0].0.clone());
        }
    }
    let today = (now + offset(now)).div_euclid(DAY) * DAY;
    let clock = |s: &str| -> Option<i64> {
        let (h, m) = s.split_once(':')?;
        let (h, m) = (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?);
        (h < 24 && m < 60 && s.len() <= 5).then_some(h * HOUR + m * 60 + 59)
    };
    let at = if from == "yesterday" {
        today - 1 - offset(now)
    } else if from == "today" {
        now
    } else if let Some(c) = clock(from) {
        // A time today (19:30): the newest then.
        today + c - offset(now)
    } else if let Some(n) = from.strip_suffix(['m', 'h', 'd', 'w']).and_then(|n| n.parse::<i64>().ok()) {
        let unit = match from.chars().last() {
            Some('m') => 60,
            Some('h') => HOUR,
            Some('d') => DAY,
            _ => 7 * DAY,
        };
        now - n * unit
    } else {
        let num = |a: usize, b: usize| from.get(a..b).and_then(|s| s.parse::<i64>().ok());
        let (Some(y), Some(m), Some(d)) = (num(0, 4), num(5, 7), num(8, 10)) else {
            bail!("which backup is {from}? An id from `nami backup list`, a day (2026-11-09), a day and time (2026-11-09 10:00), a time today (10:00), or a while ago (2h, 3d)");
        };
        let day = days_from_civil(y, m, d) * DAY;
        let local = match (num(11, 13), num(14, 16)) {
            (Some(h), mi) => day + h * HOUR + mi.unwrap_or(0) * 60 + 59,
            _ => day + DAY - 1,
        };
        local - offset(local)
    };
    match newest_by(at) {
        Some(id) => Ok(id),
        None => {
            let first = snaps.iter().map(|(_, t)| *t).min().unwrap_or(0);
            bail!("there's no backup from then: the first is from {}", when(first, now, offset))
        }
    }
}

/// The name a restored copy takes next to the one that's there: `Report (Nov 9, 10.00).pdf`.
pub fn both_name(name: &str, label: &str, is_dir: bool) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if !is_dir && i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    format!("{stem} ({}){ext}", label.replace(':', "."))
}

// ---------- the person, their folders ----------

struct Me {
    login: String,
    home: PathBuf,
}

fn me() -> Result<Me> {
    let uid = unsafe { libc::getuid() };
    let (login, home) = passwd().into_iter().find(|p| p.1 == uid).map(|p| (p.0, p.3)).unwrap_or_default();
    let login = if login.is_empty() { std::env::var("USER").unwrap_or_default() } else { login };
    let home = std::env::var_os("HOME").map(PathBuf::from).filter(|h| !h.as_os_str().is_empty()).unwrap_or(home);
    anyhow::ensure!(!login.is_empty() && home.is_dir(), "who's this? (no login or home folder)");
    Ok(Me { login, home })
}

/// /etc/passwd: (login, uid, gid, home).
fn passwd() -> Vec<(String, u32, u32, PathBuf)> {
    std::fs::read_to_string("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            Some((f.first()?.to_string(), f.get(2)?.parse().ok()?, f.get(3)?.parse().ok()?, PathBuf::from(f.get(5)?)))
        })
        .collect()
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname").map(|h| h.trim().to_string()).ok().filter(|h| !h.is_empty()).unwrap_or_else(|| "this-computer".into())
}

/// Where backups are: a repository and its key, side by side.
#[derive(Clone)]
struct Place {
    dir: PathBuf,
}

impl Place {
    fn repo(&self) -> PathBuf {
        self.dir.join("repo")
    }
    fn key(&self) -> PathBuf {
        self.dir.join("key")
    }
}

/// The person's own, on this computer.
fn local(me: &Me) -> Place {
    Place { dir: root_dir().join(&me.login) }
}

/// The other place `to` says, if it's there now: a drive by its label (mounted, as Files
/// mounts one plugged in), or a folder. Err: why not.
fn other_place(me: &Me, to: &str) -> Result<Place, String> {
    let base = if let Some(label) = to.strip_prefix("disk:") {
        mount_of_label(label).ok_or_else(|| format!("the drive “{label}” isn't plugged in"))?
    } else {
        let p = PathBuf::from(to.replacen('~', &me.home.to_string_lossy(), 1));
        if !p.is_dir() {
            return Err(format!("{} isn't there", p.display()));
        }
        p
    };
    Ok(Place { dir: drive_folder(&base).join(format!("{}-{}", hostname(), me.login)) })
}

/// Where the drive labelled `label` is mounted.
fn mount_of_label(label: &str) -> Option<PathBuf> {
    // (udev writes a label's spaces and slashes as \x20, \x2f.)
    let escaped: String = label.chars().map(|c| if c == ' ' || c == '/' { format!("\\x{:02x}", c as u32) } else { c.to_string() }).collect();
    let dev = std::fs::canonicalize(format!("/dev/disk/by-label/{escaped}")).ok()?;
    let mounts = std::fs::read_to_string("/proc/self/mounts").ok()?;
    mounts.lines().find_map(|l| {
        let mut f = l.split(' ');
        let (d, at) = (f.next()?, f.next()?);
        (std::fs::canonicalize(d).ok()? == dev).then(|| PathBuf::from(at.replace("\\040", " ")))
    })
}

/// Free space of the filesystem at `p`, as a part of it (0 to 1), and its bytes free.
fn free(p: &Path) -> Option<(f64, u64)> {
    let c = CString::new(p.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 || s.f_blocks == 0 {
        return None;
    }
    Some((s.f_bavail as f64 / s.f_blocks as f64, s.f_bavail as u64 * s.f_frsize as u64))
}

/// On a battery that's under 20% and running down: its percent.
fn low_battery() -> Option<u8> {
    let dir = std::env::var_os("NAMI_POWER_SUPPLY").map(PathBuf::from).unwrap_or_else(|| "/sys/class/power_supply".into());
    let read = |p: PathBuf| std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default();
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if read(p.join("type")) == "Battery" && read(p.join("status")) == "Discharging" {
            if let Ok(c) = read(p.join("capacity")).parse::<u8>() {
                if c < 20 {
                    return Some(c);
                }
            }
        }
    }
    None
}

/// The total size of the files under `p`.
fn size_of(p: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(p) else { return 0 };
    if !m.is_dir() {
        return m.blocks() * 512;
    }
    std::fs::read_dir(p).map(|d| d.flatten().map(|e| size_of(&e.path())).sum()).unwrap_or(0)
}

// ---------- room on the disk ----------

const GB: u64 = 1_000_000_000;

/// The disk at `p`: bytes free (for anyone, not root's reserve), and its size.
fn disk(p: &Path) -> Option<(u64, u64)> {
    let c = CString::new(p.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 || s.f_blocks == 0 {
        return None;
    }
    Some((s.f_bavail as u64 * s.f_frsize as u64, s.f_blocks as u64 * s.f_frsize as u64))
}

/// The least a backup leaves free (`run`), and the least it goes on with (`stop`): 20 GB and
/// 10 GB, or a quarter and an eighth of a disk smaller than 80 GB (so a small one still backs
/// up a small home), or what NAMI_BACKUPS_FLOOR_GB says (tests).
fn floors(total: u64) -> (u64, u64) {
    match std::env::var("NAMI_BACKUPS_FLOOR_GB").ok().and_then(|f| f.parse::<f64>().ok()) {
        Some(f) => ((f * GB as f64) as u64, (f * GB as f64 / 2.) as u64),
        None => ((20 * GB).min(total / 4), (10 * GB).min(total / 8)),
    }
}

/// Whether a run that adds `estimate` leaves enough free: 15% of the disk, or `floor`,
/// whichever is more.
pub fn room_for(estimate: u64, free: u64, total: u64, floor: u64) -> bool {
    free.saturating_sub(estimate) >= (total * 15 / 100).max(floor)
}

/// Whether a run under way stops: under a tenth of the disk free, or under `floor`.
pub fn must_stop(free: u64, total: u64, floor: u64) -> bool {
    free < (total / 10).max(floor)
}

/// Backups on by themselves (nothing in user.nami): only when the first one takes under a
/// third of what's free. A new person's small home does; 290 GB on a disk with 355 GB free
/// doesn't.
pub fn fits_by_itself(estimate: u64, free: u64) -> bool {
    estimate.saturating_mul(3) < free
}

/// restic's patterns (`*`, `?`, `\` escaping) against one name.
fn wild(p: &[char], s: &[char]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some('*') => (0..=s.len()).any(|i| wild(&p[1..], &s[i..])),
        Some('?') => !s.is_empty() && wild(&p[1..], &s[1..]),
        Some('\\') if p.len() > 1 => s.first() == Some(&p[1]) && wild(&p[2..], &s[1..]),
        Some(c) => s.first() == Some(c) && wild(&p[1..], &s[1..]),
    }
}

/// What's left out, as restic reads [`exclude_patterns`]: an absolute pattern from the start
/// of a path (a folder and all in it), any other at any depth.
pub struct LeaveOut {
    from_root: Vec<Vec<Vec<char>>>,
    anywhere: Vec<Vec<Vec<char>>>,
}

impl LeaveOut {
    pub fn new(patterns: &[String]) -> LeaveOut {
        let parts = |p: &str| p.split('/').filter(|c| !c.is_empty()).map(|c| c.chars().collect()).collect::<Vec<Vec<char>>>();
        LeaveOut {
            from_root: patterns.iter().filter(|p| p.starts_with('/')).map(|p| parts(p)).collect(),
            anywhere: patterns.iter().filter(|p| !p.starts_with('/')).map(|p| parts(p)).filter(|p| !p.is_empty()).collect(),
        }
    }

    pub fn out(&self, path: &Path) -> bool {
        let names: Vec<Vec<char>> = path.components().filter_map(|c| match c {
            std::path::Component::Normal(n) => Some(n.to_string_lossy().chars().collect()),
            _ => None,
        }).collect();
        let at = |pat: &[Vec<char>], from: usize| pat.len() <= names.len() - from && pat.iter().zip(&names[from..]).all(|(p, n)| wild(p, n));
        self.from_root.iter().any(|p| !p.is_empty() && at(p, 0)) || self.anywhere.iter().any(|p| (0..names.len()).any(|i| at(p, i)))
    }
}

/// What a backup of `home` reads, roughly: the files that aren't left out (nor in a folder
/// that says it's a cache, nor on another disk), or with `since`, only those changed since
/// then. Before compressing and finding what's there already: more than it adds, not less.
pub fn estimate(home: &Path, leave: &LeaveOut, since: Option<i64>) -> u64 {
    let Ok(top) = std::fs::symlink_metadata(home) else { return 0 };
    let mut todo = vec![home.to_path_buf()];
    let mut sum = 0u64;
    while let Some(dir) = todo.pop() {
        if dir.join("CACHEDIR.TAG").is_file() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
            if leave.out(&p) {
                continue;
            }
            if m.is_dir() {
                if m.dev() == top.dev() {
                    todo.push(p);
                }
            } else if m.is_file() && since.is_none_or(|t| m.mtime() >= t || m.ctime() >= t) {
                sum += m.len();
            }
        }
    }
    sum
}

// ---------- restic ----------

fn restic_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("NAMI_RESTIC") {
        return p.into();
    }
    for dir in std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default().into_iter().chain([PathBuf::from("/run/current-system/sw/bin")]) {
        if dir.join("restic").is_file() {
            return dir.join("restic");
        }
    }
    "restic".into()
}

fn restic(place: &Place, home: &Path) -> Command {
    let mut c = Command::new(restic_bin());
    c.env("RESTIC_REPOSITORY", place.repo())
        .env("RESTIC_PASSWORD_FILE", place.key())
        .env("RESTIC_CACHE_DIR", home.join(".cache/restic"))
        // (Two cores at most: a backup is never what makes the fans go.)
        .env("GOMAXPROCS", "2")
        .env_remove("RESTIC_PASSWORD")
        .env_remove("RESTIC_PASSWORD_COMMAND");
    c
}

fn output(mut c: Command) -> Result<Vec<u8>> {
    let out = c.stdin(Stdio::null()).output().with_context(|| format!("can't run {} (namios-backups has it)", restic_bin().display()))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("{}", err.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("restic didn't say why").trim());
    }
    Ok(out.stdout)
}

/// A key and a repository there, made if they aren't.
fn ready(place: &Place, home: &Path) -> Result<()> {
    std::fs::create_dir_all(&place.dir).with_context(|| format!("can't make {}", place.dir.display()))?;
    if !place.key().exists() {
        let mut bytes = [0u8; 32];
        std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut bytes))?;
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let tmp = place.dir.join(".key.new");
        std::fs::write(&tmp, format!("{hex}\n"))?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        std::fs::rename(&tmp, place.key())?;
    }
    if !place.repo().join("config").exists() {
        let mut c = restic(place, home);
        c.args(["init", "--quiet"]);
        output(c).context("the backups couldn't be started")?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct Snap {
    id: String,
    time: String,
}

/// The backups there, as (id, time), oldest first.
fn snapshots(place: &Place, home: &Path) -> Result<Vec<(String, i64)>> {
    if !place.repo().join("config").exists() {
        return Ok(Vec::new());
    }
    let mut c = restic(place, home);
    c.args(["snapshots", "--json", "--no-lock"]);
    let snaps: Vec<Snap> = serde_json::from_slice(&output(c)?).context("restic's list of backups")?;
    let mut v: Vec<(String, i64)> = snaps.into_iter().filter_map(|s| Some((s.id, parse_time(&s.time)?))).collect();
    v.sort_by_key(|s| s.1);
    Ok(v)
}

/// restic's patterns escape these.
fn escape_pattern(p: &str) -> String {
    p.chars().flat_map(|c| if matches!(c, '*' | '?' | '[' | ']' | '\\') { vec!['\\', c] } else { vec![c] }).collect()
}

// ---------- state ----------

/// What the last backups said, for Settings and the notices: state.json, next to them.
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct State {
    /// The last backup that finished, and the last try (seconds).
    pub done: Option<i64>,
    pub tried: Option<i64>,
    /// Why the last try didn't back up ("" when it did): plain words.
    #[serde(default)]
    pub said: String,
    /// `disk-full`, `battery`, `failed`, `off` (none: fine).
    pub problem: Option<String>,
    /// The backups' size on the disk, and the home's as backed up (bytes).
    pub bytes: Option<u64>,
    pub home_bytes: Option<u64>,
    pub count: Option<usize>,
    /// The last backup's: what was new, how long it took (seconds).
    pub added: Option<u64>,
    pub took: Option<f64>,
    pub pruned: Option<i64>,
    /// The last notice: `<day> <problem>`, so it's once a day.
    #[serde(default)]
    pub told: String,
    /// The other place's.
    pub other: Option<Box<State>>,
    /// What the first backup would take, and when that was found out (seconds); the free
    /// space then.
    pub estimate: Option<u64>,
    pub estimate_at: Option<i64>,
    pub free_then: Option<u64>,
    /// Told once that they're not on by themselves (never again).
    #[serde(default)]
    pub told_too_big: bool,
}

fn read_state(place: &Place) -> State {
    std::fs::read(place.dir.join("state.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_state(place: &Place, s: &State) {
    let tmp = place.dir.join(".state.json.new");
    if std::fs::write(&tmp, serde_json::to_vec_pretty(s).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, place.dir.join("state.json"));
    }
}

/// A notification, if the person's at their desktop: once a day for each kind.
fn tell(state: &mut State, problem: &str, title: &str, body: &str) {
    let day = now().div_euclid(DAY);
    let key = format!("{day} {problem}");
    if state.told == key {
        return;
    }
    state.told = key;
    let uid = unsafe { libc::getuid() };
    let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().or_else(|| Path::new(&format!("/run/user/{uid}/bus")).exists().then(|| format!("unix:path=/run/user/{uid}/bus")));
    if let Some(bus) = bus {
        let _ = Command::new("notify-send")
            .env("DBUS_SESSION_BUS_ADDRESS", bus)
            .args(["--app-name=Backups", "--icon=document-revert", title, body])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// One backup at a time, for each person (a run waits for, or leaves be, one under way).
struct Lock(#[allow(dead_code)] std::fs::File);

fn lock(place: &Place, wait: bool) -> Result<Option<Lock>> {
    std::fs::create_dir_all(&place.dir)?;
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(place.dir.join("lock"))?;
    use std::os::fd::AsRawFd;
    let flags = libc::LOCK_EX | if wait { 0 } else { libc::LOCK_NB };
    if unsafe { libc::flock(f.as_raw_fd(), flags) } != 0 {
        return Ok(None);
    }
    Ok(Some(Lock(f)))
}

fn running(place: &Place) -> bool {
    let Ok(f) = std::fs::OpenOptions::new().write(true).open(place.dir.join("lock")) else { return false };
    use std::os::fd::AsRawFd;
    let busy = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } != 0;
    busy
}

/// As little as can be: the lowest CPU priority, and the disk only when nothing else wants it
/// (asked for now: a little more, as someone's waiting).
fn be_quiet(now: bool) {
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, if now { 10 } else { 19 });
        // ioprio_set(IOPRIO_WHO_PROCESS, this one, idle; or best-effort's lowest)
        #[cfg(target_os = "linux")]
        {
            let prio = if now { (2 << 13) | 7 } else { 3 << 13 };
            libc::syscall(libc::SYS_ioprio_set, 1, 0, prio);
        }
    }
}

// ---------- backing up ----------

#[derive(Deserialize, Default)]
struct Summary {
    #[serde(default)]
    message_type: String,
    #[serde(default)]
    data_added: u64,
    #[serde(default)]
    total_bytes_processed: u64,
    #[serde(default)]
    total_duration: f64,
}

/// A backup of the home into `place`, then the old ones thinned out. The new state.
fn back_up_into(me: &Me, place: &Place, set: &Settings, mut state: State, loud: bool) -> State {
    let t = now();
    state.tried = Some(t);
    let say = |mut state: State, problem: Option<&str>, said: String| {
        state.problem = problem.map(str::to_string);
        state.said = said;
        state
    };
    let (patterns, _) = exclude_patterns(&me.home, &set.leave_out);
    let mut patterns = patterns;
    // (The other place, when it's a folder in the home.)
    if place.dir.starts_with(&me.home) {
        patterns.push(place.dir.to_string_lossy().into_owned());
    }
    // Room for it: what it'll add (all of it the first time; since the last one after, or at
    // most the home as it was), leaving 15% of the disk or 20 GB free.
    let first = !place.repo().join("config").exists() || state.done.is_none();
    let (run_floor, stop_floor) = floors(disk(&place.dir).map_or(u64::MAX, |(_, total)| total));
    if let Some((fr, total)) = disk(&place.dir) {
        let leave = LeaveOut::new(&patterns);
        let need = if first {
            fresh_estimate(&mut state, &me.home, &leave)
        } else {
            match state.home_bytes {
                Some(upper) if room_for(upper, fr, total, run_floor) => upper,
                _ => estimate(&me.home, &leave, state.done.map(|d| d - 120)),
            }
        };
        if !room_for(need, fr, total, run_floor) {
            let said = format!(
                "Backups are paused: this one needs about {}, and the disk would have less than {} free (it has {}).",
                bytes(need),
                bytes((total * 15 / 100).max(run_floor)),
                bytes(fr)
            );
            let mut state = say(state, Some("no-room"), said.clone());
            tell(&mut state, "no-room", "Backups are paused: the disk is nearly full", &said);
            return state;
        }
    }
    if let Err(e) = ready(place, &me.home) {
        let mut state = say(state, Some("failed"), format!("The backups couldn't be started: {e:#}."));
        let said = state.said.clone();
        tell(&mut state, "failed", "Your files weren't backed up", &said);
        return state;
    }
    let ex = place.dir.join("leave-out");
    let _ = std::fs::write(&ex, patterns.join("\n") + "\n");
    let mut c = restic(place, &me.home);
    c.args(["backup", "--json", "--quiet", "--no-scan", "--one-file-system", "--exclude-caches", "--tag", "namios", "--host", &hostname()]);
    c.arg("--exclude-file").arg(&ex);
    if skip_unchanged() {
        c.arg("--skip-if-unchanged");
    }
    c.arg(&me.home);
    if loud {
        eprintln!("backing up {} into {}", me.home.display(), place.repo().display());
    }
    // Watched as it goes: stopped (cleanly: restic keeps the backups as they were) when the
    // disk gets down to a tenth free, or 10 GB.
    let (out_file, err_file) = (place.dir.join(".backup.out"), place.dir.join(".backup.err"));
    let spawned = (|| -> std::io::Result<std::process::Child> { c.stdin(Stdio::null()).stdout(std::fs::File::create(&out_file)?).stderr(std::fs::File::create(&err_file)?).spawn() })();
    let mut child = match spawned {
        Ok(ch) => ch,
        Err(e) => return say(state, Some("failed"), format!("restic couldn't start: {e}.")),
    };
    let mut stopped = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {}
            Err(_) => break None,
        }
        if stopped.is_none() {
            if let Some((fr, _)) = disk(&place.dir).filter(|(fr, total)| must_stop(*fr, *total, stop_floor)) {
                stopped = Some(fr);
                unsafe { libc::kill(child.id() as i32, libc::SIGINT) };
                // (A minute to finish; then for good.)
                let t = now();
                while now() - t < 60 && matches!(child.try_wait(), Ok(None)) {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                let _ = child.kill();
                break child.wait().ok();
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    };
    let out = std::process::Output {
        status: status.unwrap_or_else(|| std::os::unix::process::ExitStatusExt::from_raw(1 << 8)),
        stdout: std::fs::read(&out_file).unwrap_or_default(),
        stderr: std::fs::read(&err_file).unwrap_or_default(),
    };
    let _ = std::fs::remove_file(&out_file);
    let _ = std::fs::remove_file(&err_file);
    if let Some(fr) = stopped {
        // What it had written, which no backup has, goes (nothing else is touched).
        let mut c = restic(place, &me.home);
        c.args(["unlock", "--quiet"]);
        let _ = output(c);
        let mut c = restic(place, &me.home);
        c.args(["prune", "--quiet", "--max-unused", "unlimited"]);
        let _ = output(c);
        state.bytes = Some(size_of(&place.repo()));
        let said = format!("The backup stopped: the disk got nearly full ({} free). The backups from before are all there.", bytes(fr));
        let mut state = say(state, Some("disk-full"), said.clone());
        tell(&mut state, "disk-full", "A backup stopped: the disk is nearly full", "Free up some space (Settings → General → Storage), or back up to a drive instead (Settings → Backups).");
        return state;
    }
    // 3: some files couldn't be read (the rest is backed up).
    let code = out.status.code().unwrap_or(-1);
    if code != 0 && code != 3 {
        let err = String::from_utf8_lossy(&out.stderr);
        let why = err.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("").trim().to_string();
        let mut state = say(state, Some("failed"), format!("The last backup didn't finish ({why})."));
        let said = state.said.clone();
        tell(&mut state, "failed", "Your files weren't backed up", &said);
        return state;
    }
    let summary = String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| serde_json::from_str::<Summary>(l).ok()).find(|s| s.message_type == "summary").unwrap_or_default();
    state.added = Some(summary.data_added);
    state.took = Some(summary.total_duration);
    state.home_bytes = Some(summary.total_bytes_processed);
    state.done = Some(now());
    let mut said = String::new();
    if code == 3 {
        said = "Some files couldn't be read (they're another person's, or open and locked); the rest is backed up.".into();
        if loud {
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
        }
    }
    // The old ones thinned: hourly for a day, daily for a month, weekly after.
    match snapshots(place, &me.home) {
        Ok(snaps) => {
            let kept = keep(&snaps, now(), local_offset);
            let gone: Vec<&String> = snaps.iter().map(|s| &s.0).filter(|id| !kept.contains(*id)).collect();
            let mut forgot = false;
            for chunk in gone.chunks(100) {
                let mut c = restic(place, &me.home);
                c.args(["forget", "--quiet"]).args(chunk.iter().map(|s| s.as_str()));
                forgot |= output(c).is_ok();
            }
            state.count = Some(snaps.len() - gone.len());
            // What they took freed: once a day (sooner when the disk's getting full).
            let tight = free(&place.dir).is_some_and(|(p, _)| p < 0.15);
            if forgot && (tight || state.pruned.is_none_or(|p| now() - p > DAY)) {
                let mut c = restic(place, &me.home);
                c.args(["prune", "--quiet"]);
                if output(c).is_ok() {
                    state.pruned = Some(now());
                }
            }
        }
        Err(e) => said = format!("The old backups couldn't be thinned out ({e:#})."),
    }
    state.bytes = Some(size_of(&place.repo()));
    if loud {
        eprintln!("backed up: {} new, {} in all", bytes(summary.data_added), bytes(state.bytes.unwrap_or(0)));
    }
    say(state, None, said)
}

/// restic 0.17 and later: no new backup when nothing changed.
fn skip_unchanged() -> bool {
    let out = Command::new(restic_bin()).arg("version").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let v: Vec<u32> = out.split_whitespace().nth(1).unwrap_or("").split('.').filter_map(|n| n.parse().ok()).collect();
    v.len() >= 2 && (v[0], v[1]) >= (0, 17)
}

pub fn bytes(n: u64) -> String {
    match n {
        0..1000 => format!("{n} bytes"),
        1000..1_000_000 => format!("{} KB", n.div_ceil(1000)),
        1_000_000..1_000_000_000 => format!("{} MB", n / 1_000_000),
        _ => format!("{:.1} GB", n as f64 / 1e9),
    }
}

/// The first backup's size, found out again if it's a day old.
fn fresh_estimate(state: &mut State, home: &Path, leave: &LeaveOut) -> u64 {
    match (state.estimate, state.estimate_at) {
        (Some(e), Some(at)) if now() - at < DAY => e,
        _ => {
            let e = estimate(home, leave, None);
            state.estimate = Some(e);
            state.estimate_at = Some(now());
            e
        }
    }
}

/// Whether this computer's own backups run: as the person said; with nothing said, when they
/// have before, or the first fits easily. Not: why, in `state` (once, a notice).
fn on_here(me: &Me, place: &Place, set: &Settings, state: &mut State) -> bool {
    match set.on {
        Some(false) => {
            state.problem = Some("off".into());
            state.said = "Backups are off.".into();
            false
        }
        Some(true) => true,
        None if state.done.is_some() => true,
        None => {
            let (patterns, _) = exclude_patterns(&me.home, &set.leave_out);
            let need = fresh_estimate(state, &me.home, &LeaveOut::new(&patterns));
            let free = disk(&place.dir).map_or(0, |d| d.0);
            state.free_then = Some(free);
            if fits_by_itself(need, free) {
                if state.problem.as_deref() == Some("too-big") {
                    state.problem = None;
                    state.said.clear();
                }
                return true;
            }
            state.problem = Some("too-big".into());
            state.said = format!("Not on yet: your files need about {} here, and the disk has {} free.", bytes(need), bytes(free));
            if !state.told_too_big {
                state.told_too_big = true;
                let said = state.said.clone();
                // (Never again: told is told.)
                state.told.clear();
                tell(state, "too-big", "Your files aren't backed up yet", &format!("{said} Back up to a drive instead, or turn it on anyway: Settings → Backups."));
            }
            false
        }
    }
}

/// The person's backup: due (an hour since the last), or now. Exit code.
fn run_mine(asked: bool) -> Result<()> {
    let me = me()?;
    let place = local(&me);
    if !place.dir.is_dir() {
        bail!("there's no folder for your backups yet ({}): the computer makes it within the hour, or with `sudo systemctl start namios-backups`", place.dir.display());
    }
    let set = settings(&me.home);
    let mut state = read_state(&place);
    // Here (asked for now: here, unless they're off), and the other place, when there's one.
    let here = if asked { set.on != Some(false) } else { on_here(&me, &place, &set, &mut state) };
    let there = set.to.is_some() && set.on != Some(false);
    if !here && !there {
        write_state(&place, &state);
        if asked {
            bail!("{}", state.said);
        }
        return Ok(());
    }
    let last = if here { state.done } else { state.other.as_ref().and_then(|o| o.done) };
    if !asked && last.is_some_and(|d| now() - d < HOUR - 300) {
        write_state(&place, &state);
        return Ok(());
    }
    if !asked {
        if let Some(c) = low_battery() {
            state.problem = Some("battery".into());
            state.said = format!("Waiting for power: the battery's at {c}%.");
            write_state(&place, &state);
            return Ok(());
        }
    }
    be_quiet(asked);
    let held = match lock(&place, false)? {
        Some(l) => Some(l),
        None if asked => {
            eprintln!("(a backup is under way: waiting for it, then backing up)");
            lock(&place, true)?
        }
        None => None,
    };
    let Some(_held) = held else {
        return Ok(());
    };
    let mut state = if here { back_up_into(&me, &place, &set, state, asked) } else { state };
    // The other place too, when it's there.
    if let Some(to) = &set.to {
        let other = state.other.take().map(|b| *b).unwrap_or_default();
        let other = match other_place(&me, to) {
            Ok(p) => {
                let s = back_up_into(&me, &p, &set, other, asked);
                // (A copy of what it said, next to those backups, for a computer made again.)
                write_state(&p, &s);
                s
            }
            Err(why) => State { said: format!("Not on {to}: {why}."), problem: Some("away".into()), ..other },
        };
        state.other = Some(Box::new(other));
    }
    write_state(&place, &state);
    if let Some(p) = &state.problem {
        if asked {
            bail!("{} ({p})", state.said);
        }
    }
    if asked {
        println!("Backed up{}.", state.added.map(|a| format!(": {} new", bytes(a))).unwrap_or_default());
    }
    Ok(())
}

/// As root, from the timer: each person's folder made theirs, and their backup, as them.
fn run_everyone() -> Result<()> {
    let root = root_dir();
    std::fs::create_dir_all(&root)?;
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o711))?;
    let me = std::env::current_exe()?;
    for (login, uid, gid, home) in passwd() {
        if !(1000..60000).contains(&uid) || !home.starts_with("/home") || !home.is_dir() {
            continue;
        }
        let dir = root.join(&login);
        std::fs::create_dir_all(&dir)?;
        let c = CString::new(dir.as_os_str().as_bytes())?;
        unsafe { libc::chown(c.as_ptr(), uid, gid) };
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        let mut cmd = Command::new(&me);
        cmd.args(["backup", "run"]).uid(uid).gid(gid).env_clear().current_dir(&home);
        cmd.env("HOME", &home).env("USER", &login).env("LOGNAME", &login).env("PATH", std::env::var_os("PATH").unwrap_or_else(|| "/run/current-system/sw/bin".into()));
        // (Its own settings, as the service has them: NAMI_BACKUPS_DIR, _FLOOR_GB.)
        for (k, v) in std::env::vars_os().filter(|(k, _)| k.to_string_lossy().starts_with("NAMI_BACKUPS_")) {
            cmd.env(k, v);
        }
        if let Some(tz) = std::env::var_os("TZ") {
            cmd.env("TZ", tz);
        }
        let run = format!("/run/user/{uid}");
        if Path::new(&run).join("bus").exists() {
            cmd.env("XDG_RUNTIME_DIR", &run).env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={run}/bus"));
        }
        match cmd.status() {
            Ok(s) if s.success() => {}
            Ok(s) => eprintln!("{login}'s backup: {s}"),
            Err(e) => eprintln!("{login}'s backup couldn't start: {e}"),
        }
    }
    Ok(())
}

/// The backups' folder on a drive: namios-backups, or trayos-backups on one
/// tray backed up to (left as it is: another computer may still have tray).
fn drive_folder(base: &Path) -> PathBuf {
    let (new, old) = (base.join("namios-backups"), base.join("trayos-backups"));
    if !new.exists() && old.is_dir() { old } else { new }
}

// ---------- looking back ----------

/// Where to look: the person's own backups, or a drive's (`--place`: the folder there).
fn place_for(me: &Me, place: &Option<PathBuf>) -> Place {
    match place {
        Some(p) if p.join("repo").exists() => Place { dir: p.clone() },
        Some(p) => Place { dir: drive_folder(p).join(format!("{}-{}", hostname(), me.login)) },
        None => local(me),
    }
}

#[derive(Serialize)]
struct Listed {
    id: String,
    time: i64,
    when: String,
}

#[derive(Deserialize)]
struct Node {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    mtime: String,
    #[serde(default)]
    struct_type: String,
}

#[derive(Serialize, Clone)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub mtime: i64,
}

/// What was in `dir` in backup `id`: its own files and folders.
fn ls(me: &Me, place: &Place, id: &str, dir: &Path) -> Result<Vec<Entry>> {
    let mut c = restic(place, &me.home);
    c.args(["ls", "--json", "--no-lock", id]).arg(dir);
    let out = output(c)?;
    let want = dir.to_string_lossy().trim_end_matches('/').to_string();
    let mut v = Vec::new();
    for l in String::from_utf8_lossy(&out).lines() {
        let Ok(n) = serde_json::from_str::<Node>(l) else { continue };
        if n.struct_type == "snapshot" || n.path.trim_end_matches('/') == want || Path::new(&n.path).parent().map(|p| p.to_string_lossy().into_owned()) != Some(want.clone()) {
            continue;
        }
        v.push(Entry { name: n.name, dir: n.kind == "dir", size: n.size, mtime: parse_time(&n.mtime).unwrap_or(0) });
    }
    v.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(v)
}

/// The node of `path` in backup `id`, if it's there.
fn node(me: &Me, place: &Place, id: &str, path: &Path) -> Option<Entry> {
    let parent = path.parent()?;
    let name = path.file_name()?.to_string_lossy();
    ls(me, place, id, parent).ok()?.into_iter().find(|e| e.name == name)
}

#[derive(Serialize)]
struct Version {
    id: String,
    time: i64,
    when: String,
    size: u64,
    mtime: i64,
    /// The same as the file there now.
    now: bool,
}

#[derive(Deserialize)]
struct Found {
    snapshot: String,
    #[serde(default)]
    matches: Vec<Node>,
}

/// Each different version of `file` in the backups (by when it was changed, and its size),
/// newest first: the newest backup that has each.
fn versions(me: &Me, place: &Place, file: &Path) -> Result<Vec<Version>> {
    let snaps: HashMap<String, i64> = snapshots(place, &me.home)?.into_iter().collect();
    if snaps.is_empty() {
        return Ok(Vec::new());
    }
    let mut c = restic(place, &me.home);
    c.args(["find", "--json", "--no-lock"]).arg(escape_pattern(&file.to_string_lossy()));
    let found: Vec<Found> = serde_json::from_slice(&output(c)?).unwrap_or_default();
    let here = std::fs::metadata(file).ok().map(|m| (m.mtime(), m.len()));
    let want = file.to_string_lossy();
    let mut all: Vec<(i64, String, Entry)> = Vec::new();
    for f in found {
        let id = snaps.keys().find(|k| k.starts_with(&f.snapshot) || f.snapshot.starts_with(k.as_str())).cloned();
        let Some(id) = id else { continue };
        for n in f.matches.into_iter().filter(|n| n.path == want && n.kind != "dir") {
            all.push((snaps[&id], id.clone(), Entry { name: n.name, dir: false, size: n.size, mtime: parse_time(&n.mtime).unwrap_or(0) }));
        }
    }
    all.sort_by(|a, b| b.0.cmp(&a.0));
    let mut seen = HashSet::new();
    let t = now();
    Ok(all
        .into_iter()
        .filter(|(_, _, e)| seen.insert((e.mtime, e.size)))
        .map(|(time, id, e)| Version { when: when(time, t, local_offset), now: here == Some((e.mtime, e.size)), id, time, size: e.size, mtime: e.mtime })
        .collect())
}

/// What's there now is in the way: move it to the Trash (as Files would), not away for good.
fn to_trash(home: &Path, p: &Path) -> Result<()> {
    let trash = home.join(".local/share/Trash");
    let (files, info) = (trash.join("files"), trash.join("info"));
    std::fs::create_dir_all(&files)?;
    std::fs::create_dir_all(&info)?;
    let same_disk = std::fs::metadata(&files)?.dev() == std::fs::symlink_metadata(p)?.dev();
    anyhow::ensure!(same_disk, "{} is on another disk than the Trash: Keep both instead", p.display());
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut pick = name.clone();
    let mut n = 2;
    while files.join(&pick).exists() || info.join(format!("{pick}.trashinfo")).exists() {
        pick = format!("{name}.{n}");
        n += 1;
    }
    let t = now();
    let local = t + local_offset(t);
    let (y, m, d) = civil_from_days(local.div_euclid(DAY));
    let s = local.rem_euclid(DAY);
    let encoded: String = p.to_string_lossy().bytes().map(|b| if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    let mut f = std::fs::File::create(info.join(format!("{pick}.trashinfo")))?;
    writeln!(f, "[Trash Info]\nPath={encoded}\nDeletionDate={y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", s / HOUR, s % HOUR / 60, s % 60)?;
    std::fs::rename(p, files.join(&pick))?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum IfThere {
    KeepBoth,
    Replace,
}

/// Bring `path` back from backup `id` (into `into`, or where it was). Where it went.
fn restore_one(me: &Me, place: &Place, id: &str, time: i64, path: &Path, into: Option<&Path>, if_there: IfThere) -> Result<PathBuf> {
    let node = node(me, place, id, path).with_context(|| format!("{} isn't in the backup from {}", path.display(), when(time, now(), local_offset)))?;
    let name = path.file_name().context("which file?")?.to_os_string();
    let parent = into.map(Path::to_path_buf).unwrap_or_else(|| path.parent().unwrap_or(Path::new("/")).to_path_buf());
    std::fs::create_dir_all(&parent).with_context(|| format!("can't make {}", parent.display()))?;
    // Into a folder of its own next to where it goes (the same disk: it moves in whole).
    let tmp = parent.join(format!(".nami-restore-{}-{}", std::process::id(), now()));
    let _ = std::fs::remove_dir_all(&tmp);
    let mut c = restic(place, &me.home);
    c.args(["restore", "--quiet", "--verify", id, "--target"]).arg(&tmp).arg("--include").arg(escape_pattern(&path.to_string_lossy()));
    let done = output(c);
    let got = tmp.join(path.strip_prefix("/").unwrap_or(path));
    let result = (|| {
        done.context("restic couldn't bring it back")?;
        let m = std::fs::symlink_metadata(&got).with_context(|| format!("{} didn't come back from the backup", path.display()))?;
        // (Checked: a file is as long as the backup says.)
        anyhow::ensure!(node.dir || !m.is_file() || m.len() == node.size, "{} came back {} bytes long, not {}", path.display(), m.len(), node.size);
        let mut dest = parent.join(&name);
        if std::fs::symlink_metadata(&dest).is_ok() {
            match if_there {
                IfThere::Replace => to_trash(&me.home, &dest)?,
                IfThere::KeepBoth => {
                    let label = stamp(time, local_offset);
                    let base = both_name(&name.to_string_lossy(), &label, node.dir);
                    dest = parent.join(&base);
                    let mut n = 2;
                    while std::fs::symlink_metadata(&dest).is_ok() {
                        dest = parent.join(both_name(&name.to_string_lossy(), &format!("{label} {n}"), node.dir));
                        n += 1;
                    }
                }
            }
        }
        std::fs::rename(&got, &dest).with_context(|| format!("can't put it at {}", dest.display()))?;
        Ok(dest)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

// ---------- the command ----------

#[derive(clap::Subcommand)]
pub enum BackupCommand {
    /// Back up your home now (waiting for one under way)
    Now,
    /// What the computer's timer runs every hour: a backup, if one's due (as root: everyone's)
    #[command(hide = true)]
    Run,
    /// The backups to go back to, newest first
    List {
        #[arg(long)]
        json: bool,
        /// A drive's backups: the folder they're in (default: this computer's)
        #[arg(long)]
        place: Option<PathBuf>,
    },
    /// Bring a file or folder back as it was (default: the newest backup that has it)
    ///
    /// --from: an id from `nami backup list`, a day (2026-11-09: its last backup), a day
    /// and time (2026-11-09 10:00), `yesterday`, or a while ago (2h, 3d, 1w). If there's
    /// a file there now, both are kept (the one brought back is named for its time) unless
    /// --replace (the one there now goes to the Trash).
    Restore {
        paths: Vec<PathBuf>,
        #[arg(long)]
        from: Option<String>,
        /// Into this folder instead of where it was
        #[arg(long)]
        to: Option<PathBuf>,
        #[arg(long)]
        replace: bool,
        #[arg(long)]
        place: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// How your backups are doing: the last, where to, the space they use, what's left out
    Status {
        #[arg(long)]
        json: bool,
    },
    /// What was in a folder in a backup (for the timeline)
    #[command(hide = true)]
    Ls {
        dir: PathBuf,
        #[arg(long)]
        from: String,
        #[arg(long)]
        place: Option<PathBuf>,
    },
    /// A file's earlier versions (for Files' Restore an Earlier Version)
    Versions {
        file: PathBuf,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        place: Option<PathBuf>,
    },
}

fn absolute(p: &Path) -> PathBuf {
    let p = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(p) };
    // (Without following links: what was there may not be.)
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

pub fn cli(command: BackupCommand) -> Result<()> {
    match command {
        BackupCommand::Run if unsafe { libc::getuid() } == 0 => run_everyone(),
        BackupCommand::Run => run_mine(false),
        BackupCommand::Now => run_mine(true),
        BackupCommand::List { json, place } => {
            let me = me()?;
            let place = place_for(&me, &place);
            let t = now();
            let mut snaps = snapshots(&place, &me.home)?;
            snaps.reverse();
            let listed: Vec<Listed> = snaps.into_iter().map(|(id, time)| Listed { when: when(time, t, local_offset), id, time }).collect();
            if json {
                println!("{}", serde_json::to_string(&listed)?);
            } else if listed.is_empty() {
                println!("No backups yet.");
            } else {
                // Its id dim (what `nami backup restore` takes), when in words.
                let paint = crate::style::out();
                for l in &listed {
                    println!("{}  {:<22} {}", paint.dim(&l.id[..8.min(l.id.len())]), l.when, paint.dim(&ago(l.time, t)));
                }
            }
            Ok(())
        }
        BackupCommand::Ls { dir, from, place } => {
            let me = me()?;
            let place = place_for(&me, &place);
            println!("{}", serde_json::to_string(&ls(&me, &place, &from, &absolute(&dir))?)?);
            Ok(())
        }
        BackupCommand::Versions { file, json, place } => {
            let me = me()?;
            let place = place_for(&me, &place);
            let v = versions(&me, &place, &absolute(&file))?;
            if json {
                println!("{}", serde_json::to_string(&v)?);
            } else if v.is_empty() {
                println!("No earlier versions of {} in the backups.", file.display());
            } else {
                let paint = crate::style::out();
                for v in &v {
                    println!("{}  {:<22} {}{}", paint.dim(&v.id[..8.min(v.id.len())]), v.when, paint.dim(&format!("{:>9}", bytes(v.size))), if v.now { paint.added("  ✓ as it is now") } else { String::new() });
                }
            }
            Ok(())
        }
        BackupCommand::Restore { paths, from, to, replace, place, json } => {
            anyhow::ensure!(!paths.is_empty(), "what to bring back: `nami backup restore ~/Documents/report.pdf`");
            let me = me()?;
            let place = place_for(&me, &place);
            let snaps = snapshots(&place, &me.home)?;
            let t = now();
            let mut done = Vec::new();
            for p in &paths {
                let p = absolute(p);
                // The backup asked for, or the newest that has it.
                let id = match &from {
                    Some(f) => pick_backup(&snaps, f, t, local_offset)?,
                    None => snaps.iter().rev().take(60).find(|(id, _)| node(&me, &place, id, &p).is_some()).map(|s| s.0.clone()).with_context(|| format!("{} isn't in any backup", p.display()))?,
                };
                let time = snaps.iter().find(|s| s.0 == id).map(|s| s.1).unwrap_or(0);
                let dest = restore_one(&me, &place, &id, time, &p, to.as_deref(), if replace { IfThere::Replace } else { IfThere::KeepBoth })?;
                if !json {
                    println!("Brought back {} as it was {}{}", p.display(), when(time, t, local_offset).replacen("Today", "today", 1).replacen("Yesterday", "yesterday", 1), if dest != p { format!(", as {}", dest.display()) } else { String::new() });
                }
                done.push(dest.to_string_lossy().into_owned());
            }
            if json {
                println!("{}", serde_json::to_string(&done)?);
            }
            Ok(())
        }
        BackupCommand::Status { json } => {
            let me = me()?;
            let place = local(&me);
            let set = settings(&me.home);
            let state = read_state(&place);
            let (patterns, refused) = exclude_patterns(&me.home, &set.leave_out);
            let _ = patterns;
            let disk = free(if place.dir.exists() { &place.dir } else { Path::new("/") });
            let v = serde_json::json!({
                "on": set.on.unwrap_or(state.problem.as_deref() != Some("too-big")),
                "on_said": set.on,
                "to": set.to,
                "ready": place.dir.is_dir(),
                "running": running(&place),
                "leave_out": set.leave_out,
                "default_leave_out": set.default_leave_out,
                "defaults": DEFAULT_LEAVE_OUT,
                "refused": refused,
                "free": disk.map(|d| d.0),
                "free_bytes": disk.map(|d| d.1),
                "state": state,
            });
            if json {
                println!("{v}");
                return Ok(());
            }
            // A card: ✓ fine, ⚠ worth a look, as `nami status` has it.
            let t = now();
            let paint = crate::style::out();
            let say = |l: &str| println!("{}", paint.line(&format!("  {l}")));
            let on = set.on.unwrap_or(state.problem.as_deref() != Some("too-big"));
            println!("{}", paint.bold("Backups"));
            say(&if on { "✓ On · every hour".to_string() } else { "⚠ Off · Settings → Backups turns them on".to_string() });
            match state.done {
                Some(d) => say(&format!("✓ Last backup {}", ago(d, t))),
                None => say("⚠ No backup yet · nami backup now"),
            }
            if !state.said.is_empty() {
                say(&format!("⚠ {}", state.said));
            }
            if let Some(b) = state.bytes {
                say(&format!("✓ {} on this computer · {} backups", bytes(b), state.count.unwrap_or(0)));
            }
            if let Some(to) = &set.to {
                let o = state.other.as_deref();
                say(&match o.and_then(|o| o.done) {
                    Some(d) => format!("✓ Also to {to} · last {}", ago(d, t)),
                    None => format!("⚠ Also to {to} · not yet"),
                });
            }
            println!("{}", paint.dim(&format!("  Left out: {}", set.leave_out.join(", "))));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(_: i64) -> i64 {
        0
    }

    /// 2026-11-09 12:00 UTC (a Monday).
    const NOW: i64 = 1_794_225_600;

    #[test]
    fn times() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2026-11-09T12:00:00Z"), Some(NOW));
        assert_eq!(parse_time("2026-11-09T13:00:00.123456789+01:00"), Some(NOW));
        assert_eq!(parse_time("2026-11-09T07:30:00-04:30"), Some(NOW));
        assert_eq!(civil_from_days(NOW / DAY), (2026, 11, 9));
        assert_eq!(when(NOW - 2 * HOUR, NOW, utc), "Today, 10:00");
        assert_eq!(when(NOW - DAY, NOW, utc), "Yesterday, 12:00");
        assert_eq!(when(NOW - 5 * DAY, NOW, utc), "Nov 4, 12:00");
        assert_eq!(when(NOW - 400 * DAY, NOW, utc), "Oct 5, 2025, 12:00");
        assert_eq!(ago(NOW - 600, NOW), "10 minutes ago");
        assert_eq!(ago(NOW - 3600, NOW), "1 hour ago");
    }

    fn ids(kept: &HashSet<String>, snaps: &[(String, i64)]) -> Vec<i64> {
        let mut v: Vec<i64> = snaps.iter().filter(|s| kept.contains(&s.0)).map(|s| (NOW - s.1) / 60).collect();
        v.sort();
        v
    }

    #[test]
    fn all_of_the_last_day() {
        // A backup every 20 minutes for 3 hours (Back up now between the hourly ones): all
        // kept, so what was there for minutes isn't gone with them.
        let snaps: Vec<(String, i64)> = (0..9).map(|i| (format!("s{i}"), NOW - 10 * 60 - i * 20 * 60)).collect();
        assert_eq!(keep(&snaps, NOW, utc).len(), 9);
        // A day and more ago: that day's newest.
        let old: Vec<(String, i64)> = (0..9).map(|i| (format!("s{i}"), NOW - 2 * DAY - i * 20 * 60)).collect();
        assert_eq!(ids(&keep(&old, NOW, utc), &old), vec![2 * 24 * 60]);
    }

    #[test]
    fn mac_like_thinning() {
        // Every hour for 60 days.
        let snaps: Vec<(String, i64)> = (0..60 * 24).map(|i| (format!("s{i}"), NOW - i * HOUR)).collect();
        let kept = keep(&snaps, NOW, utc);
        let hours = snaps.iter().filter(|s| kept.contains(&s.0) && NOW - s.1 < DAY).count();
        let days = snaps.iter().filter(|s| kept.contains(&s.0) && NOW - s.1 >= DAY && NOW - s.1 < 30 * DAY).count();
        let weeks = snaps.iter().filter(|s| kept.contains(&s.0) && NOW - s.1 >= 30 * DAY).count();
        assert_eq!(hours, 24);
        // Days 1 to 29 back, plus yesterday's part not in the last 24 hours (its newest is an
        // hour bucket's already: the day bucket then takes the next).
        assert!((29..=30).contains(&days), "{days}");
        // 30 days more: about 5 weeks (a partial week at each end).
        assert!((4..=6).contains(&weeks), "{weeks}");
        assert!(kept.len() < 70);
    }

    #[test]
    fn the_newest_and_lone_old_ones_stay() {
        let snaps = vec![("a".to_string(), NOW - 400 * DAY), ("b".to_string(), NOW - 200 * DAY), ("c".to_string(), NOW - 199 * DAY)];
        let kept = keep(&snaps, NOW, utc);
        // b and c: the same week (Apr 22 and 23, 2026), c newer.
        assert!(kept.contains("a") && kept.contains("c") && !kept.contains("b"));
        // One backup, years old: kept.
        assert_eq!(keep(&[("x".to_string(), 0)], NOW, utc).len(), 1);
        assert!(keep(&[], NOW, utc).is_empty());
    }

    #[test]
    fn hours_and_days_are_local() {
        // 23:30 and 00:30 local (UTC+1), 3 days ago: two days, both kept.
        let offset = |_| HOUR;
        let base = (NOW - 3 * DAY).div_euclid(DAY) * DAY - HOUR;
        let snaps = vec![("before".to_string(), base - 30 * 60), ("after".to_string(), base + 30 * 60)];
        assert_eq!(keep(&snaps, NOW, offset).len(), 2);
        // In UTC they're the same day (22:30 and 23:30).
        assert_eq!(keep(&snaps, NOW, utc).len(), 1);
    }

    #[test]
    fn a_clock_set_back_keeps_them() {
        let snaps = vec![("future".to_string(), NOW + 3 * HOUR), ("now".to_string(), NOW)];
        assert_eq!(keep(&snaps, NOW, utc).len(), 2);
    }

    #[test]
    fn leave_out() {
        let home = Path::new("/home/ada");
        let lines: Vec<String> = ["~/.cache", "node_modules", "~/Videos/", "*.qcow2", "", "# a note", "~/.cache"].iter().map(|s| s.to_string()).collect();
        let (p, refused) = exclude_patterns(home, &lines);
        assert_eq!(p[..4], ["/home/ada/.cache", "node_modules", "/home/ada/Videos", "*.qcow2"]);
        assert!(refused.is_empty());
        assert!(p.contains(&"/home/ada/.cache/restic".to_string()));
        // Nothing that would leave out everything.
        for all in ["~", "~/", "/", "*", "/home", "/home/ada", "**", "/*"] {
            let (p, refused) = exclude_patterns(home, &[all.to_string()]);
            assert_eq!(refused, vec![all.to_string()], "{all}");
            assert_eq!(p.len(), 2, "{all}");
        }
    }

    #[test]
    fn the_defaults_leave_out_caches_not_files() {
        let set = settings_from("");
        assert!(set.on.is_none() && set.default_leave_out && set.to.is_none());
        let (p, refused) = exclude_patterns(Path::new("/home/anna"), &set.leave_out);
        assert!(refused.is_empty());
        for want in ["/home/anna/.cache", "/home/anna/.local/share/Trash", "node_modules", "*.qcow2", "/home/anna/.local/share/containers"] {
            assert!(p.iter().any(|x| x == want), "{want}");
        }
        // Nothing of the person's own folders.
        for theirs in ["Documents", "Pictures", "Desktop", "Music", "Videos", "Downloads", "target", "src"] {
            assert!(!p.iter().any(|x| x.ends_with(&format!("/{theirs}")) || x == theirs), "{theirs}");
        }
    }

    #[test]
    fn settings_in_user_nami() {
        let s = settings_from("packages: [jq]\nnamios:\n  backups:\n    on: false\n    to: disk:Backups\n    leave-out: [~/Videos]\n");
        assert_eq!(s, Settings { on: Some(false), to: Some("disk:Backups".into()), leave_out: vec!["~/Videos".into()], default_leave_out: false });
        let s = settings_from("namios:\n  backups:\n    to: this computer\n    leave-out: []\n");
        assert!(s.on.is_none() && s.to.is_none() && s.leave_out.is_empty() && !s.default_leave_out);
        // A file that isn't YAML: backups as they are by themselves.
        assert!(settings_from(": : [").on.is_none());
        assert_eq!(settings_from("namios:\n  backups:\n    on: true\n").on, Some(true));
    }

    #[test]
    fn on_by_itself_only_when_it_fits() {
        // Ada's: 290 GB of files, 355 GB free of 915: not by itself.
        assert!(!fits_by_itself(290 * GB, 355 * GB));
        // A new person's: a few MB.
        assert!(fits_by_itself(40_000_000, 355 * GB));
        // A third of what's free: not quite.
        assert!(fits_by_itself(100 * GB, 301 * GB) && !fits_by_itself(100 * GB, 300 * GB));
        assert!(!fits_by_itself(1, 0));
    }

    #[test]
    fn room_before_and_while() {
        let total = 915 * GB;
        // 15% of 915 GB is 137 GB: 290 GB into 355 GB free leaves 65 GB, too little.
        assert!(!room_for(290 * GB, 355 * GB, total, 20 * GB));
        assert!(room_for(200 * GB, 355 * GB, total, 20 * GB));
        // An hour's few MB: room.
        assert!(room_for(5_000_000, 355 * GB, total, 20 * GB));
        // A small disk: 20 GB is more than 15% of it.
        assert!(!room_for(GB, 20 * GB, 100 * GB, 20 * GB) && room_for(GB, 22 * GB, 100 * GB, 20 * GB));
        // A 16 GB disk's floors are a quarter and an eighth of it, not 20 GB it can't have.
        std::env::remove_var("NAMI_BACKUPS_FLOOR_GB");
        assert_eq!(floors(16 * GB), (4 * GB, 2 * GB));
        assert_eq!(floors(915 * GB), (20 * GB, 10 * GB));
        // Stopping: under a tenth, or under 10 GB.
        assert!(must_stop(90 * GB, total, 10 * GB) && !must_stop(92 * GB, total, 10 * GB));
        assert!(must_stop(9 * GB, 50 * GB, 10 * GB));
    }

    #[test]
    fn what_a_first_backup_reads() {
        let home = std::env::temp_dir().join(format!("nami-estimate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let put = |p: &str, n: usize| {
            let p = home.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, vec![b'x'; n]).unwrap();
        };
        put("Documents/a.txt", 1000);
        put("Pictures/big.raw", 50_000);
        put(".cache/thumb", 70_000);
        put("proj/node_modules/x/lib.js", 30_000);
        put("proj/target/debug/app", 90_000);
        put("proj/target/CACHEDIR.TAG", 43);
        put("proj/src/main.rs", 200);
        put("vm/disk.qcow2", 80_000);
        put(".local/share/Trash/files/old", 60_000);
        let (patterns, _) = exclude_patterns(&home, &DEFAULT_LEAVE_OUT.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let leave = LeaveOut::new(&patterns);
        assert_eq!(estimate(&home, &leave, None), 1000 + 50_000 + 200);
        // Changed since a minute from now: nothing.
        assert_eq!(estimate(&home, &leave, Some(now() + 60)), 0);
        // Nothing left out: all of it but the cache folder (CACHEDIR.TAG).
        assert_eq!(estimate(&home, &LeaveOut::new(&[]), None), 1000 + 50_000 + 70_000 + 30_000 + 200 + 80_000 + 60_000);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn leave_out_matches_as_restic_does() {
        let leave = LeaveOut::new(&["/home/j/.cache".into(), "node_modules".into(), "*.qcow2".into(), "/home/j/.var/app/*/cache".into(), "a\\*b".into()]);
        assert!(leave.out(Path::new("/home/j/.cache")) && leave.out(Path::new("/home/j/.cache/x/y")));
        assert!(!leave.out(Path::new("/home/j/.cached")) && !leave.out(Path::new("/home/j/Documents/.cache")));
        assert!(leave.out(Path::new("/home/j/p/node_modules/z")) && leave.out(Path::new("/home/j/vm/win.qcow2")));
        assert!(!leave.out(Path::new("/home/j/vm/win.qcow2.txt")));
        assert!(leave.out(Path::new("/home/j/.var/app/org.x/cache/q")) && !leave.out(Path::new("/home/j/.var/app/org.x/data")));
        assert!(leave.out(Path::new("/home/j/a*b")) && !leave.out(Path::new("/home/j/axb")));
    }

    #[test]
    fn picking_a_backup() {
        let snaps: Vec<(String, i64)> = vec![("aaaa1111".into(), NOW - 3 * DAY), ("bbbb2222".into(), NOW - DAY - 2 * HOUR), ("cccc3333".into(), NOW - 2 * HOUR), ("dddd4444".into(), NOW - 600)];
        let pick = |f: &str| pick_backup(&snaps, f, NOW, utc).unwrap();
        assert_eq!(pick("latest"), "dddd4444");
        assert_eq!(pick("bbbb"), "bbbb2222");
        assert_eq!(pick("1h"), "cccc3333");
        assert_eq!(pick("2d"), "aaaa1111");
        assert_eq!(pick("yesterday"), "bbbb2222");
        assert_eq!(pick("2026-11-09"), "dddd4444");
        assert_eq!(pick("2026-11-09 10:00"), "cccc3333");
        assert_eq!(pick("2026-11-09T09:00"), "bbbb2222");
        assert_eq!(pick("2026-11-06"), "aaaa1111");
        assert_eq!(pick("10:00"), "cccc3333");
        assert_eq!(pick("9:59"), "bbbb2222");
        assert_eq!(pick("today"), "dddd4444");
        let too_early = pick_backup(&snaps, "2026-11-01", NOW, utc).unwrap_err().to_string();
        assert!(too_early.contains("the first is from Nov 6, 12:00"), "{too_early}");
        assert!(pick_backup(&snaps, "last tuesday", NOW, utc).is_err());
        assert!(pick_backup(&[], "latest", NOW, utc).is_err());
    }

    #[test]
    fn keeping_both() {
        assert_eq!(both_name("Report.pdf", "Nov 9, 10:00", false), "Report (Nov 9, 10.00).pdf");
        assert_eq!(both_name("notes", "Nov 9, 10.00", false), "notes (Nov 9, 10.00)");
        assert_eq!(both_name(".bashrc", "Nov 9, 10:00", false), ".bashrc (Nov 9, 10.00)");
        assert!(stamp(NOW, utc).starts_with("Nov 9") && stamp(NOW, utc).ends_with(", 12.00"));
        assert_eq!(both_name("photos.2026", "Nov 9, 10:00", true), "photos.2026 (Nov 9, 10.00)");
    }

    #[test]
    fn patterns_are_escaped() {
        assert_eq!(escape_pattern("/home/j/a [1]*.txt"), "/home/j/a \\[1\\]\\*.txt");
    }

    #[test]
    fn low_battery_waits() {
        let dir = std::env::temp_dir().join(format!("nami-battery-{}", std::process::id()));
        let bat = dir.join("BAT0");
        std::fs::create_dir_all(&bat).unwrap();
        std::fs::write(bat.join("type"), "Battery\n").unwrap();
        std::fs::write(bat.join("status"), "Discharging\n").unwrap();
        std::fs::write(bat.join("capacity"), "15\n").unwrap();
        unsafe { std::env::set_var("NAMI_POWER_SUPPLY", &dir) };
        assert_eq!(low_battery(), Some(15));
        std::fs::write(bat.join("status"), "Charging\n").unwrap();
        assert_eq!(low_battery(), None);
        std::fs::write(bat.join("status"), "Discharging\n").unwrap();
        std::fs::write(bat.join("capacity"), "20\n").unwrap();
        assert_eq!(low_battery(), None);
        unsafe { std::env::remove_var("NAMI_POWER_SUPPLY") };
        let _ = std::fs::remove_dir_all(dir);
    }
}
