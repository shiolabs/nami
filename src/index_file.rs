//! The catalog as nami reads it: the JSON (index.json, or a kept copy in
//! menus/), and next to it a compact form made from it (`.idx`): every
//! package's name in order, where its entry is in the JSON, and what search
//! looks at. Both are read in place (mmap), so looking a package up reads
//! that package's few hundred bytes, not the 40-60 MB around it, and
//! nothing is parsed up front.
//!
//! The JSON stays what's checked and trusted (its signature); the compact
//! form is only ever made from it, here, and says which file it was made
//! from (device, inode, size, modification time). One that's missing or
//! was made from other bytes is made again, and one made from the same
//! bytes in another file (a copy: its sha256 matches) is kept, with a note
//! beside it that it's this file's (`tied`). Made again: next to the JSON
//! where nami may write, else in the user's cache, else in memory for this
//! run.
//!
//! Layout (little-endian):
//!   0   "NAMIIDX1"
//!   8   the JSON's size (u64), 16 its sha256 (32 bytes)
//!   48  the JSON file it was made from: dev, ino (u64), mtime s (i64), ns (u64)
//!   80  how many packages (u32), 84 how many of them this computer can install (u32:
//!       `installable`; 0 in one made before nami kept it)
//!   88  per package, by name: name (offset, length), its entry in the JSON
//!       (offset, length), its search record (offset, length): 6 u32s
//!   then the names and search records.
//!
//! A search record: flags (u8: 1 an app, 2 names a binary cache), the
//! groups it's in (u8, a bit per `query::CATEGORIES`), popularity (u16),
//! then its title, summary and description in lowercase and its icon's
//! sha256 (u32 length + text each), and its search words (u16 count, then
//! each as the texts).

use std::fs::{self, File};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};

use crate::index::Package;

const MAGIC: &[u8; 8] = b"NAMIIDX1";
const HEAD: usize = 88;
const ENTRY: usize = 24;
pub const APP: u8 = 1;
pub const NAMES_CACHE: u8 = 2;

/// Bytes read in place, or held (made in memory).
pub enum Bytes {
    Mapped(Mmap),
    Held(Vec<u8>),
}

impl std::ops::Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Bytes::Mapped(m) => m.as_slice(),
            Bytes::Held(v) => v,
        }
    }
}

/// A file mapped read-only. The files mapped are only ever replaced (renamed
/// over), never written in place, so what's mapped doesn't change under it.
pub struct Mmap {
    ptr: *mut libc::c_void,
    len: usize,
}

// Read-only memory, unmapped only when dropped.
unsafe impl Send for Mmap {}
unsafe impl Sync for Mmap {}

impl Mmap {
    fn as_slice(&self) -> &[u8] {
        // SAFETY: ptr..ptr+len is a live PROT_READ mapping owned by self.
        unsafe { std::slice::from_raw_parts(self.ptr as *const u8, self.len) }
    }
}

impl Drop for Mmap {
    fn drop(&mut self) {
        // SAFETY: mapped in `map` with this length, unmapped once.
        unsafe { libc::munmap(self.ptr, self.len) };
    }
}

/// `file`, mapped (an empty one is held: there's nothing to map).
fn map(file: &File, len: u64) -> Result<Bytes> {
    use std::os::fd::AsRawFd;
    if len == 0 {
        return Ok(Bytes::Held(Vec::new()));
    }
    let len = usize::try_from(len)?;
    // SAFETY: a fresh private read-only mapping of an open file.
    let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_PRIVATE, file.as_raw_fd(), 0) };
    if ptr == libc::MAP_FAILED {
        return Err(std::io::Error::last_os_error()).context("mapping the catalog");
    }
    Ok(Bytes::Mapped(Mmap { ptr, len }))
}

/// Which file a compact form was made from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Identity {
    dev: u64,
    ino: u64,
    mtime: i64,
    mtime_ns: u64,
}

impl Identity {
    pub fn of(m: &fs::Metadata) -> Self {
        Identity { dev: m.dev(), ino: m.ino(), mtime: m.mtime(), mtime_ns: m.mtime_nsec() as u64 }
    }
}

