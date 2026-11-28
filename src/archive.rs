//! Release archives, read by nami itself: tar (plain, gzip, xz, zstd; bzip2
//! through the computer's `bzip2`) and zip. A recipe's source is looked
//! into (what kind of project it is, its lock files) without unpacking it,
//! and the crates of a Rust project are unpacked into its vendor folder.
//!
//! What's read never leaves the folder it's unpacked into: paths with `..`
//! or a leading `/` are refused, as are links that point out of it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Cursor, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, ensure, Context, Result};

use crate::deb::{each_entry, Entry, Kind};

/// The most of one file `peek` keeps: lock files and manifests, not data.
const KEEP_MAX: u64 = 16 << 20;

/// What an archive (or a folder) has: every path in it, and the contents of
/// the files asked for. Paths are relative to its one top folder when it has
/// one (`ripgrep-14.1.1/Cargo.toml` is `Cargo.toml`), as stdenv unpacks it.
#[derive(Debug, Default, Clone)]
pub struct Tree {
    pub paths: BTreeSet<String>,
    pub files: BTreeMap<String, Vec<u8>>,
}

impl Tree {
    pub fn has(&self, path: &str) -> bool {
        self.paths.contains(path)
    }

    /// A file's contents as text, if it was kept (and is text).
    pub fn text(&self, path: &str) -> Option<&str> {
        self.files.get(path).and_then(|d| std::str::from_utf8(d).ok())
    }

    /// The kept files whose path `f` takes, in order.
    pub fn texts<'a>(&'a self, f: impl Fn(&str) -> bool + 'a) -> impl Iterator<Item = (&'a str, &'a str)> + 'a {
        self.files.iter().filter(move |(p, _)| f(p)).filter_map(|(p, d)| Some((p.as_str(), std::str::from_utf8(d).ok()?)))
    }
}

/// Look into `path` (an archive, or a folder): every path, and the
/// contents of those `want` takes (by the path as `Tree` has it).
pub fn peek(path: &Path, want: &dyn Fn(&str) -> bool) -> Result<Tree> {
    if path.is_dir() {
        return peek_dir(path, want);
    }
    let mut raw: Vec<(String, Option<Vec<u8>>)> = Vec::new();
    // Kept by the path as it is in the archive, or less its first folder:
    // which one counts is known only at the end.
    let keep = |p: &str| want(p) || p.split_once('/').is_some_and(|(_, rest)| want(rest));
    each_file(path, &mut |entry, data| {
        let kept = match (&entry.kind, keep(&entry.path)) {
            (Kind::File { .. }, true) => {
                let mut buf = Vec::new();
                data.take(KEEP_MAX).read_to_end(&mut buf)?;
                Some(buf)
            }
            _ => None,
        };
        let path = if matches!(entry.kind, Kind::Dir) { format!("{}/", entry.path) } else { entry.path.clone() };
        raw.push((path, kept));
        Ok(())
    })?;
    let top = one_top(raw.iter().map(|(p, _)| p.as_str()));
    let mut tree = Tree::default();
    for (p, data) in raw {
        let rel = match &top {
            Some(t) => match p.strip_prefix(t.as_str()).and_then(|r| r.strip_prefix('/')) {
                Some(r) => r.to_string(),
                None => continue, // the top folder itself
            },
            None => p,
        };
        let rel = rel.trim_end_matches('/').to_string();
        if rel.is_empty() {
            continue;
        }
        if let Some(d) = data.filter(|_| want(&rel)) {
            tree.files.insert(rel.clone(), d);
        }
        // Its folders too, which an archive needn't list.
        let mut at = rel.as_str();
        while let Some((parent, _)) = at.rsplit_once('/') {
            if !tree.paths.insert(parent.to_string()) {
                break;
            }
            at = parent;
        }
        tree.paths.insert(rel);
    }
    Ok(tree)
}

/// The one folder every path is in, if there's one (and nothing beside it).
fn one_top<'a>(paths: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut top: Option<&str> = None;
    let mut deeper = false;
    for p in paths {
        let (first, rest) = match p.split_once('/') {
            Some((f, r)) => (f, r),
            None => return None, // a file at the top
        };
        if top.is_some_and(|t| t != first) {
            return None;
        }
        top = Some(first);
        deeper |= !rest.is_empty();
    }
    top.filter(|_| deeper).map(String::from)
}

