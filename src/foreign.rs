//! Apps built for other distributions' downloads, installed for one person: a Debian
//! or Ubuntu package (.deb), a Fedora or openSUSE one (.rpm), an AppImage,
//! a tarball with a program in it. Opening one in Files (or from the
//! browser's downloads) shows it in Settings, to install (`nami
//! open-package`); `nami install ./foo.deb` does the same in a terminal and
//! `nami remove foo` takes it off.
//!
//! Nothing runs as root, and nothing of the package runs at all to install
//! it: its maintainer scripts (postinst...) are read, not run. What they
//! usually do is done here, for this person: the launcher's entries
//! (~/.local/share/applications, their Exec pointing into the app's folder),
//! its icon, its kinds of files (shared-mime-info's XML) and the links it
//! opens (`x-scheme-handler/vscode`, as the default when nothing else is),
//! its commands (~/.local/bin), and the libraries it needs that the
//! computer's set for other distributions (nix-ld's) hasn't, added from the
//! catalog. An Electron or Chromium app's own sandbox (chrome-sandbox,
//! setuid on other distributions) isn't needed where people may make their own
//! namespaces, which is looked at first; it's never turned off.
//!
//! Each app is a folder of its own: ~/.local/share/nami/apps/<id>, the
//! package's files as they'd be under / in `files/` (/opt and /usr as they
//! are), and `app.json`: what was put where, for Remove to take it all off.
//!
//! What can't work this way is said before: a driver for the kernel, a
//! service for the whole computer, a helper that needs an admin's rights.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::deb::{self, Entry, Kind};
use crate::rpm;

/// Keeping them up to date: `nami file-apps --update`.
#[path = "foreign_update.rs"]
pub mod update;

/// What a download is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Deb,
    Rpm,
    AppImage,
    Tarball,
}

impl Format {
    fn made_for(self) -> &'static str {
        match self {
            Format::Deb => "Debian and Ubuntu",
            Format::Rpm => "Fedora and openSUSE",
            Format::AppImage => "Any Linux",
            Format::Tarball => "Any Linux",
        }
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".local/share"))
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".config"))
}

/// Where an app's parts go: the person's own folders.
#[derive(Debug, Clone)]
pub struct Places {
    /// ~/.local/share: apps' folders (nami/apps), menu entries, kinds of file.
    pub data: PathBuf,
    /// ~/.config: mimeapps.list.
    pub config: PathBuf,
    /// ~/.local/bin, for commands: on PATH on namiOS (and most Linuxes).
    pub bin: PathBuf,
}

impl Places {
    pub fn here() -> Places {
        Places { data: data_home(), config: config_home(), bin: home().join(".local/bin") }
    }

    /// Where each app's folder is.
    pub fn apps(&self) -> PathBuf {
        self.data.join("nami/apps")
    }

    fn applications(&self) -> PathBuf {
        self.data.join("applications")
    }
}

/// What a file is, by what's in it (and, for a tarball, its name).
pub fn format_of(path: &Path) -> Option<Format> {
    let mut head = [0u8; 512];
    let mut f = fs::File::open(path).ok()?;
    let mut got = 0;
    while got < head.len() {
        match f.read(&mut head[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    let head = &head[..got];
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    if head.starts_with(b"!<arch>\n") && head.len() > 8 + 16 && head[8..].starts_with(b"debian-binary") {
        return Some(Format::Deb);
    }
    if head.starts_with(&[0xed, 0xab, 0xee, 0xdb]) {
        return Some(Format::Rpm);
    }
    if head.len() >= 64 && crate::other_linux::kind_of(head) == crate::other_linux::Kind::AppImage {
        return Some(Format::AppImage);
    }
    let tar_name = [".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.zst", ".tar.zstd", ".tzst", ".tar"].iter().any(|e| name.ends_with(e));
    let compressed = head.starts_with(&[0x1f, 0x8b]) || head.starts_with(&[0xfd, b'7', b'z', b'X', b'Z']) || head.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) || (head.len() > 262 && &head[257..262] == b"ustar");
    (tar_name && compressed).then_some(Format::Tarball)
}

/// `nami install <this>`: a download to install, not a catalog's package.
pub fn is_package_file(arg: &str) -> bool {
    let p = Path::new(arg);
    (arg.contains('/') || [".deb", ".rpm", ".appimage", ".tar.gz", ".tgz", ".tar.xz", ".tar.zst"].iter().any(|e| arg.to_lowercase().ends_with(e))) && p.is_file() && format_of(p).is_some()
}

/// A tarball, uncompressed as it's read.
fn uncompressed<'a>(r: impl BufRead + 'a) -> Result<Box<dyn Read + 'a>> {
    let mut r = r;
    let head = r.fill_buf()?.to_vec();
    Ok(if head.starts_with(&[0x1f, 0x8b]) {
        Box::new(flate2::bufread::MultiGzDecoder::new(r))
    } else if head.starts_with(&[0xfd, b'7', b'z', b'X', b'Z']) {
        Box::new(xz2::bufread::XzDecoder::new_multi_decoder(r))
    } else if head.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Box::new(zstd::stream::read::Decoder::with_buffer(r)?)
    } else {
        Box::new(r)
    })
}

type Each<'a> = dyn FnMut(&Entry, &mut dyn Read) -> Result<()> + 'a;

/// Each file of a .deb, .rpm or tarball (`data`: the file's bytes, for the first two).
fn each_file(format: Format, path: &Path, data: Option<&[u8]>, f: &mut Each) -> Result<()> {
    match (format, data) {
        (Format::Deb, Some(d)) => deb::each_entry(deb::tar_of(d, "data")?, f),
        (Format::Rpm, Some(d)) => rpm::each_entry(d, f),
        (Format::Tarball, _) => deb::each_entry(uncompressed(BufReader::with_capacity(1 << 20, fs::File::open(path)?))?, f),
        _ => bail!("no files to read"),
    }
}

/// What's in a download, as far as installing it needs to know.
#[derive(Debug, Default)]
struct Scan {
    /// Each file and link: its path (no leading `/`) and mode.
    paths: Vec<(String, u32)>,
    links: BTreeMap<String, String>,
    /// Menu entries: their path and text.
    desktops: Vec<(String, String)>,
    /// Pictures that may be its icon.
    pictures: BTreeMap<String, Vec<u8>>,
    /// Small text files of interest: apt's and yum's sources, AppStream's metainfo.
    texts: Vec<(String, String)>,
    /// Programs: (path, a 64-bit one; on ARM, a 64-bit ARM one).
    programs: Vec<(String, bool)>,
    /// On ARM: programs made for another kind of 64-bit computer (x86_64).
    other_arch: bool,
    size: u64,
}

fn picture(path: &str) -> bool {
    let p = path.to_lowercase();
    (p.ends_with(".png") || p.ends_with(".svg")) && !p.contains("node_modules/") && !p.contains("/extensions/") && p.matches('/').count() <= 6
}

fn interesting_text(path: &str) -> bool {
    path.starts_with("etc/apt/") || path.starts_with("etc/yum.repos.d/") || path.contains("/cron/") || path.starts_with("etc/cron.daily/") || path.contains("metainfo/") || path.contains("appdata/") || path.ends_with(".appdata.xml") || path.ends_with(".metainfo.xml")
}

impl Scan {
    /// Take in one file of the download.
    fn take(&mut self, e: &Entry, r: &mut dyn Read) -> Result<()> {
        match &e.kind {
            Kind::Dir => return Ok(()),
            Kind::Symlink(t) => {
                self.links.insert(e.path.clone(), t.clone());
                self.paths.push((e.path.clone(), e.mode));
                return Ok(());
            }
            Kind::Hardlink(_) => {
                self.paths.push((e.path.clone(), e.mode));
                return Ok(());
            }
            Kind::File { .. } => {}
        }
        self.paths.push((e.path.clone(), e.mode));
        let mut head = Vec::with_capacity(4096);
        Read::take(&mut *r, 4096).read_to_end(&mut head)?;
        let whole = (e.path.ends_with(".desktop") && !e.path.contains("/autostart/")) || picture(&e.path) || interesting_text(&e.path);
        let mut size = head.len() as u64;
        if whole && head.len() == 4096 {
            let mut rest = Vec::new();
            Read::take(&mut *r, 2 << 20).read_to_end(&mut rest)?;
            size += rest.len() as u64;
            head.extend(rest);
        }
        size += std::io::copy(r, &mut std::io::sink())?;
        self.size += size;
        if head.starts_with(b"\x7fELF") && e.mode & 0o111 != 0 && head.len() >= 64 {
            let elf_type = u16::from_le_bytes([head[16], head[17]]);
            // A program (not a library, which some packages mark as one too).
            if elf_type == 2 || (elf_type == 3 && !e.path.contains(".so")) {
                // (On ARM, one for x86_64 isn't one that runs here.)
                let other = crate::arch::ARM && head[4] == 2 && crate::bundle_format::elf_arch(&head) != Some(crate::arch::ARCH);
                self.other_arch |= other;
                self.programs.push((e.path.clone(), head[4] == 2 && !other));
            }
        }
        if !whole || size > 2 << 20 {
            return Ok(());
        }
        if e.path.ends_with(".desktop") {
            self.desktops.push((e.path.clone(), String::from_utf8_lossy(&head).into_owned()));
        } else if picture(&e.path) {
            if self.pictures.len() < 2000 && self.pictures.values().map(Vec::len).sum::<usize>() < 64 << 20 {
                self.pictures.insert(e.path.clone(), head);
            }
        } else if let Ok(t) = String::from_utf8(head) {
            self.texts.push((e.path.clone(), t));
        }
        Ok(())
    }

    /// The same for a folder (an AppImage's, unpacked), `prefix` taken off.
    fn take_dir(&mut self, dir: &Path) -> Result<()> {
        for (path, meta) in walk(dir, 12, 200_000) {
            let rel = path.strip_prefix(dir).unwrap().to_string_lossy().into_owned();
            let mode = meta.permissions().mode() & 0o7777;
            let kind = if meta.file_type().is_symlink() {
                Kind::Symlink(fs::read_link(&path).map(|t| t.to_string_lossy().into_owned()).unwrap_or_default())
            } else if meta.is_dir() {
                Kind::Dir
            } else {
                Kind::File { executable: mode & 0o111 != 0 }
            };
            let e = Entry { path: rel, kind, mode };
            if matches!(e.kind, Kind::File { .. }) {
                self.take(&e, &mut fs::File::open(&path)?)?;
            } else {
                self.take(&e, &mut std::io::empty())?;
            }
        }
        Ok(())
    }

    fn has(&self, path: &str) -> bool {
        self.paths.iter().any(|(p, _)| p == path)
    }
}

