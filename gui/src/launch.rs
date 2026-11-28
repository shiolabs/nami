//! Opening things outside the window: a terminal in a project's folder
//! with its tools (`nami shell`), an editor on it, an app.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A program on PATH.
pub fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|f| f.is_file()))
}

/// Start it and let it go (its exit is collected, so it leaves no zombie).
fn detach(mut cmd: Command) -> Result<(), String> {
    let name = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| format!("can't start {name}: {e}"))?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// The terminal there is, as people know it, and how it runs a command
/// in a folder: `$TERMINAL`, then those namiOS and the Linuxes nami's on come with.
pub fn terminal() -> Option<(&'static str, String)> {
    // (A Mac's: what opens a .command file, Terminal unless they've picked another.)
    if crate::look::MAC {
        return Some(("Terminal", "open".into()));
    }
    if let Some(t) = std::env::var("TERMINAL").ok().filter(|t| which(t).is_some()) {
        return Some(("Terminal", t));
    }
    const KNOWN: [(&str, &str); 11] = [
        ("ptyxis", "Terminal"), ("kgx", "Console"), ("gnome-terminal", "Terminal"), ("konsole", "Konsole"), ("cosmic-term", "Terminal"),
        ("ghostty", "Ghostty"), ("foot", "foot"), ("alacritty", "Alacritty"), ("kitty", "kitty"), ("wezterm", "WezTerm"), ("xterm", "XTerm"),
    ];
    KNOWN.iter().find(|(bin, _)| which(bin).is_some()).map(|(bin, label)| (*label, bin.to_string()))
}

/// `nami shell` in `dir`, in a terminal of its own.
pub fn open_terminal(dir: &Path) -> Result<(), String> {
    if crate::look::MAC {
        return mac_terminal(dir);
    }
    let (_, bin) = terminal().ok_or("No terminal found. Set TERMINAL or install one, such as Ptyxis, Konsole or foot.")?;
    let d = dir.display().to_string();
    let nami = std::env::var("NAMI").unwrap_or_else(|_| "nami".into());
    let mut cmd = Command::new(&bin);
    let name = Path::new(&bin).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match name.as_str() {
        "ptyxis" => cmd.args(["--new-window", "-d", &d, "--", &nami, "shell"]),
        "kgx" | "gnome-terminal" => cmd.args([&format!("--working-directory={d}"), "--", &nami, "shell"]),
        "konsole" => cmd.args(["--workdir", &d, "-e", &nami, "shell"]),
        "ghostty" => cmd.args([&format!("--working-directory={d}"), "-e", &nami, "shell"]),
        "foot" => cmd.args(["-D", &d, &nami, "shell"]),
        "alacritty" => cmd.args(["--working-directory", &d, "-e", &nami, "shell"]),
        "kitty" => cmd.args(["--directory", &d, &nami, "shell"]),
        "wezterm" => cmd.args(["start", "--cwd", &d, "--", &nami, "shell"]),
        _ => cmd.args(["-e", &nami, "shell"]),
    };
    cmd.current_dir(dir);
    detach(cmd)
}

/// A file in Text Editor, at line `line` (1-based); without it, in what opens text files.
pub fn open_text_file(file: &Path, line: Option<usize>) -> Result<(), String> {
    // (A Mac's: the app that opens text, TextEdit unless they've picked another.)
    if crate::look::MAC {
        let mut cmd = Command::new("/usr/bin/open");
        cmd.arg("-t").arg(file);
        return detach(cmd);
    }
    if let Some(bin) = which("gnome-text-editor") {
        let mut cmd = Command::new(bin);
        if let Some(n) = line {
            cmd.arg(format!("+{n}"));
        }
        cmd.arg(file);
        return detach(cmd);
    }
    let mut cmd = Command::new("xdg-open");
    cmd.arg(file);
    detach(cmd)
}

