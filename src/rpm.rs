//! RPM packages (.rpm, Fedora's and openSUSE's): what's in them, read
//! without rpm. A lead (96 bytes), a signature header, the package's own
//! header (its name, version, what it requires, its scripts), then its
//! files: a cpio archive ("newc"), compressed.

use std::collections::BTreeMap;
use std::io::Read;

use anyhow::{bail, ensure, Context, Result};

use crate::deb::{Entry, Kind};

/// A header's values, by tag: strings (a string array's each), numbers.
#[derive(Debug, Default)]
pub struct Header {
    pub strings: BTreeMap<u32, Vec<String>>,
    pub numbers: BTreeMap<u32, Vec<u64>>,
}

pub const NAME: u32 = 1000;
pub const VERSION: u32 = 1001;
pub const RELEASE: u32 = 1002;
pub const SUMMARY: u32 = 1004;
pub const DESCRIPTION: u32 = 1005;
pub const SIZE: u32 = 1009;
pub const VENDOR: u32 = 1011;
pub const PACKAGER: u32 = 1015;
pub const URL: u32 = 1020;
pub const ARCH: u32 = 1022;
pub const PREIN: u32 = 1023;
pub const POSTIN: u32 = 1024;
pub const PREUN: u32 = 1025;
pub const POSTUN: u32 = 1026;
#[cfg(test)]
pub const FILEMODES: u32 = 1030;
pub const REQUIRENAME: u32 = 1049;
#[cfg(test)]
pub const DIRINDEXES: u32 = 1116;
#[cfg(test)]
pub const BASENAMES: u32 = 1117;
#[cfg(test)]
pub const DIRNAMES: u32 = 1118;
pub const PAYLOADCOMPRESSOR: u32 = 1125;
pub const LONGSIZE: u32 = 5009;

impl Header {
    pub fn string(&self, tag: u32) -> Option<&str> {
        self.strings.get(&tag)?.first().map(String::as_str)
    }

    pub fn number(&self, tag: u32) -> Option<u64> {
        self.numbers.get(&tag)?.first().copied()
    }

    pub fn list(&self, tag: u32) -> &[String] {
        self.strings.get(&tag).map(Vec::as_slice).unwrap_or_default()
    }

    #[cfg(test)]
    /// The package's files, as paths without the leading `/`, with their modes.
    pub fn files(&self) -> Vec<(String, u32)> {
        let (dirs, bases) = (self.list(DIRNAMES), self.list(BASENAMES));
        let index = self.numbers.get(&DIRINDEXES).cloned().unwrap_or_default();
        let modes = self.numbers.get(&FILEMODES).cloned().unwrap_or_default();
        bases
            .iter()
            .enumerate()
            .filter_map(|(i, b)| {
                let d = dirs.get(*index.get(i)? as usize)?;
                Some((format!("{d}{b}").trim_start_matches('/').to_string(), modes.get(i).copied().unwrap_or(0o644) as u32))
            })
            .collect()
    }
}

fn u32_at(d: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(d.get(at..at + 4).context("an RPM cut short")?.try_into().unwrap()))
}

/// A header at `at`: its values, and where what follows it starts.
fn header(d: &[u8], at: usize) -> Result<(Header, usize)> {
    ensure!(d.get(at..at + 3) == Some(&[0x8e, 0xad, 0xe8][..]), "not an RPM package (a broken header)");
    let (count, size) = (u32_at(d, at + 8)? as usize, u32_at(d, at + 12)? as usize);
    ensure!(count < 100_000 && size < 256 << 20, "not an RPM package (a header too big)");
    let store = at + 16 + count * 16;
    let data = d.get(store..store + size).context("an RPM cut short")?;
    let mut h = Header::default();
    for i in 0..count {
        let e = at + 16 + i * 16;
        let (tag, kind, off, n) = (u32_at(d, e)?, u32_at(d, e + 4)?, u32_at(d, e + 8)? as usize, u32_at(d, e + 12)? as usize);
        let Some(rest) = data.get(off..) else { continue };
        match kind {
            // A string, a string array, an i18n string (each language's: the first is C's).
            6 | 8 | 9 => {
                let n = if kind == 6 { 1 } else { n };
                let all: Vec<String> = rest.split(|b| *b == 0).take(n).map(|s| String::from_utf8_lossy(s).into_owned()).collect();
                h.strings.insert(tag, all);
            }
            // Numbers: 8, 16, 32 and 64 bits.
            2 | 3 | 4 | 5 => {
                let w = [1usize, 2, 4, 8][kind as usize - 2];
                let all: Vec<u64> = (0..n.min(rest.len() / w)).map(|j| rest[j * w..j * w + w].iter().fold(0u64, |v, b| (v << 8) | *b as u64)).collect();
                h.numbers.insert(tag, all);
            }
            _ => {}
        }
    }
    Ok((h, store + size))
}

