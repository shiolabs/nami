//! Difference-only updates of self-updating bundles: shared by nami, which
//! makes the patches when it publishes (src/bundle_publish.rs), and
//! nami-bundle-runtime-net, which applies them (src/bundle_update.rs).
//!
//! A part's frame is its NAR, zstd-compressed (bundle_format.rs). When a
//! part changes between two releases (a new version of the app, or of a
//! library), the new NAR is mostly the old one: a patch is the new NAR
//! compressed with the old one as zstd's reference prefix (what `zstd
//! --patch-from` does), often a fraction of the new frame.
//!
//! The update must come out byte for byte as the developer published it
//! (the feed signs the whole bundle's hash), so the runtime, having the
//! new NAR, compresses it again exactly as `nami bundle` did: the same
//! zstd, the same level, the same parameters (`frame_encoder`). The feed
//! says which zstd and level made the frames (`Frames`); a runtime with
//! another zstd doesn't use the patches. The result is checked against the
//! signed description's sha256 before it's used, and anything that doesn't
//! match is downloaded in full, as before.
//!
//! The old part comes from the old bundle itself (its frame, checked
//! against its own description's sha256), not from the folder it was
//! unpacked into: that folder's files have the bundle's prefix in place of
//! /nix/store and could have been changed, and its frame is exactly what
//! the developer made the patch from.

// Shared by nami and the runtime: each uses its own half.
#![allow(dead_code)]

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Frames' window: 128 MiB (bundle.rs's frames, and so every runtime's).
pub const FRAME_WINDOW_LOG: u32 = 27;
/// The largest window a patch may need (2 GiB, zstd's most on 64-bit).
const PATCH_WINDOW_LOG_MAX: u32 = 31;
/// The most memory the runtime gives to patches at once (`memory`): a part
/// that would take more is downloaded whole.
pub const MAX_MEMORY: u64 = 1 << 30;

/// What applying a patch from an `old` to a `new` part holds in memory, at
/// most: the old NAR, and the new one's window as it's unpacked and as it's
/// compressed again.
pub fn memory(old_nar: u64, new_nar: u64) -> u64 {
    old_nar + 2 * new_nar
}

/// Memory shared by patches made at the same time: `take` waits for room.
pub struct Budget {
    free: std::sync::Mutex<u64>,
    room: std::sync::Condvar,
}

pub struct Held<'a>(&'a Budget, u64);

impl Budget {
    pub fn new(total: u64) -> Budget {
        Budget { free: std::sync::Mutex::new(total), room: std::sync::Condvar::new() }
    }

    pub fn take(&self, n: u64) -> Held<'_> {
        let mut free = self.free.lock().unwrap();
        while *free < n {
            free = self.room.wait(free).unwrap();
        }
        *free -= n;
        Held(self, n)
    }
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        *self.0.free.lock().unwrap() += self.1;
        self.0.room.notify_all();
    }
}

/// How a part's frame is compressed: one zstd frame of the NAR's `size`
/// bytes (said up front, so a small part takes little memory to make), a
/// checksum, long distance matching over a window of up to 128 MiB.
pub fn frame_encoder<'a, W: Write>(w: W, level: i32, size: u64) -> io::Result<zstd::stream::write::Encoder<'a, W>> {
    let mut enc = zstd::stream::write::Encoder::new(w, level)?;
    enc.include_checksum(true)?;
    enc.long_distance_matching(true)?;
    enc.window_log(FRAME_WINDOW_LOG)?;
    enc.set_pledged_src_size(Some(size))?;
    Ok(enc)
}

/// What made a release's frames, in its feed: a runtime that compresses
/// the same can turn a patch back into the developer's frame.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Frames {
    /// zstd's version (`1.5.7`).
    pub zstd: String,
    pub level: i32,
    /// frame_encoder's parameters: 2 is the window, checksum, long distance
    /// matching and size above. A runtime that knows another uses no patch.
    pub params: u32,
}

/// frame_encoder's parameters, as `Frames::params` says them.
pub const FRAME_PARAMS: u32 = 2;

impl Frames {
    /// This build's, at `level`.
    pub fn here(level: i32) -> Frames {
        Frames { zstd: zstd::zstd_safe::version_string().to_string(), level, params: FRAME_PARAMS }
    }

    /// This build compresses frames the same.
    pub fn same_here(&self) -> bool {
        *self == Frames::here(self.level)
    }
}