/// The editor there is for a folder, as people know it.
pub fn editor() -> Option<(&'static str, PathBuf)> {
    if crate::look::MAC {
        return mac_editor();
    }
    const KNOWN: [(&str, &str); 7] = [("code", "VS Code"), ("codium", "VSCodium"), ("zeditor", "Zed"), ("zed", "Zed"), ("subl", "Sublime Text"), ("kate", "Kate"), ("gnome-builder", "Builder")];
    KNOWN.into_iter().find_map(|(bin, label)| which(bin).map(|p| (label, p)))
}

/// A Mac's editor: its command on PATH, or the one in its app (in Applications, the computer's
/// or yours), which they needn't have put on PATH.
fn mac_editor() -> Option<(&'static str, PathBuf)> {
    const KNOWN: [(&str, &str, &str, &str); 5] = [
        ("VS Code", "code", "Visual Studio Code.app", "Contents/Resources/app/bin/code"),
        ("Cursor", "cursor", "Cursor.app", "Contents/Resources/app/bin/cursor"),
        ("VSCodium", "codium", "VSCodium.app", "Contents/Resources/app/bin/codium"),
        ("Zed", "zed", "Zed.app", "Contents/MacOS/cli"),
        ("Sublime Text", "subl", "Sublime Text.app", "Contents/SharedSupport/bin/subl"),
    ];
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let folders: Vec<PathBuf> = [Some(PathBuf::from("/Applications")), home.map(|h| h.join("Applications"))].into_iter().flatten().collect();
    KNOWN.into_iter().find_map(|(label, bin, app, cli)| {
        which(bin).or_else(|| folders.iter().map(|f| f.join(app).join(cli)).find(|p| p.is_file())).map(|p| (label, p))
    })
}

/// `nami shell` in `dir`, in a Mac's terminal: a .command file of it, opened as a double-click
/// in Finder would (in Terminal, or what they've picked for those), so nothing asks to control
/// another app.
fn mac_terminal(dir: &Path) -> Result<(), String> {
    use std::hash::{Hash, Hasher};
    let quote = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "'\\''"));
    let nami = which("nami").unwrap_or_else(|| PathBuf::from(crate::nami::program()));
    // (One a folder, made again each time: never a pile of them.)
    let mut h = std::collections::hash_map::DefaultHasher::new();
    dir.hash(&mut h);
    let file = std::env::temp_dir().join(format!("nami-shell-{:016x}.command", h.finish()));
    let text = format!("#!/bin/sh\n# Made by nami's app: `nami shell` in {}\ncd {} && exec {} shell\n", dir.display(), quote(dir), quote(&nami));
    std::fs::write(&file, text).map_err(|e| format!("{}: {e}", file.display()))?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut cmd = Command::new("/usr/bin/open");
    cmd.arg(&file);
    detach(cmd)
}

