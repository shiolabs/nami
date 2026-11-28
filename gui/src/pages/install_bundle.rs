//! A nami bundle among the apps from downloads (src/bundle_app.rs): what
//! Settings shows of one beside what it shows of a .deb. Its updates are
//! the bundle's own (src/bundle_update.rs): the version installed is read
//! from the file itself (it changes as it updates), and what its own last
//! look at its developer's feed found from the bundles' shared store,
//! ~/.cache/tray-bundle/updates.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// What its own last look found (`nami package-info`'s, and read here).
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct LastLook {
    pub checked: u64,
    pub error: Option<String>,
    pub newer: Option<String>,
    pub why: Option<String>,
}

/// What `nami package-info --json` says of a bundle beside the rest.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Extra {
    pub key: Option<String>,
    pub fingerprint: Option<String>,
    pub feed: Option<String>,
    /// "daily", "off", "none".
    pub updates: String,
    pub last_look: Option<LastLook>,
    pub installed_at: Option<String>,
    /// Bytes of its parts this computer has already (shared with other bundles).
    pub here: u64,
    /// Who made it, as the bundle says (signed with its updates).
    pub developer: Option<String>,
    /// Its key here: "unsigned", "new", "same" (trusted before) or "changed" (refused).
    pub trust: String,
    /// What its sandbox lets it use, in words; and whether it's sandboxed here.
    pub sandbox: String,
    pub sandboxed: bool,
}

/// The note under who made it: the key that signs its updates, as people
/// compare it at a glance (its first four groups), as the bundle's first
/// start and `--nami-bundle-info` show it.
pub fn key_note(e: &Extra) -> Option<String> {
    let short = e.fingerprint.as_ref()?.split(' ').take(4).collect::<Vec<_>>().join(" ");
    Some(match e.trust.as_str() {
        "changed" => format!("Another key: {short}"),
        _ => format!("Signed · Key {short}"),
    })
}

/// What app.json keeps of an installed bundle.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Kept {
    pub file: String,
    pub desktop_id: Option<String>,
    pub feed: Option<String>,
    pub key: Option<String>,
}