/// Every file, link and folder under `dir` (links not followed), up to
/// `depth` down and `most` of them.
fn walk(dir: &Path, depth: usize, most: usize) -> Vec<(PathBuf, fs::Metadata)> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((d, n)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else { continue };
        let mut here: Vec<_> = entries.flatten().collect();
        here.sort_by_key(|e| e.file_name());
        for e in here {
            let Ok(meta) = e.path().symlink_metadata() else { continue };
            if meta.is_dir() && n < depth {
                stack.push((e.path(), n + 1));
            }
            out.push((e.path(), meta));
            if out.len() >= most {
                return out;
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// What it says of itself: from its control file (deb), header (rpm) or
/// menu entry and metainfo (AppImage, tarball).
#[derive(Debug, Default, Clone)]
struct Meta {
    package: String,
    version: String,
    full_version: String,
    summary: Option<String>,
    description: Option<String>,
    publisher: Option<String>,
    homepage: Option<String>,
    size: Option<u64>,
    arch: Option<String>,
    depends: Vec<Vec<String>>,
    /// Its maintainer scripts: name and text.
    scripts: Vec<(String, String)>,
    /// What it recommends (Recommends:), as written.
    recommends: String,
}

/// `Microsoft Corporation <vscode-linux@microsoft.com>` is Microsoft Corporation.
fn without_email(s: &str) -> Option<String> {
    let s = s.split('<').next().unwrap_or(s).trim().trim_matches('"').trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// A version as people see it: no epoch (`1:`), no packaging revision (`-1`, `-1793749366`).
fn shown_version(v: &str) -> String {
    let v = v.split_once(':').filter(|(e, _)| e.bytes().all(|b| b.is_ascii_digit())).map(|(_, r)| r).unwrap_or(v);
    // A build's number (`1.18.2+64`, Flutter's) isn't the version people know.
    let v = v.rsplit_once('+').filter(|(up, b)| up.contains('.') && !b.is_empty() && b.bytes().all(|c| c.is_ascii_digit())).map(|(up, _)| up).unwrap_or(v);
    match v.rsplit_once('-') {
        Some((up, rev)) if !up.is_empty() && rev.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'+' || b == b'~') && up.contains(|c: char| c.is_ascii_digit()) => up.to_string(),
        _ => v.to_string(),
    }
}

fn deb_meta(data: &[u8]) -> Result<Meta> {
    let c = deb::control(data)?;
    let get = |k: &str| c.get(k).cloned().filter(|v| !v.is_empty());
    let desc = get("Description").unwrap_or_default();
    let (summary, rest) = desc.split_once('\n').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((desc.clone(), String::new()));
    let description = rest.lines().map(|l| if l.trim() == "." { "\n" } else { l }).collect::<Vec<_>>().join(" ").replace(" \n ", "\n\n").trim().to_string();
    let mut scripts = Vec::new();
    deb::each_entry(deb::tar_of(data, "control")?, |e, r| {
        if ["preinst", "postinst", "prerm", "postrm", "config", "triggers"].contains(&e.path.as_str()) {
            let mut s = String::new();
            let _ = r.read_to_string(&mut s);
            scripts.push((e.path.clone(), s));
        }
        Ok(())
    })?;
    let mut depends = crate::recipe::parse_depends(&get("Depends").unwrap_or_default());
    depends.extend(crate::recipe::parse_depends(&get("Recommends").unwrap_or_default()).into_iter().filter(|alts| alts.iter().any(|a| a.starts_with("lib"))));
    Ok(Meta {
        package: get("Package").unwrap_or_default(),
        version: shown_version(&get("Version").unwrap_or_default()),
        full_version: get("Version").unwrap_or_default(),
        summary: Some(summary).filter(|s| !s.is_empty()),
        description: Some(description).filter(|s| !s.is_empty()),
        publisher: get("Maintainer").as_deref().and_then(without_email),
        homepage: get("Homepage"),
        size: get("Installed-Size").and_then(|s| s.trim().parse::<u64>().ok()).map(|k| k * 1024),
        arch: get("Architecture"),
        depends,
        scripts,
        recommends: get("Recommends").unwrap_or_default(),
    })
}

fn rpm_meta(h: &rpm::Header) -> Meta {
    let s = |t| h.string(t).map(str::to_string).filter(|v| !v.is_empty());
    let mut scripts = Vec::new();
    for (tag, name) in [(rpm::PREIN, "preinst"), (rpm::POSTIN, "postinst"), (rpm::PREUN, "prerm"), (rpm::POSTUN, "postrm")] {
        if let Some(t) = s(tag) {
            scripts.push((name.to_string(), t));
        }
    }
    // `libgtk-3.so.0()(64bit)`, `nss >= 3.26`, `rpmlib(...)`, `/bin/sh`: the names.
    let depends = h.list(rpm::REQUIRENAME).iter().filter(|r| !r.starts_with("rpmlib(") && !r.starts_with('/') && !r.starts_with("config(")).map(|r| vec![r.split(['(', ' ']).next().unwrap_or(r).to_string()]).collect::<BTreeSet<_>>().into_iter().collect();
    let version = s(rpm::VERSION).unwrap_or_default();
    Meta {
        package: s(rpm::NAME).unwrap_or_default(),
        full_version: format!("{version}-{}", s(rpm::RELEASE).unwrap_or_default()),
        version,
        summary: s(rpm::SUMMARY),
        description: s(rpm::DESCRIPTION),
        publisher: s(rpm::VENDOR).or_else(|| s(rpm::PACKAGER)).as_deref().and_then(without_email),
        homepage: s(rpm::URL),
        size: h.number(rpm::LONGSIZE).or_else(|| h.number(rpm::SIZE)),
        arch: s(rpm::ARCH),
        depends,
        scripts,
        recommends: String::new(),
    }
}

/// `Obsidian-1.15.0.AppImage`, `discord-0.0.112.tar.gz`: the name and version in it.
fn name_and_version(file: &Path) -> (String, String) {
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut stem = name.as_str();
    for ext in [".AppImage", ".appimage", ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.zst", ".tar.zstd", ".tzst", ".tar"] {
        if let Some(s) = stem.strip_suffix(ext) {
            stem = s;
            break;
        }
    }
    let parts: Vec<&str> = stem.split(['-', '_']).collect();
    let (mut kept, mut version) = (Vec::new(), String::new());
    for (i, p) in parts.iter().enumerate() {
        let is_version = p.trim_start_matches(['v', 'V']).starts_with(|c: char| c.is_ascii_digit()) && p.contains(|c: char| c.is_ascii_digit());
        if i > 0 && is_version {
            version = p.trim_start_matches(['v', 'V']).to_string();
            break;
        }
        if i > 0 && ["x86", "x86-64", "x86_64", "amd64", "linux", "Linux", "x64"].contains(p) {
            break;
        }
        kept.push(*p);
    }
    let out = kept.join(" ");
    (if out.is_empty() { stem.to_string() } else { out }, version)
}

/// A desktop entry's key in its [Desktop Entry] group.
fn entry_key(text: &str, k: &str) -> Option<String> {
    crate::other_linux::key(&crate::other_linux::entry_keys(text), k).map(str::to_string).filter(|v| !v.is_empty())
}

/// The entry people start it by: shown in the menu, an app.
fn main_entry(desktops: &[(String, String)]) -> Option<&(String, String)> {
    let shown = |t: &str| entry_key(t, "NoDisplay").as_deref() != Some("true") && entry_key(t, "Hidden").as_deref() != Some("true") && entry_key(t, "Type").as_deref().is_none_or(|t| t == "Application");
    // (Not a URL handler's, not an autostart's.)
    desktops.iter().filter(|(p, t)| shown(t) && !p.contains("url-handler")).min_by_key(|(p, _)| (p.matches('/').count(), p.len())).or_else(|| desktops.iter().find(|(_, t)| shown(t)))
}

/// An id that's a folder's and file's name: lower case, letters, digits and `.+-`.
pub(crate) fn slug(s: &str) -> String {
    let out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || ".+-".contains(c) { c.to_ascii_lowercase() } else { '-' }).collect();
    let out = out.trim_matches(['-', '.']).to_string();
    let mut squeezed = String::new();
    for c in out.chars() {
        if !(c == '-' && squeezed.ends_with('-')) {
            squeezed.push(c);
        }
    }
    if squeezed.is_empty() { "app".into() } else { squeezed }
}

/// Something that won't work, said before it's installed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Warning {
    pub title: String,
    pub text: String,
    /// Said only until it's installed: what it is, rather than what won't work.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub until_installed: bool,
}

/// Where its maker publishes updates: an apt or yum repository.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Source {
    /// "apt" or "yum".
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub suite: String,
    #[serde(default)]
    pub components: String,
}

impl Source {
    fn host(&self) -> String {
        self.url.trim_start_matches("https://").trim_start_matches("http://").split('/').next().unwrap_or_default().to_string()
    }
}

/// What a download is, for Settings to show before it's installed.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Looked {
    pub file: String,
    pub format: Option<Format>,
    pub made_for: String,
    pub id: String,
    pub package: String,
    pub name: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub version: String,
    pub publisher: Option<String>,
    pub homepage: Option<String>,
    /// Bytes, installed; and the download's.
    pub size: u64,
    pub download: u64,
    /// Its icon, copied out of it (in ~/.cache/nami/packages).
    pub icon: Option<String>,
    /// It has a menu entry (else it's commands).
    pub app: bool,
    pub commands: Vec<String>,
    /// The links it opens: `vscode`, `discord`.
    pub schemes: Vec<String>,
    pub warnings: Vec<Warning>,
    /// Plain notes: what it'll do, what's been handled.
    pub notes: Vec<String>,
    /// Why it can't be installed at all.
    pub blocked: Option<String>,
    pub updates: Option<Source>,
    /// The version of it installed now, if it is.
    pub installed: Option<String>,
    /// What it asks for (Depends:, Requires:), as the catalog calls those.
    pub needs: Vec<String>,
}

/// A $VAR or ${VAR} in `line`, from the script's `VAR="value"` lines.
fn expand(line: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = line.to_string();
    for (k, v) in vars.iter().rev() {
        out = out.replace(&format!("${{{k}}}"), v).replace(&format!("${k}"), v);
    }
    out
}

/// Where its updates come from, if its scripts or files name an apt
/// source (`deb [arch=amd64] https://packages.microsoft.com/repos/code
/// stable main`, deb822's `URIs:`) or a yum one (`baseurl=`).
fn update_source(meta: &Meta, scan: &Scan) -> Option<Source> {
    let texts = meta.scripts.iter().chain(&scan.texts).map(|(_, t)| t.as_str());
    let (mut deb822, mut yum): (Option<Source>, Option<Source>) = (None, None);
    for text in texts {
        let mut vars = BTreeMap::new();
        for line in text.lines() {
            let l = line.trim();
            if let Some((k, v)) = l.split_once('=') {
                if k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') && !k.is_empty() {
                    vars.insert(k.to_string(), v.trim_matches(['"', '\'']).to_string());
                }
            }
        }
        let lines: Vec<String> = text.lines().map(|l| expand(l, &vars)).collect();
        for (i, l) in lines.iter().enumerate() {
            let t = l.trim().trim_start_matches('#').trim().trim_matches(['"', '\'']);
            let t = t.split_once("=\"").map(|(_, r)| r.trim_end_matches('"')).unwrap_or(t);
            if let Some(rest) = t.strip_prefix("deb ") {
                let rest = rest.trim();
                let rest = if rest.starts_with('[') { rest.split_once(']').map(|(_, r)| r.trim()).unwrap_or("") } else { rest };
                // (To the line's end, or its quote: `... main" > /etc/apt/sources.list.d/x.list`.)
                let rest = rest.split(['"', '\'', '>', '#']).next().unwrap_or("");
                let words: Vec<&str> = rest.split_whitespace().collect();
                if let [url, suite, comps @ ..] = words.as_slice() {
                    if url.starts_with("http") && !url.contains('$') {
                        return Some(Source { kind: "apt".into(), url: url.trim_end_matches('/').to_string(), suite: suite.to_string(), components: comps.join(" ") });
                    }
                }
            }
            if let Some(url) = t.strip_prefix("URIs:").map(str::trim).filter(|u| u.starts_with("http") && !u.contains('$')) {
                let near = |k: &str| lines[i..(i + 6).min(lines.len())].iter().find_map(|l| l.trim().strip_prefix(k).map(|v| v.trim().to_string())).unwrap_or_default();
                deb822.get_or_insert(Source { kind: "apt".into(), url: url.trim_end_matches('/').to_string(), suite: near("Suites:"), components: near("Components:") });
            }
            if let Some(url) = t.strip_prefix("baseurl=").map(str::trim).filter(|u| u.starts_with("http") && !u.contains('$')) {
                yum.get_or_insert(Source { kind: "yum".into(), url: url.trim_end_matches('/').to_string(), ..Default::default() });
            }
        }
    }
    deb822.or(yum)
}

/// What can't work installed just for one person, said before.
fn warnings(name: &str, meta: &Meta, scan: &Scan) -> Vec<Warning> {
    let mut out = Vec::new();
    let scripts: String = meta.scripts.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join("\n");
    let ran = |w: &str| scripts.lines().any(|l| !l.trim_start().starts_with('#') && l.contains(w));
    let module = scan.paths.iter().any(|(p, _)| p.starts_with("lib/modules/") || p.starts_with("usr/lib/modules/") || p.ends_with(".ko") || p.ends_with(".ko.xz") || p.ends_with(".ko.zst") || (p.starts_with("usr/src/") && p.ends_with("dkms.conf")));
    if module || ran("dkms ") {
        out.push(Warning { title: "Its driver won't work".into(), text: format!("{name} comes with a driver for the kernel, which only an admin can add to namiOS, and not this way. What needs the driver won't work."), ..Default::default() });
    }
    let mut services: Vec<String> = scan
        .paths
        .iter()
        .filter_map(|(p, _)| {
            let n = p.strip_prefix("lib/systemd/system/").or_else(|| p.strip_prefix("usr/lib/systemd/system/")).or_else(|| p.strip_prefix("etc/systemd/system/")).or_else(|| p.strip_prefix("etc/init.d/"))?;
            (!n.contains('/') && !n.ends_with(".conf")).then(|| n.to_string())
        })
        .collect();
    services.sort();
    services.dedup();
    if !services.is_empty() {
        let list = services.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        out.push(Warning {
            title: "Its background service won't run".into(),
            text: format!("{name} has a service that runs for the whole computer ({list}), which only an admin can start. Installed just for you, it doesn't run: whatever in {name} needs it won't work."),
            ..Default::default()
        });
    }
    let helpers: Vec<&str> = scan.paths.iter().filter(|(p, m)| m & 0o6000 != 0 && !p.ends_with("chrome-sandbox") && !scan.links.contains_key(p)).filter_map(|(p, _)| p.rsplit('/').next()).collect();
    if !helpers.is_empty() {
        out.push(Warning { title: "A part of it needs an admin's rights".into(), text: format!("{} is meant to run with an admin's rights, which an app installed just for you doesn't get. What uses it won't work.", helpers.join(", ")), ..Default::default() });
    }
    if scan.paths.iter().any(|(p, _)| p.starts_with("lib/udev/rules.d/") || p.starts_with("usr/lib/udev/rules.d/") || p.starts_with("etc/udev/rules.d/")) {
        out.push(Warning { title: "Its devices may not be found".into(), text: format!("{name} brings rules for devices (udev's), which only an admin can add. A device it's made for may not show up in it."), ..Default::default() });
    }
    out
}

/// Whether people may make their own namespaces here (Chromium's and
/// Electron's sandbox needs a user, a PID and a network namespace): tried.
#[cfg(not(target_os = "linux"))]
pub fn sandbox_works() -> bool {
    false
}

