//! Programs built for other distributions: a downloaded app's (VS Code's, Discord's,
//! Zed's), an AppImage, a game, a prebuilt program from GitHub, npm or pip.
//! They ask for their loader at /lib64/ld-linux-x86-64.so.2 (on ARM, /lib/ld-linux-aarch64.so.1), which on a
//! computer nami made is nix-ld: it starts them with the system's own glibc
//! and has the loader find their libraries in a set made for them (the
//! packages' `libraries:`, in /run/current-system/sw/share/nix-ld/lib), then
//! in /run/opengl-driver/lib and in the person's own set
//! (~/.local/state/nami/libraries). The system's programs never look there.
//!
//! What's here: which libraries a program lacks and which packages have them,
//! in words (`nami libraries`, also after one fails in a terminal), adding
//! them to the person's set (`--add`), starting one as Files does when it's
//! opened (`nami start`: made a program first if it isn't one, an AppImage
//! unpacked when FUSE won't do), and putting one in the launcher
//! (`nami add-to-launcher`).

use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::io::{IsTerminal, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::index::Index;
use crate::store::Store;

/// Where programs built for other distributions ask for their loader.
pub const LOADER: &str = crate::arch::LOADER;
/// The system's set: nix-ld's own default, and its loader (`ld.so`).
const SYSTEM_SET: &str = "/run/current-system/sw/share/nix-ld/lib";
/// nami's own program, as launchers name it: /nami/bin/nami.
fn nami_program() -> PathBuf {
    crate::migrate::here().join("bin/nami")
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// The person's own set: links to the libraries they added.
pub fn personal_set() -> PathBuf {
    home().join(".local/state/nami/libraries")
}

/// Where the loader looks for such a program's libraries, as nix-ld has it
/// look: NIX_LD_LIBRARY_PATH (the login sets it: the system's set, the
/// graphics driver's, the person's), else nix-ld's default; then
/// LD_LIBRARY_PATH.
pub(crate) fn search_path() -> Vec<PathBuf> {
    let given = std::env::var(format!("NIX_LD_LIBRARY_PATH_{}_linux", crate::arch::ARCH)).ok().or_else(|| std::env::var("NIX_LD_LIBRARY_PATH").ok()).filter(|s| !s.is_empty());
    let mut dirs: Vec<PathBuf> = match given {
        Some(list) => list.split(':').filter(|d| !d.is_empty()).map(PathBuf::from).collect(),
        None => vec![PathBuf::from(SYSTEM_SET)],
    };
    if let Ok(more) = std::env::var("LD_LIBRARY_PATH") {
        dirs.extend(more.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    }
    dirs
}

/// glibc's folder (its loader's), where the loader looks last: the set's
/// ld.so's, or (without nix-ld) /lib64's own.
pub(crate) fn glibc_dir() -> Option<PathBuf> {
    [format!("{SYSTEM_SET}/ld.so"), LOADER.to_string()]
        .iter()
        .filter_map(|l| std::fs::canonicalize(l).ok()?.parent().map(Path::to_path_buf))
        .find(|d| d.join("libc.so.6").exists())
}

/// A file mapped into memory, read only: a program can be hundreds of MB
/// (Electron's), of which only the headers are read.
struct Mapped {
    ptr: *mut libc::c_void,
    len: usize,
}

impl Mapped {
    fn open(path: &Path) -> Result<Mapped> {
        let f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let len = f.metadata()?.len() as usize;
        if len == 0 {
            return Ok(Mapped { ptr: std::ptr::null_mut(), len: 0 });
        }
        use std::os::fd::AsRawFd;
        // SAFETY: a private read-only mapping of a file we opened; unmapped in drop.
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_PRIVATE, f.as_raw_fd(), 0) };
        anyhow::ensure!(ptr != libc::MAP_FAILED, "reading {}: {}", path.display(), std::io::Error::last_os_error());
        Ok(Mapped { ptr, len })
    }

    fn bytes(&self) -> &[u8] {
        if self.len == 0 {
            return &[];
        }
        // SAFETY: the mapping is `len` bytes and lives as long as self.
        unsafe { std::slice::from_raw_parts(self.ptr as *const u8, self.len) }
    }
}

impl Drop for Mapped {
    fn drop(&mut self) {
        if self.len > 0 {
            // SAFETY: what mmap gave in open.
            unsafe { libc::munmap(self.ptr, self.len) };
        }
    }
}

/// What a file is, to start it.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Kind {
    /// A program built for another distribution: it asks for /lib64's loader.
    Foreign,
    /// An AppImage (a program with its own files inside).
    AppImage,
    /// One of this computer's own programs (its loader is in the store), or
    /// one that needs no loader (static).
    Native,
    /// A 32-bit program.
    Bits32,
    /// A library, which programs load.
    Library,
    /// A script (`#!`).
    Script,
    /// A Windows program.
    Windows,
    /// Anything else.
    Other,
}

