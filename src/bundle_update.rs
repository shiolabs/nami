//! Self-updating bundles (`nami bundle --update-url <url>`): shared by nami,
//! which signs and publishes, and nami-bundle-runtime-net, which checks,
//! downloads and swaps.
//!
//! A bundle made so carries, in its description (which its hash covers),
//! where its developer publishes new versions and the developer's public
//! key (ed25519, as nami signs its catalogs and caches). `nami bundle
//! --publish-dir <dir>` writes what goes there, every file at the top, ready
//! for any static host or a GitHub release:
//!
//! ```text
//! update.json          the feed: name, version, serial and the new
//!                      description's sha256 (the bundle's hash), signed
//! <sha256>.meta.json   that description
//! <sha256>.nar.zst     each part the bundle carries: its frame, as it is
//! runtime-<sha256>     the runtime in front
//! <file>               the whole bundle, for new downloads
//! ```
//!
//! The runtime looks at the feed at most once a day, from a process of its
//! own, so the program starts as fast as ever. A newer version is put
//! together beside the bundle (`.<file>.nami-update`) byte for byte as the
//! developer made it: frames it has already (in itself, or in another of the
//! person's bundles) are copied, only the rest downloaded, and every piece
//! is checked against the signed description's sha256s before it's used. At
//! the next start it replaces the bundle (a rename: all or nothing) and runs.
//! The parts it fetches from cache.nixos.org (a thin bundle's) come, checked,
//! when the new version first starts, as they always do.

// Shared by nami and the runtime: each uses its own half.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bundle_delta::{self as delta, Frames, Patch};
use crate::bundle_format::{self, Meta, Rotation, Trailer};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// The feed's file name, in the folder the bundle names.
pub const FEED: &str = "update.json";
/// How often the feed is looked at.
const DAY: u64 = 24 * 3600;
/// The most a description may be (gnome-text-editor's is 40 KiB).
const MAX_META: u64 = 64 << 20;

/// `update.json`: what the newest version is, signed by the developer.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Feed {
    pub format: u32,
    pub name: String,
    pub version: String,
    /// The new version's `Update::serial`: newer is larger.
    pub serial: u64,
    /// The new description's sha256, hex: the bundle's hash.
    pub sha256: String,
    /// The whole bundle's size, and its file name in the same folder.
    pub size: u64,
    pub file: String,
    /// `<key name>:<base64>`, over `message(name, serial, sha256)`.
    pub signature: String,
    /// The developer's key handovers (Update::rotations): a copy that
    /// trusts an earlier key follows them to the one that signed this.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rotations: Vec<Rotation>,
    /// Difference-only updates (src/bundle_delta.rs): what made the frames,
    /// the patches beside the parts, and the developer's signature over
    /// them. Older runtimes ignore them and download whole parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frames: Option<Frames>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patches: Vec<Patch>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub patches_signature: String,
}

/// What a key signs to hand over to the next.
pub fn rotation_message(from: &str, to: &str) -> Vec<u8> {
    format!("nami-bundle-key-rotation-v1\n{from}\n{to}\n").into_bytes()
}

/// `from` (the old key, whose secret half is `old`) hands over to `to`.
pub fn sign_rotation(old: &SigningKey, from: &str, to: &str) -> Rotation {
    let name = from.split(':').next().unwrap_or_default();
    Rotation { from: from.into(), to: to.into(), signature: format!("{name}:{}", B64.encode(old.sign(&rotation_message(from, to)).to_bytes())) }
}

/// `signature` (`name:base64`) of `msg` is by `public` (`name:base64`).
fn signed_by(public: &str, msg: &[u8], signature: &str) -> bool {
    let (Some((_, kb64)), Some((_, sb64))) = (public.split_once(':'), signature.split_once(':')) else { return false };
    let key: Option<[u8; 32]> = B64.decode(kb64).ok().and_then(|k| k.try_into().ok());
    let sig: Option<[u8; 64]> = B64.decode(sb64).ok().and_then(|s| s.try_into().ok());
    match (key.and_then(|k| VerifyingKey::from_bytes(&k).ok()), sig) {
        (Some(k), Some(s)) => k.verify_strict(msg, &Signature::from_bytes(&s)).is_ok(),
        _ => false,
    }
}

/// The keys `trusted` hands over to, in order, through rotations each signed
/// by the key before it (`trusted` itself first).
pub fn handed_over(trusted: &str, chain: &[Rotation]) -> Vec<String> {
    let mut keys = vec![trusted.to_string()];
    while keys.len() <= chain.len() {
        let cur = keys.last().unwrap().clone();
        let Some(r) = chain.iter().find(|r| r.from == cur && !keys.contains(&r.to) && signed_by(&r.from, &rotation_message(&r.from, &r.to), &r.signature)) else { break };
        keys.push(r.to.clone());
    }
    keys
}

/// `to` is `trusted`, or a key it handed over to.
pub fn rotated_to(trusted: &str, to: &str, chain: &[Rotation]) -> bool {
    handed_over(trusted, chain).iter().any(|k| k == to)
}

/// The same, from a key known by its fingerprint (user.nami's `key:`).
pub fn rotated_from_fingerprint(fp: &str, to: &str, chain: &[Rotation]) -> bool {
    let norm = |f: &str| f.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>().to_uppercase();
    let want = norm(fp);
    let starts = std::iter::once(to).chain(chain.iter().map(|r| r.from.as_str()));
    starts.filter(|k| fingerprint(k).is_some_and(|f| norm(&f) == want)).any(|k| rotated_to(k, to, chain))
}

/// The key a bundle's key is now (`key`, or one it handed over to) that
/// signed `feed`.
pub fn feed_signer(key: &str, feed: &Feed) -> Result<String> {
    if feed.verify(key).is_ok() {
        return Ok(key.to_string());
    }
    for k in handed_over(key, &feed.rotations).into_iter().skip(1) {
        if feed.verify(&k).is_ok() {
            return Ok(k);
        }
    }
    feed.verify(key).map(|_| key.to_string())
}

/// The key trusted for this bundle's app here (src/bundle_trust.rs), with
/// its developer's rotations: a key the trusted one handed over to is
/// trusted in its place, silently.
pub fn trust_in(dir: &Path, meta: &Meta) -> crate::bundle_trust::Trust {
    use crate::bundle_trust::{self as trust, Trust};
    match (trust::check_in(dir, meta), &meta.update) {
        (Trust::Changed { was }, Some(u)) if rotated_to(&was, &u.key, &u.rotations) => {
            let _ = trust::replace_in(dir, meta);
            Trust::Same
        }
        (t, _) => t,
    }
}

