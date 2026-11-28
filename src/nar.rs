//! NAR ("Nix ARchive") unpacking.
//!
//! Format: a stream of length-prefixed, 8-byte-padded strings.
//!   "nix-archive-1" node
//!   node := "(" "type" ( "regular" ["executable" ""] "contents" <bytes>
//!                      | "symlink" "target" <str>
//!                      | "directory" ("entry" "(" "name" <str> "node" node ")")* ) ")"

use std::fs::{self, File, FileTimes, Permissions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};

const MAX_STR: u64 = 64 * 1024;

/// Names that differ only in case, in one folder, on a Mac's store (APFS ignores case): the
/// second and later get this and a number (`Makefile~nix~case~hack~1`), as Nix does on macOS
/// (its `use-case-hack`), so a store Nix shares reads the same. Archiving takes it off again.
pub const CASE_HACK_SUFFIX: &str = "~nix~case~hack~";

/// Whether this computer's store needs the case hack: on macOS, as Nix's default there.
pub const CASE_HACK: bool = cfg!(target_os = "macos");

/// Wraps a reader, hashing and counting everything read through it.
pub struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
    pub count: u64,
}

impl<R: Read> HashingReader<R> {
    pub fn new(inner: R) -> Self {
        HashingReader { inner, hasher: Sha256::new(), count: 0 }
    }

    /// `sha256:<nixbase32>`, the form used in narinfo `NarHash`.
    pub fn finish(self) -> String {
        format!("sha256:{}", crate::nix::base32::encode(&self.hasher.finalize()))
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.count += n as u64;
        Ok(n)
    }
}

/// Serialise `path` as a NAR into `w` (the inverse of `unpack`).
pub fn dump(path: &Path, w: &mut impl Write) -> Result<()> {
    dump_with(path, w, CASE_HACK)
}

fn dump_with(path: &Path, w: &mut impl Write, hack: bool) -> Result<()> {
    write_str(w, b"nix-archive-1")?;
    dump_node(path, w, hack)
}

fn dump_node(path: &Path, w: &mut impl Write, hack: bool) -> Result<()> {
    let meta = path.symlink_metadata().with_context(|| format!("reading {}", path.display()))?;
    write_str(w, b"(")?;
    write_str(w, b"type")?;
    if meta.file_type().is_symlink() {
        write_str(w, b"symlink")?;
        write_str(w, b"target")?;
        write_str(w, fs::read_link(path)?.as_os_str().as_encoded_bytes())?;
    } else if meta.is_file() {
        write_str(w, b"regular")?;
        if meta.permissions().mode() & 0o100 != 0 {
            write_str(w, b"executable")?;
            write_str(w, b"")?;
        }
        write_str(w, b"contents")?;
        w.write_all(&meta.len().to_le_bytes())?;
        let copied = io::copy(&mut File::open(path).with_context(|| format!("reading {}", path.display()))?, w)?;
        if copied != meta.len() {
            bail!("{} changed while archiving", path.display());
        }
        w.write_all(&[0u8; 8][..((8 - meta.len() % 8) % 8) as usize])?;
    } else if meta.is_dir() {
        write_str(w, b"directory")?;
        let on_disk: Vec<_> = fs::read_dir(path)?.map(|e| e.map(|e| e.file_name())).collect::<io::Result<_>>()?;
        // (The name in the archive, the one on disk), in the archive's order.
        let mut names: Vec<(Vec<u8>, std::ffi::OsString)> = Vec::new();
        for name in on_disk {
            let bytes = name.as_encoded_bytes();
            let archived = match hack.then(|| find(bytes, CASE_HACK_SUFFIX.as_bytes())).flatten() {
                Some(at) => bytes[..at].to_vec(),
                None => bytes.to_vec(),
            };
            names.push((archived, name));
        }
        names.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some(twice) = names.windows(2).find(|p| p[0].0 == p[1].0) {
            bail!("{}: two files named {:?}", path.display(), String::from_utf8_lossy(&twice[0].0));
        }
        for (archived, name) in names {
            write_str(w, b"entry")?;
            write_str(w, b"(")?;
            write_str(w, b"name")?;
            write_str(w, &archived)?;
            write_str(w, b"node")?;
            dump_node(&path.join(&name), w, hack)?;
            write_str(w, b")")?;
        }
    } else {
        bail!("{}: can't archive special files", path.display());
    }
    write_str(w, b")")
}

fn write_str(w: &mut impl Write, s: &[u8]) -> Result<()> {
    w.write_all(&(s.len() as u64).to_le_bytes())?;
    w.write_all(s)?;
    w.write_all(&[0u8; 8][..(8 - s.len() % 8) % 8])?;
    Ok(())
}

