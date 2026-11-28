//! Debian packages (.deb): what's in them, and their files laid out the
//! way a nixpkgs package's are (bin, lib, share), for recipes that start
//! from one.
//!
//! A .deb is an ar archive: `debian-binary`, `control.tar.*` (the package's
//! name, version and what it depends on) and `data.tar.*` (its files, as
//! they go under /).

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{anyhow, bail, ensure, Context, Result};

/// The members of the ar archive: name -> contents.
fn members(deb: &[u8]) -> Result<Vec<(String, &[u8])>> {
    ensure!(deb.starts_with(b"!<arch>\n"), "not a Debian package");
    let mut out = Vec::new();
    let mut at = 8;
    while at + 60 <= deb.len() {
        let head = &deb[at..at + 60];
        let name = String::from_utf8_lossy(&head[..16]).trim().trim_end_matches('/').to_string();
        let size: usize = String::from_utf8_lossy(&head[48..58]).trim().parse().context("a broken package")?;
        let start = at + 60;
        let body = deb.get(start..start + size).ok_or_else(|| anyhow!("a package cut short"))?;
        out.push((name, body));
        at = start + size + size % 2;
    }
    Ok(out)
}

/// The tar archive in member `which` (`data` or `control`), uncompressed as it's read.
pub(crate) fn tar_of<'a>(deb: &'a [u8], which: &str) -> Result<Box<dyn Read + 'a>> {
    for (name, body) in members(deb)? {
        let Some(kind) = name.strip_prefix(which).and_then(|n| n.strip_prefix(".tar")) else { continue };
        return Ok(match kind {
            ".xz" => Box::new(xz2::read::XzDecoder::new_multi_decoder(body)),
            ".zst" => Box::new(zstd::stream::read::Decoder::new(body)?),
            ".gz" => Box::new(flate2::read::GzDecoder::new(body)),
            "" => Box::new(body),
            _ => bail!("its files are {name}, which nami doesn't unpack"),
        });
    }
    bail!("no {which}.tar in it")
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    File { executable: bool },
    Dir,
    Symlink(String),
    /// Another file in the archive, by its path.
    Hardlink(String),
}

/// One file in a tar archive. `path` has no leading `./` or `/`.
#[derive(Debug, Clone)]
pub struct Entry {
    pub path: String,
    pub kind: Kind,
    /// Its permission bits, setuid's too (0o4755).
    pub mode: u32,
}

/// Each entry of a tar stream, with a reader for a file's contents.
pub fn each_entry(mut r: impl Read, mut f: impl FnMut(&Entry, &mut dyn Read) -> Result<()>) -> Result<()> {
    let field = |h: &[u8]| String::from_utf8_lossy(h.split(|b| *b == 0).next().unwrap_or_default()).into_owned();
    let (mut long_name, mut long_link): (Option<String>, Option<String>) = (None, None);
    let mut header = [0u8; 512];
    loop {
        if !read_block(&mut r, &mut header)? || header.iter().all(|b| *b == 0) {
            return Ok(());
        }
        let h = &header;
        let size = if h[124] & 0x80 != 0 {
            // base-256, for files of 8 GiB or more
            h[125..136].iter().fold(0u64, |n, b| (n << 8) | *b as u64)
        } else {
            u64::from_str_radix(field(&h[124..136]).trim(), 8).context("a broken tar header")?
        };
        let mut name = field(&h[..100]);
        if &h[257..262] == b"ustar" && h[345] != 0 {
            name = format!("{}/{name}", field(&h[345..500]));
        }
        let link = field(&h[157..257]);
        let mode = u32::from_str_radix(field(&h[100..108]).trim(), 8).unwrap_or(0o644);
        let kind = h[156];
        let mut data = (&mut r).take(size);
        match kind {
            // GNU's long name and link target: the next entry's.
            b'L' | b'K' => {
                let mut s = String::new();
                data.read_to_string(&mut s)?;
                let s = s.trim_end_matches('\0').to_string();
                if kind == b'L' { long_name = Some(s) } else { long_link = Some(s) }
            }
            // pax: `<len> key=value\n` records for the next entry.
            b'x' => {
                let mut buf = Vec::new();
                data.read_to_end(&mut buf)?;
                for rec in String::from_utf8_lossy(&buf).split('\n') {
                    let Some((_, kv)) = rec.split_once(' ') else { continue };
                    match kv.split_once('=') {
                        Some(("path", v)) => long_name = Some(v.to_string()),
                        Some(("linkpath", v)) => long_link = Some(v.to_string()),
                        _ => {}
                    }
                }
            }
            _ => {
                let path = long_name.take().unwrap_or(name);
                let link = long_link.take().unwrap_or(link);
                let kind = match kind {
                    b'0' | 0 | b'7' => Some(Kind::File { executable: mode & 0o111 != 0 }),
                    b'5' => Some(Kind::Dir),
                    b'2' => Some(Kind::Symlink(link)),
                    b'1' => Some(Kind::Hardlink(clean(&link))),
                    _ => None, // devices, fifos, global pax headers
                };
                let path = clean(&path);
                if let (Some(kind), false) = (kind, path.is_empty()) {
                    f(&Entry { path, kind, mode }, &mut data)?;
                }
            }
        }
        std::io::copy(&mut data, &mut std::io::sink())?;
        let pad = (512 - size % 512) % 512;
        std::io::copy(&mut (&mut r).take(pad), &mut std::io::sink())?;
    }
}