/// The same, trusting a key seen for the first time (its first start, its install).
pub fn settle_in(dir: &Path, meta: &Meta) -> std::io::Result<crate::bundle_trust::Trust> {
    match trust_in(dir, meta) {
        crate::bundle_trust::Trust::New => crate::bundle_trust::record_in(dir, meta),
        t => Ok(t),
    }
}

/// What the developer's key signs.
pub fn message(name: &str, serial: u64, sha256: &str) -> Vec<u8> {
    format!("tray-bundle-update-v1\n{name}\n{serial}\n{sha256}\n").into_bytes()
}

impl Feed {
    pub fn sign(&mut self, key_name: &str, key: &SigningKey) {
        let sig = key.sign(&message(&self.name, self.serial, &self.sha256));
        self.signature = format!("{key_name}:{}", B64.encode(sig.to_bytes()));
    }

    /// Sign its patches (`frames` and `patches` set) with the key that signs it.
    pub fn sign_patches(&mut self, key_name: &str, key: &SigningKey) {
        let frames = self.frames.clone().unwrap_or_default();
        let sig = key.sign(&delta::message(&self.name, self.serial, &self.sha256, &frames, &self.patches));
        self.patches_signature = format!("{key_name}:{}", B64.encode(sig.to_bytes()));
    }

    /// Its patches, if `signer` (the key that signed the feed) signed them
    /// and this runtime compresses frames as they were made: else none.
    pub fn usable_patches(&self, signer: &str) -> &[Patch] {
        let Some(frames) = self.frames.as_ref().filter(|f| f.same_here()) else { return &[] };
        if self.patches.is_empty() || !signed_by(signer, &delta::message(&self.name, self.serial, &self.sha256, frames, &self.patches), &self.patches_signature) {
            return &[];
        }
        &self.patches
    }

    /// Signed by `public` (`name:base64`, the bundle's developer)?
    pub fn verify(&self, public: &str) -> Result<()> {
        let (kname, kb64) = public.split_once(':').context("the bundle's update key is malformed")?;
        let (sname, sb64) = self.signature.split_once(':').context("the feed isn't signed")?;
        ensure!(sname == kname, "the feed is signed by the key {sname:?}, not by this bundle's developer's ({kname:?})");
        let key: [u8; 32] = B64.decode(kb64).ok().and_then(|k| k.try_into().ok()).context("the bundle's update key is malformed")?;
        let sig: [u8; 64] = B64.decode(sb64).ok().and_then(|s| s.try_into().ok()).context("the feed's signature is malformed")?;
        VerifyingKey::from_bytes(&key)?
            .verify_strict(&message(&self.name, self.serial, &self.sha256), &Signature::from_bytes(&sig))
            .map_err(|_| anyhow!("the feed's signature doesn't match this bundle's developer's key: it was changed, or signed by someone else"))
    }
}

/// A key's public half, `name:base64`.
pub fn public_key(name: &str, key: &SigningKey) -> String {
    format!("{name}:{}", B64.encode(key.verifying_key().to_bytes()))
}

/// A public key (`name:base64`) as people compare it: the first 16 bytes
/// of its SHA-256, in groups of four hex digits (`3F2A 9C01 …`).
pub fn fingerprint(public: &str) -> Option<String> {
    let (_, b64) = public.split_once(':')?;
    let bytes = B64.decode(b64.trim()).ok().filter(|b| b.len() == 32)?;
    let h = sha256_hex(&bytes).to_uppercase();
    Some(h.as_bytes()[..32].chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" "))
}

/// `github:<owner>/<repo>` is that repository's latest release; anything
/// else is the folder's URL as it is.
pub fn feed_url(url: &str) -> String {
    match url.strip_prefix("github:") {
        Some(repo) => format!("https://github.com/{}/releases/latest/download", repo.trim_matches('/')),
        None => url.trim_end_matches('/').to_string(),
    }
}

pub fn part_file(sha256: &str) -> String {
    format!("{sha256}.nar.zst")
}

pub fn meta_file(sha256: &str) -> String {
    format!("{sha256}.meta.json")
}

pub fn runtime_file(sha256: &str) -> String {
    format!("runtime-{sha256}")
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn sha256_hex(b: &[u8]) -> String {
    bundle_format::sha256_hex(b)
}

// ---------------------------------------------------------------------------
// The runtime's half. src/bundle_runtime.rs calls these four (the runtime
// that can't fetch has its own, which do nothing).

/// At start: a newer version an earlier check got ready replaces the
/// bundle. Its version, if so: the caller starts the new file instead.
pub fn at_start(path: &Path, meta: &Meta) -> Option<String> {
    if !wanted(meta) || std::env::var_os("NAMI_BUNDLE_UPDATED").is_some() {
        return None;
    }
    swap(path, meta)
}

/// `--nami-bundle-update` (look now, and update), `--nami-bundle-updates
/// on|off`.
pub fn command(args: &[String], me: &Me) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("--nami-bundle-updates") => {
            let on = match args.get(1).map(String::as_str) {
                Some("on") => true,
                Some("off") => false,
                _ => bail!("--nami-bundle-updates on, or off"),
            };
            println!("{}", set_wanted(me.meta, on)?);
            Ok(())
        }
        _ => {
            ensure!(me.meta.update.is_some(), "{} doesn't update itself (it was made without --update-url)", me.meta.name);
            let current = if me.meta.version.is_empty() { me.hash[..12].to_string() } else { me.meta.version.clone() };
            match me.check(delta::Choice::for_update(true))? {
                Outcome::UpToDate => println!("{} {current} is the newest ({})", me.meta.name, me.update().feed),
                Outcome::ReadOnly { version, file, why, .. } => {
                    println!("{} {version} is out, but this copy can't update itself: {why}. Download it from {}, or keep the file in a folder of yours.", me.meta.name, me.url(&file))
                }
                Outcome::Staged { version, parts, downloaded, bytes, patched, .. } => {
                    match swap(me.path, me.meta) {
                        Some(_) => println!("{}: updated {} from {current} to {version}", me.path.display(), me.meta.name),
                        None => bail!("{version} was downloaded, but couldn't replace {}", me.path.display()),
                    }
                    let size = if bytes < 1 << 20 { format!("{:.0} KiB", bytes as f64 / 1024.0) } else { format!("{:.1} MiB", bytes as f64 / 1048576.0) };
                    let only = match patched {
                        0 => String::new(),
                        n if n == downloaded => ", only what changed in each".to_string(),
                        n => format!(", only what changed in {n} of them"),
                    };
                    println!("downloaded {downloaded} of its {parts} parts ({size}{only}); the rest it had already");
                }
            }
            Ok(())
        }
    }
}

/// For --nami-bundle-info.
pub fn info(me: &Me) -> Vec<String> {
    if me.meta.update.is_none() {
        return Vec::new();
    }
    me.info()
}

