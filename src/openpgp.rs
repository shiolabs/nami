//! OpenPGP, as far as checking a software repository's signature needs it
//! (RFC 4880's version 4 keys and signatures): keys read (armored or not)
//! and their fingerprints, a detached or clearsigned signature checked
//! against them. RSA and Ed25519 keys, SHA-2 hashes; SHA-1 only for the
//! old binding signatures that tie a signing subkey to its key, never for
//! what's signed. A key's own word on itself counts: when it expires (its
//! newest self-signature's, so a key its maker renewed is good again) and
//! whether its maker revoked it, or a subkey; only what the key itself
//! signed. What's left out: ECDSA, v6 keys, revocation by another key.

use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::Engine;
use sha2::Digest;

/// A public key (or subkey): its packet's body, and what it is.
#[derive(Debug, Clone)]
pub struct Key {
    pub body: Vec<u8>,
    /// OpenPGP v4 fingerprint, upper-case hex.
    pub fingerprint: String,
    /// For a subkey: its primary's fingerprint.
    pub primary: Option<String>,
    material: Material,
    /// When it was made, and (as its newest self-signature says) when it
    /// expires; None: never.
    pub created: u64,
    pub expires: Option<u64>,
    /// Revoked by its maker: the reason they gave (maybe none).
    pub revoked: Option<String>,
    /// When the self-signature its expiry is from was made.
    sig_at: u64,
}

#[derive(Debug, Clone)]
enum Material {
    Rsa { n: Vec<u8>, e: Vec<u8> },
    Ed25519([u8; 32]),
    Other,
}

/// A signature: what kind, how it was made, and over what.
#[derive(Debug, Clone)]
pub struct Signature {
    pub kind: u8,
    pub algo: u8,
    pub hash: u8,
    /// Its version-4 part that's hashed too (version to the hashed subpackets' end).
    hashed: Vec<u8>,
    left16: [u8; 2],
    mpis: Vec<Vec<u8>>,
    /// The key ID or fingerprint it says made it (a hint only: every key is tried).
    pub issuer: Option<String>,
    /// What its hashed (signed) part says: when it was made, when the key
    /// it's on expires (seconds after the key was made; 0: never), and
    /// for a revocation, why.
    pub created: u64,
    key_expires: Option<u64>,
    reason: Option<(u8, String)>,
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

/// SHA-1, for v4 fingerprints, old binding signatures and zsync's
/// checksums (as they're read: a big download isn't held whole).
#[derive(Clone)]
pub struct Sha1 {
    h: [u32; 5],
    buf: Vec<u8>,
    len: u64,
}

impl Default for Sha1 {
    fn default() -> Self {
        Sha1 { h: [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0], buf: Vec::with_capacity(64), len: 0 }
    }
}

impl Sha1 {
    pub fn update(&mut self, mut data: &[u8]) {
        self.len = self.len.wrapping_add(data.len() as u64);
        if !self.buf.is_empty() {
            let take = (64 - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() == 64 {
                let block = std::mem::take(&mut self.buf);
                self.block(&block);
            }
        }
        let mut chunks = data.chunks_exact(64);
        for c in &mut chunks {
            self.block(c);
        }
        self.buf.extend_from_slice(chunks.remainder());
    }

    fn block(&mut self, chunk: &[u8]) {
        let h = &mut self.h;
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }

    pub fn finish(mut self) -> [u8; 20] {
        let bits = self.len.wrapping_mul(8);
        let mut tail = std::mem::take(&mut self.buf);
        tail.push(0x80);
        while tail.len() % 64 != 56 {
            tail.push(0);
        }
        tail.extend_from_slice(&bits.to_be_bytes());
        for c in tail.chunks(64) {
            self.block(c);
        }
        let mut out = [0u8; 20];
        for (i, x) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
        }
        out
    }
}

pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut s = Sha1::default();
    s.update(data);
    s.finish()
}

/// A signature by a key that isn't one of those it's checked with: what it
/// says made it (a key ID or fingerprint).
#[derive(Debug)]
pub struct UnknownKey(pub String);

impl std::fmt::Display for UnknownKey {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "it's signed by a key it didn't come with ({})", self.0)
    }
}

impl std::error::Error for UnknownKey {}