fn u32_at(b: &[u8], at: usize) -> usize {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// What making the compact form found out, for `nami update`.
pub struct Made {
    pub idx: Vec<u8>,
    pub count: usize,
    /// The binary caches the catalog names (`index::catalog_caches`).
    pub caches: Vec<crate::store::Cache>,
    pub any_cache: bool,
}

/// The compact form of catalog `json` (checking that every entry reads as a
/// package), not yet tied to a file (`tie`).
pub fn make(json: &[u8]) -> Result<Made> {
    use serde_json::value::RawValue;
    let raw: std::collections::BTreeMap<String, &RawValue> = serde_json::from_slice(json).context("parsing package index")?;
    ensure!(json.len() < u32::MAX as usize, "the catalog is too big (over 4 GB)");
    let base = json.as_ptr() as usize;
    let mut names: Vec<u8> = Vec::new();
    let mut records: Vec<u8> = Vec::new();
    let mut entries: Vec<[usize; 6]> = Vec::with_capacity(raw.len());
    let mut packages_with_caches = Vec::new();
    let mut can_install = 0;
    for (name, value) in &raw {
        let pkg: Package = serde_json::from_str(value.get()).with_context(|| format!("parsing package index: {name}"))?;
        can_install += installable(&pkg) as usize;
        let at = value.get().as_ptr() as usize - base;
        let key = (names.len(), name.len());
        names.extend_from_slice(name.as_bytes());
        let rec_at = records.len();
        let mut flags = 0;
        if pkg.apps.is_some() {
            flags |= APP;
        }
        if pkg.cache.is_some() {
            flags |= NAMES_CACHE;
        }
        let mut groups = 0u8;
        for (i, (_, _, cats)) in crate::query::CATEGORIES.iter().enumerate() {
            if pkg.categories.iter().any(|c| cats.contains(&c.as_str())) {
                groups |= 1 << i;
            }
        }
        records.push(flags);
        records.push(groups);
        records.extend_from_slice(&pkg.popularity.unwrap_or(0).to_le_bytes());
        let mut text = |t: &str| {
            let t = t.to_lowercase();
            records.extend_from_slice(&(t.len() as u32).to_le_bytes());
            records.extend_from_slice(t.as_bytes());
        };
        text(pkg.title.as_deref().unwrap_or_default());
        text(pkg.summary.as_deref().unwrap_or_default());
        text(pkg.description.as_deref().unwrap_or_default());
        text(pkg.icon.as_deref().unwrap_or_default());
        let words: Vec<&String> = pkg.keywords.iter().take(u16::MAX as usize).collect();
        records.extend_from_slice(&(words.len() as u16).to_le_bytes());
        for w in words {
            let w = w.to_lowercase();
            records.extend_from_slice(&(w.len() as u32).to_le_bytes());
            records.extend_from_slice(w.as_bytes());
        }
        entries.push([key.0, key.1, at, value.get().len(), rec_at, records.len() - rec_at]);
        if pkg.cache.is_some() {
            packages_with_caches.push(pkg);
        }
    }
    let names_at = HEAD + entries.len() * ENTRY;
    let records_at = names_at + names.len();
    ensure!(records_at + records.len() < u32::MAX as usize, "the catalog is too big");
    let mut idx = Vec::with_capacity(records_at + records.len());
    idx.extend_from_slice(MAGIC);
    idx.extend_from_slice(&(json.len() as u64).to_le_bytes());
    idx.extend_from_slice(&crate::catalog::sha256_raw(json));
    idx.extend_from_slice(&[0u8; 32]); // the file's identity: `tie`
    idx.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    idx.extend_from_slice(&(can_install as u32).to_le_bytes());
    for [k, kl, v, vl, r, rl] in &entries {
        for n in [k + names_at, *kl, *v, *vl, r + records_at, *rl] {
            idx.extend_from_slice(&(n as u32).to_le_bytes());
        }
    }
    idx.extend_from_slice(&names);
    idx.extend_from_slice(&records);
    Ok(Made {
        idx,
        count: entries.len(),
        caches: crate::index::catalog_caches(packages_with_caches.iter()),
        any_cache: !packages_with_caches.is_empty(),
    })
}

/// Whether this computer can install `pkg` from the catalog: on a Mac what's ready-built (nothing
/// is built on a Mac yet), on ARM Linux that or what has a build definition, elsewhere all of
/// them (what the cache hasn't got is built). As `Builder::ensure_catalog` refuses.
pub fn installable(pkg: &Package) -> bool {
    if crate::index::MAC {
        crate::casks::ready_built(pkg)
    } else if crate::index::ARM_LINUX {
        pkg.cache.is_some() || !pkg.uncached || pkg.closure.is_some()
    } else {
        true
    }
}

/// `idx`, as made from the file `id` names.
pub fn tie(idx: &mut [u8], id: Identity) {
    let mut at = 48;
    for n in [id.dev, id.ino, id.mtime as u64, id.mtime_ns] {
        idx[at..at + 8].copy_from_slice(&n.to_le_bytes());
        at += 8;
    }
}

/// Is `idx` a whole compact form, made from a file of `size` bytes with
/// identity `id` (None: any)? Every offset in it is checked, so reading it
/// later can't go out of bounds.
fn valid(idx: &[u8], size: u64, id: Option<Identity>) -> bool {
    if idx.len() < HEAD || &idx[..8] != MAGIC || u64_at(idx, 8) != size {
        return false;
    }
    if let Some(id) = id {
        let theirs = Identity { dev: u64_at(idx, 48), ino: u64_at(idx, 56), mtime: u64_at(idx, 64) as i64, mtime_ns: u64_at(idx, 72) };
        if theirs != id {
            return false;
        }
    }
    let n = u32_at(idx, 80);
    let Some(end) = n.checked_mul(ENTRY).map(|e| e + HEAD).filter(|e| *e <= idx.len()) else { return false };
    (0..n).all(|i| {
        let e = HEAD + i * ENTRY;
        let fits = |at: usize, len: usize, limit: usize| at.checked_add(len).is_some_and(|x| x <= limit);
        fits(u32_at(idx, e), u32_at(idx, e + 4), idx.len())
            && u32_at(idx, e) >= end
            && fits(u32_at(idx, e + 8), u32_at(idx, e + 12), size as usize)
            && fits(u32_at(idx, e + 16), u32_at(idx, e + 20), idx.len())
            && u32_at(idx, e + 20) >= 22
    })
}

/// A catalog, ready to look things up in.
pub struct Catalog {
    json: Bytes,
    idx: Bytes,
    n: usize,
}

/// What search looks at, of one package.
pub struct Record<'a> {
    pub flags: u8,
    pub groups: u8,
    pub popularity: u16,
    pub title: &'a str,
    pub summary: &'a str,
    pub description: &'a str,
    /// The sha256 of its icon, if it has one.
    pub icon: &'a str,
    words: &'a [u8],
}