/// A folder's files, as `peek` gives an archive's. Version control and
/// build folders aren't looked in.
fn peek_dir(dir: &Path, want: &dyn Fn(&str) -> bool) -> Result<Tree> {
    let mut tree = Tree::default();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let entries = match fs::read_dir(dir.join(&rel)) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let r = rel.join(&name);
            let shown = r.to_string_lossy().into_owned();
            let Ok(ty) = e.file_type() else { continue };
            if ty.is_dir() {
                if [".git", "target", "node_modules", ".nami", "_build", "build", "dist", ".venv"].contains(&name.as_str()) && rel.as_os_str().is_empty() {
                    continue;
                }
                if tree.paths.len() < 100_000 {
                    stack.push(r);
                }
                tree.paths.insert(shown);
                continue;
            }
            if ty.is_file() && want(&shown) {
                if let Ok(d) = fs::read(e.path()) {
                    tree.files.insert(shown.clone(), d);
                }
            }
            tree.paths.insert(shown);
        }
    }
    Ok(tree)
}

/// Unpack archive `data` into the folder `dest` (made here), less its one
/// top folder when `strip` (a crate's `name-1.0/`, a tag's `repo-1.0/`).
pub fn unpack_bytes(data: &[u8], dest: &Path, strip: bool) -> Result<()> {
    fs::create_dir_all(dest)?;
    // First what's in it, to know its top folder; then out it comes.
    let mut paths = Vec::new();
    each_file_in(data, &mut |e, _| {
        paths.push(if matches!(e.kind, Kind::Dir) { format!("{}/", e.path) } else { e.path.clone() });
        Ok(())
    })?;
    let top = if strip { one_top(paths.iter().map(String::as_str)) } else { None };
    let mut links: Vec<(PathBuf, String)> = Vec::new();
    each_file_in(data, &mut |e, r| {
        let rel = match &top {
            Some(t) => match e.path.strip_prefix(t.as_str()).and_then(|r| r.strip_prefix('/')) {
                Some(r) if !r.is_empty() => r.to_string(),
                _ => return Ok(()),
            },
            None => e.path.clone(),
        };
        let to = inside(dest, &rel)?;
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        match &e.kind {
            Kind::Dir => fs::create_dir_all(&to)?,
            Kind::File { executable } => {
                let mut f = fs::File::create(&to).with_context(|| format!("writing {}", to.display()))?;
                std::io::copy(r, &mut f)?;
                fs::set_permissions(&to, fs::Permissions::from_mode(if *executable { 0o755 } else { 0o644 }))?;
            }
            Kind::Symlink(target) => {
                // Only links that stay inside.
                let resolved = to.parent().unwrap_or(dest).join(target);
                ensure!(!target.starts_with('/') && normal(&resolved).starts_with(dest), "{rel}: a link out of the archive ({target})");
                let _ = fs::remove_file(&to);
                std::os::unix::fs::symlink(target, &to)?;
            }
            Kind::Hardlink(other) => links.push((to, other.clone())),
        }
        Ok(())
    })?;
    for (to, other) in links {
        let other = match &top {
            Some(t) => other.strip_prefix(t.as_str()).and_then(|r| r.strip_prefix('/')).unwrap_or(&other).to_string(),
            None => other,
        };
        fs::copy(inside(dest, &other)?, &to)?;
    }
    Ok(())
}

/// `rel` under `dest`, refused if it would be anywhere else.
fn inside(dest: &Path, rel: &str) -> Result<PathBuf> {
    let p = Path::new(rel);
    ensure!(p.components().all(|c| matches!(c, Component::Normal(_))), "{rel:?}: a path out of the archive");
    Ok(dest.join(p))
}

