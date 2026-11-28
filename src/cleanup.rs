//! Cleaning up by itself: old changes go once they're old, and then what
//! only they used in /nix/store. `cleanup:` in user.nami (your changes) and
//! system.nami (the computer's) says when: 30 days unless it says
//! otherwise, `off` for never, or `{ older-than: 30d, keep: 3 }`. The
//! current change and the newest `keep` always stay, whatever their age.
//!
//! The daily update runs it (`nami gc --auto`, as root): the computer's
//! changes (on namiOS its systems, and their boot entries), each person's
//! (as them, by their own file), then the store's paths nothing uses, at
//! low priority. A person's own run of nami cleans up their changes too,
//! once a day. When the disk with /nix gets full (under 10% or 5 GB free),
//! the daily job and `nami apply` clean up early: by the same rules, but
//! without waiting for the age.

use std::path::Path;

use serde_yaml::Value;

/// How old a change gets before it goes, unless the file says otherwise.
pub const AFTER: u64 = 30 * 86400;
/// How many of the newest changes always stay.
pub const KEEP: usize = 3;
/// What's written for it: `cleanup: 30d`.
pub const KEY: &str = "cleanup";

/// When old changes go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// How old (seconds) a change gets before it goes; `None`: never by itself (`off`).
    pub after: Option<u64>,
    /// The newest changes that stay, whatever their age.
    pub keep: usize,
}

impl Default for Policy {
    fn default() -> Self {
        Policy { after: Some(AFTER), keep: KEEP }
    }
}

impl Policy {
    /// `30 days`, as the hints say it; none when it's off.
    pub fn words(&self) -> Option<String> {
        self.after.map(duration_words)
    }
}

/// The value of `cleanup:` as written: `30d`, `off`, or `{ older-than: 30d, keep: 3 }`.
pub fn parse(v: &Value) -> Result<Policy, String> {
    let age = |v: &Value| -> Result<Option<u64>, String> {
        match v {
            Value::String(s) if s.trim() == "off" || s.trim() == "never" => Ok(None),
            Value::Bool(false) => Ok(None),
            Value::String(s) => match crate::duration::seconds(s)? {
                0 => Err("a duration longer than none (30d), or off".to_string()),
                secs => Ok(Some(secs)),
            },
            other => Err(format!("a duration like 30d, or off, not {}", shown(other))),
        }
    };
    match v {
        Value::Null => Ok(Policy::default()),
        Value::Mapping(m) => {
            let mut p = Policy::default();
            for (k, val) in m {
                match k.as_str() {
                    Some("older-than") => p.after = age(val)?,
                    Some("keep") => p.keep = val.as_u64().filter(|n| *n >= 1).ok_or_else(|| format!("keep: a whole number, 1 or more (the current change always stays), not {}", shown(val)))? as usize,
                    _ => return Err(format!("`{}` isn't one of its keys (older-than, keep)", shown(k))),
                }
            }
            Ok(p)
        }
        _ => Ok(Policy { after: age(v)?, keep: KEEP }),
    }
}

/// A value as it's written, for messages.
fn shown(v: &Value) -> String {
    serde_yaml::to_string(v).unwrap_or_default().trim().to_string()
}

/// `cleanup:` of a nami file's text; `None` when it doesn't say (or can't be read: what
/// `nami check` says of it, the default meanwhile).
pub fn of_text(text: &str) -> Option<Policy> {
    let v = crate::yaml_text::from_str(text).ok()?;
    let c = v.get(KEY)?;
    Some(parse(c).unwrap_or_default())
}

/// The policy of the file at `path` (the default when it's not there or doesn't say).
pub fn of_file(path: &Path) -> Policy {
    std::fs::read_to_string(path).ok().and_then(|t| of_text(&t)).unwrap_or_default()
}