/// A patch, in a release's feed: the part `to` (its frame's sha256, in the
/// new description), from the part `from` (its frame's sha256, in an
/// earlier release); the patch file is `size` bytes with this `sha256`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Patch {
    pub from: String,
    pub to: String,
    pub size: u64,
    pub sha256: String,
}

impl Patch {
    /// Its file, beside the parts: `<to>.from-<from, 16 digits>.patch.zst`.
    pub fn file(&self) -> String {
        format!("{}.from-{}.patch.zst", self.to, &self.from[..self.from.len().min(16)])
    }
}

/// What the developer's key signs for a release's patches (beside the
/// feed's own signature, which older runtimes check alone).
pub fn message(name: &str, serial: u64, sha256: &str, frames: &Frames, patches: &[Patch]) -> Vec<u8> {
    let mut m = format!("nami-bundle-patches-v1\n{name}\n{serial}\n{sha256}\nzstd {} level {} params {}\n", frames.zstd, frames.level, frames.params);
    for p in patches {
        m += &format!("{} {} {} {}\n", p.from, p.to, p.size, p.sha256);
    }
    m.into_bytes()
}

/// Whether an update uses its patches. A check nobody waits for always
/// does (it saves the most); an update someone waits for takes the faster
/// way (`Auto`). NAMI_BUNDLE_DELTA=always|never|auto says otherwise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Choice {
    Always,
    Never,
    Auto,
}

impl Choice {
    pub fn for_update(waiting: bool) -> Choice {
        match std::env::var("NAMI_BUNDLE_DELTA").ok().as_deref().map(str::trim) {
            Some("always") => Choice::Always,
            Some("never") => Choice::Never,
            Some("auto") => Choice::Auto,
            _ if waiting => Choice::Auto,
            _ => Choice::Always,
        }
    }
}

/// What the two ways of updating take, as measured or guessed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    /// The download speed, bytes a second (None: it couldn't be measured).
    pub speed: Option<f64>,
    /// How fast this computer compresses a part again, bytes of NAR a
    /// second, one thread.
    pub rate: f64,
    pub threads: usize,
    /// The patches' bytes, the whole parts' they stand for, and the NARs
    /// to compress again.
    pub patch_bytes: u64,
    pub whole_bytes: u64,
    pub nar_bytes: u64,
}

impl Estimate {
    /// Seconds by patches, and by whole parts: None without a speed.
    pub fn seconds(&self) -> Option<(f64, f64)> {
        let speed = self.speed.filter(|s| *s > 0.0)?;
        let rebuild = self.nar_bytes as f64 / (self.rate.max(1.0) * self.threads.max(1) as f64);
        Some((self.patch_bytes as f64 / speed + rebuild, self.whole_bytes as f64 / speed))
    }

    /// The patches are the faster way (without a measured speed: no).
    pub fn patches_faster(&self) -> bool {
        self.seconds().is_some_and(|(patched, whole)| patched < whole)
    }
}

/// A rate to count on when compressing can't be measured: slower than
/// most computers (at level 19, 1.5 MiB a second a thread).
pub fn careful_rate(level: i32) -> f64 {
    let mib = match level {
        19.. => 1.5,
        16..=18 => 3.0,
        10..=15 => 10.0,
        _ => 40.0,
    };
    mib * 1048576.0
}

/// The NAR in a frame.
pub fn decompress(frame: &[u8], nar_size: u64) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(nar_size.min(MAX_MEMORY) as usize);
    zstd::stream::read::Decoder::new(frame)?.read_to_end(&mut out)?;
    Ok(out)
}

/// The window a patch from `old` to `new` bytes needs: both, as zstd's
/// --patch-from sets it.
fn patch_window_log(old: usize, new: usize) -> u32 {
    let total = (old + new).max(1 << 20) as u64;
    (64 - (total - 1).leading_zeros()).clamp(FRAME_WINDOW_LOG, PATCH_WINDOW_LOG_MAX)
}

/// The patch that makes `new` from `old` (nami's half).
pub fn make(old: &[u8], new: &[u8], level: i32) -> io::Result<Vec<u8>> {
    let mut enc = zstd::stream::write::Encoder::with_ref_prefix(Vec::new(), level, old)?;
    enc.long_distance_matching(true)?;
    enc.window_log(patch_window_log(old.len(), new.len()))?;
    enc.set_pledged_src_size(Some(new.len() as u64))?;
    enc.include_checksum(true)?;
    enc.write_all(new)?;
    enc.finish()
}