/// Just before the program starts: notices, and the day's look at the feed
/// in the background.
pub fn before_start(me: &Me, lock_fd: i32) {
    if !wanted(me.meta) {
        return;
    }
    me.notices();
    me.background(lock_fd);
}

/// The bundle that's running, as the update code needs it.
pub struct Me<'a> {
    /// The bundle's file.
    pub path: &'a Path,
    pub meta: &'a Meta,
    /// Its hash (its description's sha256).
    pub hash: &'a str,
    /// The bundle cache's folder (~/.cache/tray-bundle).
    pub cache: &'a Path,
}

/// What a check found.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    UpToDate,
    /// The newer version is beside the bundle, ready for the next start.
    Staged { version: String, serial: u64, parts: usize, downloaded: usize, bytes: u64, patched: usize },
    /// There is one, but the bundle's file can't be replaced (`why`).
    ReadOnly { version: String, serial: u64, file: String, why: String },
}

/// What's remembered between checks, in `<cache>/updates/<id>.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    /// When the feed was last looked at (s since 1970).
    checked: u64,
    #[serde(default)]
    error: String,
    #[serde(default)]
    available: Option<Available>,
    /// The serial whose "can't replace" the person was told of.
    #[serde(default)]
    told: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Available {
    version: String,
    serial: u64,
    file: String,
    /// Non-empty: why the bundle can't be replaced.
    #[serde(default)]
    why: String,
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
}

/// Updates turned off for this app (every copy of it): this file is there,
/// in `tray-bundle/updates-off` (bundles made before the rename read it
/// there) or `nami-bundle/updates-off` (0.18.6 to 0.18.8's): both are
/// written, and either is heeded.
fn off_markers(meta: &Meta) -> [PathBuf; 2] {
    let feed = meta.update.as_ref().map(|u| u.feed.as_str()).unwrap_or("");
    let file = format!("{}-{}", safe(&meta.name), &sha256_hex(feed.as_bytes())[..8]);
    ["nami-bundle", "tray-bundle"].map(|d| config_home().join(d).join("updates-off").join(&file))
}

fn off_marker_there(meta: &Meta) -> bool {
    off_markers(meta).iter().any(|m| m.exists())
}

fn safe(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect()
}

/// NAMI_BUNDLE_UPDATE=0 (or off, no, false) turns updates off for a run.
fn env_off() -> bool {
    std::env::var("NAMI_BUNDLE_UPDATE").is_ok_and(|v| ["0", "off", "no", "false"].contains(&v.trim().to_ascii_lowercase().as_str()))
}

/// This bundle updates itself, and the person hasn't turned that off (nor
/// has the computer).
pub fn wanted(meta: &Meta) -> bool {
    meta.update.is_some() && !env_off() && !off_marker_there(meta) && !system_says_off()
}

/// On namiOS, its daily updates turned off (Settings' "Update by itself,
/// every day": `without: [nami-update.timer]`): bundles don't look for
/// updates either. The timer's enabling link is the switch.
pub fn system_says_off() -> bool {
    system_says_off_at(Path::new("/"))
}

fn system_says_off_at(root: &Path) -> bool {
    // (tray-update.timer: a computer whose packages are still tray's.)
    let timers = root.join("etc/systemd/system/timers.target.wants");
    (root.join("etc/nami-os").is_dir() || root.join("etc/tray-os").is_dir()) && !timers.join("nami-update.timer").exists() && !timers.join("tray-update.timer").exists()
}

/// `--nami-bundle-updates on|off`: for this app, until said otherwise.
pub fn set_wanted(meta: &Meta, on: bool) -> Result<String> {
    ensure!(meta.update.is_some(), "{} doesn't update itself (it was made without --update-url)", meta.name);
    let [marker, tray_marker] = off_markers(meta);
    if on {
        let _ = fs::remove_file(&marker);
        let _ = fs::remove_file(&tray_marker);
        Ok(format!("updates on for {}{}", meta.name, if env_off() { " (but NAMI_BUNDLE_UPDATE turns them off here)" } else { "" }))
    } else {
        for m in [&marker, &tray_marker] {
            fs::create_dir_all(m.parent().unwrap())?;
            fs::write(m, format!("{}\n", meta.update.as_ref().unwrap().feed))?;
        }
        Ok(format!("updates off for {} (on again with --nami-bundle-updates on; {})", meta.name, marker.display()))
    }
}

/// Where a newer version waits for the next start, beside the bundle.
pub fn staged(path: &Path) -> PathBuf {
    staged_as(path, "nami")
}

/// The same, as a runtime of `who`'s names left it (`.<file>.tray-update`:
/// one made before the rename).
fn staged_as(path: &Path, who: &str) -> PathBuf {
    path.with_file_name(format!(".{}.{who}-update", path.file_name().unwrap_or_default().to_string_lossy()))
}

/// The feed that vouches for it.
fn staged_feed(staged: &Path) -> PathBuf {
    let mut f = staged.as_os_str().to_owned();
    f.push(".feed");
    PathBuf::from(f)
}

/// A newer version an earlier check put together: checked again (its feed
/// signed by this bundle's key, over its hash; newer; the same app), then
/// moved over the bundle. The new version's, if it was. (One the bundle's
/// runtime before the rename left is looked at too.)
pub fn swap(path: &Path, meta: &Meta) -> Option<String> {
    for who in ["nami", "tray"] {
        let staged = staged_as(path, who);
        if staged.symlink_metadata().is_err() {
            continue;
        }
        match try_swap(path, meta, &staged) {
            Ok(v) => return Some(v),
            Err(e) => {
                if std::env::var_os("NAMI_BUNDLE_DEBUG").is_some_and(|v| !v.is_empty() && v != "0") {
                    eprintln!("nami-bundle: not updating to {}: {e:#}", staged.display());
                }
                let _ = fs::remove_file(&staged);
                let _ = fs::remove_file(staged_feed(&staged));
            }
        }
    }
    None
}

fn try_swap(path: &Path, meta: &Meta, staged: &Path) -> Result<String> {
    let u = meta.update.as_ref().context("this bundle doesn't update itself")?;
    let feed: Feed = serde_json::from_slice(&fs::read(staged_feed(staged)).context("its feed")?).context("its feed")?;
    feed_signer(&u.key, &feed)?;
    ensure!(feed.name == meta.name, "it's {}, not {}", feed.name, meta.name);
    ensure!(feed.serial > u.serial, "it isn't newer");
    let mut f = File::open(staged)?;
    let (trailer, new) = bundle_format::read(&mut f)?;
    ensure!(trailer.hash() == feed.sha256, "it isn't what the feed signed");
    ensure!(new.name == meta.name, "it's {}, not {}", new.name, meta.name);
    same_key(meta, &new)?;
    fs::rename(staged, path).with_context(|| format!("replacing {}", path.display()))?;
    let _ = fs::remove_file(staged_feed(staged));
    Ok(new.version)
}