/// The package's header, and where its files (the compressed payload) start.
pub fn read_header(d: &[u8]) -> Result<(Header, usize)> {
    ensure!(d.starts_with(&[0xed, 0xab, 0xee, 0xdb]), "not an RPM package");
    let (_, end) = header(d, 96)?;
    // The signature's header is padded to 8 bytes.
    let main = end.div_ceil(8) * 8;
    header(d, main)
}

/// The payload, uncompressed as it's read.
fn payload<'a>(d: &'a [u8], h: &Header, at: usize) -> Result<Box<dyn Read + 'a>> {
    let body = &d[at..];
    // By its own magic: the header's word for it is sometimes missing.
    Ok(if body.starts_with(&[0x1f, 0x8b]) {
        Box::new(flate2::read::MultiGzDecoder::new(body))
    } else if body.starts_with(&[0xfd, b'7', b'z', b'X', b'Z']) {
        Box::new(xz2::read::XzDecoder::new_multi_decoder(body))
    } else if body.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Box::new(zstd::stream::read::Decoder::new(body)?)
    } else if body.starts_with(b"070701") {
        Box::new(body)
    } else {
        bail!("its files are compressed as {} , which nami can't read", h.string(PAYLOADCOMPRESSOR).unwrap_or("something unknown"))
    })
}