/// An installed bundle, as it is now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub version: String,
    /// It updates itself (made so, and not turned off).
    pub updates: bool,
    /// Made to update itself, but turned off: by the person for it, or with the computer's.
    pub off: bool,
    pub look: Option<LastLook>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Update {
    feed: String,
    serial: u64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Meta {
    name: String,
    version: String,
    update: Option<Update>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Available {
    version: String,
    serial: u64,
    why: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct State {
    checked: u64,
    error: String,
    available: Option<Available>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A bundle's description, from its trailer (bundle_format.rs: the last
/// 64 bytes say where it is; its sha256 is checked).
fn meta(file: &Path) -> Option<Meta> {
    let mut f = std::fs::File::open(file).ok()?;
    let len = f.seek(SeekFrom::End(0)).ok()?;
    f.seek(SeekFrom::Start(len.checked_sub(64)?)).ok()?;
    let mut t = [0u8; 64];
    f.read_exact(&mut t).ok()?;
    if &t[56..64] != b"TRAYBUN1" {
        return None;
    }
    let u = |o: usize| u64::from_le_bytes(t[o..o + 8].try_into().unwrap());
    let (at, n) = (u(8), u(16));
    if n > 64 << 20 || at + n + 64 != len {
        return None;
    }
    f.seek(SeekFrom::Start(at)).ok()?;
    let mut m = vec![0u8; n as usize];
    f.read_exact(&mut m).ok()?;
    if Sha256::digest(&m).as_slice() != &t[24..56] {
        return None;
    }
    serde_json::from_slice(&m).ok()
}

fn safe(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect()
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// The bundles' stores, where their checks keep notes.
fn caches() -> Vec<PathBuf> {
    bundle_stores()
}

/// The bundles' shared parts: where their runtimes keep them (src/bundle_format.rs'
/// cache_dirs). In ~/.cache it's still `tray-bundle`, as bundles already given out
/// unpack there (docs/rename-migration.md); with XDG_CACHE_HOME set, nami 0.18.6 to
/// 0.18.8's bundles used `nami-bundle` there, and those made before the rename
/// `tray-bundle`: each that's there. (And who had no home of theirs, in /tmp.)
pub fn bundle_stores() -> Vec<PathBuf> {
    let var = |k: &str| std::env::var_os(format!("NAMI_{k}")).or_else(|| std::env::var_os(format!("TRAY_{k}"))).filter(|v| !v.is_empty());
    let mut all: Vec<PathBuf> = match (var("BUNDLE_CACHE"), std::env::var_os("XDG_CACHE_HOME").filter(|c| !c.is_empty())) {
        (Some(c), _) => vec![PathBuf::from(c)],
        (None, Some(c)) => vec![PathBuf::from(&c).join("tray-bundle"), PathBuf::from(&c).join("nami-bundle")],
        (None, None) => vec![home().join(".cache/tray-bundle")],
    };
    // SAFETY: getuid can't fail.
    let uid = unsafe { libc::getuid() };
    all.extend(["tray", "nami"].map(|w| std::env::temp_dir().join(format!("{w}-bundle-{uid}"))));
    all.retain(|d| d.join("store").is_dir());
    all
}

fn config() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(".config"))
}

/// This computer's daily updates are off (namiOS: the timer's enabling link
/// gone), which turns bundles' off too (bundle_update::system_says_off).
pub fn system_off() -> bool {
    // (tray-update.timer: a computer whose packages are still tray's.)
    let timers = Path::new("/etc/systemd/system/timers.target.wants");
    (Path::new("/etc/nami-os").is_dir() || Path::new("/etc/tray-os").is_dir()) && !timers.join("nami-update.timer").exists() && !timers.join("tray-update.timer").exists()
}

/// An installed bundle's version and updates, read from it and its store.
pub fn live(k: &Kept) -> Option<Live> {
    let file = PathBuf::from(&k.file);
    let m = meta(&file)?;
    let Some(u) = &m.update else { return Some(Live { version: m.version, ..Default::default() }) };
    let feed_hash = hex(&Sha256::digest(u.feed.as_bytes()));
    // (tray-bundle/: where a bundle made before the rename turned them off.)
    let person_off = ["nami-bundle", "tray-bundle"].iter().any(|d| config().join(d).join("updates-off").join(format!("{}-{}", safe(&m.name), &feed_hash[..8])).exists());
    let off = person_off || system_off();
    // Its checks' notes for this copy: `<name>-<sha256(path, feed)>.json`.
    let id = format!("{}-{}", safe(&m.name), &hex(&Sha256::digest(format!("{}\n{}", file.display(), u.feed).as_bytes()))[..12]);
    let state: Option<State> = caches().iter().find_map(|c| std::fs::read(c.join("updates").join(format!("{id}.json"))).ok()).and_then(|d| serde_json::from_slice(&d).ok());
    let staged = ["nami", "tray"].iter().any(|w| file.with_file_name(format!(".{}.{w}-update", file.file_name().unwrap_or_default().to_string_lossy())).exists());
    let look = state.map(|s| {
        let newer = s.available.filter(|a| a.serial > u.serial);
        LastLook {
            checked: s.checked,
            error: Some(s.error).filter(|e| !e.is_empty()),
            why: newer.as_ref().map(|a| a.why.clone()).filter(|w| !w.is_empty()),
            newer: newer.filter(|a| !a.why.is_empty() || staged).map(|a| a.version),
        }
    });
    Some(Live { version: m.version, updates: !off, off, look })
}

/// How long ago, as people say it.
pub fn ago(when: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let s = now.saturating_sub(when);
    match s {
        0..120 => "just now".into(),
        120..7200 => format!("{} minutes ago", s / 60),
        7200..172800 => format!("{} hours ago", s / 3600),
        _ => format!("{} days ago", s / 86400),
    }
}

/// The Updates fact of the install view: (value, note).
pub fn updates_fact(e: &Extra) -> (String, Option<String>) {
    match e.updates.as_str() {
        "daily" => {
            // (Short: a column's width.)
            let note = match e.last_look.as_ref().filter(|l| l.checked > 0) {
                Some(LastLook { newer: Some(v), why: None, .. }) => format!("{v} on next launch"),
                Some(LastLook { newer: Some(v), .. }) => format!("{v} available"),
                Some(LastLook { error: Some(_), .. }) => "Couldn't check".into(),
                Some(l) => format!("Checked {}", ago(l.checked)),
                None => "At launch".into(),
            };
            ("Daily".into(), Some(note))
        }
        "off" => ("Off".into(), Some(if system_off() { "Off for this computer".into() } else { "Off for this app".into() })),
        _ => ("Manual".into(), Some("Open a newer download".into())),
    }
}

/// Your Apps' line for an installed bundle: what it is, the version it is now.
pub fn row_line(version: &str, live: Option<&Live>) -> String {
    let v = live.map(|l| l.version.as_str()).filter(|v| !v.is_empty()).unwrap_or(version);
    if v.is_empty() { "A nami bundle".into() } else { format!("A nami bundle · {v}") }
}

/// How it's going with its updates, as for any app from a download
/// (app_updates::line_of): the words, and whether it's a problem. A
/// bundle looks by itself as it starts, so its own last look counts too.
pub fn update_line(known: Option<&crate::pages::app_updates::AppStatus>, live: Option<&Live>, made_to: bool) -> Option<(String, bool)> {
    if !made_to {
        return Some(("This app doesn't update automatically. To update it, open a newer download.".into(), false));
    }
    let l = live?;
    if system_off() {
        return Some(("Automatic updates are off for this computer.".into(), false));
    }
    let look = l.look.as_ref().filter(|k| k.checked > 0)?;
    // (The daily run's, when it's the newer word.)
    if known.is_some_and(|s| s.checked >= look.checked && !s.said.is_empty()) {
        return None;
    }
    Some(match (&look.newer, &look.why, &look.error) {
        (Some(v), None, _) => (format!("Version {v} is ready. It's used the next time the app opens."), false),
        (Some(v), Some(why), _) => (format!("Version {v} is available but can't be installed automatically: {why}."), true),
        (None, _, Some(e)) => (format!("Couldn't check for updates: {e}"), true),
        _ => (format!("Up to date. Checked {}.", ago(look.checked)), false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_its_last_look_found_in_words() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let live = Live { version: "1.0".into(), updates: true, off: false, look: Some(LastLook { checked: now - 60, newer: Some("2.0".into()), ..Default::default() }) };
        assert_eq!(row_line("0.9", Some(&live)), "A nami bundle · 1.0");
        assert_eq!(row_line("1.0", None), "A nami bundle · 1.0");
        if !system_off() {
            assert_eq!(update_line(None, Some(&live), true), Some(("Version 2.0 is ready. It's used the next time the app opens.".into(), false)));
        }
        assert!(update_line(None, Some(&live), false).is_some_and(|(l, _)| l.starts_with("This app doesn't update automatically")));
    }
}