/// What's inside ASCII armor (each block, if there are several); binary
/// data as it is.
pub fn dearmor(data: &[u8]) -> Result<Vec<Vec<u8>>> {
    let text = String::from_utf8_lossy(data);
    if !text.contains("-----BEGIN PGP") {
        return Ok(vec![data.to_vec()]);
    }
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(raw) = lines.next() {
        // (In a script, it may be a line of `echo "-----BEGIN ...`.)
        let Some(at) = raw.find("-----BEGIN PGP") else { continue };
        if raw[at..].contains("SIGNED MESSAGE") {
            continue;
        }
        let mut lines = lines.by_ref().map(|l| l.trim().trim_matches(['"', '\'']).trim());
        // Its header lines ("Version: ..."), to the blank line; then base64.
        let mut b64 = String::new();
        let mut in_header = true;
        for l in lines.by_ref() {
            if l.contains("-----END PGP") {
                break;
            }
            if in_header {
                if l.is_empty() {
                    in_header = false;
                    continue;
                }
                if l.contains(": ") {
                    continue;
                }
                in_header = false;
            }
            // (The checksum, =XXXX, isn't data.)
            if l.starts_with('=') && l.len() == 5 {
                continue;
            }
            b64.push_str(l);
        }
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()).context("an armored block that isn't base64")?;
        out.push(bytes);
    }
    ensure!(!out.is_empty(), "no OpenPGP data in it");
    Ok(out)
}

/// The packets in binary OpenPGP data: (tag, body).
pub fn packets(data: &[u8]) -> Result<Vec<(u8, Vec<u8>)>> {
    let mut out = Vec::new();
    let mut at = 0;
    let byte = |i: usize| data.get(i).copied().ok_or_else(|| anyhow!("OpenPGP data cut short"));
    while at < data.len() {
        let b = byte(at)?;
        ensure!(b & 0x80 != 0, "not OpenPGP data");
        at += 1;
        if b & 0x40 != 0 {
            let tag = b & 0x3f;
            let mut body = Vec::new();
            loop {
                let o1 = byte(at)? as usize;
                let (len, partial) = match o1 {
                    0..=191 => {
                        at += 1;
                        (o1, false)
                    }
                    192..=223 => {
                        let l = ((o1 - 192) << 8) + byte(at + 1)? as usize + 192;
                        at += 2;
                        (l, false)
                    }
                    255 => {
                        let l = u32::from_be_bytes(data.get(at + 1..at + 5).context("OpenPGP data cut short")?.try_into().unwrap()) as usize;
                        at += 5;
                        (l, false)
                    }
                    _ => {
                        at += 1;
                        (1usize << (o1 & 0x1f), true)
                    }
                };
                body.extend_from_slice(data.get(at..at + len).context("OpenPGP data cut short")?);
                at += len;
                if !partial {
                    break;
                }
            }
            out.push((tag, body));
        } else {
            let tag = (b >> 2) & 0x0f;
            let (len, n) = match b & 3 {
                0 => (byte(at)? as usize, 1),
                1 => (((byte(at)? as usize) << 8) | byte(at + 1)? as usize, 2),
                2 => (u32::from_be_bytes(data.get(at..at + 4).context("OpenPGP data cut short")?.try_into().unwrap()) as usize, 4),
                _ => (data.len() - at, 0),
            };
            at += n;
            out.push((tag, data.get(at..at + len).context("OpenPGP data cut short")?.to_vec()));
            at += len;
        }
    }
    Ok(out)
}

/// An MPI at `at`: its bytes, and where the next thing starts.
fn mpi(d: &[u8], at: usize) -> Result<(Vec<u8>, usize)> {
    let bits = u16::from_be_bytes(d.get(at..at + 2).context("a number cut short")?.try_into().unwrap()) as usize;
    let len = bits.div_ceil(8);
    Ok((d.get(at + 2..at + 2 + len).context("a number cut short")?.to_vec(), at + 2 + len))
}

const ED25519_OID: &[u8] = &[0x2b, 0x06, 0x01, 0x04, 0x01, 0xda, 0x47, 0x0f, 0x01];

fn key_of(body: &[u8], primary: Option<String>) -> Result<Key> {
    ensure!(body.first() == Some(&4), "a version {} key, which nami can't read (only version 4)", body.first().copied().unwrap_or(0));
    ensure!(body.len() > 6, "a key cut short");
    let algo = body[5];
    let material = match algo {
        1..=3 => {
            let (n, at) = mpi(body, 6)?;
            let (e, _) = mpi(body, at)?;
            Material::Rsa { n, e }
        }
        22 => {
            let len = *body.get(6).context("a key cut short")? as usize;
            let oid = body.get(7..7 + len).context("a key cut short")?;
            let (p, _) = mpi(body, 7 + len)?;
            match (oid == ED25519_OID, p.first(), p.len()) {
                (true, Some(0x40), 33) => Material::Ed25519(p[1..].try_into().unwrap()),
                _ => Material::Other,
            }
        }
        27 => Material::Ed25519(body.get(6..38).context("a key cut short")?.try_into().unwrap()),
        _ => Material::Other,
    };
    let mut fp = vec![0x99, (body.len() >> 8) as u8, body.len() as u8];
    fp.extend_from_slice(body);
    let created = u32::from_be_bytes(body[1..5].try_into().unwrap()) as u64;
    Ok(Key { body: body.to_vec(), fingerprint: hex(&sha1(&fp)), primary, material, created, expires: None, revoked: None, sig_at: 0 })
}

