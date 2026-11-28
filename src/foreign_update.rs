//! Apps from downloads, kept up to date (`nami file-apps --update`), as the
//! rest of namiOS is: once a day in the background, on the same switch as
//! namiOS's own update (nami-update.timer), and each app's own switch.
//!
//! An update is only installed when it can be shown to be its maker's:
//! - an apt repository (the source its package's scripts would add): its
//!   InRelease (or Release and Release.gpg) signed by the key the package
//!   came with, pinned in app.json when it was installed; the Packages
//!   list's SHA-256 as the signed Release says; the .deb's as Packages says.
//! - a yum/dnf repository: repomd.xml signed (repomd.xml.asc), primary.xml's
//!   checksum as repomd says, the .rpm's as primary says.
//! - an AppImage's update information (zsync, or a GitHub release's zsync):
//!   over HTTPS from where the AppImage says, its length and SHA-1 as the
//!   .zsync says (and GitHub's SHA-256, where it gives one). Only the blocks
//!   that changed are fetched (src/zsync.rs), from the one installed (kept
//!   in its folder), each checked, then the whole file, before it's used.
//!   A signed AppImage (.sha256_sig, .sig_key) has its key pinned as it's
//!   installed; each update must be signed by it, and another key is refused.
//! - a download from a GitHub release (the browser's note of where it came
//!   from, or the package's homepage there): the same repository's newer
//!   release, the same file but for the version, only when GitHub gives
//!   its SHA-256 and the installed one matches its own release's.
//! Anything else is "check manually". A bad signature or checksum is
//! refused (logged, and said in Settings); a repository signing with
//! another key is refused, and the person told; offline, nothing's said.
//!
//! Keys are never trusted later unless they're the pinned ones: a key
//! fetched again (from where the package said, or as an update carries
//! it) only adds what a pinned key has signed for itself: a new subkey, a
//! renewal, a revocation. A key that's expired, or that its maker revoked
//! (or the subkey that signed), is refused, in plain words.
//!
//! An app that's running isn't changed under it: its update is fetched and
//! checked, and put in place once it's closed (the next run; at login, or
//! within the hour). The old version stays until the new one is in place
//! (foreign::install keeps it until the end, and puts it back if anything
//! fails), with everything it had outside its folder (menu entries, icons,
//! kinds of file, links it opens, commands, libraries from the catalog),
//! made again by the same setup as when it was installed.
//!
//! What it knows of each app is in ~/.local/share/nami/app-updates.json,
//! for Settings; what it did, in ~/.local/state/nami/app-updates.log.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::Digest;

use super::*;
use crate::openpgp;

/// How an app's updates are known to be its maker's (in app.json).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Trust {
    /// The fingerprints of the keys its repository's index (or its AppImage)
    /// is signed with, as its package came with them (pinned when it was installed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
    /// Those keys (OpenPGP, base64).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub keyring: String,
    /// Where they came from: "in the package (/usr/share/keyrings/x.gpg)", a URL.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key_from: String,
    /// Where its scripts fetch the key from: for a new subkey of it, later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_url: Option<String>,
    /// An AppImage's update information (`zsync|https://...`), and its SHA-1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appimage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    /// Where the browser said it was downloaded from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// The GitHub repository it's released on (owner/name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<String>,
}

// ---------------------------------------------------------------- at install

/// Where keyrings are put by packages.
const KEY_DIRS: &[&str] = &["usr/share/keyrings/", "etc/apt/trusted.gpg.d/", "etc/apt/keyrings/", "etc/pki/rpm-gpg/", "usr/share/distribution-gpg-keys/"];

/// The words of a line that name a key: `signed-by=/usr/share/keyrings/x.gpg`,
/// `Signed-By: ...`, `gpgkey=file:///etc/pki/rpm-gpg/X https://...`; and
/// URLs of keys a script fetches (`curl https://.../microsoft.asc`).
fn named_keys(text: &str) -> (Vec<String>, Vec<String>) {
    let (mut paths, mut urls) = (Vec::new(), Vec::new());
    let mut take = |v: &str| {
        let v = v.trim().trim_matches(['"', '\'', ']', '[', ',', ';', ')', '(']);
        if let Some(p) = v.strip_prefix("file://") {
            paths.push(p.to_string());
        } else if v.starts_with('/') && v.len() > 1 {
            paths.push(v.to_string());
        } else if v.starts_with("https://") || v.starts_with("http://") {
            // (Over plain HTTP too: a key fetched again later is only taken
            // as far as the pinned key signed it. Trusted first from HTTPS only.)
            urls.push(v.to_string());
        }
    };
    for line in text.lines() {
        let l = line.trim();
        for (i, _) in l.match_indices("signed-by=") {
            let v = &l[i + 10..];
            take(v.split([' ', ']', ',', '"', '\'']).next().unwrap_or(""));
        }
        if let Some(v) = l.strip_prefix("Signed-By:") {
            take(v);
        }
        if let Some(v) = l.strip_prefix("gpgkey=") {
            for w in v.split([' ', ',']) {
                take(w);
            }
        }
        // A key fetched by a script.
        for w in l.split(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '>' || c == '<' || c == '|') {
            let lw = w.to_lowercase();
            if (w.starts_with("https://") || w.starts_with("http://")) && (lw.ends_with(".asc") || lw.ends_with(".gpg") || lw.ends_with(".key") || lw.ends_with(".pub") || lw.contains("/keys/") || lw.contains("signing_key") || lw.contains("gpgkey")) {
                take(w);
            }
        }
    }
    paths.dedup();
    urls.dedup();
    (paths, urls)
}

/// The keys an app's package came with or names, for its repository:
/// (keyring, where from, a URL to fetch them from later).
fn find_keys(texts: &[&str], paths: &[&str], read: &mut dyn FnMut(&str) -> Option<Vec<u8>>, fetch: bool) -> (Vec<u8>, String, Option<String>) {
    let mut ring: Vec<u8> = Vec::new();
    let mut from: Vec<String> = Vec::new();
    let add = |data: &[u8], said: String, ring: &mut Vec<u8>, from: &mut Vec<String>| {
        let Ok(blocks) = openpgp::dearmor(data) else { return };
        for b in blocks {
            if openpgp::keys(&b).is_ok_and(|k| !k.is_empty()) {
                ring.extend_from_slice(&b);
                if !from.contains(&said) {
                    from.push(said.clone());
                }
            }
        }
    };
    let (mut named, mut urls) = (Vec::new(), Vec::new());
    for t in texts {
        let (p, u) = named_keys(t);
        named.extend(p);
        urls.extend(u);
        // Written out in a script (VS Code's postinst, Chrome's cron job).
        if t.contains("-----BEGIN PGP PUBLIC KEY BLOCK-----") {
            add(t.as_bytes(), "in its package's scripts".into(), &mut ring, &mut from);
        }
    }
    let mut looked: Vec<String> = Vec::new();
    for p in named.iter().map(|p| p.trim_start_matches('/').to_string()).chain(paths.iter().filter(|p| KEY_DIRS.iter().any(|d| p.starts_with(d))).map(|p| p.to_string())) {
        if looked.contains(&p) || !paths.contains(&p.as_str()) {
            continue;
        }
        looked.push(p.clone());
        if let Some(d) = read(&p) {
            add(&d, format!("in its package (/{p})"), &mut ring, &mut from);
        }
    }
    let url = urls.first().cloned();
    if ring.is_empty() && fetch {
        for u in urls.iter().filter(|u| u.starts_with("https://")).take(3) {
            if let Ok(d) = get(u, 1 << 20) {
                add(&d, u.clone(), &mut ring, &mut from);
                if !ring.is_empty() {
                    break;
                }
            }
        }
    }
    (ring, from.join(", "), url)
}

/// The browser's note of where a file was downloaded from (Chrome's,
/// curl --xattr's): `user.xdg.origin.url`.
fn origin_url(file: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(file.as_os_str().as_bytes()).ok()?;
    let name = c"user.xdg.origin.url";
    let mut buf = vec![0u8; 4096];
    #[cfg(target_os = "linux")]
    let n = unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
    #[cfg(not(target_os = "linux"))]
    let n = unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), buf.as_mut_ptr().cast(), buf.len(), 0, 0) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf8(buf).ok().map(|s| s.trim_end_matches('\0').to_string()).filter(|s| s.starts_with("http"))
}

/// owner/name of a GitHub repository a URL is of.
fn github_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://github.com/")?;
    let mut parts = rest.split('/');
    let (o, r) = (parts.next()?, parts.next()?.trim_end_matches(".git"));
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    (ok(o) && ok(r)).then(|| format!("{o}/{r}"))
}

/// An AppImage's ELF sections: (name, where it starts, its length).
fn appimage_sections(file: &Path) -> Option<Vec<(String, u64, u64)>> {
    let mut f = fs::File::open(file).ok()?;
    let mut head = [0u8; 64];
    f.read_exact(&mut head).ok()?;
    if &head[..4] != b"\x7fELF" || head[4] != 2 {
        return None;
    }
    let u16_at = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as u64;
    let u64_at = |b: &[u8], o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    let (shoff, shentsize, shnum, shstrndx) = (u64_at(&head, 0x28), u16_at(&head, 0x3a), u16_at(&head, 0x3c), u16_at(&head, 0x3e));
    if shentsize < 64 || shnum == 0 || shnum > 200 || shstrndx >= shnum {
        return None;
    }
    let read_at = |f: &mut fs::File, off: u64, len: usize| -> Option<Vec<u8>> {
        use std::io::{Seek, SeekFrom};
        f.seek(SeekFrom::Start(off)).ok()?;
        let mut b = vec![0u8; len];
        f.read_exact(&mut b).ok()?;
        Some(b)
    };
    let table = read_at(&mut f, shoff, (shentsize * shnum) as usize)?;
    let sec = |i: u64| &table[(i * shentsize) as usize..((i + 1) * shentsize) as usize];
    let strs = sec(shstrndx);
    let names = read_at(&mut f, u64_at(strs, 0x18), (u64_at(strs, 0x20) as usize).min(1 << 16))?;
    Some((0..shnum).map(|i| {
        let s = sec(i);
        let at = u32::from_le_bytes(s[0..4].try_into().unwrap()) as usize;
        let name = names.get(at..).and_then(|n| n.split(|b| *b == 0).next()).unwrap_or_default();
        (String::from_utf8_lossy(name).into_owned(), u64_at(s, 0x18), u64_at(s, 0x20))
    }).collect())
}