/// The loader a 64-bit ELF file asks for (PT_INTERP), if any.
fn interpreter(data: &[u8]) -> Option<String> {
    let u16_at = |o: usize| data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let u64_at = |o: usize| data.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()) as usize);
    let (phoff, size, count) = (u64_at(0x20)?, u16_at(0x36)?, u16_at(0x38)?);
    for i in 0..count {
        let h = phoff + i * size;
        if u32_at(h)? == 3 {
            let (at, len) = (u64_at(h + 8)?, u64_at(h + 32)?);
            return Some(String::from_utf8_lossy(data.get(at..at + len)?).trim_end_matches('\0').to_string());
        }
    }
    None
}

pub fn kind_of(data: &[u8]) -> Kind {
    if data.starts_with(b"MZ") {
        return Kind::Windows;
    }
    if data.starts_with(b"#!") {
        return Kind::Script;
    }
    if data.len() < 64 || &data[..4] != b"\x7fELF" {
        return Kind::Other;
    }
    // AppImage's mark: "AI" and its type, after the ELF magic.
    if &data[8..10] == b"AI" && matches!(data[10], 1 | 2) {
        return Kind::AppImage;
    }
    if data[4] == 1 {
        return Kind::Bits32;
    }
    match interpreter(data) {
        Some(i) if i == LOADER => Kind::Foreign,
        Some(_) => Kind::Native,
        None => {
            // A static program, or a library (one that links to others, or has a name).
            let elf_type = u16::from_le_bytes([data[16], data[17]]);
            match crate::elf::linking(data) {
                Some(l) if elf_type == 3 && (l.soname.is_some() || !l.needed.is_empty()) => Kind::Library,
                _ => Kind::Native,
            }
        }
    }
}

pub fn kind(path: &Path) -> Result<Kind> {
    Ok(kind_of(Mapped::open(path)?.bytes()))
}

/// A library a program can't find: its name, and the file that asks for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Lack {
    pub soname: String,
    pub by: PathBuf,
}

fn is_elf64(path: &Path) -> bool {
    let mut head = [0u8; 5];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && &head[..4] == b"\x7fELF" && head[4] == 2
}

/// The libraries `program` (and the libraries it loads) can't find, looked
/// for as the loader does: a file's RPATH (and the program's), the search
/// path, its RUNPATH, then glibc's own. `$ORIGIN` is the file's folder.
pub fn missing(program: &Path) -> Result<Vec<Lack>> {
    missing_in(program, &search_path(), glibc_dir().as_deref())
}

pub(crate) fn missing_in(program: &Path, path: &[PathBuf], glibc: Option<&Path>) -> Result<Vec<Lack>> {
    let main = std::fs::canonicalize(program).with_context(|| format!("{}", program.display()))?;
    let expand = |list: &Option<String>, origin: &Path| -> Vec<PathBuf> {
        let origin = origin.to_string_lossy();
        list.as_deref()
            .unwrap_or_default()
            .split(':')
            .filter(|d| !d.is_empty())
            .map(|d| PathBuf::from(d.replace("${ORIGIN}", &origin).replace("$ORIGIN", &origin).replace("${LIB}", "lib64").replace("$LIB", "lib64").replace("${PLATFORM}", crate::arch::ARCH).replace("$PLATFORM", crate::arch::ARCH)))
            .collect()
    };
    let mut main_rpath = Vec::new();
    let (mut seen, mut out, mut queue, mut looked) = (HashSet::new(), Vec::new(), VecDeque::from([main.clone()]), 0);
    while let Some(file) = queue.pop_front() {
        looked += 1;
        if looked > 3000 {
            break;
        }
        let Ok(map) = Mapped::open(&file) else { continue };
        let Some(l) = crate::elf::linking(map.bytes()) else { continue };
        let origin = file.parent().unwrap_or(Path::new("/"));
        let mut dirs = Vec::new();
        if l.runpath.is_none() {
            dirs.extend(expand(&l.rpath, origin));
            if file == main {
                main_rpath = dirs.clone();
            } else {
                dirs.extend(main_rpath.iter().cloned());
            }
        }
        dirs.extend(path.iter().cloned());
        dirs.extend(expand(&l.runpath, origin));
        dirs.extend(glibc.map(Path::to_path_buf));
        for n in l.needed {
            if !seen.insert(n.clone()) {
                continue;
            }
            let found = if n.contains('/') { Some(PathBuf::from(&n)).filter(|f| f.exists()) } else { dirs.iter().map(|d| d.join(&n)).find(|f| is_elf64(f)) };
            match found {
                Some(f) => queue.push_back(f),
                None => out.push(Lack { soname: n, by: file.clone() }),
            }
        }
    }
    Ok(out)
}

/// How a person would call the program in `file`: `Obsidian` for
/// Obsidian-1.5.12.AppImage, the file's name otherwise.
pub fn display_name(file: &Path) -> String {
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let Some(stem) = name.strip_suffix(".AppImage").or_else(|| name.strip_suffix(".appimage")) else { return name };
    // Up to a version (-1.2.3, _1.2, -v1.2) or the machine (-x86_64).
    let parts: Vec<&str> = stem.split(['-', '_']).collect();
    let mut kept = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        let version = p.trim_start_matches(['v', 'V']).starts_with(|c: char| c.is_ascii_digit());
        if i > 0 && (version || ["x86", "x86-64", "x86_64", "amd64", "linux", "Linux", "x64"].contains(p)) {
            break;
        }
        kept.push(*p);
    }
    let out = kept.join(" ");
    if out.is_empty() { name } else { out }
}

