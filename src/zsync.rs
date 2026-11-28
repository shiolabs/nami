//! zsync, as AppImageUpdate uses it: a new version of a file made from the
//! one here and only the blocks that changed, fetched by HTTP range
//! requests. The .zsync file (zsyncmake's) lists each block's weak rolling
//! checksum and the start of its MD4; the blocks the old file has are found
//! by rolling over it, and each is checked by its MD4 before it's used.
//! Each fetched block is checked the same, and the whole file by its SHA-1
//! before it's given back: anything that doesn't match is refused, nothing
//! half made is left (it's built in a `.part` file, renamed once it's whole).

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::{ensure, Context, Result};

/// A .zsync file: its header and each block's checksums.
#[derive(Debug, Clone)]
pub struct Control {
    pub sha1: String,
    pub length: u64,
    pub blocksize: usize,
    seq_matches: usize,
    rsum_bytes: usize,
    checksum_bytes: usize,
    /// Per block: its rolling checksum (as stored: masked) and its MD4's first bytes.
    blocks: Vec<(u32, Vec<u8>)>,
}

/// A fetched block that isn't what the .zsync says: changed on the way.
#[derive(Debug)]
pub struct BadBlock(pub u64);

impl std::fmt::Display for BadBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "the part fetched at byte {} isn't what its update information says", self.0)
    }
}
impl std::error::Error for BadBlock {}

/// The whole file made doesn't have the SHA-1 the .zsync says.
#[derive(Debug)]
pub struct BadSum;

impl std::fmt::Display for BadSum {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("the file made doesn't match its update information's checksum")
    }
}
impl std::error::Error for BadSum {}

/// How it went: bytes taken from the file here, and fetched.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Made {
    pub reused: u64,
    pub fetched: u64,
    pub requests: usize,
}

pub fn parse(data: &[u8]) -> Result<Control> {
    let end = data.windows(2).position(|w| w == b"\n\n").context("a .zsync file without its checksums")?;
    let head = String::from_utf8_lossy(&data[..end]);
    let mut h: HashMap<String, String> = HashMap::new();
    for l in head.lines() {
        if let Some((k, v)) = l.split_once(':') {
            h.entry(k.trim().to_string()).or_insert_with(|| v.trim().to_string());
        }
    }
    ensure!(!h.contains_key("Z-URL") && !h.contains_key("Recompress"), "a .zsync of a compressed file, which nami doesn't follow");
    let length: u64 = h.get("Length").and_then(|l| l.parse().ok()).context("a .zsync file without the file's length")?;
    let blocksize: usize = h.get("Blocksize").and_then(|l| l.parse().ok()).context("a .zsync file without its block size")?;
    ensure!((64..=1 << 20).contains(&blocksize) && blocksize.is_power_of_two(), "a .zsync file with a strange block size ({blocksize})");
    let (seq_matches, rsum_bytes, checksum_bytes) = match h.get("Hash-Lengths") {
        Some(v) => {
            let n: Vec<usize> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            ensure!(n.len() == 3, "a .zsync file with broken hash lengths ({v})");
            (n[0], n[1], n[2])
        }
        None => (1, 4, 16),
    };
    ensure!((1..=2).contains(&seq_matches) && (1..=4).contains(&rsum_bytes) && (3..=16).contains(&checksum_bytes), "a .zsync file with strange hash lengths");
    let count = length.div_ceil(blocksize as u64) as usize;
    let each = rsum_bytes + checksum_bytes;
    let sums = &data[end + 2..];
    ensure!(sums.len() >= count * each, "a .zsync file cut short ({} of {} blocks' checksums)", sums.len() / each, count);
    let blocks = (0..count)
        .map(|i| {
            let e = &sums[i * each..(i + 1) * each];
            let mut r = [0u8; 4];
            r[4 - rsum_bytes..].copy_from_slice(&e[..rsum_bytes]);
            (u32::from_be_bytes(r), e[rsum_bytes..].to_vec())
        })
        .collect();
    let sha1 = h.get("SHA-1").map(|s| s.to_lowercase()).context("a .zsync file without the file's SHA-1")?;
    Ok(Control {
        sha1,
        length,
        blocksize,
        seq_matches,
        rsum_bytes,
        checksum_bytes,
        blocks,
    })
}

