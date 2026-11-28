//! App menu entries for the apps in a profile.
//!
//! Desktops watch `~/.local/share/applications` (and /usr/local/share/...)
//! for changes, but not a profile's `share/applications`, whose directory is
//! swapped as a whole on every generation switch. So after each switch nami
//! writes each app's .desktop file there, under the same name (so it
//! overrides the profile's copy instead of showing up twice), with absolute
//! paths to the program and icon, and a marker so only nami's own entries
//! are ever replaced or removed.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

const MARKER: &str = "X-Nami-Managed=true";
/// What tray marked them with: recognised too (until 1.0).
const OLD_MARKER: &str = "X-Tray-Managed=true";
const ICON_DIRS: &[&str] = &[
    "share/icons/hicolor/scalable/apps", "share/icons/hicolor/512x512/apps", "share/icons/hicolor/256x256/apps",
    "share/icons/hicolor/128x128/apps", "share/icons/hicolor/96x96/apps", "share/icons/hicolor/64x64/apps",
    "share/icons/hicolor/48x48/apps", "share/pixmaps",
];

/// Where menu entries for this profile go (the computer's under `root`, so
/// a NAMI_ROOT install never touches the real /usr/local).
pub fn entries_dir(system: bool, root: &Path) -> Option<PathBuf> {
    // (A Mac's apps aren't .desktop files: linking them into Applications is for later.)
    if crate::index::MAC {
        return None;
    }
    if system {
        return Some(root.join("usr/local/share/applications"));
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(data.join("applications"))
}

/// Make `dest` hold one entry per app in `tree` (a generation's directory),
/// pointing through `stable` (the profile link that always means "active").
pub fn sync(tree: &Path, stable: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    let mut written = BTreeSet::new();
    if let Ok(entries) = fs::read_dir(tree.join("share/applications")) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".desktop") {
                continue;
            }
            let Ok(text) = fs::read_to_string(e.path()) else { continue };
            let target = dest.join(&name);
            // Never replace an entry you made yourself.
            if let Ok(existing) = fs::read_to_string(&target) {
                if !existing.contains(MARKER) && !existing.contains(OLD_MARKER) {
                    continue;
                }
            }
            crate::store::write_atomic(&target, rewrite(&text, tree, stable).as_bytes())?;
            written.insert(name);
        }
    }
    for e in fs::read_dir(dest)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".desktop") && !written.contains(&name) {
            if fs::read_to_string(e.path()).is_ok_and(|t| t.contains(MARKER) || t.contains(OLD_MARKER)) {
                fs::remove_file(e.path())?;
            }
        }
    }
    // (What these apps open: the person's own entries' too.)
    write_mime_cache(dest)
}

/// A folder of menu entries' `mimeinfo.cache`: which apps open which kinds of
/// file, as update-desktop-database writes it. GLib (Files, Open With, xdg-open
/// through gio) knows an app opens a kind of file only from this, not from the
/// entries themselves: without it, opening any file says no app can.
/// `entries`: each entry's name (its id) and text.
pub fn mime_cache(entries: impl IntoIterator<Item = (String, String)>) -> String {
    let mut kinds: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
    for (id, text) in entries {
        let mut in_main = false;
        let (mut mimes, mut hidden, mut app) = (None, false, false);
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                in_main = line == "[Desktop Entry]";
            } else if in_main {
                match line.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
                    Some(("MimeType", v)) => mimes = Some(v.to_string()),
                    Some(("Hidden", v)) => hidden = v == "true",
                    Some(("Type", v)) => app = v == "Application",
                    _ => {}
                }
            }
        }
        if hidden || !app {
            continue;
        }
        for mime in mimes.iter().flat_map(|m| m.split(';')).map(str::trim).filter(|m| !m.is_empty()) {
            let ids = kinds.entry(mime.to_string()).or_default();
            if !ids.contains(&id) {
                ids.push(id.clone());
            }
        }
    }
    let mut out = String::from("[MIME Cache]\n");
    for (mime, ids) in kinds {
        out += &format!("{mime}={};\n", ids.join(";"));
    }
    out
}