fn read_block(r: &mut impl Read, buf: &mut [u8; 512]) -> Result<bool> {
    let mut got = 0;
    while got < 512 {
        match r.read(&mut buf[got..])? {
            0 if got == 0 => return Ok(false),
            0 => bail!("a tar archive cut short"),
            n => got += n,
        }
    }
    Ok(true)
}

/// `./usr/bin/x` and `/usr/bin/x` are `usr/bin/x`.
pub(crate) fn clean(p: &str) -> String {
    p.trim_start_matches("./").trim_start_matches('/').trim_end_matches('/').to_string()
}

/// The file at `path` in a Debian package.
pub fn file(deb: &[u8], path: &str) -> Result<Vec<u8>> {
    let want = clean(path);
    let mut found = None;
    each_entry(tar_of(deb, "data")?, |e, r| {
        if found.is_none() && e.path == want && matches!(e.kind, Kind::File { .. }) {
            let mut v = Vec::new();
            r.read_to_end(&mut v)?;
            found = Some(v);
        }
        Ok(())
    })?;
    found.ok_or_else(|| anyhow!("no {want} in it"))
}

/// The paths of the files and links in a Debian package.
pub fn paths(deb: &[u8]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    each_entry(tar_of(deb, "data")?, |e, _| {
        if e.kind != Kind::Dir {
            out.push(e.path.clone());
        }
        Ok(())
    })?;
    Ok(out)
}

/// The package's control file: `Package:`, `Version:`, `Depends:`, ...
pub fn control(deb: &[u8]) -> Result<BTreeMap<String, String>> {
    let mut text = None;
    each_entry(tar_of(deb, "control")?, |e, r| {
        if e.path == "control" {
            let mut s = String::new();
            r.read_to_string(&mut s)?;
            text = Some(s);
        }
        Ok(())
    })?;
    Ok(parse_control(&text.ok_or_else(|| anyhow!("no control file in it"))?))
}

/// A Debian control stanza: `Key: value` lines; lines starting with a
/// space continue the one before.
pub fn parse_control(text: &str) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut last: Option<String> = None;
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(v) = last.as_ref().and_then(|k| out.get_mut(k)) {
                v.push('\n');
                v.push_str(line.trim());
            }
        } else if let Some((k, v)) = line.split_once(':') {
            out.insert(k.trim().to_string(), v.trim().to_string());
            last = Some(k.trim().to_string());
        }
    }
    out
}

/// Where a file of the package goes in a nixpkgs-style package: programs
/// in bin, libraries in lib, the rest of /usr as it is; /opt and /etc stay.
pub fn place(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    let rest = |n: usize| parts[n.min(parts.len())..].join("/");
    let under = |dir: &str, n: usize| {
        let r = rest(n);
        if r.is_empty() { dir.to_string() } else { format!("{dir}/{r}") }
    };
    match parts.as_slice() {
        ["usr", "local", ..] => place(&rest(2)),
        ["usr", "bin" | "sbin" | "games", ..] => under("bin", 2),
        ["bin" | "sbin", ..] => under("bin", 1),
        // (Debian's folder for the computer's kind: x86_64-linux-gnu, aarch64-linux-gnu.)
        ["usr" | "", "lib" | "lib64", m, ..] if *m == crate::arch::MULTIARCH => under("lib", 3),
        ["lib" | "lib64", m, ..] if *m == crate::arch::MULTIARCH => under("lib", 2),
        ["usr", "lib" | "lib64", ..] => under("lib", 2),
        ["lib64", ..] => under("lib", 1),
        ["usr", "libexec" | "share" | "include", ..] => rest(1),
        ["usr"] => String::new(),
        _ => parts.join("/"),
    }
}