/// A section's bytes, to its first NUL (at most `most`).
fn section_of(file: &Path, sections: &[(String, u64, u64)], name: &str, most: u64) -> Option<Vec<u8>> {
    use std::io::{Seek, SeekFrom};
    let (_, off, len) = sections.iter().find(|s| s.0 == name)?;
    let mut f = fs::File::open(file).ok()?;
    f.seek(SeekFrom::Start(*off)).ok()?;
    let mut b = Vec::new();
    f.take((*len).min(most)).read_to_end(&mut b).ok()?;
    let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    b.truncate(end);
    Some(b).filter(|b| !b.is_empty())
}

/// An AppImage's update information: its runtime's `.upd_info` section.
pub fn appimage_update_info(file: &Path) -> Option<String> {
    let secs = appimage_sections(file)?;
    let data = section_of(file, &secs, ".upd_info", 4096)?;
    Some(String::from_utf8_lossy(&data).trim().to_string()).filter(|t| t.contains('|'))
}

/// A signed AppImage (appimagetool --sign): its signature (`.sha256_sig`,
/// a detached one of the file's SHA-256 in hex, those two sections as
/// zeros), the key it carries (`.sig_key`), and that SHA-256.
fn appimage_signature(file: &Path) -> Option<(Vec<u8>, Vec<u8>, String)> {
    let secs = appimage_sections(file)?;
    let sig = section_of(file, &secs, ".sha256_sig", 1 << 16)?;
    let key = section_of(file, &secs, ".sig_key", 1 << 20).unwrap_or_default();
    let blank: Vec<(u64, u64)> = secs.iter().filter(|s| s.0 == ".sha256_sig" || s.0 == ".sig_key").map(|s| (s.1, s.1 + s.2)).collect();
    let mut f = fs::File::open(file).ok()?;
    let (mut h, mut at) = (sha2::Sha256::new(), 0u64);
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        for (s, e) in &blank {
            let (from, to) = ((*s).max(at), (*e).min(at + n as u64));
            if from < to {
                buf[(from - at) as usize..(to - at) as usize].fill(0);
            }
        }
        h.update(&buf[..n]);
        at += n as u64;
    }
    Some((sig, key, h.finalize().iter().map(|b| format!("{b:02x}")).collect()))
}

/// The key a signed AppImage was signed with, if its signature is good: pinned at install.
fn appimage_key(file: &Path) -> Option<(Vec<String>, Vec<u8>)> {
    let (sig, key, digest) = appimage_signature(file)?;
    let ring = openpgp::dearmor(&key).ok()?.concat();
    let keys = openpgp::keys(&ring).ok()?;
    openpgp::verify_detached(&keys, digest.as_bytes(), &sig).ok()?;
    Some((openpgp::fingerprints(&ring).ok()?, ring))
}

/// Where an installed AppImage's own file is kept, to update from by
/// what changed only (zsync): in its folder, a link to the download where
/// it can be (no room taken), else a copy.
fn kept_appimage(app_dir: &Path) -> PathBuf {
    app_dir.join("update-from.AppImage")
}

fn sha1_of(file: &Path) -> Result<String> {
    let mut h = openpgp::Sha1::default();
    let mut f = fs::File::open(file)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(openpgp::hex(&h.finish()).to_lowercase())
}

/// What an app's updates are checked by, as it's installed (finish()):
/// the keys its package came with, pinned; its AppImage's update
/// information; where it was downloaded from.
pub(super) fn at_install(file: &Path, format: Format, meta: &Meta, scan: &Scan, files: &Path, source: Option<&Source>) -> Option<Trust> {
    let t = trust_of(file, format, meta, scan, source, &mut |p| fs::read(files.join(p)).ok().filter(|d| d.len() < 1 << 20), true);
    // An AppImage that updates by zsync: its file kept, for its next update to start from.
    if t.as_ref().and_then(|t| t.appimage.as_deref()).is_some_and(|i| i.starts_with("zsync|") || i.starts_with("gh-releases-zsync|")) {
        if let Some(dir) = files.parent() {
            let keep = kept_appimage(dir);
            let _ = fs::remove_file(&keep);
            if fs::hard_link(file, &keep).is_err() {
                let _ = fs::copy(file, &keep);
            }
        }
    }
    t
}

fn trust_of(file: &Path, format: Format, meta: &Meta, scan: &Scan, source: Option<&Source>, read: &mut dyn FnMut(&str) -> Option<Vec<u8>>, fetch: bool) -> Option<Trust> {
    let mut t = Trust { origin: origin_url(file), ..Default::default() };
    if source.is_some() {
        let texts: Vec<&str> = meta.scripts.iter().chain(&scan.texts).map(|(_, t)| t.as_str()).collect();
        let paths: Vec<&str> = scan.paths.iter().map(|(p, _)| p.as_str()).collect();
        let (ring, from, url) = find_keys(&texts, &paths, read, fetch);
        if !ring.is_empty() {
            t.keys = openpgp::fingerprints(&ring).unwrap_or_default();
            t.keyring = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &ring);
            t.key_from = from;
        }
        t.key_url = url;
    }
    if format == Format::AppImage {
        t.appimage = appimage_update_info(file);
        if t.appimage.is_some() {
            t.sha1 = sha1_of(file).ok();
        }
        // Signed by its maker: that key pinned, as a repository's is.
        if let Some((fps, ring)) = appimage_key(file) {
            t.keys = fps;
            t.keyring = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &ring);
            t.key_from = "in the AppImage".into();
        }
    }
    let homepage = meta.homepage.iter().cloned().chain(scan.texts.iter().filter(|(p, _)| p.ends_with(".xml")).flat_map(|(_, x)| x.split("<url").skip(1).filter_map(|u| u.split_once('>').and_then(|(_, r)| r.split('<').next()).map(str::to_string)).collect::<Vec<_>>()));
    t.github = t.origin.as_deref().and_then(github_of).or_else(|| homepage.into_iter().find_map(|h| github_of(&h)));
    (t != Trust::default()).then_some(t)
}

// --------------------------------------------------------------- the network

/// The network couldn't be reached: said nowhere, tried again later.
#[derive(Debug)]
struct Offline(String);

impl std::fmt::Display for Offline {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "couldn't reach {}", self.0)
    }
}
impl std::error::Error for Offline {}

/// Refused: what's there isn't shown to be its maker's.
#[derive(Debug)]
struct Refused {
    why: String,
    /// The repository signs with another key now.
    key_changed: bool,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.why)
    }
}
impl std::error::Error for Refused {}

fn refused(why: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Refused { why: why.into(), key_changed: false })
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(120)).user_agent(concat!("nami/", env!("CARGO_PKG_VERSION"))).build()
}

fn host(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).split('/').next().unwrap_or_default().to_string()
}

/// Whether what's fetched from `url` can be trusted as it comes: HTTPS (or,
/// for tests only, a host NAMI_APP_UPDATES_PLAIN_HTTP names, host:port).
fn fetched_safely(url: &str) -> bool {
    url.starts_with("https://") || (url.starts_with("http://") && std::env::var("NAMI_APP_UPDATES_PLAIN_HTTP").is_ok_and(|v| v.split(',').any(|h| !h.is_empty() && h == host(url))))
}

/// Bytes [s, e) of `url`: a range request (zsync::NoRanges if the server
/// sends whole files only).
fn range_of(agent: &ureq::Agent, url: &str, s: u64, e: u64) -> Result<Vec<u8>> {
    let r = match agent.get(url).set("Range", &format!("bytes={s}-{}", e - 1)).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => bail!("{} said {code} for {url}", host(url)),
        Err(ureq::Error::Transport(t)) => return Err(anyhow::Error::new(Offline(format!("{} ({t})", host(url))))),
    };
    if r.status() != 206 {
        return Err(anyhow::Error::new(crate::zsync::NoRanges));
    }
    let mut out = Vec::with_capacity((e - s) as usize);
    r.into_reader().take(e - s + 1).read_to_end(&mut out).map_err(|err| anyhow::Error::new(Offline(format!("{} ({err})", host(url)))))?;
    Ok(out)
}

/// GET, its response (None: not there, 404).
fn response(url: &str) -> Result<Option<ureq::Response>> {
    let mut req = agent().get(url);
    if url.starts_with("https://api.github.com/") {
        req = req.set("Accept", "application/vnd.github+json");
    }
    match req.call() {
        Ok(r) => Ok(Some(r)),
        Err(ureq::Error::Status(404 | 410, _)) => Ok(None),
        Err(ureq::Error::Status(code, _)) => bail!("{} said {code} for {url}", host(url)),
        Err(ureq::Error::Transport(t)) => Err(anyhow::Error::new(Offline(format!("{} ({t})", host(url))))),
    }
}

fn get_opt(url: &str, most: u64) -> Result<Option<Vec<u8>>> {
    let Some(r) = response(url)? else { return Ok(None) };
    let mut out = Vec::new();
    r.into_reader().take(most + 1).read_to_end(&mut out).map_err(|e| anyhow::Error::new(Offline(format!("{} ({e})", host(url)))))?;
    ensure!(out.len() as u64 <= most, "{url} is bigger than it should be");
    Ok(Some(out))
}