/// The new frame, from the old part's NAR and the patch: the new NAR (of
/// `nar_size` bytes), compressed as `frames` says, written to `out`. Its
/// sha256 and size.
pub fn apply(old: &[u8], patch: impl Read, nar_size: u64, frames: &Frames, out: impl Write) -> io::Result<(String, u64)> {
    if !frames.same_here() {
        return Err(io::Error::other(format!("its parts were compressed with zstd {}, and this runtime's is {}", frames.zstd, zstd::zstd_safe::version_string())));
    }
    let mut dec = zstd::stream::read::Decoder::with_ref_prefix(io::BufReader::new(patch), old)?;
    dec.window_log_max(PATCH_WINDOW_LOG_MAX)?;
    let mut hashed = Hashed { inner: out, hash: Sha256::new(), n: 0 };
    let mut enc = frame_encoder(&mut hashed, frames.level, nar_size)?;
    io::copy(&mut dec, &mut enc)?;
    enc.finish()?;
    hashed.flush()?;
    Ok((hex(&hashed.hash.finalize()), hashed.n))
}

struct Hashed<W> {
    inner: W,
    hash: Sha256,
    n: u64,
}

impl<W: Write> Write for Hashed<W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(b)?;
        self.hash.update(&b[..n]);
        self.n += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// What a part is, whatever its version: its name less the store hash and