/// Whether people may make their own namespaces here (Chromium's and
/// Electron's sandbox needs a user, a PID and a network namespace): tried.
#[cfg(target_os = "linux")]
pub fn sandbox_works() -> bool {
    use std::os::unix::process::CommandExt;
    let mut c = Command::new("/bin/sh");
    c.args(["-c", "exit 0"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: only async-signal-safe calls (unshare), in the child after fork.
    unsafe {
        c.pre_exec(|| {
            if libc::unshare(libc::CLONE_NEWUSER) != 0 || libc::unshare(libc::CLONE_NEWPID | libc::CLONE_NEWNET) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    c.status().is_ok_and(|s| s.success())
}

/// Fedora's names for libraries and tools next to nixpkgs' (Debian's are
/// deb_names.yaml's). A library by its file (`libgtk-3.so.0()(64bit)`) is
/// looked up by that.
const FEDORA: &[(&str, &str)] = &[
    ("gtk3", "gtk3"), ("gtk4", "gtk4"), ("nss", "nss"), ("nspr", "nspr"), ("alsa-lib", "alsa-lib"), ("libX11", "libx11"), ("libxcb", "libxcb"),
    ("libXcomposite", "libxcomposite"), ("libXdamage", "libxdamage"), ("libXext", "libxext"), ("libXfixes", "libxfixes"), ("libXrandr", "libxrandr"),
    ("libXtst", "libxtst"), ("libXScrnSaver", "libxscrnsaver"), ("libxkbcommon", "libxkbcommon"), ("libxkbfile", "libxkbfile"), ("mesa-libgbm", "libgbm"),
    ("libdrm", "libdrm"), ("libsecret", "libsecret"), ("libnotify", "libnotify"), ("libappindicator-gtk3", "libappindicator"),
    ("libayatana-appindicator-gtk3", "libayatana-appindicator"), ("at-spi2-atk", "at-spi2-core"), ("at-spi2-core", "at-spi2-core"), ("atk", "at-spi2-core"),
    ("cups-libs", "cups"), ("dbus-libs", "dbus"), ("expat", "expat"), ("glib2", "glib"), ("pango", "pango"), ("cairo", "cairo"), ("libcurl", "curl"),
    ("libuuid", "libuuid"), ("systemd-libs", "systemdLibs"), ("vulkan-loader", "vulkan-loader"), ("libglvnd-egl", "libglvnd"), ("pipewire-libs", "pipewire"),
];

/// Debian's, Ubuntu's or Fedora's package `name` as the catalog calls it:
/// Some(None) for what every system has.
fn catalog_name(format: Format, name: &str) -> Option<Option<String>> {
    if let Some(soname) = name.strip_suffix("()(64bit)").or(Some(name).filter(|n| n.contains(".so"))) {
        let soname = soname.split('(').next().unwrap_or(soname);
        return crate::recipe::soname_package(soname);
    }
    match format {
        Format::Rpm => FEDORA.iter().find(|(f, _)| *f == name).map(|(_, a)| Some(a.to_string())).or_else(|| ["glibc", "bash", "coreutils", "shadow-utils", "systemd", "libgcc", "libstdc++"].contains(&name).then_some(None)),
        _ => crate::recipe::deb_package(name),
    }
}

/// The commands people know a tool package by.
const TOOLS: &[(&str, &str)] = &[("xdg-utils", "xdg-open"), ("wget", "wget"), ("curl", "curl"), ("python3", "python3"), ("perl", "perl"), ("git", "git"), ("zenity", "zenity"), ("gnupg", "gpg"), ("unzip", "unzip"), ("trash-cli", "trash-put")];

fn on_path(cmd: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
}

/// What a download asks for, as the catalog calls each (the first of
/// alternatives it knows), and the tools it uses that aren't here.
fn needs(format: Format, meta: &Meta) -> (Vec<String>, Vec<String>) {
    let (mut libs, mut tools) = (Vec::new(), Vec::new());
    for alts in &meta.depends {
        // A tool: said if none of its alternatives is here.
        // A tool: said if it's none of those it may be (any it may be that
        // nami doesn't know of may well be here: GLib's `gio`).
        let cmds: Vec<&str> = alts.iter().filter_map(|a| TOOLS.iter().find(|(p, _)| p == a).map(|(_, c)| *c)).collect();
        if !cmds.is_empty() {
            if cmds.len() == alts.len() && !cmds.iter().any(|c| on_path(c)) {
                tools.push(cmds[0].to_string());
            }
            continue;
        }
        // Libraries only (a font, certificates are the system's business).
        let library = |a: &str| a.starts_with("lib") || a.contains(".so") || (format == Format::Rpm && FEDORA.iter().any(|(f, _)| *f == a));
        if let Some(attr) = alts.iter().filter(|a| library(a)).find_map(|a| catalog_name(format, a)).flatten() {
            let attr = attr.split('^').next().map(|a| a.strip_suffix(".lib").unwrap_or(a)).unwrap_or(&attr).to_string();
            if !libs.contains(&attr) {
                libs.push(attr);
            }
        }
    }
    (libs, tools)
}

/// The best picture for icon `name` among `pictures`: an exact name (a
/// PNG, the largest; else an SVG), else Chrome's `product_logo_256.png`
/// kind, else the largest that says it's a logo or an icon.
fn best_icon<'a>(pictures: impl Iterator<Item = (&'a str, u64)> + Clone, name: Option<&str>) -> Option<String> {
    let stem = |p: &str| p.rsplit('/').next().unwrap_or(p).rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_default();
    let ext_png = |p: &str| p.to_lowercase().ends_with(".png");
    let size_of = |p: &str| -> u32 {
        // hicolor/256x256/apps, product_logo_256: the number in it.
        p.split(['/', '_', 'x', '-', '.']).filter_map(|w| w.parse::<u32>().ok()).filter(|n| (16..=1024).contains(n)).max().unwrap_or(0)
    };
    let pick = |filter: &dyn Fn(&str) -> bool| -> Option<String> {
        let all: Vec<(&str, u64)> = pictures.clone().filter(|(p, _)| filter(p)).collect();
        let png = all.iter().filter(|(p, _)| ext_png(p)).max_by_key(|(p, len)| (size_of(p).min(512), *len)).map(|(p, _)| p.to_string());
        png.or_else(|| all.iter().find(|(p, _)| !ext_png(p)).map(|(p, _)| p.to_string()))
    };
    if let Some(n) = name {
        let n = n.rsplit('/').next().unwrap_or(n);
        let n = n.strip_suffix(".png").or_else(|| n.strip_suffix(".svg")).unwrap_or(n);
        if let Some(p) = pick(&|p: &str| stem(p) == n) {
            return Some(p);
        }
    }
    pick(&|p: &str| {
        let s = stem(p).to_lowercase();
        s.contains("logo") || s.starts_with("icon") || s.ends_with("icon") || p.contains("/hicolor/")
    })
}

/// ~/.cache/nami/packages/<this download>: what's taken out of it to show it.
fn cache_for(file: &Path) -> PathBuf {
    use sha2::Digest;
    let meta = fs::metadata(file).ok();
    let key = format!("{}:{}:{:?}", file.display(), meta.as_ref().map(|m| m.len()).unwrap_or(0), meta.and_then(|m| m.modified().ok()));
    let h = sha2::Sha256::digest(key.as_bytes());
    let hex: String = h.iter().take(8).map(|b| format!("{b:02x}")).collect();
    home().join(".cache/nami/packages").join(hex)
}

/// An AppImage's squashfs: where it starts (its ELF runtime's end).
fn appimage_offset(file: &Path) -> Result<u64> {
    let mut head = [0u8; 64];
    fs::File::open(file)?.read_exact(&mut head)?;
    let u16_at = |o: usize| u16::from_le_bytes([head[o], head[o + 1]]) as u64;
    let shoff = u64::from_le_bytes(head[0x28..0x30].try_into().unwrap());
    Ok(shoff + u16_at(0x3a) * u16_at(0x3c))
}

fn unsquashfs() -> Option<PathBuf> {
    ["unsquashfs"].iter().find_map(|n| std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(n)).find(|f| f.is_file()))).or_else(|| Some(PathBuf::from("/run/current-system/sw/bin/unsquashfs")).filter(|p| p.exists()))
}

/// An AppImage's files (all, or only those `only` names) into `into`, read
/// with unsquashfs: nothing of it is started. Without unsquashfs, once
/// it's to be installed, its own runtime unpacks it.
fn unpack_appimage(file: &Path, into: &Path, only: &[&str], may_start: bool) -> Result<()> {
    if into.exists() {
        crate::store::remove_tree(into)?;
    }
    fs::create_dir_all(into.parent().unwrap())?;
    if let Some(u) = unsquashfs() {
        let off = appimage_offset(file)?;
        let out = Command::new(u).arg("-o").arg(off.to_string()).args(["-no-xattrs", "-q", "-n", "-f", "-d"]).arg(into).arg(file).args(only).stdout(Stdio::null()).stderr(Stdio::piped()).output()?;
        // (A pattern that matches nothing is an error to it: fine for `only`.)
        ensure!(out.status.success() || !only.is_empty(), "unpacking it: {}", String::from_utf8_lossy(&out.stderr).trim());
        return Ok(());
    }
    ensure!(may_start, "no unsquashfs here to look inside it");
    let tmp = into.with_extension("unpacking");
    fs::create_dir_all(&tmp)?;
    let mode = fs::metadata(file)?.permissions().mode();
    let copy = tmp.join("it.AppImage");
    fs::copy(file, &copy)?;
    fs::set_permissions(&copy, fs::Permissions::from_mode(mode | 0o700))?;
    let ok = Command::new(&copy).arg("--appimage-extract").current_dir(&tmp).stdout(Stdio::null()).stderr(Stdio::null()).status()?.success();
    ensure!(ok, "it couldn't unpack itself");
    fs::rename(tmp.join("squashfs-root"), into)?;
    crate::store::remove_tree(&tmp)?;
    Ok(())
}

/// What's in an AppImage, to show it: its entry, icon and metainfo.
fn scan_appimage(file: &Path) -> Result<Scan> {
    let dir = cache_for(file).join("appimage");
    let mut scan = Scan::default();
    // (Without unsquashfs: what its name says, until it's installed.)
    if unpack_appimage(file, &dir, &["*.desktop", ".DirIcon", "*.png", "*.svg", "usr/share/metainfo", "usr/share/icons/hicolor/256x256", "usr/share/icons/hicolor/512x512", "usr/share/icons/hicolor/scalable"], false).is_err() {
        scan.size = fs::metadata(file)?.len();
        return Ok(scan);
    }
    scan.take_dir(&dir)?;
    // .DirIcon is the AppImage's own icon (often a link to it).
    if let Some(pic) = fs::read(dir.join(".DirIcon")).ok().filter(|d| d.starts_with(b"\x89PNG") || d.starts_with(b"<")) {
        scan.pictures.insert(".DirIcon.png".into(), pic);
    }
    scan.size = fs::metadata(file)?.len();
    Ok(scan)
}

/// The metainfo's `<developer_name>` (or `<developer><name>`).
fn metainfo_developer(scan: &Scan) -> Option<String> {
    scan.texts.iter().filter(|(p, _)| p.ends_with(".xml")).find_map(|(_, t)| {
        let at = t.find("<developer_name>").map(|i| i + 16).or_else(|| t.find("<developer").and_then(|i| t[i..].find("<name>").map(|j| i + j + 6)))?;
        let end = t[at..].find('<')?;
        without_email(&t[at..at + end])
    })
}

/// Look at a download: what it is, what it'd do, and what won't work.
pub fn look(places: &Places, file: &Path) -> Result<Looked> {
    let file = fs::canonicalize(file).with_context(|| format!("{}", file.display()))?;
    let format = format_of(&file).ok_or_else(|| anyhow!("{} isn't a package nami can install: a .deb, an .rpm, an AppImage or a tarball (.tar.gz, .tar.xz, .tar.zst) with a program in it", file.display()))?;
    let (meta, scan) = read_download(&file, format)?;
    let mut l = describe(places, &file, format, &meta, &scan)?;
    // Its icon, for Settings to show.
    if let Some((path, data)) = l.icon.take().and_then(|p| scan.pictures.get(&p).map(|d| (p, d))) {
        let ext = if path.to_lowercase().ends_with(".svg") { "svg" } else { "png" };
        let to = cache_for(&file).join(format!("icon.{ext}"));
        fs::create_dir_all(to.parent().unwrap())?;
        fs::write(&to, data)?;
        l.icon = Some(to.display().to_string());
    }
    Ok(l)
}

/// A download's own words and its files, read.
fn read_download(file: &Path, format: Format) -> Result<(Meta, Scan)> {
    let mut scan = Scan::default();
    let meta = match format {
        Format::Deb => {
            let data = fs::read(file)?;
            let meta = deb_meta(&data)?;
            each_file(format, file, Some(&data), &mut |e: &Entry, r: &mut dyn Read| scan.take(e, r))?;
            meta
        }
        Format::Rpm => {
            let data = fs::read(file)?;
            let (h, _) = rpm::read_header(&data)?;
            let meta = rpm_meta(&h);
            each_file(format, file, Some(&data), &mut |e: &Entry, r: &mut dyn Read| scan.take(e, r))?;
            // (The header knows each file's mode; the payload's are the same.)
            meta
        }
        Format::Tarball => {
            each_file(format, file, None, &mut |e: &Entry, r: &mut dyn Read| scan.take(e, r))?;
            Meta::default()
        }
        Format::AppImage => {
            scan = scan_appimage(file)?;
            Meta::default()
        }
    };
    Ok((meta, scan))
}

/// The folder all of a tarball's files are in, if there's one.
fn top_folder(scan: &Scan) -> Option<String> {
    let first = scan.paths.first()?.0.split('/').next()?.to_string();
    scan.paths.iter().all(|(p, _)| p.split('/').next() == Some(first.as_str()) && p.contains('/')).then_some(first)
}

fn describe(places: &Places, file: &Path, format: Format, meta: &Meta, scan: &Scan) -> Result<Looked> {
    let entry = main_entry(&scan.desktops);
    let entry_name = entry.and_then(|(_, t)| entry_key(t, "Name"));
    let (file_name, file_version) = name_and_version(file);
    let top = top_folder(scan);
    let mut name = entry_name.clone().unwrap_or_else(|| match format {
        Format::Deb | Format::Rpm => meta.package.clone(),
        _ => top.clone().unwrap_or(file_name.clone()),
    });
    if name.is_empty() {
        name = file_name.clone();
    }
    let package = if meta.package.is_empty() { slug(&entry_name.clone().unwrap_or(file_name.clone())) } else { meta.package.clone() };
    let version = if !meta.version.is_empty() {
        meta.version.clone()
    } else {
        entry.and_then(|(_, t)| entry_key(t, "X-AppImage-Version")).unwrap_or(file_version)
    };
    let id = slug(&package);
    let mut l = Looked {
        file: file.display().to_string(),
        format: Some(format),
        made_for: format.made_for().into(),
        id: id.clone(),
        package,
        name: name.clone(),
        summary: meta.summary.clone().or_else(|| entry.and_then(|(_, t)| entry_key(t, "Comment"))),
        description: meta.description.clone(),
        version,
        publisher: meta.publisher.clone().or_else(|| metainfo_developer(scan)),
        homepage: meta.homepage.clone(),
        size: meta.size.unwrap_or(scan.size),
        download: fs::metadata(file).map(|m| m.len()).unwrap_or(0),
        app: entry.is_some(),
        ..Default::default()
    };
    if l.summary.as_deref() == Some(l.name.as_str()) {
        l.summary = None;
    }
    if l.description.is_some() && l.description == l.summary {
        l.description = None;
    }
    // A virtual machine it runs (Claude's Cowork) wants QEMU where Debian keeps it.
    if meta.recommends.contains("qemu-system") {
        let catalog = if l.package == "claude-desktop" { " namiOS's own claude-desktop package (`nami install claude-desktop`) has what Cowork needs." } else { "" };
        l.notes.push(format!("What in it runs a virtual machine needs QEMU where Debian keeps it, which an app installed just for you doesn't get.{catalog}"));
    }
    // Its icon: the entry's.
    let icon_name = entry.and_then(|(_, t)| entry_key(t, "Icon"));
    let pics = scan.pictures.iter().map(|(p, d)| (p.as_str(), d.len() as u64));
    l.icon = if format == Format::AppImage && scan.pictures.contains_key(".DirIcon.png") && icon_name.as_deref().is_none_or(|n| best_icon(pics.clone(), Some(n)).is_none()) {
        Some(".DirIcon.png".into())
    } else {
        best_icon(pics, icon_name.as_deref())
    };
    // The links it opens.
    for (_, t) in &scan.desktops {
        for m in entry_key(t, "MimeType").unwrap_or_default().split(';') {
            if let Some(s) = m.trim().strip_prefix("x-scheme-handler/") {
                if !l.schemes.contains(&s.to_string()) && !s.is_empty() {
                    l.schemes.push(s.to_string());
                }
            }
        }
    }
    l.commands = commands_of(format, meta, scan).into_iter().map(|(n, _)| n).collect();
    l.warnings = warnings(&name, meta, scan);
    l.updates = update_source(meta, scan);
    let (libs, tools) = needs(format, meta);
    l.needs = libs;
    for t in tools {
        l.notes.push(format!("It uses {t}, which isn't on this computer: `nami install {t}` adds it."));
    }
    // What can't be installed at all.
    let arch = meta.arch.as_deref().unwrap_or("");
    let programs64 = scan.programs.iter().filter(|(_, b64)| *b64).count();
    let (ours, here, look): (&[&str], _, _) = if crate::arch::ARM {
        (&["", "arm64", "aarch64", "all", "noarch"], "ARM (arm64)", "arm64 or aarch64")
    } else {
        (&["", "amd64", "all", "x86_64", "noarch"], "x86_64 (amd64)", "amd64 or x86_64")
    };
    l.blocked = if !ours.contains(&arch) {
        Some(format!("It's made for {} computers, and this one is {here}: look for the {look} download of {name}.", match arch { "arm64" | "aarch64" => "ARM", "amd64" | "x86_64" => "x86_64 (Intel and AMD)", "i386" | "i686" => "32-bit", a => a }))
    } else if programs64 == 0 && scan.other_arch {
        Some(format!("{name} is made for x86_64 (Intel and AMD) computers, and this one is ARM (aarch64): look for its arm64 or aarch64 download."))
    } else if format != Format::AppImage && programs64 == 0 && !scan.programs.is_empty() {
        Some(format!("{name} is a 32-bit program: namiOS starts 64-bit programs built for other distributions. Look for its 64-bit (x86_64 or amd64) download."))
    } else if format == Format::Tarball && scan.programs.is_empty() && scan.desktops.is_empty() {
        Some("There's no program in it to install: it's an archive of files.".into())
    } else if format != Format::AppImage && scan.programs.is_empty() && scan.desktops.is_empty() && l.commands.is_empty() {
        Some(format!("{name} has no program or app in it to install."))
    } else {
        None
    };
    // The sandbox Electron's and Chromium's apps bring.
    if scan.paths.iter().any(|(p, _)| p.ends_with("chrome-sandbox")) || (format == Format::AppImage && scan.desktops.iter().any(|(_, t)| t.contains("--no-sandbox"))) {
        let off = scan.desktops.iter().any(|(_, t)| entry_key(t, "Exec").is_some_and(|e| e.contains("--no-sandbox")));
        if sandbox_works() && off {
            l.notes.push("Its own menu entry turns its sandbox off (--no-sandbox); namiOS starts it with the sandbox on, as this computer lets apps make their own.".into());
        } else if sandbox_works() {
            l.notes.push("Its sandbox works as it is: namiOS lets apps make their own, without an admin's help.".into());
        } else if off {
            l.warnings.push(Warning { title: "It runs without its sandbox".into(), text: format!("{name}'s own menu entry turns its sandbox off (--no-sandbox), and this computer can't give it one (user namespaces are off)."), ..Default::default() });
        } else {
            l.warnings.push(Warning { title: "It won't start".into(), text: format!("{name} keeps itself in a sandbox that needs something this computer has turned off (user namespaces). namiOS doesn't turn its sandbox off for it."), ..Default::default() });
        }
    }
    if let Some(s) = &l.updates {
        l.notes.push(format!("It updates itself from {}, where its maker publishes it: once a day, each update checked against its maker's signature first.", s.host()));
    }
    if let Some(r) = read_record(places, &id) {
        l.installed = Some(r.version);
    }
    Ok(l)
}

/// Its commands: (name, the file in the package it is), from /usr/bin and
/// the links its scripts make (`update-alternatives --install
/// /usr/bin/obsidian obsidian /opt/Obsidian/obsidian`); a tarball's bin/.
fn commands_of(format: Format, meta: &Meta, scan: &Scan) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut add = |name: &str, path: String| {
        if !name.is_empty() && !name.contains('/') && !out.iter().any(|(n, _)| n == name) {
            out.push((name.to_string(), path));
        }
    };
    match format {
        Format::Deb | Format::Rpm => {
            for (p, _) in &scan.paths {
                if let Some(n) = p.strip_prefix("usr/bin/").or_else(|| p.strip_prefix("bin/")).or_else(|| p.strip_prefix("usr/games/")) {
                    add(n, p.clone());
                }
            }
            // Generic names stay the system's: an editor, a browser.
            const GENERIC: &[&str] = &["editor", "x-www-browser", "gnome-www-browser", "x-terminal-emulator", "x-session-manager", "pager", "www-browser"];
            for (_, s) in &meta.scripts {
                for l in s.lines() {
                    if l.trim_start().starts_with('#') {
                        continue;
                    }
                    let words: Vec<&str> = l.split_whitespace().map(|w| w.trim_matches(['\'', '"', ';'])).collect();
                    // (link, what it points to) the script makes.
                    let made = if let Some(i) = words.iter().position(|w| *w == "--install") {
                        match (words.get(i + 1), words.get(i + 3)) {
                            (Some(link), Some(path)) => Some((*link, *path)),
                            _ => None,
                        }
                    } else if let Some(i) = words.iter().position(|w| *w == "ln") {
                        let args: Vec<&str> = words[i + 1..].iter().copied().filter(|w| !w.starts_with('-')).take_while(|w| !["||", "&&", ";", "2>/dev/null"].contains(w)).collect();
                        let symbolic = words[i + 1..].iter().any(|w| w.starts_with('-') && !w.starts_with("--") && w.contains('s'));
                        match args.as_slice() {
                            [target, link] if symbolic => Some((*link, *target)),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    let Some((link, target)) = made else { continue };
                    let Some(at) = link.find("/usr/bin/") else { continue };
                    let n = &link[at + "/usr/bin/".len()..];
                    let target = if target.starts_with('/') { target.trim_start_matches('/').to_string() } else { format!("usr/bin/{target}") };
                    let target = target.split('/').fold(Vec::new(), |mut v: Vec<&str>, p| {
                        match p {
                            ".." => {
                                v.pop();
                            }
                            "." | "" => {}
                            p => v.push(p),
                        }
                        v
                    }).join("/");
                    if !GENERIC.contains(&n) && scan.has(&target) {
                        add(n, target);
                    }
                }
            }
            // A menu entry's program by its name alone (`Exec=localsend_app`), wherever it is.
            for (_, t) in &scan.desktops {
                let Some(cmd) = entry_key(t, "Exec").and_then(|e| shell_words(&e).into_iter().next()) else { continue };
                if cmd.contains('/') || cmd == "env" {
                    continue;
                }
                if let Some((p, _)) = scan.paths.iter().find(|(p, m)| m & 0o111 != 0 && p.rsplit('/').next() == Some(cmd.as_str()) && scan.programs.iter().any(|(q, _)| q == p) || p.rsplit('/').next() == Some(cmd.as_str()) && p.starts_with("usr/bin/")) {
                    add(&cmd, p.clone());
                }
            }
        }
        Format::Tarball => {
            let top = top_folder(scan).map(|t| format!("{t}/")).unwrap_or_default();
            for (p, m) in &scan.paths {
                let Some(rest) = p.strip_prefix(&top) else { continue };
                if let Some(n) = rest.strip_prefix("bin/").filter(|n| !n.contains('/')) {
                    add(n, p.clone());
                } else if scan.desktops.is_empty() && !rest.contains('/') && m & 0o111 != 0 && scan.programs.iter().any(|(q, _)| q == p) {
                    add(rest, p.clone());
                }
            }
        }
        Format::AppImage => {}
    }
    out
}

/// What was installed, for Remove: in the app's folder, app.json.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Record {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub full_version: String,
    pub format: String,
    pub package: String,
    #[serde(default)]
    pub publisher: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    /// The download it came from, and its SHA-256.
    pub from: String,
    pub sha256: String,
    /// When (seconds since 1970).
    pub installed: u64,
    pub size: u64,
    #[serde(default)]
    pub icon: Option<String>,
    /// Its entry people start it by (a file in ~/.local/share/applications).
    #[serde(default)]
    pub main_entry: Option<String>,
    /// Everything outside its folder that was put there: entries, kinds of
    /// file, commands.
    #[serde(default)]
    pub files: Vec<String>,
    /// Defaults set for it in ~/.config/mimeapps.list: (kind, entry).
    #[serde(default)]
    pub defaults: Vec<(String, String)>,
    /// Packages added to ~/user.nami for libraries it needs, and the links
    /// made to them in the person's own set.
    #[serde(default)]
    pub libraries: Vec<String>,
    #[serde(default)]
    pub library_links: Vec<String>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub updates: Option<Source>,
    #[serde(default)]
    pub notes: Vec<String>,
    /// How its updates are known to be its maker's: the keys pinned as it
    /// was installed, its AppImage's update information (update.rs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust: Option<update::Trust>,
}

pub fn read_record(p: &Places, id: &str) -> Option<Record> {
    serde_json::from_str(&fs::read_to_string(p.apps().join(id).join("app.json")).ok()?).ok()
}

/// Every app installed from a download, by name.
pub fn records(p: &Places) -> Vec<Record> {
    let mut out: Vec<Record> = fs::read_dir(p.apps()).into_iter().flatten().flatten().filter_map(|e| read_record(p, &e.file_name().to_string_lossy())).collect();
    out.sort_by_key(|r| r.name.to_lowercase());
    out
}

/// The installed app `name` means: its id, package or name.
pub fn find_installed(p: &Places, name: &str) -> Option<Record> {
    read_record(p, &slug(name)).or_else(|| records(p).into_iter().find(|r| r.package == name || r.name.eq_ignore_ascii_case(name)))
}

pub(crate) fn say(s: &str) {
    println!("{s}");
    let _ = std::io::stdout().flush();
}

/// `path` without escaping `root`: `a/../../etc` isn't.
fn inside(root: &Path, rel: &str) -> Result<PathBuf> {
    ensure!(rel.split('/').all(|p| p != ".." && !p.is_empty() && p != "."), "{rel}: a path nami won't unpack");
    // Not through a link the package made.
    let mut at = root.to_path_buf();
    let parts: Vec<&str> = rel.split('/').collect();
    for p in &parts[..parts.len() - 1] {
        at.push(p);
        match at.symlink_metadata() {
            Ok(m) if m.file_type().is_symlink() => bail!("{rel}: inside a link the package made"),
            Ok(_) => {}
            Err(_) => fs::create_dir_all(&at)?,
        }
    }
    Ok(root.join(rel))
}

/// A download's files into `dest`, as they'd be under / (a tarball's as it has them).
fn extract(file: &Path, format: Format, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    if format == Format::AppImage {
        let into = dest.with_extension("squashfs");
        unpack_appimage(file, &into, &[], true)?;
        fs::remove_dir(dest)?;
        fs::rename(&into, dest)?;
        return Ok(());
    }
    let data = if matches!(format, Format::Deb | Format::Rpm) { Some(fs::read(file)?) } else { None };
    let mut hardlinks = Vec::new();
    each_file(format, file, data.as_deref(), &mut |e: &Entry, r: &mut dyn Read| {
        if e.path.is_empty() {
            return Ok(());
        }
        let at = inside(dest, &e.path)?;
        if at.symlink_metadata().is_ok() && !matches!(e.kind, Kind::Dir) {
            let _ = fs::remove_file(&at);
        }
        match &e.kind {
            Kind::Dir => fs::create_dir_all(&at)?,
            Kind::File { .. } => {
                let mut f = fs::File::create(&at).with_context(|| format!("writing {}", at.display()))?;
                std::io::copy(r, &mut f)?;
                // Never setuid: nothing here runs as anyone but you.
                fs::set_permissions(&at, fs::Permissions::from_mode((e.mode & 0o777) | 0o600))?;
            }
            Kind::Symlink(t) => std::os::unix::fs::symlink(t, &at)?,
            Kind::Hardlink(t) => hardlinks.push((e.path.clone(), t.clone())),
        }
        Ok(())
    })?;
    for (to, from) in hardlinks {
        let (at, src) = (inside(dest, &to)?, inside(dest, &from)?);
        if src.symlink_metadata().is_ok_and(|m| m.is_file()) {
            fs::copy(&src, &at)?;
        }
    }
    Ok(())
}

/// `to` relative to the folder `from` is in (both relative to the same root).
fn relative(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.parent().map(|p| p.components().collect()).unwrap_or_default();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for c in &to[common..] {
        out.push(c);
    }
    out
}

/// The package's files made to work from where they are: links to `/opt/...`
/// pointing into it, scripts' `#!/bin/bash` (there's no /bin/bash here)
/// through env, and its own paths in text files (`/opt/Foo/foo`,
/// `/usr/share/foo`) where they are now.
fn settle(files: &Path) -> Result<()> {
    let all = walk(files, 30, 400_000);
    for (path, meta) in &all {
        if meta.file_type().is_symlink() {
            let Ok(target) = fs::read_link(path) else { continue };
            if let Ok(rest) = target.strip_prefix("/") {
                if files.join(rest).symlink_metadata().is_ok() {
                    let rel = relative(path.strip_prefix(files).unwrap(), rest);
                    fs::remove_file(path)?;
                    std::os::unix::fs::symlink(rel, path)?;
                }
            }
        }
    }
    for (path, meta) in &all {
        if !meta.is_file() || meta.len() > 1 << 20 || meta.len() < 4 {
            continue;
        }
        let Ok(data) = fs::read(path) else { continue };
        if data.contains(&0) {
            continue;
        }
        let Ok(text) = String::from_utf8(data) else { continue };
        let mut fixed = text.clone();
        if meta.permissions().mode() & 0o111 != 0 && fixed.starts_with("#!") {
            fixed = fix_shebang(&fixed);
        }
        if fixed.contains("/opt/") || fixed.contains("/usr/") {
            fixed = own_paths(&fixed, files);
        }
        if fixed != text {
            fs::write(path, fixed)?;
        }
    }
    Ok(())
}

/// `#!/bin/bash` as `#!/usr/bin/env bash`: what's in /bin and /usr/bin on
/// other distributions is found on PATH here (/bin/sh and /usr/bin/env are there).
fn fix_shebang(text: &str) -> String {
    let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
    let line = first.trim_start_matches("#!").trim();
    let (prog, args) = line.split_once(char::is_whitespace).map(|(p, a)| (p, a.trim())).unwrap_or((line, ""));
    if prog == "/bin/sh" || prog == "/usr/bin/env" {
        return text.to_string();
    }
    let Some(name) = prog.strip_prefix("/bin/").or_else(|| prog.strip_prefix("/usr/bin/")).or_else(|| prog.strip_prefix("/usr/local/bin/")) else { return text.to_string() };
    let args = if args.is_empty() { String::new() } else { format!(" {args}") };
    format!("#!/usr/bin/env {name}{args}\n{rest}")
}

/// Paths in `text` into /opt or /usr that the package has: where they are now.
fn own_paths(text: &str, files: &Path) -> String {
    let path_char = |c: char| c.is_ascii_alphanumeric() || "/._+-@".contains(c);
    let (mut out, mut rest) = (String::new(), text);
    while let Some(i) = rest.find('/') {
        let boundary = rest[..i].chars().next_back().is_none_or(|c| !path_char(c));
        let end = rest[i..].find(|c: char| !path_char(c)).map(|e| i + e).unwrap_or(rest.len());
        let word = &rest[i..end];
        let trimmed = word.trim_end_matches(['.', '/']);
        let ours = boundary
            && (word.starts_with("/opt/") || word.starts_with("/usr/"))
            && trimmed.split('/').all(|p| p != "..")
            && trimmed.matches('/').count() >= 2
            && files.join(trimmed.trim_start_matches('/')).symlink_metadata().is_ok();
        out.push_str(&rest[..i]);
        if ours {
            out.push_str(&files.join(trimmed.trim_start_matches('/')).display().to_string());
            out.push_str(&word[trimmed.len()..]);
        } else {
            out.push_str(word);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Quoted for a desktop entry's Exec, if it needs to be.
fn exec_quote(p: &str) -> String {
    if p.chars().any(|c| " \t\"'\\$`;&|<>()*?#~".contains(c)) {
        format!("\"{}\"", p.replace('\\', "\\\\\\\\").replace('"', "\\\\\"").replace('$', "\\\\$").replace('`', "\\\\`"))
    } else {
        p.to_string()
    }
}

/// How the program a menu entry names is found in the app's folder.
struct Finder<'a> {
    files: &'a Path,
    format: Format,
    /// Programs that need the app's own libraries found: their wrapper.
    wrappers: &'a BTreeMap<PathBuf, PathBuf>,
    /// A tarball's (or AppImage's) program, when the entry's isn't there.
    main: Option<PathBuf>,
    /// The sandbox works here: an entry's `--no-sandbox` (an AppImage's) goes, so it's on.
    sandbox: bool,
}

impl Finder<'_> {
    fn find(&self, cmd: &str) -> Option<PathBuf> {
        if self.format == Format::AppImage {
            let run = self.files.join("AppRun");
            if run.symlink_metadata().is_ok() {
                return Some(run);
            }
        }
        let found = if let Some(rest) = cmd.strip_prefix('/') {
            Some(self.files.join(rest)).filter(|p| p.symlink_metadata().is_ok())
        } else {
            ["usr/bin", "bin", "usr/local/bin", "usr/games"].iter().map(|d| self.files.join(d).join(cmd)).find(|p| p.symlink_metadata().is_ok())
        };
        let found = found.or_else(|| {
            // Its program by that name elsewhere in it (`localsend_app`, in
            // /opt/localsend_app); a tarball's main program.
            let base = cmd.rsplit('/').next().unwrap_or(cmd);
            let named = walk(self.files, 4, 50_000).into_iter().find(|(p, m)| m.is_file() && m.permissions().mode() & 0o111 != 0 && p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(base))).map(|(p, _)| p);
            named.or_else(|| self.main.clone().filter(|_| self.format == Format::Tarball))
        })?;
        let real = fs::canonicalize(&found).unwrap_or(found.clone());
        Some(self.wrappers.get(&real).cloned().unwrap_or(found))
    }
}

/// A menu entry of the package's, made to start it from its folder: Exec
/// and TryExec at its programs, Icon at its picture, not D-Bus activated
/// (its service isn't installed), and marked as this app's.
fn rewrite_entry(text: &str, finder: &Finder, icon: Option<&Path>, id: &str) -> String {
    let mut out = String::new();
    let mut in_main = false;
    for line in text.lines() {
        if line.starts_with('[') {
            if in_main {
                out.push_str(&format!("X-Tray-App={id}\n"));
            }
            in_main = line.trim() == "[Desktop Entry]";
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let (k, v) = match line.split_once('=') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => {
                out.push_str(line);
                out.push('\n');
                continue;
            }
        };
        let fixed = match k {
            "Exec" | "TryExec" => {
                let mut words = shell_words(v);
                if finder.sandbox {
                    words.retain(|w| w != "--no-sandbox");
                }
                // `env FOO=1 prog`: the program is after the settings.
                let mut at = 0;
                if words.first().map(String::as_str) == Some("env") || words.first().map(String::as_str) == Some("/usr/bin/env") {
                    at = 1;
                    while words.get(at).is_some_and(|w| w.contains('=') && !w.starts_with('-')) {
                        at += 1;
                    }
                }
                match words.get(at).and_then(|w| finder.find(w)) {
                    Some(p) => {
                        words[at] = p.display().to_string();
                        if words.first().map(String::as_str) == Some("env") {
                            words[0] = "/usr/bin/env".into();
                        }
                        let joined: Vec<String> = words.iter().enumerate().map(|(i, w)| if i == at { exec_quote(w) } else { requote(w) }).collect();
                        Some(format!("{k}={}", joined.join(" ")))
                    }
                    None if k == "TryExec" => None,
                    None => Some(line.to_string()),
                }
            }
            "Icon" => icon.map(|i| format!("Icon={}", i.display())).or_else(|| Some(line.to_string())),
            "DBusActivatable" | "X-Tray-App" => None,
            // (Its own folder, where it is now; one it doesn't have would keep it from starting.)
            "Path" if v.starts_with("/opt/") || v.starts_with("/usr/") || v.starts_with(&finder.files.display().to_string()) => {
                let here = if v.starts_with('/') && Path::new(v).starts_with(finder.files) { PathBuf::from(v) } else { finder.files.join(v.trim_start_matches('/')) };
                here.is_dir().then(|| format!("Path={}", here.display()))
            }
            _ => Some(line.to_string()),
        };
        if let Some(f) = fixed {
            out.push_str(&f);
            out.push('\n');
        }
    }
    if in_main {
        out.push_str(&format!("X-Tray-App={id}\n"));
    }
    out
}

/// A desktop entry's Exec, as words (its quoting undone).
fn shell_words(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            '\\' if quoted => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A word of an Exec line as it was written (quoted if it has to be).
fn requote(w: &str) -> String {
    if w.is_empty() || w.chars().any(|c| " \t\"'\\$`;&|<>()*?#".contains(c)) {
        format!("\"{}\"", w.replace('\\', "\\\\").replace('"', "\\\"").replace('$', "\\$").replace('`', "\\`"))
    } else {
        w.to_string()
    }
}

/// The libraries the app's programs need that aren't on the computer: the
/// folders of its own that have some (its programs are started with those
/// found), and the rest, by name.
fn libraries(files: &Path) -> Result<(Vec<PathBuf>, Vec<String>)> {
    let all = walk(files, 30, 400_000);
    // Its programs built for other distributions (they ask for /lib64's loader).
    let mut programs = Vec::new();
    let mut own: BTreeMap<String, PathBuf> = BTreeMap::new();
    for (p, m) in &all {
        if !m.is_file() {
            continue;
        }
        let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if name.contains(".so") {
            own.entry(name).or_insert_with(|| p.parent().unwrap().to_path_buf());
        } else if m.permissions().mode() & 0o111 != 0 && programs.len() < 200 && crate::other_linux::kind(p).is_ok_and(|k| k == crate::other_linux::Kind::Foreign) {
            programs.push(p.clone());
        }
    }
    let base = crate::other_linux::search_path();
    let glibc = crate::other_linux::glibc_dir();
    let lacking = |path: &[PathBuf]| -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for p in &programs {
            for l in crate::other_linux::missing_in(p, path, glibc.as_deref()).unwrap_or_default() {
                out.insert(l.soname);
            }
        }
        out
    };
    let first = lacking(&base);
    let dirs: Vec<PathBuf> = first.iter().filter_map(|s| own.get(s).cloned()).collect::<BTreeSet<_>>().into_iter().collect();
    if dirs.is_empty() {
        return Ok((dirs, first.into_iter().collect()));
    }
    let mut path = dirs.clone();
    path.extend(base);
    let rest = lacking(&path);
    Ok((dirs, rest.into_iter().collect()))
}

/// A wrapper that starts `program` with the app's own libraries found too.
fn write_wrapper(at: &Path, program: &Path, dirs: &[PathBuf]) -> Result<()> {
    let list = dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(":");
    let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let text = format!(
        "#!/bin/sh\n# Made by nami: {} with its own libraries found too.\nNIX_LD_LIBRARY_PATH={}:\"${{NIX_LD_LIBRARY_PATH:-/run/current-system/sw/share/nix-ld/lib}}\" exec {} \"$@\"\n",
        program.display(),
        q(&list),
        q(&program.display().to_string())
    );
    fs::create_dir_all(at.parent().unwrap())?;
    fs::write(at, text)?;
    fs::set_permissions(at, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

/// The store names (`hash-gtk+3-3.24.43`) of what the loader for other
/// Linuxes finds libraries in: the computer's set, the driver's, the person's.
fn libraries_here() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for d in crate::other_linux::search_path() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            if let Ok(t) = fs::read_link(e.path()) {
                if let Some(name) = t.strip_prefix("/nix/store").ok().and_then(|r| r.components().next()) {
                    out.insert(pname(&name.as_os_str().to_string_lossy()));
                }
            }
        }
    }
    out
}