/// A new version keeps the key the person trusts: one that names another is
/// refused, even signed with the old one (src/bundle_trust.rs).
fn same_key(now: &Meta, new: &Meta) -> Result<()> {
    if let (Some(u), Some(nu)) = (&now.update, &new.update) {
        ensure!(
            nu.key == u.key || rotated_to(&u.key, &nu.key, &nu.rotations),
            "the update of {} would change the key that signs it (to {}), so it wasn't installed: download it again from its developer if they really changed their key",
            now.name,
            fingerprint(&nu.key).map(|f| crate::bundle_trust::short(&f)).unwrap_or_default()
        );
    }
    Ok(())
}

/// Updates are taken only for the key the person trusts for the app: a copy
/// signed with another isn't updated (it may not be from the same developer).
fn trusted(meta: &Meta) -> Result<()> {
    if let (Some(u), crate::bundle_trust::Trust::Changed { was }) = (&meta.update, trust_in(&crate::bundle_trust::dir(), meta)) {
        bail!(
            "{} is signed with another key ({}) than the one you trusted before ({}), so it isn't updated",
            meta.name,
            fingerprint(&u.key).map(|f| crate::bundle_trust::short(&f)).unwrap_or_default(),
            fingerprint(&was).map(|f| crate::bundle_trust::short(&f)).unwrap_or_default()
        );
    }
    Ok(())
}

/// What the bundle's own checks found, as the last one left it (for
/// namiOS's Settings, src/bundle_app.rs).
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct LastLook {
    /// When (s since 1970); 0: never.
    pub checked: u64,
    pub error: Option<String>,
    /// A newer version: ready for the next start, or (`why`) not.
    pub newer: Option<String>,
    pub why: Option<String>,
}

