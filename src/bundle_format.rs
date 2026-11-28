//! The file `nami bundle` makes, as both nami (which writes it) and the
//! bundle's own runtime (which runs it) read it. No nami code beyond std,
//! serde and sha2, so the runtime stays small.
//!
//! A bundle is one file:
//!
//! ```text
//! [runtime]   a static program (src/bin/nami-bundle-runtime.rs)
//! [frames]    one zstd frame per store path: the path's NAR, as in the store
//! [meta]      JSON: what to run, and each frame's offset, size and sha256
//! [trailer]   64 bytes: where frames and meta start, sha256(meta), magic
//! ```
//!
//! The files are kept exactly as they are in /nix/store, so the same path
//! compresses to the same bytes in every bundle. On a computer, a bundle is
//! unpacked once into `~/.cache/tray-bundle/<prefix>/`, with every
//! `/nix/store` in it (in programs, libraries, scripts, links) replaced by
//! the same-length `/tmp/<prefix>`, a link to that folder. Same length, so
//! no program's layout changes: no patchelf, and the loader, rpaths and
//! data paths nixpkgs compiled in all point inside the bundle.

// Shared by nami and the runtime: each uses its own half.
#![allow(dead_code)]

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The end of every bundle.
pub const MAGIC: &[u8; 8] = b"TRAYBUN1";
pub const TRAILER_LEN: u64 = 64;
/// What every path in the bundle starts with, and its replacement's length.
pub const STORE: &str = "/nix/store";
/// The end of a nami that carries the runtime (build.sh appends it).
pub const RUNTIME_MAGIC: &[u8; 8] = b"TRAYRTM1";
/// In the runtime's ELF header, where nothing reads (e_ident's padding,
/// bytes 9 to 15): a bundle's kind of file is told from its first bytes,
/// as desktops tell kinds of file (shared-mime-info's magic:
/// application/x-tray-bundle), though its trailer is what proves it.
pub const ELF_MARK: &[u8; 7] = b"TRAYBUN";

/// The runtime, marked as a bundle's start (when its padding is free).
pub fn mark_runtime(runtime: &mut [u8]) {
    if runtime.len() > 16 && runtime.starts_with(b"\x7fELF") && runtime[9..16].iter().all(|b| *b == 0) {
        runtime[9..16].copy_from_slice(ELF_MARK);
    }
}

/// Whether the file at `path` is a bundle: its trailer and its description's hash checked.
pub fn is_bundle(path: &Path) -> bool {
    File::open(path).ok().is_some_and(|mut f| read(&mut f).is_ok())
}

// ---------------------------------------------------------------------------
// The two names. Bundles made before the rename (docs/rename-migration.md)
// carry a runtime that knows only tray's: `--tray-bundle-<what>`,
// `TRAY_BUNDLE_*`, `$XDG_CACHE_HOME/tray-bundle`, `.<file>.tray-update`.
// Those made by nami 0.18.6 to 0.18.8 know only nami's. Every runtime since
// knows both, and nami speaks to each bundle in the words its runtime knows.

/// The words a bundle's own runtime knows for `--<…>-bundle-<what>`: an
/// unknown one is the program's own argument, and starts it. `nami` if
/// its runtime knows nami's (so does every runtime since), else `tray`.
pub fn runtime_says(path: &Path) -> &'static str {
    let knows_nami = || -> Option<bool> {
        let mut f = File::open(path).ok()?;
        let (t, _) = read(&mut f).ok()?;
        let mut runtime = Vec::new();
        f.seek(SeekFrom::Start(0)).ok()?;
        (&mut f).take(t.payload_offset.min(64 << 20)).read_to_end(&mut runtime).ok()?;
        let has = |w: &[u8]| runtime.windows(w.len()).any(|x| x == w);
        Some(has(b"--nami-bundle-") || !has(b"--tray-bundle-"))
    };
    // (Not a bundle of ours, or an AppImage, whose AppRun is inside: nami's.)
    if knows_nami().unwrap_or(true) {
        "nami"
    } else {
        "tray"
    }
}

/// `--nami-bundle-<what>`, or `--tray-bundle-<what>` for a bundle whose
/// runtime knows only that (runtime_says).
pub fn flag(path: &Path, what: &str) -> String {
    format!("--{}-bundle-{what}", runtime_says(path))
}