fn get(url: &str, most: u64) -> Result<Vec<u8>> {
    get_opt(url, most)?.ok_or_else(|| anyhow!("{url} isn't there"))
}

/// A download into `to`, its SHA-256 and SHA-1 worked out as it comes.
fn download(url: &str, to: &Path, most: u64) -> Result<(String, String, u64)> {
    let r = response(url)?.ok_or_else(|| anyhow!("{url} isn't there"))?;
    fs::create_dir_all(to.parent().unwrap())?;
    let part = to.with_extension("part");
    let mut out = fs::File::create(&part)?;
    let mut reader = r.into_reader().take(most + 1);
    let (mut h256, mut h1, mut n) = (sha2::Sha256::new(), openpgp::Sha1::default(), 0u64);
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let k = reader.read(&mut buf).map_err(|e| anyhow::Error::new(Offline(format!("{} ({e})", host(url)))))?;
        if k == 0 {
            break;
        }
        n += k as u64;
        if n > most {
            drop(out);
            let _ = fs::remove_file(&part);
            return Err(refused(format!("The download from {} is bigger than its maker's list says: it was changed on the way, so it wasn't installed.", host(url))));
        }
        h256.update(&buf[..k]);
        h1.update(&buf[..k]);
        out.write_all(&buf[..k])?;
    }
    out.sync_all()?;
    fs::rename(&part, to)?;
    Ok((h256.finalize().iter().map(|b| format!("{b:02x}")).collect(), openpgp::hex(&h1.finish()).to_lowercase(), n))
}

fn hex256(d: &[u8]) -> String {
    sha2::Sha256::digest(d).iter().map(|b| format!("{b:02x}")).collect()
}

fn hex512(d: &[u8]) -> String {
    sha2::Sha512::digest(d).iter().map(|b| format!("{b:02x}")).collect()
}

/// Compressed data (gzip, xz, zstd), or not.
fn unpacked(d: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    uncompressed(BufReader::new(d))?.take(1 << 30).read_to_end(&mut out)?;
    Ok(out)
}

// ---------------------------------------------------------------- the checks

/// What's at an app's maker's.
#[derive(Debug)]
enum Found {
    /// Nothing newer: the newest version there.
    Current(String),
    /// A newer version, and (when it was to be fetched) its download, checked.
    Newer { version: String, file: Option<PathBuf> },
    /// Nothing nami can check: why.
    Manual(String),
}

fn arch_names() -> (&'static str, &'static str) {
    match std::env::consts::ARCH {
        "aarch64" => ("arm64", "aarch64"),
        _ => ("amd64", "x86_64"),
    }
}

fn downloads_of(id: &str) -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".cache")).join("nami/app-updates").join(id)
}

/// A name for a file from a URL, safe as a file name.
fn file_name_of(url: &str) -> String {
    let n = url.split(['?', '#']).next().unwrap_or(url).rsplit('/').next().unwrap_or("download");
    let n: String = n.chars().map(|c| if c.is_ascii_alphanumeric() || "._-+~".contains(c) { c } else { '_' }).collect();
    if n.is_empty() || n.starts_with('.') { format!("download{n}") } else { n }
}

/// The pinned keys, as keys: those fingerprints, and their subkeys.
fn keyring(trust: &Trust) -> Result<Vec<openpgp::Key>> {
    let ring = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &trust.keyring).unwrap_or_default();
    let keys = openpgp::keys(&ring).unwrap_or_default();
    Ok(keys.into_iter().filter(|k| trust.keys.contains(&k.fingerprint) || k.primary.as_ref().is_some_and(|p| trust.keys.contains(p))).collect())
}

/// What its maker's key says of itself now, fetched again from where its
/// package said (`more`: or as an update carries it): only what the pinned
/// keys signed is taken (a renewal, a revocation, a new subkey), and kept
/// when it says more than what's known.
fn refresh(name: &str, trust: &mut Trust, more: Option<Vec<u8>>) {
    let more = match more {
        Some(m) => m,
        None => {
            let Some(url) = trust.key_url.clone() else { return };
            match get(&url, 1 << 20) {
                Ok(d) => d,
                Err(_) => return,
            }
        }
    };
    let Ok(blocks) = openpgp::dearmor(&more) else { return };
    let before = keyring(trust).map(|k| openpgp::state(&k)).unwrap_or_default();
    let mut t2 = trust.clone();
    let mut ring = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &trust.keyring).unwrap_or_default();
    ring.extend(blocks.concat());
    t2.keyring = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &ring);
    let after = keyring(&t2).map(|k| openpgp::state(&k)).unwrap_or_default();
    if after != before {
        let said: Vec<String> = after.iter().filter(|a| !before.contains(a)).map(|(fp, exp, rev)| match (exp, rev) {
            (_, true) => format!("{fp} revoked"),
            (Some(t), _) => format!("{fp} good until {}", openpgp::date(*t)),
            (None, _) => format!("{fp} with no end"),
        }).collect();
        log(&format!("{name}: its maker's key says more now: {}", said.join(", ")));
        *trust = t2;
    }
}

/// A key's signature that can't be trusted now, in plain words.
fn unusable_words(name: &str, n: &openpgp::NotUsable) -> String {
    let key = if n.subkey { "the subkey" } else { "the key" };
    match (&n.revoked, n.expired) {
        (Some(_), _) => format!("{name}'s maker has revoked {key} they sign its updates with ({}), so the update wasn't installed. If you still trust {name}, download it from its maker again and open it.", n.fingerprint),
        (None, Some(t)) => format!("{name}'s maker signs its updates with {key} that expired on {} ({}), so the update wasn't installed. Updates come again once they renew it.", openpgp::date(t), n.fingerprint),
        _ => format!("{name}'s maker's key ({}) can't be used, so the update wasn't installed.", n.fingerprint),
    }
}

/// A signed index checked: with the pinned keys, as their maker's word on
/// them is now (fetched again: renewed, revoked, a new subkey). Refused
/// otherwise, in plain words.
fn signed<T>(name: &str, place: &str, trust: &mut Trust, check: &dyn Fn(&[openpgp::Key]) -> Result<T>) -> Result<T> {
    if keyring(trust)?.is_empty() {
        return Err(anyhow!("manual: {name} didn't come with the key its maker signs its updates with"));
    }
    refresh(name, trust, None);
    let first = match check(&keyring(trust)?) {
        Ok(v) => return Ok(v),
        Err(e) => e,
    };
    if let Some(n) = first.downcast_ref::<openpgp::NotUsable>() {
        return Err(refused(unusable_words(name, n)));
    }
    let Some(unknown) = first.downcast_ref::<openpgp::UnknownKey>().map(|u| u.0.clone()) else {
        let why = if first.to_string().contains("changed after") { format!("The list of versions at {place} was changed after its maker signed it, so the update wasn't installed.") } else { format!("The list of versions at {place} isn't signed as it should be ({first:#}), so the update wasn't installed.") };
        return Err(refused(why));
    };
    Err(anyhow::Error::new(Refused {
        why: format!("{place} now signs its updates with a different key ({unknown}) than the one {name} came with, so nothing was installed. If you trust it, download {name} from its maker again and open it."),
        key_changed: true,
    }))
}