/// `abc...-gtk+3-3.24.43-lib` is `gtk+3`: a store path's name without its hash, version and output.
fn pname(store_name: &str) -> String {
    let name = store_name.split_once('-').map(|(h, r)| if h.len() == 32 { r } else { store_name }).unwrap_or(store_name);
    let mut parts: Vec<&str> = name.split('-').collect();
    while parts.len() > 1 && ["lib", "out", "dev", "bin"].contains(parts.last().unwrap()) {
        parts.pop();
    }
    let at = parts.iter().position(|p| p.starts_with(|c: char| c.is_ascii_digit())).filter(|i| *i > 0).unwrap_or(parts.len());
    parts[..at].join("-")
}

/// Packages of the catalog for what an app needs: added to the person's
/// own file (~/user.nami) and applied, and their libraries joined to the
/// person's set, as `nami libraries --add` does. Those added, and the links.
fn add_packages(store: &crate::store::Store, index: &crate::index::Index, attrs: &[String]) -> Result<(Vec<String>, Vec<String>)> {
    let nami = std::env::current_exe().ok().filter(|p| p.exists()).unwrap_or_else(|| crate::migrate::here().join("bin/nami"));
    let file = crate::config::user_file();
    if !file.exists() {
        fs::write(&file, "packages: []\n").with_context(|| format!("writing {}", file.display()))?;
    }
    let listed = crate::config::listed_packages(&fs::read_to_string(&file).unwrap_or_default());
    let mut added = Vec::new();
    for a in attrs {
        if listed.iter().any(|(have, _)| have == a) {
            continue;
        }
        let ok = Command::new(&nami).args(["config", "add", "packages", a, "--file"]).arg(&file).stdout(Stdio::null()).stderr(Stdio::null()).status()?.success();
        ensure!(ok, "couldn't add {a} to {}", file.display());
        added.push(a.clone());
    }
    if !added.is_empty() {
        let ok = Command::new(&nami).arg("apply").arg(&file).stdout(Stdio::null()).stderr(Stdio::null()).status()?.success();
        ensure!(ok, "applying {} failed", file.display());
    }
    let set = crate::other_linux::personal_set();
    fs::create_dir_all(&set)?;
    let profile = home().join(".local/state/nami/profile/lib");
    let mut links = Vec::new();
    for a in attrs {
        // `krb5.lib` is krb5's lib output; `gst_all_1.gstreamer` is a name.
        let (pkg, out) = match index.get(a) {
            Ok(p) => (p, None),
            Err(_) => match a.rsplit_once('.').and_then(|(n, o)| Some((index.get(n).ok()?, o))) {
                Some((p, o)) => (p, Some(o)),
                None => continue,
            },
        };
        for o in out.map(|o| vec![o]).unwrap_or_else(|| vec!["lib", "out"]) {
            let Some(Some(path)) = pkg.outputs.get(o) else { continue };
            for e in fs::read_dir(store.phys(path).join("lib")).into_iter().flatten().flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                let link = set.join(&n);
                if n.contains(".so") && profile.join(&n).exists() && link.symlink_metadata().is_err() {
                    std::os::unix::fs::symlink(profile.join(&n), &link)?;
                    links.push(link.display().to_string());
                }
            }
        }
    }
    Ok((added, links))
}