/// The folders the bundles' shared store may be in, the one to use first
/// first: `NAMI_BUNDLE_CACHE` (or tray's `TRAY_BUNDLE_CACHE`), else
/// `$XDG_CACHE_HOME/tray-bundle` and `…/nami-bundle` (0.18.6 to 0.18.8's
/// name: the same folder without XDG_CACHE_HOME), else
/// `~/.cache/tray-bundle`.
pub fn cache_dirs() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let var = |k: &str| std::env::var_os(format!("NAMI_{k}")).or_else(|| std::env::var_os(format!("TRAY_{k}"))).filter(|v| !v.is_empty());
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(c) = var("BUNDLE_CACHE") {
        out.push(PathBuf::from(c));
    } else if let Some(c) = std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        out.push(PathBuf::from(&c).join("tray-bundle"));
        out.push(PathBuf::from(&c).join("nami-bundle"));
    } else if let Some(h) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        out.push(PathBuf::from(h).join(".cache/tray-bundle"));
    }
    out
}

/// Where the store is for who can't have it there (no home, or one that
/// can't be written): in /tmp, tray's name and 0.18.6 to 0.18.8's.
pub fn cache_fallbacks() -> Vec<std::path::PathBuf> {
    // SAFETY: getuid can't fail.
    let uid = unsafe { libc::getuid() };
    vec![std::env::temp_dir().join(format!("tray-bundle-{uid}")), std::env::temp_dir().join(format!("nami-bundle-{uid}"))]
}

/// Of `dirs` (cache_dirs), the one a bundle with this hash uses: where it's
/// unpacked already, else the first that has a store, else the first. (Two
/// such folders can't be one through a link: each runtime would take the
/// other's /tmp prefix for someone else's, and start its store again.)
pub fn pick_cache(dirs: &[std::path::PathBuf], hash: Option<&str>) -> Option<std::path::PathBuf> {
    hash.and_then(|h| dirs.iter().find(|d| d.join("bundles").join(h).is_file()))
        .or_else(|| dirs.iter().find(|d| d.join("store").is_dir()))
        .or_else(|| dirs.first())
        .cloned()
}

/// `TRAY_BUNDLE_X`s as `NAMI_BUNDLE_X` where that isn't set: for the
/// runtime, which reads nami's (nami itself does this for every TRAY_ one).
pub fn tray_environment() -> Vec<(String, std::ffi::OsString)> {
    let vars: Vec<(String, std::ffi::OsString)> = std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v))).collect();
    vars.iter()
        .filter_map(|(k, v)| Some((format!("NAMI_BUNDLE{}", k.strip_prefix("TRAY_BUNDLE")?), v.clone())))
        .filter(|(k, _)| !vars.iter().any(|(have, _)| have == k))
        .collect()
}

/// What the bundle is and runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Meta {
    pub format: u32,
    pub name: String,
    #[serde(default)]
    pub version: String,
    /// The program to run: a store path, e.g. /nix/store/…-jq-1.8.2-bin/bin/jq.
    pub entry: String,
    /// Other programs it can be, by name: the bundle run through a link
    /// called `jq` runs `jq` (like busybox).
    #[serde(default)]
    pub programs: std::collections::BTreeMap<String, String>,
    /// Started by the bundled loader explicitly, with these library folders
    /// (a program made elsewhere that asks for /lib64's loader).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader: Option<Loader>,
    /// The store paths, in the order their frames come.
    pub paths: Vec<PathEntry>,
    /// Every library in the bundle by soname, to tell which of the host's
    /// graphics libraries' needs it already meets.
    #[serde(default)]
    pub libs: std::collections::BTreeMap<String, String>,
    /// glibc's store path, whose loader runs everything.
    #[serde(default)]
    pub glibc: String,
    /// Desktop integration: the app's .desktop file and icon, if it has them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop: Option<Desktop>,
    /// Variables set before the program starts, `/nix/store` in them
    /// meaning the bundle's.
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub made_by: String,
    /// sha256 of the runtime in front (the bytes before the first frame), hex.
    #[serde(default)]
    pub runtime: String,
    /// Where a thin bundle's parts come from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub substituters: Vec<Substituter>,
    /// Where newer versions are published, and whose key signs them
    /// (src/bundle_update.rs). Older runtimes ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<Update>,
    /// Who made it, as they call themselves (project.nami's `bundle:
    /// developer:`, `--developer`): shown with the key that signs its
    /// updates. Older runtimes ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub developer: Option<String>,
    /// Run inside bubblewrap where the computer has it, with only these
    /// rights (src/bundle_sandbox.rs). Older runtimes ignore it: a bundle
    /// made with it always starts with a runtime that knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<Sandbox>,
    /// The computers it's for, as `uname -m` says: x86_64 or aarch64.
    /// Bundles from before it was said are x86_64 (`arch()`).
    #[serde(default)]
    pub arch: String,
}