/// The editor on `dir`, with the project's tools on its PATH (from
/// `nami env`), so its language support finds them. Getting them can
/// download them: run it away from the window.
pub fn open_editor(dir: &Path) -> Result<(), String> {
    let (_, bin) = editor().ok_or("No code editor found. Install one, such as VS Code, Zed or Kate.")?;
    let out = Command::new(std::env::var("NAMI").unwrap_or_else(|_| "nami".into())).args(["env", "--project"]).arg(dir).output().map_err(|e| format!("can't run nami: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    // Found on our PATH, not the project's.
    let mut cmd = Command::new(bin);
    cmd.envs(exports(&String::from_utf8_lossy(&out.stdout))).arg(".").current_dir(dir);
    detach(cmd)
}

/// `export NAME='value'` lines (sh quoting, as `nami env` writes them).
fn exports(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| l.strip_prefix("export ")?.split_once('='))
        .map(|(k, v)| (k.to_string(), v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')).unwrap_or(v).replace("'\\''", "'")))
        .collect()
}

#[cfg(test)]
#[test]
fn reads_exports() {
    let e = exports("export PATH='/nix/a/bin:/usr/bin'\nexport GREETING='it'\\''s'\n");
    assert_eq!(e, vec![("PATH".into(), "/nix/a/bin:/usr/bin".into()), ("GREETING".into(), "it's".into())]);
}

/// Where menu entries are: yours, then the computer's (XDG_DATA_DIRS, which
/// has nami's), each in applications/.
fn entry_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    let dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    // And Flatpak's (Flathub's apps), also when the session started before the first came.
    let flatpak = home.iter().map(|h| h.join("flatpak/exports/share")).chain([PathBuf::from("/var/lib/flatpak/exports/share")]).collect::<Vec<_>>();
    home.into_iter().chain(std::env::split_paths(&dirs)).chain([PathBuf::from("/run/current-system/sw/share")]).chain(flatpak).map(|d| d.join("applications")).collect()
}

/// The menu entry that starts an app: of its entries (`ids`), the one
/// named after it, else one with its name in it, else LibreOffice's start
/// center, else the first; never a hidden one. None: not on the computer.
pub fn app_entry(attr: &str, ids: &[String]) -> Option<PathBuf> {
    if crate::look::MAC {
        return mac_app(attr);
    }
    let short = attr.rsplit('.').next().unwrap_or(attr).to_lowercase();
    let mut ranked: Vec<&String> = ids.iter().collect();
    ranked.sort_by_key(|id| {
        let id = id.to_lowercase();
        if id == format!("{short}.desktop") { 0 } else if id.contains(&short) { 1 } else if id.starts_with("startcenter") { 2 } else { 3 }
    });
    let dirs = entry_dirs();
    ranked.into_iter().find_map(|id| {
        let file = dirs.iter().map(|d| d.join(id)).find(|f| f.is_file())?;
        let text = std::fs::read_to_string(&file).ok()?;
        (!text.lines().any(|l| l.trim() == "NoDisplay=true")).then_some(file)
    })
}

/// A Mac's app with a window that `attr` installed: its copy in Applications (`~/Applications/
/// nami Apps`, or everyone's in /Applications: nami's src/mac_apps.rs), the one Spotlight and the
/// Dock know; else (from before nami put apps there) its .app in the store, found through your
/// profile (or everyone's), where nami links what packages have in Applications/.
fn mac_app(attr: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let leaf = attr.rsplit('.').next().unwrap_or(attr);
    // `.nami-apps` in each: `<name>.app<TAB><store path it was copied from>` lines, and
    // `<TAB>cask:<token>` after for one of Homebrew's casks.
    let copies: Vec<(PathBuf, String, Option<String>)> = [home.join("Applications/nami Apps"), PathBuf::from("/Applications/nami Apps")]
        .iter()
        .flat_map(|f| {
            let list = std::fs::read_to_string(f.join(".nami-apps")).unwrap_or_default();
            list.lines()
                .filter_map(|l| {
                    let mut cols = l.split('\t');
                    let (n, s) = (cols.next()?, cols.next()?);
                    Some((f.join(n), s.to_string(), cols.next().and_then(|c| c.strip_prefix("cask:")).map(String::from)))
                })
                .collect::<Vec<_>>()
        })
        .filter(|(app, _, _)| app.join("Contents").is_dir())
        .collect();
    // A cask's by its token (`cask:iterm2`, or `iterm2` as a file has it), then by package or app name.
    let token = attr.strip_prefix("cask:").unwrap_or(attr);
    let copy = copies.iter().find(|(_, _, cask)| cask.as_deref() == Some(token))
        .or_else(|| copies.iter().find(|(_, src, _)| store_name(Path::new(src)).is_some_and(|n| named(&n, leaf))))
        .or_else(|| copies.iter().find(|(app, _, _)| app.file_stem().is_some_and(|s| letters_of(&s.to_string_lossy()) == letters_of(leaf))));
    if let Some((app, _, _)) = copy {
        return Some(app.clone());
    }
    let folders = [home.join(".local/state/nami/profile/Applications"), PathBuf::from("/nix/var/nami/profiles/default/Applications")];
    let apps: Vec<(PathBuf, PathBuf)> = folders.iter().filter_map(|f| std::fs::read_dir(f).ok()).flatten().flatten().filter_map(|e| {
        let app = e.path();
        (app.extension()? == "app").then_some(())?;
        Some((in_store(&app)?, app))
    }).collect();
    // By its package's name in the store (firefox-150.0), else by the app's own (Firefox.app).
    let by_package = apps.iter().find(|(real, _)| store_name(real).is_some_and(|n| named(&n, leaf)));
    let by_app = || apps.iter().find(|(_, app)| app.file_stem().is_some_and(|s| letters_of(&s.to_string_lossy()) == letters_of(leaf)));
    by_package.or_else(by_app).map(|(real, _)| real.clone())
}

/// Where a linked .app is in the store: the link itself, or (a folder of links, each file's)
/// where its Info.plist is.
fn in_store(app: &Path) -> Option<PathBuf> {
    if let Ok(to) = std::fs::canonicalize(app) {
        if to.starts_with("/nix/store") {
            return Some(to);
        }
    }
    let plist = std::fs::read_link(app.join("Contents/Info.plist")).ok()?;
    plist.parent()?.parent().map(Path::to_path_buf).filter(|p| p.starts_with("/nix/store"))
}

/// A store path's package: `firefox-150.0` of /nix/store/<hash>-firefox-150.0/Applications/...
fn store_name(path: &Path) -> Option<String> {
    let dir = path.strip_prefix("/nix/store").ok()?.components().next()?.as_os_str().to_string_lossy().into_owned();
    dir.split_once('-').map(|(_, name)| name.to_string())
}

/// Whether a store path's name (`firefox-150.0`) is of the package `attr` names (`firefox`, not
/// `firefox-esr`).
fn named(name: &str, attr: &str) -> bool {
    let (name, attr) = (name.to_lowercase(), attr.to_lowercase());
    name == attr || name.strip_prefix(&attr).and_then(|r| r.strip_prefix('-')).is_some_and(|v| v.starts_with(|c: char| c.is_ascii_digit()))
}

/// A name's letters and digits, in lowercase: `Visual Studio Code` is `visualstudiocode`.
fn letters_of(name: &str) -> String {
    name.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

#[cfg(test)]
#[test]
fn a_macs_app_is_found_by_its_package() {
    assert!(named("firefox-150.0", "firefox"));
    assert!(named("iterm2-3.5.14", "iterm2"));
    assert!(!named("firefox-esr-128.4", "firefox"));
    assert!(!named("firefoxpwa-2.0", "firefox"));
    assert_eq!(store_name(Path::new("/nix/store/0123456789abcdfghijklmnpqrsvwxyz-firefox-150.0/Applications/Firefox.app")).as_deref(), Some("firefox-150.0"));
    assert_eq!(letters_of("Visual Studio Code"), "visualstudiocode");
}

/// Start an app from its menu entry, as the desktop would (`gio launch`),
/// else by the entry's Exec line.
pub fn open_app(entry: &Path) -> Result<(), String> {
    // (A Mac's: its .app, as Finder opens it.)
    if crate::look::MAC {
        let mut cmd = Command::new("/usr/bin/open");
        cmd.arg(entry);
        return detach(cmd);
    }
    if let Some(gio) = which("gio") {
        let mut cmd = Command::new(gio);
        cmd.arg("launch").arg(entry);
        return detach(cmd);
    }
    let text = std::fs::read_to_string(entry).map_err(|e| format!("{}: {e}", entry.display()))?;
    let exec = text.lines().skip_while(|l| l.trim() != "[Desktop Entry]").find_map(|l| l.strip_prefix("Exec=")).ok_or_else(|| format!("{} has no Exec line", entry.display()))?;
    let mut cmd = Command::new("sh");
    cmd.args(["-c", &without_field_codes(exec)]);
    detach(cmd)
}

/// An Exec line without its field codes (%f, %U...), which are for files.
fn without_field_codes(exec: &str) -> String {
    exec.split_whitespace().filter(|w| !(w.len() == 2 && w.starts_with('%'))).collect::<Vec<_>>().join(" ").replace("%%", "%")
}

#[cfg(test)]
#[test]
fn exec_lines_lose_their_field_codes() {
    assert_eq!(without_field_codes("gimp-2.10 %U"), "gimp-2.10");
    assert_eq!(without_field_codes("vlc --started-from-file %U --x 100%%"), "vlc --started-from-file --x 100%");
}