/// For serde: `cleanup:` in a [`crate::config::Config`].
pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Policy>, D::Error> {
    let v: Value = serde::Deserialize::deserialize(d)?;
    parse(&v).map(Some).map_err(|e| serde::de::Error::custom(format!("cleanup: {e}")))
}

/// `30 days`, `12 hours`.
pub fn duration_words(secs: u64) -> String {
    let (n, one, many) = match secs {
        s if s % 86400 == 0 => (s / 86400, "day", "days"),
        s if s % 3600 == 0 => (s / 3600, "hour", "hours"),
        s if s % 60 == 0 => (s / 60, "minute", "minutes"),
        s => (s, "second", "seconds"),
    };
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Which of `steps` (each its number and when it was made, 0 if that's not known) go:
/// never those in `stay` (the current one, and on namiOS the ones the boot menu needs),
/// nor the newest `keep`; of the rest, those older than the policy says, or all of them
/// when space is `low`. Off, nothing goes by itself, low or not.
pub fn pick(steps: &[(u64, u64)], stay: &[u64], policy: &Policy, now: u64, low: bool) -> Vec<u64> {
    let Some(after) = policy.after else { return Vec::new() };
    let mut numbers: Vec<u64> = steps.iter().map(|s| s.0).collect();
    numbers.sort_unstable();
    let newest: Vec<u64> = numbers.iter().rev().take(policy.keep.max(1)).copied().collect();
    let mut out: Vec<u64> = steps
        .iter()
        .filter(|(g, _)| !stay.contains(g) && !newest.contains(g))
        // (When it was made isn't known: it isn't old, but it goes when space is low.)
        .filter(|(_, created)| low || (*created > 0 && created.saturating_add(after) <= now))
        .map(|(g, _)| *g)
        .collect();
    out.sort_unstable();
    out
}

/// The time now, or `NAMI_NOW` (seconds; for tests of what goes when).
pub fn now() -> u64 {
    std::env::var("NAMI_NOW").ok().and_then(|n| n.parse().ok()).unwrap_or_else(crate::profile::now)
}

/// Whether a disk of `total` bytes with `free` left is low: under 10% or 5 GB.
pub fn is_low(total: u64, free: u64) -> bool {
    total > 0 && (free < 5_000_000_000 || free.saturating_mul(10) < total)
}

/// Whether the disk with the store on it is low on space (`NAMI_SPACE_LOW=1` says so, for tests).
pub fn space_low(store_dir: &Path) -> bool {
    if let Ok(v) = std::env::var("NAMI_SPACE_LOW") {
        return v == "1";
    }
    let (total, free) = disk(store_dir);
    is_low(total, free)
}

/// A disk's size and what's free on it (for anyone: what isn't kept for root).
pub fn disk(path: &Path) -> (u64, u64) {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return (0, 0) };
    // SAFETY: a path, and the struct statvfs fills.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return (0, 0);
    }
    let block = s.f_frsize as u64;
    (s.f_blocks as u64 * block, s.f_bavail as u64 * block)
}

/// `4–9`, `2, 4–6 and 9`: numbers as ranges.
pub fn ranges(numbers: &[u64]) -> String {
    let mut sorted = numbers.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        let mut j = i;
        while j + 1 < sorted.len() && sorted[j + 1] == sorted[j] + 1 {
            j += 1;
        }
        parts.push(match j - i {
            0 => sorted[i].to_string(),
            1 => format!("{}, {}", sorted[i], sorted[j]),
            _ => format!("{}–{}", sorted[i], sorted[j]),
        });
        i = j + 1;
    }
    crate::tell::and_list(&parts)
}

/// `changes 4–9`, `change 3`.
pub fn changes_words(numbers: &[u64]) -> String {
    format!("{} {}", if numbers.len() == 1 { "change" } else { "changes" }, ranges(numbers))
}