/// the version numbers (`ripgrep-14.1.1` and `ripgrep-15.0.0` are both
/// `ripgrep`; `glib-2.88.3-bin` is `glib-bin`). Parts of two releases with
/// the same are patched one from the other.
pub fn kind(name: &str) -> String {
    let n = if name.len() > 33 && name.as_bytes()[32] == b'-' { &name[33..] } else { name };
    n.split('-').filter(|w| !w.starts_with(|c: char| c.is_ascii_digit())).collect::<Vec<_>>().join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Something that compresses like a program: repeats, and what doesn't.
    fn data(n: usize, seed: u64) -> Vec<u8> {
        let mut x = seed;
        (0..n)
            .map(|i| {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                if i % 7 < 3 {
                    (x >> 33) as u8
                } else {
                    b"the quick brown fox "[i % 20]
                }
            })
            .collect()
    }

    fn frame(nar: &[u8], level: i32, chunk: usize) -> Vec<u8> {
        let mut enc = frame_encoder(Vec::new(), level, nar.len() as u64).unwrap();
        for c in nar.chunks(chunk) {
            enc.write_all(c).unwrap();
        }
        enc.finish().unwrap()
    }

    #[test]
    fn a_frame_is_the_same_however_its_nar_is_written() {
        // bundle.rs writes a NAR in pieces as it walks the files; a runtime
        // writes the patched one as zstd hands it over.
        let nar = data(3 << 20, 1);
        let one = frame(&nar, 19, nar.len());
        for chunk in [1, 13, 4096, 1 << 17, 1 << 20] {
            assert!(frame(&nar, 19, chunk) == one, "chunks of {chunk}");
        }
        assert_eq!(decompress(&one, nar.len() as u64).unwrap(), nar);
    }

    #[test]
    fn a_patch_makes_the_new_frame_from_the_old_nar() {
        let old = data(4 << 20, 2);
        let mut new = old.clone();
        // A new version: some bytes changed, some inserted, some gone.
        new[1000..1100].copy_from_slice(&data(100, 9));
        new.splice(2 << 20..2 << 20, data(50_000, 3));
        new.drain(3 << 20..(3 << 20) + 20_000);
        let level = 19;
        let want = frame(&new, level, 1 << 16);
        let patch = make(&old, &new, level).unwrap();
        assert!(patch.len() * 4 < want.len(), "the patch is {} bytes, the frame {}", patch.len(), want.len());
        let mut out = Vec::new();
        let (sha, n) = apply(&old, &patch[..], new.len() as u64, &Frames::here(level), &mut out).unwrap();
        assert!(out == want);
        assert_eq!((sha, n), (hex(&Sha256::digest(&want)), want.len() as u64));
        // Against another old part it doesn't come out (and that's caught by the hash).
        let mut other = old.clone();
        other[10] ^= 1;
        let mut out = Vec::new();
        if let Ok((sha, _)) = apply(&other, &patch[..], new.len() as u64, &Frames::here(level), &mut out) {
            assert_ne!(sha, hex(&Sha256::digest(&want)));
        }
        // Another zstd, or other parameters: not used.
        let mut f = Frames::here(level);
        f.zstd = "1.4.0".into();
        assert!(apply(&old, &patch[..], new.len() as u64, &f, io::sink()).unwrap_err().to_string().contains("compressed with zstd 1.4.0"));
        f = Frames::here(level);
        f.params = 1;
        assert!(apply(&old, &patch[..], new.len() as u64, &f, io::sink()).is_err());
    }

    #[test]
    fn patches_made_together_share_the_memory() {
        let b = Budget::new(10);
        let one = b.take(6);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::scope(|s| {
            s.spawn(|| {
                let _two = b.take(6);
                tx.send(()).unwrap();
            });
            // The second waits for the first.
            assert!(rx.recv_timeout(std::time::Duration::from_millis(100)).is_err());
            drop(one);
            rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        });
        assert_eq!(memory(1, 2), 5);
    }

    #[test]
    fn an_update_someone_waits_for_takes_the_faster_way() {
        const MIB: f64 = 1048576.0;
        // FeatherPad 1.6.3 to 1.6.4: 72 MiB of patches for 139 MiB of
        // parts, 712 MiB of NAR to compress again, at 3.5 MiB/s a thread.
        let e = |speed: Option<f64>| Estimate { speed, rate: 3.5 * MIB, threads: 4, patch_bytes: 72 << 20, whole_bytes: 139 << 20, nar_bytes: 712 << 20 };
        // A fast link: whole parts (3 s, against 52 s with patches).
        assert!(!e(Some(50.0 * MIB)).patches_faster());
        // A slow one: patches (72 + 51 s against 139 s).
        assert!(e(Some(MIB)).patches_faster());
        let (p, w) = e(Some(MIB)).seconds().unwrap();
        assert!((p - (72.0 + 712.0 / 14.0)).abs() < 0.1 && (w - 139.0).abs() < 0.1, "{p} {w}");
        // Unmeasured: whole parts.
        assert!(!e(None).patches_faster());
        assert!(!e(Some(0.0)).patches_faster());
        // The careful rate makes the patches wait for a slower link.
        let careful = Estimate { rate: careful_rate(19), ..e(Some(MIB)) };
        assert!(!careful.patches_faster());
        assert!(Estimate { speed: Some(0.5 * MIB), ..careful }.patches_faster());
        assert!(careful_rate(3) > careful_rate(19));
    }

    #[test]
    fn who_waits_says_how() {
        std::env::remove_var("NAMI_BUNDLE_DELTA");
        assert_eq!(Choice::for_update(false), Choice::Always);
        assert_eq!(Choice::for_update(true), Choice::Auto);
        for (v, c) in [("never", Choice::Never), ("always", Choice::Always), ("auto", Choice::Auto)] {
            std::env::set_var("NAMI_BUNDLE_DELTA", v);
            assert_eq!(Choice::for_update(false), c);
            assert_eq!(Choice::for_update(true), c);
        }
        std::env::remove_var("NAMI_BUNDLE_DELTA");
    }

    #[test]
    fn parts_are_matched_by_kind() {
        assert_eq!(kind("pyhrj644hwwyjs4g3zigbh3hbyy6gjm9-ripgrep-14.1.1"), "ripgrep");
        assert_eq!(kind("0vw7gvcazcjqc762x7vg3l31cpx65l7i-gcc-15.3.0-libgcc"), "gcc-libgcc");
        assert_eq!(kind("pyhrj644hwwyjs4g3zigbh3hbyy6gjm9-python3.14-jaraco-functools-4.4.0"), "python3.14-jaraco-functools");
        assert_eq!(kind("1fl4qsghsryd5f764m5v64vlxdz9d8qx-gtk+3-3.24.52"), "gtk+3");
    }

    #[test]
    fn the_signed_message_covers_every_patch() {
        let f = Frames::here(19);
        let p = Patch { from: "a".repeat(64), to: "b".repeat(64), size: 10, sha256: "c".repeat(64) };
        let m = message("app", 2, "d", &f, std::slice::from_ref(&p));
        let mut q = p.clone();
        q.size = 11;
        assert_ne!(m, message("app", 2, "d", &f, &[q]));
        assert_ne!(m, message("app", 2, "d", &f, &[]));
        assert_eq!(p.file(), format!("{}.from-aaaaaaaaaaaaaaaa.patch.zst", "b".repeat(64)));
    }
}