/// The SHA-256 of a file, as hex.
fn sha256_of(file: &Path) -> Result<String> {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    let mut f = fs::File::open(file)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// `[Default Applications]` of ~/.config/mimeapps.list: `kind`'s, if it has one.
fn default_for(text: &str, kind: &str) -> Option<String> {
    let mut inside = false;
    for l in text.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            inside = l == "[Default Applications]";
        } else if inside {
            if let Some(v) = l.strip_prefix(kind).and_then(|r| r.strip_prefix('=')) {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// mimeapps.list with `kind=entry` set (None: taken out).
fn set_default(text: &str, kind: &str, entry: Option<&str>) -> String {
    let mut out = Vec::new();
    let (mut inside, mut done, mut seen) = (false, false, false);
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            if inside && !done {
                if let Some(e) = entry {
                    out.push(format!("{kind}={e}"));
                }
                done = true;
            }
            inside = t == "[Default Applications]";
            seen |= inside;
        } else if inside && t.strip_prefix(kind).is_some_and(|r| r.starts_with('=')) {
            if let (Some(e), false) = (entry, done) {
                out.push(format!("{kind}={e}"));
                done = true;
            }
            continue;
        }
        out.push(l.to_string());
    }
    if !done {
        if let Some(e) = entry {
            if !seen {
                if out.last().is_some_and(|l| !l.is_empty()) {
                    out.push(String::new());
                }
                out.push("[Default Applications]".into());
            }
            out.push(format!("{kind}={e}"));
        }
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// The program on PATH, if there is one.
fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|f| f.is_file()))
}