impl Me<'_> {
    /// What the last look at the feed found.
    pub fn last_look(&self) -> LastLook {
        let s = self.state();
        let serial = self.update().serial;
        let newer = s.available.filter(|a| a.serial > serial);
        let staged = staged(self.path).exists();
        LastLook {
            checked: s.checked,
            error: Some(s.error).filter(|e| !e.is_empty()),
            why: newer.as_ref().map(|a| a.why.clone()).filter(|w| !w.is_empty()),
            newer: newer.filter(|a| !a.why.is_empty() || staged).map(|a| a.version),
        }
    }

    /// The files the bundle's checks keep for this copy.
    pub fn state_files(&self) -> Vec<PathBuf> {
        let id = self.id();
        vec![self.dir().join(format!("{id}.json")), self.dir().join(format!("{id}.lock"))]
    }

    fn update(&self) -> &bundle_format::Update {
        self.meta.update.as_ref().expect("an updating bundle")
    }

    /// This copy of the bundle: its file and where it updates from.
    fn id(&self) -> String {
        let h = sha256_hex(format!("{}\n{}", self.path.display(), self.update().feed).as_bytes());
        format!("{}-{}", safe(&self.meta.name), &h[..12])
    }

    fn dir(&self) -> PathBuf {
        self.cache.join("updates")
    }

    fn state(&self) -> State {
        fs::read(self.dir().join(format!("{}.json", self.id()))).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
    }

    fn save(&self, s: &State) {
        let dir = self.dir();
        let _ = fs::create_dir_all(&dir);
        let file = dir.join(format!("{}.json", self.id()));
        let tmp = file.with_extension(format!("tmp{}", std::process::id()));
        if fs::write(&tmp, serde_json::to_vec(s).unwrap_or_default()).is_ok() {
            let _ = fs::rename(&tmp, &file);
        }
    }

    /// Why the bundle's file can't be replaced, if it can't.
    fn read_only(&self) -> Option<String> {
        let dir = self.path.parent().unwrap_or(Path::new("/"));
        let c = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(dir.as_os_str())).ok()?;
        // SAFETY: a valid C string.
        if unsafe { libc::access(c.as_ptr(), libc::W_OK | libc::X_OK) } != 0 {
            let e = io::Error::last_os_error();
            return Some(if e.raw_os_error() == Some(libc::EROFS) { format!("{} is on a read-only file system", dir.display()) } else { format!("{} isn't a folder you can write to", dir.display()) });
        }
        // A shared folder like /tmp: only its owner may replace a file.
        let (dm, fm) = (fs::metadata(dir).ok()?, fs::metadata(self.path).ok()?);
        // SAFETY: getuid can't fail.
        let me = unsafe { libc::getuid() };
        if dm.mode() & 0o1000 != 0 && fm.uid() != me && dm.uid() != me {
            return Some(format!("{} is someone else's, in a shared folder", self.path.display()));
        }
        None
    }

    /// The feed's URL, and a file's beside it.
    fn url(&self, file: &str) -> String {
        format!("{}/{file}", self.update().feed.trim_end_matches('/'))
    }

    /// Lines for --nami-bundle-info.
    pub fn info(&self) -> Vec<String> {
        let u = self.update();
        let mut out = vec![format!("updates:  from {} (signed by {}, {})", u.feed, u.key.split(':').next().unwrap_or(""), fingerprint(&u.key).unwrap_or_default())];
        if off_marker_there(self.meta) {
            out.push("          turned off (--nami-bundle-updates on turns them on)".into());
        } else if env_off() {
            out.push("          turned off here (NAMI_BUNDLE_UPDATE)".into());
        } else if system_says_off() {
            out.push("          turned off, with this computer's daily updates".into());
        }
        let s = self.state();
        if s.checked > 0 {
            let ago = now().saturating_sub(s.checked);
            let when = if ago < 7200 { format!("{} min ago", ago / 60) } else { format!("{} h ago", ago / 3600) };
            let what = match (&s.available, s.error.is_empty()) {
                (_, false) => format!("couldn't: {}", s.error),
                (Some(a), _) if a.serial > u.serial && !a.why.is_empty() => format!("{} is out, but {}", a.version, a.why),
                (Some(a), _) if a.serial > u.serial => format!("{} is ready for the next start", a.version),
                _ => "up to date".into(),
            };
            out.push(format!("          last looked {when}: {what}"));
        }
        out
    }

    /// At start: tell the person, once per version, of an update that
    /// can't be put in place.
    pub fn notices(&self) {
        let mut s = self.state();
        let Some(a) = s.available.clone() else { return };
        if a.why.is_empty() || a.serial <= self.update().serial || s.told == a.serial {
            return;
        }
        eprintln!("{}: version {} is out, but this copy can't update itself: {}. Download it from {}, or keep the file in a folder of yours.", self.meta.name, a.version, a.why, self.url(&a.file));
        s.told = a.serial;
        self.save(&s);
    }

    /// Time to look again (or the first time)?
    fn due(&self) -> bool {
        let every = std::env::var("NAMI_BUNDLE_UPDATE_INTERVAL").ok().and_then(|v| v.parse().ok()).unwrap_or(DAY);
        now().saturating_sub(self.state().checked) >= every
    }

    /// At start, when due: look at the feed from a process of its own (two
    /// forks, so it's no child of the program), and return at once. `fd`
    /// is the cache's lock, which that process lets go of.
    pub fn background(&self, fd: i32) {
        if !self.due() {
            return;
        }
        let mut s = self.state();
        s.checked = now();
        self.save(&s);
        // SAFETY: fork with one thread (the unpacking threads are done); the
        // children only call async-signal-safe functions until they exit,
        // apart from the check itself, which owns its process.
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
            // The child: drop its hold on the cache's lock (the program's
            // exec lets go of the other), then start the grandchild.
            libc::close(fd);
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
            libc::nice(10);
        }
        self.check_and_record();
        // SAFETY: the end of this process.
        unsafe { libc::_exit(0) }
    }

    /// Look, and remember what was found (one look at a time per copy).
    fn check_and_record(&self) {
        let _ = fs::create_dir_all(self.dir());
        let Ok(lock) = File::create(self.dir().join(format!("{}.lock", self.id()))) else { return };
        use std::os::fd::AsRawFd;
        // SAFETY: flock on a file we hold open.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return;
        }
        let result = self.check(delta::Choice::for_update(false));
        let mut s = self.state();
        s.checked = now();
        match result {
            Ok(Outcome::UpToDate) => (s.error, s.available) = (String::new(), None),
            Ok(Outcome::Staged { version, serial, .. }) => {
                s.error = String::new();
                s.available = Some(Available { version, serial, file: String::new(), why: String::new() });
            }
            Ok(Outcome::ReadOnly { version, serial, file, why }) => {
                s.error = String::new();
                s.available = Some(Available { version, serial, file, why });
            }
            Err(e) => s.error = format!("{e:#}"),
        }
        self.save(&s);
    }

    /// Look at the feed now; put a newer version together beside the
    /// bundle if there is one, with its patches as `choice` says.
    pub fn check(&self, choice: delta::Choice) -> Result<Outcome> {
        let u = self.update().clone();
        trusted(self.meta)?;
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(60))
            .user_agent(concat!("nami-bundle/", env!("CARGO_PKG_VERSION")))
            .build();
        let url = self.url(FEED);
        let data = get(&agent, &url, 1 << 20).with_context(|| format!("fetching {url}"))?;
        let feed: Feed = serde_json::from_slice(&data).with_context(|| format!("reading {url}"))?;
        let signer = feed_signer(&u.key, &feed).with_context(|| format!("{url} refused"))?;
        ensure!(feed.name == self.meta.name, "{url} is for {}, not {}", feed.name, self.meta.name);
        if feed.sha256 == self.hash || feed.serial <= u.serial {
            return Ok(Outcome::UpToDate);
        }
        if let Some(why) = self.read_only() {
            return Ok(Outcome::ReadOnly { version: feed.version, serial: feed.serial, file: feed.file, why });
        }
        // Put together already (an earlier check, not yet started)?
        let staged = staged(self.path);
        if File::open(&staged).ok().and_then(|mut f| bundle_format::read(&mut f).ok()).is_some_and(|(t, _)| t.hash() == feed.sha256) && staged_feed(&staged).exists() {
            return Ok(Outcome::Staged { version: feed.version, serial: feed.serial, parts: 0, downloaded: 0, bytes: 0, patched: 0 });
        }
        // The new description, whose hash the feed signed.
        let murl = self.url(&meta_file(&feed.sha256));
        let meta_bytes = get(&agent, &murl, MAX_META).with_context(|| format!("fetching {murl}"))?;
        ensure!(sha256_hex(&meta_bytes) == feed.sha256, "{murl} isn't the description the feed signed");
        let new: Meta = serde_json::from_slice(&meta_bytes).with_context(|| format!("reading {murl}"))?;
        ensure!(new.format == 1, "the new version is a bundle of format {}, newer than this runtime knows", new.format);
        ensure!(new.name == self.meta.name, "the new description is for {}", new.name);
        if let Some(nu) = &new.update {
            ensure!(nu.serial == feed.serial, "the new description's serial isn't the feed's");
        }
        same_key(self.meta, &new)?;
        ensure!(new.paths.iter().all(|p| bundle_format::valid_name(&p.name)), "the new description names a part unsafely");
        ensure!(!new.runtime.is_empty(), "the new description doesn't say which runtime it has");

        let dir = self.dir().join("parts");
        fs::create_dir_all(&dir)?;
        let tmp = self.path.with_file_name(format!(".{}.nami-update.tmp{}", self.path.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
        let result = self.assemble(&agent, &new, &meta_bytes, &feed, feed.usable_patches(&signer), choice, &tmp, &dir);
        let (parts, downloaded, bytes, patched) = match result {
            Ok(n) => n,
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                return Err(e);
            }
        };
        // The feed first: a staged file is always vouched for.
        let ftmp = self.path.with_file_name(format!(".{}.nami-update.feed.tmp{}", self.path.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
        fs::write(&ftmp, serde_json::to_vec(&feed)?)?;
        fs::rename(&ftmp, staged_feed(&staged))?;
        fs::rename(&tmp, &staged)?;
        let _ = fs::remove_dir_all(&dir);
        Ok(Outcome::Staged { version: feed.version, serial: feed.serial, parts, downloaded, bytes, patched })
    }

    /// The new bundle at `tmp`, byte for byte the developer's: (parts it
    /// carries, how many were downloaded, bytes downloaded, how many of
    /// those as patches).
    #[allow(clippy::too_many_arguments)]
    fn assemble(&self, agent: &ureq::Agent, new: &Meta, meta_bytes: &[u8], feed: &Feed, patches: &[Patch], choice: delta::Choice, tmp: &Path, dir: &Path) -> Result<(usize, usize, u64, usize)> {
        let local = local_frames(self.path, self.cache);
        let carried: Vec<&bundle_format::PathEntry> = new.paths.iter().filter(|p| !p.fetch).collect();
        let mut missing: Vec<&bundle_format::PathEntry> = carried.iter().copied().filter(|p| !local.contains_key(&p.sha256)).collect();
        // What changed from a part it has: only the difference, where the
        // feed has a patch. Four at a time, as much as fits in memory.
        let (mut patch_bytes, mut patched) = (0, 0);
        let frames = feed.frames.as_ref().filter(|_| !patches.is_empty());
        if let Some(frames) = frames.filter(|f| self.patches_wanted(agent, &missing, patches, f, &local, choice)) {
            let budget = delta::Budget::new(delta::MAX_MEMORY);
            let made: Vec<std::sync::Mutex<Option<u64>>> = missing.iter().map(|_| std::sync::Mutex::new(None)).collect();
            let next = std::sync::atomic::AtomicUsize::new(0);
            std::thread::scope(|s| {
                for _ in 0..4.min(missing.len()) {
                    s.spawn(|| loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(p) = missing.get(i) else { break };
                        *made[i].lock().unwrap() = self.patched(agent, p, patches, frames, &local, dir, &budget);
                    });
                }
            });
            let mut made = made.into_iter().map(|m| m.into_inner().unwrap());
            missing.retain(|_| match made.next().flatten() {
                Some(n) => {
                    patch_bytes += n;
                    patched += 1;
                    false
                }
                None => true,
            });
        }
        // The rest: downloaded whole, four at a time, each checked.
        let errors: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let next = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..4.min(missing.len()) {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(p) = missing.get(i) else { break };
                    if let Err(e) = download(agent, &self.url(&part_file(&p.sha256)), &dir.join(part_file(&p.sha256)), p.size, &p.sha256) {
                        errors.lock().unwrap().push(format!("{}: {e:#}", p.name));
                    }
                });
            }
        });
        let errors = errors.into_inner().unwrap();
        if !errors.is_empty() {
            bail!("downloading the new version's parts: {}", errors.join("; "));
        }
        let mut downloaded: u64 = missing.iter().map(|p| p.size).sum::<u64>() + patch_bytes;
        let mut count = missing.len() + patched;

        let mut out = io::BufWriter::new(File::create(tmp).with_context(|| format!("writing {}", tmp.display()))?);
        // The runtime: this bundle's own if it's the same, else downloaded.
        let (old_trailer, old_meta) = bundle_format::read(&mut File::open(self.path)?)?;
        let mut runtime = None;
        if old_meta.runtime == new.runtime {
            runtime = read_checked(self.path, 0, old_trailer.payload_offset, &new.runtime).ok();
        }
        let runtime = match runtime {
            Some(r) => r,
            None => {
                let url = self.url(&runtime_file(&new.runtime));
                let r = get(agent, &url, 64 << 20).with_context(|| format!("fetching {url}"))?;
                ensure!(sha256_hex(&r) == new.runtime, "{url} isn't the runtime the new description names");
                downloaded += r.len() as u64;
                r
            }
        };
        out.write_all(&runtime)?;
        let payload_offset = runtime.len() as u64;
        let mut at = payload_offset;
        for p in &carried {
            ensure!(p.offset == at, "the new description's parts don't add up ({})", p.name);
            let file = dir.join(part_file(&p.sha256));
            let copied = match local.get(&p.sha256) {
                Some((src, offset)) => copy_checked(src, *offset, p.size, &p.sha256, &mut out).is_ok(),
                None => false,
            };
            if !copied {
                if !file.exists() {
                    // The local copy turned out damaged: download it after all.
                    download(agent, &self.url(&part_file(&p.sha256)), &file, p.size, &p.sha256).with_context(|| format!("downloading {}", p.name))?;
                    downloaded += p.size;
                    count += 1;
                }
                copy_checked(&file, 0, p.size, &p.sha256, &mut out).with_context(|| format!("{} (downloaded)", p.name))?;
            }
            at += p.size;
        }
        out.write_all(meta_bytes)?;
        let trailer = Trailer { payload_offset, meta_offset: at, meta_len: meta_bytes.len() as u64, meta_sha256: Sha256::digest(meta_bytes).into() };
        ensure!(trailer.hash() == feed.sha256, "the new version doesn't come out as the feed signed");
        out.write_all(&trailer.encode())?;
        let f = out.into_inner().map_err(|e| e.into_error())?;
        f.sync_all()?;
        let mode = fs::metadata(self.path).map(|m| m.permissions().mode() & 0o7777).unwrap_or(0o755);
        f.set_permissions(fs::Permissions::from_mode(mode | 0o500))?;
        Ok((carried.len(), count, downloaded, patched))
    }

    /// The part `p` made from a patch against a part this computer has,
    /// into `dir`'s part file, checked against the signed description: the
    /// patch's size, if so. None: it's downloaded whole instead.
    /// Whether to use the patches, as `choice` says: for `Auto`, whichever
    /// way looks faster, from the download speed (measured on the start of
    /// the biggest part it would download whole) and how fast this computer
    /// compresses (measured on a little of a part it has).
    fn patches_wanted(&self, agent: &ureq::Agent, missing: &[&bundle_format::PathEntry], patches: &[Patch], frames: &Frames, local: &HashMap<String, (PathBuf, u64)>, choice: delta::Choice) -> bool {
        match choice {
            delta::Choice::Always => return true,
            delta::Choice::Never => return false,
            delta::Choice::Auto => {}
        }
        let found: Vec<_> = missing.iter().filter_map(|p| Some((*p, patch_for(p, patches, local)?))).collect();
        let Some((biggest, (_, src, offset, old))) = found.iter().max_by_key(|(p, _)| p.size) else { return false };
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(4);
        let e = delta::Estimate {
            speed: speed(agent, &self.url(&part_file(&biggest.sha256))),
            rate: compress_rate(src, *offset, old, frames.level).unwrap_or_else(|| delta::careful_rate(frames.level)),
            threads,
            patch_bytes: found.iter().map(|(_, (x, ..))| x.size).sum(),
            whole_bytes: found.iter().map(|(p, _)| p.size).sum(),
            nar_bytes: found.iter().map(|(p, _)| p.nar_size).sum(),
        };
        if std::env::var_os("NAMI_BUNDLE_DEBUG").is_some_and(|v| !v.is_empty() && v != "0") {
            eprintln!("nami-bundle: {e:?}: {:?} s (patches, whole)", e.seconds());
        }
        e.patches_faster()
    }

    #[allow(clippy::too_many_arguments)]
    fn patched(&self, agent: &ureq::Agent, p: &bundle_format::PathEntry, patches: &[Patch], frames: &Frames, local: &HashMap<String, (PathBuf, u64)>, dir: &Path, budget: &delta::Budget) -> Option<u64> {
        let (patch, src, offset, old) = patch_for(p, patches, local)?;
        let dest = dir.join(part_file(&p.sha256));
        let _held = budget.take(delta::memory(old.nar_size, p.nar_size));
        let result = (|| -> Result<()> {
            let old_nar = delta::decompress(&read_checked(&src, offset, old.size, &old.sha256)?, old.nar_size)?;
            let file = dir.join(patch.file());
            download(agent, &self.url(&patch.file()), &file, patch.size, &patch.sha256)?;
            let tmp = dest.with_extension(format!("tmp{}", std::process::id()));
            let made = File::create(&tmp).and_then(|out| delta::apply(&old_nar, File::open(&file)?, p.nar_size, frames, io::BufWriter::new(out)));
            let _ = fs::remove_file(&file);
            match made {
                Ok((sha, size)) if sha == p.sha256 && size == p.size => Ok(fs::rename(&tmp, &dest)?),
                other => {
                    let _ = fs::remove_file(&tmp);
                    match other {
                        Err(e) => Err(e.into()),
                        Ok(_) => Err(anyhow!("it doesn't come out as the signed description says")),
                    }
                }
            }
        })();
        match result {
            Ok(()) => Some(patch.size),
            Err(e) => {
                if std::env::var_os("NAMI_BUNDLE_DEBUG").is_some_and(|v| !v.is_empty() && v != "0") {
                    eprintln!("nami-bundle: {} whole, not from its patch: {e:#}", p.name);
                }
                None
            }
        }
    }
}