/// Wraps a writer, hashing and counting everything written through it.
pub struct HashingWriter<W> {
    pub inner: W,
    hasher: Sha256,
    pub count: u64,
}

impl<W: Write> HashingWriter<W> {
    pub fn new(inner: W) -> Self {
        HashingWriter { inner, hasher: Sha256::new(), count: 0 }
    }

    pub fn finish(self) -> String {
        format!("sha256:{}", crate::nix::base32::encode(&self.hasher.finalize()))
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.count += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// `(nar_hash, nar_size)` of a path, without keeping the archive.
pub fn hash_path(path: &Path) -> Result<(String, u64)> {
    let mut w = HashingWriter::new(io::sink());
    dump(path, &mut w)?;
    let size = w.count;
    Ok((w.finish(), size))
}

/// Unpack a NAR from `r` into `dest`, which must not exist yet.
pub fn unpack(r: &mut impl Read, dest: &Path) -> Result<()> {
    unpack_with(r, dest, CASE_HACK)
}

fn unpack_with(r: &mut impl Read, dest: &Path, hack: bool) -> Result<()> {
    expect(r, "nix-archive-1")?;
    node(r, dest, hack)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn node(r: &mut impl Read, path: &Path, hack: bool) -> Result<()> {
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
                bail!("NAR: expected 'contents', got {tag:?}");
            }
            let len = read_u64(r)?;
            let mut f = File::create(path).with_context(|| format!("creating {}", path.display()))?;
            let copied = io::copy(&mut r.take(len), &mut f)?;
            if copied != len {
                bail!("NAR: truncated file contents");
            }
            skip_padding(r, len)?;
            f.flush()?;
            f.set_permissions(Permissions::from_mode(if executable { 0o555 } else { 0o444 })).with_context(|| format!("making {} read-only", path.display()))?;
            set_canonical_mtime(&f).with_context(|| format!("setting {}'s time", path.display()))?;

            expect(r, ")")
        }
        "symlink" => {
            expect(r, "target")?;
            symlink(read_str(r)?, path).with_context(|| format!("linking {}", path.display()))?;
            expect(r, ")")
        }
        "directory" => {
            fs::create_dir(path).with_context(|| format!("creating {}", path.display()))?;
            // With the case hack: each name, in lower case, and how many have had it.
            let mut seen: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
            loop {
                match read_str(r)?.as_str() {
                    ")" => break,
                    "entry" => {
                        expect(r, "(")?;
                        expect(r, "name")?;
                        let name = read_str(r)?;
                        if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
                            bail!("NAR: unsafe entry name {name:?}");
                        }
                        expect(r, "node")?;
                        let name = if hack {
                            ensure!(!name.contains(CASE_HACK_SUFFIX), "NAR: entry name {name:?} has {CASE_HACK_SUFFIX:?} in it");
                            match seen.get_mut(&name.to_ascii_lowercase()) {
                                Some(n) => {
                                    *n += 1;
                                    format!("{name}{CASE_HACK_SUFFIX}{n}")
                                }
                                None => {
                                    seen.insert(name.to_ascii_lowercase(), 0);
                                    name
                                }
                            }
                        } else {
                            name
                        };
                        node(r, &path.join(name), hack)?;
                        expect(r, ")")?;
                    }
                    other => bail!("NAR: unexpected {other:?} in directory"),
                }
            }
            let dir = File::open(path).with_context(|| format!("opening {}", path.display()))?;
            set_canonical_mtime(&dir).with_context(|| format!("setting {}'s time", path.display()))?;
            fs::set_permissions(path, Permissions::from_mode(0o555)).with_context(|| format!("making {} read-only", path.display()))?;
            Ok(())
        }
        other => bail!("NAR: unknown node type {other:?}"),
    }
}

/// Nix sets every mtime in the store to 1 so outputs are reproducible.
fn set_canonical_mtime(f: &File) -> Result<()> {
    let t = UNIX_EPOCH + Duration::from_secs(1);
    f.set_times(FileTimes::new().set_modified(t).set_accessed(t))?;
    Ok(())
}

fn read_u64(r: &mut impl Read) -> Result<u64> {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf).context("NAR: unexpected end of archive")?;
    Ok(u64::from_le_bytes(buf))
}

fn skip_padding(r: &mut impl Read, len: u64) -> Result<()> {
    let pad = ((8 - len % 8) % 8) as usize;
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf[..pad])?;
    if buf[..pad].iter().any(|&b| b != 0) {
        bail!("NAR: non-zero padding");
    }
    Ok(())
}