fn signature_of(body: &[u8]) -> Result<Signature> {
    ensure!(body.first() == Some(&4), "a version {} signature, which nami can't read (only version 4)", body.first().copied().unwrap_or(0));
    ensure!(body.len() > 6, "a signature cut short");
    let (kind, algo, hash) = (body[1], body[2], body[3]);
    let hl = u16::from_be_bytes([body[4], body[5]]) as usize;
    let hashed = body.get(..6 + hl).context("a signature cut short")?.to_vec();
    let ul = u16::from_be_bytes(body.get(6 + hl..8 + hl).context("a signature cut short")?.try_into().unwrap()) as usize;
    let unhashed = body.get(8 + hl..8 + hl + ul).context("a signature cut short")?;
    let mut at = 8 + hl + ul;
    let left16: [u8; 2] = body.get(at..at + 2).context("a signature cut short")?.try_into().unwrap();
    at += 2;
    let mut mpis = Vec::new();
    if algo == 27 {
        mpis.push(body.get(at..at + 64).context("a signature cut short")?.to_vec());
    } else {
        while at < body.len() {
            let (m, next) = mpi(body, at)?;
            mpis.push(m);
            at = next;
        }
    }
    let mut issuer = None;
    let (mut created, mut key_expires, mut reason) = (0u64, None, None);
    for (n_area, area) in [&hashed[6..], unhashed].into_iter().enumerate() {
        let mut i = 0;
        while i < area.len() {
            let o1 = area[i] as usize;
            let (len, n) = match o1 {
                0..=191 => (o1, 1),
                192..=254 => (((o1 - 192) << 8) + *area.get(i + 1).unwrap_or(&0) as usize + 192, 2),
                _ => (area.get(i + 1..i + 5).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as usize).unwrap_or(0), 5),
            };
            let Some(sub) = area.get(i + n..i + n + len) else { break };
            let be32 = |b: &[u8]| b.get(..4).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as u64);
            match (sub.first().map(|t| t & 0x7f), n_area) {
                (Some(33), _) if sub.len() > 2 => issuer = Some(hex(&sub[2..])),
                (Some(16), _) if issuer.is_none() && sub.len() == 9 => issuer = Some(hex(&sub[1..])),
                // (Only the signed part says when and why.)
                (Some(2), 0) => created = be32(&sub[1..]).unwrap_or(0),
                (Some(9), 0) => key_expires = be32(&sub[1..]),
                (Some(29), 0) if sub.len() > 1 => reason = Some((sub[1], String::from_utf8_lossy(&sub[2..]).trim().to_string())),
                _ => {}
            }
            i += n + len;
        }
    }
    Ok(Signature { kind, algo, hash, hashed, left16, mpis, issuer, created, key_expires, reason })
}