/// The packages with these libraries, from the catalog (and the binary
/// cache's listings of what nixpkgs builds have).
fn packages_for(root: &Path, lacks: &[String]) -> Vec<(String, Option<String>)> {
    let found = Store::open_direct_readonly(root).ok().and_then(|s| Index::load(&s).ok().map(|i| (s, i)));
    lacks
        .iter()
        .map(|soname| {
            let pkg = found.as_ref().and_then(|(store, index)| crate::recipe::package_with_library(store, index, soname));
            (soname.clone(), pkg)
        })
        .collect()
}

/// What a program lacks, in words, one line each.
fn lack_lines(found: &[(String, Option<String>)]) -> Vec<String> {
    found
        .iter()
        .map(|(soname, pkg)| match pkg {
            Some(p) => format!("{soname}, which the package {p} has"),
            None => format!("{soname}, which no package in the catalog has (it may come with the program, in a folder of its own)"),
        })
        .collect()
}

/// Programs to look at for `file` once it failed to start: itself, or for
/// a script (VS Code's bin/code), the programs for other distributions beside it
/// and in the folder above.
fn candidates(file: &Path) -> Vec<PathBuf> {
    match kind(file) {
        Ok(Kind::Foreign) => vec![file.to_path_buf()],
        Ok(Kind::Script) => {
            let mut out = Vec::new();
            let dir = file.parent().unwrap_or(Path::new("."));
            for d in [Some(dir), dir.parent()].into_iter().flatten() {
                let Ok(entries) = std::fs::read_dir(d) else { continue };
                let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
                files.sort();
                for f in files.into_iter().take(80) {
                    if f.metadata().is_ok_and(|m| m.permissions().mode() & 0o111 != 0) && kind(&f).is_ok_and(|k| k == Kind::Foreign) {
                        out.push(f);
                    }
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

/// `nami libraries <program>`: what it lacks and which packages have it;
/// with `add`, those packages join the person's set. `after_failure`: a
/// terminal's program just failed to start (127): said only if it's for
/// another Linux and lacks something.
pub fn libraries(root: &Path, program: &Path, add: bool, after_failure: bool) -> Result<i32> {
    let program = if program.exists() { program.to_path_buf() } else { which(program).unwrap_or_else(|| program.to_path_buf()) };
    if after_failure && matches!(kind(&program), Ok(Kind::Bits32)) {
        eprintln!("namiOS: {} is a 32-bit program; namiOS starts 64-bit programs built for other distributions (a 64-bit build of it would start)", program.display());
        return Ok(0);
    }
    let files = if after_failure { candidates(&program) } else { vec![program.clone()] };
    if !after_failure {
        match kind(&program)? {
            Kind::Foreign => {}
            Kind::AppImage => bail!("{} is an AppImage: its libraries are inside it; start it with `nami start {}`, which says what it lacks", program.display(), program.display()),
            Kind::Native => {
                println!("{}", crate::style::out().line(&format!("✓ {} is one of this computer's own programs (or needs no libraries) · nothing to add", program.display())));
                return Ok(0);
            }
            k => bail!("{} isn't a 64-bit program for another distribution ({})", program.display(), describe(k)),
        }
    }
    let mut lacks: Vec<String> = Vec::new();
    for f in &files {
        for l in missing(f)? {
            if !lacks.contains(&l.soname) {
                lacks.push(l.soname);
            }
        }
    }
    let name = display_name(&program);
    if lacks.is_empty() {
        if !after_failure {
            println!("{}", crate::style::out().line(&format!("✓ {name} has all the libraries it needs")));
        }
        return Ok(0);
    }
    let found = packages_for(root, &lacks);
    let lines = lack_lines(&found);
    eprintln!("namiOS: {name} is built for another distribution, and needs {} this computer doesn't have:", if lines.len() == 1 { "a library" } else { "libraries" });
    for l in &lines {
        eprintln!("  {l}");
    }
    let addable: Vec<(String, String)> = found.iter().filter_map(|(s, p)| Some((s.clone(), p.clone()?))).collect();
    if add {
        if addable.is_empty() {
            bail!("no package has them: nothing to add");
        }
        add_libraries(&addable)?;
        eprintln!("{}", crate::style::err().line(&format!("✓ Added · {name} finds {} now", if addable.len() == 1 { "it" } else { "them" })));
        return Ok(0);
    }
    if !addable.is_empty() {
        let arg = shell_quote(&program.to_string_lossy());
        eprintln!("{}", crate::style::err().line(&format!("hint: add {} for your programs with `nami libraries --add {arg}`", if addable.len() == 1 { "it" } else { "them" })));
    }
    Ok(1)
}

fn describe(k: Kind) -> &'static str {
    match k {
        Kind::Foreign => "a program built for another distribution",
        Kind::AppImage => "an AppImage",
        Kind::Native => "one of this computer's own programs",
        Kind::Bits32 => "a 32-bit program",
        Kind::Library => "a library, which programs load: not one to start",
        Kind::Script => "a script",
        Kind::Windows => "a Windows program",
        Kind::Other => "not a program",
    }
}

pub(crate) fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+~".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// A command found on PATH.
fn which(name: &Path) -> Option<PathBuf> {
    if name.components().count() > 1 {
        return None;
    }
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|f| f.is_file()))
}

/// The packages join the person's own file (~/user.nami), which is applied;
/// then their libraries join the person's set, by name.
pub(crate) fn add_libraries(addable: &[(String, String)]) -> Result<()> {
    let nami = std::env::current_exe().ok().filter(|p| p.exists()).unwrap_or_else(nami_program);
    let file = crate::config::user_file();
    if !file.exists() {
        std::fs::write(&file, "packages: []\n").with_context(|| format!("writing {}", file.display()))?;
    }
    let mut pkgs: Vec<&str> = addable.iter().map(|(_, p)| p.as_str()).collect();
    pkgs.dedup();
    let listed = crate::config::listed_packages(&std::fs::read_to_string(&file).unwrap_or_default());
    for p in &pkgs {
        if listed.iter().any(|(have, _)| have == p) {
            continue;
        }
        let ok = Command::new(&nami).args(["config", "add", "packages", p, "--file"]).arg(&file).stdout(Stdio::null()).status()?.success();
        anyhow::ensure!(ok, "couldn't add {p} to {}", file.display());
    }
    let ok = Command::new(&nami).arg("apply").arg(&file).status()?.success();
    anyhow::ensure!(ok, "applying {} failed", file.display());
    let set = personal_set();
    std::fs::create_dir_all(&set)?;
    let profile = home().join(".local/state/nami/profile/lib");
    for (soname, pkg) in addable {
        let target = profile.join(soname);
        anyhow::ensure!(target.exists(), "{pkg} is yours now, but {soname} isn't in your profile's lib/ ({})", profile.display());
        let link = set.join(soname);
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&target, &link)?;
    }
    Ok(())
}

/// A notification (the desktop's), with buttons: the key of the one
/// pressed, waiting until it's answered or goes. None without a desktop.
fn notify(summary: &str, body: &str, icon: &str, actions: &[(&str, &str)]) -> Option<String> {
    std::env::var_os("WAYLAND_DISPLAY").or_else(|| std::env::var_os("DISPLAY"))?;
    let mut c = Command::new("notify-send");
    c.args(["--app-name=namiOS", &format!("--icon={icon}")]);
    for (key, label) in actions {
        c.arg(format!("--action={key}={label}"));
    }
    if !actions.is_empty() {
        c.arg("--wait");
    }
    let out = c.arg(summary).arg(body).stderr(Stdio::null()).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Said to whoever started it: in the terminal, or (from Files or the
/// launcher) as a notification.
fn tell(summary: &str, body: &str) {
    eprintln!("{summary}: {body}");
    if !std::io::stderr().is_terminal() {
        notify(summary, body, "dialog-warning", &[]);
    }
}

/// Ask whether to go on: in the terminal, or with a dialog.
fn ask(title: &str, text: &str, yes: &str) -> bool {
    // (Nobody may be asked: no.)
    if crate::errors::noninteractive() {
        eprintln!("{text} No: NAMI_NONINTERACTIVE is set.");
        return false;
    }
    if std::io::stdin().is_terminal() && std::io::stderr().is_terminal() {
        eprint!("{text} [y/N] ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        return std::io::stdin().read_line(&mut line).is_ok() && line.trim().eq_ignore_ascii_case("y");
    }
    Command::new("zenity")
        .args(["--question", "--title", title, "--text", text, "--ok-label", yes, "--cancel-label", "Cancel", "--icon=application-x-executable"])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `nami start <file>`: what Files does when a program made for another
/// Linux is opened (and the launcher's entries for one): it's made a program
/// first if it isn't one yet (asked: a download isn't), what it lacks is said
/// (with Add, as a notification), and an AppImage that FUSE can't mount is
/// unpacked to run.
pub fn start(root: &Path, file: &Path, args: &[OsString]) -> Result<i32> {
    let file = std::fs::canonicalize(file).with_context(|| format!("{}", file.display()))?;
    anyhow::ensure!(file.is_file(), "{} isn't a file", file.display());
    let name = display_name(&file);
    let k = kind(&file)?;
    match k {
        Kind::Bits32 | Kind::Library | Kind::Windows | Kind::Other => {
            let more = match k {
                Kind::Bits32 => "namiOS starts 64-bit programs built for other distributions; look for a 64-bit (x86_64) download of it",
                Kind::Windows => "it's a Windows program: it runs with Wine or Bottles, from the catalog",
                _ => describe(k),
            };
            tell(&format!("{name} can't start"), more);
            return Ok(126);
        }
        _ => {}
    }
    let mode = file.metadata()?.permissions().mode();
    if mode & 0o100 == 0 {
        let text = format!("“{name}” isn't marked as a program yet, as with anything downloaded. Start only programs from people you trust.\n\nMark it as a program and start it?");
        if !ask(&format!("Start {name}?"), &text, "Start") {
            return Ok(1);
        }
        // Runnable by whoever may read it.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode | 0o100 | ((mode & 0o044) >> 2)))
            .with_context(|| format!("making {} a program", file.display()))?;
    }
    if k == Kind::Foreign {
        let lacks: Vec<String> = missing(&file)?.into_iter().map(|l| l.soname).collect();
        if !lacks.is_empty() {
            return lacking(root, &file, &name, &lacks, args);
        }
    }
    let gui = !std::io::stderr().is_terminal();
    if k != Kind::AppImage && !gui {
        // In a terminal: it's simply the program.
        return Err(Command::new(&file).args(args).exec()).with_context(|| format!("starting {}", file.display()));
    }
    let mut run = Watched::start(&file, args, &[])?;
    if k == Kind::AppImage && run.ended_badly() && run.says_fuse() {
        eprintln!("namiOS: FUSE couldn't mount {name}: unpacking it to start it instead");
        run = Watched::start(&file, args, &[("APPIMAGE_EXTRACT_AND_RUN", "1")])?;
    }
    if let Some(soname) = run.lacks() {
        return lacking(root, &file, &name, &[soname], args);
    }
    if run.running() && gui && (k == Kind::AppImage || launcher_entry(&file).is_some()) {
        let f = file.clone();
        std::thread::spawn(move || offer_launcher(&f));
    }
    Ok(run.wait())
}

/// It lacks these: said, with Add (which adds them to the person's set and
/// starts it again).
fn lacking(root: &Path, file: &Path, name: &str, lacks: &[String], args: &[OsString]) -> Result<i32> {
    let found = packages_for(root, lacks);
    let lines = lack_lines(&found);
    let addable: Vec<(String, String)> = found.iter().filter_map(|(s, p)| Some((s.clone(), p.clone()?))).collect();
    let summary = match lacks {
        [one] => format!("{name} needs {one}"),
        _ => format!("{name} needs {} libraries", lacks.len()),
    };
    eprintln!("namiOS: {name} is built for another distribution, and needs what this computer doesn't have:");
    for l in &lines {
        eprintln!("  {l}");
    }
    let terminal = std::io::stderr().is_terminal();
    if terminal {
        if !addable.is_empty() {
            eprintln!("{}", crate::style::err().line(&format!("hint: add {} for your programs with `nami libraries --add {}`", if addable.len() == 1 { "it" } else { "them" }, shell_quote(&file.to_string_lossy()))));
        }
        return Ok(127);
    }
    // One: the summary names it, the body where it comes from.
    let body = match found.as_slice() {
        [(_, Some(p))] => format!("It's built for another distribution. The package {p} has it: Add puts it with your programs' libraries."),
        [(_, None)] => "It's built for another distribution, and no package in the catalog has it.".to_string(),
        _ => lines.join("\n"),
    };
    let actions: &[(&str, &str)] = if addable.is_empty() { &[] } else { &[("add", "Add")] };
    if notify(&summary, &body, "dialog-warning", actions).as_deref() == Some("add") {
        notify(&format!("Adding what {name} needs"), &addable.iter().map(|(_, p)| p.as_str()).collect::<Vec<_>>().join(", "), "system-software-install", &[]);
        match add_libraries(&addable) {
            Ok(()) => return start(root, file, args),
            Err(e) => {
                notify(&format!("Couldn't add what {name} needs"), &format!("{e:#}"), "dialog-error", &[]);
                return Err(e);
            }
        }
    }
    Ok(127)
}

/// A program started with its error output watched for the first seconds
/// (and passed on): what the loader or an AppImage's FUSE said.
struct Watched {
    child: std::process::Child,
    said: Arc<Mutex<String>>,
    reader: Option<std::thread::JoinHandle<()>>,
    status: Option<std::process::ExitStatus>,
}

impl Watched {
    fn start(file: &Path, args: &[OsString], env: &[(&str, &str)]) -> Result<Watched> {
        let mut child = Command::new(file).args(args).envs(env.iter().copied()).stdin(Stdio::null()).stderr(Stdio::piped()).spawn().with_context(|| format!("starting {}", file.display()))?;
        let said = Arc::new(Mutex::new(String::new()));
        let mut err = child.stderr.take().expect("piped");
        let keep = said.clone();
        let reader = std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            let mut out = std::io::stderr();
            while let Ok(n) = err.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let _ = out.write_all(&buf[..n]);
                if let Ok(mut s) = keep.lock() {
                    if s.len() < 64 * 1024 {
                        s.push_str(&String::from_utf8_lossy(&buf[..n]));
                    }
                }
            }
        });
        let mut w = Watched { child, said, reader: Some(reader), status: None };
        let until = Instant::now() + Duration::from_secs(6);
        while Instant::now() < until {
            if let Some(s) = w.child.try_wait()? {
                w.status = Some(s);
                if let Some(r) = w.reader.take() {
                    let _ = r.join();
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(w)
    }

    fn running(&self) -> bool {
        self.status.is_none()
    }

    fn ended_badly(&self) -> bool {
        self.status.is_some_and(|s| !s.success())
    }

    fn text(&self) -> String {
        self.said.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// An AppImage's runtime couldn't use FUSE.
    fn says_fuse(&self) -> bool {
        let t = self.text().to_lowercase();
        ["fuse", "fusermount", "cannot mount", "squashfuse"].iter().any(|w| t.contains(w))
    }

    /// The library the loader couldn't find, if that's why it ended.
    fn lacks(&self) -> Option<String> {
        if !self.ended_badly() {
            return None;
        }
        loader_lacks(&self.text())
    }

    fn wait(mut self) -> i32 {
        let status = match self.status {
            Some(s) => s,
            None => match self.child.wait() {
                Ok(s) => s,
                Err(_) => return 1,
            },
        };
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
        status.code().unwrap_or(128)
    }
}

/// The library named in the loader's "error while loading shared
/// libraries: libfoo.so.1: cannot open shared object file".
fn loader_lacks(text: &str) -> Option<String> {
    let after = text.split("error while loading shared libraries: ").nth(1)?;
    let name = after.split(':').next()?.trim();
    (!name.is_empty() && name.contains(".so")).then(|| name.to_string())
}

/// Where the launcher's entry for `file` goes (and whether one is there).
fn entry_path(file: &Path) -> PathBuf {
    home().join(".local/share/applications").join(format!("{}.desktop", entry_id(file)))
}

fn entry_id(file: &Path) -> String {
    let slug: String = display_name(file).chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    format!("nami-{}", slug.trim_matches('-'))
}

/// Offered once per program, after its first start from Files.
fn offer_launcher(file: &Path) {
    if entry_path(file).exists() {
        return;
    }
    let asked = home().join(".local/state/nami/launcher-offered");
    let list = std::fs::read_to_string(&asked).unwrap_or_default();
    let key = file.to_string_lossy();
    if list.lines().any(|l| l == key) {
        return;
    }
    let _ = std::fs::create_dir_all(asked.parent().unwrap());
    let _ = std::fs::write(&asked, format!("{list}{key}\n"));
    let name = display_name(file);
    if notify(&format!("Keep {name} in the launcher?"), "Then it's with your other apps, to start from there or keep in the dock.", "application-x-executable", &[("launcher", "Add to Launcher")]).as_deref() == Some("launcher") {
        if let Err(e) = add_to_launcher(file) {
            notify(&format!("Couldn't add {name} to the launcher"), &format!("{e:#}"), "dialog-error", &[]);
        }
    }
}

/// A desktop entry's keys, in its [Desktop Entry] group.
pub(crate) fn entry_keys(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim_end();
        if line.starts_with('[') {
            inside = line == "[Desktop Entry]";
            continue;
        }
        if !inside || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    out
}

pub(crate) fn key<'a>(keys: &'a [(String, String)], name: &str) -> Option<&'a str> {
    keys.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// A program's own desktop entry, if its download has one: beside it, in
/// the folder above, or in a share/applications there.
fn launcher_entry(file: &Path) -> Option<PathBuf> {
    let dir = file.parent()?;
    let program = file.file_name()?.to_string_lossy().into_owned();
    let mut found: Vec<PathBuf> = Vec::new();
    for d in [dir.to_path_buf(), dir.join("share/applications"), dir.parent().map(|p| p.to_path_buf()).unwrap_or_default(), dir.parent().map(|p| p.join("share/applications")).unwrap_or_default()] {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        let mut here: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "desktop")).collect();
        here.sort();
        found.extend(here);
    }
    // The one that starts this program, else the first.
    let starts_it = |p: &PathBuf| {
        let keys = entry_keys(&std::fs::read_to_string(p).unwrap_or_default());
        key(&keys, "Exec").and_then(|e| e.split_whitespace().next()).is_some_and(|e| e.trim_matches('"').rsplit('/').next() == Some(program.as_str()))
    };
    found.iter().find(|p| starts_it(p)).or(found.first()).cloned()
}

/// The best picture named `icon` under `dirs` (a few levels down): an SVG,
/// else the largest PNG.
fn find_icon(dirs: &[PathBuf], icon: &str) -> Option<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    let mut queue: VecDeque<(PathBuf, usize)> = dirs.iter().map(|d| (d.clone(), 0)).collect();
    let mut looked = 0;
    while let Some((d, depth)) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            looked += 1;
            if looked > 20000 {
                return best.map(|b| b.1);
            }
            let p = e.path();
            let Ok(meta) = std::fs::metadata(&p) else { continue };
            if meta.is_dir() {
                if depth < 7 {
                    queue.push_back((p, depth + 1));
                }
                continue;
            }
            let fname = e.file_name().to_string_lossy().into_owned();
            // (A PNG first: not every SVG draws everywhere; the launcher draws any PNG.)
            let score = if fname == format!("{icon}.svg") {
                1
            } else if fname == format!("{icon}.png") || fname == ".DirIcon" {
                meta.len().max(2)
            } else {
                continue;
            };
            if best.as_ref().is_none_or(|(s, _)| score > *s) {
                best = Some((score, p));
            }
        }
    }
    best.map(|b| b.1)
}