impl<'a> Record<'a> {
    /// Its search words, in lowercase.
    pub fn keywords(&self) -> impl Iterator<Item = &'a str> + 'a {
        let b = self.words;
        let n = if b.len() >= 2 { u16::from_le_bytes([b[0], b[1]]) as usize } else { 0 };
        let mut at = 2;
        (0..n).map_while(move |_| {
            let len = u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize;
            let t = std::str::from_utf8(b.get(at + 4..at + 4 + len)?).ok()?;
            at += 4 + len;
            Some(t)
        })
    }
}

impl Catalog {
    /// The catalog in `json` (a file), with its compact form: the one next
    /// to it or in `cache` if it was made from this very file, else made now
    /// (and kept where it may be).
    pub fn open(json: &Path, cache: Option<&Path>) -> Result<Catalog> {
        let file = File::open(json)?;
        let meta = file.metadata()?;
        let id = Identity::of(&meta);
        let places = places(json, cache);
        for p in &places {
            if let Some(idx) = read_idx(p, meta.len(), id) {
                let data = map(&file, meta.len())?;
                return Ok(Catalog::from_parts(data, idx));
            }
        }
        let data = map(&file, meta.len())?;
        // One made from a copy of this file (another disk, an image, a
        // restored backup): the same bytes if their sha256 is the one it
        // was made from. Checking that is far quicker than making it again.
        // That's kept beside it (`tied`), so the copy is never written again.
        let sha = crate::catalog::sha256_raw(&data);
        let copied = places.iter().find_map(|p| {
            let f = File::open(p).ok()?;
            let m = f.metadata().ok()?;
            let idx = map(&f, m.len()).ok()?;
            (valid(&idx, meta.len(), None) && idx[16..48] == sha[..]).then(|| (p, idx, Identity::of(&m)))
        });
        let mut idx = match copied {
            Some((p, idx, its)) if write(&tied(p), &tie_record(id, its)).is_ok() => return Ok(Catalog::from_parts(data, idx)),
            Some((_, idx, _)) => idx.to_vec(),
            None => make(&data).with_context(|| format!("reading {}", json.display()))?.idx,
        };
        tie(&mut idx, id);
        for p in &places {
            let ok = p.parent().is_some_and(|d| fs::create_dir_all(d).is_ok()) && write(p, &idx).is_ok();
            if ok {
                let _ = fs::remove_file(tied(p));
                break;
            }
        }
        Ok(Catalog::from_parts(data, Bytes::Held(idx)))
    }