// ------------------------------------------------------------------ checksums

/// zsync's rolling checksum of a block: (a, b), each 16 bits.
fn rsum(block: &[u8]) -> (u16, u16) {
    let (mut a, mut b) = (0u16, 0u16);
    let n = block.len();
    for (i, &c) in block.iter().enumerate() {
        a = a.wrapping_add(c as u16);
        b = b.wrapping_add(((n - i) as u16).wrapping_mul(c as u16));
    }
    (a, b)
}

fn packed(r: (u16, u16)) -> u32 {
    ((r.0 as u32) << 16) | r.1 as u32
}

/// MD4 (RFC 1320): zsync's strong checksum of a block.
pub fn md4(data: &[u8]) -> [u8; 16] {
    let mut h: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_le_bytes());
    for chunk in msg.chunks(64) {
        let x: Vec<u32> = chunk.chunks(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect();
        let [mut a, mut b, mut c, mut d] = h;
        let f = |x: u32, y: u32, z: u32| (x & y) | (!x & z);
        let g = |x: u32, y: u32, z: u32| (x & y) | (x & z) | (y & z);
        let hh = |x: u32, y: u32, z: u32| x ^ y ^ z;
        for &i in &[0usize, 4, 8, 12] {
            a = a.wrapping_add(f(b, c, d)).wrapping_add(x[i]).rotate_left(3);
            d = d.wrapping_add(f(a, b, c)).wrapping_add(x[i + 1]).rotate_left(7);
            c = c.wrapping_add(f(d, a, b)).wrapping_add(x[i + 2]).rotate_left(11);
            b = b.wrapping_add(f(c, d, a)).wrapping_add(x[i + 3]).rotate_left(19);
        }
        for &i in &[0usize, 1, 2, 3] {
            let k = 0x5a827999u32;
            a = a.wrapping_add(g(b, c, d)).wrapping_add(x[i]).wrapping_add(k).rotate_left(3);
            d = d.wrapping_add(g(a, b, c)).wrapping_add(x[i + 4]).wrapping_add(k).rotate_left(5);
            c = c.wrapping_add(g(d, a, b)).wrapping_add(x[i + 8]).wrapping_add(k).rotate_left(9);
            b = b.wrapping_add(g(c, d, a)).wrapping_add(x[i + 12]).wrapping_add(k).rotate_left(13);
        }
        for &i in &[0usize, 2, 1, 3] {
            let k = 0x6ed9eba1u32;
            a = a.wrapping_add(hh(b, c, d)).wrapping_add(x[i]).wrapping_add(k).rotate_left(3);
            d = d.wrapping_add(hh(a, b, c)).wrapping_add(x[i + 8]).wrapping_add(k).rotate_left(9);
            c = c.wrapping_add(hh(d, a, b)).wrapping_add(x[i + 4]).wrapping_add(k).rotate_left(11);
            b = b.wrapping_add(hh(c, d, a)).wrapping_add(x[i + 12]).wrapping_add(k).rotate_left(15);
        }
        for (v, n) in h.iter_mut().zip([a, b, c, d]) {
            *v = v.wrapping_add(n);
        }
    }
    let mut out = [0u8; 16];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

impl Control {
    fn mask(&self) -> u32 {
        if self.rsum_bytes == 4 { u32::MAX } else { (1u32 << (8 * self.rsum_bytes)) - 1 }
    }

    /// Whether `data` (one block, padded with zeros) is block `i`.
    fn is_block(&self, i: usize, data: &[u8]) -> bool {
        let mut padded;
        let block = if data.len() < self.blocksize {
            padded = data.to_vec();
            padded.resize(self.blocksize, 0);
            &padded[..]
        } else {
            data
        };
        md4(block)[..self.checksum_bytes] == self.blocks[i].1[..]
    }

    fn block_len(&self, i: usize) -> usize {
        (self.length - (i * self.blocksize) as u64).min(self.blocksize as u64) as usize
    }
}

// ------------------------------------------------------------------ the file

/// The blocks of `c` that `seed` (an older version) has: for each block,
/// where in the seed it is.
pub fn find(c: &Control, seed: &[u8]) -> Vec<Option<usize>> {
    let n = c.blocks.len();
    let bs = c.blocksize;
    let mut have: Vec<Option<usize>> = vec![None; n];
    if seed.is_empty() || n == 0 {
        return have;
    }
    let mask = c.mask();
    let mut table: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, (r, _)) in c.blocks.iter().enumerate() {
        table.entry(*r & mask).or_default().push(i);
    }
    // (Past its end, the seed is zeros: as zsync pads the last block.)
    let mut padded = seed.to_vec();
    padded.resize(seed.len() + 2 * bs, 0);
    let at = |o: usize| (o, o + bs);
    let sum_at = |o: usize| rsum(&padded[o..o + bs]);
    let (mut r0, mut r1) = (sum_at(0), sum_at(bs));
    let mut o = 0usize;
    let mut left = n;
    while o < seed.len() && left > 0 {
        let mut matched = false;
        if let Some(ids) = table.get(&(packed(r0) & mask)) {
            let mut strong: Option<[u8; 16]> = None;
            for &i in ids {
                if have[i].is_some() && (c.seq_matches == 1 || i + 1 >= n || have[i + 1].is_some()) {
                    continue;
                }
                // Two blocks in a row, as the .zsync was made for (their weak sums first).
                if c.seq_matches == 2 && i + 1 < n && c.blocks[i + 1].0 & mask != packed(r1) & mask {
                    continue;
                }
                let (s, e) = at(o);
                let md = *strong.get_or_insert_with(|| md4(&padded[s..e]));
                if md[..c.checksum_bytes] != c.blocks[i].1[..] {
                    continue;
                }
                if c.seq_matches == 2 && i + 1 < n && !c.is_block(i + 1, &padded[e..e + bs]) {
                    continue;
                }
                if have[i].is_none() {
                    have[i] = Some(o);
                    left -= 1;
                }
                if c.seq_matches == 2 && i + 1 < n && have[i + 1].is_none() {
                    have[i + 1] = Some(o + bs);
                    left -= 1;
                }
                matched = true;
            }
        }
        if matched {
            o += bs;
            if o >= seed.len() {
                break;
            }
            (r0, r1) = (sum_at(o), sum_at(o + bs));
            continue;
        }
        // Rolled on a byte.
        let (out0, in0, out1, in1) = (padded[o] as u16, padded[o + bs] as u16, padded[o + bs] as u16, padded[o + 2 * bs] as u16);
        let n16 = bs as u16;
        r0.0 = r0.0.wrapping_sub(out0).wrapping_add(in0);
        r0.1 = r0.1.wrapping_sub(n16.wrapping_mul(out0)).wrapping_add(r0.0);
        r1.0 = r1.0.wrapping_sub(out1).wrapping_add(in1);
        r1.1 = r1.1.wrapping_sub(n16.wrapping_mul(out1)).wrapping_add(r1.0);
        o += 1;
    }
    have
}