/// An apt repository: InRelease (or Release, Release.gpg), Packages, the .deb.
fn apt(rec: &Record, s: &Source, trust: &mut Trust, fetch: bool) -> Result<Found> {
    let base = s.url.trim_end_matches('/');
    let flat = s.suite.ends_with('/') || s.suite.is_empty();
    let dist = if flat { format!("{base}/{}", s.suite.trim_start_matches("./").trim_start_matches('/')) } else { format!("{base}/dists/{}/", s.suite) };
    let dist = if dist.ends_with('/') { dist } else { format!("{dist}/") };
    let place = host(base);
    let (release, _by) = match get_opt(&format!("{dist}InRelease"), 32 << 20)? {
        Some(d) => signed(&rec.name, &place, trust, &|k| openpgp::verify_clearsigned(k, &d))?,
        None => {
            let r = get(&format!("{dist}Release"), 32 << 20)?;
            let g = get_opt(&format!("{dist}Release.gpg"), 1 << 20)?.ok_or_else(|| refused(format!("The list of versions at {place} isn't signed, so the update wasn't installed.")))?;
            let by = signed(&rec.name, &place, trust, &|k| openpgp::verify_detached(k, &r, &g))?;
            (String::from_utf8_lossy(&r).into_owned(), by)
        }
    };
    let fields = deb::parse_control(&release);
    if !flat {
        let named = [fields.get("Suite"), fields.get("Codename")];
        if !named.iter().flatten().any(|v| *v == &s.suite) {
            return Err(refused(format!("The signed list at {place} is for another release ({}), not {}, so the update wasn't installed.", fields.get("Suite").or(fields.get("Codename")).cloned().unwrap_or_default(), s.suite)));
        }
    }
    let sums: BTreeMap<String, (String, u64)> = fields.get("SHA256").map(|v| v.lines().filter_map(|l| {
        let w: Vec<&str> = l.split_whitespace().collect();
        (w.len() == 3).then(|| (w[2].to_string(), (w[0].to_lowercase(), w[1].parse().unwrap_or(0))))
    }).collect()).unwrap_or_default();
    let (arch, _) = arch_names();
    let comps: Vec<&str> = if flat { vec![""] } else { s.components.split_whitespace().collect() };
    let comps = if comps.is_empty() { vec!["main"] } else { comps };
    let mut best: Option<BTreeMap<String, String>> = None;
    let mut listed = false;
    for comp in comps {
        let prefix = if flat { String::new() } else { format!("{comp}/binary-{arch}/") };
        let Some((path, (sum, size))) = ["Packages.xz", "Packages.gz", "Packages"].iter().find_map(|n| sums.get_key_value(&format!("{prefix}{n}"))) else { continue };
        listed = true;
        let data = get(&format!("{dist}{path}"), 512 << 20)?;
        if hex256(&data) != *sum || (*size > 0 && data.len() as u64 != *size) {
            return Err(refused(format!("The list of packages at {place} doesn't match its signed index: it was changed on the way, so the update wasn't installed.")));
        }
        let text = String::from_utf8_lossy(&unpacked(&data)?).into_owned();
        for stanza in text.split("\n\n") {
            let c = deb::parse_control(stanza);
            if c.get("Package") != Some(&rec.package) || !c.get("Architecture").is_some_and(|a| a == arch || a == "all") {
                continue;
            }
            let newer = |b: &BTreeMap<String, String>| crate::recipe::version_cmp(c.get("Version").map(String::as_str).unwrap_or(""), b.get("Version").map(String::as_str).unwrap_or("")).is_gt();
            if best.as_ref().is_none_or(newer) {
                best = Some(c);
            }
        }
    }
    if !listed {
        return Err(refused(format!("The signed index at {place} doesn't list its packages' SHA-256, so the update wasn't installed.")));
    }
    let Some(best) = best else { return Ok(Found::Manual(format!("{} isn't in the list at {place} any more.", rec.package))) };
    let version = best.get("Version").cloned().unwrap_or_default();
    let have = if rec.full_version.is_empty() { &rec.version } else { &rec.full_version };
    if !crate::recipe::version_cmp(&version, have).is_gt() {
        return Ok(Found::Current(shown_version(&version)));
    }
    if !fetch {
        return Ok(Found::Newer { version: shown_version(&version), file: None });
    }
    let (Some(file), Some(sum)) = (best.get("Filename"), best.get("SHA256")) else {
        return Err(refused(format!("{place}'s list doesn't give the SHA-256 of {} {}, so it wasn't installed.", rec.name, shown_version(&version))));
    };
    let size: u64 = best.get("Size").and_then(|s| s.parse().ok()).unwrap_or(0);
    let url = if flat { format!("{dist}{file}") } else { format!("{base}/{file}") };
    let to = downloads_of(&rec.id).join(file_name_of(file));
    let (got, _, n) = download(&url, &to, if size > 0 { size } else { 4 << 30 })?;
    if got != sum.to_lowercase() || (size > 0 && n != size) {
        let _ = fs::remove_file(&to);
        return Err(refused(format!("The download of {} {} doesn't match its signed list: it was changed on the way, so it wasn't installed.", rec.name, shown_version(&version))));
    }
    let c = deb::control(&fs::read(&to)?)?;
    if c.get("Package") != Some(&rec.package) || c.get("Version") != Some(&version) {
        let _ = fs::remove_file(&to);
        return Err(refused(format!("The download at {place} isn't the {} {} its list says, so it wasn't installed.", rec.name, shown_version(&version))));
    }
    Ok(Found::Newer { version: shown_version(&version), file: Some(to) })
}

/// An XML element's text, from `at` on: `<name>X</name>`.
fn xml_text<'a>(x: &'a str, tag: &str) -> Option<&'a str> {
    let open = x.find(&format!("<{tag}"))?;
    let start = open + x[open..].find('>')? + 1;
    let end = start + x[start..].find(&format!("</{tag}>"))?;
    Some(x[start..end].trim())
}

/// An attribute of the first `<tag ...>` in `x`.
fn xml_attr<'a>(x: &'a str, tag: &str, attr: &str) -> Option<&'a str> {
    let open = x.find(&format!("<{tag} ")).or_else(|| x.find(&format!("<{tag}\n")))?;
    let end = open + x[open..].find('>')?;
    let el = &x[open..end];
    let at = el.find(&format!(" {attr}=")).or_else(|| el.find(&format!("\n{attr}=")))? + attr.len() + 2;
    let q = el[at..].chars().next()?;
    let rest = &el[at + 1..];
    Some(&rest[..rest.find(q)?])
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// A checksum as yum gives it: (type, value): sha256 or sha512 checked; others refused.
fn sum_matches(kind: &str, want: &str, data: &[u8]) -> bool {
    match kind {
        "sha256" => hex256(data) == want.to_lowercase(),
        "sha512" => hex512(data) == want.to_lowercase(),
        _ => false,
    }
}

/// A yum repository: repomd.xml (repomd.xml.asc), primary.xml, the .rpm.
fn yum(rec: &Record, s: &Source, trust: &mut Trust, fetch: bool) -> Result<Found> {
    let base = s.url.trim_end_matches('/');
    let place = host(base);
    let repomd = get(&format!("{base}/repodata/repomd.xml"), 16 << 20)?;
    let Some(asc) = get_opt(&format!("{base}/repodata/repomd.xml.asc"), 1 << 20)? else {
        return Ok(Found::Manual(format!("Its maker doesn't sign the list of versions at {place}, so nami can't check an update is theirs: check manually.")));
    };
    signed(&rec.name, &place, trust, &|k| openpgp::verify_detached(k, &repomd, &asc))?;
    let md = String::from_utf8_lossy(&repomd).into_owned();
    let primary = md.split("<data ").skip(1).find(|d| d.starts_with("type=\"primary\"")).ok_or_else(|| anyhow!("{place}'s list of versions has no primary list"))?;
    let (kind, sum) = (xml_attr(primary, "checksum", "type").unwrap_or(""), xml_text(primary, "checksum").unwrap_or(""));
    let href = unescape(xml_attr(primary, "location", "href").ok_or_else(|| anyhow!("{place}'s list of versions doesn't say where its primary list is"))?);
    ensure!(!href.contains(".."), "a strange path in {place}'s list");
    let data = get(&format!("{base}/{href}"), 1 << 30)?;
    if !sum_matches(kind, sum, &data) {
        return Err(refused(format!("The list of packages at {place} doesn't match its signed index: it was changed on the way, so the update wasn't installed.")));
    }
    let xml = String::from_utf8_lossy(&unpacked(&data)?).into_owned();
    let (_, arch) = arch_names();
    struct Pkg { epoch: String, ver: String, rel: String, kind: String, sum: String, href: String, size: u64 }
    let mut best: Option<Pkg> = None;
    for p in xml.split("<package ").skip(1) {
        let p = p.split("</package>").next().unwrap_or(p);
        if xml_text(p, "name") != Some(rec.package.as_str()) || !xml_text(p, "arch").is_some_and(|a| a == arch || a == "noarch") {
            continue;
        }
        let pkg = Pkg {
            epoch: xml_attr(p, "version", "epoch").unwrap_or("0").to_string(),
            ver: xml_attr(p, "version", "ver").unwrap_or("").to_string(),
            rel: xml_attr(p, "version", "rel").unwrap_or("").to_string(),
            kind: xml_attr(p, "checksum", "type").unwrap_or("").to_string(),
            sum: xml_text(p, "checksum").unwrap_or("").to_string(),
            href: unescape(xml_attr(p, "location", "href").unwrap_or("")),
            size: xml_attr(p, "size", "package").and_then(|s| s.parse().ok()).unwrap_or(0),
        };
        let full = |p: &Pkg| format!("{}:{}-{}", p.epoch, p.ver, p.rel);
        if best.as_ref().is_none_or(|b| crate::recipe::version_cmp(&full(&pkg), &full(b)).is_gt()) {
            best = Some(pkg);
        }
    }
    let Some(best) = best else { return Ok(Found::Manual(format!("{} isn't in the list at {place} any more.", rec.package))) };
    let have = if rec.full_version.is_empty() { rec.version.clone() } else { rec.full_version.clone() };
    let theirs = format!("{}-{}", best.ver, best.rel);
    if !crate::recipe::version_cmp(&format!("{}:{theirs}", best.epoch), &format!("0:{have}")).is_gt() {
        return Ok(Found::Current(best.ver));
    }
    if !fetch {
        return Ok(Found::Newer { version: best.ver, file: None });
    }
    ensure!(!best.href.contains("..") && !best.href.starts_with('/'), "a strange path in {place}'s list");
    if !["sha256", "sha512"].contains(&best.kind.as_str()) {
        return Err(refused(format!("{place}'s list doesn't give a safe checksum of {} {}, so it wasn't installed.", rec.name, best.ver)));
    }
    let to = downloads_of(&rec.id).join(file_name_of(&best.href));
    download(&format!("{base}/{}", best.href), &to, if best.size > 0 { best.size } else { 4 << 30 })?;
    let d = fs::read(&to)?;
    if !sum_matches(&best.kind, &best.sum, &d) || (best.size > 0 && d.len() as u64 != best.size) {
        let _ = fs::remove_file(&to);
        return Err(refused(format!("The download of {} {} doesn't match its signed list: it was changed on the way, so it wasn't installed.", rec.name, best.ver)));
    }
    let (h, _) = rpm::read_header(&d)?;
    if h.string(rpm::NAME) != Some(rec.package.as_str()) || h.string(rpm::VERSION) != Some(best.ver.as_str()) || h.string(rpm::RELEASE) != Some(best.rel.as_str()) {
        let _ = fs::remove_file(&to);
        return Err(refused(format!("The download at {place} isn't the {} {} its list says, so it wasn't installed.", rec.name, best.ver)));
    }
    Ok(Found::Newer { version: best.ver, file: Some(to) })
}

/// A GitHub release, as its API says it: (tag, assets: name, URL, SHA-256, size).
struct Release {
    tag: String,
    assets: Vec<(String, String, Option<String>, u64)>,
}