/// What a sandboxed bundle may use beyond its window and its own data:
/// `{ network: true, files: [documents], devices: [gpu, audio] }`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Sandbox {
    #[serde(default)]
    pub network: bool,
    /// The person's folders, by their short names (bundle_sandbox::FILES).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Devices, by their short names (bundle_sandbox::DEVICES).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<String>,
}

/// The architectures bundles are made for.
pub const ARCHES: &[&str] = &["x86_64", "aarch64"];

impl Meta {
    /// What computers it's for (x86_64 when it doesn't say).
    pub fn arch(&self) -> &str {
        if self.arch.is_empty() {
            "x86_64"
        } else {
            &self.arch
        }
    }

    /// Why it can't run on this computer, if it's for another kind: for the
    /// runtime, before anything is unpacked.
    pub fn arch_problem(&self) -> Option<String> {
        let (want, here) = (self.arch(), std::env::consts::ARCH);
        (want != here).then(|| format!("{} is made for {} computers, and this one is {}: get its -{here} file instead", self.name, arch_name(want), arch_name(here)))
    }
}

/// How people know an architecture.
pub fn arch_name(arch: &str) -> String {
    match arch {
        "x86_64" => "x86_64 (Intel and AMD)".into(),
        "aarch64" => "ARM (aarch64)".into(),
        a => a.into(),
    }
}

/// The architecture an ELF file is for (its e_machine), as `uname -m` says it.
pub fn elf_arch(data: &[u8]) -> Option<&'static str> {
    if data.len() < 20 || &data[..4] != b"\x7fELF" {
        return None;
    }
    match u16::from_le_bytes([data[18], data[19]]) {
        0x3e => Some("x86_64"),
        0xb7 => Some("aarch64"),
        _ => None,
    }
}

/// glibc's loader for an architecture: the file in its lib/.
pub fn loader_name(arch: &str) -> &'static str {
    if arch == "aarch64" {
        "ld-linux-aarch64.so.1"
    } else {
        "ld-linux-x86-64.so.2"
    }
}

/// A self-updating bundle's feed: `<feed>/update.json`, signed with `key`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Update {
    /// The folder the developer publishes to (a URL), e.g.
    /// https://github.com/<owner>/<repo>/releases/latest/download.
    pub feed: String,
    /// The developer's public key, `name:base64` (ed25519).
    pub key: String,
    /// When this version was made (ms since 1970): a feed offering an
    /// older or equal one is no update.
    pub serial: u64,
    /// The developer's earlier keys, each handing over to the next
    /// (`nami bundle --rotate-key`): a copy that trusts an earlier one
    /// follows them to `key`. Older runtimes ignore it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rotations: Vec<Rotation>,
}

/// One key handing over to another: `to`, signed by `from`'s secret half.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Rotation {
    pub from: String,
    pub to: String,
    /// `<from's name>:<base64>`, over bundle_update::rotation_message(from, to).
    pub signature: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Loader {
    pub path: String,
    pub library_path: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Desktop {
    /// Its file name, e.g. org.gnome.TextEditor.desktop.
    pub id: String,
    /// Its contents, as the package has them.
    pub file: String,
    /// The icon: a store path to a .png or .svg.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PathEntry {
    /// `<hash>-<name>`.
    pub name: String,
    /// Where its frame starts, from the start of the file.
    pub offset: u64,
    pub size: u64,
    /// sha256 of the frame, hex.
    pub sha256: String,
    /// The NAR's size, unpacked.
    pub nar_size: u64,
    /// The NAR's hash, as Nix writes it (`sha256:<base32>`): what the part
    /// is, whichever bundle (or binary cache, or host store) it comes from.
    #[serde(default)]
    pub nar_hash: String,
    /// The store paths it refers to (base names, itself left out).
    #[serde(default)]
    pub references: Vec<String>,
    /// Not in the file (a thin bundle): fetched from `Meta::substituters`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fetch: bool,
}

/// A binary cache a thin bundle's parts come from, and the key its
/// narinfos must be signed with (`name:base64`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Substituter {
    pub url: String,
    pub key: String,
}