/// `to` as a path relative to the directory `from` is in.
fn relative(from: &str, to: &str) -> String {
    let from: Vec<&str> = from.split('/').filter(|p| !p.is_empty()).collect();
    let from = &from[..from.len().saturating_sub(1)];
    let to: Vec<&str> = to.split('/').filter(|p| !p.is_empty()).collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&str> = vec![".."; from.len() - common];
    parts.extend(&to[common..]);
    if parts.is_empty() { ".".into() } else { parts.join("/") }
}

/// `a/b/../c` is `a/c`; None if it climbs out of the root.
fn normalize(path: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            p => out.push(p),
        }
    }
    Some(out.join("/"))
}

/// A symlink of the package, moved to `place(link)`: its target, moved too.
/// Targets outside the package stay as they are.
fn relink(link: &str, target: &str) -> String {
    let abs = if target.starts_with('/') {
        normalize(target)
    } else {
        normalize(&format!("{}/{target}", link.rsplit_once('/').map(|(d, _)| d).unwrap_or("")))
    };
    match abs {
        Some(abs) => relative(&place(link), &place(&abs)),
        None => target.to_string(),
    }
}

/// What unpacking found: programs, menu entries and their icons.
#[derive(Debug, Default)]
pub struct Unpacked {
    /// bin/ names.
    pub programs: Vec<String>,
    /// share/applications/*.desktop, with each one's Icon= (as rewritten).
    pub apps: Vec<(String, Option<String>)>,
    /// Paths its programs have built in that are its own (see [`hard_coded`]).
    pub redirects: Vec<String>,
}

/// Packages' files in `dest` (a new directory), laid out by [`place`]:
/// the first one's, then the others' (a file that's there already stays).
/// A package marked `true` gives only its libraries (lib/). Programs under
/// /opt get links in bin, menu entries point at programs and icons by name
/// instead of at /usr or /opt, and text files' paths into the package
/// point into it.
pub fn unpack(debs: &[(&[u8], bool)], dest: &Path) -> Result<Unpacked> {
    fs::create_dir_all(dest)?;
    for (deb, only_libs) in debs {
        extract(deb, dest, *only_libs)?;
    }
    finish(dest)
}

/// One package's files, laid out by [`place`], as they are.
fn extract(deb: &[u8], dest: &Path, only_libs: bool) -> Result<()> {
    let mut hardlinks = Vec::new();
    each_entry(tar_of(deb, "data")?, |e, r| {
        ensure!(e.path.split('/').all(|p| !p.is_empty() && p != ".." && p != "."), "{}: a path nami won't unpack", e.path);
        let to = place(&e.path);
        if to.is_empty() || (only_libs && !to.starts_with("lib/")) {
            return Ok(());
        }
        let at = dest.join(&to);
        safe_parent(dest, &to)?;
        if at.symlink_metadata().is_ok() {
            return Ok(()); // a merged /usr, or another package's: the first one wins
        }
        match &e.kind {
            Kind::Dir => fs::create_dir_all(&at)?,
            Kind::File { executable } => {
                let mut f = fs::File::create(&at)?;
                std::io::copy(r, &mut f)?;
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&at, fs::Permissions::from_mode(if *executable { 0o755 } else { 0o644 }))?;
            }
            Kind::Symlink(target) => std::os::unix::fs::symlink(relink(&e.path, target), &at)?,
            Kind::Hardlink(target) => hardlinks.push((to, place(target))),
        }
        Ok(())
    })?;
    for (to, from) in hardlinks {
        safe_parent(dest, &to)?;
        safe_parent(dest, &from)?;
        let (at, src) = (dest.join(&to), dest.join(&from));
        if at.symlink_metadata().is_err() && src.symlink_metadata().is_ok_and(|m| m.is_file()) {
            fs::copy(&src, &at)?;
        }
    }
    Ok(())
}