fn releases(repo: &str) -> Result<Vec<Release>> {
    let api = std::env::var("NAMI_GITHUB_API").unwrap_or_else(|_| "https://api.github.com".into());
    let data = get(&format!("{api}/repos/{repo}/releases?per_page=30"), 16 << 20)?;
    let v: serde_json::Value = serde_json::from_slice(&data)?;
    Ok(v.as_array().into_iter().flatten().filter(|r| !r["draft"].as_bool().unwrap_or(false) && !r["prerelease"].as_bool().unwrap_or(false)).map(|r| Release {
        tag: r["tag_name"].as_str().unwrap_or("").to_string(),
        assets: r["assets"].as_array().into_iter().flatten().map(|a| (
            a["name"].as_str().unwrap_or("").to_string(),
            a["browser_download_url"].as_str().unwrap_or("").to_string(),
            a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_lowercase),
            a["size"].as_u64().unwrap_or(0),
        )).collect(),
    }).collect())
}

fn tag_version(tag: &str) -> &str {
    let t = tag.rsplit('/').next().unwrap_or(tag);
    let t = t.trim_start_matches(|c: char| !c.is_ascii_digit());
    if t.is_empty() { tag } else { t }
}

/// A newer release of the same GitHub repository: the same file, but for its version.
fn github(rec: &Record, repo: &str, fetch: bool) -> Result<Found> {
    let manual = |why: &str| Ok(Found::Manual(format!("{why}: check for a newer {} at github.com/{repo} yourself.", rec.name)));
    let from = Path::new(&rec.from).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let all = releases(repo)?;
    let Some(newest) = all.first() else { return manual("It has no releases there") };
    // The one installed is that repository's own: its file, as GitHub has it.
    let ours = all.iter().flat_map(|r| &r.assets).any(|(n, _, d, _)| *n == from && d.as_deref() == Some(rec.sha256.as_str()));
    if !ours {
        return manual("nami can't tell the one installed came from there");
    }
    let (have, theirs) = (rec.version.as_str(), tag_version(&newest.tag));
    if have.is_empty() || !from.contains(have) {
        return manual("Its file doesn't say its version");
    }
    if !crate::recipe::version_cmp(theirs, have).is_gt() {
        return Ok(Found::Current(theirs.to_string()));
    }
    let want = from.replace(have, theirs);
    let Some((name, url, digest, size)) = newest.assets.iter().find(|a| a.0 == want).cloned() else { return manual(&format!("Its release {theirs} has no {want}")) };
    let Some(digest) = digest else { return manual(&format!("GitHub doesn't give the checksum of {name}")) };
    if !url.starts_with(&format!("https://github.com/{repo}/releases/download/")) {
        return manual("Its new release is somewhere else");
    }
    if !fetch {
        return Ok(Found::Newer { version: theirs.to_string(), file: None });
    }
    let to = downloads_of(&rec.id).join(file_name_of(&name));
    let (got, _, n) = download(&url, &to, if size > 0 { size } else { 4 << 30 })?;
    if got != digest || (size > 0 && n != size) {
        let _ = fs::remove_file(&to);
        return Err(refused(format!("The download of {} {theirs} doesn't match the checksum GitHub gives for it, so it wasn't installed.", rec.name)));
    }
    Ok(Found::Newer { version: theirs.to_string(), file: Some(to) })
}

/// `*` matches anything (AppImage's update information's file patterns).
fn glob(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let mut rest = name;
    for (i, p) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(p) else { return false };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.ends_with(p);
        } else {
            let Some(at) = rest.find(p) else { return false };
            rest = &rest[at + p.len()..];
        }
    }
    true
}

/// A .zsync file's header: Filename, URL, SHA-1, Length.
fn zsync_header(data: &[u8]) -> BTreeMap<String, String> {
    let end = data.windows(2).position(|w| w == b"\n\n").unwrap_or(data.len().min(8192));
    deb::parse_control(&String::from_utf8_lossy(&data[..end]))
}

/// The file an installed AppImage's update starts from: the one kept in
/// its folder, else the download it was installed from, if it's still that.
fn appimage_seed(rec: &Record, trust: &Trust) -> Option<PathBuf> {
    let kept = kept_appimage(&Places::here().apps().join(&rec.id));
    if kept.is_file() {
        return Some(kept);
    }
    let from = PathBuf::from(&rec.from);
    (from.is_file() && trust.sha1.is_some() && sha1_of(&from).ok() == trust.sha1).then_some(from)
}

/// A new AppImage signed as the one installed was: by the key pinned then
/// (as it says of itself now: the copy the new one carries counts as far
/// as that key signed it). Refused otherwise.
fn appimage_signed(rec: &Record, version: &str, trust: &mut Trust, file: &Path) -> Result<()> {
    if trust.keys.is_empty() {
        return Ok(());
    }
    let name = &rec.name;
    let Some((sig, key, digest)) = appimage_signature(file) else {
        return Err(refused(format!("{name} {version} isn't signed by its maker, as the one installed was, so it wasn't installed.")));
    };
    refresh(name, trust, Some(key.clone()));
    let carried = openpgp::dearmor(&key).ok().and_then(|b| openpgp::fingerprints(&b.concat()).ok()).unwrap_or_default();
    let other_key = |fp: &str| anyhow::Error::new(Refused {
        why: format!("{name} {version} is signed with a different key ({fp}) than the one {name} came with, so it wasn't installed. If you trust it, download {name} from its maker again and open it."),
        key_changed: true,
    });
    if !carried.iter().any(|f| trust.keys.contains(f)) {
        return Err(other_key(carried.first().map(String::as_str).unwrap_or("one it doesn't carry")));
    }
    match openpgp::verify_detached(&keyring(trust)?, digest.as_bytes(), &sig) {
        Ok(_) => Ok(()),
        Err(e) => {
            if let Some(n) = e.downcast_ref::<openpgp::NotUsable>() {
                return Err(refused(unusable_words(name, n)));
            }
            if let Some(u) = e.downcast_ref::<openpgp::UnknownKey>() {
                return Err(other_key(&u.0));
            }
            Err(refused(format!("{name} {version}'s signature doesn't match it: it was changed after its maker signed it, so it wasn't installed.")))
        }
    }
}

/// An AppImage's update information: zsync's, or a GitHub release's. With
/// the one installed to start from, only the blocks that changed are
/// fetched (each checked, then the whole file); else all of it.
fn appimage(rec: &Record, info: &str, trust: &mut Trust, fetch: bool) -> Result<Found> {
    let parts: Vec<&str> = info.split('|').collect();
    let mut digest: Option<String> = None;
    let zsync_url = match parts.as_slice() {
        ["zsync", url] => url.to_string(),
        ["gh-releases-zsync", owner, repo, tag, pattern] => {
            let repo = format!("{owner}/{repo}");
            let all = releases(&repo)?;
            let rel = if *tag == "latest" || tag.starts_with("latest") { all.first() } else { all.iter().find(|r| r.tag == *tag) };
            let Some(rel) = rel else { return Ok(Found::Manual(format!("Its releases at github.com/{repo} aren't there any more: check manually."))) };
            let Some((name, url, _, _)) = rel.assets.iter().find(|a| glob(pattern, &a.0)) else { return Ok(Found::Manual(format!("Its newest release at github.com/{repo} has no update for it: check manually."))) };
            digest = rel.assets.iter().find(|a| Some(a.0.as_str()) == name.strip_suffix(".zsync")).and_then(|a| a.2.clone());
            url.clone()
        }
        _ => return Ok(Found::Manual(format!("It updates from a place nami can't follow ({}): check manually.", parts.first().unwrap_or(&"")))),
    };
    if !fetched_safely(&zsync_url) {
        return Ok(Found::Manual("Its updates aren't fetched safely (not over HTTPS): check manually.".into()));
    }
    // (Its header tells whether there's a newer one; to fetch it, all of it.)
    let control = if fetch {
        get(&zsync_url, 64 << 20)?
    } else {
        let r = response(&zsync_url)?.ok_or_else(|| anyhow!("{zsync_url} isn't there"))?;
        let mut head = Vec::new();
        r.into_reader().take(16 << 10).read_to_end(&mut head)?;
        head
    };
    let h = zsync_header(&control);
    let (Some(sha1), Some(target)) = (h.get("SHA-1").map(|s| s.to_lowercase()), h.get("URL")) else {
        return Ok(Found::Manual("Its update information is broken: check manually.".into()));
    };
    let length: u64 = h.get("Length").and_then(|l| l.parse().ok()).unwrap_or(0);
    let name = h.get("Filename").cloned().unwrap_or_else(|| file_name_of(target));
    let (_, v) = name_and_version(Path::new(&name));
    let version = if v.is_empty() { "newer".to_string() } else { v };
    if trust.sha1.as_deref() == Some(sha1.as_str()) {
        return Ok(Found::Current(rec.version.clone()));
    }
    if !fetch {
        return Ok(Found::Newer { version, file: None });
    }
    let url = if target.contains("://") {
        target.clone()
    } else {
        format!("{}/{}", zsync_url.rsplit_once('/').map(|(d, _)| d).unwrap_or(&zsync_url), target.trim_start_matches("./"))
    };
    if !fetched_safely(&url) {
        return Ok(Found::Manual("Its updates aren't fetched safely (not over HTTPS): check manually.".into()));
    }
    let changed = || refused(format!("The download of {} {version} doesn't match its update information: it was changed on the way, so it wasn't installed.", rec.name));
    let to = downloads_of(&rec.id).join(file_name_of(&name));
    fs::create_dir_all(to.parent().unwrap())?;
    // Only what changed, from the one installed.
    let mut done = false;
    if let (Some(seed), Ok(c)) = (appimage_seed(rec, trust), crate::zsync::parse(&control)) {
        if c.sha1 == sha1 && (length == 0 || c.length == length) {
            let agent = agent();
            match crate::zsync::make(&c, &seed, &to, &mut |s, e| range_of(&agent, &url, s, e)) {
                Ok(m) => {
                    log(&format!("{}: {version} made from the one installed and what changed: {} of {} bytes fetched, in {} requests", rec.id, m.fetched, c.length, m.requests));
                    say(&format!("Fetched only what changed: {} of {}.", size_words(m.fetched), size_words(c.length)));
                    done = true;
                }
                Err(e) if e.downcast_ref::<crate::zsync::BadBlock>().is_some() || e.downcast_ref::<crate::zsync::BadSum>().is_some() => {
                    log(&format!("{}: {e:#}", rec.id));
                    return Err(changed());
                }
                Err(e) if e.downcast_ref::<Offline>().is_some() => return Err(e),
                // (No range requests there, a .zsync nami can't follow: all of it, as before.)
                Err(e) => log(&format!("{}: fetching all of {version} ({e:#})", rec.id)),
            }
        }
    }
    if !done {
        let (_, got1, n) = download(&url, &to, if length > 0 { length } else { 4 << 30 })?;
        if got1 != sha1 || (length > 0 && n != length) {
            let _ = fs::remove_file(&to);
            return Err(changed());
        }
    }
    if digest.as_ref().is_some_and(|d| sha256_of(&to).ok().as_ref() != Some(d)) {
        let _ = fs::remove_file(&to);
        return Err(changed());
    }
    fs::set_permissions(&to, fs::Permissions::from_mode(0o755))?;
    if appimage_update_info(&to).as_deref() != Some(info) {
        let _ = fs::remove_file(&to);
        return Ok(Found::Manual(format!("Its new version updates from somewhere else: check {} manually.", rec.name)));
    }
    if let Err(e) = appimage_signed(rec, &version, trust, &to) {
        let _ = fs::remove_file(&to);
        return Err(e);
    }
    Ok(Found::Newer { version, file: Some(to) })
}