/// An AppImage's own entry and picture, unpacked (just those) into `into`.
fn appimage_parts(file: &Path, into: &Path) -> Result<(Option<String>, Option<PathBuf>)> {
    if into.exists() {
        crate::store::remove_tree(into)?;
    }
    std::fs::create_dir_all(into)?;
    let extract = |pattern: &str| {
        let _ = Command::new(file).arg("--appimage-extract").arg(pattern).current_dir(into).stdout(Stdio::null()).stderr(Stdio::null()).status();
    };
    extract("*.desktop");
    let root = into.join("squashfs-root");
    let entry = std::fs::read_dir(&root).ok().and_then(|r| r.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "desktop")));
    let text = entry.and_then(|p| std::fs::read_to_string(p).ok());
    let icon = text.as_deref().and_then(|t| key(&entry_keys(t), "Icon").map(String::from)).unwrap_or_default();
    if !icon.is_empty() {
        for p in [format!("{icon}.svg"), format!("{icon}.png"), format!("usr/share/icons/hicolor/scalable/apps/{icon}.svg"), format!("usr/share/icons/hicolor/512x512/apps/{icon}.png"), format!("usr/share/icons/hicolor/256x256/apps/{icon}.png"), format!("usr/share/icons/hicolor/128x128/apps/{icon}.png")] {
            extract(&p);
        }
    }
    extract(".DirIcon");
    // .DirIcon is often a link to the picture: that one too.
    if let Ok(target) = std::fs::read_link(root.join(".DirIcon")) {
        extract(&target.to_string_lossy());
    }
    let picture = find_icon(&[root.clone()], &icon).filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() > 0));
    Ok((text, picture))
}