/// The unpacked files made to work from where they are.
fn finish(dest: &Path) -> Result<Unpacked> {
    remove_dangling(dest)?;

    // Menu entries, and links in bin for the programs they start.
    let mut out = Unpacked::default();
    let apps = dest.join("share/applications");
    let mut desktop_files: Vec<_> = fs::read_dir(&apps).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    desktop_files.sort();
    for name in desktop_files.iter().filter(|n| n.ends_with(".desktop")) {
        let path = apps.join(name);
        if !path.symlink_metadata().is_ok_and(|m| m.is_file()) {
            continue;
        }
        let text = fs::read_to_string(&path)?;
        let stem = name.trim_end_matches(".desktop");
        let fixed = rewrite_desktop(&text, dest, stem)?;
        if fixed != text {
            fs::write(&path, &fixed)?;
        }
        let icon = fixed.lines().find_map(|l| l.strip_prefix("Icon=")).map(String::from);
        out.apps.push((format!("share/applications/{name}"), icon));
    }
    // Programs only under /opt: a link in bin to each one at its top.
    if !dest.join("bin").exists() {
        let mut found = Vec::new();
        for app in fs::read_dir(dest.join("opt")).into_iter().flatten().flatten() {
            for dir in [app.path(), app.path().join("bin")] {
                for f in fs::read_dir(&dir).into_iter().flatten().flatten() {
                    let n = f.file_name().to_string_lossy().into_owned();
                    let meta = f.metadata()?;
                    use std::os::unix::fs::PermissionsExt;
                    if meta.is_file() && meta.permissions().mode() & 0o111 != 0 && !n.contains(".so") {
                        found.push((n, f.path().strip_prefix(dest).unwrap().display().to_string()));
                    }
                }
            }
        }
        for (n, rel) in found {
            link_program(dest, &n, &rel)?;
        }
    }
    // Text files (launcher scripts, settings, data): their paths into the
    // package as @out@ paths (the build puts in the real one), the way
    // nixpkgs' substituteInPlace does.
    for (path, meta) in files(dest)? {
        if !meta.is_file() || meta.len() > 4 << 20 {
            continue;
        }
        let data = fs::read(&path)?;
        if data.contains(&0) || !data.windows(5).any(|w| w == b"/usr/" || w == b"/opt/") {
            continue;
        }
        let Ok(text) = String::from_utf8(data) else { continue };
        let fixed = rewrite_script(&text, dest);
        if fixed != text {
            fs::write(&path, fixed)?;
        }
    }
    out.programs = fs::read_dir(dest.join("bin")).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    out.programs.sort();
    out.redirects = hard_coded(dest)?;
    Ok(out)
}

