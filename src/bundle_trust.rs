//! Whose key a person trusts for each app: the first copy of a signed
//! bundle they run or install says who signs its updates (the key in its
//! description), and that's remembered, in
//! `~/.config/nami-bundle/trusted/<app>`. An update, or a new download of
//! the same app, signed with another key is refused: it may not be from the
//! developer they trusted. (A key is never replaced by an update, even one
//! signed with the old key: a developer who loses theirs starts a new app.)
//!
//! The app is what its menu entry would be (its desktop file's id), or its
//! name for a command: what a new copy would take the place of.
//!
//! Shared by nami (Settings' install page, `nami install`) and the runtime
//! (`--nami-bundle-install`, its first start, its updates): std only.

// Shared by nami and the runtime: each uses its own half.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use crate::bundle_format::Meta;

/// What's known of a bundle's key here.
#[derive(Debug, Clone, PartialEq)]
pub enum Trust {
    /// It doesn't sign updates (made without --update-url).
    Unsigned,
    /// Never seen here: trusted from now on.
    New,
    /// The key trusted before.
    Same,
    /// Another key than the one trusted before (that one's here).
    Changed { was: String },
}

pub fn dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
        .join("nami-bundle/trusted")
}

/// The app, as a file name.
pub fn app(meta: &Meta) -> String {
    crate::bundle_sandbox::app_id(meta)
}

/// The key trusted for this app, if one is.
pub fn trusted_in(dir: &Path, meta: &Meta) -> Option<String> {
    let text = fs::read_to_string(dir.join(app(meta))).ok()?;
    text.lines().next().map(|l| l.trim().to_string()).filter(|k| !k.is_empty())
}

pub fn check(meta: &Meta) -> Trust {
    check_in(&dir(), meta)
}

pub fn check_in(dir: &Path, meta: &Meta) -> Trust {
    let Some(u) = &meta.update else { return Trust::Unsigned };
    match trusted_in(dir, meta) {
        None => Trust::New,
        Some(k) if k == u.key => Trust::Same,
        Some(was) => Trust::Changed { was },
    }
}

/// Trust its key from now on, if none is trusted for it yet. What's trusted
/// already stays: a changed key is refused, never written over.
pub fn record(meta: &Meta) -> std::io::Result<Trust> {
    record_in(&dir(), meta)
}

pub fn record_in(dir: &Path, meta: &Meta) -> std::io::Result<Trust> {
    let t = check_in(dir, meta);
    if t == Trust::New {
        let u = meta.update.as_ref().expect("signed");
        fs::create_dir_all(dir)?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let file = dir.join(app(meta));
        let tmp = file.with_extension(format!("tmp{}", std::process::id()));
        fs::write(&tmp, format!("{}\n{}\n{}\n{now}\n", u.key, u.feed, meta.developer.as_deref().unwrap_or("")))?;
        fs::rename(&tmp, &file)?;
    }
    Ok(t)
}

/// Trust its key in place of the one trusted before: the developer handed
/// that one over to it (bundle_update::trust_in checked).
pub fn replace_in(dir: &Path, meta: &Meta) -> std::io::Result<Trust> {
    let _ = fs::remove_file(dir.join(app(meta)));
    record_in(dir, meta)
}

/// Forget the key trusted for it (the person removed it on purpose, after
/// being told the key changed: the next copy they run is trusted anew).
pub fn forget(meta: &Meta) {
    let _ = fs::remove_file(dir().join(app(meta)));
}

/// Who signs it, in words: "Ada Lovelace", else "its developer".
pub fn who(meta: &Meta) -> String {
    meta.developer.clone().filter(|d| !d.trim().is_empty()).unwrap_or_else(|| "its developer".into())
}

/// A fingerprint shortened to its first four groups, as people compare it at a glance.
pub fn short(fingerprint: &str) -> String {
    fingerprint.split(' ').take(4).collect::<Vec<_>>().join(" ")
}

/// The refusal, in words, for a key that changed.
/// `how`: how to trust the new key instead, where the refusal is said.
pub fn refusal(meta: &Meta, was_fp: &str, now_fp: &str, how: &str) -> String {
    let name = &meta.name;
    format!(
        "{name} is signed with another key ({}) than the one you trusted before ({}). It may not be from {}, so it wasn't installed. If its developer really changed their key, {how}.",
        short(now_fp),
        short(was_fp),
        who(meta)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle_format::Update;

    fn meta(key: &str) -> Meta {
        Meta { name: "myapp".into(), update: Some(Update { feed: "https://example.org/myapp".into(), key: key.into(), serial: 1, ..Default::default() }), developer: Some("Ada Lovelace".into()), ..Default::default() }
    }

    #[test]
    fn first_trust_then_a_changed_key_is_refused() {
        let dir = std::env::temp_dir().join(format!("nami-trust-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(check_in(&dir, &Meta { name: "jq".into(), ..Default::default() }), Trust::Unsigned);
        let a = meta("bundle-1:AAAA");
        assert_eq!(check_in(&dir, &a), Trust::New);
        assert_eq!(record_in(&dir, &a).unwrap(), Trust::New);
        assert_eq!(check_in(&dir, &a), Trust::Same);
        // Another key for the same app: refused, and the first one kept.
        let b = meta("bundle-2:BBBB");
        assert_eq!(check_in(&dir, &b), Trust::Changed { was: "bundle-1:AAAA".into() });
        assert_eq!(record_in(&dir, &b).unwrap(), Trust::Changed { was: "bundle-1:AAAA".into() });
        assert_eq!(trusted_in(&dir, &a).as_deref(), Some("bundle-1:AAAA"));
        let said = refusal(&b, "AAAA 1111 2222 3333 4444", "BBBB 5555 6666 7777 8888", "remove myapp first");
        assert!(said.contains("another key (BBBB 5555 6666 7777) than the one you trusted before (AAAA 1111 2222 3333). It may not be from Ada Lovelace"), "{said}");
        assert!(said.ends_with("If its developer really changed their key, remove myapp first."), "{said}");
        fs::remove_dir_all(&dir).unwrap();
    }
}