/// What's at the app's maker's: the newest version (and, with `fetch`,
/// it downloaded and checked).
fn look_for(rec: &Record, trust: &mut Trust, fetch: bool) -> Result<Found> {
    if let Some(s) = &rec.updates {
        return match s.kind.as_str() {
            "apt" => apt(rec, s, trust, fetch),
            // (What a nami bundle's own last look found: src/bundle_app.rs.)
            "bundle" => Ok(match crate::bundle_app::newer_version(&Places::here(), rec) {
                Some(version) => Found::Newer { version, file: None },
                None => Found::Current(rec.version.clone()),
            }),
            _ => yum(rec, s, trust, fetch),
        };
    }
    if let Some(info) = trust.appimage.clone() {
        return appimage(rec, &info, trust, fetch);
    }
    if let Some(repo) = trust.github.clone() {
        return github(rec, &repo, fetch);
    }
    Ok(Found::Manual("Its maker doesn't publish updates nami can follow: check their site for a newer version.".into()))
}

/// `nami file-apps --updates`: the newer version there, if there's one.
pub fn newer(rec: &Record) -> Result<Option<String>> {
    let mut trust = rec.trust.clone().unwrap_or_default();
    match look_for(rec, &mut trust, false) {
        Ok(Found::Newer { version, .. }) => Ok(Some(version)),
        Ok(_) => Ok(None),
        Err(e) => Err(e),
    }
}

// ------------------------------------------------------------ what's known

/// What's known of each app's updates, for Settings (app-updates.json).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Status {
    /// When everything was last looked at (a full check: daily).
    #[serde(default)]
    pub checked: u64,
    #[serde(default)]
    pub apps: BTreeMap<String, AppStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppStatus {
    /// Updates by itself (each app's switch).
    #[serde(default = "yes")]
    pub automatic: bool,
    /// When it was last looked at, and that went through.
    #[serde(default)]
    pub checked: u64,
    /// up-to-date, updated, waiting (until it's closed), refused, manual, failed.
    #[serde(default)]
    pub state: String,
    /// In plain words, for Settings.
    #[serde(default)]
    pub said: String,
    /// The newest version at its maker's.
    #[serde(default)]
    pub newest: Option<String>,
    /// When it was last updated, and from which version.
    #[serde(default)]
    pub updated: u64,
    #[serde(default)]
    pub before: Option<String>,
    /// A checked download waiting for the app to close: (file, its SHA-256, version).
    #[serde(default)]
    pub staged: Option<(String, String, String)>,
    /// The problem a notification said last (not said twice).
    #[serde(default)]
    pub told: String,
}

fn yes() -> bool {
    true
}

impl Default for AppStatus {
    fn default() -> Self {
        AppStatus { automatic: true, checked: 0, state: String::new(), said: String::new(), newest: None, updated: 0, before: None, staged: None, told: String::new() }
    }
}

fn status_file(p: &Places) -> PathBuf {
    p.data.join("nami/app-updates.json")
}

pub fn status(p: &Places) -> Status {
    fs::read_to_string(status_file(p)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(p: &Places, s: &Status) -> Result<()> {
    let f = status_file(p);
    fs::create_dir_all(f.parent().unwrap())?;
    let tmp = f.with_extension("json.new");
    fs::write(&tmp, serde_json::to_string_pretty(s)?)?;
    fs::rename(&tmp, &f)?;
    Ok(())
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A time as a date and time, UTC (for the log).
fn when(t: u64) -> String {
    let (days, secs) = ((t / 86400) as i64, t % 86400);
    // Howard Hinnant's days-to-civil.
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

fn log(line: &str) {
    let dir = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".local/state")).join("nami");
    let _ = fs::create_dir_all(&dir);
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir.join("app-updates.log")) {
        let _ = writeln!(f, "{} {line}", when(now()));
    }
    eprintln!("{line}");
}

/// One run at a time (the daily one, Settings' Update now).
struct Lock(#[allow(dead_code)] fs::File);

fn lock(p: &Places) -> Result<Lock> {
    let f = p.data.join("nami/.app-updates.lock");
    fs::create_dir_all(f.parent().unwrap())?;
    let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(&f)?;
    use std::os::unix::io::AsRawFd;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        say("Waiting for the other update of apps to finish…");
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    }
    Ok(Lock(file))
}

/// Whether namiOS updates itself daily (Settings' "Update by itself, every
/// day": nami-update.timer). The apps from downloads go with it.
fn computer_updates_itself() -> bool {
    if let Ok(v) = std::env::var("NAMI_APP_UPDATES_COMPUTER") {
        return v == "on";
    }
    let systemctl = which("systemctl").unwrap_or_else(|| PathBuf::from("/run/current-system/sw/bin/systemctl"));
    Command::new(systemctl).args(["is-enabled", "nami-update.timer"]).stderr(Stdio::null()).output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "enabled")
}

/// Whether the app is running: a program of its folder, or one started on a file of it.
fn running(places: &Places, id: &str) -> bool {
    let dir = places.apps().join(id);
    let dir = fs::canonicalize(&dir).unwrap_or(dir);
    let me = unsafe { libc::getuid() };
    let Ok(procs) = fs::read_dir("/proc") else { return false };
    for p in procs.flatten() {
        let name = p.file_name();
        if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        use std::os::unix::fs::MetadataExt;
        if p.metadata().map(|m| m.uid()).ok() != Some(me) {
            continue;
        }
        if fs::read_link(p.path().join("exe")).is_ok_and(|e| e.starts_with(&dir)) {
            return true;
        }
        if let Ok(cmd) = fs::read(p.path().join("cmdline")) {
            // (Its own path, or a link to it: a command in ~/.local/bin.)
            let ours = |a: &[u8]| {
                let p = PathBuf::from(&*String::from_utf8_lossy(a));
                p.is_absolute() && (p.starts_with(&dir) || fs::canonicalize(&p).is_ok_and(|c| c.starts_with(&dir)))
            };
            if cmd.split(|b| *b == 0).take(3).any(ours) {
                return true;
            }
        }
    }
    false
}

/// A quiet notification: no sound, low urgency.
fn notify(summary: &str, body: &str, icon: Option<&str>, quiet: bool) {
    let Some(n) = which("notify-send").or_else(|| Some(PathBuf::from("/run/current-system/sw/bin/notify-send")).filter(|p| p.exists())) else { return };
    let mut cmd = Command::new(n);
    cmd.args(["--app-name=Apps", "--hint=string:desktop-entry:nami-gui"]);
    if quiet {
        cmd.args(["--urgency=low", "--hint=boolean:suppress-sound:true"]);
    }
    cmd.arg(format!("--icon={}", icon.unwrap_or("system-software-update")));
    cmd.arg(summary).arg(body);
    let _ = cmd.stdout(Stdio::null()).stderr(Stdio::null()).status();
}

fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [a] => a.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

// ------------------------------------------------------------- the update

/// A checked download put in place of the app: the same setup as at its
/// install, the keys it was installed with kept.
fn put_in_place(root: &Path, places: &Places, rec: &Record, file: &Path, trust: &Trust) -> Result<Record> {
    let l = look(places, file)?;
    if let Some(b) = &l.blocked {
        bail!("{b}");
    }
    ensure!(l.id == rec.id, "the new version is another app ({}), not {}", l.name, rec.name);
    let libraries = std::env::var_os("NAMI_NO_LIBRARIES").is_none();
    let mut new = install(root, places, file, &Options { libraries })?;
    // What it's checked by stays as it was installed (a new key only by
    // installing it again from a download).
    let mut keep = trust.clone();
    if let Some(t) = &new.trust {
        keep.appimage = keep.appimage.clone().or(t.appimage.clone());
        if keep.appimage.is_some() {
            keep.sha1 = t.sha1.clone().or(keep.sha1);
        }
    }
    new.trust = (keep != Trust::default()).then_some(keep);
    fs::write(places.apps().join(&new.id).join("app.json"), serde_json::to_string_pretty(&new)?)?;
    Ok(new)
}

/// What a run did, for its notification.
#[derive(Default)]
struct Done {
    updated: Vec<(String, String, Option<String>)>,
    problems: Vec<(String, String, String)>,
}