/// From here on, this process runs at low priority (nice 19, idle I/O): the daily cleanup's
/// deleting doesn't slow anyone's work.
pub fn low_priority() {
    // SAFETY: plain system calls on this process.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 19);
        // ioprio_set(IOPRIO_WHO_PROCESS, this one, IOPRIO_CLASS_IDLE): Linux's.
        #[cfg(target_os = "linux")]
        libc::syscall(libc::SYS_ioprio_set, 1, 0, 3 << 13);
    }
}

/// The store paths running programs use (Nix's runtime roots): what each process runs
/// (exe), works in (cwd), has open (fd), has mapped (maps, its libraries) and has in its
/// environment (a `nami shell`'s PATH). As root every process's; as anyone else, theirs.
pub fn running_paths() -> Vec<String> {
    let mut found = std::collections::BTreeSet::new();
    let Ok(procs) = std::fs::read_dir("/proc") else { return Vec::new() };
    for p in procs.flatten() {
        let name = p.file_name();
        if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let dir = p.path();
        for link in ["exe", "cwd", "root"] {
            if let Ok(t) = std::fs::read_link(dir.join(link)) {
                store_paths_in(t.to_string_lossy().as_bytes(), &mut found);
            }
        }
        for fd in std::fs::read_dir(dir.join("fd")).into_iter().flatten().flatten() {
            if let Ok(t) = std::fs::read_link(fd.path()) {
                store_paths_in(t.to_string_lossy().as_bytes(), &mut found);
            }
        }
        for file in ["maps", "environ"] {
            if let Ok(text) = std::fs::read(dir.join(file)) {
                store_paths_in(&text, &mut found);
            }
        }
    }
    found.into_iter().collect()
}

/// Every `/nix/store/<hash>-<name>` in `text`, as that path.
fn store_paths_in(text: &[u8], found: &mut std::collections::BTreeSet<String>) {
    const AT: &[u8] = b"/nix/store/";
    let mut i = 0;
    while let Some(at) = text[i..].windows(AT.len()).position(|w| w == AT) {
        let start = i + at + AT.len();
        let len = text[start..].iter().take_while(|b| b.is_ascii_alphanumeric() || b"+-._?=".contains(b)).count();
        let base = &text[start..start + len];
        if base.len() > 33 && base[32] == b'-' && base[..32].iter().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()) {
            found.insert(format!("/nix/store/{}", String::from_utf8_lossy(base)));
        }
        i = start + len;
    }
}

/// What a cleanup did, to say in a line.
#[derive(Debug, Default)]
pub struct Done {
    /// The system's changes that went (its profile's, or on namiOS its systems).
    pub computer: Vec<u64>,
    /// Each person's that went (by their login).
    pub people: Vec<(String, Vec<u64>)>,
    /// What deleting unused store paths freed.
    pub paths: usize,
    pub bytes: u64,
}

impl Done {
    pub fn is_empty(&self) -> bool {
        self.computer.is_empty() && self.people.iter().all(|(_, g)| g.is_empty()) && self.paths == 0
    }