/// The ranges of bytes to fetch: the blocks missing, next to each other
/// together, and near ones too once there'd be too many requests.
fn ranges(c: &Control, have: &[Option<usize>]) -> Vec<(u64, u64)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for (i, h) in have.iter().enumerate() {
        if h.is_none() {
            match runs.last_mut() {
                Some((_, e)) if *e == i => *e = i + 1,
                _ => runs.push((i, i + 1)),
            }
        }
    }
    let mut gap = 1;
    while runs.len() > 64 {
        gap *= 2;
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for r in runs {
            match merged.last_mut() {
                Some((_, e)) if r.0 - *e <= gap => *e = r.1,
                _ => merged.push(r),
            }
        }
        runs = merged;
    }
    runs.into_iter().map(|(s, e)| ((s * c.blocksize) as u64, ((e * c.blocksize) as u64).min(c.length))).collect()
}

/// The new version made at `to`: the blocks `seed` has taken from it, the
/// rest fetched (`fetch(start, end)`: those bytes), each checked; renamed
/// into place only once its SHA-1 is the .zsync's.
pub fn make(c: &Control, seed: &Path, to: &Path, fetch: &mut dyn FnMut(u64, u64) -> Result<Vec<u8>>) -> Result<Made> {
    let old = fs::read(seed).with_context(|| format!("reading {}", seed.display()))?;
    let have = find(c, &old);
    let part = to.with_extension("part");
    let result = (|| -> Result<Made> {
        let mut out = fs::File::create(&part)?;
        out.set_len(c.length)?;
        let mut made = Made::default();
        for (i, h) in have.iter().enumerate() {
            if let Some(o) = h {
                let len = c.block_len(i);
                out.seek(SeekFrom::Start((i * c.blocksize) as u64))?;
                out.write_all(&old[*o..(*o + len).min(old.len())])?;
                // (The part of a last block past the seed's end: zeros, as set_len left it.)
                made.reused += len as u64;
            }
        }
        drop(old);
        for (s, e) in ranges(c, &have) {
            let data = fetch(s, e)?;
            made.requests += 1;
            ensure!(data.len() as u64 == e - s, "{} bytes came where {} were asked for", data.len(), e - s);
            made.fetched += data.len() as u64;
            let first = (s / c.blocksize as u64) as usize;
            for (k, block) in data.chunks(c.blocksize).enumerate() {
                let i = first + k;
                if have[i].is_some() {
                    continue;
                }
                if !c.is_block(i, block) {
                    return Err(anyhow::Error::new(BadBlock((i * c.blocksize) as u64)));
                }
            }
            out.seek(SeekFrom::Start(s))?;
            // (Blocks it had already, fetched with the rest: as they came, checked by the SHA-1.)
            for (k, block) in data.chunks(c.blocksize).enumerate() {
                if have[first + k].is_none() {
                    out.seek(SeekFrom::Start(s + (k * c.blocksize) as u64))?;
                    out.write_all(block)?;
                }
            }
        }
        out.sync_all()?;
        drop(out);
        let mut h = crate::openpgp::Sha1::default();
        let mut f = fs::File::open(&part)?;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let k = f.read(&mut buf)?;
            if k == 0 {
                break;
            }
            h.update(&buf[..k]);
        }
        if crate::openpgp::hex(&h.finish()).to_lowercase() != c.sha1 {
            return Err(anyhow::Error::new(BadSum));
        }
        Ok(made)
    })();
    match result {
        Ok(m) => {
            fs::rename(&part, to)?;
            Ok(m)
        }
        Err(e) => {
            let _ = fs::remove_file(&part);
            Err(e)
        }
    }
}