/// One app: its update looked for, fetched, checked and put in place (or
/// left for when it's closed).
fn one(root: &Path, places: &Places, rec: &Record, st: &mut AppStatus, full: bool, done: &mut Done) {
    let mut trust = rec.trust.clone().unwrap_or_default();
    // An update waiting for it to close.
    if let Some((file, sum, version)) = st.staged.clone() {
        let f = PathBuf::from(&file);
        if !f.exists() || sha256_of(&f).ok().as_deref() != Some(sum.as_str()) {
            st.staged = None;
        } else if running(places, &rec.id) {
            if !full {
                return;
            }
        } else {
            say(&format!("{} was closed: putting {} {version} in place…", rec.name, rec.name));
            match put_in_place(root, places, rec, &f, &trust) {
                Ok(new) => {
                    log(&format!("{}: updated from {} to {}", rec.id, rec.version, new.version));
                    st.before = Some(rec.version.clone());
                    st.updated = now();
                    st.state = "updated".into();
                    st.said = format!("Updated to {} from {}.", new.version, rec.version);
                    st.newest = Some(new.version.clone());
                    st.staged = None;
                    let _ = fs::remove_file(&f);
                    done.updated.push((new.name.clone(), new.version.clone(), new.icon.clone()));
                }
                Err(e) => {
                    log(&format!("{}: couldn't put {version} in place: {e:#}", rec.id));
                    st.state = "failed".into();
                    st.said = format!("Version {version} couldn't be put in place: {e:#}");
                    st.staged = None;
                    let _ = fs::remove_file(&f);
                }
            }
            return;
        }
    }
    if !full {
        return;
    }
    say(&format!("Looking for a newer {}…", rec.name));
    let found = look_for(rec, &mut trust, true);
    // (A new subkey of its key, found: kept.)
    if rec.trust.as_ref().is_some_and(|t| t.keyring != trust.keyring) {
        let mut r = rec.clone();
        r.trust = Some(trust.clone());
        let _ = fs::write(places.apps().join(&r.id).join("app.json"), serde_json::to_string_pretty(&r).unwrap_or_default());
    }
    match found {
        Ok(Found::Current(v)) => {
            st.checked = now();
            st.newest = Some(v);
            if st.state != "updated" || st.updated + 7 * 86400 < now() {
                st.state = "up-to-date".into();
                st.said = "Up to date.".into();
            }
            st.told.clear();
            say(&format!("{} {} is the newest.", rec.name, rec.version));
        }
        Ok(Found::Manual(why)) => {
            st.checked = now();
            st.state = "manual".into();
            st.said = why.clone();
            say(&why);
        }
        Ok(Found::Newer { version, file: Some(f) }) => {
            st.checked = now();
            st.newest = Some(version.clone());
            if running(places, &rec.id) {
                let sum = sha256_of(&f).unwrap_or_default();
                log(&format!("{}: {version} checked, waiting for it to close", rec.id));
                st.staged = Some((f.display().to_string(), sum, version.clone()));
                st.state = "waiting".into();
                st.said = format!("Version {version} is ready: it's used once {} is closed.", rec.name);
                say(&st.said);
                return;
            }
            say(&format!("Putting {} {version} in place…", rec.name));
            match put_in_place(root, places, rec, &f, &trust) {
                Ok(new) => {
                    log(&format!("{}: updated from {} to {}", rec.id, rec.version, new.version));
                    st.before = Some(rec.version.clone());
                    st.updated = now();
                    st.state = "updated".into();
                    st.said = format!("Updated to {} from {}.", new.version, rec.version);
                    st.told.clear();
                    let _ = fs::remove_file(&f);
                    done.updated.push((new.name.clone(), new.version.clone(), new.icon.clone()));
                }
                Err(e) => {
                    log(&format!("{}: couldn't put {version} in place: {e:#}", rec.id));
                    st.state = "failed".into();
                    st.said = format!("Version {version} couldn't be put in place: {e:#}");
                    let _ = fs::remove_file(&f);
                    say(&st.said);
                }
            }
        }
        Ok(Found::Newer { .. }) => {}
        Err(e) if e.downcast_ref::<Offline>().is_some() => {
            // Offline: nothing said, tried again next time.
            log(&format!("{}: {e:#} (offline: tried again later)", rec.id));
        }
        Err(e) if e.downcast_ref::<Refused>().is_some() => {
            let r = e.downcast_ref::<Refused>().unwrap();
            log(&format!("{}: REFUSED: {}", rec.id, r.why));
            st.checked = now();
            st.state = if r.key_changed { "key-changed".into() } else { "refused".into() };
            st.said = r.why.clone();
            if st.told != r.why {
                let title = if r.key_changed { format!("{} wasn't updated: its maker's key changed", rec.name) } else { format!("{} wasn't updated", rec.name) };
                done.problems.push((rec.id.clone(), title, r.why.clone()));
            }
            say(&r.why);
        }
        Err(e) => {
            let s = e.to_string();
            if let Some(why) = s.strip_prefix("manual: ") {
                st.checked = now();
                st.state = "manual".into();
                st.said = format!("{}: check manually.", first_up(why));
            } else {
                log(&format!("{}: {e:#}", rec.id));
                st.state = "failed".into();
                st.said = format!("Couldn't look for a newer version: {e:#}");
            }
            say(&st.said);
        }
    }
}