/// A desktop entry's Exec value for `nami start <file>`, with the field
/// codes (%U...) the program's own had.
fn exec_line(file: &Path, own: Option<&str>) -> String {
    let path = file.to_string_lossy();
    let quoted = if path.chars().any(|c| " \t\"'\\$`;&|<>()*?#~".contains(c)) {
        // Quoted, its `\ " $` escaped with a backslash, which is itself `\\` in the file.
        format!("\"{}\"", path.replace('\\', "\\\\\\\\").replace('"', "\\\\\"").replace('$', "\\\\$").replace('`', "\\\\`"))
    } else {
        path.into_owned()
    };
    let codes: Vec<&str> = own.unwrap_or_default().split_whitespace().filter(|w| matches!(*w, "%U" | "%u" | "%F" | "%f")).take(1).collect();
    let mut line = format!("{} start {quoted}", nami_program().display());
    for c in codes {
        line.push(' ');
        line.push_str(c);
    }
    line
}

/// `nami add-to-launcher <file>`: an entry for the program in the launcher
/// (~/.local/share/applications), with its own name and picture when its
/// download brings them (an AppImage's inside, a tarball's .desktop file),
/// started through `nami start` (which says what it lacks).
pub fn add_to_launcher(file: &Path) -> Result<PathBuf> {
    let file = std::fs::canonicalize(file).with_context(|| format!("{}", file.display()))?;
    let k = kind(&file)?;
    anyhow::ensure!(matches!(k, Kind::Foreign | Kind::AppImage | Kind::Native | Kind::Script), "{} isn't a program to start ({})", file.display(), describe(k));
    let id = entry_id(&file);
    let (text, icon) = if k == Kind::AppImage {
        let mode = file.metadata()?.permissions().mode();
        if mode & 0o100 == 0 {
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode | 0o100))?;
        }
        appimage_parts(&file, &home().join(".cache/nami/launcher").join(&id))?
    } else {
        let entry = launcher_entry(&file);
        let text = entry.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
        let icon_name = text.as_deref().and_then(|t| key(&entry_keys(t), "Icon").map(String::from));
        let icon = match icon_name.as_deref() {
            Some(i) if i.starts_with('/') && Path::new(i).is_file() => Some(PathBuf::from(i)),
            _ => {
                let name = icon_name.clone().unwrap_or_else(|| file.file_name().unwrap_or_default().to_string_lossy().into_owned());
                let dir = file.parent().unwrap_or(Path::new("/"));
                let mut dirs = vec![dir.to_path_buf()];
                dirs.extend(dir.parent().map(Path::to_path_buf).filter(|p| p != Path::new("/") && p != &home()));
                find_icon(&dirs, &name)
            }
        };
        (text, icon)
    };
    let keys = text.as_deref().map(entry_keys).unwrap_or_default();
    let mut out = String::from("[Desktop Entry]\nType=Application\n");
    let name = key(&keys, "Name").map(String::from).unwrap_or_else(|| display_name(&file));
    out += &format!("Name={name}\n");
    const DROP: &[&str] = &["Type", "Name", "Exec", "TryExec", "Icon", "Path", "DBusActivatable", "Actions", "Hidden", "NoDisplay", "X-AppImage-Version", "X-AppImage-Name", "X-AppImage-Arch"];
    for (k, v) in &keys {
        if !DROP.contains(&k.as_str()) && !k.starts_with("X-AppImage") {
            out += &format!("{k}={v}\n");
        }
    }
    out += &format!("Exec={}\n", exec_line(&file, key(&keys, "Exec")));
    if let Some(icon) = icon {
        let ext = if icon.extension().is_some_and(|e| e == "svg") { "svg" } else { "png" };
        let dest = home().join(".local/share/nami/icons").join(format!("{id}.{ext}"));
        std::fs::create_dir_all(dest.parent().unwrap())?;
        std::fs::copy(&icon, &dest).with_context(|| format!("copying {}", icon.display()))?;
        out += &format!("Icon={}\n", dest.display());
    } else {
        out += "Icon=application-x-executable\n";
    }
    if let Some(dir) = file.parent() {
        out += &format!("Path={}\n", dir.display());
    }
    out += &format!("X-Nami-Program={}\n", file.display());
    let dest = entry_path(&file);
    std::fs::create_dir_all(dest.parent().unwrap())?;
    std::fs::write(&dest, out)?;
    println!("{}", crate::style::out().line(&format!("✓ {name} is in the launcher · {}", dest.display())));
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_as_a_person_would() {
        assert_eq!(display_name(Path::new("/x/Obsidian-1.5.12.AppImage")), "Obsidian");
        assert_eq!(display_name(Path::new("/x/Cool_App-x86_64.AppImage")), "Cool App");
        assert_eq!(display_name(Path::new("/x/fd")), "fd");
        assert_eq!(display_name(Path::new("/x/zed-editor")), "zed-editor");
        assert_eq!(display_name(Path::new("/x/kdenlive-25.08.1-x86_64.AppImage")), "kdenlive");
        assert_eq!(display_name(Path::new("/x/LocalSend-1.17.0-linux-x86-64.AppImage")), "LocalSend");
    }

    #[test]
    fn the_loaders_word_for_a_missing_library() {
        let said = "./game: error while loading shared libraries: libfoo.so.1: cannot open shared object file: No such file or directory\n";
        assert_eq!(loader_lacks(said).as_deref(), Some("libfoo.so.1"));
        assert_eq!(loader_lacks("all fine"), None);
    }

    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "reads this test program as an ELF file")]
    fn what_a_file_is() {
        assert_eq!(kind_of(b"#!/bin/sh\necho hi\n"), Kind::Script);
        assert_eq!(kind_of(b"MZ\x90\x00"), Kind::Windows);
        assert_eq!(kind_of(b"hello"), Kind::Other);
        // nami's own tests are this computer's programs (or ones for another distribution, built elsewhere).
        let me = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        assert!(matches!(kind_of(&me), Kind::Native | Kind::Foreign));
        let mut appimage = me.clone();
        appimage[8..11].copy_from_slice(b"AI\x02");
        assert_eq!(kind_of(&appimage), Kind::AppImage);
        let mut old = me[..64].to_vec();
        old[4] = 1;
        assert_eq!(kind_of(&old), Kind::Bits32);
    }

    #[test]
    fn finds_what_a_program_lacks_as_the_loader_would() {
        // This test program, looked at with only its own loader's folder
        // to look in: glibc's libraries are there, nothing else is.
        let me = std::env::current_exe().unwrap();
        let data = std::fs::read(&me).unwrap();
        let Some(loader) = interpreter(&data) else { return };
        let glibc = Path::new(&loader).parent().unwrap().to_path_buf();
        let lacks = missing_in(&me, &[], Some(&glibc)).unwrap();
        // Nothing it links to past glibc is found without its RUNPATH's folders... unless it has one.
        let l = crate::elf::linking(&data).unwrap();
        if l.runpath.is_none() && l.rpath.is_none() {
            assert!(l.needed.iter().all(|n| glibc.join(n).exists() || lacks.iter().any(|x| &x.soname == n)), "{lacks:?}");
        } else {
            assert!(lacks.is_empty(), "{lacks:?}");
        }
        // A library nothing has.
        let dir = std::env::temp_dir().join(format!("nami-lacks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let none = missing_in(&me, &[dir.clone()], None).unwrap();
        assert!(none.iter().any(|x| x.soname == "libc.so.6") || l.rpath.is_some() || l.runpath.is_some(), "{none:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_launcher_entry_starts_it_through_nami() {
        assert_eq!(exec_line(Path::new("/home/a/Apps/Zed.AppImage"), Some("zed %U")), format!("{} start /home/a/Apps/Zed.AppImage %U", nami_program().display()));
        assert_eq!(exec_line(Path::new("/home/a/My Apps/x"), None), format!("{} start \"/home/a/My Apps/x\"", nami_program().display()));
        let keys = entry_keys("[Desktop Entry]\nName=Discord\nExec=/usr/bin/discord --url -- %u\n[Desktop Action x]\nName=Other\n");
        assert_eq!(key(&keys, "Name"), Some("Discord"));
        assert_eq!(keys.len(), 2);
    }
}