/// Every file, link and directory under `dir` (links not followed).
fn files(dir: &Path) -> Result<Vec<(std::path::PathBuf, fs::Metadata)>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)?.flatten() {
            let meta = e.path().symlink_metadata()?;
            if meta.is_dir() {
                stack.push(e.path());
            }
            out.push((e.path(), meta));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Directories in /usr/share and /usr/lib that many packages share: never
/// one app's own.
const SHARED: &[&str] = &[
    "applications", "icons", "pixmaps", "locale", "man", "doc", "doc-base", "info", "metainfo", "appdata", "mime",
    "glib-2.0", "dbus-1", "bash-completion", "zsh", "fish", "lintian", "menu", "polkit-1", "fonts", "themes",
    "sounds", "help", "gtk-doc", "pkgconfig", "X11", "xml", "sgml", "emacs", "vim", "gnome-shell", "gir-1.0",
    "thumbnailers", "xsessions", "wayland-sessions", "perl5", "perl", "java", "misc", "common-licenses", "keyrings",
    "zoneinfo", "terminfo", "i18n", "aclocal", "cmake", "systemd", "udev", "tmpfiles.d", "sysusers.d", "sysctl.d",
    "modules-load.d", "environment.d", "binfmt.d", "girepository-1.0", "gtk-2.0", "gtk-3.0", "gtk-4.0", "gio",
    "gdk-pixbuf-2.0", "dri", "debug", "os-release", "ssl", "jvm", "security", "mozilla", "firmware", "modules",
    "python3", "games", "plasma", "swcatalog", "dict", "hunspell", "bug", "debhelper", "x86_64-linux-gnu",
];

/// Paths under /usr and /opt that the package's programs and libraries
/// have built in and that are its own: `/usr/share/foo` when it has
/// share/foo (not a directory many packages share, like
/// /usr/share/locale), `/opt/Foo`, a program in /usr/bin it has. Where
/// they look for their files; the build points them at the package.
pub fn hard_coded(dest: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for (path, meta) in files(dest)? {
        let mut head = [0u8; 4];
        if !meta.is_file() || fs::File::open(&path)?.read(&mut head)? < 4 || &head != b"\x7fELF" {
            continue;
        }
        out.extend(crate::elf::mentioned_paths(&fs::read(&path)?).iter().filter_map(|p| own_path(p, dest)));
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// The start of absolute path `p` that's the package's own directory or
/// program, if it has it.
fn own_path(p: &str, dest: &Path) -> Option<String> {
    let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
    if parts.iter().any(|s| *s == "." || *s == "..") {
        return None;
    }
    let (depth, program) = match parts.as_slice() {
        ["usr", "bin" | "sbin" | "games", _] => (3, true),
        ["bin" | "sbin", _] => (2, true),
        ["usr", "lib", m, _, _, ..] if *m == crate::arch::MULTIARCH => (4, false),
        ["usr", "share" | "lib" | "libexec", _, ..] => (3, false),
        ["opt", _, ..] => (2, false),
        _ => return None,
    };
    let own = format!("/{}", parts[..depth].join("/"));
    let name = parts[depth - 1];
    if !program && (name.contains(".so") || (parts[0] == "usr" && (SHARED.contains(&name) || name == crate::arch::MULTIARCH))) {
        return None;
    }
    let at = dest.join(place(&own));
    let ok = if program { at.is_file() } else { at.is_dir() };
    ok.then_some(own)
}

/// Symlinks into other Ubuntu packages (`share/doc/foo/changelog.gz ->
/// ../libfoo1/...`) lead nowhere here: they go (nixpkgs' builds refuse
/// broken links).
fn remove_dangling(dest: &Path) -> Result<()> {
    loop {
        let mut gone = 0;
        let mut stack = vec![dest.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d)?.flatten() {
                let meta = e.metadata()?;
                if meta.is_dir() {
                    stack.push(e.path());
                } else if meta.file_type().is_symlink() {
                    let rel = e.path().strip_prefix(dest).unwrap().display().to_string();
                    let target = fs::read_link(e.path())?.display().to_string();
                    let parent = rel.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
                    let inside = !target.starts_with('/') && normalize(&format!("{parent}/{target}")).is_some();
                    if !inside || fs::metadata(e.path()).is_err() {
                        fs::remove_file(e.path())?;
                        gone += 1;
                    }
                }
            }
        }
        if gone == 0 {
            return Ok(());
        }
    }
}

/// Refuse to write through a symlink the package made (`a -> /etc`, then `a/passwd`).
fn safe_parent(dest: &Path, rel: &str) -> Result<()> {
    let mut at = dest.to_path_buf();
    let parts: Vec<&str> = rel.split('/').collect();
    for p in &parts[..parts.len() - 1] {
        at.push(p);
        match at.symlink_metadata() {
            Ok(m) if m.file_type().is_symlink() => bail!("{rel}: inside a symlink the package made"),
            Ok(_) => {}
            Err(_) => fs::create_dir_all(&at)?,
        }
    }
    Ok(())
}

/// bin/<name> -> `rel` (a path in the package), unless bin has a <name>.
fn link_program(dest: &Path, name: &str, rel: &str) -> Result<()> {
    let at = format!("bin/{name}");
    safe_parent(dest, &at)?;
    if dest.join(&at).symlink_metadata().is_err() {
        std::os::unix::fs::symlink(relative(&at, rel), dest.join(&at))?;
    }
    Ok(())
}

/// A script with `/opt/app/x` or `/usr/share/app/y` (files the package
/// has) as `@out@/opt/app/x` and `@out@/share/app/y`.
pub fn rewrite_script(text: &str, dest: &Path) -> String {
    let path_char = |c: char| c.is_ascii_alphanumeric() || "/._+-@".contains(c);
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find(['/']) {
        let boundary = rest[..i].chars().next_back().is_none_or(|c| !path_char(c));
        let end = rest[i..].find(|c: char| !path_char(c)).map(|e| i + e).unwrap_or(rest.len());
        let word = &rest[i..end];
        let ours = boundary
            && (word.starts_with("/opt/") || word.starts_with("/usr/"))
            && normalize(word).map(|p| place(&p)).is_some_and(|p| !p.is_empty() && dest.join(&p).symlink_metadata().is_ok());
        out.push_str(&rest[..i]);
        if ours {
            out.push_str(&format!("@out@/{}", place(&normalize(word).unwrap())));
        } else {
            out.push_str(word);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// A menu entry with `Exec=/usr/bin/x` as `Exec=x`, a program elsewhere in
/// the package (/opt/app/x) linked into bin, and `Icon=/usr/share/...png`
/// by name (icons outside share/icons and share/pixmaps are copied to
/// share/pixmaps as `<stem>`).
pub fn rewrite_desktop(text: &str, dest: &Path, stem: &str) -> Result<String> {
    let mut out = String::new();
    for line in text.lines() {
        let fixed = if let Some((key, value)) = line.split_once('=').filter(|(k, _)| *k == "Exec" || *k == "TryExec") {
            let (cmd, rest) = match value.strip_prefix('"') {
                Some(v) => match v.split_once('"') {
                    Some((c, r)) => (c, r),
                    None => (value, ""),
                },
                None => value.find(' ').map(|i| (&value[..i], &value[i..])).unwrap_or((value, "")),
            };
            match cmd.strip_prefix('/').and_then(normalize) {
                Some(abs) => {
                    let to = place(&abs);
                    let name = to.rsplit('/').next().unwrap_or(&to).to_string();
                    if dest.join(&to).symlink_metadata().is_ok() && !to.starts_with("bin/") {
                        link_program(dest, &name, &to)?;
                    }
                    format!("{key}={name}{rest}")
                }
                None => line.to_string(),
            }
        } else if let Some(icon) = line.strip_prefix("Icon=").filter(|i| i.starts_with('/')) {
            match normalize(icon).map(|a| place(&a)) {
                Some(to) if dest.join(&to).is_file() => {
                    let file = to.rsplit('/').next().unwrap_or(&to);
                    let (name, ext) = file.rsplit_once('.').unwrap_or((file, ""));
                    if to.starts_with("share/icons/") || to.starts_with("share/pixmaps/") {
                        format!("Icon={name}")
                    } else {
                        let copy = format!("share/pixmaps/{stem}.{ext}");
                        safe_parent(dest, &copy)?;
                        if dest.join(&copy).symlink_metadata().is_err() {
                            fs::copy(dest.join(&to), dest.join(&copy))?;
                        }
                        format!("Icon={stem}")
                    }
                }
                _ => line.to_string(),
            }
        } else {
            line.to_string()
        };
        out.push_str(&fixed);
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tar archive of `entries`: (path, type, link or contents, mode).
    pub fn tar(entries: &[(&str, u8, &[u8], u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (path, kind, data, mode) in entries {
            let mut h = [0u8; 512];
            h[..path.len()].copy_from_slice(path.as_bytes());
            h[100..107].copy_from_slice(format!("{mode:07o}").as_bytes());
            let size = if *kind == b'0' { data.len() } else { 0 };
            h[124..135].copy_from_slice(format!("{size:011o}").as_bytes());
            h[156] = *kind;
            if *kind == b'2' || *kind == b'1' {
                h[157..157 + data.len()].copy_from_slice(data);
            }
            h[257..262].copy_from_slice(b"ustar");
            h[148..156].copy_from_slice(b"        ");
            let sum: u32 = h.iter().map(|b| *b as u32).sum();
            h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
            out.extend_from_slice(&h);
            if *kind == b'0' {
                out.extend_from_slice(data);
                out.resize(out.len().div_ceil(512) * 512, 0);
            }
        }
        out.resize(out.len() + 1024, 0);
        out
    }

    /// A .deb with these files and this control file.
    pub fn deb(files: &[(&str, u8, &[u8], u32)], control_text: &str) -> Vec<u8> {
        let mut xz = Vec::new();
        xz2::read::XzEncoder::new(&tar(files)[..], 6).read_to_end(&mut xz).unwrap();
        let mut gz = Vec::new();
        flate2::read::GzEncoder::new(&tar(&[("./control", b'0', control_text.as_bytes(), 0o644)])[..], flate2::Compression::fast()).read_to_end(&mut gz).unwrap();
        let mut deb = b"!<arch>\n".to_vec();
        for (n, body) in [("debian-binary", b"2.0\n".to_vec()), ("control.tar.gz", gz), ("data.tar.xz", xz)] {
            deb.extend_from_slice(format!("{n:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n", 0, 0, 0, 100644, body.len()).as_bytes());
            deb.extend_from_slice(&body);
            if body.len() % 2 == 1 {
                deb.push(b'\n');
            }
        }
        deb
    }

    #[test]
    fn files_and_control() {
        let d = deb(&[("./usr/lib/shim/shimx64.efi.signed", b'0', b"shim!", 0o644)], "Package: shim\nVersion: 1:15.8-0ubuntu1\nDepends: libc6 (>= 2.34),\n libssl3\nDescription: boot\n loader\n");
        assert_eq!(file(&d, "usr/lib/shim/shimx64.efi.signed").unwrap(), b"shim!");
        assert!(file(&d, "usr/lib/shim/mmx64.efi.signed").is_err());
        let c = control(&d).unwrap();
        assert_eq!(c["Package"], "shim");
        assert_eq!(c["Depends"], "libc6 (>= 2.34),\nlibssl3");
    }

    #[test]
    fn places_files_like_nixpkgs() {
        assert_eq!(place("usr/bin/foo"), "bin/foo");
        assert_eq!(place("usr/sbin/foo"), "bin/foo");
        assert_eq!(place("usr/games/foo"), "bin/foo");
        let m = crate::arch::MULTIARCH;
        assert_eq!(place(&format!("usr/lib/{m}/libfoo.so.1")), "lib/libfoo.so.1");
        assert_eq!(place(&format!("lib/{m}/libfoo.so.1")), "lib/libfoo.so.1");
        assert_eq!(place("usr/lib/foo/helper"), "lib/foo/helper");
        assert_eq!(place("usr/share/applications/foo.desktop"), "share/applications/foo.desktop");
        assert_eq!(place("usr/local/bin/foo"), "bin/foo");
        assert_eq!(place("opt/Foo/foo"), "opt/Foo/foo");
        assert_eq!(place("etc/foo.conf"), "etc/foo.conf");
        assert_eq!(place("usr/share"), "share");
        assert_eq!(relink("usr/bin/foo", "/opt/Foo/foo"), "../opt/Foo/foo");
        assert_eq!(relink("usr/bin/foo", "../lib/foo/foo-bin"), "../lib/foo/foo-bin");
        assert_eq!(relink(&format!("usr/lib/{m}/libfoo.so"), "libfoo.so.1"), "libfoo.so.1");
        assert_eq!(relink("usr/bin/foo", "../../../../etc/x"), "../../../../etc/x");
    }

    #[test]
    fn unpacks_a_deb() {
        let lib = |f: &str| format!("./usr/lib/{}/{f}", crate::arch::MULTIARCH);
        let (bar, bar_link, bar_copy) = (lib("libbar.so.1"), lib("libbar.so"), lib("libbar-copy.so.1"));
        let desktop = b"[Desktop Entry]\nName=Foo\nExec=/opt/Foo/foo %U\nTryExec=/usr/bin/bar\nIcon=/opt/Foo/resources/foo.png\n\n[Desktop Action new]\nExec=\"/opt/Foo/foo\" --new\n";
        let d = deb(&[
            ("./", b'5', b"", 0o755),
            ("./opt/Foo/foo", b'0', b"\x7fELF", 0o755),
            ("./opt/Foo/libfoo.so", b'0', b"\x7fELF", 0o755),
            ("./opt/Foo/resources/foo.png", b'0', b"png", 0o644),
            ("./usr/share/applications/foo.desktop", b'0', desktop, 0o644),
            ("./usr/share/icons/hicolor/48x48/apps/bar.png", b'0', b"png", 0o644),
            (bar.as_str(), b'0', b"lib", 0o644),
            (bar_link.as_str(), b'2', b"libbar.so.1", 0o777),
            (bar_copy.as_str(), b'1', bar.as_bytes(), 0o644),
            ("./opt/Foo/evil", b'2', b"/etc", 0o777),
            ("./usr/share/foo-data/x", b'0', b"x", 0o644),
            ("./usr/share/doc/foo/changelog.gz", b'2', b"../libfoo1/changelog.gz", 0o777),
            ("./usr/bin/foo-launch", b'0', b"#!/bin/sh\nexec /opt/Foo/foo --data=/usr/share/foo-data \"$@\" /usr/lib/missing x/opt/Foo/foo\n", 0o755),
        ], "Package: foo\n");
        let dir = std::env::temp_dir().join(format!("nami-deb-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let got = unpack(&[(&d, false)], &dir).unwrap();
        assert_eq!(fs::read_link(dir.join("bin/foo")).unwrap().to_str(), Some("../opt/Foo/foo"));
        assert_eq!(fs::read_link(dir.join("lib/libbar.so")).unwrap().to_str(), Some("libbar.so.1"));
        assert_eq!(fs::read(dir.join("lib/libbar-copy.so.1")).unwrap(), b"lib");
        assert!(dir.join("bin/foo").exists() && !dir.join("bin/libfoo.so").exists());
        // Links into other packages go.
        assert!(dir.join("share/doc/foo/changelog.gz").symlink_metadata().is_err());
        assert!(dir.join("opt/Foo/evil").symlink_metadata().is_err());
        let text = fs::read_to_string(dir.join("share/applications/foo.desktop")).unwrap();
        assert!(text.contains("\nExec=foo %U\n"), "{text}");
        assert!(text.contains("\nTryExec=bar\n"), "{text}");
        assert!(text.contains("\nExec=foo --new\n"), "{text}");
        assert!(text.contains("\nIcon=foo\n"), "{text}");
        assert_eq!(fs::read(dir.join("share/pixmaps/foo.png")).unwrap(), b"png");
        assert_eq!(
            fs::read_to_string(dir.join("bin/foo-launch")).unwrap(),
            "#!/bin/sh\nexec @out@/opt/Foo/foo --data=@out@/share/foo-data \"$@\" /usr/lib/missing x/opt/Foo/foo\n"
        );
        assert_eq!(got.programs, vec!["foo", "foo-launch"]);
        assert_eq!(got.apps, vec![("share/applications/foo.desktop".to_string(), Some("foo".to_string()))]);
        fs::remove_dir_all(&dir).unwrap();

        // Writing through a symlink the package made is refused.
        let d = deb(&[("./usr/share/x", b'2', b"/etc", 0o777), ("./usr/share/x/passwd", b'0', b"evil", 0o644)], "");
        assert!(unpack(&[(&d, false)], &dir).is_err());
        let _ = fs::remove_dir_all(&dir);
        let d = deb(&[("./usr/../../etc/passwd", b'0', b"evil", 0o644)], "");
        assert!(unpack(&[(&d, false)], &dir).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unpacks_an_app_with_its_companions() {
        // The program, its data (another package), and a library taken from a third.
        let program = b"\x7fELF\0\0/usr/share/foo/data.csv\0/usr/share/locale\0/usr/lib/foo/plugins\0/usr/bin/foo-helper\0/usr/share/elsewhere/x\0/opt/Foo/\0";
        let app = deb(&[
            ("./usr/bin/foo", b'0', program, 0o755),
            ("./usr/bin/foo-helper", b'0', b"#!/bin/sh\ncat /usr/share/foo/data.csv\n", 0o755),
            ("./usr/lib/foo/plugins/a.so", b'0', b"\x7fELF", 0o644),
            ("./usr/share/locale/de/LC_MESSAGES/foo.mo", b'0', b"mo", 0o644),
        ], "Package: foo\n");
        let data = deb(&[
            ("./usr/share/foo/data.csv", b'0', b"a,b\n", 0o644),
            ("./usr/share/foo/settings.json", b'0', b"{\"helper\": \"/usr/bin/foo-helper\", \"other\": \"/usr/bin/sh\"}\n", 0o644),
            ("./usr/bin/foo", b'0', b"not the program", 0o755),
        ], "Package: foo-data\n");
        let (bar, bar_link) = (format!("./usr/lib/{}/libbar.so.74.2", crate::arch::MULTIARCH), format!("./usr/lib/{}/libbar.so.74", crate::arch::MULTIARCH));
        let lib = deb(&[
            (bar.as_str(), b'0', b"\x7fELF", 0o644),
            (bar_link.as_str(), b'2', b"libbar.so.74.2", 0o777),
            ("./usr/share/doc/libbar74/copyright", b'0', b"(c)", 0o644),
        ], "Package: libbar74\n");
        let dir = std::env::temp_dir().join(format!("nami-deb-test-companions-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let got = unpack(&[(&app, false), (&data, false), (&lib, true)], &dir).unwrap();
        // The first package's file wins; a library package gives only its libraries.
        assert_eq!(fs::read(dir.join("bin/foo")).unwrap(), program);
        assert_eq!(fs::read(dir.join("share/foo/data.csv")).unwrap(), b"a,b\n");
        assert_eq!(fs::read_link(dir.join("lib/libbar.so.74")).unwrap().to_str(), Some("libbar.so.74.2"));
        assert!(!dir.join("share/doc/libbar74").exists());
        // Text files anywhere point into the package.
        assert_eq!(fs::read_to_string(dir.join("bin/foo-helper")).unwrap(), "#!/bin/sh\ncat @out@/share/foo/data.csv\n");
        assert_eq!(fs::read_to_string(dir.join("share/foo/settings.json")).unwrap(), "{\"helper\": \"@out@/bin/foo-helper\", \"other\": \"/usr/bin/sh\"}\n");
        // What the program has built in: its own directories and programs, not shared ones.
        assert_eq!(got.redirects, vec!["/usr/bin/foo-helper", "/usr/lib/foo", "/usr/share/foo"]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