/// The server sends whole files only (no range requests).
#[derive(Debug)]
pub struct NoRanges;

impl std::fmt::Display for NoRanges {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("the server doesn't send parts of files")
    }
}
impl std::error::Error for NoRanges {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md4_is_md4() {
        let hex = |b: [u8; 16]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(hex(md4(b"")), "31d6cfe0d16ae931b73c59d7e0c089c0");
        assert_eq!(hex(md4(b"abc")), "a448017aaf21d8525fc10ae87aa6729d");
        assert_eq!(hex(md4(b"12345678901234567890123456789012345678901234567890123456789012345678901234567890")), "e33b4ddc9c38f2199c3e7b164fcc0536");
    }

    /// A .zsync as zsyncmake writes it, of `data`.
    pub fn zsyncmake(data: &[u8], bs: usize, (seq, rb, cb): (usize, usize, usize)) -> Vec<u8> {
        let mut out = format!(
            "zsync: 0.6.2\nFilename: app.AppImage\nMTime: Sat, 01 Jan 2000 00:00:00 +0000\nBlocksize: {bs}\nLength: {}\nHash-Lengths: {seq},{rb},{cb}\nURL: app.AppImage\nSHA-1: {}\n\n",
            data.len(),
            crate::openpgp::hex(&crate::openpgp::sha1(data)).to_lowercase()
        )
        .into_bytes();
        for chunk in data.chunks(bs) {
            let mut b = chunk.to_vec();
            b.resize(bs, 0);
            let r = packed(rsum(&b)).to_be_bytes();
            out.extend_from_slice(&r[4 - rb..]);
            out.extend_from_slice(&md4(&b)[..cb]);
        }
        out
    }