/// The kinds of file and the menu's caches, brought up to date with what's in
/// ~/.local/share (GLib reads mimeinfo.cache, and mime.cache for kinds of file).
pub(crate) fn refresh_caches(p: &Places) {
    let _ = crate::desktop::write_mime_cache(&p.applications());
    let mime = p.data.join("mime");
    if mime.join("packages").is_dir() {
        if let Some(u) = which("update-mime-database") {
            let _ = Command::new(u).arg(&mime).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
    }
}

pub struct Options {
    /// Add what it needs from the catalog (else only say so).
    pub libraries: bool,
}

/// Install a download for this person: see the top of this file.
pub fn install(root: &Path, places: &Places, file: &Path, opts: &Options) -> Result<Record> {
    let file = fs::canonicalize(file).with_context(|| format!("{}", file.display()))?;
    let format = format_of(&file).ok_or_else(|| anyhow!("{} isn't a package nami can install: a .deb, an .rpm, an AppImage or a tarball with a program in it", file.display()))?;
    ensure!(!crate::profile::is_root(), "installing a download is each person's own: run it as yourself, not as root");
    let (meta, scan) = read_download(&file, format)?;
    let looked = describe(places, &file, format, &meta, &scan)?;
    if let Some(why) = &looked.blocked {
        bail!("{why}");
    }
    let name = looked.name.clone();
    let id = looked.id.clone();
    say(&format!("Unpacking {name}…"));
    let final_dir = places.apps().join(&id);
    let new_dir = places.apps().join(format!(".{id}.new"));
    if new_dir.exists() {
        crate::store::remove_tree(&new_dir)?;
    }
    fs::create_dir_all(&new_dir)?;
    let result = (|| -> Result<Record> {
        let files_new = new_dir.join("files");
        extract(&file, format, &files_new)?;
        // Written as where they'll be: the app's own folder.
        let files = final_dir.join("files");
        // (Settled in the new folder, its paths as they'll be.)
        settle_at(&files_new, &files)?;
        // The one there before (another version): off first, its libraries
        // kept; its folder kept aside until this one's in place, and put
        // back (with what it had outside it) if it can't be.
        let old = read_record(places, &id);
        let outside = old.as_ref().map(|o| update::save_outside(places, o));
        let aside = places.apps().join(format!(".{id}.old"));
        if aside.exists() {
            crate::store::remove_tree(&aside)?;
        }
        let swapped = (|| -> Result<Record> {
            if let Some(old) = &old {
                say(&format!("Taking off the {name} that was here ({})…", old.version));
                remove_outside(places, old)?;
            }
            if final_dir.exists() {
                fs::rename(&final_dir, &aside)?;
            }
            fs::rename(&new_dir, &final_dir)?;
            finish(root, places, &file, format, &meta, &scan, &looked, opts)
        })();
        match &swapped {
            Ok(_) if aside.exists() => crate::store::remove_tree(&aside)?,
            Err(_) => {
                if aside.exists() {
                    if final_dir.exists() {
                        let _ = crate::store::remove_tree(&final_dir);
                    }
                    let _ = fs::rename(&aside, &final_dir);
                }
                if let Some(o) = outside {
                    update::restore_outside(places, o);
                }
            }
            _ => {}
        }
        swapped
    })();
    if result.is_err() && new_dir.exists() {
        let _ = crate::store::remove_tree(&new_dir);
    }
    result
}

/// settle(), with the app's paths written as `files` though it's at `at` for now.
fn settle_at(at: &Path, files: &Path) -> Result<()> {
    // Links first: as relative ones, where they are doesn't matter.
    settle(at)?;
    // Text files' paths were written as `at`'s: as `files`' now.
    let from = at.display().to_string();
    let to = files.display().to_string();
    for (path, meta) in walk(at, 30, 400_000) {
        if !meta.is_file() || meta.len() > 1 << 20 {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else { continue };
        if text.contains(&from) {
            fs::write(&path, text.replace(&from, &to))?;
        }
    }
    Ok(())
}

/// The app's folder is in place: what goes outside it, and its record.
#[allow(clippy::too_many_arguments)]
fn finish(root: &Path, places: &Places, file: &Path, format: Format, meta: &Meta, scan: &Scan, looked: &Looked, opts: &Options) -> Result<Record> {
    let (name, id) = (looked.name.clone(), looked.id.clone());
    let dir = places.apps().join(&id);
    let files = dir.join("files");
    let mut rec = Record {
        id: id.clone(),
        name: name.clone(),
        version: looked.version.clone(),
        full_version: meta.full_version.clone(),
        format: match format { Format::Deb => "deb", Format::Rpm => "rpm", Format::AppImage => "appimage", Format::Tarball => "tar" }.into(),
        package: looked.package.clone(),
        publisher: looked.publisher.clone(),
        summary: looked.summary.clone(),
        from: file.display().to_string(),
        sha256: sha256_of(file)?,
        installed: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        size: looked.size,
        updates: looked.updates.clone(),
        notes: looked.warnings.iter().map(|w| w.title.clone()).collect(),
        ..Default::default()
    };
    rec.trust = update::at_install(file, format, meta, scan, &files, looked.updates.as_ref());
    // What it needs: its own libraries found, the rest from the catalog.
    say("Checking what it needs…");
    let (own_dirs, lacking) = libraries(&files)?;
    let mut wrappers: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    if !own_dirs.is_empty() {
        for (p, m) in walk(&files, 30, 400_000) {
            if m.is_file() && m.permissions().mode() & 0o111 != 0 && crate::other_linux::kind(&p).is_ok_and(|k| k == crate::other_linux::Kind::Foreign) {
                let w = dir.join("bin").join(p.file_name().unwrap());
                if w.exists() {
                    continue;
                }
                write_wrapper(&w, &p, &own_dirs)?;
                wrappers.insert(fs::canonicalize(&p).unwrap_or(p), w);
            }
        }
    }
    // Libraries it lacks, and those its package says it needs (some are
    // loaded only once it runs) that the computer's set hasn't.
    let store = crate::store::Store::open_direct_readonly(root).ok();
    let index = store.as_ref().and_then(|s| crate::index::Index::load(s).ok());
    if let (Some(store), Some(index)) = (&store, &index) {
        let mut want: Vec<String> = Vec::new();
        let mut unknown = Vec::new();
        for soname in &lacking {
            match crate::recipe::package_with_library(store, index, soname) {
                Some(p) => {
                    if !want.contains(&p) {
                        want.push(p)
                    }
                }
                None => unknown.push(soname.clone()),
            }
        }
        let here = libraries_here();
        for attr in &looked.needs {
            let Ok(pkg) = index.get(attr) else { continue };
            let present = pkg.outputs.values().flatten().any(|p| here.contains(&pname(crate::store::base_name(p))));
            if !present && !want.contains(attr) {
                want.push(attr.clone());
            }
        }
        if !want.is_empty() {
            if opts.libraries {
                say(&format!("Adding what it needs: {}…", want.join(", ")));
                match add_packages(store, index, &want) {
                    Ok((added, links)) => {
                        rec.libraries = added;
                        rec.library_links = links;
                    }
                    Err(e) => rec.notes.push(format!("Couldn't add {}: {e:#}", want.join(", "))),
                }
            } else {
                rec.notes.push(format!("It needs {}, which aren't added (--no-libraries).", want.join(", ")));
            }
        }
        for u in unknown {
            say(&format!("It asks for {u}, which no package in the catalog has: the part of it that needs it may not work."));
            rec.notes.push(format!("It asks for {u}, which no package has."));
        }
    } else if !lacking.is_empty() {
        rec.notes.push(format!("It needs {}, which nami couldn't look up in the catalog.", lacking.join(", ")));
    }
    // The main program, for a tarball's or AppImage's entry.
    let main = main_program(&files, format, &name);
    let finder = Finder { files: &files, format, wrappers: &wrappers, main: main.clone(), sandbox: sandbox_works() };
    // Its menu entries.
    say("Putting it in place…");
    let apps = places.applications();
    fs::create_dir_all(&apps)?;
    let entries: Vec<(String, String)> = match format {
        // (Read where they are now: one may be a link to the app's own, Discord's.)
        Format::Deb | Format::Rpm => {
            let mut found = Vec::new();
            for d in ["usr/share/applications", "usr/local/share/applications"] {
                let mut names: Vec<String> = fs::read_dir(files.join(d)).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".desktop")).collect();
                names.sort();
                for n in names {
                    if let Ok(t) = fs::read_to_string(files.join(d).join(&n)) {
                        found.push((format!("{d}/{n}"), t));
                    }
                }
            }
            found
        }
        // An AppImage's: the one at its top. A tarball's: those it has.
        Format::AppImage => scan.desktops.iter().filter(|(p, _)| !p.contains('/')).cloned().collect(),
        Format::Tarball => scan.desktops.iter().filter(|(p, _)| p.matches('/').count() <= 3).cloned().collect(),
    };
    let mut entries = entries;
    if entries.is_empty() && format != Format::Deb && format != Format::Rpm {
        if let Some(m) = &main {
            entries.push((format!("{id}.desktop"), format!("[Desktop Entry]\nType=Application\nName={name}\nExec={} %U\nTerminal=false\n", m.display())));
        }
    }
    // Its picture: the one the info found, now in its folder.
    let pic_name = |t: &str| entry_key(t, "Icon");
    let pictures = || walk(&files, 12, 200_000).into_iter().filter(|(p, m)| m.is_file() && picture(&p.strip_prefix(&files).unwrap().to_string_lossy())).map(|(p, m)| (p.strip_prefix(&files).unwrap().to_string_lossy().into_owned(), m.len())).collect::<Vec<_>>();
    let all_pics = pictures();
    let icon_for = |t: &str| -> Option<PathBuf> {
        let n = pic_name(t);
        if let Some(abs) = n.as_deref().filter(|n| n.starts_with('/')) {
            let p = files.join(abs.trim_start_matches('/'));
            if p.is_file() {
                return Some(p);
            }
        }
        if format == Format::AppImage && files.join(".DirIcon").exists() && n.as_deref().is_none_or(|n| best_icon(all_pics.iter().map(|(p, l)| (p.as_str(), *l)), Some(n)).is_none()) {
            return fs::canonicalize(files.join(".DirIcon")).ok();
        }
        best_icon(all_pics.iter().map(|(p, l)| (p.as_str(), *l)), n.as_deref()).map(|p| files.join(p))
    };
    let mut schemes: Vec<(String, String)> = Vec::new();
    let main_name = main_entry(&entries).map(|(p, _)| p.clone());
    for (path, text) in &entries {
        let base = path.rsplit('/').next().unwrap_or(path).to_string();
        // Another's of that name (not this app's): this one's beside it.
        let mut out_name = base.clone();
        let existing = apps.join(&out_name);
        if existing.exists() && entry_key(&fs::read_to_string(&existing).unwrap_or_default(), "X-Tray-App").as_deref() != Some(id.as_str()) {
            out_name = format!("nami-{id}-{base}");
        }
        let icon = icon_for(text);
        let fixed = rewrite_entry(text, &finder, icon.as_deref(), &id);
        let dest = apps.join(&out_name);
        fs::write(&dest, &fixed)?;
        rec.files.push(dest.display().to_string());
        if Some(path) == main_name.as_ref() {
            rec.main_entry = Some(dest.display().to_string());
            rec.icon = icon.map(|i| i.display().to_string());
        }
        for m in entry_key(&fixed, "MimeType").unwrap_or_default().split(';').map(str::trim) {
            if m.starts_with("x-scheme-handler/") {
                schemes.push((m.to_string(), out_name.clone()));
            }
        }
    }
    if rec.icon.is_none() {
        rec.icon = looked.icon.as_ref().and_then(|_| icon_for("")).map(|p| p.display().to_string());
    }
    // Its kinds of file (shared-mime-info's), where its scripts would put them.
    let mime_dir = places.data.join("mime/packages");
    for (p, m) in walk(&files, 12, 200_000) {
        let rel = p.strip_prefix(&files).unwrap().to_string_lossy().into_owned();
        let parent = p.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        let mime_xml = rel.ends_with(".xml") && m.is_file() && (rel.contains("share/mime/packages/") || parent.contains("mime"));
        if mime_xml && fs::read_to_string(&p).is_ok_and(|t| t.contains("<mime-info")) {
            fs::create_dir_all(&mime_dir)?;
            let dest = mime_dir.join(format!("nami-{id}-{}", p.file_name().unwrap().to_string_lossy()));
            fs::copy(&p, &dest)?;
            rec.files.push(dest.display().to_string());
        }
    }
    // The links it opens: its, unless another app's set for them already.
    let mimeapps = places.config.join("mimeapps.list");
    let mut text = fs::read_to_string(&mimeapps).unwrap_or_default();
    let before = text.clone();
    // (The web's, mail's: whatever the person picked stays theirs; a browser
    // is made the default in Settings, not by installing one.)
    const EVERYONES: &[&str] = &["http", "https", "ftp", "mailto", "file", "about", "tel", "sms", "irc", "ircs", "webcal", "magnet", "ssh", "sftp", "smb", "geo", "news", "feed"];
    let system_lists: String = std::env::var("XDG_CONFIG_DIRS").unwrap_or_else(|_| "/etc/xdg".into()).split(':').map(|d| format!("{d}/mimeapps.list")).chain(std::env::var("XDG_DATA_DIRS").unwrap_or_default().split(':').filter(|d| !d.is_empty()).map(|d| format!("{d}/applications/mimeapps.list"))).filter_map(|f| fs::read_to_string(f).ok()).collect::<Vec<_>>().join("\n");
    for (kind, entry) in &schemes {
        if EVERYONES.contains(&kind.trim_start_matches("x-scheme-handler/")) || default_for(&system_lists, kind).is_some() {
            continue;
        }
        let theirs = default_for(&text, kind).filter(|d| !d.is_empty());
        let still_there = theirs.as_ref().is_some_and(|d| d.split(';').any(|e| !e.is_empty() && (apps.join(e).exists() || Path::new("/run/current-system/sw/share/applications").join(e).exists())));
        if !still_there && !rec.defaults.iter().any(|(k, _)| k == kind) {
            text = set_default(&text, kind, Some(entry));
            rec.defaults.push((kind.clone(), entry.clone()));
        }
    }
    if text != before {
        fs::create_dir_all(mimeapps.parent().unwrap())?;
        fs::write(&mimeapps, text)?;
    }
    // Its commands.
    let bin = places.bin.clone();
    for (cmd, path) in commands_of(format, meta, scan) {
        let target = finder.find(&format!("/{path}")).unwrap_or_else(|| files.join(&path));
        let link = bin.join(&cmd);
        if link.symlink_metadata().is_ok() {
            continue;
        }
        // (The computer's own of that name comes first on PATH: said.)
        if let Some(other) = which(&cmd).filter(|p| !p.starts_with(&bin)) {
            say(&format!("This computer has a {cmd} of its own ({}), which comes first; this one is {}.", other.display(), link.display()));
        }
        fs::create_dir_all(&bin)?;
        std::os::unix::fs::symlink(&target, &link)?;
        rec.files.push(link.display().to_string());
        rec.commands.push(cmd);
    }
    refresh_caches(places);
    fs::write(dir.join("app.json"), serde_json::to_string_pretty(&rec)?)?;
    let what = if rec.main_entry.is_some() { ": in the launcher".to_string() } else if !rec.commands.is_empty() { format!(", for the terminal: {}", rec.commands.join(", ")) } else { String::new() };
    say(&format!("{name} {} is installed{what}.", rec.version));
    Ok(rec)
}