/// The keys in a keyring (armored or binary, one block or several): each
/// primary key and subkey, subkeys only with a good binding signature by
/// their primary. What each key signed about itself, in any of the blocks,
/// counts: its newest self-signature's expiry, a revocation. (Several
/// copies of one key, as fetched again later, are one key.)
pub fn keys(data: &[u8]) -> Result<Vec<Key>> {
    let mut out: Vec<Key> = Vec::new();
    // (A subkey's revocation can come before its binding.)
    let mut revoked_subs: Vec<(String, String)> = Vec::new();
    let one = |out: &mut Vec<Key>, k: &Key| match out.iter().position(|o| o.fingerprint == k.fingerprint) {
        Some(i) => i,
        None => {
            out.push(k.clone());
            out.len() - 1
        }
    };
    for block in dearmor(data)? {
        let mut primary: Option<usize> = None;
        let mut sub: Option<Key> = None;
        let mut uid: Option<Vec<u8>> = None;
        for (tag, body) in packets(&block)? {
            match tag {
                6 => {
                    (sub, uid) = (None, None);
                    primary = key_of(&body, None).ok().map(|k| one(&mut out, &k));
                }
                13 | 17 if sub.is_none() => {
                    // A user ID (or attribute), as its certification hashes it.
                    let mut v = vec![if tag == 13 { 0xb4 } else { 0xd1 }];
                    v.extend_from_slice(&(body.len() as u32).to_be_bytes());
                    v.extend_from_slice(&body);
                    uid = Some(v);
                }
                14 => {
                    uid = None;
                    sub = primary.and_then(|p| key_of(&body, Some(out[p].fingerprint.clone())).ok());
                }
                2 => {
                    let Some(p) = primary else { continue };
                    let Ok(sig) = signature_of(&body) else { continue };
                    let pk = out[p].clone();
                    let reason = sig.reason.as_ref().map(|(_, r)| r.clone()).unwrap_or_default();
                    match (sig.kind, &sub) {
                        // The key revoked, by itself.
                        (0x20, _) => {
                            if out[p].revoked.is_none() && check(&sig, &framed(&pk.body), &pk, true).is_ok() {
                                out[p].revoked = Some(reason);
                            }
                        }
                        // Its own word on itself: a user ID's certification, or a direct one.
                        (0x10..=0x13 | 0x1f, None) => {
                            let mut data = framed(&pk.body);
                            if sig.kind != 0x1f {
                                let Some(u) = &uid else { continue };
                                data.extend_from_slice(u);
                            }
                            if sig.created >= pk.sig_at && check(&sig, &data, &pk, true).is_ok() {
                                out[p].sig_at = sig.created;
                                out[p].expires = sig.key_expires.filter(|e| *e > 0).map(|e| pk.created + e);
                            }
                        }
                        (0x18 | 0x28, Some(s)) => {
                            let mut data = framed(&pk.body);
                            data.extend(framed(&s.body));
                            if check(&sig, &data, &pk, true).is_err() {
                                continue;
                            }
                            if sig.kind == 0x28 {
                                revoked_subs.push((s.fingerprint.clone(), reason));
                                continue;
                            }
                            let i = one(&mut out, s);
                            if sig.created >= out[i].sig_at {
                                out[i].sig_at = sig.created;
                                out[i].expires = sig.key_expires.filter(|e| *e > 0).map(|e| s.created + e);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    for (fp, why) in revoked_subs {
        if let Some(k) = out.iter_mut().find(|k| k.fingerprint == fp && k.revoked.is_none()) {
            k.revoked = Some(why);
        }
    }
    Ok(out)
}

/// A key that made a signature, but can't be trusted now: it expired, or
/// its maker revoked it (or the subkey that signed).
#[derive(Debug, Clone, PartialEq)]
pub struct NotUsable {
    pub fingerprint: String,
    pub subkey: bool,
    /// When it expired; or it was revoked, for this reason (maybe "").
    pub expired: Option<u64>,
    pub revoked: Option<String>,
}

impl std::fmt::Display for NotUsable {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let what = if self.subkey { "the subkey that signed it" } else { "its key" };
        match (&self.expired, &self.revoked) {
            (_, Some(r)) if !r.is_empty() => write!(f, "{what} ({}) was revoked by its maker: {r}", self.fingerprint),
            (_, Some(_)) => write!(f, "{what} ({}) was revoked by its maker", self.fingerprint),
            (Some(t), _) => write!(f, "{what} ({}) expired on {}", self.fingerprint, date(*t)),
            _ => write!(f, "{what} ({}) can't be used", self.fingerprint),
        }
    }
}

impl std::error::Error for NotUsable {}

/// A time as a date (UTC), as people write it: 2025-06-01.
pub fn date(t: u64) -> String {
    // Howard Hinnant's days-to-civil.
    let z = (t / 86400) as i64 + 719468;
    let (era, doe) = (z.div_euclid(146097), z.rem_euclid(146097));
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{m:02}-{d:02}", yoe + era * 400 + (m <= 2) as i64)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Whether `key` (of `keyring`) can be trusted at `at`: it, and for a
/// subkey its primary, neither expired nor revoked.
pub fn usable(keyring: &[Key], key: &Key, at: u64) -> std::result::Result<(), NotUsable> {
    let primary = key.primary.as_ref().and_then(|p| keyring.iter().find(|k| k.fingerprint == *p));
    for (k, subkey) in primary.map(|p| (p, false)).into_iter().chain([(key, key.primary.is_some())]) {
        let expired = k.expires.filter(|e| *e <= at);
        if k.revoked.is_some() || expired.is_some() {
            return Err(NotUsable { fingerprint: k.fingerprint.clone(), subkey, expired, revoked: k.revoked.clone() });
        }
    }
    Ok(())
}

/// What a keyring's keys say of themselves, to tell whether a copy fetched
/// again said more: each key, when it expires, whether it's revoked.
pub fn state(keys: &[Key]) -> Vec<(String, Option<u64>, bool)> {
    let mut v: Vec<_> = keys.iter().map(|k| (k.fingerprint.clone(), k.expires, k.revoked.is_some())).collect();
    v.sort();
    v
}

/// A key's packet body, as it's hashed in a binding signature.
fn framed(body: &[u8]) -> Vec<u8> {
    let mut v = vec![0x99, (body.len() >> 8) as u8, body.len() as u8];
    v.extend_from_slice(body);
    v
}

/// The primary keys' fingerprints in a keyring.
pub fn fingerprints(data: &[u8]) -> Result<Vec<String>> {
    Ok(keys(data)?.into_iter().filter(|k| k.primary.is_none()).map(|k| k.fingerprint).collect())
}

fn digest(hash: u8, data: &[u8]) -> Result<(Vec<u8>, &'static [u8])> {
    Ok(match hash {
        2 => (sha1(data).to_vec(), &[0x30, 0x21, 0x30, 0x09, 0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a, 0x05, 0x00, 0x04, 0x14]),
        8 => (sha2::Sha256::digest(data).to_vec(), &[0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20]),
        9 => (sha2::Sha384::digest(data).to_vec(), &[0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02, 0x05, 0x00, 0x04, 0x30]),
        10 => (sha2::Sha512::digest(data).to_vec(), &[0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05, 0x00, 0x04, 0x40]),
        11 => (sha2::Sha224::digest(data).to_vec(), &[0x30, 0x2d, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x04, 0x05, 0x00, 0x04, 0x1c]),
        h => bail!("signed with a hash nami doesn't know ({h})"),
    })
}

fn left_pad(v: &[u8], n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n.saturating_sub(v.len())];
    out.extend_from_slice(v);
    out
}

/// Whether `sig` is `key`'s over `data` (as hashed: canonical text already).
fn check(sig: &Signature, data: &[u8], key: &Key, binding: bool) -> Result<()> {
    ensure!(sig.hash != 2 || binding, "signed with SHA-1, which isn't safe any more");
    let mut input = data.to_vec();
    input.extend_from_slice(&sig.hashed);
    input.extend_from_slice(&[0x04, 0xff]);
    input.extend_from_slice(&(sig.hashed.len() as u32).to_be_bytes());
    let (hashed, prefix) = digest(sig.hash, &input)?;
    ensure!(hashed[..2] == sig.left16, "not this key's");
    match (&key.material, sig.algo) {
        (Material::Rsa { n, e }, 1 | 3) => {
            let pk = rsa::RsaPublicKey::new_with_max_size(rsa::BigUint::from_bytes_be(n), rsa::BigUint::from_bytes_be(e), 8192).map_err(|e| anyhow!("{e}"))?;
            let s = left_pad(sig.mpis.first().context("no signature")?, n.len());
            let scheme = rsa::Pkcs1v15Sign { hash_len: Some(hashed.len()), prefix: prefix.into() };
            pk.verify(scheme, &hashed, &s).map_err(|_| anyhow!("not this key's"))
        }
        (Material::Ed25519(p), 22 | 27) => {
            let raw = if sig.algo == 27 {
                sig.mpis.first().cloned().context("no signature")?
            } else {
                ensure!(sig.mpis.len() == 2, "a broken signature");
                [left_pad(&sig.mpis[0], 32), left_pad(&sig.mpis[1], 32)].concat()
            };
            let vk = ed25519_dalek::VerifyingKey::from_bytes(p).map_err(|_| anyhow!("a broken key"))?;
            let s = ed25519_dalek::Signature::from_slice(&raw).map_err(|_| anyhow!("a broken signature"))?;
            vk.verify_strict(&hashed, &s).map_err(|_| anyhow!("not this key's"))
        }
        _ => bail!("not this key's"),
    }
}

/// Text as a text signature (0x01) hashes it: each line ending CR LF.
fn canonical(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + text.len() / 32);
    let mut i = 0;
    while i < text.len() {
        match text[i] {
            b'\r' if text.get(i + 1) == Some(&b'\n') => {
                out.extend_from_slice(b"\r\n");
                i += 1;
            }
            b'\n' | b'\r' => out.extend_from_slice(b"\r\n"),
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// Why none of `keyring` made `sig`: it was changed after it was signed
/// (it says one of them made it), or another key made it.
fn not_by(keyring: &[Key], sig: &Signature) -> anyhow::Error {
    let issuer = sig.issuer.clone().unwrap_or_default();
    let ours = !issuer.is_empty() && keyring.iter().any(|k| k.fingerprint.ends_with(&issuer));
    if ours {
        anyhow!("its signature doesn't match what's signed: it was changed after it was signed")
    } else {
        anyhow::Error::new(UnknownKey(if issuer.is_empty() { "a key that doesn't say whose".into() } else { issuer }))
    }
}

/// The key of `keyring` that made `sig` over `hashed` and can be trusted
/// now (Ok(Err): why not: another key's, changed, expired, revoked). Err:
/// refused whoever made it (SHA-1, a hash nami doesn't know).
fn by<'a>(keyring: &'a [Key], sig: &Signature, hashed: &[u8]) -> Result<std::result::Result<&'a Key, anyhow::Error>> {
    let mut unusable = None;
    for key in keyring {
        match check(sig, hashed, key, false) {
            Ok(()) => match usable(keyring, key, now()) {
                Ok(()) => return Ok(Ok(key)),
                Err(e) => unusable = Some(e),
            },
            Err(e) if e.to_string().contains("SHA-1") || e.to_string().contains("hash") => return Err(e),
            Err(_) => {}
        }
    }
    Ok(Err(match unusable {
        Some(e) => anyhow::Error::new(e),
        None => not_by(keyring, sig),
    }))
}

/// The key of `keyring` whose signature `sig_data` is over `data`: its
/// fingerprint (the primary's, for a subkey). Refused otherwise.
pub fn verify_detached(keyring: &[Key], data: &[u8], sig_data: &[u8]) -> Result<String> {
    let mut why = anyhow!("there's no signature in it");
    for block in dearmor(sig_data)? {
        for (tag, body) in packets(&block)? {
            if tag != 2 {
                continue;
            }
            let sig = signature_of(&body)?;
            let hashed = match sig.kind {
                0x00 => data.to_vec(),
                0x01 => canonical(data),
                k => bail!("a signature of a kind that doesn't sign a file ({k:#x})"),
            };
            match by(keyring, &sig, &hashed)? {
                Ok(key) => return Ok(key.primary.clone().unwrap_or_else(|| key.fingerprint.clone())),
                Err(e) => why = e,
            }
        }
    }
    Err(why)
}

/// A clearsigned text (apt's InRelease): the text that's signed, and the
/// fingerprint of the key of `keyring` that signed it. Only the signed
/// text is given back: nothing around it.
pub fn verify_clearsigned(keyring: &[Key], data: &[u8]) -> Result<(String, String)> {
    let text = std::str::from_utf8(data).context("not text")?;
    let start = text.find("-----BEGIN PGP SIGNED MESSAGE-----").context("it isn't signed")?;
    let rest = &text[start..];
    // The armor headers (Hash: ...), to the blank line.
    let body_at = rest.find("\n\n").map(|i| i + 2).or_else(|| rest.find("\r\n\r\n").map(|i| i + 4)).context("a broken signed text")?;
    let body = &rest[body_at..];
    let sig_at = body.find("\n-----BEGIN PGP SIGNATURE-----").context("a signed text without its signature")?;
    let signed = &body[..sig_at];
    let sig_block = &body[sig_at + 1..];
    ensure!(sig_block.contains("-----END PGP SIGNATURE-----"), "a signed text cut short");
    let lines: Vec<String> = signed.split('\n').map(|l| {
        let l = l.strip_suffix('\r').unwrap_or(l);
        let l = l.strip_prefix("- ").unwrap_or(l);
        l.to_string()
    }).collect();
    // As hashed: each line without its trailing spaces, CR LF between, none after the last.
    let hashed = lines.iter().map(|l| l.trim_end_matches([' ', '\t'])).collect::<Vec<_>>().join("\r\n");
    let mut why = anyhow!("there's no signature in it");
    for block in dearmor(sig_block.as_bytes())? {
        for (tag, body) in packets(&block)? {
            if tag != 2 {
                continue;
            }
            let sig = signature_of(&body)?;
            ensure!(sig.kind == 0x01, "a signature of the wrong kind for signed text");
            match by(keyring, &sig, hashed.as_bytes())? {
                Ok(key) => return Ok((lines.join("\n"), key.primary.clone().unwrap_or_else(|| key.fingerprint.clone()))),
                Err(e) => why = e,
            }
        }
    }
    Err(why)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_is_sha1() {
        assert_eq!(hex(&sha1(b"abc")), "A9993E364706816ABA3E25717850C26C9CD0D89D");
        assert_eq!(hex(&sha1(b"")), "DA39A3EE5E6B4B0D3255BFEF95601890AFD80709");
        assert_eq!(hex(&sha1(&[b'a'; 1000])), "291E9A6C66994949B57BA5E650361E98FC36B1BA");
    }

    #[test]
    fn armor_and_packets_are_read() {
        let a = "-----BEGIN PGP PUBLIC KEY BLOCK-----\nVersion: x\n\nmQENBA==\n=abcd\n-----END PGP PUBLIC KEY BLOCK-----\n";
        let b = dearmor(a.as_bytes()).unwrap();
        assert_eq!(b, vec![vec![0x99, 0x01, 0x0d, 0x04]]);
        // An old-format packet, then a new-format one.
        let p = packets(&[0x98, 0x02, 4, 5, 0xc2, 0x01, 9]).unwrap();
        assert_eq!(p, vec![(6, vec![4, 5]), (2, vec![9])]);
    }

    const RSA: &[u8] = include_bytes!("../tests/fixtures/openpgp/rsa.gpg");
    const ED: &[u8] = include_bytes!("../tests/fixtures/openpgp/ed.asc");
    const RELEASE: &[u8] = include_bytes!("../tests/fixtures/openpgp/Release");

    #[test]
    fn keys_and_their_fingerprints() {
        assert_eq!(fingerprints(RSA).unwrap(), vec!["9782AD08406E8500EDE25C10E5B13C40D29750B6"]);
        let ed = keys(ED).unwrap();
        // The primary, and its signing subkey (bound to it).
        assert_eq!(ed.iter().map(|k| k.fingerprint.as_str()).collect::<Vec<_>>(), vec!["53901FC523D6FCE8A3F1C47E1B592B09FA2FDB68", "D356C080B5B2D0B45AC490731A68B46057DFB3A0"]);
        assert_eq!(ed[1].primary.as_deref(), Some("53901FC523D6FCE8A3F1C47E1B592B09FA2FDB68"));
    }

    #[test]
    fn signatures_are_checked() {
        let (rsa, ed) = (keys(RSA).unwrap(), keys(ED).unwrap());
        let both: Vec<Key> = rsa.iter().chain(&ed).cloned().collect();
        // Clearsigned (InRelease), by either.
        let (text, by) = verify_clearsigned(&rsa, include_bytes!("../tests/fixtures/openpgp/InRelease-rsa")).unwrap();
        assert_eq!(text.as_bytes(), RELEASE);
        assert_eq!(by, "9782AD08406E8500EDE25C10E5B13C40D29750B6");
        let (_, by) = verify_clearsigned(&both, include_bytes!("../tests/fixtures/openpgp/InRelease-ed")).unwrap();
        assert_eq!(by, "53901FC523D6FCE8A3F1C47E1B592B09FA2FDB68");
        // Detached, binary and armored.
        assert!(verify_detached(&rsa, RELEASE, include_bytes!("../tests/fixtures/openpgp/Release.gpg")).is_ok());
        assert!(verify_detached(&ed, RELEASE, include_bytes!("../tests/fixtures/openpgp/Release.asc")).is_ok());
        // Another key's: refused.
        assert!(verify_clearsigned(&ed, include_bytes!("../tests/fixtures/openpgp/InRelease-rsa")).unwrap_err().to_string().contains("didn't come with"));
        assert!(verify_detached(&rsa, RELEASE, include_bytes!("../tests/fixtures/openpgp/Release.asc")).is_err());
        // Changed: refused.
        let mut changed = RELEASE.to_vec();
        changed[3] ^= 1;
        assert!(verify_detached(&rsa, &changed, include_bytes!("../tests/fixtures/openpgp/Release.gpg")).is_err());
        let tampered = String::from_utf8_lossy(include_bytes!("../tests/fixtures/openpgp/InRelease-rsa")).replace(" 12 main", " 13 main");
        assert!(verify_clearsigned(&rsa, tampered.as_bytes()).is_err());
        assert!(verify_clearsigned(&rsa, tampered.as_bytes()).unwrap_err().to_string().contains("changed after"));
        assert!(verify_detached(&ed, RELEASE, include_bytes!("../tests/fixtures/openpgp/Release.gpg")).unwrap_err().downcast_ref::<UnknownKey>().is_some());
        // In a script, as an echo's.
        let script = format!("#!/bin/sh\necho \"{}\" | gpg --dearmor > /usr/share/keyrings/x.gpg\n", String::from_utf8_lossy(ED).trim());
        assert_eq!(fingerprints(script.as_bytes()).unwrap(), vec!["53901FC523D6FCE8A3F1C47E1B592B09FA2FDB68"]);
    }

    /// Binary OpenPGP data, its packets as given (new format).
    fn enpacket(ps: &[(u8, Vec<u8>)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (tag, body) in ps {
            out.push(0xc0 | tag);
            out.push(0xff);
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            out.extend_from_slice(body);
        }
        out
    }

    fn fx(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/tests/fixtures/openpgp/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    fn ring(names: &[&str]) -> Vec<Key> {
        let all: Vec<u8> = names.iter().flat_map(|n| dearmor(&fx(n)).unwrap().concat()).collect();
        keys(&all).unwrap()
    }

    #[test]
    fn an_expired_key_and_a_renewed_one() {
        let rel = fx("Release");
        let sig = fx("Release-lapsed.asc");
        // (Made in 2024, to expire on 2025-01-01: its signing subkey's signature from before.)
        let lapsed = ring(&["lapsed.asc"]);
        assert_eq!(lapsed.len(), 2);
        assert_eq!(lapsed[0].expires.map(date).as_deref(), Some("2025-01-01"));
        let e = verify_detached(&lapsed, &rel, &sig).unwrap_err();
        let n = e.downcast_ref::<NotUsable>().expect("not usable");
        assert_eq!((n.fingerprint.as_str(), n.subkey), ("BF6EE4CF3ABE6CA132F895EB3A2A6FA105E90351", false));
        assert!(e.to_string().contains("expired on 2025-01-01"), "{e}");
        // Renewed by its maker (a newer self-signature, to 2099), fetched again: good.
        let renewed = ring(&["lapsed.asc", "lapsed-renewed.asc"]);
        assert_eq!(renewed[0].expires.map(date).as_deref(), Some("2099-01-01"));
        assert_eq!(verify_detached(&renewed, &rel, &sig).unwrap(), "BF6EE4CF3ABE6CA132F895EB3A2A6FA105E90351");
        // (Either order: the newest self-signature counts.)
        assert!(verify_detached(&ring(&["lapsed-renewed.asc", "lapsed.asc"]), &rel, &sig).is_ok());
        // A "renewal" its key didn't sign (changed): not taken.
        let block = dearmor(&fx("lapsed-renewed.asc")).unwrap().concat();
        let mut ps = packets(&block).unwrap();
        let at = ps.iter().position(|(t, b)| *t == 2 && b[1] == 0x13).unwrap();
        let last = ps[at].1.len() - 1;
        ps[at].1[last] ^= 1;
        let mut forged = dearmor(&fx("lapsed.asc")).unwrap().concat();
        forged.extend(enpacket(&ps));
        let k = keys(&forged).unwrap();
        assert_eq!(k[0].expires.map(date).as_deref(), Some("2025-01-01"));
        assert!(verify_detached(&k, &rel, &sig).unwrap_err().downcast_ref::<NotUsable>().is_some());
        // Nor another key's self-signature put on it: its user ID's signature from the other key.
        let other = dearmor(ED).unwrap().concat();
        let theirs = packets(&other).unwrap().into_iter().find(|(t, b)| *t == 2 && b[1] == 0x13).unwrap();
        let mut swapped = packets(&dearmor(&fx("lapsed.asc")).unwrap().concat()).unwrap();
        swapped.insert(2, theirs);
        let k = keys(&enpacket(&swapped)).unwrap();
        assert_eq!(k[0].expires.map(date).as_deref(), Some("2025-01-01"));
    }

    #[test]
    fn revoked_keys_and_subkeys() {
        let rel = fx("Release");
        // Good until its maker revoked it.
        let before = ring(&["revoked-before.asc"]);
        assert!(verify_detached(&before, &rel, &fx("Release-revoked.asc")).is_ok());
        let after = ring(&["revoked-before.asc", "revoked.asc"]);
        let e = verify_detached(&after, &rel, &fx("Release-revoked.asc")).unwrap_err();
        assert!(e.downcast_ref::<NotUsable>().is_some_and(|n| n.revoked.is_some() && !n.subkey), "{e}");
        assert!(e.to_string().contains("was revoked by its maker"), "{e}");
        // A revocation that isn't the key's own (changed): not taken.
        let mut ps = packets(&dearmor(&fx("revoked.asc")).unwrap().concat()).unwrap();
        let at = ps.iter().position(|(t, b)| *t == 2 && b[1] == 0x20).unwrap();
        let last = ps[at].1.len() - 1;
        ps[at].1[last] ^= 1;
        assert!(verify_detached(&keys(&enpacket(&ps)).unwrap(), &rel, &fx("Release-revoked.asc")).is_ok());
        // The subkey that signed, revoked (its reason said); or expired.
        let e = verify_detached(&ring(&["subrevoked.asc"]), &rel, &fx("Release-subrevoked.asc")).unwrap_err();
        let n = e.downcast_ref::<NotUsable>().unwrap();
        assert_eq!((n.fingerprint.as_str(), n.subkey, n.revoked.as_deref()), ("5F1A76D34FC8D1BC3F038C6C14914D23D61095B3", true, Some("It was lost.")));
        assert!(e.to_string().contains("the subkey that signed it (5F1A76D34FC8D1BC3F038C6C14914D23D61095B3) was revoked by its maker: It was lost."), "{e}");
        let e = verify_detached(&ring(&["subexpired.asc"]), &rel, &fx("Release-subexpired.asc")).unwrap_err();
        assert!(e.downcast_ref::<NotUsable>().is_some_and(|n| n.subkey && n.expired.is_some()), "{e}");
        // Keys that never expire, as before.
        assert!(keys(RSA).unwrap().iter().all(|k| k.expires.is_none() && k.revoked.is_none()));
        assert_eq!(date(0), "1970-01-01");
    }

    #[test]
    fn text_is_canonical() {
        assert_eq!(canonical(b"a\nb\r\nc"), b"a\r\nb\r\nc");
    }
}