    fn noise(n: usize, seed: u64) -> Vec<u8> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    }

    #[test]
    fn only_what_changed_is_fetched() {
        let dir = std::env::temp_dir().join(format!("nami-zsync-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let old = noise(300_000, 1);
        // The new one: a few bytes changed, some put in (everything after moves), its end different.
        let mut new = old.clone();
        new[1000] ^= 0xff;
        new.splice(100_000..100_000, noise(777, 2));
        new.truncate(250_123);
        new.extend(noise(5000, 3));
        let seed = dir.join("old");
        fs::write(&seed, &old).unwrap();
        for lengths in [(1, 4, 16), (2, 2, 5), (2, 3, 8)] {
            let c = parse(&zsyncmake(&new, 2048, lengths)).unwrap();
            assert_eq!(c.length, new.len() as u64);
            let to = dir.join("new");
            let mut asked = 0;
            let made = make(&c, &seed, &to, &mut |s, e| {
                asked += e - s;
                Ok(new[s as usize..e as usize].to_vec())
            })
            .unwrap();
            assert_eq!(fs::read(&to).unwrap(), new, "{lengths:?}");
            assert_eq!(made.fetched, asked);
            // (About four blocks changed, and the end.)
            assert!(made.fetched < 20 * 2048, "{lengths:?}: fetched {}", made.fetched);
            assert_eq!(made.reused + made.fetched, new.len() as u64);
            let _ = fs::remove_file(&to);
            // A fetched block changed on the way: refused, nothing left.
            let e = make(&c, &seed, &to, &mut |s, e| {
                let mut d = new[s as usize..e as usize].to_vec();
                d[0] ^= 1;
                Ok(d)
            })
            .unwrap_err();
            assert!(e.downcast_ref::<BadBlock>().is_some(), "{e}");
            assert!(!to.exists() && !to.with_extension("part").exists());
        }
        // Nothing to start from: all of it fetched; a .zsync that says another SHA-1: refused.
        let empty = dir.join("empty");
        fs::write(&empty, b"").unwrap();
        let mut z = zsyncmake(&new, 2048, (2, 2, 5));
        let c = parse(&z).unwrap();
        let made = make(&c, &empty, &dir.join("n2"), &mut |s, e| Ok(new[s as usize..e as usize].to_vec())).unwrap();
        assert_eq!(made.fetched, new.len() as u64);
        let at = z.windows(6).position(|w| w == b"SHA-1:").unwrap() + 7;
        z[at] = if z[at] == b'0' { b'1' } else { b'0' };
        let c = parse(&z).unwrap();
        assert!(make(&c, &seed, &dir.join("n3"), &mut |s, e| Ok(new[s as usize..e as usize].to_vec())).unwrap_err().downcast_ref::<BadSum>().is_some());
        assert!(!dir.join("n3").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