/// Each file of the package, with a reader for its contents.
pub fn each_entry(d: &[u8], mut f: impl FnMut(&Entry, &mut dyn Read) -> Result<()>) -> Result<()> {
    let (h, at) = read_header(d)?;
    let mut r = payload(d, &h, at)?;
    let mut read = 0usize;
    // Hard links: their data comes with the last of them.
    let mut waiting: BTreeMap<u64, Vec<(String, u32)>> = BTreeMap::new();
    loop {
        let mut head = [0u8; 110];
        r.read_exact(&mut head).context("the package's files cut short")?;
        read += 110;
        ensure!(&head[..6] == b"070701" || &head[..6] == b"070702", "the package's files aren't a cpio archive nami can read");
        let field = |i: usize| u64::from_str_radix(std::str::from_utf8(&head[6 + i * 8..14 + i * 8]).unwrap_or("0"), 16).unwrap_or(0);
        let (ino, mode, nlink, size, namesize) = (field(0), field(1) as u32, field(4), field(6) as usize, field(11) as usize);
        let mut name = vec![0u8; namesize];
        r.read_exact(&mut name)?;
        read += namesize;
        let pad = (4 - read % 4) % 4;
        std::io::copy(&mut (&mut r).take(pad as u64), &mut std::io::sink())?;
        read += pad;
        let name = String::from_utf8_lossy(&name).trim_end_matches('\0').to_string();
        if name == "TRAILER!!!" {
            return Ok(());
        }
        let path = name.trim_start_matches("./").trim_start_matches('/').trim_end_matches('/').to_string();
        let mut data = (&mut r).take(size as u64);
        let kind = match mode & 0o170000 {
            0o040000 => Some(Kind::Dir),
            0o120000 => {
                let mut t = String::new();
                data.read_to_string(&mut t)?;
                Some(Kind::Symlink(t))
            }
            0o100000 if size == 0 && nlink > 1 => {
                waiting.entry(ino).or_default().push((path.clone(), mode));
                None
            }
            0o100000 => Some(Kind::File { executable: mode & 0o111 != 0 }),
            _ => None,
        };
        if let (Some(kind), false) = (kind, path.is_empty()) {
            let file = matches!(kind, Kind::File { .. });
            f(&Entry { path: path.clone(), kind, mode: mode & 0o7777 }, &mut data)?;
            if file {
                for (other, m) in waiting.remove(&ino).unwrap_or_default() {
                    f(&Entry { path: other, kind: Kind::Hardlink(path.clone()), mode: m & 0o7777 }, &mut std::io::empty())?;
                }
            }
        }
        std::io::copy(&mut data, &mut std::io::sink())?;
        read += size;
        let pad = (4 - read % 4) % 4;
        std::io::copy(&mut (&mut r).take(pad as u64), &mut std::io::sink())?;
        read += pad;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A header of these strings and numbers.
    fn header_bytes(strings: &[(u32, &[&str])], numbers: &[(u32, u32, &[u64])]) -> Vec<u8> {
        let (mut index, mut data) = (Vec::new(), Vec::new());
        for (tag, values) in strings {
            index.push((*tag, if values.len() == 1 { 6 } else { 8 }, data.len() as u32, values.len() as u32));
            for v in *values {
                data.extend_from_slice(v.as_bytes());
                data.push(0);
            }
        }
        for (tag, kind, values) in numbers {
            let w = [1usize, 2, 4, 8][*kind as usize - 2];
            while data.len() % w != 0 {
                data.push(0);
            }
            index.push((*tag, *kind, data.len() as u32, values.len() as u32));
            for v in *values {
                data.extend_from_slice(&v.to_be_bytes()[8 - w..]);
            }
        }
        let mut out = vec![0x8e, 0xad, 0xe8, 0x01, 0, 0, 0, 0];
        out.extend_from_slice(&(index.len() as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        for (t, k, o, n) in index {
            for v in [t, k, o, n] {
                out.extend_from_slice(&v.to_be_bytes());
            }
        }
        out.extend_from_slice(&data);
        out
    }

    fn cpio(files: &[(&str, u32, &[u8], u64, u64)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut entry = |name: &str, mode: u32, data: &[u8], ino: u64, nlink: u64| {
            let n = format!("{name}\0");
            out.extend_from_slice(format!("070701{ino:08x}{mode:08x}{:08x}{:08x}{nlink:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}", 0, 0, 0, data.len(), 0, 0, 0, 0, n.len(), 0).as_bytes());
            out.extend_from_slice(n.as_bytes());
            while out.len() % 4 != 0 {
                out.push(0);
            }
            out.extend_from_slice(data);
            while out.len() % 4 != 0 {
                out.push(0);
            }
        };
        for (name, mode, data, ino, nlink) in files {
            entry(name, *mode, data, *ino, *nlink);
        }
        entry("TRAILER!!!", 0, b"", 0, 1);
        out
    }

    /// An .rpm of these files, its header saying these strings.
    pub fn rpm(files: &[(&str, u32, &[u8], u64, u64)], strings: &[(u32, &[&str])]) -> Vec<u8> {
        let mut d = vec![0xed, 0xab, 0xee, 0xdb];
        d.resize(96, 0);
        d.extend(header_bytes(&[], &[]));
        while d.len() % 8 != 0 {
            d.push(0);
        }
        let names: Vec<String> = files.iter().map(|f| f.0.trim_start_matches('.').to_string()).collect();
        let (dirs, bases): (Vec<String>, Vec<String>) = names.iter().map(|n| { let (d, b) = n.rsplit_once('/').unwrap(); (format!("{d}/"), b.to_string()) }).unzip();
        let dir_refs: Vec<&str> = dirs.iter().map(String::as_str).collect();
        let base_refs: Vec<&str> = bases.iter().map(String::as_str).collect();
        let idx: Vec<u64> = (0..files.len() as u64).collect();
        let modes: Vec<u64> = files.iter().map(|f| f.1 as u64).collect();
        let mut all: Vec<(u32, &[&str])> = strings.to_vec();
        all.push((DIRNAMES, &dir_refs));
        all.push((BASENAMES, &base_refs));
        d.extend(header_bytes(&all, &[(DIRINDEXES, 4, &idx), (FILEMODES, 3, &modes), (SIZE, 4, &[1234])]));
        let mut z = Vec::new();
        xz2::read::XzEncoder::new(&cpio(files)[..], 1).read_to_end(&mut z).unwrap();
        d.extend(z);
        d
    }

    #[test]
    fn reads_a_package() {
        let d = rpm(
            &[
                ("./usr/bin/foo", 0o100755, b"\x7fELF", 1, 1),
                ("./usr/share/foo/a", 0o100644, b"", 2, 2),
                ("./usr/share/foo/b", 0o100644, b"same", 2, 2),
                ("./usr/bin/foo-link", 0o120777, b"foo", 3, 1),
                ("./usr/lib/foo/chrome-sandbox", 0o104755, b"x", 4, 1),
            ],
            &[(NAME, &["foo"]), (VERSION, &["1.2"]), (RELEASE, &["3.fc41"]), (VENDOR, &["Foo Inc."]), (REQUIRENAME, &["libgtk-3.so.0()(64bit)", "nss", "rpmlib(PayloadIsXz)"])],
        );
        let (h, _) = read_header(&d).unwrap();
        assert_eq!(h.string(NAME), Some("foo"));
        assert_eq!(h.string(VENDOR), Some("Foo Inc."));
        assert_eq!(h.number(SIZE), Some(1234));
        assert_eq!(h.list(REQUIRENAME).len(), 3);
        assert_eq!(h.files()[0], ("usr/bin/foo".to_string(), 0o100755));
        let mut seen = Vec::new();
        each_entry(&d, |e, r| {
            let mut v = Vec::new();
            r.read_to_end(&mut v)?;
            seen.push((e.path.clone(), e.kind.clone(), e.mode, v));
            Ok(())
        })
        .unwrap();
        assert_eq!(seen[0], ("usr/bin/foo".into(), Kind::File { executable: true }, 0o755, b"\x7fELF".to_vec()));
        assert_eq!(seen[1], ("usr/share/foo/b".into(), Kind::File { executable: false }, 0o644, b"same".to_vec()));
        assert_eq!(seen[2].1, Kind::Hardlink("usr/share/foo/b".into()));
        assert_eq!(seen[2].0, "usr/share/foo/a");
        assert_eq!(seen[3].1, Kind::Symlink("foo".into()));
        assert_eq!(seen[4].2, 0o4755);
        assert!(read_header(b"not an rpm at all, no").is_err());
    }
}