    /// `Cleaned up: changes 4–9, ada's changes 2–3, 2.3 GB freed` (silent: none).
    pub fn line(&self, low: bool) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        if !self.computer.is_empty() {
            parts.push(changes_words(&self.computer));
        }
        for (who, steps) in self.people.iter().filter(|(_, g)| !g.is_empty()) {
            parts.push(format!("{who}'s {}", changes_words(steps)));
        }
        if self.paths > 0 {
            parts.push(format!("{} freed", crate::tell::size(self.bytes)));
        }
        let head = if low { "Space is low, so old changes were cleaned up early" } else { "Cleaned up" };
        Some(format!("{head}: {}", parts.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86400;

    fn steps(ages_in_days: &[(u64, u64)], now: u64) -> Vec<(u64, u64)> {
        ages_in_days.iter().map(|(g, age)| (*g, now - age * DAY)).collect()
    }

    #[test]
    fn reads_what_the_file_says() {
        let p = |s: &str| parse(&serde_yaml::from_str::<Value>(s).unwrap());
        assert_eq!(p("30d").unwrap(), Policy::default());
        assert_eq!(p("7d").unwrap(), Policy { after: Some(7 * DAY), keep: 3 });
        assert_eq!(p("2w").unwrap().after, Some(14 * DAY));
        assert_eq!(p("off").unwrap(), Policy { after: None, keep: 3 });
        assert_eq!(p("false").unwrap().after, None);
        assert_eq!(p("{ older-than: 90d, keep: 5 }").unwrap(), Policy { after: Some(90 * DAY), keep: 5 });
        assert_eq!(p("{ keep: 10 }").unwrap(), Policy { after: Some(AFTER), keep: 10 });
        assert_eq!(p("{ older-than: off }").unwrap().after, None);
        assert!(p("{ keep: 0 }").unwrap_err().contains("1 or more"));
        assert!(p("{ older: 3d }").unwrap_err().contains("older-than, keep"));
        assert!(p("3x").unwrap_err().contains("s, m, h, d or w"));
        assert!(p("0d").is_err());
        assert!(p("[1]").is_err());
        assert_eq!(of_text("packages: [jq]\ncleanup: 7d\n"), Some(Policy { after: Some(7 * DAY), keep: 3 }));
        assert_eq!(of_text("packages: [jq]\n"), None);
        // (Written wrong: the default meanwhile; `nami check` says what's wrong.)
        assert_eq!(of_text("cleanup: soon\n"), Some(Policy::default()));
    }

    #[test]
    fn old_changes_go_and_the_newest_stay() {
        let now = 1_800_000_000;
        // Changes 1 to 7, 1 the oldest: 60, 45, 31, 29, 10, 2 and 0 days old; 7 is current.
        let s = steps(&[(1, 60), (2, 45), (3, 31), (4, 29), (5, 10), (6, 2), (7, 0)], now);
        assert_eq!(pick(&s, &[7], &Policy::default(), now, false), [1, 2, 3]);
        // A week: 4 too (5, 6 and 7 are the newest three).
        assert_eq!(pick(&s, &[7], &Policy { after: Some(7 * DAY), keep: 3 }, now, false), [1, 2, 3, 4]);
        // Ninety days: none is that old.
        assert!(pick(&s, &[7], &Policy { after: Some(90 * DAY), keep: 3 }, now, false).is_empty());
        // Off: nothing.
        assert!(pick(&s, &[7], &Policy { after: None, keep: 3 }, now, false).is_empty());
    }

    #[test]
    fn the_newest_keep_stay_whatever_their_age() {
        let now = 1_800_000_000;
        let s = steps(&[(1, 400), (2, 300), (3, 200), (4, 100)], now);
        assert_eq!(pick(&s, &[4], &Policy::default(), now, false), [1]);
        assert_eq!(pick(&s, &[4], &Policy { after: Some(DAY), keep: 1 }, now, false), [1, 2, 3]);
        assert!(pick(&s, &[4], &Policy { after: Some(DAY), keep: 10 }, now, false).is_empty());
        // keep: 0 is read as 1: the newest is never taken.
        assert_eq!(pick(&s, &[], &Policy { after: Some(DAY), keep: 0 }, now, false), [1, 2, 3]);
    }

    #[test]
    fn the_current_change_always_stays() {
        let now = 1_800_000_000;
        // Gone back to 2 (nami undo), which is old: it stays, as do the newest three.
        let s = steps(&[(1, 90), (2, 80), (3, 70), (4, 60), (5, 50), (6, 40)], now);
        assert_eq!(pick(&s, &[2], &Policy::default(), now, false), [1, 3]);
        assert_eq!(pick(&s, &[2], &Policy::default(), now, true), [1, 3]);
        // On namiOS: the running one, and the one before it, for the boot menu.
        assert_eq!(pick(&s, &[1, 2], &Policy::default(), now, false), [3]);
    }

    #[test]
    fn low_space_cleans_up_early() {
        let now = 1_800_000_000;
        let s = steps(&[(1, 5), (2, 4), (3, 3), (4, 2), (5, 1), (6, 0)], now);
        assert!(pick(&s, &[6], &Policy::default(), now, false).is_empty());
        assert_eq!(pick(&s, &[6], &Policy::default(), now, true), [1, 2, 3]);
        assert_eq!(pick(&s, &[6], &Policy { after: Some(AFTER), keep: 5 }, now, true), [1]);
        // Off stays off.
        assert!(pick(&s, &[6], &Policy { after: None, keep: 1 }, now, true).is_empty());
        // When a change was made isn't known (an image's, dated 1970): it goes only then.
        let unknown = [(1, 0), (2, 0), (3, now), (4, now), (5, now)];
        assert!(pick(&unknown, &[5], &Policy { after: Some(DAY), keep: 3 }, now, false).is_empty());
        assert_eq!(pick(&unknown, &[5], &Policy { after: Some(DAY), keep: 3 }, now, true), [1, 2]);
    }

    #[test]
    fn low_is_under_ten_percent_or_five_gb() {
        const GB: u64 = 1_000_000_000;
        assert!(!is_low(500 * GB, 100 * GB));
        assert!(is_low(500 * GB, 40 * GB));
        assert!(is_low(500 * GB, 4 * GB));
        assert!(is_low(20 * GB, 4 * GB));
        assert!(!is_low(20 * GB, 6 * GB));
        assert!(!is_low(0, 0), "not known: not low");
    }

    #[test]
    fn says_ranges_and_durations() {
        assert_eq!(ranges(&[4, 5, 6, 7, 8, 9]), "4–9");
        assert_eq!(ranges(&[9, 2, 4, 5, 6]), "2, 4–6 and 9");
        assert_eq!(ranges(&[3]), "3");
        assert_eq!(ranges(&[3, 4]), "3, 4");
        assert_eq!(changes_words(&[3]), "change 3");
        assert_eq!(duration_words(30 * DAY), "30 days");
        assert_eq!(duration_words(7 * DAY), "7 days");
        assert_eq!(duration_words(14 * DAY), "14 days");
        assert_eq!(duration_words(DAY), "1 day");
        assert_eq!(duration_words(90 * DAY), "90 days");
        assert_eq!(duration_words(12 * 3600), "12 hours");
        let done = Done { computer: vec![4, 5, 6, 7, 8, 9], people: vec![("ada".into(), vec![2, 3])], paths: 120, bytes: 2_300_000_000 };
        assert_eq!(done.line(false).unwrap(), "Cleaned up: changes 4–9, ada's changes 2, 3, 2.3 GB freed");
        assert_eq!(Done::default().line(false), None);
    }

    #[test]
    fn finds_store_paths_in_what_processes_have() {
        let mut found = std::collections::BTreeSet::new();
        let maps = b"7f00-7f01 r-xp 0 00:1 2 /nix/store/0c3ssd2rjqakfggl1xcgndrxq3ll8bqz-glibc-2.40/lib/libc.so.6\n\
            PATH=/nix/store/1d8kd9s0q7n3c2mqcz9wq9a1b6xv0jrf-jq-1.7/bin:/usr/bin\0HOME=/home/ada\0/nix/store/short-x\0/nix/store/";
        store_paths_in(maps, &mut found);
        let found: Vec<&str> = found.iter().map(String::as_str).collect();
        assert_eq!(found, ["/nix/store/0c3ssd2rjqakfggl1xcgndrxq3ll8bqz-glibc-2.40", "/nix/store/1d8kd9s0q7n3c2mqcz9wq9a1b6xv0jrf-jq-1.7"]);
        // This test's own program: running, so found.
        let me = std::env::current_exe().unwrap();
        if me.starts_with("/nix/store") {
            assert!(running_paths().iter().any(|p| me.starts_with(p)));
        }
    }
}