    /// A catalog held in memory (another platform's, for `nami bundle --arch`).
    pub fn from_vec(json: Vec<u8>) -> Result<Catalog> {
        let made = make(&json)?;
        Ok(Catalog::from_parts(Bytes::Held(json), Bytes::Held(made.idx)))
    }

    fn from_parts(json: Bytes, idx: Bytes) -> Catalog {
        let n = u32_at(&idx, 80);
        Catalog { json, idx, n }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    /// How many of its packages this computer can install (`installable`). None from a compact
    /// form made before nami kept it.
    pub fn installable(&self) -> Option<usize> {
        Some(u32_at(&self.idx, 84)).filter(|&n| n > 0)
    }

    /// The JSON's sha256 (hex): which catalog it is.
    pub fn sha256(&self) -> String {
        hex(&self.idx[16..48])
    }

    /// The JSON, as it is on disk.
    pub fn json(&self) -> &[u8] {
        &self.json
    }

    fn entry(&self, i: usize, field: usize) -> (usize, usize) {
        let e = HEAD + i * ENTRY + field * 8;
        (u32_at(&self.idx, e), u32_at(&self.idx, e + 4))
    }

    /// The `i`th package's name (in sorted order).
    pub fn name(&self, i: usize) -> &str {
        let (at, len) = self.entry(i, 0);
        std::str::from_utf8(&self.idx[at..at + len]).unwrap_or_default()
    }

    /// Its entry in the JSON.
    pub fn value(&self, i: usize) -> &[u8] {
        let (at, len) = self.entry(i, 1);
        &self.json[at..at + len]
    }

    pub fn record(&self, i: usize) -> Record<'_> {
        let (at, len) = self.entry(i, 2);
        let r = &self.idx[at..at + len];
        let mut pos = 4;
        let mut text = || {
            let len = r.get(pos..pos + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
            let t = r.get(pos + 4..pos + 4 + len).and_then(|t| std::str::from_utf8(t).ok()).unwrap_or_default();
            pos += 4 + len;
            t
        };
        let (title, summary, description, icon) = (text(), text(), text(), text());
        Record { flags: r[0], groups: r[1], popularity: u16::from_le_bytes([r[2], r[3]]), title, summary, description, icon, words: r.get(pos..).unwrap_or_default() }
    }

    /// (name, sha256) of every package with an icon.
    pub fn icons(&self) -> Vec<(&str, &str)> {
        (0..self.n).map(|i| (self.name(i), self.record(i).icon)).filter(|(_, sha)| !sha.is_empty()).collect()
    }

    /// Where `name` is, if the catalog has it.
    pub fn find(&self, name: &str) -> Option<usize> {
        let (mut lo, mut hi) = (0, self.n);
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.name(mid).cmp(name) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return Some(mid),
            }
        }
        None
    }

    /// The first package whose name is `prefix` or after it.
    pub fn first_from(&self, prefix: &str) -> usize {
        let (mut lo, mut hi) = (0, self.n);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.name(mid) < prefix {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }
}

/// Where a catalog file's compact form may be: next to it, and in `cache`
/// (named by the file's path).
fn places(json: &Path, cache: Option<&Path>) -> Vec<PathBuf> {
    let mut out = vec![json.with_extension("idx")];
    if let Some(c) = cache {
        let abs = fs::canonicalize(json).unwrap_or_else(|_| json.to_path_buf());
        out.push(c.join(format!("{}.idx", &crate::catalog::sha256_hex(abs.as_os_str().as_encoded_bytes())[..16])));
    }
    out
}