/// Write `dir`'s mimeinfo.cache, from the entries in it.
pub fn write_mime_cache(dir: &Path) -> Result<()> {
    let mut entries: Vec<(String, String)> = Vec::new();
    for e in fs::read_dir(dir)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".desktop") {
            if let Ok(text) = fs::read_to_string(e.path()) {
                entries.push((name, text));
            }
        }
    }
    entries.sort();
    crate::store::write_atomic(&dir.join("mimeinfo.cache"), mime_cache(entries).as_bytes())
}

fn rewrite(text: &str, tree: &Path, stable: &Path) -> String {
    let mut out = String::new();
    let mut in_main = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            if in_main {
                out += &format!("{MARKER}\n");
            }
            in_main = trimmed == "[Desktop Entry]";
        }
        let fixed = if let Some(cmd) = line.strip_prefix("Exec=") {
            format!("Exec={}", absolute_program(cmd, tree, stable))
        } else if let Some(cmd) = line.strip_prefix("TryExec=") {
            format!("TryExec={}", absolute_program(cmd, tree, stable))
        } else if let Some(icon) = line.strip_prefix("Icon=") {
            format!("Icon={}", icon_path(icon, tree, stable).unwrap_or_else(|| icon.to_string()))
        } else {
            line.to_string()
        };
        out += &fixed;
        out.push('\n');
    }
    if in_main {
        out += &format!("{MARKER}\n");
    }
    out
}

/// `firefox --new-window %u` -> `<profile>/bin/firefox --new-window %u`,
/// if the profile has that program.
fn absolute_program(cmd: &str, tree: &Path, stable: &Path) -> String {
    let (prog, rest) = cmd.split_once(' ').map(|(p, r)| (p, format!(" {r}"))).unwrap_or((cmd, String::new()));
    if !prog.contains('/') && tree.join("bin").join(prog).exists() {
        format!("{}{rest}", stable.join("bin").join(prog).display())
    } else {
        cmd.to_string()
    }
}

fn icon_path(icon: &str, tree: &Path, stable: &Path) -> Option<String> {
    if icon.contains('/') {
        return None;
    }
    for dir in ICON_DIRS {
        for ext in ["svg", "png", "xpm"] {
            let rel = format!("{dir}/{icon}.{ext}");
            if tree.join(&rel).exists() {
                return Some(stable.join(rel).display().to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_exec_and_icon_and_marks() {
        let tree = std::env::temp_dir().join(format!("nami-desktop-{}", std::process::id()));
        fs::create_dir_all(tree.join("bin")).unwrap();
        fs::create_dir_all(tree.join("share/icons/hicolor/128x128/apps")).unwrap();
        fs::write(tree.join("bin/firefox"), "").unwrap();
        fs::write(tree.join("share/icons/hicolor/128x128/apps/firefox.png"), "").unwrap();
        let text = "[Desktop Entry]\nName=Firefox\nExec=firefox --name firefox %U\nIcon=firefox\n[Desktop Action new]\nExec=firefox --new-window %U\n";
        let out = rewrite(text, &tree, Path::new("/p"));
        assert_eq!(out, "[Desktop Entry]\nName=Firefox\nExec=/p/bin/firefox --name firefox %U\nIcon=/p/share/icons/hicolor/128x128/apps/firefox.png\nX-Nami-Managed=true\n[Desktop Action new]\nExec=/p/bin/firefox --new-window %U\n");
        fs::remove_dir_all(&tree).unwrap();
    }

    #[test]
    fn lists_what_opens_what() {
        let entries = [
            ("b.desktop", "[Desktop Entry]\nType=Application\nName=B\nMimeType=text/plain;image/png;\n"),
            ("a.desktop", "# a comment\n[Desktop Entry]\nType=Application\nMimeType=text/plain\n[Desktop Action x]\nMimeType=video/mp4;\n"),
            ("gone.desktop", "[Desktop Entry]\nType=Application\nHidden=true\nMimeType=text/plain;\n"),
            ("link.desktop", "[Desktop Entry]\nType=Link\nMimeType=text/plain;\n"),
        ];
        let out = mime_cache(entries.iter().map(|(a, b)| (a.to_string(), b.to_string())));
        assert_eq!(out, "[MIME Cache]\nimage/png=b.desktop;\ntext/plain=b.desktop;a.desktop;\n");
    }
}