/// `p` with `..` and `.` worked out, without asking the file system.
fn normal(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

/// Each entry of the archive at `path`, as deb.rs reads a tar's.
fn each_file(path: &Path, f: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>) -> Result<()> {
    let data = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    each_file_in(&data, f).with_context(|| format!("reading {}", path.display()))
}

fn each_file_in(data: &[u8], f: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>) -> Result<()> {
    match data {
        [0x1f, 0x8b, ..] => each_entry(flate2::read::MultiGzDecoder::new(data), f),
        [0xfd, b'7', b'z', b'X', b'Z', 0, ..] => each_entry(xz2::read::XzDecoder::new_multi_decoder(data), f),
        [0x28, 0xb5, 0x2f, 0xfd, ..] => each_entry(zstd::stream::read::Decoder::new(data)?, f),
        [b'B', b'Z', b'h', ..] => each_entry(Cursor::new(bunzip2(data)?), f),
        [b'P', b'K', 3, 4, ..] | [b'P', b'K', 5, 6, ..] => each_zip_entry(data, f),
        _ if data.get(257..262) == Some(b"ustar") => each_entry(data, f),
        _ => bail!("not an archive nami knows (tar, .tar.gz, .tar.xz, .tar.zst, .tar.bz2 or .zip)"),
    }
}

/// bzip2, through the computer's own `bzip2` (rare enough not to carry).
fn bunzip2(data: &[u8]) -> Result<Vec<u8>> {
    let mut child = Command::new("bzip2")
        .arg("-dc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| anyhow!("a .tar.bz2 needs bzip2 on this computer to look into"))?;
    let mut stdin = child.stdin.take().unwrap();
    let input = data.to_vec();
    let writer = std::thread::spawn(move || std::io::Write::write_all(&mut stdin, &input));
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out)?;
    let _ = writer.join();
    ensure!(child.wait()?.success(), "bzip2 couldn't unpack it");
    Ok(out)
}

/// Each entry of a zip file: stored or deflated, from its central directory.
fn each_zip_entry(data: &[u8], f: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>) -> Result<()> {
    let u16_at = |at: usize| -> Result<usize> { Ok(u16::from_le_bytes(data.get(at..at + 2).ok_or_else(|| anyhow!("a zip cut short"))?.try_into().unwrap()) as usize) };
    let u32_at = |at: usize| -> Result<usize> { Ok(u32::from_le_bytes(data.get(at..at + 4).ok_or_else(|| anyhow!("a zip cut short"))?.try_into().unwrap()) as usize) };
    // The end of central directory record: in the last 64 KiB and 22 bytes.
    let start = data.len().saturating_sub(65_557);
    let eocd = (start..data.len().saturating_sub(21)).rev().find(|&i| data[i..].starts_with(b"PK\x05\x06")).ok_or_else(|| anyhow!("not a zip file (no central directory)"))?;
    let count = u16_at(eocd + 10)?;
    let mut at = u32_at(eocd + 16)?;
    ensure!(count != 0xffff && at != 0xffff_ffff, "a zip64 file: nami reads only zip files under 4 GB");
    for _ in 0..count {
        ensure!(data.get(at..at + 4) == Some(b"PK\x01\x02"), "a broken zip central directory");
        let method = u16_at(at + 10)?;
        let size = u32_at(at + 20)?;
        let (name_len, extra_len, comment_len) = (u16_at(at + 28)?, u16_at(at + 30)?, u16_at(at + 32)?);
        let attrs = u32_at(at + 38)?;
        let local = u32_at(at + 42)?;
        let name = String::from_utf8_lossy(data.get(at + 46..at + 46 + name_len).ok_or_else(|| anyhow!("a zip cut short"))?).into_owned();
        at += 46 + name_len + extra_len + comment_len;
        ensure!(data.get(local..local + 4) == Some(b"PK\x03\x04"), "{name}: a broken zip entry");
        let body = local + 30 + u16_at(local + 26)? + u16_at(local + 28)?;
        let raw = data.get(body..body + size).ok_or_else(|| anyhow!("{name}: a zip cut short"))?;
        let mut contents: Box<dyn Read> = match method {
            0 => Box::new(raw),
            8 => Box::new(flate2::read::DeflateDecoder::new(raw)),
            m => bail!("{name}: zip compression method {m} isn't one nami reads"),
        };
        let mode = (attrs >> 16) as u32;
        let path = crate::deb::clean(&name);
        if path.is_empty() {
            continue;
        }
        let kind = if name.ends_with('/') || mode & 0o170000 == 0o040000 {
            Kind::Dir
        } else if mode & 0o170000 == 0o120000 {
            let mut target = String::new();
            contents.read_to_string(&mut target)?;
            Kind::Symlink(target)
        } else {
            Kind::File { executable: mode & 0o111 != 0 }
        };
        f(&Entry { path, kind, mode: mode & 0o7777 }, &mut contents)?;
    }
    Ok(())
}

/// The names in a zip file and each one's contents: what Go's module
/// zips are checked by (vendor.rs).
pub fn zip_files(data: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    each_zip_entry(data, &mut |e, r| {
        if let Kind::File { .. } = e.kind {
            let mut d = Vec::new();
            r.read_to_end(&mut d)?;
            out.push((e.path.clone(), d));
        }
        Ok(())
    })?;
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::deb::tests::tar;

    fn gz(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        flate2::read::GzEncoder::new(data, flate2::Compression::fast()).read_to_end(&mut out).unwrap();
        out
    }

    /// A zip of (name, contents), deflated.
    pub fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let (mut body, mut dir) = (Vec::new(), Vec::new());
        for (name, data) in files {
            let mut deflated = Vec::new();
            flate2::read::DeflateEncoder::new(*data, flate2::Compression::fast()).read_to_end(&mut deflated).unwrap();
            let offset = body.len() as u32;
            let le16 = |v: usize| (v as u16).to_le_bytes();
            let le32 = |v: usize| (v as u32).to_le_bytes();
            body.extend(b"PK\x03\x04");
            body.extend([20, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            body.extend(le32(deflated.len()));
            body.extend(le32(data.len()));
            body.extend(le16(name.len()));
            body.extend(le16(0));
            body.extend(name.as_bytes());
            body.extend(&deflated);
            dir.extend(b"PK\x01\x02");
            dir.extend([20, 3, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            dir.extend(le32(deflated.len()));
            dir.extend(le32(data.len()));
            dir.extend(le16(name.len()));
            dir.extend([0, 0, 0, 0, 0, 0, 0, 0]);
            dir.extend(le32(0o100644 << 16));
            dir.extend(le32(offset as usize));
            dir.extend(name.as_bytes());
        }
        let (at, len, n) = (body.len() as u32, dir.len() as u32, files.len() as u16);
        body.extend(dir);
        body.extend(b"PK\x05\x06\0\0\0\0");
        body.extend(n.to_le_bytes());
        body.extend(n.to_le_bytes());
        body.extend(len.to_le_bytes());
        body.extend(at.to_le_bytes());
        body.extend([0, 0]);
        body
    }

    #[test]
    fn peeks_into_a_tarball_less_its_top_folder() {
        let t = tar(&[
            ("ripgrep-14.1.1/", b'5', b"", 0o755),
            ("ripgrep-14.1.1/Cargo.toml", b'0', b"[package]\nname = \"ripgrep\"\n", 0o644),
            ("ripgrep-14.1.1/Cargo.lock", b'0', b"version = 3\n", 0o644),
            ("ripgrep-14.1.1/crates/core/main.rs", b'0', b"fn main() {}", 0o644),
        ]);
        let dir = std::env::temp_dir().join(format!("nami-archive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("rg.tar.gz"), gz(&t)).unwrap();
        let tree = peek(&dir.join("rg.tar.gz"), &|p| p == "Cargo.toml" || p == "Cargo.lock").unwrap();
        assert!(tree.has("Cargo.toml") && tree.has("crates/core/main.rs") && tree.has("crates"));
        assert_eq!(tree.text("Cargo.lock"), Some("version = 3\n"));
        assert!(!tree.files.contains_key("crates/core/main.rs"));
        // Unpacked, less its folder; a path out of it is refused.
        unpack_bytes(&fs::read(dir.join("rg.tar.gz")).unwrap(), &dir.join("out"), true).unwrap();
        assert_eq!(fs::read_to_string(dir.join("out/crates/core/main.rs")).unwrap(), "fn main() {}");
        let evil = gz(&tar(&[("a/../../x", b'0', b"no", 0o644)]));
        assert!(unpack_bytes(&evil, &dir.join("evil"), false).is_err());
        let link = gz(&tar(&[("a/l", b'2', b"../../../etc/passwd", 0o777)]));
        assert!(unpack_bytes(&link, &dir.join("link"), false).is_err());
        // A zip, the same way.
        fs::write(dir.join("p.zip"), zip(&[("pkg-1.0/pyproject.toml", b"[project]\nname='x'\n"), ("pkg-1.0/x/__init__.py", b"")])).unwrap();
        let tree = peek(&dir.join("p.zip"), &|p| p == "pyproject.toml").unwrap();
        assert_eq!(tree.text("pyproject.toml"), Some("[project]\nname='x'\n"));
        assert!(tree.has("x/__init__.py"));
        // Several things at the top: nothing stripped.
        let flat = gz(&tar(&[("a.c", b'0', b"", 0o644), ("Makefile", b'0', b"all:", 0o644)]));
        fs::write(dir.join("flat.tgz"), flat).unwrap();
        assert!(peek(&dir.join("flat.tgz"), &|_| true).unwrap().has("Makefile"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