/// The patch that makes `p` from a part this computer has: the patch, the
/// file and offset of that part's frame, and its entry.
fn patch_for<'a>(p: &bundle_format::PathEntry, patches: &'a [Patch], local: &HashMap<String, (PathBuf, u64)>) -> Option<(&'a Patch, PathBuf, u64, bundle_format::PathEntry)> {
    patches.iter().filter(|x| x.to == p.sha256 && x.size < p.size).find_map(|x| {
        let (src, offset) = local.get(&x.from)?;
        let old = local_entry(src, &x.from)?;
        (delta::memory(old.nar_size, p.nar_size) <= delta::MAX_MEMORY).then(|| (x, src.clone(), *offset, old))
    })
}

/// The download speed from `url` (bytes a second): over up to 1 MiB or two
/// seconds after its first bytes, at least 64 KiB of it; else None.
fn speed(agent: &ureq::Agent, url: &str) -> Option<f64> {
    let mut r = agent.get(url).call().ok()?.into_reader();
    let mut buf = vec![0u8; 1 << 16];
    r.read(&mut buf).ok().filter(|n| *n > 0)?;
    let start = std::time::Instant::now();
    let mut n = 0u64;
    while n < 1 << 20 && start.elapsed() < Duration::from_secs(2) {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(k) => n += k as u64,
        }
    }
    let secs = start.elapsed().as_secs_f64();
    (n >= 64 << 10 && secs > 0.0).then(|| n as f64 / secs)
}