/// The fixed-size end of a bundle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trailer {
    pub payload_offset: u64,
    pub meta_offset: u64,
    pub meta_len: u64,
    /// sha256 of the meta, which holds every frame's sha256: the bundle's hash.
    pub meta_sha256: [u8; 32],
}

impl Trailer {
    pub fn encode(&self) -> [u8; TRAILER_LEN as usize] {
        let mut out = [0u8; TRAILER_LEN as usize];
        out[0..8].copy_from_slice(&self.payload_offset.to_le_bytes());
        out[8..16].copy_from_slice(&self.meta_offset.to_le_bytes());
        out[16..24].copy_from_slice(&self.meta_len.to_le_bytes());
        out[24..56].copy_from_slice(&self.meta_sha256);
        out[56..64].copy_from_slice(MAGIC);
        out
    }

    pub fn decode(b: &[u8]) -> Option<Trailer> {
        if b.len() != TRAILER_LEN as usize || &b[56..64] != MAGIC {
            return None;
        }
        let u = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        Some(Trailer { payload_offset: u(0), meta_offset: u(8), meta_len: u(16), meta_sha256: b[24..56].try_into().unwrap() })
    }

    /// The bundle's hash, as people see it.
    pub fn hash(&self) -> String {
        hex(&self.meta_sha256)
    }
}