/// A tarball's or AppImage's program: AppRun; else the one named as the
/// app (or its folder) is; else the largest program in it.
fn main_program(files: &Path, format: Format, name: &str) -> Option<PathBuf> {
    if format == Format::AppImage {
        return Some(files.join("AppRun")).filter(|p| p.symlink_metadata().is_ok());
    }
    if format != Format::Tarball {
        return None;
    }
    let want = slug(name);
    let mut best: Option<(u8, u64, PathBuf)> = None;
    for (p, m) in walk(files, 3, 20_000) {
        if !m.is_file() || m.permissions().mode() & 0o111 == 0 || !matches!(crate::other_linux::kind(&p), Ok(crate::other_linux::Kind::Foreign | crate::other_linux::Kind::Native)) {
            continue;
        }
        let n = slug(&p.file_name().unwrap_or_default().to_string_lossy());
        let dir = p.parent().and_then(|d| d.file_name()).map(|d| slug(&d.to_string_lossy())).unwrap_or_default();
        let score = if n == want { 3 } else if n == dir { 2 } else { 1 };
        if best.as_ref().is_none_or(|(s, l, _)| (score, m.len()) > (*s, *l)) {
            best = Some((score, m.len(), p));
        }
    }
    best.map(|b| b.2)
}

/// What an app put outside its folder, taken off (the folder stays).
pub(crate) fn remove_outside(places: &Places, rec: &Record) -> Result<()> {
    let dir = places.apps().join(&rec.id);
    for f in &rec.files {
        let p = Path::new(f);
        let ours = match p.symlink_metadata() {
            Ok(m) if m.file_type().is_symlink() => fs::read_link(p).is_ok_and(|t| t.starts_with(&dir)),
            Ok(_) => !f.ends_with(".desktop") || entry_key(&fs::read_to_string(p).unwrap_or_default(), "X-Tray-App").as_deref() == Some(rec.id.as_str()),
            Err(_) => false,
        };
        if ours {
            fs::remove_file(p).with_context(|| format!("removing {f}"))?;
        }
    }
    let mimeapps = places.config.join("mimeapps.list");
    if !rec.defaults.is_empty() {
        let mut text = fs::read_to_string(&mimeapps).unwrap_or_default();
        for (kind, entry) in &rec.defaults {
            if default_for(&text, kind).as_deref() == Some(entry.as_str()) {
                text = set_default(&text, kind, None);
            }
        }
        fs::write(&mimeapps, text)?;
    }
    refresh_caches(places);
    Ok(())
}