/// How fast this computer compresses a part again (bytes of NAR a second,
/// one thread): 2 MiB of the old part `old`, as frames are compressed,
/// counted at four fifths, to be safe.
fn compress_rate(file: &Path, offset: u64, old: &bundle_format::PathEntry, level: i32) -> Option<f64> {
    let mut f = File::open(file).ok()?;
    f.seek(SeekFrom::Start(offset)).ok()?;
    let mut sample = Vec::new();
    zstd::stream::read::Decoder::new(f.take(old.size)).ok()?.take(2 << 20).read_to_end(&mut sample).ok()?;
    if sample.len() < 256 << 10 {
        return None;
    }
    let start = std::time::Instant::now();
    let mut enc = delta::frame_encoder(io::sink(), level, sample.len() as u64).ok()?;
    enc.write_all(&sample).ok()?;
    enc.finish().ok()?;
    Some(sample.len() as f64 / start.elapsed().as_secs_f64().max(1e-3) * 0.8)
}

/// The entry of the part whose frame is `sha256`, in the bundle `file`.
fn local_entry(file: &Path, sha256: &str) -> Option<bundle_format::PathEntry> {
    let (_, meta) = bundle_format::read(&mut File::open(file).ok()?).ok()?;
    meta.paths.into_iter().find(|p| !p.fetch && p.sha256 == sha256)
}

/// Frames this computer has: in the bundle itself, and in the other bundles
/// the cache knows of, by their sha256: (file, offset).
fn local_frames(me: &Path, cache: &Path) -> HashMap<String, (PathBuf, u64)> {
    let mut files = vec![me.to_path_buf()];
    for e in fs::read_dir(cache.join("bundles")).into_iter().flatten().flatten() {
        let text = fs::read_to_string(e.path()).unwrap_or_default();
        let mut lines = text.lines();
        let (path, _, kind) = (lines.next().unwrap_or(""), lines.next(), lines.next().unwrap_or(""));
        if kind == "file" && Path::new(path) != me {
            files.push(PathBuf::from(path));
        }
    }
    let mut out = HashMap::new();
    for f in files {
        let Ok(mut file) = File::open(&f) else { continue };
        let Ok((_, meta)) = bundle_format::read(&mut file) else { continue };
        for p in meta.paths.iter().filter(|p| !p.fetch) {
            out.entry(p.sha256.clone()).or_insert((f.clone(), p.offset));
        }
    }
    out
}

/// `size` bytes of `file` from `offset`, if their sha256 is `sha256`.
fn read_checked(file: &Path, offset: u64, size: u64, sha256: &str) -> Result<Vec<u8>> {
    let mut f = File::open(file)?;
    f.seek(SeekFrom::Start(offset))?;
    let mut data = Vec::with_capacity(size as usize);
    f.take(size).read_to_end(&mut data)?;
    ensure!(data.len() as u64 == size && sha256_hex(&data) == sha256, "{} isn't what it should be", file.display());
    Ok(data)
}

/// Copy `size` bytes of `file` from `offset` to `out`, checking their sha256.
fn copy_checked(file: &Path, offset: u64, size: u64, sha256: &str, out: &mut impl Write) -> Result<()> {
    let mut f = File::open(file)?;
    f.seek(SeekFrom::Start(offset))?;
    // Checked first, then copied: what's written is always right.
    let mut h = Sha256::new();
    let n = io::copy(&mut (&f).take(size), &mut HashOnly(&mut h))?;
    ensure!(n == size && bundle_format::hex(&h.finalize()) == sha256, "{} isn't what it should be", file.display());
    f.seek(SeekFrom::Start(offset))?;
    io::copy(&mut f.take(size), out)?;
    Ok(())
}

struct HashOnly<'a>(&'a mut Sha256);

impl Write for HashOnly<'_> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.update(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn get(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    agent.get(url).call()?.into_reader().take(limit + 1).read_to_end(&mut data)?;
    ensure!(data.len() as u64 <= limit, "more than {limit} bytes");
    Ok(data)
}