/// Every part of a bundle (its runtime and each frame) against the hashes
/// in its meta, which `read` checked against the trailer.
pub fn verify(file: &mut File, trailer: &Trailer, meta: &Meta) -> io::Result<()> {
    let mut buf = vec![0u8; 1 << 20];
    let mut part = |file: &mut File, offset: u64, size: u64, want: &str, what: &str| -> io::Result<()> {
        file.seek(SeekFrom::Start(offset))?;
        let mut h = Sha256::new();
        let mut left = size;
        while left > 0 {
            let n = file.read(&mut buf[..left.min(1 << 20) as usize])?;
            if n == 0 {
                return Err(bad(format!("{what}: cut short")));
            }
            h.update(&buf[..n]);
            left -= n as u64;
        }
        if hex(&h.finalize()) != want {
            return Err(bad(format!("{what} doesn't match the bundle's hash: the file is damaged or was changed")));
        }
        Ok(())
    };
    if !meta.runtime.is_empty() {
        part(file, 0, trailer.payload_offset, &meta.runtime, "the runtime")?;
    }
    for p in meta.paths.iter().filter(|p| !p.fetch) {
        part(file, p.offset, p.size, &p.sha256, &p.name)?;
    }
    Ok(())
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn sha256_hex(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// A bundle's trailer and meta, checked against each other.
pub fn read(file: &mut File) -> io::Result<(Trailer, Meta)> {
    let len = file.seek(SeekFrom::End(0))?;
    if len < TRAILER_LEN {
        return Err(bad("not a bundle (too short)"));
    }
    file.seek(SeekFrom::Start(len - TRAILER_LEN))?;
    let mut t = [0u8; TRAILER_LEN as usize];
    file.read_exact(&mut t)?;
    let trailer = Trailer::decode(&t).ok_or_else(|| bad("not a bundle (no trailer)"))?;
    if trailer.meta_offset + trailer.meta_len + TRAILER_LEN != len || trailer.payload_offset > trailer.meta_offset || trailer.meta_len > 64 << 20 {
        return Err(bad("damaged bundle (its parts don't add up)"));
    }
    file.seek(SeekFrom::Start(trailer.meta_offset))?;
    let mut meta = vec![0u8; trailer.meta_len as usize];
    file.read_exact(&mut meta)?;
    if Sha256::digest(&meta).as_slice() != trailer.meta_sha256 {
        return Err(bad("damaged bundle (its description doesn't match its hash)"));
    }
    let meta: Meta = serde_json::from_slice(&meta).map_err(|e| bad(format!("damaged bundle: {e}")))?;
    if meta.format != 1 {
        return Err(bad(format!("a bundle of format {}, newer than this runtime knows", meta.format)));
    }
    for p in &meta.paths {
        if !p.fetch && (p.offset < trailer.payload_offset || p.offset + p.size > trailer.meta_offset) || !valid_name(&p.name) {
            return Err(bad(format!("damaged bundle (path {})", p.name)));
        }
    }
    Ok((trailer, meta))
}

/// A store path's base name, safe to use as a file name.
pub fn valid_name(name: &str) -> bool {
    name.len() > 33 && name.as_bytes()[32] == b'-' && !name.contains('/') && name != "." && name != ".." && !name.starts_with('.')
}

/// Every `from` in `buf` becomes `to` (the same length).
pub fn replace_all(buf: &mut [u8], from: &[u8], to: &[u8]) -> usize {
    assert_eq!(from.len(), to.len());
    if from.is_empty() || buf.len() < from.len() {
        return 0;
    }
    let (first, mut n, mut i) = (from[0], 0, 0);
    while i + from.len() <= buf.len() {
        match buf[i..=buf.len() - from.len()].iter().position(|b| *b == first) {
            None => break,
            Some(p) => {
                i += p;
                if &buf[i..i + from.len()] == from {
                    buf[i..i + from.len()].copy_from_slice(to);
                    n += 1;
                    i += from.len();
                } else {
                    i += 1;
                }
            }
        }
    }
    n
}

/// The `n`th candidate for the prefix of a person's bundle store (`seed`:
/// who, and where the store is), which all their bundles share: `/tmp/`
/// and five letters, the same length as `/nix/store`.
pub fn prefix_candidate(seed: &str, n: u32) -> String {
    const ALPHABET: &[u8] = b"0123456789abcdfghijklmnpqrsvwxyz";
    let h = Sha256::digest(format!("tray-bundle:{seed}:{n}").as_bytes());
    let name: String = h[..5].iter().map(|b| ALPHABET[(*b & 31) as usize] as char).collect();
    let p = format!("/tmp/{name}");
    debug_assert_eq!(p.len(), STORE.len());
    p
}

const MAX_STR: u64 = 64 * 1024;

/// Hashes what's written through it, counting.
pub struct HashWriter {
    pub hash: Sha256,
    pub count: u64,
}

impl Write for HashWriter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.hash.update(b);
        self.count += b.len() as u64;
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Nix's hash of a path (`sha256:<base32>` of its NAR) and the NAR's size:
/// to tell whether a host store's copy is the very part a bundle carries.
pub fn nar_hash(path: &Path) -> io::Result<(String, u64)> {
    let mut w = HashWriter { hash: Sha256::new(), count: 0 };
    nar_str(&mut w, b"nix-archive-1")?;
    nar_node(path, &mut w)?;
    Ok((format!("sha256:{}", crate::nix::base32::encode(&w.hash.finalize())), w.count))
}

fn nar_str(w: &mut impl Write, s: &[u8]) -> io::Result<()> {
    w.write_all(&(s.len() as u64).to_le_bytes())?;
    w.write_all(s)?;
    w.write_all(&[0u8; 8][..(8 - s.len() % 8) % 8])
}

fn nar_node(path: &Path, w: &mut impl Write) -> io::Result<()> {
    let m = fs::symlink_metadata(path)?;
    nar_str(w, b"(")?;
    nar_str(w, b"type")?;
    if m.file_type().is_symlink() {
        nar_str(w, b"symlink")?;
        nar_str(w, b"target")?;
        nar_str(w, fs::read_link(path)?.as_os_str().as_bytes())?;
    } else if m.is_dir() {
        nar_str(w, b"directory")?;
        let mut names: Vec<std::ffi::OsString> = fs::read_dir(path)?.map(|e| e.map(|e| e.file_name())).collect::<io::Result<_>>()?;
        names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        for n in names {
            nar_str(w, b"entry")?;
            nar_str(w, b"(")?;
            nar_str(w, b"name")?;
            nar_str(w, n.as_bytes())?;
            nar_str(w, b"node")?;
            nar_node(&path.join(&n), w)?;
            nar_str(w, b")")?;
        }
    } else {
        nar_str(w, b"regular")?;
        if m.permissions().mode() & 0o100 != 0 {
            nar_str(w, b"executable")?;
            nar_str(w, b"")?;
        }
        nar_str(w, b"contents")?;
        w.write_all(&m.len().to_le_bytes())?;
        let n = io::copy(&mut File::open(path)?, w)?;
        if n != m.len() {
            return Err(bad(format!("{} changed while being read", path.display())));
        }
        w.write_all(&[0u8; 8][..((8 - n % 8) % 8) as usize])?;
    }
    nar_str(w, b")")
}

/// Unpack one NAR into `dest` (which must not exist), every `from` in it
/// (contents and link targets) replaced by `to`. Files keep nix's mtime of
/// 1 (Python's caches compare it), and are read-only like the store's;
/// folders stay writable so the cache can be cleaned.
pub fn unpack(r: &mut impl Read, dest: &Path, from: &[u8], to: &[u8]) -> io::Result<()> {
    expect(r, "nix-archive-1")?;
    node(r, dest, from, to)
}

fn node(r: &mut impl Read, path: &Path, from: &[u8], to: &[u8]) -> io::Result<()> {
    expect(r, "(")?;
    expect(r, "type")?;
    match read_str(r)?.as_str() {
        "regular" => {
            let mut tag = read_str(r)?;
            let executable = tag == "executable";
            if executable {
                expect(r, "")?;
                tag = read_str(r)?;
            }
            if tag != "contents" {
                return Err(bad("NAR: expected contents"));
            }
            let len = read_u64(r)?;
            let mut data = Vec::with_capacity(len.min(1 << 30) as usize);
            r.take(len).read_to_end(&mut data)?;
            if data.len() as u64 != len {
                return Err(bad("NAR: truncated"));
            }
            skip_padding(r, len)?;
            if !from.is_empty() {
                replace_all(&mut data, from, to);
            }
            let mut f = File::create(path)?;
            f.write_all(&data)?;
            f.set_permissions(fs::Permissions::from_mode(if executable { 0o555 } else { 0o444 }))?;
            set_mtime(&f)?;
            expect(r, ")")
        }
        "symlink" => {
            expect(r, "target")?;
            let mut target = read_str(r)?.into_bytes();
            if !from.is_empty() {
                replace_all(&mut target, from, to);
            }
            symlink(std::ffi::OsStr::from_bytes(&target), path)?;
            expect(r, ")")
        }
        "directory" => {
            fs::create_dir(path)?;
            loop {
                match read_str(r)?.as_str() {
                    ")" => break,
                    "entry" => {
                        expect(r, "(")?;
                        expect(r, "name")?;
                        let name = read_str(r)?;
                        if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
                            return Err(bad(format!("NAR: unsafe name {name:?}")));
                        }
                        expect(r, "node")?;
                        node(r, &path.join(name), from, to)?;
                        expect(r, ")")?;
                    }
                    other => return Err(bad(format!("NAR: unexpected {other:?}"))),
                }
            }
            set_mtime(&File::open(path)?)?;
            Ok(())
        }
        other => Err(bad(format!("NAR: unknown node type {other:?}"))),
    }
}

fn set_mtime(f: &File) -> io::Result<()> {
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
    f.set_times(fs::FileTimes::new().set_modified(t).set_accessed(t))
}

fn read_u64(r: &mut impl Read) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn skip_padding(r: &mut impl Read, len: u64) -> io::Result<()> {
    let pad = ((8 - len % 8) % 8) as usize;
    let mut b = [0u8; 8];
    r.read_exact(&mut b[..pad])?;
    if b[..pad].iter().any(|x| *x != 0) {
        return Err(bad("NAR: non-zero padding"));
    }
    Ok(())
}

fn read_str(r: &mut impl Read) -> io::Result<String> {
    let len = read_u64(r)?;
    if len > MAX_STR {
        return Err(bad("NAR: string too long"));
    }
    let mut b = vec![0u8; len as usize];
    r.read_exact(&mut b)?;
    skip_padding(r, len)?;
    String::from_utf8(b).map_err(|_| bad("NAR: not UTF-8"))
}

fn expect(r: &mut impl Read, want: &str) -> io::Result<()> {
    let got = read_str(r)?;
    if got != want {
        return Err(bad(format!("NAR: expected {want:?}, got {got:?}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailer_round_trip() {
        let t = Trailer { payload_offset: 1000, meta_offset: 5000, meta_len: 321, meta_sha256: [7; 32] };
        assert_eq!(Trailer::decode(&t.encode()), Some(t));
        let mut broken = t.encode();
        broken[60] ^= 1;
        assert_eq!(Trailer::decode(&broken), None);
    }

    /// A bundle of no parts whose runtime is `runtime`.
    fn tiny_bundle(path: &Path, runtime: &[u8]) {
        let meta = serde_json::to_vec(&Meta { format: 1, name: "x".into(), entry: "/nix/store/x".into(), ..Default::default() }).unwrap();
        let t = Trailer { payload_offset: runtime.len() as u64, meta_offset: runtime.len() as u64, meta_len: meta.len() as u64, meta_sha256: Sha256::digest(&meta).into() };
        fs::write(path, [runtime, &meta[..], &t.encode()[..]].concat()).unwrap();
    }

    #[test]
    fn speaks_each_runtime_its_words() {
        let dir = std::env::temp_dir().join(format!("nami-bundle-words-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // Made before the rename: tray's words only. 0.18.6 to 0.18.8: nami's. Since: both.
        let cases: [(&[u8], &str); 3] = [(b"\x7fELF.. --tray-bundle- ..", "tray"), (b"\x7fELF.. --nami-bundle- ..", "nami"), (b"\x7fELF --tray-bundle- --nami-bundle-", "nami")];
        for (runtime, says) in cases {
            let f = dir.join("b");
            tiny_bundle(&f, runtime);
            assert!(is_bundle(&f));
            assert_eq!(runtime_says(&f), says);
            assert_eq!(flag(&f, "clean"), format!("--{says}-bundle-clean"));
        }
        // Not a bundle (an AppImage: its AppRun is inside): nami's.
        fs::write(dir.join("c"), b"--tray-bundle-").unwrap();
        assert_eq!(runtime_says(&dir.join("c")), "nami");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn picks_the_store_a_bundle_is_in() {
        let dir = std::env::temp_dir().join(format!("nami-bundle-pick-{}", std::process::id()));
        let (tray, nami) = (dir.join("tray-bundle"), dir.join("nami-bundle"));
        let dirs = vec![tray.clone(), nami.clone()];
        let _ = fs::remove_dir_all(&dir);
        // None yet: tray's name.
        assert_eq!(pick_cache(&dirs, Some("h")), Some(tray.clone()));
        // Only nami's has a store (0.18.6 to 0.18.8 with XDG_CACHE_HOME): that one.
        fs::create_dir_all(nami.join("store")).unwrap();
        assert_eq!(pick_cache(&dirs, None), Some(nami.clone()));
        // Both: tray's, but a bundle unpacked in nami's stays there.
        fs::create_dir_all(tray.join("store")).unwrap();
        fs::create_dir_all(nami.join("bundles")).unwrap();
        fs::write(nami.join("bundles/h"), "").unwrap();
        assert_eq!(pick_cache(&dirs, None), Some(tray.clone()));
        assert_eq!(pick_cache(&dirs, Some("h")), Some(nami.clone()));
        assert_eq!(pick_cache(&dirs, Some("other")), Some(tray));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replaces_same_length() {
        let mut b = b"x/nix/store/abc /nix/stor /nix/store/def/nix/store".to_vec();
        assert_eq!(replace_all(&mut b, b"/nix/store", b"/tmp/12345"), 3);
        assert_eq!(&b[..], b"x/tmp/12345/abc /nix/stor /tmp/12345/def/tmp/12345");
        let mut short = b"/nix".to_vec();
        assert_eq!(replace_all(&mut short, b"/nix/store", b"/tmp/12345"), 0);
    }

    #[test]
    fn prefixes_are_store_length_and_differ() {
        let a = prefix_candidate("1000:/home/a/.cache/tray-bundle", 0);
        assert_eq!(a.len(), STORE.len());
        assert!(a.starts_with("/tmp/"));
        assert_ne!(a, prefix_candidate("1000:/home/a/.cache/tray-bundle", 1));
        assert_ne!(a, prefix_candidate("1001:/home/a/.cache/tray-bundle", 0));
        assert_eq!(a, prefix_candidate("1000:/home/a/.cache/tray-bundle", 0));
    }

    #[test]
    fn the_runtime_is_marked_where_nothing_reads() {
        let mut elf = b"\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x03\x00".to_vec();
        mark_runtime(&mut elf);
        assert_eq!(&elf[9..16], ELF_MARK);
        assert_eq!(&elf[..9], b"\x7fELF\x02\x01\x01\x00\x00");
        let again = elf.clone();
        mark_runtime(&mut elf);
        assert_eq!(elf, again);
    }

    #[test]
    fn says_its_architecture() {
        // Made before it said: x86_64, and its JSON reads as before.
        let old: Meta = serde_json::from_str(r#"{"format":1,"name":"jq","entry":"/nix/store/x","paths":[]}"#).unwrap();
        assert_eq!(old.arch(), "x86_64");
        let arm = Meta { name: "jq".into(), arch: "aarch64".into(), ..Default::default() };
        let json = serde_json::to_string(&arm).unwrap();
        assert!(json.contains(r#""arch":"aarch64""#));
        let back: Meta = serde_json::from_str(&json).unwrap();
        assert_eq!(back.arch(), "aarch64");
        // This computer's runs; the other's is refused, saying why.
        let here = Meta { arch: std::env::consts::ARCH.into(), ..Default::default() };
        assert_eq!(here.arch_problem(), None);
        let other = if std::env::consts::ARCH == "aarch64" { "x86_64" } else { "aarch64" };
        let p = Meta { name: "jq".into(), arch: other.into(), ..Default::default() }.arch_problem().unwrap();
        assert!(p.starts_with(&format!("jq is made for {} computers", arch_name(other))), "{p}");
        let mut elf = vec![0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0xb7, 0];
        assert_eq!(elf_arch(&elf), Some("aarch64"));
        elf[18] = 0x3e;
        assert_eq!(elf_arch(&elf), Some("x86_64"));
        assert_eq!(elf_arch(b"#!/bin/sh"), None);
    }

    #[test]
    fn names() {
        assert!(valid_name("0123456789abcdfghijklmnpqrsvwxyz-jq-1.8"));
        assert!(!valid_name("../../etc"));
        assert!(!valid_name("0123456789abcdfghijklmnpqrsvwxyz/x"));
    }

    fn s(out: &mut Vec<u8>, v: &[u8]) {
        out.extend((v.len() as u64).to_le_bytes());
        out.extend(v);
        out.resize(out.len() + (8 - v.len() % 8) % 8, 0);
    }

    #[test]
    fn unpacks_rewriting_store_paths() {
        let mut nar = Vec::new();
        for t in ["nix-archive-1", "(", "type", "directory", "entry", "(", "name", "bin", "node", "(", "type", "directory", "entry", "(", "name", "hi", "node", "(", "type", "regular", "executable", "", "contents"] {
            s(&mut nar, t.as_bytes());
        }
        s(&mut nar, b"#!/nix/store/aaa-bash/bin/sh\necho hi\n");
        for t in [")", ")", ")", ")", "entry", "(", "name", "lib", "node", "(", "type", "symlink", "target", "/nix/store/bbb-lib/lib", ")", ")", ")"] {
            s(&mut nar, t.as_bytes());
        }
        let dir = std::env::temp_dir().join(format!("nami-bundle-fmt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out");
        unpack(&mut nar.as_slice(), &out, b"/nix/store", b"/tmp/abcde").unwrap();
        assert_eq!(fs::read_to_string(out.join("bin/hi")).unwrap(), "#!/tmp/abcde/aaa-bash/bin/sh\necho hi\n");
        assert_eq!(fs::read_link(out.join("lib")).unwrap().to_str(), Some("/tmp/abcde/bbb-lib/lib"));
        use std::os::unix::fs::MetadataExt;
        assert_eq!(fs::metadata(out.join("bin/hi")).unwrap().mtime(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }
}