/// The compact form at `path`, if it's whole and was made from the file
/// of `size` bytes with identity `id`, or from the same bytes as it (`tied`).
fn read_idx(path: &Path, size: u64, id: Identity) -> Option<Bytes> {
    let f = File::open(path).ok()?;
    let m = f.metadata().ok()?;
    let idx = map(&f, m.len()).ok()?;
    if valid(&idx, size, Some(id)) {
        return Some(idx);
    }
    let same = fs::read(tied(path)).is_ok_and(|t| t == tie_record(id, Identity::of(&m)));
    (same && valid(&idx, size, None)).then_some(idx)
}

/// Where it's kept that the compact form at `idx` (that very file), made
/// from another file, was found to be made from the same bytes as the
/// catalog file with an identity: a copy's (`Catalog::open`). 64 bytes,
/// where tying the form itself would write all of it again (a computer
/// image's, over a layer: 16 MB more on every computer's disk).
fn tied(idx: &Path) -> PathBuf {
    PathBuf::from(format!("{}-tie", idx.display()))
}

/// "NAMITIE1", the catalog file's identity, the compact form's.
fn tie_record(json: Identity, idx: Identity) -> Vec<u8> {
    let mut out = b"NAMITIE1".to_vec();
    for id in [json, idx] {
        for n in [id.dev, id.ino, id.mtime as u64, id.mtime_ns] {
            out.extend_from_slice(&n.to_le_bytes());
        }
    }
    out
}

/// The sha256 of the catalog file `json`: from its compact form when that
/// was made from this very file, else by reading it.
pub fn sha256_of(json: &Path, cache: Option<&Path>) -> Result<String> {
    let file = File::open(json)?;
    let meta = file.metadata()?;
    let id = Identity::of(&meta);
    for p in places(json, cache) {
        if let Some(idx) = read_idx(&p, meta.len(), id) {
            return Ok(hex(&idx[16..48]));
        }
    }
    let data = map(&file, meta.len())?;
    Ok(crate::catalog::sha256_hex(&data))
}

/// Keep `idx` (made from the same JSON) as the compact form of the file
/// `json`, just written.
pub fn keep(json: &Path, idx: &[u8]) -> Result<()> {
    let meta = fs::metadata(json)?;
    if !valid(idx, meta.len(), None) {
        bail!("not a compact form of {}", json.display());
    }
    let mut idx = idx.to_vec();
    tie(&mut idx, Identity::of(&meta));
    let at = json.with_extension("idx");
    write(&at, &idx)?;
    let _ = fs::remove_file(tied(&at));
    Ok(())
}