/// `nami remove <app>`: an app installed from a download, taken off with
/// all it put anywhere; the libraries added for it too, unless another
/// such app needs them.
pub fn remove(places: &Places, rec: &Record) -> Result<()> {
    say(&format!("Removing {}…", rec.name));
    remove_outside(places, rec)?;
    let others: BTreeSet<String> = records(places).into_iter().filter(|r| r.id != rec.id).flat_map(|r| r.libraries).collect();
    let ours: Vec<&String> = rec.libraries.iter().filter(|l| !others.contains(*l)).collect();
    if !ours.is_empty() {
        let nami = std::env::current_exe().ok().filter(|p| p.exists()).unwrap_or_else(|| crate::migrate::here().join("bin/nami"));
        let file = crate::config::user_file();
        for l in &rec.library_links {
            let p = Path::new(l);
            if p.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
                let _ = fs::remove_file(p);
            }
        }
        for p in &ours {
            let _ = Command::new(&nami).args(["config", "remove", "packages", p, "--file"]).arg(&file).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
        let _ = Command::new(&nami).arg("apply").arg(&file).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    let dir = places.apps().join(&rec.id);
    if dir.exists() {
        crate::store::remove_tree(&dir)?;
    }
    say(&format!("{} is removed.", rec.name));
    Ok(())
}

/// `nami open-package <file>`, what opening a download in Files does: an app
/// to install shows in Settings; a tarball that's only files (no program)
/// is unpacked next to it, as Files does with any other archive.
pub fn open(file: &Path) -> Result<i32> {
    let file = fs::canonicalize(file).with_context(|| format!("{}", file.display()))?;
    let format = format_of(&file);
    if format == Some(Format::Tarball) {
        let mut scan = Scan::default();
        each_file(Format::Tarball, &file, None, &mut |e: &Entry, r: &mut dyn Read| scan.take(e, r))?;
        if scan.programs.is_empty() && scan.desktops.is_empty() {
            let at = unpack_beside(&file)?;
            println!("{}", crate::style::out().line(&format!("✓ Unpacked into {}", at.display())));
            return Ok(0);
        }
    }
    if format.is_none() {
        bail!("{} isn't a package nami can install", file.display());
    }
    let gui = which("nami-gui").unwrap_or_else(|| PathBuf::from("nami-gui"));
    // (Settings goes on by itself: this is done once it's started.)
    Command::new(gui).arg("--open-file").arg(&file).stdin(Stdio::null()).spawn().context("starting Settings (nami-gui)")?;
    Ok(0)
}

/// An archive of files unpacked beside it: into its one folder, or a folder
/// named as it is; a number after the name if that's taken.
fn unpack_beside(file: &Path) -> Result<PathBuf> {
    let dir = file.parent().unwrap_or(Path::new("."));
    let (stem, _) = name_and_version(file);
    let stem = {
        let n = file.file_name().unwrap().to_string_lossy().into_owned();
        [".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.zst", ".tar.zstd", ".tzst", ".tar"].iter().find_map(|e| n.strip_suffix(e).map(String::from)).unwrap_or(stem)
    };
    let tmp = dir.join(format!(".{stem}.unpacking"));
    if tmp.exists() {
        crate::store::remove_tree(&tmp)?;
    }
    extract(file, Format::Tarball, &tmp)?;
    let only: Vec<_> = fs::read_dir(&tmp)?.flatten().collect();
    let (from, name) = match only.as_slice() {
        [one] if one.path().is_dir() => (one.path(), one.file_name().to_string_lossy().into_owned()),
        _ => (tmp.clone(), stem.clone()),
    };
    let mut at = dir.join(&name);
    let mut n = 2;
    while at.symlink_metadata().is_ok() {
        at = dir.join(format!("{name} ({n})"));
        n += 1;
    }
    fs::rename(&from, &at)?;
    if tmp.exists() {
        let _ = crate::store::remove_tree(&tmp);
    }
    Ok(at)
}

/// What's newer where its maker publishes updates (its apt or yum
/// repository, checked as an update would be; an AppImage's update
/// information; a GitHub release): the version there, if it's newer.
pub fn newer(rec: &Record) -> Result<Option<String>> {
    update::newer(rec)
}

/// Bytes as people say them: 12 MB, 1.1 GB.
pub fn size_words(n: u64) -> String {
    let mb = n as f64 / 1e6;
    match mb {
        m if m >= 1000. => format!("{:.1} GB", m / 1000.),
        m if m >= 10. => format!("{} MB", m.round()),
        m if m >= 1. => format!("{m:.1} MB"),
        _ => format!("{} kB", (n as f64 / 1e3).round().max(1.)),
    }
}

/// `nami package-info <file>`, in words.
pub fn said(l: &Looked) -> String {
    let mut out = format!("{} {}\n", l.name, l.version);
    if let Some(s) = &l.summary {
        out += &format!("  {s}\n");
    }
    out += &format!("  From: {}\n", l.publisher.as_deref().unwrap_or("not said"));
    out += &format!("  Made for: {}\n  Size: {} installed ({} download)\n", l.made_for, size_words(l.size), size_words(l.download));
    if !l.commands.is_empty() {
        out += &format!("  Commands: {}\n", l.commands.join(", "));
    }
    if !l.schemes.is_empty() {
        out += &format!("  Opens links: {}\n", l.schemes.iter().map(|s| format!("{s}://")).collect::<Vec<_>>().join(", "));
    }
    if let Some(v) = &l.installed {
        out += &format!("  Installed now: {v}\n");
    }
    if let Some(b) = &l.blocked {
        out += &format!("\nIt can't be installed: {b}\n");
    }
    for w in &l.warnings {
        out += &format!("\n{}: {}\n", w.title, w.text);
    }
    for n in &l.notes {
        out += &format!("\n{n}\n");
    }
    out
}

/// `nami file-apps`: the apps installed from downloads (and, with
/// `updates`, what's newer where their makers publish them).
pub fn list(json: bool, updates: bool) -> Result<()> {
    let places = Places::here();
    let all = records(&places);
    if json {
        let mut v = serde_json::to_value(&all)?;
        // A bundle's own: where it updates from, who signs it, what its last look found.
        for (r, item) in all.iter().zip(v.as_array_mut().into_iter().flatten()) {
            if r.format == "bundle" {
                item["bundle"] = serde_json::json!(crate::bundle_app::update_info(&places, r));
            }
        }
        if updates {
            for (r, item) in all.iter().zip(v.as_array_mut().into_iter().flatten()) {
                item["newer"] = serde_json::json!(newer(r).ok().flatten());
            }
        }
        println!("{v}");
        return Ok(());
    }
    let p = crate::style::out();
    if all.is_empty() {
        println!("No apps installed from downloads.");
        println!("{}", p.line("hint: `nami install ./app.deb` installs one"));
    }
    // A line each: its name, version, and what kind of file it came as.
    let w = all.iter().map(|r| format!("{} {}", r.name, r.version).chars().count()).max().unwrap_or(0);
    for r in &all {
        let named = format!("{} {}", r.name, r.version);
        let mut line = format!("{} {}{}  {}", r.name, p.dim(&r.version), " ".repeat(w - named.chars().count()), p.dim(&format!("{} · {}", r.format, r.id)));
        if updates {
            line += &match newer(r) {
                Ok(Some(v)) => format!("  {}", p.changed(&format!("~ {v} is out, at {}", r.updates.as_ref().map(Source::host).unwrap_or_default()))),
                Ok(None) if r.updates.is_some() => format!("  {}", p.dim("up to date")),
                Ok(None) => format!("  {}", p.dim("its maker publishes no updates nami can follow")),
                Err(e) => format!("  {}", p.warning(&format!("couldn't look ({e:#})"))),
            };
        }
        println!("{line}");
    }
    Ok(())
}

/// `nami install ./foo.deb`, in a terminal (or Settings, which reads what it says).
pub fn install_cli(root: &Path, files: &[String], libraries: bool) -> Result<()> {
    for f in files {
        install(root, &Places::here(), Path::new(f), &Options { libraries })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_names_as_people_say_them() {
        assert_eq!(shown_version("1.145.0-1793749366"), "1.145.0");
        assert_eq!(shown_version("1:2.3-0ubuntu1"), "2.3");
        assert_eq!(shown_version("15.2.0-1"), "15.2.0");
        assert_eq!(shown_version("4.16336.0"), "4.16336.0");
        assert_eq!(without_email("Microsoft Corporation <vscode-linux@microsoft.com>").as_deref(), Some("Microsoft Corporation"));
        assert_eq!(name_and_version(Path::new("/d/Obsidian-1.15.0.AppImage")), ("Obsidian".into(), "1.15.0".into()));
        assert_eq!(name_and_version(Path::new("/d/discord-0.0.112.tar.gz")), ("discord".into(), "0.0.112".into()));
        assert_eq!(slug("Google Chrome"), "google-chrome");
        assert_eq!(slug("code"), "code");
        assert_eq!(pname("0123456789abcdef0123456789abcdef-gtk+3-3.24.43"), "gtk+3");
        assert_eq!(pname("0123456789abcdef0123456789abcdef-libgcc-14.2.0-lib"), "libgcc");
        assert_eq!(pname("0123456789abcdef0123456789abcdef-nss-3.101"), "nss");
    }

    #[test]
    fn where_updates_come_from() {
        let scripts = |s: &str| Meta { scripts: vec![("postinst".into(), s.into())], ..Default::default() };
        let scan = Scan::default();
        let vscode = scripts("# deb [arch=amd64] https://packages.microsoft.com/repos/code stable main\ncat > x <<EOF\nTypes: deb\nURIs: https://packages.microsoft.com/repos/code\nSuites: stable\nComponents: main\nEOF\n");
        assert_eq!(update_source(&vscode, &scan), Some(Source { kind: "apt".into(), url: "https://packages.microsoft.com/repos/code".into(), suite: "stable".into(), components: "main".into() }));
        let claude = scripts("APT_REPO_URL=\"https://downloads.claude.ai/claude-desktop/apt/stable\"\nAPT_SUITE=\"stable\"\nAPT_COMPONENT=\"main\"\ncat <<EOF\ndeb [arch=amd64,arm64 signed-by=/usr/share/keyrings/k.asc] $APT_REPO_URL $APT_SUITE $APT_COMPONENT\nEOF\n");
        assert_eq!(update_source(&claude, &scan).map(|s| (s.url, s.suite)), Some(("https://downloads.claude.ai/claude-desktop/apt/stable".into(), "stable".into())));
        let chrome = scripts("REPOCONFIG=\"deb [arch=amd64] https://dl.google.com/linux/chrome/deb/ stable main\"\n");
        assert_eq!(update_source(&chrome, &scan).map(|s| s.url), Some("https://dl.google.com/linux/chrome/deb".into()));
        let yum = scripts("[code]\nname=Visual Studio Code\nbaseurl=https://packages.microsoft.com/yumrepos/vscode\n");
        assert_eq!(update_source(&yum, &scan).map(|s| s.kind), Some("yum".into()));
        assert_eq!(update_source(&scripts("echo hi"), &scan), None);
    }

    #[test]
    fn scripts_and_entries_made_to_work_here() {
        assert_eq!(fix_shebang("#!/bin/bash\necho hi\n"), "#!/usr/bin/env bash\necho hi\n");
        assert_eq!(fix_shebang("#!/bin/sh\necho hi\n"), "#!/bin/sh\necho hi\n");
        assert_eq!(fix_shebang("#!/usr/bin/python3 -u\nx\n"), "#!/usr/bin/env python3 -u\nx\n");
        assert_eq!(shell_words("\"/opt/My App/app\" --x %U"), vec!["/opt/My App/app", "--x", "%U"]);
        assert_eq!(shell_words("env A=1 foo"), vec!["env", "A=1", "foo"]);
        let mimeapps = "[Added Associations]\nx=y.desktop\n\n[Default Applications]\ntext/plain=a.desktop\n";
        let set = set_default(mimeapps, "x-scheme-handler/vscode", Some("code-url-handler.desktop"));
        assert_eq!(default_for(&set, "x-scheme-handler/vscode").as_deref(), Some("code-url-handler.desktop"));
        assert_eq!(default_for(&set, "text/plain").as_deref(), Some("a.desktop"));
        let back = set_default(&set, "x-scheme-handler/vscode", None);
        assert_eq!(default_for(&back, "x-scheme-handler/vscode"), None);
        assert_eq!(default_for(&set_default("", "x-scheme-handler/a", Some("b.desktop")), "x-scheme-handler/a").as_deref(), Some("b.desktop"));
    }

    #[test]
    fn icons_are_found_as_a_desktop_would() {
        let pics = [("usr/share/icons/hicolor/48x48/apps/code.png", 10u64), ("usr/share/icons/hicolor/256x256/apps/code.png", 50), ("usr/share/pixmaps/other.png", 99)];
        assert_eq!(best_icon(pics.iter().map(|(p, l)| (*p, *l)), Some("code")).as_deref(), Some("usr/share/icons/hicolor/256x256/apps/code.png"));
        let chrome = [("opt/google/chrome/product_logo_16.png", 1u64), ("opt/google/chrome/product_logo_256.png", 30), ("opt/google/chrome/product_logo_128.png", 20)];
        assert_eq!(best_icon(chrome.iter().map(|(p, l)| (*p, *l)), Some("google-chrome")).as_deref(), Some("opt/google/chrome/product_logo_256.png"));
    }

    /// A .deb with a program, its entry and icon, a URL it opens, a command, a
    /// kind of file, and a script that would do all that as root.
    fn sample_deb() -> Vec<u8> {
        let entry = b"[Desktop Entry]\nName=Foo Editor\nComment=Edits foo\nExec=/opt/Foo/foo %U\nIcon=foo\nType=Application\nMimeType=text/x-foo;x-scheme-handler/foo;\nDBusActivatable=true\n\n[Desktop Action new]\nName=New\nExec=/opt/Foo/foo --new\n";
        let mime = b"<?xml version=\"1.0\"?>\n<mime-info xmlns=\"http://www.freedesktop.org/standards/shared-mime-info\"><mime-type type=\"text/x-foo\"><glob pattern=\"*.foo\"/></mime-type></mime-info>\n";
        deb::tests::deb(
            &[
                ("./opt/Foo/foo", b'0', b"#!/bin/bash\nexec /opt/Foo/resources/run \"$@\"\n", 0o755),
                ("./opt/Foo/resources/run", b'0', b"#!/bin/sh\necho running\n", 0o755),
                ("./opt/Foo/chrome-sandbox", b'0', b"\x7fELF", 0o4755),
                ("./usr/bin/foo", b'2', b"/opt/Foo/foo", 0o777),
                ("./usr/share/applications/foo.desktop", b'0', entry, 0o644),
                ("./usr/share/icons/hicolor/256x256/apps/foo.png", b'0', b"\x89PNGfoo", 0o644),
                ("./usr/share/mime/packages/foo.xml", b'0', mime, 0o644),
                ("./lib/systemd/system/foo-daemon.service", b'0', b"[Service]\n", 0o644),
            ],
            // (Made for this computer's kind: amd64, or arm64 on ARM.)
            &format!("Package: foo-editor\nVersion: 2:1.2.3-4\nArchitecture: {}\nMaintainer: Foo Inc. <foo@example.com>\nInstalled-Size: 12\nDepends: libgtk-3-0, xdg-utils\nHomepage: https://foo.example\nDescription: Edits foo\n Foo Editor edits foo files.\n", crate::arch::DEB),
        )
    }

    #[test]
    fn looks_at_a_deb_before_installing_it() {
        let dir = std::env::temp_dir().join(format!("nami-foreign-look-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("foo_1.2.3_amd64.deb");
        fs::write(&f, sample_deb()).unwrap();
        assert_eq!(format_of(&f), Some(Format::Deb));
        let data = fs::read(&f).unwrap();
        let meta = deb_meta(&data).unwrap();
        let mut scan = Scan::default();
        each_file(Format::Deb, &f, Some(&data), &mut |e: &Entry, r: &mut dyn Read| scan.take(e, r)).unwrap();
        let l = describe(&Places { data: dir.join("data"), config: dir.join("config"), bin: dir.join("bin") }, &f, Format::Deb, &meta, &scan).unwrap();
        assert_eq!((l.name.as_str(), l.version.as_str(), l.id.as_str()), ("Foo Editor", "1.2.3", "foo-editor"));
        assert_eq!(l.publisher.as_deref(), Some("Foo Inc."));
        assert_eq!(l.size, 12 * 1024);
        assert_eq!(l.schemes, vec!["foo"]);
        assert_eq!(l.commands, vec!["foo"]);
        assert_eq!(l.icon.as_deref(), Some("usr/share/icons/hicolor/256x256/apps/foo.png"));
        assert!(l.blocked.is_none());
        assert!(l.warnings.iter().any(|w| w.text.contains("foo-daemon.service")), "{:?}", l.warnings);
        assert!(l.needs.contains(&"gtk3".to_string()), "{:?}", l.needs);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn installs_a_deb_for_one_person_and_removes_it_all() {
        let dir = std::env::temp_dir().join(format!("nami-foreign-install-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let home_dir = dir.join("home");
        fs::create_dir_all(&home_dir).unwrap();
        let places = Places { data: home_dir.join(".local/share"), config: home_dir.join(".config"), bin: home_dir.join(".local/bin") };
        let f = dir.join("foo.deb");
        fs::write(&f, sample_deb()).unwrap();
        let rec = install(&dir.join("no-store"), &places, &f, &Options { libraries: false }).unwrap();
        let app = home_dir.join(".local/share/nami/apps/foo-editor");
        let files = app.join("files");
        // Its files as under /, the script made to work here, the link relative.
        assert_eq!(fs::read_to_string(files.join("opt/Foo/foo")).unwrap(), format!("#!/usr/bin/env bash\nexec {}/opt/Foo/resources/run \"$@\"\n", files.display()));
        assert_eq!(fs::read_link(files.join("usr/bin/foo")).unwrap(), PathBuf::from("../../opt/Foo/foo"));
        // Never setuid.
        assert_eq!(fs::metadata(files.join("opt/Foo/chrome-sandbox")).unwrap().permissions().mode() & 0o7000, 0);
        // In the launcher: started from its folder, its icon, marked as this app's.
        let entry = fs::read_to_string(home_dir.join(".local/share/applications/foo.desktop")).unwrap();
        assert!(entry.contains(&format!("\nExec={}/opt/Foo/foo %U\n", files.display())), "{entry}");
        assert!(entry.contains(&format!("\nExec={}/opt/Foo/foo --new\n", files.display())), "{entry}");
        assert!(entry.contains(&format!("\nIcon={}/usr/share/icons/hicolor/256x256/apps/foo.png\n", files.display())), "{entry}");
        assert!(entry.contains("X-Tray-App=foo-editor") && !entry.contains("DBusActivatable"), "{entry}");
        // What opens foo:// links, and the kind of file.
        let mimeapps = fs::read_to_string(home_dir.join(".config/mimeapps.list")).unwrap();
        assert_eq!(default_for(&mimeapps, "x-scheme-handler/foo").as_deref(), Some("foo.desktop"));
        let cache = fs::read_to_string(home_dir.join(".local/share/applications/mimeinfo.cache")).unwrap();
        assert!(cache.contains("x-scheme-handler/foo=foo.desktop;"), "{cache}");
        assert!(home_dir.join(".local/share/mime/packages/nami-foo-editor-foo.xml").exists());
        // Its command.
        assert_eq!(fs::read_link(home_dir.join(".local/bin/foo")).unwrap(), files.join("usr/bin/foo"));
        assert_eq!(rec.commands, vec!["foo"]);
        assert_eq!(find_installed(&places, "foo-editor").map(|r| r.version), Some("1.2.3".into()));
        assert_eq!(find_installed(&places, "Foo Editor").map(|r| r.id), Some("foo-editor".into()));
        // Installed again: the same, once.
        install(&dir.join("no-store"), &places, &f, &Options { libraries: false }).unwrap();
        assert_eq!(records(&places).len(), 1);
        // Removed: nothing left of it.
        remove(&places, &find_installed(&places, "foo-editor").unwrap()).unwrap();
        assert!(!app.exists());
        assert!(!home_dir.join(".local/share/applications/foo.desktop").exists());
        assert!(home_dir.join(".local/bin/foo").symlink_metadata().is_err());
        assert!(!home_dir.join(".local/share/mime/packages/nami-foo-editor-foo.xml").exists());
        assert_eq!(default_for(&fs::read_to_string(home_dir.join(".config/mimeapps.list")).unwrap(), "x-scheme-handler/foo"), None);
        assert!(records(&places).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_tarball_of_files_is_only_unpacked() {
        let dir = std::env::temp_dir().join(format!("nami-foreign-tar-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut gz = Vec::new();
        flate2::read::GzEncoder::new(&deb::tests::tar(&[("notes/", b'5', b"", 0o755), ("notes/a.txt", b'0', b"hello", 0o644)])[..], flate2::Compression::fast()).read_to_end(&mut gz).unwrap();
        let f = dir.join("notes-1.0.tar.gz");
        fs::write(&f, gz).unwrap();
        assert_eq!(format_of(&f), Some(Format::Tarball));
        assert_eq!(open(&f).unwrap(), 0);
        assert_eq!(fs::read_to_string(dir.join("notes/a.txt")).unwrap(), "hello");
        assert_eq!(open(&f).unwrap(), 0);
        assert!(dir.join("notes (2)/a.txt").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