fn read_str(r: &mut impl Read) -> Result<String> {
    let len = read_u64(r)?;
    if len > MAX_STR {
        bail!("NAR: string too long ({len} bytes)");
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    skip_padding(r, len)?;
    Ok(String::from_utf8(buf)?)
}

fn expect(r: &mut impl Read, want: &str) -> Result<()> {
    let got = read_str(r)?;
    if got != want {
        bail!("NAR: expected {want:?}, got {got:?}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(out: &mut Vec<u8>, v: &[u8]) {
        out.extend((v.len() as u64).to_le_bytes());
        out.extend(v);
        out.resize(out.len() + (8 - v.len() % 8) % 8, 0);
    }

    #[test]
    fn unpacks_tree() {
        let mut nar = Vec::new();
        for t in ["nix-archive-1", "(", "type", "directory", "entry", "(", "name", "bin", "node", "(", "type", "directory",
                  "entry", "(", "name", "hi", "node", "(", "type", "regular", "executable", "", "contents"] {
            s(&mut nar, t.as_bytes());
        }
        s(&mut nar, b"#!/bin/sh\necho hi\n");
        for t in [")", ")", ")", ")", "entry", "(", "name", "link", "node", "(", "type", "symlink", "target", "bin/hi", ")", ")", ")"] {
            s(&mut nar, t.as_bytes());
        }
        let dir = std::env::temp_dir().join(format!("nami-nar-test-{}", std::process::id()));
        let out = dir.join("out");
        fs::create_dir_all(&dir).unwrap();
        unpack(&mut nar.as_slice(), &out).unwrap();
        assert_eq!(fs::read_to_string(out.join("bin/hi")).unwrap(), "#!/bin/sh\necho hi\n");
        assert_eq!(fs::metadata(out.join("bin/hi")).unwrap().permissions().mode() & 0o777, 0o555);
        assert_eq!(fs::read_link(out.join("link")).unwrap().to_str(), Some("bin/hi"));
        // Round trip: dumping what we unpacked gives back the same bytes.
        let mut again = Vec::new();
        dump(&out, &mut again).unwrap();
        assert_eq!(again, nar);
        crate::store::make_writable(&dir).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Names only their case tells apart, as Nix writes them on macOS (where it's on), and
    /// back to the same archive.
    #[test]
    fn case_hack() {
        let mut nar = Vec::new();
        s(&mut nar, b"nix-archive-1");
        for t in ["(", "type", "directory"] {
            s(&mut nar, t.as_bytes());
        }
        for (name, body) in [("Makefile", "a"), ("aa", "c"), ("makefile", "b"), ("makeFile", "d")] {
            let _ = body;
            for t in ["entry", "(", "name", name, "node", "(", "type", "regular", "contents", body, ")", ")"] {
                s(&mut nar, t.as_bytes());
            }
        }
        s(&mut nar, b")");
        // (In the order Nix sorts them: bytes. The archive above isn't, but unpacking doesn't mind.)
        let dir = std::env::temp_dir().join(format!("nami-nar-hack-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out");
        unpack_with(&mut nar.as_slice(), &out, true).unwrap();
        assert_eq!(fs::read_to_string(out.join("Makefile")).unwrap(), "a");
        assert_eq!(fs::read_to_string(out.join("makefile~nix~case~hack~1")).unwrap(), "b");
        assert_eq!(fs::read_to_string(out.join("makeFile~nix~case~hack~2")).unwrap(), "d");
        let mut again = Vec::new();
        dump_with(&out, &mut again, true).unwrap();
        let mut sorted = Vec::new();
        s(&mut sorted, b"nix-archive-1");
        for t in ["(", "type", "directory"] {
            s(&mut sorted, t.as_bytes());
        }
        for (name, body) in [("Makefile", "a"), ("aa", "c"), ("makeFile", "d"), ("makefile", "b")] {
            for t in ["entry", "(", "name", name, "node", "(", "type", "regular", "contents", body, ")", ")"] {
                s(&mut sorted, t.as_bytes());
            }
        }
        s(&mut sorted, b")");
        assert_eq!(again, sorted);
        // A name with the suffix in an archive is refused, rather than taken for the hack's.
        let mut bad = Vec::new();
        for t in ["nix-archive-1", "(", "type", "directory", "entry", "(", "name", "x~nix~case~hack~1", "node", "(", "type", "regular", "contents", "", ")", ")", ")"] {
            s(&mut bad, t.as_bytes());
        }
        assert!(unpack_with(&mut bad.as_slice(), &dir.join("bad"), true).is_err());
        crate::store::make_writable(&dir).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }
}