/// A part's frame to `dest`, if it is the `size` bytes whose sha256 is `sha256`.
fn download(agent: &ureq::Agent, url: &str, dest: &Path, size: u64, sha256: &str) -> Result<()> {
    if dest.metadata().is_ok_and(|m| m.len() == size) && read_hash(dest).is_ok_and(|h| h == sha256) {
        return Ok(());
    }
    let tmp = dest.with_extension(format!("tmp{}", std::process::id()));
    let result = (|| -> Result<()> {
        let mut r = agent.get(url).call().with_context(|| format!("fetching {url}"))?.into_reader().take(size + 1);
        let mut f = File::create(&tmp)?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        let mut n = 0u64;
        loop {
            let k = r.read(&mut buf)?;
            if k == 0 {
                break;
            }
            h.update(&buf[..k]);
            f.write_all(&buf[..k])?;
            n += k as u64;
        }
        ensure!(n == size, "{url} is {n} bytes, not the {size} the signed description says");
        ensure!(bundle_format::hex(&h.finalize()) == sha256, "{url} doesn't match the signed description's sha256: refused");
        Ok(())
    })();
    match result {
        Ok(()) => Ok(fs::rename(&tmp, dest)?),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn read_hash(file: &Path) -> io::Result<String> {
    let mut h = Sha256::new();
    io::copy(&mut File::open(file)?, &mut HashOnly(&mut h))?;
    Ok(bundle_format::hex(&h.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feeds_are_signed_and_checked() {
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let public = public_key("dev-1", &key);
        let mut f = Feed { format: 1, name: "app".into(), version: "2".into(), serial: 20, sha256: "ab".repeat(32), size: 9, file: "app-x86_64".into(), ..Default::default() };
        f.sign("dev-1", &key);
        f.verify(&public).unwrap();
        // Anything signed changed: refused.
        for change in [|f: &mut Feed| f.serial = 30, |f: &mut Feed| f.sha256 = "cd".repeat(32), |f: &mut Feed| f.name = "other".into()] {
            let mut g = f.clone();
            change(&mut g);
            assert!(g.verify(&public).is_err());
        }
        // Another key, even under the same name: refused.
        let other = SigningKey::from_bytes(&[4u8; 32]);
        let mut g = f.clone();
        g.sign("dev-1", &other);
        assert!(g.verify(&public).unwrap_err().to_string().contains("doesn't match"));
        g.sign("someone", &other);
        assert!(g.verify(&public).unwrap_err().to_string().contains("not by this bundle's developer"));
    }

    #[test]
    fn a_key_hands_over_only_when_the_old_one_signs_it() {
        let (a, b, c) = (SigningKey::from_bytes(&[1u8; 32]), SigningKey::from_bytes(&[2u8; 32]), SigningKey::from_bytes(&[3u8; 32]));
        let (pa, pb, pc) = (public_key("ka", &a), public_key("kb", &b), public_key("kc", &c));
        let ab = sign_rotation(&a, &pa, &pb);
        let bc = sign_rotation(&b, &pb, &pc);
        assert!(rotated_to(&pa, &pb, std::slice::from_ref(&ab)));
        assert!(rotated_to(&pa, &pc, &[bc.clone(), ab.clone()]), "a chain, in any order");
        assert!(!rotated_to(&pb, &pa, &[ab.clone()]), "never back");
        // Signed by the new key, not the old: refused.
        let forged = sign_rotation(&c, &pa, &pc);
        assert!(!rotated_to(&pa, &pc, &[forged]));
        // From user.nami's fingerprint of the first key.
        assert!(rotated_from_fingerprint(&fingerprint(&pa).unwrap(), &pc, &[ab.clone(), bc.clone()]));
        assert!(!rotated_from_fingerprint(&fingerprint(&pc).unwrap(), &pa, &[ab.clone(), bc.clone()]));
        // A feed signed by the new key: its signer, found through the rotations.
        let mut f = Feed { format: 1, name: "app".into(), version: "2".into(), serial: 20, sha256: "ab".repeat(32), size: 9, file: "app-x86_64".into(), ..Default::default() };
        f.sign("kc", &c);
        assert!(feed_signer(&pa, &f).is_err(), "no rotations: refused");
        f.rotations = vec![ab, bc];
        assert_eq!(feed_signer(&pa, &f).unwrap(), pc);
        // Trusted here: the old key gives way to the one it handed over to.
        let dir = std::env::temp_dir().join(format!("nami-rot-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let meta = |k: &str, r: Vec<Rotation>| Meta { name: "app".into(), update: Some(bundle_format::Update { feed: "https://e/x".into(), key: k.into(), serial: 1, rotations: r }), ..Default::default() };
        settle_in(&dir, &meta(&pa, vec![])).unwrap();
        assert_eq!(trust_in(&dir, &meta(&pc, f.rotations.clone())), crate::bundle_trust::Trust::Same);
        assert_eq!(crate::bundle_trust::trusted_in(&dir, &meta(&pc, vec![])).as_deref(), Some(pc.as_str()));
        assert!(matches!(trust_in(&dir, &meta(&pb, vec![])), crate::bundle_trust::Trust::Changed { .. }), "back to an earlier key: refused");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_update_keeps_the_key() {
        let key = |k: &str| Meta { name: "app".into(), update: Some(bundle_format::Update { feed: "https://e/x".into(), key: k.into(), serial: 1, ..Default::default() }), ..Default::default() };
        assert!(same_key(&key("a:AAAA"), &key("a:AAAA")).is_ok());
        assert!(same_key(&key("a:AAAA"), &key("b:BBBB")).unwrap_err().to_string().contains("would change the key"));
        // A new version that stops updating itself is the developer's to make.
        assert!(same_key(&key("a:AAAA"), &Meta { name: "app".into(), ..Default::default() }).is_ok());
    }

    #[test]
    fn github_shorthand() {
        assert_eq!(feed_url("github:me/app"), "https://github.com/me/app/releases/latest/download");
        assert_eq!(feed_url("https://example.com/app/"), "https://example.com/app");
    }

    #[test]
    fn keys_as_people_compare_them() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let fp = fingerprint(&public_key("bundle-x", &key)).unwrap();
        assert_eq!(fp.len(), 39);
        assert_eq!(fp.split(' ').count(), 8);
        assert_eq!(fp, fp.to_uppercase());
        assert_eq!(fingerprint("bundle-x:bm90IGEga2V5"), None);
    }

    #[test]
    fn the_computers_switch() {
        let tmp = std::env::temp_dir().join(format!("nami-switch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        // Not namiOS: nothing to say.
        fs::create_dir_all(&tmp).unwrap();
        assert!(!system_says_off_at(&tmp));
        fs::create_dir_all(tmp.join("etc/nami-os")).unwrap();
        assert!(system_says_off_at(&tmp));
        fs::create_dir_all(tmp.join("etc/systemd/system/timers.target.wants")).unwrap();
        fs::write(tmp.join("etc/systemd/system/timers.target.wants/nami-update.timer"), "").unwrap();
        assert!(!system_says_off_at(&tmp));
        fs::remove_dir_all(&tmp).unwrap();
    }
}