fn first_up(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// `nami file-apps --update [app]`: each app (or one) updated now.
/// `background`: the daily run's (the computer's switch and each app's
/// count, a full look only once a day, notifications).
pub fn run(root: &Path, places: &Places, app: Option<&str>, background: bool) -> Result<()> {
    ensure!(!crate::profile::is_root(), "apps from downloads are each person's own: run it as yourself, not as root");
    let _lock = lock(places)?;
    if background && !computer_updates_itself() {
        say("This computer doesn't update by itself (Settings, Computer: Updates), so neither do the apps from downloads.");
        return Ok(());
    }
    let mut status = status(places);
    let recs = match app {
        Some(a) => vec![find_installed(places, a).ok_or_else(|| anyhow!("no app called {a} was installed from a download (`nami file-apps` lists them)"))?],
        None => records(places),
    };
    let full = !background || now().saturating_sub(status.checked) >= 20 * 3600;
    let mut done = Done::default();
    for rec in &recs {
        let mut st = status.apps.get(&rec.id).cloned().unwrap_or_default();
        if background && !st.automatic {
            continue;
        }
        if rec.format == "bundle" {
            // A nami bundle updates itself, as it does anywhere (src/bundle_app.rs).
            if full {
                if let Some(v) = crate::bundle_app::update_now(places, rec, &mut st) {
                    done.updated.push((rec.name.clone(), v, rec.icon.clone()));
                }
            }
        } else {
            one(root, places, rec, &mut st, full, &mut done);
        }
        status.apps.insert(rec.id.clone(), st);
        save(places, &status)?;
    }
    if full && app.is_none() {
        status.checked = now();
    }
    // (Apps removed since: forgotten.)
    let ids: BTreeSet<String> = records(places).into_iter().map(|r| r.id).collect();
    status.apps.retain(|id, _| ids.contains(id));
    save(places, &status)?;
    if background {
        match done.updated.as_slice() {
            [] => {}
            [(name, version, icon)] => notify(&format!("{name} was updated"), &format!("Version {version}."), icon.as_deref(), true),
            many => notify(&format!("{} apps were updated", many.len()), &and_list(&many.iter().map(|m| m.0.clone()).collect::<Vec<_>>()), None, true),
        }
        for (id, title, why) in &done.problems {
            notify(title, why, Some("dialog-warning"), false);
            if let Some(st) = status.apps.get_mut(id) {
                st.told = why.clone();
            }
        }
        if !done.problems.is_empty() {
            save(places, &status)?;
        }
    }
    if recs.is_empty() {
        say("No apps installed from downloads.");
    }
    Ok(())
}

/// `nami file-apps --automatic on|off <app>`: its own switch.
pub fn set_automatic(places: &Places, app: &str, on: bool) -> Result<()> {
    let rec = find_installed(places, app).ok_or_else(|| anyhow!("no app called {app} was installed from a download"))?;
    let _lock = lock(places)?;
    let mut s = status(places);
    s.apps.entry(rec.id.clone()).or_default().automatic = on;
    // A nami bundle's own switch, the same (it looks as it starts too).
    if rec.format == "bundle" {
        crate::bundle_app::set_automatic(places, &rec, on);
    }
    save(places, &s)?;
    say(&format!("{} {}", rec.name, if on { "updates by itself." } else { "doesn't update by itself now (`nami file-apps --update` updates it)." }));
    Ok(())
}

/// `nami file-apps --check <download>`: where its updates would come from,
/// and whether they can be checked, without installing it (read only).
pub fn check_download(file: &Path) -> Result<()> {
    let file = fs::canonicalize(file)?;
    let format = format_of(&file).ok_or_else(|| anyhow!("{} isn't a package nami can install", file.display()))?;
    let (meta, scan) = read_download(&file, format)?;
    let source = update_source(&meta, &scan);
    // The package's key files, read from it.
    let mut wanted: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    if matches!(format, Format::Deb | Format::Rpm) {
        let data = fs::read(&file)?;
        each_file(format, &file, Some(&data), &mut |e: &Entry, r: &mut dyn Read| {
            let p = e.path.trim_start_matches("./").to_string();
            if matches!(e.kind, Kind::File { .. }) && (KEY_DIRS.iter().any(|d| p.starts_with(d)) || p.ends_with(".gpg") || p.ends_with(".asc")) {
                let mut d = Vec::new();
                r.take(1 << 20).read_to_end(&mut d)?;
                wanted.insert(p, d);
            }
            Ok(())
        })?;
    }
    let trust = trust_of(&file, format, &meta, &scan, source.as_ref(), &mut |p| wanted.get(p).cloned(), true).unwrap_or_default();
    let looked = describe(&Places::here(), &file, format, &meta, &scan)?;
    println!("{} {} ({})", looked.name, meta.full_version, file.display());
    match &source {
        Some(s) => println!("  updates from: {} {} {} {}", s.kind, s.url, s.suite, s.components),
        None => println!("  updates from: no repository"),
    }
    if !trust.keys.is_empty() {
        println!("  its key: {} ({})", trust.keys.join(", "), trust.key_from);
    }
    if let Some(u) = &trust.key_url {
        println!("  its key's URL: {u}");
    }
    if let Some(a) = &trust.appimage {
        println!("  AppImage update information: {a}");
    }
    if let Some(g) = &trust.github {
        println!("  on GitHub: {g}");
    }
    let rec = Record {
        id: looked.id.clone(),
        name: looked.name.clone(),
        version: looked.version.clone(),
        full_version: meta.full_version.clone(),
        package: looked.package.clone(),
        from: file.display().to_string(),
        sha256: sha256_of(&file)?,
        updates: source,
        ..Default::default()
    };
    let mut t = trust.clone();
    let how = if rec.updates.is_some() { "its maker's signature on the list of versions is good, and so is the list's checksum; " } else { "" };
    match look_for(&rec, &mut t, false) {
        Ok(Found::Current(v)) => println!("  checked: {how}the newest there is {v}, which this is"),
        Ok(Found::Newer { version, .. }) => println!("  checked: {how}the newest there is {version} (this is {})", rec.version),
        Ok(Found::Manual(why)) => println!("  {why}"),
        Err(e) => println!("  can't follow its updates: {e:#}"),
    }
    Ok(())
}

// ------------------------------------------------- the swap, put back if need be

/// What an app had outside its folder, as it was: put back if its new
/// version can't be put in place.
pub(super) struct Outside {
    files: Vec<(PathBuf, Option<PathBuf>, Option<Vec<u8>>)>,
    mimeapps: Option<Vec<u8>>,
}

pub(super) fn save_outside(places: &Places, rec: &Record) -> Outside {
    let files = rec.files.iter().map(PathBuf::from).filter_map(|p| {
        let m = p.symlink_metadata().ok()?;
        if m.file_type().is_symlink() {
            Some((p.clone(), fs::read_link(&p).ok(), None))
        } else if m.is_file() && m.len() < 16 << 20 {
            Some((p.clone(), None, fs::read(&p).ok()))
        } else {
            None
        }
    }).collect();
    Outside { files, mimeapps: fs::read(places.config.join("mimeapps.list")).ok() }
}

pub(super) fn restore_outside(places: &Places, o: Outside) {
    for (p, link, data) in o.files {
        let _ = fs::remove_file(&p);
        if let Some(parent) = p.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match (link, data) {
            (Some(t), _) => {
                let _ = std::os::unix::fs::symlink(t, &p);
            }
            (None, Some(d)) => {
                let _ = fs::write(&p, d);
            }
            _ => {}
        }
    }
    if let Some(m) = o.mimeapps {
        let _ = fs::write(places.config.join("mimeapps.list"), m);
    }
    refresh_caches(places);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_named_in_scripts_and_sources() {
        let (p, u) = named_keys("deb [arch=amd64 signed-by=/usr/share/keyrings/microsoft.gpg] https://packages.microsoft.com/repos/code stable main\nTypes: deb\nSigned-By: /etc/apt/keyrings/x.asc\ngpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-x https://example.org/key.asc\ncurl -fsSL https://dl.google.com/linux/linux_signing_key.pub | gpg --dearmor\n");
        assert_eq!(p, vec!["/usr/share/keyrings/microsoft.gpg", "/etc/apt/keyrings/x.asc", "/etc/pki/rpm-gpg/RPM-GPG-KEY-x"]);
        assert_eq!(u, vec!["https://example.org/key.asc", "https://dl.google.com/linux/linux_signing_key.pub"]);
    }

    #[test]
    fn keys_are_found_in_the_package() {
        let ring = include_bytes!("../tests/fixtures/openpgp/rsa.gpg");
        let script = "echo 'deb [signed-by=/usr/share/keyrings/t.gpg] http://x/apt stable main' > /etc/apt/sources.list.d/t.list";
        let (got, from, url) = find_keys(&[script], &["usr/share/keyrings/t.gpg", "usr/bin/t"], &mut |p| (p == "usr/share/keyrings/t.gpg").then(|| ring.to_vec()), false);
        assert_eq!(openpgp::fingerprints(&got).unwrap(), vec!["9782AD08406E8500EDE25C10E5B13C40D29750B6"]);
        assert_eq!(from, "in its package (/usr/share/keyrings/t.gpg)");
        assert_eq!(url, None);
        // Written out in a script.
        let ed = String::from_utf8_lossy(include_bytes!("../tests/fixtures/openpgp/ed.asc")).into_owned();
        let (got, from, _) = find_keys(&[&format!("cat > /x.asc <<EOF\n{ed}EOF\n")], &[], &mut |_| None, false);
        assert_eq!(openpgp::fingerprints(&got).unwrap(), vec!["53901FC523D6FCE8A3F1C47E1B592B09FA2FDB68"]);
        assert_eq!(from, "in its package's scripts");
    }

    #[test]
    fn pinned_keys_only() {
        let ring = include_bytes!("../tests/fixtures/openpgp/rsa.gpg").to_vec();
        let mut both = ring.clone();
        both.extend(openpgp::dearmor(include_bytes!("../tests/fixtures/openpgp/ed.asc")).unwrap().concat());
        let t = Trust { keys: vec!["9782AD08406E8500EDE25C10E5B13C40D29750B6".into()], keyring: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &both), ..Default::default() };
        // (A key in the keyring that isn't pinned isn't trusted.)
        assert_eq!(keyring(&t).unwrap().len(), 1);
    }

    #[test]
    fn github_and_appimage_words() {
        assert_eq!(github_of("https://github.com/localsend/localsend/releases/download/v1.18.2/x.deb").as_deref(), Some("localsend/localsend"));
        assert_eq!(github_of("https://github.com/a/b").as_deref(), Some("a/b"));
        assert_eq!(github_of("https://gitlab.com/a/b"), None);
        assert!(glob("Obsidian-*.AppImage.zsync", "Obsidian-1.9.AppImage.zsync"));
        assert!(!glob("Obsidian-*-arm64.AppImage.zsync", "Obsidian-1.9.AppImage.zsync"));
        assert_eq!(tag_version("v1.18.2"), "1.18.2");
        let z = zsync_header(b"zsync: 0.6.2\nFilename: App-2.0-x86_64.AppImage\nURL: App-2.0-x86_64.AppImage\nSHA-1: ABC\nLength: 12\n\n\x00\x01");
        assert_eq!(z.get("SHA-1").map(String::as_str), Some("ABC"));
        assert_eq!(z.get("Length").map(String::as_str), Some("12"));
        assert_eq!(file_name_of("https://x/pool/main/c/code/code_1.2_amd64.deb?x=1"), "code_1.2_amd64.deb");
        assert_eq!(file_name_of("https://x/.."), "download..");
    }

    #[test]
    fn xml_is_read_as_yum_writes_it() {
        let p = r#"<package type="rpm"><name>t</name><arch>x86_64</arch><version epoch="0" ver="2" rel="1"/><checksum type="sha256" pkgid="YES">AB</checksum><location href="t-2-1.x86_64.rpm"/><size package="10" installed="2"/></package>"#;
        assert_eq!(xml_text(p, "name"), Some("t"));
        assert_eq!(xml_attr(p, "version", "ver"), Some("2"));
        assert_eq!(xml_attr(p, "checksum", "type"), Some("sha256"));
        assert_eq!(xml_text(p, "checksum"), Some("AB"));
        assert_eq!(xml_attr(p, "size", "package"), Some("10"));
    }

    #[test]
    fn a_signed_appimage() {
        // (Made as appimagetool --sign makes one: tests/fixtures/appimage, its own key.)
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/appimage/signed.AppImage");
        assert_eq!(appimage_update_info(&file).as_deref(), Some("zsync|https://example.org/App-x86_64.AppImage.zsync"));
        let (fps, _) = appimage_key(&file).expect("its signature checks out");
        assert_eq!(fps, vec!["4B48493EF810BB244B503A13B982EE86724AE2D1"]);
        // A byte of it changed (past its sections): its signature doesn't match.
        let dir = std::env::temp_dir().join(format!("nami-appimage-sig-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut d = fs::read(&file).unwrap();
        let last = d.len() - 3;
        d[last] ^= 1;
        fs::write(dir.join("changed.AppImage"), &d).unwrap();
        assert!(appimage_key(&dir.join("changed.AppImage")).is_none());
        assert!(appimage_signature(&dir.join("changed.AppImage")).is_some());
        // Its update held to the key it was installed with: another key's refused.
        let rec = Record { name: "App".into(), ..Default::default() };
        let mut t = Trust { keys: vec!["9782AD08406E8500EDE25C10E5B13C40D29750B6".into()], keyring: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, include_bytes!("../tests/fixtures/openpgp/rsa.gpg")), ..Default::default() };
        let e = appimage_signed(&rec, "2.0", &mut t, &file).unwrap_err();
        assert!(e.downcast_ref::<Refused>().is_some_and(|r| r.key_changed), "{e}");
        let (fps, ring) = appimage_key(&file).unwrap();
        let mut t = Trust { keys: fps, keyring: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &ring), ..Default::default() };
        assert!(appimage_signed(&rec, "2.0", &mut t, &file).is_ok());
        let e = appimage_signed(&rec, "2.0", &mut t, &dir.join("changed.AppImage")).unwrap_err();
        assert!(e.to_string().contains("changed after its maker signed it"), "{e}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dates_for_the_log() {
        assert_eq!(when(0), "1970-01-01 00:00:00Z");
        assert_eq!(when(1793819036), "2026-11-04 19:03:56Z");
    }
}