/// Write `path` whole or not at all (a temporary file of its own, renamed).
fn write(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = PathBuf::from(format!("{}.{}.tmp", path.display(), std::process::id()));
    let done = fs::write(&tmp, data).and_then(|_| fs::rename(&tmp, path));
    if done.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(done?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many this computer can install, kept in the compact form: a Mac's ready-built ones, ARM
    /// Linux's and those with a build definition, all of them elsewhere; none said in an old one.
    #[test]
    fn says_how_many_this_computer_can_install() {
        let text = serde_json::json!({
            "built": {"name": "built-1", "outputs": {}},
            "uncached": {"name": "uncached-1", "outputs": {}, "uncached": true},
            "buildable": {"name": "buildable-1", "outputs": {}, "uncached": true, "closure": "0000"},
        })
        .to_string();
        let want = if crate::index::MAC { 1 } else if crate::index::ARM_LINUX { 2 } else { 3 };
        let c = Catalog::from_vec(text.clone().into_bytes()).unwrap();
        assert_eq!((c.len(), c.installable()), (3, Some(want)));
        let mut old = make(text.as_bytes()).unwrap().idx;
        old[84..88].copy_from_slice(&[0; 4]);
        assert!(valid(&old, text.len() as u64, None));
        assert_eq!(Catalog::from_parts(Bytes::Held(text.into_bytes()), Bytes::Held(old)).installable(), None);
    }

    #[test]
    fn looks_up_without_reading_everything() {
        let dir = std::env::temp_dir().join(format!("nami-index-file-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let json = dir.join("index.json");
        let text = serde_json::json!({
            "zed": {"name": "zed-1.0", "outputs": {"out": "/nix/store/00000000000000000000000000000000-zed-1.0"}, "apps": ["zed.desktop"],
                "title": "Zed", "keywords": ["Editor", "Code"], "categories": ["Development"], "popularity": 700},
            "hello": {"name": "hello-2.12", "outputs": {"out": "/nix/store/11111111111111111111111111111111-hello-2.12"}, "description": "Says Hello"},
            "a\"quoted": {"name": "q-1", "outputs": {}},
        })
        .to_string();
        fs::write(&json, &text).unwrap();
        let c = Catalog::open(&json, None).unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!(c.sha256(), crate::catalog::sha256_hex(text.as_bytes()));
        assert_eq!((c.name(0), c.name(1), c.name(2)), ("a\"quoted", "hello", "zed"));
        let z = c.find("zed").unwrap();
        let pkg: Package = serde_json::from_slice(c.value(z)).unwrap();
        assert_eq!(pkg.name, "zed-1.0");
        let r = c.record(z);
        assert_eq!((r.flags, r.groups, r.popularity, r.title), (APP, 1 << 5, 700, "zed"));
        assert_eq!(r.keywords().collect::<Vec<_>>(), ["editor", "code"]);
        assert_eq!(c.record(c.find("hello").unwrap()).description, "says hello");
        assert!(c.find("nope").is_none());
        assert_eq!(c.first_from("h"), 1);
        assert_eq!(c.installable(), Some(3));
        // Kept next to it, and used again as long as it's that file.
        let idx = fs::read(json.with_extension("idx")).unwrap();
        assert!(valid(&idx, text.len() as u64, Some(Identity::of(&fs::metadata(&json).unwrap()))));
        assert_eq!(sha256_of(&json, None).unwrap(), c.sha256());
        // Another file in its place: made again.
        std::thread::sleep(std::time::Duration::from_millis(10));
        let other = text.replace("Says Hello", "Says Hi!!!");
        crate::store::write_atomic(&json, other.as_bytes()).unwrap();
        let c = Catalog::open(&json, None).unwrap();
        assert_eq!(c.sha256(), crate::catalog::sha256_hex(other.as_bytes()));
        assert_eq!(c.record(c.find("hello").unwrap()).description, "says hi!!!");
        // A copy of both (another disk, an image): the copy's compact form is
        // used, tied to the new file, not made again.
        let copy = dir.join("copy");
        fs::create_dir_all(&copy).unwrap();
        fs::copy(&json, copy.join("index.json")).unwrap();
        // Marked, to tell it apart from one made again: hello's description
        // (its third text, after the record's 4 bytes and two empty texts).
        let mut marked = fs::read(json.with_extension("idx")).unwrap();
        let at = u32_at(&marked, HEAD + ENTRY + 16) + 4 + 4 + 4 + 4;
        marked[at] = b'S';
        fs::write(copy.join("index.idx"), &marked).unwrap();
        let c = Catalog::open(&copy.join("index.json"), None).unwrap();
        assert_eq!(c.record(c.find("hello").unwrap()).description, "Says hi!!!");
        // The copy's form is left as it was; beside it, that it's this file's.
        assert_eq!(fs::read(copy.join("index.idx")).unwrap(), marked);
        assert_eq!(fs::metadata(copy.join("index.idx-tie")).unwrap().len(), 72);
        assert_eq!(sha256_of(&copy.join("index.json"), None).unwrap(), crate::catalog::sha256_hex(other.as_bytes()));
        let c = Catalog::open(&copy.join("index.json"), None).unwrap();
        assert_eq!(c.record(c.find("hello").unwrap()).description, "Says hi!!!");
        // ...but not one made from other bytes of the same size.
        fs::write(copy.join("index.json"), text.replace("Says Hello", "Says Ho!!!!")).unwrap();
        let c = Catalog::open(&copy.join("index.json"), None).unwrap();
        assert_eq!(c.record(c.find("hello").unwrap()).description, "says ho!!!!");
        assert!(!copy.join("index.idx-tie").exists());
        // A broken one is made again too.
        fs::write(json.with_extension("idx"), b"NAMIIDX1 broken").unwrap();
        assert_eq!(Catalog::open(&json, None).unwrap().len(), 3);
        // Where it can't be written: kept in the cache instead.
        let cache = dir.join("cache");
        fs::remove_file(json.with_extension("idx")).unwrap();
        fs::create_dir_all(json.with_extension("idx")).unwrap(); // in the way
        Catalog::open(&json, Some(&cache)).unwrap();
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);
        assert_eq!(Catalog::open(&json, Some(&cache)).unwrap().len(), 3);
        // Not a catalog: refused.
        fs::write(&json, "[1, 2]").unwrap();
        assert!(Catalog::open(&json, None).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
